//! Cross-crate DSL handle traits.
//!
//! Downstream crates (`meerkat-mcp`, `meerkat-comms`, `meerkat-session`) drive
//! DSL transitions through these trait objects without importing
//! `meerkat-runtime`. Concrete impls live in `meerkat-runtime`, where the DSL
//! authority lives.
//!
//! The mob side (`meerkat-mob`) already depends on `meerkat-runtime` and owns
//! its MobMachine DSL authority in-crate, so it drives DSL transitions via
//! direct `dsl_authority.apply(...)` calls — no cross-crate trait required.
//!
//! Trait methods are named per-DSL input, not per-authority input.
//! DSL-owned discriminants (turn phase, drain mode, surface phase, surface
//! pending/staged op, auth lease phase) flow as typed enums defined here in
//! `meerkat-core` — each maps 1-to-1 with the typed DSL state that
//! [`meerkat-runtime::meerkat_machine`] owns. Free-form `String` values are
//! reserved for opaque identifiers (surface ids, binding keys, error
//! messages) and for DSL fields that are still literal-string today; those
//! will retype in lockstep with their DSL slot.
//!
//! Return type is `Result<(), DslTransitionError>`. The DSL decides legality;
//! phase/field reads happen elsewhere (direct DSL state accessors, not via
//! these traits).

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::LoopState;
use crate::comms::InputSource;
use crate::lifecycle::run_primitive::ModelId;
use crate::lifecycle::{InputId, RunId};
use crate::ops::{AsyncOpRef, OperationId};
use crate::peer_correlation::{
    InboundPeerRequestState, InteractionStreamState, OutboundPeerRequestState, PeerCorrelationId,
};
use crate::retry::LlmRetrySchedule;
use crate::tool_scope::{
    ExternalToolSurfaceBaseState, ExternalToolSurfaceDeltaOperation, ExternalToolSurfaceDeltaPhase,
    ExternalToolSurfaceGlobalPhase, ExternalToolSurfacePendingOp, ExternalToolSurfaceStagedOp,
};
use crate::turn_execution_authority::{
    TurnFailureReason, TurnPhase, TurnPrimitiveKind, TurnTerminalOutcome,
};
use crate::types::SessionId;

// ---------------------------------------------------------------------------
// Typed cross-crate enums for DSL-owned discriminants.
//
// Each maps 1-to-1 with the typed DSL state that meerkat-runtime's
// MeerkatMachine / AuthMachine own. The runtime handle impls do a single
// exhaustive `match` from DSL-typed to handle-typed — no string parsing,
// no `_ => default` arms, no parallel adapters.
// ---------------------------------------------------------------------------

/// Mode for a comms drain task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainMode {
    /// Legacy timed drain with idle timeout.
    Timed,
    /// Live session ingress while a runtime-backed session is attached.
    AttachedSession,
    /// Long-lived host drain (no idle timeout, respawnable on failure).
    PersistentHost,
}

/// Reason a drain task exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainExitReason {
    IdleTimeout,
    Dismissed,
    Failed,
    Aborted,
    SessionShutdown,
}

/// Session model-routing baseline handle.
///
/// Runtime-backed surfaces create sessions before the factory has resolved the
/// final LLM identity. The factory uses this handle after resolution so the DSL
/// owns the canonical baseline model before tools such as `generate_image`
/// resolve `Auto` provider targets.
pub trait ModelRoutingHandle: Send + Sync {
    /// Set the session's canonical model-routing baseline.
    fn set_baseline(
        &self,
        baseline_model: ModelId,
        realtime_capable: bool,
    ) -> Result<(), DslTransitionError>;
}

impl DrainExitReason {
    /// Stable discriminant for wire logging (drain exit reason is not yet a
    /// typed DSL field; the handle passes the discriminant through).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IdleTimeout => "IdleTimeout",
            Self::Dismissed => "Dismissed",
            Self::Failed => "Failed",
            Self::Aborted => "Aborted",
            Self::SessionShutdown => "SessionShutdown",
        }
    }
}

/// Auth lease lifecycle phase, projected from the per-binding AuthMachine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthLeasePhase {
    Valid,
    Expiring,
    Refreshing,
    ReauthRequired,
    Released,
}

/// Typed classification of why a DSL transition was rejected.
///
/// Emitted by the generated kernel's `apply` / `apply_signal` methods and
/// bridged into [`DslTransitionError::kind`]. Callers that fire
/// idempotently (realtime dispatchers, monotonic watermark advances,
/// etc.) inspect this to distinguish "input was out of scope for this
/// phase" (a real error) from "input was recognised but the guard dropped
/// it" (a successful no-op).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DslRejectionKind {
    /// No transition is declared for this `(phase, trigger)` pair — the
    /// shell fired an input that is semantically out of scope for the
    /// current phase. This is a programming mistake on the shell side.
    NoMatchingTransition,
    /// A transition is declared for this `(phase, trigger)` pair but
    /// every candidate transition's guard evaluated false. Callers
    /// firing idempotently treat this as a no-op; callers firing
    /// unconditionally treat it as a user-visible error.
    GuardRejected,
}

/// Error surfaced when a DSL transition is rejected.
///
/// Wraps the generated kernel's typed rejection. Trait impls populate
/// `context` from the trait method name so callers can tell which handle
/// rejected; `kind` lets callers distinguish guard rejection from
/// out-of-scope input without substring-matching the rendered message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("DSL transition rejected in {context}: {reason}")]
pub struct DslTransitionError {
    /// Name of the trait method / DSL variant whose transition was rejected.
    pub context: &'static str,
    /// Typed classification of the rejection — see [`DslRejectionKind`].
    pub kind: DslRejectionKind,
    /// Underlying rejection reason (typically the generated
    /// `NoMatchingTransition`/`GuardRejected` formatted).
    pub reason: String,
}

impl DslTransitionError {
    /// Construct a `NoMatchingTransition` error with the given context
    /// and reason. Back-compat constructor used by legacy callers that
    /// haven't adopted the typed `kind` field; new code paths should
    /// prefer [`DslTransitionError::no_matching`] or
    /// [`DslTransitionError::guard_rejected`] for clarity.
    pub fn new(context: &'static str, reason: impl Into<String>) -> Self {
        Self::no_matching(context, reason)
    }

    /// Construct an error with `kind = NoMatchingTransition`.
    pub fn no_matching(context: &'static str, reason: impl Into<String>) -> Self {
        Self {
            context,
            kind: DslRejectionKind::NoMatchingTransition,
            reason: reason.into(),
        }
    }

    /// Construct an error with `kind = GuardRejected`.
    pub fn guard_rejected(context: &'static str, reason: impl Into<String>) -> Self {
        Self {
            context,
            kind: DslRejectionKind::GuardRejected,
            reason: reason.into(),
        }
    }

    /// True iff this rejection came from a guard evaluating false.
    pub fn is_guard_rejected(&self) -> bool {
        self.kind == DslRejectionKind::GuardRejected
    }
}

// ---------------------------------------------------------------------------
// Cross-crate peer prompt/context projection seam
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerResponseProgressProjectionPhase {
    Accepted,
    InProgress,
    PartialResult,
}

impl PeerResponseProgressProjectionPhase {
    fn label(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::InProgress => "in_progress",
            Self::PartialResult => "partial_result",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerResponseTerminalProjectionStatus {
    Completed,
    Failed,
    Cancelled,
}

impl PeerResponseTerminalProjectionStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PeerConversationProjection {
    Message {
        peer_id: String,
    },
    Request {
        peer_id: crate::comms::PeerId,
        display_name: Option<String>,
        request_id: String,
        intent: String,
        payload: Option<serde_json::Value>,
    },
    ResponseProgress {
        peer_id: String,
        request_id: String,
        phase: PeerResponseProgressProjectionPhase,
        payload: Option<serde_json::Value>,
    },
    ResponseTerminal {
        peer_id: String,
        request_id: String,
        status: PeerResponseTerminalProjectionStatus,
        payload: Option<serde_json::Value>,
    },
}

impl PeerConversationProjection {
    pub fn block_prefix_text(&self) -> Option<String> {
        match self {
            Self::Message { peer_id } => Some(format!("[COMMS MESSAGE from {peer_id}]")),
            Self::Request { .. }
            | Self::ResponseProgress { .. }
            | Self::ResponseTerminal { .. } => None,
        }
    }

    pub fn prompt_text(&self) -> String {
        match self {
            Self::Message { .. } => String::new(),
            Self::Request {
                peer_id,
                display_name,
                request_id,
                intent,
                payload,
            } => {
                let display_suffix = display_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(|name| format!(" (display_name: {name})"))
                    .unwrap_or_default();
                let response_call = crate::interaction::SendResponseCallProjection::new(
                    *peer_id,
                    display_name.as_deref(),
                    request_id.clone(),
                );
                format!(
                    "[SYSTEM NOTICE][PEER_REQUEST] Correlated peer request from peer_id {peer_id}{display_suffix}. Intent: {intent}. Request ID: {request_id}. Params: {}. This is not a normal user request and not a prompt for direct user-facing output. {} Do not use send_message for this reply.",
                    format_peer_projection_payload(payload.as_ref()),
                    response_call.instruction_text()
                )
            }
            Self::ResponseProgress {
                peer_id,
                request_id,
                phase,
                payload,
            } => format!(
                "[SYSTEM NOTICE][PEER_RESPONSE_PROGRESS] Correlated peer response progress from {peer_id}. Request ID: {request_id}. Phase: {}. Payload: {}.",
                phase.label(),
                format_peer_projection_payload(payload.as_ref())
            ),
            Self::ResponseTerminal {
                peer_id,
                request_id,
                status,
                payload,
            } => format!(
                "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] Correlated peer response from {peer_id}. Request ID: {request_id}. Status: {}. Result: {}.",
                status.label(),
                format_peer_projection_payload(payload.as_ref())
            ),
        }
    }

    pub fn context_key(&self) -> Option<String> {
        match self {
            Self::ResponseTerminal {
                peer_id,
                request_id,
                ..
            } => Some(peer_response_terminal_context_key(peer_id, request_id)),
            Self::Message { .. } | Self::Request { .. } | Self::ResponseProgress { .. } => None,
        }
    }
}

pub fn peer_response_terminal_context_key(peer_id: &str, request_id: &str) -> String {
    format!("peer_response_terminal:{peer_id}:{request_id}")
}

fn format_peer_projection_payload(payload: Option<&serde_json::Value>) -> String {
    serde_json::to_string_pretty(payload.unwrap_or(&serde_json::Value::Null))
        .unwrap_or_else(|_| "null".to_string())
}

// ---------------------------------------------------------------------------
// TurnStateHandle
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnStateSnapshot {
    pub active_run_id: Option<RunId>,
    /// Observable loop-state projection supplied by the turn-state owner.
    ///
    /// Consumers should not reclassify [`TurnPhase`] locally. Runtime-backed
    /// handles derive this from the same DSL snapshot as `turn_phase`; test
    /// handles do the same from their in-core test state.
    pub loop_state: LoopState,
    pub turn_phase: TurnPhase,
    /// Typed primitive kind recorded by the DSL (dogma #5, #19 — no stringly
    /// discriminants). `None` means no primitive is currently in flight.
    pub primitive_kind: Option<TurnPrimitiveKind>,
    pub admitted_content_shape: Option<String>,
    pub vision_enabled: bool,
    pub image_tool_results_enabled: bool,
    pub tool_calls_pending: u64,
    pub pending_op_refs: BTreeSet<AsyncOpRef>,
    pub barrier_operation_ids: BTreeSet<OperationId>,
    pub has_barrier_ops: bool,
    pub barrier_satisfied: bool,
    pub boundary_count: u64,
    pub cancel_after_boundary: bool,
    /// Typed terminal outcome recorded by the DSL (dogma #5, #19 — no stringly
    /// discriminants). `None` means the turn has not reached a terminal phase.
    pub terminal_outcome: Option<TurnTerminalOutcome>,
    pub extraction_attempts: u64,
    pub max_extraction_retries: u64,
    pub llm_retry_attempt: u32,
    pub llm_retry_max_retries: u32,
    pub llm_retry_selected_delay_ms: u64,
}

/// Turn-execution DSL handle.
pub trait TurnStateHandle: Send + Sync {
    fn start_conversation_run(
        &self,
        run_id: RunId,
        primitive_kind: TurnPrimitiveKind,
        admitted_content_shape: String,
        vision_enabled: bool,
        image_tool_results_enabled: bool,
        max_extraction_retries: u64,
    ) -> Result<(), DslTransitionError>;

    fn start_immediate_append(
        &self,
        run_id: RunId,
        admitted_content_shape: String,
    ) -> Result<(), DslTransitionError>;

    fn start_immediate_context(
        &self,
        run_id: RunId,
        admitted_content_shape: String,
    ) -> Result<(), DslTransitionError>;

    fn primitive_applied(&self) -> Result<(), DslTransitionError>;

    fn llm_returned_tool_calls(&self, tool_count: u64) -> Result<(), DslTransitionError>;

    fn llm_returned_terminal(&self) -> Result<(), DslTransitionError>;

    fn register_pending_ops(
        &self,
        op_refs: BTreeSet<AsyncOpRef>,
        barrier_operation_ids: BTreeSet<OperationId>,
    ) -> Result<(), DslTransitionError>;

    fn tool_calls_resolved(&self) -> Result<(), DslTransitionError>;

    fn ops_barrier_satisfied(
        &self,
        operation_ids: BTreeSet<OperationId>,
    ) -> Result<(), DslTransitionError>;

    fn boundary_continue(&self) -> Result<(), DslTransitionError>;

    fn boundary_complete(&self) -> Result<(), DslTransitionError>;

    fn enter_extraction(&self, max_retries: u32) -> Result<(), DslTransitionError>;

    fn extraction_start(&self) -> Result<(), DslTransitionError>;

    fn extraction_validation_passed(&self) -> Result<(), DslTransitionError>;

    fn extraction_validation_failed(&self, error: String) -> Result<(), DslTransitionError>;

    fn recoverable_failure(&self, retry: LlmRetrySchedule) -> Result<(), DslTransitionError>;

    fn fatal_failure(&self, reason: TurnFailureReason) -> Result<(), DslTransitionError>;

    fn retry_requested(&self, retry_attempt: u32) -> Result<(), DslTransitionError>;

    fn cancel_now(&self) -> Result<(), DslTransitionError>;

    fn request_cancel_after_boundary(&self) -> Result<(), DslTransitionError>;

    fn cancellation_observed(&self) -> Result<(), DslTransitionError>;

    fn acknowledge_terminal(&self, outcome: TurnTerminalOutcome) -> Result<(), DslTransitionError>;

    fn turn_limit_reached(&self) -> Result<(), DslTransitionError>;

    fn budget_exhausted(&self) -> Result<(), DslTransitionError>;

    fn time_budget_exceeded(&self) -> Result<(), DslTransitionError>;

    fn force_cancel_no_run(&self) -> Result<(), DslTransitionError>;

    fn run_completed(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn run_failed(
        &self,
        run_id: RunId,
        reason: TurnFailureReason,
    ) -> Result<(), DslTransitionError>;

    fn run_cancelled(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn snapshot(&self) -> TurnStateSnapshot;
}

// ---------------------------------------------------------------------------
// CommsDrainHandle
// ---------------------------------------------------------------------------

/// Comms drain lifecycle DSL handle.
///
/// Covers the `drain_phase`/`drain_mode` DSL substate: ensure/spawn/stop the
/// comms drain task and observe clean vs respawnable exits.
pub trait CommsDrainHandle: Send + Sync {
    /// Fire the `EnsureDrainRunning` signal — lazy spawn path.
    fn ensure_drain_running(&self) -> Result<(), DslTransitionError>;

    /// Fire the `SpawnDrain { mode }` input — explicit spawn with typed mode.
    fn spawn_drain(&self, mode: DrainMode) -> Result<(), DslTransitionError>;

    /// Fire the `StopDrain` input.
    fn stop_drain(&self) -> Result<(), DslTransitionError>;

    /// Fire the `DrainExitedClean` input (drain stopped without failure).
    fn drain_exited_clean(&self) -> Result<(), DslTransitionError>;

    /// Fire the `DrainExitedRespawnable` input (drain exited and can be respawned).
    fn drain_exited_respawnable(&self) -> Result<(), DslTransitionError>;

    /// Fire the `NotifyDrainExited { reason }` input with a typed reason.
    fn notify_drain_exited(&self, reason: DrainExitReason) -> Result<(), DslTransitionError>;
}

// ---------------------------------------------------------------------------
// ExternalToolSurfaceHandle
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceSnapshot {
    pub surface_id: String,
    /// Typed base lifecycle state (dogma #5, #17 — no stringly discriminants
    /// across the cross-crate handle boundary).
    pub base_state: Option<ExternalToolSurfaceBaseState>,
    pub pending_op: ExternalToolSurfacePendingOp,
    pub staged_op: ExternalToolSurfaceStagedOp,
    pub staged_intent_sequence: Option<u64>,
    pub pending_task_sequence: Option<u64>,
    pub pending_lineage_sequence: Option<u64>,
    pub inflight_calls: u64,
    /// Typed last-emitted delta operation (dogma #5, #17).
    pub last_delta_operation: Option<ExternalToolSurfaceDeltaOperation>,
    /// Typed last-emitted delta phase (dogma #5, #17).
    pub last_delta_phase: Option<ExternalToolSurfaceDeltaPhase>,
    pub removal_draining_since_ms: Option<u64>,
    pub removal_timeout_at_ms: Option<u64>,
    pub removal_applied_at_turn: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceDiagnosticSnapshot {
    pub surface_phase: ExternalToolSurfaceGlobalPhase,
    pub known_surfaces: BTreeSet<String>,
    pub visible_surfaces: BTreeSet<String>,
    pub snapshot_epoch: u64,
    pub snapshot_aligned_epoch: u64,
    pub has_pending_or_staged: bool,
    pub entries: Vec<SurfaceSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalToolSurfaceInput {
    StageAdd {
        surface_id: String,
        now_ms: u64,
    },
    StageRemove {
        surface_id: String,
        now_ms: u64,
    },
    StageReload {
        surface_id: String,
        now_ms: u64,
    },
    ApplyBoundary {
        surface_id: String,
        now_ms: u64,
        staged_intent_sequence: u64,
        applied_at_turn: u64,
    },
    MarkPendingSucceeded {
        surface_id: String,
        pending_task_sequence: u64,
        staged_intent_sequence: u64,
    },
    MarkPendingFailed {
        surface_id: String,
        pending_task_sequence: u64,
        staged_intent_sequence: u64,
        reason: String,
    },
    CallStarted {
        surface_id: String,
    },
    CallFinished {
        surface_id: String,
    },
    FinalizeRemovalClean {
        surface_id: String,
    },
    FinalizeRemovalForced {
        surface_id: String,
    },
    SnapshotAligned {
        epoch: u64,
    },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalToolSurfaceEffect {
    ScheduleSurfaceCompletion {
        surface_id: String,
        operation: ExternalToolSurfaceDeltaOperation,
        pending_task_sequence: u64,
        staged_intent_sequence: u64,
        applied_at_turn: u64,
    },
    RefreshVisibleSurfaceSet {
        snapshot_epoch: u64,
    },
    EmitExternalToolDelta {
        surface_id: String,
        operation: ExternalToolSurfaceDeltaOperation,
        phase: ExternalToolSurfaceDeltaPhase,
    },
    CloseSurfaceConnection {
        surface_id: String,
    },
    RejectSurfaceCall {
        surface_id: String,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalToolSurfaceTransition {
    pub phase: ExternalToolSurfaceGlobalPhase,
    pub effects: Vec<ExternalToolSurfaceEffect>,
}

/// External tool surface lifecycle DSL handle.
pub trait ExternalToolSurfaceHandle: Send + Sync {
    fn apply_surface_input(
        &self,
        input: ExternalToolSurfaceInput,
    ) -> Result<ExternalToolSurfaceTransition, DslTransitionError>;

    fn register(&self, surface_id: String) -> Result<(), DslTransitionError>;

    fn stage_add(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError>;

    fn stage_remove(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError>;

    fn stage_reload(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError>;

    fn apply_boundary(
        &self,
        surface_id: String,
        now_ms: u64,
        staged_intent_sequence: u64,
        applied_at_turn: u64,
    ) -> Result<(), DslTransitionError>;

    fn mark_pending_succeeded(
        &self,
        surface_id: String,
        pending_task_sequence: u64,
        staged_intent_sequence: u64,
    ) -> Result<(), DslTransitionError>;

    fn mark_pending_failed(
        &self,
        surface_id: String,
        pending_task_sequence: u64,
        staged_intent_sequence: u64,
        reason: String,
    ) -> Result<(), DslTransitionError>;

    fn call_started(&self, surface_id: String) -> Result<(), DslTransitionError>;

    fn call_finished(&self, surface_id: String) -> Result<(), DslTransitionError>;

    fn finalize_removal_clean(&self, surface_id: String) -> Result<(), DslTransitionError>;

    fn finalize_removal_forced(&self, surface_id: String) -> Result<(), DslTransitionError>;

    fn snapshot_aligned(&self, epoch: u64) -> Result<(), DslTransitionError>;

    fn shutdown_surface(&self) -> Result<(), DslTransitionError>;

    fn surface_snapshot(&self, surface_id: &str) -> Option<SurfaceSnapshot>;

    fn diagnostic_snapshot(&self) -> SurfaceDiagnosticSnapshot;

    fn visible_surfaces(&self) -> BTreeSet<String>;

    fn removing_surfaces(&self) -> BTreeSet<String>;

    fn pending_surfaces(&self) -> BTreeSet<String>;

    fn has_pending_or_staged(&self) -> bool;

    fn snapshot_epoch(&self) -> u64;

    fn snapshot_aligned_epoch(&self) -> u64;
}

// ---------------------------------------------------------------------------
// PeerCommsHandle
// ---------------------------------------------------------------------------

/// Peer comms ingress classification DSL handle.
///
/// Covers the peer-envelope classification signals on the MeerkatMachine DSL.
/// Runtime-backed comms ingress must fire one of these signals before the
/// comms shell computes its stored compatibility projection (`PeerInputClass`,
/// auth exemption, lifecycle subject, rendered text). A rejection is
/// authoritative and callers fail closed. Standalone comms runtimes may have
/// no session DSL handle; those retain the historical in-process projection
/// path for wire compatibility.
pub trait PeerCommsHandle: Send + Sync {
    /// Fire the `ClassifyExternalEnvelope` signal.
    fn classify_external_envelope(&self) -> Result<(), DslTransitionError>;

    /// Fire the `ClassifyPlainEvent` signal.
    fn classify_plain_event(&self) -> Result<(), DslTransitionError>;

    /// Fire the `SetPeerIngressContext { keep_alive }` input.
    fn set_peer_ingress_context(&self, keep_alive: bool) -> Result<(), DslTransitionError>;
}

// ---------------------------------------------------------------------------
// SessionAdmissionHandle
// ---------------------------------------------------------------------------

/// Session turn admission DSL handle.
///
/// Covers the admission-adjacent inputs on the MeerkatMachine DSL: ingest an
/// input into the session, accept it (with or without wake), prepare a run,
/// and commit or fail the run. These inputs manage the input-lifecycle
/// substate maps (`input_phases`, `input_run_associations`, etc.) and the
/// top-level `current_run_id` / `pre_run_phase` fields.
pub trait SessionAdmissionHandle: Send + Sync {
    /// Fire the `Ingest { runtime_id, work_id, origin }` input.
    ///
    /// `runtime_id` is the stringified logical runtime id; `work_id` the
    /// stringified work identifier (typically the same domain as `InputId`).
    /// `origin` is the typed transport source that admitted the input
    /// (dogma #5, #17 — no stringly discriminants across the handle boundary).
    fn ingest(
        &self,
        runtime_id: &str,
        work_id: &str,
        origin: InputSource,
    ) -> Result<(), DslTransitionError>;

    /// Fire the `AcceptWithCompletion { input_id, request_immediate_processing,
    /// interrupt_yielding, wake_if_idle, run_id }` input.
    ///
    /// `wake_if_idle` carries the policy-level "this input must wake the
    /// runtime loop once the session reaches idle" intent (e.g.
    /// `peer_response_terminal` staged while running): the DSL's
    /// Running+Queued transition splits on it and emits a
    /// `PostAdmissionSignal::WakeLoop` so the pending wake lands on the
    /// next idle reach. Idle/Attached queued arms already wake
    /// unconditionally, so the flag is ignored in those guards.
    fn accept_with_completion(
        &self,
        input_id: &InputId,
        request_immediate_processing: bool,
        interrupt_yielding: bool,
        wake_if_idle: bool,
    ) -> Result<(), DslTransitionError>;

    /// Fire the `AcceptWithoutWake { input_id }` input.
    fn accept_without_wake(&self, input_id: &InputId) -> Result<(), DslTransitionError>;

    /// Fire the `Prepare { session_id, run_id }` input — bound for the session this handle was prepared for.
    fn prepare(&self, run_id: &RunId) -> Result<(), DslTransitionError>;

    /// Fire the `Commit { input_id, run_id }` input.
    fn commit(&self, input_id: &InputId, run_id: &RunId) -> Result<(), DslTransitionError>;

    /// Fire the `Fail { run_id }` input.
    fn fail(&self, run_id: &RunId) -> Result<(), DslTransitionError>;

    /// Fire the `Recycle` input — session transitions into the recycle path.
    fn recycle(&self) -> Result<(), DslTransitionError>;
}

// ---------------------------------------------------------------------------
// AuthLeaseHandle (Phase 1.5-rev)
// ---------------------------------------------------------------------------

/// Typed key for one auth lease machine.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LeaseKey {
    pub realm: crate::connection::RealmId,
    pub binding: crate::connection::BindingId,
    pub profile: Option<crate::connection::ProfileId>,
}

impl LeaseKey {
    pub fn new(
        realm: crate::connection::RealmId,
        binding: crate::connection::BindingId,
        profile: Option<crate::connection::ProfileId>,
    ) -> Self {
        Self {
            realm,
            binding,
            profile,
        }
    }

    pub fn from_connection_ref(connection_ref: &crate::connection::ConnectionRef) -> Self {
        Self {
            realm: connection_ref.realm.clone(),
            binding: connection_ref.binding.clone(),
            profile: connection_ref.profile.clone(),
        }
    }
}

impl std::fmt::Display for LeaseKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.profile {
            Some(profile) => write!(f, "{}:{}:{}", self.realm, self.binding, profile),
            None => write!(f, "{}:{}", self.realm, self.binding),
        }
    }
}

/// Observable snapshot of an auth lease's DSL state for a given [`LeaseKey`].
///
/// Returned by [`AuthLeaseHandle::snapshot`]. If the binding is not tracked
/// at all, `phase` is `None` and `expires_at` is `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthLeaseSnapshot {
    pub phase: Option<AuthLeasePhase>,
    pub expires_at: Option<u64>,
}

/// Window (in seconds) before `expires_at` at which a `valid` lease is
/// eligible to transition into `expiring` at the next CallingLlm
/// boundary. Owned here — on the handle trait module — rather than in
/// shell code, per dogma §9 ("policy composes at the facade/factory
/// seam, not in random helpers") and §20 ("every important behavior
/// reduces to one clear owner").
///
/// The actual state transition is gated by the AuthMachine DSL's
/// `MarkAuthExpiring` input (which enforces the `valid → expiring`
/// legality); this constant only controls *when* the runner fires
/// that input, not whether the transition is legal.
pub const AUTH_LEASE_TTL_REFRESH_WINDOW_SECS: u64 = 60;

/// Auth lease lifecycle DSL handle.
pub trait AuthLeaseHandle: Send + Sync {
    /// Fire `AcquireAuthLease { lease_key, expires_at }` — unconditional.
    ///
    /// Moves the binding into `auth_valid_leases` and records its expiry.
    fn acquire_lease(
        &self,
        lease_key: &LeaseKey,
        expires_at: u64,
    ) -> Result<(), DslTransitionError>;

    /// Fire `MarkAuthExpiring { lease_key }` — only legal from `valid`.
    fn mark_expiring(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError>;

    /// Fire `BeginAuthRefresh { lease_key }` — legal from `valid` or
    /// `expiring`.
    ///
    /// Provides the DSL-level refresh dedup: once the binding is in
    /// `auth_refreshing_leases`, no concurrent `BeginAuthRefresh` is
    /// permitted until `CompleteAuthRefresh` or `AuthRefreshFailed` moves
    /// it back out.
    fn begin_refresh(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError>;

    /// Fire `CompleteAuthRefresh { lease_key, new_expires_at, now }` — only
    /// legal from `refreshing`.
    fn complete_refresh(
        &self,
        lease_key: &LeaseKey,
        new_expires_at: u64,
        now: u64,
    ) -> Result<(), DslTransitionError>;

    /// Fire `AuthRefreshFailed { lease_key, permanent }` — only legal from
    /// `refreshing`. `permanent=true` routes to `reauth_required` and emits a
    /// reauth notice; `permanent=false` routes back to `expiring`.
    fn refresh_failed(
        &self,
        lease_key: &LeaseKey,
        permanent: bool,
    ) -> Result<(), DslTransitionError>;

    /// Fire `MarkReauthRequired { lease_key }` — any known state → reauth.
    fn mark_reauth_required(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError>;

    /// Fire `ReleaseAuthLease { lease_key }` — removes the binding from all
    /// sets and the expiry map.
    fn release_lease(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError>;

    /// Observe the current DSL-level state of a binding.
    fn snapshot(&self, lease_key: &LeaseKey) -> AuthLeaseSnapshot;
}

// ---------------------------------------------------------------------------
// McpServerLifecycleHandle (Phase 5G / T5g)
// ---------------------------------------------------------------------------

/// MCP client handshake lifecycle DSL handle (session-scoped).
///
/// Routes each per-server MCP handshake event into the MeerkatMachine DSL's
/// `mcp_server_states` substate. Distinct from the external-tool surface
/// lifecycle (which tracks staged/pending *tool surface* intents): this handle
/// tracks per-server *connection* lifecycle (PendingConnect → Connected |
/// Failed | Disconnected), keyed by the configured MCP server name.
///
/// Read side (`pending_server_ids`) is the authoritative source for the
/// `[MCP_PENDING]` system-notice toggle — any server in `PendingConnect` means
/// the notice is emitted; otherwise the notice is suppressed.
///
/// Concrete impls live in `meerkat-runtime`; standalone callers (tests,
/// fixtures) pass `None` for the handle and the router's shell-level behavior
/// remains identical (DSL record-keeping is skipped, which is fine because
/// there is no session DSL to mirror into).
pub trait McpServerLifecycleHandle: Send + Sync {
    /// Fire `McpServerConnectPending { server_id }` — server staged for
    /// background connect.
    fn apply_connect_pending(&self, server_id: &str) -> Result<(), DslTransitionError>;

    /// Fire `McpServerConnected { server_id }` — handshake succeeded.
    fn apply_connected(&self, server_id: &str) -> Result<(), DslTransitionError>;

    /// Fire `McpServerFailed { server_id, error }` — handshake failed.
    fn apply_failed(&self, server_id: &str, error: &str) -> Result<(), DslTransitionError>;

    /// Fire `McpServerDisconnected { server_id }` — connection closed.
    fn apply_disconnected(&self, server_id: &str) -> Result<(), DslTransitionError>;

    /// Fire `McpServerReload { server_id }` — reload requested; server returns
    /// to `PendingConnect` while the shell tears down and redials.
    fn apply_reload(&self, server_id: &str) -> Result<(), DslTransitionError>;

    /// Observe the set of server ids currently in `PendingConnect`.
    ///
    /// Used by the agent loop to drive the `[MCP_PENDING]` system-notice
    /// lifecycle: non-empty → emit notice; empty → strip notice.
    fn pending_server_ids(&self) -> BTreeSet<String>;
}

// ---------------------------------------------------------------------------
// PeerInteractionHandle (W1-A / issue #264)
// ---------------------------------------------------------------------------

/// Terminal disposition companion for [`PeerInteractionHandle::response_terminal`].
///
/// Carried as a typed wire value so the DSL can route `Completed` / `Failed`
/// terminal transitions without the shell re-interpreting `ResponseStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PeerTerminalDisposition {
    /// Terminal response with `Completed` status.
    Completed,
    /// Terminal response with `Failed` status.
    Failed,
}

/// Peer request / response lifecycle DSL handle (W1-A).
///
/// Routes the full peer-interaction lifecycle — outbound `Sent`,
/// progress / terminal response arrival, timeouts, and inbound
/// `Received` / `Replied` — into the MeerkatMachine DSL's
/// `pending_peer_requests` / `inbound_peer_requests` substate maps.
///
/// Terminal transitions emit a DSL-owned cleanup effect that the shell
/// observes to drop any subscriber / stream channel associated with the
/// correlation id. The channels themselves live in shell-owned maps (they
/// hold `mpsc::Sender` values that cannot live in DSL state); those maps
/// are strict projections of DSL state, with the invariant "channel live
/// iff `corr_id ∈ pending ∧ state ≠ terminal`" enforced by the effect.
pub trait PeerInteractionHandle: Send + Sync {
    /// Fire `PeerRequestSent { corr_id, to }`.
    ///
    /// Guard: `corr_id` is not already in `pending_peer_requests`.
    fn request_sent(
        &self,
        corr_id: PeerCorrelationId,
        to: String,
    ) -> Result<(), DslTransitionError>;

    /// Fire `PeerResponseProgressArrived { corr_id }`.
    ///
    /// Guard: `corr_id` is in `pending_peer_requests`. Progress after
    /// progress is admitted as a self-loop (the DSL overwrites the state
    /// slot). Rejects on unknown corr_id.
    fn response_progress(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Fire `PeerResponseTerminalArrived { corr_id, disposition }`.
    ///
    /// Guard: `corr_id` is in `pending_peer_requests`. Terminal transitions
    /// remove the map entry and emit the `PeerInteractionCleanup` effect,
    /// so any second terminal on the same corr_id is rejected at the
    /// `pending_exists` guard by construction.
    fn response_terminal(
        &self,
        corr_id: PeerCorrelationId,
        disposition: PeerTerminalDisposition,
    ) -> Result<(), DslTransitionError>;

    /// Fire `PeerRequestTimedOut { corr_id }`.
    ///
    /// Guard: `corr_id` is in `pending_peer_requests`. Like `response_terminal`,
    /// the map entry is removed on success and the `PeerInteractionCleanup`
    /// effect is emitted; subsequent fires fail the guard.
    fn request_timed_out(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Fire `PeerRequestReceived { corr_id }` (inbound).
    ///
    /// Guard: `corr_id` is not already in `inbound_peer_requests`.
    fn request_received(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Fire `PeerResponseReplied { corr_id }` (inbound reply sent).
    ///
    /// Guard: `corr_id` is in `inbound_peer_requests` with state `Received`.
    fn response_replied(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Observe the DSL-owned state of an outbound peer request.
    ///
    /// Returns `None` if the correlation id is not in `pending_peer_requests`.
    fn outbound_state(&self, corr_id: PeerCorrelationId) -> Option<OutboundPeerRequestState>;

    /// Observe the DSL-owned state of an inbound peer request.
    fn inbound_state(&self, corr_id: PeerCorrelationId) -> Option<InboundPeerRequestState>;

    /// Install a projection-cleanup observer for the peer-interaction
    /// lifecycle. The runtime handle invokes the observer whenever a DSL
    /// transition emits `PeerInteractionCleanup`, closing the loop
    /// "terminal transition → effect → shell projection cleanup".
    ///
    /// Implementations with no observer simply drop any emitted cleanup
    /// notifications on the floor. Standalone / WASM paths leave this
    /// unset.
    fn install_cleanup_observer(&self, observer: Arc<dyn PeerInteractionCleanupObserver>);
}

/// Observer invoked by [`PeerInteractionHandle`] when a DSL
/// `PeerInteractionCleanup` effect is emitted.
///
/// Shell-owned projection consumers (the comms runtime's subscriber /
/// stream registries) implement this to drop channel entries keyed on the
/// terminated correlation id. The observer is invoked under the same
/// authority lock as the transition that emitted the effect, so the
/// "terminal transition → effect → cleanup" chain is causal, not lexically
/// adjacent.
pub trait PeerInteractionCleanupObserver: Send + Sync {
    /// Called once per emitted `PeerInteractionCleanup { corr_id }` effect.
    ///
    /// Idempotent: a well-formed DSL run emits exactly one cleanup per
    /// correlation id because terminal transitions remove the map entry
    /// (subsequent attempts are rejected at the `pending_exists` guard),
    /// but observers should tolerate a redundant call defensively.
    fn on_peer_interaction_cleanup(&self, corr_id: PeerCorrelationId);
}

/// Session-context advancement DSL handle (W2-E / issue #264).
///
/// Shell callers fire `context_advanced(updated_at_ms)` at every site that
/// mutates canonical session truth (prompt append, external content
/// injection, tool-result append, external assistant output,
/// runtime-system-context append, any `summary_tx.send_replace`). The
/// transition is monotonic: the DSL guard drops ticks whose `updated_at_ms`
/// isn't strictly greater than the last recorded watermark, so callers can
/// fire unconditionally post-mutation.
///
/// Every successful transition emits `SessionContextAdvanced` which is
/// dispatched to the installed [`SessionContextAdvancedObserver`] — the
/// realtime projection consumer uses the observer to drive a typed
/// `ProjectionFreshness` state instead of polling a watch channel.
pub trait SessionContextHandle: Send + Sync {
    /// Fire `AdvanceSessionContext { updated_at_ms }`.
    ///
    /// Guard: `updated_at_ms` is strictly greater than the last recorded
    /// watermark. Returns `Ok(false)` when the guard rejects the tick as
    /// non-advancing (duplicate or out-of-order); returns `Ok(true)` when
    /// the transition lands and the effect is emitted. Transition errors
    /// (lock poisoning, unexpected DSL state) surface as `Err`.
    fn context_advanced(&self, updated_at_ms: u64) -> Result<bool, DslTransitionError>;

    /// The monotonic watermark in milliseconds of the last successful
    /// `AdvanceSessionContext` transition recorded on this handle.
    ///
    /// Returns `0` before any advance has been recorded. The realtime
    /// projection consumer reads this once at install time to seed its
    /// `ProjectionFreshness` baseline, so the consumer and the DSL agree
    /// on the initial frontier by construction (no two-read race).
    fn current_watermark_ms(&self) -> u64;

    /// Install a typed observer for `SessionContextAdvanced` effect
    /// emission. Implementations without an installed observer drop the
    /// effect on the floor (standalone / WASM paths).
    fn install_observer(&self, observer: Arc<dyn SessionContextAdvancedObserver>);

    /// Atomically install a typed observer and return the current watermark
    /// as a single critical section. Implementations MUST hold the same
    /// authority lock that `context_advanced` uses for both the watermark
    /// read and the observer installation, so no `SessionContextAdvanced`
    /// effect can slip between "sampled baseline" and "observer visible".
    ///
    /// Callers use the returned `u64` as their `ProjectionFreshness`
    /// baseline; any subsequent `context_advanced` tick is guaranteed to
    /// either (a) have already been included in the returned watermark, or
    /// (b) be visible to the observer. The `current_watermark_ms` +
    /// `install_observer` pair is NOT a substitute: a transition can land
    /// between those two non-atomic steps and be lost to both the baseline
    /// and the observer.
    fn install_observer_with_baseline(
        &self,
        observer: Arc<dyn SessionContextAdvancedObserver>,
    ) -> u64 {
        // Default combines the two primitives for backwards compatibility
        // with any external impls that do not yet override this method.
        // Runtime impls override to provide the atomic guarantee.
        self.install_observer(observer);
        self.current_watermark_ms()
    }
}

/// Observer invoked by [`SessionContextHandle`] when a DSL
/// `SessionContextAdvanced` effect is emitted (W2-E / issue #264).
///
/// The realtime projection consumer implements this to advance its typed
/// `ProjectionFreshness` state. Runtime handles sample the installed
/// observer under the same authority lock as the transition that emitted
/// the effect, then dispatch the callback immediately after releasing the
/// lock so re-entrant observer implementations can safely route back
/// through the same DSL authority.
pub trait SessionContextAdvancedObserver: Send + Sync {
    /// Called once per emitted `SessionContextAdvanced { updated_at_ms }`
    /// effect. `updated_at_ms` is the monotonic millisecond watermark of
    /// the canonical session-context mutation that produced this tick.
    fn on_session_context_advanced(&self, updated_at_ms: u64);
}

// ---------------------------------------------------------------------------
// SessionClaimHandle (dogma #2 — canonical session-identity owner)
// ---------------------------------------------------------------------------

/// Error surfaced by [`SessionClaimHandle::try_acquire`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionClaimError {
    /// Another live claim already exists for this session id.
    #[error("session identity already claimed: {0}")]
    SessionIdentityInUse(SessionId),
}

/// RAII token returned by [`SessionClaimHandle::try_acquire`].
///
/// While alive, the underlying registry guarantees no other caller can
/// acquire a claim for the same `session_id`. Drop releases the claim back
/// through the owning handle.
pub struct SessionClaim {
    session_id: SessionId,
    handle: Arc<dyn SessionClaimHandle>,
}

impl SessionClaim {
    /// Construct a new claim — only [`SessionClaimHandle`] impls should call
    /// this, immediately after they have inserted `session_id` into their
    /// canonical registry under a single critical section.
    pub fn new(session_id: SessionId, handle: Arc<dyn SessionClaimHandle>) -> Self {
        Self { session_id, handle }
    }

    /// The session id this claim covers.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
}

impl Drop for SessionClaim {
    fn drop(&mut self) {
        self.handle.release(&self.session_id);
    }
}

impl std::fmt::Debug for SessionClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionClaim")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

/// Process-scope canonical owner of "this session id is currently active."
///
/// One canonical owner per process: `MeerkatMachine` exposes its registry
/// when a runtime is wired (so every live runtime-registered session also
/// owns its identity claim), and a default in-process registry covers bare
/// `AgentFactory` callers without a runtime. Either way, "this session id
/// is in use" lives in a typed owner — never in process-global shell
/// bookkeeping.
pub trait SessionClaimHandle: Send + Sync {
    /// Atomically reserve `session_id`. Returns a [`SessionClaim`] whose
    /// `Drop` releases the slot. Returns
    /// [`SessionClaimError::SessionIdentityInUse`] if another live claim
    /// already covers this session.
    ///
    /// Implementations MUST insert under a single critical section so two
    /// concurrent callers cannot both succeed.
    fn try_acquire(
        self: Arc<Self>,
        session_id: &SessionId,
    ) -> Result<SessionClaim, SessionClaimError>;

    /// Release a claim previously created by [`Self::try_acquire`].
    ///
    /// Called from [`SessionClaim`]'s `Drop`. Idempotent: releasing an
    /// unknown id is a no-op (the registry was already cleared, e.g. via
    /// runtime teardown).
    fn release(&self, session_id: &SessionId);
}

/// In-process default [`SessionClaimHandle`] for bare-usage paths that have
/// no `MeerkatMachine` available (standalone `AgentFactory` callers, doc
/// examples, simple SDK consumers). One process-global instance keeps the
/// "one active claim per session id" invariant intact even when no runtime
/// is wired.
pub struct DefaultSessionClaimRegistry {
    claims: std::sync::Mutex<std::collections::HashSet<SessionId>>,
}

impl DefaultSessionClaimRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self {
            claims: std::sync::Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Process-global instance — used by bare-usage facade builders.
    pub fn global() -> Arc<Self> {
        use std::sync::OnceLock;
        static GLOBAL: OnceLock<Arc<DefaultSessionClaimRegistry>> = OnceLock::new();
        Arc::clone(GLOBAL.get_or_init(|| Arc::new(DefaultSessionClaimRegistry::new())))
    }
}

impl Default for DefaultSessionClaimRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionClaimHandle for DefaultSessionClaimRegistry {
    fn try_acquire(
        self: Arc<Self>,
        session_id: &SessionId,
    ) -> Result<SessionClaim, SessionClaimError> {
        let mut claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !claims.insert(session_id.clone()) {
            return Err(SessionClaimError::SessionIdentityInUse(session_id.clone()));
        }
        drop(claims);
        Ok(SessionClaim::new(
            session_id.clone(),
            self as Arc<dyn SessionClaimHandle>,
        ))
    }

    fn release(&self, session_id: &SessionId) {
        let mut claims = self
            .claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        claims.remove(session_id);
    }
}

// ---------------------------------------------------------------------------
// InteractionStreamHandle (U6 / dogma #5)
// ---------------------------------------------------------------------------

/// Interaction stream lifecycle DSL handle.
///
/// Routes the reservation/attach/completion/expire/close-early lifecycle of
/// a streamed interaction into the MeerkatMachine DSL's `interaction_streams`
/// substate map. The shell-side `interaction_stream_registry` projects
/// sender/receiver channels off this map; terminal transitions emit
/// [`InteractionStreamCleanupObserver::on_interaction_stream_cleanup`], which
/// the comms runtime uses to drop the channel projection.
///
/// Reservation TTL is shell-owned mechanics: the runtime holds the timestamp
/// and decides when to fire `expired`. Every state-meaning decision (is the
/// reservation still claimable? has the consumer attached? did a terminal
/// event win the race?) lives in the DSL.
pub trait InteractionStreamHandle: Send + Sync {
    /// Fire `InteractionStreamReserved { corr_id }`.
    ///
    /// Guard: `corr_id` is not already in `interaction_streams`. Rejected
    /// duplicates surface as [`DslTransitionError`] so the shell can refuse
    /// to register two channels under the same key.
    fn reserved(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Fire `InteractionStreamAttached { corr_id }`.
    ///
    /// Guard: state is `Reserved`. Rejected if the reservation already
    /// expired, the consumer already attached, or the entry never existed.
    fn attached(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Fire `InteractionStreamCompleted { corr_id }`.
    ///
    /// Guard: state is `Attached`. Terminal — emits the cleanup effect.
    fn completed(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Fire `InteractionStreamExpired { corr_id }`.
    ///
    /// Guard: state is `Reserved`. Terminal — emits the cleanup effect.
    fn expired(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Fire `InteractionStreamClosedEarly { corr_id }`.
    ///
    /// Guard: state is `Attached`. Terminal — emits the cleanup effect.
    fn closed_early(&self, corr_id: PeerCorrelationId) -> Result<(), DslTransitionError>;

    /// Read the DSL-owned state for a given correlation id, if any.
    ///
    /// Returns `None` when the entry has already been removed (terminal or
    /// never reserved). Active states (`Reserved`, `Attached`) surface as
    /// `Some(..)`; terminal variants surface only via the
    /// `InteractionStreamStateChanged` effect, never on the active map.
    fn state(&self, corr_id: PeerCorrelationId) -> Option<InteractionStreamState>;

    /// Install a projection-cleanup observer for the interaction stream
    /// lifecycle. The runtime handle invokes the observer whenever a DSL
    /// transition emits `InteractionStreamCleanup`, closing the loop
    /// "terminal transition → effect → shell projection cleanup".
    fn install_cleanup_observer(&self, observer: Arc<dyn InteractionStreamCleanupObserver>);
}

/// Observer invoked by [`InteractionStreamHandle`] when a DSL
/// `InteractionStreamCleanup` effect is emitted.
///
/// Shell-owned projection consumers (the comms runtime's
/// `interaction_stream_registry`) implement this to drop channel entries
/// keyed on the terminated correlation id. Runtime handles sample the
/// observer under the same authority lock as the transition that emitted
/// the effect, then dispatch after releasing the lock.
pub trait InteractionStreamCleanupObserver: Send + Sync {
    /// Called once per emitted `InteractionStreamCleanup { corr_id }` effect.
    ///
    /// Idempotent in the well-formed case (terminal transitions remove the
    /// map entry so subsequent fires fail the guard), but observers should
    /// tolerate redundant calls defensively.
    fn on_interaction_stream_cleanup(&self, corr_id: PeerCorrelationId);
}

// ---------------------------------------------------------------------------
// RealtimeProductTurnHandle (U9 / dogma #4 — realtime WS lifecycle owner)
// ---------------------------------------------------------------------------

/// Realtime provider-session projection freshness (dogma round 2, U-C /
/// dogma #1, #3, #13, #20).
///
/// Canonical typed mirror of the DSL-owned `realtime_projection_freshness`
/// field. Replaces the socket-local `ProjectionFreshness` enum previously
/// owned by `meerkat-rpc::realtime_ws`. The realtime-WS dispatcher reads
/// this enum via [`RealtimeProductTurnHandle::projection_freshness`] and
/// fires typed inputs for each observer tick, turn end, and refresh drain
/// — no shell-local freshness state, no socket-local observer queue.
///
/// The `frontier_ms` companion carries the monotonic watermark: it is the
/// `baseline_ms` while `Clean`, or the `new_at_ms` of the pending advance
/// while `StaleDeferred` / `StaleImmediate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeProjectionFreshness {
    /// Provider projection matches canonical session state at the
    /// frontier watermark. No refresh owed.
    #[default]
    Clean,
    /// Canonical state advanced while the provider turn was live; refresh
    /// is blocked until the turn terminates so barge-in continuity isn't
    /// broken.
    StaleDeferred,
    /// Refresh owed at the next drain site (idle input-chunk arrival or
    /// turn end).
    StaleImmediate,
}

/// Typed classification of what a clean provider-session close means for
/// the realtime channel's reconnect behavior (dogma round 2, U-C /
/// dogma #1, #3, #18, #20).
///
/// Replaces the shell-local boolean pair
/// (`client_has_submitted_input`, `last_turn_terminally_completed`) that
/// used to co-decide `needs_reattach` in `meerkat-rpc::realtime_ws`. The
/// realtime-WS dispatcher reads this via
/// [`RealtimeProductTurnHandle::reconnect_policy_on_clean_close`] at the
/// clean-close branch point and dispatches on the typed value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeReconnectPolicy {
    /// A clean close has no in-flight work to recover — either the
    /// client never submitted input, or the last observed turn reached a
    /// terminal completion.
    #[default]
    CleanExit,
    /// The client issued work that has not yet reached a terminal turn
    /// completion; a clean close is a mid-work disconnect and the
    /// channel should proactively reattach.
    ReattachAndRecover,
}

/// Observer invoked by [`RealtimeProductTurnHandle`] when the DSL emits a
/// `RealtimeProjectionFreshnessChanged` effect (dogma round 2, U-C).
///
/// Shell-owned consumers (the realtime-WS dispatch loop) implement this to
/// wake the socket's `tokio::select!` loop so it can read the new freshness
/// state and drain if necessary. Runtime handles sample the observer under
/// the same authority lock as the transition that emitted the effect, then
/// dispatch after releasing the lock so future observers can safely
/// re-enter the DSL if needed.
pub trait RealtimeProjectionFreshnessObserver: Send + Sync {
    /// Called once per emitted `RealtimeProjectionFreshnessChanged`
    /// effect. `new_freshness` is the post-transition discriminant;
    /// `frontier_ms` is the post-transition monotonic watermark.
    fn on_realtime_projection_freshness_changed(
        &self,
        new_freshness: RealtimeProjectionFreshness,
        frontier_ms: u64,
    );
}

/// Realtime product-turn lifecycle phase (U9 / dogma #4).
///
/// Canonical typed mirror of the DSL-owned
/// `realtime_product_turn_phase` field. The realtime-WS shell reads this
/// enum via [`RealtimeProductTurnHandle::current_phase`] instead of
/// tracking `product_turn_in_flight` / `product_turn_committed` /
/// `product_output_started` as shell locals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RealtimeProductTurnPhase {
    /// No turn in flight on the provider session.
    #[default]
    Idle,
    /// Input accepted by the provider; no `TurnCommitted` or output
    /// delta observed yet.
    AwaitingProgress,
    /// `TurnCommitted` arrived but no output delta / tool call yet.
    Committed,
    /// Output delta / tool call arrived but `TurnCommitted` not yet.
    OutputStarted,
    /// Both `TurnCommitted` and output delta observed — preemption on a
    /// subsequent input chunk is semantically sound.
    Preemptible,
}

/// Realtime product-turn lifecycle DSL handle (U9 / dogma #4).
///
/// Routes every lifecycle observation (input accepted, TurnCommitted,
/// output delta / tool call, interrupt, logical turn terminal) into the
/// session's MeerkatMachine DSL. Idempotent fires (e.g., a second
/// `turn_committed()` call on the same turn) are guard-rejected at the
/// DSL layer and surfaced as `Ok(false)` so the shell can fire
/// unconditionally.
///
/// The shell reads [`current_phase`] / [`is_in_flight`] /
/// [`should_preempt_on_input`] for routing decisions; no shell-local
/// boolean state, no helper-local event matching.
pub trait RealtimeProductTurnHandle: Send + Sync {
    /// Fire `ProductTurnInFlight`. Returns `Ok(true)` when the turn
    /// advanced from `Idle`; `Ok(false)` when already in-flight.
    fn turn_in_flight(&self) -> Result<bool, DslTransitionError>;

    /// Fire `ProductTurnCommitted`. Returns `Ok(true)` when the phase
    /// advanced (`AwaitingProgress` → `Committed`, or `OutputStarted` →
    /// `Preemptible`); `Ok(false)` otherwise.
    fn turn_committed(&self) -> Result<bool, DslTransitionError>;

    /// Fire `ProductOutputStarted`. Returns `Ok(true)` when the phase
    /// advanced (`AwaitingProgress` → `OutputStarted`, or `Committed` →
    /// `Preemptible`); `Ok(false)` otherwise.
    fn output_started(&self) -> Result<bool, DslTransitionError>;

    /// Fire `ProductTurnInterrupted`. Returns `Ok(true)` when the
    /// output-started milestone was cleared (`Preemptible` →
    /// `Committed`, or `OutputStarted` → `AwaitingProgress`); `Ok(false)`
    /// otherwise.
    fn turn_interrupted(&self) -> Result<bool, DslTransitionError>;

    /// Fire `ProductTurnTerminal`. Returns `Ok(true)` when the phase
    /// advanced back to `Idle` from any non-`Idle` phase; `Ok(false)`
    /// when already `Idle`.
    fn turn_terminal(&self) -> Result<bool, DslTransitionError>;

    /// Read the current typed phase from the DSL.
    fn current_phase(&self) -> RealtimeProductTurnPhase;

    /// True iff the provider session has an active turn (any non-`Idle`
    /// phase). Used by the realtime-WS projection-refresh gate.
    fn is_in_flight(&self) -> bool {
        self.current_phase() != RealtimeProductTurnPhase::Idle
    }

    /// True iff an input chunk arriving now should preempt the current
    /// provider-managed turn — i.e. the turn has reached `Preemptible`.
    ///
    /// Preemption is only sound once the committed turn has visible
    /// assistant-side progress. Before that, the provider remains the
    /// semantic owner of whether the current input stream still belongs
    /// to the same utterance; preempting early would cancel a response
    /// before the assistant has had a chance to speak.
    fn should_preempt_on_input(&self) -> bool {
        self.current_phase() == RealtimeProductTurnPhase::Preemptible
    }

    // ---- Projection freshness (dogma round 2, U-C / dogma #1, #3, #13, #20) ----

    /// Fire `RealtimeProjectionAdvanceObserved { advanced_at_ms }` — the
    /// shell received a `SessionContextAdvanced` observer tick. The DSL
    /// decides the resulting freshness based on the current product-turn
    /// phase (live → `StaleDeferred`; idle → `StaleImmediate`).
    ///
    /// Returns `Ok(true)` when the advance transitioned state,
    /// `Ok(false)` when the DSL monotonic guard rejected the tick as
    /// non-advancing (or when the caller fired redundantly below the
    /// current frontier).
    fn projection_advance_observed(&self, advanced_at_ms: u64) -> Result<bool, DslTransitionError>;

    /// Fire `RealtimeProjectionRefreshed { observed_ms }` — the shell
    /// completed a provider-session refresh drain. Returns to `Clean`.
    fn projection_refreshed(&self, observed_ms: u64) -> Result<bool, DslTransitionError>;

    /// Fire `RealtimeProjectionReset { baseline_ms }` — the shell closed
    /// or reconnected the product session and wants to re-seed the
    /// `Clean` baseline at the current DSL session-context watermark.
    fn projection_reset(&self, baseline_ms: u64) -> Result<bool, DslTransitionError>;

    /// Read the current typed projection-freshness discriminant from the
    /// DSL. The shell reads this at the canonical drain sites (observer
    /// tick arrival, input-chunk refresh gate, turn-end drain) to decide
    /// whether to rebuild the provider session's projection.
    fn projection_freshness(&self) -> RealtimeProjectionFreshness;

    /// Read the current monotonic frontier watermark from the DSL.
    fn projection_frontier_ms(&self) -> u64;

    /// Convenience accessor: `true` iff the current freshness is
    /// `StaleImmediate` — the shell uses this at drain sites.
    fn is_projection_stale_immediate(&self) -> bool {
        self.projection_freshness() == RealtimeProjectionFreshness::StaleImmediate
    }

    /// Install a typed observer for `RealtimeProjectionFreshnessChanged`
    /// effect emission. Implementations without an installed observer
    /// drop the effect (standalone / WASM paths).
    fn install_projection_freshness_observer(
        &self,
        observer: Arc<dyn RealtimeProjectionFreshnessObserver>,
    );

    /// Atomically install a typed observer and return the current
    /// freshness snapshot as a single authority read. Implementations
    /// MUST hold the same authority lock that projection transitions use
    /// for both observer installation and the `(freshness, frontier)`
    /// sample, so no `RealtimeProjectionFreshnessChanged` effect can
    /// slip between "observer visible" and "socket seeded from the DSL".
    ///
    /// Callers that only need best-effort notification may use
    /// [`Self::install_projection_freshness_observer`]. Realtime socket
    /// bindings should prefer this method so wake registration and the
    /// typed DSL snapshot share one ordering point.
    fn install_projection_freshness_observer_with_snapshot(
        &self,
        observer: Arc<dyn RealtimeProjectionFreshnessObserver>,
    ) -> (RealtimeProjectionFreshness, u64) {
        self.install_projection_freshness_observer(observer);
        (self.projection_freshness(), self.projection_frontier_ms())
    }

    // ---- Reconnect policy (dogma round 2, U-C / dogma #1, #3, #18, #20) ----

    /// Fire `ClassifyRealtimeClientInputSubmitted` — the client's input
    /// chunk was accepted by the provider session. Returns `Ok(false)`
    /// when already `ReattachAndRecover`.
    fn classify_client_input_submitted(&self) -> Result<bool, DslTransitionError>;

    /// Fire `ClassifyRealtimeMidTurnActivity` — the provider session
    /// emitted a non-terminal activity (e.g. a tool call) inside a live
    /// turn. Returns `Ok(false)` when already `ReattachAndRecover`.
    fn classify_mid_turn_activity(&self) -> Result<bool, DslTransitionError>;

    /// Fire `ClassifyRealtimeTurnTerminated` — the current turn reached
    /// a logical terminal stop reason. Also folds in the pending
    /// `StaleDeferred → StaleImmediate` promotion so the turn-end drain
    /// site picks up any pending advance.
    fn classify_turn_terminated(&self) -> Result<bool, DslTransitionError>;

    /// Read the typed reconnect-policy classification. The shell reads
    /// this at the clean-close branch to decide whether a clean provider-
    /// session close should trigger a proactive reattach
    /// (`ReattachAndRecover`) or close the channel cleanly
    /// (`CleanExit`).
    fn reconnect_policy_on_clean_close(&self) -> RealtimeReconnectPolicy;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{
        PeerConversationProjection, PeerResponseProgressProjectionPhase,
        PeerResponseTerminalProjectionStatus, peer_response_terminal_context_key,
    };

    #[test]
    fn peer_terminal_projection_owns_prompt_and_context_key() {
        let projection = PeerConversationProjection::ResponseTerminal {
            peer_id: "analyst-rt".into(),
            request_id: "req-123".into(),
            status: PeerResponseTerminalProjectionStatus::Completed,
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "token": "birch seventeen"
            })),
        };

        assert_eq!(
            projection.context_key().as_deref(),
            Some("peer_response_terminal:analyst-rt:req-123")
        );
        assert_eq!(
            projection.prompt_text(),
            "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] Correlated peer response from analyst-rt. Request ID: req-123. Status: completed. Result: {\n  \"request_intent\": \"checksum_token\",\n  \"token\": \"birch seventeen\"\n}."
        );
    }

    #[test]
    fn peer_progress_projection_formats_phase_from_shared_seam() {
        let projection = PeerConversationProjection::ResponseProgress {
            peer_id: "operator-rt".into(),
            request_id: "req-789".into(),
            phase: PeerResponseProgressProjectionPhase::PartialResult,
            payload: Some(serde_json::json!({ "chunk": "alpha" })),
        };

        assert_eq!(projection.context_key(), None);
        assert_eq!(
            projection.prompt_text(),
            "[SYSTEM NOTICE][PEER_RESPONSE_PROGRESS] Correlated peer response progress from operator-rt. Request ID: req-789. Phase: partial_result. Payload: {\n  \"chunk\": \"alpha\"\n}."
        );
    }

    #[test]
    fn peer_terminal_context_key_helper_stays_canonical() {
        assert_eq!(
            peer_response_terminal_context_key("peer-a", "req-z"),
            "peer_response_terminal:peer-a:req-z"
        );
    }
}
