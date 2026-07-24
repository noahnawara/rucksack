use crate::agent::AgentKind;
use crate::files::{atomic_write_json, read_json_if_exists, remove_if_exists, with_advisory_lock};
use crate::paths::AppPaths;
use crate::policy::Focus;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;
use uuid::Uuid;

const SESSION_STATE_VERSION: u32 = 1;
const ACTIVE_POLICY_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionPhase {
    Preflight,
    PolicyActive,
    WaitingForHotspot,
    WaitingForUnplug,
    Ready,
    Active,
    WaitingForApproval,
    WaitingForInput,
    TemporarilyOffline,
    IdleGrace,
    Completed,
    Releasing,
    Released,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub version: u32,
    #[serde(default)]
    pub revision: u64,
    pub id: Uuid,
    pub lease_id: Uuid,
    pub owner_uid: u32,
    pub agent: AgentKind,
    pub project_dir: PathBuf,
    pub focus: Focus,
    pub phase: SessionPhase,
    pub started_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub daemon_pid: Option<u32>,
    pub expected_hotspot_ssid: Option<String>,
    pub observed_hotspot_ssid: Option<String>,
    #[serde(default)]
    pub commute_route_interface: Option<String>,
    #[serde(default)]
    pub commute_route_gateway: Option<String>,
    pub route_interface: Option<String>,
    pub battery_percent: Option<u8>,
    #[serde(default)]
    pub network_reachable: Option<bool>,
    #[serde(default)]
    pub network_outage_started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub phase_before_offline: Option<SessionPhase>,
    #[serde(default)]
    pub idle_grace_started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
    pub previous_sleep_disabled: Option<u8>,
    pub remote_owned_by_rucksack: bool,
    pub remote_pid: Option<u32>,
    pub remote_confirmed_by_user: bool,
    pub last_event: Option<String>,
    pub release_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivePolicy {
    pub version: u32,
    pub session_id: Uuid,
    pub agent: AgentKind,
    pub focus: Focus,
    pub project_dir: PathBuf,
    pub activated_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub policy: String,
}

#[derive(Debug, Error)]
#[error(
    "Session state changed before it could be saved (session {session_id}, expected revision {expected_revision}, observed session {observed_session_id:?} revision {observed_revision:?})"
)]
pub struct SessionStateWriteConflict {
    pub session_id: Uuid,
    pub expected_revision: u64,
    pub observed_session_id: Option<Uuid>,
    pub observed_revision: Option<u64>,
}

impl SessionState {
    pub fn load(paths: &AppPaths) -> Result<Option<Self>> {
        load_session_file(&paths.session_file)
    }

    pub fn save(&mut self, paths: &AppPaths) -> Result<()> {
        with_advisory_lock(&session_lock_path(paths), || {
            validate_session_version(self, &paths.session_file)?;
            let current = load_session_file(&paths.session_file)?;
            validate_write_revision(self, current.as_ref())?;
            let observed_revision = current.as_ref().map_or(0, |state| state.revision);
            let next_revision = observed_revision
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("Session state revision overflow"))?;
            let mut next = self.clone();
            next.revision = next_revision;
            atomic_write_json(&paths.session_file, &next)?;
            self.revision = next_revision;
            Ok(())
        })
    }

    pub fn update(
        paths: &AppPaths,
        expected_id: Uuid,
        transform: impl FnOnce(Self) -> Result<Self>,
    ) -> Result<Option<Self>> {
        Self::update_current(paths, |current| {
            if current.id != expected_id {
                anyhow::bail!(
                    "Session state identity changed during update: expected {}, observed {}",
                    expected_id,
                    current.id
                );
            }
            transform(current)
        })
    }

    pub fn update_current(
        paths: &AppPaths,
        transform: impl FnOnce(Self) -> Result<Self>,
    ) -> Result<Option<Self>> {
        with_advisory_lock(&session_lock_path(paths), || {
            let Some(current) = load_session_file(&paths.session_file)? else {
                return Ok(None);
            };
            let expected_id = current.id;
            let next_revision = current
                .revision
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("Session state revision overflow"))?;
            let mut next = transform(current)?;
            if next.id != expected_id {
                anyhow::bail!(
                    "Session state transform changed its identity from {} to {}",
                    expected_id,
                    next.id
                );
            }
            next.revision = next_revision;
            atomic_write_json(&paths.session_file, &next)?;
            Ok(Some(next))
        })
    }

    pub fn clear(paths: &AppPaths) -> Result<()> {
        with_advisory_lock(&session_lock_path(paths), || {
            remove_if_exists(&paths.session_file)
        })
    }

    pub fn remaining_minutes(&self, now: DateTime<Utc>) -> i64 {
        (self.expires_at - now).num_minutes().max(0)
    }
}

fn session_lock_path(paths: &AppPaths) -> PathBuf {
    paths.session_file.with_extension("lock")
}

fn load_session_file(path: &std::path::Path) -> Result<Option<SessionState>> {
    let session = read_json_if_exists::<SessionState>(path)?;
    if let Some(session) = &session {
        validate_session_version(session, path)?;
    }
    Ok(session)
}

fn validate_session_version(session: &SessionState, path: &std::path::Path) -> Result<()> {
    if session.version != SESSION_STATE_VERSION {
        anyhow::bail!(
            "Unsupported session state version {} in {}; expected version {}",
            session.version,
            path.display(),
            SESSION_STATE_VERSION
        );
    }
    Ok(())
}

fn validate_write_revision(pending: &SessionState, current: Option<&SessionState>) -> Result<()> {
    let write_is_current = match current {
        None => pending.revision == 0,
        Some(current) if current.id == pending.id => current.revision == pending.revision,
        Some(current) => {
            pending.revision == 0
                && matches!(current.phase, SessionPhase::Released | SessionPhase::Failed)
        }
    };
    if write_is_current {
        return Ok(());
    }
    Err(SessionStateWriteConflict {
        session_id: pending.id,
        expected_revision: pending.revision,
        observed_session_id: current.map(|state| state.id),
        observed_revision: current.map(|state| state.revision),
    }
    .into())
}

impl ActivePolicy {
    pub fn load(paths: &AppPaths) -> Result<Option<Self>> {
        let policy = read_json_if_exists::<Self>(&paths.policy_file)?;
        if let Some(policy) = &policy {
            if policy.version != ACTIVE_POLICY_VERSION {
                anyhow::bail!(
                    "Unsupported active policy version {} in {}; expected version {}",
                    policy.version,
                    paths.policy_file.display(),
                    ACTIVE_POLICY_VERSION
                );
            }
        }
        Ok(policy)
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        if self.version != ACTIVE_POLICY_VERSION {
            anyhow::bail!(
                "Cannot save active policy version {}; expected version {}",
                self.version,
                ACTIVE_POLICY_VERSION
            );
        }
        atomic_write_json(&paths.policy_file, self)
    }

    pub fn clear(paths: &AppPaths) -> Result<()> {
        remove_if_exists(&paths.policy_file)
    }

    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        now < self.expires_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use tempfile::tempdir;

    fn test_paths(root: &std::path::Path) -> AppPaths {
        let data_dir = root.join("data");
        let log_dir = root.join("logs");
        AppPaths {
            home: root.to_path_buf(),
            data_dir: data_dir.clone(),
            config_file: data_dir.join("config.toml"),
            session_file: data_dir.join("session.json"),
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
            version: SESSION_STATE_VERSION,
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
            battery_percent: Some(80),
            network_reachable: Some(true),
            network_outage_started_at: None,
            phase_before_offline: None,
            idle_grace_started_at: None,
            completed_at: None,
            previous_sleep_disabled: Some(0),
            remote_owned_by_rucksack: false,
            remote_pid: None,
            remote_confirmed_by_user: true,
            last_event: None,
            release_reason: None,
        }
    }

    fn sample_policy(session: &SessionState) -> ActivePolicy {
        ActivePolicy {
            version: ACTIVE_POLICY_VERSION,
            session_id: session.id,
            agent: session.agent,
            focus: session.focus,
            project_dir: session.project_dir.clone(),
            activated_at: session.started_at,
            expires_at: session.expires_at,
            policy: "Keep work bounded.".to_owned(),
        }
    }

    #[test]
    fn stale_heartbeat_cannot_overwrite_a_newer_hook_lifecycle_update() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let mut initial = sample_session();
        initial.save(&paths).unwrap();
        let mut stale_heartbeat = SessionState::load(&paths).unwrap().unwrap();

        SessionState::update(&paths, initial.id, |mut current| {
            current.phase = SessionPhase::Completed;
            current.last_event = Some("SessionEnd".to_owned());
            current.completed_at = Some(Utc::now());
            Ok(current)
        })
        .unwrap();

        stale_heartbeat.last_heartbeat_at = Some(Utc::now());
        stale_heartbeat.battery_percent = Some(75);
        let error = stale_heartbeat.save(&paths).unwrap_err();
        assert!(error.downcast_ref::<SessionStateWriteConflict>().is_some());

        let persisted = SessionState::load(&paths).unwrap().unwrap();
        assert_eq!(persisted.phase, SessionPhase::Completed);
        assert_eq!(persisted.last_event.as_deref(), Some("SessionEnd"));
        assert_eq!(persisted.battery_percent, Some(80));
        assert_eq!(persisted.revision, 2);
    }

    #[test]
    fn legacy_session_without_revision_loads_at_revision_zero() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let mut value = serde_json::to_value(sample_session()).unwrap();
        value.as_object_mut().unwrap().remove("revision");
        atomic_write_json(&paths.session_file, &value).unwrap();

        let loaded = SessionState::load(&paths).unwrap().unwrap();

        assert_eq!(loaded.revision, 0);
    }

    #[test]
    fn unknown_session_and_policy_versions_are_rejected() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let session = sample_session();
        let mut session_value = serde_json::to_value(&session).unwrap();
        session_value["version"] = Value::from(2);
        atomic_write_json(&paths.session_file, &session_value).unwrap();
        assert!(SessionState::load(&paths)
            .unwrap_err()
            .to_string()
            .contains("Unsupported session state version 2"));

        let mut policy_value = serde_json::to_value(sample_policy(&session)).unwrap();
        policy_value["version"] = Value::from(2);
        atomic_write_json(&paths.policy_file, &policy_value).unwrap();
        assert!(ActivePolicy::load(&paths)
            .unwrap_err()
            .to_string()
            .contains("Unsupported active policy version 2"));
    }
}
