use crate::helper_client::HelperClient;
use crate::report;
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use rucksack_core::agent::{deactivate_cursor_rule, AgentKind};
use rucksack_core::files::{append_line, with_advisory_lock};
use rucksack_core::network::{
    internet_probe, probe, provider_probe_url, read_default_route, read_wifi_status, RouteStatus,
    WifiStatus, DEFAULT_INTERNET_PROBE_URL,
};
use rucksack_core::power::{
    read_power_status, read_thermal_status, PowerSource, ThermalLevel, ThermalStatus,
};
use rucksack_core::state::{
    ActivePolicy, SessionEndKind, SessionPhase, SessionState, SessionStateWriteConflict,
};
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
    let daemon_pid = std::process::id();
    let Some(started) = SessionState::update(paths, session_id, |session| {
        establish_watcher_state(session, daemon_pid, Utc::now())
    })?
    else {
        anyhow::bail!("Session state was removed before the safety watcher started");
    };
    log(
        paths,
        &format!(
            "safety watcher established at revision {}",
            started.revision
        ),
    )?;

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
                cleanup_policy(paths, session.id, session.agent, &session.project_dir)?;
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
                if let Some(reason) = thermal_release_reason(&thermal) {
                    release(&helper, paths, &mut session, &reason, |_| true)?;
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
                report::sample_mobile_data(&mut session, false);
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

fn establish_watcher_state(
    mut session: SessionState,
    daemon_pid: u32,
    established_at: DateTime<Utc>,
) -> Result<SessionState> {
    let initial_phase = session.phase;
    session.phase = watcher_start_phase(initial_phase)?;
    session.daemon_pid = Some(daemon_pid);
    session.last_heartbeat_at = Some(established_at);
    if initial_phase == SessionPhase::Ready {
        session.last_event = Some("safety watcher established".to_owned());
    }
    Ok(session)
}

fn watcher_start_phase(phase: SessionPhase) -> Result<SessionPhase> {
    if matches!(
        phase,
        SessionPhase::Releasing | SessionPhase::Released | SessionPhase::Failed
    ) {
        anyhow::bail!(
            "Cannot establish the safety watcher while session state is {:?}",
            phase
        );
    }
    if phase == SessionPhase::Ready {
        Ok(SessionPhase::Active)
    } else {
        Ok(phase)
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

fn thermal_release_reason(thermal: &ThermalStatus) -> Option<String> {
    if !thermal.throttled
        && !matches!(
            thermal.level,
            ThermalLevel::Serious | ThermalLevel::Critical
        )
    {
        return None;
    }

    let cpu_speed = thermal
        .cpu_speed_limit_percent
        .map(|percent| format!("{percent}%"))
        .unwrap_or_else(|| "unknown".to_owned());
    let scheduler = thermal
        .scheduler_limit_percent
        .map(|percent| format!("{percent}%"))
        .unwrap_or_else(|| "unknown".to_owned());
    let available_cpus = thermal
        .available_cpus
        .map(|count| count.to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    Some(format!(
        "thermal safety release: level={:?}, CPU speed limit={cpu_speed}, scheduler limit={scheduler}, available CPUs={available_cpus}",
        thermal.level
    ))
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
    let terminal_lock = paths.terminal_lock_file();
    with_advisory_lock(&terminal_lock, || {
        release_locked(helper, paths, session, reason, should_release)
    })
}

fn release_locked(
    helper: &HelperClient,
    paths: &AppPaths,
    session: &mut SessionState,
    reason: &str,
    should_release: impl FnOnce(&SessionState) -> bool,
) -> Result<bool> {
    let terminal_battery_percent = session.battery_percent;
    let terminal_network_reachable = session.network_reachable;
    let terminal_route_interface = session.route_interface.clone();
    let terminal_observed_hotspot_ssid = session.observed_hotspot_ssid.clone();
    let mut release_selected = false;
    let Some(releasing) = SessionState::update(paths, session.id, |mut current| {
        if should_release(&current) {
            release_selected = true;
            current.phase = SessionPhase::Releasing;
            current.release_reason = Some(reason.to_owned());
        }
        Ok(current)
    })?
    else {
        return Ok(false);
    };
    if !release_selected {
        *session = releasing;
        return Ok(false);
    }
    *session = releasing;

    let result = helper.release(session.lease_id, reason);
    match result {
        Ok(status)
            if !status.active && status.sleep_disabled == session.previous_sleep_disabled => {}
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

    // Final accounting is best-effort and must never delay restoration of normal sleep.
    let released_at = Utc::now();
    report::sample_mobile_data(session, true);
    session.ended_at = Some(released_at);
    session.battery_percent = terminal_battery_percent;
    session.network_reachable = terminal_network_reachable;
    session.route_interface = terminal_route_interface;
    session.observed_hotspot_ssid = terminal_observed_hotspot_ssid;
    let final_accounting = session.clone();
    let finalizing_result = SessionState::update(paths, session.id, |mut current| {
        current.ended_at = final_accounting.ended_at;
        current.battery_percent = final_accounting.battery_percent;
        current.network_reachable = final_accounting.network_reachable;
        current.route_interface = final_accounting.route_interface.clone();
        current.observed_hotspot_ssid = final_accounting.observed_hotspot_ssid.clone();
        current.mobile_data_end = final_accounting.mobile_data_end.clone();
        current.mobile_data_finalized = final_accounting.mobile_data_finalized;
        current.mobile_data_error = final_accounting.mobile_data_error.clone();
        Ok(current)
    });
    let mut completion_errors = Vec::new();
    match finalizing_result {
        Ok(Some(finalizing)) => *session = finalizing,
        Ok(None) => {}
        Err(error) => completion_errors.push(format!(
            "final session accounting could not be persisted: {error:#}"
        )),
    }
    if let Err(error) = cleanup_policy(paths, session.id, session.agent, &session.project_dir) {
        completion_errors.push(format!("commute mode cleanup failed: {error:#}"));
    }
    if let Err(error) = report::archive_session(paths, session, SessionEndKind::Automatic) {
        completion_errors.push(format!(
            "the completed-session report could not be saved: {error:#}"
        ));
    }
    if !completion_errors.is_empty() {
        let message = format!(
            "normal sleep was restored but {}",
            completion_errors.join("; ")
        );
        log(paths, &message)?;
        return Err(anyhow!(message));
    }
    if let Some(released) = SessionState::update(paths, session.id, |mut current| {
        current.phase = SessionPhase::Released;
        current.last_event = Some(format!("normal sleep restored: {reason}"));
        Ok(current)
    })? {
        *session = released;
    }
    log(
        paths,
        &format!("lease released and completed-session report saved: {reason}"),
    )?;
    Ok(true)
}

pub fn cleanup_policy(
    paths: &AppPaths,
    session_id: Uuid,
    agent: AgentKind,
    project: &std::path::Path,
) -> Result<()> {
    if agent != AgentKind::Cursor {
        ActivePolicy::clear_if_session(paths, session_id)?;
        return Ok(());
    }

    let pending_result = ActivePolicy::set_cleanup_pending(paths, session_id, true);
    match deactivate_cursor_rule(project) {
        Ok(_) => {
            ActivePolicy::clear_if_session(paths, session_id)?;
            Ok(())
        }
        Err(cursor_error) => match pending_result {
            Ok(_) => Err(cursor_error).context(
                "Could not remove Cursor commute files; inactive cleanup state was preserved",
            ),
            Err(policy_error) => Err(anyhow!(
                "Could not preserve inactive Cursor cleanup state: {policy_error:#}; could not remove Cursor commute files: {cursor_error:#}"
            )),
        },
    }
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
    use rucksack_core::agent::activate_cursor_rule;
    use rucksack_core::protocol::{HelperOperation, HelperRequest, HelperResponse, HelperStatus};
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn test_paths(root: &Path) -> AppPaths {
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

    fn cursor_session(project_dir: PathBuf) -> SessionState {
        let now = Utc::now();
        SessionState {
            version: 1,
            revision: 0,
            id: Uuid::new_v4(),
            lease_id: Uuid::new_v4(),
            owner_uid: 501,
            agent: AgentKind::Cursor,
            project_dir,
            provider_session_id: None,
            focus: rucksack_core::Focus::Continue,
            phase: SessionPhase::Active,
            started_at: now,
            expires_at: now + chrono::Duration::minutes(30),
            last_heartbeat_at: None,
            daemon_pid: None,
            expected_hotspot_ssid: Some("Noah".to_owned()),
            observed_hotspot_ssid: Some("Noah".to_owned()),
            commute_route_interface: Some("en0".to_owned()),
            commute_route_gateway: Some("172.20.10.1".to_owned()),
            route_interface: Some("en0".to_owned()),
            start_battery_percent: Some(80),
            battery_percent: Some(78),
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

    fn active_policy(session: &SessionState) -> ActivePolicy {
        ActivePolicy {
            version: 1,
            session_id: session.id,
            agent: session.agent,
            focus: session.focus,
            project_dir: session.project_dir.clone(),
            provider_session_id: None,
            confirmation_token: Some("rucksack-test-0123456789abcdef".to_owned()),
            cleanup_pending: false,
            activated_at: session.started_at,
            expires_at: session.expires_at,
            policy: "test policy".to_owned(),
        }
    }

    #[test]
    fn sensor_failures_are_consecutive_and_bounded() {
        assert_eq!(next_sensor_failure_count(0, false), 1);
        assert_eq!(next_sensor_failure_count(1, false), 2);
        assert_eq!(next_sensor_failure_count(2, false), 3);
        assert_eq!(next_sensor_failure_count(2, true), 0);
    }

    #[test]
    fn watcher_handshake_precedes_terminal_release_paths() {
        assert_eq!(
            watcher_start_phase(SessionPhase::Ready).unwrap(),
            SessionPhase::Active
        );
        assert_eq!(
            watcher_start_phase(SessionPhase::Completed).unwrap(),
            SessionPhase::Completed
        );
        assert!(watcher_start_phase(SessionPhase::Releasing).is_err());
        assert!(watcher_start_phase(SessionPhase::Released).is_err());
        assert!(watcher_start_phase(SessionPhase::Failed).is_err());
    }

    #[test]
    fn thermal_pressure_or_cpu_throttling_releases_the_lease() {
        let nominal = ThermalStatus {
            level: ThermalLevel::Nominal,
            cpu_speed_limit_percent: Some(100),
            scheduler_limit_percent: Some(100),
            available_cpus: Some(10),
            throttled: false,
            raw: String::new(),
        };
        assert!(thermal_release_reason(&nominal).is_none());

        let fair = ThermalStatus {
            level: ThermalLevel::Fair,
            ..nominal.clone()
        };
        assert!(thermal_release_reason(&fair).is_none());

        let serious = ThermalStatus {
            level: ThermalLevel::Serious,
            ..nominal.clone()
        };
        assert!(thermal_release_reason(&serious)
            .unwrap()
            .contains("level=Serious"));

        let critical = ThermalStatus {
            level: ThermalLevel::Critical,
            ..nominal.clone()
        };
        assert!(thermal_release_reason(&critical)
            .unwrap()
            .contains("level=Critical"));

        let throttled = ThermalStatus {
            level: ThermalLevel::Nominal,
            cpu_speed_limit_percent: Some(80),
            scheduler_limit_percent: Some(70),
            available_cpus: Some(8),
            throttled: true,
            raw: String::new(),
        };
        let reason = thermal_release_reason(&throttled).unwrap();
        assert!(reason.contains("level=Nominal"));
        assert!(reason.contains("CPU speed limit=80%"));
        assert!(reason.contains("scheduler limit=70%"));
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
    fn a_missing_route_uses_the_reconnect_grace_instead_of_automatic_unpack() {
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

    #[test]
    fn helper_expiry_before_daemon_release_still_finishes_accounting() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let project = directory.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let mut session = cursor_session(project);
        session.agent = AgentKind::Codex;
        session.save(&paths).unwrap();
        active_policy(&session).save(&paths).unwrap();

        let socket = directory.path().join("helper.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let lease_id = session.lease_id;
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request_line)
                .unwrap();
            let request: HelperRequest = serde_json::from_str(&request_line).unwrap();
            assert!(matches!(
                &request.operation,
                HelperOperation::Release {
                    lease_id: requested_lease_id,
                    ..
                } if *requested_lease_id == lease_id
            ));
            let response = HelperResponse::failure(
                request.request_id,
                "no active lease",
                Some(HelperStatus {
                    active: false,
                    lease_id: None,
                    owner_uid: None,
                    created_at: None,
                    expires_at: None,
                    hard_expires_at: None,
                    previous_sleep_disabled: None,
                    sleep_disabled: Some(0),
                    reason: None,
                    last_reasserted_at: None,
                }),
            );
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            stream.write_all(&bytes).unwrap();
            stream.flush().unwrap();
        });

        assert!(release_locked(
            &HelperClient::new(&socket),
            &paths,
            &mut session,
            "hard timeout reached",
            |_| true,
        )
        .unwrap());

        let persisted = SessionState::load(&paths).unwrap().unwrap();
        assert_eq!(persisted.phase, SessionPhase::Released);
        assert_eq!(
            persisted.release_reason.as_deref(),
            Some("hard timeout reached")
        );
        assert!(ActivePolicy::load(&paths).unwrap().is_none());
        let report = rucksack_core::SessionReport::load(&paths).unwrap().unwrap();
        assert_eq!(report.session_id, session.id);
        assert_eq!(report.release_reason, "hard timeout reached");
        server.join().unwrap();
    }

    #[test]
    fn automatic_release_attempts_cleanup_and_report_after_final_accounting_fails() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let project = directory.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let mut session = cursor_session(project.clone());
        session.save(&paths).unwrap();
        let policy = active_policy(&session);
        policy.save(&paths).unwrap();
        activate_cursor_rule(&project, &policy).unwrap();

        let socket = directory.path().join("helper.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let lease_id = session.lease_id;
        let session_file = paths.session_file.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request_line)
                .unwrap();
            let request: HelperRequest = serde_json::from_str(&request_line).unwrap();
            assert!(matches!(
                &request.operation,
                HelperOperation::Release {
                    lease_id: requested_lease_id,
                    ..
                } if *requested_lease_id == lease_id
            ));
            fs::remove_file(&session_file).unwrap();
            fs::create_dir(&session_file).unwrap();
            let response = HelperResponse::success(
                request.request_id,
                Some(HelperStatus {
                    active: false,
                    lease_id: None,
                    owner_uid: None,
                    created_at: None,
                    expires_at: None,
                    hard_expires_at: None,
                    previous_sleep_disabled: None,
                    sleep_disabled: Some(0),
                    reason: None,
                    last_reasserted_at: None,
                }),
            );
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            stream.write_all(&bytes).unwrap();
            stream.flush().unwrap();
        });

        let error = release_locked(
            &HelperClient::new(&socket),
            &paths,
            &mut session,
            "test release",
            |_| true,
        )
        .unwrap_err();

        let message = format!("{error:#}");
        assert!(message.contains("final session accounting could not be persisted"));
        assert!(message.contains("the completed-session report could not be saved"));
        assert!(ActivePolicy::load(&paths).unwrap().is_none());
        assert!(!project.join(".cursor/rules/rucksack-commute.mdc").exists());
        assert!(!project.join(".cursor/commands/commute-mode.md").exists());
        assert!(rucksack_core::SessionReport::load(&paths)
            .unwrap()
            .is_none());
        assert!(paths.session_file.is_dir());
        server.join().unwrap();
    }

    #[test]
    fn automatic_release_removes_policy_and_cursor_files_when_report_archival_fails() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let project = directory.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let mut session = cursor_session(project.clone());
        session.save(&paths).unwrap();
        let policy = active_policy(&session);
        policy.save(&paths).unwrap();
        activate_cursor_rule(&project, &policy).unwrap();
        assert!(project.join(".cursor/rules/rucksack-commute.mdc").exists());
        assert!(project.join(".cursor/commands/commute-mode.md").exists());

        fs::create_dir_all(&paths.report_file).unwrap();
        let socket = directory.path().join("helper.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let lease_id = session.lease_id;
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request_line)
                .unwrap();
            let request: HelperRequest = serde_json::from_str(&request_line).unwrap();
            assert!(matches!(
                &request.operation,
                HelperOperation::Release {
                    lease_id: requested_lease_id,
                    ..
                } if *requested_lease_id == lease_id
            ));
            let response = HelperResponse::success(
                request.request_id,
                Some(HelperStatus {
                    active: false,
                    lease_id: None,
                    owner_uid: None,
                    created_at: None,
                    expires_at: None,
                    hard_expires_at: None,
                    previous_sleep_disabled: None,
                    sleep_disabled: Some(0),
                    reason: None,
                    last_reasserted_at: None,
                }),
            );
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            stream.write_all(&bytes).unwrap();
            stream.flush().unwrap();
        });

        let result = release_locked(
            &HelperClient::new(&socket),
            &paths,
            &mut session,
            "test release",
            |_| true,
        );

        assert!(result.is_err());
        assert!(ActivePolicy::load(&paths).unwrap().is_none());
        assert!(!project.join(".cursor/rules/rucksack-commute.mdc").exists());
        assert!(!project.join(".cursor/commands/commute-mode.md").exists());
        assert_eq!(
            SessionState::load(&paths).unwrap().unwrap().phase,
            SessionPhase::Releasing
        );
        server.join().unwrap();
    }

    #[test]
    fn automatic_release_archives_the_report_when_cursor_cleanup_fails() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let project = directory.path().join("project");
        let mut session = cursor_session(project.clone());
        session.save(&paths).unwrap();
        active_policy(&session).save(&paths).unwrap();
        let rule = project.join(".cursor/rules/rucksack-commute.mdc");
        fs::create_dir_all(rule.parent().unwrap()).unwrap();
        fs::write(&rule, "unowned rule").unwrap();

        let socket = directory.path().join("helper.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let lease_id = session.lease_id;
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request_line)
                .unwrap();
            let request: HelperRequest = serde_json::from_str(&request_line).unwrap();
            assert!(matches!(
                &request.operation,
                HelperOperation::Release {
                    lease_id: requested_lease_id,
                    ..
                } if *requested_lease_id == lease_id
            ));
            let response = HelperResponse::success(
                request.request_id,
                Some(HelperStatus {
                    active: false,
                    lease_id: None,
                    owner_uid: None,
                    created_at: None,
                    expires_at: None,
                    hard_expires_at: None,
                    previous_sleep_disabled: None,
                    sleep_disabled: Some(0),
                    reason: None,
                    last_reasserted_at: None,
                }),
            );
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            stream.write_all(&bytes).unwrap();
            stream.flush().unwrap();
        });

        let result = release_locked(
            &HelperClient::new(&socket),
            &paths,
            &mut session,
            "test release",
            |_| true,
        );

        assert!(result.is_err());
        let retained_policy = ActivePolicy::load(&paths).unwrap().unwrap();
        assert_eq!(retained_policy.session_id, session.id);
        assert!(retained_policy.cleanup_pending);
        assert!(!retained_policy.is_active(Utc::now()));
        assert!(rule.exists());
        assert_eq!(
            rucksack_core::SessionReport::load(&paths)
                .unwrap()
                .unwrap()
                .session_id,
            session.id
        );
        assert_eq!(
            SessionState::load(&paths).unwrap().unwrap().phase,
            SessionPhase::Releasing
        );
        server.join().unwrap();
    }

    #[test]
    fn cleanup_policy_retains_inactive_locator_after_cursor_cleanup_fails() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let project = directory.path().join("project");
        let session = cursor_session(project.clone());
        active_policy(&session).save(&paths).unwrap();
        let rule = project.join(".cursor/rules/rucksack-commute.mdc");
        fs::create_dir_all(rule.parent().unwrap()).unwrap();
        fs::write(&rule, "unowned rule").unwrap();

        assert!(cleanup_policy(&paths, session.id, AgentKind::Cursor, &project).is_err());
        let retained_policy = ActivePolicy::load(&paths).unwrap().unwrap();
        assert_eq!(retained_policy.session_id, session.id);
        assert!(retained_policy.cleanup_pending);
        assert!(!retained_policy.is_active(Utc::now()));
        assert!(rule.exists());
    }
}
