use crate::cli::{AdapterCommand, Cli, Command, HelperCommand, PackArgs, SetupArgs};
use crate::doctor::{self, CheckLevel};
use crate::flow;
use crate::helper_client::HelperClient;
use crate::install;
use crate::output::{JsonFailureEmitted, Output};
use anyhow::{anyhow, Result};
use rucksack_core::agent::{
    codex_pair, detect_all, install_adapters, remove_adapters, verify_adapters, AdapterFileStatus,
    AdapterVerificationReport, AgentKind,
};
use rucksack_core::network::read_wifi_status;
use rucksack_core::{AppPaths, Config, SessionState};

pub fn run(cli: Cli) -> Result<()> {
    let output = Output::new(cli.verbose, cli.json);
    let paths = AppPaths::discover()?;

    let command = match cli.command {
        None => {
            let config = Config::load(&paths)?;
            flow::pack(&default_pack_args(), &output, &paths, &config)?;
            "pack"
        }
        Some(Command::Setup(args)) => {
            let config = Config::load(&paths)?;
            setup(&args, &output, &paths, config)?;
            "setup"
        }
        Some(Command::Doctor(args)) => {
            let config = Config::load(&paths)?;
            run_doctor(&args, &output, &paths, &config)?;
            "doctor"
        }
        Some(Command::Pack(args)) => {
            let config = Config::load(&paths)?;
            flow::pack(&args, &output, &paths, &config)?;
            "pack"
        }
        Some(Command::Status(args)) => {
            flow::status(&args, &output, &paths)?;
            "status"
        }
        Some(Command::Unpack(args)) => {
            let config = Config::load(&paths)?;
            flow::unpack(&args, &output, &paths, &config)?;
            "unpack"
        }
        Some(Command::Report) => {
            crate::report::run(&output, &paths)?;
            "report"
        }
        Some(Command::Recover(args)) => {
            flow::recover(&args, &output, &paths)?;
            "recover"
        }
        Some(Command::Adapters { command }) => {
            adapters(command, &output, &paths)?;
            "adapters"
        }
        Some(Command::Pair(args)) => {
            pair(args.agent, &output)?;
            "pair"
        }
        Some(Command::Helper { command }) => {
            helper(command, &output)?;
            "helper"
        }
        Some(Command::Hook(args)) => {
            crate::hooks::run(args.agent, &paths)?;
            return Ok(());
        }
        Some(Command::Daemon(args)) => {
            let config = Config::load(&paths)?;
            crate::daemon::run(args.session_id, &paths, &config)?;
            return Ok(());
        }
    };
    output.finish_json(command)
}

fn run_doctor(
    args: &crate::cli::DoctorArgs,
    output: &Output,
    paths: &AppPaths,
    config: &Config,
) -> Result<()> {
    let project = std::env::current_dir()?;
    let binary = std::env::current_exe()?;
    let report = doctor::run(paths, config, &project, &binary, args.agent);
    if output.json() {
        if report.ready {
            return output.emit_json(&report);
        }
        output.emit_json_failure(&report, "readiness checks failed")?;
        return Err(JsonFailureEmitted.into());
    }

    output.title("Readiness");
    for check in &report.checks {
        match check.level {
            CheckLevel::Pass => output.pass(format!("{} · {}", check.name, check.summary)),
            CheckLevel::Warning => output.warn(format!("{} · {}", check.name, check.summary)),
            CheckLevel::Fail => output.fail(format!("{} · {}", check.name, check.summary)),
        }
        if let Some(detail) = &check.detail {
            output.detail(detail);
        }
    }
    output.blank();
    if report.ready {
        output.plain("Preflight checks passed.");
        Ok(())
    } else {
        output.plain("Preflight checks failed. Fix them before packing.");
        Err(anyhow!("readiness checks failed"))
    }
}

fn setup(args: &SetupArgs, output: &Output, paths: &AppPaths, mut config: Config) -> Result<()> {
    apply_explicit_setup_network(args, &mut config)?;
    validate_setup_config(&config)?;

    if args.yes
        && args.hotspot.is_none()
        && !args.usb
        && config.hotspot.ssid.is_none()
        && config.hotspot.require_verified_ssid
    {
        anyhow::bail!("Non-interactive setup requires `--hotspot \"My iPhone\"` or `--usb`.");
    }

    output.title("Three things make the handoff reliable");

    output.section("1. Hotspot");
    configure_setup_network(args, output, &mut config)?;
    validate_setup_config(&config)?;

    output.blank();
    output.section("2. Power helper");
    if args.no_helper {
        output.warn("Skipped by request");
    } else if install::installed_helper_exists() && HelperClient::default().is_available() {
        output.pass("Installed and reachable");
    } else {
        let install_now = args.yes
            || output.confirm(
                "Install the time-limited closed-lid helper? macOS will authenticate once.",
                true,
            )?;
        if install_now {
            install::install_helper()?;
            output.pass("Power helper installed");
        } else {
            output.warn("Helper not installed; closed-lid mode will refuse to start");
        }
    }

    output.blank();
    output.section("3. Coding agents");
    if args.no_adapters {
        output.warn("Skipped by request");
    } else {
        let detections = detect_all(None);
        let agents = detections
            .iter()
            .filter(|item| item.installed)
            .map(|item| item.kind)
            .collect::<Vec<_>>();
        if agents.is_empty() {
            output.warn("No supported agent installation was detected");
        } else {
            for detection in &detections {
                if detection.installed {
                    output.pass(format!("{} found", detection.kind.display_name()));
                }
            }
            let install_now = args.yes
                || output.confirm("Install reversible native Commute Mode adapters?", true)?;
            if install_now {
                let binary = std::env::current_exe()?;
                let report = install_adapters(paths, &binary, &agents)?;
                require_current_adapters(&report.verification)?;
                let file_count = report
                    .verification
                    .agents
                    .iter()
                    .map(|evidence| evidence.files.len())
                    .sum::<usize>();
                output.pass(format!("{} adapter files ready", file_count));
                if agents.contains(&AgentKind::Codex) {
                    output.plain(
                        "Codex: open `/hooks`, review the Rucksack entries, and trust them once.",
                    );
                }
            }
        }
    }

    config.save(paths)?;
    output.blank();
    output.pass(format!(
        "Configuration saved to {}",
        paths.config_file.display()
    ));
    output.plain("Setup complete. Run `rucksack pack` when you walk out.");
    Ok(())
}

fn apply_explicit_setup_network(args: &SetupArgs, config: &mut Config) -> Result<()> {
    if let Some(ssid) = &args.hotspot {
        config.hotspot.ssid = Some(validate_network_name(ssid, "--hotspot")?);
        config.hotspot.require_verified_ssid = true;
        config.hotspot.require_iphone_usb = false;
    } else if args.usb {
        config.hotspot.ssid = None;
        config.hotspot.require_verified_ssid = false;
        config.hotspot.require_iphone_usb = true;
    }
    Ok(())
}

fn configure_setup_network(args: &SetupArgs, output: &Output, config: &mut Config) -> Result<()> {
    if args.hotspot.is_some() {
        let ssid = config
            .hotspot
            .ssid
            .as_deref()
            .ok_or_else(|| anyhow!("Validated hotspot name was not retained"))?;
        output.pass(format!("Saved “{ssid}”"));
    } else if args.usb {
        output.pass("Saved iPhone USB tethering");
    } else if config.hotspot.require_iphone_usb {
        output.pass("Using iPhone USB tethering");
    } else if let Some(ssid) = config.hotspot.ssid.as_deref() {
        output.pass(format!("Using saved hotspot “{ssid}”"));
    } else if !config.hotspot.require_verified_ssid {
        output.pass("Explicit default-route mode enabled · Wi-Fi or USB tether supported");
    } else {
        match read_wifi_status() {
            Ok(wifi) if wifi.ssid.is_some() => {
                let ssid = validate_network_name(
                    wifi.ssid.as_deref().unwrap_or_default(),
                    "Current Wi-Fi network",
                )?;
                let save = output.confirm(
                    &format!("Is “{ssid}” the phone hotspot you intend to use for Commute Mode?"),
                    false,
                )?;
                if save {
                    config.hotspot.ssid = Some(ssid.clone());
                    config.hotspot.require_verified_ssid = true;
                    config.hotspot.require_iphone_usb = false;
                    output.pass(format!("Saved “{ssid}”"));
                } else {
                    anyhow::bail!(
                        "No commute network was selected. Re-run with `--hotspot \"My iPhone\"` or `--usb`."
                    );
                }
            }
            Ok(wifi) if wifi.redacted => {
                output.warn(
                    "Wi-Fi is connected, but macOS privacy-redacted its name. Enter the phone hotspot name explicitly.",
                );
                save_prompted_hotspot(output, config)?;
            }
            Ok(_) => {
                output.warn("No Wi-Fi network is currently selected.");
                save_prompted_hotspot(output, config)?;
            }
            Err(error) => {
                output.warn(format!("Could not inspect Wi-Fi: {error}"));
                save_prompted_hotspot(output, config)?;
            }
        }
    }
    Ok(())
}

fn validate_setup_config(config: &Config) -> Result<()> {
    config
        .validate()
        .map_err(|error| anyhow!("Configuration is not safe: {error}"))
}

fn save_prompted_hotspot(output: &Output, config: &mut Config) -> Result<()> {
    let input = output.input("Phone hotspot name (or Ctrl-C to cancel): ")?;
    let ssid = validate_network_name(&input, "Hotspot name")?;
    config.hotspot.ssid = Some(ssid.clone());
    config.hotspot.require_verified_ssid = true;
    config.hotspot.require_iphone_usb = false;
    output.pass(format!("Saved “{ssid}”"));
    Ok(())
}

fn validate_network_name(value: &str, label: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!(
            "{label} cannot be empty. Re-run setup with `--hotspot \"My iPhone\"` or `--usb`."
        );
    }
    if value.chars().any(char::is_control) {
        anyhow::bail!("{label} cannot contain terminal control characters");
    }
    Ok(value.to_owned())
}

fn adapters(command: AdapterCommand, output: &Output, paths: &AppPaths) -> Result<()> {
    match command {
        AdapterCommand::Install(args) => {
            let agents = args
                .agent
                .map(|agent| vec![agent])
                .unwrap_or_else(|| AgentKind::ALL.to_vec());
            let binary = std::env::current_exe()?;
            let report = install_adapters(paths, &binary, &agents)?;
            require_current_adapters(&report.verification)?;
            output.title("Agent adapters");
            for path in report.changed {
                output.pass(format!("Updated {}", path.display()));
            }
            for path in report.unchanged {
                output.detail(format!("Already installed {}", path.display()));
            }
            if agents.contains(&AgentKind::Codex) {
                output.plain(
                    "Codex: open `/hooks`, review the Rucksack entries, and trust them once.",
                );
            }
            Ok(())
        }
        AdapterCommand::Status => {
            let binary = std::env::current_exe()?;
            let verification = verify_adapters(paths, &binary, &AgentKind::ALL);
            if output.json() {
                return output.emit_json(&verification);
            }
            output.title("Agent adapters");
            for evidence in &verification.agents {
                if evidence.current {
                    output.pass(format!(
                        "{} adapter is current",
                        evidence.agent.display_name()
                    ));
                } else {
                    output.warn(format!(
                        "{} adapter is not ready",
                        evidence.agent.display_name()
                    ));
                }
                for file in &evidence.files {
                    if file.is_current() {
                        output.detail(format!(
                            "{} · current · {}",
                            evidence.agent.display_name(),
                            file.path.display()
                        ));
                    } else {
                        output.warn(format!(
                            "{} · {} · {}",
                            evidence.agent.display_name(),
                            adapter_status_name(file.status),
                            file.path.display()
                        ));
                        output.detail(&file.detail);
                    }
                }
            }
            Ok(())
        }
        AdapterCommand::Remove(args) => {
            if SessionState::load(paths)?.is_some() {
                anyhow::bail!(
                    "A Commute Mode session is active. Run `rucksack unpack` before removing adapters."
                );
            }
            let agents = args
                .agent
                .map(|agent| vec![agent])
                .unwrap_or_else(|| AgentKind::ALL.to_vec());
            let changed = remove_adapters(paths, &agents)?;
            output.title("Agent adapters");
            if changed.is_empty() {
                output.pass("No managed entries needed removal");
            } else {
                for path in changed {
                    output.pass(format!("Removed Rucksack entries from {}", path.display()));
                }
            }
            Ok(())
        }
    }
}

fn require_current_adapters(verification: &AdapterVerificationReport) -> Result<()> {
    if verification.current {
        return Ok(());
    }
    let failures = verification
        .agents
        .iter()
        .flat_map(|evidence| {
            evidence
                .files
                .iter()
                .filter(|file| !file.is_current())
                .map(|file| {
                    format!(
                        "{} {} at {}: {}",
                        evidence.agent.display_name(),
                        adapter_status_name(file.status),
                        file.path.display(),
                        file.detail
                    )
                })
        })
        .collect::<Vec<_>>()
        .join("; ");
    anyhow::bail!("Adapter verification failed after installation: {failures}")
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

fn pair(agent: AgentKind, output: &Output) -> Result<()> {
    match agent {
        AgentKind::Codex => {
            let result = codex_pair()?;
            if !result.success() {
                anyhow::bail!("Codex pairing failed: {}", result.combined_trimmed());
            }
            let value = serde_json::from_str::<serde_json::Value>(&result.stdout)
                .unwrap_or_else(|_| serde_json::json!({ "raw": result.stdout.trim() }));
            if output.json() {
                output.emit_json(&value)?;
                return Ok(());
            }
            output.title("Codex Remote Control");
            if let Some(code) = value
                .get("pairingCode")
                .or_else(|| value.get("manualPairingCode"))
                .and_then(serde_json::Value::as_str)
            {
                output.plain("Pairing code");
                output.plain(format!("  {code}"));
            } else {
                output.plain(result.stdout.trim());
            }
            if let Some(expires) = value.get("expiresAt").and_then(serde_json::Value::as_str) {
                output.plain(format!("Expires at {expires}"));
            }
            output.plain("Open ChatGPT on your phone and enter the code.");
            Ok(())
        }
        AgentKind::Claude => {
            output.title("Claude Code Remote Control");
            output.plain("In the active Claude Code session, run `/remote-control`.");
            output.plain("Use `/commute-mode` once if the session predates Rucksack activation.");
            Ok(())
        }
        AgentKind::Cursor => {
            output.title("Cursor Remote Control");
            output.plain("In Cursor, open Agents → Remote Control and pair Cursor on your phone.");
            output.plain("Cursor does not currently expose a stable CLI pairing API.");
            Ok(())
        }
    }
}

fn helper(command: HelperCommand, output: &Output) -> Result<()> {
    match command {
        HelperCommand::Install => {
            output.title("Power helper");
            install::install_helper()?;
            output.pass("Installed and reachable");
            Ok(())
        }
        HelperCommand::Status => {
            let status = HelperClient::default().status()?;
            if output.json() {
                output.emit_json(&status)?;
            } else {
                output.title("Power helper");
                match status {
                    Some(status) if status.active => {
                        output.pass("Active lease");
                        output.detail(format!("{status:?}"));
                    }
                    Some(status) => {
                        output.pass("Installed · no active lease");
                        output.detail(format!("{status:?}"));
                    }
                    None => output.pass("Installed · no active lease"),
                }
                let (helper, plist) = install::helper_paths();
                output.detail(format!("{helper}\n{plist}"));
            }
            Ok(())
        }
        HelperCommand::Uninstall => {
            output.title("Power helper");
            install::uninstall_helper()?;
            output.pass("Normal sleep restored");
            output.pass("Helper removed");
            Ok(())
        }
    }
}

fn default_pack_args() -> PackArgs {
    PackArgs {
        agent: None,
        hotspot: None,
        usb: false,
        duration_minutes: None,
        focus: None,
        yes: false,
        allow_unverified_ssid: false,
        allow_unverified_remote: false,
    }
}
