//! §13 InputState — per-input data shell.
//!
//! Canonical lifecycle truth for every input lives in the MeerkatMachine DSL
//! (`input_phases` map plus the `QueueAccepted` / `StageForRun` / etc.
//! transitions). This module owns ONLY the per-input shell metadata that has
//! no DSL home: a history log, timestamps, retry counter, policy snapshot,
//! durability class, idempotency key, and the cached payload needed to
//! rebuild queued work after recovery. None of it drives semantics.
//!
//! Mutation rule: callers update `InputState` fields directly (they are
//! `pub`). Transitions do NOT flow through an `apply()` method on this type —
//! there isn't one and deliberately never will be. Every lifecycle advance
//! is a DSL transition fired via the driver's `dsl_apply` helper; the
//! shell metadata is written alongside it at the same callsite.

use chrono::{DateTime, Utc};
use meerkat_core::lifecycle::{InputId, RunId};
use serde::{Deserialize, Serialize};

use crate::identifiers::PolicyVersion;
use crate::input::Input;
use crate::policy::PolicyDecision;

/// The lifecycle state of an input — mirrors the DSL's `input_phases` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputLifecycleState {
    Accepted,
    Queued,
    Staged,
    Applied,
    AppliedPendingConsumption,
    Consumed,
    Superseded,
    Coalesced,
    Abandoned,
}

impl InputLifecycleState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Consumed | Self::Superseded | Self::Coalesced | Self::Abandoned
        )
    }
}

/// Why an input was abandoned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputAbandonReason {
    Retired,
    Reset,
    Stopped,
    Destroyed,
    Cancelled,
    MaxAttemptsExhausted { attempts: u32 },
}

/// Terminal outcome of an input — written by the shell alongside the
/// matching DSL terminal transition. Superseded / Coalesced carry caller-
/// provided references that the DSL transition cannot know at apply time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome_type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputTerminalOutcome {
    Consumed,
    Superseded { superseded_by: InputId },
    Coalesced { aggregate_id: InputId },
    Abandoned { reason: InputAbandonReason },
}

/// A single entry in the input's state history (shell bookkeeping).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputStateHistoryEntry {
    pub timestamp: DateTime<Utc>,
    pub from: InputLifecycleState,
    pub to: InputLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Snapshot of the policy that was applied to this input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicySnapshot {
    pub version: PolicyVersion,
    pub decision: PolicyDecision,
}

/// How a derived input can be reconstructed after crash recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source_type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReconstructionSource {
    Projection {
        rule_id: String,
        source_event_id: String,
    },
    Coalescing {
        source_input_ids: Vec<InputId>,
    },
}

/// An event on an input's state (for event sourcing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputStateEvent {
    pub timestamp: DateTime<Utc>,
    pub state: InputLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Maximum stage → rollback cycles before the shell chooses Abandon instead
/// of RollbackStaged at the callsite. Shell retry-policy mechanic; not a DSL
/// guard.
pub const MAX_STAGE_ATTEMPTS: u32 = 3;

/// Per-input shell data. Plain fields, no hidden state machine.
///
/// The canonical phase of this input lives in the DSL's `input_phases` map.
/// The `phase` field below is a shell mirror written at the same callsite
/// that fires the DSL transition, used for observability, snapshot
/// assembly, and on-disk persistence. Queries that need live semantic truth
/// should read the DSL, not this struct.
#[derive(Debug, Clone)]
pub struct InputState {
    pub input_id: InputId,
    pub phase: InputLifecycleState,
    pub terminal_outcome: Option<InputTerminalOutcome>,
    pub last_run_id: Option<RunId>,
    pub last_boundary_sequence: Option<u64>,
    pub attempt_count: u32,
    pub history: Vec<InputStateHistoryEntry>,
    pub updated_at: DateTime<Utc>,
    pub policy: Option<PolicySnapshot>,
    pub durability: Option<crate::input::InputDurability>,
    pub idempotency_key: Option<crate::identifiers::IdempotencyKey>,
    pub recovery_count: u32,
    pub reconstruction_source: Option<ReconstructionSource>,
    pub persisted_input: Option<Input>,
    pub created_at: DateTime<Utc>,
}

impl InputState {
    /// Create a fresh InputState in the `Accepted` phase.
    pub fn new_accepted(input_id: InputId) -> Self {
        let now = Utc::now();
        Self {
            input_id,
            phase: InputLifecycleState::Accepted,
            terminal_outcome: None,
            last_run_id: None,
            last_boundary_sequence: None,
            attempt_count: 0,
            history: Vec::new(),
            updated_at: now,
            policy: None,
            durability: None,
            idempotency_key: None,
            recovery_count: 0,
            reconstruction_source: None,
            persisted_input: None,
            created_at: now,
        }
    }

    /// Current lifecycle phase mirror (shell copy of the DSL's `input_phases`).
    pub fn current_state(&self) -> InputLifecycleState {
        self.phase
    }

    /// Whether the mirrored phase is terminal.
    pub fn is_terminal(&self) -> bool {
        self.phase.is_terminal()
    }

    /// Shell-side terminal outcome (populated alongside DSL terminal transitions).
    pub fn terminal_outcome(&self) -> Option<&InputTerminalOutcome> {
        self.terminal_outcome.as_ref()
    }

    pub fn history(&self) -> &[InputStateHistoryEntry] {
        &self.history
    }

    pub fn last_run_id(&self) -> Option<&RunId> {
        self.last_run_id.as_ref()
    }

    pub fn last_boundary_sequence(&self) -> Option<u64> {
        self.last_boundary_sequence
    }

    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    pub fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    /// Stamp receipt metadata (run id + boundary sequence) on the shell copy.
    /// Pure shell mechanics, not a lifecycle transition; used by the
    /// persistence layer after a boundary receipt lands.
    pub fn stamp_receipt_metadata(&mut self, run_id: RunId, boundary_sequence: u64) {
        self.last_run_id = Some(run_id);
        self.last_boundary_sequence = Some(boundary_sequence);
    }
}

// ---------------------------------------------------------------------------
// Custom Serialize / Deserialize — preserves the on-disk wire format
// ---------------------------------------------------------------------------
//
// `InputStateSerde` is the on-disk contract exercised by
// `recovery_contract`, `recovery_replay`, and `driver_persistent` tests.
// Field names, types, defaults, and `skip_serializing_if` markers are kept
// verbatim from the pre-5A/4 release.

#[derive(Serialize, Deserialize)]
struct InputStateSerde {
    input_id: InputId,
    current_state: InputLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy: Option<PolicySnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_outcome: Option<InputTerminalOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    durability: Option<crate::input::InputDurability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    idempotency_key: Option<crate::identifiers::IdempotencyKey>,
    #[serde(default)]
    attempt_count: u32,
    #[serde(default)]
    recovery_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    history: Vec<InputStateHistoryEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reconstruction_source: Option<ReconstructionSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    persisted_input: Option<Input>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_boundary_sequence: Option<u64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl Serialize for InputState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let helper = InputStateSerde {
            input_id: self.input_id.clone(),
            current_state: self.phase,
            policy: self.policy.clone(),
            terminal_outcome: self.terminal_outcome.clone(),
            durability: self.durability,
            idempotency_key: self.idempotency_key.clone(),
            attempt_count: self.attempt_count,
            recovery_count: self.recovery_count,
            history: self.history.clone(),
            reconstruction_source: self.reconstruction_source.clone(),
            persisted_input: self.persisted_input.clone(),
            last_run_id: self.last_run_id.clone(),
            last_boundary_sequence: self.last_boundary_sequence,
            created_at: self.created_at,
            updated_at: self.updated_at,
        };
        helper.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for InputState {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let helper = InputStateSerde::deserialize(deserializer)?;
        Ok(InputState {
            input_id: helper.input_id,
            phase: helper.current_state,
            terminal_outcome: helper.terminal_outcome,
            last_run_id: helper.last_run_id,
            last_boundary_sequence: helper.last_boundary_sequence,
            attempt_count: helper.attempt_count,
            history: helper.history,
            updated_at: helper.updated_at,
            policy: helper.policy,
            durability: helper.durability,
            idempotency_key: helper.idempotency_key,
            recovery_count: helper.recovery_count,
            reconstruction_source: helper.reconstruction_source,
            persisted_input: helper.persisted_input,
            created_at: helper.created_at,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::policy::{
        ApplyMode, ConsumePoint, DrainPolicy, QueueMode, RoutingDisposition, WakeMode,
    };
    use meerkat_core::ops::{OpEvent, OperationId};

    #[test]
    fn new_accepted_starts_in_accepted_phase() {
        let id = InputId::new();
        let state = InputState::new_accepted(id.clone());
        assert_eq!(state.input_id, id);
        assert_eq!(state.phase, InputLifecycleState::Accepted);
        assert!(!state.is_terminal());
        assert_eq!(state.attempt_count, 0);
        assert!(state.history.is_empty());
        assert!(state.terminal_outcome.is_none());
    }

    #[test]
    fn lifecycle_state_terminal() {
        assert!(InputLifecycleState::Consumed.is_terminal());
        assert!(InputLifecycleState::Superseded.is_terminal());
        assert!(InputLifecycleState::Coalesced.is_terminal());
        assert!(InputLifecycleState::Abandoned.is_terminal());

        assert!(!InputLifecycleState::Accepted.is_terminal());
        assert!(!InputLifecycleState::Queued.is_terminal());
        assert!(!InputLifecycleState::Staged.is_terminal());
        assert!(!InputLifecycleState::Applied.is_terminal());
        assert!(!InputLifecycleState::AppliedPendingConsumption.is_terminal());
    }

    #[test]
    fn lifecycle_state_serde() {
        for state in [
            InputLifecycleState::Accepted,
            InputLifecycleState::Queued,
            InputLifecycleState::Staged,
            InputLifecycleState::Applied,
            InputLifecycleState::AppliedPendingConsumption,
            InputLifecycleState::Consumed,
            InputLifecycleState::Superseded,
            InputLifecycleState::Coalesced,
            InputLifecycleState::Abandoned,
        ] {
            let json = serde_json::to_value(state).unwrap();
            let parsed: InputLifecycleState = serde_json::from_value(json).unwrap();
            assert_eq!(state, parsed);
        }
    }

    #[test]
    fn input_state_serde_roundtrip_preserves_fields() {
        let mut state = InputState::new_accepted(InputId::new());
        state.policy = Some(PolicySnapshot {
            version: PolicyVersion(1),
            decision: PolicyDecision {
                apply_mode: ApplyMode::StageRunStart,
                wake_mode: WakeMode::WakeIfIdle,
                queue_mode: QueueMode::Fifo,
                consume_point: ConsumePoint::OnRunComplete,
                drain_policy: DrainPolicy::QueueNextTurn,
                routing_disposition: RoutingDisposition::Queue,
                record_transcript: true,
                emit_operator_content: true,
                policy_version: PolicyVersion(1),
            },
        });
        state.phase = InputLifecycleState::Queued;
        state.history.push(InputStateHistoryEntry {
            timestamp: state.updated_at,
            from: InputLifecycleState::Accepted,
            to: InputLifecycleState::Queued,
            reason: Some("QueueAccepted".into()),
        });

        let json = serde_json::to_value(&state).unwrap();
        let parsed: InputState = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.input_id, state.input_id);
        assert_eq!(parsed.phase, state.phase);
        assert_eq!(parsed.history.len(), 1);
    }

    #[test]
    fn input_state_deserializes_legacy_persisted_input_tags() {
        let mut continuation_state = InputState::new_accepted(InputId::new());
        continuation_state.persisted_input = Some(Input::Continuation(
            crate::input::ContinuationInput::detached_background_op_completed(),
        ));
        let mut continuation_json = serde_json::to_value(&continuation_state).unwrap();
        continuation_json["persisted_input"]["input_type"] =
            serde_json::Value::String("system_generated".into());
        let parsed: InputState = serde_json::from_value(continuation_json).unwrap();
        assert!(matches!(
            parsed.persisted_input,
            Some(Input::Continuation(_))
        ));

        let mut operation_state = InputState::new_accepted(InputId::new());
        operation_state.persisted_input = Some(Input::Operation(crate::input::OperationInput {
            header: crate::input::InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: crate::input::InputOrigin::System,
                durability: crate::input::InputDurability::Derived,
                visibility: crate::input::InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            operation_id: OperationId::new(),
            event: OpEvent::Cancelled {
                id: OperationId::new(),
            },
        }));
        let mut operation_json = serde_json::to_value(&operation_state).unwrap();
        operation_json["persisted_input"]["input_type"] =
            serde_json::Value::String("projected".into());
        let parsed: InputState = serde_json::from_value(operation_json).unwrap();
        assert!(matches!(parsed.persisted_input, Some(Input::Operation(_))));
    }

    #[test]
    fn abandon_reason_serde() {
        for reason in [
            InputAbandonReason::Retired,
            InputAbandonReason::Reset,
            InputAbandonReason::Destroyed,
            InputAbandonReason::Cancelled,
        ] {
            let json = serde_json::to_value(&reason).unwrap();
            let parsed: InputAbandonReason = serde_json::from_value(json).unwrap();
            assert_eq!(reason, parsed);
        }
    }

    #[test]
    fn terminal_outcome_consumed_serde() {
        let outcome = InputTerminalOutcome::Consumed;
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["outcome_type"], "consumed");
        let parsed: InputTerminalOutcome = serde_json::from_value(json).unwrap();
        assert_eq!(outcome, parsed);
    }

    #[test]
    fn terminal_outcome_superseded_serde() {
        let outcome = InputTerminalOutcome::Superseded {
            superseded_by: InputId::new(),
        };
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["outcome_type"], "superseded");
        let parsed: InputTerminalOutcome = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, InputTerminalOutcome::Superseded { .. }));
    }

    #[test]
    fn terminal_outcome_abandoned_serde() {
        let outcome = InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Retired,
        };
        let json = serde_json::to_value(&outcome).unwrap();
        let parsed: InputTerminalOutcome = serde_json::from_value(json).unwrap();
        assert!(matches!(
            parsed,
            InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::Retired,
            }
        ));
    }

    #[test]
    fn reconstruction_source_serde() {
        let sources = vec![
            ReconstructionSource::Projection {
                rule_id: "rule-1".into(),
                source_event_id: "evt-1".into(),
            },
            ReconstructionSource::Coalescing {
                source_input_ids: vec![InputId::new(), InputId::new()],
            },
        ];
        for source in sources {
            let json = serde_json::to_value(&source).unwrap();
            assert!(json["source_type"].is_string());
            let parsed: ReconstructionSource = serde_json::from_value(json).unwrap();
            let _ = parsed;
        }
    }

    #[test]
    fn input_state_event_serde() {
        let event = InputStateEvent {
            timestamp: Utc::now(),
            state: InputLifecycleState::Queued,
            detail: Some("queued for processing".into()),
        };
        let json = serde_json::to_value(&event).unwrap();
        let parsed: InputStateEvent = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.state, InputLifecycleState::Queued);
    }
}
