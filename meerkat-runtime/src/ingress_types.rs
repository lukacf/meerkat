//! Wire-type shells preserved from the deleted runtime ingress authority.
//!
//! These types name admission metadata that persists beyond the authority
//! itself. They are pure data — no authority methods, no shadow state. The
//! DSL owns ingress semantics (queue lanes, input phases, admission
//! ordering); these types just carry content-shape / correlation metadata
//! from the admission point to observability readers.

use meerkat_core::lifecycle::RuntimeExecutionKind;
use meerkat_core::lifecycle::run_primitive::{
    ConversationAppend, ConversationContextAppend, PeerResponseTerminalApplyIntent,
    RunApplyBoundary,
};
use serde::{Deserialize, Serialize};

use crate::identifiers::{InputKind, KindId};
use crate::policy::{ApplyMode, PolicyDecision};

/// Content shape classification for admitted inputs.
///
/// Used by the admitted-input snapshot surface so callers can correlate
/// admissions by content type without re-parsing the Input payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentShape(InputKind);

impl ContentShape {
    pub const fn from_kind(kind: InputKind) -> Self {
        Self(kind)
    }

    pub const fn from_kind_id(kind_id: KindId) -> Self {
        Self(kind_id.kind())
    }

    pub const fn kind(self) -> InputKind {
        self.0
    }

    pub fn as_str(self) -> &'static str {
        self.0.as_str()
    }
}

impl From<InputKind> for ContentShape {
    fn from(kind: InputKind) -> Self {
        Self::from_kind(kind)
    }
}

impl From<KindId> for ContentShape {
    fn from(kind_id: KindId) -> Self {
        Self::from_kind_id(kind_id)
    }
}

impl std::fmt::Display for ContentShape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeInputSemantics {
    pub boundary: RunApplyBoundary,
    pub execution_kind: RuntimeExecutionKind,
    pub peer_response_terminal_apply_intent: Option<PeerResponseTerminalApplyIntent>,
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
    fn boundary_from_policy(policy: &PolicyDecision) -> RunApplyBoundary {
        match policy.apply_mode {
            ApplyMode::StageRunBoundary => RunApplyBoundary::RunCheckpoint,
            ApplyMode::InjectNow => RunApplyBoundary::Immediate,
            ApplyMode::StageRunStart | ApplyMode::Ignore => RunApplyBoundary::RunStart,
        }
    }

    pub fn from_policy_and_execution_kind(
        policy: &PolicyDecision,
        execution_kind: RuntimeExecutionKind,
        peer_response_terminal_apply_intent: Option<PeerResponseTerminalApplyIntent>,
    ) -> Self {
        Self {
            boundary: Self::boundary_from_policy(policy),
            execution_kind,
            peer_response_terminal_apply_intent,
        }
    }

    pub fn from_policy_and_kind(policy: &PolicyDecision, kind: InputKind) -> Self {
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
        let peer_response_terminal_apply_intent = match kind {
            InputKind::PeerResponseTerminal => {
                Some(PeerResponseTerminalApplyIntent::AppendContextAndRun)
            }
            InputKind::Prompt
            | InputKind::PeerMessage
            | InputKind::PeerRequest
            | InputKind::PeerResponseProgress
            | InputKind::FlowStep
            | InputKind::ExternalEvent
            | InputKind::Continuation
            | InputKind::Operation => None,
        };
        Self::from_policy_and_execution_kind(
            policy,
            execution_kind,
            peer_response_terminal_apply_intent,
        )
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
        assert_eq!(
            semantics.peer_response_terminal_apply_intent,
            Some(PeerResponseTerminalApplyIntent::AppendContextAndRun)
        );
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
        assert_eq!(semantics.peer_response_terminal_apply_intent, None);
    }

    #[test]
    fn admitted_content_shape_is_closed_to_input_kind_contract() {
        let shapes = [
            (InputKind::Prompt, "prompt"),
            (InputKind::PeerMessage, "peer_message"),
            (InputKind::PeerRequest, "peer_request"),
            (InputKind::PeerResponseProgress, "peer_response_progress"),
            (InputKind::PeerResponseTerminal, "peer_response_terminal"),
            (InputKind::FlowStep, "flow_step"),
            (InputKind::ExternalEvent, "external_event"),
            (InputKind::Continuation, "continuation"),
            (InputKind::Operation, "operation"),
        ];

        for (kind, label) in shapes {
            let shape = ContentShape::from_kind(kind);
            assert_eq!(shape.kind(), kind);
            assert_eq!(shape.as_str(), label);
            assert_eq!(shape.to_string(), label);
        }
    }

    #[test]
    fn admitted_content_shape_source_has_no_string_newtype_contract() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .join("ingress_types.rs"),
        )
        .expect("read ingress types source");

        let forbidden = ["pub struct ContentShape", "(pub String)"].concat();
        assert!(
            !source.contains(&forbidden),
            "runtime admitted-input ContentShape must not be a public arbitrary string newtype"
        );
    }
}
