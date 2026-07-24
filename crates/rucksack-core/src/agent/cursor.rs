use super::json_hooks::{
    cursor_hook, install_events, preflight_events, preflight_uninstall_events, uninstall_events,
    verify_events,
};
use super::{
    shell_quote, AdapterFileEvidence, AgentAdapterEvidence, AgentKind, ManagedFileKind, Mutation,
    MANAGED_MARKER,
};
use crate::files::{atomic_write, reject_symlink, remove_if_exists};
use crate::paths::AppPaths;
use crate::state::ActivePolicy;
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CURSOR_RULE_RELATIVE: &str = ".cursor/rules/rucksack-commute.mdc";
const CURSOR_COMMAND_RELATIVE: &str = ".cursor/commands/commute-mode.md";
const EXCLUDE_BEGIN: &str = "# rucksack-managed:begin";
const EXCLUDE_END: &str = "# rucksack-managed:end";

pub fn preflight_install(paths: &AppPaths) -> Result<()> {
    preflight_events(AgentKind::Cursor, &paths.cursor_hooks)
}

pub fn preflight_uninstall(paths: &AppPaths) -> Result<()> {
    preflight_uninstall_events(AgentKind::Cursor, &paths.cursor_hooks)
}

pub fn install(paths: &AppPaths, binary: &Path) -> Result<Vec<Mutation>> {
    preflight_install(paths)?;
    let events = expected_events(binary);
    let hooks = install_events(
        AgentKind::Cursor,
        &paths.cursor_hooks,
        &events,
        ManagedFileKind::Hooks,
    )?;
    Ok(vec![hooks])
}

pub fn uninstall(paths: &AppPaths) -> Result<Vec<PathBuf>> {
    preflight_uninstall(paths)?;
    let mut changed = Vec::new();
    if uninstall_events(AgentKind::Cursor, &paths.cursor_hooks)? {
        changed.push(paths.cursor_hooks.clone());
    }
    Ok(changed)
}

pub fn verify(paths: &AppPaths, binary: &Path) -> AgentAdapterEvidence {
    let events = expected_events(binary);
    let files = vec![verify_events(
        AgentKind::Cursor,
        &paths.cursor_hooks,
        &events,
    )];
    AgentAdapterEvidence {
        agent: AgentKind::Cursor,
        current: files.iter().all(AdapterFileEvidence::is_current),
        files,
    }
}

fn expected_events(binary: &Path) -> Vec<(&'static str, Value)> {
    let command = format!("{} hook cursor # {}", shell_quote(binary), MANAGED_MARKER);
    vec![
        ("sessionStart", cursor_hook(&command, 5)),
        ("beforeSubmitPrompt", cursor_hook(&command, 5)),
        ("beforeShellExecution", cursor_hook(&command, 5)),
        ("afterShellExecution", cursor_hook(&command, 5)),
        ("afterFileEdit", cursor_hook(&command, 5)),
        ("stop", cursor_hook(&command, 5)),
        ("sessionEnd", cursor_hook(&command, 5)),
    ]
}

/// Install the active commute policy into the current Cursor workspace.
///
/// Cursor does not currently support file-backed global rules. The reversible policy therefore
/// lives in the active project as an always-applied rule plus a `/commute-mode` command for an
/// already-open conversation. Both files are excluded locally through `.git/info/exclude` when
/// the project is a Git worktree; no tracked repository file is edited.
pub fn activate_cursor_rule(project: &Path, policy: &ActivePolicy) -> Result<Vec<PathBuf>> {
    let rule = cursor_rule_path(project);
    let command = cursor_command_path(project);
    ensure_owned_rule_or_absent(&rule)?;
    ensure_owned_command_or_absent(&command)?;
    preflight_git_excludes(project)?;

    let rule_text = format!(
        r#"---
description: Rucksack Commute Mode for a closed, battery-powered Mac
alwaysApply: true
---

<!-- {marker} -->

{policy}
"#,
        marker = MANAGED_MARKER,
        policy = policy.policy
    );
    let command_text = format!(
        r#"<!-- {marker} -->
# Commute Mode

Apply the following temporary operating policy for this conversation. Acknowledge it briefly,
then continue the current bounded task without widening scope.

{policy}
"#,
        marker = MANAGED_MARKER,
        policy = policy.policy
    );

    atomic_write(&rule, rule_text.as_bytes(), 0o600)?;
    atomic_write(&command, command_text.as_bytes(), 0o600)?;
    install_git_excludes(project)?;
    Ok(vec![rule, command])
}

pub fn deactivate_cursor_rule(project: &Path) -> Result<Vec<PathBuf>> {
    let rule = cursor_rule_path(project);
    let command = cursor_command_path(project);
    ensure_owned_rule_or_absent(&rule)?;
    ensure_owned_command_or_absent(&command)?;
    preflight_git_excludes(project)?;

    let mut changed = Vec::new();
    for path in [rule, command] {
        if !path.exists() {
            continue;
        }
        remove_if_exists(&path)?;
        changed.push(path);
    }
    remove_git_excludes(project)?;
    Ok(changed)
}

pub fn cursor_rule_path(project: &Path) -> PathBuf {
    project.join(CURSOR_RULE_RELATIVE)
}

pub fn cursor_command_path(project: &Path) -> PathBuf {
    project.join(CURSOR_COMMAND_RELATIVE)
}

fn ensure_owned_rule_or_absent(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    reject_symlink(path)?;
    let existing =
        fs::read_to_string(path).with_context(|| format!("Could not read {}", path.display()))?;
    let marker = format!("\n---\n\n<!-- {MANAGED_MARKER} -->\n");
    if !existing.starts_with("---\n") || !existing.contains(&marker) {
        return Err(anyhow!(
            "Refusing to overwrite unowned Cursor rule {}",
            path.display()
        ));
    }
    Ok(())
}

fn ensure_owned_command_or_absent(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    reject_symlink(path)?;
    let existing =
        fs::read_to_string(path).with_context(|| format!("Could not read {}", path.display()))?;
    let prefix = format!("<!-- {MANAGED_MARKER} -->\n");
    if !existing.starts_with(&prefix) {
        return Err(anyhow!(
            "Refusing to overwrite unowned Cursor command {}",
            path.display()
        ));
    }
    Ok(())
}

fn preflight_git_excludes(project: &Path) -> Result<()> {
    let Some(path) = git_exclude_path(project) else {
        return Ok(());
    };
    reject_symlink(&path)?;
    if !path.exists() {
        return Ok(());
    }
    let existing = fs::read_to_string(&path)
        .with_context(|| format!("Could not read Git exclude file {}", path.display()))?;
    let begin_count = existing.matches(EXCLUDE_BEGIN).count();
    let end_count = existing.matches(EXCLUDE_END).count();
    if begin_count == 0 && end_count == 0 {
        return Ok(());
    }
    let valid_block = begin_count == 1
        && end_count == 1
        && existing.find(EXCLUDE_BEGIN) < existing.find(EXCLUDE_END);
    if !valid_block {
        return Err(anyhow!(
            "Found an incomplete or duplicate Rucksack block in {}; refusing to rewrite it",
            path.display()
        ));
    }
    Ok(())
}

fn install_git_excludes(project: &Path) -> Result<()> {
    preflight_git_excludes(project)?;
    let Some(path) = git_exclude_path(project) else {
        return Ok(());
    };
    let existing = if path.exists() {
        fs::read_to_string(&path)
            .with_context(|| format!("Could not read Git exclude file {}", path.display()))?
    } else {
        String::new()
    };
    if existing.contains(EXCLUDE_BEGIN) {
        return Ok(());
    }
    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(EXCLUDE_BEGIN);
    next.push('\n');
    next.push_str("/.cursor/rules/rucksack-commute.mdc\n");
    next.push_str("/.cursor/commands/commute-mode.md\n");
    next.push_str(EXCLUDE_END);
    next.push('\n');
    atomic_write(&path, next.as_bytes(), 0o600)
}

fn remove_git_excludes(project: &Path) -> Result<()> {
    preflight_git_excludes(project)?;
    let Some(path) = git_exclude_path(project) else {
        return Ok(());
    };
    if !path.exists() {
        return Ok(());
    }
    let existing = fs::read_to_string(&path)?;
    let Some(start) = existing.find(EXCLUDE_BEGIN) else {
        return Ok(());
    };
    let Some(end_offset) = existing[start..].find(EXCLUDE_END) else {
        return Err(anyhow!(
            "Found an incomplete Rucksack block in {}; refusing to rewrite it",
            path.display()
        ));
    };
    let mut end = start + end_offset + EXCLUDE_END.len();
    if existing.as_bytes().get(end) == Some(&b'\n') {
        end += 1;
    }
    let mut next = String::with_capacity(existing.len());
    next.push_str(&existing[..start]);
    next.push_str(&existing[end..]);
    atomic_write(&path, next.as_bytes(), 0o600)
}

fn git_exclude_path(project: &Path) -> Option<PathBuf> {
    let output = Command::new("/usr/bin/git")
        .arg("-C")
        .arg(project)
        .args(["rev-parse", "--git-path", "info/exclude"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let path = PathBuf::from(text.trim());
    if path.as_os_str().is_empty() {
        None
    } else if path.is_absolute() {
        Some(path)
    } else {
        Some(project.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Focus;
    use chrono::{Duration, Utc};
    use tempfile::tempdir;
    use uuid::Uuid;

    fn active_policy(project: &Path) -> ActivePolicy {
        let activated_at = Utc::now();
        ActivePolicy {
            version: 1,
            session_id: Uuid::new_v4(),
            agent: AgentKind::Cursor,
            focus: Focus::Continue,
            project_dir: project.to_path_buf(),
            activated_at,
            expires_at: activated_at + Duration::minutes(30),
            policy: "Keep the current task bounded.".to_owned(),
        }
    }

    fn initialize_git_repository(project: &Path) {
        let status = Command::new("/usr/bin/git")
            .arg("init")
            .arg("--quiet")
            .arg(project)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn cursor_rule_round_trip_preserves_unrelated_git_excludes() {
        let directory = tempdir().unwrap();
        let project = directory.path();
        initialize_git_repository(project);
        let exclude = git_exclude_path(project).unwrap();
        fs::write(&exclude, "# keep-this-entry\n/local-only\n").unwrap();

        let policy = active_policy(project);
        activate_cursor_rule(project, &policy).unwrap();
        activate_cursor_rule(project, &policy).unwrap();

        assert!(cursor_rule_path(project).exists());
        assert!(cursor_command_path(project).exists());
        let installed_exclude = fs::read_to_string(&exclude).unwrap();
        assert_eq!(installed_exclude.matches(EXCLUDE_BEGIN).count(), 1);
        assert!(installed_exclude.contains("# keep-this-entry"));
        assert!(installed_exclude.contains("/local-only"));

        deactivate_cursor_rule(project).unwrap();

        assert!(!cursor_rule_path(project).exists());
        assert!(!cursor_command_path(project).exists());
        let removed_exclude = fs::read_to_string(&exclude).unwrap();
        assert!(!removed_exclude.contains(EXCLUDE_BEGIN));
        assert!(removed_exclude.contains("# keep-this-entry"));
        assert!(removed_exclude.contains("/local-only"));
    }

    #[test]
    fn unowned_cursor_command_blocks_activation_before_rule_write() {
        let directory = tempdir().unwrap();
        let project = directory.path();
        let command = cursor_command_path(project);
        fs::create_dir_all(command.parent().unwrap()).unwrap();
        fs::write(
            &command,
            "# User command\n\nThis mentions rucksack-managed incidentally.\n",
        )
        .unwrap();

        let error = activate_cursor_rule(project, &active_policy(project)).unwrap_err();

        assert!(error.to_string().contains("unowned Cursor command"));
        assert!(!cursor_rule_path(project).exists());
        assert_eq!(
            fs::read_to_string(command).unwrap(),
            "# User command\n\nThis mentions rucksack-managed incidentally.\n"
        );
    }

    #[test]
    fn malformed_git_exclude_blocks_activation_before_file_writes() {
        let directory = tempdir().unwrap();
        let project = directory.path();
        initialize_git_repository(project);
        let exclude = git_exclude_path(project).unwrap();
        fs::write(&exclude, format!("{EXCLUDE_BEGIN}\n")).unwrap();

        let error = activate_cursor_rule(project, &active_policy(project)).unwrap_err();

        assert!(error.to_string().contains("incomplete or duplicate"));
        assert!(!cursor_rule_path(project).exists());
        assert!(!cursor_command_path(project).exists());
    }
}
