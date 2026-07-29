use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use rucksack_core::protocol::{
    HelperOperation, HelperRequest, HelperResponse, HelperStatus, DEFAULT_HELPER_SOCKET,
    HELPER_PROTOCOL_VERSION,
};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use uuid::Uuid;

const CALL_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// Talks to the root helper over its Unix socket.
///
/// Every call is one request and one response, and every failure is a plain `anyhow::Error`.
/// Callers never need to distinguish "the request never arrived" from "the reply was lost":
/// `pack` registers its rollback before acquiring anything, and `unpack` escalates through
/// release, then recover, then asking macOS directly.
#[derive(Debug, Clone)]
pub struct HelperClient {
    socket: PathBuf,
}

impl Default for HelperClient {
    fn default() -> Self {
        Self {
            socket: PathBuf::from(DEFAULT_HELPER_SOCKET),
        }
    }
}

impl HelperClient {
    #[cfg(test)]
    pub fn new(socket: impl AsRef<Path>) -> Self {
        Self {
            socket: socket.as_ref().to_path_buf(),
        }
    }

    pub fn status(&self) -> Result<Option<HelperStatus>> {
        self.call(HelperOperation::Status)
    }

    pub fn acquire(
        &self,
        lease_id: Uuid,
        ttl_seconds: u64,
        hard_expires_at: DateTime<Utc>,
        reason: impl Into<String>,
    ) -> Result<HelperStatus> {
        self.call(HelperOperation::Acquire {
            lease_id,
            ttl_seconds,
            hard_expires_at,
            reason: reason.into(),
        })?
        .ok_or_else(|| anyhow!("The helper acquired a lease but reported no status."))
    }

    pub fn renew(&self, lease_id: Uuid, ttl_seconds: u64) -> Result<HelperStatus> {
        self.call(HelperOperation::Renew {
            lease_id,
            ttl_seconds,
        })?
        .ok_or_else(|| anyhow!("The helper renewed a lease but reported no status."))
    }

    pub fn release(&self, lease_id: Uuid, reason: impl Into<String>) -> Result<HelperStatus> {
        self.call(HelperOperation::Release {
            lease_id,
            reason: reason.into(),
        })?
        .ok_or_else(|| anyhow!("The helper released a lease but reported no status."))
    }

    pub fn recover(&self) -> Result<Option<HelperStatus>> {
        self.call(HelperOperation::Recover)
    }

    fn call(&self, operation: HelperOperation) -> Result<Option<HelperStatus>> {
        let request = HelperRequest::new(operation);
        let mut stream = UnixStream::connect(&self.socket)
            .with_context(|| format!("Could not reach the helper at {}", self.socket.display()))?;
        stream.set_read_timeout(Some(CALL_TIMEOUT))?;
        stream.set_write_timeout(Some(CALL_TIMEOUT))?;

        let mut encoded = serde_json::to_vec(&request)?;
        encoded.push(b'\n');
        stream.write_all(&encoded)?;
        stream.flush()?;

        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        if line.len() > MAX_RESPONSE_BYTES {
            anyhow::bail!("The helper reply was too large to trust.");
        }
        // A closed connection is not malformed JSON, and saying so sent people looking in the wrong
        // place. The helper hangs up without replying when it refuses the caller — most often a
        // `cargo install`ed rucksack talking to a notarized helper that authenticates by code
        // signature — and "invalid JSON" describes none of that.
        if line.trim().is_empty() {
            anyhow::bail!(
                "The power helper closed the connection without replying.\nIt may be refusing this copy of rucksack: run `rucksack helper install` so the installed helper matches this build."
            );
        }
        let response: HelperResponse =
            serde_json::from_str(&line).context("The helper returned invalid JSON.")?;
        if response.protocol != HELPER_PROTOCOL_VERSION {
            anyhow::bail!(
                "The helper speaks protocol {} and rucksack speaks {HELPER_PROTOCOL_VERSION}. Reinstall the helper.",
                response.protocol
            );
        }
        if response.request_id != request.request_id {
            anyhow::bail!("The helper answered a different request.");
        }
        if !response.ok {
            anyhow::bail!(
                "{}",
                response
                    .error
                    .unwrap_or_else(|| "The helper refused the request.".to_owned())
            );
        }
        Ok(response.status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::thread;
    use tempfile::tempdir;

    fn status() -> HelperStatus {
        HelperStatus {
            version: None,
            active: false,
            lease_id: None,
            owner_uid: None,
            created_at: None,
            expires_at: None,
            hard_expires_at: None,
            previous_sleep_disabled: None,
            sleep_disabled: Some(0),
            reason: None,
        }
    }

    /// Stand up a fake helper that answers one request with whatever `reply` builds.
    fn fake_helper(
        reply: impl Fn(&HelperRequest) -> HelperResponse + Send + 'static,
    ) -> (tempfile::TempDir, PathBuf) {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("helper.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            let request: HelperRequest = serde_json::from_str(&line).unwrap();
            let mut encoded = serde_json::to_vec(&reply(&request)).unwrap();
            encoded.push(b'\n');
            stream.write_all(&encoded).unwrap();
            stream.flush().unwrap();
        });
        (directory, socket)
    }

    fn ok_response(request: &HelperRequest) -> HelperResponse {
        HelperResponse {
            protocol: HELPER_PROTOCOL_VERSION,
            request_id: request.request_id,
            ok: true,
            error: None,
            status: Some(status()),
        }
    }

    #[test]
    fn a_successful_call_returns_the_status() {
        let (_directory, socket) = fake_helper(ok_response);

        let reported = HelperClient::new(&socket).status().unwrap();

        assert_eq!(reported.map(|status| status.active), Some(false));
    }

    #[test]
    fn an_unreachable_helper_says_so() {
        let directory = tempdir().unwrap();

        let error = HelperClient::new(directory.path().join("absent.sock"))
            .status()
            .unwrap_err();

        assert!(error.to_string().contains("Could not reach the helper"));
    }

    #[test]
    fn a_refusal_surfaces_the_helpers_own_reason() {
        let (_directory, socket) = fake_helper(|request| HelperResponse {
            protocol: HELPER_PROTOCOL_VERSION,
            request_id: request.request_id,
            ok: false,
            error: Some("another user owns the lease".to_owned()),
            status: None,
        });

        let error = HelperClient::new(&socket).recover().unwrap_err();

        assert!(error.to_string().contains("another user owns the lease"));
    }

    /// A helper from a different release must never be interpreted.
    #[test]
    fn a_protocol_mismatch_is_refused() {
        let (_directory, socket) = fake_helper(|request| HelperResponse {
            protocol: HELPER_PROTOCOL_VERSION + 1,
            request_id: request.request_id,
            ok: true,
            error: None,
            status: Some(status()),
        });

        let error = HelperClient::new(&socket).status().unwrap_err();

        assert!(error.to_string().contains("Reinstall the helper"));
    }

    #[test]
    fn a_reply_to_a_different_request_is_refused() {
        let (_directory, socket) = fake_helper(|_| HelperResponse {
            protocol: HELPER_PROTOCOL_VERSION,
            request_id: Uuid::new_v4(),
            ok: true,
            error: None,
            status: Some(status()),
        });

        let error = HelperClient::new(&socket).status().unwrap_err();

        assert!(error.to_string().contains("different request"));
    }

    #[test]
    fn acquire_insists_on_a_status() {
        let (_directory, socket) = fake_helper(|request| HelperResponse {
            protocol: HELPER_PROTOCOL_VERSION,
            request_id: request.request_id,
            ok: true,
            error: None,
            status: None,
        });

        let error = HelperClient::new(&socket)
            .acquire(Uuid::new_v4(), 90, Utc::now(), "test")
            .unwrap_err();

        assert!(error.to_string().contains("reported no status"));
    }
}
