use crate::helper_client::HelperClient;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rucksack_core::agent::{deactivate_cursor_rule, AgentKind};
use rucksack_core::files::append_line;
use rucksack_core::network::{
    internet_probe, probe, provider_probe_url, read_default_route, read_wifi_status, RouteStatus,
    WifiStatus, DEFAULT_INTERNET_PROBE_URL,
};
use rucksack_core::power::{read_power_status, read_thermal_status, PowerSource, ThermalLevel};
use rucksack_core::state::{ActivePolicy, SessionPhase, SessionState, SessionStateWriteConflict};
use rucksack_core::{AppPaths, Config};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

const MAX_CONSECUTIVE_SENSOR_FAILURES: u8 = 3;

pub fn run(session_id: Uuid, paths: &AppPaths, config: &Config) -> Result<()> {
    config
        .validate()
        .map_err(|error| anyhow::anyhow!("Configuration is not safe for the watcher: {error}"))?;
    let helper = HelperClient::default();
    let mut power_sensor_failures: u8 = 0;
    let mut thermal_sensor_failures: u8 = 0;
    let mut network_outage_started: Option<Instant> = None;
    log(paths, &format!("daemon started for session {session_id}"))?;

    loop {
        let Some(mut session) = SessionState::load(paths)? else {
            log(paths, "session file removed; daemon exiting")?;
            return Ok(());
        };
        if session.id != session_id {
            anyhow::bail!(
                "daemon session {} does not match state session {}",
                session_id,
                session.id
            );
        }
        if matches!(
            session.phase,
            SessionPhase::Released | SessionPhase::Releasing | SessionPhase::Failed
        ) {
            log(paths, "session already released; daemon exiting")?;
            return Ok(());
        }
        session.daemon_pid = Some(std::process::id());

        let now = Utc::now();
        if now >= session.expires_at {
            if release(
                &helper,
                paths,
                &mut session,
                "hard timeout reached",
                |current| Utc::now() >= current.expires_at,
            )? {
                return Ok(());
            }
            continue;
        }
        match effective_phase(&session) {
            SessionPhase::Completed => {
                if release(
                    &helper,
                    paths,
                    &mut session,
                    "agent reported the session completed",
                    |current| effective_phase(current) == SessionPhase::Completed,
                )? {
                    return Ok(());
                }
                continue;
            }
            SessionPhase::IdleGrace => {
                let Some(idle_started_at) = session.idle_grace_started_at else {
                    if release(
                        &helper,
                        paths,
                        &mut session,
                        "agent became idle without a verifiable grace timestamp",
                        |current| {
                            effective_phase(current) == SessionPhase::IdleGrace
                                && current.idle_grace_started_at.is_none()
                        },
                    )? {
                        return Ok(());
                    }
                    continue;
                };
                let grace = chrono::Duration::seconds(config.session.idle_grace_seconds as i64);
                if now.signed_duration_since(idle_started_at) >= grace {
                    if release(
                        &helper,
                        paths,
                        &mut session,
                        "agent idle grace elapsed",
                        |current| {
                            effective_phase(current) == SessionPhase::IdleGrace
                                && current.idle_grace_started_at.is_some_and(|started_at| {
                                    Utc::now().signed_duration_since(started_at) >= grace
                                })
                        },
                    )? {
                        return Ok(());
                    }
                    continue;
                }
            }
            _ => {}
        }

        match helper.renew(session.lease_id, config.session.helper_ttl_seconds) {
            Ok(status) => {
                session.previous_sleep_disabled = status.previous_sleep_disabled;
                session.last_heartbeat_at = Some(now);
            }
            Err(error) => {
                let failure = format!("helper heartbeat failed: {error}");
                let Some(updated) = SessionState::update(paths, session.id, |mut current| {
                    current.phase = SessionPhase::Failed;
                    current.last_event = Some(failure);
                    Ok(current)
                })?
                else {
                    return Err(error)
                        .context("Power-helper heartbeat failed after session state was removed");
                };
                session = updated;
                cleanup_policy(paths, session.agent, &session.project_dir)?;
                log(
                    paths,
                    &format!(
                        "helper heartbeat failed; helper TTL will restore normal sleep: {error}"
                    ),
                )?;
                return Err(error).context("Power-helper heartbeat failed");
            }
        }

        let power_error = match read_power_status() {
            Ok(power) => {
                session.battery_percent = power.percent;
                if let Some(percent) = power.percent {
                    if percent <= config.safety.sleep_battery_percent {
                        release(
                            &helper,
                            paths,
                            &mut session,
                            &format!(
                                "battery reached the {}% safety floor",
                                config.safety.sleep_battery_percent
                            ),
                            |_| true,
                        )?;
                        return Ok(());
                    }
                    if power.source == PowerSource::Battery
                        && percent <= config.safety.warn_battery_percent
                    {
                        session.last_event = Some(format!(
                            "battery is {percent}% · sleep releases at {}%",
                            config.safety.sleep_battery_percent
                        ));
                    }
                }
                if power.percent.is_none() {
                    Some("battery percentage is unavailable".to_owned())
                } else if power.source == PowerSource::Unknown {
                    Some("power source is unknown".to_owned())
                } else {
                    None
                }
            }
            Err(error) => Some(error.to_string()),
        };
        power_sensor_failures =
            next_sensor_failure_count(power_sensor_failures, power_error.is_none());
        if power_sensor_failures >= MAX_CONSECUTIVE_SENSOR_FAILURES {
            release(
                &helper,
                paths,
                &mut session,
                &format!(
                    "battery safety sensor failed {power_sensor_failures} consecutive times: {}",
                    power_error.as_deref().unwrap_or("unknown failure")
                ),
                |_| true,
            )?;
            return Ok(());
        }

        let thermal_error = match read_thermal_status() {
            Ok(thermal) if thermal.level == ThermalLevel::Unknown => {
                Some("thermal pressure is unknown".to_owned())
            }
            Ok(thermal) => {
                if thermal.throttled {
                    release(
                        &helper,
                        paths,
                        &mut session,
                        "thermal pressure detected",
                        |_| true,
                    )?;
                    return Ok(());
                }
                None
            }
            Err(error) => Some(error.to_string()),
        };
        thermal_sensor_failures =
            next_sensor_failure_count(thermal_sensor_failures, thermal_error.is_none());
        if thermal_sensor_failures >= MAX_CONSECUTIVE_SENSOR_FAILURES {
            release(
                &helper,
                paths,
                &mut session,
                &format!(
                    "thermal safety sensor failed {thermal_sensor_failures} consecutive times: {}",
                    thermal_error.as_deref().unwrap_or("unknown failure")
                ),
                |_| true,
            )?;
            return Ok(());
        }
        if power_error.is_some() || thermal_error.is_some() {
            session.last_event = Some(format!(
                "safety sensor retry: power={} thermal={}",
                power_error.as_deref().unwrap_or("ok"),
                thermal_error.as_deref().unwrap_or("ok")
            ));
        }

        let wifi = read_wifi_status().ok();
        session.observed_hotspot_ssid = wifi.as_ref().and_then(|status| status.ssid.clone());
        let (route_reachable, route_detail) = match read_default_route() {
            Ok(route) => {
                if let Some(reason) = commute_network_change(
                    session.expected_hotspot_ssid.as_deref(),
                    session.commute_route_interface.as_deref(),
                    session.commute_route_gateway.as_deref(),
                    wifi.as_ref(),
                    &route,
                ) {
                    release(&helper, paths, &mut session, &reason, |_| true)?;
                    return Ok(());
                }
                let reachable = route.interface.is_some();
                session.route_interface = route.interface;
                (reachable, route.detail)
            }
            Err(error) => {
                session.route_interface = None;
                (false, error.to_string())
            }
        };

        let internet = internet_probe(
            DEFAULT_INTERNET_PROBE_URL,
            config.hotspot.probe_timeout_seconds,
        );
        let provider = probe(
            provider_probe_url(session.agent),
            config.hotspot.probe_timeout_seconds,
        );
        let remote_path_reachable = route_reachable && internet.reachable && provider.reachable;
        let previous_network = session.network_reachable;
        session.network_reachable = Some(remote_path_reachable);

        let network_now = Instant::now();
        if !remote_path_reachable && network_outage_started.is_none() {
            network_outage_started = Some(reconstruct_outage_start(
                session.network_outage_started_at,
                Utc::now(),
                network_now,
                Duration::from_secs(config.session.network_outage_grace_seconds),
            ));
        }
        let outage = evaluate_network_outage(
            remote_path_reachable,
            network_outage_started,
            network_now,
            Duration::from_secs(config.session.network_outage_grace_seconds),
        );
        match outage {
            NetworkOutage::Online => {
                network_outage_started = None;
                session.network_outage_started_at = None;
                if session.phase == SessionPhase::TemporarilyOffline {
                    session.phase = session
                        .phase_before_offline
                        .take()
                        .unwrap_or(SessionPhase::Active);
                }
                if previous_network == Some(false) {
                    session.last_event = Some("mobile remote path restored".to_owned());
                }
            }
            NetworkOutage::WithinGrace {
                started_at,
                elapsed,
            } => {
                network_outage_started = Some(started_at);
                if session.network_outage_started_at.is_none() {
                    session.network_outage_started_at = Some(Utc::now());
                }
                if session.phase != SessionPhase::TemporarilyOffline {
                    session.phase_before_offline = Some(session.phase);
                    session.phase = SessionPhase::TemporarilyOffline;
                }
                session.last_event = Some(format!(
                    "remote path unavailable for {}s: route={} internet={} provider={}",
                    elapsed.as_secs(),
                    route_detail,
                    internet.detail,
                    provider.detail
                ));
            }
            NetworkOutage::Release { elapsed } => {
                release(
                    &helper,
                    paths,
                    &mut session,
                    &format!(
                        "remote path unavailable for {}s (grace {}s)",
                        elapsed.as_secs(),
                        config.session.network_outage_grace_seconds
                    ),
                    |_| true,
                )?;
                return Ok(());
            }
        }

        if remote_path_reachable {
            session.phase = match session.phase {
                SessionPhase::Preflight
                | SessionPhase::PolicyActive
                | SessionPhase::WaitingForHotspot
                | SessionPhase::WaitingForUnplug
                | SessionPhase::Ready => SessionPhase::Active,
                _ => session.phase,
            };
        }
        if !persist_heartbeat(&mut session, paths)? {
            log(
                paths,
                "session changed during heartbeat; preserving the newer state and retrying",
            )?;
            continue;
        }
        log(
            paths,
            &format!(
                "heartbeat ok battery={:?} route={:?} internet={} provider={} power_sensor_failures={} thermal_sensor_failures={}",
                session.battery_percent,
                session.route_interface,
                internet.reachable,
                provider.reachable,
                power_sensor_failures,
                thermal_sensor_failures
            ),
        )?;

        thread::sleep(Duration::from_secs(config.session.heartbeat_seconds));
    }
}

fn effective_phase(session: &SessionState) -> SessionPhase {
    if session.phase == SessionPhase::TemporarilyOffline {
        session.phase_before_offline.unwrap_or(SessionPhase::Active)
    } else {
        session.phase
    }
}

fn commute_network_change(
    expected_ssid: Option<&str>,
    expected_interface: Option<&str>,
    expected_gateway: Option<&str>,
    wifi: Option<&WifiStatus>,
    route: &RouteStatus,
) -> Option<String> {
    if let (Some(expected), Some(observed)) = (
        expected_ssid,
        wifi.and_then(|status| status.ssid.as_deref()),
    ) {
        if expected != observed {
            return Some(format!(
                "configured hotspot {expected:?} was replaced by Wi-Fi {observed:?}"
            ));
        }
    }

    if let (Some(expected), Some(observed)) = (expected_interface, route.interface.as_deref()) {
        if expected != observed {
            return Some(format!(
                "commute route moved from interface {expected:?} to {observed:?}"
            ));
        }
    }

    if let (Some(expected), Some(observed)) = (expected_gateway, route.gateway.as_deref()) {
        if expected != observed {
            return Some(format!(
                "commute route moved from gateway {expected:?} to {observed:?}"
            ));
        }
    }
    None
}

fn persist_heartbeat(session: &mut SessionState, paths: &AppPaths) -> Result<bool> {
    match session.save(paths) {
        Ok(()) => Ok(true),
        Err(error) if error.downcast_ref::<SessionStateWriteConflict>().is_some() => Ok(false),
        Err(error) => Err(error),
    }
}

fn next_sensor_failure_count(current: u8, successful: bool) -> u8 {
    if successful {
        0
    } else {
        current.saturating_add(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NetworkOutage {
    Online,
    WithinGrace {
        started_at: Instant,
        elapsed: Duration,
    },
    Release {
        elapsed: Duration,
    },
}

fn evaluate_network_outage(
    reachable: bool,
    started_at: Option<Instant>,
    now: Instant,
    grace: Duration,
) -> NetworkOutage {
    if reachable {
        return NetworkOutage::Online;
    }
    let started_at = started_at.unwrap_or(now);
    let elapsed = now.saturating_duration_since(started_at);
    if elapsed >= grace {
        NetworkOutage::Release { elapsed }
    } else {
        NetworkOutage::WithinGrace {
            started_at,
            elapsed,
        }
    }
}

fn reconstruct_outage_start(
    persisted_started_at: Option<DateTime<Utc>>,
    wall_now: DateTime<Utc>,
    monotonic_now: Instant,
    grace: Duration,
) -> Instant {
    let Some(persisted_started_at) = persisted_started_at else {
        return monotonic_now;
    };
    let wall_age = wall_now
        .signed_duration_since(persisted_started_at)
        .to_std()
        .unwrap_or(Duration::ZERO);
    let conservative_age = wall_age.min(grace);
    monotonic_now
        .checked_sub(conservative_age)
        .unwrap_or(monotonic_now)
}

fn release(
    helper: &HelperClient,
    paths: &AppPaths,
    session: &mut SessionState,
    reason: &str,
    should_release: impl FnOnce(&SessionState) -> bool,
) -> Result<bool> {
    let Some(releasing) = SessionState::update(paths, session.id, |mut current| {
        if should_release(&current) {
            current.phase = SessionPhase::Releasing;
            current.release_reason = Some(reason.to_owned());
        }
        Ok(current)
    })?
    else {
        return Ok(false);
    };
    if releasing.phase != SessionPhase::Releasing {
        *session = releasing;
        return Ok(false);
    }
    *session = releasing;

    let result = helper.release(session.lease_id, reason);
    match result {
        Ok(status)
            if !status.active && status.sleep_disabled == session.previous_sleep_disabled =>
        {
            log(paths, &format!("lease released: {reason}"))?;
        }
        Ok(status) => {
            log(
                paths,
                &format!("lease release returned unexpected state: {:?}", status),
            )?;
            anyhow::bail!(
                "The helper did not prove normal sleep was restored; leaving the session in recovery state"
            );
        }
        Err(error) => {
            log(paths, &format!("lease release failed: {error}"))?;
            return Err(error).context("Could not restore normal sleep");
        }
    }

    cleanup_policy(paths, session.agent, &session.project_dir)?;
    if let Some(released) = SessionState::update(paths, session.id, |mut current| {
        current.phase = SessionPhase::Released;
        current.last_event = Some(format!("normal sleep restored: {reason}"));
        Ok(current)
    })? {
        *session = released;
    }
    Ok(true)
}

pub fn cleanup_policy(paths: &AppPaths, agent: AgentKind, project: &std::path::Path) -> Result<()> {
    if agent == AgentKind::Cursor {
        deactivate_cursor_rule(project)?;
    }
    ActivePolicy::clear(paths)
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

    #[test]
    fn sensor_failures_are_consecutive_and_bounded() {
        assert_eq!(next_sensor_failure_count(0, false), 1);
        assert_eq!(next_sensor_failure_count(1, false), 2);
        assert_eq!(next_sensor_failure_count(2, false), 3);
        assert_eq!(next_sensor_failure_count(2, true), 0);
    }

    #[test]
    fn network_outage_releases_at_grace_boundary() {
        let now = Instant::now();
        let grace = Duration::from_secs(300);
        let started_at = now.checked_sub(grace).unwrap();
        assert_eq!(
            evaluate_network_outage(false, Some(started_at), now, grace),
            NetworkOutage::Release { elapsed: grace }
        );
        assert_eq!(
            evaluate_network_outage(true, Some(started_at), now, grace),
            NetworkOutage::Online
        );
    }

    #[test]
    fn persisted_outage_age_survives_watcher_restart() {
        let wall_now = DateTime::parse_from_rfc3339("2026-07-24T10:05:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let persisted = wall_now - chrono::Duration::minutes(5);
        let monotonic_now = Instant::now();
        let grace = Duration::from_secs(300);
        let reconstructed =
            reconstruct_outage_start(Some(persisted), wall_now, monotonic_now, grace);
        assert_eq!(
            evaluate_network_outage(false, Some(reconstructed), monotonic_now, grace),
            NetworkOutage::Release { elapsed: grace }
        );
    }

    #[test]
    fn returning_to_a_different_live_network_ends_the_commute_route() {
        let hotspot = WifiStatus {
            device: Some("en0".to_owned()),
            connected: true,
            ssid: Some("Noah".to_owned()),
            redacted: false,
            detail: "test".to_owned(),
        };
        let same_route = RouteStatus {
            interface: Some("en0".to_owned()),
            gateway: Some("172.20.10.1".to_owned()),
            detail: "test".to_owned(),
        };
        assert!(commute_network_change(
            Some("Noah"),
            Some("en0"),
            Some("172.20.10.1"),
            Some(&hotspot),
            &same_route,
        )
        .is_none());

        let normal_wifi = WifiStatus {
            ssid: Some("zeitgeistX".to_owned()),
            ..hotspot.clone()
        };
        assert!(commute_network_change(
            Some("Noah"),
            Some("en0"),
            Some("172.20.10.1"),
            Some(&normal_wifi),
            &same_route,
        )
        .is_some());

        let office_route = RouteStatus {
            gateway: Some("192.168.1.1".to_owned()),
            ..same_route
        };
        assert!(commute_network_change(
            Some("Noah"),
            Some("en0"),
            Some("172.20.10.1"),
            None,
            &office_route,
        )
        .is_some());
    }

    #[test]
    fn a_missing_route_uses_the_reconnect_grace_instead_of_auto_arrival() {
        let missing_route = RouteStatus {
            interface: None,
            gateway: None,
            detail: "no route".to_owned(),
        };

        assert!(commute_network_change(
            Some("Noah"),
            Some("en0"),
            Some("172.20.10.1"),
            None,
            &missing_route,
        )
        .is_none());
    }
}
