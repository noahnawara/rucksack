use crate::system::{run_bounded_cleared, CommandResult};
use anyhow::{anyhow, Result};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::time::{Duration, Instant};

const NETWORK_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const NETWORK_COMMAND_MAX_OUTPUT_BYTES: usize = 64 * 1024;
pub const DEFAULT_INTERNET_PROBE_URL: &str = "http://captive.apple.com/hotspot-detect.html";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WifiStatus {
    pub device: Option<String>,
    pub connected: bool,
    pub ssid: Option<String>,
    pub redacted: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteStatus {
    pub interface: Option<String>,
    pub gateway: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    pub url: String,
    pub reachable: bool,
    pub status: Option<u16>,
    pub elapsed_ms: u128,
    pub detail: String,
}

pub fn read_wifi_status() -> Result<WifiStatus> {
    let hardware = run_network_command("/usr/sbin/networksetup", &["-listallhardwareports"])?;
    require_success("networksetup -listallhardwareports", &hardware)?;
    let device = parse_wifi_device(&hardware.stdout);
    let Some(device_name) = device.clone() else {
        return Ok(WifiStatus {
            device: None,
            connected: false,
            ssid: None,
            redacted: false,
            detail: "No Wi-Fi hardware port found".to_owned(),
        });
    };
    let current = run_network_command(
        "/usr/sbin/networksetup",
        &["-getairportnetwork", &device_name],
    )?;
    let mut parsed = parse_current_network(&current.stdout, &current.stderr);
    if !parsed.0 {
        let routed_over_wifi =
            current_route_interface().ok().flatten().as_deref() == Some(device_name.as_str());
        if routed_over_wifi {
            parsed.0 = true;
            parsed.2 = true;
            parsed.3 = format!(
                "{}; default route confirms Wi-Fi is active but macOS did not expose the SSID",
                parsed.3
            );
        }
    }
    Ok(WifiStatus {
        device,
        connected: parsed.0,
        ssid: parsed.1,
        redacted: parsed.2,
        detail: parsed.3,
    })
}

pub fn read_iphone_usb_device() -> Result<Option<String>> {
    let hardware = run_network_command("/usr/sbin/networksetup", &["-listallhardwareports"])?;
    require_success("networksetup -listallhardwareports", &hardware)?;
    Ok(parse_hardware_device(&hardware.stdout, &["iphone usb"]))
}

pub fn connect_saved_wifi(device: &str, ssid: &str) -> Result<()> {
    let args = saved_wifi_connection_args(device, ssid)?;
    let borrowed = args.iter().map(String::as_str).collect::<Vec<_>>();
    let result = run_network_command("/usr/sbin/networksetup", &borrowed)?;
    require_wifi_join_success(&result)
}

pub fn saved_wifi_connection_args(device: &str, ssid: &str) -> Result<Vec<String>> {
    let device = device.trim();
    let ssid = ssid.trim();
    if device.is_empty() {
        return Err(anyhow!("Wi-Fi device name cannot be empty"));
    }
    if ssid.is_empty() {
        return Err(anyhow!("Saved hotspot name cannot be empty"));
    }
    if device.chars().any(char::is_control) || ssid.chars().any(char::is_control) {
        return Err(anyhow!(
            "Wi-Fi device and hotspot name cannot contain control characters"
        ));
    }
    Ok(vec![
        "-setairportnetwork".to_owned(),
        device.to_owned(),
        ssid.to_owned(),
    ])
}

pub fn parse_wifi_device(text: &str) -> Option<String> {
    parse_hardware_device(text, &["wi-fi", "airport"])
}

fn parse_hardware_device(text: &str, accepted_ports: &[&str]) -> Option<String> {
    let mut accepted_port = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(port) = line.strip_prefix("Hardware Port:") {
            let port = port.trim().to_ascii_lowercase();
            accepted_port = accepted_ports.contains(&port.as_str());
            continue;
        }
        if accepted_port {
            if let Some(device) = line.strip_prefix("Device:") {
                return Some(device.trim().to_owned());
            }
        }
    }
    None
}

pub fn parse_current_network(stdout: &str, stderr: &str) -> (bool, Option<String>, bool, String) {
    let combined = format!("{}\n{}", stdout.trim(), stderr.trim());
    let lower = combined.to_ascii_lowercase();
    if lower.contains("not associated")
        || lower.contains("not connected")
        || lower.contains("could not find network")
    {
        return (false, None, false, combined.trim().to_owned());
    }
    let value = stdout.lines().find_map(|line| {
        line.split_once(':')
            .map(|(_, value)| value.trim().to_owned())
    });
    let redacted = value
        .as_deref()
        .is_some_and(|ssid| ssid.is_empty() || ssid.eq_ignore_ascii_case("<redacted>"));
    let connected = value.is_some();
    (
        connected,
        value.filter(|ssid| !ssid.is_empty() && !ssid.eq_ignore_ascii_case("<redacted>")),
        redacted,
        combined.trim().to_owned(),
    )
}

pub fn read_default_route() -> Result<RouteStatus> {
    let result = run_network_command("/sbin/route", &["-n", "get", "default"])?;
    require_success("route -n get default", &result)?;
    Ok(parse_default_route(&result.stdout))
}

pub fn parse_default_route(text: &str) -> RouteStatus {
    RouteStatus {
        interface: field(text, "interface"),
        gateway: field(text, "gateway"),
        detail: text.trim().to_owned(),
    }
}

fn current_route_interface() -> Result<Option<String>> {
    Ok(read_default_route()?.interface)
}

fn run_network_command(path: &str, args: &[&str]) -> Result<CommandResult> {
    run_bounded_cleared(
        path,
        args,
        NETWORK_COMMAND_TIMEOUT,
        NETWORK_COMMAND_MAX_OUTPUT_BYTES,
    )
}

fn field(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (candidate, value) = line.trim().split_once(':')?;
        if candidate.trim() == key {
            Some(value.trim().to_owned())
        } else {
            None
        }
    })
}

/// Verify an actual internet path, not merely an HTTP response.
///
/// The default Apple captive-network endpoint is intercepted by many login portals. For that
/// endpoint, rucksack requires Apple's exact success page and final host. A redirect to a portal,
/// a 200 response with login HTML, or a truncated response fails the preflight.
pub fn internet_probe(url: &str, timeout_seconds: u64) -> ProbeResult {
    probe_with_policy(url, timeout_seconds, true)
}

fn probe_with_policy(url: &str, timeout_seconds: u64, strict_internet: bool) -> ProbeResult {
    let started = Instant::now();
    let client = match Client::builder()
        .timeout(Duration::from_secs(timeout_seconds))
        .user_agent("rucksack/0.1")
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return ProbeResult {
                url: url.to_owned(),
                reachable: false,
                status: None,
                elapsed_ms: started.elapsed().as_millis(),
                detail: error.to_string(),
            }
        }
    };
    match client.get(url).send() {
        Ok(mut response) => {
            let status = response.status();
            if strict_internet && is_apple_captive_probe(url) {
                let final_host = response.url().host_str().map(str::to_owned);
                let mut body = String::new();
                let body_result = response.by_ref().take(64 * 1024).read_to_string(&mut body);
                let success_body = apple_captive_success_body(&body);
                let reachable = status.is_success()
                    && final_host.as_deref() == Some("captive.apple.com")
                    && body_result.is_ok()
                    && success_body;
                let detail = if reachable {
                    format!("{} · Apple captive-network success page", status)
                } else {
                    format!(
                        "captive-network verification failed: status={} final_host={:?} success_body={} read_ok={}",
                        status,
                        final_host,
                        success_body,
                        body_result.is_ok()
                    )
                };
                ProbeResult {
                    url: url.to_owned(),
                    reachable,
                    status: Some(status.as_u16()),
                    elapsed_ms: started.elapsed().as_millis(),
                    detail,
                }
            } else {
                ProbeResult {
                    url: url.to_owned(),
                    reachable: true,
                    status: Some(status.as_u16()),
                    elapsed_ms: started.elapsed().as_millis(),
                    detail: status.to_string(),
                }
            }
        }
        Err(error) => ProbeResult {
            url: url.to_owned(),
            reachable: false,
            status: error.status().map(|status| status.as_u16()),
            elapsed_ms: started.elapsed().as_millis(),
            detail: error.to_string(),
        },
    }
}

fn is_apple_captive_probe(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|parsed| {
        parsed.host_str() == Some("captive.apple.com") && parsed.path() == "/hotspot-detect.html"
    })
}

fn apple_captive_success_body(body: &str) -> bool {
    let normalized = body.to_ascii_lowercase();
    normalized.contains("<title>success</title>") && normalized.contains("<body>success</body>")
}

fn require_success(name: &str, result: &CommandResult) -> Result<()> {
    if result.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "{name} failed with exit code {}: {}",
            result.code,
            result.combined_trimmed()
        ))
    }
}

fn require_wifi_join_success(result: &CommandResult) -> Result<()> {
    let detail = result.combined_trimmed();
    if result.success() && result.stdout.is_empty() && result.stderr.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "networksetup -setairportnetwork failed with exit code {}: {}",
            result.code,
            detail
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wifi_device() {
        let text = "Hardware Port: Wi-Fi\nDevice: en0\nEthernet Address: aa:bb\n";
        assert_eq!(parse_wifi_device(text).as_deref(), Some("en0"));
    }

    #[test]
    fn parses_ssid() {
        let parsed = parse_current_network("Current Wi-Fi Network: Max's iPhone\n", "");
        assert!(parsed.0);
        assert_eq!(parsed.1.as_deref(), Some("Max's iPhone"));
    }

    #[test]
    fn parses_usb_tether_default_route() {
        let route = parse_default_route(
            "   route to: default\n\
             destination: default\n\
             gateway: link#24\n\
             interface: en7\n",
        );
        assert_eq!(route.interface.as_deref(), Some("en7"));
        assert_eq!(route.gateway.as_deref(), Some("link#24"));
    }

    #[test]
    fn parses_iphone_usb_device() {
        let text = "Hardware Port: Wi-Fi\nDevice: en0\n\n\
                    Hardware Port: iPhone USB\nDevice: en7\n";
        assert_eq!(
            parse_hardware_device(text, &["iphone usb"]).as_deref(),
            Some("en7")
        );
    }

    #[test]
    fn saved_wifi_command_never_contains_a_password_argument() {
        let args = saved_wifi_connection_args("en0", "Noah's iPhone").unwrap();
        assert_eq!(args, vec!["-setairportnetwork", "en0", "Noah's iPhone"]);
        assert_eq!(args.len(), 3);
    }

    #[test]
    fn saved_wifi_command_rejects_control_characters() {
        assert!(saved_wifi_connection_args("en0", "Noah\nother").is_err());
        assert!(saved_wifi_connection_args("en0\tother", "Noah").is_err());
    }

    #[test]
    fn wifi_join_rejects_macos_failure_text_with_zero_exit_code() {
        let result = CommandResult {
            code: 0,
            stdout: "Could not find network Noah.\n".to_owned(),
            stderr: String::new(),
        };
        let error = require_wifi_join_success(&result).unwrap_err();
        assert!(error.to_string().contains("Could not find network Noah."));
    }

    #[test]
    fn wifi_join_rejects_unknown_output_with_zero_exit_code() {
        let result = CommandResult {
            code: 0,
            stdout: "Unexpected networksetup diagnostic\n".to_owned(),
            stderr: String::new(),
        };

        assert!(require_wifi_join_success(&result).is_err());
    }

    #[test]
    fn wifi_join_accepts_silent_zero_exit_code() {
        let result = CommandResult {
            code: 0,
            stdout: String::new(),
            stderr: String::new(),
        };

        assert!(require_wifi_join_success(&result).is_ok());
    }

    #[test]
    fn recognizes_only_the_expected_apple_probe() {
        assert!(is_apple_captive_probe(
            "http://captive.apple.com/hotspot-detect.html"
        ));
        assert!(!is_apple_captive_probe(
            "https://example.com/hotspot-detect.html"
        ));
        assert!(!is_apple_captive_probe("http://captive.apple.com/other"));
    }

    #[test]
    fn validates_apple_success_page() {
        assert!(apple_captive_success_body(
            "<HTML><HEAD><TITLE>Success</TITLE></HEAD><BODY>Success</BODY></HTML>"
        ));
        assert!(!apple_captive_success_body(
            "<html><title>Sign in</title><body>Success</body></html>"
        ));
    }
}
