use crate::lease::LeaseManager;
use crate::power_events;
use anyhow::{anyhow, Context, Result};
use rucksack_core::files::ensure_private_dir;
use rucksack_core::protocol::{
    HelperRequest, HelperResponse, DEFAULT_HELPER_SOCKET, HELPER_PROTOCOL_VERSION,
};
#[cfg(target_os = "macos")]
use security_framework::os::macos::code_signing::{
    Flags, GuestAttributes, SecCode, SecRequirement,
};
use std::ffi::CString;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(target_os = "macos")]
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const STATE_PATH: &str = "/var/db/rucksack/helper-state.json";
const MAX_REQUEST_BYTES: u64 = 256 * 1024;
const MAX_CONCURRENT_CONNECTIONS: usize = 32;
const CONNECTION_IO_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(any(test, target_os = "macos"))]
const CLIENT_SIGNING_IDENTIFIER: &str = "io.rucksack.cli";
#[cfg(any(test, target_os = "macos"))]
const APPLE_TEAM_ID_LENGTH: usize = 10;

/// The Apple Developer team whose signed `rucksack` this helper will talk to.
///
/// Set when the notarized package is built, absent when someone builds from source. Signed builds
/// therefore verify the calling binary's code signature; source builds authenticate by UID alone,
/// which is what makes `cargo build --release` usable at all. Either way the socket is root:admin
/// 0660 and every lease is owner-checked.
#[cfg(target_os = "macos")]
const COMPILED_TEAM_ID: Option<&str> = option_env!("RUCKSACK_TEAM_ID");

struct ConnectionPermit {
    active_connections: Arc<AtomicUsize>,
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.active_connections.fetch_sub(1, Ordering::AcqRel);
    }
}

pub fn run() -> Result<()> {
    if !cfg!(target_os = "macos") {
        anyhow::bail!("rucksack-helper supports macOS only");
    }
    if unsafe { libc::geteuid() } != 0 {
        anyhow::bail!("rucksack-helper must run as root");
    }

    ensure_private_dir(Path::new("/var/db/rucksack"))?;
    let manager = Arc::new(Mutex::new(LeaseManager::open(STATE_PATH)?));
    validate_client_authentication_configuration()?;

    let _power_observer = start_power_observer(manager.clone())?;
    let _watchdog = start_watchdog(manager.clone())?;

    // Create the socket with its final 0660 ceiling; secure_socket then sets the
    // root:admin owner atomically enough for local launch-daemon use.
    unsafe {
        libc::umask(0o117);
    }
    let socket = Path::new(DEFAULT_HELPER_SOCKET);
    if socket.exists() {
        fs::remove_file(socket)
            .with_context(|| format!("Could not remove stale socket {}", socket.display()))?;
    }

    let listener = UnixListener::bind(socket)
        .with_context(|| format!("Could not bind {}", socket.display()))?;
    secure_socket(socket)?;

    let active_connections = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let Some(permit) = try_acquire_connection(&active_connections) else {
                    eprintln!(
                        "rucksack-helper request rejected: maximum concurrent connection limit reached"
                    );
                    continue;
                };
                let manager = manager.clone();
                if let Err(error) = thread::Builder::new()
                    .name("rucksack-helper-request".to_owned())
                    .spawn(move || {
                        let _permit = permit;
                        if let Err(error) = handle_connection(stream, manager) {
                            eprintln!("rucksack-helper request failed: {error:#}");
                        }
                    })
                {
                    eprintln!("rucksack-helper could not start request worker: {error}");
                }
            }
            Err(error) => eprintln!("rucksack-helper accept failed: {error}"),
        }
    }
    Ok(())
}

fn handle_connection(stream: UnixStream, manager: Arc<Mutex<LeaseManager>>) -> Result<()> {
    stream
        .set_read_timeout(Some(CONNECTION_IO_TIMEOUT))
        .context("Could not set helper connection read timeout")?;
    stream
        .set_write_timeout(Some(CONNECTION_IO_TIMEOUT))
        .context("Could not set helper connection write timeout")?;
    let caller_uid = authenticate_peer(&stream)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader
        .by_ref()
        .take(MAX_REQUEST_BYTES + 1)
        .read_line(&mut line)?;
    if line.len() as u64 > MAX_REQUEST_BYTES {
        anyhow::bail!("request exceeded 256 KiB");
    }

    let request: HelperRequest =
        serde_json::from_str(&line).context("Request was not valid helper JSON")?;
    let response = if request.protocol != HELPER_PROTOCOL_VERSION {
        HelperResponse::failure(
            request.request_id,
            format!(
                "unsupported protocol {}; expected {}",
                request.protocol, HELPER_PROTOCOL_VERSION
            ),
            None,
        )
    } else {
        let mut manager = manager
            .lock()
            .map_err(|_| anyhow!("lease mutex poisoned"))?;
        match manager.handle(caller_uid, request.operation) {
            Ok(status) => HelperResponse::success(request.request_id, Some(status)),
            Err(error) => {
                let status = manager.status().ok();
                HelperResponse::failure(request.request_id, error.to_string(), status)
            }
        }
    };

    let mut writer = stream;
    let mut bytes = serde_json::to_vec(&response)?;
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

fn try_acquire_connection(active_connections: &Arc<AtomicUsize>) -> Option<ConnectionPermit> {
    active_connections
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
            if active < MAX_CONCURRENT_CONNECTIONS {
                Some(active + 1)
            } else {
                None
            }
        })
        .ok()?;
    Some(ConnectionPermit {
        active_connections: active_connections.clone(),
    })
}

fn start_watchdog(manager: Arc<Mutex<LeaseManager>>) -> Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("rucksack-helper-watchdog".to_owned())
        .spawn(move || loop {
            thread::sleep(Duration::from_secs(5));
            let result = manager
                .lock()
                .map_err(|_| anyhow!("lease mutex poisoned"))
                .and_then(|mut manager| manager.watchdog_tick());
            if let Err(error) = result {
                eprintln!("rucksack-helper watchdog: {error:#}");
            }
        })
        .context("Could not start helper watchdog")
}

fn start_power_observer(
    manager: Arc<Mutex<LeaseManager>>,
) -> Result<(JoinHandle<()>, JoinHandle<()>)> {
    let (sender, receiver) = mpsc::channel();
    let listener = power_events::spawn(sender).context("Could not start power-source observer")?;
    let worker = thread::Builder::new()
        .name("rucksack-helper-power-events".to_owned())
        .spawn(move || {
            while receiver.recv().is_ok() {
                let result = manager
                    .lock()
                    .map_err(|_| anyhow!("lease mutex poisoned"))
                    .and_then(|mut manager| manager.power_source_changed());
                if let Err(error) = result {
                    eprintln!("rucksack-helper power event: {error:#}");
                }
            }
            eprintln!(
                "rucksack-helper power observer stopped unexpectedly; exiting so launchd recovery restores normal sleep"
            );
            std::process::exit(1);
        })
        .context("Could not start power-event worker")?;
    Ok((listener, worker))
}

#[cfg(target_os = "macos")]
fn authenticate_peer(stream: &UnixStream) -> Result<u32> {
    let uid = peer_uid(stream)?;
    if let Some(team_id) = COMPILED_TEAM_ID {
        validate_peer_code_signature(peer_pid(stream)?, team_id)?;
    }
    Ok(uid)
}

#[cfg(not(target_os = "macos"))]
fn authenticate_peer(_stream: &UnixStream) -> Result<u32> {
    anyhow::bail!("peer authentication is implemented only on macOS")
}

#[cfg(target_os = "macos")]
fn peer_uid(stream: &UnixStream) -> Result<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if result == 0 {
        Ok(uid)
    } else {
        Err(std::io::Error::last_os_error()).context("getpeereid failed")
    }
}

#[cfg(target_os = "macos")]
fn peer_pid(stream: &UnixStream) -> Result<libc::pid_t> {
    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            std::ptr::addr_of_mut!(pid).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("LOCAL_PEERPID failed");
    }
    if length as usize != std::mem::size_of::<libc::pid_t>() {
        anyhow::bail!(
            "LOCAL_PEERPID returned {} bytes; expected {}",
            length,
            std::mem::size_of::<libc::pid_t>()
        );
    }
    if pid <= 0 {
        anyhow::bail!("LOCAL_PEERPID returned invalid PID {pid}");
    }
    Ok(pid)
}

#[cfg(target_os = "macos")]
fn validate_peer_code_signature(pid: libc::pid_t, team_id: &str) -> Result<()> {
    let requirement_text = client_code_requirement(team_id)?;
    let requirement: SecRequirement = requirement_text
        .parse()
        .context("Could not compile the rucksack client code requirement")?;
    let mut attributes = GuestAttributes::new();
    attributes.set_pid(pid);
    let code = SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE)
        .with_context(|| format!("Could not resolve the code signature for peer PID {pid}"))?;
    code.check_validity(Flags::NO_NETWORK_ACCESS, &requirement)
        .with_context(|| {
            format!(
                "Rejected peer PID {pid}: code signature must identify {CLIENT_SIGNING_IDENTIFIER} from Apple Developer team {team_id}"
            )
        })
}

#[cfg(any(test, target_os = "macos"))]
fn validate_team_id(team_id: &str) -> Result<()> {
    if team_id.len() != APPLE_TEAM_ID_LENGTH
        || !team_id
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        anyhow::bail!(
            "RUCKSACK_TEAM_ID must contain exactly {APPLE_TEAM_ID_LENGTH} uppercase ASCII letters or digits"
        );
    }
    Ok(())
}

#[cfg(any(test, target_os = "macos"))]
fn client_code_requirement(team_id: &str) -> Result<String> {
    validate_team_id(team_id)?;
    Ok(format!(
        "identifier \"{CLIENT_SIGNING_IDENTIFIER}\" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"{team_id}\""
    ))
}

#[cfg(target_os = "macos")]
fn validate_client_authentication_configuration() -> Result<()> {
    match COMPILED_TEAM_ID {
        Some(team_id) => validate_team_id(team_id),
        None => {
            eprintln!(
                "rucksack-helper: built without RUCKSACK_TEAM_ID, so clients are authenticated by user ID alone. Do not distribute this binary."
            );
            Ok(())
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn validate_client_authentication_configuration() -> Result<()> {
    anyhow::bail!("client authentication is implemented only on macOS")
}

fn secure_socket(path: &Path) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o660))?;
    let group = CString::new("admin")?;
    let path_c = CString::new(path.as_os_str().as_encoded_bytes())?;
    unsafe {
        let group_entry = libc::getgrnam(group.as_ptr());
        if group_entry.is_null() {
            anyhow::bail!("Could not resolve the macOS admin group");
        }
        let gid = (*group_entry).gr_gid;
        if libc::chown(path_c.as_ptr(), 0, gid) != 0 {
            return Err(std::io::Error::last_os_error()).context("Could not chown helper socket");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_limit_is_bounded_and_released() {
        let active_connections = Arc::new(AtomicUsize::new(0));
        let permits = (0..MAX_CONCURRENT_CONNECTIONS)
            .map(|_| try_acquire_connection(&active_connections).unwrap())
            .collect::<Vec<_>>();

        assert!(try_acquire_connection(&active_connections).is_none());
        drop(permits);
        assert!(try_acquire_connection(&active_connections).is_some());
    }

    #[test]
    fn accepts_valid_apple_team_identifier() {
        validate_team_id("A1B2C3D4E5").unwrap();
    }

    #[test]
    fn rejects_team_identifier_injection() {
        let error = client_code_requirement("TEAM\" or true").unwrap_err();
        assert!(error.to_string().contains("uppercase ASCII"));
    }

    #[test]
    fn client_requirement_pins_identifier_team_and_developer_id() {
        let requirement = client_code_requirement("A1B2C3D4E5").unwrap();

        assert!(requirement.contains("identifier \"io.rucksack.cli\""));
        assert!(requirement.contains("1.2.840.113635.100.6.1.13"));
        assert!(requirement.contains("subject.OU] = \"A1B2C3D4E5\""));

        #[cfg(target_os = "macos")]
        requirement
            .parse::<security_framework::os::macos::code_signing::SecRequirement>()
            .unwrap();
    }

    #[cfg(all(target_os = "macos", not(debug_assertions)))]
    #[test]
    fn release_authentication_reads_peer_pid_and_rejects_wrong_signature() {
        let (client, _server) = UnixStream::pair().unwrap();
        let pid = peer_pid(&client).unwrap();

        assert_eq!(u32::try_from(pid).unwrap(), std::process::id());
        let error = validate_peer_code_signature(pid, "A1B2C3D4E5").unwrap_err();
        let error_chain = format!("{error:#}");
        assert!(
            error_chain.contains("Rejected peer PID"),
            "unexpected validation error: {error_chain}"
        );
    }
}
