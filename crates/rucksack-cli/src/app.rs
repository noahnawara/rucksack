use crate::cli::{Cli, Command, HelperCommand, PackArgs};
use crate::flow;
use crate::helper_client::HelperClient;
use crate::install;
use crate::output::Output;
use anyhow::{Context, Result};
use rucksack_core::{codex, AppPaths, Config};

pub fn run(cli: Cli) -> Result<()> {
    let output = Output::new(cli.verbose);
    let paths = AppPaths::discover()?;

    match cli.command.unwrap_or(Command::Pack(PackArgs::default())) {
        Command::Pack(args) => {
            let config = Config::load(&paths)?;
            flow::pack(&args, &output, &paths, &config)
        }
        Command::Status(args) => flow::status(&args, &output, &paths),
        Command::Unpack => flow::unpack(&output, &paths),
        Command::Pair => pair(&output),
        Command::Star => crate::star::star(&output),
        Command::Helper { command } => helper(command, &output),
        Command::Daemon(args) => {
            let config = Config::load(&paths)?;
            crate::daemon::run(args.session_id, &paths, &config)
        }
    }
}

/// Print a Codex pairing code for the phone.
fn pair(output: &Output) -> Result<()> {
    let result = codex::pair().context("Could not ask Codex for a pairing code.")?;
    if !result.success() {
        anyhow::bail!(
            "Codex could not produce a pairing code: {}",
            result.combined_trimmed()
        );
    }
    let parsed = serde_json::from_str::<serde_json::Value>(&result.stdout).unwrap_or_default();
    match parsed
        .get("pairingCode")
        .or_else(|| parsed.get("manualPairingCode"))
        .and_then(serde_json::Value::as_str)
    {
        Some(code) => output.done(format!("Pairing code: {code}")),
        None => output.done(result.stdout.trim()),
    }
    output.step("Enter it in ChatGPT on your phone.");
    Ok(())
}

fn helper(command: HelperCommand, output: &Output) -> Result<()> {
    match command {
        HelperCommand::Install => {
            install::install_helper(output)?;
            output.done("The power helper is installed.");
            Ok(())
        }
        HelperCommand::Status => {
            match HelperClient::default().status()? {
                Some(status) if status.active => output.done("The power helper holds a lease."),
                Some(_) => output.done("The power helper is installed and idle."),
                None => output.done("The power helper is installed and idle."),
            }
            let (helper, plist) = install::helper_paths();
            output.detail(format!("{helper}\n{plist}"));
            Ok(())
        }
        HelperCommand::Uninstall => {
            install::uninstall_helper()?;
            output.done("The power helper is gone. This Mac sleeps normally.");
            Ok(())
        }
    }
}
