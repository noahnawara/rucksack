use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use rucksack_core::protocol::{
    HelperOperation, HelperRequest, HelperResponse, HelperStatus, DEFAULT_HELPER_SOCKET,
    HELPER_PROTOCOL_VERSION,
};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct HelperClient {
    socket: PathBuf,
}

#[derive(Debug)]
pub struct HelperCallError {
    error: anyhow::Error,
    request_may_have_been_processed: bool,
    acquire_state_is_ambiguous: bool,
    response_status: Option<HelperStatus>,
}

impl HelperCallError {
    fn before_send(error: anyhow::Error) -> Self {
        Self {
            error,
            request_may_have_been_processed: false,
            acquire_state_is_ambiguous: false,
            response_status: None,
        }
    }

    fn after_send(error: anyhow::Error) -> Self {
        Self {
            error,
            request_may_have_been_processed: true,
            acquire_state_is_ambiguous: true,
            response_status: None,
        }
    }

    fn response_error(error: anyhow::Error, response_status: Option<HelperStatus>) -> Self {
        Self {
            error,
            request_may_have_been_processed: true,
            acquire_state_is_ambiguous: false,
            response_status,
        }
    }

    fn ambiguous_response(error: anyhow::Error, response_status: Option<HelperStatus>) -> Self {
        Self {
            error,
            request_may_have_been_processed: true,
            acquire_state_is_ambiguous: true,
            response_status,
        }
    }

    pub fn acquire_needs_cleanup(&self, lease_id: Uuid) -> bool {
        if !self.request_may_have_been_processed {
            return false;
        }
        if self.acquire_state_is_ambiguous {
            return true;
        }
        match self.response_status.as_ref() {
            Some(status) if !status.active => false,
            Some(status)
                if status
                    .lease_id
                    .is_some_and(|active_lease| active_lease != lease_id) =>
            {
                false
            }
            _ => true,
        }
    }

    pub fn into_anyhow(self) -> anyhow::Error {
        self.error
    }
}

impl Default for HelperClient {
    fn default() -> Self {
        Self {
            socket: PathBuf::from(DEFAULT_HELPER_SOCKET),
        }
    }
}

impl HelperClient {
    #[allow(dead_code)]
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
    ) -> std::result::Result<HelperStatus, HelperCallError> {
        self.call_with_delivery_state(HelperOperation::Acquire {
            lease_id,
            ttl_seconds,
            hard_expires_at,
            reason: reason.into(),
        })?
        .ok_or_else(|| {
            HelperCallError::ambiguous_response(
                anyhow!("helper returned no status after acquiring a lease"),
                None,
            )
        })
    }

    pub fn renew(&self, lease_id: Uuid, ttl_seconds: u64) -> Result<HelperStatus> {
        self.call(HelperOperation::Renew {
            lease_id,
            ttl_seconds,
        })?
        .ok_or_else(|| anyhow!("helper returned no status after renewing a lease"))
    }

    pub fn reassert(&self, lease_id: Uuid) -> Result<HelperStatus> {
        self.call(HelperOperation::Reassert { lease_id })?
            .ok_or_else(|| anyhow!("helper returned no status after reasserting a lease"))
    }

    pub fn release(&self, lease_id: Uuid, reason: impl Into<String>) -> Result<HelperStatus> {
        self.call(HelperOperation::Release {
            lease_id,
            reason: reason.into(),
        })?
        .ok_or_else(|| anyhow!("helper returned no status after releasing a lease"))
    }

    pub fn recover(&self) -> Result<Option<HelperStatus>> {
        self.call(HelperOperation::Recover)
    }

    pub fn is_available(&self) -> bool {
        self.status().is_ok()
    }

    fn call(&self, operation: HelperOperation) -> Result<Option<HelperStatus>> {
        self.call_with_delivery_state(operation)
            .map_err(HelperCallError::into_anyhow)
    }

    fn call_with_delivery_state(
        &self,
        operation: HelperOperation,
    ) -> std::result::Result<Option<HelperStatus>, HelperCallError> {
        let request = HelperRequest::new(operation);
        let mut stream = UnixStream::connect(&self.socket)
            .with_context(|| format!("Could not connect to {}", self.socket.display()))
            .map_err(HelperCallError::before_send)?;
        stream
            .set_read_timeout(Some(Duration::from_secs(8)))
            .map_err(|error| HelperCallError::before_send(error.into()))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(8)))
            .map_err(|error| HelperCallError::before_send(error.into()))?;

        let mut encoded = serde_json::to_vec(&request)
            .map_err(|error| HelperCallError::before_send(error.into()))?;
        encoded.push(b'\n');
        stream
            .write_all(&encoded)
            .map_err(|error| HelperCallError::after_send(error.into()))?;
        stream
            .flush()
            .map_err(|error| HelperCallError::after_send(error.into()))?;

        let mut response_line = String::new();
        BufReader::new(stream)
            .read_line(&mut response_line)
            .map_err(|error| HelperCallError::after_send(error.into()))?;
        if response_line.len() > 256 * 1024 {
            return Err(HelperCallError::after_send(anyhow!(
                "helper response exceeded the size limit"
            )));
        }
        let response: HelperResponse = serde_json::from_str(&response_line)
            .context("The helper returned invalid JSON")
            .map_err(HelperCallError::after_send)?;
        if response.protocol != HELPER_PROTOCOL_VERSION {
            return Err(HelperCallError::ambiguous_response(
                anyhow!(
                    "helper response protocol {} did not match expected protocol {}",
                    response.protocol,
                    HELPER_PROTOCOL_VERSION
                ),
                response.status,
            ));
        }
        if response.request_id != request.request_id {
            return Err(HelperCallError::ambiguous_response(
                anyhow!("helper response request_id did not match"),
                response.status,
            ));
        }
        if !response.ok {
            return Err(HelperCallError::response_error(
                anyhow!(
                    "{}",
                    response
                        .error
                        .unwrap_or_else(|| "helper operation failed".to_owned())
                ),
                response.status,
            ));
        }
        Ok(response.status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rucksack_core::protocol::{HelperOperation, HelperRequest, HelperResponse};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::thread;
    use tempfile::tempdir;

    fn inactive_status() -> HelperStatus {
        HelperStatus {
            active: false,
            lease_id: None,
            owner_uid: None,
            created_at: None,
            expires_at: None,
            hard_expires_at: None,
            previous_sleep_disabled: None,
            sleep_disabled: Some(0),
            reason: None,
            last_reasserted_at: None,
        }
    }

    fn active_status(lease_id: Option<Uuid>) -> HelperStatus {
        HelperStatus {
            active: true,
            lease_id,
            owner_uid: Some(501),
            created_at: None,
            expires_at: None,
            hard_expires_at: None,
            previous_sleep_disabled: Some(0),
            sleep_disabled: Some(1),
            reason: None,
            last_reasserted_at: None,
        }
    }

    #[test]
    fn acquire_marks_a_lost_response_as_ambiguous() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("helper.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let lease_id = Uuid::new_v4();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut request_line = String::new();
            BufReader::new(stream).read_line(&mut request_line).unwrap();
            let request: HelperRequest = serde_json::from_str(&request_line).unwrap();
            assert!(matches!(
                request.operation,
                HelperOperation::Acquire {
                    lease_id: requested_lease_id,
                    ..
                } if requested_lease_id == lease_id
            ));
        });

        let error = HelperClient::new(&socket)
            .acquire(
                lease_id,
                60,
                Utc::now() + chrono::Duration::minutes(5),
                "test",
            )
            .unwrap_err();

        assert!(error.acquire_needs_cleanup(lease_id));
        server.join().unwrap();
    }

    #[test]
    fn acquire_rejection_with_an_inactive_status_needs_no_cleanup() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("helper.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let lease_id = Uuid::new_v4();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request_line)
                .unwrap();
            let request: HelperRequest = serde_json::from_str(&request_line).unwrap();
            let response = HelperResponse::failure(
                request.request_id,
                "acquire rejected",
                Some(inactive_status()),
            );
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            stream.write_all(&bytes).unwrap();
            stream.flush().unwrap();
        });

        let error = HelperClient::new(&socket)
            .acquire(
                lease_id,
                60,
                Utc::now() + chrono::Duration::minutes(5),
                "test",
            )
            .unwrap_err();

        assert!(!error.acquire_needs_cleanup(lease_id));
        server.join().unwrap();
    }

    #[test]
    fn acquire_rejection_without_status_needs_cleanup() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("helper.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let lease_id = Uuid::new_v4();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request_line)
                .unwrap();
            let request: HelperRequest = serde_json::from_str(&request_line).unwrap();
            let response = HelperResponse::failure(request.request_id, "acquire rejected", None);
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            stream.write_all(&bytes).unwrap();
            stream.flush().unwrap();
        });

        let error = HelperClient::new(&socket)
            .acquire(
                lease_id,
                60,
                Utc::now() + chrono::Duration::minutes(5),
                "test",
            )
            .unwrap_err();

        assert!(error.acquire_needs_cleanup(lease_id));
        server.join().unwrap();
    }

    #[test]
    fn processed_acquire_rejection_only_skips_cleanup_for_authoritative_status() {
        let candidate_lease = Uuid::new_v4();
        let other_lease = Uuid::new_v4();

        assert!(!HelperCallError::response_error(
            anyhow!("acquire rejected"),
            Some(inactive_status())
        )
        .acquire_needs_cleanup(candidate_lease));
        assert!(!HelperCallError::response_error(
            anyhow!("acquire rejected"),
            Some(active_status(Some(other_lease))),
        )
        .acquire_needs_cleanup(candidate_lease));
        assert!(HelperCallError::response_error(
            anyhow!("acquire rejected"),
            Some(active_status(Some(candidate_lease))),
        )
        .acquire_needs_cleanup(candidate_lease));
        assert!(HelperCallError::response_error(
            anyhow!("acquire rejected"),
            Some(active_status(None)),
        )
        .acquire_needs_cleanup(candidate_lease));
    }

    #[test]
    fn acquire_success_without_status_needs_cleanup() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("helper.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let lease_id = Uuid::new_v4();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request_line)
                .unwrap();
            let request: HelperRequest = serde_json::from_str(&request_line).unwrap();
            let response = HelperResponse::success(request.request_id, None);
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            stream.write_all(&bytes).unwrap();
            stream.flush().unwrap();
        });

        let error = HelperClient::new(&socket)
            .acquire(
                lease_id,
                60,
                Utc::now() + chrono::Duration::minutes(5),
                "test",
            )
            .unwrap_err();

        assert!(error.acquire_needs_cleanup(lease_id));
        server.join().unwrap();
    }

    #[test]
    fn acquire_rejects_a_mismatched_protocol_and_preserves_its_status() {
        let directory = tempdir().unwrap();
        let socket = directory.path().join("helper.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let lease_id = Uuid::new_v4();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request_line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request_line)
                .unwrap();
            let request: HelperRequest = serde_json::from_str(&request_line).unwrap();
            let mut response = HelperResponse::failure(
                request.request_id,
                "acquire rejected",
                Some(inactive_status()),
            );
            response.protocol = HELPER_PROTOCOL_VERSION + 1;
            let mut bytes = serde_json::to_vec(&response).unwrap();
            bytes.push(b'\n');
            stream.write_all(&bytes).unwrap();
            stream.flush().unwrap();
        });

        let error = HelperClient::new(&socket)
            .acquire(
                lease_id,
                60,
                Utc::now() + chrono::Duration::minutes(5),
                "test",
            )
            .unwrap_err();

        assert!(error.acquire_needs_cleanup(lease_id));
        assert_eq!(
            error
                .response_status
                .as_ref()
                .and_then(|status| status.sleep_disabled),
            Some(0)
        );
        server.join().unwrap();
    }

    #[test]
    fn acquire_marks_a_connection_failure_as_not_sent() {
        let directory = tempdir().unwrap();
        let lease_id = Uuid::new_v4();
        let error = HelperClient::new(directory.path().join("missing.sock"))
            .acquire(
                lease_id,
                60,
                Utc::now() + chrono::Duration::minutes(5),
                "test",
            )
            .unwrap_err();

        assert!(!error.acquire_needs_cleanup(lease_id));
    }
}
