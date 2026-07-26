use crate::system::{require_success, run_bounded_cleared, CommandResult};
use anyhow::{anyhow, Result};
use reqwest::blocking::Client;
use std::io::Read;
use std::time::Duration;

const NETWORK_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const NETWORK_COMMAND_MAX_OUTPUT_BYTES: usize = 64 * 1024;
pub const DEFAULT_INTERNET_PROBE_URL: &str = "http://captive.apple.com/hotspot-detect.html";

/// The Wi-Fi interface and, when macOS is willing to say, the network it is on.
///
/// `ssid` is `None` whenever macOS withholds the name, which it does for any process without
/// Location Services. That is common, so nothing may treat a missing name as "not connected".
#[derive(Debug, Clone)]
pub struct WifiStatus {
    pub device: Option<String>,
    pub ssid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteStatus {
    pub interface: Option<String>,
    pub gateway: Option<String>,
}

pub fn read_wifi_status() -> Result<WifiStatus> {
    let hardware = run_network_command("/usr/sbin/networksetup", &["-listallhardwareports"])?;
    require_success("networksetup -listallhardwareports", &hardware)?;
    let Some(device) = parse_wifi_device(&hardware.stdout) else {
        return Ok(WifiStatus {
            device: None,
            ssid: None,
        });
    };
    let current = run_network_command("/usr/sbin/networksetup", &["-getairportnetwork", &device])?;
    Ok(WifiStatus {
        ssid: parse_ssid(&current.stdout, &current.stderr),
        device: Some(device),
    })
}

pub fn read_iphone_usb_device() -> Result<Option<String>> {
    let hardware = run_network_command("/usr/sbin/networksetup", &["-listallhardwareports"])?;
    require_success("networksetup -listallhardwareports", &hardware)?;
    Ok(parse_hardware_device(&hardware.stdout, &["iphone usb"]))
}

/// Ask macOS to join a network it already has credentials for.
///
/// This fails more often than it succeeds: it cannot supply a keychain password and cannot see an
/// Apple Instant Hotspot at all. A failed attempt can also drop the connection the Mac already
/// had, so callers should only reach for it when there is nothing to lose.
pub fn connect_saved_wifi(device: &str, ssid: &str) -> Result<()> {
    let args = saved_wifi_connection_args(device, ssid)?;
    let borrowed = args.iter().map(String::as_str).collect::<Vec<_>>();
    let result = run_network_command("/usr/sbin/networksetup", &borrowed)?;
    require_wifi_join_success(&result)
}

/// Switch the Wi-Fi radio off or on.
///
/// Turning it back on makes macOS choose a network again from what is in range, which is the only
/// way left to ask for that: `airport -z` was removed in macOS 14.4, and `networksetup` has no
/// disassociate of its own.
pub fn set_wifi_power(device: &str, on: bool) -> Result<()> {
    let args = wifi_power_args(device, on)?;
    let borrowed = args.iter().map(String::as_str).collect::<Vec<_>>();
    let result = run_network_command("/usr/sbin/networksetup", &borrowed)?;
    require_success("networksetup -setairportpower", &result)
}

fn wifi_power_args(device: &str, on: bool) -> Result<Vec<String>> {
    let device = device.trim();
    if device.is_empty() {
        return Err(anyhow!("Wi-Fi device name cannot be empty"));
    }
    if device.chars().any(char::is_control) {
        return Err(anyhow!(
            "Wi-Fi device name cannot contain control characters"
        ));
    }
    Ok(vec![
        "-setairportpower".to_owned(),
        device.to_owned(),
        if on { "on" } else { "off" }.to_owned(),
    ])
}

fn saved_wifi_connection_args(device: &str, ssid: &str) -> Result<Vec<String>> {
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

fn parse_wifi_device(text: &str) -> Option<String> {
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

/// The current network's name, or `None` when there is not one to report.
///
/// macOS says `<redacted>` when the caller lacks Location Services, and reports "not associated"
/// when Wi-Fi is off or unjoined. Both mean the same thing here: no name to compare against.
fn parse_ssid(stdout: &str, stderr: &str) -> Option<String> {
    let combined = format!("{}\n{}", stdout.trim(), stderr.trim()).to_ascii_lowercase();
    if combined.contains("not associated")
        || combined.contains("not connected")
        || combined.contains("could not find network")
    {
        return None;
    }
    stdout
        .lines()
        .find_map(|line| line.split_once(':').map(|(_, value)| value.trim()))
        .filter(|ssid| !ssid.is_empty() && !ssid.eq_ignore_ascii_case("<redacted>"))
        .map(ToOwned::to_owned)
}

pub fn read_default_route() -> Result<RouteStatus> {
    let result = run_network_command("/sbin/route", &["-n", "get", "default"])?;
    require_success("route -n get default", &result)?;
    Ok(parse_default_route(&result.stdout))
}

fn parse_default_route(text: &str) -> RouteStatus {
    RouteStatus {
        interface: field(text, "interface"),
        gateway: field(text, "gateway"),
    }
}

/// Bytes an interface has carried since it came up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceTraffic {
    pub bytes_in: u64,
    pub bytes_out: u64,
}

impl InterfaceTraffic {
    pub fn total(self) -> u64 {
        self.bytes_in.saturating_add(self.bytes_out)
    }
}

/// How much this interface has carried, or `None` when macOS reports nothing usable for it.
///
/// The counters are cumulative since the interface came up, not since anyone started watching, so a
/// caller who wants "during the trip" has to difference two readings — and has to treat a decrease
/// as unavailable, because an interface that cycles starts again from zero. What makes the
/// difference meaningful at all is that the counters *do* survive changing network on the same
/// interface, which is the ordinary case for a Mac that goes from Wi-Fi to a phone and back.
pub fn read_interface_traffic(interface: &str) -> Result<Option<InterfaceTraffic>> {
    if interface.is_empty() || !interface.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(anyhow!(
            "Interface name must be ASCII alphanumeric, got {interface:?}"
        ));
    }
    let result = run_network_command("/usr/sbin/netstat", &["-ibnI", interface])?;
    require_success("netstat -ibnI", &result)?;
    Ok(parse_interface_traffic(&result.stdout))
}

/// Read the byte columns out of `netstat -ibn`, anchored on the right.
///
/// Anchoring right is not fussiness. A real NIC's link row carries an Address column and has eleven
/// fields; `lo0`, `gif0` and every `utun` leave that column empty and have ten. Counting from the
/// left therefore reads the wrong column for half the interfaces on a Mac — `Ierrs` instead of
/// `Ibytes`, a plausible small number rather than an obvious failure. The final seven fields are
/// always the counters, so the shape is verified rather than assumed, and anything else is no
/// answer instead of a wrong one.
fn parse_interface_traffic(text: &str) -> Option<InterfaceTraffic> {
    text.lines().find_map(|line| {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if !matches!(fields.len(), 10 | 11) || !fields.get(2)?.starts_with("<Link") {
            return None;
        }
        let counters = fields[fields.len() - 7..]
            .iter()
            .map(|field| field.parse::<u64>().ok())
            .collect::<Option<Vec<_>>>()?;
        Some(InterfaceTraffic {
            bytes_in: counters[2],
            bytes_out: counters[5],
        })
    })
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
        (candidate.trim() == key).then(|| value.trim().to_owned())
    })
}

/// Does this route actually reach the internet?
///
/// Not merely "did something answer": many hotel and café networks intercept the Apple
/// captive-network endpoint, so rucksack requires Apple's exact success page from Apple's own
/// host. A redirect to a portal, a 200 with login HTML, or a truncated body all count as offline,
/// because a Mac behind a portal is a Mac that cannot be reached from a phone.
pub fn reaches_internet(url: &str, timeout_seconds: u64) -> bool {
    let Ok(client) = Client::builder()
        .timeout(Duration::from_secs(timeout_seconds))
        .user_agent("rucksack/0.1")
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
    else {
        return false;
    };
    let Ok(mut response) = client.get(url).send() else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    if !is_apple_captive_probe(url) {
        return true;
    }
    if response.url().host_str() != Some("captive.apple.com") {
        return false;
    }
    let mut body = String::new();
    response
        .by_ref()
        .take(64 * 1024)
        .read_to_string(&mut body)
        .is_ok()
        && apple_captive_success_body(&body)
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

/// `networksetup` reports a failed join on stdout while still exiting 0.
fn require_wifi_join_success(result: &CommandResult) -> Result<()> {
    if result.success() && result.stdout.is_empty() && result.stderr.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "networksetup -setairportnetwork failed with exit code {}: {}",
            result.code,
            result.combined_trimmed()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_wifi_device() {
        let text = "Hardware Port: Wi-Fi\nDevice: en0\nEthernet Address: aa:bb\n";
        assert_eq!(parse_wifi_device(text).as_deref(), Some("en0"));
    }

    #[test]
    fn parses_the_iphone_usb_device() {
        let text = "Hardware Port: Wi-Fi\nDevice: en0\n\n\
                    Hardware Port: iPhone USB\nDevice: en7\n";
        assert_eq!(
            parse_hardware_device(text, &["iphone usb"]).as_deref(),
            Some("en7")
        );
    }

    #[test]
    fn reads_a_network_name() {
        assert_eq!(
            parse_ssid("Current Wi-Fi Network: Max's iPhone\n", "").as_deref(),
            Some("Max's iPhone")
        );
    }

    /// Both of these mean "no name to compare against", and neither means "offline".
    #[test]
    fn a_hidden_or_absent_name_is_no_name() {
        assert_eq!(parse_ssid("Current Wi-Fi Network: <redacted>\n", ""), None);
        assert_eq!(parse_ssid("Current Wi-Fi Network: \n", ""), None);
        assert_eq!(
            parse_ssid("You are not associated with an AirPort network.\n", ""),
            None
        );
    }

    #[test]
    fn parses_a_usb_tether_default_route() {
        let route = parse_default_route(
            "   route to: default\n\
             destination: default\n\
             gateway: link#24\n\
             interface: en7\n",
        );
        assert_eq!(route.interface.as_deref(), Some("en7"));
        assert_eq!(route.gateway.as_deref(), Some("link#24"));
    }

    /// A password must never reach the process table.
    #[test]
    fn the_join_command_carries_no_password() {
        let args = saved_wifi_connection_args("en0", "Noah's iPhone").unwrap();
        assert_eq!(args, vec!["-setairportnetwork", "en0", "Noah's iPhone"]);
    }

    #[test]
    fn the_power_command_names_the_device_and_the_state() {
        assert_eq!(
            wifi_power_args("en0", false).unwrap(),
            vec!["-setairportpower", "en0", "off"]
        );
        assert_eq!(
            wifi_power_args("en0", true).unwrap(),
            vec!["-setairportpower", "en0", "on"]
        );
    }

    #[test]
    fn the_power_command_rejects_a_bad_device() {
        assert!(wifi_power_args("", true).is_err());
        assert!(wifi_power_args("en0\nother", true).is_err());
    }

    #[test]
    fn the_join_command_rejects_control_characters() {
        assert!(saved_wifi_connection_args("en0", "Noah\nother").is_err());
        assert!(saved_wifi_connection_args("en0\tother", "Noah").is_err());
    }

    /// `networksetup` exits 0 even when it did not join, so the output is the real signal.
    #[test]
    fn a_join_that_only_looks_successful_is_rejected() {
        let refused = CommandResult {
            code: 0,
            stdout: "Could not find network Noah.\n".to_owned(),
            stderr: String::new(),
        };
        assert!(require_wifi_join_success(&refused)
            .unwrap_err()
            .to_string()
            .contains("Could not find network Noah."));

        let silent = CommandResult {
            code: 0,
            stdout: String::new(),
            stderr: String::new(),
        };
        assert!(require_wifi_join_success(&silent).is_ok());
    }

    /// Both real row widths, from a real Mac. A left-to-right parser reads `Ierrs` as `Ibytes` on
    /// the narrow one and reports a handful of bytes for a whole commute.
    #[test]
    fn reads_the_byte_columns_at_either_row_width() {
        let wide = "Name       Mtu   Network       Address            Ipkts Ierrs     Ibytes    Opkts Oerrs     Obytes  Coll\n\
                    en0        1500  <Link#12>   8a:b7:b8:89:4f:67 30128622     0 23308376472 29417243     0 26102435913     0\n";
        assert_eq!(
            parse_interface_traffic(wide),
            Some(InterfaceTraffic {
                bytes_in: 23_308_376_472,
                bytes_out: 26_102_435_913,
            })
        );

        let narrow = "Name       Mtu   Network       Address            Ipkts Ierrs     Ibytes    Opkts Oerrs     Obytes  Coll\n\
                      lo0        16384 <Link#1>                      25758690     0 12175663798 25758690     0 12175663798     0\n";
        assert_eq!(
            parse_interface_traffic(narrow),
            Some(InterfaceTraffic {
                bytes_in: 12_175_663_798,
                bytes_out: 12_175_663_798,
            })
        );
    }

    /// An unknown interface exits 0 with only a header. That is no answer, not zero bytes.
    #[test]
    fn no_link_row_is_no_answer() {
        let header = "Name       Mtu   Network       Address            Ipkts Ierrs     Ibytes    Opkts Oerrs     Obytes  Coll\n";
        assert_eq!(parse_interface_traffic(header), None);
        assert_eq!(parse_interface_traffic(""), None);
        assert_eq!(
            parse_interface_traffic("en0 1500 <Link#12> junk here\n"),
            None
        );
    }

    #[test]
    fn an_interface_name_never_reaches_argv_unchecked() {
        assert!(read_interface_traffic("en0; rm -rf /").is_err());
        assert!(read_interface_traffic("").is_err());
    }

    #[test]
    fn only_apples_own_probe_gets_the_strict_body_check() {
        assert!(is_apple_captive_probe(
            "http://captive.apple.com/hotspot-detect.html"
        ));
        assert!(!is_apple_captive_probe(
            "https://example.com/hotspot-detect.html"
        ));
        assert!(!is_apple_captive_probe("http://captive.apple.com/other"));
    }

    /// A captive portal that returns 200 with its own page is still offline.
    #[test]
    fn a_portal_page_is_not_success() {
        assert!(apple_captive_success_body(
            "<HTML><HEAD><TITLE>Success</TITLE></HEAD><BODY>Success</BODY></HTML>"
        ));
        assert!(!apple_captive_success_body(
            "<html><title>Sign in</title><body>Success</body></html>"
        ));
    }
}
