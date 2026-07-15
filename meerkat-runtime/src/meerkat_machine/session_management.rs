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
enum UnregisterTeardownAdmission {
    AnyCurrentRegistration,
    ExactTerminalUnattachedRegistration,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UnregisterTeardownWait {
    CallerGrace,
    UntilTerminal,
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

fn pending_unregister_finalization_matches(
    current: Option<&PendingUnregisterFinalization>,
    expected: Option<&PendingUnregisterFinalization>,
) -> bool {
    match (current, expected) {
        (None, None) => true,
        (Some(current), Some(expected)) => {
            current.finalization_id == expected.finalization_id
                && current.durability_authority.action == expected.durability_authority.action
                && current.committed_snapshot.state() == expected.committed_snapshot.state()
        }
        _ => false,
    }
}

#[derive(Clone)]
struct UnregisterEntryIncarnationWitness {
    mutation_gate: Arc<Mutex<()>>,
    ops_lifecycle: Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>,
    #[cfg(feature = "live")]
    live_lifecycle_gate: Arc<Mutex<()>>,
}

impl UnregisterEntryIncarnationWitness {
    fn matches(&self, entry: &RuntimeSessionEntry) -> bool {
        if !Arc::ptr_eq(&entry.mutation_gate, &self.mutation_gate)
            || !Arc::ptr_eq(&entry.ops_lifecycle, &self.ops_lifecycle)
        {
            return false;
        }
        #[cfg(feature = "live")]
        if !Arc::ptr_eq(&entry.live_lifecycle_gate, &self.live_lifecycle_gate) {
            return false;
        }
        true
    }
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

/// Private-field witness that the generated final-unregister durability
/// verdict selected `DeleteSnapshot`. Store finalization tokens must consume
/// this witness, so callers cannot turn a generic lifecycle commit into an
/// ops-snapshot deletion.
pub(crate) struct DeleteOpsFinalizationAuthority {
    _private: (),
}

/// Private-field witness that the generated final-unregister durability
/// verdict selected `RetainSnapshot`. The runtime may preserve the retired ops
/// epoch, but it must still persist the completed lifecycle image without an
/// unfinished unregister-progress prefix.
pub(crate) struct RetainOpsFinalizationAuthority {
    _private: (),
}

#[cfg(test)]
impl DeleteOpsFinalizationAuthority {
    pub(crate) fn for_store_test() -> Self {
        Self { _private: () }
    }
}

impl RuntimeOpsLifecycleDurabilityAuthority {
    #[cfg(test)]
    pub(super) fn action(
        &self,
    ) -> crate::meerkat_machine::dsl::RuntimeOpsLifecycleDurabilityAction {
        self.action
    }

    fn deletes_snapshot(&self) -> bool {
        self.action
            == crate::meerkat_machine::dsl::RuntimeOpsLifecycleDurabilityAction::DeleteSnapshot
    }

    fn delete_ops_finalization_authority(&self) -> Option<DeleteOpsFinalizationAuthority> {
        self.deletes_snapshot()
            .then_some(DeleteOpsFinalizationAuthority { _private: () })
    }

    fn retain_ops_finalization_authority(&self) -> Option<RetainOpsFinalizationAuthority> {
        (self.action
            == crate::meerkat_machine::dsl::RuntimeOpsLifecycleDurabilityAction::RetainSnapshot)
            .then_some(RetainOpsFinalizationAuthority { _private: () })
    }
}

/// Runtime-owned decorator for surface executors that delegate adapter
/// unregister to `MeerkatMachine`.
///
/// The machine mints the attachment ID and retains the matching ID beside the
/// loop `JoinHandle`; neither a surface nor a stale executor can manufacture
/// authority over a replacement attachment.
pub(super) enum MachineManagedPostStopFence {
    AwaitingStop,
    Retained(crate::tokio::sync::OwnedMutexGuard<()>),
    UnregisterSagaOwned,
    NeedsFence,
}

struct MachineManagedPostStopExecutor {
    inner: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    machine: std::sync::Weak<MeerkatMachine>,
    session_id: SessionId,
    attachment_id: RuntimeLoopAttachmentId,
    post_stop_fence: MachineManagedPostStopFence,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl meerkat_core::lifecycle::CoreExecutor for MachineManagedPostStopExecutor {
    fn boundary_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorBoundaryHandle>> {
        self.inner.boundary_handle()
    }

    fn interrupt_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>> {
        self.inner.interrupt_handle()
    }

    fn publication_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>> {
        self.inner.publication_handle()
    }

    fn machine_managed_post_stop_unregister(&self) -> bool {
        // The machine samples this capability from `inner` before constructing
        // the decorator and retains the exact attachment-local cleanup handle.
        // Reporting it again here would make an already-managed executor look
        // eligible for a second layer of machine-managed decoration.
        false
    }

    fn post_stop_cleanup_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPostStopCleanupHandle>> {
        // The inner handle is captured before decoration and published beside
        // the exact attachment. The decorated loop owns only the fenced handoff
        // into that machine-retained cleanup authority.
        None
    }

    fn turn_finalization_boundary_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationBoundaryHandle>> {
        self.inner.turn_finalization_boundary_handle()
    }

    async fn apply(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
        primitive: meerkat_core::lifecycle::run_primitive::RunPrimitive,
    ) -> Result<
        meerkat_core::lifecycle::core_executor::CoreApplyOutput,
        meerkat_core::lifecycle::core_executor::CoreExecutorError,
    > {
        self.inner.apply(run_id, primitive).await
    }

    async fn checkpoint_committed_session_snapshot(
        &mut self,
        session_snapshot: &[u8],
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        self.inner
            .checkpoint_committed_session_snapshot(session_snapshot)
            .await
    }

    async fn reconcile_committed_compaction_projections(
        &mut self,
        intents: &[meerkat_core::CompactionProjectionIntent],
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        self.inner
            .reconcile_committed_compaction_projections(intents)
            .await
    }

    async fn abort_uncommitted_compaction_projections(
        &mut self,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        self.inner.abort_uncommitted_compaction_projections().await
    }

    async fn publish_interaction_terminals(
        &mut self,
        events: &[meerkat_core::event::AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        meerkat_core::lifecycle::core_executor::CoreExecutorError,
    > {
        self.inner.publish_interaction_terminals(events).await
    }

    async fn cancel_after_boundary(
        &mut self,
        reason: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        self.inner.cancel_after_boundary(reason).await
    }

    async fn stop_runtime_executor(
        &mut self,
        reason: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        use meerkat_core::lifecycle::core_executor::CoreExecutorError;

        self.inner.stop_runtime_executor(reason).await?;
        let machine = self.machine.upgrade().ok_or_else(|| {
            CoreExecutorError::control_failed_runtime(format!(
                "runtime machine disappeared before post-stop cleanup fencing for session {}",
                self.session_id
            ))
        })?;
        self.post_stop_fence = machine
            .lock_post_stop_cleanup_attachment(&self.session_id, self.attachment_id)
            .await
            .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))?;
        Ok(())
    }

    async fn cleanup_after_runtime_stop_terminalized(
        &mut self,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        use meerkat_core::lifecycle::core_executor::CoreExecutorError;

        let machine = self.machine.upgrade().ok_or_else(|| {
            CoreExecutorError::control_failed_runtime(format!(
                "runtime machine disappeared before post-stop unregister for session {}",
                self.session_id
            ))
        })?;
        let fence = std::mem::replace(
            &mut self.post_stop_fence,
            MachineManagedPostStopFence::NeedsFence,
        );
        let gate_guard = match fence {
            MachineManagedPostStopFence::Retained(gate_guard) => gate_guard,
            MachineManagedPostStopFence::UnregisterSagaOwned => return Ok(()),
            MachineManagedPostStopFence::NeedsFence => {
                match machine
                    .lock_post_stop_cleanup_attachment(&self.session_id, self.attachment_id)
                    .await
                    .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))?
                {
                    MachineManagedPostStopFence::Retained(gate_guard) => gate_guard,
                    MachineManagedPostStopFence::UnregisterSagaOwned => return Ok(()),
                    MachineManagedPostStopFence::AwaitingStop
                    | MachineManagedPostStopFence::NeedsFence => {
                        return Err(CoreExecutorError::control_failed_runtime(format!(
                            "post-stop cleanup for session {} failed to reacquire its exact mutation fence",
                            self.session_id
                        )));
                    }
                }
            }
            MachineManagedPostStopFence::AwaitingStop => {
                self.post_stop_fence = MachineManagedPostStopFence::AwaitingStop;
                return Err(CoreExecutorError::control_failed_runtime(format!(
                    "post-stop cleanup for session {} has no retained mutation fence",
                    self.session_id
                )));
            }
        };
        machine
            .complete_terminalized_runtime_loop_cleanup_if_current_with_guard(
                &self.session_id,
                self.attachment_id,
                gate_guard,
            )
            .await
            .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))
    }
}

#[cfg(test)]
mod machine_managed_executor_forwarding_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CompactionForwardingProbe {
        reconciles: Arc<AtomicUsize>,
        aborts: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::lifecycle::CoreExecutor for CompactionForwardingProbe {
        async fn apply(
            &mut self,
            _run_id: meerkat_core::RunId,
            _primitive: meerkat_core::lifecycle::run_primitive::RunPrimitive,
        ) -> Result<
            meerkat_core::lifecycle::core_executor::CoreApplyOutput,
            meerkat_core::lifecycle::CoreExecutorError,
        > {
            Err(meerkat_core::lifecycle::CoreExecutorError::Internal(
                "compaction forwarding probe cannot apply turns".to_string(),
            ))
        }

        async fn reconcile_committed_compaction_projections(
            &mut self,
            intents: &[meerkat_core::CompactionProjectionIntent],
        ) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
            assert!(intents.is_empty());
            self.reconciles.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn abort_uncommitted_compaction_projections(
            &mut self,
        ) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
            self.aborts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), meerkat_core::lifecycle::CoreExecutorError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn machine_managed_post_stop_decorator_forwards_compaction_contract() {
        let reconciles = Arc::new(AtomicUsize::new(0));
        let aborts = Arc::new(AtomicUsize::new(0));
        let mut executor = MachineManagedPostStopExecutor {
            inner: Box::new(CompactionForwardingProbe {
                reconciles: Arc::clone(&reconciles),
                aborts: Arc::clone(&aborts),
            }),
            machine: std::sync::Weak::new(),
            session_id: SessionId::new(),
            attachment_id: RuntimeLoopAttachmentId::new(),
            post_stop_fence: MachineManagedPostStopFence::AwaitingStop,
        };

        meerkat_core::lifecycle::CoreExecutor::reconcile_committed_compaction_projections(
            &mut executor,
            &[],
        )
        .await
        .expect("decorator must forward exact empty compaction authority");
        meerkat_core::lifecycle::CoreExecutor::abort_uncommitted_compaction_projections(
            &mut executor,
        )
        .await
        .expect("decorator must forward rejected-boundary compaction cleanup");

        assert_eq!(reconciles.load(Ordering::SeqCst), 1);
        assert_eq!(aborts.load(Ordering::SeqCst), 1);
    }
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

/// Mechanical result of publishing a runtime session entry. The generated
/// registration phase remains authoritative; this only tells the command
/// shell whether the entry it just inserted was reconstructed at the durable
/// unregister retry boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RegisterSessionInnerOutcome {
    Existing,
    Inserted,
    InsertedColdRecoveredDraining,
}

impl RegisterSessionInnerOutcome {
    #[must_use]
    pub(super) const fn inserted(self) -> bool {
        matches!(self, Self::Inserted | Self::InsertedColdRecoveredDraining)
    }

    #[must_use]
    const fn from_storeless_inserted(inserted: bool) -> Self {
        if inserted {
            Self::Inserted
        } else {
            Self::Existing
        }
    }
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
    /// Acquire the same session-scoped lease as `live/open`. A missing session
    /// is idempotent (`Ok(None)`). The unregister owner opens `Draining` under
    /// the matching mutation gate before proving physical channel absence, so
    /// close callbacks cannot deadlock on that gate and terminal cleanup
    /// cannot replace the entry between exact revalidation and close.
    ///
    /// Callers must acquire this before the ordinary mutation gate and retain
    /// it through the final unregister marker and entry removal.
    #[cfg(feature = "live")]
    pub(super) async fn acquire_unregister_live_lifecycle_lease(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<crate::member_live::MemberLiveLifecycleLease>, RuntimeDriverError> {
        if self.session_live_lifecycle_gate(session_id).await.is_none() {
            return Ok(None);
        }
        match self.acquire_live_open_lifecycle_lease(session_id).await {
            Ok(lease) => Ok(Some(lease)),
            Err(_) if self.session_live_lifecycle_gate(session_id).await.is_none() => Ok(None),
            Err(error) => Err(RuntimeDriverError::Internal(error.to_string())),
        }
    }

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
    ) -> Result<RegisterSessionInnerOutcome, RuntimeDriverError> {
        self.register_session_inner_with_materialization_origin(session_id, None)
            .await
    }

    pub(super) async fn register_session_inner_for_actor_materialization(
        &self,
        session_id: SessionId,
        claim_state: Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
    ) -> Result<bool, RuntimeDriverError> {
        self.register_session_inner_with_materialization_origin(session_id, Some(claim_state))
            .await
            .map(RegisterSessionInnerOutcome::inserted)
    }

    async fn register_session_inner_with_materialization_origin(
        &self,
        session_id: SessionId,
        materialization_claim_state: Option<
            Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
        >,
    ) -> Result<RegisterSessionInnerOutcome, RuntimeDriverError> {
        // Serialize the full absent-check -> durable recovery/epoch selection
        // -> map publication transaction with final unregister removal. The
        // session mutation gate does not exist while the entry is absent, so it
        // cannot close this ABA window by itself.
        let _registration_transaction_guard = self
            .lock_session_registration_transaction(&session_id)
            .await;
        self.register_session_inner_under_registration_transaction(
            session_id,
            materialization_claim_state,
        )
        .await
    }

    /// Register while the caller owns this session's stable absent-entry
    /// transaction. Archive uses this form so its durable absence decision,
    /// optional cold recovery, and exact entry capture are one transaction;
    /// ordinary registration enters through the guarded wrapper above.
    pub(super) async fn register_session_inner_under_registration_transaction(
        &self,
        session_id: SessionId,
        materialization_claim_state: Option<
            Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
        >,
    ) -> Result<RegisterSessionInnerOutcome, RuntimeDriverError> {
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
                    return Ok(RegisterSessionInnerOutcome::Existing);
                }
            }
            return self
                .register_storeless_session_inner_sync_build_step(
                    session_id,
                    materialization_claim_state,
                )
                .map(RegisterSessionInnerOutcome::from_storeless_inserted);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if storeless {
            return Box::pin(
                self.register_storeless_session_inner(session_id, materialization_claim_state),
            )
            .await
            .map(RegisterSessionInnerOutcome::from_storeless_inserted);
        }
        Box::pin(self.register_session_inner_impl(session_id, materialization_claim_state)).await
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
        self.register_storeless_session_inner_sync_build_step(session_id, None)
    }

    #[cfg(target_arch = "wasm32")]
    #[inline(never)]
    pub(super) fn register_storeless_session_inner_sync_build_step(
        &self,
        session_id: SessionId,
        materialization_claim_state: Option<
            Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
        >,
    ) -> Result<bool, RuntimeDriverError> {
        let (runtime_id, session_entry) =
            self.make_storeless_session_entry_sync(&session_id, materialization_claim_state)?;
        self.insert_storeless_session_sync(session_id, runtime_id, session_entry)
    }

    #[cfg(target_arch = "wasm32")]
    #[inline(never)]
    fn make_storeless_session_entry_sync(
        &self,
        session_id: &SessionId,
        materialization_claim_state: Option<
            Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
        >,
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
            #[cfg(feature = "live")]
            live_lifecycle_gate: Arc::new(Mutex::new(())),
            supervisor_rotation_task: Arc::new(SupervisorRotationTaskSlot::new()),
            control_projection,
            driver: Arc::new(Mutex::new(entry)),
            ops_lifecycle,
            ops_lifecycle_persistence_worker: None,
            epoch_id,
            handle_teardown_gate,
            materialization_claim_state: materialization_claim_state.unwrap_or_else(|| {
                Arc::new(std::sync::Mutex::new(
                    crate::RuntimeActorMaterializationClaimState::new(false),
                ))
            }),
            cursor_state,
            completions: Arc::new(Mutex::new(crate::completion::CompletionRegistry::new())),
            tool_visibility_owner,
            canonical_runtime_bindings: None,
            attachment_slot: RuntimeLoopAttachmentSlot::Empty,
            runtime_loop_teardown: None,
            unregister_coordinator: None,
            runtime_stop_cleanup_coordinator: None,
            pending_revival_lifecycle_persist: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            pending_unregister_finalization: None,
            unregister_teardown_observations: Arc::new(
                UnregisterTeardownMechanicalObservations::new(),
            ),
            publication_handle: None,
            post_stop_cleanup_handle: None,
            post_stop_cleanup_attachment_id: None,
            post_stop_cleanup_complete: false,
            post_stop_cleanup_gate: Arc::new(Mutex::new(())),
            provisional_interrupt_handle: None,
            provisional_materialization_claim_id: None,
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
                "MeerkatMachine::register_session_inner inserted storeless session"
            );
            Ok(true)
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn register_storeless_session_inner(
        &self,
        session_id: SessionId,
        materialization_claim_state: Option<
            Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
        >,
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
            #[cfg(feature = "live")]
            live_lifecycle_gate: Arc::new(Mutex::new(())),
            supervisor_rotation_task: Arc::new(SupervisorRotationTaskSlot::new()),
            control_projection,
            driver: Arc::new(Mutex::new(entry)),
            ops_lifecycle,
            ops_lifecycle_persistence_worker: None,
            epoch_id,
            handle_teardown_gate,
            materialization_claim_state: materialization_claim_state.unwrap_or_else(|| {
                Arc::new(std::sync::Mutex::new(
                    crate::RuntimeActorMaterializationClaimState::new(false),
                ))
            }),
            cursor_state,
            completions: Arc::new(Mutex::new(crate::completion::CompletionRegistry::new())),
            tool_visibility_owner,
            canonical_runtime_bindings: None,
            attachment_slot: RuntimeLoopAttachmentSlot::Empty,
            runtime_loop_teardown: None,
            unregister_coordinator: None,
            runtime_stop_cleanup_coordinator: None,
            pending_revival_lifecycle_persist: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            pending_unregister_finalization: None,
            unregister_teardown_observations: Arc::new(
                UnregisterTeardownMechanicalObservations::new(),
            ),
            publication_handle: None,
            post_stop_cleanup_handle: None,
            post_stop_cleanup_attachment_id: None,
            post_stop_cleanup_complete: false,
            post_stop_cleanup_gate: Arc::new(Mutex::new(())),
            provisional_interrupt_handle: None,
            provisional_materialization_claim_id: None,
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
                    "MeerkatMachine::register_session_inner inserted storeless session"
                );
                Ok(true)
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut sessions = self.sessions.write().await;
            if let Some(existing) = sessions.get_mut(&session_id) {
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
                    "MeerkatMachine::register_session_inner inserted storeless session"
                );
                Ok(true)
            }
        }
    }

    async fn register_session_inner_impl(
        &self,
        session_id: SessionId,
        materialization_claim_state: Option<
            Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
        >,
    ) -> Result<RegisterSessionInnerOutcome, RuntimeDriverError> {
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
                return Ok(RegisterSessionInnerOutcome::Existing);
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
        let recovered_unregister_retry = recovery_observation.recovered_from_snapshot
            && recovery_observation.unregister_progress.is_some();
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
        let cold_recovered_generated_draining = recovered_unregister_retry
            && dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .state()
                .registration_phase
                == crate::meerkat_machine::dsl::RegistrationPhase::Draining;
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
            #[cfg(feature = "live")]
            live_lifecycle_gate: Arc::new(Mutex::new(())),
            supervisor_rotation_task: Arc::new(SupervisorRotationTaskSlot::new()),
            control_projection,
            driver: Arc::new(Mutex::new(entry)),
            ops_lifecycle,
            ops_lifecycle_persistence_worker: None,
            epoch_id,
            handle_teardown_gate,
            materialization_claim_state: materialization_claim_state.unwrap_or_else(|| {
                Arc::new(std::sync::Mutex::new(
                    crate::RuntimeActorMaterializationClaimState::new(false),
                ))
            }),
            cursor_state,
            completions: Arc::new(Mutex::new(crate::completion::CompletionRegistry::new())),
            tool_visibility_owner,
            canonical_runtime_bindings: None,
            attachment_slot: RuntimeLoopAttachmentSlot::Empty,
            runtime_loop_teardown: None,
            unregister_coordinator: None,
            runtime_stop_cleanup_coordinator: None,
            pending_revival_lifecycle_persist: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            pending_unregister_finalization: None,
            unregister_teardown_observations: recovered_teardown_observations,
            publication_handle: None,
            post_stop_cleanup_handle: None,
            post_stop_cleanup_attachment_id: None,
            post_stop_cleanup_complete: false,
            post_stop_cleanup_gate: Arc::new(Mutex::new(())),
            provisional_interrupt_handle: None,
            provisional_materialization_claim_id: None,
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
            Ok(RegisterSessionInnerOutcome::Existing)
        } else {
            sessions.insert(session_id, session_entry);
            tracing::debug!(
                %runtime_id,
                "MeerkatMachine::register_session_inner inserted session"
            );
            if cold_recovered_generated_draining {
                Ok(RegisterSessionInnerOutcome::InsertedColdRecoveredDraining)
            } else {
                Ok(RegisterSessionInnerOutcome::Inserted)
            }
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
        session_snapshot: Vec<u8>,
    ) -> Result<(), RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::CommitServiceTurnTerminalReceipt {
                    session_id: session_id.clone(),
                    session_snapshot,
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

    /// Ensure an exact runtime attachment whose executor may retain the
    /// machine-issued attachment witness before construction.
    ///
    /// A newly attached executor is returned as a non-clone pending lease.
    /// Its runtime loop has reconciled startup state, but it is not exposed as
    /// serving and is not woken until the caller publishes any surface
    /// sidecars and commits the lease. If a committed attachment already
    /// exists, the factory is not invoked and its exact witness is returned.
    pub async fn ensure_session_with_executor_factory<F>(
        self: &Arc<Self>,
        session_id: SessionId,
        executor_factory: F,
    ) -> Result<EnsureRuntimeExecutorAttachment, RuntimeDriverError>
    where
        F: FnOnce(
                RuntimeExecutorAttachmentWitness,
            ) -> Box<dyn meerkat_core::lifecycle::CoreExecutor>
            + Send
            + 'static,
    {
        let cleanup_spawner = super::MachineCleanupTaskSpawner::acquire()?;
        let machine = Arc::clone(self);
        cleanup_spawner
            .spawn(async move {
                machine
                    .ensure_session_with_executor_factory_inner(
                        session_id,
                        None,
                        false,
                        executor_factory,
                    )
                    .await
            })
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "owned exact executor attachment saga ended without a result: {error}"
                ))
            })?
    }

    pub(super) async fn ensure_session_with_executor_factory_for_materialization<F>(
        self: &Arc<Self>,
        session_id: SessionId,
        expected_claim: super::RuntimeExecutorAttachmentMaterializationClaim,
        turn_finalization_boundary_already_held: bool,
        executor_factory: F,
    ) -> Result<EnsureRuntimeExecutorAttachment, RuntimeDriverError>
    where
        F: FnOnce(
                RuntimeExecutorAttachmentWitness,
            ) -> Box<dyn meerkat_core::lifecycle::CoreExecutor>
            + Send
            + 'static,
    {
        let cleanup_spawner = super::MachineCleanupTaskSpawner::acquire()?;
        let machine = Arc::clone(self);
        cleanup_spawner
            .spawn(async move {
                machine
                    .ensure_session_with_executor_factory_inner(
                        session_id,
                        Some(expected_claim),
                        turn_finalization_boundary_already_held,
                        executor_factory,
                    )
                    .await
            })
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "owned claim-bound executor attachment saga ended without a result: {error}"
                ))
            })?
    }

    /// Open an actor-only reconstruction interval for one exact committed
    /// executor attachment. The returned lease retains this session's mutation
    /// gate, so teardown or same-SessionId replacement cannot cross the service
    /// actor build. It carries no authority to attach or unregister an executor.
    /// Callers that also need the session service's turn-finalization boundary
    /// must acquire that boundary first (canonical B -> M order).
    pub async fn prepare_attached_session_actor_recovery(
        self: &Arc<Self>,
        witness: &RuntimeExecutorAttachmentWitness,
    ) -> Result<super::PreparedAttachedSessionActorRecovery, RuntimeDriverError> {
        if !witness.belongs_to(self) {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "actor-recovery witness belongs to another machine".to_string(),
            });
        }
        let mutation_guard = self
            .lock_current_session_mutation_gate(witness.session_id())
            .await
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        let (claim_id, previous_phase, claim_state, bindings) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(witness.session_id())
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            let exact_attached = entry.epoch_id == witness.epoch_id
                && entry.generated_executor_registration_active()
                && matches!(
                    &entry.attachment_slot,
                    RuntimeLoopAttachmentSlot::Attached(attachment)
                        if attachment.id == witness.attachment_id
                            && !attachment.wake_tx.is_closed()
                            && !attachment.effect_tx.is_closed()
                );
            if !exact_attached {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "executor attachment for session {} changed before actor recovery",
                        witness.session_id()
                    ),
                });
            }
            let claim_id = uuid::Uuid::new_v4();
            let previous_phase;
            {
                let mut state = entry
                    .materialization_claim_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.current.is_some()
                    || !matches!(
                        state.phase,
                        crate::RuntimeActorMaterializationClaimPhase::Vacant
                            | crate::RuntimeActorMaterializationClaimPhase::RetainedActor
                    )
                {
                    return Err(RuntimeDriverError::ValidationFailed {
                        reason: format!(
                            "session {} already has an actor-materialization owner",
                            witness.session_id()
                        ),
                    });
                }
                previous_phase = state.phase;
                state.current = Some(claim_id);
                state.phase = crate::RuntimeActorMaterializationClaimPhase::Prepared;
            }
            let claim_state = Arc::clone(&entry.materialization_claim_state);
            let runtime_authority = crate::session_runtime_bindings_authority(
                witness.session_id().clone(),
                entry.epoch_id.clone(),
                Arc::clone(&entry.dsl_authority),
                Arc::clone(&entry.handle_teardown_gate),
                Some(claim_id),
                Arc::clone(&claim_state),
                None,
                false,
            );
            let bindings = match entry.canonical_runtime_bindings.as_ref() {
                Some(canonical) => canonical.__clone_with_runtime_authority(runtime_authority),
                None => {
                    let changed = {
                        let mut state = claim_state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if state.current == Some(claim_id) {
                            state.current = None;
                            state.phase = previous_phase;
                            Some(Arc::clone(&state.changed))
                        } else {
                            None
                        }
                    };
                    if let Some(changed) = changed {
                        changed.notify_waiters();
                    }
                    return Err(RuntimeDriverError::Internal(format!(
                        "serving executor attachment for session {} lost its canonical runtime handle bundle",
                        witness.session_id()
                    )));
                }
            };
            (claim_id, previous_phase, claim_state, bindings)
        };
        Ok(super::PreparedAttachedSessionActorRecovery::new(
            bindings,
            witness.clone(),
            claim_id,
            claim_state,
            previous_phase,
            mutation_guard,
        ))
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
        if !entry.physical_attachment_is_live() {
            entry.provisional_interrupt_handle = Some(handle);
        }
        Ok(())
    }

    /// Install the temporary interrupt and service-cleanup authorities for a
    /// prepared session before its runtime loop executor is attached.
    ///
    /// The cleanup authority is load-bearing for prepare -> actor-create: a raw
    /// unregister can enter `Draining` before executor attachment, so it must
    /// still acquire the session recovery gate and remove any actor whose
    /// creation already won that gate. The provisional incarnation is replaced
    /// atomically when the real runtime-loop attachment is published.
    pub async fn install_prepared_session_executor_handles(
        &self,
        bindings: &meerkat_core::SessionRuntimeBindings,
        interrupt_handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>,
        cleanup_handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorPostStopCleanupHandle>,
    ) -> Result<(), RuntimeDriverError> {
        let authority = bindings
            .__runtime_authority()
            .downcast_ref::<crate::SessionRuntimeBindingsAuthority>()
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "prepared session handles require MeerkatMachine binding authority"
                    .to_string(),
            })?;
        if bindings.session_id() != &authority.session_id
            || bindings.epoch_id() != &authority.epoch_id
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "prepared session binding identity does not match its runtime authority"
                    .to_string(),
            });
        }
        let claim_id = authority.materialization_claim_id.ok_or_else(|| {
            RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "prepared binding for session {} has no actor-materialization authority",
                    bindings.session_id()
                ),
            }
        })?;
        let session_id = bindings.session_id();
        let expected_dsl_authority = Arc::clone(&authority.dsl_authority);
        let expected_teardown_gate = Arc::clone(&authority.teardown_gate);
        let _mutation_guard = self
            .lock_current_session_mutation_gate(session_id)
            .await
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        if entry.epoch_id != *bindings.epoch_id()
            || !Arc::ptr_eq(&entry.dsl_authority, &expected_dsl_authority)
            || !Arc::ptr_eq(&entry.handle_teardown_gate, &expected_teardown_gate)
            || !Arc::ptr_eq(
                &entry.materialization_claim_state,
                &authority.materialization_claim_state,
            )
        {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "prepared binding epoch/authority for session {session_id} was replaced"
                ),
            });
        }
        {
            let state = entry
                .materialization_claim_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.exact_claim_is(
                claim_id,
                &[
                    crate::RuntimeActorMaterializationClaimPhase::Prepared,
                    crate::RuntimeActorMaterializationClaimPhase::Staged,
                ],
            ) {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "prepared materialization claim for session {session_id} is no longer current"
                    ),
                });
            }
        }
        if entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state()
            .registration_phase
            == crate::meerkat_machine::dsl::RegistrationPhase::Draining
        {
            return Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            });
        }
        if entry.clear_dead_attachment() {
            entry
                .stage_generated_executor_exit_observation()
                .map_err(|reason| {
                    RuntimeDriverError::Internal(format!(
                        "generated MeerkatMachine rejected executor-exit observation: {reason}"
                    ))
                })?;
        }
        if entry.physical_attachment_is_live() {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "session {session_id} attached an executor before provisional handle installation"
                ),
            });
        }
        if entry
            .provisional_materialization_claim_id
            .is_some_and(|installed| installed != claim_id)
        {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "session {session_id} already has a different provisional materialization owner"
                ),
            });
        }
        entry.install_provisional_interrupt_handle(claim_id, interrupt_handle);
        entry.install_provisional_post_stop_cleanup_handle(claim_id, cleanup_handle);
        Ok(())
    }

    /// Transfer rollback ownership to a published staged-session slot while
    /// preserving the exact claim for its later actor creation.
    pub async fn commit_prepared_session_materialization_staged(
        &self,
        bindings: &meerkat_core::SessionRuntimeBindings,
    ) -> Result<(), RuntimeDriverError> {
        let authority = bindings
            .__runtime_authority()
            .downcast_ref::<crate::SessionRuntimeBindingsAuthority>()
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "staged materialization commit requires MeerkatMachine authority"
                    .to_string(),
            })?;
        let claim_id = authority.materialization_claim_id.ok_or_else(|| {
            RuntimeDriverError::StaleAuthority {
                reason: "staged materialization binding has no exact claim".to_string(),
            }
        })?;
        let _gate_guard = self
            .lock_current_session_mutation_gate(bindings.session_id())
            .await
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(bindings.session_id())
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        if entry.epoch_id != *bindings.epoch_id()
            || !Arc::ptr_eq(
                &entry.materialization_claim_state,
                &authority.materialization_claim_state,
            )
        {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "staged materialization registration was replaced".to_string(),
            });
        }
        let mut state = entry
            .materialization_claim_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.exact_claim_is(
            claim_id,
            &[crate::RuntimeActorMaterializationClaimPhase::Prepared],
        ) {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "staged materialization claim is no longer prepared".to_string(),
            });
        }
        state.phase = crate::RuntimeActorMaterializationClaimPhase::Staged;
        Ok(())
    }

    /// Commit a created actor that intentionally remains without an executor.
    /// The exact provisional cleanup remains retained for ordinary unregister.
    pub async fn commit_prepared_session_actor_unattached(
        &self,
        bindings: &meerkat_core::SessionRuntimeBindings,
    ) -> Result<(), RuntimeDriverError> {
        let authority = bindings
            .__runtime_authority()
            .downcast_ref::<crate::SessionRuntimeBindingsAuthority>()
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "actor materialization commit requires MeerkatMachine authority"
                    .to_string(),
            })?;
        let claim_id = authority.materialization_claim_id.ok_or_else(|| {
            RuntimeDriverError::StaleAuthority {
                reason: "actor materialization binding has no exact claim".to_string(),
            }
        })?;
        let _gate_guard = self
            .lock_current_session_mutation_gate(bindings.session_id())
            .await
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        let changed = {
            let sessions = self.sessions.read().await;
            let entry =
                sessions
                    .get(bindings.session_id())
                    .ok_or(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    })?;
            if entry.epoch_id != *bindings.epoch_id()
                || !Arc::ptr_eq(
                    &entry.materialization_claim_state,
                    &authority.materialization_claim_state,
                )
            {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: "actor materialization registration was replaced".to_string(),
                });
            }
            let mut state = entry
                .materialization_claim_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !state.exact_claim_is(
                claim_id,
                &[crate::RuntimeActorMaterializationClaimPhase::ActorMaterializedPendingCommit],
            ) {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: "actor materialization claim is no longer committable".to_string(),
                });
            }
            state.current = None;
            state.phase = crate::RuntimeActorMaterializationClaimPhase::RetainedActor;
            state.rollback_registration_available = false;
            Arc::clone(&state.changed)
        };
        changed.notify_waiters();
        Ok(())
    }

    /// Roll back only the runtime registration created by the exact
    /// `prepare_bindings` call represented by `bindings`.
    ///
    /// Idempotent preparation of a pre-existing entry does not mint rollback
    /// authority, and a stale binding can never unregister a same-SessionId
    /// replacement.
    pub async fn unregister_session_if_bindings_owned_registration(
        &self,
        bindings: &meerkat_core::SessionRuntimeBindings,
    ) -> Result<bool, RuntimeDriverError> {
        self.rollback_session_materialization_bindings(bindings, false)
            .await
    }

    /// Exact rollback variant for runtime-loop rematerialization, where the
    /// caller already owns the persistent service turn-finalization boundary.
    pub async fn unregister_session_if_bindings_owned_registration_under_turn_finalization_boundary(
        &self,
        bindings: &meerkat_core::SessionRuntimeBindings,
    ) -> Result<bool, RuntimeDriverError> {
        self.rollback_session_materialization_bindings(bindings, true)
            .await
    }

    async fn rollback_session_materialization_bindings(
        &self,
        bindings: &meerkat_core::SessionRuntimeBindings,
        turn_finalization_boundary_already_held: bool,
    ) -> Result<bool, RuntimeDriverError> {
        let authority = bindings
            .__runtime_authority()
            .downcast_ref::<crate::SessionRuntimeBindingsAuthority>()
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "runtime registration rollback requires MeerkatMachine binding authority"
                    .to_string(),
            })?;
        if bindings.session_id() != &authority.session_id
            || bindings.epoch_id() != &authority.epoch_id
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "runtime registration rollback binding identity mismatch".to_string(),
            });
        }
        let Some(claim_id) = authority.materialization_claim_id else {
            return Ok(false);
        };
        let exact_claim = {
            let mut state = authority
                .materialization_claim_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.current != Some(claim_id) {
                false
            } else if state.phase == crate::RuntimeActorMaterializationClaimPhase::Aborting {
                true
            } else if matches!(
                state.phase,
                crate::RuntimeActorMaterializationClaimPhase::Prepared
                    | crate::RuntimeActorMaterializationClaimPhase::Staged
                    | crate::RuntimeActorMaterializationClaimPhase::ActorCreating
                    | crate::RuntimeActorMaterializationClaimPhase::ActorMaterializedPendingCommit
            ) {
                state.phase = crate::RuntimeActorMaterializationClaimPhase::Aborting;
                true
            } else {
                false
            }
        };
        if !exact_claim {
            return Ok(false);
        }
        self.abort_prepared_session_materialization_claim(
            bindings.session_id(),
            claim_id,
            Some(bindings.epoch_id()),
            Some(&authority.materialization_claim_state),
            turn_finalization_boundary_already_held,
        )
        .await
    }

    pub(super) async fn abort_prepared_session_materialization_claim(
        &self,
        session_id: &SessionId,
        claim_id: uuid::Uuid,
        expected_epoch: Option<&meerkat_core::RuntimeEpochId>,
        expected_claim_state: Option<
            &Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
        >,
        turn_finalization_boundary_already_held: bool,
    ) -> Result<bool, RuntimeDriverError> {
        let Some(gate_guard) = self.lock_current_session_mutation_gate(session_id).await else {
            return Ok(false);
        };
        let (rollback_registration, provisional_cleanup_attachment_id) = {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Ok(false);
            };
            if expected_epoch.is_some_and(|epoch| &entry.epoch_id != epoch)
                || expected_claim_state
                    .is_some_and(|state| !Arc::ptr_eq(&entry.materialization_claim_state, state))
            {
                return Ok(false);
            }
            let mut state = entry
                .materialization_claim_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.current != Some(claim_id) {
                return Ok(false);
            }
            if state.phase != crate::RuntimeActorMaterializationClaimPhase::Aborting {
                if matches!(
                    state.phase,
                    crate::RuntimeActorMaterializationClaimPhase::RetainedActor
                        | crate::RuntimeActorMaterializationClaimPhase::Vacant
                ) {
                    return Ok(false);
                }
                state.phase = crate::RuntimeActorMaterializationClaimPhase::Aborting;
            }
            (
                state.rollback_registration_available,
                (entry.provisional_materialization_claim_id == Some(claim_id))
                    .then_some(entry.post_stop_cleanup_attachment_id)
                    .flatten(),
            )
        };

        if rollback_registration {
            // Lock ordering is live/lifecycle -> mutation everywhere. The
            // claim inspection above needed the mutation fence first, so drop
            // it, acquire the raw live lease, then re-acquire the mutation
            // gate tied to that exact lease and revalidate before unregister.
            // Physical absence is proved only after canonical unregister has
            // published Draining and released the mutation gate for callbacks.
            #[cfg(feature = "live")]
            let (gate_guard, live_lifecycle_lease) = {
                drop(gate_guard);
                let Some(live_lifecycle_lease) = self
                    .acquire_unregister_live_lifecycle_lease(session_id)
                    .await?
                else {
                    return Ok(false);
                };
                let Some(gate_guard) = self
                    .lock_session_mutation_gate_for_live_lifecycle_lease(
                        session_id,
                        &live_lifecycle_lease,
                    )
                    .await
                else {
                    return Ok(false);
                };
                let still_owns_rollback = {
                    let sessions = self.sessions.read().await;
                    sessions.get(session_id).is_some_and(|entry| {
                        !expected_epoch.is_some_and(|epoch| &entry.epoch_id != epoch)
                            && !expected_claim_state.is_some_and(|state| {
                                !Arc::ptr_eq(&entry.materialization_claim_state, state)
                            })
                            && {
                                let state = entry
                                    .materialization_claim_state
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                state.current == Some(claim_id)
                                    && state.phase
                                        == crate::RuntimeActorMaterializationClaimPhase::Aborting
                                    && state.rollback_registration_available
                            }
                    })
                };
                if !still_owns_rollback {
                    return Ok(false);
                }
                (gate_guard, Some(live_lifecycle_lease))
            };
            if turn_finalization_boundary_already_held
                && let Some(attachment_id) = provisional_cleanup_attachment_id
            {
                // B is already held and this exact M guard still fences the
                // prepared registration. Complete actor cleanup in B -> M -> R
                // order before canonical unregister can expose its ordinary
                // cleanup path.
                self.complete_post_stop_cleanup_if_needed(session_id, attachment_id, true)
                    .await?;
            }
            drop(gate_guard);
            #[cfg(feature = "live")]
            drop(live_lifecycle_lease);
            self.join_or_start_unregister_teardown(
                session_id,
                expected_epoch,
                UnregisterTeardownCaller::Explicit,
            )
            .await?;
            return Ok(true);
        }

        // A materialization against a pre-existing registration owns only its
        // exact actor/sidecar attempt. A boundary-owned caller keeps M while
        // invoking the non-reentrant cleanup seam, preserving B -> M -> R and
        // preventing an ordinary unregister from taking cleanup_gate -> B.
        // The ordinary path releases M around the callback, then revalidates.
        let _gate_guard = if turn_finalization_boundary_already_held {
            if let Some(attachment_id) = provisional_cleanup_attachment_id {
                self.complete_post_stop_cleanup_if_needed(session_id, attachment_id, true)
                    .await?;
            }
            gate_guard
        } else {
            drop(gate_guard);
            if let Some(attachment_id) = provisional_cleanup_attachment_id {
                self.complete_post_stop_cleanup_if_needed(session_id, attachment_id, false)
                    .await?;
            }
            let Some(gate_guard) = self.lock_current_session_mutation_gate(session_id).await else {
                return Ok(false);
            };
            gate_guard
        };
        let changed = {
            let mut sessions = self.sessions.write().await;
            let Some(entry) = sessions.get_mut(session_id) else {
                return Ok(false);
            };
            if expected_epoch.is_some_and(|epoch| &entry.epoch_id != epoch)
                || expected_claim_state
                    .is_some_and(|state| !Arc::ptr_eq(&entry.materialization_claim_state, state))
                || entry
                    .provisional_materialization_claim_id
                    .is_some_and(|id| id != claim_id)
            {
                return Ok(false);
            }
            let changed = {
                let mut state = entry
                    .materialization_claim_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !state.exact_claim_is(
                    claim_id,
                    &[crate::RuntimeActorMaterializationClaimPhase::Aborting],
                ) {
                    return Ok(false);
                }
                state.current = None;
                state.phase = crate::RuntimeActorMaterializationClaimPhase::Vacant;
                Arc::clone(&state.changed)
            };
            entry.provisional_interrupt_handle = None;
            entry.provisional_materialization_claim_id = None;
            entry.post_stop_cleanup_handle = None;
            entry.post_stop_cleanup_attachment_id = None;
            entry.post_stop_cleanup_complete = false;
            entry.post_stop_cleanup_gate = Arc::new(Mutex::new(()));
            changed
        };
        changed.notify_waiters();
        Ok(false)
    }

    /// Normalize the one ownerless executor shape admitted by Mob
    /// missing-live revival while the caller retains the exact session
    /// mutation gate acquired by materialization preparation.
    ///
    /// This deliberately does not replace a live/dead attachment or join a
    /// teardown. It only re-drives machine authority after physical absence
    /// has already been proven by an empty exact attachment slot. The driver,
    /// runtime epoch, input ledger/queue, and completion registry remain the
    /// same objects; only the stale executor-binding tuple is cleared.
    pub(super) async fn normalize_missing_live_session_materialization(
        &self,
        session_id: &SessionId,
        expected_epoch: &meerkat_core::RuntimeEpochId,
        materialization_claim_id: uuid::Uuid,
        expected_claim_state: &Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
        _mutation_guard: &crate::tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<(), RuntimeDriverError> {
        let (driver, dsl_authority, pending_lifecycle_persist) = {
            let mut sessions = self.sessions.write().await;
            let entry = sessions
                .get_mut(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            if &entry.epoch_id != expected_epoch
                || !Arc::ptr_eq(&entry.materialization_claim_state, expected_claim_state)
            {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "missing-live materialization for session {session_id} lost its exact runtime epoch or claim slot"
                    ),
                });
            }
            {
                let claim = entry
                    .materialization_claim_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !claim.exact_claim_is(
                    materialization_claim_id,
                    &[crate::RuntimeActorMaterializationClaimPhase::Prepared],
                ) {
                    return Err(RuntimeDriverError::StaleAuthority {
                        reason: format!(
                            "missing-live materialization for session {session_id} no longer owns its exact prepared claim"
                        ),
                    });
                }
            }
            if let Some(error) = entry.dsl_mutation_blocked_by_unregister(session_id) {
                return Err(error);
            }
            {
                let authority = entry
                    .dsl_authority
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let state = authority.state();
                let expected_session_id = dsl::SessionId::from_domain(session_id);
                if state.session_id.as_ref() != Some(&expected_session_id)
                    || !matches!(
                        state.lifecycle_phase,
                        dsl::MeerkatPhase::Idle | dsl::MeerkatPhase::Attached
                    )
                    || !matches!(
                        state.registration_phase,
                        dsl::RegistrationPhase::Queuing | dsl::RegistrationPhase::Active
                    )
                    || state.current_run_id.is_some()
                    || state.runtime_stop_deferred
                {
                    return Err(RuntimeDriverError::NotReady {
                        state: dsl_authority::runtime_phase_from_authority(&authority),
                    });
                }
            }
            if entry.runtime_stop_cleanup_coordinator.is_some() {
                // A completed exact stop receipt has no live owner and may be
                // retired. Pending, failed, stale, or attachment-owned cleanup
                // remains fail-closed inside this exact helper.
                entry.retire_completed_runtime_stop_after_revival(session_id)?;
            }
            if !matches!(entry.attachment_slot, RuntimeLoopAttachmentSlot::Empty)
                || entry.runtime_loop_teardown.is_some()
                || entry.runtime_stop_cleanup_coordinator.is_some()
                || entry.unregister_coordinator.is_some()
                || entry.pending_unregister_finalization.is_some()
                || entry.provisional_materialization_claim_id.is_some()
                || entry.post_stop_cleanup_handle.is_some()
                    != entry.post_stop_cleanup_attachment_id.is_some()
                || entry
                    .post_stop_cleanup_attachment_id
                    .is_some_and(|_| !entry.post_stop_cleanup_complete)
            {
                return Err(RuntimeDriverError::RuntimeStopInProgress {
                    runtime_id: entry.runtime_id.clone(),
                });
            }
            (
                Arc::clone(&entry.driver),
                Arc::clone(&entry.dsl_authority),
                Arc::clone(&entry.pending_revival_lifecycle_persist),
            )
        };

        // M is retained by the caller. Take the driver projection lock before
        // the synchronous DSL authority lock, matching the established
        // projection-realization ordering. There are no await points after
        // authority changes, so cancellation cannot expose a half-normalized
        // binding tuple.
        let mut driver_guard = driver.lock().await;
        let mut authority = dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous_snapshot = authority.snapshot();
        let normalize_result = (|| -> Result<(), RuntimeDriverError> {
            let state = authority.state();
            let expected_session_id = dsl::SessionId::from_domain(session_id);
            let admissible_shape = state.session_id.as_ref() == Some(&expected_session_id)
                && matches!(
                    state.lifecycle_phase,
                    dsl::MeerkatPhase::Idle | dsl::MeerkatPhase::Attached
                )
                && matches!(
                    state.registration_phase,
                    dsl::RegistrationPhase::Queuing | dsl::RegistrationPhase::Active
                )
                && state.current_run_id.is_none()
                && !state.runtime_stop_deferred;
            if !admissible_shape {
                return Err(RuntimeDriverError::NotReady {
                    state: dsl_authority::runtime_phase_from_authority(&authority),
                });
            }

            let exited = Self::stage_runtime_owner_dsl_transition_on_locked_authority(
                &mut authority,
                crate::meerkat_machine_types::MeerkatMachineFieldlessRuntimeInternalInput::RuntimeExecutorExited,
            )
            .map_err(RuntimeDriverError::Internal)?;
            if exited.has_routed_signal_effect() {
                return Err(RuntimeDriverError::Internal(
                    "missing-live executor-exit observation unexpectedly emitted a routed signal"
                        .into(),
                ));
            }
            let readmitted = Self::stage_dsl_transition_on_locked_authority(
                &mut authority,
                dsl::MeerkatMachineInput::RegisterSession {
                    session_id: expected_session_id,
                },
                "MissingLiveMaterializationReadmit",
            )
            .map_err(RuntimeDriverError::Internal)?;
            if readmitted.has_routed_signal_effect() {
                return Err(RuntimeDriverError::Internal(
                    "missing-live same-session readmission unexpectedly emitted a routed signal"
                        .into(),
                ));
            }

            let repaired = authority.state();
            if repaired.lifecycle_phase != dsl::MeerkatPhase::Idle
                || repaired.registration_phase != dsl::RegistrationPhase::Queuing
                || repaired.current_run_id.is_some()
                || repaired.runtime_stop_deferred
                || repaired.active_runtime_id.is_some()
                || repaired.active_fence_token.is_some()
                || repaired.active_runtime_generation.is_some()
                || repaired.active_runtime_epoch_id.is_some()
            {
                return Err(RuntimeDriverError::Internal(
                    "missing-live normalization did not produce cleared Idle/Queuing executor authority"
                        .into(),
                ));
            }
            Ok(())
        })();
        if let Err(error) = normalize_result {
            authority.restore_snapshot(previous_snapshot);
            return Err(error);
        }

        // No driver ledger or queue mutation occurs: only mirror the repaired
        // coarse phase and bind the next exact attachment commit to durable
        // lifecycle publication.
        driver_guard.set_control_projection(RuntimeState::Idle, None, None);
        pending_lifecycle_persist.store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    pub(super) async fn ensure_session_with_executor_inner(
        self: &Arc<Self>,
        session_id: SessionId,
        executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    ) -> Result<(), RuntimeDriverError> {
        match self
            .ensure_session_with_executor_factory_inner(session_id, None, false, move |_| executor)
            .await?
        {
            EnsureRuntimeExecutorAttachment::Existing(_) => Ok(()),
            EnsureRuntimeExecutorAttachment::Pending(pending) => pending.commit().await.map(|_| ()),
        }
    }

    async fn ensure_session_with_executor_factory_inner<F>(
        self: &Arc<Self>,
        session_id: SessionId,
        expected_materialization_claim: Option<
            super::RuntimeExecutorAttachmentMaterializationClaim,
        >,
        turn_finalization_boundary_already_held: bool,
        executor_factory: F,
    ) -> Result<EnsureRuntimeExecutorAttachment, RuntimeDriverError>
    where
        F: FnOnce(
            RuntimeExecutorAttachmentWitness,
        ) -> Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    {
        // Acquire the cancellation-safe dispatcher before this saga claims or
        // publishes any attachment authority. A transient runtime-build
        // failure therefore leaves the machine untouched and remains
        // retryable by the next caller.
        let cleanup_spawner = super::MachineCleanupTaskSpawner::acquire()?;

        enum ExistingExecutorClaim {
            AlreadyClaimed(RuntimeExecutorAttachmentWitness),
            Blocked(RuntimeDriverError),
            Rejected(String),
            Claimed {
                gate: Arc<Mutex<()>>,
                driver: SharedDriver,
                completions: SharedCompletionRegistry,
                ops_lifecycle: Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>,
                epoch_id: meerkat_core::RuntimeEpochId,
                dsl_authority: Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
                staged: Box<StagedSessionDslInput>,
                repaired_dead_attachment: bool,
                _gate_guard: crate::tokio::sync::OwnedMutexGuard<()>,
            },
        }

        // The cold executor path bypasses register_session_inner, so it must
        // own the same stable absent-entry transaction slot through durable
        // recovery and session-map publication. Release it before executor
        // construction/startup, which is already fenced by the newly
        // published entry's mutation gate and exact attachment id.
        let registration_transaction_guard = self
            .lock_session_registration_transaction(&session_id)
            .await;
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
                let materialization_rejection = {
                    let state = entry
                        .materialization_claim_state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    match expected_materialization_claim.as_ref() {
                        Some(expected)
                            if entry.epoch_id == expected.epoch_id
                                && Arc::ptr_eq(
                                    &entry.materialization_claim_state,
                                    &expected.claim_state,
                                )
                                && state.exact_claim_is(
                                    expected.claim_id,
                                    &[crate::RuntimeActorMaterializationClaimPhase::ActorMaterializedPendingCommit],
                                ) => None,
                        Some(_) => Some(RuntimeDriverError::StaleAuthority {
                            reason: format!(
                                "session {session_id} no longer owns the exact actor-materialization claim required for executor attachment"
                            ),
                        }),
                        None
                            if state.current.is_none()
                                && matches!(
                                    state.phase,
                                    crate::RuntimeActorMaterializationClaimPhase::Vacant
                                        | crate::RuntimeActorMaterializationClaimPhase::RetainedActor
                                ) => None,
                        None => Some(RuntimeDriverError::ValidationFailed {
                            reason: format!(
                                "session {session_id} has an outstanding actor-materialization claim; attach through PreparedSessionMaterialization"
                            ),
                        }),
                    }
                };
                if let Some(error) = materialization_rejection {
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
                    match &entry.attachment_slot {
                        RuntimeLoopAttachmentSlot::Attached(attachment)
                            if entry.attachment_is_live() =>
                        {
                            break ExistingExecutorClaim::AlreadyClaimed(
                                RuntimeExecutorAttachmentWitness::new(
                                    Arc::downgrade(&self.shared),
                                    session_id.clone(),
                                    entry.epoch_id.clone(),
                                    attachment.id,
                                ),
                            );
                        }
                        RuntimeLoopAttachmentSlot::Pending(_) => {
                            break ExistingExecutorClaim::Blocked(
                                RuntimeDriverError::RuntimeStopInProgress {
                                    runtime_id: entry.runtime_id.clone(),
                                },
                            );
                        }
                        RuntimeLoopAttachmentSlot::Attached(_) => {
                            break ExistingExecutorClaim::Rejected(format!(
                                "session {session_id} retains a dead executor attachment after repair"
                            ));
                        }
                        RuntimeLoopAttachmentSlot::Empty => {
                            break ExistingExecutorClaim::Rejected(format!(
                                "session {session_id} has an active executor registration without an exact attachment"
                            ));
                        }
                    }
                }
                if entry.has_live_attachment() {
                    match entry.stage_generated_executor_registration_claim(&session_id) {
                        Ok(_) => {
                            break ExistingExecutorClaim::Rejected(format!(
                                "session {session_id} granted a second executor claim over a live attachment"
                            ));
                        }
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
                            epoch_id: entry.epoch_id.clone(),
                            dsl_authority: Arc::clone(&entry.dsl_authority),
                            staged: Box::new(staged),
                            repaired_dead_attachment,
                            _gate_guard: gate_guard,
                        };
                    }
                    Err(reason) => break ExistingExecutorClaim::Rejected(reason),
                }
            }

            if expected_materialization_claim.is_some() {
                break ExistingExecutorClaim::Blocked(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "session {session_id} lost the registration owned by its actor-materialization claim"
                    ),
                });
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
                    #[cfg(feature = "live")]
                    live_lifecycle_gate: Arc::new(Mutex::new(())),
                    supervisor_rotation_task: Arc::new(SupervisorRotationTaskSlot::new()),
                    control_projection,
                    driver: driver.clone(),
                    ops_lifecycle: recovered_ops.clone(),
                    ops_lifecycle_persistence_worker: None,
                    epoch_id: recovered_epoch,
                    handle_teardown_gate: crate::handles::HandleTeardownGate::open(),
                    materialization_claim_state: Arc::new(std::sync::Mutex::new(
                        crate::RuntimeActorMaterializationClaimState::new(false),
                    )),
                    cursor_state: recovered_cursors,
                    completions: completions.clone(),
                    tool_visibility_owner,
                    canonical_runtime_bindings: None,
                    attachment_slot: RuntimeLoopAttachmentSlot::Empty,
                    runtime_loop_teardown: None,
                    unregister_coordinator: None,
                    runtime_stop_cleanup_coordinator: None,
                    pending_revival_lifecycle_persist: Arc::new(
                        std::sync::atomic::AtomicBool::new(false),
                    ),
                    pending_unregister_finalization: None,
                    unregister_teardown_observations: recovered_teardown_observations,
                    publication_handle: None,
                    post_stop_cleanup_handle: None,
                    post_stop_cleanup_attachment_id: None,
                    post_stop_cleanup_complete: false,
                    post_stop_cleanup_gate: Arc::new(Mutex::new(())),
                    provisional_interrupt_handle: None,
                    provisional_materialization_claim_id: None,
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
                        epoch_id: entry.epoch_id.clone(),
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
        drop(registration_transaction_guard);

        let (
            driver,
            completions,
            ops_lifecycle,
            epoch_id,
            dsl_authority,
            staged_registration,
            repaired_dead_attachment,
            registration_gate,
            _gate_guard,
        ) = match existing {
            ExistingExecutorClaim::AlreadyClaimed(witness) => {
                return Ok(EnsureRuntimeExecutorAttachment::Existing(witness));
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
                epoch_id,
                dsl_authority,
                staged,
                repaired_dead_attachment,
                _gate_guard,
            } => (
                driver,
                completions,
                ops_lifecycle,
                epoch_id,
                dsl_authority,
                staged,
                repaired_dead_attachment,
                gate,
                _gate_guard,
            ),
        };

        let pending_revival_lifecycle_persist = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(&session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            if entry.epoch_id != epoch_id
                || !Arc::ptr_eq(&entry.mutation_gate, &registration_gate)
                || !Arc::ptr_eq(&entry.dsl_authority, &dsl_authority)
                || !Arc::ptr_eq(&entry.driver, &driver)
            {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "session {session_id} changed before its exact executor attachment could inherit missing-live persistence"
                    ),
                });
            }
            entry
                .pending_revival_lifecycle_persist
                .load(std::sync::atomic::Ordering::Acquire)
        };
        let attachment_id = RuntimeLoopAttachmentId::new();
        let witness = RuntimeExecutorAttachmentWitness::new(
            Arc::downgrade(&self.shared),
            session_id.clone(),
            epoch_id,
            attachment_id,
        );
        let persist_lifecycle_on_commit =
            staged_registration.revived_stopped_session() || pending_revival_lifecycle_persist;
        let prepublish = async {
            let executor = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                executor_factory(witness.clone())
            }))
            .map_err(|_| {
                RuntimeDriverError::Internal(format!(
                    "executor factory panicked while attaching session {session_id}"
                ))
            })?;
            let machine_managed_post_stop_unregister =
                executor.machine_managed_post_stop_unregister();
            let post_stop_cleanup_handle = if machine_managed_post_stop_unregister {
                Some(executor.post_stop_cleanup_handle().ok_or_else(|| {
                    RuntimeDriverError::ValidationFailed {
                        reason: format!(
                            "machine-managed post-stop unregister for session {session_id} requires a cloneable cleanup handle"
                        ),
                    }
                })?)
            } else {
                None
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
                !driver_guard.as_driver().active_input_ids().is_empty()
            };

            // Wire persistence before startup recovery. Lifecycle revival is
            // deliberately persisted only by the later exact commit: a failed
            // pre-publish attempt must not leave durable Attached truth behind.
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
                let persistence_worker = spawn_ops_lifecycle_persistence_worker(
                    Arc::clone(store),
                    runtime_id,
                    persist_rx,
                )?;
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

            let completion_feed = ops_lifecycle.completion_feed_handle();
            let executor: Box<dyn meerkat_core::lifecycle::CoreExecutor> =
                if machine_managed_post_stop_unregister {
                    Box::new(MachineManagedPostStopExecutor {
                        inner: executor,
                        machine: Arc::downgrade(self),
                        session_id: session_id.clone(),
                        attachment_id,
                        post_stop_fence: MachineManagedPostStopFence::AwaitingStop,
                    })
                } else {
                    executor
                };
            let boundary_handle = executor.boundary_handle();
            let interrupt_handle = executor.interrupt_handle();
            let publication_handle = executor.publication_handle();
            let (wake_tx, wake_rx) = mpsc::channel(16);
            let (effect_tx, effect_rx) = mpsc::channel(16);
            let entry_cursor_state = {
                let sessions = self.sessions.read().await;
                sessions
                    .get(&session_id)
                    .map(|entry| Arc::clone(&entry.cursor_state))
            };
            let mut pending_loop = Some(
                crate::runtime_loop::spawn_runtime_loop_with_completions(
                    driver.clone(),
                    executor,
                    wake_rx,
                    effect_rx,
                    Some(completions.clone()),
                    Some(completion_feed),
                    Some(
                        Arc::clone(&ops_lifecycle)
                            as Arc<dyn meerkat_core::OpsLifecycleRegistry>,
                    ),
                    entry_cursor_state,
                    Arc::downgrade(self),
                    session_id.clone(),
                    turn_finalization_boundary_already_held,
                ),
            );
            // Take both startup capabilities before the next await. Dropping
            // either half closes the loop before it can serve work.
            let startup_authority_transfer = pending_loop
                .as_mut()
                .ok_or_else(|| {
                    RuntimeDriverError::Internal("runtime loop handle missing after spawn".into())
                })?
                .take_startup_authority_transfer()?;
            let serving_release = pending_loop
                .as_mut()
                .ok_or_else(|| {
                    RuntimeDriverError::Internal("runtime loop handle missing after spawn".into())
                })?
                .take_serving_release()?;
            let startup = pending_loop
                .as_ref()
                .map(crate::runtime_loop::SpawnedRuntimeLoop::startup_slot)
                .ok_or_else(|| {
                    RuntimeDriverError::Internal("runtime loop startup handle missing".into())
                })?;
            Ok::<_, RuntimeDriverError>((
                pending_loop,
                startup_authority_transfer,
                serving_release,
                startup,
                wake_tx,
                effect_tx,
                boundary_handle,
                interrupt_handle,
                publication_handle,
                post_stop_cleanup_handle,
                machine_managed_post_stop_unregister,
                should_wake,
            ))
        }
        .await;
        let (
            mut pending_loop,
            startup_authority_transfer,
            serving_release,
            startup,
            wake_tx,
            effect_tx,
            boundary_handle,
            interrupt_handle,
            publication_handle,
            post_stop_cleanup_handle,
            machine_managed_post_stop_unregister,
            should_wake,
        ) = match prepublish {
            Ok(prepublish) => prepublish,
            Err(error) => {
                Self::restore_dsl_authority_snapshot(
                    &dsl_authority,
                    staged_registration.previous_snapshot.clone(),
                );
                driver
                    .lock()
                    .await
                    .sync_control_projection_from_dsl_authority();
                return Err(error);
            }
        };

        let published = {
            let mut sessions = self.sessions.write().await;
            match sessions.get_mut(&session_id) {
                None => false,
                Some(entry) => {
                    entry.clear_dead_attachment();
                    if entry.physical_attachment_is_live() {
                        false
                    } else if !Arc::ptr_eq(&entry.mutation_gate, &registration_gate)
                        || !Arc::ptr_eq(&entry.dsl_authority, &dsl_authority)
                        || !Arc::ptr_eq(&entry.driver, &driver)
                        || !Arc::ptr_eq(&entry.completions, &completions)
                    {
                        tracing::warn!(
                            %session_id,
                            "runtime session entry changed while wiring executor; aborting stale loop attachment"
                        );
                        false
                    } else {
                        match pending_loop.take() {
                            Some(spawned_loop) => {
                                match entry.attach_runtime_loop(
                                    attachment_id,
                                    wake_tx.clone(),
                                    effect_tx,
                                    serving_release,
                                    boundary_handle,
                                    interrupt_handle,
                                    publication_handle,
                                    post_stop_cleanup_handle,
                                    machine_managed_post_stop_unregister,
                                    expected_materialization_claim.as_ref(),
                                    spawned_loop,
                                ) {
                                    Ok(()) => true,
                                    Err(spawned_loop) => {
                                        pending_loop = Some(spawned_loop);
                                        tracing::warn!(
                                            %session_id,
                                            "runtime materialization began aborting before executor attachment published"
                                        );
                                        false
                                    }
                                }
                            }
                            None => {
                                tracing::error!(
                                    %session_id,
                                    "runtime loop handle missing during attachment publish"
                                );
                                false
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
            Self::restore_dsl_authority_snapshot(
                &dsl_authority,
                staged_registration.previous_snapshot,
            );
            let mut driver_guard = driver.lock().await;
            driver_guard.sync_control_projection_from_dsl_authority();
            return Err(RuntimeDriverError::Internal(format!(
                "runtime session {session_id} refused its exact pending attachment"
            )));
        }

        // Publishing the attachment is not readiness. The exact executor must
        // first reconcile the durable compaction projection outbox, including
        // the authoritative empty observation used to abort pre-commit stages.
        // Transfer the already-owned registration guard only after attachment
        // publication. The loop first acquires its turn boundary, then receives
        // this exact M guard and owns both through startup recovery; the caller
        // no longer waits for a task that is itself blocked trying to reacquire
        // the same gate.
        let startup_result = match startup_authority_transfer.send(Some(_gate_guard)) {
            Ok(()) => startup.wait().await,
            Err(error) => Err(error),
        };
        if let Err(startup_error) = startup_result {
            return match self
                .cleanup_unserved_executor_attachment(
                    &witness,
                    turn_finalization_boundary_already_held,
                )
                .await
            {
                Ok(_) => Err(startup_error),
                Err(cleanup_error) => Err(RuntimeDriverError::Internal(format!(
                    "{startup_error}; additionally failed to unregister the failed runtime-loop startup: {cleanup_error}"
                ))),
            };
        }

        let pending_guard = Arc::clone(&registration_gate).lock_owned().await;
        let exact_pending_is_current = {
            let sessions = self.sessions.read().await;
            sessions.get(&session_id).is_some_and(|entry| {
                entry.epoch_id == witness.epoch_id
                    && Arc::ptr_eq(&entry.mutation_gate, &registration_gate)
                    && Arc::ptr_eq(&entry.dsl_authority, &dsl_authority)
                    && Arc::ptr_eq(&entry.driver, &driver)
                    && Arc::ptr_eq(&entry.completions, &completions)
                    && matches!(
                        &entry.attachment_slot,
                        RuntimeLoopAttachmentSlot::Pending(attachment)
                            if attachment.id == witness.attachment_id
                                && !attachment.wake_tx.is_closed()
                                && !attachment.effect_tx.is_closed()
                    )
            })
        };
        if !exact_pending_is_current {
            drop(pending_guard);
            let _ = self
                .cleanup_unserved_executor_attachment(
                    &witness,
                    turn_finalization_boundary_already_held,
                )
                .await;
            return Err(RuntimeDriverError::Internal(format!(
                "runtime session {session_id} lost its exact pending attachment after startup"
            )));
        }

        Ok(EnsureRuntimeExecutorAttachment::Pending(
            PendingRuntimeExecutorAttachment::new(
                Arc::clone(self),
                witness,
                pending_guard,
                cleanup_spawner,
                should_wake,
                persist_lifecycle_on_commit,
            ),
        ))
    }

    /// Retire a candidate that exited before its serving gate opened.
    ///
    /// The runtime-loop watcher deliberately leaves `UnservedAttachment` to
    /// this exact startup owner. When B is already held, complete the
    /// attachment-local actor cleanup non-reentrantly before canonical
    /// unregister; the ordinary path would otherwise wait on the same B.
    async fn cleanup_unserved_executor_attachment(
        self: &Arc<Self>,
        witness: &RuntimeExecutorAttachmentWitness,
        turn_finalization_boundary_already_held: bool,
    ) -> Result<bool, RuntimeDriverError> {
        if !turn_finalization_boundary_already_held {
            return self
                .unregister_executor_attachment_if_current(witness)
                .await;
        }
        self.complete_executor_attachment_cleanup_under_runtime_turn_boundary(witness)
            .await?;
        let Some(guard) = self
            .lock_current_session_mutation_gate(witness.session_id())
            .await
        else {
            return Ok(false);
        };
        self.unregister_executor_attachment_if_current_with_guard(witness.clone(), guard)
            .await
    }

    /// Return the exact committed attachment currently serving this session.
    ///
    /// A generated Active registration or a physically live pending loop is
    /// not sufficient: callers receive a witness only after the surface has
    /// committed the exact attachment transaction.
    pub async fn current_executor_attachment_witness(
        self: &Arc<Self>,
        session_id: &SessionId,
    ) -> Option<RuntimeExecutorAttachmentWitness> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id)?;
        if !entry.generated_executor_registration_active() || !entry.attachment_is_live() {
            return None;
        }
        let RuntimeLoopAttachmentSlot::Attached(attachment) = &entry.attachment_slot else {
            return None;
        };
        Some(RuntimeExecutorAttachmentWitness::new(
            Arc::downgrade(&self.shared),
            session_id.clone(),
            entry.epoch_id.clone(),
            attachment.id,
        ))
    }

    /// Return the exact machine registration currently occupying
    /// `session_id`, independently of whether it has an executor attachment.
    ///
    /// The witness combines durable epoch identity with opaque in-process
    /// entry identity and is suitable only for APIs that revalidate both
    /// against this machine. It is not attachment authority and cannot be used
    /// to publish or clean attachment-local state.
    pub async fn current_session_registration_witness(
        &self,
        session_id: &SessionId,
    ) -> Option<RuntimeSessionRegistrationWitness> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id)?;
        Some(RuntimeSessionRegistrationWitness::new(
            Arc::downgrade(&self.shared),
            session_id.clone(),
            entry.epoch_id.clone(),
            Arc::downgrade(&entry.mutation_gate),
        ))
    }

    /// Unregister only when `witness` still names this machine's exact current
    /// terminal registration and that registration has no executor, retained
    /// attachment cleanup, or actor-materialization owner.
    ///
    /// `Stopped` and `Retired` are the two quiescent lifecycle outcomes used
    /// by runtime disposal. Admission is checked atomically with installation
    /// of the existing machine-owned unregister coordinator while holding the
    /// stable registration transaction and the witness's exact mutation gate.
    /// A stale witness is an idempotent `Ok(false)` and can never remove a
    /// same-SessionId replacement.
    pub async fn unregister_terminal_session_registration_if_current(
        &self,
        witness: &RuntimeSessionRegistrationWitness,
    ) -> Result<bool, RuntimeDriverError> {
        if !witness.belongs_to(self) {
            return Ok(false);
        }
        self.join_or_start_unregister_teardown_with_admission(
            witness.session_id(),
            Some(witness.epoch_id()),
            UnregisterTeardownCaller::Explicit,
            UnregisterTeardownAdmission::ExactTerminalUnattachedRegistration,
            Some(witness),
            UnregisterTeardownWait::CallerGrace,
        )
        .await
    }

    /// Unregister only when `witness` still names this machine's exact current
    /// attachment. A stale attachment is an idempotent `Ok(false)` and can
    /// never open the drain window for a same-SessionId replacement.
    pub async fn unregister_executor_attachment_if_current(
        self: &Arc<Self>,
        witness: &RuntimeExecutorAttachmentWitness,
    ) -> Result<bool, RuntimeDriverError> {
        if !witness.belongs_to(self) {
            return Ok(false);
        }
        let cleanup_spawner = super::MachineCleanupTaskSpawner::acquire()?;
        let Some(guard) = self
            .lock_current_session_mutation_gate(witness.session_id())
            .await
        else {
            return Ok(false);
        };
        self.spawn_executor_attachment_retirement_with_guard(
            witness.clone(),
            guard,
            cleanup_spawner,
        )
        .wait()
        .await
    }

    /// Complete the exact attachment's service cleanup while the caller owns
    /// the stable runtime-turn finalization boundary.
    ///
    /// This is the non-reentrant half of a boundary-owned attachment abort.
    /// After it returns, ordinary exact unregister observes the cleanup as
    /// complete and never attempts to reacquire the caller's boundary.
    pub async fn complete_executor_attachment_cleanup_under_runtime_turn_boundary(
        self: &Arc<Self>,
        witness: &RuntimeExecutorAttachmentWitness,
    ) -> Result<(), RuntimeDriverError> {
        if !witness.belongs_to(self) {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "attachment cleanup witness belongs to another machine".to_string(),
            });
        }
        let exact_current = {
            let sessions = self.sessions.read().await;
            sessions.get(witness.session_id()).is_some_and(|entry| {
                entry.epoch_id == witness.epoch_id
                    && (entry.owns_runtime_loop_attachment(witness.attachment_id)
                        || entry.post_stop_cleanup_attachment_id == Some(witness.attachment_id))
            })
        };
        if !exact_current {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "attachment for session {} changed before boundary-owned cleanup",
                    witness.session_id()
                ),
            });
        }
        self.complete_post_stop_cleanup_if_needed(witness.session_id(), witness.attachment_id, true)
            .await
    }

    /// Acquire exact machine mutation authority for a surface-owned actor and
    /// executor retirement transaction.
    ///
    /// The caller must already hold the service turn-finalization boundary.
    /// Returning `Some` proves that `witness` still names the exact committed,
    /// serving attachment while the returned lease retains the machine gate.
    /// Stale, absent, pending, or already-draining attachments are a no-op.
    pub async fn prepare_executor_attachment_retirement_under_runtime_turn_boundary(
        self: &Arc<Self>,
        witness: &RuntimeExecutorAttachmentWitness,
    ) -> Result<Option<super::PreparedRuntimeExecutorAttachmentRetirement>, RuntimeDriverError>
    {
        if !witness.belongs_to(self) {
            return Ok(None);
        }
        let cleanup_spawner = super::MachineCleanupTaskSpawner::acquire()?;
        let Some(mutation_guard) = self
            .lock_current_session_mutation_gate(witness.session_id())
            .await
        else {
            return Ok(None);
        };
        let exact_serving_attachment = {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(witness.session_id()) else {
                drop(mutation_guard);
                return Ok(None);
            };
            let registration_phase = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .state()
                .registration_phase;
            entry.epoch_id == witness.epoch_id
                && entry.generated_executor_registration_active()
                && registration_phase != dsl::RegistrationPhase::Draining
                && matches!(
                    &entry.attachment_slot,
                    RuntimeLoopAttachmentSlot::Attached(attachment)
                        if attachment.id == witness.attachment_id
                            && !attachment.wake_tx.is_closed()
                            && !attachment.effect_tx.is_closed()
                )
        };
        if !exact_serving_attachment {
            drop(mutation_guard);
            return Ok(None);
        }
        Ok(Some(
            super::PreparedRuntimeExecutorAttachmentRetirement::new(
                Arc::clone(self),
                witness.clone(),
                mutation_guard,
                cleanup_spawner,
            ),
        ))
    }

    pub(super) fn spawn_executor_attachment_retirement_with_guard(
        self: &Arc<Self>,
        witness: RuntimeExecutorAttachmentWitness,
        mutation_guard: crate::tokio::sync::OwnedMutexGuard<()>,
        cleanup_spawner: super::MachineCleanupTaskSpawner,
    ) -> super::RuntimeExecutorAttachmentRetirementCompletion {
        let machine = Arc::clone(self);
        let task_witness = witness;
        let (result_tx, result_rx) = crate::tokio::sync::oneshot::channel();
        cleanup_spawner.spawn(async move {
            let result = machine
                .unregister_executor_attachment_if_current_with_guard(
                    task_witness.clone(),
                    mutation_guard,
                )
                .await;
            if let Err(error) = &result {
                tracing::warn!(
                    session_id = %task_witness.session_id(),
                    %error,
                    "owned exact runtime attachment retirement failed"
                );
            }
            let _ = result_tx.send(result);
        });
        super::RuntimeExecutorAttachmentRetirementCompletion::new(result_rx)
    }

    pub(super) async fn commit_pending_executor_attachment<F>(
        self: &Arc<Self>,
        witness: &RuntimeExecutorAttachmentWitness,
        _mutation_guard: &crate::tokio::sync::OwnedMutexGuard<()>,
        should_wake: bool,
        persist_lifecycle_on_commit: bool,
        retain_for_publication: bool,
        on_committed: F,
    ) -> Result<RuntimeExecutorAttachmentWitness, RuntimeDriverError>
    where
        F: FnOnce(&RuntimeExecutorAttachmentWitness) -> Result<(), RuntimeDriverError>,
    {
        async {
            if !witness.belongs_to(self) {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: "pending executor attachment belongs to another machine".to_string(),
                });
            }
            if persist_lifecycle_on_commit && !retain_for_publication {
                let driver = {
                    let sessions = self.sessions.read().await;
                    let entry = sessions.get(witness.session_id()).ok_or(
                        RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        },
                    )?;
                    let exact_pending = entry.epoch_id == witness.epoch_id
                        && matches!(
                            &entry.attachment_slot,
                            RuntimeLoopAttachmentSlot::Pending(attachment)
                                if attachment.id == witness.attachment_id
                                    && !attachment.wake_tx.is_closed()
                                    && !attachment.effect_tx.is_closed()
                        );
                    if !exact_pending {
                        return Err(RuntimeDriverError::StaleAuthority {
                            reason: format!(
                                "pending executor attachment for session {} changed before lifecycle commit",
                                witness.session_id()
                            ),
                        });
                    }
                    Arc::clone(&entry.driver)
                };
                // Persist revival only at the exact attachment commit boundary.
                // Until this succeeds the durable Stopped observation remains
                // truthful and abort can remove the provisional attachment.
                let mut driver_guard = driver.lock().await;
                driver_guard
                    .persist_current_machine_lifecycle("resume")
                    .await?;
            }
            let wake_tx = {
                let mut sessions = self.sessions.write().await;
                let entry = sessions.get_mut(witness.session_id()).ok_or(
                    RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    },
                )?;
                if entry.epoch_id != witness.epoch_id {
                    return Err(RuntimeDriverError::StaleAuthority {
                        reason: format!(
                            "pending executor attachment for session {} belongs to a stale runtime epoch",
                            witness.session_id()
                        ),
                    });
                }
                if !entry.generated_executor_registration_active() {
                    return Err(RuntimeDriverError::ValidationFailed {
                        reason: format!(
                            "pending executor attachment for session {} lost its generated registration",
                            witness.session_id()
                        ),
                    });
                }
                let exact_pending = matches!(
                    &entry.attachment_slot,
                    RuntimeLoopAttachmentSlot::Pending(attachment)
                        if attachment.id == witness.attachment_id
                            && !attachment.wake_tx.is_closed()
                            && !attachment.effect_tx.is_closed()
                );
                if !exact_pending {
                    return Err(RuntimeDriverError::StaleAuthority {
                        reason: format!(
                            "pending executor attachment for session {} is no longer current",
                            witness.session_id()
                        ),
                    });
                }
                if retain_for_publication {
                    // Keep both the mechanical slot and the runtime-loop
                    // serving release pending. The exact M guard remains in
                    // the returned publication lease, whose final commit
                    // publishes all surface-owned state before releasing M.
                    return Ok(witness.clone());
                }
                let mut attachment = match std::mem::replace(
                    &mut entry.attachment_slot,
                    RuntimeLoopAttachmentSlot::Empty,
                ) {
                    RuntimeLoopAttachmentSlot::Pending(attachment) => attachment,
                    other => {
                        entry.attachment_slot = other;
                        return Err(RuntimeDriverError::StaleAuthority {
                            reason: format!(
                                "pending executor attachment for session {} changed before commit",
                                witness.session_id()
                            ),
                        });
                    }
                };
                let wake_tx = attachment.wake_tx.clone();
                let Some(serving_release) = attachment.serving_release.take() else {
                    entry.attachment_slot = RuntimeLoopAttachmentSlot::Pending(attachment);
                    return Err(RuntimeDriverError::Internal(format!(
                        "pending executor attachment for session {} lost its serving release",
                        witness.session_id()
                    )));
                };
                let publication = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    on_committed(witness)
                }))
                .map_err(|_| {
                    RuntimeDriverError::Internal(format!(
                        "surface activation panicked while committing attachment for session {}",
                        witness.session_id()
                    ))
                })
                .and_then(std::convert::identity);
                if let Err(error) = publication {
                    attachment.serving_release = Some(serving_release);
                    entry.attachment_slot = RuntimeLoopAttachmentSlot::Pending(attachment);
                    return Err(error);
                }
                if let Err(error) = serving_release.release() {
                    // Callback publications are required to be reversible
                    // until this returns. The caller still owns M and unwinds
                    // those staged values before exact abort.
                    entry.attachment_slot = RuntimeLoopAttachmentSlot::Pending(attachment);
                    return Err(error);
                }
                entry.attachment_slot = RuntimeLoopAttachmentSlot::Attached(attachment);
                if persist_lifecycle_on_commit {
                    entry
                        .pending_revival_lifecycle_persist
                        .store(false, std::sync::atomic::Ordering::Release);
                }
                wake_tx
            };
            if should_wake {
                let _ = wake_tx.try_send(());
            }
            Ok(witness.clone())
        }
        .await
    }

    /// Finish an exact retained attachment publication while the caller still
    /// owns the same session mutation gate used to prepare the attachment.
    ///
    /// The runtime-loop serving token is released only here. Because M remains
    /// held through the synchronous publication callback, the loop cannot
    /// mutate session state between serving release and publication of the
    /// attachment-bound sidecars.
    pub(super) async fn commit_retained_executor_attachment_publication<F>(
        self: &Arc<Self>,
        witness: &RuntimeExecutorAttachmentWitness,
        _mutation_guard: &crate::tokio::sync::OwnedMutexGuard<()>,
        should_wake: bool,
        persist_lifecycle_on_commit: bool,
        on_committed: F,
    ) -> Result<RuntimeExecutorAttachmentWitness, RuntimeDriverError>
    where
        F: FnOnce(
            &RuntimeExecutorAttachmentWitness,
            Arc<crate::tokio::sync::Mutex<()>>,
        ) -> Result<(), RuntimeDriverError>,
    {
        if !witness.belongs_to(self) {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "retained executor attachment belongs to another machine".to_string(),
            });
        }
        if persist_lifecycle_on_commit {
            let driver = {
                let sessions = self.sessions.read().await;
                let entry =
                    sessions
                        .get(witness.session_id())
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?;
                let exact_pending = entry.epoch_id == witness.epoch_id
                    && matches!(
                        &entry.attachment_slot,
                        RuntimeLoopAttachmentSlot::Pending(attachment)
                            if attachment.id == witness.attachment_id
                                && !attachment.wake_tx.is_closed()
                                && !attachment.effect_tx.is_closed()
                                && attachment.serving_release.is_some()
                    );
                if !exact_pending {
                    return Err(RuntimeDriverError::StaleAuthority {
                        reason: format!(
                            "retained executor attachment for session {} changed before lifecycle publication",
                            witness.session_id()
                        ),
                    });
                }
                Arc::clone(&entry.driver)
            };
            let mut driver_guard = driver.lock().await;
            driver_guard
                .persist_current_machine_lifecycle("resume")
                .await?;
        }
        let wake_tx = {
            let mut sessions = self.sessions.write().await;
            let entry =
                sessions
                    .get_mut(witness.session_id())
                    .ok_or(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    })?;
            if entry.epoch_id != witness.epoch_id {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "retained executor attachment for session {} belongs to a stale runtime epoch",
                        witness.session_id()
                    ),
                });
            }
            if !entry.generated_executor_registration_active() {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: format!(
                        "retained executor attachment for session {} lost its generated registration",
                        witness.session_id()
                    ),
                });
            }
            let exact_pending = matches!(
                &entry.attachment_slot,
                RuntimeLoopAttachmentSlot::Pending(attachment)
                    if attachment.id == witness.attachment_id
                        && !attachment.wake_tx.is_closed()
                        && !attachment.effect_tx.is_closed()
                        && attachment.serving_release.is_some()
            );
            if !exact_pending {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "retained executor attachment for session {} is no longer the exact non-serving candidate",
                        witness.session_id()
                    ),
                });
            }
            let session_mutation_gate = Arc::clone(&entry.mutation_gate);
            let mut attachment = match std::mem::replace(
                &mut entry.attachment_slot,
                RuntimeLoopAttachmentSlot::Empty,
            ) {
                RuntimeLoopAttachmentSlot::Pending(attachment) => attachment,
                other => {
                    entry.attachment_slot = other;
                    return Err(RuntimeDriverError::StaleAuthority {
                        reason: format!(
                            "retained executor attachment for session {} changed before final publication",
                            witness.session_id()
                        ),
                    });
                }
            };
            let wake_tx = attachment.wake_tx.clone();
            let Some(serving_release) = attachment.serving_release.take() else {
                entry.attachment_slot = RuntimeLoopAttachmentSlot::Pending(attachment);
                return Err(RuntimeDriverError::Internal(format!(
                    "retained executor attachment for session {} lost its serving release",
                    witness.session_id()
                )));
            };
            let publication = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                on_committed(witness, session_mutation_gate)
            }))
            .map_err(|_| {
                RuntimeDriverError::Internal(format!(
                    "surface publication panicked for retained attachment {}",
                    witness.session_id()
                ))
            })
            .and_then(std::convert::identity);
            if let Err(error) = publication {
                // Publication failed before the serving token was consumed.
                // Restore the token and exact pending slot so the caller's
                // retained M lease can abort or retry without ever exposing a
                // serving runtime.
                attachment.serving_release = Some(serving_release);
                entry.attachment_slot = RuntimeLoopAttachmentSlot::Pending(attachment);
                return Err(error);
            }
            if let Err(error) = serving_release.release() {
                // The receiver is already gone, so the candidate cannot ever
                // serve. Keep it Pending for exact abort. Reversible surface
                // and residency publications unwind synchronously while the
                // caller still owns M.
                entry.attachment_slot = RuntimeLoopAttachmentSlot::Pending(attachment);
                return Err(error);
            }
            entry.attachment_slot = RuntimeLoopAttachmentSlot::Attached(attachment);
            if persist_lifecycle_on_commit {
                entry
                    .pending_revival_lifecycle_persist
                    .store(false, std::sync::atomic::Ordering::Release);
            }
            wake_tx
        };
        if should_wake {
            let _ = wake_tx.try_send(());
        }
        Ok(witness.clone())
    }

    pub(super) async fn abort_pending_executor_attachment(
        self: &Arc<Self>,
        witness: RuntimeExecutorAttachmentWitness,
        mutation_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<(), RuntimeDriverError> {
        self.unregister_executor_attachment_if_current_with_guard(witness, mutation_guard)
            .await
            .map(|_| ())
    }

    pub(super) async fn unregister_executor_attachment_if_current_with_guard(
        self: &Arc<Self>,
        witness: RuntimeExecutorAttachmentWitness,
        mutation_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<bool, RuntimeDriverError> {
        if !witness.belongs_to(self) {
            drop(mutation_guard);
            return Ok(false);
        }
        let registration_phase = {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(witness.session_id()) else {
                drop(mutation_guard);
                return Ok(false);
            };
            if entry.epoch_id != witness.epoch_id {
                drop(mutation_guard);
                return Ok(false);
            }
            let registration_phase = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .state()
                .registration_phase;
            let exact_attachment_is_current = entry
                .owns_runtime_loop_attachment(witness.attachment_id)
                || (registration_phase == dsl::RegistrationPhase::Draining
                    && entry.post_stop_cleanup_attachment_id == Some(witness.attachment_id));
            if !exact_attachment_is_current {
                drop(mutation_guard);
                return Ok(false);
            }
            registration_phase
        };

        if registration_phase != dsl::RegistrationPhase::Draining {
            let staged = self
                .stage_begin_unregister_session_authority(witness.session_id())
                .await
                .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
            self.commit_session_dsl_transition(
                witness.session_id(),
                staged,
                "BeginUnregisterSession(exact attachment abort)",
            )
            .await
            .map_err(RuntimeDriverError::Internal)?;
        }
        drop(mutation_guard);
        self.join_or_start_unregister_teardown_with_admission(
            witness.session_id(),
            Some(witness.epoch_id()),
            UnregisterTeardownCaller::Explicit,
            UnregisterTeardownAdmission::AnyCurrentRegistration,
            None,
            UnregisterTeardownWait::UntilTerminal,
        )
        .await?;
        Ok(true)
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
        let observed_teardown = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).and_then(|entry| {
                entry
                    .runtime_loop_teardown
                    .as_ref()
                    .map(|slot| (entry.epoch_id.clone(), Arc::clone(slot)))
            })
        };
        let stop_result = self
            .join_or_start_runtime_stop_cleanup(
                session_id,
                RuntimeStopCleanupCaller::ExplicitStop,
                Some(reason),
                None,
            )
            .await;
        if matches!(
            &stop_result,
            Err(RuntimeDriverError::RuntimeStopInProgress { .. })
        ) && self.session_dsl_state(session_id).await.is_ok_and(|state| {
            state.registration_phase == crate::meerkat_machine::dsl::RegistrationPhase::Draining
        }) {
            // The post-M phase check in coordinator installation may have
            // observed a newer Draining transition than our optimistic sample
            // above. Join its canonical unregister owner; a genuinely
            // mid-apply ordinary stop remains Queuing and keeps the typed
            // RuntimeStopInProgress result from the stop coordinator.
            return match self.unregister_session_inner(session_id).await {
                Err(RuntimeDriverError::UnregisterInProgress { .. }) => {
                    Err(RuntimeDriverError::RuntimeStopInProgress {
                        runtime_id: LogicalRuntimeId::for_session(session_id),
                    })
                }
                result => result,
            };
        }
        stop_result?;
        // A failed machine-managed cleanup leaves the exact executor in its
        // teardown slot for an explicit retry. Re-check the exact epoch+slot
        // afterward in case a concurrent explicit unregister opened Draining
        // while cleanup was in flight; never touch a replacement attachment
        // that may have reused the SessionId.
        if let Some((observed_epoch, observed_teardown_slot)) = observed_teardown {
            return match self
                .join_unregister_if_observed_runtime_loop_became_draining(
                    session_id,
                    &observed_epoch,
                    &observed_teardown_slot,
                )
                .await
            {
                Err(RuntimeDriverError::UnregisterInProgress { .. }) => {
                    Err(RuntimeDriverError::RuntimeStopInProgress {
                        runtime_id: LogicalRuntimeId::for_session(session_id),
                    })
                }
                result => result,
            };
        }
        Ok(())
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
        if disposition.requires_unregister() {
            // A teardown slot is attachment-local while the runtime epoch is
            // registration-local.  A dead A can be repaired by B without
            // changing epochs, so an unlocked slot sample followed by an
            // epoch-only unregister would let A tear down B.  Acquire M first,
            // revalidate the exact slot under it, and retain M until Draining
            // is committed.  Once Draining is visible no replacement can enter
            // before the owned unregister saga takes over.
            let Some(observed_epoch) = self
                .begin_unregister_for_observed_runtime_loop_if_current(
                    session_id,
                    &observed_teardown_slot,
                )
                .await?
            else {
                return Ok(());
            };
            return self
                .join_or_start_unregister_teardown(
                    session_id,
                    Some(&observed_epoch),
                    UnregisterTeardownCaller::RuntimeLoopWatcher,
                )
                .await;
        }

        let (generated_draining, observed_epoch, preinstalled_stop_coordinator, driver) = {
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
            let generated_draining = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .state()
                .registration_phase
                == crate::meerkat_machine::dsl::RegistrationPhase::Draining;
            (
                generated_draining,
                entry.epoch_id.clone(),
                entry.runtime_stop_cleanup_coordinator.is_some(),
                Arc::clone(&entry.driver),
            )
        };
        if generated_draining {
            return self
                .join_or_start_unregister_teardown(
                    session_id,
                    Some(&observed_epoch),
                    UnregisterTeardownCaller::RuntimeLoopWatcher,
                )
                .await;
        }
        if preinstalled_stop_coordinator {
            // An explicit stop already owns the typed result channel. Join it
            // rather than creating a second result owner.
            self.join_or_start_runtime_stop_cleanup(
                session_id,
                RuntimeStopCleanupCaller::RuntimeLoopWatcher,
                None,
                Some(Arc::clone(&observed_teardown_slot)),
            )
            .await?;
        } else {
            // A spontaneous loop exit can hand off a machine-managed executor
            // that still retains M from its stop hook. Installing a coordinator
            // first also takes M and therefore deadlocks before cleanup_once can
            // consume that exact fence. The teardown slot is already the
            // single mechanical executor owner: claim its idempotent cleanup
            // directly, publish the same acknowledgement, then re-check the
            // generated Draining verdict below. A racing explicit coordinator
            // can only join this slot and observes the same Cleaned result.
            let cleanup_result = observed_teardown_slot.cleanup_once(&driver).await;
            observed_teardown_slot.acknowledge_runtime_stop_result(cleanup_result.clone());
            cleanup_result?;
        }
        self.join_unregister_if_observed_runtime_loop_became_draining(
            session_id,
            &observed_epoch,
            &observed_teardown_slot,
        )
        .await
    }

    /// Begin unregister only while `observed_teardown_slot` is still the exact
    /// current runtime-loop incarnation for this session.
    ///
    /// The mutation guard is deliberately retained across the slot check and
    /// the generated Draining commit.  Epoch identity alone is insufficient:
    /// repairing a dead attachment preserves the registration epoch.
    async fn begin_unregister_for_observed_runtime_loop_if_current(
        &self,
        session_id: &SessionId,
        observed_teardown_slot: &Arc<crate::runtime_loop::RuntimeLoopTeardownSlot>,
    ) -> Result<Option<meerkat_core::RuntimeEpochId>, RuntimeDriverError> {
        let Some(mutation_guard) = self.lock_current_session_mutation_gate(session_id).await else {
            return Ok(None);
        };
        let (observed_epoch, registration_phase) = {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                drop(mutation_guard);
                return Ok(None);
            };
            let exact_slot_is_current = entry
                .runtime_loop_teardown
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, observed_teardown_slot));
            if !exact_slot_is_current {
                drop(mutation_guard);
                return Ok(None);
            }
            let registration_phase = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .state()
                .registration_phase;
            (entry.epoch_id.clone(), registration_phase)
        };

        if registration_phase != dsl::RegistrationPhase::Draining {
            let staged = self
                .stage_begin_unregister_session_authority(session_id)
                .await
                .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
            self.commit_session_dsl_transition(
                session_id,
                staged,
                "BeginUnregisterSession(exact runtime-loop handoff)",
            )
            .await
            .map_err(RuntimeDriverError::Internal)?;
        }
        drop(mutation_guard);
        Ok(Some(observed_epoch))
    }

    /// A machine-managed executor can publish its loop handoff while generated
    /// registration is still Queuing, while a concurrent explicit unregister
    /// opens Draining against that same exact slot. Re-check after ordinary
    /// stop cleanup so the newer unregister owner cannot be stranded after the
    /// watcher made its first disposition sample.
    pub(super) async fn join_unregister_if_observed_runtime_loop_became_draining(
        &self,
        session_id: &SessionId,
        observed_epoch: &meerkat_core::RuntimeEpochId,
        observed_teardown_slot: &Arc<crate::runtime_loop::RuntimeLoopTeardownSlot>,
    ) -> Result<(), RuntimeDriverError> {
        let became_draining = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).is_some_and(|entry| {
                &entry.epoch_id == observed_epoch
                    && entry
                        .runtime_loop_teardown
                        .as_ref()
                        .is_some_and(|current| Arc::ptr_eq(current, observed_teardown_slot))
                    && entry
                        .dsl_authority
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .state()
                        .registration_phase
                        == crate::meerkat_machine::dsl::RegistrationPhase::Draining
            })
        };
        if !became_draining {
            return Ok(());
        }
        self.join_or_start_unregister_teardown(
            session_id,
            Some(observed_epoch),
            UnregisterTeardownCaller::RuntimeLoopWatcher,
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
                work: RuntimeStopCleanupWork,
            },
            Completed(RuntimeStopCleanupResult),
            GeneratedDraining,
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
            let generated_draining = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .state()
                .registration_phase
                == crate::meerkat_machine::dsl::RegistrationPhase::Draining;
            if caller == RuntimeStopCleanupCaller::ExplicitStop && generated_draining {
                // The optimistic sample in request_runtime_stop can become
                // stale while this caller waits for M behind a terminal loop
                // handoff. Draining is the newer generated authority: never
                // install or dispatch an ordinary StopRuntimeExecutor request
                // against it. Drop M below and join the canonical unregister
                // owner instead.
                CoordinatorDecision::GeneratedDraining
            } else {
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
                                let (result_tx, result_rx) =
                                    crate::tokio::sync::watch::channel(None);
                                entry.runtime_stop_cleanup_coordinator =
                                    Some(RuntimeStopCleanupCoordinator {
                                        epoch_id: epoch_id.clone(),
                                        coordinator_id,
                                        teardown_slot: entry.runtime_loop_teardown.clone(),
                                        result_rx: result_rx.clone(),
                                    });
                                CoordinatorDecision::Start {
                                    epoch_id,
                                    coordinator_id,
                                    result_tx,
                                    result_rx,
                                    teardown_slot: entry.runtime_loop_teardown.clone(),
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
                        entry.runtime_stop_cleanup_coordinator =
                            Some(RuntimeStopCleanupCoordinator {
                                epoch_id: epoch_id.clone(),
                                coordinator_id,
                                teardown_slot: entry.runtime_loop_teardown.clone(),
                                result_rx: result_rx.clone(),
                            });
                        CoordinatorDecision::Start {
                            epoch_id,
                            coordinator_id,
                            result_tx,
                            result_rx,
                            teardown_slot: entry.runtime_loop_teardown.clone(),
                            work: match initial_reason {
                                Some(reason) => RuntimeStopCleanupWork::Request { reason },
                                None => RuntimeStopCleanupWork::CleanupOnly,
                            },
                        }
                    }
                }
            }
        };
        drop(coordinator_gate);

        let mut result_rx = match decision {
            CoordinatorDecision::GeneratedDraining => {
                return Err(RuntimeDriverError::RuntimeStopInProgress {
                    runtime_id: LogicalRuntimeId::for_session(session_id),
                });
            }
            CoordinatorDecision::Join(result_rx) => result_rx,
            CoordinatorDecision::Completed(result) => return result,
            CoordinatorDecision::Start {
                epoch_id,
                coordinator_id,
                result_tx,
                result_rx,
                teardown_slot,
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

    #[cfg(test)]
    pub(crate) async fn join_explicit_runtime_stop_after_phase_sample_for_test(
        &self,
        session_id: &SessionId,
        reason: String,
    ) -> Result<(), RuntimeDriverError> {
        self.join_or_start_runtime_stop_cleanup(
            session_id,
            RuntimeStopCleanupCaller::ExplicitStop,
            Some(reason),
            None,
        )
        .await
    }

    async fn run_owned_runtime_stop_cleanup(
        &self,
        session_id: &SessionId,
        epoch_id: &meerkat_core::RuntimeEpochId,
        teardown_slot: Option<Arc<crate::runtime_loop::RuntimeLoopTeardownSlot>>,
        work: RuntimeStopCleanupWork,
    ) -> Result<(), RuntimeDriverError> {
        let stop_completion = match work {
            RuntimeStopCleanupWork::Request { reason } => {
                self.dispatch_owned_runtime_stop_request(session_id, epoch_id, reason)
                    .await?
            }
            RuntimeStopCleanupWork::CleanupOnly => None,
        };

        let (driver, completions, publication_handle) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .filter(|entry| &entry.epoch_id == epoch_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            (
                Arc::clone(&entry.driver),
                Arc::clone(&entry.completions),
                entry.publication_handle(),
            )
        };

        let cleanup_result = match teardown_slot.as_ref() {
            Some(teardown_slot) => {
                teardown_slot.wait_until_published().await;
                teardown_slot.cleanup_once(&driver).await
            }
            None => crate::control_plane::terminalize_async_stop(
                &driver,
                Some(&completions),
                publication_handle,
                None,
            )
            .await
            .map(|_guard| ()),
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
        self.join_or_start_unregister_teardown_with_admission(
            session_id,
            expected_epoch,
            caller,
            UnregisterTeardownAdmission::AnyCurrentRegistration,
            None,
            UnregisterTeardownWait::CallerGrace,
        )
        .await
        .map(|_| ())
    }

    fn require_terminal_unattached_registration(
        session_id: &SessionId,
        entry: &RuntimeSessionEntry,
    ) -> Result<(), RuntimeDriverError> {
        // Generated lifecycle authority is the terminality owner. The coarse
        // control projection can legitimately remain Idle while recovery's
        // terminal-precedence arbitration publishes generated Stopped (the
        // exact ops-only cold-resume shape this cleanup serves).
        let runtime_state = {
            let authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            super::dsl_authority::runtime_phase_from_authority(&authority)
        };
        if !matches!(runtime_state, RuntimeState::Stopped | RuntimeState::Retired) {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "exact registration cleanup for session {session_id} requires Stopped or Retired runtime authority, found {runtime_state:?}"
                ),
            });
        }
        let materialization_vacant = {
            let claim = entry
                .materialization_claim_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            claim.current.is_none()
                && claim.phase == crate::RuntimeActorMaterializationClaimPhase::Vacant
                && !claim.rollback_registration_available
        };
        // An ordinary Stop retains its completed coordinator as an idempotent
        // result cache. That is terminal shell truth, not an active cleanup
        // owner; canonical unregister will join the completed result. A
        // pending or failed stop still rejects exact registration disposal.
        let runtime_stop_cleanup_quiescent = match &entry.runtime_stop_cleanup_coordinator {
            None => true,
            Some(coordinator) => {
                coordinator.teardown_slot.is_none()
                    && coordinator
                        .result_rx
                        .borrow()
                        .as_ref()
                        .is_some_and(Result::is_ok)
            }
        };
        let attachment_local_authority_absent =
            matches!(entry.attachment_slot, RuntimeLoopAttachmentSlot::Empty)
                && entry.runtime_loop_teardown.is_none()
                && runtime_stop_cleanup_quiescent
                && entry.post_stop_cleanup_handle.is_none()
                && entry.post_stop_cleanup_attachment_id.is_none()
                && entry.provisional_interrupt_handle.is_none()
                && entry.provisional_materialization_claim_id.is_none()
                && entry.publication_handle.is_none()
                && materialization_vacant;
        if !attachment_local_authority_absent {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "exact terminal registration cleanup for session {session_id} requires an unattached registration with no retained attachment or materialization authority"
                ),
            });
        }
        Ok(())
    }

    async fn join_or_start_unregister_teardown_with_admission(
        &self,
        session_id: &SessionId,
        expected_epoch: Option<&meerkat_core::RuntimeEpochId>,
        caller: UnregisterTeardownCaller,
        admission: UnregisterTeardownAdmission,
        expected_registration: Option<&RuntimeSessionRegistrationWitness>,
        wait: UnregisterTeardownWait,
    ) -> Result<bool, RuntimeDriverError> {
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
            RegistrationChanged,
            Completed(Result<(), RuntimeDriverError>),
            Retry,
        }

        // Joiners may observe an already-installed coordinator without taking
        // its registration transaction gate. Every other decision is made
        // while holding the stable gate, so an absent observation cannot race
        // a cold registrar that has selected an epoch but not published its
        // entry yet. The worker releases this first section after validating
        // its pending-finalization branch, then reacquires a second section
        // after surface cleanup through exact durable removal.
        let (decision, mut registration_transaction_guard, exact_mutation_guard) = loop {
            let installed_coordinator = {
                let sessions = self.sessions.read().await;
                admission == UnregisterTeardownAdmission::AnyCurrentRegistration
                    && sessions.get(session_id).is_some_and(|entry| {
                        !expected_epoch.is_some_and(|expected| expected != &entry.epoch_id)
                            && entry.unregister_coordinator.is_some()
                    })
            };
            let registration_transaction_guard = if installed_coordinator {
                None
            } else {
                Some(self.lock_session_registration_transaction(session_id).await)
            };
            let exact_mutation_guard =
                if admission == UnregisterTeardownAdmission::ExactTerminalUnattachedRegistration {
                    match expected_registration
                        .and_then(RuntimeSessionRegistrationWitness::registration_gate)
                    {
                        Some(gate) => Some(gate.lock_owned().await),
                        None => None,
                    }
                } else {
                    None
                };
            let decision = {
                let mut sessions = self.sessions.write().await;
                match sessions.get_mut(session_id) {
                    None => CoordinatorDecision::AlreadyAbsent,
                    Some(entry)
                        if expected_epoch.is_some_and(|expected| expected != &entry.epoch_id) =>
                    {
                        CoordinatorDecision::EpochChanged
                    }
                    Some(entry)
                        if admission
                            == UnregisterTeardownAdmission::ExactTerminalUnattachedRegistration
                            && !expected_registration
                                .is_some_and(|witness| witness.matches_entry(entry)) =>
                    {
                        CoordinatorDecision::RegistrationChanged
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
                        } else if registration_transaction_guard.is_none() {
                            // The optimistic read saw a coordinator, but it
                            // completed or the entry was replaced before this
                            // authoritative read. Without the stable registration
                            // transaction guard this pass is observation-only: it
                            // must not install a successor coordinator on the
                            // changed entry.
                            CoordinatorDecision::Retry
                        } else {
                            if admission
                                == UnregisterTeardownAdmission::ExactTerminalUnattachedRegistration
                            {
                                Self::require_terminal_unattached_registration(session_id, entry)?;
                            }
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
            if registration_transaction_guard.is_none()
                && !matches!(
                    &decision,
                    CoordinatorDecision::Join(_) | CoordinatorDecision::Completed(_)
                )
            {
                // The coordinator seen by the optimistic join check completed
                // or changed before the authoritative decision. Retry and take
                // the gate before observing absence or installing a successor.
                continue;
            }
            break (
                decision,
                registration_transaction_guard,
                exact_mutation_guard,
            );
        };

        // Exact admission retains T + the witness's exact M through pointer
        // comparison, quiescence validation, and coordinator installation.
        // The owned unregister worker reacquires M through its own exact entry
        // incarnation, so release this admission fence before joining/spawning.
        drop(exact_mutation_guard);

        let mut result_rx = match decision {
            CoordinatorDecision::Join(result_rx) => {
                drop(registration_transaction_guard.take());
                result_rx
            }
            CoordinatorDecision::AlreadyAbsent
            | CoordinatorDecision::EpochChanged
            | CoordinatorDecision::RegistrationChanged => {
                drop(registration_transaction_guard.take());
                return Ok(false);
            }
            CoordinatorDecision::Completed(result) => {
                drop(registration_transaction_guard.take());
                return result.map(|()| true);
            }
            CoordinatorDecision::Retry => {
                drop(registration_transaction_guard.take());
                return Err(RuntimeDriverError::Internal(format!(
                    "unregister coordinator retry escaped its decision loop for session {session_id}"
                )));
            }
            CoordinatorDecision::Start {
                epoch_id,
                coordinator_id,
                result_tx,
                result_rx,
                teardown_slot,
            } => {
                let registration_transaction_guard = registration_transaction_guard
                    .take()
                    .ok_or_else(|| {
                        RuntimeDriverError::Internal(format!(
                            "unregister coordinator for session {session_id} started without its registration transaction gate"
                        ))
                    })?;
                let saga_machine = self.clone();
                let saga_session_id = session_id.clone();
                let saga_epoch_id = epoch_id.clone();
                let worker = crate::tokio::spawn(unregister_coordinator_poll_scope(
                    coordinator_id,
                    async move {
                        saga_machine
                            .run_owned_unregister_teardown(
                                &saga_session_id,
                                &saga_epoch_id,
                                coordinator_id,
                                registration_transaction_guard,
                            )
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
        match wait {
            UnregisterTeardownWait::UntilTerminal => wait_for_owned_result.await.map(|()| true),
            UnregisterTeardownWait::CallerGrace => {
                match crate::tokio::time::timeout(
                    UNREGISTER_CALLER_WAIT_GRACE,
                    wait_for_owned_result,
                )
                .await
                {
                    Ok(result) => result.map(|()| true),
                    Err(_elapsed) => Err(RuntimeDriverError::UnregisterInProgress {
                        runtime_id: LogicalRuntimeId::for_session(session_id),
                    }),
                }
            }
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
        coordinator_id: uuid::Uuid,
        registration_transaction_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<(), RuntimeDriverError> {
        let result = self
            .run_owned_unregister_teardown_inner(
                session_id,
                epoch_id,
                coordinator_id,
                registration_transaction_guard,
            )
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

    pub(super) async fn lock_post_stop_cleanup_attachment(
        &self,
        session_id: &SessionId,
        attachment_id: RuntimeLoopAttachmentId,
    ) -> Result<MachineManagedPostStopFence, RuntimeDriverError> {
        let gate_guard = self
            .lock_current_session_mutation_gate(session_id)
            .await
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        let (authorized, unregister_saga_owns_post_stop) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            let registration_phase = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .state()
                .registration_phase;
            (
                // The machine-minted cleanup id is the exact attachment
                // incarnation fence. A task-dead attachment can already have
                // released its channel slot while its retained cleanup/teardown
                // handoff is still current; requiring physical slot ownership
                // here would reject the only owner capable of cleaning it.
                // Replacement publishes a fresh id under this same mutation
                // gate, so stale A can never acquire authority over B.
                entry.post_stop_cleanup_attachment_id == Some(attachment_id),
                registration_phase == crate::meerkat_machine::dsl::RegistrationPhase::Draining,
            )
        };
        if !authorized {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "runtime loop no longer owns post-stop cleanup authority for session {session_id}"
                ),
            });
        }
        if unregister_saga_owns_post_stop {
            // Generated Draining means the independently-owned unregister saga
            // already owns final cleanup. Retaining M in the loop handoff would
            // deadlock that saga when it commits producer dispositions before
            // consuming this executor. The attachment check above is still the
            // exact fence; only its mechanical lock is released here.
            drop(gate_guard);
            Ok(MachineManagedPostStopFence::UnregisterSagaOwned)
        } else {
            Ok(MachineManagedPostStopFence::Retained(gate_guard))
        }
    }

    async fn complete_post_stop_cleanup_if_needed(
        &self,
        session_id: &SessionId,
        attachment_id: RuntimeLoopAttachmentId,
        turn_finalization_boundary_already_held: bool,
    ) -> Result<(), RuntimeDriverError> {
        // Cleanup intentionally runs without the session mutation gate. A
        // persistent surface takes its stable turn-finalization boundary before
        // its recovery gate here. Runtime/direct commits use the same outer
        // boundary and then machine mutation -> recovery, so cleanup never
        // introduces the old recovery -> machine inversion. Generated Draining
        // fences explicit unregister; for ordinary stop, the committed Stopped
        // lifecycle plus the exact cleanup id fence readmission/replacement.
        // This attachment-local mutex makes concurrent loop/external/retry
        // attempts exactly-once.
        let cleanup_gate = {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Ok(());
            };
            if entry.post_stop_cleanup_attachment_id != Some(attachment_id) {
                return Ok(());
            }
            if entry.post_stop_cleanup_complete {
                return Ok(());
            }
            Arc::clone(&entry.post_stop_cleanup_gate)
        };
        let _cleanup_guard = cleanup_gate.lock().await;

        let cleanup_handle = {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Ok(());
            };
            if entry.post_stop_cleanup_attachment_id != Some(attachment_id) {
                return Ok(());
            }
            if entry.post_stop_cleanup_complete {
                return Ok(());
            }
            entry.post_stop_cleanup_handle.clone()
        };

        if let Some(cleanup_handle) = cleanup_handle {
            let cleanup_result = if turn_finalization_boundary_already_held {
                cleanup_handle
                    .cleanup_after_runtime_stop_terminalized_under_turn_finalization_boundary()
                    .await
            } else {
                cleanup_handle
                    .cleanup_after_runtime_stop_terminalized()
                    .await
            };
            cleanup_result.map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "post-stop service cleanup failed for session {session_id}: {error}"
                ))
            })?;
        }

        let mut sessions = self.sessions.write().await;
        let Some(entry) = sessions.get_mut(session_id) else {
            return Ok(());
        };
        if entry.post_stop_cleanup_attachment_id != Some(attachment_id) {
            return Ok(());
        }
        entry.post_stop_cleanup_complete = true;
        Ok(())
    }

    /// Complete service/surface cleanup for exactly the attachment whose
    /// runtime-owned decorator has committed an ordinary stop.
    ///
    /// The opaque attachment ID is minted and retained by this machine. The
    /// mutation gate, exact-ID check, and committed `Stopped` witness together
    /// prevent stale A from cleaning replacement B. Ordinary stop preserves the
    /// generated registered `Stopped` state; only an explicit unregister (or an
    /// abnormal loop-exit disposition) opens the machine-owned `Draining` saga.
    async fn complete_terminalized_runtime_loop_cleanup_if_current_with_guard(
        &self,
        session_id: &SessionId,
        attachment_id: RuntimeLoopAttachmentId,
        gate_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<(), RuntimeDriverError> {
        let (driver, registration_phase) = {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Ok(());
            };
            if entry.post_stop_cleanup_attachment_id != Some(attachment_id) {
                // This loop is stale relative to the current service cleanup
                // authority; it must not touch a replacement actor.
                return Ok(());
            }
            let registration_phase = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .state()
                .registration_phase;
            (Arc::clone(&entry.driver), registration_phase)
        };
        #[cfg(test)]
        {
            let mut fail_session = self
                .test_fail_post_stop_unregister_after_fence
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if fail_session.as_ref() == Some(session_id) {
                *fail_session = None;
                drop(fail_session);
                drop(gate_guard);
                return Err(RuntimeDriverError::Internal(
                    "injected post-stop cleanup failure after fence consumption".to_string(),
                ));
            }
        }
        if !matches!(
            driver.lock().await.runtime_state(),
            RuntimeState::Stopped | RuntimeState::Retired
        ) {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "post-stop cleanup for session {session_id} requires committed Stopped or Retired runtime authority"
                ),
            });
        }
        if registration_phase == crate::meerkat_machine::dsl::RegistrationPhase::Draining {
            // An external unregister already owns the generated drain and is
            // awaiting this loop. Release the mutation fence and return; that
            // owner invokes the retained cleanup handle outside the machine
            // gate after this loop has quiesced.
            drop(gate_guard);
            return Ok(());
        }
        if registration_phase != crate::meerkat_machine::dsl::RegistrationPhase::Queuing {
            drop(gate_guard);
            return Ok(());
        }
        // Service cleanup may acquire the stable turn-finalization boundary B.
        // Release M first, then let the compare-and-remove cleanup revalidate
        // this exact id before and after its callback. The committed Stopped
        // phase prevents readmission until the ordinary stop coordinator has
        // completed, and any later replacement necessarily publishes a new id.
        drop(gate_guard);
        self.complete_post_stop_cleanup_if_needed(session_id, attachment_id, false)
            .await
    }

    #[cfg(test)]
    pub(crate) fn fail_next_post_stop_unregister_after_fence_for_test(
        &self,
        session_id: &SessionId,
    ) {
        *self
            .test_fail_post_stop_unregister_after_fence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(session_id.clone());
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
        let (begin_input, context) = {
            let sessions = self.sessions.read().await;
            let entry = sessions.get(session_id).ok_or_else(|| {
                RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                }
                .to_string()
            })?;
            let unserved_attachment =
                matches!(entry.attachment_slot, RuntimeLoopAttachmentSlot::Pending(_));
            let authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = authority.state();
            let session_id = crate::meerkat_machine::dsl::SessionId::from_domain(session_id);
            let agent_runtime_id = state.active_runtime_id.clone();
            let fence_token = state.active_fence_token;
            let generation = state.active_runtime_generation;
            let runtime_epoch_id = state.active_runtime_epoch_id.clone();
            if unserved_attachment {
                (
                    crate::meerkat_machine::dsl::MeerkatMachineInput::BeginUnregisterUnservedAttachment {
                        session_id,
                        agent_runtime_id,
                        fence_token,
                        generation,
                        runtime_epoch_id,
                    },
                    "BeginUnregisterUnservedAttachment",
                )
            } else {
                (
                    crate::meerkat_machine::dsl::MeerkatMachineInput::BeginUnregisterSession {
                        session_id,
                        agent_runtime_id,
                        fence_token,
                        generation,
                        runtime_epoch_id,
                    },
                    "BeginUnregisterSession",
                )
            }
        };
        self.stage_session_dsl_transition(session_id, begin_input, context)
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
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        // Once the final transition is staged, every detached session-owned
        // handle must fail closed on both persistent and storeless machines.
        // An ordinary persistence failure rolls back only to the completed-
        // drain retry anchor; it never reopens the retired runtime epoch.
        entry.close_handle_teardown_gate();
        let staged = Self::stage_dsl_transition_on_authority(
            &entry.dsl_authority,
            unregister_input,
            "UnregisterSession",
        )
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        if self.store.is_some() {
            entry.pending_unregister_finalization = Some(PendingUnregisterFinalization {
                finalization_id: uuid::Uuid::new_v4(),
                durability_authority: authority.clone(),
                committed_snapshot: staged.committed_snapshot.clone(),
            });
        }
        drop(sessions);
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

        match durability_authority.action {
            crate::meerkat_machine::dsl::RuntimeOpsLifecycleDurabilityAction::DeleteSnapshot => {
                let delete_authority = durability_authority
                    .delete_ops_finalization_authority()
                    .ok_or_else(|| {
                        RuntimeDriverError::Internal(
                            "generated DeleteSnapshot verdict lost its finalization witness"
                                .to_string(),
                        )
                    })?;
                driver
                    .commit_unregister_finalization(
                        "unregister",
                        retired_ops_epoch,
                        delete_authority,
                    )
                    .await
            }
            crate::meerkat_machine::dsl::RuntimeOpsLifecycleDurabilityAction::RetainSnapshot => {
                // Retaining the ops snapshot does not authorize retaining an
                // unfinished unregister prefix. Persist the generated final image so
                // Retired remains terminal with its bindings and drain obligations
                // cleared instead of leaving recovery anchored in Draining.
                let retain_authority = durability_authority
                    .retain_ops_finalization_authority()
                    .ok_or_else(|| {
                        RuntimeDriverError::Internal(
                            "generated RetainSnapshot verdict lost its finalization witness"
                                .to_string(),
                        )
                    })?;
                driver
                    .persist_completed_unregister_machine_lifecycle("unregister", retain_authority)
                    .await
            }
        }
    }

    /// Publish the generated terminal lifecycle and retire the matching ops
    /// snapshot through the store's single atomic unregister transaction. The
    /// live entry stays installed until that transaction succeeds; on failure
    /// the generated completed-drain snapshot remains the retry anchor.
    async fn finalize_unregister_durability_transaction(
        &self,
        driver: SharedDriver,
        durability_authority: RuntimeOpsLifecycleDurabilityAuthority,
        retired_ops_epoch: &meerkat_core::RuntimeEpochId,
    ) -> Result<(), RuntimeDriverError> {
        self.finalize_unregistered_session(driver, durability_authority, retired_ops_epoch)
            .await
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
    /// 3. **Drop the mutation gate.** With `Draining` already visible, prove
    ///    physical live-channel absence while retaining the live/lifecycle
    ///    lease. Close callbacks may re-acquire the mutation gate, but no Open
    ///    or same-ID replacement can enter the window.
    /// 4. The in-flight run commits and the loop exits through
    ///    `lock_current_runtime_loop_driver_authority`, which re-acquires this
    ///    same mutation gate — awaiting the loop under the gate would deadlock.
    ///    The loop's own commits are exactly what we wait for.
    /// 5. The independently-owned saga awaits the exact runtime-loop
    ///    `JoinHandle` without aborting it. Caller-visible waiting and live
    ///    interrupt delivery are bounded separately; timeout returns typed
    ///    in-progress truth while this saga remains joinable. The comms drain
    ///    task's `JoinError::is_cancelled` is benign — it was just aborted.
    /// 6. Re-acquire the gate; resolve any completion waiters the in-flight run
    ///    did not already resolve with the runtime-terminated outcome.
    /// 7. Fire the three `*ForUnregister` feedback inputs to close the
    ///    obligations.
    /// 8. Stage + commit the final `UnregisterSession`; persist, remove the
    ///    entry, finalize.
    async fn run_owned_unregister_teardown_inner(
        &self,
        session_id: &SessionId,
        epoch_id: &meerkat_core::RuntimeEpochId,
        coordinator_id: uuid::Uuid,
        registration_transaction_guard: crate::tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<(), RuntimeDriverError> {
        let (pending_finalization, entry_incarnation) = {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Ok(());
            };
            if &entry.epoch_id != epoch_id
                || !entry
                    .unregister_coordinator
                    .as_ref()
                    .is_some_and(|coordinator| {
                        coordinator.epoch_id == *epoch_id
                            && coordinator.coordinator_id == coordinator_id
                    })
            {
                return Ok(());
            }
            (
                entry.pending_unregister_finalization.clone(),
                UnregisterEntryIncarnationWitness {
                    mutation_gate: Arc::clone(&entry.mutation_gate),
                    ops_lifecycle: Arc::clone(&entry.ops_lifecycle),
                    #[cfg(feature = "live")]
                    live_lifecycle_gate: Arc::clone(&entry.live_lifecycle_gate),
                },
            )
        };
        // The present exact coordinator plus generated Draining/pending truth
        // is now the process-local replacement fence. T protects coordinator
        // installation and the final compare-remove only; retaining it across
        // driver/store work would make same-ID registration wait indefinitely
        // instead of observing typed in-progress/finalization truth.
        drop(registration_transaction_guard);
        #[cfg(feature = "live")]
        let live_lifecycle_lease = match self
            .acquire_unregister_live_lifecycle_lease(session_id)
            .await?
        {
            Some(lease) => lease,
            None => return Ok(()),
        };
        #[cfg(feature = "live")]
        let Some(gate_guard) = self
            .lock_exact_unregister_mutation_gate(
                session_id,
                &entry_incarnation,
                &live_lifecycle_lease,
            )
            .await
        else {
            return Ok(());
        };
        #[cfg(not(feature = "live"))]
        let Some(gate_guard) = self
            .lock_exact_unregister_mutation_gate(session_id, &entry_incarnation)
            .await
        else {
            return Ok(());
        };
        tracing::info!(%session_id, "MeerkatMachine::unregister_session_inner_locked_authorized start");
        let (driver_handle, completions, publication_handle, post_stop_cleanup_attachment_id) = {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Ok(());
            };
            if &entry.epoch_id != epoch_id
                || !entry_incarnation.matches(entry)
                || !entry
                    .unregister_coordinator
                    .as_ref()
                    .is_some_and(|coordinator| {
                        coordinator.epoch_id == *epoch_id
                            && coordinator.coordinator_id == coordinator_id
                    })
            {
                return Ok(());
            }
            (
                Arc::clone(&entry.driver),
                Arc::clone(&entry.completions),
                entry.publication_handle(),
                entry.post_stop_cleanup_attachment_id,
            )
        };

        // Do not cross the generated Draining boundary unless every active
        // directed input already has an exact terminal publication
        // capability. Draining deliberately prevents attaching a new
        // executor, so a later missing-publisher error would otherwise be a
        // permanent teardown state rather than a recoverable precondition.
        if publication_handle.is_none()
            && driver_handle
                .lock()
                .await
                .active_inputs_require_terminal_publication()?
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "session {session_id} has active directed input but no terminal publication capability"
                ),
            });
        }

        if let Some(pending) = pending_finalization {
            let exact_retry_witness_is_current = {
                let sessions = self.sessions.read().await;
                let Some(entry) = sessions.get(session_id) else {
                    return Ok(());
                };
                Self::finalized_unregister_entry_is_exact(
                    entry,
                    session_id,
                    epoch_id,
                    &entry_incarnation,
                    &driver_handle,
                    coordinator_id,
                    &pending.committed_snapshot,
                    Some(&pending),
                )?
            };
            if !exact_retry_witness_is_current {
                return Err(RuntimeDriverError::UnregisterFinalizationOutcomeUnknown {
                    reason: format!(
                        "session {session_id} no longer matches the exact generated finalization witness"
                    ),
                });
            }
            // Pending finalization has already closed handle issuance and
            // retired every operation producer. Release M before the
            // idempotent store transaction; coordinator + pending witness
            // continue to reject every ordinary mutation. Retain L as the
            // physical-absence witness until durable finalization finishes.
            drop(gate_guard);
            let result = self
                .finalize_unregistered_session(
                    Arc::clone(&driver_handle),
                    pending.durability_authority.clone(),
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
                .compare_remove_finalized_unregister_entry(
                    session_id,
                    epoch_id,
                    &entry_incarnation,
                    &driver_handle,
                    coordinator_id,
                    &pending.committed_snapshot,
                    Some(&pending),
                    #[cfg(feature = "live")]
                    live_lifecycle_lease,
                )
                .await;
        }

        // Fence prepared/creating actors before generated Draining becomes
        // visible. The same synchronous claim lock makes this linear with a
        // competing materialization commit.
        {
            let sessions = self.sessions.read().await;
            if let Some(entry) = sessions.get(session_id) {
                let mut state = entry
                    .materialization_claim_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.current.is_some()
                    && matches!(
                        state.phase,
                        crate::RuntimeActorMaterializationClaimPhase::Prepared
                            | crate::RuntimeActorMaterializationClaimPhase::Staged
                            | crate::RuntimeActorMaterializationClaimPhase::ActorCreating
                            | crate::RuntimeActorMaterializationClaimPhase::ActorMaterializedPendingCommit
                    )
                {
                    state.phase = crate::RuntimeActorMaterializationClaimPhase::Aborting;
                }
            }
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
        #[cfg(feature = "live")]
        self.prove_member_live_absence_while_lease_held(session_id, &live_lifecycle_lease)
            .await
            .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?;
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
        #[cfg(feature = "live")]
        let Some(pre_cleanup_gate) = self
            .lock_exact_unregister_mutation_gate(
                session_id,
                &entry_incarnation,
                &live_lifecycle_lease,
            )
            .await
        else {
            return Ok(());
        };
        #[cfg(not(feature = "live"))]
        let Some(pre_cleanup_gate) = self
            .lock_exact_unregister_mutation_gate(session_id, &entry_incarnation)
            .await
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
                let _guard = crate::control_plane::terminalize_async_stop(
                    &driver_handle,
                    Some(&completions),
                    publication_handle,
                    None,
                )
                .await?;
            }
        }

        // Phase 5: re-acquire the gate. If the session vanished while the gate
        // was released (e.g. a racing teardown), the drain already completed
        // elsewhere — nothing left to commit.
        #[cfg(feature = "live")]
        let Some(_gate_guard) = self
            .lock_exact_unregister_mutation_gate(
                session_id,
                &entry_incarnation,
                &live_lifecycle_lease,
            )
            .await
        else {
            tracing::debug!(
                %session_id,
                "session removed by a concurrent teardown during unregister drain (benign)"
            );
            return Ok(());
        };
        #[cfg(not(feature = "live"))]
        let Some(_gate_guard) = self
            .lock_exact_unregister_mutation_gate(session_id, &entry_incarnation)
            .await
        else {
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

        // Canonical terminalization above durably publishes every semantic
        // runtime-termination recipient before resolving its waiter bundle.
        // Anything left in the registry is therefore orphaned plumbing, not a
        // second public terminal that unregister may fabricate.
        if unregister_state.unregister_completion_waiter_drain_pending {
            completions.lock().await.fail_not_pending_waiters(
                |_| false,
                crate::completion::CompletionWaitError::AuthorityUnavailable(
                    "runtime session unregistered after canonical terminal recipients resolved"
                        .to_string(),
                ),
            );
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

        // Surface cleanup acquires the persistent turn-finalization boundary
        // before its recovery gate. The absent-entry transaction was released
        // once Draining became durable; release mutation authority too before
        // crossing that seam. The retained live/lifecycle lease prevents live
        // rematerialization, while archive/retire reject this coordinator
        // under the registration transaction before attempting that lease.
        //
        // A non-machine-managed executor has no attachment-local cleanup ID:
        // its exact cleanup is owned solely by RuntimeLoopTeardownSlot above.
        // Machine-managed attachments retain a cloneable service handle here;
        // the teardown slot owns only their machine-unregister decorator, so
        // the attachment-local gate/complete bit makes this call exactly once.
        drop(_gate_guard);
        let post_stop_cleanup_result = match post_stop_cleanup_attachment_id {
            Some(attachment_id) => {
                self.complete_post_stop_cleanup_if_needed(session_id, attachment_id, false)
                    .await
            }
            None => Ok(()),
        };
        #[cfg(feature = "live")]
        let Some(finalization_gate_guard) = self
            .lock_exact_unregister_mutation_gate(
                session_id,
                &entry_incarnation,
                &live_lifecycle_lease,
            )
            .await
        else {
            return Ok(());
        };
        #[cfg(not(feature = "live"))]
        let Some(finalization_gate_guard) = self
            .lock_exact_unregister_mutation_gate(session_id, &entry_incarnation)
            .await
        else {
            return Ok(());
        };
        {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Ok(());
            };
            if &entry.epoch_id != epoch_id
                || !Arc::ptr_eq(&entry.driver, &driver_handle)
                || !Arc::ptr_eq(&entry.ops_lifecycle, &entry_incarnation.ops_lifecycle)
                || !entry
                    .unregister_coordinator
                    .as_ref()
                    .is_some_and(|coordinator| {
                        coordinator.epoch_id == *epoch_id
                            && coordinator.coordinator_id == coordinator_id
                    })
                || entry
                    .dsl_authority
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .state()
                    .registration_phase
                    != crate::meerkat_machine::dsl::RegistrationPhase::Draining
            {
                return Ok(());
            }
        }
        // Keep the generated Draining retry anchor and the exact cloneable
        // cleanup authority on failure. The next owned unregister saga retries
        // this handle before it can retire ops or commit final Unregister.
        post_stop_cleanup_result?;

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
            if &entry.epoch_id != epoch_id
                || !Arc::ptr_eq(&entry.driver, &driver_handle)
                || !Arc::ptr_eq(&entry.ops_lifecycle, &entry_incarnation.ops_lifecycle)
                || !entry
                    .unregister_coordinator
                    .as_ref()
                    .is_some_and(|coordinator| {
                        coordinator.epoch_id == *epoch_id
                            && coordinator.coordinator_id == coordinator_id
                    })
                || entry
                    .dsl_authority
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .state()
                    .registration_phase
                    != crate::meerkat_machine::dsl::RegistrationPhase::Draining
            {
                return Ok(());
            }
            Arc::clone(&entry_incarnation.ops_lifecycle)
        };
        // Registry retirement may synchronously wait for persistence
        // acknowledgements while holding the registry's own write lock. M is
        // only the capture fence: release it before that work, while the exact
        // coordinator, Draining state, and retained live lease continue to
        // reject replacement and rematerialization.
        drop(finalization_gate_guard);
        ops_lifecycle
            .retire_owner_for_unregister("runtime session unregistered".into())
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "failed to terminalize ops lifecycle before unregister: {error}"
                ))
            })?;

        let persistence_worker = loop {
            #[cfg(feature = "live")]
            let Some(persistence_gate_guard) = self
                .lock_exact_unregister_mutation_gate(
                    session_id,
                    &entry_incarnation,
                    &live_lifecycle_lease,
                )
                .await
            else {
                return Ok(());
            };
            #[cfg(not(feature = "live"))]
            let Some(persistence_gate_guard) = self
                .lock_exact_unregister_mutation_gate(session_id, &entry_incarnation)
                .await
            else {
                return Ok(());
            };
            let mut sessions = match self.sessions.try_write() {
                Ok(sessions) => sessions,
                Err(_) => {
                    drop(persistence_gate_guard);
                    crate::tokio::task::yield_now().await;
                    continue;
                }
            };
            let Some(entry) = sessions.get_mut(session_id) else {
                return Ok(());
            };
            if &entry.epoch_id != epoch_id
                || !Arc::ptr_eq(&entry.driver, &driver_handle)
                || !Arc::ptr_eq(&entry.ops_lifecycle, &ops_lifecycle)
                || !entry
                    .unregister_coordinator
                    .as_ref()
                    .is_some_and(|coordinator| {
                        coordinator.epoch_id == *epoch_id
                            && coordinator.coordinator_id == coordinator_id
                    })
                || entry
                    .dsl_authority
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .state()
                    .registration_phase
                    != crate::meerkat_machine::dsl::RegistrationPhase::Draining
            {
                return Ok(());
            }
            let worker = entry.ops_lifecycle_persistence_worker.take();
            drop(sessions);
            drop(persistence_gate_guard);
            break worker;
        };
        // The retired registry has closed its persistence producer. Release M
        // before joining the backend worker. L remains held as the physical-
        // absence witness until durable finalization finishes.
        if let Some(persistence_worker) = persistence_worker {
            join_ops_lifecycle_persistence_worker(persistence_worker).await?;
        }

        // Phase 7: stage + commit the final UnregisterSession.
        #[cfg(feature = "live")]
        let Some(final_stage_gate_guard) = self
            .lock_exact_unregister_mutation_gate(
                session_id,
                &entry_incarnation,
                &live_lifecycle_lease,
            )
            .await
        else {
            return Ok(());
        };
        #[cfg(not(feature = "live"))]
        let Some(final_stage_gate_guard) = self
            .lock_exact_unregister_mutation_gate(session_id, &entry_incarnation)
            .await
        else {
            return Ok(());
        };
        {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Ok(());
            };
            if &entry.epoch_id != epoch_id
                || !Arc::ptr_eq(&entry.driver, &driver_handle)
                || !Arc::ptr_eq(&entry.ops_lifecycle, &ops_lifecycle)
                || entry.pending_unregister_finalization.is_some()
                || entry.ops_lifecycle_persistence_worker.is_some()
                || !entry
                    .unregister_coordinator
                    .as_ref()
                    .is_some_and(|coordinator| {
                        coordinator.epoch_id == *epoch_id
                            && coordinator.coordinator_id == coordinator_id
                    })
                || entry
                    .dsl_authority
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .state()
                    .registration_phase
                    != crate::meerkat_machine::dsl::RegistrationPhase::Draining
            {
                return Ok(());
            }
        }
        tracing::info!(%session_id, "MeerkatMachine::unregister_session_inner_locked_authorized staging unregister");
        let (staged, durability_authority) =
            self.stage_unregister_session_authority(session_id).await?;
        let expected_terminal_snapshot = staged.committed_snapshot.clone();
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
        let expected_pending_finalization = {
            let sessions = self.sessions.read().await;
            let entry = sessions.get(session_id).ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "session disappeared after final unregister commit: {session_id}"
                ))
            })?;
            if &entry.epoch_id != epoch_id
                || !Arc::ptr_eq(&entry.driver, &driver_handle)
                || !entry
                    .unregister_coordinator
                    .as_ref()
                    .is_some_and(|coordinator| {
                        coordinator.epoch_id == *epoch_id
                            && coordinator.coordinator_id == coordinator_id
                    })
            {
                return Ok(());
            }
            entry.pending_unregister_finalization.clone()
        };
        if self.store.is_some() && expected_pending_finalization.is_none() {
            return Err(RuntimeDriverError::Internal(format!(
                "persistent final unregister for session {session_id} lost its exact finalization witness"
            )));
        }
        // Pending/terminal generated truth now rejects every ordinary
        // mutation. Release M before the atomic store transaction; the final
        // T -> exact-L -> exact-M section below is only the compare-remove
        // commit point. Retain L through the store operation so physical
        // absence remains stable.
        drop(final_stage_gate_guard);
        tracing::info!(%session_id, "MeerkatMachine::unregister_session_inner_locked_authorized committed unregister");
        tracing::info!(%session_id, "MeerkatMachine::unregister_session_inner_locked_authorized finalizing durable unregister");
        let finalization_result = self
            .finalize_unregister_durability_transaction(
                Arc::clone(&driver_handle),
                durability_authority,
                epoch_id,
            )
            .await;
        if let Err(error) = finalization_result {
            if !matches!(
                &error,
                RuntimeDriverError::UnregisterFinalizationOutcomeUnknown { .. }
            ) {
                let rollback_restored = loop {
                    #[cfg(feature = "live")]
                    let clear_gate = self
                        .lock_exact_unregister_mutation_gate(
                            session_id,
                            &entry_incarnation,
                            &live_lifecycle_lease,
                        )
                        .await;
                    #[cfg(not(feature = "live"))]
                    let clear_gate = self
                        .lock_exact_unregister_mutation_gate(session_id, &entry_incarnation)
                        .await;
                    let Some(clear_gate) = clear_gate else {
                        break false;
                    };
                    let mut sessions = match self.sessions.try_write() {
                        Ok(sessions) => sessions,
                        Err(_) => {
                            drop(clear_gate);
                            crate::tokio::task::yield_now().await;
                            continue;
                        }
                    };
                    if let Some(entry) = sessions.get_mut(session_id)
                        && &entry.epoch_id == epoch_id
                        && Arc::ptr_eq(&entry.driver, &driver_handle)
                        && entry
                            .unregister_coordinator
                            .as_ref()
                            .is_some_and(|coordinator| {
                                coordinator.epoch_id == *epoch_id
                                    && coordinator.coordinator_id == coordinator_id
                            })
                        && pending_unregister_finalization_matches(
                            entry.pending_unregister_finalization.as_ref(),
                            expected_pending_finalization.as_ref(),
                        )
                        && entry
                            .dsl_authority
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .snapshot()
                            .state()
                            == expected_terminal_snapshot.state()
                    {
                        Self::restore_dsl_authority_snapshot(
                            &entry.dsl_authority,
                            unregister_rollback_snapshot.clone(),
                        );
                        entry.pending_unregister_finalization = None;
                        entry.sync_control_projection_from_dsl_authority();
                        drop(sessions);
                        drop(clear_gate);
                        break true;
                    }
                    drop(sessions);
                    drop(clear_gate);
                    break false;
                };
                if !rollback_restored {
                    return Err(RuntimeDriverError::UnregisterFinalizationOutcomeUnknown {
                        reason: format!(
                            "session {session_id} changed before a definite unregister finalization failure could restore its exact Draining witness: {error}"
                        ),
                    });
                }
                let rollback_result = {
                    let mut driver = driver_handle.lock().await;
                    driver
                        .persist_current_machine_lifecycle("unregister rollback")
                        .await
                };
                return match rollback_result {
                    Ok(()) => Err(error),
                    Err(rollback_error) => Err(RuntimeDriverError::Internal(format!(
                        "{error}; additionally failed to persist unregister rollback: {rollback_error}"
                    ))),
                };
            }
            return Err(error);
        }
        tracing::info!(%session_id, "MeerkatMachine::unregister_session_inner_locked_authorized removing entry");
        self.compare_remove_finalized_unregister_entry(
            session_id,
            epoch_id,
            &entry_incarnation,
            &driver_handle,
            coordinator_id,
            &expected_terminal_snapshot,
            expected_pending_finalization.as_ref(),
            #[cfg(feature = "live")]
            live_lifecycle_lease,
        )
        .await?;
        tracing::info!(%session_id, "MeerkatMachine::unregister_session_inner_locked_authorized complete");
        Ok(())
    }

    #[cfg(feature = "live")]
    async fn lock_exact_unregister_mutation_gate(
        &self,
        session_id: &SessionId,
        entry_incarnation: &UnregisterEntryIncarnationWitness,
        live_lifecycle_lease: &crate::member_live::MemberLiveLifecycleLease,
    ) -> Option<crate::tokio::sync::OwnedMutexGuard<()>> {
        if !live_lifecycle_lease.matches_gate(&entry_incarnation.live_lifecycle_gate) {
            return None;
        }
        loop {
            let guard = Arc::clone(&entry_incarnation.mutation_gate)
                .lock_owned()
                .await;
            let sessions = match self.sessions.try_read() {
                Ok(sessions) => sessions,
                Err(_) => {
                    drop(guard);
                    crate::tokio::task::yield_now().await;
                    continue;
                }
            };
            let entry = sessions.get(session_id)?;
            if entry_incarnation.matches(entry) {
                return Some(guard);
            }
            return None;
        }
    }

    #[cfg(not(feature = "live"))]
    async fn lock_exact_unregister_mutation_gate(
        &self,
        session_id: &SessionId,
        entry_incarnation: &UnregisterEntryIncarnationWitness,
    ) -> Option<crate::tokio::sync::OwnedMutexGuard<()>> {
        loop {
            let guard = Arc::clone(&entry_incarnation.mutation_gate)
                .lock_owned()
                .await;
            let sessions = match self.sessions.try_read() {
                Ok(sessions) => sessions,
                Err(_) => {
                    drop(guard);
                    crate::tokio::task::yield_now().await;
                    continue;
                }
            };
            let entry = sessions.get(session_id)?;
            if entry_incarnation.matches(entry) {
                return Some(guard);
            }
            return None;
        }
    }

    /// Commit the process-local half of a successful durable unregister.
    ///
    /// Slow mechanical joins run after an exact L + M capture with M released.
    /// The unregister worker's original exact L stays held through those joins
    /// and the final compare-remove. The commit point then takes T while that
    /// L is still held, acquires exact M, revalidates the complete witness, and
    /// removes from the map without awaiting while all three fences are held.
    /// This bounded L -> T section is safe only after the exact unregister
    /// coordinator is installed: every archive/retire T -> L path rejects that
    /// coordinator under T before attempting L.
    // These fields are the exact unregister witness and remain explicit so no
    // partial lifecycle bundle can be mistaken for removal authority.
    #[allow(clippy::too_many_arguments)]
    async fn compare_remove_finalized_unregister_entry(
        &self,
        session_id: &SessionId,
        epoch_id: &meerkat_core::RuntimeEpochId,
        entry_incarnation: &UnregisterEntryIncarnationWitness,
        driver: &SharedDriver,
        coordinator_id: uuid::Uuid,
        expected_terminal_snapshot: &crate::meerkat_machine::dsl::MeerkatMachineAuthoritySnapshot,
        expected_pending: Option<&PendingUnregisterFinalization>,
        #[cfg(feature = "live")] live_lifecycle_lease: crate::member_live::MemberLiveLifecycleLease,
    ) -> Result<(), RuntimeDriverError> {
        self.quiesce_finalized_unregister_entry_mechanics(
            session_id,
            epoch_id,
            entry_incarnation,
            driver,
            coordinator_id,
            expected_terminal_snapshot,
            expected_pending,
            #[cfg(feature = "live")]
            &live_lifecycle_lease,
        )
        .await?;

        loop {
            let registration_transaction_guard =
                self.lock_session_registration_transaction(session_id).await;
            #[cfg(feature = "live")]
            let Some(mutation_guard) = self
                .lock_exact_unregister_mutation_gate(
                    session_id,
                    entry_incarnation,
                    &live_lifecycle_lease,
                )
                .await
            else {
                return Ok(());
            };
            #[cfg(not(feature = "live"))]
            let Some(mutation_guard) = self
                .lock_exact_unregister_mutation_gate(session_id, entry_incarnation)
                .await
            else {
                return Ok(());
            };
            let mut sessions = match self.sessions.try_write() {
                Ok(sessions) => sessions,
                Err(_) => {
                    drop(mutation_guard);
                    drop(registration_transaction_guard);
                    crate::tokio::task::yield_now().await;
                    continue;
                }
            };
            let Some(entry) = sessions.get(session_id) else {
                return Ok(());
            };
            if !Self::finalized_unregister_entry_is_exact(
                entry,
                session_id,
                epoch_id,
                entry_incarnation,
                driver,
                coordinator_id,
                expected_terminal_snapshot,
                expected_pending,
            )? {
                return Ok(());
            }
            let removed_entry = sessions.remove(session_id);
            drop(sessions);
            drop(mutation_guard);
            #[cfg(feature = "live")]
            drop(live_lifecycle_lease);
            drop(registration_transaction_guard);
            drop(removed_entry);
            return Ok(());
        }
    }

    // Keep the same exact unregister witness explicit across the mechanical
    // quiescence phase rather than introducing a second shadow authority type.
    #[allow(clippy::too_many_arguments)]
    async fn quiesce_finalized_unregister_entry_mechanics(
        &self,
        session_id: &SessionId,
        epoch_id: &meerkat_core::RuntimeEpochId,
        entry_incarnation: &UnregisterEntryIncarnationWitness,
        driver: &SharedDriver,
        coordinator_id: uuid::Uuid,
        expected_terminal_snapshot: &crate::meerkat_machine::dsl::MeerkatMachineAuthoritySnapshot,
        expected_pending: Option<&PendingUnregisterFinalization>,
        #[cfg(feature = "live")]
        live_lifecycle_lease: &crate::member_live::MemberLiveLifecycleLease,
    ) -> Result<(), RuntimeDriverError> {
        let (drain_task, rotation_task) = loop {
            #[cfg(feature = "live")]
            let Some(mutation_guard) = self
                .lock_exact_unregister_mutation_gate(
                    session_id,
                    entry_incarnation,
                    live_lifecycle_lease,
                )
                .await
            else {
                return Ok(());
            };
            #[cfg(not(feature = "live"))]
            let Some(mutation_guard) = self
                .lock_exact_unregister_mutation_gate(session_id, entry_incarnation)
                .await
            else {
                return Ok(());
            };
            let mut sessions = match self.sessions.try_write() {
                Ok(sessions) => sessions,
                Err(_) => {
                    drop(mutation_guard);
                    crate::tokio::task::yield_now().await;
                    continue;
                }
            };
            let Some(entry) = sessions.get_mut(session_id) else {
                return Ok(());
            };
            if !Self::finalized_unregister_entry_is_exact(
                entry,
                session_id,
                epoch_id,
                entry_incarnation,
                driver,
                coordinator_id,
                expected_terminal_snapshot,
                expected_pending,
            )? {
                return Ok(());
            }
            let mechanics = (
                entry.drain_slot.take_handle(),
                Arc::clone(&entry.supervisor_rotation_task),
            );
            drop(sessions);
            drop(mutation_guard);
            break mechanics;
        };
        if let Some(drain_task) = drain_task {
            drain_task.abort();
            let _ = drain_task.await;
        }
        let rotation_handle = rotation_task.abort_keeping_handle().await;
        if let Some(rotation_handle) = rotation_handle {
            let _ = rotation_handle.await;
        }
        Ok(())
    }

    // Exact compare-and-remove validation intentionally names every witness
    // component at this single ownership seam.
    #[allow(clippy::too_many_arguments)]
    fn finalized_unregister_entry_is_exact(
        entry: &RuntimeSessionEntry,
        session_id: &SessionId,
        epoch_id: &meerkat_core::RuntimeEpochId,
        entry_incarnation: &UnregisterEntryIncarnationWitness,
        driver: &SharedDriver,
        coordinator_id: uuid::Uuid,
        expected_terminal_snapshot: &crate::meerkat_machine::dsl::MeerkatMachineAuthoritySnapshot,
        expected_pending: Option<&PendingUnregisterFinalization>,
    ) -> Result<bool, RuntimeDriverError> {
        let expected_state = expected_terminal_snapshot.state();
        if expected_state.registration_phase
            != crate::meerkat_machine::dsl::RegistrationPhase::Draining
        {
            return Err(RuntimeDriverError::Internal(format!(
                "final unregister removal for session {session_id} received a non-Draining witness"
            )));
        }
        if &entry.epoch_id != epoch_id
            || !entry_incarnation.matches(entry)
            || !Arc::ptr_eq(&entry.driver, driver)
        {
            return Ok(false);
        }
        if !entry
            .unregister_coordinator
            .as_ref()
            .is_some_and(|coordinator| {
                coordinator.epoch_id == *epoch_id && coordinator.coordinator_id == coordinator_id
            })
        {
            return Ok(false);
        }
        let current_state = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .snapshot();
        if current_state.state() != expected_state
            || !pending_unregister_finalization_matches(
                entry.pending_unregister_finalization.as_ref(),
                expected_pending,
            )
            || entry.handle_teardown_gate.is_open()
            || entry.ops_lifecycle_persistence_worker.is_some()
        {
            return Err(RuntimeDriverError::UnregisterFinalizationOutcomeUnknown {
                reason: format!(
                    "session {session_id} no longer matches the exact post-commit unregister witness"
                ),
            });
        }
        Ok(true)
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

    #[cfg(any(target_arch = "wasm32", test))]
    pub async fn discard_terminal_storeless_session(&self, _session_id: &SessionId) -> bool {
        // Compatibility seam for the WASM provisioner. Teardown must be owned
        // by the canonical unregister coordinator, which installs the exact
        // registration transaction, attachment-incarnation, and final
        // compare-remove witnesses. This shortcut cannot establish those
        // witnesses atomically and therefore always fails closed; its caller
        // immediately falls through to canonical unregister.
        false
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

    /// Process-mechanical witness for an attached runtime loop whose command
    /// channels are still open. Unlike `session_has_executor`, this does not
    /// read the generated registration claim, which can remain Active after a
    /// task exits unexpectedly.
    pub async fn session_has_live_executor_attachment(&self, session_id: &SessionId) -> bool {
        self.sessions
            .read()
            .await
            .get(session_id)
            .is_some_and(RuntimeSessionEntry::physical_attachment_is_live)
    }

    /// Arm a one-shot loop abort immediately after the materializer's next
    /// executor ensure returns. This deterministically exercises the startup
    /// window in which runtime-loop cleanup can race sidecar publication.
    #[cfg(feature = "test-support")]
    pub fn test_stop_next_executor_after_ensure(&self) {
        self.test_stop_executor_after_ensure
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// Arm the post-ensure stop fault and pause its owner before cleanup.
    /// Cross-crate lock-order tests use the pause to acquire a competing
    /// service boundary without relying on scheduler timing.
    #[cfg(feature = "test-support")]
    pub fn test_pause_next_executor_after_ensure(&self) {
        self.test_pause_executor_after_ensure
            .store(true, std::sync::atomic::Ordering::Release);
        self.test_stop_next_executor_after_ensure();
    }

    #[cfg(feature = "test-support")]
    pub async fn test_wait_for_executor_after_ensure_pause(&self) {
        self.test_executor_after_ensure_pause_reached
            .notified()
            .await;
    }

    #[cfg(feature = "test-support")]
    pub fn test_release_executor_after_ensure_pause(&self) {
        self.test_executor_after_ensure_pause_release.notify_one();
    }

    /// Run the executor-attach post-ensure fault when test support armed it.
    /// Always present so production callers need no feature-dependent API;
    /// without `test-support` this compiles to a no-op.
    #[doc(hidden)]
    pub async fn run_executor_attach_post_ensure_test_hook(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        #[cfg(feature = "test-support")]
        {
            if self
                .test_stop_executor_after_ensure
                .swap(false, std::sync::atomic::Ordering::AcqRel)
            {
                if self
                    .test_pause_executor_after_ensure
                    .swap(false, std::sync::atomic::Ordering::AcqRel)
                {
                    self.test_executor_after_ensure_pause_reached.notify_one();
                    self.test_executor_after_ensure_pause_release
                        .notified()
                        .await;
                }
                let aborted_exact_pending = {
                    let sessions = self.sessions.read().await;
                    sessions.get(session_id).is_some_and(|entry| {
                        let RuntimeLoopAttachmentSlot::Pending(attachment) = &entry.attachment_slot
                        else {
                            return false;
                        };
                        attachment.loop_handle.abort();
                        true
                    })
                };
                return Err(RuntimeDriverError::Internal(if aborted_exact_pending {
                    "test fault: aborted pending executor in attach post-ensure window".to_string()
                } else {
                    format!(
                        "test fault: no exact pending executor existed in attach post-ensure window for {session_id}"
                    )
                }));
            }
        }
        #[cfg(not(feature = "test-support"))]
        let _ = session_id;
        Ok(())
    }

    /// Test-support fault injector: abort the runtime-loop task while retaining
    /// its pre-exit lifecycle/registration authority and without invoking the
    /// executor's terminal cleanup hook. This models a task panic or an early
    /// runtime-loop startup return so higher-level reconciliation can prove it
    /// does not depend on cooperative cleanup.
    #[cfg(feature = "test-support")]
    pub async fn test_abort_runtime_loop_without_cleanup(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, RuntimeDriverError> {
        let handle = {
            let mut sessions = self.sessions.write().await;
            let entry = sessions
                .get_mut(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            match std::mem::replace(&mut entry.attachment_slot, RuntimeLoopAttachmentSlot::Empty) {
                RuntimeLoopAttachmentSlot::Pending(attachment)
                | RuntimeLoopAttachmentSlot::Attached(attachment) => {
                    // Abort while the task's channel senders are still retained
                    // by `attachment`, then drop those mechanics only after the
                    // cancellation request is committed.
                    attachment.loop_handle.abort();
                    Some(attachment.loop_handle)
                }
                RuntimeLoopAttachmentSlot::Empty => None,
            }
        };
        let Some(handle) = handle else {
            return Ok(false);
        };
        match handle.await {
            Ok(()) => Ok(true),
            Err(error) if error.is_cancelled() => Ok(true),
            Err(error) => Err(RuntimeDriverError::Internal(format!(
                "fault-injected runtime loop ended unexpectedly: {error}"
            ))),
        }
    }

    /// Unit-test fault injector for the exact final-publication rollback path.
    /// The runtime mechanics stay otherwise live and Pending, but the serving
    /// sender is replaced with one whose receiver is already closed.
    #[cfg(test)]
    pub(crate) async fn test_fail_next_attachment_serving_release(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        let RuntimeLoopAttachmentSlot::Pending(attachment) = &mut entry.attachment_slot else {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "session {session_id} has no pending attachment for serving-release fault"
                ),
            });
        };
        let original = attachment
            .serving_release
            .replace(crate::runtime_loop::RuntimeLoopServingRelease::closed_receiver_for_test());
        // Keep the real startup receiver waiting so the attachment's other
        // channels remain live through validation. Exact abort terminates the
        // loop after this one-shot fault; leaking this test-only sender avoids
        // adding fault state to the production attachment representation.
        if let Some(original) = original {
            std::mem::forget(original);
        }
        Ok(())
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

    /// Cooperative boundary cancel for a host-materialized member, fenced to
    /// the exact registered host residency under the session mutation gate.
    /// A delayed controller command can therefore never target a replacement
    /// host incarnation that reused the same session/member tuple.
    pub async fn cancel_after_boundary_for_member_incarnation(
        &self,
        session_id: &SessionId,
        expected_member: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    ) -> Result<(), RuntimeDriverError> {
        self.cancel_after_boundary_inner_for_incarnation(session_id, Some(expected_member), true)
            .await
    }

    /// Bridge cooperative cancel whose `None` target is an exact peer-only
    /// authority claim. The stable residency slot is held through the runtime
    /// effect, preventing a peer-only command from overtaking a placed-member
    /// install on the same session id.
    pub async fn cancel_after_boundary_for_optional_member_incarnation(
        &self,
        session_id: &SessionId,
        expected_member: Option<
            &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        >,
    ) -> Result<(), RuntimeDriverError> {
        self.cancel_after_boundary_inner_for_incarnation(session_id, expected_member, true)
            .await
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

        let (driver, completions, publication_handle) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            (
                entry.driver.clone(),
                entry.completions.clone(),
                entry.publication_handle(),
            )
        };

        let (abandoned, completion_input_ids, candidate_owner_input_id) = {
            let mut driver = driver.lock().await;
            let completion_input_ids = driver.as_driver().active_input_ids();
            let prepared = driver.prepare_runless_runtime_terminated_interaction_outboxes(
                &completion_input_ids,
                reason.clone(),
            )?;
            let abandoned = match driver
                .abandon_pending_inputs(crate::input_state::InputAbandonReason::Retired)
                .await
            {
                Ok(abandoned) => abandoned,
                Err(error) => {
                    driver.rollback_prepared_runless_interaction_terminal_outboxes(prepared);
                    return Err(error);
                }
            };
            let candidate_owner_input_id =
                crate::meerkat_machine::driver::DriverEntry::commit_prepared_runless_interaction_terminal_outboxes(prepared);
            (abandoned, completion_input_ids, candidate_owner_input_id)
        };
        crate::control_plane::publish_and_resolve_runless_runtime_termination(
            &driver,
            Some(&completions),
            publication_handle.as_deref(),
            &completion_input_ids,
            candidate_owner_input_id.as_ref(),
            &reason,
        )
        .await?;
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
