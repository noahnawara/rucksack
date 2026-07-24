use clap::{Args, Parser, Subcommand};
use rucksack_core::{AgentKind, Focus};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "rucksack",
    version,
    about = "Leave the desk. Keep the agent.",
    long_about = "A safe macOS handoff ritual for local coding agents, closed-lid execution, and mobile hotspots."
)]
pub struct Cli {
    /// Show implementation details and probe output.
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Emit newline-delimited machine-readable progress and results.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// One-time helper, adapter, and hotspot setup.
    Setup(SetupArgs),

    /// Check whether this Mac is ready for a commute.
    Doctor(DoctorArgs),

    /// Prepare the Mac, remote, hotspot, and agent policy.
    Leave(LeaveArgs),

    /// Compatibility alias for `leave`.
    Pack(LeaveArgs),

    /// Show the active lease, connection, safety, and agent state.
    Status(StatusArgs),

    /// Restore normal sleep and remove Commute Mode.
    Arrive(ArriveArgs),

    /// Compatibility alias for `arrive`.
    Unpack(ArriveArgs),

    /// Restore normal sleep after an interrupted session.
    Recover(RecoverArgs),

    /// Install, inspect, or remove native agent adapters.
    Adapters {
        #[command(subcommand)]
        command: AdapterCommand,
    },

    /// Pair or explain pairing for a provider remote.
    Pair(PairArgs),

    /// Install and inspect the privileged power helper.
    Helper {
        #[command(subcommand)]
        command: HelperCommand,
    },

    /// Internal lifecycle hook entrypoint.
    #[command(hide = true)]
    Hook(HookArgs),

    /// Internal user daemon.
    #[command(hide = true)]
    Daemon(DaemonArgs),
}

#[derive(Debug, Args)]
pub struct SetupArgs {
    /// Hotspot SSID to save. If omitted, setup reads the current Wi-Fi network.
    #[arg(long, conflicts_with = "usb")]
    pub hotspot: Option<String>,

    /// Require iPhone USB tethering as the commute's default route.
    #[arg(long)]
    pub usb: bool,

    /// Skip the administrator helper installation.
    #[arg(long)]
    pub no_helper: bool,

    /// Skip agent adapter installation.
    #[arg(long)]
    pub no_adapters: bool,

    /// Accept defaults without confirmation.
    #[arg(long)]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Check only this agent's remote and adapter.
    #[arg(long)]
    pub agent: Option<AgentKind>,
}

#[derive(Debug, Clone, Args)]
pub struct LeaveArgs {
    /// Agent to hand off. Auto-detected when omitted.
    #[arg(long)]
    pub agent: Option<AgentKind>,

    /// Expected hotspot SSID.
    #[arg(long, conflicts_with = "usb")]
    pub hotspot: Option<String>,

    /// Require iPhone USB tethering as the commute's default route.
    #[arg(long)]
    pub usb: bool,

    /// Maximum session duration, such as 45m, 90m, or 2h.
    #[arg(long = "for", value_parser = parse_duration_minutes)]
    pub duration_minutes: Option<u64>,

    /// Behavioral focus: continue, finish, investigate, review, or low-power.
    #[arg(long)]
    pub focus: Option<Focus>,

    /// Accept prompts where the state can still be measured.
    #[arg(long)]
    pub yes: bool,

    /// Accept a privacy-redacted SSID after an exact join request or your Wi-Fi-menu confirmation.
    #[arg(long)]
    pub allow_unverified_ssid: bool,

    /// Continue after an explicit provider-remote confirmation rather than a machine check.
    #[arg(long)]
    pub allow_unverified_remote: bool,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Print the complete session and helper records.
    #[arg(long)]
    pub full: bool,
}

#[derive(Debug, Args)]
pub struct ArriveArgs {
    /// Clear state even if no active session is found.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct RecoverArgs {
    /// Perform the recovery without an interactive confirmation.
    #[arg(long)]
    pub yes: bool,
}

#[derive(Debug, Subcommand)]
pub enum AdapterCommand {
    /// Install hooks, skills, and reversible rules.
    Install(AdapterArgs),
    /// Show installed adapter files.
    Status,
    /// Remove only Rucksack-owned entries.
    Remove(AdapterArgs),
}

#[derive(Debug, Args)]
pub struct AdapterArgs {
    /// Limit the operation to one provider.
    #[arg(long)]
    pub agent: Option<AgentKind>,
}

#[derive(Debug, Args)]
pub struct PairArgs {
    pub agent: AgentKind,
}

#[derive(Debug, Subcommand)]
pub enum HelperCommand {
    /// Install the root LaunchDaemon. Requires one administrator authentication.
    Install,
    /// Show helper status.
    Status,
    /// Restore any lease and remove the helper.
    Uninstall,
}

#[derive(Debug, Args)]
pub struct HookArgs {
    pub agent: AgentKind,
}

#[derive(Debug, Args)]
pub struct DaemonArgs {
    #[arg(long)]
    pub session_id: Uuid,
}

pub fn parse_duration_minutes(value: &str) -> Result<u64, String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() {
        return Err("duration cannot be empty".to_owned());
    }

    let mut total = 0u64;
    let mut number = String::new();
    let mut saw_unit = false;

    for character in value.chars() {
        if character.is_ascii_digit() {
            number.push(character);
            continue;
        }
        if number.is_empty() {
            return Err(format!("invalid duration {value:?}"));
        }
        let amount = number
            .parse::<u64>()
            .map_err(|_| format!("invalid duration {value:?}"))?;
        number.clear();
        match character {
            'h' => total = total.saturating_add(amount.saturating_mul(60)),
            'm' => total = total.saturating_add(amount),
            _ => return Err("use minutes or hours, for example 75m or 1h30m".to_owned()),
        }
        saw_unit = true;
    }

    if !number.is_empty() {
        let amount = number
            .parse::<u64>()
            .map_err(|_| format!("invalid duration {value:?}"))?;
        if saw_unit {
            return Err("a trailing number needs an m or h unit".to_owned());
        }
        total = amount;
    }

    if total == 0 || total > 24 * 60 {
        return Err("duration must be between 1 minute and 24 hours".to_owned());
    }
    Ok(total)
}

#[allow(dead_code)]
pub fn canonical_config_path(value: Option<PathBuf>) -> Option<PathBuf> {
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_duration() {
        assert_eq!(parse_duration_minutes("75m").unwrap(), 75);
        assert_eq!(parse_duration_minutes("1h30m").unwrap(), 90);
        assert_eq!(parse_duration_minutes("45").unwrap(), 45);
    }
}
