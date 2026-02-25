//! Wire event envelope.

use serde::{Deserialize, Serialize};

use crate::version::ContractVersion;
use meerkat_core::{AgentEvent, SessionId};

/// Canonical event envelope for wire protocol.
///
/// Wraps an [`AgentEvent`] with session context and contract version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireEvent {
    pub session_id: SessionId,
    pub sequence: u64,
    pub event: AgentEvent,
    pub contract_version: ContractVersion,
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::{ToolConfigChangeOperation, ToolConfigChangedPayload};

    #[test]
    fn wire_event_roundtrip_tool_config_changed() {
        let event = WireEvent {
            session_id: SessionId::new(),
            sequence: 42,
            event: AgentEvent::ToolConfigChanged {
                payload: ToolConfigChangedPayload {
                    operation: ToolConfigChangeOperation::Remove,
                    target: "filesystem".to_string(),
                    status: "staged".to_string(),
                    persisted: false,
                    applied_at_turn: Some(3),
                },
            },
            contract_version: ContractVersion::CURRENT,
        };

        let encoded = serde_json::to_value(&event).expect("serialize");
        let decoded: WireEvent = serde_json::from_value(encoded).expect("deserialize");
        match decoded.event {
            AgentEvent::ToolConfigChanged { payload } => {
                assert_eq!(payload.operation, ToolConfigChangeOperation::Remove);
                assert_eq!(payload.target, "filesystem");
                assert_eq!(payload.status, "staged");
                assert!(!payload.persisted);
                assert_eq!(payload.applied_at_turn, Some(3));
            }
            other => panic!("expected tool_config_changed, got {other:?}"),
        }
    }
}
