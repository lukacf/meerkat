//! Cross-crate DSL handle traits for the MeerkatMachine DSL.
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
//! Parameters use primitive types (`String`, `u64`, `bool`) or core-resident
//! newtypes ([`InputId`], [`RunId`]). Identifiers that currently live in
//! downstream crates (for example `SurfaceId` in `meerkat-mcp`) are passed as
//! strings; the DSL stores those as opaque string keys already.
//!
//! Return type is `Result<(), DslTransitionError>`. The DSL decides legality;
//! phase/field reads happen elsewhere (direct DSL state accessors, not via
//! these traits).

use std::collections::BTreeSet;

use crate::lifecycle::{InputId, RunId};

/// Error surfaced when a DSL transition is rejected.
///
/// Wraps the generated kernel's `NoMatchingTransition` and carries the
/// transition name for diagnostics. Trait impls populate `context` from the
/// trait method name so callers can tell which handle rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("DSL transition rejected in {context}: {reason}")]
pub struct DslTransitionError {
    /// Name of the trait method / DSL variant whose transition was rejected.
    pub context: &'static str,
    /// Underlying rejection reason (typically the generated
    /// `NoMatchingTransition { phase, trigger }` formatted).
    pub reason: String,
}

impl DslTransitionError {
    /// Construct a new transition error with the given context and reason.
    pub fn new(context: &'static str, reason: impl Into<String>) -> Self {
        Self {
            context,
            reason: reason.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// TurnStateHandle
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnStateSnapshot {
    pub turn_phase: String,
    pub primitive_kind: Option<String>,
    pub admitted_content_shape: Option<String>,
    pub vision_enabled: bool,
    pub image_tool_results_enabled: bool,
    pub tool_calls_pending: u64,
    pub pending_op_refs: BTreeSet<String>,
    pub barrier_operation_ids: BTreeSet<String>,
    pub has_barrier_ops: bool,
    pub barrier_satisfied: bool,
    pub boundary_count: u64,
    pub cancel_after_boundary: bool,
    pub terminal_outcome: Option<String>,
    pub extraction_attempts: u64,
    pub max_extraction_retries: u64,
}

/// Turn-execution DSL handle.
pub trait TurnStateHandle: Send + Sync {
    fn start_conversation_run(
        &self,
        run_id: RunId,
        primitive_kind: String,
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
        op_refs: BTreeSet<String>,
        barrier_operation_ids: BTreeSet<String>,
    ) -> Result<(), DslTransitionError>;

    fn tool_calls_resolved(&self) -> Result<(), DslTransitionError>;

    fn ops_barrier_satisfied(
        &self,
        operation_ids: BTreeSet<String>,
    ) -> Result<(), DslTransitionError>;

    fn boundary_continue(&self) -> Result<(), DslTransitionError>;

    fn boundary_complete(&self) -> Result<(), DslTransitionError>;

    fn enter_extraction(&self) -> Result<(), DslTransitionError>;

    fn extraction_start(&self) -> Result<(), DslTransitionError>;

    fn extraction_validation_passed(&self) -> Result<(), DslTransitionError>;

    fn extraction_validation_failed(&self, error: String) -> Result<(), DslTransitionError>;

    fn recoverable_failure(&self, error: String) -> Result<(), DslTransitionError>;

    fn fatal_failure(&self, error: String) -> Result<(), DslTransitionError>;

    fn retry_requested(&self) -> Result<(), DslTransitionError>;

    fn cancel_now(&self) -> Result<(), DslTransitionError>;

    fn request_cancel_after_boundary(&self) -> Result<(), DslTransitionError>;

    fn cancellation_observed(&self) -> Result<(), DslTransitionError>;

    fn acknowledge_terminal(&self, outcome: String) -> Result<(), DslTransitionError>;

    fn turn_limit_reached(&self) -> Result<(), DslTransitionError>;

    fn budget_exhausted(&self) -> Result<(), DslTransitionError>;

    fn time_budget_exceeded(&self) -> Result<(), DslTransitionError>;

    fn force_cancel_no_run(&self) -> Result<(), DslTransitionError>;

    fn run_completed(&self, run_id: RunId) -> Result<(), DslTransitionError>;

    fn run_failed(&self, run_id: RunId, error: String) -> Result<(), DslTransitionError>;

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

    /// Fire the `SpawnDrain { mode }` input — explicit spawn with mode.
    ///
    /// `mode` is the stringified drain-mode discriminant (e.g., `"Timed"`,
    /// `"AttachedSession"`, `"PersistentHost"`). Trait consumers format via
    /// their local enum's `Display`/`to_string`; the DSL stores the raw string.
    fn spawn_drain(&self, mode: &str) -> Result<(), DslTransitionError>;

    /// Fire the `StopDrain` input.
    fn stop_drain(&self) -> Result<(), DslTransitionError>;

    /// Fire the `DrainExitedClean` input (drain stopped without failure).
    fn drain_exited_clean(&self) -> Result<(), DslTransitionError>;

    /// Fire the `DrainExitedRespawnable` input (drain exited and can be respawned).
    fn drain_exited_respawnable(&self) -> Result<(), DslTransitionError>;

    /// Fire the `NotifyDrainExited { reason }` input.
    fn notify_drain_exited(&self, reason: &str) -> Result<(), DslTransitionError>;
}

// ---------------------------------------------------------------------------
// ExternalToolSurfaceHandle
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceSnapshot {
    pub surface_id: String,
    pub base_state: Option<String>,
    pub pending_op: Option<String>,
    pub staged_op: Option<String>,
    pub staged_intent_sequence: Option<u64>,
    pub pending_task_sequence: Option<u64>,
    pub pending_lineage_sequence: Option<u64>,
    pub inflight_calls: u64,
    pub last_delta_operation: Option<String>,
    pub last_delta_phase: Option<String>,
    pub removal_draining_since_ms: Option<u64>,
    pub removal_timeout_at_ms: Option<u64>,
    pub removal_applied_at_turn: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceDiagnosticSnapshot {
    pub surface_phase: String,
    pub known_surfaces: BTreeSet<String>,
    pub visible_surfaces: BTreeSet<String>,
    pub snapshot_epoch: u64,
    pub snapshot_aligned_epoch: u64,
    pub has_pending_or_staged: bool,
    pub entries: Vec<SurfaceSnapshot>,
}

/// External tool surface lifecycle DSL handle.
pub trait ExternalToolSurfaceHandle: Send + Sync {
    fn register(&self, surface_id: String) -> Result<(), DslTransitionError>;

    fn stage_add(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError>;

    fn stage_remove(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError>;

    fn stage_reload(&self, surface_id: String, now_ms: u64) -> Result<(), DslTransitionError>;

    fn apply_boundary(
        &self,
        surface_id: String,
        now_ms: u64,
        current_turn: u64,
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
/// Envelope state (raw_item_id, peer_id, request_id, etc.) is staged via
/// other DSL inputs before these signals fire; the signals themselves are
/// parameterless — they advance the classification lifecycle phase.
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
    fn ingest(
        &self,
        runtime_id: &str,
        work_id: &str,
        origin: &str,
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
        run_id: &RunId,
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

/// Observable snapshot of an auth lease's DSL state for a given binding_key.
///
/// Returned by [`AuthLeaseHandle::snapshot`]. The `state` field is one of the
/// DSL-defined states: `"valid"`, `"expiring"`, `"refreshing"`,
/// `"reauth_required"`. If the binding is not tracked at all, `state` is
/// `None` and `expires_at` is `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthLeaseSnapshot {
    pub state: Option<String>,
    pub expires_at: Option<u64>,
}

/// Window (in seconds) before `expires_at` at which a `valid` lease is
/// eligible to transition into `expiring` at the next CallingLlm
/// boundary. Owned here — on the handle trait module — rather than in
/// shell code, per dogma §9 ("policy composes at the facade/factory
/// seam, not in random helpers") and §20 ("every important behavior
/// reduces to one clear owner").
///
/// The actual state transition is gated by the MeerkatMachine DSL's
/// `MarkAuthExpiring` input (which enforces the `valid → expiring`
/// legality); this constant only controls *when* the runner fires
/// that input, not whether the transition is legal.
pub const AUTH_LEASE_TTL_REFRESH_WINDOW_SECS: u64 = 60;

/// Auth lease lifecycle DSL handle.
///
/// Covers the auth-lifecycle inputs on the MeerkatMachine DSL. Each method
/// drives the corresponding DSL transition and returns
/// `Err(DslTransitionError)` if the guard rejects.
pub trait AuthLeaseHandle: Send + Sync {
    /// Fire `AcquireAuthLease { binding_key, expires_at }` — unconditional.
    ///
    /// Moves the binding into `auth_valid_leases` and records its expiry.
    fn acquire_lease(&self, binding_key: &str, expires_at: u64) -> Result<(), DslTransitionError>;

    /// Fire `MarkAuthExpiring { binding_key }` — only legal from `valid`.
    fn mark_expiring(&self, binding_key: &str) -> Result<(), DslTransitionError>;

    /// Fire `BeginAuthRefresh { binding_key }` — legal from `valid` or
    /// `expiring`.
    ///
    /// Provides the DSL-level refresh dedup: once the binding is in
    /// `auth_refreshing_leases`, no concurrent `BeginAuthRefresh` is
    /// permitted until `CompleteAuthRefresh` or `AuthRefreshFailed` moves
    /// it back out.
    fn begin_refresh(&self, binding_key: &str) -> Result<(), DslTransitionError>;

    /// Fire `CompleteAuthRefresh { binding_key, new_expires_at, now }` — only
    /// legal from `refreshing`.
    fn complete_refresh(
        &self,
        binding_key: &str,
        new_expires_at: u64,
        now: u64,
    ) -> Result<(), DslTransitionError>;

    /// Fire `AuthRefreshFailed { binding_key, permanent }` — only legal from
    /// `refreshing`. `permanent=true` routes to `reauth_required` and emits a
    /// reauth notice; `permanent=false` routes back to `expiring`.
    fn refresh_failed(&self, binding_key: &str, permanent: bool) -> Result<(), DslTransitionError>;

    /// Fire `MarkReauthRequired { binding_key }` — any known state → reauth.
    fn mark_reauth_required(&self, binding_key: &str) -> Result<(), DslTransitionError>;

    /// Fire `ReleaseAuthLease { binding_key }` — removes the binding from all
    /// sets and the expiry map.
    fn release_lease(&self, binding_key: &str) -> Result<(), DslTransitionError>;

    /// Observe the current DSL-level state of a binding.
    fn snapshot(&self, binding_key: &str) -> AuthLeaseSnapshot;
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
