//! §14 AcceptOutcome — result of accepting an input.

use meerkat_core::lifecycle::InputId;
use meerkat_core::types::HandlingMode;
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::driver::PostAdmissionSignal;
use crate::input_state::{InputState, InputStateSeed};
use crate::meerkat_machine::dsl as mm_dsl;
use crate::policy::PolicyDecision;
use crate::runtime_state::RuntimeState;

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

/// Coarse accept flags used by the MeerkatMachine DSL's
/// `AcceptWithCompletion` branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoarseAdmissionFlags {
    pub request_immediate_processing: bool,
    pub interrupt_yielding: bool,
    pub wake_if_idle: bool,
}

/// Typed machine input that authorized a live admission resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MachineAdmissionAuthority {
    input_id: String,
    input_kind: mm_dsl::AdmissionInputKind,
    requested_lane: Option<mm_dsl::InputLane>,
    silent_intent_match: bool,
    existing_superseded_input_id: Option<String>,
    runtime_running: bool,
    without_wake: bool,
}

impl MachineAdmissionAuthority {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        input_id: String,
        input_kind: mm_dsl::AdmissionInputKind,
        requested_lane: Option<mm_dsl::InputLane>,
        silent_intent_match: bool,
        existing_superseded_input_id: Option<InputId>,
        runtime_running: bool,
        without_wake: bool,
    ) -> Self {
        Self {
            input_id,
            input_kind,
            requested_lane,
            silent_intent_match,
            existing_superseded_input_id: existing_superseded_input_id.map(|id| id.to_string()),
            runtime_running,
            without_wake,
        }
    }

    pub(crate) fn input_id(&self) -> &str {
        &self.input_id
    }

    pub(crate) fn without_wake(&self) -> bool {
        self.without_wake
    }

    pub(crate) fn to_dsl_input(&self) -> mm_dsl::MeerkatMachineInput {
        mm_dsl::MeerkatMachineInput::ResolveAdmissionPlan {
            input_id: self.input_id.clone(),
            input_kind: self.input_kind,
            requested_lane: self.requested_lane,
            silent_intent_match: self.silent_intent_match,
            existing_superseded_input_id: self.existing_superseded_input_id.clone(),
            runtime_running: self.runtime_running,
            without_wake: self.without_wake,
        }
    }
}

/// Machine-owned resolution of an accepted input's semantic admission path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAdmission {
    policy: PolicyDecision,
    handling_mode: HandlingMode,
    runtime_semantics: crate::ingress_types::RuntimeInputSemantics,
    primitive_projection: crate::ingress_types::RuntimeInputProjection,
    admission_plan: AdmissionPlan,
    coarse_flags: CoarseAdmissionFlags,
    requires_active_pre_admission: bool,
    authority: MachineAdmissionAuthority,
}

impl ResolvedAdmission {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_machine_resolution(
        policy: PolicyDecision,
        handling_mode: HandlingMode,
        runtime_semantics: crate::ingress_types::RuntimeInputSemantics,
        primitive_projection: crate::ingress_types::RuntimeInputProjection,
        admission_plan: AdmissionPlan,
        coarse_flags: CoarseAdmissionFlags,
        requires_active_pre_admission: bool,
        authority: MachineAdmissionAuthority,
    ) -> Self {
        Self {
            policy,
            handling_mode,
            runtime_semantics,
            primitive_projection,
            admission_plan,
            coarse_flags,
            requires_active_pre_admission,
            authority,
        }
    }

    pub(crate) fn coarse_flags(&self) -> CoarseAdmissionFlags {
        self.coarse_flags
    }

    pub(crate) fn requires_active_runtime_pre_admission(&self) -> bool {
        self.requires_active_pre_admission
    }

    pub(crate) fn stages_run_boundary(&self) -> bool {
        self.policy.apply_mode == crate::policy::ApplyMode::StageRunBoundary
    }

    pub(crate) fn authority(&self) -> &MachineAdmissionAuthority {
        &self.authority
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        PolicyDecision,
        HandlingMode,
        crate::ingress_types::RuntimeInputSemantics,
        crate::ingress_types::RuntimeInputProjection,
        AdmissionPlan,
    ) {
        (
            self.policy,
            self.handling_mode,
            self.runtime_semantics,
            self.primitive_projection,
            self.admission_plan,
        )
    }
}

/// Typed reason why an input was rejected at the accept boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "reject_type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum RejectReason {
    /// Runtime is not in a state that accepts input (e.g. stopped, destroyed).
    NotReady {
        /// The runtime state that caused the rejection.
        state: RuntimeState,
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
    /// Peer response terminal fact failed typed validation.
    PeerResponseTerminalInvalid {
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
            Self::PeerResponseTerminalInvalid { detail } => write!(f, "{detail}"),
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
        /// Machine-owned lifecycle seed paired with `state`.
        seed: InputStateSeed,
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

/// Derive the handling mode from a resolved policy decision.
pub fn handling_mode_from_policy(policy: &PolicyDecision) -> HandlingMode {
    match policy.routing_disposition {
        crate::policy::RoutingDisposition::Steer | crate::policy::RoutingDisposition::Immediate => {
            // Immediate routing must use the steer lane so runtime-owned
            // semantic facts (for example terminal peer responses) cannot get
            // stranded behind ordinary queued prompts before their immediate
            // apply boundary is drained. This preserves the checked-in policy
            // contract without upgrading WakeIfIdle into an active-turn
            // interrupt on this branch.
            HandlingMode::Steer
        }
        _ => HandlingMode::Queue,
    }
}

/// Classify the machine-owned post-admission control signal for a resolved
/// accept outcome.
///
/// Admission-time wake / interrupt / immediate-processing semantics are owned
/// by the checked-in Meerkat machine. The runtime driver still carries the
/// accumulated DSL-emitted signal for loop mechanics, but plain accept
/// classification should flow through this typed helper.
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
            seed: InputStateSeed::new_accepted(),
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
            state: RuntimeState::Stopped,
        };
        assert_eq!(
            not_ready.to_string(),
            "runtime not accepting input while in state: stopped"
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

        let terminal = RejectReason::PeerResponseTerminalInvalid {
            detail: "correlation id cannot be empty".into(),
        };
        assert_eq!(terminal.to_string(), "correlation id cannot be empty");
    }

    #[test]
    fn reject_reason_serde_round_trip() {
        let reasons = vec![
            RejectReason::NotReady {
                state: RuntimeState::Destroyed,
            },
            RejectReason::DurabilityViolation {
                detail: "external derived".into(),
            },
            RejectReason::PeerHandlingModeInvalid {
                detail: "forbidden".into(),
            },
            RejectReason::PeerResponseTerminalInvalid {
                detail: "bad terminal".into(),
            },
        ];
        for reason in reasons {
            let json = serde_json::to_value(&reason).unwrap();
            let parsed: RejectReason = serde_json::from_value(json).unwrap();
            assert_eq!(parsed, reason);
        }
    }

    #[test]
    fn immediate_routing_uses_steer_handling_mode() {
        let policy = PolicyDecision {
            apply_mode: ApplyMode::InjectNow,
            wake_mode: WakeMode::WakeIfIdle,
            queue_mode: QueueMode::None,
            consume_point: ConsumePoint::OnApply,
            drain_policy: DrainPolicy::Immediate,
            routing_disposition: RoutingDisposition::Immediate,
            record_transcript: true,
            emit_operator_content: true,
            policy_version: PolicyVersion(1),
        };

        assert_eq!(handling_mode_from_policy(&policy), HandlingMode::Steer);
    }
}
