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
use crate::system::{run_owned, which, CommandResult};
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn preflight_install(paths: &AppPaths) -> Result<()> {
    preflight_events(AgentKind::Codex, &paths.codex_hooks)?;
    preflight_managed_skill(&paths.codex_skill)
}

pub fn preflight_uninstall(paths: &AppPaths) -> Result<()> {
    preflight_uninstall_events(AgentKind::Codex, &paths.codex_hooks)?;
    preflight_remove_managed_skill(&paths.codex_skill)
}

pub fn install(paths: &AppPaths, binary: &Path) -> Result<Vec<Mutation>> {
    preflight_install(paths)?;
    let events = expected_events(binary);
    let hooks = install_events(
        AgentKind::Codex,
        &paths.codex_hooks,
        &events,
        ManagedFileKind::Hooks,
    )?;
    let skill = install_managed_skill(AgentKind::Codex, &paths.codex_skill)?;
    Ok(vec![hooks, skill])
}

pub fn uninstall(paths: &AppPaths) -> Result<Vec<PathBuf>> {
    preflight_uninstall(paths)?;
    let mut changed = Vec::new();
    if uninstall_events(AgentKind::Codex, &paths.codex_hooks)? {
        changed.push(paths.codex_hooks.clone());
    }
    if remove_managed_skill(&paths.codex_skill)? {
        changed.push(paths.codex_skill.clone());
    }
    Ok(changed)
}

pub fn verify(paths: &AppPaths, binary: &Path) -> AgentAdapterEvidence {
    let events = expected_events(binary);
    let files = vec![
        verify_events(AgentKind::Codex, &paths.codex_hooks, &events),
        verify_managed_skill(&paths.codex_skill),
    ];
    AgentAdapterEvidence {
        agent: AgentKind::Codex,
        current: files.iter().all(super::AdapterFileEvidence::is_current),
        files,
    }
}

fn expected_events(binary: &Path) -> Vec<(&'static str, Value)> {
    let command = format!("{} hook codex # {}", shell_quote(binary), MANAGED_MARKER);
    vec![
        (
            "SessionStart",
            wrapped_hook(&command, Some("Loading Commute Mode"), 5),
        ),
        ("UserPromptSubmit", wrapped_hook(&command, None, 5)),
        (
            "PermissionRequest",
            wrapped_hook(&command, Some("Reporting approval state"), 5),
        ),
        ("PostToolUse", wrapped_hook(&command, None, 5)),
        ("Stop", wrapped_hook(&command, None, 5)),
        ("SessionEnd", wrapped_hook(&command, None, 3)),
    ]
}

pub fn codex_remote_start() -> Result<CommandResult> {
    let executable = which("codex").ok_or_else(|| anyhow!("codex is not on PATH"))?;
    let args = vec![
        "remote-control".to_owned(),
        "start".to_owned(),
        "--json".to_owned(),
    ];
    run_owned(executable, &args)
}

pub fn codex_remote_stop() -> Result<CommandResult> {
    let executable = which("codex").ok_or_else(|| anyhow!("codex is not on PATH"))?;
    let args = vec![
        "remote-control".to_owned(),
        "stop".to_owned(),
        "--json".to_owned(),
    ];
    run_owned(executable, &args)
}

pub fn codex_pair() -> Result<CommandResult> {
    let executable = which("codex").ok_or_else(|| anyhow!("codex is not on PATH"))?;
    let args = vec![
        "remote-control".to_owned(),
        "pair".to_owned(),
        "--json".to_owned(),
    ];
    run_owned(executable, &args)
}
