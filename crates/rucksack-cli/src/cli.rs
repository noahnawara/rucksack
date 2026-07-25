use clap::{Args, Parser, Subcommand};
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "rucksack",
    version,
    about = "close the lid. keep your agents running.",
    long_about = "close the lid. keep your agents running.\n\nrucksack switches this Mac to your phone hotspot and keeps it awake on battery, so the work you have running survives the walk to the train."
)]
pub struct Cli {
    /// Show the probes and paths behind each step.
    #[arg(long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Switch to your hotspot and keep this Mac awake with the lid closed.
    Pack(PackArgs),

    /// Say whether this Mac is still packed.
    Status(StatusArgs),

    /// Let this Mac sleep normally again.
    Unpack,

    /// Print a Codex Remote Control pairing code.
    Pair,

    /// Star rucksack on GitHub.
    Star,

    /// Install and inspect the privileged power helper.
    Helper {
        #[command(subcommand)]
        command: HelperCommand,
    },

    /// Internal safety watcher.
    #[command(hide = true)]
    Daemon(DaemonArgs),
}

#[derive(Debug, Clone, Default, Args)]
pub struct PackArgs {
    /// Hotspot to join. Remembered for next time.
    #[arg(long, conflicts_with_all = ["usb", "here"])]
    pub hotspot: Option<String>,

    /// Use the iPhone's USB tethering instead of Wi-Fi.
    #[arg(long, conflicts_with = "here")]
    pub usb: bool,

    /// Keep the network this Mac is on. Use it when you are already tethered.
    #[arg(long)]
    pub here: bool,

    /// How long to stay awake, such as 45m, 90m, or 2h. Defaults to 24h.
    #[arg(long = "for", value_parser = parse_duration_minutes)]
    pub duration_minutes: Option<u64>,

    /// Fail instead of warning when Remote Control cannot be started.
    #[arg(long)]
    pub require_remote: bool,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Print the whole session record.
    #[arg(long)]
    pub full: bool,
}

#[derive(Debug, Subcommand)]
pub enum HelperCommand {
    /// Install the root helper. macOS authenticates once.
    Install,
    /// Show what the helper reports.
    Status,
    /// Restore normal sleep and remove the helper.
    Uninstall,
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn parses_duration() {
        assert_eq!(parse_duration_minutes("75m").unwrap(), 75);
        assert_eq!(parse_duration_minutes("1h30m").unwrap(), 90);
        assert_eq!(parse_duration_minutes("45").unwrap(), 45);
        assert!(parse_duration_minutes("0m").is_err());
        assert!(parse_duration_minutes("48h").is_err());
    }

    /// The whole surface is five verbs. Anything more is a thing to learn.
    #[test]
    fn the_command_surface_stays_small() {
        let help = Cli::command().render_help().to_string();
        for verb in ["pack", "status", "unpack", "pair", "star", "helper"] {
            assert!(help.contains(&format!("\n  {verb} ")), "{verb} missing");
        }
        for gone in [
            "setup", "doctor", "report", "recover", "adapters", "hook", "leave", "arrive",
        ] {
            assert!(!help.contains(&format!("\n  {gone} ")), "{gone} came back");
        }
        assert!(!help.contains("--json"));
    }

    #[test]
    fn pack_takes_no_questions() {
        let help = Cli::command()
            .find_subcommand_mut("pack")
            .expect("pack exists")
            .render_help()
            .to_string();
        for gone in ["--yes", "--agent", "--focus", "--allow-unverified"] {
            assert!(!help.contains(gone), "{gone} came back");
        }
    }
}
