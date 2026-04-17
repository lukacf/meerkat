//! MeerkatMachine — session-scoped execution kernel.
//!
//! One of two kernels in the Meerkat two-kernel architecture:
//!
//! - **MeerkatMachine** (this module) owns session-scoped runtime state:
//!   input ingress, run lifecycle, completion waiters, async-ops registry,
//!   comms drain, and tool visibility publication. All mutations flow through
//!   one unified internal reducer, gated by TLA+-derived precondition guards.
//!
//! - **MobMachine** (`meerkat-mob`) owns mob-scoped orchestration: roster,
//!   flow frames, delegation, and inter-member wiring.
//!
//! MeerkatMachine lives in `meerkat-runtime` so `meerkat-session` does not
//! depend on runtime execution internals. When a session registers a
//! `CoreExecutor`, a background `RuntimeLoop` task is spawned. Input acceptance
//! queues through the driver; wake signals the loop; the loop dequeues, stages,
//! applies via `CoreExecutor`, and marks inputs consumed.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::sync::RwLock as StdRwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use meerkat_core::BlobStore;
use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::SessionId;
use meerkat_core::{
    SessionToolVisibilityState, ToolFilter, ToolScopeApplyError, ToolScopeRevision,
    ToolScopeStageError, ToolVisibilityOwner, ToolVisibilityWitness,
};

use crate::accept::AcceptOutcome;
use crate::driver::ephemeral::EphemeralRuntimeDriver;
use crate::driver::persistent::PersistentRuntimeDriver;
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_state::InputLifecycleState;
use crate::meerkat_machine_types::{
    HydratedSessionLlmState, MeerkatAdmittedInputSnapshot, MeerkatBindingSnapshot,
    MeerkatCompletionWaiterSnapshot, MeerkatCompletionWaitersSnapshot, MeerkatControlSnapshot,
    MeerkatCursorSnapshot, MeerkatDrainSnapshot, MeerkatDriverKind, MeerkatFormalStateProjection,
    MeerkatInputsSnapshot, MeerkatLedgerSnapshot, MeerkatMachineCommand,
    MeerkatMachineCommandError, MeerkatMachineCommandResult, MeerkatMachineLegacyRunPrepared,
    MeerkatMachineSpineSnapshot, MeerkatOpsSnapshot, SessionLlmCapabilityDelta,
    SessionLlmCapabilitySurface, SessionLlmCapabilitySurfaceStatus, SessionLlmReconfigureHost,
    SessionLlmReconfigureReport, SessionLlmReconfigureRequest, SessionToolVisibilityDelta,
};
use crate::runtime_state::RuntimeState;
use crate::service_ext::{RuntimeMode, SessionServiceRuntimeExt};
use crate::store::RuntimeStore;
use crate::tokio;
use crate::tokio::sync::{Mutex, RwLock, mpsc};
#[cfg(test)]
use crate::traits::RuntimeDriver;
use crate::traits::{
    DestroyReport, RecoveryReport, RecycleReport, ResetReport, RetireReport,
    RuntimeControlPlaneError, RuntimeDriverError,
};

/// Error type for [`MeerkatMachine::prepare_bindings`].
#[derive(Debug, thiserror::Error)]
pub enum RuntimeBindingsError {
    /// Session was not found after registration (should not happen in practice).
    #[error("session {0} not found in runtime adapter after registration")]
    SessionNotFound(SessionId),
}

pub(crate) use driver::{
    DriverEntry, SharedCompletionRegistry, SharedDriver, commit_runtime_loop_run,
    fail_runtime_loop_run, machine_apply_run_return_projection, machine_begin_run, machine_destroy,
    machine_executor_attach_projection, machine_input_boundary,
    machine_prepare_bindings_projection, machine_recover_ephemeral_driver,
    machine_recover_persistent_driver, machine_recycle_preserving_work, machine_reset,
    machine_retire, machine_select_runtime_loop_batch, machine_stop_runtime,
    machine_unregister_session_projection, prepare_runtime_loop_batch_start,
};

pub(crate) mod driver;

mod comms_drain;
mod dispatch_control;
mod dispatch_drain;
mod dispatch_ingress;
mod dispatch_session;
#[allow(unused_variables, dead_code, clippy::cmp_owned)]
#[allow(clippy::assign_op_pattern)]
pub(crate) mod dsl;
pub(crate) mod dsl_authority;
mod llm_reconfigure;
mod runtime_control;
mod session_management;
mod traits;
mod visibility;

pub use comms_drain::{CommsDrainMode, CommsDrainPhase, DrainExitReason};
pub(crate) use comms_drain::{CommsDrainSlot, abort_slot};
pub(crate) use visibility::MachineToolVisibilityOwner;

/// Per-session state: driver + registration phase.
struct RuntimeSessionEntry {
    /// Per-session mutation gate.
    ///
    /// Serializes same-session mutating commands across the full
    /// DSL-stage → driver-mutate → DSL-sync span. Without this gate,
    /// two concurrent commands on the same session can interleave between
    /// the DSL projection sync (which releases `sessions` lock) and the
    /// driver mutation (which acquires `driver` lock independently).
    ///
    /// This is NOT a replacement for `sessions` RwLock or `driver` Mutex —
    /// it is an additional serialization point that spans the entire
    /// multi-step mutation window.
    mutation_gate: Arc<Mutex<()>>,
    /// Shared driver handle (accessed by both adapter methods and RuntimeLoop).
    driver: SharedDriver,
    /// Canonical coarse control projection for this session.
    ///
    /// The driver reads this to realize shell mechanics, but machine-facing
    /// queries should publish from this shared cell rather than treating the
    /// driver shell as the source of lifecycle truth.
    control_projection: Arc<StdRwLock<crate::driver::ephemeral::RuntimeControlProjection>>,
    /// Shared async-operation lifecycle registry for this runtime/session.
    ops_lifecycle: Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>,
    /// Runtime epoch identity — stable across rebuilds, rotated on reset/restart-without-recovery.
    epoch_id: meerkat_core::RuntimeEpochId,
    /// Shared consumer cursor state for the epoch.
    cursor_state: Arc<meerkat_core::EpochCursorState>,
    /// Completion waiters (accessed by accept_input_with_completion and RuntimeLoop).
    completions: SharedCompletionRegistry,
    /// Canonical durable visibility owner for this session.
    tool_visibility_owner: Arc<MachineToolVisibilityOwner>,
    /// Machine-owned current durable/live LLM identity for the registered session.
    current_llm_identity: Option<meerkat_core::SessionLlmIdentity>,
    /// Machine-owned current capability surface for the registered session.
    current_capability_surface: Option<SessionLlmCapabilitySurface>,
    /// Whether the machine has a resolved capability surface for the current identity.
    capability_surface_status: SessionLlmCapabilitySurfaceStatus,
    /// Registration phase — explicit type-level distinction between
    /// "registered but inert" and "executor attached."
    phase: RegistrationPhase,
    /// Detached-wake state for background op completions.
    /// Shared with the runtime loop which selects on the Notify directly.
    detached_wake: Option<Arc<crate::detached_wake::DetachedWakeState>>,
    /// DSL authority for coarse lifecycle phase transitions.
    /// Sync field — validates transitions, writes back phase.
    ///
    /// `Arc<std::sync::Mutex<_>>` so cross-crate handle impls
    /// (`meerkat-runtime::handles::*`) can share the same underlying authority
    /// from a sync context without awaiting the outer `sessions` tokio lock.
    /// The Arc heap-allocates the authority's large expanded state (31 fields
    /// including several Maps/Sets) so holding a reference to a
    /// `RuntimeSessionEntry` does not bloat async future sizes.
    dsl_authority: Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
}

/// Capability bundle for an attached runtime loop.
///
/// Keep all loop-related handles together so "attached vs detached" cannot
/// drift into partially-populated shell state.
struct RuntimeLoopAttachment {
    wake_tx: mpsc::Sender<()>,
    control_tx: mpsc::Sender<RunControlCommand>,
    _loop_handle: tokio::task::JoinHandle<()>,
}

/// Explicit registration phase — the type-level distinction between
/// "registered but inert," "attachment in progress," and "executor attached."
///
/// Replaces the implicit `Option<RuntimeLoopAttachment>` discriminant
/// (Dogma §8: Option must not hide ownership uncertainty).
enum RegistrationPhase {
    /// Registered via `prepare_bindings()`. No executor — inputs queue
    /// but are not processed until an executor attaches.
    Queuing,
    /// `ensure_session_with_executor()` is in progress — another task is
    /// wiring the runtime loop. Concurrent callers must treat this as
    /// "attachment pending" and not race a second loop spawn.
    Attaching,
    /// Executor attached with live channels. Inputs are processed
    /// by the RuntimeLoop.
    Active(RuntimeLoopAttachment),
}

impl RuntimeSessionEntry {
    fn control_snapshot(&self) -> crate::driver::ephemeral::RuntimeControlProjection {
        self.control_projection
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| {
                tracing::error!("runtime control projection lock poisoned");
                poisoned.into_inner().clone()
            })
    }

    fn attachment_is_live(&self) -> bool {
        match &self.phase {
            RegistrationPhase::Active(attachment) => {
                !attachment.wake_tx.is_closed() && !attachment.control_tx.is_closed()
            }
            RegistrationPhase::Queuing | RegistrationPhase::Attaching => false,
        }
    }

    /// Returns `true` if an executor is attached with live channels, OR if
    /// attachment is in progress (another task is wiring the loop).
    /// Used by external-facing queries (`session_has_executor`) to prevent
    /// concurrent callers from racing a second loop spawn.
    fn has_attachment_or_attaching(&self) -> bool {
        matches!(self.phase, RegistrationPhase::Attaching) || self.attachment_is_live()
    }

    /// Returns `true` only if the executor is fully attached with live channels.
    /// Used by internal publish logic within `ensure_session_with_executor`
    /// where the caller itself may have set `Attaching`.
    fn has_live_attachment(&self) -> bool {
        self.attachment_is_live()
    }

    fn attach_runtime_loop(
        &mut self,
        wake_tx: mpsc::Sender<()>,
        control_tx: mpsc::Sender<RunControlCommand>,
        loop_handle: tokio::task::JoinHandle<()>,
    ) {
        self.phase = RegistrationPhase::Active(RuntimeLoopAttachment {
            wake_tx,
            control_tx,
            _loop_handle: loop_handle,
        });
    }

    fn clear_dead_attachment(&mut self) -> bool {
        if matches!(self.phase, RegistrationPhase::Active(_)) && !self.attachment_is_live() {
            // Don't regress to Queuing if another task is mid-attach;
            // Active with dead channels goes back to Queuing for retry.
            self.phase = RegistrationPhase::Queuing;
            // Clear detached wake state — it will be re-created on
            // re-registration along with the new runtime loop.
            self.detached_wake = None;
            return true;
        }
        false
    }

    fn wake_sender(&self) -> Option<mpsc::Sender<()>> {
        match &self.phase {
            RegistrationPhase::Active(attachment)
                if !attachment.wake_tx.is_closed() && !attachment.control_tx.is_closed() =>
            {
                Some(attachment.wake_tx.clone())
            }
            _ => None,
        }
    }

    fn control_sender(&self) -> Option<mpsc::Sender<RunControlCommand>> {
        match &self.phase {
            RegistrationPhase::Active(attachment)
                if !attachment.wake_tx.is_closed() && !attachment.control_tx.is_closed() =>
            {
                Some(attachment.control_tx.clone())
            }
            _ => None,
        }
    }
}

impl MeerkatMachine {
    /// Acquire the per-session mutation gate.
    ///
    /// Returns an `Arc<Mutex<()>>` that the caller must `.lock().await` and
    /// hold across the full DSL-stage → driver-mutate → DSL-sync span.
    /// Returns `None` if the session is not registered.
    async fn session_mutation_gate(&self, session_id: &SessionId) -> Option<Arc<Mutex<()>>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|entry| Arc::clone(&entry.mutation_gate))
    }

    async fn stage_session_dsl_input(
        &self,
        session_id: &SessionId,
        input: dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<Box<dsl::MeerkatMachineState>, String> {
        let mut sessions = self.sessions.write().await;
        let entry = sessions.get_mut(session_id).ok_or_else(|| {
            RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            }
            .to_string()
        })?;
        let mut authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous_state = Box::new(authority.state.clone());
        dsl::MeerkatMachineMutator::apply(&mut *authority, input)
            .map_err(|err| dsl_authority::map_error(err, context))?;
        Ok(previous_state)
    }

    async fn restore_session_dsl_state(
        &self,
        session_id: &SessionId,
        state: Box<dsl::MeerkatMachineState>,
    ) {
        let mut sessions = self.sessions.write().await;
        if let Some(entry) = sessions.get_mut(session_id) {
            let mut authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *authority = dsl::MeerkatMachineAuthority::from_state(*state);
        }
    }

    /// Fire `RuntimeExecutorExited` on the session's DSL authority after the
    /// runtime loop has realised an async stop into the driver's control
    /// projection. Called via `Weak<Self>` from `control_plane`.
    pub(crate) async fn notify_runtime_executor_exited(&self, session_id: &SessionId) {
        let mut sessions = self.sessions.write().await;
        if let Some(entry) = sessions.get_mut(session_id) {
            let mut authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Err(err) = dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                dsl::MeerkatMachineInput::RuntimeExecutorExited,
            ) {
                tracing::debug!(
                    %session_id,
                    error = %dsl_authority::map_error(err, "RuntimeExecutorExited"),
                    "DSL rejected RuntimeExecutorExited (terminal or phase-not-covered)"
                );
            }
        }
    }
}

/// Per-session comms drain slot, driven by direct in-kernel state.
/// Session-scoped execution kernel for the Meerkat runtime.
///
/// Owns per-session runtime state (driver, ops registry, completion waiters,
/// comms drain, epoch bindings) and routes all internal mutations through one
/// canonical command reducer, with smaller group handlers retained only as
/// implementation detail helpers.
pub struct MeerkatMachine {
    /// Per-session entries.
    sessions: RwLock<HashMap<SessionId, RuntimeSessionEntry>>,
    /// Runtime mode.
    mode: RuntimeMode,
    /// Optional RuntimeStore for persistent drivers.
    store: Option<Arc<dyn RuntimeStore>>,
    /// Blob store used by persistent drivers for durable input externalization.
    blob_store: Option<Arc<dyn BlobStore>>,
    /// Per-session comms drain lifecycle.
    comms_drain_slots: RwLock<HashMap<SessionId, CommsDrainSlot>>,
    /// Runtime-owned shell seam for live session LLM reconfiguration I/O.
    llm_reconfigure_host: StdRwLock<Option<Arc<dyn SessionLlmReconfigureHost>>>,
}

impl MeerkatMachine {
    fn normalize_destroyed_error(err: RuntimeDriverError) -> RuntimeDriverError {
        match err {
            RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            } => RuntimeDriverError::Destroyed,
            other => other,
        }
    }

    /// Create an ephemeral adapter (all sessions use EphemeralRuntimeDriver).
    pub fn ephemeral() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            mode: RuntimeMode::V9Compliant,
            store: None,
            blob_store: None,
            comms_drain_slots: RwLock::new(HashMap::new()),
            llm_reconfigure_host: StdRwLock::new(None),
        }
    }

    /// Create a persistent adapter with a RuntimeStore.
    pub fn persistent(store: Arc<dyn RuntimeStore>, blob_store: Arc<dyn BlobStore>) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            mode: RuntimeMode::V9Compliant,
            store: Some(store),
            blob_store: Some(blob_store),
            comms_drain_slots: RwLock::new(HashMap::new()),
            llm_reconfigure_host: StdRwLock::new(None),
        }
    }

    /// Create a persistent adapter with a RuntimeStore but no blob store.
    ///
    /// The driver will fall back to ephemeral mode for sessions (no durable
    /// boundary commits), but ops lifecycle recovery from the store still works.
    /// Primarily useful for tests that need to verify recovery without needing
    /// a full blob store.
    pub fn persistent_without_blobs(store: Arc<dyn RuntimeStore>) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            mode: RuntimeMode::V9Compliant,
            store: Some(store),
            blob_store: None,
            comms_drain_slots: RwLock::new(HashMap::new()),
            llm_reconfigure_host: StdRwLock::new(None),
        }
    }

    /// Create a driver entry for a session.
    fn make_driver(&self, session_id: &SessionId) -> DriverEntry {
        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let control_projection = Arc::new(StdRwLock::new(
            crate::driver::ephemeral::RuntimeControlProjection::default(),
        ));
        match (&self.store, &self.blob_store) {
            (Some(store), Some(blob_store)) => {
                DriverEntry::Persistent(PersistentRuntimeDriver::new_with_control(
                    runtime_id,
                    store.clone(),
                    blob_store.clone(),
                    control_projection,
                ))
            }
            (Some(_store), None) => {
                tracing::warn!(
                    %session_id,
                    "persistent runtime store present but blob store missing; \
                     falling back to ephemeral driver"
                );
                DriverEntry::Ephemeral(EphemeralRuntimeDriver::new_with_control(
                    runtime_id,
                    control_projection,
                ))
            }
            _ => DriverEntry::Ephemeral(EphemeralRuntimeDriver::new_with_control(
                runtime_id,
                control_projection,
            )),
        }
    }

    /// Recover or create fresh ops lifecycle state for a session.
    ///
    /// This is the single canonical recovery seam. Both `register_session()`
    /// and `ensure_session_with_executor()`'s cold path call this to create
    /// epoch-local state. If a durable store is available, attempts to load
    /// the persisted snapshot; otherwise creates fresh state with a new epoch.
    async fn recover_or_create_ops_state(
        &self,
        session_id: &SessionId,
    ) -> (
        Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>,
        meerkat_core::RuntimeEpochId,
        Arc<meerkat_core::EpochCursorState>,
    ) {
        if let Some(ref store) = self.store {
            let runtime_id = crate::identifiers::LogicalRuntimeId::new(session_id.to_string());
            match store.load_ops_lifecycle(&runtime_id).await {
                Ok(Some(snapshot)) => {
                    let recovered_epoch = snapshot.epoch_id.clone();
                    let recovered_cursors = meerkat_core::EpochCursorState::from_recovered(
                        snapshot.cursors.agent_applied_cursor,
                        snapshot.cursors.runtime_observed_seq,
                        snapshot.cursors.runtime_last_injected_seq,
                    );
                    let recovered_ops_count = snapshot.completion_entries.len();
                    let registry =
                        crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::from_recovered(snapshot);
                    tracing::info!(
                        %session_id,
                        epoch_id = %recovered_epoch,
                        recovered_ops = recovered_ops_count,
                        "ops lifecycle recovered from durable store (same epoch)"
                    );
                    (
                        Arc::new(registry),
                        recovered_epoch,
                        Arc::new(recovered_cursors),
                    )
                }
                Ok(None) => {
                    tracing::debug!(%session_id, "no persisted ops lifecycle; fresh epoch");
                    (
                        Arc::new(crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()),
                        meerkat_core::RuntimeEpochId::new(),
                        Arc::new(meerkat_core::EpochCursorState::new()),
                    )
                }
                Err(err) => {
                    tracing::warn!(
                        %session_id,
                        error = %err,
                        "failed to load ops lifecycle; epoch rotated"
                    );
                    (
                        Arc::new(crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()),
                        meerkat_core::RuntimeEpochId::new(),
                        Arc::new(meerkat_core::EpochCursorState::new()),
                    )
                }
            }
        } else {
            (
                Arc::new(crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()),
                meerkat_core::RuntimeEpochId::new(),
                Arc::new(meerkat_core::EpochCursorState::new()),
            )
        }
    }

    async fn execute_meerkat_machine_command(
        &self,
        self_handle: Option<Arc<Self>>,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, MeerkatMachineCommandError> {
        match command {
            MeerkatMachineCommand::EnsureSessionWithExecutor { .. } => {
                let self_handle = self_handle.ok_or_else(|| {
                    MeerkatMachineCommandError::Driver(RuntimeDriverError::Internal(
                        "EnsureSessionWithExecutor requires Arc<Self> machine handle".into(),
                    ))
                })?;
                self_handle
                    .execute_meerkat_machine_ensure_session_command(command)
                    .await
                    .map_err(Into::into)
            }
            MeerkatMachineCommand::RegisterSession { .. }
            | MeerkatMachineCommand::UnregisterSession { .. }
            | MeerkatMachineCommand::SetSilentIntents { .. }
            | MeerkatMachineCommand::InterruptCurrentRun { .. }
            | MeerkatMachineCommand::CancelAfterBoundary { .. }
            | MeerkatMachineCommand::StopRuntimeExecutor { .. }
            | MeerkatMachineCommand::ContainsSession { .. }
            | MeerkatMachineCommand::SessionHasExecutor { .. }
            | MeerkatMachineCommand::SessionHasComms { .. }
            | MeerkatMachineCommand::OpsLifecycleRegistry { .. }
            | MeerkatMachineCommand::PrepareBindings { .. }
            | MeerkatMachineCommand::InputState { .. }
            | MeerkatMachineCommand::ListActiveInputs { .. }
            | MeerkatMachineCommand::ReconfigureSessionLlmIdentity { .. }
            | MeerkatMachineCommand::StagePersistentFilter { .. }
            | MeerkatMachineCommand::RequestDeferredTools { .. }
            | MeerkatMachineCommand::PublishCommittedVisibleSet { .. } => self
                .execute_meerkat_machine_session_command(command)
                .await
                .map_err(Into::into),
            MeerkatMachineCommand::SetPeerIngressContext { .. }
            | MeerkatMachineCommand::NotifyDrainExited { .. } => {
                let self_handle = self_handle.ok_or_else(|| {
                    MeerkatMachineCommandError::Driver(RuntimeDriverError::Internal(
                        "drain command requires Arc<Self> machine handle".into(),
                    ))
                })?;
                self_handle
                    .execute_meerkat_machine_drain_command(command)
                    .await
                    .map_err(Into::into)
            }
            MeerkatMachineCommand::AbortAll
            | MeerkatMachineCommand::Abort { .. }
            | MeerkatMachineCommand::Wait { .. } => self
                .execute_meerkat_machine_drain_local_command(command)
                .await
                .map_err(Into::into),
            MeerkatMachineCommand::Ingest { .. }
            | MeerkatMachineCommand::PublishEvent { .. }
            | MeerkatMachineCommand::Retire { .. }
            | MeerkatMachineCommand::Recycle { .. }
            | MeerkatMachineCommand::Reset { .. }
            | MeerkatMachineCommand::Recover { .. }
            | MeerkatMachineCommand::Destroy { .. }
            | MeerkatMachineCommand::RuntimeState { .. }
            | MeerkatMachineCommand::LoadBoundaryReceipt { .. } => self
                .execute_meerkat_machine_control_command(command)
                .await
                .map_err(Into::into),
            MeerkatMachineCommand::AcceptWithCompletion { .. }
            | MeerkatMachineCommand::AcceptWithoutWake { .. } => self
                .execute_meerkat_machine_ingress_command(command)
                .await
                .map_err(Into::into),
            MeerkatMachineCommand::Prepare { .. }
            | MeerkatMachineCommand::Commit { .. }
            | MeerkatMachineCommand::Fail { .. } => self
                .execute_meerkat_machine_legacy_run_command(command)
                .await
                .map_err(Into::into),
        }
    }

    /// Register a runtime driver for a session (no RuntimeLoop — inputs queue but
    /// nothing processes them automatically). Useful for tests and legacy mode.
    pub async fn register_session(&self, session_id: SessionId) {
        let _ = self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::RegisterSession { session_id },
            )
            .await;
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
#[path = "../meerkat_machine_tests.rs"]
mod tests;
