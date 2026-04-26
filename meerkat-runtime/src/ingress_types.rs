//! Wire-type shells preserved from the deleted runtime ingress authority.
//!
//! These types name admission metadata that persists beyond the authority
//! itself. They are pure data — no authority methods, no shadow state. The
//! DSL owns ingress semantics (queue lanes, input phases, admission
//! ordering); these types just carry content-shape / correlation metadata
//! from the admission point to observability readers.

use meerkat_core::lifecycle::RuntimeExecutionKind;
use meerkat_core::lifecycle::run_primitive::{
    ConversationAppend, ConversationContextAppend, RunApplyBoundary,
};

use crate::identifiers::InputKind;
use crate::policy::{ApplyMode, PolicyDecision};

/// Content shape classification for admitted inputs.
///
/// Used by the admitted-input snapshot surface so callers can correlate
/// admissions by content type without re-parsing the Input payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentShape(pub String);

/// Reservation key for admitted inputs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReservationKey(pub String);

/// Request ID for correlation tracking.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(pub String);

/// Machine-owned runtime-loop semantics captured at admission.
///
/// The runtime loop must not re-read peer conventions, continuation payloads,
/// or handling-mode hints to decide how a dequeued input runs. Admission has
/// already resolved the typed policy/kind tuple; this record is the canonical
/// carrier from that decision point to `RunPrimitive` construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeInputSemantics {
    pub boundary: RunApplyBoundary,
    pub execution_kind: RuntimeExecutionKind,
}

/// Admitted conversation projection for one input.
///
/// The raw input payload is still retained for durability/replay, but the
/// runtime loop consumes this admitted projection when constructing
/// `RunPrimitive` so dequeue mechanics do not reinterpret peer conventions or
/// terminal status payloads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeInputProjection {
    pub append: Option<ConversationAppend>,
    pub context_append: Option<ConversationContextAppend>,
}

impl RuntimeInputSemantics {
    pub fn from_policy_and_kind(policy: &PolicyDecision, kind: InputKind) -> Self {
        let boundary = match policy.apply_mode {
            ApplyMode::StageRunBoundary => RunApplyBoundary::RunCheckpoint,
            ApplyMode::InjectNow => RunApplyBoundary::Immediate,
            ApplyMode::StageRunStart | ApplyMode::Ignore => RunApplyBoundary::RunStart,
        };
        let execution_kind = match kind {
            InputKind::Continuation => RuntimeExecutionKind::ResumePending,
            InputKind::Prompt
            | InputKind::PeerMessage
            | InputKind::PeerRequest
            | InputKind::PeerResponseProgress
            | InputKind::PeerResponseTerminal
            | InputKind::FlowStep
            | InputKind::ExternalEvent
            | InputKind::Operation => RuntimeExecutionKind::ContentTurn,
        };
        Self {
            boundary,
            execution_kind,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{ConsumePoint, DrainPolicy, QueueMode, RoutingDisposition, WakeMode};

    fn policy(apply_mode: ApplyMode) -> PolicyDecision {
        PolicyDecision {
            apply_mode,
            wake_mode: WakeMode::WakeIfIdle,
            queue_mode: QueueMode::Fifo,
            consume_point: ConsumePoint::OnRunComplete,
            drain_policy: DrainPolicy::QueueNextTurn,
            routing_disposition: RoutingDisposition::Queue,
            record_transcript: true,
            emit_operator_content: true,
            policy_version: crate::policy_table::DEFAULT_POLICY_VERSION,
        }
    }

    #[test]
    fn terminal_peer_response_keeps_content_turn_execution_kind() {
        let semantics = RuntimeInputSemantics::from_policy_and_kind(
            &policy(ApplyMode::StageRunBoundary),
            InputKind::PeerResponseTerminal,
        );

        assert_eq!(semantics.boundary, RunApplyBoundary::RunCheckpoint);
        assert_eq!(semantics.execution_kind, RuntimeExecutionKind::ContentTurn);
    }

    #[test]
    fn continuation_is_the_only_resume_pending_execution_kind() {
        let semantics = RuntimeInputSemantics::from_policy_and_kind(
            &policy(ApplyMode::StageRunBoundary),
            InputKind::Continuation,
        );

        assert_eq!(semantics.boundary, RunApplyBoundary::RunCheckpoint);
        assert_eq!(
            semantics.execution_kind,
            RuntimeExecutionKind::ResumePending
        );
    }
}
