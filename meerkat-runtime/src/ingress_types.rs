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
use meerkat_core::types::HandlingMode;
use serde::{Deserialize, Serialize};

use crate::identifiers::{InputKind, KindId};

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
    pub(crate) boundary: RunApplyBoundary,
    pub(crate) execution_kind: RuntimeExecutionKind,
    pub(crate) execution_handling_mode: Option<HandlingMode>,
    pub(crate) peer_response_terminal_apply_intent: Option<PeerResponseTerminalApplyIntent>,
    /// #338: machine-owned verdict that this admitted input requires a live
    /// channel interrupt (true iff the admitted lane is `Steer`). Carried
    /// end-to-end so the live-projection consumer reads the typed fact instead
    /// of re-scanning `handling_mode == Steer`.
    #[serde(default)]
    pub(crate) live_interrupt_required: bool,
}

/// Admitted conversation projection for one input.
///
/// The raw input payload is still retained for durability/replay, but the
/// runtime loop consumes this admitted projection when constructing
/// `RunPrimitive` so dequeue mechanics do not reinterpret peer conventions or
/// terminal status payloads.
// Cannot derive `Eq`: `peer_response_terminal` carries a typed fact whose
// render payload is a `serde_json::Value`, which is `PartialEq` but not `Eq`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuntimeInputProjection {
    /// Host-attached injected-context appends staged immediately BEFORE
    /// `append` (the input's own transcript append) when the batch primitive
    /// is constructed. A distinct slot — not `additional_appends` — because
    /// the ordering invariant is "injected context lands before the turn's
    /// user/peer append" and `additional_appends` chain AFTER it.
    pub injected_context_appends: Vec<ConversationAppend>,
    pub append: Option<ConversationAppend>,
    pub additional_appends: Vec<ConversationAppend>,
    pub context_append: Option<ConversationContextAppend>,
    /// Typed terminal-peer-response fact this projection carries when the
    /// admitted input is a peer terminal response. The producer threads the
    /// typed [`meerkat_core::PeerResponseTerminalFact`] all the way to the
    /// realtime/live consumer instead of re-deriving it from the flattened
    /// `context_append` prose text.
    pub peer_response_terminal: Option<meerkat_core::PeerResponseTerminalFact>,
}

impl RuntimeInputSemantics {
    pub fn try_from_generated_admission(
        input: &crate::input::Input,
        runtime_idle: bool,
    ) -> Result<Self, String> {
        crate::policy_table::generated_admission_projection_for_input(input, runtime_idle)
            .map(|projection| projection.runtime_semantics)
    }

    pub fn boundary(&self) -> RunApplyBoundary {
        self.boundary
    }

    pub fn execution_kind(&self) -> RuntimeExecutionKind {
        self.execution_kind
    }

    pub fn peer_response_terminal_apply_intent(&self) -> Option<PeerResponseTerminalApplyIntent> {
        self.peer_response_terminal_apply_intent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_peer_response_keeps_content_turn_execution_kind() {
        let semantics = crate::policy_table::generated_admission_projection_for_kind(
            KindId::new(InputKind::PeerResponseTerminal),
            false,
        )
        .expect("generated admission projection")
        .runtime_semantics;

        assert_eq!(semantics.boundary, RunApplyBoundary::RunStart);
        assert_eq!(semantics.execution_kind, RuntimeExecutionKind::ContentTurn);
        assert_eq!(semantics.execution_handling_mode, None);
        assert_eq!(
            semantics.peer_response_terminal_apply_intent,
            Some(PeerResponseTerminalApplyIntent::AppendContextAndRun)
        );
    }

    #[test]
    fn continuation_is_the_only_resume_pending_execution_kind() {
        let semantics = crate::policy_table::generated_admission_projection_for_kind(
            KindId::new(InputKind::Continuation),
            false,
        )
        .expect("generated admission projection")
        .runtime_semantics;

        assert_eq!(semantics.boundary, RunApplyBoundary::RunCheckpoint);
        assert_eq!(
            semantics.execution_kind,
            RuntimeExecutionKind::ResumePending
        );
        assert_eq!(semantics.execution_handling_mode, None);
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
