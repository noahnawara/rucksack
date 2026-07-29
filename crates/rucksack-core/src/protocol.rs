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
    /// Adding and removing optional fields is survivable in both directions, and the test at the
    /// bottom of this file is what keeps that true: serde reads a missing `Option` as `None` and
    /// ignores fields it does not recognise. What is *not* survivable is a shape change — a field
    /// changing type, or an operation being renamed — because both ends parse before they look at
    /// `protocol`, so the version that was supposed to explain the skew is never reached.
    ///
    /// `#[serde(default)]` is therefore belt-and-braces rather than load-bearing. It stays because
    /// this is the field whose whole job is to survive meeting a stranger.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A CLI and a helper from different releases have to survive meeting each other.
    ///
    /// `cargo install` replaces `~/.cargo/bin`; only `rucksack helper install` replaces the copy in
    /// `/Library/PrivilegedHelperTools`. Skew is therefore the normal state between an update and
    /// the next `helper install`, and it is resolved by a process holding a laptop awake in a bag.
    ///
    /// Both directions have to be non-fatal, and both are, for reasons worth pinning: serde reads a
    /// missing `Option` field as `None` without needing `#[serde(default)]`, and ignores fields it
    /// does not know about. So a helper may stop sending a field, and may start sending one, without
    /// either end failing to parse. This test is what makes that a property rather than a memory.
    #[test]
    fn a_status_survives_meeting_a_different_release() {
        // A newer helper that has stopped sending fields this build knows about.
        let fewer = r#"{"protocol":2,"request_id":"00000000-0000-0000-0000-000000000000","ok":true,"status":{"active":false}}"#;
        let parsed: HelperResponse = serde_json::from_str(fewer).expect("fewer fields still parse");
        let status = parsed.status.expect("a status");
        assert!(!status.active);
        assert_eq!(status.lease_id, None);
        assert_eq!(status.version, None);

        // An older helper that still sends fields this build has dropped.
        let more = r#"{"protocol":2,"request_id":"00000000-0000-0000-0000-000000000000","ok":true,"status":{"active":true,"last_reasserted_at":"2026-07-24T18:42:00Z","some_field_from_the_future":7}}"#;
        let parsed: HelperResponse = serde_json::from_str(more).expect("extra fields still parse");
        assert!(parsed.status.expect("a status").active);
    }
}
