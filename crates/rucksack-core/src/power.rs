use crate::system::{require_success, run_bounded_cleared, CommandResult};
use anyhow::{anyhow, Result};
use regex::Regex;
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
    pub charging: bool,
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

    let percent_re = Regex::new(r"(?P<percent>\d{1,3})%")?;
    let percent = percent_re
        .captures(text)
        .and_then(|captures| captures.name("percent"))
        .and_then(|value| value.as_str().parse::<u8>().ok())
        .filter(|value| *value <= 100);
    let lower = text.to_ascii_lowercase();
    let charging = lower.contains("charging")
        && !lower.contains("not charging")
        && !lower.contains("discharging");

    Ok(PowerStatus {
        source,
        source_label,
        percent,
        charging,
        raw: text.to_owned(),
    })
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
    } else if speed.is_some() || scheduler.is_some() || no_thermal_warning {
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
        assert!(!status.charging);
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

    #[test]
    fn no_recorded_thermal_warning_is_nominal() {
        let thermal = parse_thermal(
            "Note: No thermal warning level has been recorded\n\
             Note: No performance warning level has been recorded\n\
             Note: No CPU power status has been recorded\n",
        );
        assert!(!thermal.throttled);
        assert_eq!(thermal.level, ThermalLevel::Nominal);
    }
}
