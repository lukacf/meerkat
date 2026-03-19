//! RuntimeSessionAdapter — wraps a SessionService with per-session RuntimeDrivers.
//!
//! This adapter lives in meerkat-runtime so that meerkat-session doesn't need
//! to depend on meerkat-runtime. Surfaces use this adapter to get v9 runtime
//! capabilities on top of any SessionService implementation.
//!
//! When a session is registered with a `CoreExecutor`, a background RuntimeLoop
//! task is spawned per session. `accept_input()` queues the input in the driver
//! and, if wake is requested, signals the loop. The loop dequeues, stages,
//! applies via CoreExecutor (which calls SessionService::start_turn()), and
//! marks inputs as consumed.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

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
use crate::runtime_state::{RuntimeState, RuntimeStateTransitionError};
use crate::service_ext::{RuntimeMode, SessionServiceRuntimeExt};
use crate::store::RuntimeStore;
use crate::tokio;
use crate::tokio::sync::{Mutex, RwLock, mpsc};
use crate::traits::{
    DestroyReport, RecoveryReport, RecycleReport, ResetReport, RetireReport,
    RuntimeControlPlaneError, RuntimeDriver, RuntimeDriverError,
};

/// Shared driver handle used by both the adapter and the RuntimeLoop.
pub(crate) type SharedDriver = Arc<Mutex<DriverEntry>>;

/// Per-session runtime driver entry.
pub(crate) enum DriverEntry {
    Ephemeral(EphemeralRuntimeDriver),
    Persistent(PersistentRuntimeDriver),
}

impl DriverEntry {
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

    /// Check if the runtime is specifically in the Idle state (no executor attached).
    #[allow(dead_code)]
    pub(crate) fn is_idle(&self) -> bool {
        match self {
            DriverEntry::Ephemeral(d) => d.is_idle(),
            DriverEntry::Persistent(d) => d.is_idle(),
        }
    }

    /// Check if the runtime is idle or attached (quiescent with or without executor).
    pub(crate) fn is_idle_or_attached(&self) -> bool {
        match self {
            DriverEntry::Ephemeral(d) => d.is_idle_or_attached(),
            DriverEntry::Persistent(d) => d.is_idle_or_attached(),
        }
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
            DriverEntry::Ephemeral(d) => d.state_machine_ref().can_process_queue(),
            DriverEntry::Persistent(d) => d.inner_ref().state_machine_ref().can_process_queue(),
        }
    }

    /// Check and clear the wake flag.
    pub(crate) fn take_wake_requested(&mut self) -> bool {
        match self {
            DriverEntry::Ephemeral(d) => d.take_wake_requested(),
            DriverEntry::Persistent(d) => d.take_wake_requested(),
        }
    }

    /// Check and clear the immediate processing flag.
    pub(crate) fn take_process_requested(&mut self) -> bool {
        match self {
            DriverEntry::Ephemeral(d) => d.take_process_requested(),
            DriverEntry::Persistent(d) => d.take_process_requested(),
        }
    }

    /// Dequeue the next input for processing.
    pub(crate) fn dequeue_next(&mut self) -> Option<(InputId, Input)> {
        match self {
            DriverEntry::Ephemeral(d) => d.dequeue_next(),
            DriverEntry::Persistent(d) => d.dequeue_next(),
        }
    }

    /// Requeue an input at the front of the queue.
    pub(crate) fn enqueue_front_input(&mut self, input_id: InputId, input: Input) {
        match self {
            DriverEntry::Ephemeral(d) => d.enqueue_front_input(input_id, input),
            DriverEntry::Persistent(d) => d.enqueue_front_input(input_id, input),
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

    /// Apply an input after successful immediate execution.
    #[allow(dead_code)]
    pub(crate) fn apply_input(
        &mut self,
        input_id: &InputId,
        run_id: &RunId,
    ) -> Result<(), InputLifecycleError> {
        match self {
            DriverEntry::Ephemeral(d) => d.apply_input(input_id, run_id),
            DriverEntry::Persistent(d) => d.apply_input(input_id, run_id),
        }
    }

    /// Consume an input after successful immediate execution.
    #[allow(dead_code)]
    pub(crate) fn consume_inputs(
        &mut self,
        input_ids: &[InputId],
        run_id: &RunId,
    ) -> Result<(), InputLifecycleError> {
        match self {
            DriverEntry::Ephemeral(d) => d.consume_inputs(input_ids, run_id),
            DriverEntry::Persistent(d) => d.consume_inputs(input_ids, run_id),
        }
    }

    /// Roll back staged inputs after failed immediate execution.
    #[allow(dead_code)]
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

/// Per-session state: driver + optional RuntimeLoop.
struct RuntimeSessionEntry {
    /// Shared driver handle (accessed by both adapter methods and RuntimeLoop).
    driver: SharedDriver,
    /// Shared async-operation lifecycle registry for this runtime/session.
    ops_lifecycle: Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>,
    /// Completion waiters (accessed by accept_input_with_completion and RuntimeLoop).
    completions: SharedCompletionRegistry,
    /// Wake signal sender (if a RuntimeLoop is attached).
    wake_tx: Option<mpsc::Sender<()>>,
    /// Run-control sender for cancelling the current run.
    control_tx: Option<mpsc::Sender<RunControlCommand>>,
    /// Loop task handle (dropped on unregister, which closes the channel).
    _loop_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Wraps a SessionService to provide v9 runtime capabilities.
///
/// Maintains a per-session RuntimeDriver registry. When sessions are registered
/// with a `CoreExecutor`, a RuntimeLoop task is spawned that processes queued
/// inputs by calling `CoreExecutor::apply()` (which triggers
/// `SessionService::start_turn()` under the hood).
pub struct RuntimeSessionAdapter {
    /// Per-session entries.
    sessions: RwLock<HashMap<SessionId, RuntimeSessionEntry>>,
    /// Runtime mode.
    mode: RuntimeMode,
    /// Optional RuntimeStore for persistent drivers.
    store: Option<Arc<dyn RuntimeStore>>,
}

impl RuntimeSessionAdapter {
    /// Create an ephemeral adapter (all sessions use EphemeralRuntimeDriver).
    pub fn ephemeral() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            mode: RuntimeMode::V9Compliant,
            store: None,
        }
    }

    /// Create a persistent adapter with a RuntimeStore.
    pub fn persistent(store: Arc<dyn RuntimeStore>) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            mode: RuntimeMode::V9Compliant,
            store: Some(store),
        }
    }

    /// Create a driver entry for a session.
    fn make_driver(&self, session_id: &SessionId) -> DriverEntry {
        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        match &self.store {
            Some(store) => {
                DriverEntry::Persistent(PersistentRuntimeDriver::new(runtime_id, store.clone()))
            }
            None => DriverEntry::Ephemeral(EphemeralRuntimeDriver::new(runtime_id)),
        }
    }

    /// Register a runtime driver for a session (no RuntimeLoop — inputs queue but
    /// nothing processes them automatically). Useful for tests and legacy mode.
    pub async fn register_session(&self, session_id: SessionId) {
        if self.contains_session(&session_id).await {
            return;
        }
        let mut entry = self.make_driver(&session_id);
        if let Err(err) = entry.as_driver_mut().recover().await {
            tracing::error!(%session_id, error = %err, "failed to recover runtime driver during registration");
            return;
        }
        let session_entry = RuntimeSessionEntry {
            driver: Arc::new(Mutex::new(entry)),
            ops_lifecycle: Arc::new(crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()),
            completions: Arc::new(Mutex::new(crate::completion::CompletionRegistry::new())),
            wake_tx: None,
            control_tx: None,
            _loop_handle: None,
        };
        let mut sessions = self.sessions.write().await;
        sessions.entry(session_id).or_insert(session_entry);
    }

    /// Set the silent comms intents for a session's runtime driver.
    ///
    /// Peer requests whose intent matches one of these strings will be accepted
    /// without triggering an LLM turn (ApplyMode::Ignore, WakeMode::None).
    pub async fn set_session_silent_intents(&self, session_id: &SessionId, intents: Vec<String>) {
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
        self.ensure_session_with_executor(session_id, executor)
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
        let mut executor = Some(executor);
        let upgrade = {
            let mut sessions = self.sessions.write().await;
            if let Some(entry) = sessions.get_mut(&session_id) {
                if entry.wake_tx.is_some() && entry.control_tx.is_some() {
                    return;
                }

                let driver = Arc::clone(&entry.driver);
                let (wake_tx, wake_rx) = mpsc::channel(16);
                let (control_tx, control_rx) = mpsc::channel(16);
                let Some(executor) = executor.take() else {
                    tracing::error!(%session_id, "executor missing while upgrading existing runtime session");
                    return;
                };
                let handle = crate::runtime_loop::spawn_runtime_loop_with_completions(
                    driver.clone(),
                    executor,
                    wake_rx,
                    control_rx,
                    Some(entry.completions.clone()),
                );

                entry.wake_tx = Some(wake_tx.clone());
                entry.control_tx = Some(control_tx);
                entry._loop_handle = Some(handle);
                Some((driver, wake_tx))
            } else {
                None
            }
        };

        if let Some((driver, wake_tx)) = upgrade {
            let should_wake = {
                let mut driver = driver.lock().await;
                // Transition Idle → Attached now that an executor is wired up
                let _ = driver.attach();
                !driver.as_driver().active_input_ids().is_empty()
            };
            if should_wake {
                let _ = wake_tx.try_send(());
            }
            return;
        }

        let mut recovered_entry = self.make_driver(&session_id);
        if let Err(err) = recovered_entry.as_driver_mut().recover().await {
            tracing::error!(%session_id, error = %err, "failed to recover runtime driver during registration");
            return;
        }

        let driver = {
            let mut sessions = self.sessions.write().await;
            if let Some(entry) = sessions.get(&session_id) {
                if entry.wake_tx.is_some() && entry.control_tx.is_some() {
                    return;
                }
                entry.driver.clone()
            } else {
                let driver = Arc::new(Mutex::new(recovered_entry));
                sessions.insert(
                    session_id.clone(),
                    RuntimeSessionEntry {
                        driver: driver.clone(),
                        ops_lifecycle: Arc::new(
                            crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new(),
                        ),
                        completions: Arc::new(Mutex::new(
                            crate::completion::CompletionRegistry::new(),
                        )),
                        wake_tx: None,
                        control_tx: None,
                        _loop_handle: None,
                    },
                );
                driver
            }
        };

        let (wake_tx, wake_rx) = mpsc::channel(16);
        let (control_tx, control_rx) = mpsc::channel(16);
        let Some(executor) = executor.take() else {
            tracing::error!(%session_id, "executor missing while registering runtime session");
            return;
        };

        // Get or create completions for this session
        let completions = {
            let sessions = self.sessions.read().await;
            sessions.get(&session_id).map(|e| e.completions.clone())
        };

        let handle = crate::runtime_loop::spawn_runtime_loop_with_completions(
            driver.clone(),
            executor,
            wake_rx,
            control_rx,
            completions,
        );

        let mut sessions = self.sessions.write().await;
        let Some(entry) = sessions.get_mut(&session_id) else {
            return;
        };
        if entry.wake_tx.is_some() && entry.control_tx.is_some() {
            return;
        }
        entry.wake_tx = Some(wake_tx.clone());
        entry.control_tx = Some(control_tx);
        entry._loop_handle = Some(handle);
        drop(sessions);

        let should_wake = {
            let mut driver = driver.lock().await;
            // Transition Idle → Attached now that an executor is wired up
            let _ = driver.attach();
            !driver.as_driver().active_input_ids().is_empty()
        };
        if should_wake {
            let _ = wake_tx.try_send(());
        }
    }

    /// Unregister a session's runtime driver.
    ///
    /// Detaches the executor (Attached → Idle) before removal, then drops
    /// the wake channel sender, which causes the RuntimeLoop to exit.
    pub async fn unregister_session(&self, session_id: &SessionId) {
        let mut sessions = self.sessions.write().await;
        if let Some(entry) = sessions.get(session_id) {
            let mut driver = entry.driver.lock().await;
            let _ = driver.detach(); // Attached → Idle (no-op if not Attached)
            drop(driver);
        }
        sessions.remove(session_id);
    }

    /// Check whether a runtime driver is already registered for a session.
    pub async fn contains_session(&self, session_id: &SessionId) -> bool {
        self.sessions.read().await.contains_key(session_id)
    }

    /// Cancel the currently-running turn for a registered session.
    pub async fn interrupt_current_run(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        let Some(control_tx) = &entry.control_tx else {
            return Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            });
        };
        control_tx
            .send(RunControlCommand::CancelCurrentRun {
                reason: "mob interrupt".to_string(),
            })
            .await
            .map_err(|err| RuntimeDriverError::Internal(format!("failed to send interrupt: {err}")))
    }

    /// Stop the attached runtime executor through the out-of-band control
    /// channel. When no loop is attached yet, a stop command is applied directly
    /// against the driver so queued work is still terminated consistently.
    pub async fn stop_runtime_executor(
        &self,
        session_id: &SessionId,
        command: RunControlCommand,
    ) -> Result<(), RuntimeDriverError> {
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;

        if let Some(control_tx) = &entry.control_tx
            && control_tx.send(command.clone()).await.is_ok()
        {
            return Ok(());
        }

        if matches!(command, RunControlCommand::StopRuntimeExecutor { .. }) {
            let mut driver = entry.driver.lock().await;
            driver
                .as_driver_mut()
                .on_runtime_control(crate::traits::RuntimeControlCommand::Stop)
                .await?;
            drop(driver);
            let mut completions = entry.completions.lock().await;
            completions.resolve_all_terminated("runtime stopped");
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
        let driver = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?
                .driver
                .clone()
        };

        let (input_id, run_id, primitive) = {
            let mut driver = driver.lock().await;
            if !driver.is_idle_or_attached() || !driver.as_driver().active_input_ids().is_empty() {
                return Err(RuntimeDriverError::NotReady {
                    state: driver.as_driver().runtime_state(),
                });
            }
            let outcome = driver.as_driver_mut().accept_input(input).await?;
            let input_id = match outcome {
                AcceptOutcome::Accepted { input_id, .. } => input_id,
                AcceptOutcome::Deduplicated { existing_id, .. } => existing_id,
                AcceptOutcome::Rejected { reason } => {
                    return Err(RuntimeDriverError::ValidationFailed { reason });
                }
            };

            if !driver.is_idle_or_attached() {
                return Err(RuntimeDriverError::NotReady {
                    state: driver.as_driver().runtime_state(),
                });
            }

            let (dequeued_id, dequeued_input) = driver.dequeue_next().ok_or_else(|| {
                RuntimeDriverError::Internal("accepted input was not queued for execution".into())
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
                RuntimeDriverError::Internal(format!("failed to stage accepted input: {err}"))
            })?;
            let primitive = crate::runtime_loop::input_to_primitive(&dequeued_input, dequeued_id);
            (input_id, run_id, primitive)
        };

        match op(run_id.clone(), primitive.clone()).await {
            Ok((result, output)) => {
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
                    self.unregister_session(session_id).await;
                    return Err(RuntimeDriverError::Internal(format!(
                        "failed to persist runtime completion snapshot: {err}"
                    )));
                }
                Ok(result)
            }
            Err(err) => {
                let mut driver = driver.lock().await;
                if let Err(run_err) = driver
                    .as_driver_mut()
                    .on_run_event(meerkat_core::lifecycle::RunEvent::RunFailed {
                        run_id,
                        error: err.to_string(),
                        recoverable: true,
                    })
                    .await
                {
                    drop(driver);
                    self.unregister_session(session_id).await;
                    return Err(RuntimeDriverError::Internal(format!(
                        "failed to persist runtime failure snapshot: {run_err}"
                    )));
                }
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
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;

        let (outcome, should_wake, should_process, handle) = {
            let mut driver = entry.driver.lock().await;
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
                            let mut completions = entry.completions.lock().await;
                            completions.register(input_id.clone())
                        })
                    };
                    let wake = driver.take_wake_requested();
                    let process_now = driver.take_process_requested();
                    (result, wake, process_now, handle)
                }
                AcceptOutcome::Deduplicated { existing_id, .. } => {
                    // Check if the existing input is already terminal
                    let existing_state = driver.as_driver().input_state(existing_id);
                    let is_terminal = existing_state
                        .map(|s| s.current_state().is_terminal())
                        .unwrap_or(true); // missing state = already cleaned up = terminal

                    if is_terminal {
                        // Input already processed — no handle, no waiter
                        (result, false, false, None)
                    } else {
                        // In-flight — join existing waiters via multi-waiter Vec
                        let handle = {
                            let mut completions = entry.completions.lock().await;
                            completions.register(existing_id.clone())
                        };
                        (result, false, false, Some(handle))
                    }
                }
                AcceptOutcome::Rejected { reason } => {
                    return Err(RuntimeDriverError::ValidationFailed {
                        reason: reason.clone(),
                    });
                }
            }
        };

        if (should_wake || should_process)
            && let Some(ref wake_tx) = entry.wake_tx
        {
            let _ = wake_tx.try_send(());
        }

        Ok((outcome, handle))
    }

    /// Get the shared completion registry for a session.
    ///
    /// Used by the runtime loop to resolve waiters on input consumption.
    #[allow(dead_code)]
    pub(crate) async fn completion_registry(
        &self,
        session_id: &SessionId,
    ) -> Option<SharedCompletionRegistry> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).map(|e| e.completions.clone())
    }

    /// Get the shared ops lifecycle registry for a session/runtime instance.
    pub async fn ops_lifecycle_registry(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|e| Arc::clone(&e.ops_lifecycle))
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SessionServiceRuntimeExt for RuntimeSessionAdapter {
    fn runtime_mode(&self) -> RuntimeMode {
        self.mode
    }

    async fn accept_input(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;

        // Accept input and check wake under the driver lock
        let (outcome, should_wake, should_process) = {
            let mut driver = entry.driver.lock().await;
            let result = driver.as_driver_mut().accept_input(input).await?;
            let wake = driver.take_wake_requested();
            let process_now = driver.take_process_requested();
            (result, wake, process_now)
        };

        // Signal the RuntimeLoop if wake or immediate processing was requested.
        if (should_wake || should_process)
            && let Some(ref wake_tx) = entry.wake_tx
        {
            // Non-blocking: if the channel is full, the loop is already processing
            let _ = wake_tx.try_send(());
        }

        Ok(outcome)
    }

    async fn accept_input_with_completion(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<(AcceptOutcome, Option<crate::completion::CompletionHandle>), RuntimeDriverError>
    {
        RuntimeSessionAdapter::accept_input_with_completion(self, session_id, input).await
    }

    async fn runtime_state(
        &self,
        session_id: &SessionId,
    ) -> Result<RuntimeState, RuntimeDriverError> {
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        let driver = entry.driver.lock().await;
        Ok(driver.as_driver().runtime_state())
    }

    async fn retire_runtime(
        &self,
        session_id: &SessionId,
    ) -> Result<RetireReport, RuntimeDriverError> {
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        let mut driver = entry.driver.lock().await;
        let mut report = driver.as_driver_mut().retire().await?;
        drop(driver); // Release driver lock before waking

        if report.inputs_pending_drain > 0 {
            // Wake the runtime loop so it drains already-queued inputs.
            // Retired state allows processing but rejects new accepts.
            if let Some(ref wake_tx) = entry.wake_tx
                && wake_tx.send(()).await.is_ok()
            {
                return Ok(report);
            }

            // No live loop can drain this retired queue. Abandon the queued work
            // now so later recovery/upgrade paths do not execute inputs whose
            // waiters were already terminated.
            let mut driver = entry.driver.lock().await;
            let abandoned = driver
                .abandon_pending_inputs(crate::input_state::InputAbandonReason::Retired)
                .await?;
            drop(driver);
            let mut completions = entry.completions.lock().await;
            completions.resolve_all_terminated("retired without runtime loop");
            report.inputs_abandoned += abandoned;
            report.inputs_pending_drain = 0;
        }

        Ok(report)
    }

    async fn reset_runtime(
        &self,
        session_id: &SessionId,
    ) -> Result<ResetReport, RuntimeDriverError> {
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        let mut driver = entry.driver.lock().await;
        if matches!(driver.as_driver().runtime_state(), RuntimeState::Running) {
            return Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Running,
            });
        }
        let report = driver.as_driver_mut().reset().await?;

        // Resolve all pending completion waiters — reset discards all queued work
        let mut completions = entry.completions.lock().await;
        completions.resolve_all_terminated("runtime reset");

        Ok(report)
    }

    async fn input_state(
        &self,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<InputState>, RuntimeDriverError> {
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        let driver = entry.driver.lock().await;
        Ok(driver.as_driver().input_state(input_id).cloned())
    }

    async fn list_active_inputs(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<InputId>, RuntimeDriverError> {
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        let driver = entry.driver.lock().await;
        Ok(driver.as_driver().active_input_ids())
    }
}

// ---------------------------------------------------------------------------
// RuntimeControlPlane implementation
// ---------------------------------------------------------------------------

impl RuntimeSessionAdapter {
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
            entry.wake_tx.clone(),
        ))
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl crate::traits::RuntimeControlPlane for RuntimeSessionAdapter {
    async fn ingest(
        &self,
        runtime_id: &LogicalRuntimeId,
        input: Input,
    ) -> Result<AcceptOutcome, RuntimeControlPlaneError> {
        let (session_id, driver, _completions, wake_tx) = self.lookup_entry(runtime_id).await?;
        let _ = session_id;

        let (outcome, should_wake, should_process) = {
            let mut drv = driver.lock().await;
            let result = drv
                .as_driver_mut()
                .accept_input(input)
                .await
                .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
            let wake = drv.take_wake_requested();
            let process_now = drv.take_process_requested();
            (result, wake, process_now)
        };

        if (should_wake || should_process)
            && let Some(ref tx) = wake_tx
        {
            let _ = tx.try_send(());
        }

        Ok(outcome)
    }

    async fn publish_event(
        &self,
        event: crate::runtime_event::RuntimeEventEnvelope,
    ) -> Result<(), RuntimeControlPlaneError> {
        let runtime_id = event.runtime_id.clone();
        let (_session_id, driver, _completions, _wake_tx) = self.lookup_entry(&runtime_id).await?;

        let mut drv = driver.lock().await;
        drv.as_driver_mut()
            .on_runtime_event(event)
            .await
            .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))
    }

    async fn retire(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        let (session_id, driver, completions, wake_tx) = self.lookup_entry(runtime_id).await?;
        let _ = session_id;

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
                return Ok(report);
            }

            // No live loop — abandon queued work
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

        Ok(report)
    }

    async fn recycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RecycleReport, RuntimeControlPlaneError> {
        let (_session_id, driver, completions, wake_tx) = self.lookup_entry(runtime_id).await?;

        // Recycle = reset the driver to Idle and re-queue any recoverable inputs.
        // For persistent drivers, the store already has the input states; recovery
        // will replay them. For ephemeral drivers, inputs that survived are re-queued.
        let transferred = {
            let mut drv = driver.lock().await;
            let state = drv.as_driver().runtime_state();
            if matches!(state, RuntimeState::Running) {
                return Err(RuntimeControlPlaneError::InvalidState { state });
            }

            // Count non-terminal inputs before reset — these are transferred.
            let active = drv.as_driver().active_input_ids().len();

            // Reset to Idle, which re-queues Staged inputs and abandons nothing
            // that can be recovered.
            let _report = drv
                .as_driver_mut()
                .reset()
                .await
                .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;

            // Run recovery to rebuild queue from persisted state
            let _recovery = drv
                .as_driver_mut()
                .recover()
                .await
                .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;

            active
        };

        // Resolve existing completion waiters since inputs were recycled
        {
            let mut comp = completions.lock().await;
            comp.resolve_all_terminated("recycled");
        }

        // Wake the runtime loop to process re-queued inputs
        if let Some(ref tx) = wake_tx {
            let _ = tx.try_send(());
        }

        Ok(RecycleReport {
            inputs_transferred: transferred,
        })
    }

    async fn reset(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<crate::traits::ResetReport, RuntimeControlPlaneError> {
        let (_session_id, driver, completions, _wake_tx) = self.lookup_entry(runtime_id).await?;

        let mut drv = driver.lock().await;
        if matches!(drv.as_driver().runtime_state(), RuntimeState::Running) {
            return Err(RuntimeControlPlaneError::InvalidState {
                state: RuntimeState::Running,
            });
        }
        let report = drv
            .as_driver_mut()
            .reset()
            .await
            .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
        drop(drv);

        let mut comp = completions.lock().await;
        comp.resolve_all_terminated("runtime reset");

        Ok(report)
    }

    async fn recover(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RecoveryReport, RuntimeControlPlaneError> {
        let (_session_id, driver, _completions, wake_tx) = self.lookup_entry(runtime_id).await?;

        let mut drv = driver.lock().await;
        let report = drv
            .as_driver_mut()
            .recover()
            .await
            .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
        drop(drv);

        if let Some(ref tx) = wake_tx {
            let _ = tx.try_send(());
        }

        Ok(report)
    }

    async fn destroy(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<DestroyReport, RuntimeControlPlaneError> {
        let (_session_id, driver, completions, _wake_tx) = self.lookup_entry(runtime_id).await?;

        let mut drv = driver.lock().await;
        let report = drv
            .as_driver_mut()
            .destroy()
            .await
            .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
        drop(drv);

        let mut comp = completions.lock().await;
        comp.resolve_all_terminated("runtime destroyed");

        Ok(report)
    }

    async fn runtime_state(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RuntimeState, RuntimeControlPlaneError> {
        let (_session_id, driver, _completions, _wake_tx) = self.lookup_entry(runtime_id).await?;

        let drv = driver.lock().await;
        Ok(drv.as_driver().runtime_state())
    }

    async fn load_boundary_receipt(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<Option<meerkat_core::lifecycle::RunBoundaryReceipt>, RuntimeControlPlaneError> {
        match &self.store {
            Some(store) => store
                .load_boundary_receipt(runtime_id, run_id, sequence)
                .await
                .map_err(|e| RuntimeControlPlaneError::StoreError(e.to_string())),
            None => {
                // Ephemeral mode — no persisted receipts
                Ok(None)
            }
        }
    }
}
