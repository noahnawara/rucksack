use super::json_hooks::{
    cursor_hook, install_events, preflight_events, preflight_uninstall_events, uninstall_events,
    verify_events,
};
use super::{
    shell_quote, AdapterFileEvidence, AgentAdapterEvidence, AgentKind, ManagedFileKind, Mutation,
    MANAGED_MARKER,
};
use crate::files::{AnchoredFile, AnchoredFileContents, AnchoredRoot};
use crate::paths::AppPaths;
use crate::state::ActivePolicy;
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const CURSOR_RULE_RELATIVE: &str = ".cursor/rules/rucksack-commute.mdc";
const CURSOR_COMMAND_RELATIVE: &str = ".cursor/commands/commute-mode.md";
const CURSOR_COMMAND_TEMPLATE: &str =
    include_str!("../../../../assets/adapters/cursor/commute-mode.md");
const CURSOR_POLICY_PLACEHOLDER: &str = "{{RUCKSACK_ACTIVE_POLICY}}";
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
    activate_cursor_rule_after_prepare(project, policy, || Ok(()))
}

fn activate_cursor_rule_after_prepare(
    project: &Path,
    policy: &ActivePolicy,
    after_prepare: impl FnOnce() -> Result<()>,
) -> Result<Vec<PathBuf>> {
    let rule = cursor_rule_path(project);
    let command = cursor_command_path(project);
    let mut managed_files = prepare_cursor_files(project, &rule, &command)?;

    let rule_text = format!(
        r#"---
description: rucksack Commute Mode for a closed, battery-powered Mac
alwaysApply: true
---

<!-- {marker} -->

Read `~/Library/Application Support/Rucksack/active-policy.json` before applying the
temporary policy below. Apply it only while `cleanup_pending` is false and `expires_at` is
still in the future. If that state is missing, invalid, expired, or cleanup-pending, ignore
the policy below and continue under the existing Cursor configuration.

{policy}
"#,
        marker = MANAGED_MARKER,
        policy = policy.policy
    );
    let command_text = render_cursor_command(&policy.policy)?;

    let activation = || -> Result<()> {
        after_prepare()?;
        managed_files
            .rule
            .file
            .write_atomic(rule_text.as_bytes(), 0o600)?;
        managed_files
            .command
            .file
            .write_atomic(command_text.as_bytes(), 0o600)?;
        install_git_excludes(managed_files.git_exclude.as_mut())?;
        verify_cursor_file_bindings(&managed_files)
    };
    if let Err(error) = activation() {
        let rollback_errors = restore_cursor_files(&mut managed_files);
        if rollback_errors.is_empty() {
            return Err(error);
        }
        return Err(anyhow!(
            "{error:#}; Cursor activation rollback was incomplete:\n- {}",
            rollback_errors.join("\n- ")
        ));
    }
    Ok(vec![rule, command])
}

fn render_cursor_command(policy: &str) -> Result<String> {
    if CURSOR_COMMAND_TEMPLATE
        .matches(CURSOR_POLICY_PLACEHOLDER)
        .count()
        != 1
    {
        anyhow::bail!("Cursor command template must contain exactly one policy placeholder");
    }
    Ok(CURSOR_COMMAND_TEMPLATE.replace(CURSOR_POLICY_PLACEHOLDER, policy))
}

pub fn deactivate_cursor_rule(project: &Path) -> Result<Vec<PathBuf>> {
    deactivate_cursor_rule_after_prepare(project, || Ok(()))
}

fn deactivate_cursor_rule_after_prepare(
    project: &Path,
    after_prepare: impl FnOnce() -> Result<()>,
) -> Result<Vec<PathBuf>> {
    let rule = cursor_rule_path(project);
    let command = cursor_command_path(project);
    let mut managed_files = prepare_cursor_files(project, &rule, &command)?;
    let mut changed: Vec<PathBuf> = Vec::new();
    if managed_files.rule.was_present() {
        changed.push(rule);
    }
    if managed_files.command.was_present() {
        changed.push(command);
    }

    let deactivation = || -> Result<()> {
        after_prepare()?;
        managed_files.rule.file.remove_if_exists()?;
        managed_files.command.file.remove_if_exists()?;
        remove_git_excludes(managed_files.git_exclude.as_mut())?;
        verify_cursor_file_bindings(&managed_files)
    };
    if let Err(error) = deactivation() {
        let rollback_errors = restore_cursor_files(&mut managed_files);
        if rollback_errors.is_empty() {
            return Err(error);
        }
        return Err(anyhow!(
            "{error:#}; Cursor deactivation rollback was incomplete:\n- {}",
            rollback_errors.join("\n- ")
        ));
    }
    Ok(changed)
}

pub fn cursor_rule_path(project: &Path) -> PathBuf {
    project.join(CURSOR_RULE_RELATIVE)
}

pub fn cursor_command_path(project: &Path) -> PathBuf {
    project.join(CURSOR_COMMAND_RELATIVE)
}

#[derive(Debug)]
enum PreviousManagedFile {
    Missing,
    Present { bytes: Vec<u8>, mode: u32 },
}

#[derive(Debug)]
struct ManagedFileSnapshot {
    file: AnchoredFile,
    previous: PreviousManagedFile,
}

impl ManagedFileSnapshot {
    fn was_present(&self) -> bool {
        matches!(self.previous, PreviousManagedFile::Present { .. })
    }
}

#[derive(Debug)]
struct CursorManagedFiles {
    rule: ManagedFileSnapshot,
    command: ManagedFileSnapshot,
    git_exclude: Option<ManagedFileSnapshot>,
}

fn prepare_cursor_files(project: &Path, rule: &Path, command: &Path) -> Result<CursorManagedFiles> {
    let project_root = AnchoredRoot::open(project)?;
    let rule = capture_managed_file(project_root.file(rule)?)?;
    let command = capture_managed_file(project_root.file(command)?)?;
    ensure_owned_rule_or_absent(&rule)?;
    ensure_owned_command_or_absent(&command)?;
    let git_exclude = capture_git_exclude(project)?;
    Ok(CursorManagedFiles {
        rule,
        command,
        git_exclude,
    })
}

fn capture_managed_file(mut file: AnchoredFile) -> Result<ManagedFileSnapshot> {
    let previous = match file.read()? {
        Some(AnchoredFileContents { bytes, mode }) => PreviousManagedFile::Present { bytes, mode },
        None => PreviousManagedFile::Missing,
    };
    Ok(ManagedFileSnapshot { file, previous })
}

fn restore_cursor_files(files: &mut CursorManagedFiles) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();
    if let Some(git_exclude) = files.git_exclude.as_mut() {
        restore_managed_file_or_record(git_exclude, &mut errors);
    }
    restore_managed_file_or_record(&mut files.command, &mut errors);
    restore_managed_file_or_record(&mut files.rule, &mut errors);
    errors
}

fn restore_managed_file_or_record(snapshot: &mut ManagedFileSnapshot, errors: &mut Vec<String>) {
    let path = snapshot.file.path().to_path_buf();
    if let Err(error) = restore_managed_file(snapshot) {
        errors.push(format!("{}: {error:#}", path.display()));
    }
}

fn restore_managed_file(snapshot: &mut ManagedFileSnapshot) -> Result<()> {
    match &snapshot.previous {
        PreviousManagedFile::Missing => {
            snapshot.file.remove_if_exists()?;
            Ok(())
        }
        PreviousManagedFile::Present { bytes, mode } => snapshot.file.write_atomic(bytes, *mode),
    }
}

fn verify_cursor_file_bindings(files: &CursorManagedFiles) -> Result<()> {
    files.rule.file.verify_parent_binding()?;
    files.command.file.verify_parent_binding()?;
    if let Some(git_exclude) = files.git_exclude.as_ref() {
        git_exclude.file.verify_parent_binding()?;
    }
    Ok(())
}

fn ensure_owned_rule_or_absent(snapshot: &ManagedFileSnapshot) -> Result<()> {
    let Some(existing) = previous_text(snapshot, "Cursor rule")? else {
        return Ok(());
    };
    let marker = format!("\n---\n\n<!-- {MANAGED_MARKER} -->\n");
    if !existing.starts_with("---\n") || !existing.contains(&marker) {
        return Err(anyhow!(
            "Refusing to overwrite unowned Cursor rule {}",
            snapshot.file.path().display()
        ));
    }
    Ok(())
}

fn ensure_owned_command_or_absent(snapshot: &ManagedFileSnapshot) -> Result<()> {
    let Some(existing) = previous_text(snapshot, "Cursor command")? else {
        return Ok(());
    };
    let prefix = format!("<!-- {MANAGED_MARKER} -->\n");
    if !existing.starts_with(&prefix) {
        return Err(anyhow!(
            "Refusing to overwrite unowned Cursor command {}",
            snapshot.file.path().display()
        ));
    }
    Ok(())
}

fn capture_git_exclude(project: &Path) -> Result<Option<ManagedFileSnapshot>> {
    let git_root_path = project.join(".git");
    let metadata = match fs::symlink_metadata(&git_root_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            reject_nested_git_project(project)?;
            return Ok(None);
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Could not inspect {}", git_root_path.display()));
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(anyhow!(
            "Refusing to traverse symlinked Git directory {}",
            git_root_path.display()
        ));
    }
    if !metadata.is_dir() {
        return Err(anyhow!(
            "Cursor Commute Mode requires the selected project to be a Git worktree root with a real .git directory; refusing to create unexcluded files in {}",
            project.display()
        ));
    }

    let path = git_root_path.join("info/exclude");
    let root = AnchoredRoot::open(git_exclude_managed_root(&path)?)?;
    let snapshot = capture_managed_file(root.file(&path)?)?;
    preflight_git_exclude(&snapshot)?;
    Ok(Some(snapshot))
}

fn reject_nested_git_project(project: &Path) -> Result<()> {
    for ancestor in project.ancestors().skip(1) {
        let git_path = ancestor.join(".git");
        match fs::symlink_metadata(&git_path) {
            Ok(_) => {
                return Err(anyhow!(
                    "Cursor Commute Mode requires the selected project to be the Git worktree root {}; refusing to create unexcluded files in {}",
                    ancestor.display(),
                    project.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Could not inspect {}", git_path.display()));
            }
        }
    }
    Ok(())
}

fn preflight_git_exclude(snapshot: &ManagedFileSnapshot) -> Result<()> {
    let Some(existing) = previous_text(snapshot, "Git exclude file")? else {
        return Ok(());
    };
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
            "Found an incomplete or duplicate rucksack block in {}; refusing to rewrite it",
            snapshot.file.path().display()
        ));
    }
    Ok(())
}

fn git_exclude_managed_root(path: &Path) -> Result<&Path> {
    path.parent()
        .and_then(Path::parent)
        .with_context(|| format!("Git exclude path has no managed root: {}", path.display()))
}

fn install_git_excludes(snapshot: Option<&mut ManagedFileSnapshot>) -> Result<()> {
    let Some(snapshot) = snapshot else {
        return Ok(());
    };
    let existing = previous_text(snapshot, "Git exclude file")?
        .unwrap_or_default()
        .to_owned();
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
    let mode = previous_mode(snapshot, 0o600);
    snapshot.file.write_atomic(next.as_bytes(), mode)
}

fn remove_git_excludes(snapshot: Option<&mut ManagedFileSnapshot>) -> Result<()> {
    let Some(snapshot) = snapshot else {
        return Ok(());
    };
    let Some(existing) = previous_text(snapshot, "Git exclude file")?.map(str::to_owned) else {
        return Ok(());
    };
    let Some(start) = existing.find(EXCLUDE_BEGIN) else {
        return Ok(());
    };
    let Some(end_offset) = existing[start..].find(EXCLUDE_END) else {
        return Err(anyhow!(
            "Found an incomplete rucksack block in {}; refusing to rewrite it",
            snapshot.file.path().display()
        ));
    };
    let mut end = start + end_offset + EXCLUDE_END.len();
    if existing.as_bytes().get(end) == Some(&b'\n') {
        end += 1;
    }
    let mut next = String::with_capacity(existing.len());
    next.push_str(&existing[..start]);
    next.push_str(&existing[end..]);
    let mode = previous_mode(snapshot, 0o600);
    snapshot.file.write_atomic(next.as_bytes(), mode)
}

fn previous_text<'a>(
    snapshot: &'a ManagedFileSnapshot,
    file_kind: &str,
) -> Result<Option<&'a str>> {
    let PreviousManagedFile::Present { bytes, .. } = &snapshot.previous else {
        return Ok(None);
    };
    std::str::from_utf8(bytes).map(Some).with_context(|| {
        format!(
            "{file_kind} {} is not valid UTF-8",
            snapshot.file.path().display()
        )
    })
}

fn previous_mode(snapshot: &ManagedFileSnapshot, missing_mode: u32) -> u32 {
    match snapshot.previous {
        PreviousManagedFile::Missing => missing_mode,
        PreviousManagedFile::Present { mode, .. } => mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Focus;
    use chrono::{Duration, Utc};
    use std::fs;
    use std::process::Command;
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
            provider_session_id: None,
            confirmation_token: Some("rucksack-test-0123456789abcdef".to_owned()),
            cleanup_pending: false,
            activated_at,
            expires_at: activated_at + Duration::minutes(30),
            policy: "Run every workload required by the current task.".to_owned(),
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

    fn owned_rule(body: &str) -> String {
        format!(
            "---\ndescription: existing rule\nalwaysApply: true\n---\n\n<!-- {MANAGED_MARKER} -->\n\n{body}\n"
        )
    }

    fn owned_command(body: &str) -> String {
        format!("<!-- {MANAGED_MARKER} -->\n\n{body}\n")
    }

    #[cfg(unix)]
    fn replace_directory_with_symlink(directory: &Path, held: &Path, external: &Path) {
        use std::os::unix::fs::symlink;

        fs::rename(directory, held).unwrap();
        symlink(external, directory).unwrap();
    }

    #[test]
    fn cursor_rule_round_trip_preserves_unrelated_git_excludes() {
        let directory = tempdir().unwrap();
        let project = directory.path();
        initialize_git_repository(project);
        let exclude = project.join(".git/info/exclude");
        fs::write(&exclude, "# keep-this-entry\n/local-only\n").unwrap();

        let policy = active_policy(project);
        activate_cursor_rule(project, &policy).unwrap();
        activate_cursor_rule(project, &policy).unwrap();

        assert!(cursor_rule_path(project).exists());
        assert!(cursor_command_path(project).exists());
        let installed_command = fs::read_to_string(cursor_command_path(project)).unwrap();
        assert!(installed_command.contains("existing instructions and active Cursor configuration"));
        assert!(installed_command.contains(&policy.policy));
        assert!(installed_command.contains("cleanup_pending"));
        assert!(!installed_command.contains(CURSOR_POLICY_PLACEHOLDER));
        assert!(!installed_command.contains("bounded task"));
        assert!(!installed_command.contains("without widening scope"));
        let installed_rule = fs::read_to_string(cursor_rule_path(project)).unwrap();
        assert!(installed_rule.contains("cleanup_pending"));
        assert!(installed_rule.contains("expires_at"));
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
        let exclude = project.join(".git/info/exclude");
        fs::write(&exclude, format!("{EXCLUDE_BEGIN}\n")).unwrap();

        let error = activate_cursor_rule(project, &active_policy(project)).unwrap_err();

        assert!(error.to_string().contains("incomplete or duplicate"));
        assert!(!cursor_rule_path(project).exists());
        assert!(!cursor_command_path(project).exists());
    }

    #[test]
    fn activation_failure_restores_both_cursor_files() {
        let directory = tempdir().unwrap();
        let project = directory.path();
        initialize_git_repository(project);

        let rule = cursor_rule_path(project);
        let command = cursor_command_path(project);
        fs::create_dir_all(rule.parent().unwrap()).unwrap();
        fs::create_dir_all(command.parent().unwrap()).unwrap();
        let original_rule = owned_rule("keep the original rule");
        let original_command = owned_command("keep the original command");
        fs::write(&rule, &original_rule).unwrap();
        fs::write(&command, &original_command).unwrap();

        let git_info = project.join(".git/info");
        fs::remove_dir_all(&git_info).unwrap();
        fs::write(&git_info, "not a directory").unwrap();

        let error = activate_cursor_rule(project, &active_policy(project)).unwrap_err();

        assert!(error.to_string().contains("not a directory"));
        assert_eq!(fs::read_to_string(rule).unwrap(), original_rule);
        assert_eq!(fs::read_to_string(command).unwrap(), original_command);
    }

    #[test]
    fn activation_failure_removes_new_cursor_files() {
        let directory = tempdir().unwrap();
        let project = directory.path();
        initialize_git_repository(project);

        let git_info = project.join(".git/info");
        fs::remove_dir_all(&git_info).unwrap();
        fs::write(&git_info, "not a directory").unwrap();

        let error = activate_cursor_rule(project, &active_policy(project)).unwrap_err();

        assert!(error.to_string().contains("not a directory"));
        assert!(!cursor_rule_path(project).exists());
        assert!(!cursor_command_path(project).exists());
    }

    #[cfg(unix)]
    #[test]
    fn rollback_restores_git_exclude_bytes_and_mode() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let project = directory.path();
        initialize_git_repository(project);
        let rule = cursor_rule_path(project);
        let command = cursor_command_path(project);
        let exclude = project.join(".git/info/exclude");
        let original = b"# preserve these bytes\n/local-only\n";
        fs::write(&exclude, original).unwrap();
        fs::set_permissions(&exclude, fs::Permissions::from_mode(0o640)).unwrap();
        let mut managed_files = prepare_cursor_files(project, &rule, &command).unwrap();

        crate::files::atomic_write(&exclude, b"changed\n", 0o600).unwrap();
        let rollback_errors = restore_cursor_files(&mut managed_files);

        assert!(rollback_errors.is_empty(), "{rollback_errors:?}");
        assert_eq!(fs::read(&exclude).unwrap(), original);
        assert_eq!(
            fs::metadata(&exclude).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[cfg(unix)]
    #[test]
    fn activation_parent_swaps_do_not_write_external_files_and_rollback_stays_anchored() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let project = directory.path();
        initialize_git_repository(project);
        let rule = cursor_rule_path(project);
        let command = cursor_command_path(project);
        let exclude = project.join(".git/info/exclude");
        fs::create_dir_all(rule.parent().unwrap()).unwrap();
        fs::create_dir_all(command.parent().unwrap()).unwrap();
        let original_rule = owned_rule("original rule");
        let original_command = owned_command("original command");
        let original_exclude = b"# original exclude\n";
        fs::write(&rule, &original_rule).unwrap();
        fs::write(&command, &original_command).unwrap();
        fs::write(&exclude, original_exclude).unwrap();
        fs::set_permissions(&exclude, fs::Permissions::from_mode(0o640)).unwrap();

        let external = tempdir().unwrap();
        let external_rules = external.path().join("rules");
        let external_commands = external.path().join("commands");
        let external_info = external.path().join("info");
        fs::create_dir_all(&external_rules).unwrap();
        fs::create_dir_all(&external_commands).unwrap();
        fs::create_dir_all(&external_info).unwrap();
        let external_rule = external_rules.join("rucksack-commute.mdc");
        let external_command = external_commands.join("commute-mode.md");
        let external_exclude = external_info.join("exclude");
        fs::write(&external_rule, b"external rule\n").unwrap();
        fs::write(&external_command, b"external command\n").unwrap();
        fs::write(&external_exclude, b"external exclude\n").unwrap();

        let held_rules = project.join(".cursor/rules-held");
        let held_commands = project.join(".cursor/commands-held");
        let held_info = project.join(".git/info-held");
        let error = activate_cursor_rule_after_prepare(project, &active_policy(project), || {
            replace_directory_with_symlink(
                &project.join(".cursor/rules"),
                &held_rules,
                &external_rules,
            );
            replace_directory_with_symlink(
                &project.join(".cursor/commands"),
                &held_commands,
                &external_commands,
            );
            replace_directory_with_symlink(&project.join(".git/info"), &held_info, &external_info);
            Ok(())
        })
        .unwrap_err();

        assert!(error.to_string().contains("symlinked"));
        assert_eq!(fs::read(&external_rule).unwrap(), b"external rule\n");
        assert_eq!(fs::read(&external_command).unwrap(), b"external command\n");
        assert_eq!(fs::read(&external_exclude).unwrap(), b"external exclude\n");
        assert_eq!(
            fs::read_to_string(held_rules.join("rucksack-commute.mdc")).unwrap(),
            original_rule
        );
        assert_eq!(
            fs::read_to_string(held_commands.join("commute-mode.md")).unwrap(),
            original_command
        );
        let restored_exclude = held_info.join("exclude");
        assert_eq!(fs::read(&restored_exclude).unwrap(), original_exclude);
        assert_eq!(
            fs::metadata(restored_exclude).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[cfg(unix)]
    #[test]
    fn deactivation_parent_swaps_do_not_remove_external_files() {
        let directory = tempdir().unwrap();
        let project = directory.path();
        initialize_git_repository(project);
        let exclude = project.join(".git/info/exclude");
        fs::write(&exclude, b"# keep\n").unwrap();
        activate_cursor_rule(project, &active_policy(project)).unwrap();
        let installed_rule = fs::read(cursor_rule_path(project)).unwrap();
        let installed_command = fs::read(cursor_command_path(project)).unwrap();
        let installed_exclude = fs::read(&exclude).unwrap();

        let external = tempdir().unwrap();
        let external_rules = external.path().join("rules");
        let external_commands = external.path().join("commands");
        let external_info = external.path().join("info");
        fs::create_dir_all(&external_rules).unwrap();
        fs::create_dir_all(&external_commands).unwrap();
        fs::create_dir_all(&external_info).unwrap();
        let external_rule = external_rules.join("rucksack-commute.mdc");
        let external_command = external_commands.join("commute-mode.md");
        let external_exclude = external_info.join("exclude");
        fs::write(&external_rule, b"external rule\n").unwrap();
        fs::write(&external_command, b"external command\n").unwrap();
        fs::write(&external_exclude, b"external exclude\n").unwrap();

        let held_rules = project.join(".cursor/rules-held");
        let held_commands = project.join(".cursor/commands-held");
        let held_info = project.join(".git/info-held");
        let error = deactivate_cursor_rule_after_prepare(project, || {
            replace_directory_with_symlink(
                &project.join(".cursor/rules"),
                &held_rules,
                &external_rules,
            );
            replace_directory_with_symlink(
                &project.join(".cursor/commands"),
                &held_commands,
                &external_commands,
            );
            replace_directory_with_symlink(&project.join(".git/info"), &held_info, &external_info);
            Ok(())
        })
        .unwrap_err();

        assert!(error.to_string().contains("symlinked"));
        assert_eq!(fs::read(&external_rule).unwrap(), b"external rule\n");
        assert_eq!(fs::read(&external_command).unwrap(), b"external command\n");
        assert_eq!(fs::read(&external_exclude).unwrap(), b"external exclude\n");
        assert_eq!(
            fs::read(held_rules.join("rucksack-commute.mdc")).unwrap(),
            installed_rule
        );
        assert_eq!(
            fs::read(held_commands.join("commute-mode.md")).unwrap(),
            installed_command
        );
        assert_eq!(
            fs::read(held_info.join("exclude")).unwrap(),
            installed_exclude
        );
    }

    #[test]
    fn git_file_indirection_blocks_activation_before_mutating_any_files() {
        let directory = tempdir().unwrap();
        let project = directory.path();
        let external = tempdir().unwrap();
        fs::create_dir_all(external.path().join("info")).unwrap();
        let external_exclude = external.path().join("info/exclude");
        fs::write(&external_exclude, b"external exclude\n").unwrap();
        fs::write(
            project.join(".git"),
            format!("gitdir: {}\n", external.path().display()),
        )
        .unwrap();

        let error = activate_cursor_rule(project, &active_policy(project)).unwrap_err();

        assert!(error.to_string().contains("Git worktree root"));
        assert_eq!(fs::read(&external_exclude).unwrap(), b"external exclude\n");
        assert!(!cursor_rule_path(project).exists());
        assert!(!cursor_command_path(project).exists());
    }

    #[test]
    fn nested_git_project_blocks_activation_before_mutating_any_files() {
        let directory = tempdir().unwrap();
        initialize_git_repository(directory.path());
        let project = directory.path().join("nested/project");
        fs::create_dir_all(&project).unwrap();

        let error = activate_cursor_rule(&project, &active_policy(&project)).unwrap_err();

        assert!(error.to_string().contains("Git worktree root"));
        assert!(!cursor_rule_path(&project).exists());
        assert!(!cursor_command_path(&project).exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_rule_parent_blocks_activation_without_external_write() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let project = directory.path();
        let external = tempdir().unwrap();
        fs::create_dir_all(project.join(".cursor")).unwrap();
        symlink(external.path(), project.join(".cursor/rules")).unwrap();

        let error = activate_cursor_rule(project, &active_policy(project)).unwrap_err();

        assert!(error.to_string().contains("symlinked path"));
        assert!(!external.path().join("rucksack-commute.mdc").exists());
        assert!(!cursor_command_path(project).exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_rule_parent_blocks_deactivation_without_external_remove() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let project = directory.path();
        let external = tempdir().unwrap();
        let external_rule = external.path().join("rucksack-commute.mdc");
        fs::write(&external_rule, owned_rule("keep this external rule")).unwrap();
        fs::create_dir_all(project.join(".cursor")).unwrap();
        symlink(external.path(), project.join(".cursor/rules")).unwrap();

        let error = deactivate_cursor_rule(project).unwrap_err();

        assert!(error.to_string().contains("symlinked path"));
        assert!(external_rule.exists());
    }
}
