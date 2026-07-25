use crate::files::{atomic_write, reject_symlink, remove_if_exists};
use crate::paths::AppPaths;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// Marks a file as rucksack's, so uninstalling never deletes something the user wrote.
pub const MANAGED_MARKER: &str = "rucksack-managed";

const SKILL: &str = include_str!("../../../assets/adapters/shared/rucksack/SKILL.md");

fn managed_text() -> String {
    format!("<!-- {MANAGED_MARKER} -->\n{SKILL}")
}

fn managed_prefix() -> String {
    format!("<!-- {MANAGED_MARKER} -->\n")
}

/// Teach the installed coding agents that `rucksack` exists.
///
/// Best-effort by design: the skill only makes "pack my Mac" work as a sentence inside a
/// conversation, so failing to write it must never stop the Mac from staying awake. Skill
/// directories that do not exist are skipped — that agent is not installed.
pub fn install(paths: &AppPaths) -> Result<()> {
    for skill in [&paths.codex_skill, &paths.claude_skill] {
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
    let managed = managed_text();
    match fs::read_to_string(skill) {
        Ok(current) if current == managed => return Ok(()),
        Ok(current) if !current.starts_with(&managed_prefix()) => {
            // Someone else's skill lives here. Refusing is not ceremony: overwriting it would
            // destroy work rucksack did not create.
            anyhow::bail!(
                "Refusing to overwrite {}, which rucksack does not own.",
                skill.display()
            );
        }
        _ => {}
    }
    atomic_write(skill, managed.as_bytes(), 0o600)
        .with_context(|| format!("Could not write {}", skill.display()))
}

/// Delete a skill from releases that named this `commute-mode`.
///
/// `commute-mode` named an internal state rather than the product, so nobody could guess it.
/// Leaving it behind would give one product two skills.
fn remove_if_owned(skill: &Path) -> Result<()> {
    if !skill.exists() {
        return Ok(());
    }
    reject_symlink(skill)?;
    let current =
        fs::read_to_string(skill).with_context(|| format!("Could not read {}", skill.display()))?;
    if !current.starts_with(&managed_prefix()) {
        return Ok(());
    }
    remove_if_exists(skill)?;
    if let Some(directory) = skill.parent() {
        // Succeeds only when rucksack's file was the only thing in it.
        let _ = fs::remove_dir(directory);
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
        }
    }

    fn seed_legacy(path: &PathBuf, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn installs_only_where_the_agent_already_lives() {
        let home = tempdir().unwrap();
        let paths = paths(home.path());
        fs::create_dir_all(home.path().join(".agents/skills")).unwrap();

        install(&paths).unwrap();

        assert!(paths.codex_skill.exists());
        assert!(!paths.claude_skill.exists(), "Claude Code is not installed");
    }

    #[test]
    fn replaces_the_legacy_commute_mode_skill() {
        let home = tempdir().unwrap();
        let paths = paths(home.path());
        fs::create_dir_all(home.path().join(".agents/skills")).unwrap();
        seed_legacy(
            &paths.legacy_codex_skill(),
            "<!-- rucksack-managed -->\nold\n",
        );

        install(&paths).unwrap();

        assert!(paths.codex_skill.exists());
        assert!(!paths.legacy_codex_skill().exists());
    }

    #[test]
    fn never_deletes_a_commute_mode_skill_someone_else_wrote() {
        let home = tempdir().unwrap();
        let paths = paths(home.path());
        seed_legacy(&paths.legacy_codex_skill(), "my own notes\n");

        install(&paths).unwrap();

        assert!(paths.legacy_codex_skill().exists());
    }

    #[test]
    fn refuses_to_overwrite_an_unowned_skill() {
        let home = tempdir().unwrap();
        let paths = paths(home.path());
        fs::create_dir_all(paths.codex_skill.parent().unwrap()).unwrap();
        fs::write(&paths.codex_skill, "hand written\n").unwrap();

        assert!(install(&paths).is_err());
        assert_eq!(
            fs::read_to_string(&paths.codex_skill).unwrap(),
            "hand written\n"
        );
    }
}
