//! §14 AcceptOutcome — result of accepting an input.

use meerkat_core::lifecycle::InputId;
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::driver::PostAdmissionSignal;
use crate::input_state::InputState;
use crate::policy::PolicyDecision;

/// Typed reason why an input was rejected at the accept boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "reject_type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RejectReason {
    /// Runtime is not in a state that accepts input (e.g. stopped, destroyed).
    NotReady {
        /// The runtime state that caused the rejection.
        state: String,
    },
    /// Input failed durability validation.
    DurabilityViolation {
        /// Description of the violation.
        detail: String,
    },
    /// Peer input carried a forbidden handling_mode.
    PeerHandlingModeInvalid {
        /// Description of the violation.
        detail: String,
    },
}

impl fmt::Display for RejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotReady { state } => {
                write!(f, "runtime not accepting input while in state: {state}")
            }
            Self::DurabilityViolation { detail } => write!(f, "{detail}"),
            Self::PeerHandlingModeInvalid { detail } => write!(f, "{detail}"),
        }
    }
}

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
        reason: RejectReason,
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

/// Classify the machine-owned post-admission control signal for a resolved
/// accept outcome.
///
/// Admission-time wake / interrupt / immediate-processing semantics are owned
/// by the checked-in Meerkat machine. The runtime helper may still carry other
/// wake signals for non-admission bookkeeping, but plain accept classification
/// should flow through this function.
pub fn post_admission_signal_from_accept_outcome(
    outcome: &AcceptOutcome,
    request_immediate_processing: bool,
) -> PostAdmissionSignal {
    if !matches!(outcome, AcceptOutcome::Accepted { .. }) {
        return PostAdmissionSignal::None;
    }
    if request_immediate_processing {
        return PostAdmissionSignal::RequestImmediateProcessing;
    }

    match outcome {
        AcceptOutcome::Accepted { policy, .. } => match policy.wake_mode {
            crate::WakeMode::InterruptYielding => PostAdmissionSignal::InterruptYielding,
            crate::WakeMode::WakeIfIdle => PostAdmissionSignal::WakeLoop,
            crate::WakeMode::None => PostAdmissionSignal::None,
        },
        AcceptOutcome::Deduplicated { .. } | AcceptOutcome::Rejected { .. } => {
            PostAdmissionSignal::None
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::identifiers::PolicyVersion;
    use crate::policy::{
        ApplyMode, ConsumePoint, DrainPolicy, QueueMode, RoutingDisposition, WakeMode,
    };

    #[test]
    fn accepted_serde() {
        let outcome = AcceptOutcome::Accepted {
            input_id: InputId::new(),
            policy: PolicyDecision {
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
            reason: RejectReason::DurabilityViolation {
                detail: "durability violation".into(),
            },
        };
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["outcome_type"], "rejected");
        assert_eq!(json["reason"]["reject_type"], "durability_violation");
        let parsed: AcceptOutcome = serde_json::from_value(json).unwrap();
        assert!(parsed.is_rejected());
    }

    #[test]
    fn reject_reason_display() {
        let not_ready = RejectReason::NotReady {
            state: "Stopped".into(),
        };
        assert_eq!(
            not_ready.to_string(),
            "runtime not accepting input while in state: Stopped"
        );

        let durability = RejectReason::DurabilityViolation {
            detail: "Derived durability forbidden for prompt".into(),
        };
        assert_eq!(
            durability.to_string(),
            "Derived durability forbidden for prompt"
        );

        let peer = RejectReason::PeerHandlingModeInvalid {
            detail: "handling_mode is forbidden on ResponseProgress peer inputs".into(),
        };
        assert_eq!(
            peer.to_string(),
            "handling_mode is forbidden on ResponseProgress peer inputs"
        );
    }

    #[test]
    fn reject_reason_serde_round_trip() {
        let reasons = vec![
            RejectReason::NotReady {
                state: "Destroyed".into(),
            },
            RejectReason::DurabilityViolation {
                detail: "external derived".into(),
            },
            RejectReason::PeerHandlingModeInvalid {
                detail: "forbidden".into(),
            },
        ];
        for reason in reasons {
            let json = serde_json::to_value(&reason).unwrap();
            let parsed: RejectReason = serde_json::from_value(json).unwrap();
            assert_eq!(parsed, reason);
        }
    }
}
