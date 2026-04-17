//! §14 AcceptOutcome — result of accepting an input.

use meerkat_core::lifecycle::InputId;
use meerkat_core::types::HandlingMode;
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::driver::PostAdmissionSignal;
use crate::input::Input;
use crate::input_state::InputState;
use crate::policy::PolicyDecision;
use crate::policy_table::DefaultPolicyTable;

// `AcceptOutcome` is a domain envelope. The wire shape lives in
// `meerkat-contracts::wire::runtime::RuntimeAcceptResult` and is materialized
// by per-surface handlers (see `meerkat-rpc::handlers::runtime`). The envelope
// therefore carries the live `InputState` shell (no Serialize/Deserialize) and
// a typed `RejectReason` that retains its own serde derives because rejection
// payloads are translated into wire-facing strings.

/// Machine-owned queue action for an admitted input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionQueueAction {
    None,
    EnqueueTo { target: HandlingMode },
    EnqueueFront { target: HandlingMode },
}

/// Machine-owned action against an existing queued input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExistingQueuedAdmissionAction {
    Coalesce { existing_id: InputId },
    Supersede { existing_id: InputId },
}

/// Machine-owned admission plan for an accepted input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionPlan {
    ConsumedOnAccept,
    Queued {
        persist_and_queue: bool,
        queue_action: AdmissionQueueAction,
        existing_action: Option<ExistingQueuedAdmissionAction>,
    },
}

/// Machine-owned resolution of an accepted input's semantic admission path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAdmission {
    pub policy: PolicyDecision,
    pub handling_mode: HandlingMode,
    pub admission_plan: AdmissionPlan,
}

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
///
/// Domain envelope returned to in-process callers. Surface crates translate it
/// into the wire shape (`RuntimeAcceptResult` in `meerkat-contracts`) before
/// emitting it on the network, so this type intentionally has no
/// `Serialize`/`Deserialize` derives.
#[derive(Debug, Clone)]
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

/// Classify the machine-owned admission disposition for an accepted input.
///
/// This is the semantic answer to “what happens to an accepted input?” Helpers
/// should only apply the already-decided queue/lifecycle mutations.
pub fn admission_plan_from_policy(
    policy: &PolicyDecision,
    handling_mode: HandlingMode,
    existing_superseded_id: Option<InputId>,
) -> AdmissionPlan {
    if policy.apply_mode == crate::policy::ApplyMode::Ignore
        && policy.consume_point == crate::policy::ConsumePoint::OnAccept
    {
        return AdmissionPlan::ConsumedOnAccept;
    }

    if policy.apply_mode == crate::policy::ApplyMode::Ignore {
        return AdmissionPlan::Queued {
            persist_and_queue: false,
            queue_action: AdmissionQueueAction::None,
            existing_action: None,
        };
    }

    match policy.queue_mode {
        crate::policy::QueueMode::Coalesce => AdmissionPlan::Queued {
            persist_and_queue: true,
            queue_action: AdmissionQueueAction::EnqueueTo {
                target: handling_mode,
            },
            existing_action: existing_superseded_id
                .map(|existing_id| ExistingQueuedAdmissionAction::Coalesce { existing_id }),
        },
        crate::policy::QueueMode::Supersede => AdmissionPlan::Queued {
            persist_and_queue: true,
            queue_action: AdmissionQueueAction::EnqueueTo {
                target: handling_mode,
            },
            existing_action: existing_superseded_id
                .map(|existing_id| ExistingQueuedAdmissionAction::Supersede { existing_id }),
        },
        crate::policy::QueueMode::Priority => AdmissionPlan::Queued {
            persist_and_queue: true,
            queue_action: AdmissionQueueAction::EnqueueFront {
                target: handling_mode,
            },
            existing_action: None,
        },
        crate::policy::QueueMode::Fifo | crate::policy::QueueMode::None => AdmissionPlan::Queued {
            persist_and_queue: true,
            queue_action: AdmissionQueueAction::EnqueueTo {
                target: handling_mode,
            },
            existing_action: None,
        },
    }
}

/// Derive the handling mode from a resolved policy decision.
pub fn handling_mode_from_policy(policy: &PolicyDecision) -> HandlingMode {
    match policy.routing_disposition {
        crate::policy::RoutingDisposition::Steer => HandlingMode::Steer,
        _ => HandlingMode::Queue,
    }
}

/// Whether this input requests immediate processing after admission.
///
/// This remains narrower than "routes through the steer lane". Some inputs
/// route through checkpoint/steer paths for batching, but only explicit steer
/// intent should request in-turn processing.
pub fn requests_immediate_processing(input: &Input) -> bool {
    matches!(input.handling_mode(), Some(HandlingMode::Steer))
}

/// Resolve the machine-owned semantic admission path for an accepted input.
///
/// Runtime helpers may still perform mechanical queue lookups (for example,
/// determining which existing queued input would be superseded), but the
/// semantic decision about policy, routing, and admission disposition is owned
/// here rather than inside the driver helper.
pub fn resolve_admission(
    input: &Input,
    runtime_idle: bool,
    silent_intents: &[String],
    existing_superseded_id: Option<InputId>,
) -> ResolvedAdmission {
    let mut policy = DefaultPolicyTable::resolve(input, runtime_idle);
    crate::silent_intent::apply_silent_intent_override(input, silent_intents, &mut policy);
    let handling_mode = handling_mode_from_policy(&policy);
    let admission_plan = admission_plan_from_policy(&policy, handling_mode, existing_superseded_id);

    ResolvedAdmission {
        policy,
        handling_mode,
        admission_plan,
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
    fn accepted_classifier() {
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
        assert!(outcome.is_accepted());
        assert!(!outcome.is_deduplicated());
        assert!(!outcome.is_rejected());
    }

    #[test]
    fn deduplicated_classifier() {
        let outcome = AcceptOutcome::Deduplicated {
            input_id: InputId::new(),
            existing_id: InputId::new(),
        };
        assert!(!outcome.is_accepted());
        assert!(outcome.is_deduplicated());
        assert!(!outcome.is_rejected());
    }

    #[test]
    fn rejected_classifier() {
        let outcome = AcceptOutcome::Rejected {
            reason: RejectReason::DurabilityViolation {
                detail: "durability violation".into(),
            },
        };
        assert!(!outcome.is_accepted());
        assert!(!outcome.is_deduplicated());
        assert!(outcome.is_rejected());
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
