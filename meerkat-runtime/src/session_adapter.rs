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
use tokio::sync::{Mutex, RwLock, mpsc};

use crate::accept::AcceptOutcome;
use crate::driver::ephemeral::EphemeralRuntimeDriver;
use crate::driver::persistent::PersistentRuntimeDriver;
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_machine::InputStateMachineError;
use crate::input_state::InputState;
use crate::runtime_state::{RuntimeState, RuntimeStateTransitionError};
use crate::service_ext::{RuntimeMode, SessionServiceRuntimeExt};
use crate::store::RuntimeStore;
use crate::traits::{ResetReport, RetireReport, RuntimeDriver, RuntimeDriverError};

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

    /// Check if the runtime is idle.
    pub(crate) fn is_idle(&self) -> bool {
        match self {
            DriverEntry::Ephemeral(d) => d.is_idle(),
            DriverEntry::Persistent(d) => d.is_idle(),
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
    ) -> Result<(), InputStateMachineError> {
        match self {
            DriverEntry::Ephemeral(d) => d.stage_input(input_id, run_id),
            DriverEntry::Persistent(d) => d.stage_input(input_id, run_id),
        }
    }

    /// Apply an input after successful immediate execution.
    pub(crate) fn apply_input(
        &mut self,
        input_id: &InputId,
        run_id: &RunId,
    ) -> Result<(), InputStateMachineError> {
        match self {
            DriverEntry::Ephemeral(d) => d.apply_input(input_id, run_id),
            DriverEntry::Persistent(d) => d.apply_input(input_id, run_id),
        }
    }

    /// Consume an input after successful immediate execution.
    pub(crate) fn consume_inputs(
        &mut self,
        input_ids: &[InputId],
        run_id: &RunId,
    ) -> Result<(), InputStateMachineError> {
        match self {
            DriverEntry::Ephemeral(d) => d.consume_inputs(input_ids, run_id),
            DriverEntry::Persistent(d) => d.consume_inputs(input_ids, run_id),
        }
    }

    /// Roll back staged inputs after failed immediate execution.
    pub(crate) fn rollback_staged(
        &mut self,
        input_ids: &[InputId],
    ) -> Result<(), InputStateMachineError> {
        match self {
            DriverEntry::Ephemeral(d) => d.rollback_staged(input_ids),
            DriverEntry::Persistent(d) => d.rollback_staged(input_ids),
        }
    }
}

/// Per-session state: driver + optional RuntimeLoop.
struct RuntimeSessionEntry {
    /// Shared driver handle (accessed by both adapter methods and RuntimeLoop).
    driver: SharedDriver,
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

    /// Create a legacy/degraded adapter (no runtime capabilities).
    pub fn legacy() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            mode: RuntimeMode::LegacyDegraded,
            store: None,
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
            wake_tx: None,
            control_tx: None,
            _loop_handle: None,
        };
        let mut sessions = self.sessions.write().await;
        sessions.entry(session_id).or_insert(session_entry);
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
    /// If a driver is already registered, the provided executor is dropped and
    /// the existing driver is preserved.
    pub async fn ensure_session_with_executor(
        &self,
        session_id: SessionId,
        executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    ) {
        if self.contains_session(&session_id).await {
            return;
        }

        let mut entry = self.make_driver(&session_id);
        if let Err(err) = entry.as_driver_mut().recover().await {
            tracing::error!(%session_id, error = %err, "failed to recover runtime driver during registration");
            return;
        }
        let driver = Arc::new(Mutex::new(entry));
        let (wake_tx, wake_rx) = mpsc::channel(16);
        let (control_tx, control_rx) = mpsc::channel(16);
        let handle =
            crate::runtime_loop::spawn_runtime_loop(driver.clone(), executor, wake_rx, control_rx);
        let session_entry = RuntimeSessionEntry {
            driver,
            wake_tx: Some(wake_tx),
            control_tx: Some(control_tx),
            _loop_handle: Some(handle),
        };
        let mut sessions = self.sessions.write().await;
        if sessions.contains_key(&session_id) {
            return;
        }
        sessions.insert(session_id, session_entry);
    }

    /// Unregister a session's runtime driver.
    ///
    /// Drops the wake channel sender, which causes the RuntimeLoop to exit.
    pub async fn unregister_session(&self, session_id: &SessionId) {
        self.sessions.write().await.remove(session_id);
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
            if !driver.is_idle() || !driver.as_driver().active_input_ids().is_empty() {
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

            if !driver.is_idle() {
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
                    return Err(RuntimeDriverError::Internal(format!(
                        "runtime boundary commit failed: {err}"
                    )));
                }
                driver
                    .as_driver_mut()
                    .on_run_event(meerkat_core::lifecycle::RunEvent::RunCompleted {
                        run_id,
                        consumed_input_ids: vec![input_id],
                    })
                    .await?;
                Ok(result)
            }
            Err(err) => {
                let mut driver = driver.lock().await;
                driver
                    .as_driver_mut()
                    .on_run_event(meerkat_core::lifecycle::RunEvent::RunFailed {
                        run_id,
                        error: err.to_string(),
                        recoverable: true,
                    })
                    .await?;
                Err(err)
            }
        }
    }
}

#[async_trait::async_trait]
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
        driver.as_driver_mut().retire().await
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
        driver.as_driver_mut().reset().await
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input::*;
    use chrono::Utc;

    fn make_prompt(text: &str) -> Input {
        Input::Prompt(PromptInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            text: text.into(),
            turn_metadata: None,
        })
    }

    #[tokio::test]
    async fn ephemeral_adapter_accept_and_query() {
        let adapter = RuntimeSessionAdapter::ephemeral();
        let sid = SessionId::new();
        adapter.register_session(sid.clone()).await;

        let input = make_prompt("hello");
        let outcome = adapter.accept_input(&sid, input).await.unwrap();
        assert!(outcome.is_accepted());

        let state = adapter.runtime_state(&sid).await.unwrap();
        assert_eq!(state, RuntimeState::Idle);

        let active = adapter.list_active_inputs(&sid).await.unwrap();
        assert_eq!(active.len(), 1);
    }

    #[tokio::test]
    async fn persistent_adapter_accept() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let adapter = RuntimeSessionAdapter::persistent(store);
        let sid = SessionId::new();
        adapter.register_session(sid.clone()).await;

        let input = make_prompt("hello");
        let outcome = adapter.accept_input(&sid, input).await.unwrap();
        assert!(outcome.is_accepted());
    }

    #[tokio::test]
    async fn legacy_mode() {
        let adapter = RuntimeSessionAdapter::legacy();
        assert_eq!(adapter.runtime_mode(), RuntimeMode::LegacyDegraded);
    }

    #[tokio::test]
    async fn unregistered_session_errors() {
        let adapter = RuntimeSessionAdapter::ephemeral();
        let sid = SessionId::new();
        let result = adapter.accept_input(&sid, make_prompt("hi")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn unregister_removes_driver() {
        let adapter = RuntimeSessionAdapter::ephemeral();
        let sid = SessionId::new();
        adapter.register_session(sid.clone()).await;
        adapter.unregister_session(&sid).await;

        let result = adapter.runtime_state(&sid).await;
        assert!(result.is_err());
    }

    /// Test that accept_input with a RuntimeLoop triggers input processing.
    #[tokio::test]
    async fn accept_with_executor_triggers_loop() {
        use meerkat_core::lifecycle::RunId;
        use meerkat_core::lifecycle::core_executor::{
            CoreApplyOutput, CoreExecutor, CoreExecutorError,
        };
        use meerkat_core::lifecycle::run_control::RunControlCommand;
        use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
        use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
        use std::sync::atomic::{AtomicBool, Ordering};

        // Track whether apply was called
        let apply_called = Arc::new(AtomicBool::new(false));
        let apply_called_clone = apply_called.clone();

        struct TestExecutor {
            called: Arc<AtomicBool>,
        }

        #[async_trait::async_trait]
        impl CoreExecutor for TestExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                self.called.store(true, Ordering::SeqCst);
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
                })
            }

            async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
                Ok(())
            }
        }

        let adapter = RuntimeSessionAdapter::ephemeral();
        let sid = SessionId::new();
        let executor = Box::new(TestExecutor {
            called: apply_called_clone,
        });
        adapter
            .register_session_with_executor(sid.clone(), executor)
            .await;

        // Accept input — should trigger the loop
        let input = make_prompt("hello from executor test");
        let outcome = adapter.accept_input(&sid, input).await.unwrap();
        assert!(outcome.is_accepted());

        // Give the loop time to process
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert!(
            apply_called.load(Ordering::SeqCst),
            "CoreExecutor::apply() should have been called by the RuntimeLoop"
        );

        // After processing, the input should be consumed and the runtime back to Idle
        let state = adapter.runtime_state(&sid).await.unwrap();
        assert_eq!(state, RuntimeState::Idle);

        // The input should be consumed (terminal)
        let active = adapter.list_active_inputs(&sid).await.unwrap();
        assert!(active.is_empty(), "All inputs should be consumed");
    }

    /// Test that a failed executor re-queues the input (not stranded in APC).
    #[tokio::test]
    async fn failed_executor_requeues_input() {
        use crate::input_state::InputLifecycleState;
        use meerkat_core::lifecycle::core_executor::{
            CoreApplyOutput, CoreExecutor, CoreExecutorError,
        };
        use meerkat_core::lifecycle::run_control::RunControlCommand;
        use meerkat_core::lifecycle::run_primitive::RunPrimitive;
        struct FailingExecutor;

        #[async_trait::async_trait]
        impl CoreExecutor for FailingExecutor {
            async fn apply(
                &mut self,
                _run_id: RunId,
                _primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
                Err(CoreExecutorError::ApplyFailed {
                    reason: "LLM error".into(),
                })
            }

            async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
                Ok(())
            }
        }

        let adapter = RuntimeSessionAdapter::ephemeral();
        let sid = SessionId::new();
        adapter
            .register_session_with_executor(sid.clone(), Box::new(FailingExecutor))
            .await;

        let input = make_prompt("hello failing");
        let input_id = input.id().clone();
        adapter.accept_input(&sid, input).await.unwrap();

        // Give the loop time to process and fail
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Runtime should be back to Idle (not stuck in Running)
        let state = adapter.runtime_state(&sid).await.unwrap();
        assert_eq!(state, RuntimeState::Idle);

        // Input should be rolled back to Queued (not stranded in APC)
        let is = adapter.input_state(&sid, &input_id).await.unwrap().unwrap();
        assert_eq!(
            is.current_state,
            InputLifecycleState::Queued,
            "Failed execution should roll input back to Queued, not strand in AppliedPendingConsumption"
        );
    }

    /// Test that BoundaryApplied fires with correct receipt on success.
    #[tokio::test]
    async fn successful_execution_fires_boundary_applied() {
        use crate::input_state::InputLifecycleState;
        use meerkat_core::lifecycle::RunId;
        use meerkat_core::lifecycle::core_executor::{
            CoreApplyOutput, CoreExecutor, CoreExecutorError,
        };
        use meerkat_core::lifecycle::run_control::RunControlCommand;
        use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
        use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;

        struct SuccessExecutor;

        #[async_trait::async_trait]
        impl CoreExecutor for SuccessExecutor {
            async fn apply(
                &mut self,
                run_id: RunId,
                primitive: RunPrimitive,
            ) -> Result<CoreApplyOutput, CoreExecutorError> {
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
                })
            }

            async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
                Ok(())
            }
        }

        let adapter = RuntimeSessionAdapter::ephemeral();
        let sid = SessionId::new();
        adapter
            .register_session_with_executor(sid.clone(), Box::new(SuccessExecutor))
            .await;

        let input = make_prompt("hello success");
        let input_id = input.id().clone();
        adapter.accept_input(&sid, input).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Input should have gone through full lifecycle: Queued → Staged → Applied → APC → Consumed
        let is = adapter.input_state(&sid, &input_id).await.unwrap().unwrap();
        assert_eq!(
            is.current_state,
            InputLifecycleState::Consumed,
            "Successful execution should consume the input"
        );

        // Runtime should be back to Idle
        let state = adapter.runtime_state(&sid).await.unwrap();
        assert_eq!(state, RuntimeState::Idle);
    }
}
