use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rucksack_core::files::append_line;
use rucksack_core::state::{ActivePolicy, SessionPhase, SessionState};
use rucksack_core::{AgentKind, AppPaths};
use serde_json::{json, Map, Value};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

const MAX_HOOK_INPUT: u64 = 1024 * 1024;
const MAX_HOOK_EVENT_NAME: usize = 64;
const MAX_PROVIDER_SESSION_ID: usize = 256;
const MAX_HOOK_PROMPT: usize = 16 * 1024;
const HOOK_EVENT_KEYS: &[&str] = &[
    "hook_event_name",
    "hookEventName",
    "event_name",
    "eventName",
    "event",
];
const HOOK_CWD_KEYS: &[&str] = &[
    "cwd",
    "working_directory",
    "workingDirectory",
    "workspace_path",
    "workspacePath",
    "workspace_root",
    "workspaceRoot",
];
const HOOK_WORKSPACE_ROOTS_KEYS: &[&str] = &["workspace_roots", "workspaceRoots"];
const HOOK_SESSION_ID_KEYS: &[&str] = &[
    "session_id",
    "sessionId",
    "thread_id",
    "threadId",
    "conversation_id",
    "conversationId",
];
const HOOK_PROMPT_KEYS: &[&str] = &["prompt"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct HookPayload {
    event: String,
    project_dir: PathBuf,
    provider_session_id: String,
    prompt: Option<String>,
}

pub(crate) fn commute_mode_confirmation_command(agent: AgentKind, token: &str) -> String {
    let command = match agent {
        AgentKind::Codex => "$commute-mode",
        AgentKind::Claude | AgentKind::Cursor => "/commute-mode",
    };
    format!("{command} {token}")
}

pub fn run(agent: AgentKind, paths: &AppPaths) -> Result<()> {
    let mut input = String::new();
    io::stdin()
        .take(MAX_HOOK_INPUT + 1)
        .read_to_string(&mut input)
        .context("Could not read hook input")?;
    if input.len() as u64 > MAX_HOOK_INPUT {
        anyhow::bail!("hook input exceeded 1 MiB");
    }

    let payload = serde_json::from_str::<Value>(&input).unwrap_or(Value::Null);
    let Some(hook) = parse_hook_payload(&payload) else {
        return Ok(());
    };

    if let Some(output) = handle_hook(agent, &hook, paths, Utc::now())? {
        println!("{}", serde_json::to_string(&output)?);
    }
    Ok(())
}

fn handle_hook(
    agent: AgentKind,
    hook: &HookPayload,
    paths: &AppPaths,
    now: DateTime<Utc>,
) -> Result<Option<Value>> {
    if let Some(session) = SessionState::load(paths)? {
        let session = if hook_matches_session(agent, hook, &session) {
            session
        } else if let Some(session) =
            bind_session_for_confirmation(agent, hook, &session, paths, now)?
        {
            session
        } else {
            return Ok(None);
        };
        record_event(&session, &hook.event, paths)?;
        return Ok(active_policy_for_session(&session, paths, now)?
            .and_then(|policy| policy_context_output(agent, &hook.event, &policy.policy)));
    }

    Ok(bind_policy_for_confirmation(agent, hook, paths, now)?
        .and_then(|policy| policy_context_output(agent, &hook.event, &policy.policy)))
}

fn policy_context_output(agent: AgentKind, event: &str, policy: &str) -> Option<Value> {
    match agent {
        AgentKind::Codex | AgentKind::Claude
            if matches!(event, "SessionStart" | "UserPromptSubmit") =>
        {
            Some(json!({
                "hookSpecificOutput": {
                    "hookEventName": event,
                    "additionalContext": policy
                }
            }))
        }
        // Cursor's always-applied project rule and `/commute-mode` command are authoritative.
        // Cursor hooks record lifecycle state but do not claim per-prompt context delivery.
        _ => None,
    }
}

fn record_event(session: &SessionState, event: &str, paths: &AppPaths) -> Result<()> {
    let now = Utc::now();
    let Some(_) = SessionState::update(paths, session.id, |current| {
        Ok(apply_lifecycle_event(current, event, now))
    })?
    else {
        return Ok(());
    };
    let timestamp = now.to_rfc3339();
    let line = format!("{timestamp} agent={} event={event}", session.agent);
    append_line(&paths.log_dir.join("hooks.log"), &line)?;
    Ok(())
}

fn bind_policy_for_confirmation(
    agent: AgentKind,
    hook: &HookPayload,
    paths: &AppPaths,
    now: DateTime<Utc>,
) -> Result<Option<ActivePolicy>> {
    let Some(policy) = ActivePolicy::load(paths)? else {
        return Ok(None);
    };
    if policy_matches_hook(agent, hook, &policy, now) {
        return Ok(Some(policy));
    }
    if !matches!(
        hook.event.as_str(),
        "UserPromptSubmit" | "beforeSubmitPrompt"
    ) {
        return Ok(None);
    }
    if !policy_accepts_confirmation(agent, hook, &policy, now) {
        return Ok(None);
    }
    let Some(confirmation_token) = policy.confirmation_token.as_deref() else {
        return Ok(None);
    };
    if !matches_confirmation_prompt(agent, hook, confirmation_token) {
        return Ok(None);
    }
    let Some(policy) = ActivePolicy::bind_provider_session(
        paths,
        policy.session_id,
        &hook.provider_session_id,
        confirmation_token,
    )?
    else {
        return Ok(None);
    };
    if !policy_matches_hook(agent, hook, &policy, now) {
        return Ok(None);
    }
    Ok(Some(policy))
}

fn bind_session_for_confirmation(
    agent: AgentKind,
    hook: &HookPayload,
    session: &SessionState,
    paths: &AppPaths,
    now: DateTime<Utc>,
) -> Result<Option<SessionState>> {
    if !matches!(
        hook.event.as_str(),
        "UserPromptSubmit" | "beforeSubmitPrompt"
    ) || !session_accepts_confirmation(agent, hook, session)
    {
        return Ok(None);
    }
    let Some(policy) = ActivePolicy::load(paths)? else {
        return Ok(None);
    };
    if !policy_matches_session_identity(&policy, session, now) {
        return Ok(None);
    }
    let Some(confirmation_token) = policy.confirmation_token.as_deref() else {
        return Ok(None);
    };
    if !matches_confirmation_prompt(agent, hook, confirmation_token) {
        return Ok(None);
    }
    let Some(policy) = ActivePolicy::bind_provider_session(
        paths,
        policy.session_id,
        &hook.provider_session_id,
        confirmation_token,
    )?
    else {
        return Ok(None);
    };
    if !policy_matches_hook(agent, hook, &policy, now) {
        return Ok(None);
    }
    let Some(session) =
        SessionState::bind_provider_session(paths, session.id, &hook.provider_session_id)?
    else {
        return Ok(None);
    };
    if !hook_matches_session(agent, hook, &session) {
        return Ok(None);
    }
    Ok(Some(session))
}

fn active_policy_for_session(
    session: &SessionState,
    paths: &AppPaths,
    now: DateTime<Utc>,
) -> Result<Option<ActivePolicy>> {
    let Some(policy) = ActivePolicy::load(paths)? else {
        return Ok(None);
    };
    if !policy_matches_session_identity(&policy, session, now)
        || policy.provider_session_id != session.provider_session_id
        || session.provider_session_id.is_none()
    {
        return Ok(None);
    }
    Ok(Some(policy))
}

fn policy_matches_session_identity(
    policy: &ActivePolicy,
    session: &SessionState,
    now: DateTime<Utc>,
) -> bool {
    policy.is_active(now)
        && policy.session_id == session.id
        && policy.agent == session.agent
        && same_project(&policy.project_dir, &session.project_dir)
}

fn policy_accepts_confirmation(
    agent: AgentKind,
    hook: &HookPayload,
    policy: &ActivePolicy,
    now: DateTime<Utc>,
) -> bool {
    policy.is_active(now)
        && policy.agent == agent
        && hook_targets_project(hook, &policy.project_dir)
}

fn policy_matches_hook(
    agent: AgentKind,
    hook: &HookPayload,
    policy: &ActivePolicy,
    now: DateTime<Utc>,
) -> bool {
    policy_accepts_confirmation(agent, hook, policy, now)
        && policy.provider_session_id.as_deref() == Some(hook.provider_session_id.as_str())
}

fn matches_confirmation_prompt(agent: AgentKind, hook: &HookPayload, token: &str) -> bool {
    let expected = commute_mode_confirmation_command(agent, token);
    hook.prompt.as_deref() == Some(expected.as_str())
}

fn hook_matches_session(agent: AgentKind, hook: &HookPayload, session: &SessionState) -> bool {
    session.agent == agent
        && !is_terminal_phase(session.phase)
        && session.provider_session_id.as_deref() == Some(hook.provider_session_id.as_str())
        && hook_targets_project(hook, &session.project_dir)
}

fn session_accepts_confirmation(
    agent: AgentKind,
    hook: &HookPayload,
    session: &SessionState,
) -> bool {
    session.agent == agent
        && !is_terminal_phase(session.phase)
        && session.provider_session_id.is_none()
        && hook_targets_project(hook, &session.project_dir)
}

fn hook_targets_project(hook: &HookPayload, project_dir: &Path) -> bool {
    let Ok(hook_dir) = hook.project_dir.canonicalize() else {
        return false;
    };
    let Ok(project_dir) = project_dir.canonicalize() else {
        return false;
    };
    match (repository_root(&hook_dir), repository_root(&project_dir)) {
        (Some(hook_root), Some(project_root)) => hook_root == project_root,
        _ => hook_dir == project_dir,
    }
}

fn repository_root(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|candidate| candidate.join(".git").exists())
        .map(Path::to_path_buf)
}

fn same_project(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn apply_lifecycle_event(
    mut session: SessionState,
    event: &str,
    now: DateTime<Utc>,
) -> SessionState {
    session.last_event = Some(event.to_owned());
    let was_offline = session.phase == SessionPhase::TemporarilyOffline;
    let reducer_phase = if was_offline {
        session.phase_before_offline.unwrap_or(SessionPhase::Active)
    } else {
        session.phase
    };
    let update = reduce_lifecycle_event(
        reducer_phase,
        event,
        now,
        session.idle_grace_started_at,
        session.completed_at,
    );
    session.idle_grace_started_at = update.idle_grace_started_at;
    session.completed_at = update.completed_at;
    if was_offline && !is_terminal_phase(update.phase) {
        session.phase_before_offline = Some(update.phase);
    } else {
        session.phase = update.phase;
        if update.phase != SessionPhase::TemporarilyOffline {
            session.phase_before_offline = None;
        }
    }
    session
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LifecycleUpdate {
    phase: SessionPhase,
    idle_grace_started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
}

fn reduce_lifecycle_event(
    current_phase: SessionPhase,
    event: &str,
    now: DateTime<Utc>,
    idle_grace_started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
) -> LifecycleUpdate {
    if is_terminal_phase(current_phase) {
        return LifecycleUpdate {
            phase: current_phase,
            idle_grace_started_at,
            completed_at,
        };
    }

    match event.to_ascii_lowercase().as_str() {
        "sessionend" => LifecycleUpdate {
            phase: SessionPhase::Completed,
            idle_grace_started_at: None,
            completed_at: Some(now),
        },
        "stop" | "afteragentresponse" => LifecycleUpdate {
            phase: SessionPhase::IdleGrace,
            idle_grace_started_at: Some(now),
            completed_at: None,
        },
        "permissionrequest" => active_update(SessionPhase::WaitingForApproval),
        "notification" => active_update(SessionPhase::WaitingForInput),
        "userpromptsubmit"
        | "beforesubmitprompt"
        | "sessionstart"
        | "posttooluse"
        | "beforetooluse"
        | "pretooluse"
        | "aftershellexecution"
        | "afterfileedit" => active_update(SessionPhase::Active),
        _ => LifecycleUpdate {
            phase: current_phase,
            idle_grace_started_at,
            completed_at,
        },
    }
}

fn active_update(phase: SessionPhase) -> LifecycleUpdate {
    LifecycleUpdate {
        phase,
        idle_grace_started_at: None,
        completed_at: None,
    }
}

fn is_terminal_phase(phase: SessionPhase) -> bool {
    matches!(
        phase,
        SessionPhase::Completed
            | SessionPhase::Releasing
            | SessionPhase::Released
            | SessionPhase::Failed
    )
}

fn parse_hook_payload(value: &Value) -> Option<HookPayload> {
    let object = value.as_object()?;
    let event = unique_string_field(object, HOOK_EVENT_KEYS, safe_event_name)??;
    let project_dir = find_hook_project_dir(object)??;
    let provider_session_id =
        unique_string_field(object, HOOK_SESSION_ID_KEYS, safe_provider_session_id)??;
    let prompt = unique_string_field(object, HOOK_PROMPT_KEYS, safe_prompt)?;
    Some(HookPayload {
        event,
        project_dir,
        provider_session_id,
        prompt,
    })
}

#[cfg(test)]
fn find_event_name(value: &Value) -> Option<String> {
    unique_string_field(value.as_object()?, HOOK_EVENT_KEYS, safe_event_name).flatten()
}

fn unique_string_field(
    object: &Map<String, Value>,
    keys: &[&str],
    validate: impl Fn(&str) -> Option<String>,
) -> Option<Option<String>> {
    let mut value = None;
    for key in keys {
        let Some(candidate) = object.get(*key) else {
            continue;
        };
        let candidate = candidate.as_str().and_then(&validate)?;
        if value
            .as_ref()
            .is_some_and(|existing| existing != &candidate)
        {
            return None;
        }
        value = Some(candidate);
    }
    Some(value)
}

fn find_hook_project_dir(object: &Map<String, Value>) -> Option<Option<PathBuf>> {
    let direct = unique_path_field(object, HOOK_CWD_KEYS)?;
    let roots = workspace_roots(object)?;
    let Some(direct) = direct else {
        return match roots {
            Some(roots) if roots.len() == 1 => Some(roots.into_iter().next()),
            Some(_) => None,
            None => Some(None),
        };
    };
    if roots.is_some_and(|roots| !roots.iter().any(|root| direct.starts_with(root.as_path()))) {
        return None;
    }
    Some(Some(direct))
}

fn workspace_roots(object: &Map<String, Value>) -> Option<Option<Vec<PathBuf>>> {
    let mut roots = None;
    for key in HOOK_WORKSPACE_ROOTS_KEYS {
        let Some(candidate) = object.get(*key) else {
            continue;
        };
        let candidate = candidate.as_array()?;
        let candidate = candidate
            .iter()
            .map(|value| value.as_str().and_then(safe_absolute_path))
            .collect::<Option<Vec<_>>>()?;
        if roots
            .as_ref()
            .is_some_and(|existing| existing != &candidate)
        {
            return None;
        }
        roots = Some(candidate);
    }
    Some(roots)
}

fn unique_path_field(object: &Map<String, Value>, keys: &[&str]) -> Option<Option<PathBuf>> {
    let mut path = None;
    for key in keys {
        let Some(candidate) = object.get(*key) else {
            continue;
        };
        let candidate = candidate.as_str().and_then(safe_absolute_path)?;
        if path.as_ref().is_some_and(|existing| existing != &candidate) {
            return None;
        }
        path = Some(candidate);
    }
    Some(path)
}

fn safe_absolute_path(value: &str) -> Option<PathBuf> {
    if value.is_empty() {
        return None;
    }
    let path = PathBuf::from(value);
    path.is_absolute().then_some(path)
}

fn safe_event_name(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > MAX_HOOK_EVENT_NAME
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    Some(value.to_owned())
}

fn safe_provider_session_id(value: &str) -> Option<String> {
    if value.is_empty()
        || value.len() > MAX_PROVIDER_SESSION_ID
        || value.chars().any(char::is_control)
    {
        return None;
    }
    Some(value.to_owned())
}

fn safe_prompt(value: &str) -> Option<String> {
    (value.len() <= MAX_HOOK_PROMPT).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rucksack_core::policy::Focus;
    use rucksack_core::state::SessionStateWriteConflict;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;
    use uuid::Uuid;

    const TEST_CONFIRMATION_TOKEN: &str = "rucksack-test-0123456789abcdef";

    fn test_paths(root: &Path) -> AppPaths {
        let data_dir = root.join("data");
        let log_dir = root.join("logs");
        AppPaths {
            home: root.to_path_buf(),
            data_dir: data_dir.clone(),
            config_file: data_dir.join("config.toml"),
            session_file: data_dir.join("session.json"),
            report_file: data_dir.join("last-report.json"),
            policy_file: data_dir.join("active-policy.json"),
            adapter_manifest_file: data_dir.join("adapters.json"),
            log_dir: log_dir.clone(),
            daemon_log: log_dir.join("daemon.log"),
            codex_hooks: root.join(".codex/hooks.json"),
            codex_skill: root.join(".agents/skills/commute-mode/SKILL.md"),
            claude_settings: root.join(".claude/settings.json"),
            claude_skill: root.join(".claude/skills/commute-mode/SKILL.md"),
            cursor_hooks: root.join(".cursor/hooks.json"),
        }
    }

    fn sample_session() -> SessionState {
        let now = Utc::now();
        SessionState {
            version: 1,
            revision: 0,
            id: Uuid::new_v4(),
            lease_id: Uuid::new_v4(),
            owner_uid: 501,
            agent: AgentKind::Codex,
            project_dir: PathBuf::from("/workspace/project"),
            provider_session_id: Some("provider-session-1".to_owned()),
            focus: Focus::Continue,
            phase: SessionPhase::Active,
            started_at: now,
            expires_at: now + chrono::Duration::hours(1),
            last_heartbeat_at: None,
            daemon_pid: None,
            expected_hotspot_ssid: None,
            observed_hotspot_ssid: None,
            commute_route_interface: None,
            commute_route_gateway: None,
            route_interface: None,
            start_battery_percent: Some(80),
            battery_percent: Some(80),
            network_reachable: Some(true),
            network_outage_started_at: None,
            phase_before_offline: None,
            idle_grace_started_at: None,
            completed_at: None,
            ended_at: None,
            mobile_data_start: None,
            mobile_data_end: None,
            mobile_data_finalized: false,
            mobile_data_error: None,
            previous_sleep_disabled: Some(0),
            remote_owned_by_rucksack: false,
            remote_pid: None,
            remote_confirmed_by_user: true,
            last_event: None,
            release_reason: None,
        }
    }

    fn sample_policy(session: &SessionState) -> ActivePolicy {
        ActivePolicy {
            version: 1,
            session_id: session.id,
            agent: session.agent,
            focus: session.focus,
            project_dir: session.project_dir.clone(),
            provider_session_id: session.provider_session_id.clone(),
            confirmation_token: session
                .provider_session_id
                .is_none()
                .then(|| TEST_CONFIRMATION_TOKEN.to_owned()),
            cleanup_pending: false,
            activated_at: session.started_at,
            expires_at: session.expires_at,
            policy: "lease nonce rucksack-test-42".to_owned(),
        }
    }

    fn project_dir(root: &Path, name: &str) -> PathBuf {
        let project = root.join(name);
        fs::create_dir_all(&project).unwrap();
        project.canonicalize().unwrap()
    }

    fn hook(event: &str, project_dir: &Path, provider_session_id: &str) -> HookPayload {
        HookPayload {
            event: event.to_owned(),
            project_dir: project_dir.to_path_buf(),
            provider_session_id: provider_session_id.to_owned(),
            prompt: None,
        }
    }

    fn confirmation_hook(
        agent: AgentKind,
        project_dir: &Path,
        provider_session_id: &str,
        token: &str,
    ) -> HookPayload {
        HookPayload {
            event: match agent {
                AgentKind::Cursor => "beforeSubmitPrompt".to_owned(),
                AgentKind::Codex | AgentKind::Claude => "UserPromptSubmit".to_owned(),
            },
            project_dir: project_dir.to_path_buf(),
            provider_session_id: provider_session_id.to_owned(),
            prompt: Some(commute_mode_confirmation_command(agent, token)),
        }
    }

    #[test]
    fn stop_event_enters_idle_grace() {
        let now = DateTime::parse_from_rfc3339("2026-07-24T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let update = reduce_lifecycle_event(SessionPhase::Active, "Stop", now, None, None);
        assert_eq!(update.phase, SessionPhase::IdleGrace);
        assert_eq!(update.idle_grace_started_at, Some(now));
        assert_eq!(update.completed_at, None);
    }

    #[test]
    fn user_activity_clears_idle_grace() {
        let now = DateTime::parse_from_rfc3339("2026-07-24T10:01:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let idle_started = now - chrono::Duration::minutes(1);
        let update = reduce_lifecycle_event(
            SessionPhase::IdleGrace,
            "UserPromptSubmit",
            now,
            Some(idle_started),
            None,
        );
        assert_eq!(update.phase, SessionPhase::Active);
        assert_eq!(update.idle_grace_started_at, None);
    }

    #[test]
    fn session_end_marks_work_completed() {
        let now = DateTime::parse_from_rfc3339("2026-07-24T10:02:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let update = reduce_lifecycle_event(SessionPhase::Active, "SessionEnd", now, None, None);
        assert_eq!(update.phase, SessionPhase::Completed);
        assert_eq!(update.completed_at, Some(now));
    }

    #[test]
    fn event_name_rejects_log_control_characters_and_oversized_values() {
        assert_eq!(
            find_event_name(&json!({"hook_event_name": "Stop\nforged=true"})),
            None
        );
        assert_eq!(
            find_event_name(&json!({"hook_event_name": "x".repeat(65)})),
            None
        );
        assert_eq!(
            find_event_name(&json!({"hook_event_name": "PostToolUse"})),
            Some("PostToolUse".to_owned())
        );
    }

    #[test]
    fn session_end_survives_a_stale_daemon_heartbeat_write() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let mut initial = sample_session();
        initial.save(&paths).unwrap();
        let mut daemon_snapshot = SessionState::load(&paths).unwrap().unwrap();

        record_event(&initial, "SessionEnd", &paths).unwrap();
        daemon_snapshot.last_heartbeat_at = Some(Utc::now());
        let error = daemon_snapshot.save(&paths).unwrap_err();

        assert!(error.downcast_ref::<SessionStateWriteConflict>().is_some());
        let persisted = SessionState::load(&paths).unwrap().unwrap();
        assert_eq!(persisted.phase, SessionPhase::Completed);
        assert_eq!(persisted.last_event.as_deref(), Some("SessionEnd"));
        assert!(persisted.last_heartbeat_at.is_none());
    }

    #[test]
    fn uncorrelated_hook_does_not_rewrite_the_active_session() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let mut initial = sample_session();
        initial.project_dir = project_dir(directory.path(), "project");
        initial.save(&paths).unwrap();
        let revision = initial.revision;
        let other_project = project_dir(directory.path(), "other-project");

        let output = handle_hook(
            AgentKind::Codex,
            &hook("SessionEnd", &other_project, "provider-session-1"),
            &paths,
            Utc::now(),
        )
        .unwrap();

        assert!(output.is_none());
        let persisted = SessionState::load(&paths).unwrap().unwrap();
        assert_eq!(persisted.revision, revision);
        assert_eq!(persisted.phase, SessionPhase::Active);
        assert!(persisted.last_event.is_none());
    }

    #[test]
    fn nested_project_hook_does_not_rewrite_the_active_session() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let project = project_dir(directory.path(), "project");
        fs::create_dir_all(project.join(".git")).unwrap();
        let nested_project = project_dir(&project, "nested-project");
        fs::create_dir_all(nested_project.join(".git")).unwrap();
        let mut initial = sample_session();
        initial.project_dir = project;
        initial.save(&paths).unwrap();
        let revision = initial.revision;

        let output = handle_hook(
            AgentKind::Codex,
            &hook("SessionEnd", &nested_project, "provider-session-1"),
            &paths,
            Utc::now(),
        )
        .unwrap();

        assert!(output.is_none());
        let persisted = SessionState::load(&paths).unwrap().unwrap();
        assert_eq!(persisted.revision, revision);
        assert_eq!(persisted.phase, SessionPhase::Active);
    }

    #[test]
    fn same_repository_subdirectory_hook_updates_lifecycle() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let project = project_dir(directory.path(), "project");
        fs::create_dir_all(project.join(".git")).unwrap();
        let subdirectory = project_dir(&project, "src");
        let mut initial = sample_session();
        initial.project_dir = project;
        initial.save(&paths).unwrap();

        let output = handle_hook(
            AgentKind::Codex,
            &hook("Stop", &subdirectory, "provider-session-1"),
            &paths,
            Utc::now(),
        )
        .unwrap();

        assert!(output.is_none());
        let persisted = SessionState::load(&paths).unwrap().unwrap();
        assert_eq!(persisted.phase, SessionPhase::IdleGrace);
        assert_eq!(persisted.last_event.as_deref(), Some("Stop"));
    }

    #[test]
    fn wrong_provider_session_cannot_rewrite_matching_project_lifecycle() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let project = project_dir(directory.path(), "project");
        let mut initial = sample_session();
        initial.project_dir = project.clone();
        initial.save(&paths).unwrap();
        let revision = initial.revision;

        let output = handle_hook(
            AgentKind::Codex,
            &hook("SessionEnd", &project, "another-provider-session"),
            &paths,
            Utc::now(),
        )
        .unwrap();

        assert!(output.is_none());
        let persisted = SessionState::load(&paths).unwrap().unwrap();
        assert_eq!(persisted.revision, revision);
        assert_eq!(persisted.phase, SessionPhase::Active);
        assert!(persisted.last_event.is_none());
    }

    #[test]
    fn competing_provider_sessions_cannot_take_over_the_same_project() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let project = project_dir(directory.path(), "project");
        let mut session = sample_session();
        session.project_dir = project.clone();
        session.provider_session_id = None;
        sample_policy(&session).save(&paths).unwrap();

        let first = handle_hook(
            AgentKind::Codex,
            &confirmation_hook(
                AgentKind::Codex,
                &project,
                "provider-session-a",
                TEST_CONFIRMATION_TOKEN,
            ),
            &paths,
            Utc::now(),
        )
        .unwrap();
        let competing_policy = handle_hook(
            AgentKind::Codex,
            &confirmation_hook(
                AgentKind::Codex,
                &project,
                "provider-session-b",
                TEST_CONFIRMATION_TOKEN,
            ),
            &paths,
            Utc::now(),
        )
        .unwrap();

        assert!(first.is_some());
        assert!(competing_policy.is_none());
        assert!(SessionState::load(&paths).unwrap().is_none());
        assert_eq!(
            ActivePolicy::load(&paths)
                .unwrap()
                .unwrap()
                .provider_session_id
                .as_deref(),
            Some("provider-session-a")
        );
        assert_eq!(
            ActivePolicy::load(&paths)
                .unwrap()
                .unwrap()
                .confirmation_token,
            None
        );

        session.provider_session_id = Some("provider-session-a".to_owned());
        session.save(&paths).unwrap();
        let revision = session.revision;

        let competing_context = handle_hook(
            AgentKind::Codex,
            &hook("UserPromptSubmit", &project, "provider-session-b"),
            &paths,
            Utc::now(),
        )
        .unwrap();
        let competing_lifecycle = handle_hook(
            AgentKind::Codex,
            &hook("SessionEnd", &project, "provider-session-b"),
            &paths,
            Utc::now(),
        )
        .unwrap();

        assert!(competing_context.is_none());
        assert!(competing_lifecycle.is_none());
        let persisted_session = SessionState::load(&paths).unwrap().unwrap();
        assert_eq!(persisted_session.revision, revision);
        assert_eq!(persisted_session.phase, SessionPhase::Active);
        assert!(persisted_session.last_event.is_none());
        assert_eq!(
            persisted_session.provider_session_id.as_deref(),
            Some("provider-session-a")
        );
        assert_eq!(
            ActivePolicy::load(&paths)
                .unwrap()
                .unwrap()
                .provider_session_id
                .as_deref(),
            Some("provider-session-a")
        );
    }

    #[test]
    fn policy_context_requires_a_correlated_project_and_provider_session() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let project = project_dir(directory.path(), "project");
        let other_project = project_dir(directory.path(), "other-project");
        let mut session = sample_session();
        session.project_dir = project.clone();
        session.save(&paths).unwrap();
        sample_policy(&session).save(&paths).unwrap();

        let wrong_project = handle_hook(
            AgentKind::Codex,
            &hook("UserPromptSubmit", &other_project, "provider-session-1"),
            &paths,
            Utc::now(),
        )
        .unwrap();
        let wrong_session = handle_hook(
            AgentKind::Codex,
            &hook("UserPromptSubmit", &project, "another-provider-session"),
            &paths,
            Utc::now(),
        )
        .unwrap();
        let matching = handle_hook(
            AgentKind::Codex,
            &hook("UserPromptSubmit", &project, "provider-session-1"),
            &paths,
            Utc::now(),
        )
        .unwrap();

        assert!(wrong_project.is_none());
        assert!(wrong_session.is_none());
        assert_eq!(
            matching,
            Some(json!({
                "hookSpecificOutput": {
                    "hookEventName": "UserPromptSubmit",
                    "additionalContext": "lease nonce rucksack-test-42"
                }
            }))
        );
    }

    #[test]
    fn confirmation_prompt_requires_the_exact_command_and_token() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let project = project_dir(directory.path(), "project");
        let other_project = project_dir(directory.path(), "other-project");
        let mut session = sample_session();
        session.project_dir = project.clone();
        session.provider_session_id = None;
        let policy = sample_policy(&session);
        policy.save(&paths).unwrap();

        let unrelated = handle_hook(
            AgentKind::Codex,
            &confirmation_hook(
                AgentKind::Codex,
                &other_project,
                "wrong-provider-session",
                TEST_CONFIRMATION_TOKEN,
            ),
            &paths,
            Utc::now(),
        )
        .unwrap();
        let missing_prompt = handle_hook(
            AgentKind::Codex,
            &hook("UserPromptSubmit", &project, "provider-session-1"),
            &paths,
            Utc::now(),
        )
        .unwrap();
        let wrong_token = handle_hook(
            AgentKind::Codex,
            &confirmation_hook(
                AgentKind::Codex,
                &project,
                "provider-session-1",
                "rucksack-wrong-token",
            ),
            &paths,
            Utc::now(),
        )
        .unwrap();
        let matching = handle_hook(
            AgentKind::Codex,
            &confirmation_hook(
                AgentKind::Codex,
                &project,
                "provider-session-1",
                TEST_CONFIRMATION_TOKEN,
            ),
            &paths,
            Utc::now(),
        )
        .unwrap();

        assert!(unrelated.is_none());
        assert!(missing_prompt.is_none());
        assert!(wrong_token.is_none());
        assert!(matching.is_some());
        assert_eq!(
            ActivePolicy::load(&paths)
                .unwrap()
                .unwrap()
                .provider_session_id
                .as_deref(),
            Some("provider-session-1")
        );
        assert_eq!(
            ActivePolicy::load(&paths)
                .unwrap()
                .unwrap()
                .confirmation_token,
            None
        );
    }

    #[test]
    fn unverified_session_stays_inert_until_a_token_bearing_prompt_binds_it() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let project = project_dir(directory.path(), "project");
        let mut session = sample_session();
        session.project_dir = project.clone();
        session.provider_session_id = None;
        session.save(&paths).unwrap();
        sample_policy(&session).save(&paths).unwrap();

        let unverified = handle_hook(
            AgentKind::Codex,
            &hook("UserPromptSubmit", &project, "late-provider-session"),
            &paths,
            Utc::now(),
        )
        .unwrap();
        let output = handle_hook(
            AgentKind::Codex,
            &confirmation_hook(
                AgentKind::Codex,
                &project,
                "late-provider-session",
                TEST_CONFIRMATION_TOKEN,
            ),
            &paths,
            Utc::now(),
        )
        .unwrap();

        assert!(unverified.is_none());
        assert!(output.is_some());
        assert_eq!(
            SessionState::load(&paths)
                .unwrap()
                .unwrap()
                .provider_session_id
                .as_deref(),
            Some("late-provider-session")
        );
        assert_eq!(
            ActivePolicy::load(&paths)
                .unwrap()
                .unwrap()
                .provider_session_id
                .as_deref(),
            Some("late-provider-session")
        );
    }

    #[test]
    fn hook_payload_requires_all_correlation_fields_and_supports_cursor_workspace_roots() {
        assert!(parse_hook_payload(&json!({
            "hook_event_name": "UserPromptSubmit",
            "cwd": "/workspace/project"
        }))
        .is_none());
        assert!(parse_hook_payload(&json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "provider-session-1"
        }))
        .is_none());
        assert!(parse_hook_payload(&json!({
            "hook_event_name": "UserPromptSubmit",
            "cwd": "/workspace/project",
            "workspace_path": "/workspace/other-project",
            "session_id": "provider-session-1"
        }))
        .is_none());
        assert!(parse_hook_payload(&json!({
            "hook_event_name": "UserPromptSubmit",
            "cwd": "/workspace/project",
            "workspace_roots": ["/workspace/other-project"],
            "session_id": "provider-session-1"
        }))
        .is_none());
        assert_eq!(
            parse_hook_payload(&json!({
                "hook_event_name": "UserPromptSubmit",
                "cwd": "/workspace/project/src",
                "workspace_roots": ["/workspace/project"],
                "session_id": "provider-session-1",
                "prompt": "$commute-mode rucksack-test-0123456789abcdef"
            })),
            Some(HookPayload {
                event: "UserPromptSubmit".to_owned(),
                project_dir: PathBuf::from("/workspace/project/src"),
                provider_session_id: "provider-session-1".to_owned(),
                prompt: Some("$commute-mode rucksack-test-0123456789abcdef".to_owned()),
            })
        );
        assert_eq!(
            parse_hook_payload(&json!({
                "event_name": "beforeSubmitPrompt",
                "workspace_roots": ["/workspace/project"],
                "conversation_id": "cursor-conversation-1",
                "prompt": "/commute-mode rucksack-test-0123456789abcdef"
            })),
            Some(HookPayload {
                event: "beforeSubmitPrompt".to_owned(),
                project_dir: PathBuf::from("/workspace/project"),
                provider_session_id: "cursor-conversation-1".to_owned(),
                prompt: Some("/commute-mode rucksack-test-0123456789abcdef".to_owned()),
            })
        );
    }

    #[test]
    fn confirmation_commands_are_agent_specific_and_exact() {
        assert_eq!(
            commute_mode_confirmation_command(AgentKind::Codex, TEST_CONFIRMATION_TOKEN),
            "$commute-mode rucksack-test-0123456789abcdef"
        );
        for agent in [AgentKind::Claude, AgentKind::Cursor] {
            assert_eq!(
                commute_mode_confirmation_command(agent, TEST_CONFIRMATION_TOKEN),
                "/commute-mode rucksack-test-0123456789abcdef"
            );
        }
    }

    #[test]
    fn codex_and_claude_receive_exact_policy_context() {
        let policy = "lease nonce rucksack-test-42";
        for agent in [AgentKind::Codex, AgentKind::Claude] {
            for event in ["SessionStart", "UserPromptSubmit"] {
                let output = policy_context_output(agent, event, policy).unwrap();
                assert_eq!(
                    output,
                    json!({
                        "hookSpecificOutput": {
                            "hookEventName": event,
                            "additionalContext": policy
                        }
                    })
                );
            }
        }
    }

    #[test]
    fn policy_context_is_never_returned_for_permission_or_cursor_hooks() {
        assert!(
            policy_context_output(AgentKind::Codex, "PermissionRequest", "do not return this")
                .is_none()
        );
        assert!(policy_context_output(
            AgentKind::Claude,
            "PermissionRequest",
            "do not return this"
        )
        .is_none());
        assert!(
            policy_context_output(AgentKind::Cursor, "sessionStart", "do not return this")
                .is_none()
        );
    }
}
