use crate::agent::AgentKind;
use crate::files::{atomic_write_json, read_json_if_exists, remove_if_exists, with_advisory_lock};
use crate::network::InterfaceCounters;
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
const SESSION_REPORT_VERSION: u32 = 1;

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
    #[serde(default)]
    pub start_battery_percent: Option<u8>,
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
    #[serde(default)]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub mobile_data_start: Option<InterfaceCounters>,
    #[serde(default)]
    pub mobile_data_end: Option<InterfaceCounters>,
    #[serde(default)]
    pub mobile_data_finalized: bool,
    #[serde(default)]
    pub mobile_data_error: Option<String>,
    pub previous_sleep_disabled: Option<u8>,
    pub remote_owned_by_rucksack: bool,
    pub remote_pid: Option<u32>,
    pub remote_confirmed_by_user: bool,
    pub last_event: Option<String>,
    pub release_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionEndKind {
    Unpack,
    Automatic,
    Recovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileDataUsage {
    pub interface: String,
    pub received_bytes: u64,
    pub sent_bytes: u64,
    pub total_bytes: u64,
    pub measured_from: DateTime<Utc>,
    pub measured_to: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum MobileDataEstimate {
    Available {
        usage: MobileDataUsage,
    },
    Partial {
        usage: MobileDataUsage,
        reason: String,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionReport {
    pub version: u32,
    pub session_id: Uuid,
    pub agent: AgentKind,
    pub project_dir: PathBuf,
    pub focus: Focus,
    pub started_at: DateTime<Utc>,
    pub released_at: DateTime<Utc>,
    pub duration_seconds: u64,
    pub end_kind: SessionEndKind,
    pub release_reason: String,
    pub start_battery_percent: Option<u8>,
    pub end_battery_percent: Option<u8>,
    pub expected_hotspot_ssid: Option<String>,
    pub route_interface: Option<String>,
    pub route_gateway: Option<String>,
    pub network_reachable_at_end: Option<bool>,
    pub remote_confirmed_by_user: bool,
    pub mobile_data: MobileDataEstimate,
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

    pub fn clear_if_current(paths: &AppPaths, expected_id: Uuid) -> Result<bool> {
        with_advisory_lock(&session_lock_path(paths), || {
            let Some(current) = load_session_file(&paths.session_file)? else {
                return Ok(false);
            };
            if current.id != expected_id {
                return Ok(false);
            }
            remove_if_exists(&paths.session_file)?;
            Ok(true)
        })
    }

    pub fn remaining_minutes(&self, now: DateTime<Utc>) -> i64 {
        (self.expires_at - now).num_minutes().max(0)
    }
}

impl SessionReport {
    pub fn from_session(session: &SessionState, end_kind: SessionEndKind) -> Result<Self> {
        let released_at = session
            .ended_at
            .ok_or_else(|| anyhow::anyhow!("Completed session has no end timestamp"))?;
        let release_reason = session
            .release_reason
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Completed session has no release reason"))?;
        let duration_seconds = released_at
            .signed_duration_since(session.started_at)
            .num_seconds()
            .max(0) as u64;

        Ok(Self {
            version: SESSION_REPORT_VERSION,
            session_id: session.id,
            agent: session.agent,
            project_dir: session.project_dir.clone(),
            focus: session.focus,
            started_at: session.started_at,
            released_at,
            duration_seconds,
            end_kind,
            release_reason,
            start_battery_percent: session.start_battery_percent,
            end_battery_percent: session.battery_percent,
            expected_hotspot_ssid: session.expected_hotspot_ssid.clone(),
            route_interface: session
                .commute_route_interface
                .clone()
                .or_else(|| session.route_interface.clone()),
            route_gateway: session.commute_route_gateway.clone(),
            network_reachable_at_end: session.network_reachable,
            remote_confirmed_by_user: session.remote_confirmed_by_user,
            mobile_data: mobile_data_estimate(session),
        })
    }

    pub fn load(paths: &AppPaths) -> Result<Option<Self>> {
        let report = read_json_if_exists::<Self>(&paths.report_file)?;
        if let Some(report) = &report {
            validate_report_version(report, &paths.report_file)?;
        }
        Ok(report)
    }

    pub fn save(&self, paths: &AppPaths) -> Result<Self> {
        validate_report_version(self, &paths.report_file)?;
        with_advisory_lock(&report_lock_path(paths), || {
            let existing = read_json_if_exists::<Self>(&paths.report_file)?;
            if let Some(existing) = existing.as_ref() {
                validate_report_version(existing, &paths.report_file)?;
                if existing.session_id == self.session_id {
                    return Ok(existing.clone());
                }
            }

            let current_session = load_session_file(&paths.session_file)?;
            if current_session
                .as_ref()
                .is_some_and(|session| session.id == self.session_id)
            {
                atomic_write_json(&paths.report_file, self)?;
                return Ok(self.clone());
            }

            if let Some(existing) = existing {
                return Ok(existing);
            }
            anyhow::bail!(
                "Refusing to save report for session {} because it is not the current session",
                self.session_id
            )
        })
    }
}

fn session_lock_path(paths: &AppPaths) -> PathBuf {
    paths.session_file.with_extension("lock")
}

fn report_lock_path(paths: &AppPaths) -> PathBuf {
    paths.report_file.with_extension("lock")
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

fn validate_report_version(report: &SessionReport, path: &std::path::Path) -> Result<()> {
    if report.version != SESSION_REPORT_VERSION {
        anyhow::bail!(
            "Unsupported session report version {} in {}; expected version {}",
            report.version,
            path.display(),
            SESSION_REPORT_VERSION
        );
    }
    Ok(())
}

fn mobile_data_estimate(session: &SessionState) -> MobileDataEstimate {
    let Some(start) = session.mobile_data_start.as_ref() else {
        return MobileDataEstimate::Unavailable {
            reason: session
                .mobile_data_error
                .clone()
                .unwrap_or_else(|| "no interface-counter baseline was recorded".to_owned()),
        };
    };
    let Some(end) = session.mobile_data_end.as_ref() else {
        return MobileDataEstimate::Unavailable {
            reason: session
                .mobile_data_error
                .clone()
                .unwrap_or_else(|| "No final interface-counter sample was recorded".to_owned()),
        };
    };
    if start.interface != end.interface {
        return MobileDataEstimate::Unavailable {
            reason: format!(
                "Counter interface changed from {} to {}",
                start.interface, end.interface
            ),
        };
    }
    let Some(received_bytes) = end.received_bytes.checked_sub(start.received_bytes) else {
        return MobileDataEstimate::Unavailable {
            reason: "Received-byte counter reset during the session".to_owned(),
        };
    };
    let Some(sent_bytes) = end.sent_bytes.checked_sub(start.sent_bytes) else {
        return MobileDataEstimate::Unavailable {
            reason: "Sent-byte counter reset during the session".to_owned(),
        };
    };
    let Some(total_bytes) = received_bytes.checked_add(sent_bytes) else {
        return MobileDataEstimate::Unavailable {
            reason: "Interface byte total overflowed".to_owned(),
        };
    };
    let usage = MobileDataUsage {
        interface: start.interface.clone(),
        received_bytes,
        sent_bytes,
        total_bytes,
        measured_from: start.sampled_at,
        measured_to: end.sampled_at,
    };
    if session.mobile_data_finalized {
        MobileDataEstimate::Available { usage }
    } else {
        MobileDataEstimate::Partial {
            usage,
            reason: session
                .mobile_data_error
                .clone()
                .unwrap_or_else(|| "The final interface-counter sample was unavailable".to_owned()),
        }
    }
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
            report_file: data_dir.join("last-report.json"),
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
            start_battery_percent: Some(80),
            battery_percent: Some(80),
            network_reachable: Some(true),
            network_outage_started_at: None,
            phase_before_offline: None,
            idle_grace_started_at: None,
            completed_at: None,
            ended_at: None,
            mobile_data_start: None,
            mobile_data_end: None,
            mobile_data_finalized: false,
            mobile_data_error: None,
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
    fn legacy_session_without_report_fields_loads_with_safe_defaults() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let mut value = serde_json::to_value(sample_session()).unwrap();
        let object = value.as_object_mut().unwrap();
        for field in [
            "revision",
            "start_battery_percent",
            "ended_at",
            "mobile_data_start",
            "mobile_data_end",
            "mobile_data_finalized",
            "mobile_data_error",
        ] {
            object.remove(field);
        }
        atomic_write_json(&paths.session_file, &value).unwrap();

        let loaded = SessionState::load(&paths).unwrap().unwrap();

        assert_eq!(loaded.revision, 0);
        assert_eq!(loaded.start_battery_percent, None);
        assert_eq!(loaded.ended_at, None);
        assert_eq!(loaded.mobile_data_start, None);
        assert_eq!(loaded.mobile_data_end, None);
        assert!(!loaded.mobile_data_finalized);
        assert_eq!(loaded.mobile_data_error, None);
    }

    #[test]
    fn identity_scoped_clear_never_removes_a_newer_session() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let mut old_session = sample_session();
        old_session.phase = SessionPhase::Released;
        old_session.save(&paths).unwrap();

        let mut new_session = sample_session();
        new_session.save(&paths).unwrap();

        assert!(!SessionState::clear_if_current(&paths, old_session.id).unwrap());
        assert_eq!(
            SessionState::load(&paths).unwrap().unwrap().id,
            new_session.id
        );
        assert!(SessionState::clear_if_current(&paths, new_session.id).unwrap());
        assert!(SessionState::load(&paths).unwrap().is_none());
    }

    #[test]
    fn completed_report_persists_aggregate_interface_delta() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let mut session = sample_session();
        let started_at = DateTime::parse_from_rfc3339("2026-07-24T14:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let ended_at = started_at + chrono::Duration::seconds(95);
        session.started_at = started_at;
        session.ended_at = Some(ended_at);
        session.release_reason = Some("user unpacked".to_owned());
        session.mobile_data_start = Some(InterfaceCounters {
            interface: "en0".to_owned(),
            received_bytes: 1_000,
            sent_bytes: 2_000,
            sampled_at: started_at,
        });
        session.mobile_data_end = Some(InterfaceCounters {
            interface: "en0".to_owned(),
            received_bytes: 4_000,
            sent_bytes: 7_000,
            sampled_at: ended_at,
        });
        session.mobile_data_finalized = true;

        session.save(&paths).unwrap();
        let report = SessionReport::from_session(&session, SessionEndKind::Unpack).unwrap();
        report.save(&paths).unwrap();
        let loaded = SessionReport::load(&paths).unwrap().unwrap();

        assert_eq!(loaded.session_id, session.id);
        assert_eq!(loaded.duration_seconds, 95);
        assert!(matches!(
            loaded.mobile_data,
            MobileDataEstimate::Available {
                usage: MobileDataUsage {
                    received_bytes: 3_000,
                    sent_bytes: 5_000,
                    total_bytes: 8_000,
                    ..
                }
            }
        ));
    }

    #[test]
    fn a_stale_session_cannot_replace_the_current_report() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let mut older_session = sample_session();
        older_session.ended_at = Some(older_session.started_at + chrono::Duration::seconds(30));
        older_session.release_reason = Some("older automatic release".to_owned());
        let older = SessionReport::from_session(&older_session, SessionEndKind::Automatic).unwrap();

        let mut newer_session = sample_session();
        newer_session.started_at = older_session.started_at + chrono::Duration::minutes(1);
        newer_session.ended_at = Some(newer_session.started_at + chrono::Duration::seconds(30));
        newer_session.release_reason = Some("newer automatic release".to_owned());
        let newer = SessionReport::from_session(&newer_session, SessionEndKind::Automatic).unwrap();

        newer_session.save(&paths).unwrap();
        newer.save(&paths).unwrap();
        let retained = older.save(&paths).unwrap();
        assert_eq!(retained.session_id, newer.session_id);
        let mut stale_same_session = newer.clone();
        stale_same_session.released_at = newer.released_at - chrono::Duration::seconds(1);
        stale_same_session.release_reason = "stale same-session writer".to_owned();
        let retained = stale_same_session.save(&paths).unwrap();
        assert_eq!(retained.release_reason, "newer automatic release");

        let persisted = SessionReport::load(&paths).unwrap().unwrap();
        assert_eq!(persisted.session_id, newer.session_id);
        assert_eq!(persisted.release_reason, "newer automatic release");
    }

    #[test]
    fn counter_reset_is_reported_as_unavailable_instead_of_zero() {
        let mut session = sample_session();
        let ended_at = session.started_at + chrono::Duration::seconds(30);
        session.ended_at = Some(ended_at);
        session.release_reason = Some("automatic release".to_owned());
        session.mobile_data_start = Some(InterfaceCounters {
            interface: "en0".to_owned(),
            received_bytes: 10_000,
            sent_bytes: 20_000,
            sampled_at: session.started_at,
        });
        session.mobile_data_end = Some(InterfaceCounters {
            interface: "en0".to_owned(),
            received_bytes: 9_000,
            sent_bytes: 21_000,
            sampled_at: ended_at,
        });
        session.mobile_data_finalized = true;

        let report = SessionReport::from_session(&session, SessionEndKind::Automatic).unwrap();

        assert!(matches!(
            report.mobile_data,
            MobileDataEstimate::Unavailable { ref reason }
                if reason.contains("Received-byte counter reset")
        ));
    }

    #[test]
    fn missing_final_counter_sample_is_unavailable_instead_of_zero() {
        let mut session = sample_session();
        session.ended_at = Some(session.started_at + chrono::Duration::seconds(30));
        session.release_reason = Some("automatic release".to_owned());
        session.mobile_data_start = Some(InterfaceCounters {
            interface: "en0".to_owned(),
            received_bytes: 10_000,
            sent_bytes: 20_000,
            sampled_at: session.started_at,
        });

        let report = SessionReport::from_session(&session, SessionEndKind::Automatic).unwrap();

        assert!(matches!(
            report.mobile_data,
            MobileDataEstimate::Unavailable { ref reason }
                if reason.contains("No final interface-counter sample")
        ));
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

        let mut completed = sample_session();
        completed.ended_at = Some(Utc::now());
        completed.release_reason = Some("automatic release".to_owned());
        let report = SessionReport::from_session(&completed, SessionEndKind::Automatic).unwrap();
        let mut report_value = serde_json::to_value(report).unwrap();
        report_value["version"] = Value::from(2);
        atomic_write_json(&paths.report_file, &report_value).unwrap();
        assert!(SessionReport::load(&paths)
            .unwrap_err()
            .to_string()
            .contains("Unsupported session report version 2"));
    }
}
