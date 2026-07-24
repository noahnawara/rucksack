use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use rucksack_core::protocol::{
    HelperOperation, HelperRequest, HelperResponse, HelperStatus, DEFAULT_HELPER_SOCKET,
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
    ) -> Result<HelperStatus> {
        self.call(HelperOperation::Acquire {
            lease_id,
            ttl_seconds,
            hard_expires_at,
            reason: reason.into(),
        })?
        .ok_or_else(|| anyhow!("helper returned no status after acquiring a lease"))
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
        let request = HelperRequest::new(operation);
        let mut stream = UnixStream::connect(&self.socket)
            .with_context(|| format!("Could not connect to {}", self.socket.display()))?;
        stream.set_read_timeout(Some(Duration::from_secs(8)))?;
        stream.set_write_timeout(Some(Duration::from_secs(8)))?;

        let mut encoded = serde_json::to_vec(&request)?;
        encoded.push(b'\n');
        stream.write_all(&encoded)?;
        stream.flush()?;

        let mut response_line = String::new();
        BufReader::new(stream).read_line(&mut response_line)?;
        if response_line.len() > 256 * 1024 {
            anyhow::bail!("helper response exceeded the size limit");
        }
        let response: HelperResponse =
            serde_json::from_str(&response_line).context("The helper returned invalid JSON")?;
        if response.request_id != request.request_id {
            anyhow::bail!("helper response request_id did not match");
        }
        if !response.ok {
            return Err(anyhow!(
                "{}",
                response
                    .error
                    .unwrap_or_else(|| "helper operation failed".to_owned())
            ));
        }
        Ok(response.status)
    }
}
