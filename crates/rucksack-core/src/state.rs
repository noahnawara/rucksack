use crate::files::{atomic_write_json, read_json_if_exists, remove_if_exists, with_advisory_lock};
use crate::paths::AppPaths;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

const SESSION_STATE_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionPhase {
    Ready,
    Active,
    Released,
}

/// A lease on this Mac staying awake.
///
/// The lease belongs to the host, not to a conversation: closing the lid affects every process, so
/// no agent, task, or provider session is part of its identity. Every running task benefits, and a
/// task finishing does not end it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub version: u32,
    pub id: Uuid,
    pub lease_id: Uuid,
    pub phase: SessionPhase,
    pub started_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub daemon_pid: Option<u32>,
    pub hotspot: Option<String>,
    pub route_interface: Option<String>,
    pub battery_percent: Option<u8>,
    pub online: bool,
    pub last_event: Option<String>,
    pub release_reason: Option<String>,
}

impl SessionState {
    pub fn new(lease_id: Uuid, started_at: DateTime<Utc>, expires_at: DateTime<Utc>) -> Self {
        Self {
            version: SESSION_STATE_VERSION,
            id: Uuid::new_v4(),
            lease_id,
            phase: SessionPhase::Ready,
            started_at,
            expires_at,
            ended_at: None,
            last_heartbeat_at: None,
            daemon_pid: None,
            hotspot: None,
            route_interface: None,
            battery_percent: None,
            online: true,
            last_event: None,
            release_reason: None,
        }
    }

    /// Load the session, or `None` when there is none.
    ///
    /// A file rucksack cannot parse is reported as an error so callers can decide; `unpack` treats
    /// it as "no session" and deletes it, because state nobody can read is not worth blocking the
    /// one command that restores normal sleep.
    pub fn load(paths: &AppPaths) -> Result<Option<Self>> {
        let session = read_json_if_exists::<Self>(&paths.session_file)?;
        if let Some(session) = &session {
            if session.version != SESSION_STATE_VERSION {
                anyhow::bail!(
                    "Unsupported session state version {} in {}; expected {SESSION_STATE_VERSION}",
                    session.version,
                    paths.session_file.display()
                );
            }
        }
        Ok(session)
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        with_advisory_lock(&lock_path(paths), || {
            atomic_write_json(&paths.session_file, self)
        })
    }

    /// Apply a change to whatever session is on disk, if it is still the same one.
    pub fn update(
        paths: &AppPaths,
        expected_id: Uuid,
        change: impl FnOnce(&mut Self),
    ) -> Result<Option<Self>> {
        with_advisory_lock(&lock_path(paths), || {
            let Some(mut current) = read_json_if_exists::<Self>(&paths.session_file)? else {
                return Ok(None);
            };
            if current.id != expected_id {
                return Ok(None);
            }
            change(&mut current);
            atomic_write_json(&paths.session_file, &current)?;
            Ok(Some(current))
        })
    }

    pub fn clear(paths: &AppPaths) -> Result<()> {
        with_advisory_lock(&lock_path(paths), || remove_if_exists(&paths.session_file))
    }

    pub fn remaining_minutes(&self, now: DateTime<Utc>) -> u64 {
        (self.expires_at - now).num_minutes().max(0) as u64
    }

    pub fn is_holding_a_lease(&self) -> bool {
        self.phase != SessionPhase::Released
    }
}

fn lock_path(paths: &AppPaths) -> PathBuf {
    paths.session_file.with_extension("lock")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn paths(root: &std::path::Path) -> AppPaths {
        let data = root.join("data");
        AppPaths {
            home: root.to_path_buf(),
            data_dir: data.clone(),
            config_file: data.join("config.toml"),
            session_file: data.join("session.json"),
            log_dir: root.join("logs"),
            daemon_log: root.join("logs/daemon.log"),
            codex_skill: root.join(".agents/skills/rucksack/SKILL.md"),
            claude_skill: root.join(".claude/skills/rucksack/SKILL.md"),
        }
    }

    fn session() -> SessionState {
        let now = Utc::now();
        SessionState::new(Uuid::new_v4(), now, now + chrono::Duration::hours(1))
    }

    #[test]
    fn saves_loads_and_clears() {
        let home = tempdir().unwrap();
        let paths = paths(home.path());
        let original = session();
        original.save(&paths).unwrap();

        let loaded = SessionState::load(&paths).unwrap().unwrap();
        assert_eq!(loaded.id, original.id);

        SessionState::clear(&paths).unwrap();
        assert!(SessionState::load(&paths).unwrap().is_none());
    }

    #[test]
    fn an_update_for_a_different_session_is_ignored() {
        let home = tempdir().unwrap();
        let paths = paths(home.path());
        session().save(&paths).unwrap();

        let outcome = SessionState::update(&paths, Uuid::new_v4(), |session| {
            session.phase = SessionPhase::Released;
        })
        .unwrap();

        assert!(outcome.is_none());
        assert_eq!(
            SessionState::load(&paths).unwrap().unwrap().phase,
            SessionPhase::Ready
        );
    }

    #[test]
    fn a_state_file_from_another_version_is_rejected_rather_than_guessed_at() {
        let home = tempdir().unwrap();
        let paths = paths(home.path());
        let mut value = serde_json::to_value(session()).unwrap();
        value["version"] = serde_json::Value::from(99);
        fs::create_dir_all(paths.session_file.parent().unwrap()).unwrap();
        fs::write(
            &paths.session_file,
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();

        assert!(SessionState::load(&paths)
            .unwrap_err()
            .to_string()
            .contains("version 99"));
    }
}
