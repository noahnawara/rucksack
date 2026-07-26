use crate::helper_client::HelperClient;
use crate::thermal::{ends_a_lease, read_thermal_state};
use anyhow::{Context, Result};
use chrono::Utc;
use rucksack_core::files::{append_line, with_advisory_lock};
use rucksack_core::network::{
    reaches_internet, read_default_route, read_interface_traffic, read_wifi_status,
    InterfaceTraffic, DEFAULT_INTERNET_PROBE_URL,
};
use rucksack_core::power::{read_power_status, read_thermal_status, PowerSource};
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
    let mut traffic = Traffic::Waiting;

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
            release(&helper, paths, session_id, &reason, health.battery_percent)?;
            log(paths, &format!("released: {reason}"))?;
            return Ok(());
        }

        helper
            .renew(session.lease_id, config.session.helper_ttl_seconds)
            .context("The power helper stopped answering, so it will restore sleep on its own.")?;

        let observed = observe(config, &health, session.hotspot.is_none());
        traffic = traffic.advance(observed.route_interface.as_deref(), observed.traffic_total);
        SessionState::update(paths, session_id, |session| {
            session.last_heartbeat_at = Some(Utc::now());
            session.battery_percent = observed.battery_percent;
            session.bytes_moved = traffic.moved();
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
///
/// Heat is asked of both sources macOS offers, because neither covers the fleet on its own:
/// `pmset -g therm` still reports real throttling on Intel, and `ProcessInfo.thermalState` is the
/// only one of the two that ever moves on Apple silicon. Either saying too hot is enough; both
/// staying silent is not heat.
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
        too_hot: read_thermal_status()
            .is_ok_and(|thermal| thermal.throttled || ends_a_lease(thermal.level))
            || read_thermal_state().is_some_and(ends_a_lease),
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

/// How much the trip has moved over the network, kept honest for its whole length.
///
/// The Mac's counters answer "since this interface came up", so the figure worth reporting is a
/// difference between two readings. Every state here exists to make that difference either right or
/// absent, because a plausible wrong number is the one outcome worse than no number:
///
/// - The baseline is taken on the first heartbeat, which is after `pack` has already proved the
///   commute network. So the count is traffic over *that* network, not over the office Wi-Fi the
///   user was on ten minutes earlier.
/// - A heartbeat with nothing to read carries the figure forward untouched. A commute is full of
///   tunnels, and losing the route is not losing the trip.
/// - A different interface, or a total that has gone backwards, gives up for good. Both mean the
///   counter no longer refers to the same thing it did, and no honest arithmetic recovers it.
///
/// The figure therefore spans the first heartbeat to the last, not `pack` to `unpack`, and reads
/// slightly low: measured against a wider independent reading on a real trip it came to 84 MB
/// against 102 MB. Low is the right direction for a number a user might be billed for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Traffic {
    /// No baseline taken yet.
    Waiting,
    Counting {
        interface: String,
        baseline: u64,
        moved: u64,
    },
    /// The counter stopped meaning what it meant. Never resumes, because a resumed count would
    /// silently leave a stretch of the trip out of a number presented as the whole of it.
    Unavailable,
}

impl Traffic {
    /// Fold in one heartbeat's reading.
    fn advance(self, interface: Option<&str>, total: Option<u64>) -> Self {
        match (self, interface, total) {
            (Traffic::Unavailable, _, _) => Traffic::Unavailable,
            (current, None, _) | (current, _, None) => current,
            (Traffic::Waiting, Some(interface), Some(total)) => Traffic::Counting {
                interface: interface.to_owned(),
                baseline: total,
                moved: 0,
            },
            (
                Traffic::Counting {
                    interface: started_on,
                    baseline,
                    ..
                },
                Some(interface),
                Some(total),
            ) => {
                if interface != started_on || total < baseline {
                    return Traffic::Unavailable;
                }
                Traffic::Counting {
                    interface: started_on,
                    baseline,
                    moved: total - baseline,
                }
            }
        }
    }

    fn moved(&self) -> Option<u64> {
        match self {
            Traffic::Counting { moved, .. } => Some(*moved),
            Traffic::Waiting | Traffic::Unavailable => None,
        }
    }
}

struct Observed {
    battery_percent: Option<u8>,
    route_interface: Option<String>,
    hotspot: Option<String>,
    online: bool,
    traffic_total: Option<u64>,
}

/// Record where the Mac is, for `status`. Purely observational.
///
/// Reuses the battery reading the release check already took, and only asks macOS for the network
/// name when rucksack does not have one — each of those is a subprocess, every heartbeat, for as
/// long as the lid is closed. The traffic read adds one more, deliberately: it is the only way to
/// tell the user afterwards what the trip cost them, and reading it here rather than at `unpack`
/// is what confines the figure to the commute network.
fn observe(config: &Config, health: &Health, want_name: bool) -> Observed {
    let route_interface = read_default_route().ok().and_then(|route| route.interface);
    let online = route_interface.is_some()
        && reaches_internet(
            DEFAULT_INTERNET_PROBE_URL,
            config.hotspot.probe_timeout_seconds,
        );
    let traffic_total = route_interface
        .as_deref()
        .and_then(|interface| read_interface_traffic(interface).ok().flatten())
        .map(InterfaceTraffic::total);
    Observed {
        battery_percent: health.battery_percent,
        route_interface,
        hotspot: want_name
            .then(|| read_wifi_status().ok().and_then(|status| status.ssid))
            .flatten(),
        online,
        traffic_total,
    }
}

/// Hand sleep back and record why, under the same lock `unpack` uses.
///
/// Takes the battery reading the release decision was made on, so the trip report ends on the number
/// that ended the trip rather than the one from a heartbeat up to half a minute earlier. A reading
/// that failed is left out instead of overwriting the last good one.
fn release(
    helper: &HelperClient,
    paths: &AppPaths,
    session_id: Uuid,
    reason: &str,
    battery_percent: Option<u8>,
) -> Result<()> {
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
            if let Some(percent) = battery_percent {
                session.battery_percent = Some(percent);
            }
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

    /// The baseline is the first reading, and the figure is the difference from it.
    #[test]
    fn counts_from_the_first_reading() {
        let traffic = Traffic::Waiting
            .advance(Some("en0"), Some(1_000))
            .advance(Some("en0"), Some(1_500))
            .advance(Some("en0"), Some(9_000));

        assert_eq!(traffic.moved(), Some(8_000));
    }

    /// Nothing to read yet is not zero bytes — it is no answer.
    #[test]
    fn says_nothing_before_it_has_a_baseline() {
        assert_eq!(Traffic::Waiting.moved(), None);
        assert_eq!(Traffic::Waiting.advance(None, None).moved(), None);
        assert_eq!(Traffic::Waiting.advance(Some("en0"), None).moved(), None);
    }

    /// A tunnel is exactly when the count must survive, so a blind heartbeat changes nothing.
    #[test]
    fn an_outage_carries_the_figure_forward() {
        let before = Traffic::Waiting
            .advance(Some("en0"), Some(1_000))
            .advance(Some("en0"), Some(4_000));
        let through_a_tunnel = before
            .clone()
            .advance(None, None)
            .advance(None, Some(9_999))
            .advance(Some("en0"), None);

        assert_eq!(through_a_tunnel.moved(), Some(3_000));
        assert_eq!(
            through_a_tunnel.advance(Some("en0"), Some(6_000)).moved(),
            Some(5_000)
        );
    }

    /// A counter that went backwards reset, and no arithmetic recovers what it counted.
    #[test]
    fn a_reset_counter_gives_up_rather_than_guess() {
        let reset = Traffic::Waiting
            .advance(Some("en0"), Some(9_000))
            .advance(Some("en0"), Some(12));

        assert_eq!(reset.moved(), None);
        // And never resumes, because a resumed count would omit part of the trip silently.
        assert_eq!(reset.advance(Some("en0"), Some(50_000)).moved(), None);
    }

    /// Wi-Fi to USB tethering mid-trip: two counters, no comparable total.
    #[test]
    fn a_changed_interface_gives_up_too() {
        let switched = Traffic::Waiting
            .advance(Some("en0"), Some(1_000))
            .advance(Some("en7"), Some(80_000));

        assert_eq!(switched.moved(), None);
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
