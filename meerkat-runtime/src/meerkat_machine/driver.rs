//! Runtime driver entry point and lifecycle.

use std::sync::Arc;

use crate::runtime_ingress_authority::RuntimeIngressMutator;
use meerkat_core::lifecycle::{InputId, RunId};

use crate::driver::ephemeral::EphemeralRuntimeDriver;
use crate::driver::persistent::PersistentRuntimeDriver;
use crate::identifiers::LogicalRuntimeId;
use crate::input_lifecycle_authority::InputLifecycleInput;
use crate::input_state::{
    InputLifecycleState, InputState, InputStateHistoryEntry, InputTerminalOutcome,
};
use crate::runtime_state::RuntimeState;
use crate::tokio::sync::Mutex;
use crate::traits::{
    DestroyReport, RecoveryReport, ResetReport, RetireReport, RuntimeDriver, RuntimeDriverError,
};
use chrono::Utc;

/// Shared driver handle used by both the adapter and the RuntimeLoop.
pub(crate) type SharedDriver = Arc<Mutex<DriverEntry>>;

/// Per-session runtime driver entry.
pub(crate) enum DriverEntry {
    Ephemeral(EphemeralRuntimeDriver),
    Persistent(PersistentRuntimeDriver),
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

    /// Check if the runtime can process queued inputs (Idle, Attached, or Retired).
    pub(crate) fn can_process_queue(&self) -> bool {
        match self {
            DriverEntry::Ephemeral(d) => d.can_process_queue(),
            DriverEntry::Persistent(d) => d.inner_ref().can_process_queue(),
        }
    }

    /// Inspect the current typed post-admission signal without draining it.
    pub(crate) fn post_admission_signal(&self) -> crate::driver::ephemeral::PostAdmissionSignal {
        match self {
            DriverEntry::Ephemeral(d) => d.post_admission_signal(),
            DriverEntry::Persistent(d) => d.post_admission_signal(),
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

    /// Get a reference to the ingress authority.
    pub(crate) fn ingress(&self) -> &crate::runtime_ingress_authority::RuntimeIngressAuthority {
        match self {
            DriverEntry::Ephemeral(d) => d.ingress(),
            DriverEntry::Persistent(d) => d.inner_ref().ingress(),
        }
    }

    pub(crate) fn runtime_state(&self) -> crate::runtime_state::RuntimeState {
        self.control_projection_handle()
            .read()
            .map(|guard| guard.phase)
            .unwrap_or_else(|poisoned| {
                tracing::error!("runtime control projection lock poisoned");
                poisoned.into_inner().phase
            })
    }

    pub(crate) fn current_run_id(&self) -> Option<RunId> {
        self.control_projection_handle()
            .read()
            .map(|guard| guard.current_run_id.clone())
            .unwrap_or_else(|poisoned| {
                tracing::error!("runtime control projection lock poisoned");
                poisoned.into_inner().current_run_id.clone()
            })
    }

    pub(crate) fn pre_run_phase(&self) -> Option<crate::runtime_state::RuntimeState> {
        self.control_projection_handle()
            .read()
            .map(|guard| guard.pre_run_phase)
            .unwrap_or_else(|poisoned| {
                tracing::error!("runtime control projection lock poisoned");
                poisoned.into_inner().pre_run_phase
            })
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

    pub(crate) fn has_queued_input_outside(&self, excluded: &[InputId]) -> bool {
        match self {
            DriverEntry::Ephemeral(d) => d.has_queued_input_outside(excluded),
            DriverEntry::Persistent(d) => d.has_queued_input_outside(excluded),
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
        error: String,
        recoverable: bool,
    ) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => {
                let _ = (error, recoverable);
                d.machine_realize_run_failed(&run_id, &contributing_input_ids, &replay_plan)
            }
            DriverEntry::Persistent(d) => {
                d.machine_realize_run_failed(
                    &run_id,
                    &contributing_input_ids,
                    &replay_plan,
                    &error,
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
    ) -> Result<(), crate::input_lifecycle_authority::InputLifecycleError> {
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
    ) -> Result<(), crate::input_lifecycle_authority::InputLifecycleError> {
        match self {
            DriverEntry::Ephemeral(d) => d.machine_realize_stage_batch(input_ids, run_id),
            DriverEntry::Persistent(d) => d.machine_realize_stage_batch(input_ids, run_id),
        }
    }

    /// Roll back staged inputs after a failed staging attempt.
    pub(crate) fn rollback_staged(
        &mut self,
        input_ids: &[InputId],
    ) -> Result<(), crate::input_lifecycle_authority::InputLifecycleError> {
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

    pub(crate) async fn finalize_retire(&mut self) -> Result<RetireReport, RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => Ok(d.finalize_retire()),
            DriverEntry::Persistent(d) => d.finalize_retire().await,
        }
    }

    pub(crate) async fn finalize_reset(&mut self) -> Result<ResetReport, RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => Ok(d.reset_cleanup()),
            DriverEntry::Persistent(d) => d.finalize_reset().await,
        }
    }

    pub(crate) async fn destroy(&mut self) -> Result<DestroyReport, RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => {
                let abandoned = d.destroy_cleanup();
                Ok(DestroyReport {
                    inputs_abandoned: abandoned,
                })
            }
            DriverEntry::Persistent(d) => d.destroy().await,
        }
    }

    pub(crate) async fn finalize_stop_runtime(&mut self) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => {
                d.finalize_stop_runtime();
                Ok(())
            }
            DriverEntry::Persistent(d) => d.finalize_stop_runtime().await,
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
    let pre_run_phase = crate::runtime_state::run_start_pre_phase_from_phase(from)?;
    if driver.current_run_id().is_some() {
        return Err(crate::runtime_state::RuntimeStateTransitionError {
            from,
            to: RuntimeState::Running,
        });
    }
    driver.set_control_projection(RuntimeState::Running, Some(run_id), Some(pre_run_phase));
    Ok(())
}

pub(crate) fn machine_apply_run_return_projection(
    driver: &mut DriverEntry,
    run_id: &RunId,
    next_phase: RuntimeState,
) -> Result<(), crate::runtime_state::RuntimeStateTransitionError> {
    machine_validate_active_run(driver, run_id, next_phase)?;
    let current_phase = driver.runtime_state();
    if matches!(
        current_phase,
        RuntimeState::Retired | RuntimeState::Stopped | RuntimeState::Destroyed
    ) {
        return Ok(());
    }
    driver.set_control_projection(next_phase, None, None);
    Ok(())
}

pub(crate) fn slice_starts_with(seq: &[InputId], prefix: &[InputId]) -> bool {
    prefix.len() <= seq.len() && seq[..prefix.len()] == *prefix
}

pub(crate) fn machine_input_boundary(
    driver: &DriverEntry,
    work_id: &InputId,
) -> meerkat_core::lifecycle::run_primitive::RunApplyBoundary {
    match driver.ingress().handling_mode(work_id) {
        Some(meerkat_core::types::HandlingMode::Steer) => {
            meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint
        }
        _ => meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
    }
}

pub(crate) fn machine_select_runtime_loop_batch(driver: &DriverEntry) -> Vec<InputId> {
    let ingress = driver.ingress();
    if !ingress.steer_queue().is_empty() {
        let first = &ingress.steer_queue()[0];
        let target_boundary = machine_input_boundary(driver, first);
        return ingress
            .steer_queue()
            .iter()
            .take_while(|id| machine_input_boundary(driver, id) == target_boundary)
            .cloned()
            .collect();
    }

    if let Some(first) = ingress.queue().first() {
        if ingress.is_prompt(first) {
            return vec![first.clone()];
        }
        return ingress
            .queue()
            .iter()
            .take_while(|id| !ingress.is_prompt(id))
            .cloned()
            .collect();
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
        let lifecycle = driver
            .as_driver()
            .input_state(work_id)
            .map(InputState::current_state);
        if lifecycle != Some(InputLifecycleState::Queued) {
            return Err(RuntimeDriverError::Internal(format!(
                "stage drain snapshot requires queued contributors, but {work_id:?} is {lifecycle:?}"
            )));
        }
    }

    let ingress = driver.ingress();
    let source_queue = if ingress.steer_queue().is_empty() {
        ingress.queue()
    } else {
        ingress.steer_queue()
    };
    if !slice_starts_with(source_queue, contributing_work_ids) {
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
        let lifecycle = driver
            .as_driver()
            .input_state(work_id)
            .map(InputState::current_state);
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
        let lifecycle = driver
            .as_driver()
            .input_state(work_id)
            .map(InputState::current_state);
        if lifecycle != Some(InputLifecycleState::AppliedPendingConsumption) {
            return Err(RuntimeDriverError::Internal(format!(
                "run completed requires contributors pending consumption, but {work_id:?} is {lifecycle:?}"
            )));
        }
    }

    Ok(())
}

pub(crate) fn machine_staged_contributors(driver: &DriverEntry) -> Vec<InputId> {
    driver
        .as_driver()
        .active_input_ids()
        .into_iter()
        .filter(|work_id| {
            driver
                .as_driver()
                .input_state(work_id)
                .map(InputState::current_state)
                == Some(InputLifecycleState::Staged)
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
        let lifecycle = driver
            .as_driver()
            .input_state(work_id)
            .map(InputState::current_state);
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
    mut state: InputState,
) -> Result<InputState, RuntimeDriverError> {
    let applied_boundary_committed = if matches!(
        state.current_state(),
        InputLifecycleState::Applied | InputLifecycleState::AppliedPendingConsumption
    ) {
        Some(
            match (state.last_run_id().cloned(), state.last_boundary_sequence()) {
                (Some(run_id), Some(sequence)) => store
                    .load_boundary_receipt(runtime_id, &run_id, sequence)
                    .await
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?
                    .is_some(),
                _ => false,
            },
        )
    } else {
        None
    };

    let _ = machine_apply_recovered_input_normalization(&mut state, applied_boundary_committed);

    Ok(state)
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct MachineRecoveryDelta {
    pub recovered: usize,
    pub abandoned: usize,
    pub requeued: usize,
}

pub(crate) fn machine_apply_recovered_input_normalization(
    state: &mut InputState,
    applied_boundary_committed: Option<bool>,
) -> MachineRecoveryDelta {
    let mut delta = MachineRecoveryDelta::default();

    match state.current_state() {
        InputLifecycleState::Accepted => {
            let consume_on_accept = state
                .policy
                .as_ref()
                .map(|policy| {
                    policy.decision.apply_mode == crate::policy::ApplyMode::Ignore
                        && policy.decision.consume_point == crate::policy::ConsumePoint::OnAccept
                })
                .unwrap_or(false);
            if consume_on_accept {
                let _ = state.apply(InputLifecycleInput::ConsumeOnAccept);
                delta.abandoned += 1;
            } else {
                let _ = state.apply(InputLifecycleInput::QueueAccepted);
                delta.requeued += 1;
            }
            delta.recovered += 1;
        }
        InputLifecycleState::Staged => {
            let _ = state.apply(InputLifecycleInput::RollbackStaged);
            delta.requeued += 1;
            delta.recovered += 1;
        }
        InputLifecycleState::Applied | InputLifecycleState::AppliedPendingConsumption => {
            if let Some(has_receipt) = applied_boundary_committed {
                let now = Utc::now();
                let from = state.current_state();
                let auth = crate::input_lifecycle_authority::InputLifecycleAuthority::restore(
                    if has_receipt {
                        InputLifecycleState::Consumed
                    } else {
                        InputLifecycleState::Queued
                    },
                    if has_receipt {
                        Some(InputTerminalOutcome::Consumed)
                    } else {
                        None
                    },
                    state.last_run_id().cloned(),
                    state.last_boundary_sequence(),
                    state.attempt_count(),
                    {
                        let mut h = state.history().to_vec();
                        h.push(InputStateHistoryEntry {
                            timestamp: now,
                            from,
                            to: if has_receipt {
                                InputLifecycleState::Consumed
                            } else {
                                InputLifecycleState::Queued
                            },
                            reason: Some(if has_receipt {
                                "recovery: boundary receipt already committed".into()
                            } else {
                                "recovery: missing boundary receipt".into()
                            }),
                        });
                        h
                    },
                    now,
                );
                *state.authority_mut() = auth;
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
    pub content_shape: crate::runtime_ingress_authority::ContentShape,
    pub handling_mode: meerkat_core::types::HandlingMode,
    pub is_prompt: bool,
    pub lifecycle_state: InputLifecycleState,
    pub policy: crate::policy::PolicyDecision,
}

pub(crate) fn machine_build_recovered_ingress_entry(
    state: &InputState,
) -> Option<RecoveredIngressEntry> {
    let handling_mode = state
        .policy
        .as_ref()
        .map(|policy| crate::accept::handling_mode_from_policy(&policy.decision))
        .unwrap_or(meerkat_core::types::HandlingMode::Queue);
    let content_shape = state
        .persisted_input
        .as_ref()
        .map(|input| crate::runtime_ingress_authority::ContentShape(input.kind_id().to_string()))
        .unwrap_or_else(|| crate::runtime_ingress_authority::ContentShape("unknown".into()));
    let policy = match state.policy.as_ref() {
        Some(policy) => policy.decision.clone(),
        None => match state.persisted_input.as_ref() {
            Some(input) => crate::policy_table::DefaultPolicyTable::resolve(input, true),
            None => return None,
        },
    };

    Some(RecoveredIngressEntry {
        content_shape,
        handling_mode,
        is_prompt: matches!(
            state.persisted_input.as_ref(),
            Some(crate::input::Input::Prompt(_))
        ),
        lifecycle_state: state.current_state(),
        policy,
    })
}

pub(crate) fn machine_realize_recovered_runtime_state(
    driver: &mut crate::driver::ephemeral::EphemeralRuntimeDriver,
    runtime_state: RuntimeState,
) {
    match runtime_state {
        RuntimeState::Retired if driver.runtime_state() != RuntimeState::Retired => {
            driver.set_control_projection(RuntimeState::Retired, None, None);
        }
        RuntimeState::Stopped
            if driver.runtime_state() != RuntimeState::Stopped
                && driver.runtime_state() != RuntimeState::Destroyed =>
        {
            driver.set_control_projection(RuntimeState::Stopped, None, None);
            driver.stop_runtime_cleanup();
        }
        RuntimeState::Destroyed if driver.runtime_state() != RuntimeState::Destroyed => {
            driver.set_control_projection(RuntimeState::Destroyed, None, None);
            driver.destroy_cleanup();
        }
        _ => {}
    }
}

pub(crate) fn machine_recover_ephemeral_driver(
    driver: &mut crate::driver::ephemeral::EphemeralRuntimeDriver,
) -> Result<RecoveryReport, RuntimeDriverError> {
    let mut recovered = 0;
    let mut abandoned = 0;
    let mut requeued = 0;

    let active_ids: Vec<InputId> = driver
        .ledger()
        .iter_non_terminal()
        .map(|(input_id, _)| input_id.clone())
        .collect();

    for input_id in &active_ids {
        let Some(state) = driver.ledger_mut().get_mut(input_id) else {
            continue;
        };
        let delta = machine_apply_recovered_input_normalization(state, None);
        recovered += delta.recovered;
        abandoned += delta.abandoned;
        requeued += delta.requeued;
    }

    let mut rebuilt_ingress = crate::runtime_ingress_authority::RuntimeIngressAuthority::new();
    let recovered_entries: Vec<_> = driver
        .ledger()
        .iter()
        .filter_map(|(input_id, state)| {
            machine_build_recovered_ingress_entry(state).map(|entry| {
                (
                    input_id.clone(),
                    entry.content_shape,
                    entry.handling_mode,
                    entry.is_prompt,
                    entry.lifecycle_state,
                    entry.policy,
                )
            })
        })
        .collect();

    for (input_id, content_shape, handling_mode, is_prompt, lifecycle_state, policy) in
        recovered_entries
    {
        rebuilt_ingress
            .apply(
                crate::runtime_ingress_authority::RuntimeIngressInput::AdmitRecovered {
                    work_id: input_id.clone(),
                    content_shape,
                    handling_mode,
                    is_prompt,
                    lifecycle_state,
                    policy,
                    request_id: None,
                    reservation_key: None,
                },
            )
            .map_err(|err| {
                RuntimeDriverError::Internal(format!(
                    "ingress authority rejected rebuilt recovered input '{input_id}': {err}"
                ))
            })?;
    }

    driver.replace_ingress(rebuilt_ingress);
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
    let stored_states = store
        .load_input_states(runtime_id)
        .await
        .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;

    let mut recovered_payloads = Vec::new();

    for state in stored_states {
        let state = machine_normalize_recovered_input_state(store, runtime_id, state).await?;

        if driver.input_state(&state.input_id).is_none() {
            let Some(entry) = machine_build_recovered_ingress_entry(&state) else {
                driver.ledger_mut().recover(state);
                continue;
            };

            let inserted = driver.ledger_mut().recover(state.clone());
            if !inserted {
                continue;
            }

            if let Some(input) = state.persisted_input.clone() {
                recovered_payloads.push((state.input_id.clone(), input));
            }

            driver.admit_recovered_to_ingress(
                state.input_id.clone(),
                entry.content_shape,
                entry.handling_mode,
                entry.is_prompt,
                state.current_state(),
                entry.policy,
                None,
                None,
            )?;
        }
    }

    let report = machine_recover_ephemeral_driver(driver)?;

    for (input_id, _input) in recovered_payloads {
        let should_requeue = driver.input_state(&input_id).is_some_and(|state| {
            state.current_state() == crate::input_state::InputLifecycleState::Queued
        });
        if should_requeue && !driver.has_queued_input(&input_id) {
            return Err(RuntimeDriverError::Internal(format!(
                "persistent recover left queued input '{input_id}' out of the runtime queue projection"
            )));
        }
    }

    if let Some(runtime_state) = store
        .load_runtime_state(runtime_id)
        .await
        .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?
    {
        machine_realize_recovered_runtime_state(driver, runtime_state);

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
        match driver.ingress().handling_mode(work_id) {
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
        | RuntimeState::Retired => {}
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

    driver.set_control_projection(RuntimeState::Stopped, None, None);
    driver.finalize_stop_runtime().await
}

pub(crate) async fn machine_destroy(
    driver: &mut DriverEntry,
) -> Result<DestroyReport, RuntimeDriverError> {
    match driver.runtime_state() {
        RuntimeState::Initializing | RuntimeState::Destroyed => {
            return Err(RuntimeDriverError::Internal(
                crate::runtime_state::RuntimeStateTransitionError {
                    from: driver.runtime_state(),
                    to: RuntimeState::Destroyed,
                }
                .to_string(),
            ));
        }
        RuntimeState::Idle
        | RuntimeState::Attached
        | RuntimeState::Running
        | RuntimeState::Retired
        | RuntimeState::Stopped => {}
    }

    driver.set_control_projection(RuntimeState::Destroyed, None, None);
    driver.destroy().await
}

pub(crate) async fn machine_retire(
    driver: &mut DriverEntry,
) -> Result<RetireReport, RuntimeDriverError> {
    match driver.runtime_state() {
        RuntimeState::Idle | RuntimeState::Attached | RuntimeState::Running => {}
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

    let current_run_id = driver.current_run_id();
    let pre_run_phase = driver.pre_run_phase();
    driver.set_control_projection(RuntimeState::Retired, current_run_id, pre_run_phase);
    driver.finalize_retire().await
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

    driver.set_control_projection(RuntimeState::Idle, None, None);
    driver.finalize_reset().await
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
        let _ = machine_apply_run_return_projection(&mut driver, &run_id, next_phase);
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
) -> Result<(), RuntimeDriverError> {
    let mut driver = driver.lock().await;
    let next_phase =
        crate::runtime_state::run_return_phase_from_pre_run_phase(driver.pre_run_phase());
    machine_validate_boundary_applied(&driver, &receipt.contributing_input_ids)?;
    if let Err(err) = driver
        .machine_realize_boundary_applied(run_id.clone(), receipt, session_snapshot)
        .await
    {
        let unwind_run_id = run_id.clone();
        let staged_input_ids = machine_staged_contributors(&driver);
        machine_validate_run_failed(&driver, &staged_input_ids)?;
        let replay_plan = machine_build_replay_plan(&driver, &staged_input_ids, "RunFailed");
        if let Err(unwind_err) = driver
            .machine_realize_run_failed(
                unwind_run_id.clone(),
                staged_input_ids,
                replay_plan,
                format!("boundary commit failed: {err}"),
                true,
            )
            .await
        {
            return Err(RuntimeDriverError::Internal(format!(
                "runtime boundary commit failed: {err}; additionally failed to unwind runtime state: {unwind_err}"
            )));
        }
        if let Err(unwind_err) =
            machine_apply_run_return_projection(&mut driver, &unwind_run_id, next_phase)
        {
            return Err(RuntimeDriverError::Internal(format!(
                "runtime boundary commit failed: {err}; additionally failed to unwind runtime state: {unwind_err}"
            )));
        }
        return Err(RuntimeDriverError::Internal(format!(
            "runtime boundary commit failed: {err}"
        )));
    }

    let completed_run_id = run_id.clone();
    machine_validate_run_completed(&driver, &consumed_input_ids)?;
    driver
        .machine_realize_run_completed(completed_run_id.clone(), consumed_input_ids)
        .await
        .map_err(|err| {
            RuntimeDriverError::Internal(format!(
                "failed to persist runtime completion snapshot: {err}"
            ))
        })?;
    machine_apply_run_return_projection(&mut driver, &completed_run_id, next_phase).map_err(
        |err| {
            RuntimeDriverError::Internal(format!(
                "failed to apply runtime return projection after completion: {err}"
            ))
        },
    )?;

    Ok(())
}

pub(crate) async fn fail_runtime_loop_run(
    driver: &SharedDriver,
    run_id: RunId,
    error: String,
) -> Result<(), RuntimeDriverError> {
    let mut driver = driver.lock().await;
    let next_phase =
        crate::runtime_state::run_return_phase_from_pre_run_phase(driver.pre_run_phase());
    let failed_run_id = run_id.clone();
    let staged_input_ids = machine_staged_contributors(&driver);
    machine_validate_run_failed(&driver, &staged_input_ids)?;
    let replay_plan = machine_build_replay_plan(&driver, &staged_input_ids, "RunFailed");
    driver
        .machine_realize_run_failed(
            failed_run_id.clone(),
            staged_input_ids,
            replay_plan,
            error,
            true,
        )
        .await
        .map_err(|run_err| {
            RuntimeDriverError::Internal(format!("failed to record run-failed event: {run_err}"))
        })?;
    machine_apply_run_return_projection(&mut driver, &failed_run_id, next_phase).map_err(|err| {
        RuntimeDriverError::Internal(format!(
            "failed to apply runtime return projection after failure: {err}"
        ))
    })
}
