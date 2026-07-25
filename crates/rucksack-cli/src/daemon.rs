use crate::helper_client::HelperClient;
use anyhow::{Context, Result};
use chrono::Utc;
use rucksack_core::files::{append_line, with_advisory_lock};
use rucksack_core::network::{
    reaches_internet, read_default_route, read_wifi_status, DEFAULT_INTERNET_PROBE_URL,
};
use rucksack_core::power::{read_power_status, read_thermal_status, PowerSource, ThermalLevel};
use rucksack_core::state::{SessionPhase, SessionState};
use rucksack_core::{AppPaths, Config};
use std::thread;
use std::time::Duration;
use uuid::Uuid;

/// How many times a sensor may fail before rucksack stops trusting the Mac.
const MAX_SENSOR_FAILURES: u8 = 3;

/// Watch the host conditions that make a closed lid unsafe.
///
/// The lease belongs to the Mac, so only host-level facts end it: the time limit, the battery
/// floor, real thermal throttling, or a battery gauge that has gone silent while on battery.
/// Losing the network and agents finishing their work are both recorded and neither releases it —
/// a train going into a tunnel, or a task completing, must not put the machine to sleep.
pub fn run(session_id: Uuid, paths: &AppPaths, config: &Config) -> Result<()> {
    config
        .validate()
        .map_err(|error| anyhow::anyhow!("The configuration is not usable: {error}"))?;
    let helper = HelperClient::default();
    let mut blind_reads = 0u8;

    log(paths, &format!("watching session {session_id}"))?;
    let established = SessionState::update(paths, session_id, |session| {
        session.phase = SessionPhase::Active;
        session.daemon_pid = Some(std::process::id());
        session.last_heartbeat_at = Some(Utc::now());
    })?;
    if established.is_none() {
        anyhow::bail!("The session disappeared before the watcher started.");
    }

    loop {
        let Some(session) = SessionState::load(paths)? else {
            log(paths, "session cleared; watcher exiting")?;
            return Ok(());
        };
        if session.id != session_id || !session.is_holding_a_lease() {
            log(paths, "session ended elsewhere; watcher exiting")?;
            return Ok(());
        }

        let health = read_health(&mut blind_reads);
        if let Some(reason) = release_reason(&session, config, &health, blind_reads) {
            release(&helper, paths, session_id, &reason)?;
            log(paths, &format!("released: {reason}"))?;
            return Ok(());
        }

        helper
            .renew(session.lease_id, config.session.helper_ttl_seconds)
            .context("The power helper stopped answering, so it will restore sleep on its own.")?;

        let observed = observe(config, &health, session.hotspot.is_none());
        SessionState::update(paths, session_id, |session| {
            session.last_heartbeat_at = Some(Utc::now());
            session.battery_percent = observed.battery_percent;
            session.route_interface = observed.route_interface.clone();
            session.hotspot = observed.hotspot.clone().or_else(|| session.hotspot.clone());
            if session.online != observed.online {
                session.last_event = Some(
                    if observed.online {
                        "the network came back"
                    } else {
                        "the network went away; the lease is still held"
                    }
                    .to_owned(),
                );
            }
            session.online = observed.online;
        })?;

        thread::sleep(Duration::from_secs(config.session.heartbeat_seconds));
    }
}

/// What the Mac's own sensors say right now.
#[derive(Debug, Clone, Copy)]
struct Health {
    battery_percent: Option<u8>,
    too_hot: bool,
}

/// Read the sensors, and count how long the battery gauge has been silent.
///
/// A silent gauge on AC power is ordinary and resets the count. On battery it means rucksack is
/// flying blind, and only a run of failures — never one — is allowed to end a lease. The count
/// lives here rather than inside `release_reason` so the rule cannot be tripped by a single read.
fn read_health(consecutive_blind_reads: &mut u8) -> Health {
    let power = read_power_status();
    let readable = power
        .as_ref()
        .is_ok_and(|power| power.percent.is_some() || power.source != PowerSource::Battery);
    *consecutive_blind_reads = if readable {
        0
    } else {
        consecutive_blind_reads.saturating_add(1)
    };
    Health {
        battery_percent: power.ok().and_then(|power| power.percent),
        too_hot: read_thermal_status().is_ok_and(|thermal| {
            thermal.throttled
                || matches!(
                    thermal.level,
                    ThermalLevel::Serious | ThermalLevel::Critical
                )
        }),
    }
}

/// The only reasons a host lease ends by itself.
///
/// Deliberately pure, so the rule that a healthy Mac keeps its lease is a test rather than a hope.
fn release_reason(
    session: &SessionState,
    config: &Config,
    health: &Health,
    consecutive_blind_reads: u8,
) -> Option<String> {
    if Utc::now() >= session.expires_at {
        return Some("the time limit was reached".to_owned());
    }
    if health
        .battery_percent
        .is_some_and(|percent| percent <= config.safety.sleep_battery_percent)
    {
        return Some(format!(
            "the battery reached the {}% floor",
            config.safety.sleep_battery_percent
        ));
    }
    if consecutive_blind_reads >= MAX_SENSOR_FAILURES {
        return Some("the battery level could not be read".to_owned());
    }
    if health.too_hot {
        return Some("this Mac got too hot".to_owned());
    }
    None
}

struct Observed {
    battery_percent: Option<u8>,
    route_interface: Option<String>,
    hotspot: Option<String>,
    online: bool,
}

/// Record where the Mac is, for `status`. Purely observational.
///
/// Reuses the battery reading the release check already took, and only asks macOS for the network
/// name when rucksack does not have one — each of those is a subprocess, every heartbeat, for as
/// long as the lid is closed.
fn observe(config: &Config, health: &Health, want_name: bool) -> Observed {
    let route_interface = read_default_route().ok().and_then(|route| route.interface);
    let online = route_interface.is_some()
        && reaches_internet(
            DEFAULT_INTERNET_PROBE_URL,
            config.hotspot.probe_timeout_seconds,
        );
    Observed {
        battery_percent: health.battery_percent,
        route_interface,
        hotspot: want_name
            .then(|| read_wifi_status().ok().and_then(|status| status.ssid))
            .flatten(),
        online,
    }
}

/// Hand sleep back and record why, under the same lock `unpack` uses.
fn release(helper: &HelperClient, paths: &AppPaths, session_id: Uuid, reason: &str) -> Result<()> {
    with_advisory_lock(&paths.terminal_lock_file(), || {
        let Some(session) = SessionState::load(paths)? else {
            return Ok(());
        };
        if session.id != session_id || !session.is_holding_a_lease() {
            return Ok(());
        }
        let status = helper
            .release(session.lease_id, reason)
            .context("Could not restore normal sleep.")?;
        if status.active {
            anyhow::bail!("The power helper still reports an active lease after releasing it.");
        }
        SessionState::update(paths, session_id, |session| {
            session.phase = SessionPhase::Released;
            session.ended_at = Some(Utc::now());
            session.release_reason = Some(reason.to_owned());
            session.last_event = Some(format!("normal sleep restored: {reason}"));
        })?;
        Ok(())
    })
}

fn log(paths: &AppPaths, message: &str) -> Result<()> {
    append_line(
        &paths.daemon_log,
        &format!("{} {message}", Utc::now().to_rfc3339()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(expires_in: chrono::Duration) -> SessionState {
        let now = Utc::now();
        SessionState::new(Uuid::new_v4(), now, now + expires_in)
    }

    fn healthy() -> Health {
        Health {
            battery_percent: Some(80),
            too_hot: false,
        }
    }

    fn reason(session: &SessionState, health: Health, failures: u8) -> Option<String> {
        release_reason(session, &Config::default(), &health, failures)
    }

    /// The regression that matters most: nothing ordinary may end a live lease.
    ///
    /// Every false release here is a Mac that fell asleep in someone's bag with work in flight.
    #[test]
    fn a_healthy_mac_keeps_its_lease() {
        let live = session(chrono::Duration::hours(1));

        assert_eq!(reason(&live, healthy(), 0), None);

        // Nor do any of the things that used to end it.
        let offline_and_idle = Health {
            battery_percent: Some(16),
            ..healthy()
        };
        assert_eq!(
            reason(&live, offline_and_idle, MAX_SENSOR_FAILURES - 1),
            None
        );

        // A silent gauge is only dangerous once it has been silent repeatedly.
        let without_a_gauge = Health {
            battery_percent: None,
            ..healthy()
        };
        assert_eq!(
            reason(&live, without_a_gauge, MAX_SENSOR_FAILURES - 1),
            None
        );
    }

    #[test]
    fn the_time_limit_ends_the_lease() {
        let expired = session(chrono::Duration::seconds(-1));

        assert_eq!(
            reason(&expired, healthy(), 0).as_deref(),
            Some("the time limit was reached")
        );
    }

    #[test]
    fn the_battery_floor_ends_the_lease() {
        let live = session(chrono::Duration::hours(1));
        let flat = Health {
            battery_percent: Some(15),
            ..healthy()
        };

        assert_eq!(
            reason(&live, flat, 0).as_deref(),
            Some("the battery reached the 15% floor")
        );
    }

    #[test]
    fn a_gauge_that_keeps_failing_ends_the_lease() {
        let live = session(chrono::Duration::hours(1));
        let blind = Health {
            battery_percent: None,
            ..healthy()
        };

        assert_eq!(reason(&live, blind, MAX_SENSOR_FAILURES - 1), None);
        assert_eq!(
            reason(&live, blind, MAX_SENSOR_FAILURES).as_deref(),
            Some("the battery level could not be read")
        );
    }

    #[test]
    fn real_heat_ends_the_lease() {
        let live = session(chrono::Duration::hours(1));
        let hot = Health {
            too_hot: true,
            ..healthy()
        };

        assert_eq!(
            reason(&live, hot, 0).as_deref(),
            Some("this Mac got too hot")
        );
    }
}
