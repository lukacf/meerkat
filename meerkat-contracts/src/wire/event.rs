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
    fn wire_event_policy_stop_preserves_terminal_provider_error() {
        use meerkat_core::error::{
            AgentError, LlmFailureReason, LlmProviderError, LlmProviderErrorKind,
        };
        use meerkat_core::event::AgentErrorReport;

        let error = AgentError::llm(
            "openai",
            LlmFailureReason::ProviderError(LlmProviderError::non_retryable(
                LlmProviderErrorKind::PolicyStop,
                serde_json::json!({
                    "code": "misalignment_policy_violation",
                    "message": "Operator review required",
                }),
            )),
            "Operator review required",
        );
        let report = AgentErrorReport::from_agent_error(&error);
        let session_id = SessionId::new();
        let event = WireEvent {
            session_id: session_id.clone(),
            sequence: 1,
            event: AgentEvent::RunFailed {
                session_id,
                error_report: report.clone(),
                terminal_cause_kind: Some(meerkat_core::TurnTerminalCauseKind::LlmFailure),
            },
            contract_version: ContractVersion::CURRENT,
        };
        let encoded = serde_json::to_value(&event).expect("serialize policy stop");
        let reason = &encoded["event"]["error_report"]["reason"];
        assert_eq!(reason["reason_type"], "llm_provider_error");
        assert_eq!(reason["provider_error_kind"], "policy_stop");
        assert_eq!(reason["provider_error_retryability"], "non_retryable");
        assert_eq!(
            reason["provider_error"]["code"],
            "misalignment_policy_violation"
        );
        let decoded: WireEvent = serde_json::from_value(encoded).expect("deserialize policy stop");
        let AgentEvent::RunFailed { error_report, .. } = decoded.event else {
            panic!("policy stop must remain a failed run");
        };
        assert_eq!(error_report, report);
    }

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
    fn wire_event_turn_completed_carries_resolved_model_attribution() {
        let usage = meerkat_core::TurnUsage::new(
            meerkat_core::Usage {
                input_tokens: 300,
                output_tokens: 150,
                cache_creation_tokens: Some(0),
                cache_read_tokens: Some(4000),
                provider_accounting: None,
            },
            meerkat_core::ProviderTokenAccounting::anthropic("claude-opus-5", 300, 0, 4000),
        );
        let event = WireEvent {
            session_id: SessionId::new(),
            sequence: 7,
            event: AgentEvent::TurnCompleted {
                stop_reason: meerkat_core::StopReason::EndTurn,
                usage: Some(usage.clone()),
            },
            contract_version: ContractVersion::CURRENT,
        };

        let encoded = serde_json::to_value(&event).expect("serialize");
        assert_eq!(
            encoded["event"]["usage"]["accounting"]["provider"],
            serde_json::json!("anthropic"),
            "a consumer reading only the event stream must see the provider"
        );
        assert_eq!(
            encoded["event"]["usage"]["accounting"]["model"],
            serde_json::json!("claude-opus-5"),
            "a consumer reading only the event stream must see the model"
        );
        assert_eq!(
            encoded["event"]["usage"]["accounting"]["presented_tokens"],
            serde_json::json!(4300)
        );

        let decoded: WireEvent = serde_json::from_value(encoded).expect("deserialize");
        match decoded.event {
            AgentEvent::TurnCompleted {
                usage: decoded_usage,
                ..
            } => assert_eq!(decoded_usage, Some(usage.clone())),
            other => panic!("expected turn_completed, got {other:?}"),
        }

        // The cumulative run boundary deliberately carries no single-model
        // attribution: one run may span providers and models.
        let cumulative = meerkat_core::CumulativeUsage::from_usage(usage.into_inner());
        let run = WireEvent {
            session_id: SessionId::new(),
            sequence: 8,
            event: AgentEvent::RunCompleted {
                session_id: SessionId::new(),
                result: "done".to_string(),
                structured_output: None,
                extraction_required: false,
                usage: cumulative,
                terminal_cause_kind: None,
            },
            contract_version: ContractVersion::CURRENT,
        };
        let run_encoded = serde_json::to_value(&run).expect("serialize run");
        assert!(
            run_encoded["event"]["usage"].get("accounting").is_none()
                && run_encoded["event"]["usage"]
                    .get("provider_accounting")
                    .is_none(),
            "run-level cumulative usage must not claim one per-call model"
        );
    }

    #[test]
    fn wire_turn_usage_preserves_attribution_and_normalized_total() {
        let usage = meerkat_core::TurnUsage::new(
            meerkat_core::Usage {
                input_tokens: 1000,
                output_tokens: 200,
                cache_creation_tokens: Some(4000),
                cache_read_tokens: Some(0),
                provider_accounting: None,
            },
            meerkat_core::ProviderTokenAccounting::anthropic("claude-opus-5", 1000, 4000, 0),
        );
        let wire: crate::wire::WireTurnUsage = usage.into();
        assert_eq!(wire.accounting.model, "claude-opus-5");
        assert_eq!(wire.accounting.provider, meerkat_core::Provider::Anthropic);
        assert_eq!(wire.usage.input_tokens, 1000);
        assert_eq!(
            wire.usage.total_tokens, 5200,
            "wire total for one call is presented input plus output"
        );

        let encoded = serde_json::to_value(&wire).expect("serialize");
        let decoded: crate::wire::WireTurnUsage =
            serde_json::from_value(encoded).expect("deserialize");
        assert_eq!(decoded, wire);
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
