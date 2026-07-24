use anyhow::{Context, Result};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub home: PathBuf,
    pub data_dir: PathBuf,
    pub config_file: PathBuf,
    pub session_file: PathBuf,
    pub policy_file: PathBuf,
    pub adapter_manifest_file: PathBuf,
    pub log_dir: PathBuf,
    pub daemon_log: PathBuf,
    pub codex_hooks: PathBuf,
    pub codex_skill: PathBuf,
    pub claude_settings: PathBuf,
    pub claude_skill: PathBuf,
    pub cursor_hooks: PathBuf,
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
            home: home.clone(),
            data_dir: base.clone(),
            config_file: base.join("config.toml"),
            session_file: base.join("session.json"),
            policy_file: base.join("active-policy.json"),
            adapter_manifest_file: base.join("adapters.json"),
            log_dir: log_dir.clone(),
            daemon_log: log_dir.join("daemon.log"),
            codex_hooks: home.join(".codex/hooks.json"),
            codex_skill: home.join(".agents/skills/commute-mode/SKILL.md"),
            claude_settings: home.join(".claude/settings.json"),
            claude_skill: home.join(".claude/skills/commute-mode/SKILL.md"),
            cursor_hooks: home.join(".cursor/hooks.json"),
        })
    }
}

fn log_directory(
    home: &std::path::Path,
    data_dir: &std::path::Path,
    data_local_dir: Option<PathBuf>,
) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let _ = data_dir;
        let _ = data_local_dir;
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
