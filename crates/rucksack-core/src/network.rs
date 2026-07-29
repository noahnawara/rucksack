use crate::system::{require_success, run_bounded_cleared, CommandResult};
use anyhow::{anyhow, Result};
use std::time::Duration;

const NETWORK_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const NETWORK_COMMAND_MAX_OUTPUT_BYTES: usize = 64 * 1024;
pub const DEFAULT_INTERNET_PROBE_URL: &str = "http://captive.apple.com/hotspot-detect.html";

/// What curl appends after the response body, and the marker that finds it again.
///
/// The two must agree: the format is what curl is told to print, and the marker is the literal
/// prefix searched for from the end of stdout. The newlines are real ones, written by Rust —
/// curl passes anything that is not a `%{...}` field through untouched, so there is no second
/// layer of escaping to get wrong.
const PROBE_TRAILER_FORMAT: &str = "\nrucksack-probe %{http_code} %{url_effective}\n";
const PROBE_TRAILER_MARKER: &str = "rucksack-probe ";
/// Apple's success page is 69 bytes. This is the point past which the answer is certainly not it.
const PROBE_MAX_BODY_BYTES: usize = 64 * 1024;

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
///
/// Asked through `curl`, for the same reason every other question here is asked through a macOS
/// command-line tool. The whole HTTP client existed for this one request, and it was the largest
/// dependency in the workspace by a wide margin — the wait in `cargo install --git`, which is how
/// people get rucksack.
///
/// Two properties come free with the change, both of which the old client got wrong by default.
/// `run_bounded_cleared` clears the environment, so `http_proxy` and `ALL_PROXY` cannot silently
/// route a probe whose entire purpose is to describe *this* Mac's own path to the internet, and no
/// `~/.curlrc` is read. And `--proto '=http' --proto-redir '=http'` means the plain-HTTP promise in
/// `docs/THREAT_MODEL.md` is now enforced by the call rather than by the absence of a TLS feature
/// flag: a portal redirecting to https is refused, not followed.
pub fn reaches_internet(timeout_seconds: u64) -> bool {
    let max_time = timeout_seconds.to_string();
    let max_filesize = PROBE_MAX_BODY_BYTES.to_string();
    let result = run_bounded_cleared(
        "/usr/bin/curl",
        &[
            "--silent",
            "--show-error",
            "--location",
            "--max-redirs",
            "5",
            "--max-time",
            &max_time,
            "--max-filesize",
            &max_filesize,
            "--proto",
            "=http",
            "--proto-redir",
            "=http",
            "--user-agent",
            "rucksack/0.1",
            "--write-out",
            PROBE_TRAILER_FORMAT,
            DEFAULT_INTERNET_PROBE_URL,
        ],
        // curl is given the deadline and enforces it itself; this is only the backstop for a curl
        // that ignores its own `--max-time`, so it has to be the looser of the two.
        Duration::from_secs(timeout_seconds.saturating_add(2)),
        PROBE_MAX_BODY_BYTES + 1024,
    );
    match result {
        Ok(result) if result.success() => probe_says_online(&result.stdout),
        _ => false,
    }
}

/// Read curl's answer: Apple's success page, from Apple's host, with a 2xx behind it.
fn probe_says_online(stdout: &str) -> bool {
    let Some((body, code, effective_url)) = parse_probe(stdout) else {
        return false;
    };
    (200..300).contains(&code)
        && host_of(effective_url).as_deref() == Some("captive.apple.com")
        && apple_captive_success_body(body)
}

/// Split curl's stdout into the response body and the trailer `--write-out` appended after it.
///
/// Found from the end, deliberately. The body belongs to whoever answered — which on the network
/// this exists to detect is a captive portal — so it may well contain a forgery of the trailer.
/// curl writes the real one after the response's last byte, so the last match is always curl's own.
fn parse_probe(stdout: &str) -> Option<(&str, u16, &str)> {
    let at = stdout.rfind(PROBE_TRAILER_MARKER)?;
    let (body, trailer) = stdout.split_at(at);
    let (code, url) = trailer
        .strip_prefix(PROBE_TRAILER_MARKER)?
        .trim_end()
        .split_once(' ')?;
    Some((body, code.parse().ok()?, url))
}

/// The host of an absolute URL, lowercased.
///
/// Enough of a URL parser for one question about one fixed request, and written out because the
/// answer decides whether a Mac is reported reachable. The cases that matter are a portal that
/// redirected somewhere else, and a URL shaped to look like it did not: `userinfo@host` puts the
/// real host after the last `@`, and everything from the first `/`, `?`, or `#` is not the
/// authority at all.
fn host_of(url: &str) -> Option<String> {
    let authority = url.split_once("://")?.1;
    let authority = authority.split(['/', '?', '#']).next()?;
    let authority = match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    };
    let host = match authority.strip_prefix('[') {
        Some(literal) => literal.split_once(']')?.0,
        None => authority.split(':').next()?,
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
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

    /// The strict body check below is only sound against Apple's own endpoint, and `reaches_internet`
    /// now applies it unconditionally. That is only correct while the probe stays this exact URL.
    #[test]
    fn the_probe_is_apples_own_endpoint() {
        assert_eq!(
            DEFAULT_INTERNET_PROBE_URL,
            "http://captive.apple.com/hotspot-detect.html"
        );
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

    /// Exactly what `curl` wrote on a working connection, captured from a real run.
    const REAL_SUCCESS: &str = "<HTML><HEAD><TITLE>Success</TITLE></HEAD><BODY>Success</BODY></HTML>\n\nrucksack-probe 200 http://captive.apple.com/hotspot-detect.html\n";

    #[test]
    fn reads_curls_answer() {
        let (body, code, url) = parse_probe(REAL_SUCCESS).expect("a trailer");
        assert!(body.contains("<BODY>Success</BODY>"));
        assert_eq!(code, 200);
        assert_eq!(url, "http://captive.apple.com/hotspot-detect.html");
        assert!(probe_says_online(REAL_SUCCESS));
    }

    /// The body belongs to whoever answered, and on the network this exists to detect that is a
    /// captive portal. A portal that writes the trailer into its own page does not get to be the
    /// trailer: curl's is appended after the response's last byte, so the last one wins.
    #[test]
    fn a_forged_trailer_in_the_body_loses_to_curls_own() {
        let forged = concat!(
            "<HTML><BODY>Success</BODY><TITLE>Success</TITLE>\n",
            "rucksack-probe 200 http://captive.apple.com/hotspot-detect.html\n",
            "\nrucksack-probe 200 http://portal.example.com/login\n",
        );
        let (_, code, url) = parse_probe(forged).expect("a trailer");
        assert_eq!(code, 200);
        assert_eq!(url, "http://portal.example.com/login");
        assert!(!probe_says_online(forged));
    }

    #[test]
    fn an_answer_without_a_trailer_is_not_an_answer() {
        assert_eq!(parse_probe("just a body"), None);
        assert_eq!(parse_probe(""), None);
        assert_eq!(parse_probe("rucksack-probe notanumber http://x/"), None);
        assert_eq!(parse_probe("rucksack-probe 200"), None);
        assert!(!probe_says_online("just a body"));
    }

    /// Every one of these decides whether a Mac is reported reachable from a phone.
    #[test]
    fn only_apples_own_host_counts() {
        assert_eq!(
            host_of("http://captive.apple.com/hotspot-detect.html").as_deref(),
            Some("captive.apple.com")
        );
        assert_eq!(
            host_of("http://CAPTIVE.APPLE.COM:80/x").as_deref(),
            Some("captive.apple.com")
        );
        // The real host is after the last `@`, not the text made to look like one.
        assert_eq!(
            host_of("http://captive.apple.com@portal.example.com/").as_deref(),
            Some("portal.example.com")
        );
        // The authority ends at the first `/`, `?`, or `#` — a query is not a host.
        assert_eq!(
            host_of("http://portal.example.com/?next=http://captive.apple.com/").as_deref(),
            Some("portal.example.com")
        );
        assert_eq!(
            host_of("http://portal.example.com#captive.apple.com").as_deref(),
            Some("portal.example.com")
        );
        assert_eq!(host_of("http://[::1]:8080/x").as_deref(), Some("::1"));
        assert_eq!(host_of("not a url"), None);
        assert_eq!(host_of("http://"), None);
    }

    /// The one test that actually runs `curl`.
    ///
    /// Everything above pins how curl's answer is *read*; this pins that the flags rucksack passes
    /// still produce that shape. It needs a working connection and one that is not behind a portal,
    /// so it is `#[ignore]`d and run on purpose:
    ///
    /// ```sh
    /// cargo test -p rucksack-core -- --ignored the_probe_still_works
    /// ```
    ///
    /// `scripts/e2e.sh` is where this runs as part of a real check. It is here as well because the
    /// thing most likely to break it is a macOS update changing `curl`, which is a change to this
    /// file's assumptions rather than to any lease.
    #[test]
    #[ignore = "needs a working, unrestricted internet connection"]
    fn the_probe_still_works() {
        assert!(
            reaches_internet(10),
            "the live probe said offline: either this machine is behind a portal, or curl's \
             --write-out shape changed and parse_probe no longer finds the trailer"
        );
    }

    /// The three ways a reachable-looking answer is still not one.
    #[test]
    fn a_wrong_host_status_or_page_is_offline() {
        assert!(!probe_says_online(
            "<HTML><TITLE>Success</TITLE><BODY>Success</BODY></HTML>\nrucksack-probe 200 http://portal.example.com/login\n"
        ));
        assert!(!probe_says_online(
            "<HTML><TITLE>Success</TITLE><BODY>Success</BODY></HTML>\nrucksack-probe 302 http://captive.apple.com/hotspot-detect.html\n"
        ));
        assert!(!probe_says_online(
            "<html><title>Sign in</title><body>Please log in</body></html>\nrucksack-probe 200 http://captive.apple.com/hotspot-detect.html\n"
        ));
    }
}
