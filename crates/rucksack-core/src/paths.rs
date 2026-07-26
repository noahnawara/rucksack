use anyhow::{Context, Result};
use std::path::PathBuf;

/// Everywhere rucksack keeps state, on this Mac.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub home: PathBuf,
    pub data_dir: PathBuf,
    pub config_file: PathBuf,
    pub session_file: PathBuf,
    pub log_dir: PathBuf,
    pub daemon_log: PathBuf,
    pub codex_skill: PathBuf,
    pub claude_skill: PathBuf,
    pub cursor_skill: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self> {
        let home =
            dirs::home_dir().context("Could not determine the current user's home directory")?;
        let base = dirs::data_dir()
            .unwrap_or_else(|| home.join(".local/share"))
            .join("Rucksack");
        let log_dir = log_directory(&home, &base, dirs::data_local_dir());

        Ok(Self {
            config_file: base.join("config.toml"),
            session_file: base.join("session.json"),
            daemon_log: log_dir.join("daemon.log"),
            codex_skill: home.join(".agents/skills/rucksack/SKILL.md"),
            claude_skill: home.join(".claude/skills/rucksack/SKILL.md"),
            cursor_skill: home.join(".cursor/skills-cursor/rucksack/SKILL.md"),
            home,
            data_dir: base,
            log_dir,
        })
    }

    /// Serialises `pack` and `unpack` against each other.
    pub fn terminal_lock_file(&self) -> PathBuf {
        self.session_file.with_extension("terminal.lock")
    }

    /// Where a spawned Codex Remote Control writes, kept apart from the watcher's own log so a
    /// failure in one is never read as a failure in the other.
    pub fn remote_control_log(&self) -> PathBuf {
        self.log_dir.join("remote-control.log")
    }

    /// Records that rucksack has asked about starring the project, so it only ever asks once.
    pub fn star_prompt_marker(&self) -> PathBuf {
        self.data_dir.join("asked-about-star")
    }

    pub fn legacy_codex_skill(&self) -> PathBuf {
        self.home.join(".agents/skills/commute-mode/SKILL.md")
    }

    pub fn legacy_claude_skill(&self) -> PathBuf {
        self.home.join(".claude/skills/commute-mode/SKILL.md")
    }
}

fn log_directory(
    home: &std::path::Path,
    data_dir: &std::path::Path,
    data_local_dir: Option<PathBuf>,
) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let _ = (data_dir, data_local_dir);
        home.join("Library/Logs/Rucksack")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = home;
        data_local_dir
            .map(|root| root.join("Rucksack/Logs"))
            .unwrap_or_else(|| data_dir.join("Logs"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn log_directory_keeps_logs_inside_the_rucksack_namespace() {
        let path = log_directory(
            Path::new("/Users/test"),
            Path::new("/data/Rucksack"),
            Some(PathBuf::from("/local-data")),
        );
        #[cfg(target_os = "macos")]
        assert_eq!(path, Path::new("/Users/test/Library/Logs/Rucksack"));
        #[cfg(not(target_os = "macos"))]
        assert_eq!(path, Path::new("/local-data/Rucksack/Logs"));
    }
}
