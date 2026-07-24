use crate::helper_client::HelperClient;
use crate::install::installed_helper_exists;
use crate::onboarding::{assess, bases, ProviderOnboarding};
use rucksack_core::agent::{detect_all, verify_adapters, AdapterFileStatus, AgentAdapterEvidence};
use rucksack_core::network::{
    internet_probe, probe, provider_probe_url, read_default_route, read_iphone_usb_device,
    read_wifi_status, DEFAULT_INTERNET_PROBE_URL,
};
use rucksack_core::onboarding::RemoteOnboardingRegistry;
use rucksack_core::power::{
    read_active_sleep_utilities, read_power_status, read_sleep_disabled, read_thermal_status,
    PowerSource, ThermalLevel,
};
use rucksack_core::{AgentKind, AppPaths, Config};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckLevel {
    Pass,
    Warning,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub level: CheckLevel,
    pub summary: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub ready: bool,
    pub checks: Vec<Check>,
}

pub fn run(
    paths: &AppPaths,
    config: &Config,
    project: &Path,
    rucksack_binary: &Path,
    agent: Option<AgentKind>,
) -> DoctorReport {
    let mut checks = Vec::new();

    checks.push(if cfg!(target_os = "macos") {
        pass("macOS", "Supported host")
    } else {
        fail("macOS", "Rucksack closed-lid mode requires macOS")
    });

    checks.push(if installed_helper_exists() {
        match HelperClient::default().status() {
            Ok(status) => Check {
                name: "Power helper".to_owned(),
                level: CheckLevel::Pass,
                summary: if status.as_ref().is_some_and(|status| status.active) {
                    "Installed and serving an active lease".to_owned()
                } else {
                    "Installed and reachable".to_owned()
                },
                detail: status.and_then(|status| serde_json::to_string(&status).ok()),
            },
            Err(error) => Check {
                name: "Power helper".to_owned(),
                level: CheckLevel::Fail,
                summary: "Installed but unreachable".to_owned(),
                detail: Some(error.to_string()),
            },
        }
    } else {
        fail(
            "Power helper",
            "Not installed; run `rucksack helper install`",
        )
    });

    match config.validate() {
        Ok(()) => checks.push(pass("Configuration", "Safety thresholds are consistent")),
        Err(error) => checks.push(Check {
            name: "Configuration".to_owned(),
            level: CheckLevel::Fail,
            summary: error,
            detail: Some(paths.config_file.display().to_string()),
        }),
    }

    match read_power_status() {
        Ok(power) => {
            let level = if power.percent.is_none()
                || power.source == PowerSource::Unknown
                || power
                    .percent
                    .is_some_and(|percent| percent < config.safety.minimum_start_battery_percent)
            {
                CheckLevel::Fail
            } else {
                CheckLevel::Pass
            };
            checks.push(Check {
                name: "Battery".to_owned(),
                level,
                summary: format!(
                    "{}{}",
                    power.source_label,
                    power
                        .percent
                        .map(|percent| format!(" · {percent}%"))
                        .unwrap_or_default()
                ),
                detail: None,
            });
        }
        Err(error) => checks.push(with_error("Battery", error)),
    }

    match read_sleep_disabled() {
        Ok(value) => checks.push(Check {
            name: "Sleep state".to_owned(),
            level: if value == 0 {
                CheckLevel::Pass
            } else {
                CheckLevel::Fail
            },
            summary: if value == 0 {
                "Normal sleep is enabled".to_owned()
            } else {
                "SleepDisabled is already 1; end the utility that owns it before using Rucksack"
                    .to_owned()
            },
            detail: Some(format!("SleepDisabled={value}")),
        }),
        Err(error) => checks.push(with_error("Sleep state", error)),
    }

    match read_active_sleep_utilities() {
        Ok(utilities) if utilities.is_empty() => checks.push(pass(
            "Sleep assertions",
            "No active Amphetamine or caffeinate assertions",
        )),
        Ok(utilities) => {
            let names = utilities
                .iter()
                .map(|utility| utility.display_name())
                .collect::<Vec<_>>()
                .join(", ");
            checks.push(Check {
                name: "Sleep assertions".to_owned(),
                level: CheckLevel::Fail,
                summary: format!("Active keep-awake utilities: {names}"),
                detail: Some(
                    "End these utilities before starting Rucksack so sleep ownership is unambiguous"
                        .to_owned(),
                ),
            });
        }
        Err(error) => checks.push(with_error("Sleep assertions", error)),
    }

    match read_thermal_status() {
        Ok(thermal) => checks.push(Check {
            name: "Thermals".to_owned(),
            level: if thermal.throttled || thermal.level == ThermalLevel::Unknown {
                CheckLevel::Fail
            } else {
                CheckLevel::Pass
            },
            summary: if thermal.throttled {
                "Thermal throttling is active".to_owned()
            } else if thermal.level == ThermalLevel::Unknown {
                "Thermal pressure is unavailable".to_owned()
            } else {
                format!("{:?}", thermal.level)
            },
            detail: thermal
                .cpu_speed_limit_percent
                .map(|speed| format!("CPU speed limit {speed}%")),
        }),
        Err(error) => checks.push(Check {
            name: "Thermals".to_owned(),
            level: CheckLevel::Warning,
            summary: "Could not read thermal pressure".to_owned(),
            detail: Some(error.to_string()),
        }),
    }

    if config.hotspot.require_verified_ssid {
        match read_wifi_status() {
            Ok(wifi) => {
                let expected = config.hotspot.ssid.as_deref();
                let exact = expected
                    .zip(wifi.ssid.as_deref())
                    .is_some_and(|(a, b)| a == b);
                let level = if !wifi.connected {
                    CheckLevel::Fail
                } else if expected.is_some() && !exact {
                    CheckLevel::Warning
                } else {
                    CheckLevel::Pass
                };
                checks.push(Check {
                    name: "Wi-Fi".to_owned(),
                    level,
                    summary: wifi.ssid.clone().unwrap_or_else(|| {
                        if wifi.redacted {
                            "Connected; SSID is privacy-redacted".to_owned()
                        } else {
                            "No verified Wi-Fi network".to_owned()
                        }
                    }),
                    detail: Some(wifi.detail),
                });
            }
            Err(error) => checks.push(with_error("Wi-Fi", error)),
        }
    } else {
        checks.push(pass(
            "Wi-Fi",
            if config.hotspot.require_iphone_usb {
                "Not required in iPhone USB mode"
            } else {
                "Not required in default-route mode"
            },
        ));
    }

    if config.hotspot.require_iphone_usb {
        match (read_iphone_usb_device(), read_default_route()) {
            (Ok(Some(device)), Ok(route)) => checks.push(Check {
                name: "Default route".to_owned(),
                level: if route.interface.as_deref() == Some(device.as_str()) {
                    CheckLevel::Pass
                } else {
                    CheckLevel::Fail
                },
                summary: match route.interface.as_deref() {
                    Some(interface) if interface == device => {
                        format!("Through iPhone USB interface {device}")
                    }
                    Some(interface) => {
                        format!("Through {interface}; expected iPhone USB interface {device}")
                    }
                    None => format!("No route through iPhone USB interface {device}"),
                },
                detail: route.gateway.map(|gateway| format!("Gateway {gateway}")),
            }),
            (Ok(None), _) => checks.push(fail(
                "Default route",
                "No iPhone USB network service was found",
            )),
            (Err(error), _) => checks.push(with_error("iPhone USB", error)),
            (_, Err(error)) => checks.push(with_error("Default route", error)),
        }
    } else {
        match read_default_route() {
            Ok(route) => checks.push(Check {
                name: "Default route".to_owned(),
                level: if route.interface.is_some() {
                    CheckLevel::Pass
                } else {
                    CheckLevel::Fail
                },
                summary: route
                    .interface
                    .clone()
                    .map(|interface| format!("Through {interface}"))
                    .unwrap_or_else(|| "No interface".to_owned()),
                detail: route.gateway.map(|gateway| format!("Gateway {gateway}")),
            }),
            Err(error) => checks.push(with_error("Default route", error)),
        }
    }

    let internet = internet_probe(
        DEFAULT_INTERNET_PROBE_URL,
        config.hotspot.probe_timeout_seconds,
    );
    checks.push(Check {
        name: "Internet".to_owned(),
        level: if internet.reachable {
            CheckLevel::Pass
        } else {
            CheckLevel::Fail
        },
        summary: if internet.reachable {
            format!("Reachable in {} ms", internet.elapsed_ms)
        } else {
            "Probe failed".to_owned()
        },
        detail: Some(internet.detail),
    });

    let detections = detect_all(Some(project));
    let selected = match agent {
        Some(kind) => detections
            .into_iter()
            .filter(|item| item.kind == kind)
            .collect(),
        None => detections,
    };
    let adapter_agents = selected
        .iter()
        .filter(|detection| detection.installed || detection.running)
        .map(|detection| detection.kind)
        .collect::<Vec<_>>();
    for detection in &selected {
        checks.push(Check {
            name: detection.kind.display_name().to_owned(),
            level: if detection.installed {
                CheckLevel::Pass
            } else if agent.is_some() {
                CheckLevel::Fail
            } else {
                CheckLevel::Warning
            },
            summary: detection.detail.clone(),
            detail: Some(format!("PIDs {:?}", detection.matching_pids)),
        });
        let provider = probe(
            provider_probe_url(detection.kind),
            config.hotspot.probe_timeout_seconds,
        );
        checks.push(Check {
            name: format!("{} endpoint", detection.kind.display_name()),
            level: if provider.reachable {
                CheckLevel::Pass
            } else {
                CheckLevel::Warning
            },
            summary: if provider.reachable {
                "Reachable".to_owned()
            } else {
                "Could not reach provider".to_owned()
            },
            detail: Some(provider.detail),
        });
    }

    if adapter_agents.is_empty() {
        checks.push(warning(
            "Agent adapters",
            "No installed agent has an adapter to verify",
        ));
    } else {
        let verification = verify_adapters(paths, rucksack_binary, &adapter_agents);
        let onboarding_registry = RemoteOnboardingRegistry::load(paths);
        if let Err(error) = &onboarding_registry {
            checks.push(with_error("Remote onboarding", error));
        }
        for evidence in verification.agents {
            checks.push(adapter_check(&evidence));
            let Some(detection) = selected
                .iter()
                .find(|detection| detection.kind == evidence.agent)
            else {
                checks.push(fail(
                    "Remote onboarding",
                    "Agent detection disappeared during readiness checks",
                ));
                continue;
            };
            if !evidence.current {
                checks.push(fail(
                    &format!("{} Remote Control", evidence.agent.display_name()),
                    "Adapter must be current before onboarding can be assessed",
                ));
                continue;
            }
            let Ok(registry) = onboarding_registry.as_ref() else {
                continue;
            };
            match bases(detection, &evidence) {
                Ok(provider_bases) => checks.push(onboarding_check(
                    evidence.agent,
                    &assess(registry, evidence.agent, &provider_bases),
                )),
                Err(error) => checks.push(with_error(
                    &format!("{} Remote Control", evidence.agent.display_name()),
                    error,
                )),
            }
        }
    }

    let ready = checks
        .iter()
        .all(|check| !matches!(check.level, CheckLevel::Fail));
    DoctorReport { ready, checks }
}

fn onboarding_check(agent: AgentKind, onboarding: &ProviderOnboarding) -> Check {
    let name = format!("{} Remote Control", agent.display_name());
    if onboarding.is_current() {
        return Check {
            name,
            level: CheckLevel::Pass,
            summary: "Pairing, native trust, and baseline phone visibility confirmed by you"
                .to_owned(),
            detail: Some(onboarding.detail()),
        };
    }
    Check {
        name,
        level: CheckLevel::Fail,
        summary: "Onboarding incomplete; run `rucksack setup` interactively".to_owned(),
        detail: Some(onboarding.detail()),
    }
}

fn pass(name: &str, summary: &str) -> Check {
    Check {
        name: name.to_owned(),
        level: CheckLevel::Pass,
        summary: summary.to_owned(),
        detail: None,
    }
}

fn warning(name: &str, summary: &str) -> Check {
    Check {
        name: name.to_owned(),
        level: CheckLevel::Warning,
        summary: summary.to_owned(),
        detail: None,
    }
}

fn fail(name: &str, summary: &str) -> Check {
    Check {
        name: name.to_owned(),
        level: CheckLevel::Fail,
        summary: summary.to_owned(),
        detail: None,
    }
}

fn with_error(name: &str, error: impl std::fmt::Display) -> Check {
    Check {
        name: name.to_owned(),
        level: CheckLevel::Fail,
        summary: "Check failed".to_owned(),
        detail: Some(error.to_string()),
    }
}

fn adapter_check(evidence: &AgentAdapterEvidence) -> Check {
    let name = format!("{} adapter", evidence.agent.display_name());
    if evidence.current {
        return pass(&name, "Current, owned, and bound to this executable");
    }
    let issues = evidence
        .files
        .iter()
        .filter(|file| !file.is_current())
        .map(|file| adapter_status_name(file.status))
        .collect::<Vec<_>>()
        .join(", ");
    Check {
        name,
        level: CheckLevel::Fail,
        summary: format!("Not ready: {issues}"),
        detail: serde_json::to_string(evidence).ok(),
    }
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
