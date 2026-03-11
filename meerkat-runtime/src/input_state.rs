//! §13 InputState — the lifecycle state machine for inputs.
//!
//! Every accepted input has an InputState that tracks its progression
//! through the runtime lifecycle.

use chrono::{DateTime, Utc};
use meerkat_core::lifecycle::InputId;
use serde::{Deserialize, Serialize};

use crate::identifiers::PolicyVersion;
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputState {
    /// The input ID.
    pub input_id: InputId,
    /// Current lifecycle state.
    pub current_state: InputLifecycleState,
    /// Policy snapshot applied to this input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<PolicySnapshot>,
    /// Terminal outcome (set when state becomes terminal).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_outcome: Option<InputTerminalOutcome>,
    /// Durability requirement (retained for recovery correctness).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub durability: Option<crate::input::InputDurability>,
    /// Idempotency key (retained for dedup across restarts).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<crate::identifiers::IdempotencyKey>,
    /// Number of times this input has been applied (for crash-loop detection).
    #[serde(default)]
    pub attempt_count: u32,
    /// Number of times this input has been recovered.
    #[serde(default)]
    pub recovery_count: u32,
    /// State transition history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<InputStateHistoryEntry>,
    /// How to reconstruct this input (for derived inputs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconstruction_source: Option<ReconstructionSource>,
    /// When the input was created.
    pub created_at: DateTime<Utc>,
    /// When the input was last updated.
    pub updated_at: DateTime<Utc>,
}

impl InputState {
    /// Create a new InputState in the Accepted state.
    pub fn new_accepted(input_id: InputId) -> Self {
        let now = Utc::now();
        Self {
            input_id,
            current_state: InputLifecycleState::Accepted,
            policy: None,
            terminal_outcome: None,
            durability: None,
            idempotency_key: None,
            attempt_count: 0,
            recovery_count: 0,
            history: Vec::new(),
            reconstruction_source: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Check if the input is in a terminal state.
    pub fn is_terminal(&self) -> bool {
        self.current_state.is_terminal()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::policy::{ApplyMode, ConsumePoint, QueueMode, WakeMode};

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
        assert_eq!(state.current_state, InputLifecycleState::Accepted);
        assert!(!state.is_terminal());
        assert!(state.history.is_empty());
        assert!(state.terminal_outcome.is_none());
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
                record_transcript: true,
                emit_operator_content: true,
                policy_version: PolicyVersion(1),
            },
        });
        state.history.push(InputStateHistoryEntry {
            timestamp: Utc::now(),
            from: InputLifecycleState::Accepted,
            to: InputLifecycleState::Queued,
            reason: Some("policy resolved".into()),
        });

        let json = serde_json::to_value(&state).unwrap();
        let parsed: InputState = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.input_id, state.input_id);
        assert_eq!(parsed.current_state, state.current_state);
        assert_eq!(parsed.history.len(), 1);
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
            // Verify the tag is present
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
