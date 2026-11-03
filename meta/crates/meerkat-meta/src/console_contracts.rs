//! Console-facing contract types for the planned identity-native operator workbench.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const IDENTITY_STREAM_NAME: &str = "identity";
pub const ALL_EVENTS_STREAM_NAME: &str = "all_events";
pub const SYSTEM_EVENT_IDENTITY: &str = "_system";
/// Identity of the console's synthetic source-gap markers.
pub(crate) const CONSOLE_GAP_IDENTITY: &str = "__console__";

/// Whether `identity` is one of the console's runtime-plane identities:
/// [`SYSTEM_EVENT_IDENTITY`] (the runtime's own events, such as identity-less
/// memory events) or [`CONSOLE_GAP_IDENTITY`]. The console treats frames
/// under them as the runtime's, never a member's, so no member may take one.
pub(crate) fn is_runtime_plane_identity(identity: &str) -> bool {
    identity == SYSTEM_EVENT_IDENTITY || identity == CONSOLE_GAP_IDENTITY
}

/// Most distinct members [`warn_runtime_plane_member_drop`] remembers.
const RUNTIME_PLANE_DROP_WARNINGS_CAP: usize = 1024;

/// Log that a console writer dropped a member's event because it resolved to
/// a runtime-plane identity: once per runtime id (or, without one, per
/// member id) per process, so a member that keeps emitting does not flood the
/// log. Past the cap a drop logs at debug only.
pub(crate) fn warn_runtime_plane_member_drop(
    member_id: &str,
    runtime_id: Option<&str>,
    identity: &str,
    writer: &'static str,
) {
    static WARNED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    let first = {
        let mut warned = WARNED
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        warned.len() < RUNTIME_PLANE_DROP_WARNINGS_CAP
            && warned.insert(runtime_id.unwrap_or(member_id).to_string())
    };
    if first {
        tracing::warn!(
            member_id,
            runtime_id = runtime_id.unwrap_or_default(),
            identity,
            writer,
            "dropping a member's console event: it resolves to a reserved runtime-plane identity"
        );
    } else {
        tracing::debug!(
            member_id,
            runtime_id = runtime_id.unwrap_or_default(),
            identity,
            writer,
            "dropping a member's console event: it resolves to a reserved runtime-plane identity"
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleIdentityEventEnvelope {
    pub event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_id: Option<String>,
    pub identity: String,
    pub event_type: String,
    pub timestamp_ms: u64,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayUnavailableError {
    pub error: String,
    pub stream: String,
    pub requested_last_event_id: String,
    pub latest_event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleInteractionRejectedError {
    pub code: i64,
    pub message: String,
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn console_identity_event_envelope_roundtrips_with_interaction_id() {
        let original = ConsoleIdentityEventEnvelope {
            event_id: "evt-1".to_string(),
            interaction_id: Some("turn-123".to_string()),
            identity: "identity:luka".to_string(),
            event_type: "interaction_complete".to_string(),
            timestamp_ms: 1_717_171_717,
            data: serde_json::json!({
                "status": "ok",
                "tool_call_id": "tool-1"
            }),
        };

        let encoded = serde_json::to_value(&original).expect("envelope should serialize");
        let decoded: ConsoleIdentityEventEnvelope =
            serde_json::from_value(encoded).expect("envelope should deserialize");

        assert_eq!(decoded, original);
    }

    #[test]
    fn console_identity_event_envelope_roundtrips_without_interaction_id() {
        let original = ConsoleIdentityEventEnvelope {
            event_id: "evt-2".to_string(),
            interaction_id: None,
            identity: "identity:luka".to_string(),
            event_type: "lease_updated".to_string(),
            timestamp_ms: 1_717_171_718,
            data: serde_json::json!({
                "state": "healthy"
            }),
        };

        let encoded = serde_json::to_value(&original).expect("envelope should serialize");
        let decoded: ConsoleIdentityEventEnvelope =
            serde_json::from_value(encoded).expect("envelope should deserialize");

        assert_eq!(decoded, original);
    }

    #[test]
    fn replay_unavailable_error_roundtrips_through_json() {
        let original = ReplayUnavailableError {
            error: "replay_unavailable".to_string(),
            stream: "identity".to_string(),
            requested_last_event_id: "evt-1".to_string(),
            latest_event_id: "evt-9".to_string(),
        };

        let encoded = serde_json::to_value(&original).expect("replay error should serialize");
        let decoded: ReplayUnavailableError =
            serde_json::from_value(encoded).expect("replay error should deserialize");

        assert_eq!(decoded, original);
    }

    #[test]
    fn console_interaction_rejected_error_roundtrips_through_json() {
        let original = ConsoleInteractionRejectedError {
            code: -32001,
            message: "unknown identity".to_string(),
        };

        let encoded = serde_json::to_value(&original).expect("rejection error should serialize");
        let decoded: ConsoleInteractionRejectedError =
            serde_json::from_value(encoded).expect("rejection error should deserialize");

        assert_eq!(decoded, original);
    }
}
