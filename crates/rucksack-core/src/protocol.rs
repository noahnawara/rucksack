use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const HELPER_PROTOCOL_VERSION: u16 = 2;
pub const DEFAULT_HELPER_SOCKET: &str = "/var/run/rucksack-helper.sock";
pub const MAX_HELPER_TTL_SECONDS: u64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelperRequest {
    pub protocol: u16,
    pub request_id: Uuid,
    pub operation: HelperOperation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum HelperOperation {
    Acquire {
        lease_id: Uuid,
        ttl_seconds: u64,
        hard_expires_at: DateTime<Utc>,
        reason: String,
    },
    Renew {
        lease_id: Uuid,
        ttl_seconds: u64,
    },
    Release {
        lease_id: Uuid,
        reason: String,
    },
    Recover,
    Status,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelperResponse {
    pub protocol: u16,
    pub request_id: Uuid,
    pub ok: bool,
    pub error: Option<String>,
    pub status: Option<HelperStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelperStatus {
    /// The version of the helper binary that is actually installed and answering.
    ///
    /// `cargo install` replaces the two binaries in `~/.cargo/bin`; the helper that holds this Mac
    /// awake is a copy at `/Library/PrivilegedHelperTools`, and only `rucksack helper install`
    /// refreshes it. Updating without that second step leaves a new CLI talking to an old helper,
    /// and until this field existed neither end could tell — `helper status` reported "installed and
    /// idle" against a build from a different release.
    ///
    /// It works today only because the wire format has not changed between versions. The first time
    /// it does, a skew fails at parse with nothing to explain it, on the process that keeps a laptop
    /// awake in a bag.
    ///
    /// `#[serde(default)]` so a CLI that knows about this field can still read a helper that predates
    /// it — which is exactly the skew being detected, and it must not become a parse error.
    #[serde(default)]
    pub version: Option<String>,
    pub active: bool,
    pub lease_id: Option<Uuid>,
    pub owner_uid: Option<u32>,
    pub created_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub hard_expires_at: Option<DateTime<Utc>>,
    pub previous_sleep_disabled: Option<u8>,
    pub sleep_disabled: Option<u8>,
    pub reason: Option<String>,
    pub last_reasserted_at: Option<DateTime<Utc>>,
}

impl HelperRequest {
    pub fn new(operation: HelperOperation) -> Self {
        Self {
            protocol: HELPER_PROTOCOL_VERSION,
            request_id: Uuid::new_v4(),
            operation,
        }
    }
}

impl HelperResponse {
    pub fn success(request_id: Uuid, status: Option<HelperStatus>) -> Self {
        Self {
            protocol: HELPER_PROTOCOL_VERSION,
            request_id,
            ok: true,
            error: None,
            status,
        }
    }

    pub fn failure(
        request_id: Uuid,
        error: impl Into<String>,
        status: Option<HelperStatus>,
    ) -> Self {
        Self {
            protocol: HELPER_PROTOCOL_VERSION,
            request_id,
            ok: false,
            error: Some(error.into()),
            status,
        }
    }
}
