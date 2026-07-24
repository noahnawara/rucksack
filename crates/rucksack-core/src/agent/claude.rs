use super::json_hooks::{
    install_events, preflight_events, preflight_uninstall_events, uninstall_events, verify_events,
    wrapped_hook,
};
use super::{
    install_managed_skill, preflight_managed_skill, preflight_remove_managed_skill,
    remove_managed_skill, shell_quote, verify_managed_skill, AgentAdapterEvidence, AgentKind,
    ManagedFileKind, Mutation, MANAGED_MARKER,
};
use crate::paths::AppPaths;
use anyhow::Result;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn preflight_install(paths: &AppPaths) -> Result<()> {
    preflight_events(AgentKind::Claude, &paths.claude_settings)?;
    preflight_managed_skill(&paths.claude_skill)
}

pub fn preflight_uninstall(paths: &AppPaths) -> Result<()> {
    preflight_uninstall_events(AgentKind::Claude, &paths.claude_settings)?;
    preflight_remove_managed_skill(&paths.claude_skill)
}

pub fn install(paths: &AppPaths, binary: &Path) -> Result<Vec<Mutation>> {
    preflight_install(paths)?;
    let events = expected_events(binary);
    let hooks = install_events(
        AgentKind::Claude,
        &paths.claude_settings,
        &events,
        ManagedFileKind::Hooks,
    )?;
    let skill = install_managed_skill(AgentKind::Claude, &paths.claude_skill)?;
    Ok(vec![hooks, skill])
}

pub fn uninstall(paths: &AppPaths) -> Result<Vec<PathBuf>> {
    preflight_uninstall(paths)?;
    let mut changed = Vec::new();
    if uninstall_events(AgentKind::Claude, &paths.claude_settings)? {
        changed.push(paths.claude_settings.clone());
    }
    if remove_managed_skill(&paths.claude_skill)? {
        changed.push(paths.claude_skill.clone());
    }
    Ok(changed)
}

pub fn verify(paths: &AppPaths, binary: &Path) -> AgentAdapterEvidence {
    let events = expected_events(binary);
    let files = vec![
        verify_events(AgentKind::Claude, &paths.claude_settings, &events),
        verify_managed_skill(&paths.claude_skill),
    ];
    AgentAdapterEvidence {
        agent: AgentKind::Claude,
        current: files.iter().all(super::AdapterFileEvidence::is_current),
        files,
    }
}

fn expected_events(binary: &Path) -> Vec<(&'static str, Value)> {
    let command = format!("{} hook claude # {}", shell_quote(binary), MANAGED_MARKER);
    vec![
        (
            "SessionStart",
            wrapped_hook(&command, Some("Loading Commute Mode"), 5),
        ),
        ("UserPromptSubmit", wrapped_hook(&command, None, 5)),
        (
            "Notification",
            wrapped_hook(&command, Some("Reporting input state"), 5),
        ),
        (
            "PermissionRequest",
            wrapped_hook(&command, Some("Reporting approval state"), 5),
        ),
        ("PostToolUse", wrapped_hook(&command, None, 5)),
        ("Stop", wrapped_hook(&command, None, 5)),
        ("SessionEnd", wrapped_hook(&command, None, 5)),
    ]
}

pub fn claude_remote_server_command(project_name: Option<&str>) -> Vec<String> {
    let mut args = vec!["remote-control".to_owned()];
    if let Some(name) = project_name.filter(|name| !name.trim().is_empty()) {
        args.push("--name".to_owned());
        args.push(name.to_owned());
    }
    args
}

pub fn claude_remote_user_instruction() -> &'static str {
    "In the active Claude Code session, run `/remote-control`, then return when `/rc active` appears."
}
