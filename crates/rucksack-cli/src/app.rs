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
        Command::Status(args) => {
            let config = Config::load(&paths)?;
            flow::status(&args, &output, &paths, &config)
        }
        Command::Unpack => flow::unpack(&output, &paths),
        Command::Pair => pair(&output, &paths, &Config::load(&paths)?),
        Command::Star => crate::star::star(&output),
        Command::Helper { command } => helper(command, &output),
        Command::Daemon(args) => {
            let config = Config::load(&paths)?;
            crate::daemon::run(args.session_id, &paths, &config)
        }
    }
}

/// Print a Codex pairing code for the phone.
///
/// Unlike `pack`, this one is allowed to fail: it does nothing except ask Codex a question, so
/// somebody who typed it with Codex switched off should be told why nothing happened.
fn pair(output: &Output, paths: &AppPaths, config: &Config) -> Result<()> {
    if !config.adapters.codex {
        anyhow::bail!(
            "Pairing is Codex's, and `adapters.codex` is off in {}.\nSet it to true, then run `rucksack pair` again.",
            paths.config_file.display()
        );
    }
    let result = codex::pair(&paths.home).context("Could not ask Codex for a pairing code.")?;
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
            let status = HelperClient::default().status()?;
            if status.as_ref().is_some_and(|status| status.active) {
                output.done("The power helper holds a lease.");
            } else {
                output.done("The power helper is installed and idle.");
            }
            if let Some(warning) = stale_helper_warning(
                install::installed_helper_matches_source(),
                status.as_ref().and_then(|status| status.version.as_deref()),
                env!("CARGO_PKG_VERSION"),
            ) {
                output.warn(warning);
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

/// What to say when the helper answering is not the one this CLI shipped with.
///
/// `cargo install` replaces the binaries in `~/.cargo/bin`. The helper that actually holds this Mac
/// awake is a copy at `/Library/PrivilegedHelperTools`, refreshed only by `rucksack helper install`,
/// so an update that skips that step leaves a new CLI talking to an old helper for good.
///
/// A helper with no version at all is the same fault seen from further away: the field has existed
/// since it was added, so silence means the installed helper predates it and is certainly stale.
///
/// Deliberately pure, and deliberately not an error. A skew is worth saying out loud, but the helper
/// still holds leases and refusing to talk to it would strand someone mid-commute over a mismatch
/// that, today, still works.
pub fn stale_helper_warning(
    matches_source: Option<bool>,
    installed: Option<&str>,
    running: &str,
) -> Option<String> {
    let action = "Run `rucksack helper install` to update it.";
    match (matches_source, installed) {
        // The bytes are the whole answer when they can be read.
        (Some(true), _) => None,
        (Some(false), Some(installed)) if installed != running => Some(format!(
            "The installed helper is {installed}, but this is rucksack {running}. {action}"
        )),
        (Some(false), _) => Some(format!(
            "The installed helper is a different build of {running} than this one. {action}"
        )),
        // No bytes to compare, so fall back to what the helper says about itself.
        (None, Some(installed)) if installed == running => None,
        (None, Some(installed)) => Some(format!(
            "The installed helper is {installed}, but this is rucksack {running}. {action}"
        )),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matching_helper_is_not_worth_mentioning() {
        assert_eq!(
            stale_helper_warning(Some(true), Some("0.1.0-alpha.6"), "0.1.0-alpha.6"),
            None
        );
    }

    /// The case a version comparison cannot see, and the reason the bytes lead.
    ///
    /// `helper install` followed by `cargo install --force` leaves two different binaries that both
    /// call themselves the same release. Observed twice on one machine in one evening, and reported
    /// as a match both times by the version check this now backs up.
    #[test]
    fn a_different_build_of_the_same_version_is_still_stale() {
        let warning = stale_helper_warning(Some(false), Some("0.1.0-alpha.6"), "0.1.0-alpha.6")
            .expect("different bytes are a skew whatever the version says");

        assert!(warning.contains("a different build"));
        assert!(warning.contains("rucksack helper install"));
    }

    /// Bytes outrank the version, so matching bytes end the question.
    #[test]
    fn matching_bytes_settle_it() {
        assert_eq!(
            stale_helper_warning(Some(true), Some("0.1.0-alpha.4"), "0.1.0-alpha.6"),
            None
        );
    }

    /// Neither signal available is not evidence of a fault, so it accuses nobody.
    #[test]
    fn nothing_to_compare_says_nothing() {
        assert_eq!(stale_helper_warning(None, None, "0.1.0-alpha.6"), None);
    }

    /// The state this Mac was actually in: updated CLI, helper left behind by an update that
    /// stopped after `cargo install`.
    #[test]
    fn a_helper_from_another_release_says_both_versions_and_what_to_do() {
        let warning = stale_helper_warning(Some(false), Some("0.1.0-alpha.4"), "0.1.0-alpha.6")
            .expect("a skew is worth reporting");

        assert!(warning.contains("0.1.0-alpha.4"), "names what is installed");
        assert!(warning.contains("0.1.0-alpha.6"), "names what is running");
        assert!(warning.contains("rucksack helper install"), "says the fix");
    }

    /// The bytes cannot always be read — no helper installed, no sibling binary to compare — and
    /// the version is still worth asking in that case.
    #[test]
    fn the_version_still_answers_when_the_bytes_cannot() {
        let warning = stale_helper_warning(None, Some("0.1.0-alpha.4"), "0.1.0-alpha.6")
            .expect("a version mismatch is a skew even with no bytes to weigh it against");

        assert!(warning.contains("0.1.0-alpha.4"), "names what is installed");
        assert!(warning.contains("0.1.0-alpha.6"), "names what is running");
        assert!(warning.contains("rucksack helper install"));
    }
}
