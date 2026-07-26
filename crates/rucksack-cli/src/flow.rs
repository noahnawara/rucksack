use crate::cli::{PackArgs, StatusArgs};
use crate::helper_client::HelperClient;
use crate::install;
use crate::output::Output;
use crate::thermal::{ends_a_lease, read_thermal_state};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Local, Utc};
use rucksack_core::files::{ensure_private_dir, with_advisory_lock};
use rucksack_core::network::{
    connect_saved_wifi, reaches_internet, read_default_route, read_iphone_usb_device,
    read_wifi_status, RouteStatus, DEFAULT_INTERNET_PROBE_URL,
};
use rucksack_core::power::{read_power_status, read_sleep_disabled, read_thermal_status};
use rucksack_core::state::{silence_tolerance, Limit, SessionState};
use rucksack_core::system::{processes, run, ProcessInfo};
use rucksack_core::{codex, skill, AdaptersConfig, AppPaths, Config};
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

    // First, because the first pack is the one that needs it most.
    //
    // The skill is what tells an agent how to behave while pack is waiting, so writing it at the end
    // gave it to everyone except the person who had not run pack before — and a pack interrupted
    // during a network wait never wrote it at all. It also cannot fail a pack from here, for the same
    // reason it could not before: making "pack my Mac" work as a sentence is worth nothing next to
    // keeping the Mac awake.
    if let Err(error) = skill::install(paths, &base_config.adapters) {
        output.detail(format!("Skill not installed: {error:#}"));
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
    require_no_thermal_pressure()?;

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

    start_remote_control(args.require_remote, &config.adapters, paths, output)?;

    let mut session = SessionState::new(lease_id, started_at, expires_at);
    session.hotspot = network.ssid;
    session.route_interface = network.route.interface;
    session.battery_percent = battery;
    session.started_battery_percent = battery;
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

/// Refuse to pack a Mac the watcher would release on its first heartbeat.
///
/// This asks exactly what the watcher asks, from both sources, so the two cannot disagree.
/// Accepting a pack under heat that ends a lease would put the Mac to sleep seconds later, having
/// already changed the network — the opposite of what the user asked for.
///
/// macOS declining to report thermal pressure still means silence, not heat.
fn require_no_thermal_pressure() -> Result<()> {
    let thermal = read_thermal_status().context("Could not read thermal pressure.")?;
    if thermal.throttled
        || ends_a_lease(thermal.level)
        || read_thermal_state().is_some_and(ends_a_lease)
    {
        anyhow::bail!(
            "This Mac is already too hot, so a closed lid would make it worse.\nLet it cool down, then run `rucksack pack` again."
        );
    }
    Ok(())
}

/// Install the power helper the first time it is needed.
///
/// The helper is the one part that genuinely needs an administrator, so rucksack asks macOS for it
/// during `pack` rather than sending the user through a separate setup command first.
/// Make sure the root power helper is there, installing it on the first pack.
///
/// The failure this names is the common one, not an exotic one. Installing runs `sudo`, and `sudo`
/// asks for a password on a controlling terminal — which the agent session that runs `rucksack pack`
/// for most users does not have. So the first pack of a fresh install fails here, and the only thing
/// that fixes it is a human running one command in a real terminal window. Saying so is the
/// difference between a product that looks broken and one that takes thirty seconds to set up.
fn ensure_helper(helper: HelperClient, output: &Output) -> Result<HelperClient> {
    if install::installed_helper_exists() && helper.status().is_ok() {
        return Ok(helper);
    }
    install::install_helper(output).context(
        "Could not install the power helper, and rucksack cannot hold the lid closed without it.\nRun `rucksack helper install` in a terminal window — it needs a password — then run `rucksack pack` again.",
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

/// What rucksack must see before it believes the Mac has moved networks.
///
/// Arrival cannot be judged on the network name alone, because macOS hides it from any process
/// without Location Services. It cannot be judged on "the internet works" either: the office
/// network the user is walking away from also works, and accepting it packs a Mac that goes
/// offline at the door.
#[derive(Debug)]
struct CommuteTarget {
    /// The saved network's name, when there is one and macOS will reveal it.
    expected: Option<String>,
    /// Where the route pointed before the switch.
    baseline: Option<RouteStatus>,
}

/// The gateways only an iOS Personal Hotspot serves, so either one is positive proof of a phone
/// hotspot even when macOS redacts the network name.
///
/// On a dual-stack carrier the hotspot serves 172.20.10.0/28 with itself as the gateway. On an
/// IPv6-only carrier iOS runs 464XLAT instead and hands the Mac 192.0.0.2/32 with the gateway
/// 192.0.0.1, out of the RFC 7335 IPv4 Service Continuity prefix. That prefix is reserved for this
/// use, but only these host addresses are proof, so the match stays exact rather than by subnet.
const PERSONAL_HOTSPOT_GATEWAYS: [&str; 2] = ["172.20.10.1", "192.0.0.1"];

fn is_personal_hotspot(route: &RouteStatus) -> bool {
    route
        .gateway
        .as_deref()
        .is_some_and(|gateway| PERSONAL_HOTSPOT_GATEWAYS.contains(&gateway))
}

/// Has the Mac left the network it started on?
///
/// Proven by the network's name, by an iPhone hotspot gateway, or by the route visibly moving.
/// Never by "the internet works": the network being walked away from also works.
fn has_switched(target: &CommuteTarget, route: &RouteStatus, ssid: Option<&str>) -> bool {
    let named = target.expected.is_some() && ssid == target.expected.as_deref();
    let moved = target
        .baseline
        .as_ref()
        .is_some_and(|baseline| baseline != route);
    named || moved || is_personal_hotspot(route)
}

/// Is there a working internet connection right now, and what is it?
///
/// The probe runs last, so a poll loop never pays a six-second HTTP request before it knows the
/// route is even plausible. The Wi-Fi name is read only when someone will compare it.
fn online(config: &Config, want_name: bool) -> Option<CommuteNetwork> {
    let route = read_default_route().ok()?;
    route.interface.as_ref()?;
    let ssid = want_name
        .then(|| read_wifi_status().ok().and_then(|status| status.ssid))
        .flatten();
    reaches_internet(
        DEFAULT_INTERNET_PROBE_URL,
        config.hotspot.probe_timeout_seconds,
    )
    .then_some(CommuteNetwork { route, ssid })
}

/// Has the Mac arrived on the commute network?
fn arrived(target: &CommuteTarget, config: &Config) -> Option<CommuteNetwork> {
    let route = read_default_route().ok()?;
    route.interface.as_ref()?;
    // Only the named case needs the name, and reading it costs two subprocesses.
    let ssid = target
        .expected
        .is_some()
        .then(|| read_wifi_status().ok().and_then(|status| status.ssid))
        .flatten();
    if !has_switched(target, &route, ssid.as_deref()) {
        return None;
    }
    reaches_internet(
        DEFAULT_INTERNET_PROBE_URL,
        config.hotspot.probe_timeout_seconds,
    )
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

    // `--here` is the user saying this network is the commute network.
    //
    // rucksack cannot always tell: macOS hides network names from processes without Location
    // Services, and only an iPhone hotspot has a recognisable gateway. Rather than leave someone
    // on a travel router or an Android hotspot waiting for a switch they have already made, take
    // them at their word — and still refuse if the network does not actually reach the internet.
    if args.here {
        let network = online(config, true).context(
            "This Mac has no working internet connection right now.\nConnect it, then run `rucksack pack --here` again.",
        )?;
        announce(&network, output);
        return Ok(network);
    }

    let baseline = read_default_route().ok();
    let expected = config.hotspot.ssid.clone();
    // Already on a phone hotspot, or already on the saved network: nothing to switch.
    let already_there = baseline.as_ref().is_some_and(is_personal_hotspot)
        || (expected.is_some()
            && read_wifi_status().ok().and_then(|status| status.ssid) == expected);
    if already_there {
        if let Some(network) = online(config, true) {
            announce(&network, output);
            return Ok(network);
        }
    }

    let target = CommuteTarget { expected, baseline };
    let instruction = wait_instruction(target.expected.as_deref());

    // Only ask macOS to switch networks when there is nothing to lose.
    //
    // `networksetup -setairportnetwork` cannot supply a keychain password and cannot see an Apple
    // Instant Hotspot at all, so it usually fails — and a failed attempt drops the connection the
    // Mac already had. Gambling a working network on it would make rucksack the reason someone
    // went offline. When the Mac is already online, the Wi-Fi menu is both faster and safe.
    if let Some(expected) = target.expected.clone() {
        if online(config, false).is_none() {
            output.step(format!("Connecting to {expected}…"));
            let joined = match read_wifi_status().ok().and_then(|status| status.device) {
                Some(device) => connect_saved_wifi(&device, &expected)
                    .inspect_err(|error| output.detail(format!("Automatic join failed: {error}")))
                    .is_ok(),
                None => false,
            };
            if joined {
                if let Some(network) = wait_for_join(config) {
                    output.step("Joined.");
                    return Ok(network);
                }
            }
        }
    }

    Ok(wait_for_network(&instruction, &target, config, output))
}

/// The one thing to do, and the one thing not to do while it is being done.
///
/// The lid clause is the whole message. Until `pack` prints its verdict nothing is holding this Mac
/// awake, so a lid closed during the wait sleeps it and takes every running task down — the exact
/// loss rucksack exists to prevent, arrived at by being helpful. It is one clause in one string
/// because a user who is already halfway out of the door will read one clause.
fn wait_instruction(expected: Option<&str>) -> String {
    match expected {
        Some(expected) => {
            format!("Choose “{expected}” in Wi-Fi — keep the lid open until this says Packed. Waiting…")
        }
        None => "Switch this Mac to your hotspot in Wi-Fi — keep the lid open until this says Packed. Waiting…".to_owned(),
    }
}

/// The one sentence that reports a successful arrival.
fn announce(network: &CommuteNetwork, output: &Output) {
    output.step(match network.ssid.as_deref() {
        Some(ssid) => format!("On {ssid}."),
        None => "Online.".to_owned(),
    });
}

/// Wait for the user to pick a network, for as long as it takes.
///
/// Never gives up and never asks for a re-run: the user is mid-stride, and the one thing they must
/// not have to do is start over. Ctrl-C is the way out.
fn wait_for_network(
    instruction: &str,
    target: &CommuteTarget,
    config: &Config,
    output: &Output,
) -> CommuteNetwork {
    // Printed before System Settings opens, which steals focus.
    output.step(instruction);
    open_wifi_settings(output);

    let network = poll(
        output,
        "Still waiting for the network — lid open.",
        || arrived(target, config),
    );
    announce(&network, output);
    network
}

/// Give a confirmed automatic join a moment to come up, before falling back to the Wi-Fi menu.
fn wait_for_join(config: &Config) -> Option<CommuteNetwork> {
    let deadline = Instant::now() + AUTOMATIC_JOIN_TIMEOUT;
    loop {
        if let Some(network) = online(config, true) {
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
    let on_the_iphone = || {
        online(config, false)
            .filter(|network| network.route.interface.as_deref() == Some(device.as_str()))
    };
    if on_the_iphone().is_none() {
        output.step(
            "Turn on Personal Hotspot on the connected iPhone — keep the lid open until this says Packed. Waiting…",
        );
    }
    let network = poll(
        output,
        "Still waiting for the iPhone — lid open.",
        on_the_iphone,
    );
    output.step("Online through the iPhone.");
    Ok(network)
}

/// Wait for something to become true, for as long as it takes.
///
/// Never gives up and never asks for a re-run: the user is mid-stride, and the one thing they must
/// not have to do is start over. Ctrl-C is the way out.
fn poll(
    output: &Output,
    still_waiting: &str,
    mut ready: impl FnMut() -> Option<CommuteNetwork>,
) -> CommuteNetwork {
    let started = Instant::now();
    let mut next_tick = WAIT_TICK;
    loop {
        if let Some(network) = ready() {
            return network;
        }
        if started.elapsed() >= next_tick {
            output.step(still_waiting);
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
    match run("/usr/bin/open", &[WIFI_SETTINGS_URL]) {
        Ok(result) if result.success() => {}
        Ok(result) => output.detail(format!(
            "Could not open Wi-Fi settings: {}",
            result.combined_trimmed()
        )),
        Err(error) => output.detail(format!("Could not open Wi-Fi settings: {error}")),
    }
}

/// What `pack` does about Remote Control, decided before anything is spawned.
#[derive(Debug, PartialEq, Eq)]
enum RemoteControl {
    /// Start it, with the Codex that was resolved.
    Start(std::path::PathBuf),
    /// Switched off on purpose. Nothing is wrong, so nothing is said above `--verbose`.
    Off(String),
    /// Wanted and not available. Warned about, and carried on from.
    Unavailable(String),
    /// `--require-remote` asked for the opposite of carrying on.
    Refuse(String),
}

/// Decide what to do about Remote Control, from facts alone.
///
/// Pure, because this is the rule that keeps a lease: rucksack ships to people who run only Claude
/// Code, only Cursor, or only Codex, and an agent that one of them does not have must cost them
/// nothing. `--require-remote` is the single way to ask for a Codex problem to end a pack, and it
/// has to be asked for explicitly.
fn plan_remote_control(
    require_remote: bool,
    adapters: &AdaptersConfig,
    codex: Option<std::path::PathBuf>,
) -> RemoteControl {
    if !adapters.codex {
        let message = "Remote Control is Codex's, and `adapters.codex` is off in your configuration.\nSet it to true, then run `rucksack pack` again.";
        return if require_remote {
            RemoteControl::Refuse(message.to_owned())
        } else {
            RemoteControl::Off("Codex is switched off; Remote Control not started.".to_owned())
        };
    }
    match codex {
        Some(executable) => RemoteControl::Start(executable),
        None if require_remote => RemoteControl::Refuse(
            "Codex was not found, so Remote Control could not be started.\nInstall Codex, then run `rucksack pack` again.".to_owned(),
        ),
        None => RemoteControl::Unavailable(
            "Codex was not found, so your phone may not reach this work. Your tasks keep running."
                .to_owned(),
        ),
    }
}

/// Start Codex Remote Control without making the user wait for it.
///
/// Remote Control is how a phone reaches the work; it is not what keeps the Mac awake. So it is
/// spawned and forgotten, and only `--require-remote` makes a failure fatal.
fn start_remote_control(
    require_remote: bool,
    adapters: &AdaptersConfig,
    paths: &AppPaths,
    output: &Output,
) -> Result<()> {
    let executable =
        match plan_remote_control(require_remote, adapters, codex::executable(&paths.home)) {
            RemoteControl::Start(executable) => executable,
            RemoteControl::Off(detail) => {
                output.detail(detail);
                return Ok(());
            }
            RemoteControl::Unavailable(warning) => {
                output.warn(warning);
                return Ok(());
            }
            RemoteControl::Refuse(error) => anyhow::bail!(error),
        };

    if require_remote {
        let result = codex::start_remote_control(&paths.home)?;
        if !result.success() {
            anyhow::bail!(
                "Codex Remote Control could not be started: {}\nThis Mac still sleeps normally. Run `rucksack pair`, then run `rucksack pack` again.",
                result.combined_trimmed()
            );
        }
        output.detail(result.combined_trimmed());
        return Ok(());
    }

    // Codex gets its own log rather than the daemon's. Sharing one made a Codex that failed on
    // startup look exactly like a watcher that had crashed — the two are separate processes, and a
    // reader working out why a Mac went to sleep should not have to know that.
    ensure_private_dir(&paths.log_dir)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.remote_control_log())?;
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

pub fn status(args: &StatusArgs, output: &Output, paths: &AppPaths, config: &Config) -> Result<()> {
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

    // Answering "am I still packed?" from the session file alone keeps `status` free of the round
    // trip to the helper, which makes the root daemon run `pmset`. Only the not-packed answer needs
    // to know whether something else is holding sleep off.
    if let Some(session) = session
        .as_ref()
        .filter(|session| session.is_holding_a_lease())
    {
        if let Some(dead) = dead_session(
            session,
            watcher_is_running(session),
            config.session.heartbeat_seconds,
            Utc::now(),
        ) {
            output.done("Not packed. This Mac will sleep when you close the lid.");
            output.step(dead.describe(session));
            output.step("Run `rucksack unpack`, then `rucksack pack` again to protect it.");
            if args.full {
                output.step(serde_json::to_string_pretty(session)?);
            }
            return Ok(());
        }
        output.done(format!(
            "Packed · {} · battery {} · {}",
            session
                .hotspot
                .as_deref()
                .or(session.route_interface.as_deref())
                .unwrap_or("online"),
            session
                .battery_percent
                .map(|percent| format!("{percent}%"))
                .unwrap_or_else(|| "unknown".to_owned()),
            format_limit(session.binding_limit(
                Utc::now(),
                silence_tolerance(config.session.heartbeat_seconds)
            ))
        ));
        if !session.online {
            output.step("Offline right now. This Mac is still awake, but nothing can reach it.");
        }
        if let Some(event) = &session.last_event {
            output.detail(format!("Last event: {event}"));
        }
        if args.full {
            output.step(serde_json::to_string_pretty(session)?);
        }
        return Ok(());
    }

    let helper = HelperClient::default().status();
    let holding = helper.as_ref().ok().and_then(Option::as_ref);
    if holding.is_some_and(|status| status.active) {
        output.done("Not packed, but something is still holding this Mac awake.");
        output.step("Run `rucksack unpack` to let it sleep again.");
    } else if holding.is_some_and(|status| status.sleep_disabled == Some(1)) {
        output.done("Not packed, but this Mac still will not sleep.");
        output.step("Another app owns that setting; quit it to let this Mac sleep again.");
    } else {
        output.done("Not packed. This Mac sleeps normally.");
        if let Some(reason) = session.and_then(|session| session.release_reason) {
            output.step(format!("The last session ended: {reason}"));
        }
    }
    if let Err(error) = helper {
        output.detail(format!("Power helper: {error:#}"));
    }
    Ok(())
}

/// How many heartbeats may be missed before the record stops describing anything real.
///
/// The watcher writes one every `heartbeat_seconds` and renews the power lease in the same breath,
/// for `helper_ttl_seconds` — three beats against one, at the defaults. So by the third missed beat
/// the helper has already handed sleep back on its own, and a file still reading "active" is
/// describing a Mac that can sleep the moment the lid closes.
const MAX_MISSED_HEARTBEATS: u64 = 3;

/// Why a session that still claims a lease is not actually holding one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeadSession {
    /// The watcher stopped writing long enough ago that the lease it renews has lapsed.
    HeartbeatStopped,
    /// No watcher for this session is running.
    WatcherGone,
}

impl DeadSession {
    fn describe(self, session: &SessionState) -> String {
        match self {
            Self::HeartbeatStopped => match session.last_heartbeat_at {
                Some(at) => format!("The safety watcher last reported at {}.", format_time(at)),
                None => "The safety watcher never reported in.".to_owned(),
            },
            Self::WatcherGone => "The safety watcher is no longer running.".to_owned(),
        }
    }
}

/// Whether the session on disk still describes a Mac that is being kept awake.
///
/// `status` reads a file the watcher writes, and a watcher that dies leaves its last words behind —
/// which say "active". Read on its own, the record therefore reports a protected Mac at exactly the
/// moment it stopped being one, and someone closes the lid on it. Two facts close that gap without
/// the round trip to the root helper that the packed answer deliberately avoids: the process that
/// renews the lease has to exist, and it has to have written recently enough that what it renews
/// cannot have lapsed. Neither is evidence about the trip; both are evidence the record is alive.
///
/// Deliberately pure, so "a healthy session is still packed" is a test rather than a hope.
fn dead_session(
    session: &SessionState,
    watcher_is_running: bool,
    heartbeat_seconds: u64,
    now: DateTime<Utc>,
) -> Option<DeadSession> {
    let silence = session
        .last_heartbeat_at
        .map(|at| (now - at).num_seconds())
        .unwrap_or(i64::MAX);
    let tolerated =
        i64::try_from(heartbeat_seconds.saturating_mul(MAX_MISSED_HEARTBEATS)).unwrap_or(i64::MAX);
    if silence > tolerated {
        return Some(DeadSession::HeartbeatStopped);
    }
    if !watcher_is_running {
        return Some(DeadSession::WatcherGone);
    }
    None
}

/// Is the watcher this session named still running?
///
/// Matched the way `unpack` matches it before signalling, because a pid on its own is not enough:
/// macOS reuses pids, and some unrelated process inheriting one would look exactly like a live
/// session. A `ps` that will not run answers "yes" — a probe that failed is not evidence of death,
/// and the heartbeat has already had its say about whether anything is alive.
fn watcher_is_running(session: &SessionState) -> bool {
    let Some(pid) = session.daemon_pid else {
        return false;
    };
    let Ok(processes) = processes() else {
        return true;
    };
    processes
        .iter()
        .any(|process| process.pid == pid && is_watcher_for(process, session.id))
}

pub fn unpack(output: &Output, paths: &AppPaths) -> Result<()> {
    with_terminal_operation(paths, || unpack_locked(output, paths))
}

/// What `unpack` did to this Mac, so each outcome gets its own honest sentence.
///
/// "Sleep is normal now" and "rucksack let go of something" are different facts. A Mac that was
/// never packed reaches normal sleep with nothing released, and reporting that as a release would
/// describe a state nobody measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Restore {
    /// The lease the session file named was released.
    ReleasedOurs,
    /// A lease was standing that no session file named, and it was released.
    ReleasedUntracked,
    /// Nothing was holding sleep off: this Mac already slept normally.
    AlreadyNormal,
}

impl Restore {
    /// Did rucksack hand sleep back just now, rather than find it already normal?
    fn released(self) -> bool {
        matches!(self, Self::ReleasedOurs | Self::ReleasedUntracked)
    }
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

    let restored = restore_normal_sleep(&helper, session.as_ref())?;
    if let Some(session) = session.as_ref() {
        if let Some(pid) = session.daemon_pid {
            stop_watcher(pid, session.id);
        }
        report_outcome(session, restored.released(), output);
    } else if restored == Restore::ReleasedUntracked {
        output.step("Released a power lease rucksack was not tracking.");
    }

    SessionState::clear(paths).context("Could not clear the session state.")?;
    if read_sleep_disabled().unwrap_or(0) != 0 {
        output.warn("Sleep is still switched off — something else is holding it.");
    }
    // Before the verdict, which is always the last line.
    //
    // Gated on there having been a session as well as a release: letting go of a lease no session
    // file names is recovery from something that went wrong, not a trip someone just took, and the
    // one mention is only ever spent once.
    if restored.released() && session.is_some() {
        crate::star::mention_once(paths, output);
    }
    output.done(if restored.released() {
        "Unpacked. This Mac sleeps normally."
    } else {
        "Already unpacked. This Mac sleeps normally."
    });
    Ok(())
}

/// Give sleep back, escalating until something works.
///
/// The helper is asked first, and macOS itself is the last word: a Mac that answers
/// `SleepDisabled=0` with nothing released was already sleeping normally, and anything else is
/// somebody else's hold that rucksack must not claim to have undone.
fn restore_normal_sleep(helper: &HelperClient, session: Option<&SessionState>) -> Result<Restore> {
    if let Some(restored) = release_through_helper(helper, session) {
        return Ok(restored);
    }
    match read_sleep_disabled() {
        Ok(0) => Ok(Restore::AlreadyNormal),
        Ok(_) => anyhow::bail!(
            "Something is holding this Mac awake and rucksack could not release it.\nQuit any closed-display utility, then run `rucksack unpack` again."
        ),
        Err(error) => Err(error).context("Could not confirm this Mac can sleep again."),
    }
}

/// Ask the helper to let go, and report only what it actually let go of.
///
/// Ordered so the lease id is used while it is still known: releasing by id, then by owner uid.
/// `recover` answers identically whether it released a lease or found none to release, so whether
/// one was standing is read first — otherwise "there was nothing to do" reads as a release, and
/// `unpack` announces work it never did.
fn release_through_helper(
    helper: &HelperClient,
    session: Option<&SessionState>,
) -> Option<Restore> {
    if let Some(session) = session {
        if let Ok(status) = helper.release(session.lease_id, "user unpacked") {
            if !status.active {
                return Some(Restore::ReleasedOurs);
            }
        }
    }
    let standing = helper
        .status()
        .ok()
        .flatten()
        .is_some_and(|status| status.active);
    match helper.recover() {
        Ok(Some(status)) if standing && !status.active => Some(Restore::ReleasedUntracked),
        _ => None,
    }
}

/// Say what the trip was, then the one thing left to do about it.
fn report_outcome(session: &SessionState, released: bool, output: &Output) {
    // A session still holding a lease that rucksack did not release is an inconsistency, not a trip.
    if !released && session.is_holding_a_lease() {
        return;
    }
    output.step(trip_line(session, Utc::now()));
    if let Some(line) = closing_line(session, read_default_route().ok().as_ref()) {
        output.step(line);
    }
}

/// The trip, in facts.
///
/// Each fact disappears rather than guesses: no battery gauge, a byte counter that reset, or a
/// session written before those fields existed all shorten the line instead of inventing a number.
/// The entire claim rucksack makes about itself is that what it printed is what it saw, and a
/// friendly estimate here would cost that for the sake of a prettier sentence.
fn trip_line(session: &SessionState, now: DateTime<Utc>) -> String {
    // `ended_at` whenever the session ended itself: a Mac that went to sleep at 14:12 and is
    // unpacked at 19:00 was packed for the first stretch, not for the five hours since.
    let until = session.ended_at.unwrap_or(now);
    let minutes = (until - session.started_at).num_minutes().max(0) as u64;
    let mut line = format!("Packed for {}", format_duration(minutes));
    if let (Some(from), Some(to)) = (session.started_battery_percent, session.battery_percent) {
        line.push_str(&format!(" · battery {from}% → {to}%"));
    }
    if let Some(bytes) = session.bytes_moved {
        line.push_str(&format!(" · {}", format_bytes(bytes)));
    }
    line
}

/// The single most useful thing left to say, or nothing.
///
/// One line, never two: `unpack` gets three in total and the verdict owns the last. A release reason
/// wins over the network hint, because "your battery ran out" is both more important and the reason
/// the user is reading this at all.
fn closing_line(session: &SessionState, route: Option<&RouteStatus>) -> Option<String> {
    if !session.is_holding_a_lease() {
        if let Some(reason) = &session.release_reason {
            let when = session
                .ended_at
                .map(format_time)
                .unwrap_or_else(|| "earlier".to_owned());
            return Some(format!("This Mac went back to sleep at {when} — {reason}."));
        }
    }
    // Nobody notices they are still on cellular until the bill does.
    route
        .filter(|route| is_personal_hotspot(route))
        .map(|_| "Still on your phone. Pick a Wi-Fi network when you can.".to_owned())
}

/// Bytes as a person would say them.
///
/// Truncated rather than rounded, so the figure is never larger than what actually moved, and in
/// decimal units because that is what a phone bill counts in.
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1_000;
    const MB: u64 = 1_000_000;
    const GB: u64 = 1_000_000_000;
    match bytes {
        bytes if bytes < KB => format!("{bytes} B"),
        bytes if bytes < MB => format!("{} KB", bytes / KB),
        bytes if bytes < GB => format!("{} MB", bytes / MB),
        bytes => format!("{}.{} GB", bytes / GB, (bytes % GB) / (GB / 10)),
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

/// Say which limit is being reported, and only claim precision for the one that has it.
///
/// The lease clock is arithmetic on a known deadline. The battery is projected from drain that is
/// still being measured and will move with the workload, so it is marked as an estimate rather than
/// dressed up as the same kind of number.
fn format_limit(limit: Limit) -> String {
    match limit {
        Limit::Lease(minutes) => format!("{} left", format_duration(minutes)),
        Limit::Battery(minutes) => format!("~{} of charge left", format_duration(minutes)),
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
    use rucksack_core::protocol::{HelperOperation, HelperRequest, HelperResponse, HelperStatus};
    use rucksack_core::state::SessionPhase;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    fn route(interface: &str, gateway: &str) -> RouteStatus {
        RouteStatus {
            interface: Some(interface.to_owned()),
            gateway: Some(gateway.to_owned()),
        }
    }

    fn codex_at(path: &str) -> Option<std::path::PathBuf> {
        Some(std::path::PathBuf::from(path))
    }

    /// The regression that lost a lease: an agent this Mac does not have must not end a pack.
    ///
    /// Most Macs running rucksack have one of the three agents, so a missing Codex is the ordinary
    /// case rather than the exception. Any answer other than "carry on" here is a Mac that went to
    /// sleep in a bag because of a CLI that had nothing to do with keeping it awake.
    #[test]
    fn a_missing_codex_never_ends_a_pack() {
        let all_on = AdaptersConfig::default();

        assert!(matches!(
            plan_remote_control(false, &all_on, None),
            RemoteControl::Unavailable(_)
        ));
    }

    /// An adapter switched off is obeyed, and saying so is not a warning.
    #[test]
    fn an_adapter_switched_off_is_skipped_quietly() {
        let codex_off = AdaptersConfig {
            codex: false,
            ..AdaptersConfig::default()
        };

        // Not even asked for, so a Codex that is sitting right there is left alone.
        assert!(matches!(
            plan_remote_control(false, &codex_off, codex_at("/usr/local/bin/codex")),
            RemoteControl::Off(_)
        ));
    }

    /// `--require-remote` is the one way to ask for the opposite, so it still refuses.
    #[test]
    fn require_remote_refuses_what_it_cannot_start() {
        let all_on = AdaptersConfig::default();
        let codex_off = AdaptersConfig {
            codex: false,
            ..AdaptersConfig::default()
        };

        assert!(matches!(
            plan_remote_control(true, &all_on, None),
            RemoteControl::Refuse(_)
        ));
        assert!(matches!(
            plan_remote_control(true, &codex_off, codex_at("/usr/local/bin/codex")),
            RemoteControl::Refuse(_)
        ));
    }

    #[test]
    fn a_codex_that_is_there_and_wanted_is_started() {
        assert_eq!(
            plan_remote_control(
                false,
                &AdaptersConfig::default(),
                codex_at("/usr/local/bin/codex")
            ),
            RemoteControl::Start(std::path::PathBuf::from("/usr/local/bin/codex"))
        );
    }

    /// The network the user is walking away from also reaches the internet.
    ///
    /// Accepting it would pack a Mac that goes offline at the front door, which is the whole
    /// failure rucksack exists to prevent.
    #[test]
    fn a_working_office_network_is_not_the_commute_network() {
        let office = route("en0", "192.168.1.1");
        let target = CommuteTarget {
            expected: Some("Noah".to_owned()),
            baseline: Some(office.clone()),
        };

        assert!(!has_switched(&target, &office, None));
        assert!(!has_switched(&target, &office, Some("Office")));

        // Only a name, a moved route, or an iPhone gateway counts.
        assert!(has_switched(&target, &office, Some("Noah")));
        assert!(has_switched(&target, &route("en7", "192.168.1.1"), None));
        assert!(has_switched(&target, &route("en0", "172.20.10.1"), None));
    }

    /// With no saved hotspot there is no name to compare, so the route itself must move.
    #[test]
    fn without_a_saved_hotspot_the_route_still_has_to_change() {
        let office = route("en0", "192.168.1.1");
        let target = CommuteTarget {
            expected: None,
            baseline: Some(office.clone()),
        };

        assert!(!has_switched(&target, &office, None));
        assert!(!has_switched(&target, &office, Some("Office")));
        assert!(has_switched(&target, &route("en0", "172.20.10.1"), None));
        assert!(has_switched(&target, &route("en0", "192.0.0.1"), None));
        assert!(has_switched(&target, &route("en7", "192.168.1.1"), None));
    }

    /// The one signal that survives macOS hiding the network name.
    ///
    /// Both gateways are real: 172.20.10.1 on a dual-stack carrier, and 192.0.0.1 when the carrier
    /// is IPv6-only and iOS hands out a 464XLAT service-continuity address instead.
    #[test]
    fn an_iphone_hotspot_gateway_is_proof_on_its_own() {
        assert!(is_personal_hotspot(&route("en0", "172.20.10.1")));
        assert!(is_personal_hotspot(&route("en0", "192.0.0.1")));
        assert!(!is_personal_hotspot(&route("en0", "192.168.1.1")));
        assert!(!is_personal_hotspot(&route("en0", "192.0.0.2")));
    }

    /// A session that started `minutes` before `now`, anchored so the duration cannot drift.
    fn trip(now: DateTime<Utc>, minutes: i64) -> SessionState {
        let started_at = now - ChronoDuration::minutes(minutes);
        let mut session = SessionState::new(
            Uuid::new_v4(),
            started_at,
            started_at + ChronoDuration::hours(24),
        );
        session.phase = SessionPhase::Active;
        session
    }

    /// Facts only. Anything rucksack could not measure leaves the line shorter.
    #[test]
    fn a_trip_report_omits_what_it_could_not_measure() {
        let now = Utc::now();

        let mut full = trip(now, 192);
        full.started_battery_percent = Some(79);
        full.battery_percent = Some(61);
        full.bytes_moved = Some(240_331_847);
        assert_eq!(
            trip_line(&full, now),
            "Packed for 3h 12m · battery 79% → 61% · 240 MB"
        );

        let mut no_bytes = full.clone();
        no_bytes.bytes_moved = None;
        assert_eq!(
            trip_line(&no_bytes, now),
            "Packed for 3h 12m · battery 79% → 61%"
        );

        let mut no_battery = full.clone();
        no_battery.started_battery_percent = None;
        assert_eq!(trip_line(&no_battery, now), "Packed for 3h 12m · 240 MB");

        // A session written before these fields existed measures neither.
        let bare = trip(now, 192);
        assert_eq!(trip_line(&bare, now), "Packed for 3h 12m");
    }

    /// A Mac that slept at 14:12 and is unpacked at 19:00 was packed for the first stretch.
    #[test]
    fn a_finished_trip_is_measured_to_when_it_ended() {
        let now = Utc::now();
        let mut session = trip(now, 300);
        session.phase = SessionPhase::Released;
        session.ended_at = Some(session.started_at + ChronoDuration::hours(1));
        session.release_reason = Some("the battery reached the 15% floor".to_owned());

        assert_eq!(trip_line(&session, now), "Packed for 1 hour");
    }

    /// Three lines total, so the closing slot holds one fact and the release reason outranks it.
    #[test]
    fn a_closing_line_is_never_two_facts() {
        let now = Utc::now();
        let hotspot = route("en0", "192.0.0.1");

        let live = trip(now, 30);
        assert_eq!(
            closing_line(&live, Some(&hotspot)).as_deref(),
            Some("Still on your phone. Pick a Wi-Fi network when you can.")
        );
        assert_eq!(
            closing_line(&live, Some(&route("en0", "192.168.1.1"))),
            None
        );
        assert_eq!(closing_line(&live, None), None);

        let mut ended = trip(now, 30);
        ended.phase = SessionPhase::Released;
        ended.release_reason = Some("this Mac got too hot".to_owned());
        ended.ended_at = Some(now);
        let closing = closing_line(&ended, Some(&hotspot)).expect("a reason to report");
        assert!(closing.contains("too hot"));
        assert!(!closing.contains("phone"), "one fact, not two");
    }

    /// Truncated, never rounded: the figure must never exceed what actually moved.
    #[test]
    fn formats_bytes_a_person_would_say() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1_000), "1 KB");
        assert_eq!(format_bytes(999_999), "999 KB");
        assert_eq!(format_bytes(1_000_000), "1 MB");
        assert_eq!(format_bytes(240_331_847), "240 MB");
        assert_eq!(format_bytes(999_999_999), "999 MB");
        assert_eq!(format_bytes(1_000_000_000), "1.0 GB");
        assert_eq!(format_bytes(1_299_999_999), "1.2 GB");
    }

    /// The clause that stops someone closing the lid on an unheld lease.
    #[test]
    fn the_waiting_instruction_always_carries_the_lid() {
        let named = wait_instruction(Some("Noah"));
        assert!(named.contains("“Noah”"));
        assert!(named.contains("lid open"));

        let unnamed = wait_instruction(None);
        assert!(unnamed.contains("lid open"));
    }

    #[test]
    fn formats_durations_a_person_would_say() {
        assert_eq!(format_duration(45), "45m");
        assert_eq!(format_duration(60), "1 hour");
        assert_eq!(format_duration(24 * 60), "24 hours");
        assert_eq!(format_duration(90), "1h 30m");
    }

    /// Stand up a helper that answers every request the call under test makes.
    fn fake_helper(
        reply: impl Fn(&HelperRequest) -> HelperResponse + Send + 'static,
    ) -> (tempfile::TempDir, HelperClient) {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("helper.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_err() {
                    return;
                }
                let request: HelperRequest = serde_json::from_str(&line).unwrap();
                let mut encoded = serde_json::to_vec(&reply(&request)).unwrap();
                encoded.push(b'\n');
                let _ = stream.write_all(&encoded);
                let _ = stream.flush();
            }
        });
        let helper = HelperClient::new(&socket);
        (directory, helper)
    }

    fn helper_status(active: bool) -> HelperStatus {
        HelperStatus {
            active,
            lease_id: None,
            owner_uid: None,
            created_at: None,
            expires_at: None,
            hard_expires_at: None,
            previous_sleep_disabled: None,
            sleep_disabled: Some(u8::from(active)),
            reason: None,
            last_reasserted_at: None,
        }
    }

    /// A helper with nothing to release is not a release.
    ///
    /// `recover` reports "no lease is active" both when it has just released one and when there
    /// was never one to release. Reading that as a release is what made a second `unpack` in a
    /// row claim it had let go of something.
    #[test]
    fn a_helper_holding_nothing_releases_nothing() {
        let (_directory, helper) = fake_helper(|request| match request.operation {
            // With no lease to own, the helper refuses a release by id, as `require_owner` does.
            HelperOperation::Release { .. } => {
                HelperResponse::failure(request.request_id, "no active lease", None)
            }
            _ => HelperResponse::success(request.request_id, Some(helper_status(false))),
        });

        assert_eq!(release_through_helper(&helper, None), None);
        assert_eq!(
            release_through_helper(&helper, Some(&trip(Utc::now(), 30))),
            None
        );
    }

    /// A lease standing with no session file to name it is the case `recover` exists for.
    #[test]
    fn a_standing_lease_no_session_names_is_released() {
        let (_directory, helper) = fake_helper(|request| match request.operation {
            // The status asked before recovering is the only proof a lease was ever standing.
            HelperOperation::Status => {
                HelperResponse::success(request.request_id, Some(helper_status(true)))
            }
            _ => HelperResponse::success(request.request_id, Some(helper_status(false))),
        });

        assert_eq!(
            release_through_helper(&helper, None),
            Some(Restore::ReleasedUntracked)
        );
    }

    /// Releasing by lease id is rucksack letting go of its own session, and needs no escalation.
    #[test]
    fn releasing_by_lease_id_needs_no_recovery() {
        let (_directory, helper) = fake_helper(|request| match request.operation {
            HelperOperation::Release { .. } => {
                HelperResponse::success(request.request_id, Some(helper_status(false)))
            }
            _ => HelperResponse::failure(request.request_id, "should not be asked", None),
        });

        assert_eq!(
            release_through_helper(&helper, Some(&trip(Utc::now(), 30))),
            Some(Restore::ReleasedOurs)
        );
    }

    /// No helper to answer means nothing was released, and macOS gets the last word.
    #[test]
    fn an_unreachable_helper_releases_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let helper = HelperClient::new(directory.path().join("absent.sock"));

        assert_eq!(release_through_helper(&helper, None), None);
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

    fn watched(now: DateTime<Utc>) -> SessionState {
        let mut session = SessionState::new(Uuid::new_v4(), now, now + ChronoDuration::hours(23));
        session.phase = SessionPhase::Active;
        session.daemon_pid = Some(42);
        session.last_heartbeat_at = Some(now);
        session
    }

    /// A watcher that is beating and running is packed, and a late beat is not a dead one.
    #[test]
    fn a_live_session_is_still_packed() {
        let now = Utc::now();
        let session = watched(now);

        assert_eq!(dead_session(&session, true, 30, now), None);
        assert_eq!(
            dead_session(&session, true, 30, now + ChronoDuration::seconds(75)),
            None
        );
    }

    /// The reported failure: a record still saying "active" after the watcher behind it died.
    ///
    /// It said `Packed · en0 · battery 100% · 23h 53m left` for nine minutes with no lease held and
    /// no process to renew one. Every minute of that was an invitation to close the lid.
    #[test]
    fn a_watcher_that_stopped_beating_is_not_packed() {
        let now = Utc::now();
        let session = watched(now);

        assert_eq!(
            dead_session(&session, true, 30, now + ChronoDuration::minutes(9)),
            Some(DeadSession::HeartbeatStopped)
        );
    }

    /// A fresh beat is not enough: the process that wrote it must still be there to write the next.
    #[test]
    fn a_watcher_that_is_gone_is_not_packed() {
        let now = Utc::now();
        let mut session = watched(now);

        assert_eq!(
            dead_session(&session, false, 30, now),
            Some(DeadSession::WatcherGone)
        );

        // A session that never recorded a pid never had a watcher to lose.
        session.daemon_pid = None;
        assert!(!watcher_is_running(&session));
    }
}
