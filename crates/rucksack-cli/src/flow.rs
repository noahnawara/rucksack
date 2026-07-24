use crate::cli::{ArriveArgs, LeaveArgs, RecoverArgs, StatusArgs};
use crate::daemon::cleanup_policy;
use crate::helper_client::HelperClient;
use crate::output::Output;
use anyhow::{anyhow, Context, Result};
use chrono::{Duration as ChronoDuration, Utc};
use rucksack_core::agent::{
    activate_cursor_rule, claude_remote_user_instruction, codex_remote_start, codex_remote_stop,
    detect, detect_all, install_adapters, verify_adapter, AdapterFileStatus, AgentAdapterEvidence,
    AgentDetection, AgentKind, ProjectMatch,
};
use rucksack_core::files::ensure_private_dir;
use rucksack_core::network::{
    connect_saved_wifi, internet_probe, probe, provider_probe_url, read_default_route,
    read_iphone_usb_device, read_wifi_status, ProbeResult, RouteStatus, WifiStatus,
    DEFAULT_INTERNET_PROBE_URL,
};
use rucksack_core::policy::{render_policy, PolicyContext};
use rucksack_core::power::{
    read_active_sleep_utilities, read_power_status, read_sleep_disabled, read_thermal_status,
    PowerSource, ThermalLevel, ThermalStatus,
};
use rucksack_core::state::{ActivePolicy, SessionPhase, SessionState};
use rucksack_core::system::current_uid;
use rucksack_core::{AppPaths, Config};
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

const PREFLIGHT_PROBE_ATTEMPTS: u8 = 3;
const PREFLIGHT_PROBE_RETRY_DELAY: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RedactedWifiEvidence {
    SavedNetworkJoin,
    UserConfirmation,
}

#[derive(Debug, Clone)]
struct VerifiedWifi {
    status: WifiStatus,
    redacted_evidence: Option<RedactedWifiEvidence>,
}

pub fn leave(
    args: &LeaveArgs,
    output: &Output,
    paths: &AppPaths,
    base_config: &Config,
) -> Result<()> {
    let mut cleanup = LeaveCleanup::new(paths);
    match leave_inner(args, output, paths, base_config, &mut cleanup) {
        Ok(()) => {
            cleanup.committed = true;
            Ok(())
        }
        Err(error) => {
            let rollback_errors = cleanup.rollback();
            cleanup.committed = true;
            if rollback_errors.is_empty() {
                Err(error)
            } else {
                Err(anyhow!(
                    "{error:#}\nPreflight rollback was incomplete:\n- {}",
                    rollback_errors.join("\n- ")
                ))
            }
        }
    }
}

fn leave_inner(
    args: &LeaveArgs,
    output: &Output,
    paths: &AppPaths,
    base_config: &Config,
    cleanup: &mut LeaveCleanup<'_>,
) -> Result<()> {
    if !cfg!(target_os = "macos") {
        anyhow::bail!("Closed-lid Commute Mode currently requires macOS");
    }
    if let Some(existing) = SessionState::load(paths)? {
        if !matches!(
            existing.phase,
            SessionPhase::Released | SessionPhase::Failed
        ) {
            anyhow::bail!(
                "Rucksack is already active for {}. Run `rucksack status` or `rucksack arrive`.",
                existing.agent.display_name()
            );
        }
    }

    let mut config = base_config.clone();
    if let Some(ssid) = &args.hotspot {
        config.hotspot.ssid = Some(ssid.clone());
        config.hotspot.require_verified_ssid = true;
        config.hotspot.require_iphone_usb = false;
    } else if args.usb {
        config.hotspot.ssid = None;
        config.hotspot.require_verified_ssid = false;
        config.hotspot.require_iphone_usb = true;
    }
    if let Some(minutes) = args.duration_minutes {
        config.session.duration_minutes = minutes;
    }
    if let Some(focus) = args.focus {
        config.session.focus = focus;
    }
    config
        .validate()
        .map_err(|error| anyhow!("Configuration is not safe: {error}"))?;
    let require_verified_wifi = if args.hotspot.is_some() {
        true
    } else {
        config.hotspot.require_verified_ssid
    };
    let require_iphone_usb = config.hotspot.require_iphone_usb;
    if require_verified_wifi && config.hotspot.ssid.is_none() {
        anyhow::bail!(
            "No commute hotspot is configured. Run `rucksack setup --hotspot \"My iPhone\"`, pass `--hotspot`, or use `--usb`."
        );
    }
    require_no_active_sleep_utilities()?;

    let helper = HelperClient::default();
    helper
        .status()
        .context("Power helper unavailable. Run `rucksack helper install` first.")?;

    let project_dir = std::env::current_dir()?
        .canonicalize()
        .unwrap_or(std::env::current_dir()?);
    let agent = select_agent(args.agent.or(config.default_agent), &project_dir, output)?;

    output.title("Preparing this Mac for the walk home");
    output.section("Agent");
    let detection = detect(agent, Some(&project_dir))?;
    if detection.installed {
        output.pass(format!("{} is installed", agent.display_name()));
    } else {
        anyhow::bail!("{} was not found on this Mac", agent.display_name());
    }
    match detection.project_match {
        ProjectMatch::Matched => output.pass(&detection.detail),
        ProjectMatch::Different | ProjectMatch::Unknown | ProjectMatch::NotRunning => {
            output.warn(&detection.detail)
        }
        ProjectMatch::NotRequested if detection.running => output.pass(&detection.detail),
        ProjectMatch::NotRequested => output.warn(&detection.detail),
    }

    ensure_adapter(paths, agent, args.yes, output)?;

    let now = Utc::now();
    let expires_at = now + ChronoDuration::minutes(config.session.duration_minutes as i64);
    let policy = ActivePolicy {
        version: 1,
        session_id: Uuid::new_v4(),
        agent,
        focus: config.session.focus,
        project_dir: project_dir.clone(),
        activated_at: now,
        expires_at,
        policy: render_policy(&PolicyContext {
            focus: config.session.focus,
            minutes_remaining: config.session.duration_minutes,
            battery_floor_percent: config.safety.sleep_battery_percent,
            project_name: project_name(&project_dir),
        }),
    };
    policy.save(paths)?;
    cleanup.policy_active = true;
    if agent == AgentKind::Cursor {
        activate_cursor_rule(&project_dir, &policy)?;
        cleanup.cursor_project = Some(project_dir.clone());
    }
    output.pass(format!("Commute policy loaded · {}", config.session.focus));

    let remote = prepare_remote(agent, &detection, args, output)?;
    cleanup.remote_agent = Some(agent);
    cleanup.remote_owned = remote.owned;
    cleanup.remote_pid = remote.pid;

    output.blank();
    output.section("Connection");
    let expected_ssid = config.hotspot.ssid.as_deref();
    let (verified_wifi, route) = if require_verified_wifi {
        let expected_ssid =
            expected_ssid.context("Verified Wi-Fi mode requires a configured hotspot")?;
        let wifi = wait_for_expected_wifi(expected_ssid, args, output)?;
        describe_wifi(&wifi.status, expected_ssid, wifi.redacted_evidence, output)?;
        let route =
            read_default_route().context("No usable default route after hotspot connection")?;
        require_wifi_default_route(&wifi.status, &route)?;
        (Some(wifi), route)
    } else if require_iphone_usb {
        (None, wait_for_iphone_usb_route(output)?)
    } else {
        output.pass("Explicit default-route mode · Wi-Fi or USB tether supported");
        match read_wifi_status() {
            Ok(wifi) => {
                output.detail(wifi.detail.clone());
            }
            Err(error) => {
                output.detail(format!(
                    "Wi-Fi inspection unavailable in route mode: {error}"
                ));
            }
        }
        (
            None,
            read_default_route().context("No usable default route after network connection")?,
        )
    };
    let route_interface = route
        .interface
        .clone()
        .ok_or_else(|| anyhow!("The Mac has no default network interface"))?;
    output.pass(format!("Default route through {route_interface}"));
    output.detail(&route.detail);

    let internet = require_probe(
        DEFAULT_INTERNET_PROBE_URL,
        config.hotspot.probe_timeout_seconds,
        "Internet",
        output,
    )?;
    output.detail(format!(
        "{} returned {:?} in {} ms",
        internet.url, internet.status, internet.elapsed_ms
    ));

    let provider = probe_provider_with_retries(agent, config.hotspot.probe_timeout_seconds, output);
    if provider.reachable {
        output.pass(format!("{} endpoint reachable", agent.display_name()));
    } else if args.allow_unverified_remote {
        output.warn(format!(
            "{} endpoint could not be reached; continuing because --allow-unverified-remote was set",
            agent.display_name()
        ));
    } else {
        anyhow::bail!(
            "{} endpoint is unreachable: {}",
            agent.display_name(),
            provider.detail
        );
    }

    output.blank();
    output.section("Power");
    let before = read_power_status().context("Could not read the battery preflight sensor")?;
    let before_percent = before
        .percent
        .context("Battery percentage is unavailable; refusing closed-lid mode")?;
    if before_percent < config.safety.minimum_start_battery_percent {
        anyhow::bail!(
            "Battery is {before_percent}%; Commute Mode requires at least {}%",
            config.safety.minimum_start_battery_percent
        );
    }
    require_known_safe_thermal()?;

    let lease_id = Uuid::new_v4();
    let helper_status = helper.acquire(
        lease_id,
        config.session.helper_ttl_seconds,
        expires_at,
        format!("{} commute", agent.display_name()),
    )?;
    cleanup.lease_id = Some(lease_id);
    let previous_sleep_disabled = helper_status.previous_sleep_disabled;
    if previous_sleep_disabled != Some(0) {
        anyhow::bail!(
            "SleepDisabled was already enabled before Rucksack started. End Amphetamine or any other closed-lid utility first so the safety floor can restore ordinary sleep."
        );
    }
    if helper_status.sleep_disabled != Some(1) {
        anyhow::bail!("The helper did not verify SleepDisabled=1");
    }
    if helper_status.hard_expires_at != Some(expires_at) {
        anyhow::bail!("The helper did not persist the requested non-renewable session deadline");
    }
    output.pass("Closed-lid lease acquired");

    let on_battery = if before.source == PowerSource::Battery {
        output.pass("Already running on battery");
        before
    } else {
        output.action("Unplug this Mac while the lid is still open");
        wait_for_battery(Duration::from_secs(90), || {
            helper
                .renew(lease_id, config.session.helper_ttl_seconds)
                .map(|_| ())
        })?
    };
    let battery_percent = on_battery
        .percent
        .context("Battery percentage became unavailable after unplugging")?;
    output.pass(format!("Running on battery · {battery_percent}%"));

    let reasserted = helper.reassert(lease_id)?;
    if reasserted.sleep_disabled != Some(1) {
        anyhow::bail!("Closed-lid lease did not survive the power-source transition");
    }
    output.pass("Closed-lid lease re-armed");

    let after_wifi = if require_verified_wifi {
        Some(read_wifi_status()?)
    } else {
        read_wifi_status().ok()
    };
    let after_route = if require_iphone_usb {
        wait_for_iphone_usb_route(output)?
    } else {
        read_default_route()?
    };
    if after_route.interface.is_none() {
        anyhow::bail!("The default route disappeared after unplugging");
    }
    if let (Some(verified_wifi), Some(after_wifi)) = (verified_wifi.as_ref(), after_wifi.as_ref()) {
        let expected_ssid =
            expected_ssid.context("Verified Wi-Fi mode requires a configured hotspot")?;
        require_wifi_default_route(after_wifi, &after_route)?;
        let redacted_evidence =
            post_unplug_redacted_evidence(verified_wifi, &route, after_wifi, &after_route)?;
        describe_wifi(after_wifi, expected_ssid, redacted_evidence, output)?;
    }
    let after_internet = require_probe(
        DEFAULT_INTERNET_PROBE_URL,
        config.hotspot.probe_timeout_seconds,
        "Internet after unplugging",
        output,
    )?;
    let after_provider =
        probe_provider_with_retries(agent, config.hotspot.probe_timeout_seconds, output);
    if !after_provider.reachable && !args.allow_unverified_remote {
        anyhow::bail!(
            "{} became unreachable after unplugging: {}",
            agent.display_name(),
            after_provider.detail
        );
    }
    output.pass("Network and remote route survived the transition");
    output.detail(format!(
        "Post-unplug probe {} ms; provider {}",
        after_internet.elapsed_ms, after_provider.detail
    ));

    output.blank();
    output.section("Safety");
    output.pass(format!(
        "Battery {battery_percent}% · warn at {}% · sleep at {}%",
        config.safety.warn_battery_percent, config.safety.sleep_battery_percent
    ));

    let thermal = require_known_safe_thermal()?;
    output.pass(format!("Thermals {:?}", thermal.level).to_ascii_lowercase());
    output.pass(format!(
        "Ends at {}",
        expires_at.with_timezone(&chrono::Local).format("%H:%M")
    ));

    let bind_commute_route = require_verified_wifi || require_iphone_usb;
    let commute_route_interface = if bind_commute_route {
        after_route.interface.clone()
    } else {
        None
    };
    let commute_route_gateway = if bind_commute_route {
        after_route.gateway.clone()
    } else {
        None
    };
    let mut session = SessionState {
        version: 1,
        revision: 0,
        id: policy.session_id,
        lease_id,
        owner_uid: current_uid(),
        agent,
        project_dir: project_dir.clone(),
        focus: config.session.focus,
        phase: SessionPhase::Ready,
        started_at: now,
        expires_at,
        last_heartbeat_at: None,
        daemon_pid: None,
        expected_hotspot_ssid: expected_ssid.map(ToOwned::to_owned),
        observed_hotspot_ssid: after_wifi.and_then(|wifi| wifi.ssid),
        commute_route_interface,
        commute_route_gateway,
        route_interface: after_route.interface,
        battery_percent: Some(battery_percent),
        network_reachable: Some(true),
        network_outage_started_at: None,
        phase_before_offline: None,
        idle_grace_started_at: None,
        completed_at: None,
        previous_sleep_disabled,
        remote_owned_by_rucksack: remote.owned,
        remote_pid: remote.pid,
        remote_confirmed_by_user: remote.confirmed_by_user,
        last_event: Some("post-unplug preflight passed".to_owned()),
        release_reason: None,
    };
    session.save(paths)?;

    helper.renew(lease_id, config.session.helper_ttl_seconds)?;
    let daemon_pid = spawn_daemon(session.id, paths)?;
    cleanup.daemon_pid = Some(daemon_pid);
    let session = wait_for_daemon(session.id, daemon_pid, paths, Duration::from_secs(8))?;

    output.blank();
    output.plain("Ready.");
    output.plain("Lock your Mac, close the lid, and go.");
    output.plain("Normal sleep will be restored automatically.");
    if output.json() {
        output.emit_json(&session)?;
    }
    Ok(())
}

pub fn status(args: &StatusArgs, output: &Output, paths: &AppPaths) -> Result<()> {
    let (session, session_error) = match SessionState::load(paths) {
        Ok(session) => (session, None),
        Err(error) => (None, Some(error.to_string())),
    };
    let (helper, helper_error) = match HelperClient::default().status() {
        Ok(helper) => (helper, None),
        Err(error) => (None, Some(error.to_string())),
    };

    #[derive(Serialize)]
    struct StatusView {
        session: Option<SessionState>,
        helper: Option<rucksack_core::protocol::HelperStatus>,
        session_error: Option<String>,
        helper_error: Option<String>,
    }
    let view = StatusView {
        session,
        helper,
        session_error,
        helper_error,
    };
    if output.json() {
        return output.emit_json(&view);
    }

    let Some(session) = view.session.as_ref() else {
        output.title("No active commute");
        if let Some(error) = view.session_error.as_deref() {
            output.warn(format!("Session state is unreadable: {error}"));
            output.plain("Run `rucksack recover` to restore sleep and quarantine stale state.");
        }
        match view.helper.as_ref() {
            Some(status) if status.active => {
                output.warn("The helper reports an active lease without user session state.");
                output.plain("Run `rucksack recover`.");
            }
            Some(status) if status.sleep_disabled == Some(0) => {
                output.pass("This Mac will sleep normally");
            }
            Some(status) if status.sleep_disabled == Some(1) => {
                output.warn("SleepDisabled is still enabled, but Rucksack owns no lease.");
                output.plain("End the other closed-lid utility or run `sudo pmset -a disablesleep 0` only after confirming it is safe.");
            }
            _ => output.warn("Rucksack could not prove the current sleep state."),
        }
        if let Some(error) = view.helper_error.as_deref() {
            output.warn(format!("Power helper status failed: {error}"));
        }
        if args.full {
            output.blank();
            output.plain(serde_json::to_string_pretty(&view)?);
        }
        return Ok(());
    };

    output.title(if matches!(session.phase, SessionPhase::Released) {
        "The last commute has ended"
    } else {
        "Commute Mode is active"
    });
    output.pass(format!(
        "{} · {}",
        session.agent.display_name(),
        session.project_dir.display()
    ));
    if session.network_reachable == Some(false) {
        output.warn("Remote network path is temporarily unavailable");
    } else if let Some(ssid) = &session.observed_hotspot_ssid {
        output.pass(format!("Online through {ssid}"));
    } else if let Some(interface) = &session.route_interface {
        output.pass(format!("Network route through {interface}"));
    }
    output.plain(format!(
        "Battery {} · {} minutes remaining",
        session
            .battery_percent
            .map(|percent| format!("{percent}%"))
            .unwrap_or_else(|| "unknown".to_owned()),
        session.remaining_minutes(Utc::now())
    ));
    output.plain(format!("State: {:?}", session.phase));
    if let Some(event) = &session.last_event {
        output.detail(format!("Last event: {event}"));
    }
    if let Some(reason) = &session.release_reason {
        output.plain(format!("Ended because: {reason}"));
    }
    if let Some(helper) = view.helper.as_ref() {
        output.detail(format!("Helper: {:?}", helper));
    }
    if let Some(error) = view.helper_error.as_deref() {
        output.warn(format!("Power helper status failed: {error}"));
    }
    if let Some(error) = view.session_error.as_deref() {
        output.warn(format!("Session state warning: {error}"));
    }
    if args.full {
        output.blank();
        output.plain(serde_json::to_string_pretty(&view)?);
    }
    Ok(())
}

pub fn arrive(
    _args: &ArriveArgs,
    output: &Output,
    paths: &AppPaths,
    config: &Config,
) -> Result<()> {
    output.title("Restoring this Mac");
    let session = SessionState::load(paths)?;
    if let Some(session) = &session {
        if let Some(pid) = session.daemon_pid {
            kill_process(pid);
        }
        let helper = HelperClient::default();
        match helper.release(session.lease_id, "user arrived") {
            Ok(status)
                if !status.active
                    && status.sleep_disabled == session.previous_sleep_disabled =>
            {
                output.pass("Normal sleep restored")
            }
            Ok(status) => anyhow::bail!(
                "The helper did not prove normal sleep was restored: {:?}. Session state was kept for recovery.",
                status
            ),
            Err(error) => {
                let status = helper.status().ok().flatten();
                if status.as_ref().is_some_and(|status| {
                    !status.active && status.sleep_disabled == session.previous_sleep_disabled
                }) {
                    output.pass("Normal sleep already restored");
                } else {
                    return Err(error).context("Could not restore normal sleep");
                }
            }
        }
        cleanup_policy(paths, session.agent, &session.project_dir)?;
        output.pass("Commute policy removed");

        if config.session.stop_owned_remote_on_arrive && session.remote_owned_by_rucksack {
            stop_owned_remote(session);
        }
        SessionState::clear(paths)?;
        output.pass("Watcher stopped");
    } else {
        let helper = HelperClient::default();
        let status = helper.status().context("Could not read the power helper")?;
        if status.as_ref().is_some_and(|status| status.active) {
            let recovered = helper
                .recover()?
                .context("The helper returned no status after recovery")?;
            if recovered.active || recovered.sleep_disabled != Some(0) {
                anyhow::bail!(
                    "The helper did not prove normal sleep was restored: {:?}",
                    recovered
                );
            }
            output.pass("Recovered an untracked power lease");
        } else if status
            .as_ref()
            .is_some_and(|status| status.sleep_disabled != Some(0))
        {
            anyhow::bail!(
                "Rucksack owns no lease, but SleepDisabled is not 0. End the utility that owns the setting before claiming normal sleep."
            );
        }
        cleanup_orphaned_policy(paths)?;
        output.pass("Normal sleep is enabled");
    }
    output.blank();
    output.plain("Arrived.");
    Ok(())
}

pub fn recover(args: &RecoverArgs, output: &Output, paths: &AppPaths) -> Result<()> {
    output.title("Recovery");
    if !args.yes
        && !output.confirm(
            "Restore normal sleep and clear any interrupted Rucksack state?",
            true,
        )?
    {
        output.plain("No changes made.");
        return Ok(());
    }

    restore_sleep_for_recovery(output)?;

    let mut cleanup_errors: Vec<String> = Vec::new();
    let session = match SessionState::load(paths) {
        Ok(session) => session,
        Err(error) => {
            match quarantine_corrupt_file(&paths.session_file, "session") {
                Ok(path) => output.warn(format!(
                    "Unreadable session state was quarantined at {}: {error}",
                    path.display()
                )),
                Err(quarantine_error) => cleanup_errors.push(format!(
                    "session state is unreadable ({error}) and could not be quarantined: {quarantine_error}"
                )),
            }
            None
        }
    };
    if let Some(session) = session.as_ref() {
        if let Some(pid) = session.daemon_pid {
            kill_process(pid);
        }
    }

    let policy = match ActivePolicy::load(paths) {
        Ok(policy) => policy,
        Err(error) => {
            match quarantine_corrupt_file(&paths.policy_file, "policy") {
                Ok(path) => output.warn(format!(
                    "Unreadable policy state was quarantined at {}: {error}",
                    path.display()
                )),
                Err(quarantine_error) => cleanup_errors.push(format!(
                    "policy state is unreadable ({error}) and could not be quarantined: {quarantine_error}"
                )),
            }
            None
        }
    };

    let cursor_project = session
        .as_ref()
        .filter(|session| session.agent == AgentKind::Cursor)
        .map(|session| session.project_dir.as_path())
        .or_else(|| {
            policy
                .as_ref()
                .filter(|policy| policy.agent == AgentKind::Cursor)
                .map(|policy| policy.project_dir.as_path())
        });
    if let Some(project) = cursor_project {
        if let Err(error) = rucksack_core::agent::deactivate_cursor_rule(project) {
            cleanup_errors.push(format!(
                "could not remove the Cursor commute rule from {}: {error}",
                project.display()
            ));
        }
    }
    if policy.is_some() {
        if let Err(error) = ActivePolicy::clear(paths) {
            cleanup_errors.push(format!("could not clear policy state: {error}"));
        }
    }
    if session.is_some() {
        if let Err(error) = SessionState::clear(paths) {
            cleanup_errors.push(format!("could not clear session state: {error}"));
        }
    }

    if !cleanup_errors.is_empty() {
        anyhow::bail!(
            "Normal sleep is restored, but recovery cleanup is incomplete:\n- {}",
            cleanup_errors.join("\n- ")
        );
    }

    output.pass("Temporary policy and stale state cleared");
    output.blank();
    output.plain("This Mac will sleep normally.");
    Ok(())
}

fn restore_sleep_for_recovery(output: &Output) -> Result<()> {
    let helper = HelperClient::default();
    match helper.recover() {
        Ok(Some(status)) if !status.active && status.sleep_disabled == Some(0) => {
            output.pass("Normal sleep restored")
        }
        Ok(Some(status)) => anyhow::bail!(
            "Recovery did not prove normal sleep was restored: {:?}. State was kept for manual recovery.",
            status
        ),
        Ok(None) => {
            let value = read_sleep_disabled()
                .context("The helper returned no status and pmset could not verify normal sleep")?;
            if value != 0 {
                anyhow::bail!(
                    "The helper returned no status and pmset reports SleepDisabled={value}"
                );
            }
            output.pass("Normal sleep independently verified with pmset");
        }
        Err(error) => {
            let value = read_sleep_disabled().ok();
            if value == Some(0) {
                output.pass("Normal sleep is already enabled");
            } else {
                return Err(error).context(format!(
                    "Helper recovery failed and pmset did not verify SleepDisabled=0 (observed {value:?})"
                ));
            }
        }
    }
    Ok(())
}

fn quarantine_corrupt_file(path: &Path, label: &str) -> Result<PathBuf> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            anyhow!(
                "{label} state path has no valid file name: {}",
                path.display()
            )
        })?;
    let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let quarantine = path.with_file_name(format!("{file_name}.corrupt-{timestamp}"));
    fs::rename(path, &quarantine).with_context(|| {
        format!(
            "Could not quarantine corrupt {label} state from {} to {}",
            path.display(),
            quarantine.display()
        )
    })?;
    Ok(quarantine)
}

fn cleanup_orphaned_policy(paths: &AppPaths) -> Result<()> {
    if let Some(policy) = ActivePolicy::load(paths)? {
        if policy.agent == AgentKind::Cursor {
            rucksack_core::agent::deactivate_cursor_rule(&policy.project_dir)?;
        }
    }
    ActivePolicy::clear(paths)
}

fn select_agent(
    requested: Option<AgentKind>,
    project: &Path,
    output: &Output,
) -> Result<AgentKind> {
    if let Some(agent) = requested {
        return Ok(agent);
    }
    let detections = detect_all(Some(project));
    let mut candidates = detections
        .iter()
        .filter(|detection| detection.project_match == ProjectMatch::Matched)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        candidates = detections
            .iter()
            .filter(|detection| detection.running)
            .collect();
    }
    if candidates.is_empty() {
        candidates = detections
            .iter()
            .filter(|detection| detection.installed)
            .collect();
    }
    if candidates.is_empty() {
        anyhow::bail!("No Codex, Claude Code, or Cursor installation was found");
    }
    let options = candidates
        .iter()
        .map(|detection| detection.detail.clone())
        .collect::<Vec<_>>();
    let selected = output.choose(
        "Select the agent you will control from your phone:",
        &options,
    )?;
    Ok(candidates[selected].kind)
}

fn ensure_adapter(paths: &AppPaths, agent: AgentKind, yes: bool, output: &Output) -> Result<()> {
    let binary = std::env::current_exe()?;
    let evidence = verify_adapter(paths, &binary, agent);
    if evidence.current {
        output.pass("Native Commute Mode adapter verified");
        return Ok(());
    }
    report_adapter_issues(&evidence, output);

    if output.json() && !yes {
        anyhow::bail!(
            "{} adapter is required. Run `rucksack adapters install --agent {}` or retry with --yes.",
            agent.display_name(),
            agent
        );
    }
    let should_install = yes
        || output.confirm(
            &format!(
                "Install or repair the reversible {} Commute Mode adapter now?",
                agent.display_name()
            ),
            true,
        )?;
    if !should_install {
        anyhow::bail!(
            "{} adapter is required. Run `rucksack adapters install --agent {}`.",
            agent.display_name(),
            agent
        );
    }
    install_adapters(paths, &binary, &[agent])?;
    let installed = verify_adapter(paths, &binary, agent);
    if !installed.current {
        anyhow::bail!(
            "{} adapter installation did not verify: {}",
            agent.display_name(),
            adapter_issue_summary(&installed)
        );
    }
    output.pass("Native Commute Mode adapter installed and verified");
    Ok(())
}

fn report_adapter_issues(evidence: &AgentAdapterEvidence, output: &Output) {
    for file in evidence.files.iter().filter(|file| !file.is_current()) {
        output.warn(format!(
            "{} adapter {} · {}",
            evidence.agent.display_name(),
            adapter_status_name(file.status),
            file.path.display()
        ));
        output.detail(&file.detail);
    }
}

fn adapter_issue_summary(evidence: &AgentAdapterEvidence) -> String {
    evidence
        .files
        .iter()
        .filter(|file| !file.is_current())
        .map(|file| {
            format!(
                "{} at {}: {}",
                adapter_status_name(file.status),
                file.path.display(),
                file.detail
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn adapter_status_name(status: AdapterFileStatus) -> &'static str {
    match status {
        AdapterFileStatus::Current => "current",
        AdapterFileStatus::Missing => "missing",
        AdapterFileStatus::Invalid => "invalid",
        AdapterFileStatus::Unowned => "unowned",
        AdapterFileStatus::Outdated => "outdated",
    }
}

#[derive(Debug)]
struct RemotePreparation {
    owned: bool,
    pid: Option<u32>,
    confirmed_by_user: bool,
}

fn prepare_remote(
    agent: AgentKind,
    detection: &AgentDetection,
    args: &LeaveArgs,
    output: &Output,
) -> Result<RemotePreparation> {
    match agent {
        AgentKind::Codex => {
            let remote_started = match codex_remote_start() {
                Ok(result) if result.success() => {
                    output.pass("Codex Remote Control daemon is running");
                    output.detail(result.combined_trimmed());
                    true
                }
                Ok(result) => {
                    output.warn(format!(
                        "Codex Remote Control could not be started automatically: {}",
                        result.combined_trimmed()
                    ));
                    false
                }
                Err(error) => {
                    output.warn(format!(
                        "Codex Remote Control could not be started automatically: {error:#}"
                    ));
                    false
                }
            };
            if !remote_started && !detection.running {
                anyhow::bail!(
                    "No running Codex conversation was detected and Remote Control could not be started"
                );
            }
            output.action("In an already-open Codex conversation, invoke `$commute-mode` once.");
            output.action("Open ChatGPT on your phone and confirm this Codex session appears.");
            let confirmed = confirm_remote(args, output, "Can your phone see the Codex session?")?;
            output.pass(if confirmed {
                "Codex session visibility confirmed by you"
            } else {
                "Codex session visibility accepted without phone verification"
            });
            // `start` may attach to a pre-existing daemon. Until the JSON schema exposes
            // ownership explicitly, never stop it automatically during rollback or arrival.
            Ok(RemotePreparation {
                owned: false,
                pid: None,
                confirmed_by_user: confirmed,
            })
        }
        AgentKind::Claude => {
            if detection.running {
                output.pass("Claude Code conversation detected");
            } else {
                output.warn(
                    "Rucksack cannot prove which Claude Code conversation should be handed off",
                );
            }
            output.action(claude_remote_user_instruction());
            output.action("Then invoke `/commute-mode` once in that exact conversation.");
            let confirmed =
                confirm_remote(args, output, "Can your phone see that Claude session?")?;
            output.pass(if confirmed {
                "Claude Remote Control confirmed by you"
            } else {
                "Claude Remote Control accepted without phone verification"
            });
            Ok(RemotePreparation {
                owned: false,
                pid: None,
                confirmed_by_user: confirmed,
            })
        }
        AgentKind::Cursor => {
            output.action("In an already-open Cursor conversation, invoke `/commute-mode` once.");
            output.action("In Cursor, open Agents → Remote Control.");
            output.action("Confirm that Cursor on your phone can see this local agent.");
            let confirmed = confirm_remote(args, output, "Can your phone see the Cursor agent?")?;
            output.pass(if confirmed {
                "Cursor Remote Control confirmed by you"
            } else {
                "Cursor Remote Control accepted without phone verification"
            });
            Ok(RemotePreparation {
                owned: false,
                pid: None,
                confirmed_by_user: confirmed,
            })
        }
    }
}

fn confirm_remote(args: &LeaveArgs, output: &Output, prompt: &str) -> Result<bool> {
    if args.allow_unverified_remote {
        output.warn("Phone visibility was not verified; continuing by explicit override");
        return Ok(false);
    }
    if args.yes {
        anyhow::bail!(
            "Phone visibility cannot be measured automatically. Omit --yes to confirm it, or add --allow-unverified-remote to accept that risk."
        );
    }
    if output.confirm(prompt, true)? {
        Ok(true)
    } else {
        anyhow::bail!("Remote Control was not confirmed on the phone")
    }
}

fn wait_for_expected_wifi(
    expected: &str,
    args: &LeaveArgs,
    output: &Output,
) -> Result<VerifiedWifi> {
    let current = read_wifi_status()?;
    if wifi_is_acceptable(&current, expected, None) {
        return Ok(VerifiedWifi {
            status: current,
            redacted_evidence: None,
        });
    }

    let mut redacted_evidence: Option<RedactedWifiEvidence> = None;
    let mut manual_selection_required = false;
    if let Some(device) = current.device.as_deref() {
        let should_switch = args.yes
            || output.confirm(
                &format!("Ask macOS to switch Wi-Fi to the saved hotspot “{expected}”?"),
                true,
            )?;
        if should_switch {
            output.checking(format!("Requesting saved hotspot “{expected}”"));
            match connect_saved_wifi(device, expected) {
                Ok(()) => {
                    output.pass("macOS accepted the exact saved-hotspot join request");
                    redacted_evidence = Some(RedactedWifiEvidence::SavedNetworkJoin);
                }
                Err(error) => {
                    output.warn(format!("Automatic hotspot switch failed: {error}"));
                    manual_selection_required = true;
                }
            }
        } else {
            manual_selection_required = true;
        }
    } else {
        output.warn("macOS did not report a Wi-Fi device for automatic hotspot switching");
        manual_selection_required = true;
    }

    if manual_selection_required {
        output.action(
            "Open the Wi-Fi menu and select the phone under Personal Hotspots (Instant Hotspot), then return here.",
        );
        if args.allow_unverified_ssid {
            if args.yes {
                anyhow::bail!(
                    "The automatic hotspot switch failed and a privacy-redacted SSID cannot be confirmed with --yes. Re-run interactively, select “{expected}” in the Wi-Fi menu, and confirm it."
                );
            }
            if !output.confirm(
                &format!("Does the Wi-Fi menu now show “{expected}” as connected?"),
                false,
            )? {
                anyhow::bail!("The configured hotspot was not confirmed in the Wi-Fi menu");
            }
            output.pass("Hotspot selection confirmed by you");
            redacted_evidence = Some(RedactedWifiEvidence::UserConfirmation);
        }
    }

    output.action(format!("Connect “{expected}” on this Mac"));

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let wifi = read_wifi_status()?;
        let usable_redacted_evidence = if args.allow_unverified_ssid {
            redacted_evidence
        } else {
            None
        };
        if wifi_is_acceptable(&wifi, expected, usable_redacted_evidence) {
            return Ok(VerifiedWifi {
                redacted_evidence: wifi.redacted.then_some(usable_redacted_evidence).flatten(),
                status: wifi,
            });
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "The expected hotspot did not become verifiable. Current state: {}",
                wifi.detail
            );
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn require_wifi_default_route(wifi: &WifiStatus, route: &RouteStatus) -> Result<()> {
    let wifi_device = wifi
        .device
        .as_deref()
        .context("macOS did not report the verified Wi-Fi device")?;
    let route_interface = route
        .interface
        .as_deref()
        .context("The Mac has no default network interface")?;
    if route_interface != wifi_device {
        anyhow::bail!(
            "The default route uses {route_interface:?}, not the verified Wi-Fi device {wifi_device:?}. Make the configured hotspot the default network route and retry."
        );
    }
    Ok(())
}

fn post_unplug_redacted_evidence(
    verified_wifi: &VerifiedWifi,
    verified_route: &RouteStatus,
    after_wifi: &WifiStatus,
    after_route: &RouteStatus,
) -> Result<Option<RedactedWifiEvidence>> {
    if after_wifi.ssid.is_some() || !after_wifi.redacted {
        return Ok(None);
    }
    let evidence = verified_wifi.redacted_evidence.context(
        "The hotspot SSID became privacy-redacted after unplugging without saved-network join or user-confirmation evidence",
    )?;
    if verified_route.interface != after_route.interface
        || verified_route.gateway != after_route.gateway
    {
        anyhow::bail!(
            "The default route changed while the hotspot SSID was privacy-redacted (interface {:?} → {:?}, gateway {:?} → {:?}); refusing to bind an unverified route",
            verified_route.interface,
            after_route.interface,
            verified_route.gateway,
            after_route.gateway
        );
    }
    Ok(Some(evidence))
}

fn wait_for_iphone_usb_route(output: &Output) -> Result<RouteStatus> {
    let device = read_iphone_usb_device()?
        .context("macOS does not currently expose an iPhone USB network service")?;
    output.action(
        "Turn on Personal Hotspot on the connected iPhone; disconnect Wi-Fi if macOS keeps it as the default route.",
    );

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let route = read_default_route()?;
        if route.interface.as_deref() == Some(device.as_str()) {
            return Ok(route);
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "iPhone USB is available on {device}, but the default route remained {:?}. Enable Personal Hotspot and make iPhone USB the active route.",
                route.interface
            );
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn wifi_is_acceptable(
    wifi: &WifiStatus,
    expected: &str,
    redacted_evidence: Option<RedactedWifiEvidence>,
) -> bool {
    if !wifi.connected {
        return false;
    }
    wifi.ssid.as_deref() == Some(expected) || (wifi.redacted && redacted_evidence.is_some())
}

fn describe_wifi(
    wifi: &WifiStatus,
    expected: &str,
    redacted_evidence: Option<RedactedWifiEvidence>,
    output: &Output,
) -> Result<()> {
    if !wifi.connected {
        anyhow::bail!("Wi-Fi is not connected: {}", wifi.detail);
    }
    if let Some(ssid) = &wifi.ssid {
        if expected != ssid {
            anyhow::bail!("Wi-Fi is on {ssid:?}, not the configured hotspot {expected:?}",);
        }
        output.pass(format!("Wi-Fi: {ssid}"));
        return Ok(());
    }
    if wifi.redacted && redacted_evidence.is_some() {
        output.warn("Wi-Fi is connected, but macOS privacy-redacted the SSID");
        return Ok(());
    }
    anyhow::bail!(
        "Wi-Fi is active but the SSID cannot be verified. Grant Location Services to the terminal or use --allow-unverified-ssid after checking the Wi-Fi menu."
    )
}

fn require_probe(url: &str, timeout: u64, label: &str, output: &Output) -> Result<ProbeResult> {
    output.checking(format!("Checking {label}"));
    let result = retry_probe(label, output, || internet_probe(url, timeout));
    if result.reachable {
        output.pass(format!("{label}: reachable"));
        Ok(result)
    } else {
        Err(anyhow!("{label} probe failed: {}", result.detail))
    }
}

fn probe_provider_with_retries(agent: AgentKind, timeout: u64, output: &Output) -> ProbeResult {
    let label = format!("{} endpoint", agent.display_name());
    retry_probe(&label, output, || probe(provider_probe_url(agent), timeout))
}

fn retry_probe(label: &str, output: &Output, request: impl Fn() -> ProbeResult) -> ProbeResult {
    let mut result = request();
    for attempt in 1..PREFLIGHT_PROBE_ATTEMPTS {
        if result.reachable {
            return result;
        }
        output.warn(format!(
            "{label} probe attempt {attempt}/{PREFLIGHT_PROBE_ATTEMPTS} failed; retrying"
        ));
        output.detail(&result.detail);
        thread::sleep(PREFLIGHT_PROBE_RETRY_DELAY);
        result = request();
    }
    result
}

fn require_no_active_sleep_utilities() -> Result<()> {
    let active = read_active_sleep_utilities()
        .context("Could not inspect active third-party sleep assertions")?;
    if active.is_empty() {
        return Ok(());
    }
    let names = active
        .iter()
        .map(|utility| utility.display_name())
        .collect::<Vec<_>>()
        .join(", ");
    anyhow::bail!(
        "Active sleep utility detected: {names}. End that utility's session and rerun `rucksack leave`; Rucksack will not stop it automatically."
    )
}

fn require_known_safe_thermal() -> Result<ThermalStatus> {
    let thermal =
        read_thermal_status().context("Could not read the thermal-pressure preflight sensor")?;
    if thermal.level == ThermalLevel::Unknown {
        anyhow::bail!(
            "Thermal pressure is unknown; refusing closed-lid mode until macOS reports a known state"
        );
    }
    if thermal.throttled {
        anyhow::bail!("Thermal throttling is already active; refusing closed-lid mode");
    }
    Ok(thermal)
}

fn wait_for_battery<F>(
    timeout: Duration,
    mut heartbeat: F,
) -> Result<rucksack_core::power::PowerStatus>
where
    F: FnMut() -> Result<()>,
{
    let deadline = Instant::now() + timeout;
    let mut last_heartbeat = Instant::now();
    loop {
        let power = read_power_status()?;
        if power.source == PowerSource::Battery {
            return Ok(power);
        }
        if Instant::now() >= deadline {
            anyhow::bail!("External power is still connected; the lid must remain open");
        }
        if last_heartbeat.elapsed() >= Duration::from_secs(15) {
            heartbeat()?;
            last_heartbeat = Instant::now();
        }
        thread::sleep(Duration::from_millis(750));
    }
}

fn wait_for_daemon(
    session_id: Uuid,
    daemon_pid: u32,
    paths: &AppPaths,
    timeout: Duration,
) -> Result<SessionState> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(session) = SessionState::load(paths)? {
            if session.id == session_id
                && session.daemon_pid == Some(daemon_pid)
                && session.last_heartbeat_at.is_some()
                && session.phase == SessionPhase::Active
            {
                return Ok(session);
            }
        }
        if Instant::now() >= deadline {
            anyhow::bail!("The safety watcher did not establish its first heartbeat");
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn spawn_daemon(session_id: Uuid, paths: &AppPaths) -> Result<u32> {
    let executable = std::env::current_exe()?;
    ensure_private_dir(&paths.log_dir)?;
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.daemon_log)?;
    let stderr = stdout.try_clone()?;
    let mut command = Command::new(executable);
    command
        .arg("daemon")
        .arg("--session-id")
        .arg(session_id.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    let child = command
        .spawn()
        .context("Could not start the Rucksack watcher")?;
    Ok(child.id())
}

fn stop_owned_remote(session: &SessionState) {
    match session.agent {
        AgentKind::Codex => {
            let _ = codex_remote_stop();
        }
        AgentKind::Claude => {
            if let Some(pid) = session.remote_pid {
                kill_process(pid);
            }
        }
        AgentKind::Cursor => {}
    }
}

fn kill_process(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

fn project_name(project: &Path) -> Option<String> {
    project
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
}

struct LeaveCleanup<'a> {
    paths: &'a AppPaths,
    lease_id: Option<Uuid>,
    policy_active: bool,
    cursor_project: Option<std::path::PathBuf>,
    remote_agent: Option<AgentKind>,
    remote_owned: bool,
    remote_pid: Option<u32>,
    daemon_pid: Option<u32>,
    committed: bool,
}

impl<'a> LeaveCleanup<'a> {
    fn new(paths: &'a AppPaths) -> Self {
        Self {
            paths,
            lease_id: None,
            policy_active: false,
            cursor_project: None,
            remote_agent: None,
            remote_owned: false,
            remote_pid: None,
            daemon_pid: None,
            committed: false,
        }
    }

    fn rollback(&mut self) -> Vec<String> {
        if self.committed {
            return Vec::new();
        }
        let mut errors = Vec::new();
        if let Some(pid) = self.daemon_pid {
            kill_process(pid);
        }
        if let Some(lease_id) = self.lease_id {
            match HelperClient::default().release(lease_id, "preflight failed") {
                Ok(status) if !status.active && status.sleep_disabled == Some(0) => {}
                Ok(status) => errors.push(format!(
                    "power helper did not prove normal sleep was restored: {status:?}"
                )),
                Err(error) => errors.push(format!("could not release power-helper lease: {error}")),
            }
        }
        if let Some(project) = &self.cursor_project {
            if let Err(error) = rucksack_core::agent::deactivate_cursor_rule(project) {
                errors.push(format!(
                    "could not remove Cursor commute files from {}: {error}",
                    project.display()
                ));
            }
        }
        if self.policy_active {
            if let Err(error) = ActivePolicy::clear(self.paths) {
                errors.push(format!("could not clear the active policy: {error}"));
            }
        }
        if self.remote_owned {
            match self.remote_agent {
                Some(AgentKind::Codex) => match codex_remote_stop() {
                    Ok(result) if result.success() => {}
                    Ok(result) => errors.push(format!(
                        "could not stop Rucksack-owned Codex Remote Control: {}",
                        result.combined_trimmed()
                    )),
                    Err(error) => errors.push(format!(
                        "could not stop Rucksack-owned Codex Remote Control: {error}"
                    )),
                },
                Some(AgentKind::Claude) => {
                    if let Some(pid) = self.remote_pid {
                        kill_process(pid);
                    }
                }
                _ => {}
            }
        }
        errors
    }
}

impl Drop for LeaveCleanup<'_> {
    fn drop(&mut self) {
        if !self.committed {
            for error in self.rollback() {
                eprintln!("Rucksack rollback warning: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wifi(ssid: Option<&str>, redacted: bool) -> WifiStatus {
        WifiStatus {
            device: Some("en0".to_owned()),
            connected: true,
            ssid: ssid.map(ToOwned::to_owned),
            redacted,
            detail: "test Wi-Fi".to_owned(),
        }
    }

    fn route(interface: &str, gateway: &str) -> RouteStatus {
        RouteStatus {
            interface: Some(interface.to_owned()),
            gateway: Some(gateway.to_owned()),
            detail: "test route".to_owned(),
        }
    }

    #[test]
    fn redacted_wifi_requires_exact_join_or_user_confirmation_evidence() {
        let redacted = wifi(None, true);

        assert!(!wifi_is_acceptable(&redacted, "Noah", None));
        assert!(wifi_is_acceptable(
            &redacted,
            "Noah",
            Some(RedactedWifiEvidence::SavedNetworkJoin)
        ));
        assert!(wifi_is_acceptable(&wifi(Some("Noah"), false), "Noah", None));
        assert!(!wifi_is_acceptable(
            &wifi(Some("zeitgeistX"), false),
            "Noah",
            Some(RedactedWifiEvidence::UserConfirmation)
        ));
    }

    #[test]
    fn strict_wifi_requires_the_wifi_device_as_default_route() {
        let verified_wifi = wifi(Some("Noah"), false);

        assert!(require_wifi_default_route(&verified_wifi, &route("en0", "172.20.10.1")).is_ok());
        assert!(require_wifi_default_route(&verified_wifi, &route("en2", "192.168.1.1")).is_err());
    }

    #[test]
    fn redacted_wifi_rejects_route_drift_after_unplug() {
        let verified_wifi = VerifiedWifi {
            status: wifi(None, true),
            redacted_evidence: Some(RedactedWifiEvidence::SavedNetworkJoin),
        };
        let verified_route = route("en0", "172.20.10.1");
        let drifted_route = route("en0", "192.168.1.1");

        assert!(post_unplug_redacted_evidence(
            &verified_wifi,
            &verified_route,
            &wifi(None, true),
            &drifted_route
        )
        .is_err());
        assert_eq!(
            post_unplug_redacted_evidence(
                &verified_wifi,
                &verified_route,
                &wifi(None, true),
                &verified_route
            )
            .unwrap(),
            Some(RedactedWifiEvidence::SavedNetworkJoin)
        );
    }
}
