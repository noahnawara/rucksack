use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rucksack_core::files::{
    atomic_write_json, ensure_private_dir, read_json_if_exists, remove_if_exists,
};
use rucksack_core::power::read_sleep_disabled;
use rucksack_core::protocol::{HelperOperation, HelperStatus, MAX_HELPER_TTL_SECONDS};
use rucksack_core::system::run_bounded_cleared;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use uuid::Uuid;

const STATE_VERSION: u32 = 2;
const MAX_REASON_BYTES: usize = 256;
const MAX_HARD_LEASE_DURATION_SECONDS: i64 = 24 * 60 * 60;
const POWER_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const POWER_COMMAND_MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedLease {
    version: u32,
    lease_id: Uuid,
    owner_uid: u32,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    hard_expires_at: DateTime<Utc>,
    previous_sleep_disabled: u8,
    reason: String,
    last_reasserted_at: Option<DateTime<Utc>>,
}

enum PersistedState {
    Missing,
    Valid(PersistedLease),
    Invalid(String),
}

pub struct LeaseManager {
    state_path: PathBuf,
    lease: Option<PersistedLease>,
}

impl LeaseManager {
    pub fn open(state_path: impl AsRef<Path>) -> Result<Self> {
        let state_path = state_path.as_ref().to_path_buf();
        if let Some(parent) = state_path.parent() {
            ensure_private_dir(parent)?;
        }
        let stale = match load_persisted_state(&state_path) {
            PersistedState::Missing => None,
            PersistedState::Valid(lease) => Some(lease),
            PersistedState::Invalid(error) => {
                recover_invalid_persisted_state(&state_path, &error)?;
                None
            }
        };
        let mut manager = Self {
            state_path,
            lease: stale,
        };
        if manager.lease.is_some() {
            manager.restore_stale_on_startup()?;
        }
        Ok(manager)
    }

    pub fn handle(&mut self, caller_uid: u32, operation: HelperOperation) -> Result<HelperStatus> {
        match operation {
            HelperOperation::Acquire {
                lease_id,
                ttl_seconds,
                hard_expires_at,
                reason,
            } => self.acquire(caller_uid, lease_id, ttl_seconds, hard_expires_at, reason),
            HelperOperation::Renew {
                lease_id,
                ttl_seconds,
            } => self.renew(caller_uid, lease_id, ttl_seconds),
            HelperOperation::Reassert { lease_id } => {
                self.require_owner(caller_uid, lease_id)?;
                self.reassert_now()?;
                self.status()
            }
            HelperOperation::Release {
                lease_id,
                reason: _,
            } => {
                self.require_owner(caller_uid, lease_id)?;
                self.release_internal()
            }
            HelperOperation::Recover => self.recover(caller_uid),
            HelperOperation::Status => self.status(),
        }
    }

    pub fn watchdog_tick(&mut self) -> Result<()> {
        let Some(lease) = self.lease.as_ref() else {
            return Ok(());
        };
        let now = Utc::now();
        if now >= lease.expires_at || now >= lease.hard_expires_at {
            self.release_internal()?;
            return Ok(());
        }
        if read_sleep_disabled().ok() != Some(1) {
            self.reassert_now()?;
        }
        Ok(())
    }

    pub fn power_source_changed(&mut self) -> Result<()> {
        if self.restore_if_deadline_elapsed()? {
            return Ok(());
        }
        if self.lease.is_none() {
            return Ok(());
        }

        // First write immediately, then again after the power-management transition settles.
        // If either write or verification fails, restore the pre-rucksack baseline. A helper
        // that cannot prove the override is active must fail safe to ordinary sleep.
        let reassert = (|| -> Result<()> {
            set_sleep_disabled(1)?;
            thread::sleep(Duration::from_millis(250));
            set_sleep_disabled(1)?;
            verify_sleep_disabled(1)
        })();

        if let Err(error) = reassert {
            let restore = self.release_internal();
            return match restore {
                Ok(_) => Err(error).context(
                    "Closed-lid mode could not be re-armed after the power-source change; the previous sleep state was restored",
                ),
                Err(restore_error) => Err(anyhow!(
                    "Closed-lid re-arm failed ({error}); restoring the previous sleep state also failed ({restore_error})"
                )),
            };
        }

        if self.restore_if_deadline_elapsed()? {
            anyhow::bail!("the lease deadline elapsed during power-source recovery");
        }
        if let Some(lease) = self.lease.as_mut() {
            lease.last_reasserted_at = Some(Utc::now());
        }
        self.persist()
    }

    pub fn status(&self) -> Result<HelperStatus> {
        let observed = read_sleep_disabled().ok();
        Ok(match &self.lease {
            Some(lease) => HelperStatus {
                active: true,
                lease_id: Some(lease.lease_id),
                owner_uid: Some(lease.owner_uid),
                created_at: Some(lease.created_at),
                expires_at: Some(lease.expires_at),
                hard_expires_at: Some(lease.hard_expires_at),
                previous_sleep_disabled: Some(lease.previous_sleep_disabled),
                sleep_disabled: observed,
                reason: Some(lease.reason.clone()),
                last_reasserted_at: lease.last_reasserted_at,
            },
            None => HelperStatus {
                active: false,
                lease_id: None,
                owner_uid: None,
                created_at: None,
                expires_at: None,
                hard_expires_at: None,
                previous_sleep_disabled: None,
                sleep_disabled: observed,
                reason: None,
                last_reasserted_at: None,
            },
        })
    }

    fn acquire(
        &mut self,
        caller_uid: u32,
        lease_id: Uuid,
        ttl_seconds: u64,
        hard_expires_at: DateTime<Utc>,
        reason: String,
    ) -> Result<HelperStatus> {
        validate_ttl(ttl_seconds)?;
        if reason.len() > MAX_REASON_BYTES {
            anyhow::bail!("lease reason exceeds {MAX_REASON_BYTES} bytes");
        }

        if let Some(existing) = &self.lease {
            if existing.owner_uid == caller_uid && existing.lease_id == lease_id {
                if existing.hard_expires_at != hard_expires_at {
                    anyhow::bail!("an active lease's hard deadline cannot be changed");
                }
                return self.renew(caller_uid, lease_id, ttl_seconds);
            }
            anyhow::bail!(
                "a closed-lid lease is already owned by UID {} until {}",
                existing.owner_uid,
                existing.expires_at
            );
        }

        let now = Utc::now();
        validate_hard_deadline(now, hard_expires_at)?;
        let expires_at = clamped_expiration(now, ttl_seconds, hard_expires_at)?;
        let previous = read_sleep_disabled()
            .context("Could not read SleepDisabled before acquiring the lease")?;
        if previous != 0 {
            anyhow::bail!(
                "SleepDisabled is already {previous}; stop Amphetamine or any other closed-lid utility before starting rucksack so the helper has an unambiguous rollback target"
            );
        }
        self.lease = Some(PersistedLease {
            version: STATE_VERSION,
            lease_id,
            owner_uid: caller_uid,
            created_at: now,
            expires_at,
            hard_expires_at,
            previous_sleep_disabled: previous,
            reason,
            last_reasserted_at: None,
        });

        // Persist the rollback target before changing the global power setting.
        self.persist()?;
        if let Err(acquire_error) = set_sleep_disabled(1).and_then(|_| verify_sleep_disabled(1)) {
            let restore =
                set_sleep_disabled(previous).and_then(|_| verify_sleep_disabled(previous));
            return match restore {
                Ok(()) => {
                    self.lease = None;
                    remove_if_exists(&self.state_path)?;
                    Err(acquire_error).context("Could not acquire the closed-lid lease")
                }
                Err(restore_error) => {
                    if let Some(lease) = self.lease.as_mut() {
                        lease.expires_at = Utc::now();
                    }
                    match self.persist() {
                        Ok(()) => Err(anyhow!(
                            "Could not acquire the closed-lid lease ({acquire_error}); restoring normal sleep also failed ({restore_error}); the lease was marked expired for watchdog recovery"
                        )),
                        Err(persist_error) => Err(anyhow!(
                            "Could not acquire the closed-lid lease ({acquire_error}); restoring normal sleep also failed ({restore_error}); recording the expired recovery state failed ({persist_error})"
                        )),
                    }
                }
            };
        }
        if self.restore_if_deadline_elapsed()? {
            anyhow::bail!("the lease deadline elapsed during acquisition");
        }
        if let Some(lease) = self.lease.as_mut() {
            lease.last_reasserted_at = Some(Utc::now());
        }
        self.persist()?;
        self.status()
    }

    fn renew(&mut self, caller_uid: u32, lease_id: Uuid, ttl_seconds: u64) -> Result<HelperStatus> {
        validate_ttl(ttl_seconds)?;
        self.require_owner(caller_uid, lease_id)?;
        if self.restore_if_deadline_elapsed()? {
            anyhow::bail!("the lease deadline has elapsed");
        }
        let now = Utc::now();
        let hard_expires_at = self
            .lease
            .as_ref()
            .context("no active lease")?
            .hard_expires_at;
        if let Some(lease) = self.lease.as_mut() {
            lease.expires_at = clamped_expiration(now, ttl_seconds, hard_expires_at)?;
        }
        if read_sleep_disabled().ok() != Some(1) {
            self.reassert_now()?;
        } else {
            self.persist()?;
        }
        self.status()
    }

    fn reassert_now(&mut self) -> Result<()> {
        if self.restore_if_deadline_elapsed()? {
            anyhow::bail!("the lease deadline has elapsed");
        }
        if self.lease.is_none() {
            anyhow::bail!("no active lease to reassert");
        }
        set_sleep_disabled(1)?;
        verify_sleep_disabled(1)?;
        if self.restore_if_deadline_elapsed()? {
            anyhow::bail!("the lease deadline elapsed while reasserting");
        }
        if let Some(lease) = self.lease.as_mut() {
            lease.last_reasserted_at = Some(Utc::now());
        }
        self.persist()
    }

    fn release_internal(&mut self) -> Result<HelperStatus> {
        let Some(lease) = self.lease.clone() else {
            return self.status();
        };
        set_sleep_disabled(lease.previous_sleep_disabled)?;
        verify_sleep_disabled(lease.previous_sleep_disabled)?;
        self.lease = None;
        remove_if_exists(&self.state_path)?;
        self.status()
    }

    fn recover(&mut self, caller_uid: u32) -> Result<HelperStatus> {
        if self.lease.is_some() {
            self.require_owner_uid(caller_uid)?;
            return self.release_internal();
        }
        self.status()
    }

    fn restore_if_deadline_elapsed(&mut self) -> Result<bool> {
        let now = Utc::now();
        let elapsed = self
            .lease
            .as_ref()
            .is_some_and(|lease| now >= lease.expires_at || now >= lease.hard_expires_at);
        if !elapsed {
            return Ok(false);
        }
        self.release_internal()?;
        Ok(true)
    }

    fn restore_stale_on_startup(&mut self) -> Result<()> {
        let lease = self
            .lease
            .clone()
            .context("restore_stale_on_startup called without state")?;
        set_sleep_disabled(lease.previous_sleep_disabled)?;
        verify_sleep_disabled(lease.previous_sleep_disabled)?;
        self.lease = None;
        remove_if_exists(&self.state_path)?;
        Ok(())
    }

    fn require_owner(&self, caller_uid: u32, lease_id: Uuid) -> Result<()> {
        let lease = self.lease.as_ref().context("no active lease")?;
        self.require_owner_uid(caller_uid)?;
        if lease.lease_id != lease_id {
            anyhow::bail!("lease ID does not match the active lease");
        }
        Ok(())
    }

    fn require_owner_uid(&self, caller_uid: u32) -> Result<()> {
        let lease = self.lease.as_ref().context("no active lease")?;
        if caller_uid != 0 && caller_uid != lease.owner_uid {
            anyhow::bail!("lease belongs to a different user");
        }
        Ok(())
    }

    fn persist(&self) -> Result<()> {
        if let Some(lease) = &self.lease {
            atomic_write_json(&self.state_path, lease)
        } else {
            remove_if_exists(&self.state_path)
        }
    }
}

impl PersistedLease {
    fn validate(&self) -> Result<()> {
        if self.version != STATE_VERSION {
            anyhow::bail!(
                "unsupported persisted lease version {}; expected {}",
                self.version,
                STATE_VERSION
            );
        }
        if self.previous_sleep_disabled != 0 {
            anyhow::bail!(
                "persisted rollback baseline must be 0, observed {}",
                self.previous_sleep_disabled
            );
        }
        if self.expires_at <= self.created_at {
            anyhow::bail!("persisted lease expiry must be after creation");
        }
        if self.hard_expires_at <= self.created_at {
            anyhow::bail!("persisted hard deadline must be after lease creation");
        }
        let maximum_hard_deadline = maximum_hard_deadline(self.created_at)?;
        if self.hard_expires_at > maximum_hard_deadline {
            anyhow::bail!(
                "persisted hard deadline exceeds the {}-second safety maximum",
                MAX_HARD_LEASE_DURATION_SECONDS
            );
        }
        if self.expires_at > self.hard_expires_at {
            anyhow::bail!("persisted lease expiry exceeds its hard deadline");
        }
        if self.reason.len() > MAX_REASON_BYTES {
            anyhow::bail!("persisted lease reason exceeds {MAX_REASON_BYTES} bytes");
        }
        Ok(())
    }
}

fn load_persisted_state(path: &Path) -> PersistedState {
    match read_json_if_exists::<PersistedLease>(path) {
        Ok(None) => PersistedState::Missing,
        Ok(Some(lease)) => match lease.validate() {
            Ok(()) => PersistedState::Valid(lease),
            Err(error) => PersistedState::Invalid(error.to_string()),
        },
        Err(error) => PersistedState::Invalid(error.to_string()),
    }
}

fn recover_invalid_persisted_state(path: &Path, state_error: &str) -> Result<()> {
    if let Err(recovery_error) = set_sleep_disabled(0).and_then(|_| verify_sleep_disabled(0)) {
        return Err(anyhow!(
            "persisted helper state is invalid ({state_error}); fail-safe restoration to SleepDisabled=0 also failed ({recovery_error})"
        ));
    }
    remove_if_exists(path).with_context(|| {
        format!(
            "Normal sleep was restored after invalid persisted state, but {} could not be removed",
            path.display()
        )
    })?;
    eprintln!(
        "rucksack-helper recovery: event=invalid-persisted-state action=restored-normal-sleep detail={state_error}"
    );
    Ok(())
}

fn validate_ttl(ttl_seconds: u64) -> Result<()> {
    if !(30..=MAX_HELPER_TTL_SECONDS).contains(&ttl_seconds) {
        anyhow::bail!(
            "helper TTL must be between 30 and {} seconds",
            MAX_HELPER_TTL_SECONDS
        );
    }
    Ok(())
}

fn validate_hard_deadline(now: DateTime<Utc>, hard_expires_at: DateTime<Utc>) -> Result<()> {
    if hard_expires_at <= now {
        anyhow::bail!("lease hard deadline must be in the future");
    }
    if hard_expires_at > maximum_hard_deadline(now)? {
        anyhow::bail!(
            "lease hard deadline must be no more than {} seconds in the future",
            MAX_HARD_LEASE_DURATION_SECONDS
        );
    }
    Ok(())
}

fn maximum_hard_deadline(start: DateTime<Utc>) -> Result<DateTime<Utc>> {
    start
        .checked_add_signed(ChronoDuration::seconds(MAX_HARD_LEASE_DURATION_SECONDS))
        .context("Could not represent the maximum lease hard deadline")
}

fn requested_expiration(now: DateTime<Utc>, ttl_seconds: u64) -> Result<DateTime<Utc>> {
    let ttl_seconds = i64::try_from(ttl_seconds).context("helper TTL could not fit in i64")?;
    now.checked_add_signed(ChronoDuration::seconds(ttl_seconds))
        .context("Could not represent helper lease expiry")
}

fn clamped_expiration(
    now: DateTime<Utc>,
    ttl_seconds: u64,
    hard_expires_at: DateTime<Utc>,
) -> Result<DateTime<Utc>> {
    Ok(requested_expiration(now, ttl_seconds)?.min(hard_expires_at))
}

fn verify_sleep_disabled(expected: u8) -> Result<()> {
    let observed = read_sleep_disabled()?;
    if observed == expected {
        Ok(())
    } else {
        Err(anyhow!(
            "SleepDisabled verification failed: expected {expected}, observed {observed}"
        ))
    }
}

fn set_sleep_disabled(value: u8) -> Result<()> {
    if value > 1 {
        anyhow::bail!("invalid SleepDisabled value {value}");
    }
    let value_argument = if value == 1 { "1" } else { "0" };
    let result = run_bounded_cleared(
        "/usr/bin/pmset",
        &["-a", "disablesleep", value_argument],
        POWER_COMMAND_TIMEOUT,
        POWER_COMMAND_MAX_OUTPUT_BYTES,
    )
    .context("Could not execute bounded /usr/bin/pmset mutation")?;
    if result.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "pmset mutation failed with exit code {}: {}",
            result.code,
            result.combined_trimmed()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rucksack_core::files::atomic_write;
    use std::fs;

    fn persisted_lease(version: u32, previous_sleep_disabled: u8) -> PersistedLease {
        let created_at = Utc::now();
        PersistedLease {
            version,
            lease_id: Uuid::new_v4(),
            owner_uid: 501,
            created_at,
            expires_at: created_at + ChronoDuration::seconds(90),
            hard_expires_at: created_at + ChronoDuration::hours(1),
            previous_sleep_disabled,
            reason: "test lease".to_owned(),
            last_reasserted_at: None,
        }
    }

    #[test]
    fn rejects_unknown_persisted_version() {
        let error = persisted_lease(STATE_VERSION + 1, 0)
            .validate()
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("unsupported persisted lease version"));
    }

    #[test]
    fn rejects_nonzero_persisted_baseline() {
        let error = persisted_lease(STATE_VERSION, 1).validate().unwrap_err();
        assert!(error.to_string().contains("rollback baseline"));
    }

    #[test]
    fn active_lease_requires_owner_or_root_uid() {
        let manager = LeaseManager {
            state_path: PathBuf::new(),
            lease: Some(persisted_lease(STATE_VERSION, 0)),
        };

        manager.require_owner_uid(501).unwrap();
        manager.require_owner_uid(0).unwrap();
        assert!(manager.require_owner_uid(502).is_err());
    }

    #[test]
    fn rejects_persisted_deadline_beyond_safety_maximum() {
        let mut lease = persisted_lease(STATE_VERSION, 0);
        lease.hard_expires_at =
            lease.created_at + ChronoDuration::seconds(MAX_HARD_LEASE_DURATION_SECONDS + 1);

        let error = lease.validate().unwrap_err();
        assert!(error.to_string().contains("safety maximum"));
    }

    #[test]
    fn rejects_elapsed_or_overlong_requested_hard_deadline() {
        let now = Utc::now();

        assert!(validate_hard_deadline(now, now).is_err());
        assert!(validate_hard_deadline(
            now,
            now + ChronoDuration::seconds(MAX_HARD_LEASE_DURATION_SECONDS + 1)
        )
        .is_err());
    }

    #[test]
    fn clamps_requested_expiration_to_hard_deadline() {
        let now = Utc::now();
        let hard_expires_at = now + ChronoDuration::seconds(45);
        let expiration = clamped_expiration(now, 90, hard_expires_at).unwrap();

        assert_eq!(expiration, hard_expires_at);
    }

    #[test]
    fn corrupt_persisted_state_is_classified_for_recovery() {
        let directory =
            std::env::temp_dir().join(format!("rucksack-helper-test-{}", Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("helper-state.json");
        atomic_write(&path, b"{invalid-json", 0o600).unwrap();

        assert!(matches!(
            load_persisted_state(&path),
            PersistedState::Invalid(_)
        ));

        remove_if_exists(&path).unwrap();
        fs::remove_dir(&directory).unwrap();
    }
}
