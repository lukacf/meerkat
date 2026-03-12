//! §14 AcceptOutcome — result of accepting an input.

use meerkat_core::lifecycle::InputId;
use serde::{Deserialize, Serialize};

use crate::input_state::InputState;
use crate::policy::PolicyDecision;

/// Outcome of `RuntimeDriver::accept_input()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome_type", rename_all = "snake_case")]
#[non_exhaustive]
#[allow(clippy::large_enum_variant)]
pub enum AcceptOutcome {
    /// Input was accepted and processing has begun.
    Accepted {
        /// The assigned input ID.
        input_id: InputId,
        /// The policy decision applied to this input.
        policy: PolicyDecision,
        /// Current input state.
        state: InputState,
    },
    /// Input was deduplicated (idempotency key matched an existing input).
    Deduplicated {
        /// The new input ID that was deduplicated.
        input_id: InputId,
        /// The existing input ID that was matched.
        existing_id: InputId,
    },
    /// Input was rejected (validation failed, durability violation, etc.).
    Rejected {
        /// Why the input was rejected.
        reason: String,
    },
}

impl AcceptOutcome {
    /// Check if the input was accepted.
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted { .. })
    }

    /// Check if the input was deduplicated.
    pub fn is_deduplicated(&self) -> bool {
        matches!(self, Self::Deduplicated { .. })
    }

    /// Check if the input was rejected.
    pub fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected { .. })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::identifiers::PolicyVersion;
    use crate::policy::{ApplyMode, ConsumePoint, QueueMode, WakeMode};

    #[test]
    fn accepted_serde() {
        let outcome = AcceptOutcome::Accepted {
            input_id: InputId::new(),
            policy: PolicyDecision {
                apply_mode: ApplyMode::StageRunStart,
                wake_mode: WakeMode::WakeIfIdle,
                queue_mode: QueueMode::Fifo,
                consume_point: ConsumePoint::OnRunComplete,
                record_transcript: true,
                emit_operator_content: true,
                policy_version: PolicyVersion(1),
            },
            state: InputState::new_accepted(InputId::new()),
        };
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["outcome_type"], "accepted");
        let parsed: AcceptOutcome = serde_json::from_value(json).unwrap();
        assert!(parsed.is_accepted());
        assert!(!parsed.is_deduplicated());
        assert!(!parsed.is_rejected());
    }

    #[test]
    fn deduplicated_serde() {
        let outcome = AcceptOutcome::Deduplicated {
            input_id: InputId::new(),
            existing_id: InputId::new(),
        };
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["outcome_type"], "deduplicated");
        let parsed: AcceptOutcome = serde_json::from_value(json).unwrap();
        assert!(parsed.is_deduplicated());
    }

    #[test]
    fn rejected_serde() {
        let outcome = AcceptOutcome::Rejected {
            reason: "durability violation".into(),
        };
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["outcome_type"], "rejected");
        let parsed: AcceptOutcome = serde_json::from_value(json).unwrap();
        assert!(parsed.is_rejected());
    }
}
