//! Wire event envelope and replay contracts.

use serde::{Deserialize, Serialize};

use crate::version::ContractVersion;
use meerkat_core::{AgentEvent, RuntimeMetadata, SessionId};

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

/// Authoritative event source scope for the generic replay surface.
///
/// This is intentionally a typed enum. Product concepts such as projects or
/// threads should be represented by opaque metadata on the owning runtime
/// objects, not by adding product-specific replay scopes here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventReplayScope {
    Session { session_id: SessionId },
}

impl EventReplayScope {
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        match self {
            Self::Session { session_id } => session_id,
        }
    }
}

/// Cursor into a replayable event source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EventReplayCursor {
    pub scope: EventReplayScope,
    /// Monotonically increasing sequence within the replay source. Sequence 0
    /// is the stable empty-source cursor.
    pub sequence: u64,
}

/// Validation failure for a replay cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum EventReplayCursorError {
    ScopeMismatch,
    AheadOfLatest {
        requested_sequence: u64,
        latest_sequence: u64,
    },
    SequenceOverflow,
}

impl EventReplayCursor {
    #[must_use]
    pub fn new(scope: EventReplayScope, sequence: u64) -> Self {
        Self { scope, sequence }
    }

    #[must_use]
    pub fn is_for_scope(&self, scope: &EventReplayScope) -> bool {
        &self.scope == scope
    }

    #[must_use]
    pub fn next_sequence(&self) -> Option<u64> {
        self.sequence.checked_add(1)
    }

    pub fn validate_for_list_since(
        &self,
        scope: &EventReplayScope,
        latest_sequence: u64,
    ) -> Result<u64, EventReplayCursorError> {
        if !self.is_for_scope(scope) {
            return Err(EventReplayCursorError::ScopeMismatch);
        }
        if self.sequence > latest_sequence {
            return Err(EventReplayCursorError::AheadOfLatest {
                requested_sequence: self.sequence,
                latest_sequence,
            });
        }
        self.next_sequence()
            .ok_or(EventReplayCursorError::SequenceOverflow)
    }
}

/// Stable replay event id derived from the owning source and source sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EventReplayEventId {
    pub scope: EventReplayScope,
    pub sequence: u64,
}

/// Typed replay event envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EventReplayEnvelope {
    pub event_id: EventReplayEventId,
    pub cursor: EventReplayCursor,
    pub timestamp_ms: u64,
    pub source: EventReplayScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mob_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "RuntimeMetadata::is_empty")]
    pub metadata: RuntimeMetadata,
    pub event: AgentEvent,
}

/// Parameters for `events/latest_cursor`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EventsLatestCursorParams {
    pub scope: EventReplayScope,
}

/// Result for `events/latest_cursor`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EventsLatestCursorResult {
    pub contract_version: ContractVersion,
    pub cursor: EventReplayCursor,
}

/// Parameters for `events/list_since`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EventsListSinceParams {
    pub scope: EventReplayScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<EventReplayCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Result for `events/list_since`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EventsListSinceResult {
    pub contract_version: ContractVersion,
    pub scope: EventReplayScope,
    pub from_cursor: EventReplayCursor,
    pub latest_cursor: EventReplayCursor,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<EventReplayEnvelope>,
    pub has_more: bool,
}

/// Parameters for `events/snapshot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EventsSnapshotParams {
    pub scope: EventReplayScope,
}

/// Snapshot payload for the first generic replay slice.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventsSnapshotBody {
    Session {
        session: crate::wire::session::WireSessionInfo,
    },
}

/// Result for `events/snapshot`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EventsSnapshotResult {
    pub contract_version: ContractVersion,
    pub scope: EventReplayScope,
    pub cursor: EventReplayCursor,
    pub snapshot: EventsSnapshotBody,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::{ToolConfigChangeOperation, ToolConfigChangedPayload};

    #[test]
    fn wire_event_roundtrip_tool_config_changed() {
        let event = WireEvent {
            session_id: SessionId::new(),
            sequence: 42,
            event: AgentEvent::ToolConfigChanged {
                payload: ToolConfigChangedPayload::new(
                    ToolConfigChangeOperation::Remove,
                    "filesystem",
                    meerkat_core::ToolConfigChangeStatus::external_tool_delta(
                        meerkat_core::ExternalToolDeltaPhase::Pending,
                        None,
                    ),
                    false,
                )
                .with_applied_at_turn(Some(3)),
            },
            contract_version: ContractVersion::CURRENT,
        };

        let encoded = serde_json::to_value(&event).expect("serialize");
        let decoded: WireEvent = serde_json::from_value(encoded).expect("deserialize");
        match decoded.event {
            AgentEvent::ToolConfigChanged { payload } => {
                assert_eq!(payload.operation, ToolConfigChangeOperation::Remove);
                assert_eq!(payload.target, "filesystem");
                assert_eq!(payload.status_text(), "pending");
                assert!(!payload.persisted);
                assert_eq!(payload.applied_at_turn, Some(3));
            }
            other => panic!("expected tool_config_changed, got {other:?}"),
        }
    }

    #[test]
    fn event_replay_cursor_is_typed_and_scope_checked() {
        let session_a = SessionId::new();
        let session_b = SessionId::new();
        let scope_a = EventReplayScope::Session {
            session_id: session_a,
        };
        let scope_b = EventReplayScope::Session {
            session_id: session_b,
        };
        let cursor = EventReplayCursor::new(scope_a.clone(), 3);

        assert!(cursor.is_for_scope(&scope_a));
        assert!(!cursor.is_for_scope(&scope_b));
        assert_eq!(cursor.next_sequence(), Some(4));
    }

    #[test]
    fn event_replay_cursor_validation_rejects_stale_and_invalid_inputs() {
        let scope = EventReplayScope::Session {
            session_id: SessionId::new(),
        };
        let other_scope = EventReplayScope::Session {
            session_id: SessionId::new(),
        };

        assert_eq!(
            EventReplayCursor::new(scope.clone(), 2)
                .validate_for_list_since(&scope, 5)
                .expect("valid cursor"),
            3
        );
        assert_eq!(
            EventReplayCursor::new(other_scope, 2).validate_for_list_since(&scope, 5),
            Err(EventReplayCursorError::ScopeMismatch)
        );
        assert_eq!(
            EventReplayCursor::new(scope.clone(), 6).validate_for_list_since(&scope, 5),
            Err(EventReplayCursorError::AheadOfLatest {
                requested_sequence: 6,
                latest_sequence: 5
            })
        );
        assert_eq!(
            EventReplayCursor::new(scope.clone(), u64::MAX)
                .validate_for_list_since(&scope, u64::MAX),
            Err(EventReplayCursorError::SequenceOverflow)
        );
    }

    #[test]
    fn list_since_params_roundtrip_uses_cursor_object_not_folklore_string() {
        let session_id = SessionId::new();
        let scope = EventReplayScope::Session { session_id };
        let params = EventsListSinceParams {
            scope: scope.clone(),
            cursor: Some(EventReplayCursor::new(scope, 2)),
            limit: Some(10),
        };

        let value = serde_json::to_value(&params).expect("serialize params");
        assert!(value["cursor"].is_object());
        assert!(value["cursor"].get("sequence").is_some());
        assert!(value["cursor"].get("scope").is_some());
        assert!(!value["cursor"].is_string());
        let decoded: EventsListSinceParams =
            serde_json::from_value(value).expect("deserialize params");
        assert_eq!(decoded.limit, Some(10));
    }
}
