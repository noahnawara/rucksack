use crate::files::{atomic_write_json, read_json_if_exists, remove_if_exists, with_advisory_lock};
use crate::paths::AppPaths;
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

const SESSION_STATE_VERSION: u32 = 2;

/// How many heartbeats of silence mean nobody is writing this session any more.
///
/// The watcher writes one every `heartbeat_seconds` and renews the power lease in the same breath,
/// for `helper_ttl_seconds` — three beats against one, at the defaults. So by the third missed beat
/// the helper has already handed sleep back on its own, and a file still reading "active" is
/// describing a Mac that can sleep the moment the lid closes.
///
/// One constant, because every caller is asking the same question: is anyone still writing this. The
/// watcher asks it of a gap in its own samples, `status` asks it of a file on disk, and a session
/// answers the same way whether the silence came from sleep, a crash, or a killed watcher.
pub const SILENT_HEARTBEATS: u64 = 3;

/// The longest silence that can still be a running watcher, whatever the heartbeat is set to.
const MAX_SILENCE_SECONDS: u64 = 3600;

/// How long a session may go quiet before its recorded figures stop being current.
pub fn silence_tolerance(heartbeat_seconds: u64) -> Duration {
    Duration::seconds(
        heartbeat_seconds
            .saturating_mul(SILENT_HEARTBEATS)
            .min(MAX_SILENCE_SECONDS) as i64,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionPhase {
    Ready,
    Active,
    Released,
}

/// What ends a session first, and how many minutes of it are left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// The configured lease clock.
    Lease(u64),
    /// The battery reaching its floor, projected from the drain observed so far.
    Battery(u64),
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
    /// The battery when the trip began, so the end has something to be compared against.
    ///
    /// Added after version 2 shipped, and deliberately `#[serde(default)]` rather than a version
    /// bump: `load` refuses a version it does not know, so bumping would strand every session that
    /// was already running when the user upgraded — a Mac awake in a bag with a state file its own
    /// binary will not read. An older binary meeting a newer file simply ignores the extra fields.
    #[serde(default)]
    pub started_battery_percent: Option<u8>,
    /// Bytes carried over the commute interface, or `None` when that cannot be said honestly.
    #[serde(default)]
    pub bytes_moved: Option<u64>,
    /// Minutes until the battery reaches its floor, projected from the drops observed so far.
    ///
    /// `None` until enough has been seen to measure a rate, and again whenever the Mac is charging.
    /// Written by the watcher rather than computed on read, because the drops it is measured from
    /// only exist in the running daemon; `status` is a separate process with no history of its own.
    ///
    /// `#[serde(default)]` for the same reason as `started_battery_percent`: a version bump would
    /// strand a session that was already running when the user upgraded.
    #[serde(default)]
    pub battery_minutes_remaining: Option<u64>,
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
            started_battery_percent: None,
            bytes_moved: None,
            battery_minutes_remaining: None,
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

    /// Whichever limit ends this session first.
    ///
    /// The lease is a configured ceiling and the battery is a measured one, and on a commute the
    /// battery is nearly always the smaller. Reporting the lease alone tells someone they have a day
    /// when they have an afternoon, which is the wrong number to plan a journey around.
    ///
    /// The projection is only reported while the watcher is still taking it. A session file keeps
    /// its last written figure forever, so a watcher that died an hour ago would otherwise have
    /// `status` reading out a stale estimate in the present tense — the same over-promising this
    /// exists to remove, in a new place. Past `tolerance`, only the lease clock is claimed, because
    /// that one is arithmetic on a deadline rather than a measurement somebody has to keep taking.
    pub fn binding_limit(&self, now: DateTime<Utc>, tolerance: Duration) -> Limit {
        let lease_minutes = self.remaining_minutes(now);
        let still_measuring = self
            .last_heartbeat_at
            .is_some_and(|beat| now - beat <= tolerance);
        match self.battery_minutes_remaining {
            Some(minutes) if still_measuring && minutes < lease_minutes => Limit::Battery(minutes),
            _ => Limit::Lease(lease_minutes),
        }
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
            cursor_skill: root.join(".cursor/skills-cursor/rucksack/SKILL.md"),
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

    fn tolerance() -> Duration {
        silence_tolerance(30)
    }

    /// The whole point: a day of lease on an afternoon of charge reports the afternoon.
    #[test]
    fn the_battery_binds_when_it_runs_out_first() {
        let now = Utc::now();
        let mut session = SessionState::new(Uuid::new_v4(), now, now + Duration::hours(24));
        session.last_heartbeat_at = Some(now);
        session.battery_minutes_remaining = Some(260);

        assert_eq!(session.binding_limit(now, tolerance()), Limit::Battery(260));
    }

    /// A short trip on a full battery is limited by what the user asked for, not by the charge.
    #[test]
    fn the_lease_binds_when_it_expires_first() {
        let now = Utc::now();
        let mut session = SessionState::new(Uuid::new_v4(), now, now + Duration::hours(1));
        session.last_heartbeat_at = Some(now);
        session.battery_minutes_remaining = Some(260);

        assert_eq!(session.binding_limit(now, tolerance()), Limit::Lease(60));
    }

    /// Before the drain has been measured there is only one limit to report, and it is the lease.
    #[test]
    fn an_unmeasured_battery_leaves_the_lease_in_charge() {
        let now = Utc::now();
        let session = SessionState::new(Uuid::new_v4(), now, now + Duration::hours(2));

        assert_eq!(session.binding_limit(now, tolerance()), Limit::Lease(120));
    }

    /// A watcher that stopped leaves its last estimate behind, and it must not be read out as if
    /// somebody were still taking it. This is the failure that started the whole change.
    #[test]
    fn a_stale_session_stops_claiming_a_battery_it_is_no_longer_watching() {
        let now = Utc::now();
        let mut session = SessionState::new(Uuid::new_v4(), now, now + Duration::hours(24));
        session.last_heartbeat_at = Some(now - Duration::minutes(9));
        session.battery_minutes_remaining = Some(260);

        assert_eq!(session.binding_limit(now, tolerance()), Limit::Lease(1440));
    }

    /// A session that has never beaten has nothing to go stale, and equally nothing to claim.
    #[test]
    fn a_session_that_never_beat_claims_no_battery() {
        let now = Utc::now();
        let mut session = SessionState::new(Uuid::new_v4(), now, now + Duration::hours(24));
        session.battery_minutes_remaining = Some(260);

        assert_eq!(session.binding_limit(now, tolerance()), Limit::Lease(1440));
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
