//! §13 InputState -- the lifecycle state machine for inputs.
//!
//! Every accepted input has an InputState that tracks its progression
//! through the runtime lifecycle. Canonical lifecycle state is owned by
//! [`InputLifecycleAuthority`] -- all transitions flow through
//! `authority().apply()`.

use chrono::{DateTime, Utc};
use meerkat_core::lifecycle::{InputId, RunId};
use serde::{Deserialize, Serialize};

use crate::identifiers::PolicyVersion;
use crate::input::Input;
use crate::input_lifecycle_authority::{
    InputLifecycleAuthority, InputLifecycleError, InputLifecycleInput, InputLifecycleMutator,
    InputLifecycleTransition,
};
use crate::policy::PolicyDecision;

/// The lifecycle state of an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputLifecycleState {
    /// Input has been accepted but not yet queued.
    Accepted,
    /// Input is in the processing queue.
    Queued,
    /// Input has been staged at a run boundary.
    Staged,
    /// Input's boundary primitive has been applied.
    Applied,
    /// Applied and pending consumption (run in progress).
    AppliedPendingConsumption,
    /// Input has been fully consumed (terminal).
    Consumed,
    /// Input was superseded by a newer input (terminal).
    Superseded,
    /// Input was coalesced into an aggregate (terminal).
    Coalesced,
    /// Input was abandoned (retire/reset/destroy) (terminal).
    Abandoned,
}

impl InputLifecycleState {
    /// Check if this is a terminal state (no further transitions possible).
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
    /// Runtime was retired.
    Retired,
    /// Runtime was reset.
    Reset,
    /// Runtime was destroyed.
    Destroyed,
    /// Input was explicitly cancelled.
    Cancelled,
}

/// Terminal outcome of an input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome_type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum InputTerminalOutcome {
    /// Successfully consumed by a run.
    Consumed,
    /// Superseded by a newer input.
    Superseded { superseded_by: InputId },
    /// Coalesced into an aggregate.
    Coalesced { aggregate_id: InputId },
    /// Abandoned.
    Abandoned { reason: InputAbandonReason },
}

/// A single entry in the input's state history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputStateHistoryEntry {
    /// When this transition occurred.
    pub timestamp: DateTime<Utc>,
    /// The state transitioned from.
    pub from: InputLifecycleState,
    /// The state transitioned to.
    pub to: InputLifecycleState,
    /// Optional reason for the transition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Snapshot of the policy that was applied to this input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicySnapshot {
    /// The policy version.
    pub version: PolicyVersion,
    /// The decision that was made.
    pub decision: PolicyDecision,
}

/// How a derived input can be reconstructed after crash recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source_type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReconstructionSource {
    /// Derived from a projection rule.
    Projection {
        rule_id: String,
        source_event_id: String,
    },
    /// Derived from coalescing.
    Coalescing { source_input_ids: Vec<InputId> },
}

/// An event on an input's state (for event sourcing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputStateEvent {
    /// When the event occurred.
    pub timestamp: DateTime<Utc>,
    /// The new lifecycle state.
    pub state: InputLifecycleState,
    /// Optional detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Full state of an input in the runtime.
///
/// Canonical lifecycle state (phase, terminal_outcome, history, last_run_id,
/// last_boundary_sequence, updated_at) is owned by the embedded
/// [`InputLifecycleAuthority`]. All transitions must flow through
/// [`InputState::apply`]. Direct mutation of canonical fields is not possible.
///
/// Operational fields (input_id, policy, durability, idempotency_key, etc.)
/// are caller-managed and do not participate in the state machine.
#[derive(Debug, Clone)]
pub struct InputState {
    /// The input ID.
    pub input_id: InputId,
    /// The canonical lifecycle authority -- owns phase, terminal_outcome,
    /// history, last_run_id, last_boundary_sequence, updated_at.
    authority: InputLifecycleAuthority,
    /// Policy snapshot applied to this input.
    pub policy: Option<PolicySnapshot>,
    /// Durability requirement (retained for recovery correctness).
    pub durability: Option<crate::input::InputDurability>,
    /// Idempotency key (retained for dedup across restarts).
    pub idempotency_key: Option<crate::identifiers::IdempotencyKey>,
    /// Number of times this input has been applied (for crash-loop detection).
    pub attempt_count: u32,
    /// Number of times this input has been recovered.
    pub recovery_count: u32,
    /// How to reconstruct this input (for derived inputs).
    pub reconstruction_source: Option<ReconstructionSource>,
    /// Persisted input payload used to rebuild queued work after recovery.
    pub persisted_input: Option<Input>,
    /// When the input was created.
    pub created_at: DateTime<Utc>,
}

impl InputState {
    /// Create a new InputState in the Accepted state.
    pub fn new_accepted(input_id: InputId) -> Self {
        let now = Utc::now();
        Self {
            input_id,
            authority: InputLifecycleAuthority::new_at(now),
            policy: None,
            durability: None,
            idempotency_key: None,
            attempt_count: 0,
            recovery_count: 0,
            reconstruction_source: None,
            persisted_input: None,
            created_at: now,
        }
    }

    // ---- Canonical field accessors (delegate to authority) ----

    /// Current lifecycle state.
    pub fn current_state(&self) -> InputLifecycleState {
        self.authority.phase()
    }

    /// Check if the input is in a terminal state.
    pub fn is_terminal(&self) -> bool {
        self.authority.is_terminal()
    }

    /// Terminal outcome (set when state becomes terminal).
    pub fn terminal_outcome(&self) -> Option<&InputTerminalOutcome> {
        self.authority.terminal_outcome()
    }

    /// State transition history.
    pub fn history(&self) -> &[InputStateHistoryEntry] {
        self.authority.history()
    }

    /// Last run that touched this input.
    pub fn last_run_id(&self) -> Option<&RunId> {
        self.authority.last_run_id()
    }

    /// Boundary receipt sequence for the last applied run.
    pub fn last_boundary_sequence(&self) -> Option<u64> {
        self.authority.last_boundary_sequence()
    }

    /// When the input was last updated.
    pub fn updated_at(&self) -> DateTime<Utc> {
        self.authority.updated_at()
    }

    // ---- Authority access ----

    /// Apply a lifecycle input through the authority.
    ///
    /// This is the ONLY way to mutate canonical lifecycle state.
    pub fn apply(
        &mut self,
        input: InputLifecycleInput,
    ) -> Result<InputLifecycleTransition, InputLifecycleError> {
        self.authority.apply(input)
    }

    /// Check if a transition would be accepted without applying it.
    pub fn can_accept(&self, input: &InputLifecycleInput) -> bool {
        self.authority.can_accept(input)
    }

    /// Set the terminal outcome after a transition (for Superseded/Coalesced
    /// which need caller-provided data).
    pub fn set_terminal_outcome(&mut self, outcome: InputTerminalOutcome) {
        self.authority.set_terminal_outcome(outcome);
    }

    /// Get a reference to the authority (for advanced read-only operations).
    pub fn authority(&self) -> &InputLifecycleAuthority {
        &self.authority
    }

    /// Get a mutable reference to the authority (for recovery paths).
    pub fn authority_mut(&mut self) -> &mut InputLifecycleAuthority {
        &mut self.authority
    }
}

// ---------------------------------------------------------------------------
// Custom Serialize/Deserialize to preserve wire format
// ---------------------------------------------------------------------------

/// Serialization helper to keep the same wire format as the old InputState.
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
            current_state: self.authority.phase(),
            policy: self.policy.clone(),
            terminal_outcome: self.authority.terminal_outcome().cloned(),
            durability: self.durability,
            idempotency_key: self.idempotency_key.clone(),
            attempt_count: self.attempt_count,
            recovery_count: self.recovery_count,
            history: self.authority.history().to_vec(),
            reconstruction_source: self.reconstruction_source.clone(),
            persisted_input: self.persisted_input.clone(),
            last_run_id: self.authority.last_run_id().cloned(),
            last_boundary_sequence: self.authority.last_boundary_sequence(),
            created_at: self.created_at,
            updated_at: self.authority.updated_at(),
        };
        helper.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for InputState {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let helper = InputStateSerde::deserialize(deserializer)?;
        let authority = InputLifecycleAuthority::restore(
            helper.current_state,
            helper.terminal_outcome,
            helper.last_run_id,
            helper.last_boundary_sequence,
            helper.history,
            helper.updated_at,
        );
        Ok(InputState {
            input_id: helper.input_id,
            authority,
            policy: helper.policy,
            durability: helper.durability,
            idempotency_key: helper.idempotency_key,
            attempt_count: helper.attempt_count,
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
        ApplyMode, ConsumePoint, DrainPolicy, InterruptPolicy, QueueMode, RoutingDisposition,
        WakeMode,
    };
    use meerkat_core::ops::{OpEvent, OperationId};

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
    fn input_state_new_accepted() {
        let id = InputId::new();
        let state = InputState::new_accepted(id.clone());
        assert_eq!(state.input_id, id);
        assert_eq!(state.current_state(), InputLifecycleState::Accepted);
        assert!(!state.is_terminal());
        assert!(state.history().is_empty());
        assert!(state.terminal_outcome().is_none());
        assert!(state.policy.is_none());
    }

    #[test]
    fn input_state_serde_roundtrip() {
        let mut state = InputState::new_accepted(InputId::new());
        state.policy = Some(PolicySnapshot {
            version: PolicyVersion(1),
            decision: PolicyDecision {
                apply_mode: ApplyMode::StageRunStart,
                wake_mode: WakeMode::WakeIfIdle,
                queue_mode: QueueMode::Fifo,
                consume_point: ConsumePoint::OnRunComplete,
                interrupt_policy: InterruptPolicy::None,
                drain_policy: DrainPolicy::QueueNextTurn,
                routing_disposition: RoutingDisposition::Queue,
                record_transcript: true,
                emit_operator_content: true,
                policy_version: PolicyVersion(1),
            },
        });
        state.apply(InputLifecycleInput::QueueAccepted).unwrap();

        let json = serde_json::to_value(&state).unwrap();
        let parsed: InputState = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.input_id, state.input_id);
        assert_eq!(parsed.current_state(), state.current_state());
        assert_eq!(parsed.history().len(), 1);
    }

    #[test]
    fn input_state_deserializes_legacy_persisted_input_tags() {
        let mut continuation_state = InputState::new_accepted(InputId::new());
        continuation_state.persisted_input = Some(Input::Continuation(
            crate::input::ContinuationInput::terminal_peer_response_for_request(
                "legacy continuation",
                Some("req-legacy".into()),
            ),
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
