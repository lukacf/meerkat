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
use std::sync::Arc;

use crate::lifecycle::{InputId, RunId};
use crate::peer_correlation::{
    InboundPeerRequestState, OutboundPeerRequestState, PeerCorrelationId,
};
use crate::types::SessionId;

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
/// `ProjectionFreshness` state. The observer is invoked under the same
/// authority lock as the transition that emitted the effect, so the
/// "mutation → transition → observer" chain is causal, not lexically
/// adjacent.
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
