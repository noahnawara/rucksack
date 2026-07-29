use crate::helper_client::HelperClient;
use crate::thermal::{ends_a_lease, read_thermal_state};
use anyhow::{Context, Result};
use chrono::Utc;
use rucksack_core::drain::Drain;
use rucksack_core::files::{append_line, with_advisory_lock};
use rucksack_core::network::{
    reaches_internet, read_default_route, read_interface_traffic, read_wifi_status,
    InterfaceTraffic,
};
use rucksack_core::power::{
    minutes_until_floor, read_power_status, read_thermal_status, PowerSource,
};
use rucksack_core::state::{
    silence_tolerance, SessionPhase, SessionState, CHECKPOINT_CLEAR_MINUTES,
    CHECKPOINT_LEAD_MINUTES,
};
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
    let mut drain = Drain::default();
    let sleep_gap = silence_tolerance(config.session.heartbeat_seconds);

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
        drain = drain.advance(Utc::now(), health.battery_percent, sleep_gap);
        let battery_minutes_remaining =
            battery_minutes_remaining(&drain, &health, config.safety.sleep_battery_percent);
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
        let remaining = minutes_remaining(
            session.remaining_minutes(Utc::now()),
            battery_minutes_remaining,
        );
        let ending_soon = remaining <= CHECKPOINT_LEAD_MINUTES;
        let reprieved = remaining > CHECKPOINT_CLEAR_MINUTES;
        let announcing = ending_soon && session.checkpoint_requested_at.is_none();
        let calling_off = reprieved && session.checkpoint_requested_at.is_some();
        SessionState::update(paths, session_id, |session| {
            session.last_heartbeat_at = Some(Utc::now());
            session.battery_percent = observed.battery_percent;
            session.battery_minutes_remaining = battery_minutes_remaining;
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
            // Last, so the one event that asks the reader to *do* something is not overwritten by a
            // tunnel in the same heartbeat.
            if ending_soon && session.checkpoint_requested_at.is_none() {
                session.checkpoint_requested_at = Some(Utc::now());
                session.last_event = Some(
                    "winding down; write down where you got to before this Mac sleeps".to_owned(),
                );
            } else if reprieved && session.checkpoint_requested_at.is_some() {
                session.checkpoint_requested_at = None;
                session.last_event =
                    Some("the end moved back; this Mac is not sleeping soon".to_owned());
            }
        })?;
        if announcing {
            log(paths, "winding down; checkpoint requested")?;
        }
        if calling_off {
            log(paths, "wind-down called off; the end moved back")?;
        }

        thread::sleep(Duration::from_secs(config.session.heartbeat_seconds));
    }
}

/// What the Mac's own sensors say right now.
#[derive(Debug, Clone, Copy)]
struct Health {
    battery_percent: Option<u8>,
    /// What macOS estimates is left before empty, when it is willing to say and discharging.
    minutes_to_empty: Option<u64>,
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
    let power = power.ok();
    Health {
        battery_percent: power.as_ref().and_then(|power| power.percent),
        minutes_to_empty: power.as_ref().and_then(|power| power.minutes_to_empty),
        too_hot: read_thermal_status().is_ok_and(|thermal| ends_a_lease(thermal.level))
            || read_thermal_state().is_some_and(ends_a_lease),
    }
}

/// How long the battery has left, measured if rucksack can, borrowed from macOS until it can.
///
/// The drain model needs three readings before it can name a rate: the first is a baseline and
/// produces no drop, and two drops are needed to measure between. On a commute that is eight to ten
/// minutes of a session reporting only the lease clock — which is the one number certain to be
/// wrong, at the exact moment someone is deciding whether to walk away from the machine.
///
/// macOS has an answer in the meantime, so use it rather than saying nothing, and stop using it the
/// moment there is a measurement of this Mac's actual workload. Neither figure is dressed up as the
/// other; both are estimates, and `status` marks them as such.
fn battery_minutes_remaining(drain: &Drain, health: &Health, floor_percent: u8) -> Option<u64> {
    if let Some(measured) = drain.minutes_until(floor_percent) {
        return Some(measured);
    }
    minutes_until_floor(
        health.minutes_to_empty?,
        health.battery_percent?,
        floor_percent,
    )
}

/// How long this session has left, from the watcher's own readings rather than the session file.
///
/// The same "whichever binds first" rule `status` applies, but asked of figures measured moments ago
/// instead of ones read back off disk. The watcher needs no staleness check because it is the thing
/// whose freshness `status` was checking.
///
/// An unmeasured battery leaves the lease clock in charge, which is the honest answer rather than a
/// cautious one: nothing has been observed that says the battery ends this sooner.
fn minutes_remaining(lease_minutes: u64, battery_minutes: Option<u64>) -> u64 {
    battery_minutes.map_or(lease_minutes, |battery| battery.min(lease_minutes))
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
    let online =
        route_interface.is_some() && reaches_internet(config.hotspot.probe_timeout_seconds);
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
            minutes_to_empty: None,
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
            battery_percent: Some(11),
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
            battery_percent: Some(10),
            ..healthy()
        };

        assert_eq!(
            reason(&live, flat, 0).as_deref(),
            Some("the battery reached the 10% floor")
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

    /// The commute case: a day of lease left, an hour of charge, so the hour is what is left.
    #[test]
    fn the_shorter_of_the_two_limits_is_what_remains() {
        assert_eq!(minutes_remaining(1_440, Some(59)), 59);
    }

    /// A short trip on a full battery is bounded by what the user asked for.
    #[test]
    fn a_long_battery_does_not_extend_a_short_lease() {
        assert_eq!(minutes_remaining(60, Some(400)), 60);
    }

    /// Nothing measured is not the same as nothing left. Treating an unmeasured battery as zero
    /// would announce a wind-down on every session that starts on mains power.
    #[test]
    fn an_unmeasured_battery_leaves_the_lease_in_charge() {
        assert_eq!(minutes_remaining(1_440, None), 1_440);
    }

    /// The threshold `pack` promises and the watcher delivers are the same number, so a session that
    /// was told it gets ten minutes' notice gets ten minutes' notice.
    #[test]
    fn the_warning_fires_at_the_lead_pack_promised() {
        assert!(minutes_remaining(1_440, Some(CHECKPOINT_LEAD_MINUTES)) <= CHECKPOINT_LEAD_MINUTES);
        assert!(
            minutes_remaining(1_440, Some(CHECKPOINT_LEAD_MINUTES + 1)) > CHECKPOINT_LEAD_MINUTES
        );
    }

    /// Plugging in on the train is the case the warning has to be able to take back.
    ///
    /// Both battery sources go quiet on mains power, so the lease clock takes over and what is left
    /// jumps to hours. Without this, `status` would go on saying the Mac sleeps soon for the rest of
    /// a session spent on a charger.
    #[test]
    fn mains_power_clears_the_projection_and_reprieves_the_session() {
        let charged = minutes_remaining(1_440, None);

        assert!(charged > CHECKPOINT_CLEAR_MINUTES);
        assert!(charged > CHECKPOINT_LEAD_MINUTES);
    }

    /// A projection wobbling either side of the lead must not retract a deadline every heartbeat,
    /// so the way back is deliberately further out than the way in.
    #[test]
    fn a_wobble_over_the_lead_does_not_call_the_warning_off() {
        let wobble = minutes_remaining(1_440, Some(CHECKPOINT_LEAD_MINUTES + 1));

        assert!(wobble > CHECKPOINT_LEAD_MINUTES);
        assert!(wobble <= CHECKPOINT_CLEAR_MINUTES);
    }

    fn at(minute: i64) -> chrono::DateTime<Utc> {
        chrono::DateTime::from_timestamp(minute * 60, 0).unwrap()
    }

    fn gap() -> chrono::Duration {
        chrono::Duration::minutes(2)
    }

    /// The opening minutes of every session, which used to report nothing.
    ///
    /// macOS says three hours to empty; the floor is nearer than empty, so the session is shorter
    /// than the battery.
    #[test]
    fn macos_answers_until_this_mac_has_been_measured() {
        let health = Health {
            battery_percent: Some(57),
            minutes_to_empty: Some(189),
            too_hot: false,
        };

        assert_eq!(
            battery_minutes_remaining(&Drain::default(), &health, 15),
            Some(139)
        );
    }

    /// Once there is a rate measured from this Mac's own workload, the borrowed figure stops being
    /// used — a general-purpose estimate should not outrank a measurement of the actual machine.
    #[test]
    fn a_measured_rate_outranks_the_borrowed_one() {
        let drain = Drain::default()
            .advance(at(0), Some(57), gap())
            .advance(at(1), Some(56), gap())
            .advance(at(2), Some(55), gap());
        let health = Health {
            battery_percent: Some(55),
            minutes_to_empty: Some(189),
            too_hot: false,
        };

        // Forty percent of headroom at a percent a minute, not macOS's slower guess.
        assert_eq!(battery_minutes_remaining(&drain, &health, 15), Some(40));
    }

    /// On mains power, and in the minute after a wake, macOS offers nothing. Neither does rucksack.
    #[test]
    fn nothing_is_claimed_when_neither_source_has_an_answer() {
        let health = Health {
            battery_percent: Some(57),
            minutes_to_empty: None,
            too_hot: false,
        };

        assert_eq!(
            battery_minutes_remaining(&Drain::default(), &health, 15),
            None
        );
    }

    /// A gauge that cannot be read cannot be scaled against, however confident macOS sounds.
    #[test]
    fn an_unreadable_gauge_borrows_nothing() {
        let health = Health {
            battery_percent: None,
            minutes_to_empty: Some(189),
            too_hot: false,
        };

        assert_eq!(
            battery_minutes_remaining(&Drain::default(), &health, 15),
            None
        );
    }
}
