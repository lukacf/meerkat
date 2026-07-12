use super::*;

type OpsLifecyclePersistenceReceiver = crate::tokio::sync::mpsc::UnboundedReceiver<
    crate::ops_lifecycle::OpsLifecyclePersistenceRequest,
>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum UnregisterTeardownCaller {
    Explicit,
    RuntimeLoopWatcher,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuntimeStopCleanupCaller {
    ExplicitStop,
    ExplicitUnregister,
    RuntimeLoopWatcher,
}

enum RuntimeStopCleanupWork {
    Request { reason: String },
    CleanupOnly,
}

/// Maximum time a caller waits synchronously for the independently-owned
/// unregister saga. Elapsing this grace never cancels the saga or its exact
/// runtime-loop JoinHandle; it only returns typed in-progress truth.
const UNREGISTER_CALLER_WAIT_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Maximum time an explicit stop caller waits for the independently-owned
/// cleanup coordinator. The coordinator and exact executor remain owned after
/// this elapses; only the caller receives typed in-progress truth.
const RUNTIME_STOP_CALLER_WAIT_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Live interrupt delivery is cooperative and therefore cannot be allowed to
/// hold the owned unregister saga forever. Dropping an elapsed interrupt
/// future does not touch the exact executor, which remains owned by the loop.
const UNREGISTER_INTERRUPT_DELIVERY_GRACE: std::time::Duration =
    std::time::Duration::from_millis(250);

std::thread_local! {
    /// Coordinator identity is scoped to each poll of the owned saga future.
    ///
    /// Tokio exposes task IDs on native targets, but the browser runtime does
    /// not. Poll scoping preserves exact self-join detection on both runtimes:
    /// concurrent tasks on one thread restore the prior value before another
    /// future can be polled, while a native task may freely migrate threads
    /// between polls.
    static ACTIVE_UNREGISTER_COORDINATOR: std::cell::Cell<Option<uuid::Uuid>> =
        const { std::cell::Cell::new(None) };
    static ACTIVE_RUNTIME_STOP_COORDINATOR: std::cell::Cell<Option<uuid::Uuid>> =
        const { std::cell::Cell::new(None) };
}

struct UnregisterCoordinatorPollScope<F> {
    coordinator_id: uuid::Uuid,
    future: Pin<Box<F>>,
}

struct RestoreUnregisterCoordinator(Option<uuid::Uuid>);

impl Drop for RestoreUnregisterCoordinator {
    fn drop(&mut self) {
        ACTIVE_UNREGISTER_COORDINATOR.with(|active| active.set(self.0));
    }
}

impl<F: Future> Future for UnregisterCoordinatorPollScope<F> {
    type Output = F::Output;

    fn poll(
        self: Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = self.get_mut();
        let previous =
            ACTIVE_UNREGISTER_COORDINATOR.with(|active| active.replace(Some(this.coordinator_id)));
        let _restore = RestoreUnregisterCoordinator(previous);
        this.future.as_mut().poll(context)
    }
}

fn unregister_coordinator_poll_scope<F: Future>(
    coordinator_id: uuid::Uuid,
    future: F,
) -> UnregisterCoordinatorPollScope<F> {
    UnregisterCoordinatorPollScope {
        coordinator_id,
        future: Box::pin(future),
    }
}

fn active_unregister_coordinator() -> Option<uuid::Uuid> {
    ACTIVE_UNREGISTER_COORDINATOR.with(std::cell::Cell::get)
}

struct RuntimeStopCoordinatorPollScope<F> {
    coordinator_id: uuid::Uuid,
    future: Pin<Box<F>>,
}

struct RestoreRuntimeStopCoordinator(Option<uuid::Uuid>);

impl Drop for RestoreRuntimeStopCoordinator {
    fn drop(&mut self) {
        ACTIVE_RUNTIME_STOP_COORDINATOR.with(|active| active.set(self.0));
    }
}

impl<F: Future> Future for RuntimeStopCoordinatorPollScope<F> {
    type Output = F::Output;

    fn poll(
        self: Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = self.get_mut();
        let previous = ACTIVE_RUNTIME_STOP_COORDINATOR
            .with(|active| active.replace(Some(this.coordinator_id)));
        let _restore = RestoreRuntimeStopCoordinator(previous);
        this.future.as_mut().poll(context)
    }
}

fn runtime_stop_coordinator_poll_scope<F: Future>(
    coordinator_id: uuid::Uuid,
    future: F,
) -> RuntimeStopCoordinatorPollScope<F> {
    RuntimeStopCoordinatorPollScope {
        coordinator_id,
        future: Box::pin(future),
    }
}

fn active_runtime_stop_coordinator() -> Option<uuid::Uuid> {
    ACTIVE_RUNTIME_STOP_COORDINATOR.with(std::cell::Cell::get)
}

#[derive(Debug, Clone)]
pub(super) struct RuntimeOpsLifecycleDurabilityAuthority {
    action: crate::meerkat_machine::dsl::RuntimeOpsLifecycleDurabilityAction,
}

#[derive(Debug, Clone)]
struct RuntimeLifecycleRecoveryObservation {
    runtime_state: RuntimeState,
    agent_runtime_id: Option<LogicalRuntimeId>,
    fence_token: Option<u64>,
    runtime_generation: Option<crate::meerkat_machine::dsl::Generation>,
    runtime_epoch_id: Option<crate::meerkat_machine::dsl::RuntimeEpochId>,
    unregister_progress: Option<crate::store::MachineUnregisterProgressSnapshot>,
    recovered_from_snapshot: bool,
}

impl RuntimeLifecycleRecoveryObservation {
    fn from_snapshot(snapshot: Option<crate::store::MachineLifecycleSnapshot>) -> Self {
        let Some(snapshot) = snapshot else {
            return Self {
                runtime_state: RuntimeState::Idle,
                agent_runtime_id: None,
                fence_token: None,
                runtime_generation: None,
                runtime_epoch_id: None,
                unregister_progress: None,
                recovered_from_snapshot: false,
            };
        };
        let binding = snapshot.binding();
        Self {
            runtime_state: snapshot.runtime_state(),
            agent_runtime_id: binding
                .agent_runtime_id()
                .map(|value| LogicalRuntimeId::new(value.to_owned())),
            fence_token: binding.fence_token(),
            runtime_generation: binding
                .runtime_generation()
                .map(crate::meerkat_machine::dsl::Generation::from),
            runtime_epoch_id: binding
                .runtime_epoch_id()
                .map(crate::meerkat_machine::dsl::RuntimeEpochId::from),
            unregister_progress: snapshot.unregister_progress().cloned(),
            recovered_from_snapshot: true,
        }
    }

    fn requires_observed_recovery(&self) -> bool {
        self.recovered_from_snapshot
            && (self.runtime_state != RuntimeState::Idle
                || self.agent_runtime_id.is_some()
                || self.fence_token.is_some()
                || self.runtime_generation.is_some()
                || self.runtime_epoch_id.is_some()
                || self.unregister_progress.is_some())
    }
}

fn fresh_registered_runtime_authority(
    session_id: &SessionId,
    context: &'static str,
) -> Result<crate::meerkat_machine::dsl::MeerkatMachineAuthority, RuntimeDriverError> {
    let mut authority = super::dsl_authority::new_initialized_authority(context);
    crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        &mut authority,
        crate::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
            session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
        },
    )
    .map_err(|err| {
        RuntimeDriverError::Internal(super::dsl_authority::map_error(
            err,
            "fresh session registration",
        ))
    })?;
    Ok(authority)
}

pub(super) fn replay_durable_unregister_progress(
    authority: &mut crate::meerkat_machine::dsl::MeerkatMachineAuthority,
    session_id: &SessionId,
    progress: Option<&crate::store::MachineUnregisterProgressSnapshot>,
) -> Result<(), RuntimeDriverError> {
    let Some(progress) = progress else {
        return Ok(());
    };
    let state = authority.state();
    let begin = crate::meerkat_machine::dsl::MeerkatMachineInput::BeginUnregisterSession {
        session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
        agent_runtime_id: state.active_runtime_id.clone(),
        fence_token: state.active_fence_token,
        generation: state.active_runtime_generation,
        runtime_epoch_id: state.active_runtime_epoch_id.clone(),
    };
    crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(authority, begin).map_err(
        |error| RuntimeDriverError::RecoveryCorruption {
            reason: format!(
                "failed to replay durable unregister drain for session {session_id}: {error}"
            ),
        },
    )?;

    let mut feedback = Vec::new();
    if !progress.runtime_loop_drain_pending() {
        feedback.push(
            crate::meerkat_machine::dsl::MeerkatMachineInput::RuntimeLoopStoppedForUnregister {
                session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
                forced_abort: progress.runtime_loop_forced_abort(),
            },
        );
    }
    if !progress.comms_drain_exit_pending() {
        feedback.push(
            crate::meerkat_machine::dsl::MeerkatMachineInput::CommsDrainExitedForUnregister {
                session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
                forced_abort: progress.comms_drain_forced_abort(),
            },
        );
    }
    if !progress.completion_waiter_drain_pending() {
        feedback.push(
            crate::meerkat_machine::dsl::MeerkatMachineInput::CompletionWaitersResolvedForUnregister {
                session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
            },
        );
    }
    for input in feedback {
        crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(authority, input).map_err(
            |error| RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "failed to replay durable unregister feedback for session {session_id}: {error}"
                ),
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod unregister_progress_recovery_tests {
    use super::*;

    #[test]
    fn durable_unregister_progress_replays_through_generated_feedback() {
        let session_id = SessionId::new();
        let mut authority = fresh_registered_runtime_authority(
            &session_id,
            "durable unregister progress replay test",
        )
        .expect("fresh authority");
        let progress =
            crate::store::MachineUnregisterProgressSnapshot::new(false, true, false, true, false);

        replay_durable_unregister_progress(&mut authority, &session_id, Some(&progress))
            .expect("generated unregister progress replay");

        let state = authority.state();
        assert_eq!(
            state.registration_phase,
            crate::meerkat_machine::dsl::RegistrationPhase::Draining
        );
        assert!(!state.unregister_runtime_loop_drain_pending);
        assert!(state.unregister_comms_drain_exit_pending);
        assert!(!state.unregister_completion_waiter_drain_pending);
        assert!(state.unregister_runtime_loop_forced_abort);
        assert!(!state.unregister_comms_drain_forced_abort);
    }

    #[test]
    fn crash_recovered_pending_producers_close_as_forced_process_loss() {
        let session_id = SessionId::new();
        let progress =
            crate::store::MachineUnregisterProgressSnapshot::new(true, true, true, false, false);
        let observations = UnregisterTeardownMechanicalObservations::from_durable_process_recovery(
            Some(&progress),
        );
        assert!(
            observations
                .runtime_loop_forced_abort
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert!(
            observations
                .comms_drain_forced_abort
                .load(std::sync::atomic::Ordering::Acquire)
        );

        let mut authority = fresh_registered_runtime_authority(
            &session_id,
            "crash-recovered unregister process-loss test",
        )
        .expect("fresh authority");
        replay_durable_unregister_progress(&mut authority, &session_id, Some(&progress))
            .expect("durable BeginUnregister replay");
        for input in [
            crate::meerkat_machine::dsl::MeerkatMachineInput::RuntimeLoopStoppedForUnregister {
                session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
                forced_abort: observations
                    .runtime_loop_forced_abort
                    .load(std::sync::atomic::Ordering::Acquire),
            },
            crate::meerkat_machine::dsl::MeerkatMachineInput::CommsDrainExitedForUnregister {
                session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
                forced_abort: observations
                    .comms_drain_forced_abort
                    .load(std::sync::atomic::Ordering::Acquire),
            },
        ] {
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(&mut authority, input)
                .expect("recovered producer feedback must apply");
        }
        let state = authority.state();
        assert!(!state.unregister_runtime_loop_drain_pending);
        assert!(!state.unregister_comms_drain_exit_pending);
        assert!(state.unregister_runtime_loop_forced_abort);
        assert!(state.unregister_comms_drain_forced_abort);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod ops_persistence_worker_tests {
    use super::*;
    use meerkat_core::ops_lifecycle::{
        OperationId, OperationKind, OperationSpec, OpsLifecycleError, OpsLifecycleRegistry,
    };

    #[tokio::test]
    async fn unregister_closes_and_joins_ops_persistence_before_late_callback() {
        let store: Arc<dyn RuntimeStore> = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let runtime_id = LogicalRuntimeId::new("ops-worker-unregister-test");
        let epoch_id = meerkat_core::RuntimeEpochId::new();
        let cursor_state = Arc::new(meerkat_core::EpochCursorState::new());
        let registry = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new();
        let (persist_tx, persist_rx) = crate::tokio::sync::mpsc::unbounded_channel();
        let worker = spawn_ops_lifecycle_persistence_worker(
            Arc::clone(&store),
            runtime_id.clone(),
            persist_rx,
        )
        .expect("persistence worker");
        registry.set_persistence_channel(persist_tx, epoch_id.clone(), cursor_state);

        let operation_id = OperationId::new();
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: SessionId::new(),
                display_name: "detached callback".into(),
                source_label: "ops worker test".into(),
                operation_source: None,
                child_session_id: None,
                expect_peer_channel: false,
            })
            .unwrap();
        registry.provisioning_succeeded(&operation_id).unwrap();
        registry
            .retire_owner_for_unregister("test unregister".into())
            .unwrap();
        join_ops_lifecycle_persistence_worker(worker)
            .await
            .expect("closed persistence worker must join");

        let persisted = store
            .load_ops_lifecycle(&runtime_id)
            .await
            .unwrap()
            .expect("terminal owner snapshot must be durable before join");
        assert_eq!(persisted.epoch_id, epoch_id);
        assert_eq!(
            registry.report_progress(
                &operation_id,
                meerkat_core::ops_lifecycle::OperationProgressUpdate {
                    message: "late".into(),
                    percent: None,
                },
            ),
            Err(OpsLifecycleError::OwnerRetired)
        );
    }
}

fn runtime_ops_lifecycle_durability_authority_from_effects(
    session_id: &SessionId,
    effects: &[crate::meerkat_machine::dsl::MeerkatMachineEffect],
) -> Result<RuntimeOpsLifecycleDurabilityAuthority, RuntimeDriverError> {
    let expected_session_id = crate::meerkat_machine::dsl::SessionId::from_domain(session_id);
    effects
        .iter()
        .find_map(|effect| match effect {
            crate::meerkat_machine::dsl::MeerkatMachineEffect::RuntimeOpsLifecycleDurabilityResolved {
                session_id,
                action,
                ..
            } if session_id == &expected_session_id => {
                Some(RuntimeOpsLifecycleDurabilityAuthority { action: *action })
            }
            _ => None,
        })
        .ok_or_else(|| {
            RuntimeDriverError::Internal(format!(
                "UnregisterSession for session '{session_id}' emitted no RuntimeOpsLifecycleDurabilityResolved effect"
            ))
        })
}

async fn persist_ops_lifecycle_request(
    store: &Arc<dyn RuntimeStore>,
    runtime_id: &LogicalRuntimeId,
    request: crate::ops_lifecycle::OpsLifecyclePersistenceRequest,
) {
    let result = store
        .persist_ops_lifecycle(runtime_id, request.snapshot())
        .await
        .map_err(|error| {
            meerkat_core::ops_lifecycle::OpsLifecycleError::Internal(format!(
                "failed to persist ops lifecycle snapshot: {error}"
            ))
        });
    if let Err(error) = &result {
        tracing::warn!(
            %runtime_id,
            error = %error,
            "failed to persist ops lifecycle snapshot"
        );
    }
    request.complete(result);
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_ops_lifecycle_persistence_worker(
    store: Arc<dyn RuntimeStore>,
    runtime_id: LogicalRuntimeId,
    mut persist_rx: OpsLifecyclePersistenceReceiver,
) -> Result<OpsLifecyclePersistenceWorker, RuntimeDriverError> {
    let thread_name = format!("ops-lifecycle-persist-{runtime_id}");
    let worker_runtime_id = runtime_id.clone();
    let handle = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let runtime = match crate::tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::error!(
                        %worker_runtime_id,
                        error = %error,
                        "failed to start ops lifecycle persistence worker runtime"
                    );
                    return;
                }
            };
            runtime.block_on(async move {
                while let Some(request) = persist_rx.recv().await {
                    persist_ops_lifecycle_request(&store, &worker_runtime_id, request).await;
                }
            });
        })
        .map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "failed to spawn ops lifecycle persistence worker for {runtime_id}: {error}"
            ))
        })?;
    Ok(OpsLifecyclePersistenceWorker { handle })
}

#[cfg(target_arch = "wasm32")]
fn spawn_ops_lifecycle_persistence_worker(
    store: Arc<dyn RuntimeStore>,
    runtime_id: LogicalRuntimeId,
    mut persist_rx: OpsLifecyclePersistenceReceiver,
) -> Result<OpsLifecyclePersistenceWorker, RuntimeDriverError> {
    let handle = crate::tokio::spawn(async move {
        while let Some(request) = persist_rx.recv().await {
            persist_ops_lifecycle_request(&store, &runtime_id, request).await;
        }
    });
    Ok(OpsLifecyclePersistenceWorker { handle })
}

#[cfg(not(target_arch = "wasm32"))]
async fn join_ops_lifecycle_persistence_worker(
    worker: OpsLifecyclePersistenceWorker,
) -> Result<(), RuntimeDriverError> {
    crate::tokio::task::spawn_blocking(move || worker.handle.join())
        .await
        .map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "ops lifecycle persistence join task failed: {error}"
            ))
        })?
        .map_err(|_| {
            RuntimeDriverError::Internal("ops lifecycle persistence worker panicked".into())
        })
}

#[cfg(target_arch = "wasm32")]
async fn join_ops_lifecycle_persistence_worker(
    worker: OpsLifecyclePersistenceWorker,
) -> Result<(), RuntimeDriverError> {
    worker.handle.await.map_err(|error| {
        RuntimeDriverError::Internal(format!("ops lifecycle persistence worker failed: {error}"))
    })
}

impl MeerkatMachine {
    async fn durable_lifecycle_for_registration(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<crate::store::MachineLifecycleSnapshot>, RuntimeDriverError> {
        let Some(store) = self.store.as_ref() else {
            return Ok(None);
        };
        crate::store::load_machine_lifecycle(store.as_ref(), runtime_id)
            .await
            .map_err(|err| RuntimeDriverError::Internal(err.to_string()))
    }

    pub(super) async fn register_session_inner(
        &self,
        session_id: SessionId,
    ) -> Result<bool, RuntimeDriverError> {
        let storeless = self.store.is_none();
        tracing::debug!(%session_id, storeless, "MeerkatMachine::register_session_inner start");
        #[cfg(target_arch = "wasm32")]
        if storeless {
            {
                tracing::debug!(%session_id, "MeerkatMachine::register_session_inner attempting storeless existing check lock");
                let mut sessions = self.sessions.try_write().map_err(|_| {
                    tracing::warn!(
                        %session_id,
                        "storeless session map busy while checking existing registration"
                    );
                    RuntimeDriverError::Internal(format!(
                        "storeless session map busy while registering {session_id}"
                    ))
                })?;
                tracing::debug!(%session_id, "MeerkatMachine::register_session_inner locked storeless existing check");
                if let Some(existing) = sessions.get_mut(&session_id) {
                    tracing::debug!(
                        %session_id,
                        "MeerkatMachine::register_session_inner found existing session"
                    );
                    if existing.clear_dead_attachment() {
                        existing.stage_generated_executor_exit_observation().map_err(|reason| {
                            RuntimeDriverError::Internal(format!(
                                "generated MeerkatMachine rejected executor-exit observation: {reason}"
                            ))
                        })?;
                    }
                    return Ok(false);
                }
            }
            return self.register_storeless_session_inner_sync_build_step(session_id);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if storeless {
            return Box::pin(self.register_storeless_session_inner(session_id)).await;
        }
        Box::pin(self.register_session_inner_impl(session_id)).await
    }

    #[cfg(target_arch = "wasm32")]
    #[inline(never)]
    #[allow(dead_code)]
    fn register_storeless_session_inner_sync(
        &self,
        session_id: SessionId,
    ) -> Result<bool, RuntimeDriverError> {
        tracing::debug!(%session_id, "MeerkatMachine::register_storeless_session_inner_sync start");
        {
            tracing::debug!(%session_id, "MeerkatMachine::register_storeless_session_inner_sync attempting existing check lock");
            let mut sessions = self.sessions.try_write().map_err(|_| {
                tracing::warn!(
                    %session_id,
                    "storeless session map busy while checking existing registration"
                );
                RuntimeDriverError::Internal(format!(
                    "storeless session map busy while registering {session_id}"
                ))
            })?;
            tracing::debug!(%session_id, "MeerkatMachine::register_storeless_session_inner_sync locked existing check");
            if let Some(existing) = sessions.get_mut(&session_id) {
                tracing::debug!(
                    %session_id,
                    "MeerkatMachine::register_session_inner found existing session"
                );
                if let Some(error) = existing.registration_blocked_by_unregister(&session_id) {
                    return Err(error);
                }
                if existing.clear_dead_attachment() {
                    existing.stage_generated_executor_exit_observation().map_err(|reason| {
                        RuntimeDriverError::Internal(format!(
                            "generated MeerkatMachine rejected executor-exit observation: {reason}"
                        ))
                    })?;
                }
                return Ok(false);
            }
        }
        self.register_storeless_session_inner_sync_build_step(session_id)
    }

    #[cfg(target_arch = "wasm32")]
    #[inline(never)]
    pub(super) fn register_storeless_session_inner_sync_build_step(
        &self,
        session_id: SessionId,
    ) -> Result<bool, RuntimeDriverError> {
        let (runtime_id, session_entry) = self.make_storeless_session_entry_sync(&session_id)?;
        self.insert_storeless_session_sync(session_id, runtime_id, session_entry)
    }

    #[cfg(target_arch = "wasm32")]
    #[inline(never)]
    fn make_storeless_session_entry_sync(
        &self,
        session_id: &SessionId,
    ) -> Result<(LogicalRuntimeId, RuntimeSessionEntry), RuntimeDriverError> {
        let runtime_id = Self::logical_runtime_id(session_id);
        let recovered_authority =
            fresh_registered_runtime_authority(session_id, "fresh storeless session registration")?;
        let initial_runtime_state =
            super::dsl_authority::runtime_phase_from_authority(&recovered_authority);
        let dsl_authority = Arc::new(std::sync::Mutex::new(recovered_authority));
        let entry = self.make_driver(
            runtime_id.clone(),
            Arc::clone(&dsl_authority),
            initial_runtime_state,
        );
        let control_projection = entry.control_projection_handle();
        let (ops_lifecycle, epoch_id, cursor_state) = Self::fresh_ops_state();
        let handle_teardown_gate = crate::handles::HandleTeardownGate::open();
        let tool_visibility_owner = Arc::new(MachineToolVisibilityOwner::new());
        tool_visibility_owner.bind_dsl_authority(Arc::clone(&dsl_authority));
        let session_entry = RuntimeSessionEntry {
            runtime_id: runtime_id.clone(),
            mutation_gate: Arc::new(Mutex::new(())),
            supervisor_rotation_task: Arc::new(SupervisorRotationTaskSlot::new()),
            control_projection,
            driver: Arc::new(Mutex::new(entry)),
            ops_lifecycle,
            ops_lifecycle_persistence_worker: None,
            epoch_id,
            handle_teardown_gate,
            cursor_state,
            completions: Arc::new(Mutex::new(crate::completion::CompletionRegistry::new())),
            tool_visibility_owner,
            attachment_slot: RuntimeLoopAttachmentSlot::Empty,
            runtime_loop_teardown: None,
            unregister_coordinator: None,
            runtime_stop_cleanup_coordinator: None,
            pending_unregister_finalization: None,
            unregister_teardown_observations: Arc::new(
                UnregisterTeardownMechanicalObservations::new(),
            ),
            provisional_interrupt_handle: None,
            dsl_authority,
            drain_slot: CommsDrainSlot::new(),
        };
        Ok((runtime_id, session_entry))
    }

    #[cfg(target_arch = "wasm32")]
    #[inline(never)]
    fn insert_storeless_session_sync(
        &self,
        session_id: SessionId,
        runtime_id: LogicalRuntimeId,
        session_entry: RuntimeSessionEntry,
    ) -> Result<bool, RuntimeDriverError> {
        let mut sessions = self.sessions.try_write().map_err(|_| {
            tracing::warn!(
                %session_id,
                "storeless session map busy while inserting registration"
            );
            RuntimeDriverError::Internal(format!(
                "storeless session map busy while inserting {session_id}"
            ))
        })?;
        tracing::debug!(%session_id, "MeerkatMachine::register_storeless_session_inner_sync locked insert");
        if let Some(existing) = sessions.get_mut(&session_id) {
            if existing.clear_dead_attachment() {
                existing
                    .stage_generated_executor_exit_observation()
                    .map_err(|reason| {
                        RuntimeDriverError::Internal(format!(
                            "generated MeerkatMachine rejected executor-exit observation: {reason}"
                        ))
                    })?;
            }
            Ok(false)
        } else {
            sessions.insert(session_id, session_entry);
            tracing::debug!(
                %runtime_id,
                "MeerkatMachine::register_session_inner inserted storeless session"
            );
            Ok(true)
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn register_storeless_session_inner(
        &self,
        session_id: SessionId,
    ) -> Result<bool, RuntimeDriverError> {
        #[cfg(target_arch = "wasm32")]
        {
            let mut sessions = self.sessions.try_write().map_err(|_| {
                RuntimeDriverError::Internal(format!(
                    "storeless session map busy while registering {session_id}"
                ))
            })?;
            if let Some(existing) = sessions.get_mut(&session_id) {
                tracing::debug!(
                    %session_id,
                    "MeerkatMachine::register_session_inner found existing session"
                );
                if let Some(error) = existing.registration_blocked_by_unregister(&session_id) {
                    return Err(error);
                }
                if existing.clear_dead_attachment() {
                    existing.stage_generated_executor_exit_observation().map_err(|reason| {
                        RuntimeDriverError::Internal(format!(
                            "generated MeerkatMachine rejected executor-exit observation: {reason}"
                        ))
                    })?;
                }
                return Ok(false);
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut sessions = self.sessions.write().await;
            if let Some(existing) = sessions.get_mut(&session_id) {
                tracing::debug!(
                    %session_id,
                    "MeerkatMachine::register_session_inner found existing session"
                );
                if existing.clear_dead_attachment() {
                    existing.stage_generated_executor_exit_observation().map_err(|reason| {
                        RuntimeDriverError::Internal(format!(
                            "generated MeerkatMachine rejected executor-exit observation: {reason}"
                        ))
                    })?;
                }
                return Ok(false);
            }
        }

        let runtime_id = Self::logical_runtime_id(&session_id);
        let recovered_authority = fresh_registered_runtime_authority(
            &session_id,
            "fresh storeless session registration",
        )?;
        let initial_runtime_state =
            super::dsl_authority::runtime_phase_from_authority(&recovered_authority);
        let dsl_authority = Arc::new(std::sync::Mutex::new(recovered_authority));
        let mut entry = self.make_driver(
            runtime_id.clone(),
            Arc::clone(&dsl_authority),
            initial_runtime_state,
        );
        tracing::debug!(
            %session_id,
            %runtime_id,
            "MeerkatMachine::register_session_inner recovering storeless driver"
        );
        if let Err(err) = entry.as_driver_mut().recover().await {
            tracing::error!(%session_id, error = %err, "failed to recover runtime driver during registration");
            return Err(err);
        }
        let control_projection = entry.control_projection_handle();

        let (ops_lifecycle, epoch_id, cursor_state) = Self::fresh_ops_state();
        let handle_teardown_gate = crate::handles::HandleTeardownGate::open();
        let tool_visibility_owner = Arc::new(MachineToolVisibilityOwner::new());
        tool_visibility_owner.bind_dsl_authority(Arc::clone(&dsl_authority));
        let session_entry = RuntimeSessionEntry {
            runtime_id: runtime_id.clone(),
            mutation_gate: Arc::new(Mutex::new(())),
            supervisor_rotation_task: Arc::new(SupervisorRotationTaskSlot::new()),
            control_projection,
            driver: Arc::new(Mutex::new(entry)),
            ops_lifecycle,
            ops_lifecycle_persistence_worker: None,
            epoch_id,
            handle_teardown_gate,
            cursor_state,
            completions: Arc::new(Mutex::new(crate::completion::CompletionRegistry::new())),
            tool_visibility_owner,
            attachment_slot: RuntimeLoopAttachmentSlot::Empty,
            runtime_loop_teardown: None,
            unregister_coordinator: None,
            runtime_stop_cleanup_coordinator: None,
            pending_unregister_finalization: None,
            unregister_teardown_observations: Arc::new(
                UnregisterTeardownMechanicalObservations::new(),
            ),
            provisional_interrupt_handle: None,
            dsl_authority,
            drain_slot: CommsDrainSlot::new(),
        };
        #[cfg(target_arch = "wasm32")]
        {
            let mut sessions = self.sessions.try_write().map_err(|_| {
                RuntimeDriverError::Internal(format!(
                    "storeless session map busy while inserting {session_id}"
                ))
            })?;
            if let Some(existing) = sessions.get_mut(&session_id) {
                if existing.clear_dead_attachment() {
                    existing
                        .stage_generated_executor_exit_observation()
                        .map_err(|reason| {
                            RuntimeDriverError::Internal(format!(
                                "generated MeerkatMachine rejected executor-exit observation: {reason}"
                            ))
                        })?;
                }
                Ok(false)
            } else {
                sessions.insert(session_id, session_entry);
                tracing::debug!(
                    %runtime_id,
                    "MeerkatMachine::register_session_inner inserted storeless session"
                );
                Ok(true)
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut sessions = self.sessions.write().await;
            if let Some(existing) = sessions.get_mut(&session_id) {
                if existing.clear_dead_attachment() {
                    existing
                        .stage_generated_executor_exit_observation()
                        .map_err(|reason| {
                            RuntimeDriverError::Internal(format!(
                                "generated MeerkatMachine rejected executor-exit observation: {reason}"
                            ))
                        })?;
                }
                Ok(false)
            } else {
                sessions.insert(session_id, session_entry);
                tracing::debug!(
                    %runtime_id,
                    "MeerkatMachine::register_session_inner inserted storeless session"
                );
                Ok(true)
            }
        }
    }

    async fn register_session_inner_impl(
        &self,
        session_id: SessionId,
    ) -> Result<bool, RuntimeDriverError> {
        {
            let mut sessions = self.sessions.write().await;
            if let Some(existing) = sessions.get_mut(&session_id) {
                tracing::debug!(
                    %session_id,
                    "MeerkatMachine::register_session_inner found existing session"
                );
                if let Some(error) = existing.registration_blocked_by_unregister(&session_id) {
                    return Err(error);
                }
                if existing.clear_dead_attachment() {
                    existing.stage_generated_executor_exit_observation().map_err(|reason| {
                        RuntimeDriverError::Internal(format!(
                            "generated MeerkatMachine rejected executor-exit observation: {reason}"
                        ))
                    })?;
                }
                return Ok(false);
            }
        }

        let runtime_id = Self::logical_runtime_id(&session_id);
        tracing::debug!(
            %session_id,
            %runtime_id,
            "MeerkatMachine::register_session_inner loading durable lifecycle"
        );
        let recovery_observation = RuntimeLifecycleRecoveryObservation::from_snapshot(
            self.durable_lifecycle_for_registration(&runtime_id).await?,
        );
        let recovered_teardown_observations = Arc::new(
            UnregisterTeardownMechanicalObservations::from_durable_process_recovery(
                recovery_observation.unregister_progress.as_ref(),
            ),
        );
        tracing::debug!(
            %session_id,
            %runtime_id,
            "MeerkatMachine::register_session_inner loaded durable lifecycle"
        );
        let observed_runtime_state = recovery_observation.runtime_state;
        let requires_observed_recovery = recovery_observation.requires_observed_recovery();
        let mut recovered_authority = if requires_observed_recovery {
            super::dsl_authority::recover_authority_from_runtime_observation(
                &session_id,
                observed_runtime_state,
                recovery_observation.agent_runtime_id.as_ref(),
                None,
                None,
                std::collections::BTreeSet::new(),
                recovery_observation.fence_token,
                recovery_observation.runtime_generation,
                recovery_observation.runtime_epoch_id,
            )
            .map_err(|err| {
                RuntimeDriverError::Internal(super::dsl_authority::map_error(
                    err,
                    "session registration DSL recovery",
                ))
            })?
        } else {
            fresh_registered_runtime_authority(&session_id, "fresh session registration")?
        };
        replay_durable_unregister_progress(
            &mut recovered_authority,
            &session_id,
            recovery_observation.unregister_progress.as_ref(),
        )?;
        // Seed the driver's initial phase from the recovered DSL authority
        // uniformly (same as the storeless paths): the authority is the owner;
        // the driver control projection mirrors it, never the raw observation.
        let initial_runtime_state =
            super::dsl_authority::runtime_phase_from_authority(&recovered_authority);
        let dsl_authority = Arc::new(std::sync::Mutex::new(recovered_authority));
        tracing::debug!(
            %session_id,
            %runtime_id,
            ?initial_runtime_state,
            "MeerkatMachine::register_session_inner recovered authority"
        );
        let mut entry = self.make_driver(
            runtime_id.clone(),
            Arc::clone(&dsl_authority),
            initial_runtime_state,
        );
        tracing::debug!(
            %session_id,
            %runtime_id,
            "MeerkatMachine::register_session_inner recovering driver"
        );
        if let Err(err) = entry.as_driver_mut().recover().await {
            tracing::error!(%session_id, error = %err, "failed to recover runtime driver during registration");
            return Err(err);
        }
        tracing::debug!(
            %session_id,
            %runtime_id,
            "MeerkatMachine::register_session_inner recovered driver"
        );
        let control_projection = entry.control_projection_handle();

        tracing::debug!(
            %session_id,
            %runtime_id,
            "MeerkatMachine::register_session_inner recovering ops state"
        );
        let (ops_lifecycle, epoch_id, cursor_state) = if self.store.is_some()
            || (requires_observed_recovery && initial_runtime_state != RuntimeState::Idle)
        {
            self.recover_or_create_ops_state(&session_id, &runtime_id)
                .await?
        } else {
            Self::fresh_ops_state()
        };
        tracing::debug!(
            %session_id,
            %runtime_id,
            %epoch_id,
            "MeerkatMachine::register_session_inner recovered ops state"
        );

        let tool_visibility_owner = Arc::new(MachineToolVisibilityOwner::new());
        // Bind the DSL authority into the visibility owner so its staging
        // trait calls route through the canonical DSL counter
        // `next_staged_visibility_revision` (dogma round 4, wave 2b #12).
        tool_visibility_owner.bind_dsl_authority(Arc::clone(&dsl_authority));
        let handle_teardown_gate = crate::handles::HandleTeardownGate::open();
        let session_entry = RuntimeSessionEntry {
            runtime_id: runtime_id.clone(),
            mutation_gate: Arc::new(Mutex::new(())),
            supervisor_rotation_task: Arc::new(SupervisorRotationTaskSlot::new()),
            control_projection,
            driver: Arc::new(Mutex::new(entry)),
            ops_lifecycle,
            ops_lifecycle_persistence_worker: None,
            epoch_id,
            handle_teardown_gate,
            cursor_state,
            completions: Arc::new(Mutex::new(crate::completion::CompletionRegistry::new())),
            tool_visibility_owner,
            attachment_slot: RuntimeLoopAttachmentSlot::Empty,
            runtime_loop_teardown: None,
            unregister_coordinator: None,
            runtime_stop_cleanup_coordinator: None,
            pending_unregister_finalization: None,
            unregister_teardown_observations: recovered_teardown_observations,
            provisional_interrupt_handle: None,
            dsl_authority,
            drain_slot: CommsDrainSlot::new(),
        };
        tracing::debug!(
            %session_id,
            %runtime_id,
            "MeerkatMachine::register_session_inner inserting session"
        );
        let mut sessions = self.sessions.write().await;
        if let Some(existing) = sessions.get_mut(&session_id) {
            tracing::debug!(
                %session_id,
                %runtime_id,
                "MeerkatMachine::register_session_inner found existing session before insert"
            );
            if let Some(error) = existing.registration_blocked_by_unregister(&session_id) {
                return Err(error);
            }
            if existing.clear_dead_attachment() {
                existing
                    .stage_generated_executor_exit_observation()
                    .map_err(|reason| {
                        RuntimeDriverError::Internal(format!(
                            "generated MeerkatMachine rejected executor-exit observation: {reason}"
                        ))
                    })?;
            }
            Ok(false)
        } else {
            sessions.insert(session_id, session_entry);
            tracing::debug!(
                %runtime_id,
                "MeerkatMachine::register_session_inner inserted session"
            );
            Ok(true)
        }
    }

    pub(super) async fn unregister_session_inner_if_epoch(
        &self,
        session_id: &SessionId,
        epoch_id: &meerkat_core::RuntimeEpochId,
    ) -> Result<(), RuntimeDriverError> {
        self.join_or_start_unregister_teardown(
            session_id,
            Some(epoch_id),
            UnregisterTeardownCaller::Explicit,
        )
        .await
    }

    pub(super) async fn compensate_inserted_session_error(
        &self,
        session_id: &SessionId,
        epoch_id: &meerkat_core::RuntimeEpochId,
        primary_error: RuntimeDriverError,
        context: &'static str,
    ) -> RuntimeDriverError {
        match self
            .unregister_session_inner_if_epoch(session_id, epoch_id)
            .await
        {
            Ok(()) => primary_error,
            Err(cleanup_error) => RuntimeDriverError::Internal(format!(
                "{primary_error}; additionally failed to unregister newly inserted session during {context}: {cleanup_error}"
            )),
        }
    }

    /// Set the silent comms intents for a session's runtime driver.
    ///
    /// Peer requests whose intent matches one of these strings will be accepted
    /// without triggering an LLM turn (ApplyMode::Ignore, WakeMode::None).
    pub async fn set_session_silent_intents(
        &self,
        session_id: &SessionId,
        intents: Vec<String>,
    ) -> Result<(), RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::SetSilentIntents {
                    session_id: session_id.clone(),
                    intents,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::Unit => Ok(()),
            other => Err(RuntimeDriverError::Internal(format!(
                "set_session_silent_intents: unexpected command result variant: {other:?}"
            ))),
        }
    }

    pub async fn commit_service_turn_terminal_receipt(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::CommitServiceTurnTerminalReceipt {
                    session_id: session_id.clone(),
                },
            )
            .await
            .map_err(|err| match err {
                MeerkatMachineCommandError::Driver(err) => err,
                MeerkatMachineCommandError::Control(err) => {
                    RuntimeDriverError::Internal(err.to_string())
                }
            })? {
            MeerkatMachineCommandResult::Unit => Ok(()),
            _ => Err(RuntimeDriverError::Internal(
                "commit_service_turn_terminal_receipt: unexpected command result variant".into(),
            )),
        }
    }

    /// Register a runtime driver for a session WITH a RuntimeLoop backed by a
    /// `CoreExecutor`. Takes `self: &Arc<Self>` because executor attachment is
    /// routed through the Arc-backed command path that owns runtime-loop spawn.
    pub async fn register_session_with_executor(
        self: &Arc<Self>,
        session_id: SessionId,
        executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    ) -> Result<(), RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                Some(Arc::clone(self)),
                MeerkatMachineCommand::EnsureSessionWithExecutor {
                    session_id,
                    executor,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::Unit => Ok(()),
            other => Err(RuntimeDriverError::Internal(format!(
                "register_session_with_executor: unexpected command result variant: {other:?}"
            ))),
        }
    }

    /// Ensure a runtime driver with executor exists for the session.
    ///
    /// If a session was already registered without a loop, upgrade the
    /// existing driver in place so queued inputs remain attached to the same
    /// runtime ledger and can start draining immediately. See
    /// `register_session_with_executor` for why this takes `self: &Arc<Self>`.
    pub async fn ensure_session_with_executor(
        self: &Arc<Self>,
        session_id: SessionId,
        executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    ) -> Result<(), RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                Some(Arc::clone(self)),
                MeerkatMachineCommand::EnsureSessionWithExecutor {
                    session_id,
                    executor,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::Unit => Ok(()),
            other => Err(RuntimeDriverError::Internal(format!(
                "ensure_session_with_executor: unexpected command result variant: {other:?}"
            ))),
        }
    }

    /// Install a temporary live interrupt handle for a prepared session before
    /// its runtime loop executor is attached.
    ///
    /// Runtime-backed surfaces use this during eager session materialization:
    /// the session service owns the first turn until `create_session` returns,
    /// but explicit user interrupts must still route through
    /// `MeerkatMachine::hard_cancel_current_run`.
    pub async fn install_prepared_session_interrupt_handle(
        &self,
        session_id: &SessionId,
        handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>,
    ) -> Result<(), RuntimeDriverError> {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        if entry.clear_dead_attachment() {
            entry
                .stage_generated_executor_exit_observation()
                .map_err(|reason| {
                    RuntimeDriverError::Internal(format!(
                        "generated MeerkatMachine rejected executor-exit observation: {reason}"
                    ))
                })?;
        }
        entry.install_provisional_interrupt_handle(handle);
        Ok(())
    }

    pub(super) async fn ensure_session_with_executor_inner(
        self: &Arc<Self>,
        session_id: SessionId,
        executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    ) -> Result<(), RuntimeDriverError> {
        enum ExistingExecutorClaim {
            AlreadyClaimed,
            Blocked(RuntimeDriverError),
            Rejected(String),
            Claimed {
                gate: Arc<Mutex<()>>,
                driver: SharedDriver,
                completions: SharedCompletionRegistry,
                ops_lifecycle: Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>,
                dsl_authority: Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
                staged: Box<StagedSessionDslInput>,
                repaired_dead_attachment: bool,
                _gate_guard: crate::tokio::sync::OwnedMutexGuard<()>,
            },
        }

        let existing = loop {
            if let Some(gate) = self.session_mutation_gate(&session_id).await {
                let gate_guard = Arc::clone(&gate).lock_owned().await;
                let mut sessions = self.sessions.write().await;
                let Some(entry) = sessions.get_mut(&session_id) else {
                    continue;
                };
                if !Arc::ptr_eq(&entry.mutation_gate, &gate) {
                    continue;
                }
                if let Some(error) = entry.registration_blocked_by_unregister(&session_id) {
                    break ExistingExecutorClaim::Blocked(error);
                }
                let repaired_dead_attachment = entry.clear_dead_attachment();
                let repaired_deferred_stop =
                    repaired_dead_attachment && entry.generated_stop_deferred();
                if repaired_dead_attachment
                    && !repaired_deferred_stop
                    && let Err(reason) = entry.stage_generated_executor_exit_observation()
                {
                    break ExistingExecutorClaim::Rejected(reason);
                }
                if entry.generated_executor_registration_active() && !repaired_deferred_stop {
                    break ExistingExecutorClaim::AlreadyClaimed;
                }
                if entry.has_live_attachment() {
                    match entry.stage_generated_executor_registration_claim(&session_id) {
                        Ok(_) => break ExistingExecutorClaim::AlreadyClaimed,
                        Err(reason) => break ExistingExecutorClaim::Rejected(reason),
                    }
                }
                match entry.stage_generated_executor_registration_claim(&session_id) {
                    Ok(staged) => {
                        break ExistingExecutorClaim::Claimed {
                            gate,
                            driver: entry.driver.clone(),
                            completions: entry.completions.clone(),
                            ops_lifecycle: entry.ops_lifecycle.clone(),
                            dsl_authority: Arc::clone(&entry.dsl_authority),
                            staged: Box::new(staged),
                            repaired_dead_attachment,
                            _gate_guard: gate_guard,
                        };
                    }
                    Err(reason) => break ExistingExecutorClaim::Rejected(reason),
                }
            }

            let runtime_id = Self::logical_runtime_id(&session_id);
            let recovery_observation =
                match self.durable_lifecycle_for_registration(&runtime_id).await {
                    Ok(snapshot) => RuntimeLifecycleRecoveryObservation::from_snapshot(snapshot),
                    Err(err) => {
                        tracing::error!(
                            %session_id,
                            error = %err,
                            "failed to load durable runtime state during executor registration"
                        );
                        return Err(err);
                    }
                };
            let recovered_teardown_observations = Arc::new(
                UnregisterTeardownMechanicalObservations::from_durable_process_recovery(
                    recovery_observation.unregister_progress.as_ref(),
                ),
            );
            let observed_runtime_state = recovery_observation.runtime_state;
            let requires_observed_recovery = recovery_observation.requires_observed_recovery();
            let mut recovered_authority = if requires_observed_recovery {
                match super::dsl_authority::recover_authority_from_runtime_observation(
                    &session_id,
                    observed_runtime_state,
                    recovery_observation.agent_runtime_id.as_ref(),
                    None,
                    None,
                    std::collections::BTreeSet::new(),
                    recovery_observation.fence_token,
                    recovery_observation.runtime_generation,
                    recovery_observation.runtime_epoch_id,
                ) {
                    Ok(authority) => authority,
                    Err(err) => {
                        let mapped =
                            super::dsl_authority::map_error(err, "session recovery DSL recovery");
                        tracing::error!(
                            %session_id,
                            error = %mapped,
                            "failed to recover generated runtime authority during executor registration"
                        );
                        return Err(RuntimeDriverError::Internal(mapped));
                    }
                }
            } else {
                fresh_registered_runtime_authority(&session_id, "fresh executor registration")?
            };
            replay_durable_unregister_progress(
                &mut recovered_authority,
                &session_id,
                recovery_observation.unregister_progress.as_ref(),
            )?;
            // Seed the driver's initial phase from the recovered DSL authority
            // uniformly: the authority is the owner; the driver control
            // projection mirrors it, never the raw observation.
            let initial_runtime_state =
                super::dsl_authority::runtime_phase_from_authority(&recovered_authority);
            let dsl_authority = Arc::new(std::sync::Mutex::new(recovered_authority));
            let mut recovered_entry = self.make_driver(
                runtime_id.clone(),
                Arc::clone(&dsl_authority),
                initial_runtime_state,
            );
            if let Err(err) = recovered_entry.as_driver_mut().recover().await {
                tracing::error!(
                    %session_id,
                    error = %err,
                    "failed to recover runtime driver during registration"
                );
                return Err(err);
            }
            // Recover ops state OUTSIDE the sessions lock to avoid blocking
            // other adapter operations behind potentially slow disk I/O.
            let (recovered_ops, recovered_epoch, recovered_cursors) = if self.store.is_some()
                || (requires_observed_recovery && initial_runtime_state != RuntimeState::Idle)
            {
                match self
                    .recover_or_create_ops_state(&session_id, &runtime_id)
                    .await
                {
                    Ok(recovered) => recovered,
                    Err(err) => {
                        tracing::error!(
                            %session_id,
                            error = %err,
                            "failed to recover ops lifecycle during executor registration"
                        );
                        return Err(err);
                    }
                }
            } else {
                Self::fresh_ops_state()
            };

            let mutation_gate = Arc::new(Mutex::new(()));
            let gate_guard = Arc::clone(&mutation_gate).lock_owned().await;
            let mut sessions = self.sessions.write().await;
            if sessions.contains_key(&session_id) {
                continue;
            }

            let control_projection = recovered_entry.control_projection_handle();
            let driver = Arc::new(Mutex::new(recovered_entry));
            let completions = Arc::new(Mutex::new(crate::completion::CompletionRegistry::new()));
            let tool_visibility_owner = Arc::new(MachineToolVisibilityOwner::new());
            // Bind the DSL authority before the entry is inserted — any
            // subsequent staging trait call must see the bound authority.
            tool_visibility_owner.bind_dsl_authority(Arc::clone(&dsl_authority));
            sessions.insert(
                session_id.clone(),
                RuntimeSessionEntry {
                    runtime_id,
                    mutation_gate: Arc::clone(&mutation_gate),
                    supervisor_rotation_task: Arc::new(SupervisorRotationTaskSlot::new()),
                    control_projection,
                    driver: driver.clone(),
                    ops_lifecycle: recovered_ops.clone(),
                    ops_lifecycle_persistence_worker: None,
                    epoch_id: recovered_epoch,
                    handle_teardown_gate: crate::handles::HandleTeardownGate::open(),
                    cursor_state: recovered_cursors,
                    completions: completions.clone(),
                    tool_visibility_owner,
                    attachment_slot: RuntimeLoopAttachmentSlot::Empty,
                    runtime_loop_teardown: None,
                    unregister_coordinator: None,
                    runtime_stop_cleanup_coordinator: None,
                    pending_unregister_finalization: None,
                    unregister_teardown_observations: recovered_teardown_observations,
                    provisional_interrupt_handle: None,
                    dsl_authority: Arc::clone(&dsl_authority),
                    drain_slot: CommsDrainSlot::new(),
                },
            );
            let Some(entry) = sessions.get_mut(&session_id) else {
                return Err(RuntimeDriverError::Internal(format!(
                    "session {session_id} missing after executor recovery insert"
                )));
            };
            match entry.stage_generated_executor_registration_claim(&session_id) {
                Ok(staged) => {
                    break ExistingExecutorClaim::Claimed {
                        gate: mutation_gate,
                        driver,
                        completions,
                        ops_lifecycle: recovered_ops,
                        dsl_authority,
                        staged: Box::new(staged),
                        repaired_dead_attachment: false,
                        _gate_guard: gate_guard,
                    };
                }
                Err(reason) => {
                    sessions.remove(&session_id);
                    break ExistingExecutorClaim::Rejected(reason);
                }
            }
        };

        let (
            driver,
            completions,
            ops_lifecycle,
            dsl_authority,
            staged_registration,
            repaired_dead_attachment,
            registration_gate,
            _gate_guard,
        ) = match existing {
            ExistingExecutorClaim::AlreadyClaimed => {
                return Ok(());
            }
            ExistingExecutorClaim::Blocked(error) => return Err(error),
            ExistingExecutorClaim::Rejected(reason) => {
                tracing::warn!(
                    %session_id,
                    error = %reason,
                    "generated MeerkatMachine rejected executor registration"
                );
                // Stage-first classification: a claim rejected on a Destroyed
                // binding surfaces as the terminal `Destroyed` truth.
                return Err(self
                    .classify_session_dsl_rejection(&session_id, reason)
                    .await);
            }
            ExistingExecutorClaim::Claimed {
                gate,
                driver,
                completions,
                ops_lifecycle,
                dsl_authority,
                staged,
                repaired_dead_attachment,
                _gate_guard,
            } => (
                driver,
                completions,
                ops_lifecycle,
                dsl_authority,
                staged,
                repaired_dead_attachment,
                gate,
                _gate_guard,
            ),
        };

        let should_wake = {
            let mut driver_guard = driver.lock().await;
            driver_guard.sync_control_projection_from_dsl_authority();
            if repaired_dead_attachment {
                tracing::warn!(
                    %session_id,
                    "runtime driver registration was repaired by generated executor authority; publishing attachment"
                );
            }
            if staged_registration.revived_stopped_session() {
                // Machine-emitted revival (`EnsureSessionWithExecutorStopped`
                // re-admitted a stopped session to Attached): refresh the
                // durable lifecycle record so cross-process readers never
                // observe a stale `Stopped` snapshot for a revived session.
                driver_guard
                    .persist_current_machine_lifecycle("resume")
                    .await?;
            }
            !driver_guard.as_driver().active_input_ids().is_empty()
        };

        // Wire persistence channel if a durable store is available.
        if let Some(ref store) = self.store {
            let (persist_tx, persist_rx) = crate::tokio::sync::mpsc::unbounded_channel::<
                crate::ops_lifecycle::OpsLifecyclePersistenceRequest,
            >();
            let (entry_epoch_id, entry_cursor, runtime_id) = {
                let sessions = self.sessions.read().await;
                let entry = sessions.get(&session_id).ok_or_else(|| {
                    RuntimeDriverError::Internal(format!(
                        "session {session_id} disappeared before ops persistence wiring"
                    ))
                })?;
                (
                    entry.epoch_id.clone(),
                    Arc::clone(&entry.cursor_state),
                    entry.runtime_id.clone(),
                )
            };
            let persistence_worker =
                spawn_ops_lifecycle_persistence_worker(Arc::clone(store), runtime_id, persist_rx)?;
            let previous_worker = {
                let mut sessions = self.sessions.write().await;
                let entry = sessions.get_mut(&session_id).ok_or_else(|| {
                    RuntimeDriverError::Internal(format!(
                        "session {session_id} disappeared while installing ops persistence worker"
                    ))
                })?;
                entry
                    .ops_lifecycle_persistence_worker
                    .replace(persistence_worker)
            };
            ops_lifecycle.set_persistence_channel(persist_tx, entry_epoch_id, entry_cursor);
            if let Some(previous_worker) = previous_worker {
                join_ops_lifecycle_persistence_worker(previous_worker).await?;
            }
        }

        // Get the completion feed from the registry for feed-based idle wake.
        let completion_feed = ops_lifecycle.completion_feed_handle();

        let boundary_handle = executor.boundary_handle();
        let interrupt_handle = executor.interrupt_handle();
        let (wake_tx, wake_rx) = mpsc::channel(16);
        let (effect_tx, effect_rx) = mpsc::channel(16);
        let entry_cursor_state = {
            let sessions = self.sessions.read().await;
            sessions
                .get(&session_id)
                .map(|e| Arc::clone(&e.cursor_state))
        };
        let mut pending_loop = Some(crate::runtime_loop::spawn_runtime_loop_with_completions(
            driver.clone(),
            executor,
            wake_rx,
            effect_rx,
            Some(completions.clone()),
            Some(completion_feed),
            Some(Arc::clone(&ops_lifecycle) as Arc<dyn meerkat_core::OpsLifecycleRegistry>),
            entry_cursor_state,
            Arc::downgrade(self),
            session_id.clone(),
        ));
        let startup = pending_loop
            .as_ref()
            .map(crate::runtime_loop::SpawnedRuntimeLoop::startup_slot)
            .ok_or_else(|| {
                RuntimeDriverError::Internal("runtime loop startup handle missing".into())
            })?;

        let (published, detach_after_abort) = {
            let mut sessions = self.sessions.write().await;
            match sessions.get_mut(&session_id) {
                None => (false, true),
                Some(entry) => {
                    entry.clear_dead_attachment();
                    if entry.has_live_attachment() {
                        (false, false)
                    } else if !Arc::ptr_eq(&entry.mutation_gate, &registration_gate)
                        || !Arc::ptr_eq(&entry.dsl_authority, &dsl_authority)
                        || !Arc::ptr_eq(&entry.driver, &driver)
                        || !Arc::ptr_eq(&entry.completions, &completions)
                    {
                        tracing::warn!(
                            %session_id,
                            "runtime session entry changed while wiring executor; aborting stale loop attachment"
                        );
                        (false, true)
                    } else {
                        match pending_loop.take() {
                            Some(spawned_loop) => {
                                entry.attach_runtime_loop(
                                    wake_tx.clone(),
                                    effect_tx,
                                    boundary_handle,
                                    interrupt_handle,
                                    spawned_loop,
                                );
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
            if let Some(spawned_loop) = pending_loop.take() {
                spawned_loop.loop_handle.abort();
            }
            if detach_after_abort {
                Self::restore_dsl_authority_snapshot(
                    &dsl_authority,
                    staged_registration.previous_snapshot,
                );
                let mut driver_guard = driver.lock().await;
                driver_guard.sync_control_projection_from_dsl_authority();
                return Err(RuntimeDriverError::Internal(
                    "runtime session entry changed while wiring executor".into(),
                ));
            }
            return Ok(());
        }

        // Publishing the attachment is not readiness. The exact executor must
        // first reconcile the durable compaction projection outbox, including
        // the authoritative empty observation used to abort pre-commit stages.
        // Keep the registration gate until that owner-owned startup boundary
        // finishes, so no competing lifecycle mutation can mistake attached
        // channels for a ready executor.
        if let Err(startup_error) = startup.wait().await {
            drop(_gate_guard);
            return match self.unregister_session(&session_id).await {
                Ok(()) => Err(startup_error),
                Err(cleanup_error) => Err(RuntimeDriverError::Internal(format!(
                    "{startup_error}; additionally failed to unregister the failed runtime-loop startup: {cleanup_error}"
                ))),
            };
        }

        if should_wake {
            let _ = wake_tx.try_send(());
        }
        Ok(())
    }

    /// Unregister a session's runtime driver through the owned teardown saga.
    ///
    /// Durably opens `BeginUnregisterSession`, joins or performs the exact
    /// ordinary-stop cleanup, closes generated teardown obligations, commits
    /// `UnregisterSession`, and only then removes the registered entry.
    pub async fn unregister_session(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        self.join_or_start_unregister_teardown(session_id, None, UnregisterTeardownCaller::Explicit)
            .await
    }

    /// Start or join the one owned ordinary-stop operation for this epoch.
    /// The coordinator owns both effect delivery and exact-executor cleanup;
    /// callers only join its typed result.
    pub(super) async fn request_runtime_stop(
        &self,
        session_id: &SessionId,
        reason: String,
    ) -> Result<(), RuntimeDriverError> {
        let generated_draining = self.session_dsl_state(session_id).await.is_ok_and(|state| {
            state.registration_phase == crate::meerkat_machine::dsl::RegistrationPhase::Draining
        });
        if generated_draining {
            return match self.unregister_session_inner(session_id).await {
                Err(RuntimeDriverError::UnregisterInProgress { .. }) => {
                    Err(RuntimeDriverError::RuntimeStopInProgress {
                        runtime_id: LogicalRuntimeId::for_session(session_id),
                    })
                }
                result => result,
            };
        }
        self.join_or_start_runtime_stop_cleanup(
            session_id,
            RuntimeStopCleanupCaller::ExplicitStop,
            Some(reason),
            None,
        )
        .await
    }

    /// Observe a runtime-loop handoff without turning a completed failed
    /// cleanup into an implicit retry. Generated Draining is the primary
    /// unregister authority; the typed handoff disposition is a fail-closed
    /// fallback for a durability failure before the loop exits.
    pub(crate) async fn observe_runtime_loop_teardown(
        &self,
        session_id: &SessionId,
        observed_teardown_slot: Arc<crate::runtime_loop::RuntimeLoopTeardownSlot>,
        disposition: crate::runtime_loop::RuntimeLoopTeardownDisposition,
    ) -> Result<(), RuntimeDriverError> {
        let generated_draining = {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Ok(());
            };
            let current_slot_matches = entry
                .runtime_loop_teardown
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &observed_teardown_slot));
            if !current_slot_matches {
                return Ok(());
            }
            entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .state()
                .registration_phase
                == crate::meerkat_machine::dsl::RegistrationPhase::Draining
        };
        if generated_draining || disposition.requires_unregister() {
            return self
                .join_or_start_unregister_teardown(
                    session_id,
                    None,
                    UnregisterTeardownCaller::RuntimeLoopWatcher,
                )
                .await;
        }
        self.join_or_start_runtime_stop_cleanup(
            session_id,
            RuntimeStopCleanupCaller::RuntimeLoopWatcher,
            None,
            Some(observed_teardown_slot),
        )
        .await
    }

    async fn join_or_start_runtime_stop_cleanup(
        &self,
        session_id: &SessionId,
        caller: RuntimeStopCleanupCaller,
        initial_reason: Option<String>,
        expected_teardown_slot: Option<Arc<crate::runtime_loop::RuntimeLoopTeardownSlot>>,
    ) -> Result<(), RuntimeDriverError> {
        enum CoordinatorDecision {
            Join(crate::tokio::sync::watch::Receiver<Option<RuntimeStopCleanupResult>>),
            Start {
                epoch_id: meerkat_core::RuntimeEpochId,
                coordinator_id: uuid::Uuid,
                result_tx: crate::tokio::sync::watch::Sender<Option<RuntimeStopCleanupResult>>,
                result_rx: crate::tokio::sync::watch::Receiver<Option<RuntimeStopCleanupResult>>,
                teardown_slot: Option<Arc<crate::runtime_loop::RuntimeLoopTeardownSlot>>,
                completion_authority: Arc<
                    crate::tokio::sync::Mutex<
                        Option<crate::meerkat_machine::driver::RuntimeCompletionResultAuthority>,
                    >,
                >,
                work: RuntimeStopCleanupWork,
            },
            Completed(RuntimeStopCleanupResult),
        }

        // Coordinator installation shares the session mutation gate with
        // queue admission and stopped-session revival. This is the stop
        // linearization point: once the coordinator is visible, no ordinary
        // queued batch or revival can claim the old epoch first.
        let coordinator_gate = match self.session_mutation_gate(session_id).await {
            Some(gate) => Some(gate.lock_owned().await),
            None if caller == RuntimeStopCleanupCaller::RuntimeLoopWatcher => return Ok(()),
            None => {
                return Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                });
            }
        };
        let decision = {
            let mut sessions = self.sessions.write().await;
            let Some(entry) = sessions.get_mut(session_id) else {
                return if caller == RuntimeStopCleanupCaller::RuntimeLoopWatcher {
                    Ok(())
                } else {
                    Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    })
                };
            };
            if let Some(expected) = expected_teardown_slot.as_ref() {
                let current_matches = entry
                    .runtime_loop_teardown
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, expected));
                if !current_matches {
                    return Ok(());
                }
            }
            match entry.runtime_stop_cleanup_coordinator.as_ref() {
                Some(coordinator) if coordinator.epoch_id != entry.epoch_id => {
                    return Err(RuntimeDriverError::Internal(format!(
                        "stale runtime-stop cleanup coordinator epoch for session {session_id}"
                    )));
                }
                Some(coordinator) => {
                    let coordinator_slot_is_current = match (
                        coordinator.teardown_slot.as_ref(),
                        entry.runtime_loop_teardown.as_ref(),
                    ) {
                        (Some(coordinator_slot), Some(current_slot)) => {
                            Arc::ptr_eq(coordinator_slot, current_slot)
                        }
                        (None, None) => true,
                        _ => false,
                    };
                    if !coordinator_slot_is_current {
                        return Err(RuntimeDriverError::Internal(format!(
                            "stale runtime-stop cleanup coordinator handoff for session {session_id}"
                        )));
                    }
                    if active_runtime_stop_coordinator()
                        .is_some_and(|active| active == coordinator.coordinator_id)
                    {
                        return Err(RuntimeDriverError::Internal(format!(
                            "runtime-stop cleanup for session {session_id} attempted to join its own coordinator task"
                        )));
                    }
                    let completed = coordinator.result_rx.borrow().clone();
                    match completed {
                        None => CoordinatorDecision::Join(coordinator.result_rx.clone()),
                        Some(result)
                            if result.is_err()
                                && caller != RuntimeStopCleanupCaller::RuntimeLoopWatcher =>
                        {
                            let epoch_id = entry.epoch_id.clone();
                            let coordinator_id = uuid::Uuid::new_v4();
                            let (result_tx, result_rx) = crate::tokio::sync::watch::channel(None);
                            let completion_authority =
                                Arc::clone(&coordinator.completion_authority);
                            entry.runtime_stop_cleanup_coordinator =
                                Some(RuntimeStopCleanupCoordinator {
                                    epoch_id: epoch_id.clone(),
                                    coordinator_id,
                                    teardown_slot: entry.runtime_loop_teardown.clone(),
                                    completion_authority: Arc::clone(&completion_authority),
                                    result_rx: result_rx.clone(),
                                });
                            CoordinatorDecision::Start {
                                epoch_id,
                                coordinator_id,
                                result_tx,
                                result_rx,
                                teardown_slot: entry.runtime_loop_teardown.clone(),
                                completion_authority,
                                work: RuntimeStopCleanupWork::CleanupOnly,
                            }
                        }
                        Some(result) => CoordinatorDecision::Completed(result),
                    }
                }
                None => {
                    let epoch_id = entry.epoch_id.clone();
                    let coordinator_id = uuid::Uuid::new_v4();
                    let (result_tx, result_rx) = crate::tokio::sync::watch::channel(None);
                    let completion_authority = Arc::new(crate::tokio::sync::Mutex::new(None));
                    entry.runtime_stop_cleanup_coordinator = Some(RuntimeStopCleanupCoordinator {
                        epoch_id: epoch_id.clone(),
                        coordinator_id,
                        teardown_slot: entry.runtime_loop_teardown.clone(),
                        completion_authority: Arc::clone(&completion_authority),
                        result_rx: result_rx.clone(),
                    });
                    CoordinatorDecision::Start {
                        epoch_id,
                        coordinator_id,
                        result_tx,
                        result_rx,
                        teardown_slot: entry.runtime_loop_teardown.clone(),
                        completion_authority,
                        work: match initial_reason {
                            Some(reason) => RuntimeStopCleanupWork::Request { reason },
                            None => RuntimeStopCleanupWork::CleanupOnly,
                        },
                    }
                }
            }
        };
        drop(coordinator_gate);

        let mut result_rx = match decision {
            CoordinatorDecision::Join(result_rx) => result_rx,
            CoordinatorDecision::Completed(result) => return result,
            CoordinatorDecision::Start {
                epoch_id,
                coordinator_id,
                result_tx,
                result_rx,
                teardown_slot,
                completion_authority,
                work,
            } => {
                let worker_machine = self.clone();
                let worker_session_id = session_id.clone();
                let worker_epoch_id = epoch_id;
                let worker = crate::tokio::spawn(runtime_stop_coordinator_poll_scope(
                    coordinator_id,
                    async move {
                        worker_machine
                            .run_owned_runtime_stop_cleanup(
                                &worker_session_id,
                                &worker_epoch_id,
                                teardown_slot,
                                completion_authority,
                                work,
                            )
                            .await
                    },
                ));
                crate::tokio::spawn(async move {
                    let result = match worker.await {
                        Ok(result) => result,
                        Err(join_error) => Err(RuntimeDriverError::Internal(format!(
                            "owned runtime-stop cleanup coordinator failed: {join_error}"
                        ))),
                    };
                    let _ = result_tx.send(Some(result));
                });
                result_rx
            }
        };

        let wait_for_owned_result = async {
            loop {
                if let Some(result) = result_rx.borrow().clone() {
                    return result;
                }
                result_rx.changed().await.map_err(|_| {
                    RuntimeDriverError::Internal(format!(
                        "runtime-stop cleanup coordinator result channel closed for session {session_id}"
                    ))
                })?;
            }
        };
        if caller == RuntimeStopCleanupCaller::ExplicitStop {
            return match crate::tokio::time::timeout(
                RUNTIME_STOP_CALLER_WAIT_GRACE,
                wait_for_owned_result,
            )
            .await
            {
                Ok(result) => result,
                Err(_elapsed) => Err(RuntimeDriverError::RuntimeStopInProgress {
                    runtime_id: LogicalRuntimeId::for_session(session_id),
                }),
            };
        }
        wait_for_owned_result.await
    }

    async fn run_owned_runtime_stop_cleanup(
        &self,
        session_id: &SessionId,
        epoch_id: &meerkat_core::RuntimeEpochId,
        teardown_slot: Option<Arc<crate::runtime_loop::RuntimeLoopTeardownSlot>>,
        completion_authority: Arc<
            crate::tokio::sync::Mutex<
                Option<crate::meerkat_machine::driver::RuntimeCompletionResultAuthority>,
            >,
        >,
        work: RuntimeStopCleanupWork,
    ) -> Result<(), RuntimeDriverError> {
        let stop_completion = match work {
            RuntimeStopCleanupWork::Request { reason } => {
                self.dispatch_owned_runtime_stop_request(
                    session_id,
                    epoch_id,
                    reason,
                    &completion_authority,
                )
                .await?
            }
            RuntimeStopCleanupWork::CleanupOnly => None,
        };

        let (driver, completions) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .filter(|entry| &entry.epoch_id == epoch_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            (Arc::clone(&entry.driver), Arc::clone(&entry.completions))
        };

        let cleanup_result = match teardown_slot.as_ref() {
            Some(teardown_slot) => {
                teardown_slot.wait_until_published().await;
                if completion_authority.lock().await.is_none() {
                    // Capture the generated terminal result before cleanup can
                    // advance the driver. A failed cleanup retains this exact
                    // epoch capability for the explicit retry instead of
                    // reclassifying waiters from the already-stopped phase.
                    let authority = crate::meerkat_machine::driver::
                        machine_resolve_runtime_terminated_completion_result(&driver)
                        .await?;
                    *completion_authority.lock().await = Some(authority);
                }
                match teardown_slot.cleanup_once(&driver).await {
                    Ok(()) => {
                        let runtime_terminated_completion_authority = completion_authority
                            .lock()
                            .await
                            .take()
                            .ok_or_else(|| {
                                RuntimeDriverError::Internal(format!(
                                    "runtime-stop cleanup for session {session_id} lost its generated completion authority"
                                ))
                            })?;
                        completions.lock().await.resolve_all_runtime_terminated(
                            "runtime stopped",
                            runtime_terminated_completion_authority,
                        );
                        Ok(())
                    }
                    Err(error) => Err(error),
                }
            }
            None => crate::control_plane::terminalize_async_stop(&driver, Some(&completions)).await,
        };
        if let Some(teardown_slot) = teardown_slot.as_ref() {
            teardown_slot.acknowledge_runtime_stop_result(cleanup_result.clone());
        }

        let Some(stop_completion) = stop_completion else {
            return cleanup_result;
        };
        let acknowledged_result = stop_completion.await.map_err(|_| {
            RuntimeDriverError::Internal(
                "runtime loop exited without acknowledging required stop cleanup".into(),
            )
        })?;
        match (cleanup_result, acknowledged_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) => Err(error),
            (Ok(()), Err(error)) => Err(error),
        }
    }

    async fn dispatch_owned_runtime_stop_request(
        &self,
        session_id: &SessionId,
        epoch_id: &meerkat_core::RuntimeEpochId,
        reason: String,
        completion_authority: &Arc<
            crate::tokio::sync::Mutex<
                Option<crate::meerkat_machine::driver::RuntimeCompletionResultAuthority>,
            >,
        >,
    ) -> Result<
        Option<crate::tokio::sync::oneshot::Receiver<Result<(), RuntimeDriverError>>>,
        RuntimeDriverError,
    > {
        let Some(gate) = self.session_mutation_gate(session_id).await else {
            return Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            });
        };
        let gate_guard = Arc::clone(&gate).lock_owned().await;
        let driver = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .filter(|entry| &entry.epoch_id == epoch_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            if !Arc::ptr_eq(&entry.mutation_gate, &gate) {
                return Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                });
            }
            Arc::clone(&entry.driver)
        };
        let generated_completion_authority = if completion_authority.lock().await.is_none() {
            Some(
                crate::meerkat_machine::driver::
                    machine_resolve_runtime_terminated_completion_result(&driver)
                    .await?,
            )
        } else {
            None
        };
        let staged = match self
            .stage_session_dsl_transition(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::StopRuntimeExecutor { reason },
                "StopRuntimeExecutor",
            )
            .await
        {
            Ok(staged) => staged,
            Err(reason) => {
                return Err(self
                    .classify_session_dsl_rejection(session_id, reason)
                    .await);
            }
        };
        let projected_effect =
            crate::effect::runtime_effect_projection_from_dsl_effects(&staged.effects)
                .map_err(RuntimeDriverError::Internal)?;
        let effect_tx = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .filter(|entry| &entry.epoch_id == epoch_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            if !Arc::ptr_eq(&entry.mutation_gate, &gate) {
                return Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                });
            }
            entry.effect_sender()
        };
        let (stop_completion_tx, stop_completion_rx) = crate::tokio::sync::oneshot::channel();
        let effect = projected_effect
            .into_effect()
            .with_stop_completion(stop_completion_tx)?;
        if let Some(authority) = generated_completion_authority {
            *completion_authority.lock().await = Some(authority);
        }
        drop(gate_guard);
        let Some(effect_tx) = effect_tx else {
            return Ok(None);
        };
        if effect_tx.send(effect).await.is_err() {
            return Ok(None);
        }
        Ok(Some(stop_completion_rx))
    }

    async fn join_or_start_unregister_teardown(
        &self,
        session_id: &SessionId,
        expected_epoch: Option<&meerkat_core::RuntimeEpochId>,
        caller: UnregisterTeardownCaller,
    ) -> Result<(), RuntimeDriverError> {
        enum CoordinatorDecision {
            Join(crate::tokio::sync::watch::Receiver<Option<UnregisterTeardownResult>>),
            Start {
                epoch_id: meerkat_core::RuntimeEpochId,
                coordinator_id: uuid::Uuid,
                result_tx: crate::tokio::sync::watch::Sender<Option<UnregisterTeardownResult>>,
                result_rx: crate::tokio::sync::watch::Receiver<Option<UnregisterTeardownResult>>,
                teardown_slot: Option<Arc<crate::runtime_loop::RuntimeLoopTeardownSlot>>,
            },
            AlreadyAbsent,
            EpochChanged,
            Completed(Result<(), RuntimeDriverError>),
        }

        let decision = {
            let mut sessions = self.sessions.write().await;
            match sessions.get_mut(session_id) {
                None => CoordinatorDecision::AlreadyAbsent,
                Some(entry)
                    if expected_epoch.is_some_and(|expected| expected != &entry.epoch_id) =>
                {
                    CoordinatorDecision::EpochChanged
                }
                Some(entry) => {
                    if let Some(active) = active_runtime_stop_coordinator()
                        && entry
                            .runtime_stop_cleanup_coordinator
                            .as_ref()
                            .is_some_and(|coordinator| coordinator.coordinator_id == active)
                    {
                        return Err(RuntimeDriverError::Internal(format!(
                            "unregister teardown for session {session_id} attempted to join its own coordinator task (runtime-stop cleanup)"
                        )));
                    }
                    if caller == UnregisterTeardownCaller::RuntimeLoopWatcher
                        && let Some(result) = entry
                            .runtime_loop_teardown
                            .as_ref()
                            .and_then(|slot| slot.last_unregister_result())
                    {
                        CoordinatorDecision::Completed(result)
                    } else if let Some(coordinator) = entry.unregister_coordinator.as_ref() {
                        if coordinator.epoch_id == entry.epoch_id {
                            if active_unregister_coordinator()
                                .is_some_and(|active| active == coordinator.coordinator_id)
                            {
                                return Err(RuntimeDriverError::Internal(format!(
                                    "unregister teardown for session {session_id} attempted to join its own coordinator task"
                                )));
                            }
                            CoordinatorDecision::Join(coordinator.result_rx.clone())
                        } else {
                            return Err(RuntimeDriverError::Internal(format!(
                                "stale unregister coordinator epoch for session {session_id}"
                            )));
                        }
                    } else {
                        if caller == UnregisterTeardownCaller::Explicit
                            && let Some(teardown_slot) = entry.runtime_loop_teardown.as_ref()
                        {
                            // A deliberate caller-owned retry supersedes the
                            // prior terminal error. Clear it while holding the
                            // session map lock so the loop watcher either joins
                            // this coordinator or observes its new result.
                            teardown_slot.clear_last_unregister_result();
                        }
                        let epoch_id = entry.epoch_id.clone();
                        let coordinator_id = uuid::Uuid::new_v4();
                        let (result_tx, result_rx) = crate::tokio::sync::watch::channel(None);
                        entry.unregister_coordinator = Some(UnregisterTeardownCoordinator {
                            epoch_id: epoch_id.clone(),
                            coordinator_id,
                            result_rx: result_rx.clone(),
                        });
                        CoordinatorDecision::Start {
                            epoch_id,
                            coordinator_id,
                            result_tx,
                            result_rx,
                            teardown_slot: entry.runtime_loop_teardown.clone(),
                        }
                    }
                }
            }
        };

        let mut result_rx = match decision {
            CoordinatorDecision::Join(result_rx) => result_rx,
            CoordinatorDecision::AlreadyAbsent | CoordinatorDecision::EpochChanged => {
                return Ok(());
            }
            CoordinatorDecision::Completed(result) => return result,
            CoordinatorDecision::Start {
                epoch_id,
                coordinator_id,
                result_tx,
                result_rx,
                teardown_slot,
            } => {
                let saga_machine = self.clone();
                let saga_session_id = session_id.clone();
                let saga_epoch_id = epoch_id.clone();
                let worker = crate::tokio::spawn(unregister_coordinator_poll_scope(
                    coordinator_id,
                    async move {
                        saga_machine
                            .run_owned_unregister_teardown(&saga_session_id, &saga_epoch_id)
                            .await
                    },
                ));
                let supervisor_machine = self.clone();
                let supervisor_session_id = session_id.clone();
                crate::tokio::spawn(async move {
                    let result = match worker.await {
                        Ok(result) => result,
                        Err(join_error) => Err(RuntimeDriverError::Internal(format!(
                            "owned unregister coordinator task failed: {join_error}"
                        ))),
                    };
                    if let Some(teardown_slot) = teardown_slot {
                        teardown_slot.acknowledge_unregister_result(result.clone());
                    }
                    // Existing joiners retain cloned watch receivers, so clear
                    // the matching completed coordinator before publishing.
                    // A caller that wakes on this result can therefore start a
                    // deliberate retry immediately instead of accidentally
                    // rejoining the completed error. The retained loop result
                    // prevents its watcher from starting that retry itself.
                    supervisor_machine
                        .clear_unregister_coordinator(
                            &supervisor_session_id,
                            &epoch_id,
                            coordinator_id,
                        )
                        .await;
                    let _ = result_tx.send(Some(result));
                });
                result_rx
            }
        };

        let wait_for_owned_result = async {
            loop {
                if let Some(result) = result_rx.borrow().clone() {
                    return result;
                }
                result_rx.changed().await.map_err(|_| {
                    RuntimeDriverError::Internal(format!(
                        "unregister coordinator result channel closed for session {session_id}"
                    ))
                })?;
            }
        };
        match crate::tokio::time::timeout(UNREGISTER_CALLER_WAIT_GRACE, wait_for_owned_result).await
        {
            Ok(result) => result,
            Err(_elapsed) => Err(RuntimeDriverError::UnregisterInProgress {
                runtime_id: LogicalRuntimeId::for_session(session_id),
            }),
        }
    }

    async fn clear_unregister_coordinator(
        &self,
        session_id: &SessionId,
        epoch_id: &meerkat_core::RuntimeEpochId,
        coordinator_id: uuid::Uuid,
    ) {
        let mut sessions = self.sessions.write().await;
        let Some(entry) = sessions.get_mut(session_id) else {
            return;
        };
        let should_clear = entry
            .unregister_coordinator
            .as_ref()
            .is_some_and(|coordinator| {
                coordinator.epoch_id == *epoch_id && coordinator.coordinator_id == coordinator_id
            });
        if should_clear {
            entry.unregister_coordinator = None;
        }
    }

    async fn run_owned_unregister_teardown(
        &self,
        session_id: &SessionId,
        epoch_id: &meerkat_core::RuntimeEpochId,
    ) -> Result<(), RuntimeDriverError> {
        let result = self
            .run_owned_unregister_teardown_inner(session_id, epoch_id)
            .await;
        if let Err(error) = &result {
            let completions = {
                let sessions = self.sessions.read().await;
                sessions
                    .get(session_id)
                    .filter(|entry| &entry.epoch_id == epoch_id)
                    .map(|entry| Arc::clone(&entry.completions))
            };
            if let Some(completions) = completions {
                completions.lock().await.fail_all_waiters(
                    crate::completion::CompletionWaitError::AuthorityUnavailable(error.to_string()),
                );
            }
        }
        result
    }

    /// Unregister a session and report whether the machine-owned detach and
    /// durable Idle projection committed. Callers performing compensation
    /// must use this checked form; swallowing a persistence failure would
    /// falsely claim that a resumed durable session was preserved.
    pub async fn try_unregister_session(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        self.join_or_start_unregister_teardown(session_id, None, UnregisterTeardownCaller::Explicit)
            .await
    }

    /// Stage `BeginUnregisterSession`, which opens the machine-owned drain
    /// window. Carries the same binding facts as the final `UnregisterSession`
    /// so the machine can match them against the active runtime authority.
    async fn stage_begin_unregister_session_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<StagedSessionDslInput, String> {
        let begin_input = {
            let authority = self.session_dsl_authority(session_id).await?;
            let authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = authority.state();
            crate::meerkat_machine::dsl::MeerkatMachineInput::BeginUnregisterSession {
                session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
                agent_runtime_id: state.active_runtime_id.clone(),
                fence_token: state.active_fence_token,
                generation: state.active_runtime_generation,
                runtime_epoch_id: state.active_runtime_epoch_id.clone(),
            }
        };
        self.stage_session_dsl_transition(session_id, begin_input, "BeginUnregisterSession")
            .await
    }

    async fn stage_unregister_session_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<
        (
            StagedSessionDslInput,
            RuntimeOpsLifecycleDurabilityAuthority,
        ),
        RuntimeDriverError,
    > {
        let (durability_input, unregister_input) = {
            let authority = self.session_dsl_authority(session_id).await.map_err(|reason| {
                RuntimeDriverError::ValidationFailed {
                    reason: format!(
                        "generated unregister authority unavailable for session {session_id}: {reason}"
                    ),
                }
            })?;
            let authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = authority.state();
            let dsl_session_id = crate::meerkat_machine::dsl::SessionId::from_domain(session_id);
            let agent_runtime_id = state.active_runtime_id.clone();
            let fence_token = state.active_fence_token;
            let generation = state.active_runtime_generation;
            let runtime_epoch_id = state.active_runtime_epoch_id.clone();
            (
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveRuntimeOpsLifecycleDurability {
                    session_id: dsl_session_id.clone(),
                    agent_runtime_id: agent_runtime_id.clone(),
                    fence_token,
                    generation,
                    runtime_epoch_id: runtime_epoch_id.clone(),
                },
                crate::meerkat_machine::dsl::MeerkatMachineInput::UnregisterSession {
                    session_id: dsl_session_id,
                    agent_runtime_id,
                    fence_token,
                    generation,
                    runtime_epoch_id,
                },
            )
        };
        let authority = if self.store.is_some() {
            let durability_effects = self
                .preview_session_dsl_input(
                    session_id,
                    durability_input,
                    "ResolveRuntimeOpsLifecycleDurability",
                )
                .await
                .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
            runtime_ops_lifecycle_durability_authority_from_effects(
                session_id,
                &durability_effects,
            )?
        } else {
            RuntimeOpsLifecycleDurabilityAuthority {
                action:
                    crate::meerkat_machine::dsl::RuntimeOpsLifecycleDurabilityAction::RetainSnapshot,
            }
        };
        let staged = if self.store.is_some() {
            let mut sessions = self.sessions.write().await;
            let entry = sessions
                .get_mut(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            // Once the final durable transition is staged, every detached
            // session-owned handle must fail closed. An ordinary finalization
            // failure rolls back only to the completed-drain retry anchor; it
            // never reopens the retired runtime epoch for regular work.
            entry.close_handle_teardown_gate();
            let staged = Self::stage_dsl_transition_on_authority(
                &entry.dsl_authority,
                unregister_input,
                "UnregisterSession",
            )
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
            entry.pending_unregister_finalization = Some(PendingUnregisterFinalization {
                durability_authority: authority.clone(),
                committed_snapshot: staged.committed_snapshot.clone(),
            });
            staged
        } else {
            self.stage_session_dsl_transition(session_id, unregister_input, "UnregisterSession")
                .await
                .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?
        };
        Ok((staged, authority))
    }

    async fn finalize_unregistered_session(
        &self,
        driver: SharedDriver,
        durability_authority: RuntimeOpsLifecycleDurabilityAuthority,
        retired_ops_epoch: &meerkat_core::RuntimeEpochId,
    ) -> Result<(), RuntimeDriverError> {
        let mut driver = driver.lock().await;
        driver.sync_control_projection_from_dsl_authority();

        if durability_authority.action
            == crate::meerkat_machine::dsl::RuntimeOpsLifecycleDurabilityAction::DeleteSnapshot
        {
            return driver
                .commit_unregister_finalization("unregister", retired_ops_epoch)
                .await;
        }
        // Retaining the ops snapshot does not authorize retaining an
        // unfinished unregister prefix. Persist the generated final image so
        // Retired remains terminal with its bindings and drain obligations
        // cleared instead of leaving recovery anchored in Draining.
        driver.persist_current_machine_lifecycle("unregister").await
    }

    /// Publish the generated terminal lifecycle and retire the matching ops
    /// snapshot through the store's single atomic unregister transaction. The
    /// live entry stays installed until that transaction succeeds; on failure
    /// the generated completed-drain snapshot remains the retry anchor.
    async fn finalize_unregister_durability_transaction(
        &self,
        session_id: &SessionId,
        driver: SharedDriver,
        durability_authority: RuntimeOpsLifecycleDurabilityAuthority,
        retired_ops_epoch: &meerkat_core::RuntimeEpochId,
        unregister_rollback_snapshot: crate::meerkat_machine::dsl::MeerkatMachineAuthoritySnapshot,
    ) -> Result<(), RuntimeDriverError> {
        let transaction_result = self
            .finalize_unregistered_session(
                Arc::clone(&driver),
                durability_authority,
                retired_ops_epoch,
            )
            .await;

        let Err(primary_error) = transaction_result else {
            return Ok(());
        };

        if matches!(
            &primary_error,
            RuntimeDriverError::UnregisterFinalizationOutcomeUnknown { .. }
        ) {
            // The atomic store transaction may already be durable. Retain the
            // completed-drain DSL state as an in-memory retry anchor, but do
            // not overwrite a possibly terminal lifecycle record with an
            // independent compensating write. The idempotent retry converges
            // whether the store ultimately committed or rolled back.
            return Err(primary_error);
        }
        self.restore_session_dsl_state(session_id, unregister_rollback_snapshot)
            .await;
        let rollback_result = {
            let mut driver = driver.lock().await;
            driver.sync_control_projection_from_dsl_authority();
            driver
                .persist_current_machine_lifecycle("unregister rollback")
                .await
        };
        match rollback_result {
            Ok(()) => Err(primary_error),
            Err(rollback_error) => Err(RuntimeDriverError::Internal(format!(
                "{primary_error}; additionally failed to persist unregister rollback: {rollback_error}"
            ))),
        }
    }

    async fn persist_unregister_progress(
        driver: &SharedDriver,
        context: &'static str,
    ) -> Result<(), RuntimeDriverError> {
        let mut driver = driver.lock().await;
        driver.sync_control_projection_from_dsl_authority();
        driver.persist_current_machine_lifecycle(context).await
    }

    /// Persist the generated unregister prefix before a teardown-required
    /// runtime apply releases its exact executor to the loop handoff. This
    /// method deliberately does not start or join the unregister saga: the
    /// external watcher owns that step after the loop has exited.
    pub(crate) async fn begin_unregister_from_runtime_loop_teardown(
        &self,
        session_id: &SessionId,
        driver: &SharedDriver,
    ) -> Result<(), RuntimeDriverError> {
        let _gate_guard = self
            .lock_current_runtime_loop_driver_authority(session_id, driver)
            .await?;
        match self
            .stage_begin_unregister_session_authority(session_id)
            .await
        {
            Ok(staged) => {
                self.commit_session_dsl_transition(
                    session_id,
                    staged,
                    "TeardownRequiredBeginUnregisterSession",
                )
                .await
                .map_err(RuntimeDriverError::Internal)?;
            }
            Err(reason) => {
                let already_draining =
                    self.session_dsl_state(session_id).await.is_ok_and(|state| {
                        state.registration_phase
                            == crate::meerkat_machine::dsl::RegistrationPhase::Draining
                    });
                if !already_draining {
                    return Err(self
                        .classify_session_dsl_rejection(session_id, reason)
                        .await);
                }
            }
        }
        Self::persist_unregister_progress(
            driver,
            "teardown-required runtime-loop unregister prefix",
        )
        .await
    }

    pub(super) async fn unregister_session_inner(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        self.join_or_start_unregister_teardown(session_id, None, UnregisterTeardownCaller::Explicit)
            .await
    }

    /// Two-phase unregister drain (campaign 0.7.2 D1).
    ///
    /// The shell must quiesce every in-process producer of session-scoped
    /// inputs before the machine commits teardown, so a run that commits
    /// terminally while unregister races it still resolves its completion
    /// waiters with the committed outcome (never an authority error).
    ///
    /// Sequence:
    /// 1. (gate held) `BeginUnregisterSession` opens the machine-owned drain
    ///    window (`registration_phase = Draining`, three obligation flags set)
    ///    and emits the three `Request*ForUnregister` owner-realized effects.
    /// 2. Discharge the runtime-loop-stop obligation by detaching the loop
    ///    channels (dropping `wake_tx`/`effect_tx`) while keeping its
    ///    `JoinHandle`; discharge the comms-drain obligation by aborting the
    ///    drain task while keeping its `JoinHandle`.
    /// 3. **Drop the mutation gate.** The in-flight run commits and the loop
    ///    exits through `lock_current_runtime_loop_driver_authority`, which
    ///    re-acquires this same gate — awaiting the loop under the gate would
    ///    deadlock. The machine-owned `Draining` marker keeps the window safe:
    ///    `EnsureSessionWithExecutor` / `BeginUnregisterSession` re-entry are
    ///    guard-rejected, and the loop's own commits are exactly what we wait
    ///    for.
    /// 4. The independently-owned saga awaits the exact runtime-loop
    ///    `JoinHandle` without aborting it. Caller-visible waiting and live
    ///    interrupt delivery are bounded separately; timeout returns typed
    ///    in-progress truth while this saga remains joinable. The comms drain
    ///    task's `JoinError::is_cancelled` is benign — it was just aborted.
    /// 5. Re-acquire the gate; resolve any completion waiters the in-flight run
    ///    did not already resolve with the runtime-terminated outcome.
    /// 6. Fire the three `*ForUnregister` feedback inputs to close the
    ///    obligations.
    /// 7. Stage + commit the final `UnregisterSession`; persist, remove the
    ///    entry, finalize.
    async fn run_owned_unregister_teardown_inner(
        &self,
        session_id: &SessionId,
        epoch_id: &meerkat_core::RuntimeEpochId,
    ) -> Result<(), RuntimeDriverError> {
        let Some(gate_guard) = self.lock_current_session_mutation_gate(session_id).await else {
            return Ok(());
        };
        tracing::info!(%session_id, "MeerkatMachine::unregister_session_inner_locked_authorized start");
        let driver_handle = {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Ok(());
            };
            if &entry.epoch_id != epoch_id {
                return Ok(());
            }
            Arc::clone(&entry.driver)
        };

        let pending_finalization = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .and_then(|entry| entry.pending_unregister_finalization.clone())
        };
        if let Some(pending) = pending_finalization {
            let exact_retry_witness_is_current = {
                let sessions = self.sessions.read().await;
                sessions.get(session_id).is_some_and(|entry| {
                    entry
                        .dsl_authority
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .snapshot()
                        .state()
                        == pending.committed_snapshot.state()
                })
            };
            if !exact_retry_witness_is_current {
                return Err(RuntimeDriverError::UnregisterFinalizationOutcomeUnknown {
                    reason: format!(
                        "session {session_id} no longer matches the exact generated finalization witness"
                    ),
                });
            }
            let result = self
                .finalize_unregistered_session(
                    Arc::clone(&driver_handle),
                    pending.durability_authority,
                    epoch_id,
                )
                .await;
            if let Err(error) = result {
                // A prior atomic finalization may already have committed. Keep
                // the terminal generated projection and its exact retry
                // witness across every acknowledgement/retry failure so no
                // ordinary lifecycle write can overwrite durable terminal
                // truth.
                return Err(error);
            }
            return self
                .remove_unregistered_session_entry(session_id, epoch_id)
                .await;
        }

        // Phase 1: open the drain window. A concurrent second unregister whose
        // BeginUnregisterSession is rejected because the window is already open
        // is a benign already-in-progress observation, not an error. The
        // machine records whether teardown intent should retain the durable
        // runtime snapshot before the drain can advance lifecycle state.
        tracing::info!(%session_id, "MeerkatMachine::unregister_session_inner_locked_authorized beginning drain window");
        match self
            .stage_begin_unregister_session_authority(session_id)
            .await
        {
            Ok(staged) => {
                self.commit_session_dsl_transition(session_id, staged, "BeginUnregisterSession")
                    .await
                    .map_err(RuntimeDriverError::Internal)?;
            }
            Err(reason) => {
                let already_draining =
                    self.session_dsl_state(session_id).await.is_ok_and(|state| {
                        state.registration_phase
                            == crate::meerkat_machine::dsl::RegistrationPhase::Draining
                    });
                if already_draining {
                    tracing::debug!(
                        %session_id,
                        "BeginUnregisterSession rejected: drain already in progress; attempting final unregister retry only"
                    );
                } else {
                    return Err(self
                        .classify_session_dsl_rejection(session_id, reason)
                        .await);
                }
            }
        }
        // Also re-persist on an already-Draining retry. A prior live feedback
        // transition may have succeeded while its store write failed; this
        // closes that durability gap before any cleanup is attempted again.
        Self::persist_unregister_progress(&driver_handle, "begin or resume unregister teardown")
            .await?;

        // Phase 2: discharge the runtime-loop-stop and comms-drain-abort
        // obligations, retaining both JoinHandles to await below. The live
        // interrupt handle is captured before `take_loop_join_handle` empties
        // the attachment slot, so the drain can hard-cancel an in-flight run
        // (see Phase 4).
        let (
            loop_handle,
            loop_interrupt_handle,
            teardown_slot,
            drain_handle,
            rotation_slot,
            teardown_observations,
        ) = {
            let mut sessions = self.sessions.write().await;
            match sessions.get_mut(session_id) {
                Some(entry) => {
                    let attachment = entry.take_runtime_loop_attachment();
                    let interrupt_handle = attachment
                        .as_ref()
                        .and_then(|attachment| attachment.interrupt_handle.clone())
                        .or_else(|| entry.interrupt_handle());
                    let loop_handle = attachment.map(|attachment| attachment.loop_handle);
                    (
                        loop_handle,
                        interrupt_handle,
                        entry.runtime_loop_teardown.clone(),
                        entry.drain_slot.abort_keeping_handle(),
                        Some(Arc::clone(&entry.supervisor_rotation_task)),
                        Arc::clone(&entry.unregister_teardown_observations),
                    )
                }
                None => {
                    return Ok(());
                }
            }
        };
        let rotation_handle = if let Some(slot) = rotation_slot {
            slot.abort_keeping_handle().await
        } else {
            None
        };

        // Phase 3: drop the mutation gate so the in-flight run and the runtime
        // loop can re-acquire it to commit and exit. Phase 4: await quiescence.
        drop(gate_guard);
        tracing::info!(%session_id, "MeerkatMachine::unregister_session_inner_locked_authorized awaiting runtime-loop and comms-drain quiescence");
        // Track whether each producer concluded cleanly or had to be
        // force-aborted after its drain grace window. The feedback inputs
        // below carry this disposition so the machine records a forced
        // teardown honestly instead of laundering it as clean quiescence.
        if let Some(loop_handle) = loop_handle {
            // Dropping `wake_tx`/`effect_tx` (above) drives the loop through its
            // canonical `StopRuntimeExecutor` exit *once it returns to its
            // `select!`* — but a loop blocked inside `CoreExecutor::apply`
            // (mid `start_turn`) never observes the closed channel. Hard-cancel
            // the in-flight run so a well-behaved executor unwinds `apply` and
            // the loop reaches its clean stop/terminal-handoff exit promptly.
            if let Some(interrupt_handle) = loop_interrupt_handle {
                match crate::tokio::time::timeout(
                    UNREGISTER_INTERRUPT_DELIVERY_GRACE,
                    interrupt_handle
                        .hard_cancel_current_run("runtime session unregistered".to_string()),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        tracing::debug!(
                            %session_id,
                            %error,
                            "in-flight run hard-cancel during unregister drain returned an error (benign if no run was active)"
                        );
                    }
                    Err(_elapsed) => {
                        tracing::warn!(
                            %session_id,
                            "in-flight run hard-cancel delivery exceeded its grace window; exact executor remains owned by the runtime loop"
                        );
                    }
                }
            }

            // The owned saga may wait indefinitely for a genuinely blocked
            // executor, but no caller future owns this wait. Aborting the loop
            // would drop the exact executor and make required external cleanup
            // impossible to retry, so machine-owned Draining truth is retained
            // until the loop actually hands the executor off.
            match loop_handle.await {
                Ok(()) => {}
                Err(join_error) => {
                    teardown_observations
                        .runtime_loop_forced_abort
                        .store(true, std::sync::atomic::Ordering::Release);
                    tracing::warn!(
                        %session_id,
                        error = %join_error,
                        "runtime loop task ended abnormally during unregister drain"
                    );
                }
            }
        }
        if let Some(drain_handle) = drain_handle {
            // The comms drain task was already aborted via
            // `abort_keeping_handle()` above; await its quiescence, but BOUND
            // the wait exactly like the runtime-loop handle. An external member
            // (e.g. a TCP transport drain) whose task is parked in an operation
            // that does not observe the cooperative abort promptly would
            // otherwise wedge teardown forever on an unbounded `.await`
            // (regression: `external_tcp_production_drain` hung past 900s). The
            // grace is far above any realistic cancel latency; on elapse we
            // abort the handle and proceed — the task is already aborted and
            // will unwind, and teardown must not stall on it.
            const COMMS_DRAIN_GRACE: std::time::Duration = std::time::Duration::from_secs(2);
            let drain_abort = drain_handle.abort_handle();
            match crate::tokio::time::timeout(COMMS_DRAIN_GRACE, drain_handle).await {
                Ok(Ok(())) => {}
                Ok(Err(join_error)) if join_error.is_cancelled() => {}
                Ok(Err(join_error)) => {
                    teardown_observations
                        .comms_drain_forced_abort
                        .store(true, std::sync::atomic::Ordering::Release);
                    tracing::warn!(
                        %session_id,
                        error = %join_error,
                        "comms drain task ended abnormally during unregister drain"
                    );
                }
                Err(_elapsed) => {
                    drain_abort.abort();
                    teardown_observations
                        .comms_drain_forced_abort
                        .store(true, std::sync::atomic::Ordering::Release);
                    tracing::warn!(
                        %session_id,
                        "comms drain task did not quiesce within the unregister drain grace window; abandoning the already-aborted drain task so teardown cannot stall"
                    );
                }
            }
        }
        if let Some(rotation_handle) = rotation_handle {
            match rotation_handle.await {
                Ok(()) => {}
                Err(join_error) if join_error.is_cancelled() => {}
                Err(join_error) => {
                    tracing::warn!(
                        %session_id,
                        error = %join_error,
                        "supervisor rotation worker ended abnormally during unregister drain"
                    );
                }
            }
        }

        if let Some(teardown_slot) = teardown_slot.as_ref() {
            teardown_slot.wait_until_published().await;
        }

        // Commit each producer disposition immediately after it becomes
        // known. These generated feedback fields are the durable resume
        // authority; later cleanup/store failures must not force a retry to
        // reconstruct outcomes from missing JoinHandles.
        let Some(pre_cleanup_gate) = self.lock_current_session_mutation_gate(session_id).await
        else {
            return Ok(());
        };
        let pre_cleanup_state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::Internal(format!(
                "unregister producer-feedback authority unavailable for session {session_id}: {reason}"
            ))
        })?;
        let producer_feedback = [
            pre_cleanup_state
                .unregister_runtime_loop_drain_pending
                .then(|| {
                    (
                        crate::meerkat_machine::dsl::MeerkatMachineInput::RuntimeLoopStoppedForUnregister {
                            session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
                            forced_abort: teardown_observations
                                .runtime_loop_forced_abort
                                .load(std::sync::atomic::Ordering::Acquire),
                        },
                        "RuntimeLoopStoppedForUnregister",
                        "unregister runtime-loop disposition",
                    )
                }),
            pre_cleanup_state
                .unregister_comms_drain_exit_pending
                .then(|| {
                    (
                        crate::meerkat_machine::dsl::MeerkatMachineInput::CommsDrainExitedForUnregister {
                            session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
                            forced_abort: teardown_observations
                                .comms_drain_forced_abort
                                .load(std::sync::atomic::Ordering::Acquire),
                        },
                        "CommsDrainExitedForUnregister",
                        "unregister comms-drain disposition",
                    )
                }),
        ];
        for feedback in producer_feedback.into_iter().flatten() {
            let (input, context, persistence_context) = feedback;
            let staged = self
                .stage_session_dsl_transition(session_id, input, context)
                .await
                .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
            self.commit_session_dsl_transition(session_id, staged, context)
                .await
                .map_err(RuntimeDriverError::Internal)?;
            Self::persist_unregister_progress(&driver_handle, persistence_context).await?;
        }
        Self::persist_unregister_progress(
            &driver_handle,
            "unregister producer dispositions reconciled",
        )
        .await?;
        drop(pre_cleanup_gate);

        // Canonical stop terminalization must succeed before any surface-owned
        // cleanup can discard live session material. The call is idempotent for
        // an already-Stopped driver and repairs a loop-local terminalization
        // failure before cleanup is retried.
        match teardown_slot {
            Some(_) => {
                self.join_or_start_runtime_stop_cleanup(
                    session_id,
                    RuntimeStopCleanupCaller::ExplicitUnregister,
                    None,
                    None,
                )
                .await?;
            }
            None => {
                // Storeless registrations and cold-recovered Draining epochs
                // have no live executor in this process. They still require
                // canonical runtime terminalization, but there is no external
                // cleanup object to fabricate or skip.
                crate::control_plane::terminalize_async_stop(&driver_handle, None).await?;
            }
        }

        // Phase 5: re-acquire the gate. If the session vanished while the gate
        // was released (e.g. a racing teardown), the drain already completed
        // elsewhere — nothing left to commit.
        let Some(_gate_guard) = self.lock_current_session_mutation_gate(session_id).await else {
            tracing::debug!(
                %session_id,
                "session removed by a concurrent teardown during unregister drain (benign)"
            );
            return Ok(());
        };
        {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Ok(());
            };
            if &entry.epoch_id != epoch_id {
                return Ok(());
            }
        }
        tracing::info!(%session_id, "MeerkatMachine::unregister_session_inner_locked_authorized re-acquired mutation gate after drain");

        let unregister_state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::Internal(format!(
                "unregister obligation authority unavailable for session {session_id}: {reason}"
            ))
        })?;

        // Resolve any completion waiters the in-flight run did not already
        // resolve. A resumed saga consults the generated obligation flag and
        // never consumes a second completion-authority token for an already
        // closed prefix.
        if unregister_state.unregister_completion_waiter_drain_pending {
            let runtime_terminated_completion_authority = crate::meerkat_machine::driver::
                machine_resolve_runtime_terminated_completion_result(&driver_handle)
                .await?;
            let completions = {
                let sessions = self.sessions.read().await;
                sessions
                    .get(session_id)
                    .map(|entry| Arc::clone(&entry.completions))
            };
            if let Some(completions) = completions {
                completions.lock().await.resolve_all_runtime_terminated(
                    "runtime session unregistered",
                    runtime_terminated_completion_authority,
                );
            }
        }

        // Phase 6: fire the three feedback inputs to close the obligations.
        // Each runtime-loop / comms-drain input carries whether that producer
        // quiesced cleanly or had to be force-aborted after its grace window,
        // so the machine records the real teardown disposition.
        let runtime_loop_forced_abort = teardown_observations
            .runtime_loop_forced_abort
            .load(std::sync::atomic::Ordering::Acquire);
        let comms_drain_forced_abort = teardown_observations
            .comms_drain_forced_abort
            .load(std::sync::atomic::Ordering::Acquire);
        let mut obligation_feedback = Vec::new();
        if unregister_state.unregister_runtime_loop_drain_pending {
            obligation_feedback.push((
                crate::meerkat_machine::dsl::MeerkatMachineInput::RuntimeLoopStoppedForUnregister {
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
                    forced_abort: runtime_loop_forced_abort,
                },
                "RuntimeLoopStoppedForUnregister",
            ));
        }
        if unregister_state.unregister_comms_drain_exit_pending {
            obligation_feedback.push((
                crate::meerkat_machine::dsl::MeerkatMachineInput::CommsDrainExitedForUnregister {
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
                    forced_abort: comms_drain_forced_abort,
                },
                "CommsDrainExitedForUnregister",
            ));
        }
        if unregister_state.unregister_completion_waiter_drain_pending {
            obligation_feedback.push((
                crate::meerkat_machine::dsl::MeerkatMachineInput::CompletionWaitersResolvedForUnregister {
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
                },
                "CompletionWaitersResolvedForUnregister",
            ));
        }
        for (input, context) in obligation_feedback {
            let staged = match self
                .stage_session_dsl_transition(session_id, input, context)
                .await
            {
                Ok(staged) => staged,
                Err(reason) => return Err(RuntimeDriverError::ValidationFailed { reason }),
            };
            self.commit_session_dsl_transition(session_id, staged, context)
                .await
                .map_err(RuntimeDriverError::Internal)?;
        }
        Self::persist_unregister_progress(
            &driver_handle,
            "unregister completion-waiter disposition reconciled",
        )
        .await?;

        // Quiesce the remaining session-scoped operation producers before
        // final lifecycle deletion. Runtime-loop and comms producers are
        // already stopped above; this generated terminalization closes every
        // live op while holding the registry write lock, persists each
        // transition synchronously, then drops the registry's persistence
        // sender. Only after the owned worker joins can the store tombstone
        // this exact epoch without a late detached callback resurrecting it.
        let ops_lifecycle = {
            let sessions = self.sessions.read().await;
            let entry = sessions.get(session_id).ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "session disappeared before ops lifecycle quiescence: {session_id}"
                ))
            })?;
            if &entry.epoch_id != epoch_id {
                return Ok(());
            }
            Arc::clone(&entry.ops_lifecycle)
        };
        ops_lifecycle
            .retire_owner_for_unregister("runtime session unregistered".into())
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "failed to terminalize ops lifecycle before unregister: {error}"
                ))
            })?;
        let persistence_worker = {
            let mut sessions = self.sessions.write().await;
            sessions
                .get_mut(session_id)
                .filter(|entry| &entry.epoch_id == epoch_id)
                .and_then(|entry| entry.ops_lifecycle_persistence_worker.take())
        };
        if let Some(persistence_worker) = persistence_worker {
            join_ops_lifecycle_persistence_worker(persistence_worker).await?;
        }

        // Phase 7: stage + commit the final UnregisterSession.
        tracing::info!(%session_id, "MeerkatMachine::unregister_session_inner_locked_authorized staging unregister");
        let (staged, durability_authority) =
            self.stage_unregister_session_authority(session_id).await?;
        if staged.has_routed_signal_effect() {
            // Final unregister currently has no cross-machine seam signal.
            // Its dispatch-failure recovery may therefore restore the local
            // Draining retry anchor. If the generated contract ever adds a
            // routed effect, fail before dispatch and restore the staged local
            // state; that future contract needs a non-rollback delivery saga.
            let previous_snapshot = staged.previous_snapshot.clone();
            self.restore_session_dsl_state(session_id, previous_snapshot)
                .await;
            if let Some(entry) = self.sessions.write().await.get_mut(session_id) {
                entry.pending_unregister_finalization = None;
            }
            return Err(RuntimeDriverError::Internal(format!(
                "final unregister for session {session_id} unexpectedly emitted a routed seam signal; refusing rollback-unsafe dispatch"
            )));
        }
        let unregister_rollback_snapshot = staged.previous_snapshot.clone();
        tracing::info!(%session_id, "MeerkatMachine::unregister_session_inner_locked_authorized committing unregister");
        if let Err(error) = self
            .commit_session_dsl_transition(session_id, staged, "UnregisterSession")
            .await
        {
            // Safe only because `has_routed_signal_effect` was rejected above
            // before dispatch. No cross-machine observer can have seen this
            // final transition while local authority is restored.
            self.restore_session_dsl_state(session_id, unregister_rollback_snapshot.clone())
                .await;
            let rollback_result = Self::persist_unregister_progress(
                &driver_handle,
                "unregister effect-dispatch rollback",
            )
            .await;
            if let Some(entry) = self.sessions.write().await.get_mut(session_id) {
                entry.pending_unregister_finalization = None;
            }
            return match rollback_result {
                Ok(()) => Err(RuntimeDriverError::Internal(error)),
                Err(rollback_error) => Err(RuntimeDriverError::Internal(format!(
                    "{error}; additionally failed to persist unregister effect-dispatch rollback: {rollback_error}"
                ))),
            };
        }
        tracing::info!(%session_id, "MeerkatMachine::unregister_session_inner_locked_authorized committed unregister");
        tracing::info!(%session_id, "MeerkatMachine::unregister_session_inner_locked_authorized finalizing durable unregister");
        let finalization_result = self
            .finalize_unregister_durability_transaction(
                session_id,
                Arc::clone(&driver_handle),
                durability_authority,
                epoch_id,
                unregister_rollback_snapshot,
            )
            .await;
        if let Err(error) = finalization_result {
            if !matches!(
                &error,
                RuntimeDriverError::UnregisterFinalizationOutcomeUnknown { .. }
            ) && let Some(entry) = self.sessions.write().await.get_mut(session_id)
            {
                entry.pending_unregister_finalization = None;
            }
            return Err(error);
        }
        tracing::info!(%session_id, "MeerkatMachine::unregister_session_inner_locked_authorized removing entry");
        self.remove_unregistered_session_entry(session_id, epoch_id)
            .await?;
        tracing::info!(%session_id, "MeerkatMachine::unregister_session_inner_locked_authorized complete");
        Ok(())
    }

    async fn remove_unregistered_session_entry(
        &self,
        session_id: &SessionId,
        epoch_id: &meerkat_core::RuntimeEpochId,
    ) -> Result<(), RuntimeDriverError> {
        let (drain_task, rotation_task) = {
            let mut sessions = self.sessions.write().await;
            let Some(entry) = sessions.get_mut(session_id) else {
                return Ok(());
            };
            if &entry.epoch_id != epoch_id {
                return Ok(());
            }
            entry.close_handle_teardown_gate();
            (
                entry.drain_slot.take_handle(),
                Arc::clone(&entry.supervisor_rotation_task),
            )
        };
        if let Some(drain_task) = drain_task {
            drain_task.abort();
            let _ = drain_task.await;
        }
        rotation_task.abort_and_wait().await;
        let removed_entry = {
            let mut sessions = self.sessions.write().await;
            if sessions
                .get(session_id)
                .is_some_and(|entry| &entry.epoch_id == epoch_id)
            {
                sessions.remove(session_id)
            } else {
                None
            }
        };
        drop(removed_entry);
        Ok(())
    }

    /// Check whether a runtime driver is already registered for a session.
    pub async fn contains_session(&self, session_id: &SessionId) -> bool {
        self.sessions.read().await.contains_key(session_id)
    }

    /// Observe whether archiving still has runtime retirement work to finish.
    ///
    /// A live registration is immediate residue. After a process restart the
    /// in-memory registry is empty, so the machine-owned durable lifecycle is
    /// also consulted: a persisted non-terminal state is unfinished retirement
    /// residue. [`RuntimeState::Retired`] and [`RuntimeState::Destroyed`] are
    /// both quiescent terminal outcomes; `Retire` cannot and need not run from
    /// Destroyed. An absent lifecycle row is likewise quiescent. Store read
    /// failures are surfaced so callers fail closed instead of misclassifying
    /// unknown durable state as a completed archive.
    pub async fn archive_runtime_residue_present(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, RuntimeDriverError> {
        if let Some(live_state) = self
            .existing_session_visible_runtime_state(session_id)
            .await
        {
            return Ok(!matches!(
                live_state,
                RuntimeState::Retired | RuntimeState::Destroyed
            ));
        }
        let Some(store) = self.store.as_ref() else {
            return Ok(false);
        };
        let runtime_id = LogicalRuntimeId::for_session(session_id);
        let durable_state = crate::store::load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?;
        Ok(durable_state
            .is_some_and(|state| !matches!(state, RuntimeState::Retired | RuntimeState::Destroyed)))
    }

    /// Drop an in-memory, storeless WASM session entry after generated runtime
    /// authority has already terminalized it.
    #[cfg(target_arch = "wasm32")]
    pub async fn discard_terminal_storeless_session(&self, session_id: &SessionId) -> bool {
        if self.store.is_some() {
            return false;
        }
        let Some(snapshot) = self.meerkat_machine_archive_snapshot(session_id).await else {
            return false;
        };
        if !matches!(
            snapshot.control.phase,
            RuntimeState::Retired | RuntimeState::Stopped
        ) || !snapshot.queue.is_empty()
            || !snapshot.steer_queue.is_empty()
        {
            return false;
        }
        let Some(_gate_guard) = self.lock_current_session_mutation_gate(session_id).await else {
            return false;
        };
        let (driver_handle, completions) = {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return false;
            };
            (Arc::clone(&entry.driver), Arc::clone(&entry.completions))
        };
        let runtime_terminated_completion_authority =
            match crate::meerkat_machine::driver::machine_resolve_runtime_terminated_completion_result(
                &driver_handle,
            )
            .await
            {
                Ok(authority) => authority,
                Err(err) => {
                    tracing::warn!(
                        %session_id,
                        error = %err,
                        "failed to resolve terminal completion authority for storeless WASM session discard"
                    );
                    return false;
                }
            };
        completions.lock().await.resolve_all_runtime_terminated(
            "storeless WASM session discarded",
            runtime_terminated_completion_authority,
        );

        // The terminal storeless session has no attached runtime loop or comms
        // drain task to quiesce, so the drain obligations are discharged
        // trivially: open the window (Begin) then immediately close all three
        // obligations before committing the final UnregisterSession. This keeps
        // the wasm discard path on the same machine-owned teardown contract as
        // the native unregister drain.
        match self
            .stage_begin_unregister_session_authority(session_id)
            .await
        {
            Ok(staged) => {
                if let Err(err) = self
                    .commit_session_dsl_transition(session_id, staged, "BeginUnregisterSession")
                    .await
                {
                    tracing::warn!(
                        %session_id,
                        error = %err,
                        "failed to open drain window for storeless WASM session discard"
                    );
                    return false;
                }
            }
            Err(reason) => {
                tracing::warn!(
                    %session_id,
                    error = %reason,
                    "generated MeerkatMachine rejected drain-window open for storeless WASM session discard"
                );
                return false;
            }
        }
        // A terminal storeless session has no runtime loop or comms drain task
        // attached, so both producers conclude trivially (cleanly) — there is
        // nothing to force-abort.
        for (input, context) in [
            (
                crate::meerkat_machine::dsl::MeerkatMachineInput::RuntimeLoopStoppedForUnregister {
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
                    forced_abort: false,
                },
                "RuntimeLoopStoppedForUnregister",
            ),
            (
                crate::meerkat_machine::dsl::MeerkatMachineInput::CommsDrainExitedForUnregister {
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
                    forced_abort: false,
                },
                "CommsDrainExitedForUnregister",
            ),
            (
                crate::meerkat_machine::dsl::MeerkatMachineInput::CompletionWaitersResolvedForUnregister {
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
                },
                "CompletionWaitersResolvedForUnregister",
            ),
        ] {
            match self
                .stage_session_dsl_transition(session_id, input, context)
                .await
            {
                Ok(staged) => {
                    if let Err(err) = self
                        .commit_session_dsl_transition(session_id, staged, context)
                        .await
                    {
                        tracing::warn!(
                            %session_id,
                            error = %err,
                            "failed to close drain obligation for storeless WASM session discard"
                        );
                        return false;
                    }
                }
                Err(reason) => {
                    tracing::warn!(
                        %session_id,
                        error = %reason,
                        "generated MeerkatMachine rejected drain feedback for storeless WASM session discard"
                    );
                    return false;
                }
            }
        }
        let rotation_slot = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .map(|entry| Arc::clone(&entry.supervisor_rotation_task))
        };
        if let Some(rotation_slot) = rotation_slot {
            rotation_slot.abort_and_wait().await;
        }
        let (staged, _durability) = match self.stage_unregister_session_authority(session_id).await
        {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(
                    %session_id,
                    error = %err,
                    "failed to stage final unregister for storeless WASM session discard"
                );
                return false;
            }
        };
        if let Err(err) = self
            .commit_session_dsl_transition(session_id, staged, "UnregisterSession")
            .await
        {
            tracing::warn!(
                %session_id,
                error = %err,
                "failed to commit final unregister for storeless WASM session discard"
            );
            return false;
        }

        let (drain_task, rotation_task) = {
            let mut sessions = self.sessions.write().await;
            let Some(entry) = sessions.get_mut(session_id) else {
                return false;
            };
            entry.close_handle_teardown_gate();
            (
                entry.drain_slot.take_handle(),
                Arc::clone(&entry.supervisor_rotation_task),
            )
        };
        if let Some(drain_task) = drain_task {
            drain_task.abort();
            let _ = drain_task.await;
        }
        rotation_task.abort_and_wait().await;
        let entry = {
            let mut sessions = self.sessions.write().await;
            sessions.remove(session_id)
        };
        let Some(entry) = entry else {
            return false;
        };
        let retired_ops_epoch = entry.epoch_id.clone();
        let driver = Arc::clone(&entry.driver);
        if let Err(err) = self
            .finalize_unregistered_session(
                driver,
                RuntimeOpsLifecycleDurabilityAuthority {
                    action: crate::meerkat_machine::dsl::RuntimeOpsLifecycleDurabilityAction::RetainSnapshot,
                },
                &retired_ops_epoch,
            )
            .await
        {
            tracing::warn!(
                %session_id,
                error = %err,
                "failed to finalize storeless WASM session discard"
            );
            return false;
        }
        true
    }

    /// Check whether a session has an active RuntimeLoop or attachment in
    /// progress.
    ///
    /// `Ok(false)` means only `Queuing` (registered via `prepare_bindings()`
    /// with no executor) or unknown. Driver faults are returned explicitly so
    /// callers cannot accidentally treat a control-plane fault as absence.
    pub async fn session_has_executor(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::SessionHasExecutor {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::Bool(present)) => Ok(present),
            Ok(other) => Err(RuntimeDriverError::Internal(format!(
                "session_has_executor: unexpected command result variant: {other:?}"
            ))),
            Err(error) => Err(MeerkatMachine::driver_error_from_command_error(error)),
        }
    }

    /// Wake the attached runtime loop when machine-owned input truth already
    /// contains active work. This does not mutate lifecycle state; it only
    /// replays the mechanical wake effect for callers that observe queued work
    /// at a boundary where user input must wait for canonical runtime work to
    /// drain.
    pub async fn wake_runtime_if_active_inputs(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, RuntimeDriverError> {
        let (driver, wake_tx) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            (entry.driver.clone(), entry.wake_sender())
        };

        let has_active_inputs = {
            let driver = driver.lock().await;
            !driver.as_driver().active_input_ids().is_empty()
        };
        if !has_active_inputs {
            return Ok(false);
        }

        let Some(wake_tx) = wake_tx else {
            return Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Idle,
            });
        };

        match wake_tx.try_send(()) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(())) => Ok(true),
            Err(mpsc::error::TrySendError::Closed(())) => Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Idle,
            }),
        }
    }

    /// Check whether a session already has a comms runtime configured.
    ///
    /// Returns `true` if `update_peer_ingress_context` was previously called
    /// with a non-None comms runtime for this session (e.g., via
    /// `SessionRuntime::enable_comms_drain`).
    pub async fn session_has_comms(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::SessionHasComms {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::Bool(present)) => Ok(present),
            Ok(other) => Err(RuntimeDriverError::Internal(format!(
                "session_has_comms: unexpected command result variant: {other:?}"
            ))),
            Err(error) => Err(MeerkatMachine::driver_error_from_command_error(error)),
        }
    }

    /// Resolve the session-liveness verdict for an attempted transcript edit
    /// (fork / rewrite / restore) through MeerkatMachine authority.
    ///
    /// The `SESSION_BUSY` disjunction (`runtime_running || has_active_inputs =>
    /// busy`) is a MeerkatMachine-owned fact. The shell extracts the two pure
    /// boolean observations it already computes — `runtime_running` from
    /// `runtime_state` and `has_active_inputs` from `list_active_inputs` — and
    /// mirrors the verdict emitted here. The classifier is a phase-preserving
    /// self-loop, so it never mutates lifecycle state. The caller fails closed
    /// (denies the edit) on any error.
    pub async fn resolve_transcript_edit_admission(
        &self,
        session_id: &SessionId,
        runtime_running: bool,
        has_active_inputs: bool,
    ) -> Result<crate::meerkat_machine::dsl::TranscriptEditAdmissionKind, RuntimeDriverError> {
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveTranscriptEditAdmission {
                    runtime_running,
                    has_active_inputs,
                },
                "ResolveTranscriptEditAdmission",
            )
            .await
            .map_err(RuntimeDriverError::Internal)?;
        effects
            .as_slice()
            .iter()
            .find_map(|effect| {
                match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::TranscriptEditAdmissionResolved {
                    verdict,
                } => Some(*verdict),
                _ => None,
            }
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(
                    "transcript-edit admission emitted no authority verdict".to_string(),
                )
            })
    }

    /// Request cancellation at the next safe boundary for the currently-running turn.
    pub async fn cancel_after_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        self.execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::CancelAfterBoundary {
                session_id: session_id.clone(),
            },
        )
        .await
        .map_err(MeerkatMachine::driver_error_from_command_error)
        .map(|_| ())
    }

    /// Realize pending-input abandonment after the machine has already entered
    /// the Retired terminal phase.
    pub async fn abandon_retired_pending_inputs(
        &self,
        session_id: &SessionId,
        reason: impl Into<String>,
    ) -> Result<usize, RuntimeDriverError> {
        let reason = reason.into();
        let state = self
            .existing_session_runtime_state(session_id)
            .await
            .unwrap_or(RuntimeState::Destroyed);
        if state != RuntimeState::Retired {
            return Err(RuntimeDriverError::NotReady { state });
        }

        let gate = self.session_mutation_gate(session_id).await;
        let _gate_guard = match gate {
            Some(ref g) => Some(g.lock().await),
            None => None,
        };

        let (driver, completions) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            (entry.driver.clone(), entry.completions.clone())
        };

        let abandoned = {
            let mut driver = driver.lock().await;
            driver
                .abandon_pending_inputs(crate::input_state::InputAbandonReason::Retired)
                .await?
        };
        let result_class =
            crate::meerkat_machine::driver::machine_resolve_runtime_terminated_completion_result(
                &driver,
            )
            .await?;
        completions
            .lock()
            .await
            .resolve_all_runtime_terminated(&reason, result_class);
        Ok(abandoned)
    }

    /// Stage a durable session visibility filter through the machine-owned visibility state.
    pub async fn stage_persistent_filter(
        &self,
        session_id: &SessionId,
        filter: meerkat_core::ToolFilter,
        witnesses: std::collections::BTreeMap<
            meerkat_core::ToolName,
            meerkat_core::ToolVisibilityWitness,
        >,
    ) -> Result<meerkat_core::ToolScopeRevision, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::StagePersistentFilter {
                    session_id: session_id.clone(),
                    filter,
                    witnesses,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::VisibilityRevision(revision) => Ok(revision),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for stage_persistent_filter: {other:?}"
            ))),
        }
    }

    /// Record durable deferred-tool visibility intent through the machine seam.
    pub async fn request_deferred_tools(
        &self,
        session_id: &SessionId,
        authorities: Vec<meerkat_core::DeferredToolLoadAuthority>,
    ) -> Result<meerkat_core::ToolScopeRevision, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::RequestDeferredTools {
                    session_id: session_id.clone(),
                    authorities,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::VisibilityRevision(revision) => Ok(revision),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for request_deferred_tools: {other:?}"
            ))),
        }
    }

    /// Publish the committed visible tool set through the machine dispatch.
    ///
    /// Routes the visibility publication through the canonical command path,
    /// enforcing session-existence and Destroyed guards per the TLA+
    /// `VisibleSurfacesMatchAppliedStateInvariant`.
    ///
    /// Returns the validated visibility state on success.
    pub async fn publish_committed_visible_set(
        &self,
        session_id: &SessionId,
        visibility_state: meerkat_core::SessionToolVisibilityState,
    ) -> Result<meerkat_core::SessionToolVisibilityState, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::PublishCommittedVisibleSet {
                    session_id: session_id.clone(),
                    visibility_state: Box::new(visibility_state),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::VisibilityPublished(state) => Ok(state),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for publish_committed_visible_set: {other:?}"
            ))),
        }
    }

    /// Install the runtime-owned shell seam for live LLM reconfiguration.
    pub fn set_session_llm_reconfigure_host(&self, host: Arc<dyn SessionLlmReconfigureHost>) {
        *self
            .llm_reconfigure_host
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(host);
    }

    // NOTE: Realtime-attachment public API was removed as part of
    // the realtime/live-topology DSL plane deletion.
    // Provider session lifecycle now lives outside MeerkatMachine (live-adapter MVP).
}
