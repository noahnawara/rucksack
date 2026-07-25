use crate::cli::{PackArgs, StatusArgs};
use crate::helper_client::HelperClient;
use crate::install;
use crate::output::Output;
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Local, Utc};
use rucksack_core::files::{ensure_private_dir, with_advisory_lock};
use rucksack_core::network::{
    connect_saved_wifi, internet_probe, read_default_route, read_iphone_usb_device,
    read_wifi_status, RouteStatus, DEFAULT_INTERNET_PROBE_URL,
};
use rucksack_core::power::{read_power_status, read_sleep_disabled, read_thermal_status};
use rucksack_core::state::SessionState;
use rucksack_core::system::{current_uid, processes, run_owned, ProcessInfo};
use rucksack_core::{codex, skill, AppPaths, Config};
use std::fs::OpenOptions;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// How long an automatic hotspot join is given before rucksack hands off to the Wi-Fi menu.
const AUTOMATIC_JOIN_TIMEOUT: Duration = Duration::from_secs(20);
const NETWORK_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How often to reassure the user that rucksack is still waiting for them.
const WAIT_TICK: Duration = Duration::from_secs(30);
const WIFI_SETTINGS_URL: &str = "x-apple.systempreferences:com.apple.Network-Settings.extension";

pub fn pack(args: &PackArgs, output: &Output, paths: &AppPaths, config: &Config) -> Result<()> {
    with_terminal_operation(paths, || {
        let mut cleanup = PackCleanup::new(paths);
        let result = pack_inner(args, output, paths, config, &mut cleanup);
        if result.is_ok() {
            cleanup.committed = true;
            return result;
        }
        let errors = cleanup.rollback();
        cleanup.committed = true;
        match result {
            Err(error) if errors.is_empty() => Err(error),
            Err(error) => Err(anyhow!(
                "{error:#}\nRollback was incomplete:\n- {}",
                errors.join("\n- ")
            )),
            Ok(()) => Ok(()),
        }
    })
}

fn pack_inner(
    args: &PackArgs,
    output: &Output,
    paths: &AppPaths,
    base_config: &Config,
    cleanup: &mut PackCleanup<'_>,
) -> Result<()> {
    if !cfg!(target_os = "macos") {
        anyhow::bail!("rucksack currently requires macOS.");
    }
    require_nothing_packed(paths)?;

    let mut config = base_config.clone();
    if let Some(ssid) = &args.hotspot {
        config.hotspot.ssid = Some(ssid.clone());
        config.hotspot.require_iphone_usb = false;
    } else if args.usb {
        config.hotspot.ssid = None;
        config.hotspot.require_iphone_usb = true;
    }
    if let Some(minutes) = args.duration_minutes {
        config.session.duration_minutes = minutes;
    }
    config
        .validate()
        .map_err(|error| anyhow!("The saved configuration is not usable: {error}"))?;

    require_normal_sleep()?;
    let battery = require_enough_battery(&config, output)?;
    require_no_thermal_throttling()?;

    let helper = ensure_helper(HelperClient::default(), output)?;
    let network = ensure_commute_network(args, &config, output)?;
    remember_hotspot(&mut config, &network, args, paths, output);

    let started_at = Utc::now();
    let expires_at = started_at + ChronoDuration::minutes(config.session.duration_minutes as i64);
    let lease_id = Uuid::new_v4();
    cleanup.helper = Some(helper.clone());
    cleanup.lease_id = Some(lease_id);
    helper
        .acquire(
            lease_id,
            config.session.helper_ttl_seconds,
            expires_at,
            "rucksack",
        )
        .context("The power helper would not hold this Mac awake.")?;

    start_remote_control(args.require_remote, paths, output)?;

    let mut session = SessionState::new(lease_id, current_uid(), started_at, expires_at);
    session.hotspot = network.ssid;
    session.route_interface = network.route.interface;
    session.battery_percent = battery;
    session.save(paths)?;
    cleanup.session = true;

    let daemon_pid = spawn_watcher(session.id, paths)?;
    cleanup.watcher = Some((daemon_pid, session.id));
    let session = wait_for_watcher(session.id, daemon_pid, paths)?;
    if !session.is_holding_a_lease() {
        anyhow::bail!(
            "The safety watcher stopped during startup: {}",
            session
                .release_reason
                .as_deref()
                .or(session.last_event.as_deref())
                .unwrap_or("no reason recorded")
        );
    }

    // The skill only makes "pack my Mac" work as a sentence, so it must never be able to fail pack.
    if let Err(error) = skill::install(paths) {
        output.detail(format!("Skill not installed: {error:#}"));
    }

    output.step(format!(
        "Awake for {}, or until the battery hits {}%. Ends {}.",
        format_duration(config.session.duration_minutes),
        config.safety.sleep_battery_percent,
        format_deadline(expires_at)
    ));
    output.done("Packed. Close the lid and go.");
    Ok(())
}

fn require_nothing_packed(paths: &AppPaths) -> Result<()> {
    // A session file rucksack cannot read must not block packing; `unpack` cleans it up.
    let Ok(Some(existing)) = SessionState::load(paths) else {
        return Ok(());
    };
    if existing.is_holding_a_lease() {
        anyhow::bail!(
            "Already packed until {}.\nRun `rucksack unpack` to stop.",
            format_deadline(existing.expires_at)
        );
    }
    Ok(())
}

/// Refuse when something else already owns the global sleep setting.
///
/// This is the one authoritative check: whatever holds it — Amphetamine, `caffeinate`, an earlier
/// crash — rucksack would not be able to hand normal sleep back afterwards.
fn require_normal_sleep() -> Result<()> {
    let disabled = read_sleep_disabled().context("Could not read this Mac's sleep setting.")?;
    if disabled != 0 {
        anyhow::bail!(
            "Sleep is already switched off by something else.\nQuit it — Amphetamine's closed-display mode, or similar — then run `rucksack pack` again."
        );
    }
    Ok(())
}

/// A Mac that is already at the floor would sleep the moment the lid closed.
///
/// A silent battery gauge is not a reason to refuse: plenty of Macs report nothing on AC power.
fn require_enough_battery(config: &Config, output: &Output) -> Result<Option<u8>> {
    let power = read_power_status().context("Could not read the battery.")?;
    if let Some(percent) = power.percent {
        if percent <= config.safety.sleep_battery_percent {
            anyhow::bail!(
                "The battery is at {percent}%, and sleep resumes at {}%, so this Mac would sleep straight away.\nPlug in for a few minutes, then run `rucksack pack` again.",
                config.safety.sleep_battery_percent
            );
        }
        if percent <= config.safety.warn_battery_percent {
            output.warn(format!(
                "Battery is {percent}%; sleep resumes at {}%.",
                config.safety.sleep_battery_percent
            ));
        }
    }
    Ok(power.percent)
}

/// Only actual throttling is a reason to refuse.
///
/// macOS declining to report thermal pressure means silence, not heat.
fn require_no_thermal_throttling() -> Result<()> {
    let thermal = read_thermal_status().context("Could not read thermal pressure.")?;
    if thermal.throttled {
        anyhow::bail!(
            "This Mac is already thermally throttled, so a closed lid would make it worse.\nLet it cool down, then run `rucksack pack` again."
        );
    }
    Ok(())
}

/// Install the power helper the first time it is needed.
///
/// The helper is the one part that genuinely needs an administrator, so rucksack asks macOS for it
/// during `pack` rather than sending the user through a separate setup command first.
fn ensure_helper(helper: HelperClient, output: &Output) -> Result<HelperClient> {
    if install::installed_helper_exists() && helper.status().is_ok() {
        return Ok(helper);
    }
    install::install_helper(output).context(
        "Could not install the power helper, and rucksack cannot hold the lid closed without it.",
    )?;
    helper
        .status()
        .context("The power helper was installed but did not answer.")?;
    Ok(helper)
}

#[derive(Debug)]
struct CommuteNetwork {
    route: RouteStatus,
    ssid: Option<String>,
}

/// What counts as being on the commute network.
///
/// Arrival cannot be judged on the network name alone, because macOS hides it from any process
/// without Location Services. It cannot be judged on "the internet works" either: the office
/// network the user is walking away from also works, and accepting it packs a Mac that goes
/// offline at the door.
#[derive(Debug)]
enum Arrival {
    /// macOS reported that it joined the network rucksack asked for.
    Joined,
    /// Proven by the network's name, or by the route visibly leaving `baseline`.
    Switched {
        expected: Option<String>,
        baseline: Option<RouteStatus>,
    },
}

/// iOS Personal Hotspot always serves 172.20.10.0/28 with itself as the gateway, so this gateway
/// is positive proof of a phone hotspot even when macOS redacts the network name.
const PERSONAL_HOTSPOT_GATEWAY: &str = "172.20.10.1";

fn is_personal_hotspot(route: &RouteStatus) -> bool {
    route.gateway.as_deref() == Some(PERSONAL_HOTSPOT_GATEWAY)
}

fn arrival_confirmed(arrival: &Arrival, route: &RouteStatus, ssid: Option<&str>) -> bool {
    match arrival {
        Arrival::Joined => true,
        Arrival::Switched { expected, baseline } => {
            let named = expected.is_some() && ssid == expected.as_deref();
            let moved = baseline.as_ref().is_some_and(|baseline| {
                baseline.interface != route.interface || baseline.gateway != route.gateway
            });
            named || moved || is_personal_hotspot(route)
        }
    }
}

/// Has the Mac arrived on a network that will survive leaving the building?
///
/// Identity is checked before the internet probe, so a poll loop does not pay a six-second HTTP
/// request against the network it is trying to leave.
fn arrived(arrival: &Arrival, config: &Config) -> Option<CommuteNetwork> {
    let route = read_default_route().ok()?;
    route.interface.as_ref()?;
    let ssid = read_wifi_status().ok().and_then(|status| status.ssid);
    if !arrival_confirmed(arrival, &route, ssid.as_deref()) {
        return None;
    }
    internet_probe(
        DEFAULT_INTERNET_PROBE_URL,
        config.hotspot.probe_timeout_seconds,
    )
    .reachable
    .then_some(CommuteNetwork { route, ssid })
}

fn ensure_commute_network(
    args: &PackArgs,
    config: &Config,
    output: &Output,
) -> Result<CommuteNetwork> {
    if config.hotspot.require_iphone_usb {
        return wait_for_iphone_usb(config, output);
    }
    let baseline = read_default_route().ok();
    let current_ssid = read_wifi_status().ok().and_then(|status| status.ssid);

    // `--here` is the user saying this network is the commute network.
    //
    // rucksack cannot always tell: macOS hides network names from processes without Location
    // Services, and only an iPhone hotspot has a recognisable gateway. Rather than leave someone
    // on a travel router or an Android hotspot waiting for a switch they have already made, take
    // them at their word — and still refuse if the network does not actually reach the internet.
    if args.here {
        let network = arrived(&Arrival::Joined, config).context(
            "This Mac has no working internet connection right now.\nConnect it, then run `rucksack pack --here` again.",
        )?;
        output.step(match network.ssid.as_deref() {
            Some(ssid) => format!("On {ssid}."),
            None => "Online.".to_owned(),
        });
        return Ok(network);
    }

    // Already on a phone hotspot, or already on the saved network: nothing to switch.
    let already_there = baseline.as_ref().is_some_and(is_personal_hotspot)
        || (config.hotspot.ssid.is_some() && current_ssid == config.hotspot.ssid);
    if already_there {
        if let Some(network) = arrived(&Arrival::Joined, config) {
            output.step(match network.ssid.as_deref() {
                Some(ssid) => format!("On {ssid}."),
                None => "Online.".to_owned(),
            });
            return Ok(network);
        }
    }

    let Some(expected) = config.hotspot.ssid.clone() else {
        return Ok(wait_for_network(
            "Switch this Mac to your hotspot in Wi-Fi. Waiting…",
            &Arrival::Switched {
                expected: None,
                baseline,
            },
            config,
            output,
        ));
    };

    // Only ask macOS to switch networks when there is nothing to lose.
    //
    // `networksetup -setairportnetwork` cannot supply a keychain password and cannot see an Apple
    // Instant Hotspot at all, so it usually fails — and a failed attempt drops the connection the
    // Mac already had. Gambling a working network on it would make rucksack the reason someone
    // went offline. When the Mac is already online, the Wi-Fi menu is both faster and safe.
    if online_now(config) {
        return Ok(wait_for_network(
            &format!("Choose “{expected}” in Wi-Fi. Waiting…"),
            &Arrival::Switched {
                expected: Some(expected),
                baseline,
            },
            config,
            output,
        ));
    }

    output.step(format!("Connecting to {expected}…"));
    let joined = match read_wifi_status().ok().and_then(|status| status.device) {
        Some(device) => connect_saved_wifi(&device, &expected)
            .inspect_err(|error| output.detail(format!("Automatic join failed: {error}")))
            .is_ok(),
        None => false,
    };
    if joined {
        if let Some(network) = wait_for_arrival(&Arrival::Joined, AUTOMATIC_JOIN_TIMEOUT, config) {
            output.step("Joined.");
            return Ok(network);
        }
    }

    Ok(wait_for_network(
        &format!("Choose “{expected}” in Wi-Fi. Waiting…"),
        &Arrival::Switched {
            expected: Some(expected),
            baseline,
        },
        config,
        output,
    ))
}

/// Does this Mac currently reach the internet at all?
fn online_now(config: &Config) -> bool {
    read_default_route().is_ok_and(|route| route.interface.is_some())
        && internet_probe(
            DEFAULT_INTERNET_PROBE_URL,
            config.hotspot.probe_timeout_seconds,
        )
        .reachable
}

/// Wait for the user to pick a network, for as long as it takes.
///
/// Never gives up and never asks for a re-run: the user is mid-stride, and the one thing they must
/// not have to do is start over. Ctrl-C is the way out.
fn wait_for_network(
    instruction: &str,
    arrival: &Arrival,
    config: &Config,
    output: &Output,
) -> CommuteNetwork {
    // Printed before System Settings opens, which steals focus.
    output.step(instruction);
    open_wifi_settings(output);

    let started = Instant::now();
    let mut next_tick = WAIT_TICK;
    loop {
        if let Some(network) = arrived(arrival, config) {
            output.step(match network.ssid.as_deref() {
                Some(ssid) => format!("On {ssid}."),
                None => "Online.".to_owned(),
            });
            return network;
        }
        if started.elapsed() >= next_tick {
            output.step("Still waiting for the network…");
            next_tick += WAIT_TICK;
        }
        thread::sleep(NETWORK_POLL_INTERVAL);
    }
}

fn wait_for_arrival(
    arrival: &Arrival,
    timeout: Duration,
    config: &Config,
) -> Option<CommuteNetwork> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(network) = arrived(arrival, config) {
            return Some(network);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(NETWORK_POLL_INTERVAL);
    }
}

fn wait_for_iphone_usb(config: &Config, output: &Output) -> Result<CommuteNetwork> {
    let device = read_iphone_usb_device()?.context(
        "macOS does not expose an iPhone USB network service right now.\nConnect the iPhone by cable and turn on Personal Hotspot, then run `rucksack pack` again.",
    )?;
    let on_iphone =
        |network: &CommuteNetwork| network.route.interface.as_deref() == Some(device.as_str());
    if let Some(network) = arrived(&Arrival::Joined, config).filter(on_iphone) {
        output.step("Online through the iPhone.");
        return Ok(network);
    }

    output.step("Turn on Personal Hotspot on the connected iPhone. Waiting…");
    let started = Instant::now();
    let mut next_tick = WAIT_TICK;
    loop {
        if let Some(network) = arrived(&Arrival::Joined, config).filter(on_iphone) {
            output.step("Online through the iPhone.");
            return Ok(network);
        }
        if started.elapsed() >= next_tick {
            output.step("Still waiting for the iPhone…");
            next_tick += WAIT_TICK;
        }
        thread::sleep(NETWORK_POLL_INTERVAL);
    }
}

/// Learn the hotspot from the first successful pack, so the next one needs no arguments.
///
/// Only ever records a network the user passed explicitly or actually arrived on.
fn remember_hotspot(
    config: &mut Config,
    network: &CommuteNetwork,
    args: &PackArgs,
    paths: &AppPaths,
    output: &Output,
) {
    if args.usb || args.here || config.hotspot.ssid.is_some() {
        // Save an explicitly passed `--hotspot` so it is remembered next time.
        if args.hotspot.is_some() {
            let _ = config.save(paths);
        }
        return;
    }
    let Some(ssid) = network.ssid.clone() else {
        return;
    };
    config.hotspot.ssid = Some(ssid.clone());
    match config.save(paths) {
        Ok(()) => output.step(format!("Saved “{ssid}” as your hotspot.")),
        Err(error) => output.detail(format!("Could not save the hotspot: {error:#}")),
    }
}

fn open_wifi_settings(output: &Output) {
    match run_owned("/usr/bin/open", &[WIFI_SETTINGS_URL.to_owned()]) {
        Ok(result) if result.success() => {}
        Ok(result) => output.detail(format!(
            "Could not open Wi-Fi settings: {}",
            result.combined_trimmed()
        )),
        Err(error) => output.detail(format!("Could not open Wi-Fi settings: {error}")),
    }
}

/// Start Codex Remote Control without making the user wait for it.
///
/// Remote Control is how a phone reaches the work; it is not what keeps the Mac awake. So it is
/// spawned and forgotten, and only `--require-remote` makes a failure fatal.
fn start_remote_control(require_remote: bool, paths: &AppPaths, output: &Output) -> Result<()> {
    let Some(executable) = codex::executable() else {
        if require_remote {
            anyhow::bail!(
                "Codex was not found, so Remote Control could not be started.\nInstall Codex, then run `rucksack pack` again."
            );
        }
        return Ok(());
    };

    if require_remote {
        let result = codex::start_remote_control()?;
        if !result.success() {
            anyhow::bail!(
                "Codex Remote Control could not be started: {}\nThis Mac still sleeps normally. Run `rucksack pair`, then run `rucksack pack` again.",
                result.combined_trimmed()
            );
        }
        output.detail(result.combined_trimmed());
        return Ok(());
    }

    ensure_private_dir(&paths.log_dir)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.daemon_log)?;
    let spawned = Command::new(executable)
        .args(codex::remote_control_arguments("start"))
        .stdin(Stdio::null())
        .stderr(log.try_clone()?)
        .stdout(log)
        .spawn();
    if let Err(error) = spawned {
        output.warn("Remote Control did not start. Your tasks keep running, but your phone may not reach them.");
        output.detail(format!("{error:#}"));
    }
    Ok(())
}

pub fn status(args: &StatusArgs, output: &Output, paths: &AppPaths) -> Result<()> {
    let helper = HelperClient::default().status();
    let session = match SessionState::load(paths) {
        Ok(session) => session,
        Err(error) => {
            output.done("Not packed. This Mac sleeps normally.");
            output.warn(format!(
                "rucksack could not read its own session state ({error}). `rucksack unpack` clears it."
            ));
            return Ok(());
        }
    };

    match session.filter(SessionState::is_holding_a_lease) {
        Some(session) => {
            output.done(format!(
                "Packed · {} · battery {} · {} left",
                session
                    .hotspot
                    .as_deref()
                    .or(session.route_interface.as_deref())
                    .unwrap_or("online"),
                session
                    .battery_percent
                    .map(|percent| format!("{percent}%"))
                    .unwrap_or_else(|| "unknown".to_owned()),
                format_duration(session.remaining_minutes(Utc::now()))
            ));
            if !session.online {
                output
                    .step("Offline right now. This Mac is still awake, but nothing can reach it.");
            }
            if let Some(event) = &session.last_event {
                output.detail(format!("Last event: {event}"));
            }
            if args.full {
                output.step(serde_json::to_string_pretty(&session)?);
            }
        }
        None => {
            let holding = helper.as_ref().ok().and_then(Option::as_ref);
            if holding.is_some_and(|status| status.active) {
                output.done("Not packed, but something is still holding this Mac awake.");
                output.step("Run `rucksack unpack` to let it sleep again.");
            } else if holding.is_some_and(|status| status.sleep_disabled == Some(1)) {
                output.done("Not packed, but this Mac still will not sleep.");
                output.step("Another app owns that setting; quit it to let this Mac sleep again.");
            } else {
                output.done("Not packed. This Mac sleeps normally.");
                if let Some(reason) = last_release_reason(paths) {
                    output.step(format!("The last session ended: {reason}"));
                }
            }
        }
    }
    if let Err(error) = helper {
        output.detail(format!("Power helper: {error:#}"));
    }
    Ok(())
}

fn last_release_reason(paths: &AppPaths) -> Option<String> {
    SessionState::load(paths).ok()?.and_then(|session| {
        (!session.is_holding_a_lease())
            .then_some(session.release_reason)
            .flatten()
    })
}

pub fn unpack(output: &Output, paths: &AppPaths) -> Result<()> {
    with_terminal_operation(paths, || unpack_locked(output, paths))
}

/// Let this Mac sleep again, from any state.
///
/// This is also the recovery path: an unreadable session file, a lease whose id no longer matches,
/// or a watcher that died all end here, because "let my Mac sleep" is the only thing the user
/// wants and it must never dead-end into a second command.
fn unpack_locked(output: &Output, paths: &AppPaths) -> Result<()> {
    let helper = HelperClient::default();
    let session = match SessionState::load(paths) {
        Ok(session) => session,
        Err(error) => {
            output.warn(format!("Ignoring unreadable session state: {error}"));
            None
        }
    };

    let released = restore_normal_sleep(&helper, session.as_ref())?;
    if let Some(session) = session.as_ref() {
        if let Some(pid) = session.daemon_pid {
            stop_watcher(pid, session.id);
        }
        report_outcome(session, released, output);
    } else if released {
        output.step("Released a power lease rucksack was not tracking.");
    }

    SessionState::clear(paths).context("Could not clear the session state.")?;
    if read_sleep_disabled().unwrap_or(0) != 0 {
        output.warn("Sleep is still switched off — something else is holding it.");
    }
    let had_something_to_release = released || session.is_some();
    output.done(if had_something_to_release {
        "Unpacked. This Mac sleeps normally."
    } else {
        "Already unpacked. This Mac sleeps normally."
    });
    if had_something_to_release {
        crate::star::offer_once(paths, output);
    }
    Ok(())
}

/// Give sleep back, escalating until something works.
///
/// Ordered so the lease id is used while it is still known: releasing by id, then by owner uid,
/// then accepting macOS's own answer that sleep is already normal.
fn restore_normal_sleep(helper: &HelperClient, session: Option<&SessionState>) -> Result<bool> {
    if let Some(session) = session {
        if let Ok(status) = helper.release(session.lease_id, "user unpacked") {
            if !status.active {
                return Ok(true);
            }
        }
    }
    if let Ok(Some(status)) = helper.recover() {
        if !status.active {
            return Ok(true);
        }
    }
    match read_sleep_disabled() {
        Ok(0) => Ok(false),
        Ok(_) => anyhow::bail!(
            "Something is holding this Mac awake and rucksack could not release it.\nQuit any closed-display utility, then run `rucksack unpack` again."
        ),
        Err(error) => Err(error).context("Could not confirm this Mac can sleep again."),
    }
}

/// Say what the session did, because the release reason is the most useful thing rucksack knows.
fn report_outcome(session: &SessionState, released: bool, output: &Output) {
    match (session.is_holding_a_lease(), &session.release_reason) {
        (false, Some(reason)) => {
            let when = session
                .ended_at
                .map(format_time)
                .unwrap_or_else(|| "earlier".to_owned());
            output.step(format!("This Mac went back to sleep at {when} — {reason}."));
        }
        _ if released => {
            let minutes = (Utc::now() - session.started_at).num_minutes().max(0) as u64;
            output.step(format!("Packed for {}.", format_duration(minutes)));
        }
        _ => {}
    }
}

fn with_terminal_operation<T>(
    paths: &AppPaths,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    with_advisory_lock(&paths.terminal_lock_file(), operation)
}

fn format_duration(minutes: u64) -> String {
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes}m"),
        (1, 0) => "1 hour".to_owned(),
        (hours, 0) => format!("{hours} hours"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    }
}

fn format_time(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%H:%M").to_string()
}

/// A session is capped at 24 hours, so "tomorrow" is as far as a deadline can reach.
fn format_deadline(at: DateTime<Utc>) -> String {
    let at = at.with_timezone(&Local);
    let suffix = if at.date_naive() == Local::now().date_naive() {
        ""
    } else {
        " tomorrow"
    };
    format!("{}{suffix}", at.format("%H:%M"))
}

fn spawn_watcher(session_id: Uuid, paths: &AppPaths) -> Result<u32> {
    ensure_private_dir(&paths.log_dir)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.daemon_log)?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("daemon")
        .arg("--session-id")
        .arg(session_id.to_string())
        .stdin(Stdio::null())
        .stderr(log.try_clone()?)
        .stdout(log);

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

    Ok(command
        .spawn()
        .context("Could not start the safety watcher.")?
        .id())
}

/// Wait for the watcher we just spawned to prove it is running.
fn wait_for_watcher(session_id: Uuid, daemon_pid: u32, paths: &AppPaths) -> Result<SessionState> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Ok(Some(session)) = SessionState::load(paths) {
            if session.id == session_id
                && session.daemon_pid == Some(daemon_pid)
                && session.last_heartbeat_at.is_some()
            {
                return Ok(session);
            }
        }
        if Instant::now() >= deadline {
            anyhow::bail!("The safety watcher did not start.");
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Stop the watcher for this session, and only that.
fn stop_watcher(pid: u32, session_id: Uuid) {
    let Ok(processes) = processes() else { return };
    let Some(process) = processes.iter().find(|process| process.pid == pid) else {
        return;
    };
    if !is_watcher_for(process, session_id) {
        return;
    }
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(pid) {
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
}

fn is_watcher_for(process: &ProcessInfo, session_id: Uuid) -> bool {
    let mut arguments = process.arguments.split_whitespace();
    let executable = arguments
        .next()
        .and_then(|argument| Path::new(argument).file_name())
        .and_then(|name| name.to_str());
    executable == Some("rucksack")
        && arguments.next() == Some("daemon")
        && arguments.next() == Some("--session-id")
        && arguments.next().and_then(|id| Uuid::parse_str(id).ok()) == Some(session_id)
}

/// Undo a half-finished `pack`, so a failure never leaves the Mac unable to sleep.
struct PackCleanup<'a> {
    paths: &'a AppPaths,
    helper: Option<HelperClient>,
    lease_id: Option<Uuid>,
    session: bool,
    watcher: Option<(u32, Uuid)>,
    committed: bool,
}

impl<'a> PackCleanup<'a> {
    fn new(paths: &'a AppPaths) -> Self {
        Self {
            paths,
            helper: None,
            lease_id: None,
            session: false,
            watcher: None,
            committed: false,
        }
    }

    fn rollback(&mut self) -> Vec<String> {
        if self.committed {
            return Vec::new();
        }
        self.committed = true;
        let mut errors = Vec::new();
        if let Some((pid, session_id)) = self.watcher.take() {
            stop_watcher(pid, session_id);
        }
        if let (Some(helper), Some(lease_id)) = (self.helper.as_ref(), self.lease_id) {
            match helper.release(lease_id, "packing failed") {
                Ok(status) if !status.active => {}
                Ok(_) => errors.push(
                    "the power helper could not confirm normal sleep was restored".to_owned(),
                ),
                Err(error) => errors.push(format!("the power helper refused to release: {error}")),
            }
        }
        if self.session {
            if let Err(error) = SessionState::clear(self.paths) {
                errors.push(format!("stale session state was left behind: {error}"));
            }
        }
        errors
    }
}

impl Drop for PackCleanup<'_> {
    fn drop(&mut self) {
        let _ = self.rollback();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(interface: &str, gateway: &str) -> RouteStatus {
        RouteStatus {
            interface: Some(interface.to_owned()),
            gateway: Some(gateway.to_owned()),
            detail: String::new(),
        }
    }

    /// The network the user is walking away from also reaches the internet.
    ///
    /// Accepting it would pack a Mac that goes offline at the front door, which is the whole
    /// failure rucksack exists to prevent.
    #[test]
    fn a_working_office_network_is_not_the_commute_network() {
        let office = route("en0", "192.168.1.1");
        let arrival = Arrival::Switched {
            expected: Some("Noah".to_owned()),
            baseline: Some(office.clone()),
        };

        assert!(!arrival_confirmed(&arrival, &office, None));
        assert!(!arrival_confirmed(&arrival, &office, Some("Office")));
        assert!(arrival_confirmed(&arrival, &office, Some("Noah")));
        assert!(arrival_confirmed(
            &arrival,
            &route("en0", "172.20.10.1"),
            None
        ));
    }

    /// With no saved hotspot there is nothing to compare a name against, so the route must move.
    #[test]
    fn without_a_saved_hotspot_the_route_still_has_to_change() {
        let office = route("en0", "192.168.1.1");
        let arrival = Arrival::Switched {
            expected: None,
            baseline: Some(office.clone()),
        };

        assert!(!arrival_confirmed(&arrival, &office, None));
        assert!(!arrival_confirmed(&arrival, &office, Some("Office")));
        assert!(arrival_confirmed(
            &arrival,
            &route("en7", "192.168.1.1"),
            None
        ));
    }

    /// The one signal that survives macOS hiding the network name.
    #[test]
    fn an_iphone_hotspot_gateway_is_proof_on_its_own() {
        assert!(is_personal_hotspot(&route("en0", "172.20.10.1")));
        assert!(!is_personal_hotspot(&route("en0", "192.168.1.1")));
    }

    #[test]
    fn a_join_that_macos_confirmed_needs_no_further_proof() {
        assert!(arrival_confirmed(
            &Arrival::Joined,
            &route("en0", "192.168.1.1"),
            None
        ));
    }

    #[test]
    fn formats_durations_a_person_would_say() {
        assert_eq!(format_duration(45), "45m");
        assert_eq!(format_duration(60), "1 hour");
        assert_eq!(format_duration(24 * 60), "24 hours");
        assert_eq!(format_duration(90), "1h 30m");
    }

    #[test]
    fn recognises_its_own_watcher_and_nothing_else() {
        let session_id = Uuid::new_v4();
        let watcher = ProcessInfo {
            pid: 42,
            command: "/usr/local/bin/rucksack".to_owned(),
            arguments: format!("/usr/local/bin/rucksack daemon --session-id {session_id}"),
        };
        assert!(is_watcher_for(&watcher, session_id));
        assert!(!is_watcher_for(&watcher, Uuid::new_v4()));

        let impostor = ProcessInfo {
            pid: 42,
            command: "/bin/rm".to_owned(),
            arguments: format!("/bin/rm daemon --session-id {session_id}"),
        };
        assert!(!is_watcher_for(&impostor, session_id));
    }
}
