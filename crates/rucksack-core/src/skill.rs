use crate::config::AdaptersConfig;
use crate::files::{atomic_write, reject_symlink, remove_if_exists};
use crate::paths::AppPaths;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// Marks a file as rucksack's, so uninstalling never deletes something the user wrote.
pub const MANAGED_MARKER: &str = "rucksack-managed";

/// The skill, marker and all.
///
/// The marker lives just *below* the YAML frontmatter, and that position is load-bearing rather
/// than cosmetic: an agent reads the `description` — the sentences that decide whether this skill
/// is offered for "I'm leaving" at all — only when the frontmatter is the very first thing in the
/// file. A comment above it costs the skill its trigger.
const SKILL: &str = include_str!("../../../assets/adapters/shared/rucksack/SKILL.md");

/// How far in to look for the marker: past any frontmatter, and no further, so prose mentioning
/// rucksack cannot be mistaken for rucksack's own handiwork.
const MARKER_SEARCH_LINES: usize = 20;

/// Backups a previous release left beside its skill, named for the marker.
const BACKUP_SUFFIX: &str = ".bak";

fn marker_comment() -> String {
    format!("<!-- {MANAGED_MARKER} -->")
}

/// Did rucksack write this?
fn is_managed(text: &str) -> bool {
    let marker = marker_comment();
    text.lines()
        .take(MARKER_SEARCH_LINES)
        .any(|line| line.trim() == marker)
}

/// Teach the installed coding agents that `rucksack` exists.
///
/// Best-effort by design: the skill only makes "pack my Mac" work as a sentence inside a
/// conversation, so failing to write it must never stop the Mac from staying awake. Skill
/// directories that do not exist are skipped — that agent is not installed — and so is any agent
/// switched off under `[adapters]`.
///
/// Clearing out an earlier release's files is not gated: those are rucksack's own leftovers, and a
/// flag turned off today is no reason to leave one behind.
pub fn install(paths: &AppPaths, adapters: &AdaptersConfig) -> Result<()> {
    for skill in [
        adapters.codex.then_some(&paths.codex_skill),
        adapters.claude.then_some(&paths.claude_skill),
        adapters.cursor.then_some(&paths.cursor_skill),
    ]
    .into_iter()
    .flatten()
    {
        let Some(agent_root) = skill.parent().and_then(Path::parent) else {
            continue;
        };
        if !agent_root.exists() {
            continue;
        }
        write_if_owned(skill)?;
    }
    for legacy in [paths.legacy_codex_skill(), paths.legacy_claude_skill()] {
        remove_if_owned(&legacy)?;
    }
    Ok(())
}

fn write_if_owned(skill: &Path) -> Result<()> {
    reject_symlink(skill)?;
    match fs::read_to_string(skill) {
        Ok(current) if current == SKILL => return Ok(()),
        Ok(current) if !is_managed(&current) => {
            // Someone else's skill lives here. Refusing is not ceremony: overwriting it would
            // destroy work rucksack did not create.
            anyhow::bail!(
                "Refusing to overwrite {}, which rucksack does not own.",
                skill.display()
            );
        }
        _ => {}
    }
    atomic_write(skill, SKILL.as_bytes(), 0o600)
        .with_context(|| format!("Could not write {}", skill.display()))
}

/// Delete a skill from releases that named this `commute-mode`.
///
/// `commute-mode` named an internal state rather than the product, so nobody could guess it.
/// Leaving it behind would give one product two skills — and leaving the *directory* behind is
/// almost as bad, because the name is the confusing part.
fn remove_if_owned(skill: &Path) -> Result<()> {
    let Some(directory) = skill.parent() else {
        return Ok(());
    };
    if !directory.exists() {
        return Ok(());
    }
    if skill.exists() {
        reject_symlink(skill)?;
        let current = fs::read_to_string(skill)
            .with_context(|| format!("Could not read {}", skill.display()))?;
        if !is_managed(&current) {
            return Ok(());
        }
        remove_if_exists(skill)?;
    }
    remove_own_backups(directory)?;
    // Succeeds only when nothing rucksack does not own was left behind.
    let _ = fs::remove_dir(directory);
    Ok(())
}

/// Remove the timestamped backups an earlier release wrote next to its skill.
///
/// They carry the marker in their filename, so rucksack can recognise its own without reading
/// them. Anything else in the directory is left alone, which is what keeps the directory itself.
fn remove_own_backups(directory: &Path) -> Result<()> {
    let infix = format!(".{MANAGED_MARKER}.");
    let entries = fs::read_dir(directory)
        .with_context(|| format!("Could not read {}", directory.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("Could not read {}", directory.display()))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.contains(&infix) && name.ends_with(BACKUP_SUFFIX) {
            remove_if_exists(&entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn paths(home: &Path) -> AppPaths {
        let data = home.join("data");
        AppPaths {
            home: home.to_path_buf(),
            data_dir: data.clone(),
            config_file: data.join("config.toml"),
            session_file: data.join("session.json"),
            log_dir: home.join("logs"),
            daemon_log: home.join("logs/daemon.log"),
            codex_skill: home.join(".agents/skills/rucksack/SKILL.md"),
            claude_skill: home.join(".claude/skills/rucksack/SKILL.md"),
            cursor_skill: home.join(".cursor/skills-cursor/rucksack/SKILL.md"),
        }
    }

    fn seed(path: &PathBuf, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    /// The whole point of the marker's position: an agent that cannot parse the frontmatter never
    /// learns what this skill is for, so the skill exists and never fires.
    #[test]
    fn the_frontmatter_comes_before_the_marker() {
        assert!(SKILL.starts_with("---\n"), "frontmatter must open the file");
        assert!(is_managed(SKILL), "rucksack must recognise its own skill");

        let description = SKILL
            .lines()
            .find(|line| line.starts_with("description:"))
            .expect("the skill declares a description");
        assert!(description.contains("I'm leaving"));
    }

    #[test]
    fn a_file_without_the_marker_is_not_ours() {
        assert!(!is_managed("---\nname: mine\n---\n\n# my own notes\n"));
    }

    /// Prose far enough down the file is not a claim of ownership.
    #[test]
    fn the_marker_only_counts_near_the_top() {
        let mut late = "filler\n".repeat(MARKER_SEARCH_LINES);
        late.push_str(&marker_comment());
        assert!(!is_managed(&late));
    }

    #[test]
    fn installs_only_where_the_agent_already_lives() {
        let home = tempdir().unwrap();
        let paths = paths(home.path());
        fs::create_dir_all(home.path().join(".agents/skills")).unwrap();

        install(&paths, &AdaptersConfig::default()).unwrap();

        assert!(paths.codex_skill.exists());
        assert!(!paths.claude_skill.exists(), "Claude Code is not installed");
        assert!(!paths.cursor_skill.exists(), "Cursor is not installed");
    }

    /// An agent switched off is not touched, even though it is installed and would have been.
    #[test]
    fn skips_an_agent_switched_off_in_the_config() {
        let home = tempdir().unwrap();
        let paths = paths(home.path());
        fs::create_dir_all(home.path().join(".agents/skills")).unwrap();
        fs::create_dir_all(home.path().join(".claude/skills")).unwrap();
        let adapters = AdaptersConfig {
            codex: false,
            ..AdaptersConfig::default()
        };

        install(&paths, &adapters).unwrap();

        assert!(!paths.codex_skill.exists(), "Codex is switched off");
        assert!(paths.claude_skill.exists(), "Claude Code is still on");
    }

    #[test]
    fn installs_for_cursor_when_cursor_is_there() {
        let home = tempdir().unwrap();
        let paths = paths(home.path());
        fs::create_dir_all(home.path().join(".cursor/skills-cursor")).unwrap();

        install(&paths, &AdaptersConfig::default()).unwrap();

        assert!(paths.cursor_skill.exists());
        assert_eq!(fs::read_to_string(&paths.cursor_skill).unwrap(), SKILL);
    }

    #[test]
    fn replaces_the_legacy_commute_mode_skill() {
        let home = tempdir().unwrap();
        let paths = paths(home.path());
        fs::create_dir_all(home.path().join(".agents/skills")).unwrap();
        seed(
            &paths.legacy_codex_skill(),
            "<!-- rucksack-managed -->\nold\n",
        );

        install(&paths, &AdaptersConfig::default()).unwrap();

        assert!(paths.codex_skill.exists());
        assert!(!paths.legacy_codex_skill().exists());
    }

    /// The directory is the confusing part, so its own leftovers must not keep it alive.
    #[test]
    fn takes_its_own_backups_with_it() {
        let home = tempdir().unwrap();
        let paths = paths(home.path());
        let legacy = paths.legacy_codex_skill();
        let directory = legacy.parent().unwrap().to_path_buf();
        seed(&legacy, "<!-- rucksack-managed -->\nold\n");
        seed(
            &directory.join("SKILL.md.rucksack-managed.20260724T141740Z.bak"),
            "older\n",
        );

        install(&paths, &AdaptersConfig::default()).unwrap();

        assert!(!directory.exists(), "the commute-mode directory is gone");
    }

    /// A backup with no skill beside it is still rucksack's to clean up.
    #[test]
    fn clears_a_directory_of_backups_alone() {
        let home = tempdir().unwrap();
        let paths = paths(home.path());
        let directory = paths.legacy_claude_skill().parent().unwrap().to_path_buf();
        seed(
            &directory.join("SKILL.md.rucksack-managed.20260725T085326Z.bak"),
            "older\n",
        );

        install(&paths, &AdaptersConfig::default()).unwrap();

        assert!(!directory.exists());
    }

    #[test]
    fn never_deletes_a_commute_mode_skill_someone_else_wrote() {
        let home = tempdir().unwrap();
        let paths = paths(home.path());
        seed(&paths.legacy_codex_skill(), "my own notes\n");

        install(&paths, &AdaptersConfig::default()).unwrap();

        assert!(paths.legacy_codex_skill().exists());
    }

    /// Rucksack's own backup goes; a file it does not recognise keeps the directory.
    #[test]
    fn leaves_a_directory_holding_someone_elses_file() {
        let home = tempdir().unwrap();
        let paths = paths(home.path());
        let legacy = paths.legacy_codex_skill();
        let directory = legacy.parent().unwrap().to_path_buf();
        seed(&legacy, "<!-- rucksack-managed -->\nold\n");
        seed(&directory.join("notes.md"), "mine\n");

        install(&paths, &AdaptersConfig::default()).unwrap();

        assert!(directory.join("notes.md").exists());
        assert!(directory.exists());
    }

    #[test]
    fn refuses_to_overwrite_an_unowned_skill() {
        let home = tempdir().unwrap();
        let paths = paths(home.path());
        fs::create_dir_all(paths.codex_skill.parent().unwrap()).unwrap();
        fs::write(&paths.codex_skill, "hand written\n").unwrap();

        assert!(install(&paths, &AdaptersConfig::default()).is_err());
        assert_eq!(
            fs::read_to_string(&paths.codex_skill).unwrap(),
            "hand written\n"
        );
    }
}
