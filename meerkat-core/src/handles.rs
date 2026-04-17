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
//! Trait methods are named per-DSL input/signal, not per-authority input.
//! Parameters use primitive types (`String`, `u64`, `bool`) or core-resident
//! newtypes ([`InputId`], [`RunId`]). Identifiers that currently live in
//! downstream crates (e.g., `SurfaceId` in `meerkat-mcp`) are passed as `&str`;
//! the DSL stores those as opaque string keys already.
//!
//! Return type is `Result<(), DslTransitionError>`. The DSL decides legality;
//! phase/field reads happen elsewhere (direct DSL state accessors, not via
//! these traits).

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

/// Turn-execution DSL handle.
///
/// Covers the run-lifecycle signals on the MeerkatMachine DSL: start signals,
/// LLM call boundary signals, pending-op outcomes, cancellation, and the
/// Prepare/Commit/Fail input triad that tracks `current_run_id` /
/// `pre_run_phase`. The DSL uses field storage for run_id already populated
/// via `prepare(run_id)`; most signals are parameterless because `current_run_id`
/// is set upstream.
pub trait TurnStateHandle: Send + Sync {
    // --- Run-start signals (no params; `current_run_id` set via Prepare) ---
    fn start_conversation_run(&self) -> Result<(), DslTransitionError>;
    fn start_immediate_append(&self) -> Result<(), DslTransitionError>;
    fn start_immediate_context(&self) -> Result<(), DslTransitionError>;

    // --- LLM call boundary signals ---
    fn call_started(&self) -> Result<(), DslTransitionError>;
    fn call_finished(&self) -> Result<(), DslTransitionError>;

    // --- Boundary / pending-op resolution signals ---
    fn boundary_applied(&self, revision: u64) -> Result<(), DslTransitionError>;
    fn pending_succeeded(&self) -> Result<(), DslTransitionError>;
    fn pending_failed(&self) -> Result<(), DslTransitionError>;

    // --- Cancellation inputs ---
    fn interrupt_current_run(&self) -> Result<(), DslTransitionError>;
    fn cancel_after_boundary(&self) -> Result<(), DslTransitionError>;

    // --- Run-lifecycle input triad (Prepare/Commit/Fail) ---
    fn prepare(&self, run_id: &RunId) -> Result<(), DslTransitionError>;
    fn commit(&self, input_id: &InputId, run_id: &RunId) -> Result<(), DslTransitionError>;
    fn fail(&self, run_id: &RunId) -> Result<(), DslTransitionError>;

    // --- Drain-queued run signal (run_id carried as param per DSL schema) ---
    fn drain_queued_run(&self, run_id: &RunId) -> Result<(), DslTransitionError>;
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

/// External tool surface lifecycle DSL handle.
///
/// Covers the MCP tool-surface signals on the MeerkatMachine DSL: stage add /
/// remove / reload, boundary apply, pending outcome, call tracking, finalize
/// clean vs forced, snapshot alignment, and shutdown. All signals are
/// parameterless — per-surface state is tracked by the DSL's own maps keyed
/// on `surface_id` (stored upstream via surface registration plumbing).
pub trait ExternalToolSurfaceHandle: Send + Sync {
    /// Fire the `StageAdd` signal.
    fn stage_add(&self) -> Result<(), DslTransitionError>;
    /// Fire the `StageRemove` signal.
    fn stage_remove(&self) -> Result<(), DslTransitionError>;
    /// Fire the `StageReload` signal.
    fn stage_reload(&self) -> Result<(), DslTransitionError>;
    /// Fire the `ApplySurfaceBoundary` signal.
    fn apply_surface_boundary(&self) -> Result<(), DslTransitionError>;
    /// Fire the `PendingSucceeded` signal for a surface pending task.
    fn pending_succeeded(&self) -> Result<(), DslTransitionError>;
    /// Fire the `PendingFailed` signal for a surface pending task.
    fn pending_failed(&self) -> Result<(), DslTransitionError>;
    /// Fire the `CallStarted` signal when an in-flight surface call begins.
    fn call_started(&self) -> Result<(), DslTransitionError>;
    /// Fire the `CallFinished` signal when an in-flight surface call ends.
    fn call_finished(&self) -> Result<(), DslTransitionError>;
    /// Fire the `FinalizeRemovalClean` signal — surface removed without force.
    fn finalize_removal_clean(&self) -> Result<(), DslTransitionError>;
    /// Fire the `FinalizeRemovalForced` signal — surface removed after timeout.
    fn finalize_removal_forced(&self) -> Result<(), DslTransitionError>;
    /// Fire the `SnapshotAligned` signal — shell rebuilt visible set to epoch.
    fn snapshot_aligned(&self) -> Result<(), DslTransitionError>;
    /// Fire the `ShutdownSurface` signal — surface dispatcher is shutting down.
    fn shutdown_surface(&self) -> Result<(), DslTransitionError>;
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

    /// Fire the `AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, run_id }` input.
    fn accept_with_completion(
        &self,
        input_id: &InputId,
        request_immediate_processing: bool,
        interrupt_yielding: bool,
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
