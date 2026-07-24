use crate::cli::{PackArgs, RecoverArgs, StatusArgs, UnpackArgs};
use crate::daemon::cleanup_policy;
use crate::helper_client::HelperClient;
use crate::output::{HumanEvent, Output};
use crate::report;
use anyhow::{anyhow, Context, Result};
use chrono::{Duration as ChronoDuration, Utc};
use rucksack_core::agent::{
    activate_cursor_rule, claude_remote_preflight, claude_remote_user_instruction,
    codex_remote_start, codex_remote_stop, detect, detect_all, install_adapters, verify_adapter,
    AdapterFileStatus, AgentAdapterEvidence, AgentDetection, AgentKind, ProjectMatch,
};
use rucksack_core::files::{ensure_private_dir, with_advisory_lock};
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
use rucksack_core::state::{
    ActivePolicy, SessionEndKind, SessionPhase, SessionReport, SessionState,
};
use rucksack_core::system::{current_uid, processes, ProcessInfo};
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

pub fn pack(
    args: &PackArgs,
    output: &Output,
    paths: &AppPaths,
    base_config: &Config,
) -> Result<()> {
    with_terminal_operation(paths, || pack_transaction(args, output, paths, base_config))
}

fn pack_transaction(
    args: &PackArgs,
    output: &Output,
    paths: &AppPaths,
    base_config: &Config,
) -> Result<()> {
    let mut cleanup = PackCleanup::new(paths);
    match pack_inner(args, output, paths, base_config, &mut cleanup) {
        Ok(()) => {
            cleanup.committed = true;
            Ok(())
        }
        Err(error) => {
            let had_safety_lease = cleanup.lease_id.is_some();
            let rollback_errors = cleanup.rollback();
            cleanup.committed = true;
            if rollback_errors.is_empty() {
                if had_safety_lease {
                    output.story(HumanEvent::Fact("normal sleep is restored".to_owned()));
                }
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

fn pack_inner(
    args: &PackArgs,
    output: &Output,
    paths: &AppPaths,
    base_config: &Config,
    cleanup: &mut PackCleanup<'_>,
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
                "Rucksack is already active for {}. Run `rucksack status` or `rucksack unpack`.",
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

    output.story(HumanEvent::Chapter("packing up".to_owned()));
    output.story(HumanEvent::RucksackAction(
        "checking the active agent".to_owned(),
    ));
    let detection = detect(agent, Some(&project_dir))?;
    if detection.installed {
        output.story(HumanEvent::Fact(format!("{agent} is installed")));
    } else {
        anyhow::bail!("{} was not found on this Mac", agent.display_name());
    }
    match detection.project_match {
        ProjectMatch::Matched => output.story(HumanEvent::Fact(format!(
            "{agent} is active in this project"
        ))),
        ProjectMatch::Different => output.story(HumanEvent::Warning(format!(
            "{agent} is running in a different project"
        ))),
        ProjectMatch::Unknown => output.story(HumanEvent::Warning(format!(
            "rucksack found {agent} but could not verify its project"
        ))),
        ProjectMatch::NotRunning => output.story(HumanEvent::Warning(format!(
            "rucksack did not find a running {agent} conversation"
        ))),
        ProjectMatch::NotRequested if detection.running => {
            output.story(HumanEvent::Fact(format!("{agent} is running")))
        }
        ProjectMatch::NotRequested => output.story(HumanEvent::Warning(format!(
            "rucksack did not find a running {agent} conversation"
        ))),
    }
    output.detail(&detection.detail);

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
    output.story(HumanEvent::RucksackAction(format!(
        "arming commute mode for {agent}"
    )));
    policy.save(paths)?;
    cleanup.policy_active = true;
    if agent == AgentKind::Cursor {
        activate_cursor_rule(&project_dir, &policy)?;
        cleanup.cursor_project = Some(project_dir.clone());
    }
    output.story(HumanEvent::Fact(format!(
        "commute mode is armed for {agent} with {} focus",
        config.session.focus
    )));

    let remote = prepare_remote(agent, &detection, args, output)?;
    cleanup.remote_agent = Some(agent);
    cleanup.remote_owned = remote.owned;
    cleanup.remote_pid = remote.pid;

    output.story(HumanEvent::RucksackAction(
        "checking the commute connection".to_owned(),
    ));
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
        output.story(HumanEvent::Fact(
            "wifi or usb tethering may provide the default route".to_owned(),
        ));
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
    output.story(HumanEvent::Fact(format!(
        "the default route uses {route_interface}"
    )));
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

    output.story(HumanEvent::RucksackAction(format!(
        "checking the {agent} endpoint"
    )));
    let provider = probe_provider_with_retries(agent, config.hotspot.probe_timeout_seconds, output);
    if provider.reachable {
        output.story(HumanEvent::Fact(format!(
            "the {agent} endpoint is reachable"
        )));
    } else if args.allow_unverified_remote {
        output.story(HumanEvent::Warning(format!(
            "rucksack could not reach {agent} and is continuing because allow unverified remote was set"
        )));
    } else {
        anyhow::bail!(
            "{} endpoint is unreachable: {}",
            agent.display_name(),
            provider.detail
        );
    }

    output.story(HumanEvent::RucksackAction(
        "checking battery and thermal safety".to_owned(),
    ));
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
    output.story(HumanEvent::RucksackAction(
        "securing the closed lid safety lease".to_owned(),
    ));
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
    output.story(HumanEvent::Fact(
        "the closed lid safety lease is active".to_owned(),
    ));

    let on_battery = if before.source == PowerSource::Battery {
        output.story(HumanEvent::Fact(
            "this mac is already running on battery".to_owned(),
        ));
        before
    } else {
        output.story(HumanEvent::UserAction(vec![
            "unplug this mac while the lid is open".to_owned(),
        ]));
        output.story(HumanEvent::Waiting {
            condition: "battery power".to_owned(),
            continuation: "packing will continue automatically".to_owned(),
        });
        wait_for_battery(Duration::from_secs(90), || {
            helper
                .renew(lease_id, config.session.helper_ttl_seconds)
                .map(|_| ())
        })?
    };
    let battery_percent = on_battery
        .percent
        .context("Battery percentage became unavailable after unplugging")?;
    output.story(HumanEvent::Fact(format!(
        "this mac is running on battery at {battery_percent} percent"
    )));

    output.story(HumanEvent::RucksackAction(
        "rearming the safety lease after unplugging".to_owned(),
    ));
    let reasserted = helper.reassert(lease_id)?;
    if reasserted.sleep_disabled != Some(1) {
        anyhow::bail!("Closed-lid lease did not survive the power-source transition");
    }
    output.story(HumanEvent::Fact(
        "the closed lid safety lease survived unplugging".to_owned(),
    ));

    output.story(HumanEvent::RucksackAction(
        "checking the connection after unplugging".to_owned(),
    ));
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
    if after_provider.reachable {
        output.story(HumanEvent::Fact(format!(
            "the network and the {agent} endpoint survived unplugging"
        )));
    } else {
        output.story(HumanEvent::Warning(format!(
            "the network survived unplugging but {agent} is still unverified"
        )));
    }
    output.detail(format!(
        "Post-unplug probe {} ms; provider {}",
        after_internet.elapsed_ms, after_provider.detail
    ));
    let commute_interface = after_route
        .interface
        .as_deref()
        .context("The commute route has no interface after unplugging")?;
    output.story(HumanEvent::RucksackAction(format!(
        "starting mobile data accounting on {commute_interface}"
    )));
    let mobile_data = report::read_mobile_data_baseline(commute_interface, output);
    if mobile_data.counters.is_some() {
        output.story(HumanEvent::Fact(format!(
            "mobile data accounting is active on {commute_interface}"
        )));
    }

    output.story(HumanEvent::RucksackAction(
        "checking the final safety limits".to_owned(),
    ));
    output.story(HumanEvent::Fact(format!(
        "battery is {battery_percent} percent. rucksack warns at {} percent and restores normal sleep at {} percent",
        config.safety.warn_battery_percent, config.safety.sleep_battery_percent
    )));

    let thermal = require_known_safe_thermal()?;
    output.story(HumanEvent::Fact(
        format!("thermal pressure is {:?}", thermal.level).to_ascii_lowercase(),
    ));
    output.story(HumanEvent::Fact(format!(
        "the session ends at {}",
        expires_at.with_timezone(&chrono::Local).format("%H.%M")
    )));

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
        start_battery_percent: Some(before_percent),
        battery_percent: Some(battery_percent),
        network_reachable: Some(true),
        network_outage_started_at: None,
        phase_before_offline: None,
        idle_grace_started_at: None,
        completed_at: None,
        ended_at: None,
        mobile_data_start: mobile_data.counters,
        mobile_data_end: None,
        mobile_data_finalized: false,
        mobile_data_error: mobile_data.error,
        previous_sleep_disabled,
        remote_owned_by_rucksack: remote.owned,
        remote_pid: remote.pid,
        remote_confirmed_by_user: remote.confirmed_by_user,
        last_event: Some("post-unplug preflight passed".to_owned()),
        release_reason: None,
    };
    session.save(paths)?;
    cleanup.session_id = Some(session.id);

    helper.renew(lease_id, config.session.helper_ttl_seconds)?;
    output.story(HumanEvent::RucksackAction(
        "starting the safety watcher".to_owned(),
    ));
    let daemon_pid = spawn_daemon(session.id, paths)?;
    cleanup.daemon_pid = Some(daemon_pid);
    output.story(HumanEvent::Waiting {
        condition: "the safety watcher to report its first heartbeat".to_owned(),
        continuation: "packing will finish automatically".to_owned(),
    });
    let session = wait_for_daemon(session.id, daemon_pid, paths, Duration::from_secs(8))?;

    match session.phase {
        SessionPhase::Failed | SessionPhase::Releasing | SessionPhase::Released => {
            anyhow::bail!(
                "The safety watcher entered {:?} during startup: {}",
                session.phase,
                session
                    .last_event
                    .as_deref()
                    .or(session.release_reason.as_deref())
                    .unwrap_or("no reason recorded")
            );
        }
        SessionPhase::Completed | SessionPhase::IdleGrace => {
            output.story(HumanEvent::Fact(
                "the safety watcher is running and the agent is already finishing".to_owned(),
            ));
            output.story(HumanEvent::Chapter("not packed".to_owned()));
            output.story(HumanEvent::UserAction(vec![
                "keep the lid open while rucksack restores normal sleep".to_owned(),
            ]));
            anyhow::bail!("the agent stopped before packing completed");
        }
        _ => {
            output.story(HumanEvent::Fact("the safety watcher is running".to_owned()));
            output.story(HumanEvent::Chapter("packed".to_owned()));
            output.story(HumanEvent::UserAction(vec![
                "lock this mac".to_owned(),
                "close the lid and go".to_owned(),
            ]));
        }
    }
    output.story(HumanEvent::Fact(
        "rucksack will restore normal sleep automatically".to_owned(),
    ));
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
    let (latest_report, report_error) = match SessionReport::load(paths) {
        Ok(report) => (report, None),
        Err(error) => (None, Some(error.to_string())),
    };
    let (helper, helper_error) = match HelperClient::default().status() {
        Ok(helper) => (helper, None),
        Err(error) => (None, Some(error.to_string())),
    };

    #[derive(Serialize)]
    struct StatusView {
        session: Option<SessionState>,
        latest_report: Option<SessionReport>,
        helper: Option<rucksack_core::protocol::HelperStatus>,
        session_error: Option<String>,
        report_error: Option<String>,
        helper_error: Option<String>,
    }
    let view = StatusView {
        session,
        latest_report,
        helper,
        session_error,
        report_error,
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
        if view.latest_report.is_some() {
            output.plain("Last completed session: run `rucksack report`.");
        }
        if let Some(error) = view.report_error.as_deref() {
            output.warn(format!("Session report is unreadable: {error}"));
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
    if matches!(session.phase, SessionPhase::Released) {
        if let Some(report) = view
            .latest_report
            .as_ref()
            .filter(|report| report.session_id == session.id)
        {
            output.story(HumanEvent::Chapter("trip report".to_owned()));
            report::show_body(output, report)?;
        } else {
            output.warn("The report for this completed session is unavailable.");
        }
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
    if let Some(error) = view.report_error.as_deref() {
        output.warn(format!("Session report warning: {error}"));
    }
    if args.full {
        output.blank();
        output.plain(serde_json::to_string_pretty(&view)?);
    }
    Ok(())
}

pub fn unpack(
    _args: &UnpackArgs,
    output: &Output,
    paths: &AppPaths,
    config: &Config,
) -> Result<()> {
    with_terminal_operation(paths, || unpack_locked(output, paths, config))
}

fn unpack_locked(output: &Output, paths: &AppPaths, config: &Config) -> Result<()> {
    output.story(HumanEvent::Chapter("unpacking".to_owned()));
    let session = SessionState::load(paths)?;
    let completed_report = if let Some(session) = &session {
        output.story(HumanEvent::RucksackAction(
            "restoring normal sleep".to_owned(),
        ));
        let helper = HelperClient::default();
        match helper.release(session.lease_id, "user unpacked") {
            Ok(status)
                if !status.active
                    && status.sleep_disabled == session.previous_sleep_disabled =>
            {
                output.story(HumanEvent::Fact("normal sleep is restored".to_owned()))
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
                    output.story(HumanEvent::Fact(
                        "normal sleep was already restored".to_owned(),
                    ));
                } else {
                    return Err(error).context("Could not restore normal sleep");
                }
            }
        }
        let daemon_pid = session
            .daemon_pid
            .filter(|_| phase_has_stoppable_watcher(session.phase));
        if let Some(pid) = daemon_pid {
            output.story(HumanEvent::RucksackAction(
                "stopping the safety watcher".to_owned(),
            ));
            if terminate_session_watcher(pid, session.id)? {
                output.story(HumanEvent::Waiting {
                    condition: "the safety watcher to stop".to_owned(),
                    continuation: "unpacking will continue automatically".to_owned(),
                });
                wait_for_session_watcher_exit(pid, session.id, Duration::from_secs(3))?;
                output.story(HumanEvent::Fact(
                    "the safety watcher has stopped".to_owned(),
                ));
            } else {
                output.story(HumanEvent::Fact(
                    "the safety watcher was already stopped".to_owned(),
                ));
            }
        }
        output.story(HumanEvent::RucksackAction(
            "removing commute mode".to_owned(),
        ));
        cleanup_policy(paths, session.agent, &session.project_dir)?;
        output.story(HumanEvent::Fact("commute mode is removed".to_owned()));

        if config.session.stop_owned_remote_on_unpack && session.remote_owned_by_rucksack {
            stop_owned_remote(session)?;
        }
        let completed_report = archive_session_for_unpack(paths, session, output)?;
        if !SessionState::clear_if_current(paths, session.id)? {
            anyhow::bail!("Session state changed during unpack; refusing to clear a newer session");
        }
        Some(completed_report)
    } else {
        output.story(HumanEvent::RucksackAction(
            "checking normal sleep".to_owned(),
        ));
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
            output.story(HumanEvent::Fact(
                "an untracked power lease was recovered".to_owned(),
            ));
        } else if status
            .as_ref()
            .is_some_and(|status| status.sleep_disabled != Some(0))
        {
            anyhow::bail!(
                "Rucksack owns no lease, but SleepDisabled is not 0. End the utility that owns the setting before claiming normal sleep."
            );
        }
        cleanup_orphaned_policy(paths)?;
        output.story(HumanEvent::Fact("normal sleep is enabled".to_owned()));
        load_report_or_quarantine(paths, output)?
    };
    output.story(HumanEvent::Chapter("unpacked".to_owned()));
    if let Some(report) = completed_report.as_ref() {
        output.story(HumanEvent::Chapter("trip report".to_owned()));
        report::show_body(output, report)?;
    }
    Ok(())
}

fn archive_session_for_unpack(
    paths: &AppPaths,
    session: &SessionState,
    output: &Output,
) -> Result<SessionReport> {
    let persisted = SessionState::load(paths)?;
    let session = persisted
        .as_ref()
        .filter(|persisted| persisted.id == session.id)
        .unwrap_or(session);
    if let Some(existing) =
        load_report_or_quarantine(paths, output)?.filter(|report| report.session_id == session.id)
    {
        return Ok(existing);
    }
    if automatic_release_completed(session) {
        let completed = finalize_automatic_session(session, Utc::now());
        return report::archive_session(paths, &completed, SessionEndKind::Automatic);
    }

    let end_battery_percent = read_power_status().ok().and_then(|power| power.percent);
    let mut completed = finalize_unpacked_session(session, Utc::now(), end_battery_percent);
    report::sample_mobile_data(&mut completed, true);
    report::archive_session(paths, &completed, SessionEndKind::Unpack)
}

fn automatic_release_completed(session: &SessionState) -> bool {
    session.phase == SessionPhase::Released
        || (session.phase == SessionPhase::Releasing && session.ended_at.is_some())
}

fn finalize_automatic_session(
    session: &SessionState,
    fallback_ended_at: chrono::DateTime<Utc>,
) -> SessionState {
    let mut completed = session.clone();
    completed.phase = SessionPhase::Released;
    completed.ended_at = completed
        .ended_at
        .or(completed.last_heartbeat_at)
        .or(Some(fallback_ended_at));
    if completed.release_reason.is_none() {
        completed.release_reason = Some("automatic release completed".to_owned());
    }
    completed
}

fn finalize_unpacked_session(
    session: &SessionState,
    ended_at: chrono::DateTime<Utc>,
    end_battery_percent: Option<u8>,
) -> SessionState {
    let mut completed = session.clone();
    completed.battery_percent = end_battery_percent;
    completed.phase = SessionPhase::Released;
    completed.ended_at = Some(ended_at);
    completed.release_reason = Some("user unpacked".to_owned());
    completed.last_event = Some("normal sleep restored: user unpacked".to_owned());
    completed
}

pub fn recover(args: &RecoverArgs, output: &Output, paths: &AppPaths) -> Result<()> {
    output.story(HumanEvent::Chapter("recovering".to_owned()));
    if !args.yes {
        output.story(HumanEvent::UserAction(vec![
            "allow rucksack to restore normal sleep and clear interrupted state".to_owned(),
        ]));
    }
    if !args.yes
        && !output.confirm(
            "restore normal sleep and clear interrupted rucksack state",
            true,
        )?
    {
        output.story(HumanEvent::Fact("nothing was changed".to_owned()));
        return Ok(());
    }

    with_terminal_operation(paths, || recover_locked(output, paths))
}

fn recover_locked(output: &Output, paths: &AppPaths) -> Result<()> {
    output.story(HumanEvent::RucksackAction(
        "restoring normal sleep".to_owned(),
    ));
    restore_sleep_for_recovery(output)?;

    let mut cleanup_errors: Vec<String> = Vec::new();
    let mut completed_report: Option<SessionReport> = None;
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
        if let Some(pid) = session
            .daemon_pid
            .filter(|_| phase_has_stoppable_watcher(session.phase))
        {
            if let Err(error) = terminate_session_watcher(pid, session.id) {
                cleanup_errors.push(format!("could not stop the safety watcher: {error:#}"));
            }
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
    if let Some(session) = session.as_ref() {
        match archive_session_for_recovery(paths, session, output) {
            Ok(report) => {
                completed_report = Some(report);
                match SessionState::clear_if_current(paths, session.id) {
                    Ok(true) => {}
                    Ok(false) => cleanup_errors.push(
                        "session state changed during recovery; refusing to clear a newer session"
                            .to_owned(),
                    ),
                    Err(error) => {
                        cleanup_errors.push(format!("could not clear session state: {error}"))
                    }
                }
            }
            Err(error) => {
                cleanup_errors.push(format!(
                    "could not preserve the completed-session report; session state was kept: {error}"
                ));
            }
        }
    }

    if !cleanup_errors.is_empty() {
        anyhow::bail!(
            "Normal sleep is restored, but recovery cleanup is incomplete:\n- {}",
            cleanup_errors.join("\n- ")
        );
    }

    output.story(HumanEvent::Fact(
        "temporary policy and stale state are cleared".to_owned(),
    ));
    output.story(HumanEvent::Chapter("recovered".to_owned()));
    output.story(HumanEvent::Fact("this mac will sleep normally".to_owned()));
    if let Some(report) = completed_report.as_ref() {
        output.story(HumanEvent::Chapter("trip report".to_owned()));
        report::show_body(output, report)?;
    }
    Ok(())
}

fn archive_session_for_recovery(
    paths: &AppPaths,
    session: &SessionState,
    output: &Output,
) -> Result<SessionReport> {
    if let Some(existing) =
        load_report_or_quarantine(paths, output)?.filter(|report| report.session_id == session.id)
    {
        return Ok(existing);
    }
    if automatic_release_completed(session) {
        let completed = finalize_automatic_session(session, Utc::now());
        return report::archive_session(paths, &completed, SessionEndKind::Automatic);
    }

    let mut completed = session.clone();
    completed.phase = SessionPhase::Released;
    completed.ended_at = completed.ended_at.or(Some(Utc::now()));
    completed.battery_percent = read_power_status().ok().and_then(|power| power.percent);
    if completed.release_reason.is_none() {
        completed.release_reason = Some("recovery requested".to_owned());
    }
    if !completed.mobile_data_finalized {
        completed.mobile_data_error = Some(
            "Session ended through recovery; the final commute-interface boundary was not verified"
                .to_owned(),
        );
    }
    completed.last_event = Some("normal sleep restored through recovery".to_owned());
    report::archive_session(paths, &completed, SessionEndKind::Recovery)
}

fn load_report_or_quarantine(paths: &AppPaths, output: &Output) -> Result<Option<SessionReport>> {
    match SessionReport::load(paths) {
        Ok(report) => Ok(report),
        Err(error) => {
            let quarantine = quarantine_corrupt_file(&paths.report_file, "report")?;
            output.warn(format!(
                "Unreadable session report was quarantined at {}: {error}",
                quarantine.display()
            ));
            Ok(None)
        }
    }
}

fn with_terminal_operation<T>(
    paths: &AppPaths,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let terminal_lock = paths.terminal_lock_file();
    with_advisory_lock(&terminal_lock, operation)
}

fn restore_sleep_for_recovery(output: &Output) -> Result<()> {
    let helper = HelperClient::default();
    match helper.recover() {
        Ok(Some(status)) if !status.active && status.sleep_disabled == Some(0) => {
            output.story(HumanEvent::Fact("normal sleep is restored".to_owned()))
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
            output.story(HumanEvent::Fact(
                "normal sleep is independently verified by macos".to_owned(),
            ));
        }
        Err(error) => {
            let value = read_sleep_disabled().ok();
            if value == Some(0) {
                output.story(HumanEvent::Fact(
                    "normal sleep is already enabled".to_owned(),
                ));
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
    output.story(HumanEvent::RucksackAction(format!(
        "checking the native {agent} commute mode adapter"
    )));
    let evidence = verify_adapter(paths, &binary, agent);
    if evidence.current {
        output.story(HumanEvent::Fact(format!(
            "the native {agent} commute mode adapter is ready"
        )));
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
    if !yes {
        output.story(HumanEvent::UserAction(vec![format!(
            "allow rucksack to install or repair the {agent} commute mode adapter"
        )]));
    }
    let should_install = yes
        || output.confirm(
            &format!("install or repair the reversible {agent} commute mode adapter now"),
            true,
        )?;
    if !should_install {
        anyhow::bail!(
            "{} adapter is required. Run `rucksack adapters install --agent {}`.",
            agent.display_name(),
            agent
        );
    }
    output.story(HumanEvent::RucksackAction(format!(
        "installing the native {agent} commute mode adapter"
    )));
    install_adapters(paths, &binary, &[agent])?;
    let installed = verify_adapter(paths, &binary, agent);
    if !installed.current {
        anyhow::bail!(
            "{} adapter installation did not verify: {}",
            agent.display_name(),
            adapter_issue_summary(&installed)
        );
    }
    output.story(HumanEvent::Fact(format!(
        "the native {agent} commute mode adapter is installed and verified"
    )));
    Ok(())
}

fn report_adapter_issues(evidence: &AgentAdapterEvidence, output: &Output) {
    for file in evidence.files.iter().filter(|file| !file.is_current()) {
        output.story(HumanEvent::Warning(format!(
            "{} adapter file is {} at {}",
            evidence.agent,
            adapter_status_name(file.status),
            file.path.display()
        )));
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
    args: &PackArgs,
    output: &Output,
) -> Result<RemotePreparation> {
    match agent {
        AgentKind::Codex => {
            output.story(HumanEvent::RucksackAction(
                "starting codex remote control".to_owned(),
            ));
            let remote_started = match codex_remote_start() {
                Ok(result) if result.success() => {
                    output.story(HumanEvent::Fact(
                        "codex remote control is running".to_owned(),
                    ));
                    output.detail(result.combined_trimmed());
                    true
                }
                Ok(result) => {
                    output.story(HumanEvent::Warning(format!(
                        "rucksack could not start codex remote control automatically because {}",
                        result.combined_trimmed()
                    )));
                    false
                }
                Err(error) => {
                    output.story(HumanEvent::Warning(format!(
                        "rucksack could not start codex remote control automatically because {error:#}"
                    )));
                    false
                }
            };
            if !remote_started && !detection.running {
                anyhow::bail!(
                    "No running Codex conversation was detected and Remote Control could not be started"
                );
            }
            output.story(HumanEvent::UserAction(vec![
                "in the open codex conversation invoke `$commute-mode`".to_owned(),
                "wait for codex to acknowledge commute mode".to_owned(),
                "open chatgpt on your phone and find this codex session".to_owned(),
            ]));
            let confirmed = confirm_remote(
                args,
                output,
                "confirm that codex acknowledged commute mode and your phone can see this session",
            )?;
            output.story(HumanEvent::Fact(if confirmed {
                "commute mode and codex phone visibility were confirmed by you".to_owned()
            } else {
                "codex phone visibility was accepted without verification".to_owned()
            }));
            // `start` may attach to a pre-existing daemon. Until the JSON schema exposes
            // ownership explicitly, never stop it automatically during rollback or unpack.
            Ok(RemotePreparation {
                owned: false,
                pid: None,
                confirmed_by_user: confirmed,
            })
        }
        AgentKind::Claude => {
            output.story(HumanEvent::RucksackAction(
                "checking claude code remote control support".to_owned(),
            ));
            let remote_support = claude_remote_preflight()
                .context("Could not inspect Claude Code Remote Control support")?;
            if !remote_support.success() {
                output.story(HumanEvent::Warning(
                    "claude code remote control is unavailable on the installed version".to_owned(),
                ));
                output.detail(remote_support.combined_trimmed());
                output.story(HumanEvent::UserAction(vec![
                    "run `claude update`".to_owned(),
                    "run `rucksack pack` again".to_owned(),
                ]));
                anyhow::bail!("claude code remote control is unavailable");
            }
            output.story(HumanEvent::Fact(
                "claude code remote control is available".to_owned(),
            ));
            if detection.running {
                output.story(HumanEvent::Fact(
                    "a claude code conversation is running".to_owned(),
                ));
            } else {
                output.story(HumanEvent::Warning(
                    "rucksack cannot verify which claude code conversation should be handed off"
                        .to_owned(),
                ));
            }
            output.story(HumanEvent::UserAction(vec![
                claude_remote_user_instruction().to_owned(),
                "wait until `/rc active` appears".to_owned(),
                "invoke `/commute-mode` in that exact conversation".to_owned(),
                "open claude on your phone and find that conversation".to_owned(),
            ]));
            let confirmed = confirm_remote(
                args,
                output,
                "confirm that claude code acknowledged commute mode and your phone can see that conversation",
            )?;
            output.story(HumanEvent::Fact(if confirmed {
                "commute mode and claude code phone visibility were confirmed by you".to_owned()
            } else {
                "claude code phone visibility was accepted without verification".to_owned()
            }));
            Ok(RemotePreparation {
                owned: false,
                pid: None,
                confirmed_by_user: confirmed,
            })
        }
        AgentKind::Cursor => {
            output.story(HumanEvent::UserAction(vec![
                "in the open cursor conversation invoke `/commute-mode`".to_owned(),
                "wait for cursor to acknowledge commute mode".to_owned(),
                "in cursor open agents and then remote control".to_owned(),
                "open cursor on your phone and find this local agent".to_owned(),
            ]));
            let confirmed = confirm_remote(
                args,
                output,
                "confirm that cursor acknowledged commute mode and your phone can see this agent",
            )?;
            output.story(HumanEvent::Fact(if confirmed {
                "commute mode and cursor phone visibility were confirmed by you".to_owned()
            } else {
                "cursor phone visibility was accepted without verification".to_owned()
            }));
            Ok(RemotePreparation {
                owned: false,
                pid: None,
                confirmed_by_user: confirmed,
            })
        }
    }
}

fn confirm_remote(args: &PackArgs, output: &Output, prompt: &str) -> Result<bool> {
    if args.allow_unverified_remote {
        output.story(HumanEvent::Warning(
            "phone visibility is unverified because allow unverified remote was set".to_owned(),
        ));
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
    args: &PackArgs,
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
        output.story(HumanEvent::RucksackAction(format!(
            "asking macos to join the saved hotspot {expected}"
        )));
        match connect_saved_wifi(device, expected) {
            Ok(()) => {
                output.story(HumanEvent::Fact(
                    "macos accepted the saved hotspot join request".to_owned(),
                ));
                redacted_evidence = Some(RedactedWifiEvidence::SavedNetworkJoin);
            }
            Err(error) => {
                output.story(HumanEvent::Warning(format!(
                    "automatic hotspot joining failed because {error}"
                )));
                manual_selection_required = true;
            }
        }
    } else {
        output.story(HumanEvent::Warning(
            "macos did not report a wifi device for automatic hotspot joining".to_owned(),
        ));
        manual_selection_required = true;
    }

    if manual_selection_required {
        output.story(HumanEvent::UserAction(vec![
            format!("open wifi and choose {expected} under personal hotspots"),
            "return here".to_owned(),
        ]));
        if args.yes {
            anyhow::bail!(
                "The automatic hotspot switch failed and requires interactive Wi-Fi-menu confirmation. Re-run without --yes, select “{expected}”, and confirm it."
            );
        }
        if !output.confirm(
            &format!("confirm that wifi now shows {expected} as connected"),
            false,
        )? {
            anyhow::bail!("The configured hotspot was not confirmed in the Wi-Fi menu");
        }
        output.story(HumanEvent::Fact(
            "the hotspot selection was confirmed by you".to_owned(),
        ));
        redacted_evidence = Some(RedactedWifiEvidence::UserConfirmation);
    }

    output.story(HumanEvent::Waiting {
        condition: format!("{expected} to become the verified wifi route"),
        continuation: "packing will continue automatically".to_owned(),
    });

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let wifi = read_wifi_status()?;
        let usable_redacted_evidence =
            accepted_redacted_evidence(args.allow_unverified_ssid, redacted_evidence);
        if wifi_is_acceptable(&wifi, expected, usable_redacted_evidence) {
            return Ok(VerifiedWifi {
                redacted_evidence: wifi.redacted.then_some(usable_redacted_evidence).flatten(),
                status: wifi,
            });
        }
        if wifi.connected
            && wifi.redacted
            && redacted_evidence == Some(RedactedWifiEvidence::SavedNetworkJoin)
            && !args.allow_unverified_ssid
        {
            output.story(HumanEvent::UserAction(vec![
                "open wifi and verify the connected hotspot name".to_owned(),
                "return here".to_owned(),
            ]));
            if args.yes {
                anyhow::bail!(
                    "macOS hid the hotspot name after the saved-network join. Re-run without --yes and confirm the connected network, or use --allow-unverified-ssid."
                );
            }
            if !output.confirm(
                &format!("confirm that wifi shows {expected} as connected"),
                false,
            )? {
                anyhow::bail!("The configured hotspot was not confirmed in the Wi-Fi menu");
            }
            output.story(HumanEvent::Fact(
                "the privacy hidden hotspot name was confirmed by you".to_owned(),
            ));
            redacted_evidence = Some(RedactedWifiEvidence::UserConfirmation);
            continue;
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

fn accepted_redacted_evidence(
    allow_saved_join: bool,
    evidence: Option<RedactedWifiEvidence>,
) -> Option<RedactedWifiEvidence> {
    match evidence {
        Some(RedactedWifiEvidence::UserConfirmation) => evidence,
        Some(RedactedWifiEvidence::SavedNetworkJoin) if allow_saved_join => evidence,
        _ => None,
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
    let current_route = read_default_route()?;
    if current_route.interface.as_deref() == Some(device.as_str()) {
        return Ok(current_route);
    }
    output.story(HumanEvent::UserAction(vec![
        "turn on personal hotspot on the connected iphone".to_owned(),
        "disconnect wifi if macos keeps using it as the default route".to_owned(),
    ]));
    output.story(HumanEvent::Waiting {
        condition: "iphone usb to become the default route".to_owned(),
        continuation: "packing will continue automatically".to_owned(),
    });

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
        output.story(HumanEvent::Fact(format!("wifi is connected to {ssid}")));
        return Ok(());
    }
    if wifi.redacted && redacted_evidence.is_some() {
        output.story(HumanEvent::Warning(
            "wifi is connected but macos hid the network name".to_owned(),
        ));
        return Ok(());
    }
    anyhow::bail!(
        "Wi-Fi is active but the SSID cannot be verified. Grant Location Services to the terminal or use --allow-unverified-ssid after checking the Wi-Fi menu."
    )
}

fn require_probe(url: &str, timeout: u64, label: &str, output: &Output) -> Result<ProbeResult> {
    let display_label = label.to_ascii_lowercase();
    output.story(HumanEvent::RucksackAction(format!(
        "checking {display_label}"
    )));
    let result = retry_probe(label, output, || internet_probe(url, timeout));
    if result.reachable {
        output.story(HumanEvent::Fact(format!("{display_label} is reachable")));
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
        output.story(HumanEvent::Warning(format!(
            "{} probe attempt {attempt} of {PREFLIGHT_PROBE_ATTEMPTS} failed. rucksack is retrying",
            label.to_ascii_lowercase()
        )));
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
        "Active sleep utility detected: {names}. End that utility's session and rerun `rucksack pack`; Rucksack will not stop it automatically."
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
            if watcher_has_started(&session, session_id, daemon_pid) {
                return Ok(session);
            }
        }
        if Instant::now() >= deadline {
            anyhow::bail!("The safety watcher did not establish its first heartbeat");
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn watcher_has_started(session: &SessionState, session_id: Uuid, daemon_pid: u32) -> bool {
    session.id == session_id
        && session.daemon_pid == Some(daemon_pid)
        && session.last_heartbeat_at.is_some()
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

fn stop_owned_remote(session: &SessionState) -> Result<()> {
    match session.agent {
        AgentKind::Codex => {
            let result = codex_remote_stop()?;
            if !result.success() {
                anyhow::bail!(
                    "Could not stop Rucksack-owned Codex Remote Control: {}",
                    result.combined_trimmed()
                );
            }
        }
        AgentKind::Claude => {
            if let Some(pid) = session.remote_pid {
                terminate_matching_process(pid, "Claude Code Remote Control", |process| {
                    process_matches_claude_remote(process)
                })?;
            }
        }
        AgentKind::Cursor => {}
    }
    Ok(())
}

fn phase_has_stoppable_watcher(phase: SessionPhase) -> bool {
    !matches!(
        phase,
        SessionPhase::Completed
            | SessionPhase::Releasing
            | SessionPhase::Released
            | SessionPhase::Failed
    )
}

fn terminate_session_watcher(pid: u32, session_id: Uuid) -> Result<bool> {
    terminate_matching_process(pid, "Rucksack safety watcher", |process| {
        process_matches_session_watcher(process, session_id)
    })
}

fn terminate_matching_process(
    pid: u32,
    label: &str,
    matches_expected_process: impl FnOnce(&ProcessInfo) -> bool,
) -> Result<bool> {
    let Some(process) = processes()?.into_iter().find(|process| process.pid == pid) else {
        return Ok(false);
    };
    if !matches_expected_process(&process) {
        anyhow::bail!(
            "Refusing to signal PID {pid} because it is not the expected {label}. Observed command: {}",
            process.arguments
        );
    }
    send_sigterm(pid, label)
}

fn send_sigterm(pid: u32, label: &str) -> Result<bool> {
    let raw_pid = checked_process_id(pid)?;
    #[cfg(unix)]
    {
        let result = unsafe { libc::kill(raw_pid, libc::SIGTERM) };
        if result == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(false);
        }
        Err(error).with_context(|| format!("Could not stop {label} at PID {pid}"))
    }

    #[cfg(not(unix))]
    {
        let _ = (raw_pid, label);
        anyhow::bail!("Process termination is unsupported on this platform")
    }
}

fn wait_for_session_watcher_exit(pid: u32, session_id: Uuid, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while session_watcher_is_running(pid, session_id)? {
        if Instant::now() >= deadline {
            anyhow::bail!("The safety watcher did not stop after SIGTERM");
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn session_watcher_is_running(pid: u32, session_id: Uuid) -> Result<bool> {
    Ok(processes()?
        .iter()
        .find(|process| process.pid == pid)
        .is_some_and(|process| process_matches_session_watcher(process, session_id)))
}

fn checked_process_id(pid: u32) -> Result<i32> {
    if pid == 0 {
        anyhow::bail!("Refusing to signal process group PID 0");
    }
    i32::try_from(pid).context("Process ID exceeds the platform PID range")
}

fn process_matches_session_watcher(process: &ProcessInfo, session_id: Uuid) -> bool {
    let mut arguments = process.arguments.split_whitespace();
    executable_name(arguments.next()) == Some("rucksack")
        && arguments.next() == Some("daemon")
        && arguments.next() == Some("--session-id")
        && arguments
            .next()
            .and_then(|value| Uuid::parse_str(value).ok())
            == Some(session_id)
        && arguments.next().is_none()
}

fn process_matches_claude_remote(process: &ProcessInfo) -> bool {
    let mut arguments = process.arguments.split_whitespace();
    executable_name(arguments.next()) == Some("claude")
        && arguments.next() == Some("remote-control")
}

fn executable_name(argument: Option<&str>) -> Option<&str> {
    Path::new(argument?).file_name()?.to_str()
}

fn project_name(project: &Path) -> Option<String> {
    project
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
}

struct PackCleanup<'a> {
    paths: &'a AppPaths,
    lease_id: Option<Uuid>,
    policy_active: bool,
    cursor_project: Option<std::path::PathBuf>,
    remote_agent: Option<AgentKind>,
    remote_owned: bool,
    remote_pid: Option<u32>,
    session_id: Option<Uuid>,
    daemon_pid: Option<u32>,
    committed: bool,
}

impl<'a> PackCleanup<'a> {
    fn new(paths: &'a AppPaths) -> Self {
        Self {
            paths,
            lease_id: None,
            policy_active: false,
            cursor_project: None,
            remote_agent: None,
            remote_owned: false,
            remote_pid: None,
            session_id: None,
            daemon_pid: None,
            committed: false,
        }
    }

    fn rollback(&mut self) -> Vec<String> {
        if self.committed {
            return Vec::new();
        }
        let mut errors = Vec::new();
        if let (Some(pid), Some(session_id)) = (self.daemon_pid, self.session_id) {
            if let Err(error) = terminate_session_watcher(pid, session_id) {
                errors.push(format!("could not stop the safety watcher: {error:#}"));
            }
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
                        match terminate_matching_process(
                            pid,
                            "Claude Code Remote Control",
                            process_matches_claude_remote,
                        ) {
                            Ok(_) => {}
                            Err(error) => errors.push(format!(
                                "could not stop Rucksack-owned Claude Code Remote Control: {error:#}"
                            )),
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(session_id) = self.session_id {
            match SessionState::clear_if_current(self.paths, session_id) {
                Ok(_) => {}
                Err(error) => errors.push(format!("could not clear failed session state: {error}")),
            }
        }
        errors
    }
}

impl Drop for PackCleanup<'_> {
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

    fn session(started_at: chrono::DateTime<Utc>) -> SessionState {
        SessionState {
            version: 1,
            revision: 0,
            id: Uuid::new_v4(),
            lease_id: Uuid::new_v4(),
            owner_uid: 501,
            agent: AgentKind::Codex,
            project_dir: PathBuf::from("/workspace/atlas"),
            focus: rucksack_core::Focus::Continue,
            phase: SessionPhase::Active,
            started_at,
            expires_at: started_at + ChronoDuration::minutes(30),
            last_heartbeat_at: None,
            daemon_pid: None,
            expected_hotspot_ssid: Some("Noah".to_owned()),
            observed_hotspot_ssid: Some("Noah".to_owned()),
            commute_route_interface: Some("en0".to_owned()),
            commute_route_gateway: Some("192.0.0.1".to_owned()),
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
    fn interactive_hotspot_confirmation_needs_no_extra_flag() {
        assert_eq!(
            accepted_redacted_evidence(false, Some(RedactedWifiEvidence::UserConfirmation)),
            Some(RedactedWifiEvidence::UserConfirmation)
        );
        assert_eq!(
            accepted_redacted_evidence(false, Some(RedactedWifiEvidence::SavedNetworkJoin)),
            None
        );
        assert_eq!(
            accepted_redacted_evidence(true, Some(RedactedWifiEvidence::SavedNetworkJoin)),
            Some(RedactedWifiEvidence::SavedNetworkJoin)
        );
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

    #[test]
    fn manual_unpack_finalization_produces_an_unpack_report() {
        let started_at = "2026-07-24T14:00:00Z"
            .parse::<chrono::DateTime<Utc>>()
            .unwrap();
        let ended_at = started_at + ChronoDuration::minutes(5);
        let completed = finalize_unpacked_session(&session(started_at), ended_at, Some(76));
        let report = SessionReport::from_session(&completed, SessionEndKind::Unpack).unwrap();

        assert_eq!(completed.phase, SessionPhase::Released);
        assert_eq!(
            completed.last_event.as_deref(),
            Some("normal sleep restored: user unpacked")
        );
        assert_eq!(report.end_kind, SessionEndKind::Unpack);
        assert_eq!(report.release_reason, "user unpacked");
        assert_eq!(report.duration_seconds, 300);
        assert_eq!(report.end_battery_percent, Some(76));

        let unavailable = finalize_unpacked_session(&session(started_at), ended_at, None);
        assert_eq!(unavailable.battery_percent, None);
    }

    #[test]
    fn automatic_release_reconstruction_preserves_the_original_end_kind() {
        let started_at = "2026-07-24T14:00:00Z"
            .parse::<chrono::DateTime<Utc>>()
            .unwrap();
        let heartbeat_at = started_at + ChronoDuration::minutes(4);
        let mut released = session(started_at);
        released.phase = SessionPhase::Released;
        released.last_heartbeat_at = Some(heartbeat_at);
        released.release_reason = Some("battery safety floor".to_owned());

        assert!(automatic_release_completed(&released));
        let completed =
            finalize_automatic_session(&released, started_at + ChronoDuration::minutes(10));
        let report = SessionReport::from_session(&completed, SessionEndKind::Automatic).unwrap();
        assert_eq!(report.released_at, heartbeat_at);
        assert_eq!(report.end_kind, SessionEndKind::Automatic);

        released.phase = SessionPhase::Releasing;
        released.ended_at = None;
        assert!(!automatic_release_completed(&released));
        released.ended_at = Some(heartbeat_at);
        assert!(automatic_release_completed(&released));
    }

    #[test]
    fn terminal_operations_are_serialized() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let first_paths = paths.clone();
        let second_paths = paths.clone();
        let (first_entered_tx, first_entered_rx) = std::sync::mpsc::channel::<()>();
        let (release_first_tx, release_first_rx) = std::sync::mpsc::channel::<()>();
        let (second_entered_tx, second_entered_rx) = std::sync::mpsc::channel::<()>();

        let first = thread::spawn(move || {
            with_terminal_operation(&first_paths, || {
                first_entered_tx.send(())?;
                release_first_rx.recv()?;
                Ok(())
            })
        });
        first_entered_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap();

        let second = thread::spawn(move || {
            with_terminal_operation(&second_paths, || {
                second_entered_tx.send(())?;
                Ok(())
            })
        });
        assert!(second_entered_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err());

        release_first_tx.send(()).unwrap();
        first.join().unwrap().unwrap();
        second_entered_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        second.join().unwrap().unwrap();
    }

    #[test]
    fn watcher_start_is_observed_before_a_completed_session_can_release() {
        let started_at = Utc::now();
        let mut started = session(started_at);
        started.phase = SessionPhase::Completed;
        started.daemon_pid = Some(42);
        started.last_heartbeat_at = Some(started_at);

        assert!(watcher_has_started(&started, started.id, 42));
        assert!(!watcher_has_started(&started, Uuid::new_v4(), 42));
        assert!(!watcher_has_started(&started, started.id, 43));
        started.last_heartbeat_at = None;
        assert!(!watcher_has_started(&started, started.id, 42));
    }

    #[test]
    fn watcher_process_identity_requires_the_exact_session_command() {
        let session_id = Uuid::new_v4();
        let expected = ProcessInfo {
            pid: 42,
            command: "/usr/local/bin/rucksack".to_owned(),
            arguments: format!("/usr/local/bin/rucksack daemon --session-id {session_id}"),
        };

        assert!(process_matches_session_watcher(&expected, session_id));

        let wrong_session = Uuid::new_v4();
        assert!(!process_matches_session_watcher(&expected, wrong_session));

        let mut extra_argument = expected.clone();
        extra_argument.arguments.push_str(" --unexpected");
        assert!(!process_matches_session_watcher(
            &extra_argument,
            session_id
        ));
    }

    #[test]
    fn process_signal_rejects_group_and_out_of_range_ids() {
        assert!(checked_process_id(0).is_err());
        assert!(checked_process_id(u32::MAX).is_err());
        assert_eq!(checked_process_id(42).unwrap(), 42);
    }

    #[test]
    fn terminal_sessions_do_not_signal_persisted_watcher_pids() {
        assert!(!phase_has_stoppable_watcher(SessionPhase::Completed));
        assert!(!phase_has_stoppable_watcher(SessionPhase::Releasing));
        assert!(!phase_has_stoppable_watcher(SessionPhase::Released));
        assert!(!phase_has_stoppable_watcher(SessionPhase::Failed));
        assert!(phase_has_stoppable_watcher(SessionPhase::Active));
        assert!(phase_has_stoppable_watcher(SessionPhase::IdleGrace));
    }

    #[test]
    fn failed_pack_rollback_clears_its_saved_session() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        fs::create_dir_all(&paths.data_dir).unwrap();
        let mut persisted = session(Utc::now());
        persisted.save(&paths).unwrap();

        let mut cleanup = PackCleanup::new(&paths);
        cleanup.session_id = Some(persisted.id);
        assert!(cleanup.rollback().is_empty());
        cleanup.committed = true;

        assert!(SessionState::load(&paths).unwrap().is_none());
    }
}
