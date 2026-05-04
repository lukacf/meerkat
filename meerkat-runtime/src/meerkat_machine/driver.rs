//! Runtime driver entry point and lifecycle.

use std::sync::Arc;

use meerkat_core::lifecycle::{CoreApplyFailureCause, InputId, RunBoundaryReceipt, RunId};

use crate::accept::{AcceptOutcome, ResolvedAdmission};
use crate::driver::ephemeral::{EphemeralDriverRollbackSnapshot, EphemeralRuntimeDriver};
use crate::driver::persistent::PersistentRuntimeDriver;
use crate::identifiers::{IdempotencyKey, LogicalRuntimeId};
use crate::ingress_types::ContentShape;
use crate::input::Input;
use crate::input_state::{
    InputLifecycleState, InputState, InputStateHistoryEntry, InputStateSeed, InputTerminalOutcome,
    StoredInputState,
};
use crate::runtime_state::RuntimeState;
use crate::tokio::sync::Mutex;
use crate::traits::{
    DestroyReport, RecoveryReport, ResetReport, RetireReport, RuntimeDriver, RuntimeDriverError,
};
use chrono::Utc;

/// Shared driver handle used by both the adapter and the RuntimeLoop.
pub(crate) type SharedDriver = Arc<Mutex<DriverEntry>>;

/// Read-only view over driver-local ingress state. The driver itself owns
/// the DSL and shell metadata; this view just forwards the read accessors
/// needed by callers that used to inspect the deleted `RuntimeIngressAuthority`.
pub(crate) struct IngressView<'a> {
    driver: &'a EphemeralRuntimeDriver,
}

impl IngressView<'_> {
    pub(crate) fn queue(&self) -> Vec<InputId> {
        self.driver.queue_lane()
    }

    pub(crate) fn steer_queue(&self) -> Vec<InputId> {
        self.driver.steer_lane()
    }

    pub(crate) fn admission_order(&self) -> &[InputId] {
        self.driver.admission_order()
    }

    pub(crate) fn handling_mode(
        &self,
        input_id: &InputId,
    ) -> Option<meerkat_core::types::HandlingMode> {
        self.driver.admitted_handling_mode(input_id)
    }

    pub(crate) fn runtime_semantics(
        &self,
        input_id: &InputId,
    ) -> Option<crate::ingress_types::RuntimeInputSemantics> {
        self.driver.admitted_runtime_semantics(input_id)
    }

    pub(crate) fn primitive_projection(
        &self,
        input_id: &InputId,
    ) -> Option<crate::ingress_types::RuntimeInputProjection> {
        self.driver.admitted_primitive_projection(input_id)
    }

    pub(crate) fn is_prompt(&self, input_id: &InputId) -> bool {
        self.driver.admitted_is_prompt(input_id)
    }

    pub(crate) fn content_shape(&self, input_id: &InputId) -> Option<ContentShape> {
        self.driver.admitted_content_shape(input_id)
    }

    pub(crate) fn policy(&self, input_id: &InputId) -> Option<&crate::policy::PolicyDecision> {
        self.driver.admitted_policy(input_id)
    }

    pub(crate) fn request_id(&self, input_id: &InputId) -> Option<crate::ingress_types::RequestId> {
        self.driver.admitted_request_id(input_id)
    }

    pub(crate) fn reservation_key(
        &self,
        input_id: &InputId,
    ) -> Option<crate::ingress_types::ReservationKey> {
        self.driver.admitted_reservation_key(input_id)
    }

    #[allow(dead_code)]
    pub(crate) fn lifecycle_state(&self, input_id: &InputId) -> Option<InputLifecycleState> {
        self.driver.ingress_lifecycle(input_id)
    }
}

/// Per-session runtime driver entry.
pub(crate) enum DriverEntry {
    Ephemeral(EphemeralRuntimeDriver),
    Persistent(PersistentRuntimeDriver),
}

pub(crate) enum PreparedDestroyLifecycle {
    Ephemeral(EphemeralDriverRollbackSnapshot),
    Persistent(EphemeralDriverRollbackSnapshot),
}

enum DriverRollbackSnapshot {
    Ephemeral(EphemeralDriverRollbackSnapshot),
    Persistent(EphemeralDriverRollbackSnapshot),
}

pub(crate) struct PreparedDestroy {
    pub(crate) report: DestroyReport,
    pub(crate) lifecycle: PreparedDestroyLifecycle,
}

impl DriverEntry {
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

    pub(crate) fn input_id_for_idempotency_key(&self, key: &IdempotencyKey) -> Option<InputId> {
        match self {
            DriverEntry::Ephemeral(d) => d.ledger().input_id_for_idempotency_key(key),
            DriverEntry::Persistent(d) => d.inner_ref().ledger().input_id_for_idempotency_key(key),
        }
    }

    pub(crate) fn resolve_admission_for_runtime_idle(
        &self,
        input: &Input,
        runtime_idle: bool,
    ) -> ResolvedAdmission {
        match self {
            DriverEntry::Ephemeral(d) => d.resolve_admission_for_runtime_idle(input, runtime_idle),
            DriverEntry::Persistent(d) => d.resolve_admission_for_runtime_idle(input, runtime_idle),
        }
    }

    pub(crate) async fn accept_resolved_input(
        &mut self,
        input: Input,
        resolved: ResolvedAdmission,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => d.accept_resolved_input(input, resolved).await,
            DriverEntry::Persistent(d) => d.accept_resolved_input(input, resolved).await,
        }
    }

    pub(crate) fn input_phase(&self, input_id: &InputId) -> Option<InputLifecycleState> {
        self.as_driver().input_phase(input_id)
    }

    pub(crate) fn input_last_run_id(&self, input_id: &InputId) -> Option<RunId> {
        self.as_driver().input_last_run_id(input_id)
    }

    pub(crate) fn input_last_boundary_sequence(&self, input_id: &InputId) -> Option<u64> {
        self.as_driver().input_last_boundary_sequence(input_id)
    }

    pub(crate) fn input_terminal_outcome(
        &self,
        input_id: &InputId,
    ) -> Option<crate::input_state::InputTerminalOutcome> {
        match self {
            DriverEntry::Ephemeral(d) => d.input_terminal_outcome(input_id),
            DriverEntry::Persistent(d) => d.inner_ref().input_terminal_outcome(input_id),
        }
    }

    /// Set the silent comms intents for the underlying driver.
    pub(crate) fn set_silent_comms_intents(&mut self, intents: Vec<String>) {
        match self {
            DriverEntry::Ephemeral(d) => d.set_silent_comms_intents(intents),
            DriverEntry::Persistent(d) => d.set_silent_comms_intents(intents),
        }
    }

    pub(crate) fn silent_comms_intents(&self) -> Vec<String> {
        match self {
            DriverEntry::Ephemeral(d) => d.silent_comms_intents(),
            DriverEntry::Persistent(d) => d.silent_comms_intents(),
        }
    }

    /// Check if the runtime is idle or attached (quiescent with or without executor).
    pub(crate) fn is_idle_or_attached(&self) -> bool {
        self.runtime_state().is_idle_or_attached()
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

    /// Check if the runtime can process queued inputs (Idle, Attached, or Retired).
    pub(crate) fn can_process_queue(&self) -> bool {
        self.runtime_state().can_process_queue()
    }

    /// Inspect the current typed post-admission signal without draining it.
    pub(crate) fn post_admission_signal(&self) -> crate::driver::ephemeral::PostAdmissionSignal {
        match self {
            DriverEntry::Ephemeral(d) => d.post_admission_signal(),
            DriverEntry::Persistent(d) => d.post_admission_signal(),
        }
    }

    pub(crate) fn shared_dsl_authority(
        &self,
    ) -> crate::driver::ephemeral::SharedIngressDslAuthority {
        match self {
            DriverEntry::Ephemeral(d) => d.shared_dsl_authority(),
            DriverEntry::Persistent(d) => d.inner_ref().shared_dsl_authority(),
        }
    }

    pub(crate) fn absorb_post_admission_effects(
        &mut self,
        effects: &[crate::meerkat_machine::dsl::MeerkatMachineEffect],
    ) {
        match self {
            DriverEntry::Ephemeral(d) => d.absorb_post_admission_effects(effects),
            DriverEntry::Persistent(d) => d.absorb_post_admission_effects(effects),
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
    pub(crate) fn dequeue_next(&mut self) -> Option<(InputId, crate::input::Input)> {
        match self {
            DriverEntry::Ephemeral(d) => d.dequeue_next(),
            DriverEntry::Persistent(d) => d.dequeue_next(),
        }
    }

    /// Dequeue a specific input by ID from whichever queue contains it.
    pub(crate) fn dequeue_by_id(
        &mut self,
        input_id: &InputId,
    ) -> Option<(InputId, crate::input::Input)> {
        match self {
            DriverEntry::Ephemeral(d) => d.dequeue_by_id(input_id),
            DriverEntry::Persistent(d) => d.dequeue_by_id(input_id),
        }
    }

    /// Get access to the driver's ingress state (queue lanes, admission
    /// metadata) through the concrete driver shell. This is a thin passthrough
    /// facade — the driver itself owns the DSL and the shell metadata maps.
    pub(crate) fn driver_ingress(&self) -> IngressView<'_> {
        match self {
            DriverEntry::Ephemeral(d) => IngressView { driver: d },
            DriverEntry::Persistent(d) => IngressView {
                driver: d.inner_ref(),
            },
        }
    }

    pub(crate) fn runtime_state(&self) -> crate::runtime_state::RuntimeState {
        let authority = self.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::meerkat_machine::dsl_authority::runtime_phase_from_authority(&authority)
    }

    pub(crate) fn current_run_id(&self) -> Option<RunId> {
        let authority = self.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::meerkat_machine::dsl_authority::current_run_id_from_authority(&authority)
    }

    pub(crate) fn pre_run_phase(&self) -> Option<crate::runtime_state::RuntimeState> {
        let authority = self.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::meerkat_machine::dsl_authority::pre_run_phase_from_authority(&authority)
    }

    pub(crate) fn set_control_projection(
        &mut self,
        next_phase: crate::runtime_state::RuntimeState,
        current_run_id: Option<RunId>,
        pre_run_phase: Option<crate::runtime_state::RuntimeState>,
    ) {
        match self {
            DriverEntry::Ephemeral(d) => {
                d.set_control_projection(next_phase, current_run_id, pre_run_phase);
            }
            DriverEntry::Persistent(d) => {
                d.set_control_projection(next_phase, current_run_id, pre_run_phase);
            }
        }
    }

    pub(crate) fn sync_control_projection_from_dsl_authority(&mut self) {
        match self {
            DriverEntry::Ephemeral(d) => d.sync_control_projection_from_dsl_authority(),
            DriverEntry::Persistent(d) => d.sync_control_projection_from_dsl_authority(),
        }
    }

    pub(crate) fn control_projection_handle(
        &self,
    ) -> Arc<std::sync::RwLock<crate::driver::ephemeral::RuntimeControlProjection>> {
        match self {
            DriverEntry::Ephemeral(d) => d.control_handle(),
            DriverEntry::Persistent(d) => d.inner_ref().control_handle(),
        }
    }

    pub(crate) fn ledger(&self) -> &crate::input_ledger::InputLedger {
        match self {
            DriverEntry::Ephemeral(d) => d.ledger(),
            DriverEntry::Persistent(d) => d.inner_ref().ledger(),
        }
    }

    fn rollback_snapshot(&self) -> DriverRollbackSnapshot {
        match self {
            DriverEntry::Ephemeral(d) => DriverRollbackSnapshot::Ephemeral(d.rollback_snapshot()),
            DriverEntry::Persistent(d) => DriverRollbackSnapshot::Persistent(d.rollback_snapshot()),
        }
    }

    fn restore_rollback_snapshot(&mut self, checkpoint: DriverRollbackSnapshot) {
        match (self, checkpoint) {
            (DriverEntry::Ephemeral(d), DriverRollbackSnapshot::Ephemeral(checkpoint)) => {
                d.restore_rollback_snapshot(checkpoint);
            }
            (DriverEntry::Persistent(d), DriverRollbackSnapshot::Persistent(checkpoint)) => {
                d.restore_rollback_snapshot(checkpoint);
            }
            _ => {}
        }
    }

    pub(crate) fn has_queued_input_outside(&self, excluded: &[InputId]) -> bool {
        match self {
            DriverEntry::Ephemeral(d) => d.has_queued_input_outside(excluded),
            DriverEntry::Persistent(d) => d.has_queued_input_outside(excluded),
        }
    }

    pub(crate) fn defer_queued_inputs_behind_backlog(&mut self, input_ids: &[InputId]) {
        match self {
            DriverEntry::Ephemeral(d) => d.defer_queued_inputs_behind_backlog(input_ids),
            DriverEntry::Persistent(d) => d.defer_queued_inputs_behind_backlog(input_ids),
        }
    }

    pub(crate) async fn machine_realize_boundary_applied(
        &mut self,
        run_id: RunId,
        receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
        session_snapshot: Option<Vec<u8>>,
    ) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => d.machine_realize_boundary_applied(&run_id, &receipt),
            DriverEntry::Persistent(d) => {
                d.machine_realize_boundary_applied(&run_id, &receipt, session_snapshot.as_ref())
                    .await
            }
        }
    }

    pub(crate) async fn machine_realize_run_completed(
        &mut self,
        run_id: RunId,
        consumed_input_ids: Vec<InputId>,
    ) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => {
                d.machine_realize_run_completed(&run_id, &consumed_input_ids)
            }
            DriverEntry::Persistent(d) => {
                d.machine_realize_run_completed(&run_id, &consumed_input_ids)
                    .await
            }
        }
    }

    pub(crate) async fn machine_realize_run_failed(
        &mut self,
        run_id: RunId,
        contributing_input_ids: Vec<InputId>,
        replay_plan: crate::driver::ephemeral::ReplayQueuedContributorsPlan,
        terminal_error: &str,
        runtime_apply_failure: Option<&CoreApplyFailureCause>,
        recoverable: bool,
    ) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => {
                let _ = (terminal_error, runtime_apply_failure, recoverable);
                d.machine_realize_run_failed(&run_id, &contributing_input_ids, &replay_plan)
            }
            DriverEntry::Persistent(d) => {
                d.machine_realize_run_failed(
                    &run_id,
                    &contributing_input_ids,
                    &replay_plan,
                    terminal_error,
                    runtime_apply_failure,
                    recoverable,
                )
                .await
            }
        }
    }

    /// Stage an input (Queued → Staged).
    pub(crate) fn stage_input(
        &mut self,
        input_id: &InputId,
        run_id: &RunId,
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => d.stage_input(input_id, run_id),
            DriverEntry::Persistent(d) => d.stage_input(input_id, run_id),
        }
    }

    /// Stage a batch of inputs atomically in a single `StageDrainSnapshot`.
    pub(crate) fn machine_realize_stage_batch(
        &mut self,
        input_ids: &[InputId],
        run_id: &RunId,
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => d.machine_realize_stage_batch(input_ids, run_id),
            DriverEntry::Persistent(d) => d.machine_realize_stage_batch(input_ids, run_id),
        }
    }

    /// Roll back staged inputs after a failed staging attempt.
    pub(crate) fn rollback_staged(
        &mut self,
        input_ids: &[InputId],
    ) -> Result<(), crate::traits::RuntimeDriverError> {
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

    pub(crate) fn prepare_destroy_lifecycle(&mut self) -> PreparedDestroy {
        match self {
            DriverEntry::Ephemeral(d) => {
                let checkpoint = d.rollback_snapshot();
                let abandoned = d.destroy_cleanup();
                PreparedDestroy {
                    report: DestroyReport {
                        inputs_abandoned: abandoned,
                    },
                    lifecycle: PreparedDestroyLifecycle::Ephemeral(checkpoint),
                }
            }
            DriverEntry::Persistent(d) => {
                let (checkpoint, report) = d.prepare_destroy_lifecycle();
                PreparedDestroy {
                    report,
                    lifecycle: PreparedDestroyLifecycle::Persistent(checkpoint),
                }
            }
        }
    }

    pub(crate) async fn commit_prepared_destroy_lifecycle(
        &mut self,
        lifecycle: PreparedDestroyLifecycle,
    ) -> Result<(), RuntimeDriverError> {
        match (self, lifecycle) {
            (DriverEntry::Ephemeral(_), PreparedDestroyLifecycle::Ephemeral(_)) => Ok(()),
            (DriverEntry::Persistent(d), PreparedDestroyLifecycle::Persistent(checkpoint)) => {
                d.commit_prepared_destroy_lifecycle(checkpoint).await
            }
            _ => Err(RuntimeDriverError::Internal(
                "destroy lifecycle prepared for a different driver kind".to_string(),
            )),
        }
    }

    pub(crate) fn rollback_prepared_destroy_lifecycle(
        &mut self,
        lifecycle: PreparedDestroyLifecycle,
    ) {
        match (self, lifecycle) {
            (DriverEntry::Ephemeral(d), PreparedDestroyLifecycle::Ephemeral(checkpoint)) => {
                d.restore_rollback_snapshot(checkpoint);
            }
            (DriverEntry::Persistent(d), PreparedDestroyLifecycle::Persistent(checkpoint)) => {
                d.rollback_prepared_destroy_lifecycle(checkpoint);
            }
            _ => {}
        }
    }
}

/// Shared completion registry (accessed by adapter for registration and loop for resolution).
pub(crate) type SharedCompletionRegistry = Arc<Mutex<crate::completion::CompletionRegistry>>;

pub(crate) fn machine_validate_active_run(
    driver: &DriverEntry,
    run_id: &RunId,
    next_phase: RuntimeState,
) -> Result<(), crate::runtime_state::RuntimeStateTransitionError> {
    match driver.runtime_state() {
        RuntimeState::Running | RuntimeState::Retired => {}
        from => {
            return Err(crate::runtime_state::RuntimeStateTransitionError {
                from,
                to: next_phase,
            });
        }
    }

    match driver.current_run_id() {
        Some(active_id) if &active_id == run_id => Ok(()),
        _ => Err(crate::runtime_state::RuntimeStateTransitionError {
            from: driver.runtime_state(),
            to: next_phase,
        }),
    }
}

pub(crate) fn machine_begin_run(
    driver: &mut DriverEntry,
    run_id: RunId,
) -> Result<(), crate::runtime_state::RuntimeStateTransitionError> {
    let from = driver.runtime_state();
    if from == RuntimeState::Running && driver.current_run_id().as_ref() == Some(&run_id) {
        return Ok(());
    }
    let pre_run_phase = crate::runtime_state::run_start_pre_phase_from_phase(from)?;
    if driver.current_run_id().is_some() {
        return Err(crate::runtime_state::RuntimeStateTransitionError {
            from,
            to: RuntimeState::Running,
        });
    }

    // DSL is authoritative for `lifecycle_phase` + `current_run_id`
    // post-#32 W6-J (dogma #1 split). Fire the typed `Prepare { session_id,
    // run_id }` DSL input; the `PrepareIdle` / `PrepareAttached` transitions
    // flip `lifecycle_phase` to Running and set `current_run_id` atomically.
    // The runtime-loop path reaches this code directly (via
    // `prepare_runtime_loop_batch_start`); previously the Attached→Running
    // hinge was shell-only via `set_control_projection`, leaving DSL's
    // `current_run_id` unbound / mismatched and the `CommitRunningTo*`
    // guards on the return path rejecting the Commit input. Both sides now
    // go through DSL uniformly. Shell `control_projection` remains only as
    // mechanical projection/event plumbing; DriverEntry reads the DSL
    // authority directly.
    let authority = driver.shared_dsl_authority();
    let dsl_session_id = {
        let auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        auth.state.session_id.clone()
    };
    if let Some(dsl_session_id) = dsl_session_id {
        let mut auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Idempotent fast-path: if DSL is already at Running with a
        // matching run_id, the caller (e.g. `dispatch_ingress::Prepare`
        // command handler) has already staged Prepare; nothing to do.
        let already_prepared = auth.state.lifecycle_phase
            == crate::meerkat_machine::dsl::MeerkatPhase::Running
            && auth.state.current_run_id.as_ref().map(|id| id.0.as_str())
                == Some(run_id.to_string().as_str());
        let is_retired_drain =
            auth.state.lifecycle_phase == crate::meerkat_machine::dsl::MeerkatPhase::Retired;
        if !already_prepared {
            let apply_result = if is_retired_drain {
                auth.apply_signal(
                    crate::meerkat_machine::dsl::MeerkatMachineSignal::DrainQueuedRun {
                        run_id: crate::meerkat_machine::dsl::RunId::from_domain(&run_id),
                    },
                )
                .map(|_| ())
            } else {
                crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                    &mut *auth,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::Prepare {
                        session_id: dsl_session_id,
                        run_id: crate::meerkat_machine::dsl::RunId::from_domain(&run_id),
                    },
                )
                .map(|_| ())
            };
            if apply_result.is_err() {
                return Err(crate::runtime_state::RuntimeStateTransitionError {
                    from,
                    to: RuntimeState::Running,
                });
            }
        }
    }

    driver.set_control_projection(RuntimeState::Running, Some(run_id), Some(pre_run_phase));
    Ok(())
}

/// Disposition the runtime-loop apply produced. `Failed` is only used after the
/// turn-state authority has recorded a typed failed terminal cause; `Rollback`
/// is non-semantic cleanup for prepare/stage failures before turn failure
/// exists.
pub(crate) enum RunReturnDisposition<'a> {
    Commit { input_id: &'a InputId },
    Failed,
    Rollback,
}

#[derive(Debug)]
pub(crate) enum RuntimeLoopRunCommitError {
    Rejected(RuntimeDriverError),
    BoundaryCommit(RuntimeDriverError),
    PostBoundaryValidation(RuntimeDriverError),
    TerminalSnapshot(RuntimeDriverError),
}

impl RuntimeLoopRunCommitError {
    pub(crate) fn should_unregister_session(&self) -> bool {
        matches!(
            self,
            Self::PostBoundaryValidation(_) | Self::TerminalSnapshot(_)
        )
    }

    pub(crate) fn into_driver_error(self) -> RuntimeDriverError {
        match self {
            Self::Rejected(err)
            | Self::BoundaryCommit(err)
            | Self::PostBoundaryValidation(err)
            | Self::TerminalSnapshot(err) => err,
        }
    }
}

impl std::fmt::Display for RuntimeLoopRunCommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(err)
            | Self::BoundaryCommit(err)
            | Self::PostBoundaryValidation(err)
            | Self::TerminalSnapshot(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RuntimeLoopRunCommitError {}

#[derive(Debug)]
pub(crate) enum RuntimeLoopRunFailError {
    Rejected(RuntimeDriverError),
    TerminalSnapshot(RuntimeDriverError),
}

impl RuntimeLoopRunFailError {
    pub(crate) fn should_unregister_session(&self) -> bool {
        matches!(self, Self::TerminalSnapshot(_))
    }

    pub(crate) fn into_driver_error(self) -> RuntimeDriverError {
        match self {
            Self::Rejected(err) | Self::TerminalSnapshot(err) => err,
        }
    }
}

impl std::fmt::Display for RuntimeLoopRunFailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(err) | Self::TerminalSnapshot(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RuntimeLoopRunFailError {}

pub(crate) fn machine_apply_run_return_projection(
    driver: &mut DriverEntry,
    run_id: &RunId,
    disposition: RunReturnDisposition<'_>,
    next_phase: RuntimeState,
) -> Result<(), crate::runtime_state::RuntimeStateTransitionError> {
    let current_phase = driver.runtime_state();
    if matches!(
        current_phase,
        RuntimeState::Retired | RuntimeState::Stopped | RuntimeState::Destroyed
    ) {
        return Ok(());
    }
    if current_phase == next_phase && driver.current_run_id().is_none() {
        return Ok(());
    }
    machine_validate_active_run(driver, run_id, next_phase)?;

    // DSL is authoritative for `lifecycle_phase` post-#32 W6-J (dogma #1
    // split). Fire the typed `Commit {input_id, run_id}` or `Fail {run_id}`
    // DSL input; the DSL's `CommitRunningTo{Idle,Attached,Retired}` /
    // `FailRunningTo{Idle,Attached,Retired}` transitions dispatch on
    // `pre_run_phase` (set by `Prepare` during `machine_begin_run`) and
    // flip `lifecycle_phase` accordingly. The runtime-loop path owns this
    // DSL transition uniformly; dispatch-ingress no longer snapshots or
    // pre-stages the return input.
    let authority = driver.shared_dsl_authority();
    {
        let mut auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let input = match disposition {
            RunReturnDisposition::Commit { input_id } => {
                crate::meerkat_machine::dsl::MeerkatMachineInput::Commit {
                    input_id: crate::meerkat_machine::dsl::InputId::from_domain(input_id),
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                }
            }
            RunReturnDisposition::Failed => {
                crate::meerkat_machine::dsl::MeerkatMachineInput::Fail {
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                }
            }
            RunReturnDisposition::Rollback => {
                crate::meerkat_machine::dsl::MeerkatMachineInput::RollbackRun {
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                }
            }
        };
        if crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(&mut *auth, input).is_err() {
            return Err(crate::runtime_state::RuntimeStateTransitionError {
                from: current_phase,
                to: next_phase,
            });
        }
    }

    // Shell `control_projection` update is retained as mechanical event
    // plumbing. DSL's Commit/Fail transition is the authoritative
    // lifecycle/run writer above; DriverEntry readers consult DSL state, so
    // this projection must stay mechanical.
    driver.set_control_projection(next_phase, None, None);
    Ok(())
}

fn machine_apply_turn_run_completed(
    driver: &mut DriverEntry,
    run_id: &RunId,
) -> Result<(), RuntimeDriverError> {
    let authority = driver.shared_dsl_authority();
    let mut auth = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if auth.state.lifecycle_phase != crate::meerkat_machine::dsl::MeerkatPhase::Running
        || auth.state.current_run_id.as_ref().map(|id| id.0.as_str())
            != Some(run_id.to_string().as_str())
    {
        return Ok(());
    }
    crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        &mut *auth,
        crate::meerkat_machine::dsl::MeerkatMachineInput::RunCompleted {
            run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
        },
    )
    .map(|_| ())
    .map_err(|err| {
        RuntimeDriverError::Internal(format!(
            "failed to apply runtime turn completion for run {run_id}: {err}"
        ))
    })
}

fn machine_apply_turn_run_failed(
    driver: &mut DriverEntry,
    run_id: &RunId,
    terminal_error: &str,
    terminal_cause_kind: meerkat_core::TurnTerminalCauseKind,
    runtime_apply_failure: Option<&CoreApplyFailureCause>,
) -> Result<(), RuntimeDriverError> {
    let authority = driver.shared_dsl_authority();
    let mut auth = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if auth.state.lifecycle_phase != crate::meerkat_machine::dsl::MeerkatPhase::Running
        || auth.state.current_run_id.as_ref().map(|id| id.0.as_str())
            != Some(run_id.to_string().as_str())
    {
        return Ok(());
    }
    crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        &mut *auth,
        crate::meerkat_machine::dsl::MeerkatMachineInput::RunFailed {
            run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
            runtime_apply_failure_cause: runtime_apply_failure
                .map(crate::meerkat_machine::dsl::RuntimeApplyFailureCause::from),
            runtime_apply_failure_message: runtime_apply_failure
                .map(|failure| failure.message().to_owned()),
            terminal_outcome: crate::meerkat_machine::dsl::TurnTerminalOutcome::Failed,
            terminal_cause_kind: terminal_cause_kind.into(),
            error: terminal_error.to_owned(),
        },
    )
    .map(|_| ())
    .map_err(|err| {
        RuntimeDriverError::Internal(format!(
            "failed to apply runtime turn failure for run {run_id}: {err}"
        ))
    })
}

pub(crate) fn slice_starts_with(seq: &[InputId], prefix: &[InputId]) -> bool {
    prefix.len() <= seq.len() && seq[..prefix.len()] == *prefix
}

pub(crate) fn machine_input_boundary(
    driver: &DriverEntry,
    work_id: &InputId,
) -> meerkat_core::lifecycle::run_primitive::RunApplyBoundary {
    driver
        .driver_ingress()
        .runtime_semantics(work_id)
        .map(|semantics| semantics.boundary)
        .unwrap_or(meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart)
}

pub(crate) fn machine_input_execution_kind(
    driver: &DriverEntry,
    work_id: &InputId,
) -> Option<meerkat_core::lifecycle::RuntimeExecutionKind> {
    driver
        .driver_ingress()
        .runtime_semantics(work_id)
        .map(|semantics| semantics.execution_kind)
}

pub(crate) fn machine_input_peer_response_terminal_apply_intent(
    driver: &DriverEntry,
    work_id: &InputId,
) -> Option<meerkat_core::lifecycle::run_primitive::PeerResponseTerminalApplyIntent> {
    driver
        .driver_ingress()
        .runtime_semantics(work_id)
        .and_then(|semantics| semantics.peer_response_terminal_apply_intent)
}

#[cfg(test)]
pub(crate) fn machine_batch_execution_kind(
    driver: &DriverEntry,
    work_ids: &[InputId],
) -> Option<meerkat_core::lifecycle::RuntimeExecutionKind> {
    let mut semantics = machine_batch_runtime_semantics(driver, work_ids)?.into_iter();
    let first = semantics.next()?.execution_kind;

    if semantics.all(|semantics| semantics.execution_kind == first) {
        Some(first)
    } else {
        None
    }
}

pub(crate) fn machine_batch_runtime_semantics(
    driver: &DriverEntry,
    work_ids: &[InputId],
) -> Option<Vec<crate::ingress_types::RuntimeInputSemantics>> {
    work_ids
        .iter()
        .map(|id| driver.driver_ingress().runtime_semantics(id))
        .collect()
}

pub(crate) fn machine_batch_primitive_projections(
    driver: &DriverEntry,
    inputs: &[(InputId, Input)],
) -> Vec<crate::ingress_types::RuntimeInputProjection> {
    let ingress = driver.driver_ingress();
    inputs
        .iter()
        .map(|(id, input)| {
            let projection = ingress.primitive_projection(id).unwrap_or_default();
            if matches!(
                input,
                Input::Peer(crate::input::PeerInput {
                    convention: Some(crate::input::PeerConvention::ResponseTerminal { .. }),
                    ..
                })
            ) {
                crate::input::runtime_input_projection_for_machine_batch(input)
            } else {
                projection
            }
        })
        .collect()
}

pub(crate) fn machine_select_runtime_loop_batch(driver: &DriverEntry) -> Vec<InputId> {
    let ingress = driver.driver_ingress();
    let should_drive_loop = |id: &InputId| {
        ingress.policy(id).is_none_or(|policy| {
            !matches!(policy.wake_mode, crate::policy::WakeMode::None)
                || matches!(
                    policy.drain_policy,
                    crate::policy::DrainPolicy::Immediate | crate::policy::DrainPolicy::SteerBatch
                )
        })
    };
    let steer = ingress.steer_queue();
    if let Some(first) = steer.first() {
        if !should_drive_loop(first) {
            return Vec::new();
        }
        let target_boundary = machine_input_boundary(driver, first);
        let Some(target_execution_kind) = machine_input_execution_kind(driver, first) else {
            return vec![first.clone()];
        };
        let target_peer_response_terminal_apply_intent =
            machine_input_peer_response_terminal_apply_intent(driver, first);
        return steer
            .iter()
            .take_while(|id| {
                machine_input_boundary(driver, id) == target_boundary
                    && machine_input_execution_kind(driver, id) == Some(target_execution_kind)
                    && machine_input_peer_response_terminal_apply_intent(driver, id)
                        == target_peer_response_terminal_apply_intent
            })
            .cloned()
            .collect();
    }

    let queue = ingress.queue();
    if let Some(driver_index) = queue.iter().position(should_drive_loop) {
        let Some(first) = queue.first() else {
            return Vec::new();
        };
        let Some(target_execution_kind) = machine_input_execution_kind(driver, first) else {
            return vec![first.clone()];
        };
        let target_peer_response_terminal_apply_intent =
            machine_input_peer_response_terminal_apply_intent(driver, first);
        let driver_is_prompt = ingress.is_prompt(&queue[driver_index]);
        let mut selected = Vec::new();
        for id in &queue {
            if machine_input_execution_kind(driver, id) != Some(target_execution_kind)
                || machine_input_peer_response_terminal_apply_intent(driver, id)
                    != target_peer_response_terminal_apply_intent
            {
                break;
            }
            if !driver_is_prompt && ingress.is_prompt(id) {
                break;
            }
            selected.push(id.clone());
            if ingress.is_prompt(id) {
                break;
            }
            if driver_is_prompt && selected.len() > driver_index {
                break;
            }
        }
        return selected;
    }

    Vec::new()
}

pub(crate) fn machine_validate_stage_drain_snapshot(
    driver: &DriverEntry,
    contributing_work_ids: &[InputId],
) -> Result<(), RuntimeDriverError> {
    if contributing_work_ids.is_empty() {
        return Err(RuntimeDriverError::Internal(
            "stage drain snapshot requires at least one contributor".to_string(),
        ));
    }

    for work_id in contributing_work_ids {
        let lifecycle = driver.as_driver().input_phase(work_id);
        if lifecycle != Some(InputLifecycleState::Queued) {
            return Err(RuntimeDriverError::Internal(format!(
                "stage drain snapshot requires queued contributors, but {work_id:?} is {lifecycle:?}"
            )));
        }
    }

    let ingress = driver.driver_ingress();
    let steer = ingress.steer_queue();
    let source_queue: Vec<InputId> = if steer.is_empty() {
        ingress.queue()
    } else {
        steer
    };
    if !slice_starts_with(&source_queue, contributing_work_ids) {
        return Err(RuntimeDriverError::Internal(
            "stage drain snapshot contributors must match the current drain-source prefix"
                .to_string(),
        ));
    }

    Ok(())
}

pub(crate) fn machine_validate_boundary_applied(
    driver: &DriverEntry,
    contributing_work_ids: &[InputId],
) -> Result<(), RuntimeDriverError> {
    if contributing_work_ids.is_empty() {
        return Err(RuntimeDriverError::Internal(
            "boundary applied requires at least one contributor".to_string(),
        ));
    }

    for work_id in contributing_work_ids {
        let lifecycle = driver.as_driver().input_phase(work_id);
        if lifecycle != Some(InputLifecycleState::Staged) {
            return Err(RuntimeDriverError::Internal(format!(
                "boundary applied requires staged contributors, but {work_id:?} is {lifecycle:?}"
            )));
        }
    }

    Ok(())
}

pub(crate) fn machine_validate_run_completed(
    driver: &DriverEntry,
    contributing_work_ids: &[InputId],
) -> Result<(), RuntimeDriverError> {
    if contributing_work_ids.is_empty() {
        return Err(RuntimeDriverError::Internal(
            "run completed requires at least one contributor".to_string(),
        ));
    }

    for work_id in contributing_work_ids {
        let lifecycle = driver.as_driver().input_phase(work_id);
        if lifecycle != Some(InputLifecycleState::AppliedPendingConsumption) {
            return Err(RuntimeDriverError::Internal(format!(
                "run completed requires contributors pending consumption, but {work_id:?} is {lifecycle:?}"
            )));
        }
    }

    Ok(())
}

pub(crate) fn machine_validate_run_commit_receipt(
    driver: &DriverEntry,
    run_id: &RunId,
    consumed_input_ids: &[InputId],
    receipt: &meerkat_core::lifecycle::RunBoundaryReceipt,
) -> Result<(), RuntimeDriverError> {
    if &receipt.run_id != run_id {
        return Err(RuntimeDriverError::Internal(format!(
            "run commit receipt run_id {:?} does not match active run {:?}",
            receipt.run_id, run_id
        )));
    }

    if consumed_input_ids != receipt.contributing_input_ids.as_slice() {
        return Err(RuntimeDriverError::Internal(format!(
            "run commit consumed inputs {:?} do not exactly match receipt contributors {:?}",
            consumed_input_ids, receipt.contributing_input_ids
        )));
    }

    machine_validate_boundary_applied(driver, &receipt.contributing_input_ids)
}

pub(crate) fn machine_staged_contributors(driver: &DriverEntry) -> Vec<InputId> {
    driver
        .as_driver()
        .active_input_ids()
        .into_iter()
        .filter(|work_id| {
            driver.as_driver().input_phase(work_id) == Some(InputLifecycleState::Staged)
        })
        .collect()
}

pub(crate) fn machine_validate_run_failed(
    driver: &DriverEntry,
    contributing_work_ids: &[InputId],
) -> Result<(), RuntimeDriverError> {
    if contributing_work_ids.is_empty() {
        return Err(RuntimeDriverError::Internal(
            "run failed requires at least one staged contributor".to_string(),
        ));
    }

    for work_id in contributing_work_ids {
        let lifecycle = driver.as_driver().input_phase(work_id);
        if lifecycle != Some(InputLifecycleState::Staged) {
            return Err(RuntimeDriverError::Internal(format!(
                "run failed requires staged contributors, but {work_id:?} is {lifecycle:?}"
            )));
        }
    }

    Ok(())
}

pub(crate) async fn machine_normalize_recovered_input_state(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    stored_runtime_id: &LogicalRuntimeId,
    mut bundle: StoredInputState,
) -> Result<StoredInputState, RuntimeDriverError> {
    let applied_boundary_committed = if matches!(
        bundle.seed.phase,
        InputLifecycleState::Applied | InputLifecycleState::AppliedPendingConsumption
    ) {
        Some(
            match (
                bundle.seed.last_run_id.clone(),
                bundle.seed.last_boundary_sequence,
            ) {
                (Some(run_id), Some(sequence)) => load_boundary_receipt_for_storage_aliases(
                    store,
                    runtime_id,
                    stored_runtime_id == runtime_id,
                    &run_id,
                    sequence,
                )
                .await
                .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?
                .is_some(),
                _ => false,
            },
        )
    } else {
        None
    };

    let _ = machine_apply_recovered_input_normalization(&mut bundle, applied_boundary_committed);

    Ok(bundle)
}

pub(super) async fn load_boundary_receipt_for_storage_aliases(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    canonical_miss_authoritative: bool,
    run_id: &RunId,
    sequence: u64,
) -> Result<Option<RunBoundaryReceipt>, crate::store::RuntimeStoreError> {
    let mut primary_alias_loaded = false;
    for (candidate_index, candidate) in runtime_id
        .storage_alias_candidates()
        .into_iter()
        .enumerate()
    {
        match store
            .load_boundary_receipt(&candidate, run_id, sequence)
            .await
        {
            Ok(Some(receipt)) => return Ok(Some(receipt)),
            Ok(None) => {
                if candidate_index == 0 && canonical_miss_authoritative {
                    return Ok(None);
                }
                if candidate_index == 0 {
                    primary_alias_loaded = true;
                }
            }
            Err(_err)
                if candidate_index > 0 && primary_alias_loaded && canonical_miss_authoritative =>
            {
                continue;
            }
            Err(err) => return Err(err),
        }
    }
    Ok(None)
}

async fn load_input_states_for_storage_aliases(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
) -> Result<Vec<(LogicalRuntimeId, StoredInputState)>, crate::store::RuntimeStoreError> {
    let mut merged: Vec<(usize, LogicalRuntimeId, StoredInputState)> = Vec::new();
    let mut primary_alias_loaded = false;

    for (candidate_index, candidate) in runtime_id
        .storage_alias_candidates()
        .into_iter()
        .enumerate()
    {
        let states = match store.load_input_states(&candidate).await {
            Ok(states) => {
                if candidate_index == 0 {
                    primary_alias_loaded = true;
                }
                states
            }
            Err(_err) if candidate_index > 0 && primary_alias_loaded => continue,
            Err(err) => return Err(err),
        };
        for state in states {
            let input_id = state.state.input_id.clone();
            if let Some((existing_index, existing_runtime_id, existing_state)) = merged
                .iter_mut()
                .find(|(_, _, existing)| existing.state.input_id == input_id)
            {
                let candidate_updated_at = state.state.updated_at();
                let existing_updated_at = existing_state.state.updated_at();
                let should_replace = if candidate_index == *existing_index {
                    candidate_updated_at > existing_updated_at
                } else {
                    candidate_index < *existing_index
                };
                if should_replace {
                    *existing_index = candidate_index;
                    *existing_runtime_id = candidate.clone();
                    *existing_state = state;
                }
            } else {
                merged.push((candidate_index, candidate.clone(), state));
            }
        }
    }

    Ok(merged
        .into_iter()
        .map(|(_, runtime_id, state)| (runtime_id, state))
        .collect())
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct MachineRecoveryDelta {
    pub recovered: usize,
    pub abandoned: usize,
    pub requeued: usize,
}

pub(crate) fn machine_apply_recovered_input_normalization(
    bundle: &mut StoredInputState,
    applied_boundary_committed: Option<bool>,
) -> MachineRecoveryDelta {
    let mut delta = MachineRecoveryDelta::default();
    let StoredInputState { state, seed } = bundle;

    match seed.phase {
        InputLifecycleState::Accepted => {
            let consume_on_accept = state
                .policy
                .as_ref()
                .map(|policy| {
                    policy.decision.apply_mode == crate::policy::ApplyMode::Ignore
                        && policy.decision.consume_point == crate::policy::ConsumePoint::OnAccept
                })
                .unwrap_or(false);
            let now = Utc::now();
            let from = seed.phase;
            if consume_on_accept {
                state.history.push(InputStateHistoryEntry {
                    timestamp: now,
                    from,
                    to: InputLifecycleState::Consumed,
                    reason: Some("recovery: ConsumeOnAccept (Ignore+OnAccept policy)".into()),
                });
                seed.phase = InputLifecycleState::Consumed;
                seed.terminal_outcome = Some(InputTerminalOutcome::Consumed);
                state.terminal_outcome = Some(InputTerminalOutcome::Consumed);
                state.updated_at = now;
                delta.abandoned += 1;
            } else {
                state.history.push(InputStateHistoryEntry {
                    timestamp: now,
                    from,
                    to: InputLifecycleState::Queued,
                    reason: Some("recovery: QueueAccepted".into()),
                });
                seed.phase = InputLifecycleState::Queued;
                state.updated_at = now;
                delta.requeued += 1;
            }
            delta.recovered += 1;
        }
        InputLifecycleState::Staged => {
            // Crashed mid-stage — rollback to Queued so the replay path can
            // pick it up. This mirrors the recovery view of
            // `RollbackStaged`: phase goes back to `Queued`, while the
            // attempt counter remains whatever the persisted shell/DSL caches
            // already recorded. The live `rollback_staged` path still owns
            // the exhausted-attempts branch once the runtime resumes.
            let now = Utc::now();
            let from = seed.phase;
            state.history.push(InputStateHistoryEntry {
                timestamp: now,
                from,
                to: InputLifecycleState::Queued,
                reason: Some("recovery: RollbackStaged".into()),
            });
            seed.phase = InputLifecycleState::Queued;
            state.updated_at = now;
            delta.requeued += 1;
            delta.recovered += 1;
        }
        InputLifecycleState::Applied | InputLifecycleState::AppliedPendingConsumption => {
            if let Some(has_receipt) = applied_boundary_committed {
                let now = Utc::now();
                let from = seed.phase;
                let to = if has_receipt {
                    InputLifecycleState::Consumed
                } else {
                    InputLifecycleState::Queued
                };
                state.history.push(InputStateHistoryEntry {
                    timestamp: now,
                    from,
                    to,
                    reason: Some(if has_receipt {
                        "recovery: boundary receipt already committed".into()
                    } else {
                        "recovery: missing boundary receipt".into()
                    }),
                });
                seed.phase = to;
                let terminal = if has_receipt {
                    Some(InputTerminalOutcome::Consumed)
                } else {
                    None
                };
                seed.terminal_outcome = terminal.clone();
                state.terminal_outcome = terminal;
                state.updated_at = now;
            }
            delta.recovered += 1;
        }
        InputLifecycleState::Queued => {
            delta.recovered += 1;
        }
        InputLifecycleState::Consumed
        | InputLifecycleState::Superseded
        | InputLifecycleState::Coalesced
        | InputLifecycleState::Abandoned => {}
    }

    delta
}

pub(crate) struct RecoveredIngressEntry {
    pub content_shape: crate::ingress_types::ContentShape,
    pub handling_mode: meerkat_core::types::HandlingMode,
    pub runtime_semantics: crate::ingress_types::RuntimeInputSemantics,
    pub primitive_projection: crate::ingress_types::RuntimeInputProjection,
    pub is_prompt: bool,
    pub policy: crate::policy::PolicyDecision,
}

fn expected_recovered_runtime_semantics(
    state: &InputState,
) -> Option<crate::ingress_types::RuntimeInputSemantics> {
    let persisted_input = state.persisted_input.as_ref()?;
    let policy = &state.policy.as_ref()?.decision;
    Some(
        crate::ingress_types::RuntimeInputSemantics::from_policy_and_kind(
            policy,
            persisted_input.kind(),
        ),
    )
}

pub(crate) fn machine_build_recovered_ingress_entry(
    state: &InputState,
) -> Option<RecoveredIngressEntry> {
    let persisted_input = state.persisted_input.as_ref()?;
    let runtime_semantics = state.runtime_semantics?;
    let policy = state.policy.as_ref()?.decision.clone();
    if runtime_semantics
        != crate::ingress_types::RuntimeInputSemantics::from_policy_and_kind(
            &policy,
            persisted_input.kind(),
        )
    {
        return None;
    }
    let handling_mode = crate::accept::handling_mode_from_policy(&policy);
    let content_shape = crate::ingress_types::ContentShape::from_kind(persisted_input.kind());
    let primitive_projection = crate::input::runtime_input_projection(persisted_input);

    Some(RecoveredIngressEntry {
        content_shape,
        handling_mode,
        runtime_semantics,
        primitive_projection,
        is_prompt: matches!(persisted_input, crate::input::Input::Prompt(_)),
        policy,
    })
}

fn missing_recovered_ingress_entry_reason(state: &InputState) -> String {
    if state.persisted_input.is_none() {
        return format!(
            "store corruption: recovered input '{}' has no persisted input; cannot derive admitted-input content shape",
            state.input_id
        );
    }
    if state.runtime_semantics.is_none() {
        return format!(
            "store corruption: recovered input '{}' missing runtime execution semantics stamp; cannot recover without runtime-stamped execution kind",
            state.input_id
        );
    }
    if state.policy.is_none() {
        return format!(
            "store corruption: recovered input '{}' missing runtime admission policy stamp; cannot recover without runtime-stamped policy and lane metadata",
            state.input_id
        );
    }
    if let Some(expected) = expected_recovered_runtime_semantics(state)
        && state.runtime_semantics != Some(expected)
    {
        return format!(
            "store corruption: recovered input '{}' has runtime execution semantics stamp that does not match persisted input kind and admission policy; cannot recover with contradictory runtime-stamped execution kind",
            state.input_id
        );
    }
    format!(
        "store corruption: recovered input '{}' is missing required admitted-input metadata",
        state.input_id
    )
}

fn recovered_runtime_state_corruption(
    runtime_state: RuntimeState,
    reason: impl Into<String>,
) -> RuntimeDriverError {
    RuntimeDriverError::RecoveryCorruption {
        reason: format!(
            "store corruption: recovered '{runtime_state}' runtime state cannot be replayed through DSL authority: {}",
            reason.into()
        ),
    }
}

fn recovered_session_id(
    driver: &crate::driver::ephemeral::EphemeralRuntimeDriver,
    runtime_state: RuntimeState,
) -> Result<crate::meerkat_machine::dsl::SessionId, RuntimeDriverError> {
    let authority = driver.shared_dsl_authority();
    let auth = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    auth.state.session_id.clone().ok_or_else(|| {
        recovered_runtime_state_corruption(
            runtime_state,
            "missing DSL session authority for session-scoped lifecycle input",
        )
    })
}

fn replay_recovered_runtime_input(
    driver: &crate::driver::ephemeral::EphemeralRuntimeDriver,
    runtime_state: RuntimeState,
    input: crate::meerkat_machine::dsl::MeerkatMachineInput,
    context: &'static str,
) -> Result<(), RuntimeDriverError> {
    let authority = driver.shared_dsl_authority();
    let mut auth = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(&mut *auth, input)
        .map(|_| ())
        .map_err(|err| {
            recovered_runtime_state_corruption(
                runtime_state,
                crate::meerkat_machine::dsl_authority::map_error(err, context),
            )
        })
}

pub(crate) fn machine_realize_recovered_runtime_state(
    driver: &mut crate::driver::ephemeral::EphemeralRuntimeDriver,
    runtime_state: RuntimeState,
) -> Result<(), RuntimeDriverError> {
    match runtime_state {
        RuntimeState::Retired if driver.runtime_state() != RuntimeState::Retired => {
            let session_id = recovered_session_id(driver, runtime_state)?;
            replay_recovered_runtime_input(
                driver,
                runtime_state,
                crate::meerkat_machine::dsl::MeerkatMachineInput::Retire { session_id },
                "RecoverRuntimeState(Retired)",
            )?;
            driver.sync_control_projection_from_dsl_authority();
        }
        RuntimeState::Stopped
            if driver.runtime_state() != RuntimeState::Stopped
                && driver.runtime_state() != RuntimeState::Destroyed =>
        {
            replay_recovered_runtime_input(
                driver,
                runtime_state,
                crate::meerkat_machine::dsl::MeerkatMachineInput::StopRuntimeExecutor {
                    reason: "recovered stopped runtime".to_string(),
                },
                "RecoverRuntimeState(Stopped)",
            )?;
            driver.sync_control_projection_from_dsl_authority();
            driver.stop_runtime_cleanup();
        }
        RuntimeState::Destroyed if driver.runtime_state() != RuntimeState::Destroyed => {
            let session_id = recovered_session_id(driver, runtime_state)?;
            replay_recovered_runtime_input(
                driver,
                runtime_state,
                crate::meerkat_machine::dsl::MeerkatMachineInput::Destroy { session_id },
                "RecoverRuntimeState(Destroyed)",
            )?;
            driver.sync_control_projection_from_dsl_authority();
            driver.destroy_cleanup();
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn machine_recover_ephemeral_driver(
    driver: &mut crate::driver::ephemeral::EphemeralRuntimeDriver,
) -> Result<RecoveryReport, RuntimeDriverError> {
    let mut recovered = 0;
    let mut abandoned = 0;
    let mut requeued = 0;

    // Normalize every active input. Build a bundle from the live driver
    // (ledger + DSL) so the normalization can read/rewrite the seed, then
    // push the normalized bundle back through `admit_recovered_to_ingress`
    // so recovery facts re-enter via typed DSL input.
    let active_ids: Vec<InputId> = driver.active_input_ids();

    let mut normalized: Vec<(InputId, StoredInputState)> = Vec::with_capacity(active_ids.len());
    for input_id in &active_ids {
        let Some(mut bundle) = driver.stored_input_state(input_id) else {
            continue;
        };
        let delta = machine_apply_recovered_input_normalization(&mut bundle, None);
        recovered += delta.recovered;
        abandoned += delta.abandoned;
        requeued += delta.requeued;
        normalized.push((input_id.clone(), bundle));
    }

    // Replay recovered lifecycle facts through the driver's DSL authority.
    // No rebuilt authority — the DSL is the only owner of recovered phase,
    // run/boundary associations, typed terminal metadata, attempt count, and
    // lane membership.
    let mut recovered_entries: Vec<(InputId, RecoveredIngressEntry, InputState, InputStateSeed)> =
        Vec::with_capacity(normalized.len());
    for (input_id, bundle) in normalized {
        let Some(entry) = machine_build_recovered_ingress_entry(&bundle.state) else {
            return Err(RuntimeDriverError::Internal(
                missing_recovered_ingress_entry_reason(&bundle.state),
            ));
        };
        recovered_entries.push((input_id, entry, bundle.state, bundle.seed));
    }

    for (input_id, entry, state, seed) in recovered_entries {
        // Persist the normalized shell back into the ledger only after we have
        // proven the recovered input can re-enter ingress with typed metadata.
        if let Some(ledger_slot) = driver.ledger_mut().get_mut(&input_id) {
            *ledger_slot = state.clone();
        }
        driver.admit_recovered_to_ingress(
            input_id,
            entry.content_shape,
            entry.handling_mode,
            entry.runtime_semantics,
            entry.primitive_projection,
            entry.is_prompt,
            &state,
            &seed,
            entry.policy,
            None,
            None,
        )?;
    }

    driver.rebuild_queue_projections_after_recovery();

    Ok(RecoveryReport {
        inputs_recovered: recovered,
        inputs_abandoned: abandoned,
        inputs_requeued: requeued,
        details: Vec::new(),
    })
}

pub(crate) async fn machine_recover_persistent_driver(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    driver: &mut crate::driver::ephemeral::EphemeralRuntimeDriver,
) -> Result<RecoveryReport, RuntimeDriverError> {
    let mut recovered_payloads = Vec::new();

    for (stored_runtime_id, bundle) in load_input_states_for_storage_aliases(store, runtime_id)
        .await
        .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?
    {
        let bundle =
            machine_normalize_recovered_input_state(store, runtime_id, &stored_runtime_id, bundle)
                .await?;

        if bundle.state.durability == Some(crate::input::InputDurability::Ephemeral) {
            continue;
        }

        if driver.input_state(&bundle.state.input_id).is_none() {
            if bundle.seed.phase.is_terminal() {
                let inserted = driver.ledger_mut().recover(bundle.state.clone());
                if !inserted {
                    continue;
                }
                driver.recover_terminal_input_lifecycle(&bundle.state.input_id, &bundle.seed)?;
                continue;
            }

            let Some(entry) = machine_build_recovered_ingress_entry(&bundle.state) else {
                return Err(RuntimeDriverError::Internal(
                    missing_recovered_ingress_entry_reason(&bundle.state),
                ));
            };

            let inserted = driver.ledger_mut().recover(bundle.state.clone());
            if !inserted {
                continue;
            }

            if let Some(input) = bundle.state.persisted_input.clone() {
                recovered_payloads.push((bundle.state.input_id.clone(), input));
            }

            driver.admit_recovered_to_ingress(
                bundle.state.input_id.clone(),
                entry.content_shape,
                entry.handling_mode,
                entry.runtime_semantics,
                entry.primitive_projection,
                entry.is_prompt,
                &bundle.state,
                &bundle.seed,
                entry.policy,
                None,
                None,
            )?;
        }
    }

    let report = machine_recover_ephemeral_driver(driver)?;

    for (input_id, _input) in recovered_payloads {
        let should_requeue =
            driver.input_phase(&input_id) == Some(crate::input_state::InputLifecycleState::Queued);
        if should_requeue && !driver.has_queued_input(&input_id) {
            return Err(RuntimeDriverError::Internal(format!(
                "persistent recover left queued input '{input_id}' out of the runtime queue projection"
            )));
        }
    }

    let mut recovered_runtime_state = None;
    let mut primary_runtime_state_loaded = false;
    for (candidate_index, candidate) in runtime_id
        .storage_alias_candidates()
        .into_iter()
        .enumerate()
    {
        match store.load_runtime_state(&candidate).await {
            Ok(state) => {
                if candidate_index == 0 {
                    primary_runtime_state_loaded = true;
                }
                recovered_runtime_state = state;
            }
            Err(_err) if candidate_index > 0 && primary_runtime_state_loaded => continue,
            Err(err) => return Err(RuntimeDriverError::Internal(err.to_string())),
        }
        if recovered_runtime_state.is_some() {
            break;
        }
    }
    if let Some(runtime_state) = recovered_runtime_state {
        machine_realize_recovered_runtime_state(driver, runtime_state)?;

        if runtime_state.is_terminal() {
            let active = driver.active_input_ids();
            if !active.is_empty() {
                return Err(RuntimeDriverError::Internal(format!(
                    "store corruption: terminal runtime '{}' has {} active inputs",
                    runtime_state,
                    active.len()
                )));
            }
        }
    }

    Ok(report)
}

pub(crate) fn machine_build_replay_plan(
    driver: &DriverEntry,
    contributing_work_ids: &[InputId],
    notice_kind: &'static str,
) -> crate::driver::ephemeral::ReplayQueuedContributorsPlan {
    let mut queue_work_ids = Vec::new();
    let mut steer_work_ids = Vec::new();
    for work_id in contributing_work_ids {
        match driver.driver_ingress().handling_mode(work_id) {
            Some(meerkat_core::types::HandlingMode::Steer) => steer_work_ids.push(work_id.clone()),
            _ => queue_work_ids.push(work_id.clone()),
        }
    }
    crate::driver::ephemeral::ReplayQueuedContributorsPlan {
        wake_runtime: !(queue_work_ids.is_empty() && steer_work_ids.is_empty()),
        queue_work_ids,
        steer_work_ids,
        notice_kind,
    }
}

pub(crate) async fn machine_stop_runtime(
    driver: &mut DriverEntry,
) -> Result<(), RuntimeDriverError> {
    match driver.runtime_state() {
        RuntimeState::Initializing
        | RuntimeState::Idle
        | RuntimeState::Attached
        | RuntimeState::Running
        | RuntimeState::Retired
        | RuntimeState::Stopped => {}
        from => {
            return Err(RuntimeDriverError::Internal(
                crate::runtime_state::RuntimeStateTransitionError {
                    from,
                    to: RuntimeState::Stopped,
                }
                .to_string(),
            ));
        }
    }

    match driver {
        DriverEntry::Ephemeral(d) => {
            d.apply_runtime_executor_exited_authority()?;
            d.sync_control_projection_from_dsl_authority();
            d.finalize_stop_runtime();
            Ok(())
        }
        DriverEntry::Persistent(d) => d.finalize_runtime_executor_exit().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued_seed() -> InputStateSeed {
        let mut seed = InputStateSeed::new_accepted();
        seed.phase = InputLifecycleState::Queued;
        seed
    }

    fn queue_policy(
        wake_mode: crate::policy::WakeMode,
        drain_policy: crate::policy::DrainPolicy,
    ) -> crate::policy::PolicyDecision {
        crate::policy::PolicyDecision {
            apply_mode: crate::policy::ApplyMode::StageRunStart,
            wake_mode,
            queue_mode: crate::policy::QueueMode::Fifo,
            consume_point: crate::policy::ConsumePoint::OnRunComplete,
            drain_policy,
            routing_disposition: crate::policy::RoutingDisposition::Queue,
            record_transcript: true,
            emit_operator_content: true,
            policy_version: crate::policy_table::DEFAULT_POLICY_VERSION,
        }
    }

    #[test]
    fn machine_batch_execution_kind_requires_admitted_semantics() {
        let driver = DriverEntry::Ephemeral(EphemeralRuntimeDriver::new(
            crate::identifiers::LogicalRuntimeId::new("test"),
        ));
        let unstamped_input = InputId::new();

        assert_eq!(
            machine_batch_execution_kind(&driver, &[unstamped_input]),
            None,
            "missing runtime semantics must not locally default to ContentTurn"
        );
    }

    #[test]
    fn recovered_ingress_entry_requires_persisted_input_payload() {
        let mut state = InputState::new_accepted(InputId::new());
        state.policy = Some(crate::input_state::PolicySnapshot {
            version: crate::identifiers::PolicyVersion(1),
            decision: queue_policy(
                crate::policy::WakeMode::WakeIfIdle,
                crate::policy::DrainPolicy::QueueNextTurn,
            ),
        });

        assert!(
            machine_build_recovered_ingress_entry(&state).is_none(),
            "recovery must not infer Prompt/ContentTurn when persisted input payload is missing"
        );
    }

    #[test]
    fn recovered_ingress_admission_rejects_mismatched_runtime_semantics() {
        let mut driver = EphemeralRuntimeDriver::new(crate::identifiers::LogicalRuntimeId::new(
            "recovered-admission-mismatch",
        ));
        let input = Input::Prompt(crate::input::PromptInput::new("queued prompt", None));
        let input_id = input.id().clone();
        let policy = queue_policy(
            crate::policy::WakeMode::WakeIfIdle,
            crate::policy::DrainPolicy::QueueNextTurn,
        );
        let mut state = InputState::new_accepted(input_id.clone());
        state.persisted_input = Some(input.clone());
        let seed = queued_seed();
        assert!(driver.ledger_mut().recover(state.clone()));
        let mut runtime_semantics =
            crate::ingress_types::RuntimeInputSemantics::from_policy_and_kind(
                &policy,
                input.kind(),
            );
        runtime_semantics.execution_kind =
            meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending;

        let err = driver
            .admit_recovered_to_ingress(
                input_id.clone(),
                ContentShape::from_kind(input.kind()),
                meerkat_core::types::HandlingMode::Queue,
                runtime_semantics,
                crate::input::runtime_input_projection(&input),
                true,
                &state,
                &seed,
                policy,
                None,
                None,
            )
            .expect_err("lower-level recovered admission must reject contradictory stamps");

        assert!(
            err.to_string()
                .contains("does not match persisted input kind and admission policy"),
            "unexpected recovery error: {err}"
        );
        assert!(
            driver.admitted_runtime_semantics(&input_id).is_none(),
            "failed recovered admission must not record mismatched runtime semantics"
        );
    }

    #[test]
    fn prompt_batch_selection_drives_incompatible_prefix_before_prompt() {
        let mut driver = EphemeralRuntimeDriver::new(crate::identifiers::LogicalRuntimeId::new(
            "mixed-prefix-test",
        ));
        let resume_input = Input::Continuation(
            crate::input::ContinuationInput::detached_background_op_completed(),
        );
        let prompt_input = Input::Prompt(crate::input::PromptInput {
            header: crate::input::InputHeader {
                id: InputId::new(),
                timestamp: chrono::Utc::now(),
                source: crate::input::InputOrigin::Operator,
                durability: crate::input::InputDurability::Durable,
                visibility: crate::input::InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            text: "drive the queue".into(),
            blocks: None,
            turn_metadata: None,
        });
        let resume_id = resume_input.id().clone();
        let prompt_id = prompt_input.id().clone();
        let mut resume_state = InputState::new_accepted(resume_id.clone());
        resume_state.persisted_input = Some(resume_input.clone());
        let mut prompt_state = InputState::new_accepted(prompt_id.clone());
        prompt_state.persisted_input = Some(prompt_input.clone());
        let seed = queued_seed();
        assert!(driver.ledger_mut().recover(resume_state.clone()));
        assert!(driver.ledger_mut().recover(prompt_state.clone()));
        let mut resume_policy = queue_policy(
            crate::policy::WakeMode::None,
            crate::policy::DrainPolicy::QueueNextTurn,
        );
        resume_policy.apply_mode = crate::policy::ApplyMode::StageRunBoundary;

        driver
            .admit_recovered_to_ingress(
                resume_id.clone(),
                ContentShape::from_kind(resume_input.kind()),
                meerkat_core::types::HandlingMode::Queue,
                crate::ingress_types::RuntimeInputSemantics {
                    boundary:
                        meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint,
                    execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                    peer_response_terminal_apply_intent: None,
                },
                crate::input::runtime_input_projection(&resume_input),
                false,
                &resume_state,
                &seed,
                resume_policy,
                None,
                None,
            )
            .expect("recover queued resume input");
        driver
            .admit_recovered_to_ingress(
                prompt_id.clone(),
                ContentShape::from_kind(prompt_input.kind()),
                meerkat_core::types::HandlingMode::Queue,
                crate::ingress_types::RuntimeInputSemantics {
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                    execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
                    peer_response_terminal_apply_intent: None,
                },
                crate::input::runtime_input_projection(&prompt_input),
                true,
                &prompt_state,
                &seed,
                queue_policy(
                    crate::policy::WakeMode::WakeIfIdle,
                    crate::policy::DrainPolicy::QueueNextTurn,
                ),
                None,
                None,
            )
            .expect("recover queued prompt input");
        driver.rebuild_queue_projections_after_recovery();

        let entry = DriverEntry::Ephemeral(driver);
        let selected = machine_select_runtime_loop_batch(&entry);

        assert_eq!(
            selected,
            vec![resume_id],
            "a later prompt may drive the queue, but selection must preserve the staged queue prefix when an older input has a different execution kind"
        );
        machine_validate_stage_drain_snapshot(&entry, &selected)
            .expect("selected incompatible prefix must satisfy staging invariants");
    }

    #[test]
    fn batch_selection_surfaces_unstamped_no_wake_prefix_before_prompt() {
        let mut driver = EphemeralRuntimeDriver::new(crate::identifiers::LogicalRuntimeId::new(
            "missing-prefix-before-prompt-selection",
        ));
        let prefix_input = Input::Continuation(
            crate::input::ContinuationInput::detached_background_op_completed(),
        );
        let prompt_input = Input::Prompt(crate::input::PromptInput::new("drive the queue", None));
        let prefix_id = prefix_input.id().clone();
        let prompt_id = prompt_input.id().clone();
        let mut prefix_state = InputState::new_accepted(prefix_id.clone());
        prefix_state.persisted_input = Some(prefix_input.clone());
        let mut prompt_state = InputState::new_accepted(prompt_id.clone());
        prompt_state.persisted_input = Some(prompt_input.clone());
        let seed = queued_seed();
        assert!(driver.ledger_mut().recover(prefix_state.clone()));
        assert!(driver.ledger_mut().recover(prompt_state.clone()));
        let mut prefix_policy = queue_policy(
            crate::policy::WakeMode::None,
            crate::policy::DrainPolicy::QueueNextTurn,
        );
        prefix_policy.apply_mode = crate::policy::ApplyMode::StageRunBoundary;

        driver
            .admit_recovered_to_ingress(
                prefix_id.clone(),
                ContentShape::from_kind(prefix_input.kind()),
                meerkat_core::types::HandlingMode::Queue,
                crate::ingress_types::RuntimeInputSemantics {
                    boundary:
                        meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint,
                    execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                    peer_response_terminal_apply_intent: None,
                },
                crate::input::runtime_input_projection(&prefix_input),
                false,
                &prefix_state,
                &seed,
                prefix_policy,
                None,
                None,
            )
            .expect("recover queued prefix input");
        driver
            .admit_recovered_to_ingress(
                prompt_id,
                ContentShape::from_kind(prompt_input.kind()),
                meerkat_core::types::HandlingMode::Queue,
                crate::ingress_types::RuntimeInputSemantics {
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                    execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
                    peer_response_terminal_apply_intent: None,
                },
                crate::input::runtime_input_projection(&prompt_input),
                true,
                &prompt_state,
                &seed,
                queue_policy(
                    crate::policy::WakeMode::WakeIfIdle,
                    crate::policy::DrainPolicy::QueueNextTurn,
                ),
                None,
                None,
            )
            .expect("recover queued prompt input");
        driver.rebuild_queue_projections_after_recovery();
        driver.clear_admitted_runtime_semantics_for_test(&prefix_id);

        let entry = DriverEntry::Ephemeral(driver);
        let selected = machine_select_runtime_loop_batch(&entry);

        assert_eq!(
            selected,
            vec![prefix_id],
            "an unstamped no-wake prefix entry must be selected before the later prompt so runtime-loop failure handling can consume the queue prefix"
        );
        machine_validate_stage_drain_snapshot(&entry, &selected)
            .expect("selected unstamped prefix must satisfy staging invariants");
        assert!(
            machine_batch_runtime_semantics(&entry, &selected).is_none(),
            "selected unstamped prefix must flow into the runtime-loop metadata conflict path"
        );
    }

    #[test]
    fn batch_selection_non_prompt_driver_stops_before_following_prompt() {
        let mut driver = EphemeralRuntimeDriver::new(crate::identifiers::LogicalRuntimeId::new(
            "non-prompt-driver-before-prompt-selection",
        ));
        let event_input = Input::ExternalEvent(crate::input::ExternalEventInput {
            header: crate::input::InputHeader {
                id: InputId::new(),
                timestamp: chrono::Utc::now(),
                source: crate::input::InputOrigin::External {
                    source_name: "scheduler".into(),
                },
                durability: crate::input::InputDurability::Durable,
                visibility: crate::input::InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "scheduler_tick".into(),
            payload: serde_json::json!({"body": "tick"}),
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        });
        let prompt_input = Input::Prompt(crate::input::PromptInput::new(
            "operator prompt must wait",
            None,
        ));
        let event_id = event_input.id().clone();
        let prompt_id = prompt_input.id().clone();
        let mut event_state = InputState::new_accepted(event_id.clone());
        event_state.persisted_input = Some(event_input.clone());
        let mut prompt_state = InputState::new_accepted(prompt_id.clone());
        prompt_state.persisted_input = Some(prompt_input.clone());
        let seed = queued_seed();
        assert!(driver.ledger_mut().recover(event_state.clone()));
        assert!(driver.ledger_mut().recover(prompt_state.clone()));

        driver
            .admit_recovered_to_ingress(
                event_id.clone(),
                ContentShape::from_kind(event_input.kind()),
                meerkat_core::types::HandlingMode::Queue,
                crate::ingress_types::RuntimeInputSemantics {
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                    execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
                    peer_response_terminal_apply_intent: None,
                },
                crate::input::runtime_input_projection(&event_input),
                false,
                &event_state,
                &seed,
                queue_policy(
                    crate::policy::WakeMode::WakeIfIdle,
                    crate::policy::DrainPolicy::QueueNextTurn,
                ),
                None,
                None,
            )
            .expect("recover queued event input");
        driver
            .admit_recovered_to_ingress(
                prompt_id,
                ContentShape::from_kind(prompt_input.kind()),
                meerkat_core::types::HandlingMode::Queue,
                crate::ingress_types::RuntimeInputSemantics {
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                    execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
                    peer_response_terminal_apply_intent: None,
                },
                crate::input::runtime_input_projection(&prompt_input),
                true,
                &prompt_state,
                &seed,
                queue_policy(
                    crate::policy::WakeMode::WakeIfIdle,
                    crate::policy::DrainPolicy::QueueNextTurn,
                ),
                None,
                None,
            )
            .expect("recover queued prompt input");
        driver.rebuild_queue_projections_after_recovery();

        let entry = DriverEntry::Ephemeral(driver);
        let selected = machine_select_runtime_loop_batch(&entry);

        assert_eq!(
            selected,
            vec![event_id],
            "a non-prompt-driven batch must not absorb a following prompt into the same run"
        );
        machine_validate_stage_drain_snapshot(&entry, &selected)
            .expect("selected non-prompt batch must satisfy staging invariants");
    }

    #[test]
    fn batch_selection_surfaces_queued_input_missing_runtime_semantics() {
        let mut driver = EphemeralRuntimeDriver::new(crate::identifiers::LogicalRuntimeId::new(
            "missing-runtime-semantics-selection",
        ));
        let input = Input::Prompt(crate::input::PromptInput::new(
            "unstamped queued prompt",
            None,
        ));
        let input_id = input.id().clone();
        let mut state = InputState::new_accepted(input_id.clone());
        state.persisted_input = Some(input.clone());
        let seed = queued_seed();
        assert!(driver.ledger_mut().recover(state.clone()));
        driver
            .admit_recovered_to_ingress(
                input_id.clone(),
                ContentShape::from_kind(input.kind()),
                meerkat_core::types::HandlingMode::Queue,
                crate::ingress_types::RuntimeInputSemantics {
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                    execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
                    peer_response_terminal_apply_intent: None,
                },
                crate::input::runtime_input_projection(&input),
                true,
                &state,
                &seed,
                queue_policy(
                    crate::policy::WakeMode::WakeIfIdle,
                    crate::policy::DrainPolicy::QueueNextTurn,
                ),
                None,
                None,
            )
            .expect("recover queued prompt input");
        driver.rebuild_queue_projections_after_recovery();
        driver.clear_admitted_runtime_semantics_for_test(&input_id);

        let selected = machine_select_runtime_loop_batch(&DriverEntry::Ephemeral(driver));

        assert_eq!(
            selected,
            vec![input_id],
            "missing runtime semantics must be selected so the runtime loop records a typed failure instead of treating the queue as empty"
        );
    }

    #[test]
    fn batch_selection_surfaces_steered_input_missing_runtime_semantics() {
        let mut driver = EphemeralRuntimeDriver::new(crate::identifiers::LogicalRuntimeId::new(
            "missing-steer-runtime-semantics-selection",
        ));
        let input = Input::Continuation(
            crate::input::ContinuationInput::detached_background_op_completed(),
        );
        let input_id = input.id().clone();
        let mut state = InputState::new_accepted(input_id.clone());
        state.persisted_input = Some(input.clone());
        let seed = queued_seed();
        let mut policy = queue_policy(
            crate::policy::WakeMode::WakeIfIdle,
            crate::policy::DrainPolicy::SteerBatch,
        );
        policy.apply_mode = crate::policy::ApplyMode::StageRunBoundary;
        policy.routing_disposition = crate::policy::RoutingDisposition::Steer;

        assert!(driver.ledger_mut().recover(state.clone()));
        driver
            .admit_recovered_to_ingress(
                input_id.clone(),
                ContentShape::from_kind(input.kind()),
                meerkat_core::types::HandlingMode::Steer,
                crate::ingress_types::RuntimeInputSemantics {
                    boundary:
                        meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint,
                    execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                    peer_response_terminal_apply_intent: None,
                },
                crate::input::runtime_input_projection(&input),
                false,
                &state,
                &seed,
                policy,
                None,
                None,
            )
            .expect("recover steered continuation input");
        driver.rebuild_queue_projections_after_recovery();
        driver.clear_admitted_runtime_semantics_for_test(&input_id);

        let selected = machine_select_runtime_loop_batch(&DriverEntry::Ephemeral(driver));

        assert_eq!(
            selected,
            vec![input_id],
            "missing runtime semantics in the steer lane must be selected so the runtime loop records a typed failure instead of treating the lane as empty"
        );
    }
}

pub(crate) fn machine_prepare_destroy(
    driver: &mut DriverEntry,
) -> Result<PreparedDestroy, RuntimeDriverError> {
    match driver.runtime_state() {
        RuntimeState::Initializing
        | RuntimeState::Idle
        | RuntimeState::Attached
        | RuntimeState::Running
        | RuntimeState::Retired
        | RuntimeState::Stopped
        | RuntimeState::Destroyed => {}
    }

    Ok(driver.prepare_destroy_lifecycle())
}

pub(crate) async fn machine_commit_prepared_destroy(
    driver: &mut DriverEntry,
    lifecycle: PreparedDestroyLifecycle,
) -> Result<(), RuntimeDriverError> {
    driver.commit_prepared_destroy_lifecycle(lifecycle).await?;
    driver.sync_control_projection_from_dsl_authority();
    Ok(())
}

pub(crate) async fn machine_retire(
    driver: &mut DriverEntry,
) -> Result<RetireReport, RuntimeDriverError> {
    match driver.runtime_state() {
        RuntimeState::Idle
        | RuntimeState::Attached
        | RuntimeState::Running
        | RuntimeState::Retired => {}
        from => {
            return Err(RuntimeDriverError::Internal(
                crate::runtime_state::RuntimeStateTransitionError {
                    from,
                    to: RuntimeState::Retired,
                }
                .to_string(),
            ));
        }
    }

    match driver {
        DriverEntry::Ephemeral(d) => {
            // Retire legality and phase ownership live in the session DSL. The
            // driver projection is the concrete cache used for drain gating.
            d.set_control_projection(RuntimeState::Retired, None, None);
            Ok(d.finalize_retire())
        }
        DriverEntry::Persistent(d) => d.realize_retire_lifecycle().await,
    }
}

pub(crate) async fn machine_reset(
    driver: &mut DriverEntry,
) -> Result<ResetReport, RuntimeDriverError> {
    match driver.runtime_state() {
        RuntimeState::Initializing
        | RuntimeState::Idle
        | RuntimeState::Attached
        | RuntimeState::Retired => {}
        from => {
            return Err(RuntimeDriverError::Internal(
                crate::runtime_state::RuntimeStateTransitionError {
                    from,
                    to: RuntimeState::Idle,
                }
                .to_string(),
            ));
        }
    }

    match driver {
        DriverEntry::Ephemeral(d) => {
            // Reset is machine-owned; mirror the accepted DSL phase into the
            // concrete projection before cleanup observes the lifecycle state.
            d.set_control_projection(RuntimeState::Idle, None, None);
            Ok(d.reset_cleanup())
        }
        DriverEntry::Persistent(d) => d.realize_reset_lifecycle().await,
    }
}

pub(crate) fn machine_prepare_bindings_projection(
    driver: &mut DriverEntry,
) -> Result<(), RuntimeDriverError> {
    match driver.runtime_state() {
        RuntimeState::Initializing | RuntimeState::Idle => {
            driver.set_control_projection(RuntimeState::Attached, None, None);
            Ok(())
        }
        RuntimeState::Attached
        | RuntimeState::Running
        | RuntimeState::Retired
        | RuntimeState::Stopped => Ok(()),
        from => Err(RuntimeDriverError::Internal(
            crate::runtime_state::RuntimeStateTransitionError {
                from,
                to: RuntimeState::Attached,
            }
            .to_string(),
        )),
    }
}

pub(crate) fn machine_executor_attach_projection(
    driver: &mut DriverEntry,
) -> Result<bool, RuntimeDriverError> {
    match driver.runtime_state() {
        RuntimeState::Idle => {
            driver.set_control_projection(RuntimeState::Attached, None, None);
            Ok(true)
        }
        RuntimeState::Attached => Ok(false),
        from => Err(RuntimeDriverError::Internal(
            crate::runtime_state::RuntimeStateTransitionError {
                from,
                to: RuntimeState::Attached,
            }
            .to_string(),
        )),
    }
}

pub(crate) fn machine_unregister_session_projection(driver: &mut DriverEntry) {
    if matches!(driver.runtime_state(), RuntimeState::Attached) {
        driver.set_control_projection(RuntimeState::Idle, None, None);
    }
}

pub(crate) async fn machine_recycle_preserving_work(
    driver: &mut DriverEntry,
) -> Result<usize, RuntimeDriverError> {
    let target_phase = match driver.runtime_state() {
        RuntimeState::Idle | RuntimeState::Retired => RuntimeState::Idle,
        RuntimeState::Attached => RuntimeState::Attached,
        from => {
            return Err(RuntimeDriverError::Internal(
                crate::runtime_state::RuntimeStateTransitionError {
                    from,
                    to: RuntimeState::Idle,
                }
                .to_string(),
            ));
        }
    };

    if driver.current_run_id().is_some() {
        return Err(RuntimeDriverError::Internal(
            crate::runtime_state::RuntimeStateTransitionError {
                from: driver.runtime_state(),
                to: target_phase,
            }
            .to_string(),
        ));
    }

    driver.set_control_projection(target_phase, None, None);
    match driver {
        DriverEntry::Ephemeral(driver) => driver.recycle_preserving_work(),
        DriverEntry::Persistent(driver) => driver.recycle_preserving_work(target_phase).await,
    }
}

pub(crate) async fn prepare_runtime_loop_batch_start(
    driver: &SharedDriver,
    run_id: RunId,
    staged_ids: &[InputId],
) -> Result<(), RuntimeDriverError> {
    let mut driver = driver.lock().await;
    machine_validate_stage_drain_snapshot(&driver, staged_ids)?;
    machine_begin_run(&mut driver, run_id.clone()).map_err(|err| {
        RuntimeDriverError::Internal(format!("failed to start runtime run: {err}"))
    })?;

    if let Err(err) = driver.machine_realize_stage_batch(staged_ids, &run_id) {
        let _ = driver.rollback_staged(staged_ids);
        let next_phase =
            crate::runtime_state::run_return_phase_from_pre_run_phase(driver.pre_run_phase());
        if let Err(rollback_err) = machine_apply_run_return_projection(
            &mut driver,
            &run_id,
            RunReturnDisposition::Rollback,
            next_phase,
        ) {
            return Err(RuntimeDriverError::Internal(format!(
                "failed to roll back runtime run after batch staging failure: {rollback_err}; staging failure: {err}"
            )));
        }
        return Err(RuntimeDriverError::Internal(format!(
            "failed to stage accepted input batch: {err}"
        )));
    }

    Ok(())
}

pub(crate) async fn commit_runtime_loop_run(
    driver: &SharedDriver,
    run_id: RunId,
    consumed_input_ids: Vec<InputId>,
    receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
    session_snapshot: Option<Vec<u8>>,
) -> Result<(), RuntimeLoopRunCommitError> {
    let mut driver = driver.lock().await;
    let next_phase =
        crate::runtime_state::run_return_phase_from_pre_run_phase(driver.pre_run_phase());
    // Pick the first consumed input as the DSL `Commit { input_id, run_id }`
    // identifier. The DSL Commit transitions only read `run_id` in their
    // guards (input_id is informational), so firing once with any
    // contributing input is sufficient to flip `lifecycle_phase`.
    let commit_input_id = consumed_input_ids.first().cloned();
    machine_validate_run_commit_receipt(&driver, &run_id, &consumed_input_ids, &receipt)
        .map_err(RuntimeLoopRunCommitError::Rejected)?;
    machine_validate_active_run(&driver, &run_id, next_phase).map_err(|err| {
        RuntimeLoopRunCommitError::Rejected(RuntimeDriverError::Internal(err.to_string()))
    })?;
    let completed_run_id = run_id.clone();

    let boundary_checkpoint = driver.rollback_snapshot();
    if let Err(err) = driver
        .machine_realize_boundary_applied(run_id.clone(), receipt, session_snapshot)
        .await
    {
        driver.restore_rollback_snapshot(boundary_checkpoint);
        return Err(RuntimeLoopRunCommitError::BoundaryCommit(
            RuntimeDriverError::Internal(format!("runtime boundary commit failed: {err}")),
        ));
    }

    let terminal_checkpoint = driver.rollback_snapshot();
    if let Err(err) = machine_validate_run_completed(&driver, &consumed_input_ids) {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunCommitError::PostBoundaryValidation(
            RuntimeDriverError::Internal(format!(
                "runtime completion validation failed after boundary commit: {err}"
            )),
        ));
    }
    if let Err(err) = machine_apply_turn_run_completed(&mut driver, &completed_run_id) {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunCommitError::Rejected(err));
    }
    let disposition = match commit_input_id.as_ref() {
        Some(input_id) => RunReturnDisposition::Commit { input_id },
        None => RunReturnDisposition::Rollback,
    };
    if let Err(err) =
        machine_apply_run_return_projection(&mut driver, &completed_run_id, disposition, next_phase)
    {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunCommitError::Rejected(
            RuntimeDriverError::Internal(format!(
                "failed to apply runtime return projection after completion: {err}"
            )),
        ));
    }
    if let Err(err) = driver
        .machine_realize_run_completed(completed_run_id.clone(), consumed_input_ids)
        .await
    {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunCommitError::TerminalSnapshot(
            RuntimeDriverError::Internal(format!(
                "failed to persist runtime completion snapshot: {err}"
            )),
        ));
    }

    Ok(())
}

pub(crate) async fn fail_runtime_loop_run(
    driver: &SharedDriver,
    run_id: RunId,
    failure: CoreApplyFailureCause,
) -> Result<(), RuntimeLoopRunFailError> {
    fail_runtime_loop_run_inner(
        driver,
        run_id,
        failure.message().to_owned(),
        meerkat_core::TurnTerminalCauseKind::RuntimeApplyFailure,
        Some(failure),
    )
    .await
}

pub(crate) async fn fail_machine_run(
    driver: &SharedDriver,
    run_id: RunId,
    failure: super::MeerkatMachineRunFailure,
) -> Result<(), RuntimeLoopRunFailError> {
    fail_runtime_loop_run_inner(
        driver,
        run_id,
        failure.error,
        failure.terminal_cause_kind,
        None,
    )
    .await
}

async fn fail_runtime_loop_run_inner(
    driver: &SharedDriver,
    run_id: RunId,
    terminal_error: String,
    terminal_cause_kind: meerkat_core::TurnTerminalCauseKind,
    runtime_apply_failure: Option<CoreApplyFailureCause>,
) -> Result<(), RuntimeLoopRunFailError> {
    if !terminal_cause_kind.is_specific_failure_cause() {
        return Err(RuntimeLoopRunFailError::Rejected(
            RuntimeDriverError::Internal(
                "machine run failure has unknown machine-owned terminal_cause_kind".to_string(),
            ),
        ));
    }

    let mut driver = driver.lock().await;
    let next_phase =
        crate::runtime_state::run_return_phase_from_pre_run_phase(driver.pre_run_phase());
    let failed_run_id = run_id.clone();
    let staged_input_ids = machine_staged_contributors(&driver);
    machine_validate_run_failed(&driver, &staged_input_ids)
        .map_err(RuntimeLoopRunFailError::Rejected)?;
    let terminal_checkpoint = driver.rollback_snapshot();
    if let Err(err) = machine_apply_turn_run_failed(
        &mut driver,
        &failed_run_id,
        &terminal_error,
        terminal_cause_kind,
        runtime_apply_failure.as_ref(),
    ) {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunFailError::Rejected(err));
    }
    let replay_plan = machine_build_replay_plan(&driver, &staged_input_ids, "RunFailed");
    if let Err(err) = machine_apply_run_return_projection(
        &mut driver,
        &failed_run_id,
        RunReturnDisposition::Failed,
        next_phase,
    ) {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunFailError::Rejected(
            RuntimeDriverError::Internal(format!(
                "failed to apply runtime return projection after failure: {err}"
            )),
        ));
    }
    if let Err(run_err) = driver
        .machine_realize_run_failed(
            failed_run_id.clone(),
            staged_input_ids,
            replay_plan,
            &terminal_error,
            runtime_apply_failure.as_ref(),
            true,
        )
        .await
    {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunFailError::TerminalSnapshot(
            RuntimeDriverError::Internal(format!("failed to record run-failed event: {run_err}")),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod run_failed_cause_tests {
    use super::*;

    fn running_driver(run_id: &RunId) -> DriverEntry {
        let driver = crate::driver::ephemeral::EphemeralRuntimeDriver::new(LogicalRuntimeId::new(
            "run-failed-cause-test",
        ));
        let entry = DriverEntry::Ephemeral(driver);
        {
            let authority = entry.shared_dsl_authority();
            let mut auth = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            auth.state.lifecycle_phase = crate::meerkat_machine::dsl::MeerkatPhase::Running;
            auth.state.current_run_id =
                Some(crate::meerkat_machine::dsl::RunId::from_domain(run_id));
            auth.state.turn_phase = crate::meerkat_machine::dsl::TurnPhase::Ready;
        }
        entry
    }

    #[test]
    fn run_failed_without_runtime_apply_cause_uses_fatal_terminal_cause() {
        let run_id = RunId::new();
        let mut driver = running_driver(&run_id);

        machine_apply_turn_run_failed(
            &mut driver,
            &run_id,
            "legacy failure",
            meerkat_core::TurnTerminalCauseKind::FatalFailure,
            None,
        )
        .expect("legacy run failure should apply");

        let authority = driver.shared_dsl_authority();
        let auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            auth.state.terminal_cause_kind,
            Some(crate::meerkat_machine::dsl::TurnTerminalCauseKind::FatalFailure)
        );
        assert_eq!(auth.state.last_runtime_apply_failure_cause, None);
        assert_eq!(auth.state.last_runtime_apply_failure_message, None);
    }

    #[test]
    fn direct_run_failure_display_message_does_not_classify_terminal_cause() {
        let run_id = RunId::new();
        let mut driver = running_driver(&run_id);

        machine_apply_turn_run_failed(
            &mut driver,
            &run_id,
            "runtime apply failure: display-only text",
            meerkat_core::TurnTerminalCauseKind::FatalFailure,
            None,
        )
        .expect("legacy run failure should apply");

        let authority = driver.shared_dsl_authority();
        let auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            auth.state.terminal_cause_kind,
            Some(crate::meerkat_machine::dsl::TurnTerminalCauseKind::FatalFailure)
        );
        assert_eq!(auth.state.last_runtime_apply_failure_cause, None);
        assert_eq!(auth.state.last_runtime_apply_failure_message, None);
    }

    #[test]
    fn run_failed_with_runtime_apply_cause_uses_runtime_apply_terminal_cause() {
        let run_id = RunId::new();
        let mut driver = running_driver(&run_id);
        let failure = CoreApplyFailureCause::runtime_turn("runtime apply failed");

        machine_apply_turn_run_failed(
            &mut driver,
            &run_id,
            failure.message(),
            meerkat_core::TurnTerminalCauseKind::RuntimeApplyFailure,
            Some(&failure),
        )
        .expect("runtime apply failure should apply");

        let authority = driver.shared_dsl_authority();
        let auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            auth.state.terminal_cause_kind,
            Some(crate::meerkat_machine::dsl::TurnTerminalCauseKind::RuntimeApplyFailure)
        );
        assert_eq!(
            auth.state.last_runtime_apply_failure_cause,
            Some(crate::meerkat_machine::dsl::RuntimeApplyFailureCause::RuntimeTurn)
        );
        assert_eq!(
            auth.state.last_runtime_apply_failure_message.as_deref(),
            Some("runtime apply failed")
        );
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::policy::{
        ApplyMode, ConsumePoint, DrainPolicy, QueueMode, RoutingDisposition, WakeMode,
    };

    fn policy(apply_mode: ApplyMode) -> crate::policy::PolicyDecision {
        crate::policy::PolicyDecision {
            apply_mode,
            wake_mode: WakeMode::WakeIfIdle,
            queue_mode: QueueMode::Fifo,
            consume_point: ConsumePoint::OnRunComplete,
            drain_policy: DrainPolicy::QueueNextTurn,
            routing_disposition: RoutingDisposition::Queue,
            record_transcript: true,
            emit_operator_content: true,
            policy_version: crate::policy_table::DEFAULT_POLICY_VERSION,
        }
    }

    fn state_with_runtime_semantics(
        input: Input,
        decision: crate::policy::PolicyDecision,
        runtime_semantics: crate::ingress_types::RuntimeInputSemantics,
    ) -> crate::input_state::InputState {
        let mut state = crate::input_state::InputState::new_accepted(input.id().clone());
        state.persisted_input = Some(input);
        state.policy = Some(crate::input_state::PolicySnapshot {
            version: crate::policy_table::DEFAULT_POLICY_VERSION,
            decision,
        });
        state.runtime_semantics = Some(runtime_semantics);
        state
    }

    #[test]
    fn recovered_ingress_entry_requires_persisted_input_for_content_shape() {
        let mut state = crate::input_state::InputState::new_accepted(InputId::new());
        state.policy = Some(crate::input_state::PolicySnapshot {
            version: crate::policy_table::DEFAULT_POLICY_VERSION,
            decision: policy(ApplyMode::StageRunStart),
        });

        assert!(
            machine_build_recovered_ingress_entry(&state).is_none(),
            "recovery must not synthesize an unknown admitted-input content shape"
        );
    }

    #[test]
    fn recovered_ingress_entry_requires_runtime_semantics_stamp() {
        let input = Input::Prompt(crate::input::PromptInput::new("queued prompt", None));
        let mut state = crate::input_state::InputState::new_accepted(input.id().clone());
        state.persisted_input = Some(input);
        state.policy = Some(crate::input_state::PolicySnapshot {
            version: crate::policy_table::DEFAULT_POLICY_VERSION,
            decision: policy(ApplyMode::StageRunStart),
        });

        assert!(
            machine_build_recovered_ingress_entry(&state).is_none(),
            "recovery must not derive execution kind from payload/policy when the durable runtime semantics stamp is missing"
        );
    }

    #[test]
    fn recovered_ingress_entry_requires_policy_snapshot() {
        let input = Input::Prompt(crate::input::PromptInput::new("queued prompt", None));
        let decision = policy(ApplyMode::StageRunStart);
        let mut state = crate::input_state::InputState::new_accepted(input.id().clone());
        state.runtime_semantics = Some(
            crate::ingress_types::RuntimeInputSemantics::from_policy_and_kind(
                &decision,
                input.kind(),
            ),
        );
        state.persisted_input = Some(input);

        assert!(
            machine_build_recovered_ingress_entry(&state).is_none(),
            "recovery must not derive policy or handling mode when the durable policy stamp is missing"
        );
    }

    #[test]
    fn recovered_ingress_entry_rejects_prompt_stamped_as_resume_pending() {
        let input = Input::Prompt(crate::input::PromptInput::new("queued prompt", None));
        let decision = policy(ApplyMode::StageRunStart);
        let mut runtime_semantics =
            crate::ingress_types::RuntimeInputSemantics::from_policy_and_kind(
                &decision,
                input.kind(),
            );
        runtime_semantics.execution_kind =
            meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending;
        let state = state_with_runtime_semantics(input, decision, runtime_semantics);

        assert!(
            machine_build_recovered_ingress_entry(&state).is_none(),
            "recovery must reject a prompt row whose durable stamp says ResumePending"
        );
    }

    #[test]
    fn recovered_ingress_entry_rejects_continuation_stamped_as_content_turn() {
        let input = Input::Continuation(
            crate::input::ContinuationInput::detached_background_op_completed(),
        );
        let decision = policy(ApplyMode::StageRunBoundary);
        let mut runtime_semantics =
            crate::ingress_types::RuntimeInputSemantics::from_policy_and_kind(
                &decision,
                input.kind(),
            );
        runtime_semantics.execution_kind =
            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn;
        let state = state_with_runtime_semantics(input, decision, runtime_semantics);

        assert!(
            machine_build_recovered_ingress_entry(&state).is_none(),
            "recovery must reject a continuation row whose durable stamp says ContentTurn"
        );
    }

    #[test]
    fn recovered_ingress_entry_rejects_boundary_mismatch() {
        let input = Input::Prompt(crate::input::PromptInput::new("queued prompt", None));
        let decision = policy(ApplyMode::StageRunStart);
        let mut runtime_semantics =
            crate::ingress_types::RuntimeInputSemantics::from_policy_and_kind(
                &decision,
                input.kind(),
            );
        runtime_semantics.boundary =
            meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint;
        let state = state_with_runtime_semantics(input, decision, runtime_semantics);

        assert!(
            machine_build_recovered_ingress_entry(&state).is_none(),
            "recovery must reject a durable stamp whose boundary disagrees with policy"
        );
    }

    #[test]
    fn recovered_ingress_entry_rejects_terminal_intent_mismatch() {
        let input = crate::input::peer_response_terminal_input(
            meerkat_core::comms::PeerId::new(),
            Some(meerkat_core::comms::PeerName::new("reviewer").expect("peer name")),
            meerkat_core::PeerCorrelationId::new(),
            meerkat_contracts::PeerResponseTerminalStatusWire::Completed,
            serde_json::json!({"status": "complete"}),
        );
        let decision = policy(ApplyMode::StageRunStart);
        let mut runtime_semantics =
            crate::ingress_types::RuntimeInputSemantics::from_policy_and_kind(
                &decision,
                input.kind(),
            );
        runtime_semantics.peer_response_terminal_apply_intent = None;
        let state = state_with_runtime_semantics(input, decision, runtime_semantics);

        assert!(
            machine_build_recovered_ingress_entry(&state).is_none(),
            "recovery must reject a terminal peer-response stamp with missing terminal apply intent"
        );
    }

    #[tokio::test]
    async fn persistent_recovery_rejects_state_without_runtime_semantics_stamp() {
        use crate::store::RuntimeStore;

        let runtime_id = LogicalRuntimeId::new("missing-runtime-semantics-stamp");
        let input = Input::Prompt(crate::input::PromptInput::new("queued prompt", None));
        let input_id = input.id().clone();
        let mut state = crate::input_state::InputState::new_accepted(input_id.clone());
        state.persisted_input = Some(input);
        state.policy = Some(crate::input_state::PolicySnapshot {
            version: crate::policy_table::DEFAULT_POLICY_VERSION,
            decision: policy(ApplyMode::StageRunStart),
        });
        let mut seed = InputStateSeed::new_accepted();
        seed.phase = InputLifecycleState::Queued;
        let bundle = crate::input_state::StoredInputState { state, seed };
        let store = crate::store::memory::InMemoryRuntimeStore::new();
        store
            .persist_input_state(&runtime_id, &bundle)
            .await
            .expect("persist corrupt recovered input state");

        let mut driver = crate::driver::ephemeral::EphemeralRuntimeDriver::new(runtime_id.clone());
        let err = machine_recover_persistent_driver(&store, &runtime_id, &mut driver)
            .await
            .expect_err("unstamped recovered input must not recover through local classification");

        assert!(
            err.to_string()
                .contains("missing runtime execution semantics stamp"),
            "unexpected recovery error: {err}"
        );
        assert!(
            driver.input_state(&input_id).is_none(),
            "failed recovery must not leave a ledger-only input row"
        );
    }

    #[tokio::test]
    async fn persistent_recovery_rejects_state_without_policy_snapshot() {
        use crate::store::RuntimeStore;

        let runtime_id = LogicalRuntimeId::new("missing-policy-snapshot");
        let input = Input::Prompt(crate::input::PromptInput::new("queued prompt", None));
        let input_id = input.id().clone();
        let decision = policy(ApplyMode::StageRunStart);
        let mut state = crate::input_state::InputState::new_accepted(input_id.clone());
        state.runtime_semantics = Some(
            crate::ingress_types::RuntimeInputSemantics::from_policy_and_kind(
                &decision,
                input.kind(),
            ),
        );
        state.persisted_input = Some(input);
        let mut seed = InputStateSeed::new_accepted();
        seed.phase = InputLifecycleState::Queued;
        let bundle = crate::input_state::StoredInputState { state, seed };
        let store = crate::store::memory::InMemoryRuntimeStore::new();
        store
            .persist_input_state(&runtime_id, &bundle)
            .await
            .expect("persist corrupt recovered input state");

        let mut driver = crate::driver::ephemeral::EphemeralRuntimeDriver::new(runtime_id.clone());
        let err = machine_recover_persistent_driver(&store, &runtime_id, &mut driver)
            .await
            .expect_err(
                "policy-less recovered input must not recover through local classification",
            );

        assert!(
            err.to_string()
                .contains("missing runtime admission policy stamp"),
            "unexpected recovery error: {err}"
        );
        assert!(
            driver.input_state(&input_id).is_none(),
            "failed recovery must not leave a ledger-only input row"
        );
    }

    #[tokio::test]
    async fn persistent_recovery_rejects_state_without_persisted_input_content_shape() {
        use crate::store::RuntimeStore;

        let runtime_id = LogicalRuntimeId::new("missing-persisted-input-content-shape");
        let input_id = InputId::new();
        let store = crate::store::memory::InMemoryRuntimeStore::new();
        let bundle = crate::input_state::StoredInputState::new_accepted(input_id.clone());
        store
            .persist_input_state(&runtime_id, &bundle)
            .await
            .expect("persist corrupt recovered input state");

        let mut driver = crate::driver::ephemeral::EphemeralRuntimeDriver::new(runtime_id.clone());
        let err = machine_recover_persistent_driver(&store, &runtime_id, &mut driver)
            .await
            .expect_err("missing persisted input must not recover as a ledger-only row");

        assert!(
            err.to_string()
                .contains("cannot derive admitted-input content shape"),
            "unexpected recovery error: {err}"
        );
        assert!(
            driver.input_state(&input_id).is_none(),
            "failed recovery must not leave a ledger-only input row"
        );
    }

    #[tokio::test]
    async fn ephemeral_recovery_rejects_state_without_persisted_input_content_shape() {
        let runtime_id = LogicalRuntimeId::new("ephemeral-missing-persisted-content-shape");
        let mut driver = crate::driver::ephemeral::EphemeralRuntimeDriver::new(runtime_id);
        let input = Input::Prompt(crate::input::PromptInput::new("queued prompt", None));
        let input_id = input.id().clone();

        driver.accept_input(input).await.expect("accept input");
        driver
            .ledger_mut()
            .get_mut(&input_id)
            .expect("accepted input ledger state")
            .persisted_input = None;

        let err = machine_recover_ephemeral_driver(&mut driver)
            .expect_err("missing persisted input must not be silently skipped");

        assert!(
            err.to_string()
                .contains("cannot derive admitted-input content shape"),
            "unexpected recovery error: {err}"
        );
    }
}
