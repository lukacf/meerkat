//! MeerkatMachine — owns per-session runtime state and command authority.
//!
//! It lives in `meerkat-runtime` so `meerkat-session` doesn't need to depend
//! on runtime execution internals. Surfaces use this authority to get v9
//! runtime capabilities on top of any `SessionService` implementation.
//!
//! When a session is registered with a `CoreExecutor`, a background RuntimeLoop
//! task is spawned per session. `accept_input()` queues the input in the driver
//! and, if wake is requested, signals the loop. The loop dequeues, stages,
//! applies via CoreExecutor (which calls SessionService::start_turn()), and
//! marks inputs as consumed.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use meerkat_core::BlobStore;
use meerkat_core::comms_drain_lifecycle_authority::{
    CommsDrainLifecycleAuthority, CommsDrainLifecycleEffect, CommsDrainMode, DrainExitReason,
};
use meerkat_core::generated::{protocol_comms_drain_abort, protocol_comms_drain_spawn};
use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::SessionId;

use crate::accept::AcceptOutcome;
use crate::driver::ephemeral::EphemeralRuntimeDriver;
use crate::driver::persistent::PersistentRuntimeDriver;
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_lifecycle_authority::InputLifecycleError;
use crate::input_state::InputState;
use crate::meerkat_machine_types::{
    MeerkatAdmittedInputSnapshot, MeerkatBindingSnapshot, MeerkatCompletionWaiterSnapshot,
    MeerkatCompletionWaitersSnapshot, MeerkatControlSnapshot, MeerkatCursorSnapshot,
    MeerkatDrainSnapshot, MeerkatDriverKind, MeerkatInputsSnapshot, MeerkatMachineControlCommand,
    MeerkatMachineControlCommandResult, MeerkatMachineDrainCommand,
    MeerkatMachineDrainCommandResult, MeerkatMachineDrainLocalCommand,
    MeerkatMachineIngressCommand, MeerkatMachineIngressCommandResult,
    MeerkatMachineLegacyRunCommand, MeerkatMachineLegacyRunCommandResult,
    MeerkatMachineLegacyRunPrepared, MeerkatMachineSessionCommand,
    MeerkatMachineSessionCommandResult, MeerkatMachineSpineSnapshot, MeerkatOpsSnapshot,
};
use crate::runtime_state::{RuntimeState, RuntimeStateTransitionError};
use crate::service_ext::{RuntimeMode, SessionServiceRuntimeExt};
use crate::store::RuntimeStore;
use crate::tokio;
use crate::tokio::sync::{Mutex, RwLock, mpsc};
use crate::traits::{
    DestroyReport, RecoveryReport, RecycleReport, ResetReport, RetireReport,
    RuntimeControlPlaneError, RuntimeDriver, RuntimeDriverError,
};

/// Error type for [`MeerkatMachine::prepare_bindings`].
#[derive(Debug, thiserror::Error)]
pub enum RuntimeBindingsError {
    /// Session was not found after registration (should not happen in practice).
    #[error("session {0} not found in runtime adapter after registration")]
    SessionNotFound(SessionId),
}

/// Shared driver handle used by both the adapter and the RuntimeLoop.
pub(crate) type SharedDriver = Arc<Mutex<DriverEntry>>;

/// Per-session runtime driver entry.
pub(crate) enum DriverEntry {
    Ephemeral(EphemeralRuntimeDriver),
    Persistent(PersistentRuntimeDriver),
}

impl DriverEntry {
    #[allow(dead_code)]
    pub(crate) fn runtime_id(&self) -> &LogicalRuntimeId {
        match self {
            DriverEntry::Ephemeral(d) => d.runtime_id(),
            DriverEntry::Persistent(d) => d.runtime_id(),
        }
    }

    pub(crate) fn as_driver(&self) -> &dyn RuntimeDriver {
        match self {
            DriverEntry::Ephemeral(d) => d,
            DriverEntry::Persistent(d) => d,
        }
    }

    pub(crate) fn as_driver_mut(&mut self) -> &mut dyn RuntimeDriver {
        match self {
            DriverEntry::Ephemeral(d) => d,
            DriverEntry::Persistent(d) => d,
        }
    }

    /// Set the silent comms intents for the underlying driver.
    pub(crate) fn set_silent_comms_intents(&mut self, intents: Vec<String>) {
        match self {
            DriverEntry::Ephemeral(d) => d.set_silent_comms_intents(intents),
            DriverEntry::Persistent(d) => d.set_silent_comms_intents(intents),
        }
    }

    /// Check if the runtime is idle or attached (quiescent with or without executor).
    pub(crate) fn is_idle_or_attached(&self) -> bool {
        match self {
            DriverEntry::Ephemeral(d) => d.is_idle_or_attached(),
            DriverEntry::Persistent(d) => d.is_idle_or_attached(),
        }
    }

    /// Whether this session is quiescent for detached-wake purposes.
    ///
    /// A session is quiescent when it is idle/attached (not running) AND has
    /// no non-terminal inputs in its ledger. Queued-only inputs intentionally
    /// block quiescence — `accept_input_without_wake` stages work without
    /// waking, so detached-wake must not race with pending queue processing.
    pub(crate) fn is_quiescent_for_detached_wake(&self) -> bool {
        self.is_idle_or_attached() && self.as_driver().active_input_ids().is_empty()
    }

    /// Attach an executor (Idle → Attached).
    pub(crate) fn attach(&mut self) -> Result<(), RuntimeStateTransitionError> {
        match self {
            DriverEntry::Ephemeral(d) => d.attach(),
            DriverEntry::Persistent(d) => d.attach(),
        }
    }

    /// Detach an executor (Attached → Idle). No-op if not Attached.
    pub(crate) fn detach(
        &mut self,
    ) -> Result<Option<crate::runtime_state::RuntimeState>, RuntimeStateTransitionError> {
        match self {
            DriverEntry::Ephemeral(d) => d.detach(),
            DriverEntry::Persistent(d) => d.detach(),
        }
    }

    /// Check if the runtime can process queued inputs (Idle, Attached, or Retired).
    pub(crate) fn can_process_queue(&self) -> bool {
        match self {
            DriverEntry::Ephemeral(d) => d.control().can_process_queue(),
            DriverEntry::Persistent(d) => d.inner_ref().control().can_process_queue(),
        }
    }

    /// Drain and return the typed post-admission signal.
    pub(crate) fn take_post_admission_signal(
        &mut self,
    ) -> crate::driver::ephemeral::PostAdmissionSignal {
        match self {
            DriverEntry::Ephemeral(d) => d.take_post_admission_signal(),
            DriverEntry::Persistent(d) => d.take_post_admission_signal(),
        }
    }

    /// Check and clear the wake flag (backward-compat wrapper).
    pub(crate) fn take_wake_requested(&mut self) -> bool {
        match self {
            DriverEntry::Ephemeral(d) => d.take_wake_requested(),
            DriverEntry::Persistent(d) => d.take_wake_requested(),
        }
    }

    /// Dequeue the next input for processing.
    pub(crate) fn dequeue_next(&mut self) -> Option<(InputId, Input)> {
        match self {
            DriverEntry::Ephemeral(d) => d.dequeue_next(),
            DriverEntry::Persistent(d) => d.dequeue_next(),
        }
    }

    /// Dequeue a specific input by ID from whichever queue contains it.
    pub(crate) fn dequeue_by_id(&mut self, input_id: &InputId) -> Option<(InputId, Input)> {
        match self {
            DriverEntry::Ephemeral(d) => d.dequeue_by_id(input_id),
            DriverEntry::Persistent(d) => d.dequeue_by_id(input_id),
        }
    }

    /// Get a reference to the ingress authority.
    pub(crate) fn ingress(&self) -> &crate::runtime_ingress_authority::RuntimeIngressAuthority {
        match self {
            DriverEntry::Ephemeral(d) => d.ingress(),
            DriverEntry::Persistent(d) => d.inner_ref().ingress(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn control(&self) -> &crate::runtime_control_authority::RuntimeControlAuthority {
        match self {
            DriverEntry::Ephemeral(d) => d.control(),
            DriverEntry::Persistent(d) => d.inner_ref().control(),
        }
    }

    pub(crate) fn has_queued_input_outside(&self, excluded: &[InputId]) -> bool {
        match self {
            DriverEntry::Ephemeral(d) => d.has_queued_input_outside(excluded),
            DriverEntry::Persistent(d) => d.has_queued_input_outside(excluded),
        }
    }

    /// Start a new run (Idle → Running).
    pub(crate) fn start_run(&mut self, run_id: RunId) -> Result<(), RuntimeStateTransitionError> {
        match self {
            DriverEntry::Ephemeral(d) => d.start_run(run_id),
            DriverEntry::Persistent(d) => d.start_run(run_id),
        }
    }

    /// Complete a run (Running → Idle).
    pub(crate) fn complete_run(&mut self) -> Result<RunId, RuntimeStateTransitionError> {
        match self {
            DriverEntry::Ephemeral(d) => d.complete_run(),
            DriverEntry::Persistent(d) => d.complete_run(),
        }
    }

    /// Stage an input (Queued → Staged).
    pub(crate) fn stage_input(
        &mut self,
        input_id: &InputId,
        run_id: &RunId,
    ) -> Result<(), InputLifecycleError> {
        match self {
            DriverEntry::Ephemeral(d) => d.stage_input(input_id, run_id),
            DriverEntry::Persistent(d) => d.stage_input(input_id, run_id),
        }
    }

    /// Stage a batch of inputs atomically in a single `StageDrainSnapshot`.
    pub(crate) fn stage_batch(
        &mut self,
        input_ids: &[InputId],
        run_id: &RunId,
    ) -> Result<(), InputLifecycleError> {
        match self {
            DriverEntry::Ephemeral(d) => d.stage_batch(input_ids, run_id),
            DriverEntry::Persistent(d) => d.stage_batch(input_ids, run_id),
        }
    }

    /// Roll back staged inputs after a failed staging attempt.
    pub(crate) fn rollback_staged(
        &mut self,
        input_ids: &[InputId],
    ) -> Result<(), InputLifecycleError> {
        match self {
            DriverEntry::Ephemeral(d) => d.rollback_staged(input_ids),
            DriverEntry::Persistent(d) => d.rollback_staged(input_ids),
        }
    }

    pub(crate) async fn abandon_pending_inputs(
        &mut self,
        reason: crate::input_state::InputAbandonReason,
    ) -> Result<usize, RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => Ok(d.abandon_pending_inputs(reason)),
            DriverEntry::Persistent(d) => d.abandon_pending_inputs(reason).await,
        }
    }
}

/// Shared completion registry (accessed by adapter for registration and loop for resolution).
pub(crate) type SharedCompletionRegistry = Arc<Mutex<crate::completion::CompletionRegistry>>;

/// Per-session state: driver + registration phase.
struct RuntimeSessionEntry {
    /// Shared driver handle (accessed by both adapter methods and RuntimeLoop).
    driver: SharedDriver,
    /// Shared async-operation lifecycle registry for this runtime/session.
    ops_lifecycle: Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>,
    /// Runtime epoch identity — stable across rebuilds, rotated on reset/restart-without-recovery.
    epoch_id: meerkat_core::RuntimeEpochId,
    /// Shared consumer cursor state for the epoch.
    cursor_state: Arc<meerkat_core::EpochCursorState>,
    /// Completion waiters (accessed by accept_input_with_completion and RuntimeLoop).
    completions: SharedCompletionRegistry,
    /// Registration phase — explicit type-level distinction between
    /// "registered but inert" and "executor attached."
    phase: RegistrationPhase,
    /// Detached-wake state for background op completions.
    /// Shared with the runtime loop which selects on the Notify directly.
    detached_wake: Option<Arc<crate::detached_wake::DetachedWakeState>>,
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

/// Per-session comms drain slot, driven by `CommsDrainLifecycleAuthority`.
///
/// ALL state transitions go through the authority -- no manual
/// `handle.is_finished()` checks in shell code.
struct CommsDrainSlot {
    authority: CommsDrainLifecycleAuthority,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl CommsDrainSlot {
    fn new() -> Self {
        Self {
            authority: CommsDrainLifecycleAuthority::new(),
            handle: None,
        }
    }
}

fn apply_runtime_drain_effects(slot: &mut CommsDrainSlot, effects: &[CommsDrainLifecycleEffect]) {
    for effect in effects {
        if let CommsDrainLifecycleEffect::AbortDrainTask = effect
            && let Some(handle) = slot.handle.take()
        {
            handle.abort();
        }
    }
}

fn abort_slot(slot: &mut CommsDrainSlot) {
    match protocol_comms_drain_abort::execute_stop_requested(&mut slot.authority) {
        Ok(result) => {
            apply_runtime_drain_effects(slot, &result.effects);
            // Under TerminalClosure policy, the abort obligation is implicitly
            // satisfied when the machine reaches Stopped phase. Drop it.
            let _ = result.obligation;
        }
        Err(_) => {
            // Already stopped or inactive — just clean up the handle
            if let Some(handle) = slot.handle.take() {
                handle.abort();
            }
        }
    }
}

/// Wraps a SessionService to provide v9 runtime capabilities.
///
/// Maintains a per-session RuntimeDriver registry. When sessions are registered
/// with a `CoreExecutor`, a RuntimeLoop task is spawned that processes queued
/// inputs by calling `CoreExecutor::apply()` (which triggers
/// `SessionService::start_turn()` under the hood).
pub struct MeerkatMachine {
    /// Per-session entries.
    sessions: RwLock<HashMap<SessionId, RuntimeSessionEntry>>,
    /// Runtime mode.
    mode: RuntimeMode,
    /// Optional RuntimeStore for persistent drivers.
    store: Option<Arc<dyn RuntimeStore>>,
    /// Blob store used by persistent drivers for durable input externalization.
    blob_store: Option<Arc<dyn BlobStore>>,
    /// Per-session comms drain lifecycle, driven by machine authority.
    comms_drain_slots: RwLock<HashMap<SessionId, CommsDrainSlot>>,
}

impl MeerkatMachine {
    /// Create an ephemeral adapter (all sessions use EphemeralRuntimeDriver).
    pub fn ephemeral() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            mode: RuntimeMode::V9Compliant,
            store: None,
            blob_store: None,
            comms_drain_slots: RwLock::new(HashMap::new()),
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
        }
    }

    /// Create a driver entry for a session.
    fn make_driver(&self, session_id: &SessionId) -> DriverEntry {
        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        match (&self.store, &self.blob_store) {
            (Some(store), Some(blob_store)) => DriverEntry::Persistent(
                PersistentRuntimeDriver::new(runtime_id, store.clone(), blob_store.clone()),
            ),
            (Some(_store), None) => {
                tracing::warn!(
                    %session_id,
                    "persistent runtime store present but blob store missing; \
                     falling back to ephemeral driver"
                );
                DriverEntry::Ephemeral(EphemeralRuntimeDriver::new(runtime_id))
            }
            _ => DriverEntry::Ephemeral(EphemeralRuntimeDriver::new(runtime_id)),
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

    async fn execute_meerkat_machine_session_command(
        &self,
        command: MeerkatMachineSessionCommand,
    ) -> Result<MeerkatMachineSessionCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineSessionCommand::RegisterSession { session_id } => {
                // Guard: DestroyedShapeInvariant — a destroyed binding must
                // never be resurrected.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                self.register_session_inner(session_id).await;
                Ok(MeerkatMachineSessionCommandResult::Unit)
            }
            MeerkatMachineSessionCommand::UnregisterSession { session_id } => {
                // Guard: session must exist before it can be unregistered.
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                self.unregister_session_inner(&session_id).await;
                Ok(MeerkatMachineSessionCommandResult::Unit)
            }
            MeerkatMachineSessionCommand::EnsureSessionWithExecutor {
                session_id,
                executor,
            } => {
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                self.ensure_session_with_executor_inner(session_id, executor)
                    .await;
                Ok(MeerkatMachineSessionCommandResult::Unit)
            }
            MeerkatMachineSessionCommand::SetSilentIntents {
                session_id,
                intents,
            } => {
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                self.set_session_silent_intents_inner(&session_id, intents)
                    .await;
                Ok(MeerkatMachineSessionCommandResult::Unit)
            }
            MeerkatMachineSessionCommand::InterruptCurrentRun { session_id } => {
                // Guard: DestroyedShapeInvariant — no mutation on destroyed sessions.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                self.interrupt_current_run_inner(&session_id)
                    .await
                    .map(|()| MeerkatMachineSessionCommandResult::Unit)
            }
            MeerkatMachineSessionCommand::CancelAfterBoundary { session_id } => {
                // Guard: DestroyedShapeInvariant — no mutation on destroyed sessions.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                self.cancel_after_boundary_inner(&session_id)
                    .await
                    .map(|()| MeerkatMachineSessionCommandResult::Unit)
            }
            MeerkatMachineSessionCommand::StopRuntimeExecutor {
                session_id,
                command,
            } => {
                // Guard: DestroyedShapeInvariant — no mutation on destroyed sessions.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                self.stop_runtime_executor_inner(&session_id, command)
                    .await
                    .map(|()| MeerkatMachineSessionCommandResult::Unit)
            }
            MeerkatMachineSessionCommand::ContainsSession { session_id } => {
                Ok(MeerkatMachineSessionCommandResult::Bool(
                    self.sessions.read().await.contains_key(&session_id),
                ))
            }
            MeerkatMachineSessionCommand::SessionHasExecutor { session_id } => {
                let sessions = self.sessions.read().await;
                Ok(MeerkatMachineSessionCommandResult::Bool(
                    sessions
                        .get(&session_id)
                        .map(RuntimeSessionEntry::has_attachment_or_attaching)
                        .unwrap_or(false),
                ))
            }
            MeerkatMachineSessionCommand::SessionHasComms { session_id } => {
                let slots = self.comms_drain_slots.read().await;
                Ok(MeerkatMachineSessionCommandResult::Bool(
                    slots.contains_key(&session_id),
                ))
            }
            MeerkatMachineSessionCommand::OpsLifecycleRegistry { session_id } => {
                let sessions = self.sessions.read().await;
                Ok(MeerkatMachineSessionCommandResult::OpsLifecycleRegistry(
                    sessions
                        .get(&session_id)
                        .map(|e| Arc::clone(&e.ops_lifecycle)),
                ))
            }
            MeerkatMachineSessionCommand::PrepareBindings { session_id } => {
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                self.register_session_inner(session_id.clone()).await;
                let sessions = self.sessions.read().await;
                let entry = sessions
                    .get(&session_id)
                    .ok_or(RuntimeDriverError::Internal(format!(
                        "session {session_id} missing after register_session_inner"
                    )))?;
                Ok(MeerkatMachineSessionCommandResult::Bindings(
                    meerkat_core::SessionRuntimeBindings {
                        session_id,
                        epoch_id: entry.epoch_id.clone(),
                        ops_lifecycle: Arc::clone(&entry.ops_lifecycle)
                            as Arc<dyn meerkat_core::OpsLifecycleRegistry>,
                        cursor_state: Arc::clone(&entry.cursor_state),
                    },
                ))
            }
            MeerkatMachineSessionCommand::InputState {
                session_id,
                input_id,
            } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    let entry = sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?;
                    entry.driver.clone()
                };
                let driver = driver.lock().await;
                Ok(MeerkatMachineSessionCommandResult::InputState(
                    driver.as_driver().input_state(&input_id).cloned(),
                ))
            }
            MeerkatMachineSessionCommand::ListActiveInputs { session_id } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    let entry = sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?;
                    entry.driver.clone()
                };
                let driver = driver.lock().await;
                Ok(MeerkatMachineSessionCommandResult::ActiveInputs(
                    driver.as_driver().active_input_ids(),
                ))
            }
        }
    }

    async fn execute_meerkat_machine_drain_command(
        self: &Arc<Self>,
        command: MeerkatMachineDrainCommand,
    ) -> Result<MeerkatMachineDrainCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineDrainCommand::SetPeerIngressContext {
                session_id,
                keep_alive,
                comms_runtime,
            } => {
                // Guard: session must exist.
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                // Guard: DrainBindingInvariant — no drain mutation on destroyed
                // sessions.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                Ok(MeerkatMachineDrainCommandResult::Spawned(
                    self.update_peer_ingress_context_inner(&session_id, keep_alive, comms_runtime)
                        .await,
                ))
            }
            MeerkatMachineDrainCommand::NotifyDrainExited { session_id, reason } => {
                // Guard: session must exist.
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                // Guard: DrainBindingInvariant — no drain mutation on destroyed
                // sessions.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                self.notify_comms_drain_exited_inner(&session_id, reason)
                    .await;
                Ok(MeerkatMachineDrainCommandResult::Notified)
            }
        }
    }

    async fn execute_meerkat_machine_drain_local_command(
        &self,
        command: MeerkatMachineDrainLocalCommand,
    ) -> Result<(), RuntimeDriverError> {
        match command {
            MeerkatMachineDrainLocalCommand::AbortAll => {
                let mut slots = self.comms_drain_slots.write().await;
                for (_, slot) in slots.iter_mut() {
                    abort_slot(slot);
                }
                Ok(())
            }
            MeerkatMachineDrainLocalCommand::Abort { session_id } => {
                // Guard: session must be registered.
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                let mut slots = self.comms_drain_slots.write().await;
                if let Some(slot) = slots.get_mut(&session_id) {
                    abort_slot(slot);
                }
                Ok(())
            }
            MeerkatMachineDrainLocalCommand::Wait { session_id } => {
                // Guard: session must be registered.
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                let handle = {
                    let mut slots = self.comms_drain_slots.write().await;
                    slots
                        .get_mut(&session_id)
                        .and_then(|slot| slot.handle.take())
                };
                if let Some(handle) = handle {
                    let _ = handle.await;
                }
                let mut slots = self.comms_drain_slots.write().await;
                if let Some(slot) = slots.get_mut(&session_id)
                    && slot.authority.phase()
                        == meerkat_core::comms_drain_lifecycle_authority::CommsDrainPhase::Running
                {
                    tracing::warn!(
                        "comms_drain: task exited without notifying authority (likely panicked), \
                         submitting Failed safety net"
                    );
                    match protocol_comms_drain_spawn::notify_task_exited(
                        &mut slot.authority,
                        DrainExitReason::Failed,
                    ) {
                        Ok(effects) => {
                            apply_runtime_drain_effects(slot, &effects);
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "comms drain authority rejected safety-net TaskExited"
                            );
                        }
                    }
                }
                Ok(())
            }
        }
    }

    async fn execute_meerkat_machine_control_command(
        &self,
        command: MeerkatMachineControlCommand,
    ) -> Result<MeerkatMachineControlCommandResult, RuntimeControlPlaneError> {
        match command {
            MeerkatMachineControlCommand::Ingest { runtime_id, input } => {
                let (_session_id, driver, _completions, wake_tx, control_tx) = {
                    let (sid, d, c, w) = self.lookup_entry(&runtime_id).await?;
                    let ctrl = {
                        let sessions = self.sessions.read().await;
                        sessions
                            .get(&sid)
                            .and_then(RuntimeSessionEntry::control_sender)
                    };
                    (sid, d, c, w, ctrl)
                };

                // Guard: AdmitQueuedInput requires phase not in
                // {Retired, Stopped, Destroyed}. Steered inputs are more
                // permissive but the dispatch cannot distinguish here, so we
                // reject all three conservatively.
                let state = Self::driver_runtime_state(&driver).await;
                if matches!(
                    state,
                    RuntimeState::Retired | RuntimeState::Stopped | RuntimeState::Destroyed
                ) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }

                let (outcome, signal) = {
                    let mut drv = driver.lock().await;
                    let result = drv
                        .as_driver_mut()
                        .accept_input(input)
                        .await
                        .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                    let signal = drv.take_post_admission_signal();
                    (result, signal)
                };

                if signal.should_wake()
                    && let Some(ref tx) = wake_tx
                {
                    let _ = tx.try_send(());
                }
                if signal.should_interrupt_yielding()
                    && let Some(ref tx) = control_tx
                {
                    let _ = tx.try_send(
                        meerkat_core::lifecycle::run_control::RunControlCommand::InterruptYielding,
                    );
                }

                Ok(MeerkatMachineControlCommandResult::AcceptOutcome(outcome))
            }
            MeerkatMachineControlCommand::PublishEvent { event } => {
                let runtime_id = event.runtime_id.clone();
                let (_session_id, driver, _completions, _wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                if matches!(
                    Self::driver_runtime_state(&driver).await,
                    RuntimeState::Destroyed
                ) {
                    return Err(RuntimeControlPlaneError::InvalidState {
                        state: RuntimeState::Destroyed,
                    });
                }

                let mut drv = driver.lock().await;
                drv.as_driver_mut()
                    .on_runtime_event(event)
                    .await
                    .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                Ok(MeerkatMachineControlCommandResult::Unit)
            }
            MeerkatMachineControlCommand::Retire { runtime_id } => {
                let (session_id, driver, completions, wake_tx) =
                    self.lookup_entry(&runtime_id).await?;
                let _ = session_id;

                let state = Self::driver_runtime_state(&driver).await;
                if matches!(state, RuntimeState::Destroyed | RuntimeState::Stopped) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }

                let mut drv = driver.lock().await;
                let mut report = drv
                    .as_driver_mut()
                    .retire()
                    .await
                    .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                drop(drv);

                if report.inputs_pending_drain > 0 {
                    if let Some(ref tx) = wake_tx
                        && tx.send(()).await.is_ok()
                    {
                        return Ok(MeerkatMachineControlCommandResult::RetireReport(report));
                    }

                    let mut drv = driver.lock().await;
                    let abandoned = drv
                        .abandon_pending_inputs(crate::input_state::InputAbandonReason::Retired)
                        .await
                        .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                    drop(drv);
                    let mut comp = completions.lock().await;
                    comp.resolve_all_terminated("retired without runtime loop");
                    report.inputs_abandoned += abandoned;
                    report.inputs_pending_drain = 0;
                }

                Ok(MeerkatMachineControlCommandResult::RetireReport(report))
            }
            MeerkatMachineControlCommand::Recycle { runtime_id } => {
                let (_session_id, driver, completions, wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                let state = Self::driver_runtime_state(&driver).await;
                if matches!(state, RuntimeState::Destroyed | RuntimeState::Running) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }

                let (transferred, active_after_recycle) = {
                    let mut drv = driver.lock().await;
                    let should_restore_attached = matches!(state, RuntimeState::Attached);

                    let transferred =
                        match &mut *drv {
                            DriverEntry::Ephemeral(driver) => driver
                                .recycle_preserving_work()
                                .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?,
                            DriverEntry::Persistent(driver) => driver
                                .recycle_preserving_work()
                                .await
                                .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?,
                        };

                    if should_restore_attached
                        && matches!(drv.as_driver().runtime_state(), RuntimeState::Idle)
                    {
                        drv.attach()
                            .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                    }

                    let active_after_recycle = drv.as_driver().active_input_ids();
                    (transferred, active_after_recycle)
                };

                {
                    let pending_after: HashSet<InputId> =
                        active_after_recycle.into_iter().collect();
                    let mut comp = completions.lock().await;
                    comp.resolve_not_pending(
                        |input_id| pending_after.contains(input_id),
                        "recycled input no longer pending",
                    );
                }

                if let Some(ref tx) = wake_tx {
                    let _ = tx.try_send(());
                }

                Ok(MeerkatMachineControlCommandResult::RecycleReport(
                    RecycleReport {
                        inputs_transferred: transferred,
                    },
                ))
            }
            MeerkatMachineControlCommand::Reset { runtime_id } => {
                let (_session_id, driver, completions, _wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                let state = Self::driver_runtime_state(&driver).await;
                if matches!(state, RuntimeState::Destroyed | RuntimeState::Running) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }

                let mut drv = driver.lock().await;
                let report = drv
                    .as_driver_mut()
                    .reset()
                    .await
                    .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                drop(drv);

                let mut comp = completions.lock().await;
                comp.resolve_all_terminated("runtime reset");

                Ok(MeerkatMachineControlCommandResult::ResetReport(report))
            }
            MeerkatMachineControlCommand::Recover { runtime_id } => {
                let (_session_id, driver, completions, wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                let state = Self::driver_runtime_state(&driver).await;
                if matches!(state, RuntimeState::Destroyed | RuntimeState::Running) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }

                let (report, active_after_recover) = {
                    let mut drv = driver.lock().await;
                    let report = drv
                        .as_driver_mut()
                        .recover()
                        .await
                        .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                    let active_after_recover = drv.as_driver().active_input_ids();
                    (report, active_after_recover)
                };

                {
                    let pending_after: HashSet<InputId> =
                        active_after_recover.into_iter().collect();
                    let mut comp = completions.lock().await;
                    comp.resolve_not_pending(
                        |input_id| pending_after.contains(input_id),
                        "recovered input no longer pending",
                    );
                }

                if let Some(ref tx) = wake_tx {
                    let _ = tx.try_send(());
                }

                Ok(MeerkatMachineControlCommandResult::RecoveryReport(report))
            }
            MeerkatMachineControlCommand::Destroy { runtime_id } => {
                let (_session_id, driver, completions, _wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                if matches!(
                    Self::driver_runtime_state(&driver).await,
                    RuntimeState::Destroyed
                ) {
                    return Err(RuntimeControlPlaneError::InvalidState {
                        state: RuntimeState::Destroyed,
                    });
                }

                let mut drv = driver.lock().await;
                let report = drv
                    .as_driver_mut()
                    .destroy()
                    .await
                    .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                drop(drv);

                let mut comp = completions.lock().await;
                comp.resolve_all_terminated("runtime destroyed");

                Ok(MeerkatMachineControlCommandResult::DestroyReport(report))
            }
            MeerkatMachineControlCommand::RuntimeState { runtime_id } => {
                let (_session_id, driver, _completions, _wake_tx) =
                    self.lookup_entry(&runtime_id).await?;
                let drv = driver.lock().await;
                Ok(MeerkatMachineControlCommandResult::RuntimeState(
                    drv.as_driver().runtime_state(),
                ))
            }
            MeerkatMachineControlCommand::LoadBoundaryReceipt {
                runtime_id,
                run_id,
                sequence,
            } => {
                let receipt = match &self.store {
                    Some(store) => store
                        .load_boundary_receipt(&runtime_id, &run_id, sequence)
                        .await
                        .map_err(|e| RuntimeControlPlaneError::StoreError(e.to_string()))?,
                    None => None,
                };
                Ok(MeerkatMachineControlCommandResult::BoundaryReceipt(receipt))
            }
        }
    }

    async fn execute_meerkat_machine_ingress_command(
        &self,
        command: MeerkatMachineIngressCommand,
    ) -> Result<MeerkatMachineIngressCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineIngressCommand::AcceptWithCompletion { session_id, input } => {
                let (driver, completions, wake_tx, control_tx) = {
                    let sessions = self.sessions.read().await;
                    let entry = sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?;
                    (
                        entry.driver.clone(),
                        entry.completions.clone(),
                        entry.wake_sender(),
                        entry.control_sender(),
                    )
                };

                // Guard: WaitingInputsInvariant — no input admission on
                // Retired, Stopped, or Destroyed sessions.
                let state = Self::driver_runtime_state(&driver).await;
                if matches!(
                    state,
                    RuntimeState::Retired | RuntimeState::Stopped | RuntimeState::Destroyed
                ) {
                    return Err(if state == RuntimeState::Destroyed {
                        RuntimeDriverError::Destroyed
                    } else {
                        RuntimeDriverError::NotReady { state }
                    });
                }

                let (outcome, signal, handle) = {
                    let mut driver = driver.lock().await;
                    let result = driver.as_driver_mut().accept_input(input).await?;

                    match &result {
                        AcceptOutcome::Accepted { input_id, .. } => {
                            let is_terminal = driver
                                .as_driver()
                                .input_state(input_id)
                                .map(|state| state.current_state().is_terminal())
                                .unwrap_or(true);
                            let handle = if is_terminal {
                                None
                            } else {
                                Some({
                                    let mut completions = completions.lock().await;
                                    completions.register(input_id.clone())
                                })
                            };
                            let signal = driver.take_post_admission_signal();
                            (result, signal, handle)
                        }
                        AcceptOutcome::Deduplicated { existing_id, .. } => {
                            let existing_state = driver.as_driver().input_state(existing_id);
                            let is_terminal = existing_state
                                .map(|s| s.current_state().is_terminal())
                                .unwrap_or(true);

                            if is_terminal {
                                (
                                    result,
                                    crate::driver::ephemeral::PostAdmissionSignal::None,
                                    None,
                                )
                            } else {
                                let handle = {
                                    let mut completions = completions.lock().await;
                                    completions.register(existing_id.clone())
                                };
                                (
                                    result,
                                    crate::driver::ephemeral::PostAdmissionSignal::None,
                                    Some(handle),
                                )
                            }
                        }
                        AcceptOutcome::Rejected { reason } => {
                            return Err(RuntimeDriverError::ValidationFailed {
                                reason: reason.to_string(),
                            });
                        }
                    }
                };

                if signal.should_wake()
                    && let Some(ref wake_tx) = wake_tx
                {
                    let _ = wake_tx.try_send(());
                }
                if signal.should_interrupt_yielding()
                    && let Some(ref tx) = control_tx
                {
                    let _ = tx.try_send(
                        meerkat_core::lifecycle::run_control::RunControlCommand::InterruptYielding,
                    );
                }

                Ok(MeerkatMachineIngressCommandResult::AcceptWithCompletion { outcome, handle })
            }
            MeerkatMachineIngressCommand::AcceptWithoutWake { session_id, input } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    let entry = sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?;
                    entry.driver.clone()
                };

                // Guard: WaitingInputsInvariant — no input admission on
                // Retired, Stopped, or Destroyed sessions.
                let state = Self::driver_runtime_state(&driver).await;
                if matches!(
                    state,
                    RuntimeState::Retired | RuntimeState::Stopped | RuntimeState::Destroyed
                ) {
                    return Err(if state == RuntimeState::Destroyed {
                        RuntimeDriverError::Destroyed
                    } else {
                        RuntimeDriverError::NotReady { state }
                    });
                }

                let outcome = {
                    let mut driver = driver.lock().await;
                    let result = driver.as_driver_mut().accept_input(input).await?;
                    let signal = driver.take_post_admission_signal();
                    debug_assert!(
                        !signal.should_process_immediately(),
                        "queue-only admission unexpectedly requested immediate processing"
                    );
                    result
                };

                Ok(MeerkatMachineIngressCommandResult::AcceptOutcome(outcome))
            }
        }
    }

    async fn execute_meerkat_machine_legacy_run_command(
        &self,
        command: MeerkatMachineLegacyRunCommand,
    ) -> Result<MeerkatMachineLegacyRunCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineLegacyRunCommand::Prepare { session_id, input } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?
                        .driver
                        .clone()
                };

                let prepared = {
                    let mut driver = driver.lock().await;
                    if !driver.is_idle_or_attached() {
                        return Err(RuntimeDriverError::NotReady {
                            state: driver.as_driver().runtime_state(),
                        });
                    }

                    let active_input_ids = driver.as_driver().active_input_ids();
                    if !active_input_ids.is_empty() {
                        let duplicate_active_input =
                            input.header().idempotency_key.as_ref().and_then(|key| {
                                active_input_ids.iter().find(|active_id| {
                                    driver
                                        .as_driver()
                                        .input_state(active_id)
                                        .and_then(|state| state.idempotency_key.as_ref())
                                        == Some(key)
                                })
                            });
                        if let Some(existing_id) = duplicate_active_input {
                            return Err(RuntimeDriverError::ValidationFailed {
                                reason: format!(
                                    "accept_input_and_run does not support deduplicated admission; existing input {existing_id} already owns execution"
                                ),
                            });
                        }
                        return Err(RuntimeDriverError::NotReady {
                            state: driver.as_driver().runtime_state(),
                        });
                    }

                    let outcome = driver.as_driver_mut().accept_input(input).await?;
                    let input_id = match outcome {
                        AcceptOutcome::Accepted { input_id, .. } => input_id,
                        AcceptOutcome::Deduplicated { existing_id, .. } => {
                            return Err(RuntimeDriverError::ValidationFailed {
                                reason: format!(
                                    "accept_input_and_run does not support deduplicated admission; existing input {existing_id} already owns execution"
                                ),
                            });
                        }
                        AcceptOutcome::Rejected { reason } => {
                            return Err(RuntimeDriverError::ValidationFailed {
                                reason: reason.to_string(),
                            });
                        }
                    };

                    if !driver.is_idle_or_attached() {
                        return Err(RuntimeDriverError::NotReady {
                            state: driver.as_driver().runtime_state(),
                        });
                    }

                    let (dequeued_id, dequeued_input) = driver.dequeue_next().ok_or_else(|| {
                        RuntimeDriverError::Internal(
                            "accepted input was not queued for execution".into(),
                        )
                    })?;
                    if dequeued_id != input_id {
                        return Err(RuntimeDriverError::NotReady {
                            state: driver.as_driver().runtime_state(),
                        });
                    }

                    let run_id = RunId::new();
                    driver.start_run(run_id.clone()).map_err(|err| {
                        RuntimeDriverError::Internal(format!("failed to start runtime run: {err}"))
                    })?;
                    driver.stage_input(&dequeued_id, &run_id).map_err(|err| {
                        RuntimeDriverError::Internal(format!(
                            "failed to stage accepted input: {err}"
                        ))
                    })?;

                    MeerkatMachineLegacyRunPrepared {
                        input_id,
                        run_id,
                        primitive: crate::runtime_loop::input_to_primitive(
                            &dequeued_input,
                            dequeued_id,
                        ),
                    }
                };

                Ok(MeerkatMachineLegacyRunCommandResult::Prepared(prepared))
            }
            MeerkatMachineLegacyRunCommand::Commit {
                session_id,
                input_id,
                run_id,
                output,
            } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?
                        .driver
                        .clone()
                };

                let mut driver = driver.lock().await;
                if let Err(err) = driver
                    .as_driver_mut()
                    .on_run_event(meerkat_core::lifecycle::RunEvent::BoundaryApplied {
                        run_id: run_id.clone(),
                        receipt: output.receipt,
                        session_snapshot: output.session_snapshot,
                    })
                    .await
                {
                    if let Err(unwind_err) = driver
                        .as_driver_mut()
                        .on_run_event(meerkat_core::lifecycle::RunEvent::RunFailed {
                            run_id,
                            error: format!("boundary commit failed: {err}"),
                            recoverable: true,
                        })
                        .await
                    {
                        return Err(RuntimeDriverError::Internal(format!(
                            "runtime boundary commit failed: {err}; additionally failed to unwind runtime state: {unwind_err}"
                        )));
                    }
                    return Err(RuntimeDriverError::Internal(format!(
                        "runtime boundary commit failed: {err}"
                    )));
                }
                if let Err(err) = driver
                    .as_driver_mut()
                    .on_run_event(meerkat_core::lifecycle::RunEvent::RunCompleted {
                        run_id,
                        consumed_input_ids: vec![input_id],
                    })
                    .await
                {
                    drop(driver);
                    self.unregister_session(&session_id).await;
                    return Err(RuntimeDriverError::Internal(format!(
                        "failed to persist runtime completion snapshot: {err}"
                    )));
                }

                Ok(MeerkatMachineLegacyRunCommandResult::Unit)
            }
            MeerkatMachineLegacyRunCommand::Fail {
                session_id,
                run_id,
                error,
            } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?
                        .driver
                        .clone()
                };

                let mut driver = driver.lock().await;
                if let Err(run_err) = driver
                    .as_driver_mut()
                    .on_run_event(meerkat_core::lifecycle::RunEvent::RunFailed {
                        run_id,
                        error,
                        recoverable: true,
                    })
                    .await
                {
                    drop(driver);
                    self.unregister_session(&session_id).await;
                    return Err(RuntimeDriverError::Internal(format!(
                        "failed to persist runtime failure snapshot: {run_err}"
                    )));
                }

                Ok(MeerkatMachineLegacyRunCommandResult::Unit)
            }
        }
    }

    /// Register a runtime driver for a session (no RuntimeLoop — inputs queue but
    /// nothing processes them automatically). Useful for tests and legacy mode.
    pub async fn register_session(&self, session_id: SessionId) {
        let _ = self
            .execute_meerkat_machine_session_command(
                MeerkatMachineSessionCommand::RegisterSession { session_id },
            )
            .await;
    }

    async fn register_session_inner(&self, session_id: SessionId) {
        {
            let mut sessions = self.sessions.write().await;
            if let Some(existing) = sessions.get_mut(&session_id) {
                existing.clear_dead_attachment();
                return;
            }
        }

        let mut entry = self.make_driver(&session_id);
        if let Err(err) = entry.as_driver_mut().recover().await {
            tracing::error!(%session_id, error = %err, "failed to recover runtime driver during registration");
            return;
        }

        let (ops_lifecycle, epoch_id, cursor_state) =
            self.recover_or_create_ops_state(&session_id).await;

        let session_entry = RuntimeSessionEntry {
            driver: Arc::new(Mutex::new(entry)),
            ops_lifecycle,
            epoch_id,
            cursor_state,
            completions: Arc::new(Mutex::new(crate::completion::CompletionRegistry::new())),
            phase: RegistrationPhase::Queuing,
            detached_wake: None,
        };
        let mut sessions = self.sessions.write().await;
        if let Some(existing) = sessions.get_mut(&session_id) {
            existing.clear_dead_attachment();
        } else {
            sessions.insert(session_id, session_entry);
        }
    }

    /// Set the silent comms intents for a session's runtime driver.
    ///
    /// Peer requests whose intent matches one of these strings will be accepted
    /// without triggering an LLM turn (ApplyMode::Ignore, WakeMode::None).
    pub async fn set_session_silent_intents(&self, session_id: &SessionId, intents: Vec<String>) {
        let _ = self
            .execute_meerkat_machine_session_command(
                MeerkatMachineSessionCommand::SetSilentIntents {
                    session_id: session_id.clone(),
                    intents,
                },
            )
            .await;
    }

    async fn set_session_silent_intents_inner(&self, session_id: &SessionId, intents: Vec<String>) {
        let sessions = self.sessions.read().await;
        if let Some(entry) = sessions.get(session_id) {
            let mut driver = entry.driver.lock().await;
            driver.set_silent_comms_intents(intents);
        }
    }

    /// Register a runtime driver for a session WITH a RuntimeLoop backed by a
    /// `CoreExecutor`. When `accept_input()` queues an input and requests wake,
    /// the loop dequeues it and calls `executor.apply()` (which triggers
    /// `SessionService::start_turn()`).
    pub async fn register_session_with_executor(
        &self,
        session_id: SessionId,
        executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    ) {
        let _ = self
            .execute_meerkat_machine_session_command(
                MeerkatMachineSessionCommand::EnsureSessionWithExecutor {
                    session_id,
                    executor,
                },
            )
            .await;
    }

    /// Ensure a runtime driver with executor exists for the session.
    ///
    /// If a session was already registered without a loop, upgrade the
    /// existing driver in place so queued inputs remain attached to the same
    /// runtime ledger and can start draining immediately.
    pub async fn ensure_session_with_executor(
        &self,
        session_id: SessionId,
        executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    ) {
        let _ = self
            .execute_meerkat_machine_session_command(
                MeerkatMachineSessionCommand::EnsureSessionWithExecutor {
                    session_id,
                    executor,
                },
            )
            .await;
    }

    async fn ensure_session_with_executor_inner(
        &self,
        session_id: SessionId,
        executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    ) {
        let existing = {
            let mut sessions = self.sessions.write().await;
            sessions.get_mut(&session_id).map(|entry| {
                entry.clear_dead_attachment();
                let occupied = entry.has_attachment_or_attaching();
                if !occupied {
                    // Claim the attachment slot so concurrent callers see
                    // Attaching and return early instead of racing a second
                    // loop spawn (which would cross-wire detached-wake state).
                    entry.phase = RegistrationPhase::Attaching;
                }
                (
                    occupied,
                    entry.driver.clone(),
                    entry.completions.clone(),
                    entry.ops_lifecycle.clone(),
                )
            })
        };

        let (driver, completions, ops_lifecycle) =
            if let Some((has_attachment, driver, completions, ops_lifecycle)) = existing {
                if has_attachment {
                    return;
                }
                (driver, completions, ops_lifecycle)
            } else {
                let mut recovered_entry = self.make_driver(&session_id);
                if let Err(err) = recovered_entry.as_driver_mut().recover().await {
                    tracing::error!(
                        %session_id,
                        error = %err,
                        "failed to recover runtime driver during registration"
                    );
                    return;
                }

                // Recover ops state OUTSIDE the sessions lock to avoid blocking
                // other adapter operations behind potentially slow disk I/O.
                let (recovered_ops, recovered_epoch, recovered_cursors) =
                    self.recover_or_create_ops_state(&session_id).await;

                // Double-check under the lock — another task may have inserted
                // the entry while we were recovering.
                let mut sessions = self.sessions.write().await;
                if let Some(entry) = sessions.get_mut(&session_id) {
                    entry.clear_dead_attachment();
                    if entry.has_attachment_or_attaching() {
                        return;
                    }
                    entry.phase = RegistrationPhase::Attaching;
                    (
                        entry.driver.clone(),
                        entry.completions.clone(),
                        entry.ops_lifecycle.clone(),
                    )
                } else {
                    let driver = Arc::new(Mutex::new(recovered_entry));
                    let completions =
                        Arc::new(Mutex::new(crate::completion::CompletionRegistry::new()));
                    sessions.insert(
                        session_id.clone(),
                        RuntimeSessionEntry {
                            driver: driver.clone(),
                            ops_lifecycle: recovered_ops.clone(),
                            epoch_id: recovered_epoch,
                            cursor_state: recovered_cursors,
                            completions: completions.clone(),
                            phase: RegistrationPhase::Queuing,
                            detached_wake: None,
                        },
                    );
                    (driver, completions, recovered_ops)
                }
            };

        let should_wake = {
            let mut driver_guard = driver.lock().await;
            if let Err(error) = driver_guard.attach() {
                let repaired = if error.from == RuntimeState::Attached
                    && error.to == RuntimeState::Attached
                {
                    tracing::warn!(
                        %session_id,
                        error = %error,
                        "runtime driver remained attached without a live published loop; detaching and retrying attachment"
                    );
                    match driver_guard.detach() {
                        Ok(_) => match driver_guard.attach() {
                            Ok(()) => true,
                            Err(retry_error) => {
                                tracing::warn!(
                                    %session_id,
                                    error = %retry_error,
                                    "failed to re-attach runtime driver after repairing stale attachment state"
                                );
                                false
                            }
                        },
                        Err(detach_error) => {
                            tracing::warn!(
                                %session_id,
                                error = %detach_error,
                                "failed to detach stale attached runtime driver before retrying attachment"
                            );
                            false
                        }
                    }
                } else {
                    false
                };
                if !repaired {
                    tracing::warn!(
                        %session_id,
                        error = %error,
                        "failed to attach runtime driver before publishing loop attachment"
                    );
                    self.revert_attaching(&session_id).await;
                    return;
                }
            }
            !driver_guard.as_driver().active_input_ids().is_empty()
        };

        // Wire detached-op wake state: the ops lifecycle registry will set
        // `pending = true` and fire `notify` when a BackgroundToolOp reaches
        // terminal. The waker task (spawned after attachment below) then injects
        // a continuation through the canonical ingress seam.
        let detached_wake_state = Arc::new(crate::detached_wake::DetachedWakeState::new());
        ops_lifecycle.set_detached_wake(Arc::clone(&detached_wake_state));

        // Wire persistence channel if a durable store is available.
        if let Some(ref store) = self.store {
            let (persist_tx, mut persist_rx) =
                crate::tokio::sync::mpsc::channel::<crate::ops_lifecycle::PersistedOpsSnapshot>(16);
            let entry_epoch_id = {
                let sessions = self.sessions.read().await;
                sessions
                    .get(&session_id)
                    .map(|e| e.epoch_id.clone())
                    .unwrap_or_else(meerkat_core::RuntimeEpochId::new)
            };
            let entry_cursor = {
                let sessions = self.sessions.read().await;
                sessions
                    .get(&session_id)
                    .map(|e| Arc::clone(&e.cursor_state))
                    .unwrap_or_else(|| Arc::new(meerkat_core::EpochCursorState::new()))
            };
            ops_lifecycle.set_persistence_channel(persist_tx, entry_epoch_id, entry_cursor);

            // Spawn persistence task
            let store_clone = Arc::clone(store);
            let runtime_id = crate::identifiers::LogicalRuntimeId::new(session_id.to_string());
            crate::tokio::spawn(async move {
                while let Some(snapshot) = persist_rx.recv().await {
                    if let Err(e) = store_clone
                        .persist_ops_lifecycle(&runtime_id, &snapshot)
                        .await
                    {
                        tracing::warn!(
                            error = %e,
                            "failed to persist ops lifecycle snapshot"
                        );
                    }
                }
            });
        }

        // Get the completion feed from the registry for feed-based idle wake.
        let completion_feed = ops_lifecycle.completion_feed_handle();

        let (wake_tx, wake_rx) = mpsc::channel(16);
        let (control_tx, control_rx) = mpsc::channel(16);
        let entry_cursor_state = {
            let sessions = self.sessions.read().await;
            sessions
                .get(&session_id)
                .map(|e| Arc::clone(&e.cursor_state))
        };
        let mut pending_loop_handle =
            Some(crate::runtime_loop::spawn_runtime_loop_with_completions(
                driver.clone(),
                executor,
                wake_rx,
                control_rx,
                Some(completions.clone()),
                Some(Arc::clone(&detached_wake_state)),
                Some(completion_feed),
                entry_cursor_state,
            ));

        let (published, detach_after_abort) = {
            let mut sessions = self.sessions.write().await;
            match sessions.get_mut(&session_id) {
                None => (false, true),
                Some(entry) => {
                    entry.clear_dead_attachment();
                    if entry.has_live_attachment() {
                        (false, false)
                    } else if !Arc::ptr_eq(&entry.driver, &driver)
                        || !Arc::ptr_eq(&entry.completions, &completions)
                    {
                        tracing::warn!(
                            %session_id,
                            "runtime session entry changed while wiring executor; aborting stale loop attachment"
                        );
                        (false, true)
                    } else {
                        match pending_loop_handle.take() {
                            Some(loop_handle) => {
                                entry.attach_runtime_loop(wake_tx.clone(), control_tx, loop_handle);
                                entry.detached_wake = Some(Arc::clone(&detached_wake_state));
                                (true, false)
                            }
                            None => {
                                tracing::error!(
                                    %session_id,
                                    "runtime loop handle missing during attachment publish"
                                );
                                (false, true)
                            }
                        }
                    }
                }
            }
        };

        if !published {
            if let Some(loop_handle) = pending_loop_handle.take() {
                loop_handle.abort();
            }
            if detach_after_abort {
                let mut driver_guard = driver.lock().await;
                let _ = driver_guard.detach();
            }
            self.revert_attaching(&session_id).await;
            return;
        }

        if should_wake {
            let _ = wake_tx.try_send(());
        }
    }

    /// Revert `Attaching → Queuing` if attachment failed. This unblocks
    /// future `ensure_session_with_executor` callers that would otherwise
    /// see `Attaching` forever and return early.
    async fn revert_attaching(&self, session_id: &SessionId) {
        let mut sessions = self.sessions.write().await;
        if let Some(entry) = sessions.get_mut(session_id)
            && matches!(entry.phase, RegistrationPhase::Attaching)
        {
            entry.phase = RegistrationPhase::Queuing;
        }
    }

    /// Unregister a session's runtime driver.
    ///
    /// Detaches the executor (Attached → Idle) before removal, then drops
    /// the wake channel sender, which causes the RuntimeLoop to exit.
    pub async fn unregister_session(&self, session_id: &SessionId) {
        let _ = self
            .execute_meerkat_machine_session_command(
                MeerkatMachineSessionCommand::UnregisterSession {
                    session_id: session_id.clone(),
                },
            )
            .await;
    }

    async fn unregister_session_inner(&self, session_id: &SessionId) {
        let entry = {
            let mut sessions = self.sessions.write().await;
            let mut slots = self.comms_drain_slots.write().await;
            // Remove + abort drain slot before dropping session binding so
            // slot keys remain a subset of registered-session keys.
            if let Some(mut slot) = slots.remove(session_id) {
                abort_slot(&mut slot);
            }
            sessions.remove(session_id)
        };

        if let Some(entry) = entry {
            let mut driver = entry.driver.lock().await;
            let _ = driver.detach(); // Attached → Idle (no-op if not Attached)
            drop(driver);

            let mut completions = entry.completions.lock().await;
            completions.resolve_all_terminated("runtime session unregistered");
        }
    }

    /// Check whether a runtime driver is already registered for a session.
    pub async fn contains_session(&self, session_id: &SessionId) -> bool {
        match self
            .execute_meerkat_machine_session_command(
                MeerkatMachineSessionCommand::ContainsSession {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineSessionCommandResult::Bool(present)) => present,
            Ok(_) => unreachable!("contains_session returned wrong result"),
            Err(_) => false,
        }
    }

    /// Check whether a session has an active RuntimeLoop or attachment in
    /// progress. Returns `false` only for `Queuing` sessions (registered via
    /// `prepare_bindings()` with no executor) and unknown sessions.
    pub async fn session_has_executor(&self, session_id: &SessionId) -> bool {
        match self
            .execute_meerkat_machine_session_command(
                MeerkatMachineSessionCommand::SessionHasExecutor {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineSessionCommandResult::Bool(present)) => present,
            Ok(_) => unreachable!("session_has_executor returned wrong result"),
            Err(_) => false,
        }
    }

    /// Check whether a session already has a comms runtime configured.
    ///
    /// Returns `true` if `update_peer_ingress_context` was previously called
    /// with a non-None comms runtime for this session (e.g., via
    /// `SessionRuntime::enable_comms_drain`).
    pub async fn session_has_comms(&self, session_id: &SessionId) -> bool {
        match self
            .execute_meerkat_machine_session_command(
                MeerkatMachineSessionCommand::SessionHasComms {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineSessionCommandResult::Bool(present)) => present,
            Ok(_) => unreachable!("session_has_comms returned wrong result"),
            Err(_) => false,
        }
    }

    /// Cancel the currently-running turn for a registered session.
    pub async fn interrupt_current_run(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        self.execute_meerkat_machine_session_command(
            MeerkatMachineSessionCommand::InterruptCurrentRun {
                session_id: session_id.clone(),
            },
        )
        .await
        .map(|_| ())
    }

    /// Request cancellation at the next safe boundary for the currently-running turn.
    pub async fn cancel_after_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        self.execute_meerkat_machine_session_command(
            MeerkatMachineSessionCommand::CancelAfterBoundary {
                session_id: session_id.clone(),
            },
        )
        .await
        .map(|_| ())
    }

    async fn interrupt_current_run_inner(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        let (driver, control_tx) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            (entry.driver.clone(), entry.control_sender())
        };

        let Some(control_tx) = control_tx else {
            let state = {
                let driver = driver.lock().await;
                driver.as_driver().runtime_state()
            };
            return Err(RuntimeDriverError::NotReady { state });
        };
        control_tx
            .send(RunControlCommand::CancelCurrentRun {
                reason: "mob interrupt".to_string(),
            })
            .await
            .map_err(|err| RuntimeDriverError::Internal(format!("failed to send interrupt: {err}")))
    }

    async fn cancel_after_boundary_inner(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        let (driver, control_tx) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            (entry.driver.clone(), entry.control_sender())
        };

        let Some(control_tx) = control_tx else {
            let state = {
                let driver = driver.lock().await;
                driver.as_driver().runtime_state()
            };
            return Err(RuntimeDriverError::NotReady { state });
        };
        control_tx
            .send(RunControlCommand::CancelAfterBoundary {
                reason: "boundary cancel".to_string(),
            })
            .await
            .map_err(|err| {
                RuntimeDriverError::Internal(format!("failed to send cancel_after_boundary: {err}"))
            })
    }

    /// Stop the attached runtime executor through the out-of-band control
    /// channel. When no loop is attached yet, a stop command is applied directly
    /// against the driver so queued work is still terminated consistently.
    pub async fn stop_runtime_executor(
        &self,
        session_id: &SessionId,
        command: RunControlCommand,
    ) -> Result<(), RuntimeDriverError> {
        self.execute_meerkat_machine_session_command(
            MeerkatMachineSessionCommand::StopRuntimeExecutor {
                session_id: session_id.clone(),
                command,
            },
        )
        .await
        .map(|_| ())
    }

    async fn stop_runtime_executor_inner(
        &self,
        session_id: &SessionId,
        command: RunControlCommand,
    ) -> Result<(), RuntimeDriverError> {
        let (driver, completions, control_tx) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            (
                entry.driver.clone(),
                entry.completions.clone(),
                entry.control_sender(),
            )
        };

        if let Some(control_tx) = control_tx
            && control_tx.send(command.clone()).await.is_ok()
        {
            return Ok(());
        }

        if matches!(command, RunControlCommand::StopRuntimeExecutor { .. }) {
            let mut driver = driver.lock().await;
            driver
                .as_driver_mut()
                .on_runtime_control(crate::traits::RuntimeControlCommand::Stop)
                .await?;
            drop(driver);
            let mut completions = completions.lock().await;
            completions.resolve_all_terminated("runtime stopped");
            drop(completions);

            // No live control sender was available for this stop path. Scrub any
            // dead attachment capabilities that may still be published.
            let mut sessions = self.sessions.write().await;
            if let Some(entry) = sessions.get_mut(session_id) {
                entry.clear_dead_attachment();
            }
            Ok(())
        } else {
            Err(RuntimeDriverError::Internal(
                "failed to send stop: runtime loop is unavailable".into(),
            ))
        }
    }

    /// Accept an input and execute it synchronously through the runtime driver.
    ///
    /// This is useful for surfaces that need the legacy request/response shape
    /// while still preserving v9 input lifecycle semantics.
    pub async fn accept_input_and_run<T, F, Fut>(
        &self,
        session_id: &SessionId,
        input: Input,
        op: F,
    ) -> Result<T, RuntimeDriverError>
    where
        F: FnOnce(RunId, meerkat_core::lifecycle::run_primitive::RunPrimitive) -> Fut,
        Fut: Future<Output = Result<(T, CoreApplyOutput), RuntimeDriverError>>,
    {
        let MeerkatMachineLegacyRunPrepared {
            input_id,
            run_id,
            primitive,
        } = match self
            .execute_meerkat_machine_legacy_run_command(MeerkatMachineLegacyRunCommand::Prepare {
                session_id: session_id.clone(),
                input,
            })
            .await?
        {
            MeerkatMachineLegacyRunCommandResult::Prepared(prepared) => prepared,
            MeerkatMachineLegacyRunCommandResult::Unit => {
                return Err(RuntimeDriverError::Internal(
                    "unexpected unit result preparing legacy Meerkat run".into(),
                ));
            }
        };

        match op(run_id.clone(), primitive).await {
            Ok((result, output)) => {
                self.execute_meerkat_machine_legacy_run_command(
                    MeerkatMachineLegacyRunCommand::Commit {
                        session_id: session_id.clone(),
                        input_id,
                        run_id,
                        output,
                    },
                )
                .await?;
                Ok(result)
            }
            Err(err) => {
                self.execute_meerkat_machine_legacy_run_command(
                    MeerkatMachineLegacyRunCommand::Fail {
                        session_id: session_id.clone(),
                        run_id,
                        error: err.to_string(),
                    },
                )
                .await?;
                Err(err)
            }
        }
    }

    /// Accept an input and return a completion handle that resolves when the
    /// input reaches a terminal state (Consumed or Abandoned).
    ///
    /// Returns `(AcceptOutcome, Option<CompletionHandle>)`:
    /// - `(Accepted, Some(handle))` — await handle for result
    /// - `(Accepted, None)` — input reached a terminal state during admission
    /// - `(Deduplicated, Some(handle))` — joined in-flight waiter
    /// - `(Deduplicated, None)` — input already terminal; no waiter needed
    /// - `(Rejected, _)` — returned as `Err(ValidationFailed)`
    pub async fn accept_input_with_completion(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<(AcceptOutcome, Option<crate::completion::CompletionHandle>), RuntimeDriverError>
    {
        match self
            .execute_meerkat_machine_ingress_command(
                MeerkatMachineIngressCommand::AcceptWithCompletion {
                    session_id: session_id.clone(),
                    input,
                },
            )
            .await?
        {
            MeerkatMachineIngressCommandResult::AcceptWithCompletion { outcome, handle } => {
                Ok((outcome, handle))
            }
            MeerkatMachineIngressCommandResult::AcceptOutcome(_) => {
                Err(RuntimeDriverError::Internal(
                    "unexpected queue-only result for accept_input_with_completion".into(),
                ))
            }
        }
    }

    /// Accept an input but intentionally do not wake the runtime loop.
    ///
    /// This is reserved for explicitly queued-only surface contracts that
    /// stage work for the next turn boundary instead of waking an idle session
    /// immediately.
    pub async fn accept_input_without_wake(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_ingress_command(
                MeerkatMachineIngressCommand::AcceptWithoutWake {
                    session_id: session_id.clone(),
                    input,
                },
            )
            .await?
        {
            MeerkatMachineIngressCommandResult::AcceptOutcome(outcome) => Ok(outcome),
            MeerkatMachineIngressCommandResult::AcceptWithCompletion { .. } => {
                Err(RuntimeDriverError::Internal(
                    "unexpected completion result for accept_input_without_wake".into(),
                ))
            }
        }
    }

    /// Get the shared ops lifecycle registry for a session/runtime instance.
    pub async fn ops_lifecycle_registry(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>> {
        match self
            .execute_meerkat_machine_session_command(
                MeerkatMachineSessionCommand::OpsLifecycleRegistry {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineSessionCommandResult::OpsLifecycleRegistry(registry)) => registry,
            Ok(_) => unreachable!("ops_lifecycle_registry returned wrong result"),
            Err(_) => None,
        }
    }

    /// Prepare canonical runtime bindings for a session.
    ///
    /// This is the single canonical helper that replaces the hand-rolled
    /// `register_session()` + `ops_lifecycle_registry()` + manual threading
    /// dance. All runtime-backed surfaces should call this instead.
    ///
    /// The method is idempotent: if the session is already registered, it
    /// returns bindings from the existing entry. The epoch_id is stable
    /// across repeated calls for the same session.
    pub async fn prepare_bindings(
        &self,
        session_id: SessionId,
    ) -> Result<meerkat_core::SessionRuntimeBindings, RuntimeBindingsError> {
        match self
            .execute_meerkat_machine_session_command(
                MeerkatMachineSessionCommand::PrepareBindings {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineSessionCommandResult::Bindings(bindings)) => Ok(bindings),
            Ok(_) => unreachable!("prepare_bindings returned wrong result"),
            Err(_) => Err(RuntimeBindingsError::SessionNotFound(session_id)),
        }
    }

    /// Capture a diagnostic snapshot of the current Meerkat runtime spine.
    ///
    /// This is an internal scaffolding aid for the MeerkatMachine refactor. It
    /// intentionally reflects current runtime truth without changing behavior.
    #[allow(dead_code)]
    #[doc(hidden)]
    pub async fn meerkat_machine_spine_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Option<MeerkatMachineSpineSnapshot> {
        let (
            driver_handle,
            completions_handle,
            ops_lifecycle,
            cursor_state,
            completions_present,
            ops_registry_present,
            attachment_live,
            detached_wake_pending,
            detached_wake_signaled,
            epoch_id,
        ) = {
            let sessions = self.sessions.read().await;
            let entry = sessions.get(session_id)?;
            let (detached_wake_pending, detached_wake_signaled) = entry
                .detached_wake
                .as_ref()
                .map(|wake| {
                    (
                        wake.pending.load(Ordering::Acquire),
                        wake.signaled.load(Ordering::Acquire),
                    )
                })
                .map_or((None, None), |(pending, signaled)| {
                    (Some(pending), Some(signaled))
                });
            (
                Arc::clone(&entry.driver),
                Arc::clone(&entry.completions),
                Arc::clone(&entry.ops_lifecycle),
                Arc::clone(&entry.cursor_state),
                true,
                true,
                entry.attachment_is_live(),
                detached_wake_pending,
                detached_wake_signaled,
                entry.epoch_id.clone(),
            )
        };
        let completion_waiters = {
            let completions = completions_handle.lock().await;
            let snapshot = completions.diagnostic_snapshot();
            MeerkatCompletionWaitersSnapshot {
                input_count: snapshot.input_count,
                waiter_count: snapshot.waiter_count,
                waiting_inputs: snapshot
                    .waiting_inputs
                    .into_iter()
                    .map(|entry| MeerkatCompletionWaiterSnapshot {
                        input_id: entry.input_id,
                        waiter_count: entry.waiter_count,
                    })
                    .collect(),
            }
        };
        let drain = {
            let slots = self.comms_drain_slots.read().await;
            if let Some(slot) = slots.get(session_id) {
                MeerkatDrainSnapshot {
                    slot_present: true,
                    phase: Some(slot.authority.phase()),
                    mode: slot.authority.mode(),
                    handle_present: slot.handle.is_some(),
                }
            } else {
                MeerkatDrainSnapshot {
                    slot_present: false,
                    phase: None,
                    mode: None,
                    handle_present: false,
                }
            }
        };

        let driver = driver_handle.lock().await;
        let driver_kind = match &*driver {
            DriverEntry::Ephemeral(_) => MeerkatDriverKind::Ephemeral,
            DriverEntry::Persistent(_) => MeerkatDriverKind::Persistent,
        };
        let control = driver.control();
        let ingress = driver.ingress();

        let binding = MeerkatBindingSnapshot {
            session_id: session_id.clone(),
            runtime_id: driver.runtime_id().clone(),
            driver_kind,
            driver_present: true,
            completions_present,
            ops_registry_present,
            attachment_live,
            detached_wake_present: detached_wake_pending.is_some(),
            epoch_id,
            cursor_state: {
                let cursor_state = cursor_state.snapshot();
                MeerkatCursorSnapshot {
                    agent_applied_cursor: cursor_state.agent_applied_cursor,
                    runtime_observed_seq: cursor_state.runtime_observed_seq,
                    runtime_last_injected_seq: cursor_state.runtime_last_injected_seq,
                }
            },
        };

        let control = MeerkatControlSnapshot {
            phase: control.phase(),
            current_run_id: control.current_run_id().cloned(),
            pre_run_phase: control.pre_run_state(),
            wake_pending: control.wake_pending(),
            process_pending: control.process_pending(),
        };

        let admission_order = ingress
            .admission_order()
            .iter()
            .cloned()
            .map(|input_id| MeerkatAdmittedInputSnapshot {
                content_shape: ingress.content_shape(&input_id),
                request_id: ingress.request_id(&input_id),
                reservation_key: ingress.reservation_key(&input_id),
                handling_mode: ingress.handling_mode(&input_id),
                lifecycle: ingress.lifecycle_state(&input_id),
                terminal_outcome: ingress.terminal_outcome(&input_id).cloned(),
                last_run_id: ingress.last_run(&input_id).cloned(),
                last_boundary_sequence: ingress.last_boundary_sequence(&input_id),
                is_prompt: ingress.is_prompt(&input_id),
                input_id,
            })
            .collect();

        let inputs = MeerkatInputsSnapshot {
            ingress_phase: ingress.phase(),
            admission_order,
            queue: ingress.queue().to_vec(),
            steer_queue: ingress.steer_queue().to_vec(),
            current_run_id: ingress.current_run().cloned(),
            current_run_contributors: ingress.current_run_contributors().to_vec(),
            wake_requested: ingress.wake_requested(),
            process_requested: ingress.process_requested(),
            silent_intent_overrides: ingress.silent_intent_overrides().iter().cloned().collect(),
        };
        let ops_snapshot = ops_lifecycle.diagnostic_snapshot();
        let ops = MeerkatOpsSnapshot {
            operation_count: ops_snapshot.operation_count,
            active_count: ops_snapshot.active_count,
            wait_request_id: ops_snapshot.wait_request_id,
            pending_wait_present: ops_snapshot.pending_wait_present,
            pending_wait_request_id: ops_snapshot.pending_wait_request_id,
            wait_operation_ids: ops_snapshot.wait_operation_ids,
            operations: ops_snapshot.operations,
            detached_wake_pending,
            detached_wake_signaled,
        };

        Some(MeerkatMachineSpineSnapshot {
            binding,
            control,
            inputs,
            completion_waiters,
            ops,
            drain,
        })
    }

    /// Manage the comms drain lifecycle for a session based on keep_alive intent.
    ///
    /// When `keep_alive` is true, spawns a drain if one is not already running.
    /// When `keep_alive` is false, aborts any running drain for the session.
    /// Returns `true` if a new drain was spawned.
    ///
    /// All state transitions go through `CommsDrainLifecycleAuthority`.
    ///
    /// `update_peer_ingress_context(...)` remains the live surface-facing seam
    /// used by CLI/REST/RPC/MCP callers and tests; it delegates to the
    /// canonical drain-lifecycle owner method below.
    pub async fn update_peer_ingress_context(
        self: &Arc<Self>,
        session_id: &SessionId,
        keep_alive: bool,
        comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
    ) -> bool {
        match self
            .execute_meerkat_machine_drain_command(
                MeerkatMachineDrainCommand::SetPeerIngressContext {
                    session_id: session_id.clone(),
                    keep_alive,
                    comms_runtime,
                },
            )
            .await
        {
            Ok(MeerkatMachineDrainCommandResult::Spawned(spawned)) => spawned,
            _ => false,
        }
    }

    /// Manage the comms drain lifecycle for a session based on keep_alive intent.
    ///
    /// When `keep_alive` is true, spawns a drain if one is not already running.
    /// When `keep_alive` is false, aborts any running drain for the session.
    /// Returns `true` if a new drain was spawned.
    ///
    /// All state transitions go through `CommsDrainLifecycleAuthority`.
    pub async fn maybe_spawn_comms_drain(
        self: &Arc<Self>,
        session_id: &SessionId,
        keep_alive: bool,
        comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
    ) -> bool {
        match self
            .execute_meerkat_machine_drain_command(
                MeerkatMachineDrainCommand::SetPeerIngressContext {
                    session_id: session_id.clone(),
                    keep_alive,
                    comms_runtime,
                },
            )
            .await
        {
            Ok(MeerkatMachineDrainCommandResult::Spawned(spawned)) => spawned,
            _ => false,
        }
    }

    async fn update_peer_ingress_context_inner(
        self: &Arc<Self>,
        session_id: &SessionId,
        keep_alive: bool,
        comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
    ) -> bool {
        if !keep_alive {
            // Explicit disable: stop any running drain for this session.
            let _ = self
                .execute_meerkat_machine_drain_local_command(
                    MeerkatMachineDrainLocalCommand::Abort {
                        session_id: session_id.clone(),
                    },
                )
                .await;
            return false;
        }

        let mode = CommsDrainMode::PersistentHost;

        let comms = match comms_runtime {
            Some(c) => c,
            None => return false,
        };

        let sessions = self.sessions.read().await;
        if !sessions.contains_key(session_id) {
            tracing::warn!(
                %session_id,
                "refusing to spawn comms drain for unregistered session"
            );
            return false;
        }
        // Keep the session read guard while mutating drain slots so unregister
        // cannot race between registration check and slot publication.
        let mut slots = self.comms_drain_slots.write().await;
        let slot = slots
            .entry(session_id.clone())
            .or_insert_with(CommsDrainSlot::new);

        let result =
            match protocol_comms_drain_spawn::execute_ensure_running(&mut slot.authority, mode) {
                Ok(r) => r,
                Err(e) => {
                    tracing::trace!(error = %e, "comms drain authority rejected EnsureRunning");
                    return false;
                }
            };

        // Execute effects from the transition
        for effect in &result.effects {
            match effect {
                CommsDrainLifecycleEffect::SpawnDrainTask { mode: spawn_mode } => {
                    let idle_timeout = match spawn_mode {
                        CommsDrainMode::PersistentHost => Some(std::time::Duration::MAX),
                        CommsDrainMode::Timed | CommsDrainMode::AttachedSession => None,
                    };
                    let handle = crate::comms_drain::spawn_comms_drain(
                        Arc::clone(self),
                        session_id.clone(),
                        comms.clone(),
                        idle_timeout,
                    );
                    slot.handle = Some(handle);
                }
                CommsDrainLifecycleEffect::AbortDrainTask => {
                    if let Some(handle) = slot.handle.take() {
                        handle.abort();
                    }
                }
            }
        }

        let Some(obligation) = result.obligation else {
            tracing::warn!(
                %session_id,
                "comms drain spawn transition emitted no obligation"
            );
            return false;
        };

        // The runtime spawns the drain task synchronously (as a tokio task
        // above), so we immediately close the spawn obligation.
        match protocol_comms_drain_spawn::submit_task_spawned(&mut slot.authority, obligation) {
            Ok(_effects) => {}
            Err(e) => {
                tracing::trace!(error = %e, "comms drain authority rejected TaskSpawned");
            }
        }
        true
    }

    /// Notify the authority that a drain task has exited with the given reason.
    ///
    /// Called from drain task exit paths (or by wrappers that detect task
    /// completion). The authority decides whether to enter ExitedRespawnable
    /// (PersistentHost + Failed) or Stopped.
    pub async fn notify_comms_drain_exited(
        self: &Arc<Self>,
        session_id: &SessionId,
        reason: DrainExitReason,
    ) {
        let _ = self
            .execute_meerkat_machine_drain_command(MeerkatMachineDrainCommand::NotifyDrainExited {
                session_id: session_id.clone(),
                reason,
            })
            .await;
    }

    async fn notify_comms_drain_exited_inner(
        &self,
        session_id: &SessionId,
        reason: DrainExitReason,
    ) {
        let mut slots = self.comms_drain_slots.write().await;
        if let Some(slot) = slots.get_mut(session_id) {
            slot.handle.take(); // clean up finished handle
            match protocol_comms_drain_spawn::notify_task_exited(&mut slot.authority, reason) {
                Ok(_effects) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "comms drain authority rejected TaskExited");
                }
            }
        }
    }

    /// Abort all active comms drain tasks.
    pub async fn abort_comms_drains(&self) {
        let _ = self
            .execute_meerkat_machine_drain_local_command(MeerkatMachineDrainLocalCommand::AbortAll)
            .await;
    }

    /// Abort the comms drain task for a specific session.
    pub async fn abort_comms_drain(&self, session_id: &SessionId) {
        let _ = self
            .execute_meerkat_machine_drain_local_command(MeerkatMachineDrainLocalCommand::Abort {
                session_id: session_id.clone(),
            })
            .await;
    }

    /// Wait for a session's comms drain task to finish.
    ///
    /// Returns immediately if no drain is active for the session.
    /// If the task already notified the authority (normal exit), this is a no-op
    /// for authority state. If the task panicked without notifying, this submits
    /// `TaskExited { Failed }` as a safety net.
    pub async fn wait_comms_drain(&self, session_id: &SessionId) {
        let _ = self
            .execute_meerkat_machine_drain_local_command(MeerkatMachineDrainLocalCommand::Wait {
                session_id: session_id.clone(),
            })
            .await;
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SessionServiceRuntimeExt for MeerkatMachine {
    fn runtime_mode(&self) -> RuntimeMode {
        self.mode
    }

    async fn accept_input(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        let runtime_id = MeerkatMachine::logical_runtime_id(session_id);
        match self
            .execute_meerkat_machine_control_command(MeerkatMachineControlCommand::Ingest {
                runtime_id,
                input,
            })
            .await
            .map_err(MeerkatMachine::driver_error_from_control_plane_error)?
        {
            MeerkatMachineControlCommandResult::AcceptOutcome(outcome) => Ok(outcome),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineControlCommandResult for SessionServiceRuntimeExt::accept_input: {other:?}"
            ))),
        }
    }

    async fn accept_input_with_completion(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<(AcceptOutcome, Option<crate::completion::CompletionHandle>), RuntimeDriverError>
    {
        MeerkatMachine::accept_input_with_completion(self, session_id, input).await
    }

    async fn runtime_state(
        &self,
        session_id: &SessionId,
    ) -> Result<RuntimeState, RuntimeDriverError> {
        let runtime_id = MeerkatMachine::logical_runtime_id(session_id);
        match self
            .execute_meerkat_machine_control_command(MeerkatMachineControlCommand::RuntimeState {
                runtime_id,
            })
            .await
            .map_err(MeerkatMachine::driver_error_from_control_plane_error)?
        {
            MeerkatMachineControlCommandResult::RuntimeState(state) => Ok(state),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineControlCommandResult for SessionServiceRuntimeExt::runtime_state: {other:?}"
            ))),
        }
    }

    async fn retire_runtime(
        &self,
        session_id: &SessionId,
    ) -> Result<RetireReport, RuntimeDriverError> {
        let runtime_id = MeerkatMachine::logical_runtime_id(session_id);
        match self
            .execute_meerkat_machine_control_command(MeerkatMachineControlCommand::Retire {
                runtime_id,
            })
            .await
            .map_err(MeerkatMachine::driver_error_from_control_plane_error)?
        {
            MeerkatMachineControlCommandResult::RetireReport(report) => Ok(report),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineControlCommandResult for SessionServiceRuntimeExt::retire_runtime: {other:?}"
            ))),
        }
    }

    async fn reset_runtime(
        &self,
        session_id: &SessionId,
    ) -> Result<ResetReport, RuntimeDriverError> {
        let runtime_id = MeerkatMachine::logical_runtime_id(session_id);
        match self
            .execute_meerkat_machine_control_command(MeerkatMachineControlCommand::Reset {
                runtime_id,
            })
            .await
            .map_err(MeerkatMachine::driver_error_from_control_plane_error)?
        {
            MeerkatMachineControlCommandResult::ResetReport(report) => Ok(report),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineControlCommandResult for SessionServiceRuntimeExt::reset_runtime: {other:?}"
            ))),
        }
    }

    async fn input_state(
        &self,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<InputState>, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_session_command(MeerkatMachineSessionCommand::InputState {
                session_id: session_id.clone(),
                input_id: input_id.clone(),
            })
            .await?
        {
            MeerkatMachineSessionCommandResult::InputState(state) => Ok(state),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineSessionCommandResult for SessionServiceRuntimeExt::input_state: {other:?}"
            ))),
        }
    }

    async fn list_active_inputs(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<InputId>, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_session_command(
                MeerkatMachineSessionCommand::ListActiveInputs {
                    session_id: session_id.clone(),
                },
            )
            .await?
        {
            MeerkatMachineSessionCommandResult::ActiveInputs(inputs) => Ok(inputs),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineSessionCommandResult for SessionServiceRuntimeExt::list_active_inputs: {other:?}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// RuntimeControlPlane implementation
// ---------------------------------------------------------------------------

impl MeerkatMachine {
    fn logical_runtime_id(session_id: &SessionId) -> LogicalRuntimeId {
        LogicalRuntimeId::new(session_id.to_string())
    }

    fn driver_error_from_control_plane_error(err: RuntimeControlPlaneError) -> RuntimeDriverError {
        match err {
            RuntimeControlPlaneError::NotFound(_) => RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            },
            RuntimeControlPlaneError::InvalidState { state } => {
                RuntimeDriverError::NotReady { state }
            }
            RuntimeControlPlaneError::StoreError(message)
            | RuntimeControlPlaneError::Internal(message) => RuntimeDriverError::Internal(message),
        }
    }

    /// Resolve a LogicalRuntimeId to a SessionId for internal lookup.
    ///
    /// The adapter uses `LogicalRuntimeId::new(session_id.to_string())` when
    /// creating drivers, so runtime IDs are UUID strings that parse back to
    /// SessionId.
    fn resolve_session_id(
        runtime_id: &LogicalRuntimeId,
    ) -> Result<SessionId, RuntimeControlPlaneError> {
        runtime_id
            .0
            .parse::<uuid::Uuid>()
            .map(SessionId)
            .map_err(|_| RuntimeControlPlaneError::NotFound(runtime_id.clone()))
    }

    async fn existing_session_runtime_state(&self, session_id: &SessionId) -> Option<RuntimeState> {
        let driver = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).map(|entry| entry.driver.clone())
        }?;
        Some(Self::driver_runtime_state(&driver).await)
    }

    async fn driver_runtime_state(driver: &SharedDriver) -> RuntimeState {
        let driver = driver.lock().await;
        driver.as_driver().runtime_state()
    }

    /// Look up the session entry for a runtime ID, returning a control-plane error
    /// if not found.
    async fn lookup_entry(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<
        (
            SessionId,
            SharedDriver,
            SharedCompletionRegistry,
            Option<mpsc::Sender<()>>,
        ),
        RuntimeControlPlaneError,
    > {
        let session_id = Self::resolve_session_id(runtime_id)?;
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(&session_id)
            .ok_or_else(|| RuntimeControlPlaneError::NotFound(runtime_id.clone()))?;
        Ok((
            session_id,
            entry.driver.clone(),
            entry.completions.clone(),
            entry.wake_sender(),
        ))
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl crate::traits::RuntimeControlPlane for MeerkatMachine {
    async fn ingest(
        &self,
        runtime_id: &LogicalRuntimeId,
        input: Input,
    ) -> Result<AcceptOutcome, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_control_command(MeerkatMachineControlCommand::Ingest {
                runtime_id: runtime_id.clone(),
                input,
            })
            .await?
        {
            MeerkatMachineControlCommandResult::AcceptOutcome(outcome) => Ok(outcome),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineControlCommandResult for ingest: {other:?}"
            ))),
        }
    }

    async fn publish_event(
        &self,
        event: crate::runtime_event::RuntimeEventEnvelope,
    ) -> Result<(), RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_control_command(MeerkatMachineControlCommand::PublishEvent {
                event,
            })
            .await?
        {
            MeerkatMachineControlCommandResult::Unit => Ok(()),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineControlCommandResult for publish_event: {other:?}"
            ))),
        }
    }

    async fn retire(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_control_command(MeerkatMachineControlCommand::Retire {
                runtime_id: runtime_id.clone(),
            })
            .await?
        {
            MeerkatMachineControlCommandResult::RetireReport(report) => Ok(report),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineControlCommandResult for retire: {other:?}"
            ))),
        }
    }

    async fn recycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RecycleReport, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_control_command(MeerkatMachineControlCommand::Recycle {
                runtime_id: runtime_id.clone(),
            })
            .await?
        {
            MeerkatMachineControlCommandResult::RecycleReport(report) => Ok(report),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineControlCommandResult for recycle: {other:?}"
            ))),
        }
    }

    async fn reset(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<crate::traits::ResetReport, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_control_command(MeerkatMachineControlCommand::Reset {
                runtime_id: runtime_id.clone(),
            })
            .await?
        {
            MeerkatMachineControlCommandResult::ResetReport(report) => Ok(report),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineControlCommandResult for reset: {other:?}"
            ))),
        }
    }

    async fn recover(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RecoveryReport, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_control_command(MeerkatMachineControlCommand::Recover {
                runtime_id: runtime_id.clone(),
            })
            .await?
        {
            MeerkatMachineControlCommandResult::RecoveryReport(report) => Ok(report),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineControlCommandResult for recover: {other:?}"
            ))),
        }
    }

    async fn destroy(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<DestroyReport, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_control_command(MeerkatMachineControlCommand::Destroy {
                runtime_id: runtime_id.clone(),
            })
            .await?
        {
            MeerkatMachineControlCommandResult::DestroyReport(report) => Ok(report),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineControlCommandResult for destroy: {other:?}"
            ))),
        }
    }

    async fn runtime_state(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RuntimeState, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_control_command(MeerkatMachineControlCommand::RuntimeState {
                runtime_id: runtime_id.clone(),
            })
            .await?
        {
            MeerkatMachineControlCommandResult::RuntimeState(state) => Ok(state),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineControlCommandResult for runtime_state: {other:?}"
            ))),
        }
    }

    async fn load_boundary_receipt(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<Option<meerkat_core::lifecycle::RunBoundaryReceipt>, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_control_command(
                MeerkatMachineControlCommand::LoadBoundaryReceipt {
                    runtime_id: runtime_id.clone(),
                    run_id: run_id.clone(),
                    sequence,
                },
            )
            .await?
        {
            MeerkatMachineControlCommandResult::BoundaryReceipt(receipt) => Ok(receipt),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineControlCommandResult for load_boundary_receipt: {other:?}"
            ))),
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use chrono::Utc;
    use meerkat_core::agent::{CommsCapabilityError, CommsRuntime};
    use meerkat_core::comms_drain_lifecycle_authority::{CommsDrainMode, CommsDrainPhase};
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_control::RunControlCommand;
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
    use meerkat_core::lifecycle::{InputId, RunId};
    use meerkat_core::ops::{OperationId, OperationResult};
    use meerkat_core::ops_lifecycle::{
        OperationKind, OperationProgressUpdate, OperationSpec, OpsLifecycleRegistry,
    };
    use tokio::sync::Notify;

    use crate::completion::CompletionOutcome;
    use crate::identifiers::IdempotencyKey;

    struct FakeDrainRuntime {
        notify: Arc<Notify>,
        dismiss: AtomicBool,
    }

    impl FakeDrainRuntime {
        fn dismissing() -> Self {
            Self {
                notify: Arc::new(Notify::new()),
                dismiss: AtomicBool::new(true),
            }
        }

        fn idle() -> Self {
            Self {
                notify: Arc::new(Notify::new()),
                dismiss: AtomicBool::new(false),
            }
        }
    }

    fn make_prompt(text: &str) -> Input {
        Input::Prompt(crate::input::PromptInput {
            header: crate::input::InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: crate::input::InputOrigin::Operator,
                durability: crate::input::InputDurability::Durable,
                visibility: crate::input::InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            text: text.into(),
            blocks: None,
            turn_metadata: None,
        })
    }

    fn make_progress_input(label: &str) -> Input {
        Input::Peer(crate::input::PeerInput {
            header: crate::input::InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: crate::input::InputOrigin::Peer {
                    peer_id: "peer-1".into(),
                    runtime_id: None,
                },
                durability: crate::input::InputDurability::Ephemeral,
                visibility: crate::input::InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseProgress {
                request_id: format!("req-{label}"),
                phase: crate::input::ResponseProgressPhase::InProgress,
            }),
            body: format!("progress-{label}"),
            blocks: None,
            handling_mode: None,
        })
    }

    #[async_trait::async_trait]
    impl CommsRuntime for FakeDrainRuntime {
        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<Notify> {
            Arc::clone(&self.notify)
        }

        fn dismiss_received(&self) -> bool {
            self.dismiss.load(Ordering::Acquire)
        }

        async fn drain_classified_inbox_interactions(
            &self,
        ) -> Result<Vec<meerkat_core::interaction::ClassifiedInboxInteraction>, CommsCapabilityError>
        {
            Ok(Vec::new())
        }
    }

    async fn spawn_test_comms_drain(
        adapter: &Arc<MeerkatMachine>,
        session_id: &SessionId,
        mode: CommsDrainMode,
        comms_runtime: Arc<dyn CommsRuntime>,
        idle_timeout: Duration,
    ) {
        adapter.register_session(session_id.clone()).await;
        let mut slots = adapter.comms_drain_slots.write().await;
        let slot = slots
            .entry(session_id.clone())
            .or_insert_with(CommsDrainSlot::new);
        let result = protocol_comms_drain_spawn::execute_ensure_running(&mut slot.authority, mode)
            .expect("ensure running");
        let obligation = result
            .obligation
            .expect("spawn obligation should be present");

        apply_runtime_drain_effects(slot, &result.effects);
        for effect in &result.effects {
            if let CommsDrainLifecycleEffect::SpawnDrainTask { .. } = effect {
                slot.handle = Some(crate::comms_drain::spawn_comms_drain(
                    Arc::clone(adapter),
                    session_id.clone(),
                    Arc::clone(&comms_runtime),
                    Some(idle_timeout),
                ));
            }
        }

        let feedback_effects =
            protocol_comms_drain_spawn::submit_task_spawned(&mut slot.authority, obligation)
                .expect("task spawned");
        apply_runtime_drain_effects(slot, &feedback_effects);
    }

    async fn current_phase(
        adapter: &Arc<MeerkatMachine>,
        session_id: &SessionId,
    ) -> Option<CommsDrainPhase> {
        let slots = adapter.comms_drain_slots.read().await;
        slots.get(session_id).map(|slot| slot.authority.phase())
    }

    async fn handle_present(adapter: &Arc<MeerkatMachine>, session_id: &SessionId) -> bool {
        let slots = adapter.comms_drain_slots.read().await;
        slots
            .get(session_id)
            .and_then(|slot| slot.handle.as_ref())
            .is_some()
    }

    async fn wait_for_phase(
        adapter: &Arc<MeerkatMachine>,
        session_id: &SessionId,
        expected: CommsDrainPhase,
    ) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if current_phase(adapter, session_id).await == Some(expected) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("phase transition");
    }

    #[tokio::test]
    async fn dismiss_exit_updates_authority_before_join() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::dismissing());

        spawn_test_comms_drain(
            &adapter,
            &session_id,
            CommsDrainMode::PersistentHost,
            comms_runtime,
            Duration::from_millis(25),
        )
        .await;

        wait_for_phase(&adapter, &session_id, CommsDrainPhase::Stopped).await;
        assert!(
            !handle_present(&adapter, &session_id).await,
            "drain task should clear its slot before wait_comms_drain joins"
        );

        adapter.wait_comms_drain(&session_id).await;
        assert_eq!(
            current_phase(&adapter, &session_id).await,
            Some(CommsDrainPhase::Stopped)
        );
    }

    #[tokio::test]
    async fn idle_timeout_updates_authority_before_join() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());

        spawn_test_comms_drain(
            &adapter,
            &session_id,
            CommsDrainMode::Timed,
            comms_runtime,
            Duration::from_millis(25),
        )
        .await;

        wait_for_phase(&adapter, &session_id, CommsDrainPhase::Stopped).await;
        assert!(
            !handle_present(&adapter, &session_id).await,
            "drain task should clear its slot before wait_comms_drain joins"
        );

        adapter.wait_comms_drain(&session_id).await;
        assert_eq!(
            current_phase(&adapter, &session_id).await,
            Some(CommsDrainPhase::Stopped)
        );
    }

    #[tokio::test]
    async fn unregister_session_aborts_and_removes_drain_slot() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());

        adapter.register_session(session_id.clone()).await;
        spawn_test_comms_drain(
            &adapter,
            &session_id,
            CommsDrainMode::PersistentHost,
            comms_runtime,
            Duration::from_secs(60),
        )
        .await;

        assert_eq!(
            current_phase(&adapter, &session_id).await,
            Some(CommsDrainPhase::Running)
        );
        assert!(handle_present(&adapter, &session_id).await);

        adapter.unregister_session(&session_id).await;

        let slots = adapter.comms_drain_slots.read().await;
        assert!(
            !slots.contains_key(&session_id),
            "unregister must remove the comms drain slot entirely"
        );
    }

    #[tokio::test]
    async fn session_service_runtime_ext_write_side_follows_machine_control_surface() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        adapter.register_session(session_id.clone()).await;

        let state =
            <MeerkatMachine as SessionServiceRuntimeExt>::runtime_state(&adapter, &session_id)
                .await
                .expect("runtime state should route through the machine seam");
        assert_eq!(state, RuntimeState::Idle);

        let outcome = <MeerkatMachine as SessionServiceRuntimeExt>::accept_input(
            &adapter,
            &session_id,
            make_prompt("service-ext-write-side"),
        )
        .await
        .expect("accept_input should route through the machine seam");
        assert!(
            matches!(outcome, AcceptOutcome::Accepted { .. }),
            "prompt should still be admitted through the SessionServiceRuntimeExt seam"
        );

        let active =
            <MeerkatMachine as SessionServiceRuntimeExt>::list_active_inputs(&adapter, &session_id)
                .await
                .expect("active inputs should still be readable");
        assert_eq!(active.len(), 1, "accepted input should remain active");
        let active_state = <MeerkatMachine as SessionServiceRuntimeExt>::input_state(
            &adapter,
            &session_id,
            &active[0],
        )
        .await
        .expect("input_state should route through the machine seam");
        assert_eq!(
            active_state.map(|state| state.current_state()),
            Some(crate::input_state::InputLifecycleState::Queued),
            "accepted prompt should still be visible through machine-routed input_state"
        );

        let retire_report =
            <MeerkatMachine as SessionServiceRuntimeExt>::retire_runtime(&adapter, &session_id)
                .await
                .expect("retire should route through the machine seam");
        assert_eq!(
            retire_report.inputs_abandoned, 1,
            "retire should still abandon queued work when no runtime loop is attached"
        );
        assert_eq!(retire_report.inputs_pending_drain, 0);

        let reset_report =
            <MeerkatMachine as SessionServiceRuntimeExt>::reset_runtime(&adapter, &session_id)
                .await
                .expect("reset should route through the machine seam");
        assert_eq!(
            reset_report.inputs_abandoned, 0,
            "reset after retire should not find residual queued work"
        );
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_reports_registered_idle_session() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let snapshot = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist for registered session");

        assert_eq!(snapshot.binding.session_id, session_id);
        assert_eq!(
            snapshot.binding.driver_kind,
            crate::meerkat_machine_types::MeerkatDriverKind::Ephemeral
        );
        assert!(snapshot.binding.driver_present);
        assert!(snapshot.binding.completions_present);
        assert!(snapshot.binding.ops_registry_present);
        assert_eq!(snapshot.control.phase, RuntimeState::Idle);
        assert!(!snapshot.binding.attachment_live);
        assert!(!snapshot.binding.detached_wake_present);
        assert_eq!(snapshot.binding.cursor_state.agent_applied_cursor, 0);
        assert_eq!(snapshot.binding.cursor_state.runtime_observed_seq, 0);
        assert_eq!(snapshot.binding.cursor_state.runtime_last_injected_seq, 0);
        assert!(snapshot.inputs.admission_order.is_empty());
        assert!(snapshot.inputs.queue.is_empty());
        assert!(snapshot.inputs.steer_queue.is_empty());
        assert_eq!(snapshot.completion_waiters.input_count, 0);
        assert_eq!(snapshot.completion_waiters.waiter_count, 0);
        assert!(snapshot.completion_waiters.waiting_inputs.is_empty());
        assert!(!snapshot.drain.slot_present);
        assert_eq!(snapshot.drain.phase, None);
        assert_eq!(snapshot.drain.mode, None);
        assert!(!snapshot.drain.handle_present);
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_tracks_queued_prompt_input() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = make_prompt("hello from the runtime spine");
        let input_id = input.id().clone();

        let (_outcome, handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("prompt should be accepted");
        assert!(
            handle.is_some(),
            "queued prompt should register a completion"
        );

        let snapshot = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist for registered session");

        assert_eq!(snapshot.control.phase, RuntimeState::Idle);
        assert_eq!(snapshot.inputs.queue, vec![input_id.clone()]);
        assert!(snapshot.inputs.steer_queue.is_empty());
        assert_eq!(snapshot.inputs.current_run_id, None);
        assert_eq!(
            snapshot.inputs.current_run_contributors,
            Vec::<InputId>::new()
        );
        assert_eq!(snapshot.inputs.admission_order.len(), 1);
        assert_eq!(snapshot.completion_waiters.input_count, 1);
        assert_eq!(snapshot.completion_waiters.waiter_count, 1);
        assert_eq!(snapshot.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            snapshot.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            snapshot.completion_waiters.waiting_inputs[0].waiter_count,
            1
        );

        let input_snapshot = &snapshot.inputs.admission_order[0];
        assert_eq!(input_snapshot.input_id, input_id);
        assert_eq!(
            input_snapshot.lifecycle,
            Some(crate::input_state::InputLifecycleState::Queued)
        );
        assert_eq!(
            input_snapshot.handling_mode,
            Some(meerkat_core::types::HandlingMode::Queue)
        );
        assert_eq!(snapshot.inputs.queue, vec![input_id.clone()]);
        assert!(snapshot.inputs.steer_queue.is_empty());
        assert!(input_snapshot.content_shape.is_some());
        assert_eq!(input_snapshot.last_run_id, None);
        assert_eq!(input_snapshot.last_boundary_sequence, None);
        assert!(input_snapshot.terminal_outcome.is_none());
        assert!(input_snapshot.is_prompt);
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_tracks_deduplicated_completion_waiters() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let mut first = make_prompt("first deduplicated input");
        if let Input::Prompt(prompt) = &mut first {
            prompt.header.idempotency_key = Some(IdempotencyKey::new("same-input"));
        }
        let first_input_id = first.id().clone();

        let (first_outcome, first_handle) = adapter
            .accept_input_with_completion(&session_id, first)
            .await
            .expect("first input should be accepted");
        assert!(matches!(first_outcome, AcceptOutcome::Accepted { .. }));
        assert!(
            first_handle.is_some(),
            "first queued input should register a completion waiter"
        );

        let mut duplicate = make_prompt("second deduplicated input");
        if let Input::Prompt(prompt) = &mut duplicate {
            prompt.header.idempotency_key = Some(IdempotencyKey::new("same-input"));
        }

        let (duplicate_outcome, duplicate_handle) = adapter
            .accept_input_with_completion(&session_id, duplicate)
            .await
            .expect("duplicate input should deduplicate");
        match duplicate_outcome {
            AcceptOutcome::Deduplicated { existing_id, .. } => {
                assert_eq!(existing_id, first_input_id);
            }
            other => panic!("expected deduplicated outcome, got {other:?}"),
        }
        assert!(
            duplicate_handle.is_some(),
            "deduplicated in-flight input should join the existing waiter set"
        );

        let snapshot = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist for registered session");

        assert_eq!(snapshot.inputs.queue, vec![first_input_id.clone()]);
        assert_eq!(snapshot.inputs.admission_order.len(), 1);
        assert_eq!(snapshot.completion_waiters.input_count, 1);
        assert_eq!(snapshot.completion_waiters.waiter_count, 2);
        assert_eq!(snapshot.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            snapshot.completion_waiters.waiting_inputs[0].input_id,
            first_input_id
        );
        assert_eq!(
            snapshot.completion_waiters.waiting_inputs[0].waiter_count,
            2
        );
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_tracks_steered_prompt_input() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = Input::Prompt(crate::input::PromptInput::new(
            "steer prompt",
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                    ..Default::default()
                },
            ),
        ));
        let input_id = input.id().clone();

        let (_outcome, handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("steered prompt should be accepted");
        assert!(
            handle.is_some(),
            "steered prompt should register a completion"
        );

        let snapshot = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist for registered session");

        assert!(snapshot.inputs.queue.is_empty());
        assert_eq!(snapshot.inputs.steer_queue, vec![input_id.clone()]);
        assert!(snapshot.inputs.wake_requested);
        assert!(snapshot.inputs.process_requested);
        assert_eq!(snapshot.inputs.admission_order.len(), 1);
        let input_snapshot = &snapshot.inputs.admission_order[0];
        assert_eq!(input_snapshot.input_id, input_id);
        assert_eq!(
            input_snapshot.handling_mode,
            Some(meerkat_core::types::HandlingMode::Steer)
        );
        assert_eq!(
            input_snapshot.lifecycle,
            Some(crate::input_state::InputLifecycleState::Queued)
        );
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_reset() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = make_prompt("reset pending waiter");
        let input_id = input.id().clone();
        let (_outcome, handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("prompt should be accepted");
        let handle = handle.expect("queued prompt should register a waiter");

        let before_reset = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before reset");
        assert_eq!(before_reset.completion_waiters.input_count, 1);
        assert_eq!(before_reset.completion_waiters.waiter_count, 1);
        assert_eq!(before_reset.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_reset.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );

        SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
            .await
            .expect("reset should succeed for idle queued runtime");

        let after_reset = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after reset");
        assert_eq!(after_reset.completion_waiters.input_count, 0);
        assert_eq!(after_reset.completion_waiters.waiter_count, 0);
        assert!(after_reset.completion_waiters.waiting_inputs.is_empty());

        match handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime reset");
            }
            other => panic!("expected runtime reset termination, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recycle() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = make_prompt("recycle pending waiter");
        let input_id = input.id().clone();
        let (_outcome, handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("prompt should be accepted");
        let handle = handle.expect("queued prompt should register a waiter");

        let before_recycle = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before recycle");
        assert_eq!(before_recycle.control.phase, RuntimeState::Idle);
        assert_eq!(before_recycle.inputs.queue, vec![input_id.clone()]);
        assert_eq!(before_recycle.completion_waiters.input_count, 1);
        assert_eq!(before_recycle.completion_waiters.waiter_count, 1);
        assert_eq!(
            before_recycle.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::recycle(&adapter, &runtime_id)
            .await
            .expect("recycle should preserve queued work");
        assert_eq!(report.inputs_transferred, 1);

        let after_recycle = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after recycle");
        assert_eq!(after_recycle.control.phase, RuntimeState::Idle);
        assert_eq!(after_recycle.inputs.queue, vec![input_id.clone()]);
        assert_eq!(after_recycle.completion_waiters.input_count, 1);
        assert_eq!(after_recycle.completion_waiters.waiter_count, 1);
        assert_eq!(after_recycle.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            after_recycle.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            after_recycle.completion_waiters.waiting_inputs[0].waiter_count,
            1
        );

        SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
            .await
            .expect("reset should terminate preserved waiter at test end");
        match handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime reset");
            }
            other => panic!("expected runtime reset termination, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_recycle_reconciles_stale_completion_waiters() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = make_prompt("preserve active waiter");
        let input_id = input.id().clone();
        let (_outcome, active_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("prompt should be accepted");
        let active_handle = active_handle.expect("queued prompt should register a waiter");

        let completions = {
            let sessions = adapter.sessions.read().await;
            Arc::clone(
                &sessions
                    .get(&session_id)
                    .expect("registered session should exist")
                    .completions,
            )
        };
        let stale_input_id = InputId::new();
        let stale_handle = {
            let mut completions = completions.lock().await;
            completions.register(stale_input_id.clone())
        };

        let before_recycle = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before recycle");
        assert_eq!(before_recycle.completion_waiters.input_count, 2);
        assert_eq!(before_recycle.completion_waiters.waiter_count, 2);
        assert!(
            before_recycle
                .completion_waiters
                .waiting_inputs
                .iter()
                .any(|entry| entry.input_id == input_id && entry.waiter_count == 1),
            "active queued input should have one visible waiter before recycle"
        );
        assert!(
            before_recycle
                .completion_waiters
                .waiting_inputs
                .iter()
                .any(|entry| entry.input_id == stale_input_id && entry.waiter_count == 1),
            "stale waiter should be visible before recycle reconciliation"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::recycle(&adapter, &runtime_id)
            .await
            .expect("recycle should reconcile waiters against active input truth");
        assert_eq!(report.inputs_transferred, 1);

        let after_recycle = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after recycle");
        assert_eq!(after_recycle.completion_waiters.input_count, 1);
        assert_eq!(after_recycle.completion_waiters.waiter_count, 1);
        assert_eq!(after_recycle.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            after_recycle.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            after_recycle.completion_waiters.waiting_inputs[0].waiter_count,
            1
        );

        match stale_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "recycled input no longer pending");
            }
            other => panic!("expected recycled stale waiter termination, got {other:?}"),
        }

        SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
            .await
            .expect("reset should terminate preserved waiter at test end");
        match active_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime reset");
            }
            other => panic!("expected runtime reset termination, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recover() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = make_prompt("recover pending waiter");
        let input_id = input.id().clone();
        let (_outcome, handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("prompt should be accepted");
        let handle = handle.expect("queued prompt should register a waiter");

        let before_recover = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before recover");
        assert_eq!(before_recover.control.phase, RuntimeState::Idle);
        assert_eq!(before_recover.inputs.queue, vec![input_id.clone()]);
        assert_eq!(before_recover.completion_waiters.input_count, 1);
        assert_eq!(before_recover.completion_waiters.waiter_count, 1);
        assert_eq!(
            before_recover.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::recover(&adapter, &runtime_id)
            .await
            .expect("recover should preserve queued work");
        assert_eq!(report.inputs_recovered, 1);

        let after_recover = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after recover");
        assert_eq!(after_recover.control.phase, RuntimeState::Idle);
        assert_eq!(after_recover.inputs.queue, vec![input_id.clone()]);
        assert_eq!(after_recover.completion_waiters.input_count, 1);
        assert_eq!(after_recover.completion_waiters.waiter_count, 1);
        assert_eq!(after_recover.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            after_recover.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            after_recover.completion_waiters.waiting_inputs[0].waiter_count,
            1
        );

        SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
            .await
            .expect("reset should terminate preserved waiter at test end");
        match handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime reset");
            }
            other => panic!("expected runtime reset termination, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_recover_reconciles_stale_completion_waiters() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = make_prompt("preserve active waiter on recover");
        let input_id = input.id().clone();
        let (_outcome, active_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("prompt should be accepted");
        let active_handle = active_handle.expect("queued prompt should register a waiter");

        let completions = {
            let sessions = adapter.sessions.read().await;
            Arc::clone(
                &sessions
                    .get(&session_id)
                    .expect("registered session should exist")
                    .completions,
            )
        };
        let stale_input_id = InputId::new();
        let stale_handle = {
            let mut completions = completions.lock().await;
            completions.register(stale_input_id.clone())
        };

        let before_recover = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before recover");
        assert_eq!(before_recover.completion_waiters.input_count, 2);
        assert_eq!(before_recover.completion_waiters.waiter_count, 2);
        assert!(
            before_recover
                .completion_waiters
                .waiting_inputs
                .iter()
                .any(|entry| entry.input_id == input_id && entry.waiter_count == 1),
            "active queued input should have one visible waiter before recover"
        );
        assert!(
            before_recover
                .completion_waiters
                .waiting_inputs
                .iter()
                .any(|entry| entry.input_id == stale_input_id && entry.waiter_count == 1),
            "stale waiter should be visible before recover reconciliation"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::recover(&adapter, &runtime_id)
            .await
            .expect("recover should reconcile waiters against active input truth");
        assert_eq!(report.inputs_recovered, 1);

        let after_recover = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after recover");
        assert_eq!(after_recover.completion_waiters.input_count, 1);
        assert_eq!(after_recover.completion_waiters.waiter_count, 1);
        assert_eq!(after_recover.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            after_recover.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            after_recover.completion_waiters.waiting_inputs[0].waiter_count,
            1
        );

        match stale_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "recovered input no longer pending");
            }
            other => panic!("expected recovered stale waiter termination, got {other:?}"),
        }

        SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
            .await
            .expect("reset should terminate preserved waiter at test end");
        match active_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime reset");
            }
            other => panic!("expected runtime reset termination, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_destroy() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = make_prompt("destroy completion waiter");
        let input_id = input.id().clone();
        let (_outcome, handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("prompt should be accepted");
        let handle = handle.expect("queued prompt should register a completion waiter");

        let before_destroy = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before destroy");
        assert_eq!(before_destroy.control.phase, RuntimeState::Idle);
        assert_eq!(before_destroy.completion_waiters.input_count, 1);
        assert_eq!(before_destroy.completion_waiters.waiter_count, 1);
        assert_eq!(before_destroy.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_destroy.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            before_destroy.completion_waiters.waiting_inputs[0].waiter_count,
            1
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
            .await
            .expect("destroy should terminate active completion waiters");
        assert_eq!(report.inputs_abandoned, 1);

        let after_destroy = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after destroy");
        assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
        assert!(
            after_destroy.inputs.queue.is_empty(),
            "destroy should not leave ordinary queued work behind once the runtime is destroyed"
        );
        assert!(
            after_destroy.inputs.steer_queue.is_empty(),
            "destroy should not leave steer-queued work behind once the runtime is destroyed"
        );
        assert_eq!(after_destroy.completion_waiters.input_count, 0);
        assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
        assert!(
            after_destroy.completion_waiters.waiting_inputs.is_empty(),
            "destroy should clear the completion waiter carrier immediately"
        );

        match handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime destroyed");
            }
            other => panic!("expected runtime destroyed termination, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_destroy_clears_steered_waiter_and_queue_but_preserves_wait_all()
     {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = Input::Prompt(crate::input::PromptInput::new(
            "destroy steered prompt",
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                    ..Default::default()
                },
            ),
        ));
        let input_id = input.id().clone();
        let (_outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("steered prompt should be accepted");
        let completion_handle =
            completion_handle.expect("steered prompt should register a completion waiter");

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "destroy steered wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_destroy = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before destroy");
        let wait_request_id = before_destroy
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_destroy.control.phase, RuntimeState::Idle);
        assert!(before_destroy.inputs.queue.is_empty());
        assert_eq!(before_destroy.inputs.steer_queue, vec![input_id.clone()]);
        assert!(before_destroy.inputs.wake_requested);
        assert!(before_destroy.inputs.process_requested);
        assert_eq!(before_destroy.completion_waiters.input_count, 1);
        assert_eq!(before_destroy.completion_waiters.waiter_count, 1);
        assert_eq!(before_destroy.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_destroy.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert!(before_destroy.ops.pending_wait_present);
        assert_eq!(
            before_destroy.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before destroy"
        );
        assert_eq!(
            before_destroy.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before destroy"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
            .await
            .expect("destroy should clear the steered completion waiter while preserving wait_all");
        assert_eq!(report.inputs_abandoned, 1);

        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime destroyed");
            }
            other => {
                panic!("expected runtime destroyed termination for steered input, got {other:?}")
            }
        }

        let after_destroy = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after destroy");
        assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
        assert_eq!(after_destroy.control.current_run_id, None);
        assert_eq!(after_destroy.inputs.current_run_id, None);
        assert!(after_destroy.inputs.queue.is_empty());
        assert!(
            after_destroy.inputs.steer_queue.is_empty(),
            "destroy should clear steered queued work immediately on the plain runtime path"
        );
        assert_eq!(after_destroy.completion_waiters.input_count, 0);
        assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
        assert!(
            after_destroy.completion_waiters.waiting_inputs.is_empty(),
            "destroy should clear input-owned steered completion waiters immediately"
        );
        assert_eq!(
            after_destroy.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "destroy should preserve the authority-owned wait request after steered completion waiters clear"
        );
        assert!(after_destroy.ops.pending_wait_present);
        assert_eq!(
            after_destroy.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "destroy should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_destroy.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "destroy should preserve the tracked wait target until the operation settles"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after destroy");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Destroyed);
        assert!(settled.inputs.queue.is_empty());
        assert!(settled.inputs.steer_queue.is_empty());
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_destroy_with_runtime_loop()
     {
        struct RecordingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for RecordingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(RecordingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                }),
            )
            .await;

        let input = make_progress_input("destroy-with-loop");
        let input_id = input.id().clone();
        let outcome = adapter
            .accept_input_without_wake(&session_id, input)
            .await
            .expect("progress input should queue without waking the attached loop");
        assert!(outcome.is_accepted());

        let handle = {
            let completions = {
                let sessions = adapter.sessions.read().await;
                sessions
                    .get(&session_id)
                    .expect("attached session should exist")
                    .completions
                    .clone()
            };
            let mut completions = completions.lock().await;
            completions.register(input_id.clone())
        };

        let before_destroy = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before destroy");
        assert_eq!(before_destroy.control.phase, RuntimeState::Attached);
        assert_eq!(before_destroy.completion_waiters.input_count, 1);
        assert_eq!(before_destroy.completion_waiters.waiter_count, 1);
        assert_eq!(before_destroy.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_destroy.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "destroy should be able to abandon queued attached-loop work before apply runs"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "destroy has not yet attempted any executor control"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
            .await
            .expect("destroy should synchronously clear queued waiters even with a live loop");
        assert_eq!(report.inputs_abandoned, 1);

        let after_destroy = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after destroy");
        assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
        assert!(
            after_destroy.inputs.queue.is_empty(),
            "destroy should not leave ordinary queued work behind even when an attached loop exists"
        );
        assert!(
            after_destroy.inputs.steer_queue.is_empty(),
            "destroy should not leave steer-queued work behind even when an attached loop exists"
        );
        assert_eq!(after_destroy.completion_waiters.input_count, 0);
        assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
        assert!(
            after_destroy.completion_waiters.waiting_inputs.is_empty(),
            "destroy should clear the completion waiter carrier immediately even when a loop is attached"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "destroy should bypass queued attached-loop work entirely"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "destroy currently bypasses the executor control seam and does not deliver an out-of-band control command"
        );

        match handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime destroyed");
            }
            other => panic!("expected runtime destroyed termination, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_attached_steered_prompt_requests_immediate_processing()
    {
        struct BlockingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));
        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let input = Input::Prompt(crate::input::PromptInput::new(
            "attached steered prompt",
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                    ..Default::default()
                },
            ),
        ));
        let input_id = input.id().clone();
        let (outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("attached steered prompt should be accepted");
        assert!(outcome.is_accepted());
        let completion_handle =
            completion_handle.expect("attached steered prompt should expose a completion waiter");

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect("attached steered prompt should request immediate processing");

        let during_apply = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist while attached steered work is active");
        assert_eq!(
            during_apply.control.phase,
            RuntimeState::Running,
            "attached steered input should enter Running rather than remain in a queue-only attached state"
        );
        assert_eq!(
            during_apply.control.current_run_id, during_apply.inputs.current_run_id,
            "attached steered input should bind control and ingress to the same active run"
        );
        assert!(
            during_apply.control.current_run_id.is_some(),
            "attached steered input should create an active run binding"
        );
        assert!(
            during_apply.inputs.queue.is_empty(),
            "attached steered input should not occupy the ordinary queue while it is actively processing"
        );
        assert!(
            during_apply.inputs.steer_queue.is_empty(),
            "attached steered input should not remain in the steer queue once immediate processing begins"
        );
        assert_eq!(during_apply.completion_waiters.input_count, 1);
        assert_eq!(during_apply.completion_waiters.waiter_count, 1);
        assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            during_apply.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "attached steered input should wake the attached loop exactly once"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            1,
            "attached steered admission currently routes one control command through the executor seam while requesting immediate processing"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .expect("attached steered prompt should finish after the executor is released");

        match completion_handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!(
                "expected attached steered prompt to complete through the live loop, got {other:?}"
            ),
        }

        let settled = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after attached steered work completes");
                if snapshot.control.phase == RuntimeState::Attached {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("attached runtime should return to Attached after steered work completes");
        assert_eq!(settled.control.current_run_id, None);
        assert_eq!(settled.inputs.current_run_id, None);
        assert!(settled.inputs.queue.is_empty());
        assert!(settled.inputs.steer_queue.is_empty());
        assert_eq!(settled.completion_waiters.input_count, 0);
        assert_eq!(settled.completion_waiters.waiter_count, 0);
        assert!(settled.completion_waiters.waiting_inputs.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_attached_steered_prompt_splits_completion_and_wait_all_lifetimes()
     {
        struct BlockingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));
        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "attached steered wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let input = Input::Prompt(crate::input::PromptInput::new(
            "attached steered split lifetimes",
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                    ..Default::default()
                },
            ),
        ));
        let input_id = input.id().clone();
        let (outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("attached steered prompt should be accepted");
        assert!(outcome.is_accepted());
        let completion_handle =
            completion_handle.expect("attached steered prompt should expose a completion waiter");

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect("attached steered prompt should request immediate processing");

        let during_apply = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist while attached steered work is active");
        let wait_request_id = during_apply
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(during_apply.control.phase, RuntimeState::Running);
        assert_eq!(
            during_apply.control.current_run_id,
            during_apply.inputs.current_run_id
        );
        assert!(during_apply.control.current_run_id.is_some());
        assert!(during_apply.inputs.queue.is_empty());
        assert!(during_apply.inputs.steer_queue.is_empty());
        assert_eq!(during_apply.completion_waiters.input_count, 1);
        assert_eq!(during_apply.completion_waiters.waiter_count, 1);
        assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            during_apply.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            during_apply.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id while the attached steered prompt is active"
        );
        assert_eq!(
            during_apply.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the background operation while attached steered work is active"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "attached steered work should wake the loop exactly once"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            1,
            "attached steered admission should still route one control command through the executor seam"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .expect("attached steered prompt should finish after the executor is released");

        match completion_handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!(
                "expected attached steered prompt to complete while wait_all remains live, got {other:?}"
            ),
        }

        let after_completion = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after attached steered completion");
                if snapshot.control.phase == RuntimeState::Attached {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("attached runtime should return to Attached after steered work completes");
        assert_eq!(after_completion.control.current_run_id, None);
        assert_eq!(after_completion.inputs.current_run_id, None);
        assert!(after_completion.inputs.queue.is_empty());
        assert!(after_completion.inputs.steer_queue.is_empty());
        assert_eq!(after_completion.completion_waiters.input_count, 0);
        assert_eq!(after_completion.completion_waiters.waiter_count, 0);
        assert!(
            after_completion
                .completion_waiters
                .waiting_inputs
                .is_empty()
        );
        assert_eq!(
            after_completion.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "attached steered completion should clear the input-owned completion waiter while preserving the ops-owned wait_all carrier"
        );
        assert!(after_completion.ops.pending_wait_present);
        assert_eq!(
            after_completion.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "request-id agreement should survive after attached steered completion clears the input waiter"
        );
        assert_eq!(
            after_completion.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "tracked wait target should remain present until the background operation itself settles"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after attached steered completion");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Attached);
        assert_eq!(settled.control.current_run_id, None);
        assert_eq!(settled.inputs.current_run_id, None);
        assert!(settled.inputs.queue.is_empty());
        assert!(settled.inputs.steer_queue.is_empty());
        assert_eq!(settled.completion_waiters.input_count, 0);
        assert_eq!(settled.completion_waiters.waiter_count, 0);
        assert!(settled.completion_waiters.waiting_inputs.is_empty());
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_attached_steered_prompt_preserves_completion_after_wait_all_settles()
     {
        struct BlockingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));
        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                // Use MobMemberChild here so the proof isolates completion-vs-wait_all
                // ordering without immediately arming the detached-wake continuation path.
                kind: OperationKind::MobMemberChild,
                owner_session_id: session_id.clone(),
                display_name: "attached steered wait-first child".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: true,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let input = Input::Prompt(crate::input::PromptInput::new(
            "attached steered wait-first split",
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                    ..Default::default()
                },
            ),
        ));
        let input_id = input.id().clone();
        let (outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("attached steered prompt should be accepted");
        assert!(outcome.is_accepted());
        let completion_handle =
            completion_handle.expect("attached steered prompt should expose a completion waiter");

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect("attached steered prompt should request immediate processing");

        let during_apply = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist while attached steered work is active");
        let wait_request_id = during_apply
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(during_apply.control.phase, RuntimeState::Running);
        assert_eq!(
            during_apply.control.current_run_id,
            during_apply.inputs.current_run_id
        );
        assert!(during_apply.control.current_run_id.is_some());
        assert!(during_apply.inputs.queue.is_empty());
        assert!(during_apply.inputs.steer_queue.is_empty());
        assert_eq!(during_apply.completion_waiters.input_count, 1);
        assert_eq!(during_apply.completion_waiters.waiter_count, 1);
        assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            during_apply.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            during_apply.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id while attached steered work is active"
        );
        assert_eq!(
            during_apply.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the live operation while attached steered work is active"
        );
        assert_eq!(apply_calls.load(Ordering::SeqCst), 1);
        assert_eq!(control_calls.load(Ordering::SeqCst), 1);

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete while attached steered work remains in flight");
        let wait_result = wait_future.await.expect("wait_all should resolve first");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let after_wait_all = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after wait_all settles");
                if snapshot.ops.wait_request_id.is_none() && !snapshot.ops.pending_wait_present {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("attached steered snapshot should eventually clear the wait_all carrier");
        assert_eq!(
            after_wait_all.control.phase,
            RuntimeState::Running,
            "attached steered work should remain Running while apply is still blocked even after wait_all settles"
        );
        assert_eq!(
            after_wait_all.control.current_run_id, after_wait_all.inputs.current_run_id,
            "attached steered work should keep control and ingress bound to the same active run until completion"
        );
        assert!(after_wait_all.control.current_run_id.is_some());
        assert!(after_wait_all.inputs.queue.is_empty());
        assert!(after_wait_all.inputs.steer_queue.is_empty());
        assert_eq!(after_wait_all.completion_waiters.input_count, 1);
        assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
        assert_eq!(after_wait_all.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            after_wait_all.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(after_wait_all.ops.wait_request_id, None);
        assert!(!after_wait_all.ops.pending_wait_present);
        assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
        assert!(
            after_wait_all.ops.wait_operation_ids.is_empty(),
            "wait_all should release the tracked wait target before the attached steered completion waiter clears"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .expect("attached steered prompt should finish after the executor is released");

        match completion_handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!(
                "expected attached steered prompt to complete after wait_all settled first, got {other:?}"
            ),
        }

        let settled = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after attached steered completion");
                if snapshot.control.phase == RuntimeState::Attached {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("attached runtime should return to Attached after steered work completes");
        assert_eq!(settled.control.current_run_id, None);
        assert_eq!(settled.inputs.current_run_id, None);
        assert!(settled.inputs.queue.is_empty());
        assert!(settled.inputs.steer_queue.is_empty());
        assert_eq!(settled.completion_waiters.input_count, 0);
        assert_eq!(settled.completion_waiters.waiter_count, 0);
        assert!(settled.completion_waiters.waiting_inputs.is_empty());
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "isolated attached steered completion should not trigger a follow-on continuation run"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            1,
            "isolated attached steered completion should not emit extra executor control commands"
        );
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_attached_steered_prompt_destroy_splits_completion_and_wait_all_lifetimes()
     {
        struct BlockingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));
        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::MobMemberChild,
                owner_session_id: session_id.clone(),
                display_name: "attached steered destroy wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: true,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let input = Input::Prompt(crate::input::PromptInput::new(
            "attached steered destroy split",
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                    ..Default::default()
                },
            ),
        ));
        let input_id = input.id().clone();
        let (outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("attached steered prompt should be accepted");
        assert!(outcome.is_accepted());
        let completion_handle =
            completion_handle.expect("attached steered prompt should expose a completion waiter");

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect("attached steered prompt should request immediate processing");

        let during_apply = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist while attached steered work is active");
        let wait_request_id = during_apply
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(during_apply.control.phase, RuntimeState::Running);
        assert_eq!(
            during_apply.control.current_run_id,
            during_apply.inputs.current_run_id
        );
        assert!(during_apply.control.current_run_id.is_some());
        assert!(during_apply.inputs.queue.is_empty());
        assert!(during_apply.inputs.steer_queue.is_empty());
        assert_eq!(during_apply.completion_waiters.input_count, 1);
        assert_eq!(during_apply.completion_waiters.waiter_count, 1);
        assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            during_apply.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            during_apply.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id while attached steered work is active"
        );
        assert_eq!(
            during_apply.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the live operation while attached steered work is active"
        );
        assert_eq!(apply_calls.load(Ordering::SeqCst), 1);
        assert_eq!(control_calls.load(Ordering::SeqCst), 1);

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
            .await
            .expect("destroy should split attached steered completion and wait_all lifetimes");

        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime destroyed");
            }
            other => panic!(
                "expected attached steered completion waiter to terminate on destroy, got {other:?}"
            ),
        }

        let after_destroy = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after destroy");
        assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
        assert!(after_destroy.inputs.queue.is_empty());
        assert!(after_destroy.inputs.steer_queue.is_empty());
        assert_eq!(after_destroy.completion_waiters.input_count, 0);
        assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
        assert!(
            after_destroy.completion_waiters.waiting_inputs.is_empty(),
            "destroy should clear the steered completion waiter immediately even while apply remains blocked"
        );
        assert_eq!(
            after_destroy.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "destroy should preserve the authority-owned wait request after the steered completion waiter clears"
        );
        assert!(after_destroy.ops.pending_wait_present);
        assert_eq!(
            after_destroy.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "destroy should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_destroy.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "destroy should preserve the tracked wait target until the waited operation settles"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .expect("blocked attached apply should finish once the executor is released");

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after destroy");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Destroyed);
        assert_eq!(settled.control.current_run_id, None);
        assert_eq!(settled.inputs.current_run_id, None);
        assert!(settled.inputs.queue.is_empty());
        assert!(settled.inputs.steer_queue.is_empty());
        assert_eq!(settled.completion_waiters.input_count, 0);
        assert_eq!(settled.completion_waiters.waiter_count, 0);
        assert!(settled.completion_waiters.waiting_inputs.is_empty());
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn interrupt_current_run_returns_not_ready_without_attached_loop() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let err = adapter
            .interrupt_current_run(&session_id)
            .await
            .expect_err("interrupt should reject when no attached loop exists");
        match err {
            RuntimeDriverError::NotReady { state } => {
                assert_eq!(state, RuntimeState::Idle);
            }
            other => panic!("expected NotReady(Idle), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancel_after_boundary_returns_not_ready_without_attached_loop() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let err = adapter
            .cancel_after_boundary(&session_id)
            .await
            .expect_err("boundary cancel should reject when no attached loop exists");
        match err {
            RuntimeDriverError::NotReady { state } => {
                assert_eq!(state, RuntimeState::Idle);
            }
            other => panic!("expected NotReady(Idle), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn interrupt_current_run_on_attached_runtime_is_deferred_until_apply_finishes() {
        struct BlockingExecutor {
            apply_calls: Arc<AtomicUsize>,
            cancel_calls: Arc<AtomicUsize>,
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                if matches!(command, RunControlCommand::CancelCurrentRun { .. }) {
                    self.cancel_calls.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let cancel_calls = Arc::new(AtomicUsize::new(0));
        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    cancel_calls: Arc::clone(&cancel_calls),
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let input = Input::Prompt(crate::input::PromptInput::new(
            "attached steered deferred interrupt",
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                    ..Default::default()
                },
            ),
        ));
        let input_id = input.id().clone();
        let (outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("attached steered prompt should be accepted");
        assert!(outcome.is_accepted());
        let completion_handle =
            completion_handle.expect("attached steered prompt should expose a completion waiter");

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect("attached steered prompt should request immediate processing");

        let during_apply = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist while apply is blocked");
        assert_eq!(during_apply.control.phase, RuntimeState::Running);
        assert_eq!(
            during_apply.control.current_run_id,
            during_apply.inputs.current_run_id
        );
        assert!(during_apply.control.current_run_id.is_some());
        assert!(during_apply.inputs.queue.is_empty());
        assert!(during_apply.inputs.steer_queue.is_empty());
        assert_eq!(during_apply.completion_waiters.input_count, 1);
        assert_eq!(during_apply.completion_waiters.waiter_count, 1);
        assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            during_apply.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "attached steered prompt should start the run exactly once"
        );
        assert_eq!(
            cancel_calls.load(Ordering::SeqCst),
            0,
            "no interrupt should reach the executor before it is requested"
        );

        adapter
            .interrupt_current_run(&session_id)
            .await
            .expect("interrupt should enqueue against the attached loop");

        let after_interrupt = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after interrupt is requested");
        assert_eq!(
            after_interrupt.control.phase,
            RuntimeState::Running,
            "interrupt should stay deferred while the attached executor is still inside apply()"
        );
        assert_eq!(
            after_interrupt.control.current_run_id,
            after_interrupt.inputs.current_run_id
        );
        assert!(after_interrupt.control.current_run_id.is_some());
        assert!(after_interrupt.inputs.queue.is_empty());
        assert!(after_interrupt.inputs.steer_queue.is_empty());
        assert_eq!(after_interrupt.completion_waiters.input_count, 1);
        assert_eq!(after_interrupt.completion_waiters.waiter_count, 1);
        assert_eq!(after_interrupt.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            after_interrupt.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            cancel_calls.load(Ordering::SeqCst),
            0,
            "cancel should remain queued until apply returns"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .expect("apply should finish after the executor is released");

        match completion_handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!(
                "expected attached queued prompt to complete normally before queued cancel drains, got {other:?}"
            ),
        }

        let settled = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after apply returns");
                if snapshot.control.phase == RuntimeState::Attached
                    && snapshot.control.current_run_id.is_none()
                    && snapshot.inputs.current_run_id.is_none()
                    && snapshot.completion_waiters.waiter_count == 0
                    && cancel_calls.load(Ordering::SeqCst) == 1
                {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("attached runtime should eventually return to Attached after queued cancel drains");
        assert!(settled.inputs.queue.is_empty());
        assert!(settled.inputs.steer_queue.is_empty());
        assert_eq!(settled.completion_waiters.input_count, 0);
        assert_eq!(settled.completion_waiters.waiter_count, 0);
        assert!(settled.completion_waiters.waiting_inputs.is_empty());
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "queued cancel should not replay the already-running attached steered turn"
        );
        assert_eq!(
            cancel_calls.load(Ordering::SeqCst),
            1,
            "queued cancel should reach the executor exactly once after apply finishes"
        );
    }

    #[tokio::test]
    async fn cancel_after_boundary_on_attached_runtime_is_deferred_until_apply_finishes() {
        struct BlockingExecutor {
            apply_calls: Arc<AtomicUsize>,
            boundary_cancel_calls: Arc<AtomicUsize>,
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                if matches!(command, RunControlCommand::CancelAfterBoundary { .. }) {
                    self.boundary_cancel_calls.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let boundary_cancel_calls = Arc::new(AtomicUsize::new(0));
        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    boundary_cancel_calls: Arc::clone(&boundary_cancel_calls),
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let input = Input::Prompt(crate::input::PromptInput::new(
            "attached steered deferred boundary cancel",
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                    ..Default::default()
                },
            ),
        ));
        let (outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("attached steered prompt should be accepted");
        assert!(outcome.is_accepted());
        let completion_handle =
            completion_handle.expect("attached steered prompt should expose a completion waiter");

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect("attached steered prompt should request immediate processing");

        adapter
            .cancel_after_boundary(&session_id)
            .await
            .expect("boundary cancel should enqueue against the attached loop");

        let after_request = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after boundary cancel is requested");
        assert_eq!(after_request.control.phase, RuntimeState::Running);
        assert!(after_request.control.current_run_id.is_some());
        assert_eq!(
            boundary_cancel_calls.load(Ordering::SeqCst),
            0,
            "boundary cancel should remain queued until apply returns"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .expect("apply should finish after the executor is released");

        match completion_handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!(
                "expected attached queued prompt to complete normally before queued boundary cancel drains, got {other:?}"
            ),
        }

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after apply returns");
                if snapshot.control.phase == RuntimeState::Attached
                    && snapshot.control.current_run_id.is_none()
                    && snapshot.inputs.current_run_id.is_none()
                    && boundary_cancel_calls.load(Ordering::SeqCst) == 1
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("attached runtime should eventually drain the queued boundary cancel");
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "queued boundary cancel should not replay the already-running attached steered turn"
        );
    }

    #[tokio::test]
    async fn running_peer_message_interrupt_yielding_drains_before_next_apply() {
        struct BlockingThenImmediateExecutor {
            apply_calls: Arc<AtomicUsize>,
            interrupt_calls: Arc<AtomicUsize>,
            events: Arc<std::sync::Mutex<Vec<&'static str>>>,
            first_apply_started: Arc<Notify>,
            allow_first_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingThenImmediateExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                let apply_index = self.apply_calls.fetch_add(1, Ordering::SeqCst);
                if apply_index == 0 {
                    self.events
                        .lock()
                        .expect("events mutex poisoned")
                        .push("apply1_start");
                    self.first_apply_started.notify_waiters();
                    self.allow_first_finish.notified().await;
                    self.events
                        .lock()
                        .expect("events mutex poisoned")
                        .push("apply1_finish");
                } else {
                    self.events
                        .lock()
                        .expect("events mutex poisoned")
                        .push("apply2_start");
                }

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                if matches!(command, RunControlCommand::InterruptYielding) {
                    self.interrupt_calls.fetch_add(1, Ordering::SeqCst);
                    self.events
                        .lock()
                        .expect("events mutex poisoned")
                        .push("interrupt_yielding");
                }
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let interrupt_calls = Arc::new(AtomicUsize::new(0));
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let first_apply_started = Arc::new(Notify::new());
        let allow_first_finish = Arc::new(Notify::new());

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingThenImmediateExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    interrupt_calls: Arc::clone(&interrupt_calls),
                    events: Arc::clone(&events),
                    first_apply_started: Arc::clone(&first_apply_started),
                    allow_first_finish: Arc::clone(&allow_first_finish),
                }),
            )
            .await;

        let first_input = Input::Prompt(crate::input::PromptInput::new(
            "attached running peer interrupt",
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                    ..Default::default()
                },
            ),
        ));
        let first_input_id = first_input.id().clone();
        let (outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, first_input)
            .await
            .expect("attached steered prompt should be accepted");
        assert!(outcome.is_accepted());
        let completion_handle =
            completion_handle.expect("attached steered prompt should expose a completion waiter");

        tokio::time::timeout(Duration::from_secs(1), first_apply_started.notified())
            .await
            .expect("attached steered prompt should request immediate processing");

        interrupt_calls.store(0, Ordering::SeqCst);
        events.lock().expect("events mutex poisoned").clear();

        let peer_input = Input::Peer(crate::input::PeerInput {
            header: crate::input::InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: crate::input::InputOrigin::Peer {
                    peer_id: "peer-interrupt".into(),
                    runtime_id: None,
                },
                durability: crate::input::InputDurability::Durable,
                visibility: crate::input::InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            body: "interrupt while running".into(),
            blocks: None,
            handling_mode: None,
        });
        let peer_input_id = peer_input.id().clone();
        let peer_outcome = adapter
            .accept_input(&session_id, peer_input)
            .await
            .expect("running peer message should be accepted");
        assert!(peer_outcome.is_accepted());

        let after_peer_accept = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist while the first apply is blocked");
        assert_eq!(after_peer_accept.control.phase, RuntimeState::Running);
        assert!(after_peer_accept.control.current_run_id.is_some());
        assert_eq!(
            after_peer_accept.control.current_run_id,
            after_peer_accept.inputs.current_run_id
        );
        assert_eq!(after_peer_accept.inputs.queue.len(), 1);
        assert_eq!(after_peer_accept.inputs.queue[0], peer_input_id);
        assert!(after_peer_accept.inputs.steer_queue.is_empty());
        assert_eq!(after_peer_accept.completion_waiters.input_count, 1);
        assert_eq!(after_peer_accept.completion_waiters.waiter_count, 1);
        assert_eq!(
            after_peer_accept.completion_waiters.waiting_inputs[0].input_id,
            first_input_id
        );
        assert_eq!(
            interrupt_calls.load(Ordering::SeqCst),
            0,
            "interrupt-yielding should remain queued until the running apply returns"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "the running turn should still be on its first apply"
        );
        {
            let event_log = events.lock().expect("events mutex poisoned");
            let first_apply_finish_index =
                event_log.iter().position(|event| *event == "apply1_finish");
            assert!(
                first_apply_finish_index.is_none(),
                "the first apply should still be blocked while interrupt-yielding is queued"
            );
            assert!(
                event_log.is_empty(),
                "no queued control or replay should be observed before the running apply returns: {event_log:?}"
            );
        }

        allow_first_finish.notify_waiters();

        let settled = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist until runtime settles");
                if snapshot.control.phase == RuntimeState::Attached
                    && snapshot.control.current_run_id.is_none()
                    && snapshot.inputs.current_run_id.is_none()
                    && snapshot.inputs.queue.is_empty()
                    && snapshot.inputs.steer_queue.is_empty()
                    && snapshot.completion_waiters.waiter_count == 0
                    && interrupt_calls.load(Ordering::SeqCst) == 1
                    && apply_calls.load(Ordering::SeqCst) == 2
                {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        let settled = match settled {
            Ok(snapshot) => snapshot,
            Err(_) => {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should still exist after timeout");
                let event_log = events.lock().expect("events mutex poisoned").clone();
                panic!(
                    "attached runtime did not settle after interrupt-yielding as expected: phase={:?} control_run={:?} ingress_run={:?} queue={:?} steer_queue={:?} waiters={} interrupt_calls={} apply_calls={} events={:?}",
                    snapshot.control.phase,
                    snapshot.control.current_run_id,
                    snapshot.inputs.current_run_id,
                    snapshot.inputs.queue,
                    snapshot.inputs.steer_queue,
                    snapshot.completion_waiters.waiter_count,
                    interrupt_calls.load(Ordering::SeqCst),
                    apply_calls.load(Ordering::SeqCst),
                    event_log,
                );
            }
        };
        assert_eq!(settled.control.phase, RuntimeState::Attached);
        assert!(settled.inputs.queue.is_empty());
        assert!(settled.inputs.steer_queue.is_empty());
        assert_eq!(settled.completion_waiters.input_count, 0);
        assert_eq!(settled.completion_waiters.waiter_count, 0);

        let (interrupt_index, second_apply_index) = {
            let event_log = events.lock().expect("events mutex poisoned");
            let interrupt_index = event_log
                .iter()
                .position(|event| *event == "interrupt_yielding")
                .expect("interrupt control should be delivered");
            let second_apply_index = event_log
                .iter()
                .position(|event| *event == "apply2_start")
                .expect("queued peer input should eventually start a second apply");
            (interrupt_index, second_apply_index)
        };
        assert!(
            interrupt_index < second_apply_index,
            "interrupt-yielding control must drain before the next queued input starts"
        );

        match completion_handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!(
                "expected first attached steered prompt to complete before queued peer input runs, got {other:?}"
            ),
        }
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_attached_steered_prompt_defers_stop_until_apply_finishes()
     {
        struct BlockingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
            stop_calls: Arc<AtomicUsize>,
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                if matches!(command, RunControlCommand::StopRuntimeExecutor { .. }) {
                    self.stop_calls.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                    stop_calls: Arc::clone(&stop_calls),
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::MobMemberChild,
                owner_session_id: session_id.clone(),
                display_name: "attached steered deferred stop wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: true,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let input = Input::Prompt(crate::input::PromptInput::new(
            "attached steered deferred stop",
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                    ..Default::default()
                },
            ),
        ));
        let input_id = input.id().clone();
        let (outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("attached steered prompt should be accepted");
        assert!(outcome.is_accepted());
        let completion_handle =
            completion_handle.expect("attached steered prompt should expose a completion waiter");

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect("attached steered prompt should request immediate processing");

        let during_apply = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist while attached steered work is active");
        let wait_request_id = during_apply
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(during_apply.control.phase, RuntimeState::Running);
        assert_eq!(
            during_apply.control.current_run_id,
            during_apply.inputs.current_run_id
        );
        assert!(during_apply.control.current_run_id.is_some());
        assert!(during_apply.inputs.queue.is_empty());
        assert!(during_apply.inputs.steer_queue.is_empty());
        assert_eq!(during_apply.completion_waiters.input_count, 1);
        assert_eq!(during_apply.completion_waiters.waiter_count, 1);
        assert_eq!(during_apply.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            during_apply.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            during_apply.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id while attached steered work is active"
        );
        assert_eq!(
            during_apply.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the live operation while attached steered work is active"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "attached steered work should wake the loop exactly once"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            1,
            "attached steered admission should still route one control command through the executor seam"
        );
        assert_eq!(
            stop_calls.load(Ordering::SeqCst),
            0,
            "no explicit stop command should have reached the executor before it is requested"
        );

        adapter
            .stop_runtime_executor(
                &session_id,
                RunControlCommand::StopRuntimeExecutor {
                    reason: "attached steered deferred stop".into(),
                },
            )
            .await
            .expect("stop should queue against the attached loop");

        let after_stop_request = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after stop is requested");
        assert_eq!(
            after_stop_request.control.phase,
            RuntimeState::Running,
            "stop should stay deferred while the attached executor is still inside apply()"
        );
        assert_eq!(
            after_stop_request.control.current_run_id,
            after_stop_request.inputs.current_run_id
        );
        assert!(after_stop_request.control.current_run_id.is_some());
        assert!(after_stop_request.inputs.queue.is_empty());
        assert!(after_stop_request.inputs.steer_queue.is_empty());
        assert_eq!(after_stop_request.completion_waiters.input_count, 1);
        assert_eq!(after_stop_request.completion_waiters.waiter_count, 1);
        assert_eq!(
            after_stop_request.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            after_stop_request.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "stop should not clear the authority-owned wait request while apply is still blocked"
        );
        assert!(after_stop_request.ops.pending_wait_present);
        assert_eq!(
            after_stop_request.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "stop should preserve request-id agreement while it remains queued behind apply()"
        );
        assert_eq!(
            after_stop_request.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "stop should preserve the tracked wait target while apply is still blocked"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            1,
            "the explicit stop command should still be queued instead of reaching the executor mid-apply"
        );
        assert_eq!(
            stop_calls.load(Ordering::SeqCst),
            0,
            "the explicit stop command should not be delivered until the loop drains controls after apply()"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete while stop is still deferred behind apply");
        let wait_result = wait_future
            .await
            .expect("wait_all should still resolve first");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let after_wait_all = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after wait_all settles");
                if snapshot.ops.wait_request_id.is_none() && !snapshot.ops.pending_wait_present {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("attached steered snapshot should eventually clear the wait_all carrier");
        assert_eq!(
            after_wait_all.control.phase,
            RuntimeState::Running,
            "stop should still be deferred while the attached executor remains inside apply()"
        );
        assert_eq!(
            after_wait_all.control.current_run_id,
            after_wait_all.inputs.current_run_id
        );
        assert!(after_wait_all.control.current_run_id.is_some());
        assert!(after_wait_all.inputs.queue.is_empty());
        assert!(after_wait_all.inputs.steer_queue.is_empty());
        assert_eq!(after_wait_all.completion_waiters.input_count, 1);
        assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
        assert_eq!(
            after_wait_all.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(after_wait_all.ops.wait_request_id, None);
        assert!(!after_wait_all.ops.pending_wait_present);
        assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
        assert!(after_wait_all.ops.wait_operation_ids.is_empty());
        assert_eq!(
            stop_calls.load(Ordering::SeqCst),
            0,
            "the queued stop command should still not reach the executor before apply() completes"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .expect("attached steered prompt should finish after the executor is released");

        match completion_handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!(
                "expected attached steered completion to finish normally before queued stop drains, got {other:?}"
            ),
        }

        let settled = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after queued stop drains");
                if snapshot.control.phase == RuntimeState::Stopped {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("attached runtime should eventually publish Stopped after apply returns");
        assert_eq!(settled.control.current_run_id, None);
        assert_eq!(settled.inputs.current_run_id, None);
        assert!(settled.inputs.queue.is_empty());
        assert!(settled.inputs.steer_queue.is_empty());
        assert_eq!(settled.completion_waiters.input_count, 0);
        assert_eq!(settled.completion_waiters.waiter_count, 0);
        assert!(settled.completion_waiters.waiting_inputs.is_empty());
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "deferred stop should not replay the attached steered input"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            2,
            "one admission control plus one deferred stop command should reach the attached executor"
        );
        assert_eq!(
            stop_calls.load(Ordering::SeqCst),
            1,
            "the queued stop command should reach the executor exactly once after apply() finishes"
        );
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_reset_with_runtime_loop()
     {
        struct RecordingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for RecordingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(RecordingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                }),
            )
            .await;

        let input = make_progress_input("reset-with-loop");
        let input_id = input.id().clone();
        let outcome = adapter
            .accept_input_without_wake(&session_id, input)
            .await
            .expect("progress input should queue without waking the attached loop");
        assert!(outcome.is_accepted());

        let handle = {
            let completions = {
                let sessions = adapter.sessions.read().await;
                sessions
                    .get(&session_id)
                    .expect("attached session should exist")
                    .completions
                    .clone()
            };
            let mut completions = completions.lock().await;
            completions.register(input_id.clone())
        };

        let before_reset = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before reset");
        assert_eq!(before_reset.control.phase, RuntimeState::Attached);
        assert_eq!(before_reset.completion_waiters.input_count, 1);
        assert_eq!(before_reset.completion_waiters.waiter_count, 1);
        assert_eq!(before_reset.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_reset.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "reset should be able to discard queued attached-loop work before apply runs"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "reset has not yet attempted any executor control"
        );

        let report = SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
            .await
            .expect("reset should succeed for attached queued runtime");
        assert_eq!(report.inputs_abandoned, 1);

        let after_reset = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after reset");
        assert_eq!(after_reset.control.phase, RuntimeState::Idle);
        assert_eq!(after_reset.completion_waiters.input_count, 0);
        assert_eq!(after_reset.completion_waiters.waiter_count, 0);
        assert!(
            after_reset.completion_waiters.waiting_inputs.is_empty(),
            "reset should clear the completion waiter carrier immediately even when a loop is attached"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "reset should bypass queued attached-loop work entirely"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "reset currently bypasses the executor control seam and does not deliver an out-of-band control command"
        );

        match handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime reset");
            }
            other => panic!("expected runtime reset termination, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_stop_runtime_executor()
    {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = make_prompt("stop completion waiter");
        let input_id = input.id().clone();
        let (_outcome, handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("prompt should be accepted");
        let handle = handle.expect("queued prompt should register a completion waiter");

        let before_stop = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before stop");
        assert_eq!(before_stop.control.phase, RuntimeState::Idle);
        assert_eq!(before_stop.completion_waiters.input_count, 1);
        assert_eq!(before_stop.completion_waiters.waiter_count, 1);
        assert_eq!(before_stop.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_stop.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );

        adapter
            .stop_runtime_executor(
                &session_id,
                RunControlCommand::StopRuntimeExecutor {
                    reason: "stop test".into(),
                },
            )
            .await
            .expect("stop should terminate active completion waiters");

        let after_stop = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after stop");
        assert_eq!(after_stop.control.phase, RuntimeState::Stopped);
        assert!(
            after_stop.inputs.queue.is_empty(),
            "stop should not leave ordinary queued work behind once the runtime is stopped"
        );
        assert!(
            after_stop.inputs.steer_queue.is_empty(),
            "stop should not leave steer-queued work behind once the runtime is stopped"
        );
        assert_eq!(after_stop.completion_waiters.input_count, 0);
        assert_eq!(after_stop.completion_waiters.waiter_count, 0);
        assert!(
            after_stop.completion_waiters.waiting_inputs.is_empty(),
            "stop should clear the completion waiter carrier immediately"
        );

        match handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime stopped");
            }
            other => panic!("expected runtime stopped termination, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_stop_runtime_executor_with_runtime_loop()
     {
        struct RecordingExecutor {
            apply_calls: Arc<AtomicUsize>,
            stop_calls: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for RecordingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                if matches!(command, RunControlCommand::StopRuntimeExecutor { .. }) {
                    self.stop_calls.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let stop_calls = Arc::new(AtomicUsize::new(0));

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(RecordingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    stop_calls: Arc::clone(&stop_calls),
                }),
            )
            .await;

        let input = make_progress_input("stop-with-loop");
        let input_id = input.id().clone();
        let outcome = adapter
            .accept_input_without_wake(&session_id, input)
            .await
            .expect("progress input should queue without waking the attached loop");
        assert!(outcome.is_accepted());

        let handle = {
            let completions = {
                let sessions = adapter.sessions.read().await;
                sessions
                    .get(&session_id)
                    .expect("attached session should exist")
                    .completions
                    .clone()
            };
            let mut completions = completions.lock().await;
            completions.register(input_id.clone())
        };

        let before_stop = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before stop");
        assert_eq!(before_stop.control.phase, RuntimeState::Attached);
        assert_eq!(before_stop.completion_waiters.input_count, 1);
        assert_eq!(before_stop.completion_waiters.waiter_count, 1);
        assert_eq!(before_stop.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_stop.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "stop should be able to preempt queued attached-loop work before apply runs"
        );

        adapter
            .stop_runtime_executor(
                &session_id,
                RunControlCommand::StopRuntimeExecutor {
                    reason: "stop attached-loop completion waiter".into(),
                },
            )
            .await
            .expect(
                "stop should terminate queued completion waiters through the live control seam",
            );

        match handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime stopped");
            }
            other => panic!("expected runtime stopped termination, got {other:?}"),
        }

        let after_stop = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after stop");
        assert_eq!(after_stop.control.phase, RuntimeState::Stopped);
        assert!(
            after_stop.inputs.queue.is_empty(),
            "stop should not leave ordinary queued work behind even when an attached loop exists"
        );
        assert!(
            after_stop.inputs.steer_queue.is_empty(),
            "stop should not leave steer-queued work behind even when an attached loop exists"
        );
        assert_eq!(after_stop.completion_waiters.input_count, 0);
        assert_eq!(after_stop.completion_waiters.waiter_count, 0);
        assert!(
            after_stop.completion_waiters.waiting_inputs.is_empty(),
            "stop through the live loop should clear the completion waiter carrier"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "stop should beat queued ordinary work on an attached runtime loop"
        );
        assert_eq!(
            stop_calls.load(Ordering::SeqCst),
            1,
            "the attached executor should observe exactly one stop-runtime-executor control"
        );
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_clears_completion_waiters_after_retire_without_runtime_loop()
     {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = make_prompt("retire completion waiter");
        let input_id = input.id().clone();
        let (_outcome, handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("prompt should be accepted");
        let handle = handle.expect("queued prompt should register a completion waiter");

        let before_retire = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before retire");
        assert_eq!(before_retire.control.phase, RuntimeState::Idle);
        assert_eq!(before_retire.completion_waiters.input_count, 1);
        assert_eq!(before_retire.completion_waiters.waiter_count, 1);
        assert_eq!(before_retire.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_retire.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
            .await
            .expect("retire should clear queued waiters when no runtime loop can drain");
        assert_eq!(report.inputs_abandoned, 1);
        assert_eq!(report.inputs_pending_drain, 0);

        let after_retire = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after retire");
        assert_eq!(after_retire.control.phase, RuntimeState::Retired);
        assert_eq!(after_retire.completion_waiters.input_count, 0);
        assert_eq!(after_retire.completion_waiters.waiter_count, 0);
        assert!(
            after_retire.completion_waiters.waiting_inputs.is_empty(),
            "retire without a live runtime loop should clear the completion waiter carrier immediately"
        );

        match handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "retired without runtime loop");
            }
            other => panic!("expected retired-without-runtime-loop termination, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_preserves_completion_waiters_after_retire_with_runtime_loop()
     {
        struct BlockingExecutor {
            apply_calls: Arc<AtomicUsize>,
            apply_started: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let apply_started = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    apply_started: Arc::clone(&apply_started),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let input = make_progress_input("retire-with-loop");
        let input_id = input.id().clone();
        let (outcome, handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("progress input should be accepted");
        assert!(outcome.is_accepted());
        let handle = handle.expect("queued progress input should expose a completion waiter");

        let before_retire = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before retire");
        assert_eq!(before_retire.control.phase, RuntimeState::Attached);
        assert_eq!(before_retire.completion_waiters.input_count, 1);
        assert_eq!(before_retire.completion_waiters.waiter_count, 1);
        assert_eq!(before_retire.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_retire.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "queued progress input should remain pending until retire wakes the attached loop"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
            .await
            .expect("retire should preserve queued work for the live runtime loop to drain");
        assert_eq!(report.inputs_abandoned, 0);
        assert_eq!(report.inputs_pending_drain, 1);

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect("retire should wake the attached runtime loop to drain queued work");

        let after_retire = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after retire wakes the loop");
        assert_eq!(
            after_retire.control.phase,
            RuntimeState::Running,
            "retire currently hands preserved queued work to the attached loop, which re-enters Running while draining it"
        );
        assert_eq!(after_retire.completion_waiters.input_count, 1);
        assert_eq!(after_retire.completion_waiters.waiter_count, 1);
        assert_eq!(after_retire.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            after_retire.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "retire should wake the attached loop exactly once for the preserved queued work"
        );

        allow_finish.notify_waiters();

        match handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!("expected retire+drain to complete queued work, got {other:?}"),
        }

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after drained completion settles");
        assert_eq!(settled.control.phase, RuntimeState::Retired);
        assert_eq!(settled.completion_waiters.input_count, 0);
        assert_eq!(settled.completion_waiters.waiter_count, 0);
        assert!(
            settled.completion_waiters.waiting_inputs.is_empty(),
            "retire+drain should clear the completion waiter carrier once the preserved work completes"
        );
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recover_with_runtime_loop()
     {
        struct BlockingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));
        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let input = make_progress_input("recover-with-loop-completion");
        let input_id = input.id().clone();
        let (outcome, handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("progress input should be accepted");
        assert!(outcome.is_accepted());
        let handle = handle.expect("queued progress input should expose a completion waiter");

        let before_recover = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before recover");
        assert_eq!(before_recover.control.phase, RuntimeState::Attached);
        assert_eq!(before_recover.completion_waiters.input_count, 1);
        assert_eq!(before_recover.completion_waiters.waiter_count, 1);
        assert_eq!(before_recover.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_recover.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            before_recover.completion_waiters.waiting_inputs[0].waiter_count,
            1
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "queued attached-loop work should remain pending until recover wakes the loop"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "recover should not yet have attempted executor control"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::recover(&adapter, &runtime_id)
            .await
            .expect("recover should preserve queued completion waiters while replaying attached-loop work");
        assert_eq!(report.inputs_recovered, 1);

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect(
                "recover should wake the attached runtime loop to replay preserved queued work",
            );

        let during_recover = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist while recover replay is in flight");
        assert_eq!(
            during_recover.control.phase,
            RuntimeState::Running,
            "recover currently re-enters Running while the attached loop replays recovered work"
        );
        assert_eq!(during_recover.completion_waiters.input_count, 1);
        assert_eq!(during_recover.completion_waiters.waiter_count, 1);
        assert_eq!(during_recover.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            during_recover.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            during_recover.completion_waiters.waiting_inputs[0].waiter_count,
            1
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "recover should wake the attached loop exactly once for the recovered queued work"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "recover should not route any out-of-band control command through the executor seam"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .expect("attached loop should finish replaying the recovered queued work");

        match handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!("expected recover+replay to complete queued work, got {other:?}"),
        }

        let settled = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after attached-loop replay");
                if snapshot.control.phase != RuntimeState::Running {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect(
            "runtime should eventually leave Running once the recovered work finishes replaying",
        );
        assert_eq!(
            settled.control.phase,
            RuntimeState::Attached,
            "recover should currently return to Attached once the recovered work finishes replaying"
        );
        assert_eq!(settled.completion_waiters.input_count, 0);
        assert_eq!(settled.completion_waiters.waiter_count, 0);
        assert!(
            settled.completion_waiters.waiting_inputs.is_empty(),
            "recover+replay should clear the completion waiter carrier once the preserved work completes"
        );
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_preserves_completion_waiters_after_recycle_with_runtime_loop()
     {
        struct BlockingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));
        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let input = make_progress_input("recycle-with-loop-completion");
        let input_id = input.id().clone();
        let (outcome, handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("progress input should be accepted");
        assert!(outcome.is_accepted());
        let handle = handle.expect("queued progress input should expose a completion waiter");

        let before_recycle = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before recycle");
        assert_eq!(before_recycle.control.phase, RuntimeState::Attached);
        assert_eq!(before_recycle.completion_waiters.input_count, 1);
        assert_eq!(before_recycle.completion_waiters.waiter_count, 1);
        assert_eq!(before_recycle.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_recycle.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            before_recycle.completion_waiters.waiting_inputs[0].waiter_count,
            1
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "queued attached-loop work should remain pending until recycle wakes the loop"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "recycle should not yet have attempted executor control"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::recycle(&adapter, &runtime_id)
            .await
            .expect("recycle should preserve queued completion waiters while replaying attached-loop work");
        assert_eq!(report.inputs_transferred, 1);

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect(
                "recycle should wake the attached runtime loop to replay preserved queued work",
            );

        let during_recycle = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist while recycle replay is in flight");
        assert_eq!(
            during_recycle.control.phase,
            RuntimeState::Running,
            "recycle currently re-enters Running while the attached loop replays preserved work"
        );
        assert_eq!(during_recycle.completion_waiters.input_count, 1);
        assert_eq!(during_recycle.completion_waiters.waiter_count, 1);
        assert_eq!(during_recycle.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            during_recycle.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            during_recycle.completion_waiters.waiting_inputs[0].waiter_count,
            1
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "recycle should wake the attached loop exactly once for the preserved queued work"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "recycle should not route any out-of-band control command through the executor seam"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .expect("attached loop should finish replaying the preserved queued work");

        match handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!("expected recycle+replay to complete queued work, got {other:?}"),
        }

        let settled = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after attached-loop replay");
                if snapshot.control.phase == RuntimeState::Attached {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("runtime should return to Attached once the preserved work finishes replaying");
        assert_eq!(settled.completion_waiters.input_count, 0);
        assert_eq!(settled.completion_waiters.waiter_count, 0);
        assert!(
            settled.completion_waiters.waiting_inputs.is_empty(),
            "recycle+replay should clear the completion waiter carrier once the preserved work completes"
        );
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_tracks_epoch_cursor_state() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let cursor_state = {
            let sessions = adapter.sessions.read().await;
            Arc::clone(
                &sessions
                    .get(&session_id)
                    .expect("registered session should exist")
                    .cursor_state,
            )
        };
        cursor_state
            .agent_applied_cursor
            .store(7, Ordering::Release);
        cursor_state
            .runtime_observed_seq
            .store(11, Ordering::Release);
        cursor_state
            .runtime_last_injected_seq
            .store(13, Ordering::Release);

        let snapshot = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist for registered session");

        assert_eq!(snapshot.binding.cursor_state.agent_applied_cursor, 7);
        assert_eq!(snapshot.binding.cursor_state.runtime_observed_seq, 11);
        assert_eq!(snapshot.binding.cursor_state.runtime_last_injected_seq, 13);
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_tracks_runtime_ops_state() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;
        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "background test op".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");
        registry
            .report_progress(
                &operation_id,
                OperationProgressUpdate {
                    message: "still working".into(),
                    percent: Some(0.5),
                },
            )
            .expect("progress update should be accepted");

        let snapshot = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist for registered session");

        assert_eq!(snapshot.ops.operation_count, 1);
        assert_eq!(snapshot.ops.active_count, 1);
        assert_eq!(snapshot.ops.wait_request_id, None);
        assert!(!snapshot.ops.pending_wait_present);
        assert_eq!(snapshot.ops.pending_wait_request_id, None);
        assert!(snapshot.ops.wait_operation_ids.is_empty());
        assert_eq!(snapshot.ops.detached_wake_pending, None);
        assert_eq!(snapshot.ops.detached_wake_signaled, None);
        assert_eq!(snapshot.ops.operations.len(), 1);

        let op = &snapshot.ops.operations[0];
        assert_eq!(op.id, operation_id);
        assert_eq!(op.kind, OperationKind::BackgroundToolOp);
        assert_eq!(op.display_name, "background test op");
        assert_eq!(op.status.as_str(), "running");
        assert!(!op.peer_ready);
        assert!(op.peer_handle.is_none());
        assert_eq!(op.progress_count, 1);
        assert_eq!(op.watcher_count, 0);
        assert_eq!(op.terminal_outcome, None);
        assert!(op.started_at_ms.is_some());
        assert!(op.completed_at_ms.is_none());
        assert!(op.elapsed_ms.is_none());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_tracks_wait_all_state() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;
        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let snapshot = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist for registered session");

        assert_eq!(snapshot.ops.operation_count, 1);
        assert_eq!(snapshot.ops.active_count, 1);
        let wait_request_id = snapshot
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert!(
            snapshot.ops.pending_wait_present,
            "pending wait carrier should be present while wait_all is active"
        );
        assert_eq!(
            snapshot.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id as the authority"
        );
        assert_eq!(snapshot.ops.wait_operation_ids, vec![operation_id.clone()]);

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete");
        let wait_result = wait_future.await.expect("wait_all should resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(wait_result.satisfied.operation_ids, vec![operation_id]);
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_recover() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;
        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "recover wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_recover = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before recover");
        let wait_request_id = before_recover
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert!(before_recover.ops.pending_wait_present);
        assert_eq!(
            before_recover.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before recover"
        );
        assert_eq!(
            before_recover.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before recover"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::recover(&adapter, &runtime_id)
            .await
            .expect("recover should preserve the active wait_all carrier");
        assert_eq!(report.inputs_recovered, 0);

        let after_recover = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after recover");
        assert_eq!(
            after_recover.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "recover should preserve the authority-owned wait request"
        );
        assert!(
            after_recover.ops.pending_wait_present,
            "recover should preserve the pending wait carrier while the operation remains active"
        );
        assert_eq!(
            after_recover.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "recover should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_recover.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "recover should preserve the tracked wait targets"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after recover");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(
            settled.control.current_run_id, None,
            "recycle should not leave a settled control-side current-run binding behind"
        );
        assert_eq!(
            settled.inputs.current_run_id, None,
            "recycle should not leave a settled ingress-side current-run binding behind"
        );
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_recover_splits_completion_and_wait_all_lifetimes() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;
        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let input = make_prompt("recover split lifetimes");
        let input_id = input.id().clone();
        let (_outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("prompt should be accepted");
        let completion_handle =
            completion_handle.expect("queued prompt should register a completion waiter");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "recover split wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_recover = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before recover");
        let wait_request_id = before_recover
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_recover.control.phase, RuntimeState::Idle);
        assert_eq!(before_recover.inputs.queue, vec![input_id.clone()]);
        assert_eq!(before_recover.completion_waiters.input_count, 1);
        assert_eq!(before_recover.completion_waiters.waiter_count, 1);
        assert_eq!(before_recover.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_recover.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert!(before_recover.ops.pending_wait_present);
        assert_eq!(
            before_recover.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before recover"
        );
        assert_eq!(
            before_recover.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before recover"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::recover(&adapter, &runtime_id)
            .await
            .expect("recover should preserve both queued input and active wait_all");
        assert_eq!(report.inputs_recovered, 1);

        let after_recover = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after recover");
        assert_eq!(after_recover.control.phase, RuntimeState::Idle);
        assert_eq!(after_recover.inputs.queue, vec![input_id.clone()]);
        assert_eq!(after_recover.completion_waiters.input_count, 1);
        assert_eq!(after_recover.completion_waiters.waiter_count, 1);
        assert_eq!(after_recover.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            after_recover.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            after_recover.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "recover should preserve the authority-owned wait request"
        );
        assert!(after_recover.ops.pending_wait_present);
        assert_eq!(
            after_recover.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "recover should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_recover.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "recover should preserve the tracked wait target"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after recover");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let after_wait_all = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(after_wait_all.control.phase, RuntimeState::Idle);
        assert_eq!(
            after_wait_all.control.current_run_id, None,
            "recover should not leave a settled control-side current-run binding behind"
        );
        assert_eq!(
            after_wait_all.inputs.current_run_id, None,
            "recover should not leave a settled ingress-side current-run binding behind"
        );
        assert_eq!(after_wait_all.inputs.queue, vec![input_id.clone()]);
        assert_eq!(after_wait_all.completion_waiters.input_count, 1);
        assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
        assert_eq!(after_wait_all.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            after_wait_all.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(after_wait_all.ops.wait_request_id, None);
        assert!(!after_wait_all.ops.pending_wait_present);
        assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
        assert!(after_wait_all.ops.wait_operation_ids.is_empty());

        SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
            .await
            .expect("reset should terminate the preserved completion waiter at test end");
        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime reset");
            }
            other => panic!("expected runtime reset termination, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_recover_preserves_steered_input_and_wait_all() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;
        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let input = Input::Prompt(crate::input::PromptInput::new(
            "recover steered prompt",
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                    ..Default::default()
                },
            ),
        ));
        let input_id = input.id().clone();
        let (_outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("steered prompt should be accepted");
        let completion_handle =
            completion_handle.expect("steered prompt should register a completion waiter");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "recover steered wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_recover = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before recover");
        let wait_request_id = before_recover
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_recover.control.phase, RuntimeState::Idle);
        assert!(before_recover.inputs.queue.is_empty());
        assert_eq!(before_recover.inputs.steer_queue, vec![input_id.clone()]);
        assert!(before_recover.inputs.wake_requested);
        assert!(before_recover.inputs.process_requested);
        assert_eq!(before_recover.completion_waiters.input_count, 1);
        assert_eq!(before_recover.completion_waiters.waiter_count, 1);
        assert_eq!(before_recover.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_recover.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            before_recover.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before recover"
        );
        assert_eq!(
            before_recover.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before recover"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::recover(&adapter, &runtime_id)
            .await
            .expect("recover should preserve steered input and active wait_all");
        assert_eq!(report.inputs_recovered, 1);

        let after_recover = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after recover");
        assert_eq!(after_recover.control.phase, RuntimeState::Idle);
        assert!(after_recover.inputs.queue.is_empty());
        assert_eq!(after_recover.inputs.steer_queue, vec![input_id.clone()]);
        assert!(after_recover.inputs.wake_requested);
        assert!(after_recover.inputs.process_requested);
        assert_eq!(after_recover.completion_waiters.input_count, 1);
        assert_eq!(after_recover.completion_waiters.waiter_count, 1);
        assert_eq!(after_recover.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            after_recover.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            after_recover.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "recover should preserve the authority-owned wait request"
        );
        assert!(after_recover.ops.pending_wait_present);
        assert_eq!(
            after_recover.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "recover should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_recover.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "recover should preserve the tracked wait target"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after recover");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let after_wait_all = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(after_wait_all.control.phase, RuntimeState::Idle);
        assert_eq!(after_wait_all.control.current_run_id, None);
        assert_eq!(after_wait_all.inputs.current_run_id, None);
        assert!(after_wait_all.inputs.queue.is_empty());
        assert_eq!(after_wait_all.inputs.steer_queue, vec![input_id.clone()]);
        assert!(after_wait_all.inputs.wake_requested);
        assert!(after_wait_all.inputs.process_requested);
        assert_eq!(after_wait_all.completion_waiters.input_count, 1);
        assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
        assert_eq!(after_wait_all.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            after_wait_all.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(after_wait_all.ops.wait_request_id, None);
        assert!(!after_wait_all.ops.pending_wait_present);
        assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
        assert!(after_wait_all.ops.wait_operation_ids.is_empty());

        SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
            .await
            .expect("reset should terminate the preserved steered completion waiter at test end");
        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime reset");
            }
            other => panic!("expected runtime reset termination, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_recycle_preserves_steered_input_and_wait_all() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;
        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let input = Input::Prompt(crate::input::PromptInput::new(
            "recycle steered prompt",
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                    ..Default::default()
                },
            ),
        ));
        let input_id = input.id().clone();
        let (_outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("steered prompt should be accepted");
        let completion_handle =
            completion_handle.expect("steered prompt should register a completion waiter");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "recycle steered wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_recycle = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before recycle");
        let wait_request_id = before_recycle
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_recycle.control.phase, RuntimeState::Idle);
        assert!(before_recycle.inputs.queue.is_empty());
        assert_eq!(before_recycle.inputs.steer_queue, vec![input_id.clone()]);
        assert!(before_recycle.inputs.wake_requested);
        assert!(before_recycle.inputs.process_requested);
        assert_eq!(before_recycle.completion_waiters.input_count, 1);
        assert_eq!(before_recycle.completion_waiters.waiter_count, 1);
        assert_eq!(before_recycle.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_recycle.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            before_recycle.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before recycle"
        );
        assert_eq!(
            before_recycle.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before recycle"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::recycle(&adapter, &runtime_id)
            .await
            .expect("recycle should preserve steered input and active wait_all");
        assert_eq!(report.inputs_transferred, 1);

        let after_recycle = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after recycle");
        assert_eq!(after_recycle.control.phase, RuntimeState::Idle);
        assert!(after_recycle.inputs.queue.is_empty());
        assert_eq!(after_recycle.inputs.steer_queue, vec![input_id.clone()]);
        assert!(after_recycle.inputs.wake_requested);
        assert!(after_recycle.inputs.process_requested);
        assert_eq!(after_recycle.completion_waiters.input_count, 1);
        assert_eq!(after_recycle.completion_waiters.waiter_count, 1);
        assert_eq!(after_recycle.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            after_recycle.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            after_recycle.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "recycle should preserve the authority-owned wait request"
        );
        assert!(after_recycle.ops.pending_wait_present);
        assert_eq!(
            after_recycle.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "recycle should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_recycle.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "recycle should preserve the tracked wait target"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after recycle");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let after_wait_all = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(after_wait_all.control.phase, RuntimeState::Idle);
        assert_eq!(after_wait_all.control.current_run_id, None);
        assert_eq!(after_wait_all.inputs.current_run_id, None);
        assert!(after_wait_all.inputs.queue.is_empty());
        assert_eq!(after_wait_all.inputs.steer_queue, vec![input_id.clone()]);
        assert!(after_wait_all.inputs.wake_requested);
        assert!(after_wait_all.inputs.process_requested);
        assert_eq!(after_wait_all.completion_waiters.input_count, 1);
        assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
        assert_eq!(after_wait_all.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            after_wait_all.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(after_wait_all.ops.wait_request_id, None);
        assert!(!after_wait_all.ops.pending_wait_present);
        assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
        assert!(after_wait_all.ops.wait_operation_ids.is_empty());

        SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
            .await
            .expect("reset should terminate the preserved steered completion waiter at test end");
        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime reset");
            }
            other => panic!("expected runtime reset termination, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_recycle() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;
        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "recycle wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_recycle = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before recycle");
        let wait_request_id = before_recycle
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert!(before_recycle.ops.pending_wait_present);
        assert_eq!(
            before_recycle.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before recycle"
        );
        assert_eq!(
            before_recycle.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before recycle"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::recycle(&adapter, &runtime_id)
            .await
            .expect("recycle should preserve the active wait_all carrier");
        assert_eq!(report.inputs_transferred, 0);

        let after_recycle = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after recycle");
        assert_eq!(
            after_recycle.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "recycle should preserve the authority-owned wait request"
        );
        assert!(
            after_recycle.ops.pending_wait_present,
            "recycle should preserve the pending wait carrier while the operation remains active"
        );
        assert_eq!(
            after_recycle.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "recycle should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_recycle.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "recycle should preserve the tracked wait targets"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after recycle");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_recycle_splits_completion_and_wait_all_lifetimes() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;
        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let input = make_prompt("recycle split lifetimes");
        let input_id = input.id().clone();
        let (_outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("prompt should be accepted");
        let completion_handle =
            completion_handle.expect("queued prompt should register a completion waiter");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "recycle split wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_recycle = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before recycle");
        let wait_request_id = before_recycle
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_recycle.control.phase, RuntimeState::Idle);
        assert_eq!(before_recycle.inputs.queue, vec![input_id.clone()]);
        assert_eq!(before_recycle.completion_waiters.input_count, 1);
        assert_eq!(before_recycle.completion_waiters.waiter_count, 1);
        assert_eq!(before_recycle.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_recycle.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert!(before_recycle.ops.pending_wait_present);
        assert_eq!(
            before_recycle.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before recycle"
        );
        assert_eq!(
            before_recycle.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before recycle"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::recycle(&adapter, &runtime_id)
            .await
            .expect("recycle should preserve both queued input and active wait_all");
        assert_eq!(report.inputs_transferred, 1);

        let after_recycle = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after recycle");
        assert_eq!(after_recycle.control.phase, RuntimeState::Idle);
        assert_eq!(after_recycle.inputs.queue, vec![input_id.clone()]);
        assert_eq!(after_recycle.completion_waiters.input_count, 1);
        assert_eq!(after_recycle.completion_waiters.waiter_count, 1);
        assert_eq!(after_recycle.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            after_recycle.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            after_recycle.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "recycle should preserve the authority-owned wait request"
        );
        assert!(after_recycle.ops.pending_wait_present);
        assert_eq!(
            after_recycle.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "recycle should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_recycle.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "recycle should preserve the tracked wait target"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after recycle");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let after_wait_all = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(after_wait_all.control.phase, RuntimeState::Idle);
        assert_eq!(after_wait_all.inputs.queue, vec![input_id.clone()]);
        assert_eq!(after_wait_all.completion_waiters.input_count, 1);
        assert_eq!(after_wait_all.completion_waiters.waiter_count, 1);
        assert_eq!(after_wait_all.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            after_wait_all.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(after_wait_all.ops.wait_request_id, None);
        assert!(!after_wait_all.ops.pending_wait_present);
        assert_eq!(after_wait_all.ops.pending_wait_request_id, None);
        assert!(after_wait_all.ops.wait_operation_ids.is_empty());

        SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
            .await
            .expect("reset should terminate the preserved completion waiter at test end");
        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime reset");
            }
            other => panic!("expected runtime reset termination, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_recover_with_runtime_loop() {
        struct BlockingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));
        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let outcome = adapter
            .accept_input_without_wake(
                &session_id,
                make_progress_input("recover-wait-all-with-loop"),
            )
            .await
            .expect("queued progress input should be accepted without waking the loop");
        assert!(outcome.is_accepted());

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "recover wait target with loop".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_recover = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before recover");
        let wait_request_id = before_recover
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_recover.control.phase, RuntimeState::Attached);
        assert!(before_recover.ops.pending_wait_present);
        assert_eq!(
            before_recover.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before recover"
        );
        assert_eq!(
            before_recover.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before recover"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "queued attached-loop work should remain pending until recover wakes the loop"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "recover should not yet have attempted executor control"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::recover(&adapter, &runtime_id)
            .await
            .expect("recover should preserve wait_all while replaying attached-loop work");
        assert_eq!(report.inputs_recovered, 1);

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect("recover should wake the attached runtime loop to replay preserved work");

        let during_recover = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist while recover replay is in flight");
        assert_eq!(
            during_recover.control.phase,
            RuntimeState::Running,
            "recover currently re-enters Running while the attached loop replays recovered work"
        );
        assert_eq!(
            during_recover.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "recover should preserve the authority-owned wait request while replay is in flight"
        );
        assert!(during_recover.ops.pending_wait_present);
        assert_eq!(
            during_recover.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "recover should preserve request-id agreement across the wait carrier seam while replaying"
        );
        assert_eq!(
            during_recover.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "recover should preserve the tracked wait target while recovered work is replaying"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "recover should wake the attached loop exactly once for the recovered queued work"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "recover should not route any out-of-band control command through the executor seam"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .expect("attached loop should finish replaying the recovered queued work");

        let after_replay = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after attached-loop replay");
                if snapshot.control.phase != RuntimeState::Running {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect(
            "runtime should eventually leave Running once the recovered work finishes replaying",
        );
        assert_eq!(
            after_replay.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "wait_all should remain active after the attached loop finishes replaying recovered work"
        );
        assert!(after_replay.ops.pending_wait_present);
        assert_eq!(
            after_replay.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "request-id agreement should survive the return to Idle after replay"
        );
        assert_eq!(
            after_replay.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "the tracked wait target should remain present until the operation itself settles"
        );
        assert_eq!(
            after_replay.control.phase,
            RuntimeState::Attached,
            "recover should currently return to Attached once the recovered work finishes replaying"
        );
        assert!(
            after_replay.inputs.queue.is_empty(),
            "recover should not leave ordinary queued work behind once the attached runtime returns to Attached after replay"
        );
        assert!(
            after_replay.inputs.steer_queue.is_empty(),
            "recover should not leave steer-queued work behind once the attached runtime returns to Attached after replay"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after recover+replay");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Attached);
        assert_eq!(
            settled.control.current_run_id, None,
            "recover should not leave a settled control-side current-run binding behind even on attached runtimes"
        );
        assert_eq!(
            settled.inputs.current_run_id, None,
            "recover should not leave a settled ingress-side current-run binding behind even on attached runtimes"
        );
        assert!(
            settled.inputs.queue.is_empty(),
            "recover should keep the attached settled snapshot free of ordinary queued work after replay completes"
        );
        assert!(
            settled.inputs.steer_queue.is_empty(),
            "recover should keep the attached settled snapshot free of steer-queued work after replay completes"
        );
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_recover_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
     {
        struct BlockingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));
        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let input = make_progress_input("recover-with-loop-split-lifetimes");
        let input_id = input.id().clone();
        let (outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("progress input should be accepted");
        assert!(outcome.is_accepted());
        let completion_handle =
            completion_handle.expect("queued progress input should expose a completion waiter");

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "recover split wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_recover = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before recover");
        let wait_request_id = before_recover
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_recover.control.phase, RuntimeState::Attached);
        assert_eq!(before_recover.completion_waiters.input_count, 1);
        assert_eq!(before_recover.completion_waiters.waiter_count, 1);
        assert_eq!(before_recover.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_recover.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert!(before_recover.ops.pending_wait_present);
        assert_eq!(
            before_recover.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before recover"
        );
        assert_eq!(
            before_recover.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before recover"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "queued attached-loop work should remain pending until recover wakes the loop"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "recover should not yet have attempted executor control"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::recover(&adapter, &runtime_id)
            .await
            .expect("recover should split completion and wait_all lifetimes on attached runtimes");
        assert_eq!(report.inputs_recovered, 1);

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect("recover should wake the attached runtime loop to replay recovered work");

        let during_recover = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist while recover replay is in flight");
        assert_eq!(
            during_recover.control.phase,
            RuntimeState::Running,
            "recover currently re-enters Running while the attached loop replays recovered work"
        );
        assert_eq!(during_recover.completion_waiters.input_count, 1);
        assert_eq!(during_recover.completion_waiters.waiter_count, 1);
        assert_eq!(during_recover.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            during_recover.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            during_recover.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "recover should preserve the authority-owned wait request while replay is in flight"
        );
        assert!(during_recover.ops.pending_wait_present);
        assert_eq!(
            during_recover.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "recover should preserve request-id agreement across the wait carrier seam while replaying"
        );
        assert_eq!(
            during_recover.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "recover should preserve the tracked wait target while recovered work is replaying"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "recover should wake the attached loop exactly once for the recovered queued work"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "recover should not route any out-of-band control command through the executor seam"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .expect("attached loop should finish replaying the recovered queued work");

        match completion_handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!("expected recover+replay to complete queued work, got {other:?}"),
        }

        let after_replay = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after attached-loop replay");
                if snapshot.control.phase != RuntimeState::Running {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect(
            "runtime should eventually leave Running once the recovered work finishes replaying",
        );
        assert_eq!(
            after_replay.control.phase,
            RuntimeState::Attached,
            "recover should currently return to Attached once the recovered work finishes replaying"
        );
        assert_eq!(after_replay.completion_waiters.input_count, 0);
        assert_eq!(after_replay.completion_waiters.waiter_count, 0);
        assert!(
            after_replay.completion_waiters.waiting_inputs.is_empty(),
            "recover should clear completion waiters once the recovered work finishes replaying"
        );
        assert_eq!(
            after_replay.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "wait_all should remain active after the recovered work finishes replaying"
        );
        assert!(after_replay.ops.pending_wait_present);
        assert_eq!(
            after_replay.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "request-id agreement should survive the return to Attached after replay"
        );
        assert_eq!(
            after_replay.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "the tracked wait target should remain present until the operation itself settles"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after recover+replay");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Attached);
        assert_eq!(
            settled.control.current_run_id, None,
            "recycle should not leave a settled control-side current-run binding behind even on attached runtimes"
        );
        assert_eq!(
            settled.inputs.current_run_id, None,
            "recycle should not leave a settled ingress-side current-run binding behind even on attached runtimes"
        );
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_recycle_with_runtime_loop() {
        struct BlockingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));
        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let outcome = adapter
            .accept_input_without_wake(
                &session_id,
                make_progress_input("recycle-wait-all-with-loop"),
            )
            .await
            .expect("queued progress input should be accepted without waking the loop");
        assert!(outcome.is_accepted());

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "recycle wait target with loop".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_recycle = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before recycle");
        let wait_request_id = before_recycle
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_recycle.control.phase, RuntimeState::Attached);
        assert!(before_recycle.ops.pending_wait_present);
        assert_eq!(
            before_recycle.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before recycle"
        );
        assert_eq!(
            before_recycle.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before recycle"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "queued attached-loop work should remain pending until recycle wakes the loop"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "recycle should not yet have attempted executor control"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::recycle(&adapter, &runtime_id)
            .await
            .expect("recycle should preserve wait_all while requeueing attached-loop work");
        assert_eq!(report.inputs_transferred, 1);

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect("recycle should wake the attached runtime loop to replay preserved work");

        let after_recycle = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after recycle wakes the loop");
        assert_eq!(
            after_recycle.control.phase,
            RuntimeState::Running,
            "recycle currently re-enters Running while the attached loop replays preserved work"
        );
        assert_eq!(
            after_recycle.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "recycle should preserve the authority-owned wait request while the attached loop is replaying preserved work"
        );
        assert!(after_recycle.ops.pending_wait_present);
        assert_eq!(
            after_recycle.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "recycle should preserve request-id agreement across the wait carrier seam while replaying"
        );
        assert_eq!(
            after_recycle.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "recycle should preserve the tracked wait target while preserved work is replaying"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "recycle should wake the attached loop exactly once for the preserved queued work"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "recycle should not route any out-of-band control command through the executor seam"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .expect("attached loop should finish replaying the preserved queued work");

        let after_replay = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after attached-loop replay");
                if snapshot.control.phase == RuntimeState::Attached {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("runtime should return to Attached once the preserved work finishes replaying");
        assert_eq!(
            after_replay.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "wait_all should remain active after the attached loop finishes replaying preserved work"
        );
        assert!(after_replay.ops.pending_wait_present);
        assert_eq!(
            after_replay.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "request-id agreement should survive the return to Attached after replay"
        );
        assert_eq!(
            after_replay.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "the tracked wait target should remain present until the operation itself settles"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after recycle+replay");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Attached);
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_recycle_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
     {
        struct BlockingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));
        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let input = make_progress_input("recycle-with-loop-split-lifetimes");
        let input_id = input.id().clone();
        let (outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("progress input should be accepted");
        assert!(outcome.is_accepted());
        let completion_handle =
            completion_handle.expect("queued progress input should expose a completion waiter");

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "recycle split wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_recycle = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before recycle");
        let wait_request_id = before_recycle
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_recycle.control.phase, RuntimeState::Attached);
        assert_eq!(before_recycle.completion_waiters.input_count, 1);
        assert_eq!(before_recycle.completion_waiters.waiter_count, 1);
        assert_eq!(before_recycle.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_recycle.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert!(before_recycle.ops.pending_wait_present);
        assert_eq!(
            before_recycle.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before recycle"
        );
        assert_eq!(
            before_recycle.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before recycle"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "queued attached-loop work should remain pending until recycle wakes the loop"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "recycle should not yet have attempted executor control"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::recycle(&adapter, &runtime_id)
            .await
            .expect("recycle should split completion and wait_all lifetimes on attached runtimes");
        assert_eq!(report.inputs_transferred, 1);

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect("recycle should wake the attached runtime loop to replay preserved work");

        let during_recycle = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist while recycle replay is in flight");
        assert_eq!(
            during_recycle.control.phase,
            RuntimeState::Running,
            "recycle currently re-enters Running while the attached loop replays preserved work"
        );
        assert_eq!(during_recycle.completion_waiters.input_count, 1);
        assert_eq!(during_recycle.completion_waiters.waiter_count, 1);
        assert_eq!(during_recycle.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            during_recycle.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            during_recycle.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "recycle should preserve the authority-owned wait request while replay is in flight"
        );
        assert!(during_recycle.ops.pending_wait_present);
        assert_eq!(
            during_recycle.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "recycle should preserve request-id agreement across the wait carrier seam while replaying"
        );
        assert_eq!(
            during_recycle.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "recycle should preserve the tracked wait target while preserved work is replaying"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "recycle should wake the attached loop exactly once for the preserved queued work"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "recycle should not route any out-of-band control command through the executor seam"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .expect("attached loop should finish replaying the preserved queued work");

        match completion_handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!("expected recycle+replay to complete queued work, got {other:?}"),
        }

        let after_replay = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after attached-loop replay");
                if snapshot.control.phase == RuntimeState::Attached {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("runtime should return to Attached once the preserved work finishes replaying");
        assert_eq!(after_replay.completion_waiters.input_count, 0);
        assert_eq!(after_replay.completion_waiters.waiter_count, 0);
        assert!(
            after_replay.completion_waiters.waiting_inputs.is_empty(),
            "recycle should clear completion waiters once the preserved work finishes replaying"
        );
        assert!(
            after_replay.inputs.queue.is_empty(),
            "recycle should not leave ordinary queued work behind once the attached runtime returns to Attached after replay"
        );
        assert!(
            after_replay.inputs.steer_queue.is_empty(),
            "recycle should not leave steer-queued work behind once the attached runtime returns to Attached after replay"
        );
        assert_eq!(
            after_replay.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "wait_all should remain active after the preserved work finishes replaying"
        );
        assert!(after_replay.ops.pending_wait_present);
        assert_eq!(
            after_replay.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "request-id agreement should survive the return to Attached after replay"
        );
        assert_eq!(
            after_replay.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "the tracked wait target should remain present until the operation itself settles"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after recycle+replay");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Attached);
        assert!(
            settled.inputs.queue.is_empty(),
            "recycle should keep the attached settled snapshot free of ordinary queued work after replay completes"
        );
        assert!(
            settled.inputs.steer_queue.is_empty(),
            "recycle should keep the attached settled snapshot free of steer-queued work after replay completes"
        );
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_reset() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;
        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "reset wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_reset = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before reset");
        let wait_request_id = before_reset
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert!(before_reset.ops.pending_wait_present);
        assert_eq!(
            before_reset.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before reset"
        );
        assert_eq!(
            before_reset.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before reset"
        );

        SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
            .await
            .expect("reset should succeed while the runtime is idle");

        let after_reset = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after reset");
        assert_eq!(
            after_reset.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "reset should preserve the authority-owned wait request"
        );
        assert!(
            after_reset.ops.pending_wait_present,
            "reset should preserve the pending wait carrier while the operation remains active"
        );
        assert_eq!(
            after_reset.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "reset should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_reset.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "reset should preserve the tracked wait targets"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after reset");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_reset_clears_steered_waiter_and_queue_but_preserves_wait_all()
     {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = Input::Prompt(crate::input::PromptInput::new(
            "reset steered prompt",
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                    ..Default::default()
                },
            ),
        ));
        let input_id = input.id().clone();
        let (_outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("steered prompt should be accepted");
        let completion_handle =
            completion_handle.expect("steered prompt should register a completion waiter");

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "reset steered wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_reset = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before reset");
        let wait_request_id = before_reset
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_reset.control.phase, RuntimeState::Idle);
        assert!(before_reset.inputs.queue.is_empty());
        assert_eq!(before_reset.inputs.steer_queue, vec![input_id.clone()]);
        assert!(before_reset.inputs.wake_requested);
        assert!(before_reset.inputs.process_requested);
        assert_eq!(before_reset.completion_waiters.input_count, 1);
        assert_eq!(before_reset.completion_waiters.waiter_count, 1);
        assert_eq!(before_reset.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_reset.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert!(before_reset.ops.pending_wait_present);
        assert_eq!(
            before_reset.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before reset"
        );
        assert_eq!(
            before_reset.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before reset"
        );

        let report = SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
            .await
            .expect("reset should clear steered completion waiters while preserving wait_all");
        assert_eq!(report.inputs_abandoned, 1);

        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime reset");
            }
            other => {
                panic!("expected runtime reset termination for steered input, got {other:?}")
            }
        }

        let after_reset = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after reset");
        assert_eq!(after_reset.control.phase, RuntimeState::Idle);
        assert_eq!(after_reset.control.current_run_id, None);
        assert_eq!(after_reset.inputs.current_run_id, None);
        assert!(after_reset.inputs.queue.is_empty());
        assert!(
            after_reset.inputs.steer_queue.is_empty(),
            "reset should clear steered queued work immediately on the plain runtime path"
        );
        assert_eq!(after_reset.completion_waiters.input_count, 0);
        assert_eq!(after_reset.completion_waiters.waiter_count, 0);
        assert!(
            after_reset.completion_waiters.waiting_inputs.is_empty(),
            "reset should clear input-owned steered completion waiters immediately"
        );
        assert_eq!(
            after_reset.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "reset should preserve the authority-owned wait request after steered completion waiters clear"
        );
        assert!(after_reset.ops.pending_wait_present);
        assert_eq!(
            after_reset.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "reset should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_reset.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "reset should preserve the tracked wait target until the operation settles"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after reset");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Idle);
        assert_eq!(settled.control.current_run_id, None);
        assert_eq!(settled.inputs.current_run_id, None);
        assert!(settled.inputs.queue.is_empty());
        assert!(settled.inputs.steer_queue.is_empty());
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_reset_splits_completion_and_wait_all_lifetimes() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = make_prompt("reset split lifetimes");
        let input_id = input.id().clone();
        let (_outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("prompt should be accepted");
        let completion_handle =
            completion_handle.expect("queued prompt should register a completion waiter");

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "reset split wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_reset = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before reset");
        let wait_request_id = before_reset
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_reset.control.phase, RuntimeState::Idle);
        assert_eq!(before_reset.completion_waiters.input_count, 1);
        assert_eq!(before_reset.completion_waiters.waiter_count, 1);
        assert_eq!(before_reset.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_reset.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert!(before_reset.ops.pending_wait_present);
        assert_eq!(
            before_reset.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before reset"
        );
        assert_eq!(
            before_reset.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before reset"
        );

        let report = SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
            .await
            .expect("reset should split completion and wait_all lifetimes");
        assert_eq!(report.inputs_abandoned, 1);

        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime reset");
            }
            other => panic!("expected runtime reset termination, got {other:?}"),
        }

        let after_reset = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after reset");
        assert_eq!(after_reset.control.phase, RuntimeState::Idle);
        assert!(
            after_reset.inputs.queue.is_empty(),
            "reset should abandon ordinary queued work immediately on the plain runtime path"
        );
        assert!(
            after_reset.inputs.steer_queue.is_empty(),
            "reset should abandon steered queued work immediately on the plain runtime path"
        );
        assert_eq!(after_reset.completion_waiters.input_count, 0);
        assert_eq!(after_reset.completion_waiters.waiter_count, 0);
        assert!(
            after_reset.completion_waiters.waiting_inputs.is_empty(),
            "reset should clear input-owned completion waiters immediately"
        );
        assert_eq!(
            after_reset.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "reset should preserve the authority-owned wait request after completion waiters clear"
        );
        assert!(after_reset.ops.pending_wait_present);
        assert_eq!(
            after_reset.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "reset should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_reset.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "reset should preserve the tracked wait target until the operation settles"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after reset");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Idle);
        assert!(
            settled.inputs.queue.is_empty(),
            "reset should not reintroduce ordinary queued work once the plain runtime settles"
        );
        assert!(
            settled.inputs.steer_queue.is_empty(),
            "reset should not reintroduce steered queued work once the plain runtime settles"
        );
        assert_eq!(
            settled.control.current_run_id, None,
            "reset should not leave a settled control-side current-run binding behind"
        );
        assert_eq!(
            settled.inputs.current_run_id, None,
            "reset should not leave a settled ingress-side current-run binding behind"
        );
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_reset_with_runtime_loop() {
        struct RecordingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for RecordingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(RecordingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                }),
            )
            .await;

        let outcome = adapter
            .accept_input_without_wake(&session_id, make_progress_input("reset-wait-all-with-loop"))
            .await
            .expect("queued progress input should be accepted without waking the loop");
        assert!(outcome.is_accepted());

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "reset wait target with loop".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_reset = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before reset");
        let wait_request_id = before_reset
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_reset.control.phase, RuntimeState::Attached);
        assert!(before_reset.ops.pending_wait_present);
        assert_eq!(
            before_reset.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before reset"
        );
        assert_eq!(
            before_reset.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before reset"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "reset should be able to discard queued attached-loop work before apply runs"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "reset has not yet attempted any executor control"
        );

        let report = SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
            .await
            .expect("reset should preserve wait_all while abandoning queued attached-loop work");
        assert_eq!(report.inputs_abandoned, 1);

        let after_reset = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after reset");
        assert_eq!(after_reset.control.phase, RuntimeState::Idle);
        assert!(
            after_reset.inputs.queue.is_empty(),
            "attached reset should abandon ordinary queued work immediately"
        );
        assert!(
            after_reset.inputs.steer_queue.is_empty(),
            "attached reset should abandon steered queued work immediately"
        );
        assert_eq!(
            after_reset.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "reset should preserve the authority-owned wait request even with an attached loop"
        );
        assert!(
            after_reset.ops.pending_wait_present,
            "reset should preserve the pending wait carrier while the operation remains active"
        );
        assert_eq!(
            after_reset.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "reset should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_reset.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "reset should preserve the tracked wait targets until the operation settles"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "reset should bypass queued attached-loop work entirely"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "reset currently bypasses the executor control seam and does not deliver an out-of-band control command"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after reset");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Idle);
        assert!(
            settled.inputs.queue.is_empty(),
            "attached reset should not reintroduce ordinary queued work once the runtime settles"
        );
        assert!(
            settled.inputs.steer_queue.is_empty(),
            "attached reset should not reintroduce steered queued work once the runtime settles"
        );
        assert_eq!(
            settled.control.current_run_id, None,
            "reset should not leave a settled control-side current-run binding behind even on attached runtimes"
        );
        assert_eq!(
            settled.inputs.current_run_id, None,
            "reset should not leave a settled ingress-side current-run binding behind even on attached runtimes"
        );
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_reset_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
     {
        struct RecordingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for RecordingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(RecordingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                }),
            )
            .await;

        let input = make_progress_input("reset-with-loop-split-lifetimes");
        let input_id = input.id().clone();
        let (outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("progress input should be accepted");
        assert!(outcome.is_accepted());
        let completion_handle =
            completion_handle.expect("queued progress input should expose a completion waiter");

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "reset split-lifetime wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_reset = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before reset");
        let wait_request_id = before_reset
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_reset.control.phase, RuntimeState::Attached);
        assert_eq!(before_reset.completion_waiters.input_count, 1);
        assert_eq!(before_reset.completion_waiters.waiter_count, 1);
        assert_eq!(before_reset.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_reset.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            before_reset.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before reset"
        );
        assert_eq!(
            before_reset.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before reset"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "reset should be able to discard queued attached-loop work before apply runs"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "reset has not yet attempted any executor control"
        );

        let report = SessionServiceRuntimeExt::reset_runtime(&adapter, &session_id)
            .await
            .expect("reset should preserve wait_all while abandoning queued attached-loop work");
        assert_eq!(report.inputs_abandoned, 1);

        let after_reset = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after reset");
        assert_eq!(after_reset.control.phase, RuntimeState::Idle);
        assert_eq!(after_reset.completion_waiters.input_count, 0);
        assert_eq!(after_reset.completion_waiters.waiter_count, 0);
        assert!(
            after_reset.completion_waiters.waiting_inputs.is_empty(),
            "reset should clear input-owned completion waiters immediately"
        );
        assert_eq!(
            after_reset.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "reset should preserve the authority-owned wait request even after clearing input waiters"
        );
        assert!(after_reset.ops.pending_wait_present);
        assert_eq!(
            after_reset.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "reset should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_reset.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "reset should preserve the tracked wait target until the operation settles"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "reset should bypass queued attached-loop work entirely"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "reset currently bypasses the executor control seam and does not deliver an out-of-band control command"
        );

        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime reset");
            }
            other => panic!(
                "expected reset to terminate the completion waiter immediately, got {other:?}"
            ),
        }

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after reset");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Idle);
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_destroy() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;
        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "destroy wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_destroy = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before destroy");
        let wait_request_id = before_destroy
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert!(before_destroy.ops.pending_wait_present);
        assert_eq!(
            before_destroy.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before destroy"
        );
        assert_eq!(
            before_destroy.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before destroy"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
            .await
            .expect("destroy should succeed for an idle runtime");
        assert_eq!(report.inputs_abandoned, 0);

        let after_destroy = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after destroy");
        assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
        assert_eq!(
            after_destroy.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "destroy currently preserves the authority-owned wait request"
        );
        assert!(
            after_destroy.ops.pending_wait_present,
            "destroy currently preserves the pending wait carrier while the operation remains active"
        );
        assert_eq!(
            after_destroy.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "destroy should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_destroy.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "destroy should preserve the tracked wait targets until the operation settles"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after destroy");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Destroyed);
        assert_eq!(
            settled.control.current_run_id, None,
            "destroy should not leave a settled control-side current-run binding behind"
        );
        assert_eq!(
            settled.inputs.current_run_id, None,
            "destroy should not leave a settled ingress-side current-run binding behind"
        );
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_destroy_splits_completion_and_wait_all_lifetimes() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = make_prompt("destroy split lifetimes");
        let input_id = input.id().clone();
        let (_outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("prompt should be accepted");
        let completion_handle =
            completion_handle.expect("queued prompt should register a completion waiter");

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "destroy split wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_destroy = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before destroy");
        let wait_request_id = before_destroy
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_destroy.control.phase, RuntimeState::Idle);
        assert_eq!(before_destroy.completion_waiters.input_count, 1);
        assert_eq!(before_destroy.completion_waiters.waiter_count, 1);
        assert_eq!(before_destroy.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_destroy.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert!(before_destroy.ops.pending_wait_present);
        assert_eq!(
            before_destroy.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before destroy"
        );
        assert_eq!(
            before_destroy.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before destroy"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
            .await
            .expect("destroy should split completion and wait_all lifetimes");
        assert_eq!(report.inputs_abandoned, 1);

        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime destroyed");
            }
            other => panic!("expected runtime destroyed termination, got {other:?}"),
        }

        let after_destroy = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after destroy");
        assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
        assert_eq!(after_destroy.completion_waiters.input_count, 0);
        assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
        assert!(
            after_destroy.completion_waiters.waiting_inputs.is_empty(),
            "destroy should clear input-owned completion waiters immediately"
        );
        assert_eq!(
            after_destroy.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "destroy should preserve the authority-owned wait request after completion waiters clear"
        );
        assert!(after_destroy.ops.pending_wait_present);
        assert_eq!(
            after_destroy.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "destroy should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_destroy.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "destroy should preserve the tracked wait target until the operation settles"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after destroy");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Destroyed);
        assert_eq!(
            settled.control.current_run_id, None,
            "destroy should not leave a settled control-side current-run binding behind even on attached runtimes"
        );
        assert_eq!(
            settled.inputs.current_run_id, None,
            "destroy should not leave a settled ingress-side current-run binding behind even on attached runtimes"
        );
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_destroy_with_runtime_loop() {
        struct RecordingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for RecordingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(RecordingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                }),
            )
            .await;

        let outcome = adapter
            .accept_input_without_wake(
                &session_id,
                make_progress_input("destroy-wait-all-with-loop"),
            )
            .await
            .expect("queued progress input should be accepted without waking the loop");
        assert!(outcome.is_accepted());

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "destroy wait target with loop".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_destroy = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before destroy");
        let wait_request_id = before_destroy
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_destroy.control.phase, RuntimeState::Attached);
        assert!(before_destroy.ops.pending_wait_present);
        assert_eq!(
            before_destroy.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before destroy"
        );
        assert_eq!(
            before_destroy.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before destroy"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "destroy should be able to abandon queued attached-loop work before apply runs"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "destroy has not yet attempted any executor control"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
            .await
            .expect("destroy should preserve wait_all while abandoning queued attached-loop work");
        assert_eq!(report.inputs_abandoned, 1);

        let after_destroy = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after destroy");
        assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
        assert_eq!(
            after_destroy.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "destroy should preserve the authority-owned wait request even with an attached loop"
        );
        assert!(
            after_destroy.ops.pending_wait_present,
            "destroy should preserve the pending wait carrier while the operation remains active"
        );
        assert_eq!(
            after_destroy.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "destroy should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_destroy.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "destroy should preserve the tracked wait targets until the operation settles"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "destroy should bypass queued attached-loop work entirely"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "destroy currently bypasses the executor control seam and does not deliver an out-of-band control command"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete");

        let waited = wait_future
            .await
            .expect("wait_all should resolve after completion");
        assert_eq!(waited.satisfied.wait_request_id, wait_request_id);
        assert_eq!(waited.satisfied.operation_ids, vec![operation_id]);

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Destroyed);
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_destroy_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
     {
        struct RecordingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for RecordingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(RecordingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                }),
            )
            .await;

        let input = make_progress_input("destroy-with-loop-split-lifetimes");
        let input_id = input.id().clone();
        let outcome = adapter
            .accept_input_without_wake(&session_id, input)
            .await
            .expect("queued progress input should be accepted without waking the loop");
        assert!(outcome.is_accepted());

        let completion_handle = {
            let completions = {
                let sessions = adapter.sessions.read().await;
                sessions
                    .get(&session_id)
                    .expect("attached session should exist")
                    .completions
                    .clone()
            };
            let mut completions = completions.lock().await;
            completions.register(input_id.clone())
        };

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "destroy split wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_destroy = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before destroy");
        let wait_request_id = before_destroy
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_destroy.control.phase, RuntimeState::Attached);
        assert_eq!(before_destroy.completion_waiters.input_count, 1);
        assert_eq!(before_destroy.completion_waiters.waiter_count, 1);
        assert_eq!(before_destroy.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_destroy.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert!(before_destroy.ops.pending_wait_present);
        assert_eq!(
            before_destroy.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before destroy"
        );
        assert_eq!(
            before_destroy.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before destroy"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
            .await
            .expect("destroy should split completion and wait_all lifetimes on attached runtimes");
        assert_eq!(report.inputs_abandoned, 1);

        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime destroyed");
            }
            other => panic!("expected runtime destroyed termination, got {other:?}"),
        }

        let after_destroy = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after destroy");
        assert_eq!(after_destroy.control.phase, RuntimeState::Destroyed);
        assert_eq!(after_destroy.completion_waiters.input_count, 0);
        assert_eq!(after_destroy.completion_waiters.waiter_count, 0);
        assert!(
            after_destroy.completion_waiters.waiting_inputs.is_empty(),
            "destroy should clear input-owned completion waiters immediately"
        );
        assert_eq!(
            after_destroy.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "destroy should preserve the authority-owned wait request after completion waiters clear"
        );
        assert!(after_destroy.ops.pending_wait_present);
        assert_eq!(
            after_destroy.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "destroy should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_destroy.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "destroy should preserve the tracked wait target until the operation settles"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "destroy should bypass queued attached-loop work entirely"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "destroy currently bypasses the executor control seam and does not deliver an out-of-band control command"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after destroy");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Destroyed);
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_stop_runtime_executor() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;
        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "stop wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_stop = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before stop");
        let wait_request_id = before_stop
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert!(before_stop.ops.pending_wait_present);
        assert_eq!(
            before_stop.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before stop"
        );
        assert_eq!(
            before_stop.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before stop"
        );

        adapter
            .stop_runtime_executor(
                &session_id,
                RunControlCommand::StopRuntimeExecutor {
                    reason: "stop wait_all test".into(),
                },
            )
            .await
            .expect("stop should preserve the active wait_all carrier");

        let after_stop = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after stop");
                if snapshot.control.phase == RuntimeState::Stopped {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("attached stop should eventually publish the Stopped phase");
        assert_eq!(
            after_stop.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "stop currently preserves the authority-owned wait request"
        );
        assert!(
            after_stop.ops.pending_wait_present,
            "stop currently preserves the pending wait carrier while the operation remains active"
        );
        assert_eq!(
            after_stop.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "stop should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_stop.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "stop should preserve the tracked wait targets until the operation settles"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after stop");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Stopped);
        assert_eq!(
            settled.control.current_run_id, None,
            "stop should not leave a settled control-side current-run binding behind"
        );
        assert_eq!(
            settled.inputs.current_run_id, None,
            "stop should not leave a settled ingress-side current-run binding behind"
        );
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_stop_runtime_executor_clears_steered_waiter_and_queue_but_preserves_wait_all()
     {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = Input::Prompt(crate::input::PromptInput::new(
            "stop steered prompt",
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                    ..Default::default()
                },
            ),
        ));
        let input_id = input.id().clone();
        let (_outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("steered prompt should be accepted");
        let completion_handle =
            completion_handle.expect("steered prompt should register a completion waiter");

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "stop steered wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_stop = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before stop");
        let wait_request_id = before_stop
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_stop.control.phase, RuntimeState::Idle);
        assert!(before_stop.inputs.queue.is_empty());
        assert_eq!(before_stop.inputs.steer_queue, vec![input_id.clone()]);
        assert!(before_stop.inputs.wake_requested);
        assert!(before_stop.inputs.process_requested);
        assert_eq!(before_stop.completion_waiters.input_count, 1);
        assert_eq!(before_stop.completion_waiters.waiter_count, 1);
        assert_eq!(before_stop.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_stop.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert!(before_stop.ops.pending_wait_present);
        assert_eq!(
            before_stop.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before stop"
        );
        assert_eq!(
            before_stop.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before stop"
        );

        adapter
            .stop_runtime_executor(
                &session_id,
                RunControlCommand::StopRuntimeExecutor {
                    reason: "stop steered split lifetimes".into(),
                },
            )
            .await
            .expect("stop should clear steered completion waiters while preserving wait_all");

        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime stopped");
            }
            other => {
                panic!("expected runtime stopped termination for steered input, got {other:?}")
            }
        }

        let after_stop = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after stop");
        assert_eq!(after_stop.control.phase, RuntimeState::Stopped);
        assert_eq!(after_stop.control.current_run_id, None);
        assert_eq!(after_stop.inputs.current_run_id, None);
        assert!(after_stop.inputs.queue.is_empty());
        assert!(
            after_stop.inputs.steer_queue.is_empty(),
            "stop should clear steered queued work immediately on the plain runtime path"
        );
        assert_eq!(after_stop.completion_waiters.input_count, 0);
        assert_eq!(after_stop.completion_waiters.waiter_count, 0);
        assert!(
            after_stop.completion_waiters.waiting_inputs.is_empty(),
            "stop should clear input-owned steered completion waiters immediately"
        );
        assert_eq!(
            after_stop.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "stop should preserve the authority-owned wait request after steered completion waiters clear"
        );
        assert!(after_stop.ops.pending_wait_present);
        assert_eq!(
            after_stop.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "stop should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_stop.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "stop should preserve the tracked wait target until the operation settles"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after stop");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Stopped);
        assert_eq!(settled.control.current_run_id, None);
        assert_eq!(settled.inputs.current_run_id, None);
        assert!(settled.inputs.queue.is_empty());
        assert!(settled.inputs.steer_queue.is_empty());
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_stop_runtime_executor_splits_completion_and_wait_all_lifetimes()
     {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = make_prompt("stop split lifetimes");
        let input_id = input.id().clone();
        let (_outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("prompt should be accepted");
        let completion_handle =
            completion_handle.expect("queued prompt should register a completion waiter");

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "stop split wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_stop = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before stop");
        let wait_request_id = before_stop
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_stop.control.phase, RuntimeState::Idle);
        assert_eq!(before_stop.completion_waiters.input_count, 1);
        assert_eq!(before_stop.completion_waiters.waiter_count, 1);
        assert_eq!(before_stop.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_stop.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert!(before_stop.ops.pending_wait_present);
        assert_eq!(
            before_stop.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before stop"
        );
        assert_eq!(
            before_stop.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before stop"
        );

        adapter
            .stop_runtime_executor(
                &session_id,
                RunControlCommand::StopRuntimeExecutor {
                    reason: "stop split lifetimes".into(),
                },
            )
            .await
            .expect("stop should split completion and wait_all lifetimes");

        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime stopped");
            }
            other => panic!("expected runtime stopped termination, got {other:?}"),
        }

        let after_stop = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after stop");
        assert_eq!(after_stop.control.phase, RuntimeState::Stopped);
        assert!(
            after_stop.inputs.queue.is_empty(),
            "stop should abandon ordinary queued work immediately on the plain runtime path"
        );
        assert!(
            after_stop.inputs.steer_queue.is_empty(),
            "stop should abandon steered queued work immediately on the plain runtime path"
        );
        assert_eq!(after_stop.completion_waiters.input_count, 0);
        assert_eq!(after_stop.completion_waiters.waiter_count, 0);
        assert!(
            after_stop.completion_waiters.waiting_inputs.is_empty(),
            "stop should clear input-owned completion waiters immediately"
        );
        assert_eq!(
            after_stop.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "stop should preserve the authority-owned wait request after completion waiters clear"
        );
        assert!(after_stop.ops.pending_wait_present);
        assert_eq!(
            after_stop.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "stop should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_stop.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "stop should preserve the tracked wait target until the operation settles"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after stop");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Stopped);
        assert!(
            settled.inputs.queue.is_empty(),
            "stop should not reintroduce ordinary queued work once the plain runtime settles"
        );
        assert!(
            settled.inputs.steer_queue.is_empty(),
            "stop should not reintroduce steered queued work once the plain runtime settles"
        );
        assert_eq!(
            settled.control.current_run_id, None,
            "stop should not leave a settled control-side current-run binding behind even on attached runtimes"
        );
        assert_eq!(
            settled.inputs.current_run_id, None,
            "stop should not leave a settled ingress-side current-run binding behind even on attached runtimes"
        );
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_stop_runtime_executor_with_runtime_loop()
     {
        struct RecordingExecutor {
            apply_calls: Arc<AtomicUsize>,
            stop_calls: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for RecordingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                if matches!(command, RunControlCommand::StopRuntimeExecutor { .. }) {
                    self.stop_calls.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let stop_calls = Arc::new(AtomicUsize::new(0));

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(RecordingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    stop_calls: Arc::clone(&stop_calls),
                }),
            )
            .await;

        let outcome = adapter
            .accept_input_without_wake(&session_id, make_progress_input("stop-wait-all-with-loop"))
            .await
            .expect("queued progress input should be accepted without waking the loop");
        assert!(outcome.is_accepted());

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "stop wait target with loop".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_stop = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before stop");
        let wait_request_id = before_stop
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_stop.control.phase, RuntimeState::Attached);
        assert!(before_stop.ops.pending_wait_present);
        assert_eq!(
            before_stop.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before stop"
        );
        assert_eq!(
            before_stop.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before stop"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "stop should be able to preempt queued attached-loop work before apply runs"
        );

        adapter
            .stop_runtime_executor(
                &session_id,
                RunControlCommand::StopRuntimeExecutor {
                    reason: "stop attached-loop wait_all".into(),
                },
            )
            .await
            .expect(
                "stop should preserve the active wait_all carrier through the live control seam",
            );

        let after_stop = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after stop");
                if snapshot.control.phase == RuntimeState::Stopped {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("attached stop should eventually publish the Stopped phase");
        assert_eq!(
            after_stop.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "stop should preserve the authority-owned wait request on attached runtimes"
        );
        assert!(after_stop.ops.pending_wait_present);
        assert_eq!(
            after_stop.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "stop should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_stop.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "stop should preserve the tracked wait target until the operation settles"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "stop should beat queued ordinary work on an attached runtime loop"
        );
        assert_eq!(
            stop_calls.load(Ordering::SeqCst),
            1,
            "the attached executor should observe exactly one stop-runtime-executor control"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after stop");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Stopped);
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_stop_runtime_executor_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
     {
        struct RecordingExecutor {
            apply_calls: Arc<AtomicUsize>,
            stop_calls: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for RecordingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                if matches!(command, RunControlCommand::StopRuntimeExecutor { .. }) {
                    self.stop_calls.fetch_add(1, Ordering::SeqCst);
                }
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let stop_calls = Arc::new(AtomicUsize::new(0));

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(RecordingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    stop_calls: Arc::clone(&stop_calls),
                }),
            )
            .await;

        let input = make_progress_input("stop-with-loop-split-lifetimes");
        let input_id = input.id().clone();
        let outcome = adapter
            .accept_input_without_wake(&session_id, input)
            .await
            .expect("queued progress input should be accepted without waking the loop");
        assert!(outcome.is_accepted());

        let completion_handle = {
            let completions = {
                let sessions = adapter.sessions.read().await;
                sessions
                    .get(&session_id)
                    .expect("attached session should exist")
                    .completions
                    .clone()
            };
            let mut completions = completions.lock().await;
            completions.register(input_id.clone())
        };

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "stop split wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_stop = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before stop");
        let wait_request_id = before_stop
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_stop.control.phase, RuntimeState::Attached);
        assert_eq!(before_stop.completion_waiters.input_count, 1);
        assert_eq!(before_stop.completion_waiters.waiter_count, 1);
        assert_eq!(before_stop.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_stop.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert!(before_stop.ops.pending_wait_present);
        assert_eq!(
            before_stop.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before stop"
        );
        assert_eq!(
            before_stop.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before stop"
        );

        adapter
            .stop_runtime_executor(
                &session_id,
                RunControlCommand::StopRuntimeExecutor {
                    reason: "stop attached-loop split lifetimes".into(),
                },
            )
            .await
            .expect("stop should split completion and wait_all lifetimes on attached runtimes");

        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "runtime stopped");
            }
            other => panic!("expected runtime stopped termination, got {other:?}"),
        }

        let after_stop = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after stop");
                if snapshot.control.phase == RuntimeState::Stopped {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("attached stop should eventually publish the Stopped phase");
        assert!(
            after_stop.inputs.queue.is_empty(),
            "attached stop should abandon ordinary queued work immediately"
        );
        assert!(
            after_stop.inputs.steer_queue.is_empty(),
            "attached stop should abandon steered queued work immediately"
        );
        assert_eq!(after_stop.completion_waiters.input_count, 0);
        assert_eq!(after_stop.completion_waiters.waiter_count, 0);
        assert!(
            after_stop.completion_waiters.waiting_inputs.is_empty(),
            "stop should clear input-owned completion waiters immediately"
        );
        assert_eq!(
            after_stop.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "stop should preserve the authority-owned wait request after completion waiters clear"
        );
        assert!(after_stop.ops.pending_wait_present);
        assert_eq!(
            after_stop.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "stop should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_stop.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "stop should preserve the tracked wait target until the operation settles"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "stop should beat queued ordinary work on an attached runtime loop"
        );
        assert_eq!(
            stop_calls.load(Ordering::SeqCst),
            1,
            "the attached executor should observe exactly one stop-runtime-executor control"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after stop");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Stopped);
        assert!(
            settled.inputs.queue.is_empty(),
            "attached stop should not reintroduce ordinary queued work once the runtime settles"
        );
        assert!(
            settled.inputs.steer_queue.is_empty(),
            "attached stop should not reintroduce steered queued work once the runtime settles"
        );
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_retire() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;
        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "retire wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_retire = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before retire");
        let wait_request_id = before_retire
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert!(before_retire.ops.pending_wait_present);
        assert_eq!(
            before_retire.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before retire"
        );
        assert_eq!(
            before_retire.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before retire"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
            .await
            .expect("retire should preserve the active wait_all carrier");
        assert_eq!(report.inputs_abandoned, 0);
        assert_eq!(report.inputs_pending_drain, 0);

        let after_retire = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after retire");
        assert_eq!(after_retire.control.phase, RuntimeState::Retired);
        assert_eq!(
            after_retire.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "retire currently preserves the authority-owned wait request"
        );
        assert!(
            after_retire.ops.pending_wait_present,
            "retire currently preserves the pending wait carrier while the operation remains active"
        );
        assert_eq!(
            after_retire.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "retire should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_retire.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "retire should preserve the tracked wait targets until the operation settles"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after retire");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Retired);
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_retire_splits_completion_and_wait_all_lifetimes() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = make_prompt("retire split lifetimes");
        let input_id = input.id().clone();
        let (_outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("prompt should be accepted");
        let completion_handle =
            completion_handle.expect("queued prompt should register a completion waiter");

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "retire split wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_retire = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before retire");
        let wait_request_id = before_retire
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_retire.control.phase, RuntimeState::Idle);
        assert_eq!(before_retire.completion_waiters.input_count, 1);
        assert_eq!(before_retire.completion_waiters.waiter_count, 1);
        assert_eq!(before_retire.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_retire.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert!(before_retire.ops.pending_wait_present);
        assert_eq!(
            before_retire.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before retire"
        );
        assert_eq!(
            before_retire.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before retire"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
            .await
            .expect("retire should split completion and wait_all lifetimes");
        assert_eq!(report.inputs_abandoned, 1);
        assert_eq!(report.inputs_pending_drain, 0);

        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "retired without runtime loop");
            }
            other => panic!("expected retire termination, got {other:?}"),
        }

        let after_retire = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after retire");
        assert_eq!(after_retire.control.phase, RuntimeState::Retired);
        assert_eq!(
            after_retire.control.current_run_id, None,
            "retire should not leave a settled current-run binding behind once the runtime is actually Retired"
        );
        assert_eq!(
            after_retire.inputs.current_run_id, None,
            "retire should clear ingress-side current-run binding once the runtime is actually Retired"
        );
        assert_eq!(after_retire.completion_waiters.input_count, 0);
        assert_eq!(after_retire.completion_waiters.waiter_count, 0);
        assert!(
            after_retire.completion_waiters.waiting_inputs.is_empty(),
            "retire should clear input-owned completion waiters immediately when no runtime loop can drain"
        );
        assert_eq!(
            after_retire.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "retire should preserve the authority-owned wait request after completion waiters clear"
        );
        assert!(after_retire.ops.pending_wait_present);
        assert_eq!(
            after_retire.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "retire should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_retire.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "retire should preserve the tracked wait target until the operation settles"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after retire");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Retired);
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_retire_clears_steered_waiter_but_leaves_steer_queue_visible()
     {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let input = Input::Prompt(crate::input::PromptInput::new(
            "retire steered prompt",
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                    ..Default::default()
                },
            ),
        ));
        let input_id = input.id().clone();
        let (_outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("steered prompt should be accepted");
        let completion_handle =
            completion_handle.expect("steered prompt should register a completion waiter");

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for registered session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "retire steered wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_retire = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before retire");
        let wait_request_id = before_retire
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_retire.control.phase, RuntimeState::Idle);
        assert!(before_retire.inputs.queue.is_empty());
        assert_eq!(before_retire.inputs.steer_queue, vec![input_id.clone()]);
        assert!(before_retire.inputs.wake_requested);
        assert!(before_retire.inputs.process_requested);
        assert_eq!(before_retire.completion_waiters.input_count, 1);
        assert_eq!(before_retire.completion_waiters.waiter_count, 1);
        assert_eq!(before_retire.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_retire.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert!(before_retire.ops.pending_wait_present);
        assert_eq!(
            before_retire.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before retire"
        );
        assert_eq!(
            before_retire.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before retire"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
            .await
            .expect(
                "retire should terminate the steered completion waiter while preserving wait_all",
            );
        assert_eq!(report.inputs_abandoned, 1);
        assert_eq!(report.inputs_pending_drain, 0);

        match completion_handle.wait().await {
            CompletionOutcome::RuntimeTerminated(reason) => {
                assert_eq!(reason, "retired without runtime loop");
            }
            other => panic!("expected retire termination for steered input, got {other:?}"),
        }

        let after_retire = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after retire");
        assert_eq!(after_retire.control.phase, RuntimeState::Retired);
        assert_eq!(after_retire.control.current_run_id, None);
        assert_eq!(after_retire.inputs.current_run_id, None);
        assert!(after_retire.inputs.queue.is_empty());
        assert_eq!(
            after_retire.inputs.steer_queue,
            vec![input_id.clone()],
            "retire currently leaves the steered input visible in the steer queue even after the steered completion waiter terminates"
        );
        assert_eq!(after_retire.completion_waiters.input_count, 0);
        assert_eq!(after_retire.completion_waiters.waiter_count, 0);
        assert!(
            after_retire.completion_waiters.waiting_inputs.is_empty(),
            "retire should clear input-owned steered completion waiters immediately"
        );
        assert_eq!(
            after_retire.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "retire should preserve the authority-owned wait request after steered completion waiters clear"
        );
        assert!(after_retire.ops.pending_wait_present);
        assert_eq!(
            after_retire.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "retire should preserve request-id agreement across the wait carrier seam"
        );
        assert_eq!(
            after_retire.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "retire should preserve the tracked wait target until the operation settles"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after retire");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Retired);
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_preserves_wait_all_after_retire_with_runtime_loop() {
        struct BlockingExecutor {
            apply_calls: Arc<AtomicUsize>,
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let input = make_progress_input("retire-wait-all-with-loop");
        let outcome = adapter
            .accept_input_without_wake(&session_id, input)
            .await
            .expect("queued progress input should be accepted without waking the loop");
        assert!(outcome.is_accepted());

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "retire wait target with loop".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_retire = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before retire");
        let wait_request_id = before_retire
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_retire.control.phase, RuntimeState::Attached);
        assert!(before_retire.ops.pending_wait_present);
        assert_eq!(
            before_retire.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before retire"
        );
        assert_eq!(
            before_retire.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before retire"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "queued attached-loop work should remain pending until retire wakes the loop"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
            .await
            .expect("retire should preserve wait_all while the live loop drains queued work");
        assert_eq!(report.inputs_abandoned, 0);
        assert_eq!(report.inputs_pending_drain, 1);

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect("retire should wake the attached runtime loop to drain queued work");

        let after_retire = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after retire wakes the loop");
        assert_eq!(
            after_retire.control.phase,
            RuntimeState::Running,
            "retire currently re-enters Running while the attached loop drains preserved work"
        );
        assert_eq!(
            after_retire.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "retire should preserve the authority-owned wait request while the attached loop is draining"
        );
        assert!(after_retire.ops.pending_wait_present);
        assert_eq!(
            after_retire.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "retire should preserve request-id agreement across the wait carrier seam while draining"
        );
        assert_eq!(
            after_retire.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "retire should preserve the tracked wait target while queued work is draining"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "retire should wake the attached loop exactly once for the preserved queued work"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .expect("attached loop should finish draining the preserved queued work");

        let after_drain = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after attached-loop drain");
                if snapshot.control.phase == RuntimeState::Retired {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("runtime should return to Retired once the preserved work finishes draining");

        assert_eq!(
            after_drain.control.current_run_id, None,
            "retire should not leave a settled control-side current-run binding once the attached loop returns to Retired"
        );
        assert_eq!(
            after_drain.inputs.current_run_id, None,
            "retire should not leave a settled ingress-side current-run binding once the attached loop returns to Retired"
        );
        assert_eq!(
            after_drain.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "wait_all should remain active after the attached loop finishes draining preserved work"
        );
        assert!(after_drain.ops.pending_wait_present);
        assert_eq!(
            after_drain.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "request-id agreement should survive the return to Retired after drain"
        );
        assert_eq!(
            after_drain.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "the tracked wait target should remain present until the operation itself settles"
        );
        assert!(
            after_drain.inputs.queue.is_empty(),
            "retire should not leave ordinary queued work behind once the attached runtime returns to Retired"
        );
        assert!(
            after_drain.inputs.steer_queue.is_empty(),
            "retire should not leave steer-queued work behind once the attached runtime returns to Retired"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after retire+drain");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Retired);
        assert!(
            settled.inputs.queue.is_empty(),
            "retire should keep the attached settled Retired snapshot free of ordinary queued work"
        );
        assert!(
            settled.inputs.steer_queue.is_empty(),
            "retire should keep the attached settled Retired snapshot free of steer-queued work"
        );
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_retire_with_runtime_loop_splits_completion_and_wait_all_lifetimes()
     {
        struct BlockingExecutor {
            apply_calls: Arc<AtomicUsize>,
            control_calls: Arc<AtomicUsize>,
            apply_started: Arc<Notify>,
            apply_finished: Arc<Notify>,
            allow_finish: Arc<Notify>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for BlockingExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.apply_calls.fetch_add(1, Ordering::SeqCst);
                self.apply_started.notify_waiters();
                self.allow_finish.notified().await;
                self.apply_finished.notify_waiters();

                Ok(CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                    run_result: None,
                })
            }

            async fn control(
                &mut self,
                _command: RunControlCommand,
            ) -> Result<(), CoreExecutorError> {
                self.control_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let control_calls = Arc::new(AtomicUsize::new(0));
        let apply_started = Arc::new(Notify::new());
        let apply_finished = Arc::new(Notify::new());
        let allow_finish = Arc::new(Notify::new());

        adapter
            .register_session_with_executor(
                session_id.clone(),
                Box::new(BlockingExecutor {
                    apply_calls: Arc::clone(&apply_calls),
                    control_calls: Arc::clone(&control_calls),
                    apply_started: Arc::clone(&apply_started),
                    apply_finished: Arc::clone(&apply_finished),
                    allow_finish: Arc::clone(&allow_finish),
                }),
            )
            .await;

        let input = make_progress_input("retire-with-loop-split-lifetimes");
        let input_id = input.id().clone();
        let (outcome, completion_handle) = adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("progress input should be accepted");
        assert!(outcome.is_accepted());
        let completion_handle =
            completion_handle.expect("queued progress input should expose a completion waiter");

        let registry = adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .expect("ops registry should exist for attached session");

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: session_id.clone(),
                display_name: "retire split-lifetime wait target".into(),
                source_label: "meerkat_machine_test".into(),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("operation should register");
        registry
            .provisioning_succeeded(&operation_id)
            .expect("operation should enter running");

        let wait_future = registry.wait_all(&RunId::new(), std::slice::from_ref(&operation_id));

        let before_retire = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist before retire");
        let wait_request_id = before_retire
            .ops
            .wait_request_id
            .clone()
            .expect("wait_all should register an authority-owned wait request");
        assert_eq!(before_retire.control.phase, RuntimeState::Attached);
        assert_eq!(before_retire.completion_waiters.input_count, 1);
        assert_eq!(before_retire.completion_waiters.waiter_count, 1);
        assert_eq!(before_retire.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            before_retire.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            before_retire.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "pending wait carrier should track the same wait request id before retire"
        );
        assert_eq!(
            before_retire.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "wait_all should track the active operation before retire"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "queued attached-loop work should remain pending until retire wakes the loop"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "retire should not yet have attempted executor control"
        );

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let report = crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
            .await
            .expect("retire should preserve queued work and wait_all while the live loop drains");
        assert_eq!(report.inputs_abandoned, 0);
        assert_eq!(report.inputs_pending_drain, 1);

        tokio::time::timeout(Duration::from_secs(1), apply_started.notified())
            .await
            .expect("retire should wake the attached runtime loop to drain queued work");

        let during_retire = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after retire wakes the loop");
        assert_eq!(
            during_retire.control.phase,
            RuntimeState::Running,
            "retire currently re-enters Running while the attached loop drains preserved work"
        );
        assert_eq!(during_retire.completion_waiters.input_count, 1);
        assert_eq!(during_retire.completion_waiters.waiter_count, 1);
        assert_eq!(during_retire.completion_waiters.waiting_inputs.len(), 1);
        assert_eq!(
            during_retire.completion_waiters.waiting_inputs[0].input_id,
            input_id
        );
        assert_eq!(
            during_retire.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "retire should preserve the authority-owned wait request while the attached loop is draining"
        );
        assert!(during_retire.ops.pending_wait_present);
        assert_eq!(
            during_retire.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "retire should preserve request-id agreement across the wait carrier seam while draining"
        );
        assert_eq!(
            during_retire.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "retire should preserve the tracked wait target while queued work is draining"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            1,
            "retire should wake the attached loop exactly once for the preserved queued work"
        );
        assert_eq!(
            control_calls.load(Ordering::SeqCst),
            0,
            "retire should not route any out-of-band control command through the executor seam"
        );

        allow_finish.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), apply_finished.notified())
            .await
            .expect("attached loop should finish draining the preserved queued work");

        match completion_handle.wait().await {
            CompletionOutcome::CompletedWithoutResult => {}
            other => panic!(
                "expected retire+drain to complete queued work while wait_all remains live, got {other:?}"
            ),
        }

        let after_drain = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = adapter
                    .meerkat_machine_spine_snapshot(&session_id)
                    .await
                    .expect("snapshot should exist after attached-loop drain");
                if snapshot.control.phase == RuntimeState::Retired {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("runtime should return to Retired once the preserved work finishes draining");
        assert!(
            after_drain.inputs.queue.is_empty(),
            "retire with a runtime loop should not leave ordinary queued work behind once the runtime returns to Retired"
        );
        assert!(
            after_drain.inputs.steer_queue.is_empty(),
            "retire with a runtime loop should not leave steer-queued work behind once the runtime returns to Retired"
        );
        assert_eq!(after_drain.completion_waiters.input_count, 0);
        assert_eq!(after_drain.completion_waiters.waiter_count, 0);
        assert!(
            after_drain.completion_waiters.waiting_inputs.is_empty(),
            "completion waiters should clear once retire-drained work completes"
        );
        assert_eq!(
            after_drain.ops.wait_request_id,
            Some(wait_request_id.clone()),
            "wait_all should remain active after input-owned completion waiters clear"
        );
        assert!(after_drain.ops.pending_wait_present);
        assert_eq!(
            after_drain.ops.pending_wait_request_id,
            Some(wait_request_id.clone()),
            "request-id agreement should survive the return to Retired after drain"
        );
        assert_eq!(
            after_drain.ops.wait_operation_ids,
            vec![operation_id.clone()],
            "the tracked wait target should remain present until the operation itself settles"
        );

        registry
            .complete_operation(
                &operation_id,
                OperationResult {
                    id: operation_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("operation should complete after retire+drain");
        let wait_result = wait_future.await.expect("wait_all should still resolve");
        assert_eq!(wait_result.satisfied.wait_request_id, wait_request_id);
        assert_eq!(
            wait_result.satisfied.operation_ids,
            vec![operation_id.clone()]
        );

        let settled = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist after wait_all settles");
        assert_eq!(settled.control.phase, RuntimeState::Retired);
        assert_eq!(settled.ops.wait_request_id, None);
        assert!(!settled.ops.pending_wait_present);
        assert_eq!(settled.ops.pending_wait_request_id, None);
        assert!(settled.ops.wait_operation_ids.is_empty());
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_tracks_comms_drain_state() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());

        spawn_test_comms_drain(
            &adapter,
            &session_id,
            CommsDrainMode::PersistentHost,
            comms_runtime,
            Duration::from_secs(60),
        )
        .await;

        let snapshot = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist for registered session");

        assert!(snapshot.drain.slot_present);
        assert_eq!(snapshot.drain.phase, Some(CommsDrainPhase::Running));
        assert_eq!(snapshot.drain.mode, Some(CommsDrainMode::PersistentHost));
        assert!(snapshot.drain.handle_present);
    }

    #[tokio::test]
    async fn meerkat_machine_spine_snapshot_tracks_stopped_comms_drain_state() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        let comms_runtime: Arc<dyn CommsRuntime> = Arc::new(FakeDrainRuntime::idle());

        let spawned = adapter
            .maybe_spawn_comms_drain(&session_id, true, Some(comms_runtime))
            .await;
        assert!(
            !spawned,
            "unregistered session should not spawn a comms drain"
        );

        adapter.register_session(session_id.clone()).await;

        let spawned = adapter
            .maybe_spawn_comms_drain(
                &session_id,
                true,
                Some(Arc::new(FakeDrainRuntime::idle()) as Arc<dyn CommsRuntime>),
            )
            .await;
        assert!(spawned, "registered session should spawn a comms drain");

        adapter.abort_comms_drain(&session_id).await;

        let snapshot = adapter
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("snapshot should exist for registered session");

        assert!(snapshot.drain.slot_present);
        assert_eq!(snapshot.drain.phase, Some(CommsDrainPhase::Stopped));
        assert_eq!(snapshot.drain.mode, Some(CommsDrainMode::PersistentHost));
        assert!(!snapshot.drain.handle_present);
    }

    // ---------------------------------------------------------------
    // A1: Session command guards (TLA+ DestroyedShapeInvariant,
    //     RunningHasActiveRunInvariant)
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn register_session_rejects_destroyed_session() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        // Transition to Destroyed via the control-plane destroy path.
        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
            .await
            .expect("destroy should succeed");

        // Second register must be rejected — DestroyedShapeInvariant forbids
        // resurrecting a destroyed binding.
        let err = adapter
            .execute_meerkat_machine_session_command(
                MeerkatMachineSessionCommand::RegisterSession {
                    session_id: session_id.clone(),
                },
            )
            .await
            .expect_err("register should reject a destroyed session");
        assert!(
            matches!(err, RuntimeDriverError::Destroyed),
            "expected Destroyed, got {err:?}"
        );
    }

    #[tokio::test]
    async fn unregister_session_rejects_unknown_session() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        // Unregister on a session that was never registered must return an error.
        let err = adapter
            .execute_meerkat_machine_session_command(
                MeerkatMachineSessionCommand::UnregisterSession {
                    session_id: session_id.clone(),
                },
            )
            .await
            .expect_err("unregister should reject an unknown session");
        assert!(
            matches!(err, RuntimeDriverError::NotReady { .. }),
            "expected NotReady, got {err:?}"
        );
    }

    #[tokio::test]
    async fn interrupt_current_run_rejects_destroyed_session() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
            .await
            .expect("destroy should succeed");

        let err = adapter
            .interrupt_current_run(&session_id)
            .await
            .expect_err("interrupt should reject a destroyed session");
        assert!(
            matches!(err, RuntimeDriverError::Destroyed),
            "expected Destroyed, got {err:?}"
        );
    }

    #[tokio::test]
    async fn cancel_after_boundary_rejects_destroyed_session() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
            .await
            .expect("destroy should succeed");

        let err = adapter
            .cancel_after_boundary(&session_id)
            .await
            .expect_err("cancel_after_boundary should reject a destroyed session");
        assert!(
            matches!(err, RuntimeDriverError::Destroyed),
            "expected Destroyed, got {err:?}"
        );
    }

    #[tokio::test]
    async fn stop_runtime_executor_rejects_destroyed_session() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
            .await
            .expect("destroy should succeed");

        let err = adapter
            .stop_runtime_executor(
                &session_id,
                RunControlCommand::StopRuntimeExecutor {
                    reason: "test".to_string(),
                },
            )
            .await
            .expect_err("stop_runtime_executor should reject a destroyed session");
        assert!(
            matches!(err, RuntimeDriverError::Destroyed),
            "expected Destroyed, got {err:?}"
        );
    }

    // ---------------------------------------------------------------
    // A2: Drain command guards (TLA+ DrainBindingInvariant,
    //     DrainModeInvariant)
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn set_peer_ingress_context_rejects_unknown_session() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();

        let spawned = adapter
            .update_peer_ingress_context(&session_id, true, None)
            .await;
        assert!(
            !spawned,
            "update_peer_ingress_context should not spawn for unknown session"
        );
    }

    #[tokio::test]
    async fn set_peer_ingress_context_rejects_destroyed_session() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        crate::traits::RuntimeControlPlane::destroy(adapter.as_ref(), &runtime_id)
            .await
            .expect("destroy should succeed");

        let spawned = adapter
            .update_peer_ingress_context(&session_id, true, None)
            .await;
        assert!(
            !spawned,
            "update_peer_ingress_context should not spawn for destroyed session"
        );
    }

    #[tokio::test]
    async fn notify_drain_exited_rejects_unknown_session() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();

        // Should not panic — the guard silently rejects.
        adapter
            .notify_comms_drain_exited(&session_id, DrainExitReason::Dismissed)
            .await;
    }

    #[tokio::test]
    async fn notify_drain_exited_rejects_destroyed_session() {
        let adapter = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        crate::traits::RuntimeControlPlane::destroy(adapter.as_ref(), &runtime_id)
            .await
            .expect("destroy should succeed");

        // Should not panic — the guard silently rejects.
        adapter
            .notify_comms_drain_exited(&session_id, DrainExitReason::Dismissed)
            .await;
    }

    #[tokio::test]
    async fn abort_comms_drain_tolerates_unknown_session() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        // Guard rejects unknown session but caller swallows the error.
        adapter.abort_comms_drain(&session_id).await;
    }

    #[tokio::test]
    async fn wait_comms_drain_tolerates_unknown_session() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        // Guard rejects unknown session but caller swallows the error.
        adapter.wait_comms_drain(&session_id).await;
    }

    // ---------------------------------------------------------------
    // A3: Control command guards (TLA+ ActiveRunPhaseInvariant,
    //     LiveBindingLifecycleInvariant, AdmitQueuedInput precondition)
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn ingest_rejects_retired_session() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
            .await
            .expect("retire should succeed");

        let input = make_prompt("should be rejected");
        let err = crate::traits::RuntimeControlPlane::ingest(&adapter, &runtime_id, input)
            .await
            .expect_err("ingest should reject a retired session");
        assert!(
            matches!(
                err,
                RuntimeControlPlaneError::InvalidState {
                    state: RuntimeState::Retired
                }
            ),
            "expected InvalidState(Retired), got {err:?}"
        );
    }

    #[tokio::test]
    async fn ingest_rejects_stopped_session() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        // Stop the session by driving it through retire → stop.
        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        adapter
            .stop_runtime_executor(
                &session_id,
                RunControlCommand::StopRuntimeExecutor {
                    reason: "test stop".to_string(),
                },
            )
            .await
            .expect("stop should succeed");

        let input = make_prompt("should be rejected");
        let err = crate::traits::RuntimeControlPlane::ingest(&adapter, &runtime_id, input)
            .await
            .expect_err("ingest should reject a stopped session");
        assert!(
            matches!(
                err,
                RuntimeControlPlaneError::InvalidState {
                    state: RuntimeState::Stopped
                }
            ),
            "expected InvalidState(Stopped), got {err:?}"
        );
    }

    #[tokio::test]
    async fn retire_rejects_initializing_session() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        // Don't register, just create a session entry in Initializing state.
        // Since there's no way to create a session in Initializing via the
        // public API (register_session transitions to Idle), we test that
        // retire guards against the union of incompatible phases. The Idle →
        // Retired path is already exercised above. Focus here on the existing
        // Stopped guard and Destroyed guard.
        adapter.register_session(session_id.clone()).await;

        // First stop
        adapter
            .stop_runtime_executor(
                &session_id,
                RunControlCommand::StopRuntimeExecutor {
                    reason: "test".to_string(),
                },
            )
            .await
            .expect("stop should succeed");

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let err = crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
            .await
            .expect_err("retire should reject a stopped session");
        assert!(
            matches!(
                err,
                RuntimeControlPlaneError::InvalidState {
                    state: RuntimeState::Stopped
                }
            ),
            "expected InvalidState(Stopped), got {err:?}"
        );
    }

    // ---------------------------------------------------------------
    // A4: Ingress command guards (TLA+ WaitingInputsInvariant)
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn accept_input_with_completion_rejects_retired_session() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        crate::traits::RuntimeControlPlane::retire(&adapter, &runtime_id)
            .await
            .expect("retire should succeed");

        let input = make_prompt("should be rejected");
        let result = adapter
            .accept_input_with_completion(&session_id, input)
            .await;
        match result {
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Retired,
            }) => {}
            Err(other) => panic!("expected NotReady(Retired), got {other:?}"),
            Ok(_) => panic!("accept_input_with_completion should reject retired session"),
        }
    }

    #[tokio::test]
    async fn accept_input_with_completion_rejects_destroyed_session() {
        let adapter = MeerkatMachine::ephemeral();
        let session_id = SessionId::new();

        adapter.register_session(session_id.clone()).await;

        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        crate::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
            .await
            .expect("destroy should succeed");

        let input = make_prompt("should be rejected");
        let result = adapter
            .accept_input_with_completion(&session_id, input)
            .await;
        match result {
            Err(RuntimeDriverError::Destroyed) => {}
            Err(other) => panic!("expected Destroyed, got {other:?}"),
            Ok(_) => panic!("accept_input_with_completion should reject destroyed session"),
        }
    }
}
