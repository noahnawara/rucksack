use crate::cli::{AdapterCommand, Cli, Command, HelperCommand, PackArgs, SetupArgs};
use crate::doctor::{self, CheckLevel};
use crate::flow;
use crate::helper_client::HelperClient;
use crate::install;
use crate::onboarding::{assess, bases, ProviderOnboardingBases};
use crate::output::{JsonFailureEmitted, Output};
use anyhow::{anyhow, Result};
use chrono::Utc;
use rucksack_core::agent::{
    codex_pair, detect, detect_all, install_adapters, remove_adapters, verify_adapter,
    verify_adapters, AdapterFileStatus, AdapterInstallReport, AdapterVerificationReport, AgentKind,
};
use rucksack_core::network::read_wifi_status;
use rucksack_core::onboarding::{
    EvidenceInvalidationReason, EvidenceKind, EvidenceSource, EvidenceStatus,
    RemoteOnboardingRegistry,
};
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
        Some(Command::Unpack) => {
            let config = Config::load(&paths)?;
            flow::unpack(&output, &paths, &config)?;
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
            pair(args.agent, &output, &paths)?;
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
    require_inactive_session_for_onboarding(paths)?;
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

    output.title("Four things make the handoff reliable");

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
                invalidate_changed_adapters(paths, &report)?;
                let file_count = report
                    .verification
                    .agents
                    .iter()
                    .map(|evidence| evidence.files.len())
                    .sum::<usize>();
                output.pass(format!("{} adapter files ready", file_count));
            }
        }
    }

    output.blank();
    output.section("4. Remote Control");
    let onboarding_complete = configure_remote_onboarding(args, output, paths)?;

    config.save(paths)?;
    output.blank();
    output.pass(format!(
        "Configuration saved to {}",
        paths.config_file.display()
    ));
    if onboarding_complete {
        output.plain("Setup complete. Run `rucksack pack` when you walk out.");
    } else {
        output.warn(
            "Core setup is saved, but Remote Control onboarding is incomplete. Re-run `rucksack setup` interactively before packing.",
        );
    }
    Ok(())
}

fn require_inactive_session_for_onboarding(paths: &AppPaths) -> Result<()> {
    if SessionState::load(paths)?.is_some_and(|session| {
        !matches!(
            session.phase,
            rucksack_core::state::SessionPhase::Released
                | rucksack_core::state::SessionPhase::Failed
        )
    }) {
        anyhow::bail!(
            "A Commute Mode session is active. Run `rucksack unpack` before changing setup."
        );
    }
    Ok(())
}

fn configure_remote_onboarding(
    args: &SetupArgs,
    output: &Output,
    paths: &AppPaths,
) -> Result<bool> {
    let binary = std::env::current_exe()?;
    let detections = detect_all(None);
    let installed = detections
        .iter()
        .filter(|detection| detection.installed)
        .collect::<Vec<_>>();
    if installed.is_empty() {
        output.warn("No supported agent is installed, so Remote Control onboarding is incomplete");
        return Ok(false);
    }

    let mut all_current = true;
    for detection in installed {
        let adapter = verify_adapter(paths, &binary, detection.kind);
        if !adapter.current {
            output.warn(format!(
                "{} adapter is not current; finish adapter setup before Remote Control onboarding",
                detection.kind.display_name()
            ));
            all_current = false;
            continue;
        }
        let provider_bases = bases(detection, &adapter)?;
        let mut registry = refresh_installation_evidence(paths, detection.kind, &provider_bases)?;

        let state = assess(&registry, detection.kind, &provider_bases);
        if state.is_current() {
            output.pass(format!(
                "{} Remote Control onboarding is current",
                detection.kind.display_name()
            ));
            continue;
        }

        let confirmations = [
            EvidenceKind::Pairing,
            EvidenceKind::NativeTrust,
            EvidenceKind::PhoneVisibility,
        ]
        .into_iter()
        .filter(|kind| state.needs_user_confirmation(*kind))
        .collect::<Vec<_>>();
        show_remote_onboarding_instructions(detection.kind, &confirmations, output);
        if args.yes || output.json() {
            output.warn(format!(
                "{} {} require interactive confirmation; non-interactive setup did not record them",
                detection.kind.display_name(),
                confirmation_names(&confirmations)
            ));
            all_current = false;
            continue;
        }
        if !output.confirm(
            &format!(
                "Confirm that {} {} {} complete",
                detection.kind.display_name(),
                confirmation_names(&confirmations),
                if confirmations.len() == 1 {
                    "is"
                } else {
                    "are"
                }
            ),
            false,
        )? {
            output.warn(format!(
                "{} Remote Control onboarding remains incomplete",
                detection.kind.display_name()
            ));
            all_current = false;
            continue;
        }

        let recorded_at = Utc::now();
        for kind in confirmations {
            registry = RemoteOnboardingRegistry::record_evidence(
                paths,
                detection.kind,
                kind,
                provider_bases.basis(kind),
                EvidenceSource::ConfirmedByUser,
                recorded_at,
            )?;
        }
        let completed = assess(&registry, detection.kind, &provider_bases);
        if completed.is_current() {
            output.pass(format!(
                "{} pairing, native trust, and baseline phone visibility were confirmed by you",
                detection.kind.display_name()
            ));
        } else {
            all_current = false;
        }
    }
    Ok(all_current)
}

fn refresh_installation_evidence(
    paths: &AppPaths,
    agent: AgentKind,
    bases: &ProviderOnboardingBases,
) -> Result<RemoteOnboardingRegistry> {
    let mut registry = RemoteOnboardingRegistry::load(paths)?;
    let installation_status =
        registry.evidence_status(agent, EvidenceKind::Installation, &bases.installation);
    if matches!(
        &installation_status,
        EvidenceStatus::BasisChanged | EvidenceStatus::Invalidated { .. }
    ) {
        let invalidated_at = Utc::now();
        for kind in [
            EvidenceKind::Pairing,
            EvidenceKind::NativeTrust,
            EvidenceKind::PhoneVisibility,
        ] {
            registry = RemoteOnboardingRegistry::invalidate_evidence(
                paths,
                agent,
                kind,
                EvidenceInvalidationReason::ProviderInstallationChanged,
                invalidated_at,
            )?;
        }
    }
    if !matches!(&installation_status, EvidenceStatus::Current) {
        registry = RemoteOnboardingRegistry::record_evidence(
            paths,
            agent,
            EvidenceKind::Installation,
            bases.installation.clone(),
            EvidenceSource::Measured,
            Utc::now(),
        )?;
    }
    Ok(registry)
}

fn show_remote_onboarding_instructions(
    agent: AgentKind,
    confirmations: &[EvidenceKind],
    output: &Output,
) {
    if confirmations.contains(&EvidenceKind::NativeTrust) {
        match agent {
            AgentKind::Codex => output
                .plain("Codex: open `/hooks`, review the marked Rucksack entries, and trust them."),
            AgentKind::Claude => output.plain(
                "Claude Code: confirm that the active workspace is trusted and native hooks are allowed.",
            ),
            AgentKind::Cursor => output.plain(
                "Cursor: confirm that the workspace is trusted and native hooks are enabled.",
            ),
        }
    }
    if !confirmations
        .iter()
        .any(|kind| matches!(kind, EvidenceKind::Pairing | EvidenceKind::PhoneVisibility))
    {
        return;
    }
    match agent {
        AgentKind::Codex => {
            output.plain(
                "Codex: if needed, run `rucksack pair codex` and finish pairing in ChatGPT.",
            );
            output.plain("Codex: confirm that ChatGPT on your phone can see a remote session.");
        }
        AgentKind::Claude => {
            output.plain(
                "Claude Code: run `/remote-control` in an active conversation and wait for `/rc active`.",
            );
            output
                .plain("Claude Code: confirm that Claude on your phone can see the conversation.");
        }
        AgentKind::Cursor => {
            output.plain("Cursor: open Agents → Remote Control and finish pairing.");
            output.plain("Cursor: confirm that Cursor on your phone can see the local agent.");
        }
    }
}

fn confirmation_names(confirmations: &[EvidenceKind]) -> String {
    confirmations
        .iter()
        .map(|kind| match kind {
            EvidenceKind::Pairing => "pairing",
            EvidenceKind::NativeTrust => "native adapter trust",
            EvidenceKind::PhoneVisibility => "baseline phone visibility",
            EvidenceKind::Installation => "installation",
        })
        .collect::<Vec<_>>()
        .join(", ")
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
            require_inactive_session_for_onboarding(paths)?;
            let agents = args
                .agent
                .map(|agent| vec![agent])
                .unwrap_or_else(|| AgentKind::ALL.to_vec());
            let binary = std::env::current_exe()?;
            let report = install_adapters(paths, &binary, &agents)?;
            require_current_adapters(&report.verification)?;
            invalidate_changed_adapters(paths, &report)?;
            output.title("Agent adapters");
            for path in report.changed {
                output.pass(format!("Updated {}", path.display()));
            }
            for path in report.unchanged {
                output.detail(format!("Already installed {}", path.display()));
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
            for agent in &agents {
                RemoteOnboardingRegistry::invalidate_evidence(
                    paths,
                    *agent,
                    EvidenceKind::NativeTrust,
                    EvidenceInvalidationReason::AdapterRemoved,
                    Utc::now(),
                )?;
            }
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

fn invalidate_changed_adapters(paths: &AppPaths, report: &AdapterInstallReport) -> Result<()> {
    let changed_agents = report
        .manifest
        .files
        .iter()
        .filter(|file| report.changed.contains(&file.path))
        .map(|file| file.agent)
        .collect::<std::collections::HashSet<_>>();
    for agent in changed_agents {
        RemoteOnboardingRegistry::invalidate_evidence(
            paths,
            agent,
            EvidenceKind::NativeTrust,
            EvidenceInvalidationReason::AdapterChanged,
            Utc::now(),
        )?;
    }
    Ok(())
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

fn pair(agent: AgentKind, output: &Output, paths: &AppPaths) -> Result<()> {
    require_inactive_session_for_onboarding(paths)?;
    for kind in [EvidenceKind::Pairing, EvidenceKind::PhoneVisibility] {
        RemoteOnboardingRegistry::invalidate_evidence(
            paths,
            agent,
            kind,
            EvidenceInvalidationReason::PairingReset,
            Utc::now(),
        )?;
    }
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
        }
        AgentKind::Claude => {
            output.title("Claude Code Remote Control");
            output.plain("In the active Claude Code session, run `/remote-control`.");
            output.plain(
                "During `rucksack pack`, invoke the exact one-time `/commute-mode rucksack-…` command it prints.",
            );
        }
        AgentKind::Cursor => {
            output.title("Cursor Remote Control");
            output.plain("In Cursor, open Agents → Remote Control and pair Cursor on your phone.");
            output.plain("Cursor does not currently expose a stable CLI pairing API.");
            output.plain(
                "During `rucksack pack`, invoke the exact one-time `/commute-mode rucksack-…` command it prints.",
            );
        }
    }
    complete_pairing_onboarding(agent, output, paths)
}

fn complete_pairing_onboarding(agent: AgentKind, output: &Output, paths: &AppPaths) -> Result<()> {
    if output.json() {
        return Ok(());
    }
    let detection = detect(agent, None)?;
    if !detection.installed {
        output.warn(format!(
            "{} is not installed; pairing evidence was not recorded",
            agent.display_name()
        ));
        return Ok(());
    }
    let binary = std::env::current_exe()?;
    let adapter = verify_adapter(paths, &binary, agent);
    if !adapter.current {
        output.warn(format!(
            "{} adapter is not current; run `rucksack setup` before recording pairing",
            agent.display_name()
        ));
        return Ok(());
    }
    let provider_bases = bases(&detection, &adapter)?;
    refresh_installation_evidence(paths, agent, &provider_bases)?;
    if !output.confirm(
        &format!(
            "Confirm that {} pairing completed and your phone can see a remote session",
            agent.display_name()
        ),
        false,
    )? {
        output.warn("Pairing and phone visibility were not recorded");
        return Ok(());
    }

    let recorded_at = Utc::now();
    RemoteOnboardingRegistry::record_evidence(
        paths,
        agent,
        EvidenceKind::Pairing,
        provider_bases.pairing,
        EvidenceSource::ConfirmedByUser,
        recorded_at,
    )?;
    RemoteOnboardingRegistry::record_evidence(
        paths,
        agent,
        EvidenceKind::PhoneVisibility,
        provider_bases.phone_visibility,
        EvidenceSource::ConfirmedByUser,
        recorded_at,
    )?;
    output.pass("Pairing and baseline phone visibility were confirmed by you");
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use rucksack_core::onboarding::{EvidenceBasis, EvidenceStatus};
    use std::path::Path;
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

    fn test_bases(installation: &str) -> ProviderOnboardingBases {
        ProviderOnboardingBases {
            installation: EvidenceBasis::from_parts(&[installation.as_bytes()]),
            pairing: EvidenceBasis::from_parts(&[b"pairing"]),
            native_trust: EvidenceBasis::from_parts(&[b"native-trust"]),
            phone_visibility: EvidenceBasis::from_parts(&[b"phone-visibility"]),
        }
    }

    #[test]
    fn provider_installation_change_invalidates_dependent_user_evidence() {
        let directory = tempdir().unwrap();
        let paths = test_paths(directory.path());
        let agent = AgentKind::Codex;
        let original = test_bases("original");
        let recorded_at = Utc::now();
        for (kind, source) in [
            (EvidenceKind::Installation, EvidenceSource::Measured),
            (EvidenceKind::Pairing, EvidenceSource::ConfirmedByUser),
            (EvidenceKind::NativeTrust, EvidenceSource::ConfirmedByUser),
            (
                EvidenceKind::PhoneVisibility,
                EvidenceSource::ConfirmedByUser,
            ),
        ] {
            RemoteOnboardingRegistry::record_evidence(
                &paths,
                agent,
                kind,
                original.basis(kind),
                source,
                recorded_at,
            )
            .unwrap();
        }

        let replacement = test_bases("replacement");
        let registry = refresh_installation_evidence(&paths, agent, &replacement).unwrap();
        let state = assess(&registry, agent, &replacement);

        assert_eq!(state.installation, EvidenceStatus::Current);
        for status in [state.pairing, state.native_trust, state.phone_visibility] {
            assert!(matches!(
                status,
                EvidenceStatus::Invalidated {
                    reason: EvidenceInvalidationReason::ProviderInstallationChanged,
                    ..
                }
            ));
        }
    }
}
