use crate::system::{require_success, run_bounded_cleared, CommandResult};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const POWER_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const POWER_COMMAND_MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PowerSource {
    Ac,
    Battery,
    Ups,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerStatus {
    pub source: PowerSource,
    pub source_label: String,
    pub percent: Option<u8>,
    /// Whether the battery is actually draining, which is the only state in which
    /// `minutes_to_empty` means what its name says.
    pub discharging: bool,
    /// What macOS thinks is left before the battery is empty, when it is willing to say.
    ///
    /// Only ever read while discharging. The same field carries time-to-full while charging, and a
    /// Mac twenty minutes from a full battery is not a Mac twenty minutes from sleep.
    pub minutes_to_empty: Option<u64>,
    pub raw: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThermalLevel {
    Nominal,
    Fair,
    Serious,
    Critical,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThermalStatus {
    pub level: ThermalLevel,
    pub cpu_speed_limit_percent: Option<u8>,
    pub scheduler_limit_percent: Option<u8>,
    pub available_cpus: Option<u16>,
    pub throttled: bool,
    pub raw: String,
}

pub fn read_power_status() -> Result<PowerStatus> {
    let result = run_pmset(&["-g", "batt"])?;
    require_success("pmset -g batt", &result)?;
    parse_battery(&result.stdout)
}

pub fn parse_battery(text: &str) -> Result<PowerStatus> {
    let source_label = text
        .lines()
        .find_map(|line| {
            let marker = "Now drawing from '";
            let start = line.find(marker)? + marker.len();
            let tail = &line[start..];
            let end = tail.find('\'')?;
            Some(tail[..end].to_owned())
        })
        .unwrap_or_else(|| "Unknown".to_owned());

    let source = match source_label.to_ascii_lowercase().as_str() {
        label if label.contains("ac power") => PowerSource::Ac,
        label if label.contains("battery") => PowerSource::Battery,
        label if label.contains("ups") => PowerSource::Ups,
        _ => PowerSource::Unknown,
    };

    let percent = parse_percent(text);
    // Ask whether the battery is draining, not whether it is filling.
    //
    // The two are not opposites in `pmset`'s vocabulary, and the gap between them had a wrong answer
    // in it. `charging && !"not charging" && !"discharging"` was three substring tests trying to
    // spell one word, and it still missed `finishing charge` — the top-off state a plugged-in Mac
    // sits in around 99% — because that string says "charge", not "charging". A Mac in it was read as
    // not charging, so the `H:MM remaining` on that line, which means time-to-full, was reported as
    // time-to-empty. `discharging` is the one state where that field means what rucksack wants, and
    // `pmset` names it in one word.
    let discharging = text.to_ascii_lowercase().contains("discharging");

    Ok(PowerStatus {
        source,
        source_label,
        percent,
        discharging,
        minutes_to_empty: parse_minutes_to_empty(text, discharging),
        raw: text.to_owned(),
    })
}

/// macOS's own estimate of how long the battery has left, in minutes.
///
/// `pmset` prints `H:MM remaining` on the battery row, and `(no estimate)` while it is still working
/// one out — which it also does for a minute or so after waking or after the load changes sharply.
///
/// Read only while discharging, because the same field means time-to-full otherwise. `0:00` is
/// how macOS spells "no estimate yet" rather than "empty now", so it is not an answer either.
fn parse_minutes_to_empty(text: &str, discharging: bool) -> Option<u64> {
    if !discharging {
        return None;
    }
    for (at, _) in text.match_indices("remaining") {
        let head = &text[..at];
        // `\s+remaining`: the word has to be a word, not the tail of a longer one.
        let before_space = head.trim_end_matches(char::is_whitespace);
        if before_space.len() == head.len() {
            continue;
        }
        let Some((hours, minutes)) = split_clock(before_space) else {
            continue;
        };
        let total = hours.checked_mul(60)?.checked_add(minutes)?;
        return (total > 0).then_some(total);
    }
    None
}

/// The `H:MM` at the end of `text`, if that is how it ends.
fn split_clock(text: &str) -> Option<(u64, u64)> {
    let head = text.trim_end_matches(|c: char| c.is_ascii_digit());
    let minutes = &text[head.len()..];
    if minutes.len() != 2 {
        return None;
    }
    let head = head.strip_suffix(':')?;
    let before = head.trim_end_matches(|c: char| c.is_ascii_digit());
    let hours = &head[before.len()..];
    if hours.is_empty() || hours.len() > 2 {
        return None;
    }
    Some((hours.parse().ok()?, minutes.parse().ok()?))
}

/// The first `NN%` in `pmset`'s output, which is the charge.
///
/// Hand-written rather than `\d{1,3}%`, because that one pattern and the clock above were the whole
/// reason the workspace compiled `regex` and its three dependencies on every `cargo install` — and
/// both were being recompiled on every heartbeat, for the life of a twenty-four-hour lease.
fn parse_percent(text: &str) -> Option<u8> {
    for (at, _) in text.match_indices('%') {
        let head = &text[..at];
        let before = head.trim_end_matches(|c: char| c.is_ascii_digit());
        let digits = &head[before.len()..];
        if digits.is_empty() || digits.len() > 3 {
            continue;
        }
        return digits.parse::<u8>().ok().filter(|value| *value <= 100);
    }
    None
}

/// Turn "minutes until empty" into "minutes until the floor", which is the figure rucksack reports.
///
/// macOS measures to 0%. rucksack stops at the floor, so the two answer different questions and the
/// difference is not small: at 57% with a 10% floor, three hours to empty is about two and a half
/// hours to sleep. Reporting the macOS number as-is would over-promise, in exactly the direction
/// this whole projection exists to prevent.
///
/// Straight-line scaling on the remaining charge. It assumes the rate that produced the estimate
/// holds, which is the same assumption macOS already made, so this adds no optimism of its own.
pub fn minutes_until_floor(minutes_to_empty: u64, percent: u8, floor_percent: u8) -> Option<u64> {
    if percent == 0 || percent <= floor_percent {
        return Some(0);
    }
    let headroom = u64::from(percent - floor_percent);
    Some(minutes_to_empty.saturating_mul(headroom) / u64::from(percent))
}

pub fn read_sleep_disabled() -> Result<u8> {
    let result = run_pmset(&["-g"])?;
    require_success("pmset -g", &result)?;
    parse_sleep_disabled(&result.stdout)
}

pub fn parse_sleep_disabled(text: &str) -> Result<u8> {
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else {
            continue;
        };
        if !key.eq_ignore_ascii_case("SleepDisabled") {
            continue;
        }
        let value = parts
            .next()
            .ok_or_else(|| anyhow!("pmset reported SleepDisabled without a value"))?
            .parse::<u8>()
            .map_err(|error| anyhow!("pmset reported an invalid SleepDisabled value: {error}"))?;
        if value > 1 {
            anyhow::bail!("pmset reported unsupported SleepDisabled value {value}");
        }
        return Ok(value);
    }

    // A clean macOS installation omits this system-wide key until `pmset
    // disablesleep` has been used for the first time. Absence therefore means
    // the normal baseline, but only after the pmset command itself succeeded.
    Ok(0)
}

pub fn read_thermal_status() -> Result<ThermalStatus> {
    let result = run_pmset(&["-g", "therm"])?;
    require_success("pmset -g therm", &result)?;
    Ok(parse_thermal(&result.stdout))
}

/// Read what `pmset -g therm` measured, and nothing more.
///
/// `CPU_Speed_Limit` and `CPU_Scheduler_Limit` are Intel-era counters. Apple silicon never
/// populates them, so on that hardware this parses three "has been recorded" notes into
/// `ThermalLevel::Unknown` at every temperature. Absent counters are therefore an unread sensor,
/// not a healthy one: reporting `Nominal` here would claim a state that was never measured, and
/// would be the only thermal level the watcher ever saw on Apple silicon. The level that reflects
/// real heat comes from `ProcessInfo.thermalState` in the unprivileged watcher.
pub fn parse_thermal(text: &str) -> ThermalStatus {
    fn value(text: &str, key: &str) -> Option<u16> {
        text.lines().find_map(|line| {
            let line = line.trim();
            if !line.starts_with(key) {
                return None;
            }
            line.split('=').nth(1)?.trim().parse::<u16>().ok()
        })
    }

    let speed = value(text, "CPU_Speed_Limit").and_then(|v| u8::try_from(v).ok());
    let scheduler = value(text, "CPU_Scheduler_Limit").and_then(|v| u8::try_from(v).ok());
    let cpus = value(text, "CPU_Available_CPUs");
    let lower = text.to_ascii_lowercase();
    let critical = lower.contains("critical");
    let serious = lower.contains("serious");
    let fair = lower.contains("fair");
    let no_thermal_warning = lower.contains("no thermal warning");
    let thermal_warning = lower.contains("thermal warning") && !no_thermal_warning;
    let throttled = speed.is_some_and(|v| v < 100)
        || scheduler.is_some_and(|v| v < 100)
        || thermal_warning
        || serious
        || critical;
    let level = if critical {
        ThermalLevel::Critical
    } else if serious || throttled {
        ThermalLevel::Serious
    } else if fair {
        ThermalLevel::Fair
    } else if speed.is_some() || scheduler.is_some() {
        ThermalLevel::Nominal
    } else {
        ThermalLevel::Unknown
    };

    ThermalStatus {
        level,
        cpu_speed_limit_percent: speed,
        scheduler_limit_percent: scheduler,
        available_cpus: cpus,
        throttled,
        raw: text.to_owned(),
    }
}

fn run_pmset(args: &[&str]) -> Result<CommandResult> {
    run_bounded_cleared(
        "/usr/bin/pmset",
        args,
        POWER_COMMAND_TIMEOUT,
        POWER_COMMAND_MAX_OUTPUT_BYTES,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_battery_output() {
        let status = parse_battery(
            "Now drawing from 'Battery Power'\n -InternalBattery-0\t78%; discharging; 4:11 remaining",
        )
        .unwrap();
        assert_eq!(status.source, PowerSource::Battery);
        assert_eq!(status.percent, Some(78));
        assert!(status.discharging);
    }

    /// The line this exists for, exactly as a discharging Mac prints it.
    #[test]
    fn reads_the_estimate_macos_already_made() {
        let status = parse_battery(
            "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t57%; discharging; 3:09 remaining present: true",
        )
        .unwrap();
        assert_eq!(status.minutes_to_empty, Some(189));
    }

    /// The same field means time-to-full while charging, and twenty minutes from a full battery is
    /// not twenty minutes from sleep.
    #[test]
    fn a_charging_estimate_is_not_time_left() {
        let status = parse_battery(
            "Now drawing from 'AC Power'\n -InternalBattery-0 (id=1)\t57%; charging; 1:23 remaining present: true",
        )
        .unwrap();
        assert_eq!(status.minutes_to_empty, None);
    }

    /// The shapes the two hand-written parsers replaced a regex for.
    ///
    /// `\d{1,3}%` and `\d{1,2}:\d{2}\s+remaining` were doing more than they looked like: bounding the
    /// digit runs, requiring whitespace before the word, and skipping a candidate that did not fit
    /// rather than taking a wrong answer from it. Each of those is a line of code now, so each gets a
    /// case.
    #[test]
    fn the_parsers_are_as_picky_as_the_patterns_were() {
        // A digit run too long to be a percentage is not one.
        assert_eq!(parse_percent("1234%"), None);
        assert_eq!(parse_percent("no digits here %"), None);
        assert_eq!(parse_percent("999%"), None); // parses, then fails the <= 100 filter
        assert_eq!(
            parse_percent(" -InternalBattery-0\t78%; discharging"),
            Some(78)
        );

        // `remaining` has to be its own word, after a clock.
        assert_eq!(split_clock("3:09"), Some((3, 9)));
        assert_eq!(split_clock("13:09"), Some((13, 9)));
        assert_eq!(split_clock("3:9"), None); // minutes are always two digits
        assert_eq!(split_clock("309"), None);
        assert_eq!(
            parse_minutes_to_empty("57%; discharging; 3:09remaining", true),
            None
        );
        assert_eq!(
            parse_minutes_to_empty("57%; discharging; 0:00 remaining", true),
            None
        );
    }

    /// The top-off state, which says "charge" and not "charging".
    ///
    /// It is the one `pmset` word the old three-substring guard could not spell, so a Mac five
    /// minutes from a full battery reported five minutes until it fell asleep.
    #[test]
    fn finishing_charge_is_not_time_left() {
        let status = parse_battery(
            "Now drawing from 'AC Power'\n -InternalBattery-0 (id=1)\t99%; finishing charge; 0:05 remaining present: true",
        )
        .unwrap();
        assert!(!status.discharging);
        assert_eq!(status.minutes_to_empty, None);
    }

    /// macOS says this for a minute or so after a wake, and after any sharp change in load.
    #[test]
    fn no_estimate_is_not_an_estimate() {
        let status = parse_battery(
            "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t57%; discharging; (no estimate) present: true",
        )
        .unwrap();
        assert_eq!(status.minutes_to_empty, None);
    }

    /// `0:00` is how macOS spells "still working it out", not "empty now".
    #[test]
    fn a_zero_estimate_is_withheld_rather_than_reported_as_none_left() {
        let status = parse_battery(
            "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t57%; discharging; 0:00 remaining present: true",
        )
        .unwrap();
        assert_eq!(status.minutes_to_empty, None);
    }

    /// The conversion that keeps the borrowed number honest: macOS measures to empty, rucksack
    /// stops at the floor, so the session is always shorter than the battery.
    #[test]
    fn the_floor_is_nearer_than_empty() {
        assert_eq!(minutes_until_floor(189, 57, 10), Some(155));
    }

    /// A Mac already at or below the floor has no minutes left, and that is a real answer.
    #[test]
    fn at_the_floor_there_is_nothing_left_to_scale() {
        assert_eq!(minutes_until_floor(120, 10, 10), Some(0));
        assert_eq!(minutes_until_floor(120, 4, 10), Some(0));
    }

    #[test]
    fn parses_sleep_disabled() {
        assert_eq!(
            parse_sleep_disabled("System-wide power settings:\n SleepDisabled          1\n")
                .unwrap(),
            1
        );
    }

    #[test]
    fn missing_sleep_disabled_means_normal_baseline() {
        assert_eq!(
            parse_sleep_disabled(
                "System-wide power settings:\nCurrently in use:\n sleep                1\n"
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn rejects_invalid_sleep_disabled_value() {
        let error = parse_sleep_disabled("SleepDisabled 2").unwrap_err();
        assert!(error.to_string().contains("unsupported"));
    }

    #[test]
    fn parses_throttling() {
        let thermal = parse_thermal(
            "CPU_Speed_Limit = 72\nCPU_Scheduler_Limit = 80\nCPU_Available_CPUs = 8\n",
        );
        assert!(thermal.throttled);
        assert_eq!(thermal.level, ThermalLevel::Serious);
    }

    /// The exact output of `pmset -g therm` on Apple silicon, at any temperature.
    ///
    /// Nothing was measured, so nothing may be claimed. Reading this as `Nominal` is what kept the
    /// thermal release condition from ever firing on the hardware rucksack targets.
    #[test]
    fn nothing_recorded_is_unknown_rather_than_healthy() {
        let thermal = parse_thermal(
            "Note: No thermal warning level has been recorded\n\
             Note: No performance warning level has been recorded\n\
             Note: No CPU power status has been recorded\n",
        );
        assert!(!thermal.throttled);
        assert_eq!(thermal.level, ThermalLevel::Unknown);
    }

    /// An Intel Mac still reports counters, and quiet counters still mean nominal.
    #[test]
    fn reported_counters_without_throttling_are_nominal() {
        let thermal = parse_thermal(
            "CPU_Speed_Limit = 100\nCPU_Scheduler_Limit = 100\nCPU_Available_CPUs = 8\n",
        );
        assert!(!thermal.throttled);
        assert_eq!(thermal.level, ThermalLevel::Nominal);
    }
}
