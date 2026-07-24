use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rucksack_core::files::append_line;
use rucksack_core::state::{ActivePolicy, SessionPhase, SessionState};
use rucksack_core::{AgentKind, AppPaths};
use serde_json::{json, Value};
use std::io::{self, Read};

const MAX_HOOK_INPUT: u64 = 1024 * 1024;
const MAX_HOOK_EVENT_NAME: usize = 64;

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
    let event = find_event_name(&payload).unwrap_or_else(|| "unknown".to_owned());
    record_event(agent, &event, paths)?;

    let Some(policy) = ActivePolicy::load(paths)? else {
        return Ok(());
    };
    if !policy.is_active(Utc::now()) || policy.agent != agent {
        return Ok(());
    }

    match agent {
        AgentKind::Codex | AgentKind::Claude
            if matches!(event.as_str(), "SessionStart" | "UserPromptSubmit") =>
        {
            let output = json!({
                "hookSpecificOutput": {
                    "hookEventName": event,
                    "additionalContext": policy.policy
                }
            });
            println!("{}", serde_json::to_string(&output)?);
        }
        // Cursor's project rule and `/commute-mode` command are the authoritative policy path.
        // Its sessionStart additional_context response has varied across Cursor releases, so this
        // hook records lifecycle state but deliberately does not claim context injection.
        AgentKind::Cursor if event.eq_ignore_ascii_case("sessionStart") => {}
        _ => {}
    }
    Ok(())
}

fn record_event(agent: AgentKind, event: &str, paths: &AppPaths) -> Result<()> {
    let now = Utc::now();
    let timestamp = now.to_rfc3339();
    let line = format!("{timestamp} agent={agent} event={event}");
    append_line(&paths.log_dir.join("hooks.log"), &line)?;

    let Some(session) = SessionState::load(paths)? else {
        return Ok(());
    };
    if session.agent != agent {
        return Ok(());
    }
    SessionState::update(paths, session.id, |current| {
        Ok(apply_lifecycle_event(current, event, now))
    })?;
    Ok(())
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

fn find_event_name(value: &Value) -> Option<String> {
    const KEYS: &[&str] = &[
        "hook_event_name",
        "hookEventName",
        "event_name",
        "eventName",
        "event",
    ];
    let object = value.as_object()?;
    for key in KEYS {
        if let Some(value) = object.get(*key).and_then(Value::as_str) {
            return safe_event_name(value);
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;
    use rucksack_core::policy::Focus;
    use rucksack_core::state::SessionStateWriteConflict;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;
    use uuid::Uuid;

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

        record_event(AgentKind::Codex, "SessionEnd", &paths).unwrap();
        daemon_snapshot.last_heartbeat_at = Some(Utc::now());
        let error = daemon_snapshot.save(&paths).unwrap_err();

        assert!(error.downcast_ref::<SessionStateWriteConflict>().is_some());
        let persisted = SessionState::load(&paths).unwrap().unwrap();
        assert_eq!(persisted.phase, SessionPhase::Completed);
        assert_eq!(persisted.last_event.as_deref(), Some("SessionEnd"));
        assert!(persisted.last_heartbeat_at.is_none());
    }

    #[test]
    fn another_agents_hook_does_not_rewrite_the_active_session() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let mut initial = sample_session();
        initial.save(&paths).unwrap();
        let revision = initial.revision;

        record_event(AgentKind::Claude, "SessionEnd", &paths).unwrap();

        let persisted = SessionState::load(&paths).unwrap().unwrap();
        assert_eq!(persisted.revision, revision);
        assert_eq!(persisted.phase, SessionPhase::Active);
        assert!(persisted.last_event.is_none());
    }
}
