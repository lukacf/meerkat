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

use chrono::Utc;
use meerkat_core::BlobStore;
use meerkat_core::comms_drain_lifecycle_authority::{
    CommsDrainLifecycleAuthority, CommsDrainLifecycleEffect, CommsDrainMode, DrainExitReason,
};
use meerkat_core::generated::{protocol_comms_drain_abort, protocol_comms_drain_spawn};
use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::{HandlingMode, SessionId};
use meerkat_core::{
    SessionToolVisibilityState, ToolFilter, ToolScopeApplyError, ToolScopeRevision,
    ToolScopeStageError, ToolVisibilityOwner, ToolVisibilityWitness,
};

use crate::accept::AcceptOutcome;
use crate::driver::ephemeral::EphemeralRuntimeDriver;
use crate::driver::persistent::PersistentRuntimeDriver;
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_lifecycle_authority::{InputLifecycleError, InputLifecycleInput};
use crate::input_state::{
    InputLifecycleState, InputState, InputStateHistoryEntry, InputTerminalOutcome,
};
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
use crate::policy::PolicyDecision;
use crate::runtime_ingress_authority::{ContentShape, RuntimeIngressMutator};
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
    ) -> Result<(), InputLifecycleError> {
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
    ) -> Result<(), InputLifecycleError> {
        match self {
            DriverEntry::Ephemeral(d) => d.machine_realize_stage_batch(input_ids, run_id),
            DriverEntry::Persistent(d) => d.machine_realize_stage_batch(input_ids, run_id),
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

fn machine_validate_active_run(
    driver: &DriverEntry,
    run_id: &RunId,
    next_phase: RuntimeState,
) -> Result<(), RuntimeStateTransitionError> {
    match driver.runtime_state() {
        RuntimeState::Running | RuntimeState::Retired => {}
        from => {
            return Err(RuntimeStateTransitionError {
                from,
                to: next_phase,
            });
        }
    }

    match driver.current_run_id() {
        Some(active_id) if &active_id == run_id => Ok(()),
        _ => Err(RuntimeStateTransitionError {
            from: driver.runtime_state(),
            to: next_phase,
        }),
    }
}

fn machine_begin_run(
    driver: &mut DriverEntry,
    run_id: RunId,
) -> Result<(), RuntimeStateTransitionError> {
    let from = driver.runtime_state();
    let pre_run_phase = crate::runtime_state::run_start_pre_phase_from_phase(from)?;
    if driver.current_run_id().is_some() {
        return Err(RuntimeStateTransitionError {
            from,
            to: RuntimeState::Running,
        });
    }
    driver.set_control_projection(RuntimeState::Running, Some(run_id), Some(pre_run_phase));
    Ok(())
}

fn machine_apply_run_return_projection(
    driver: &mut DriverEntry,
    run_id: &RunId,
    next_phase: RuntimeState,
) -> Result<(), RuntimeStateTransitionError> {
    machine_validate_active_run(driver, run_id, next_phase)?;
    driver.set_control_projection(next_phase, None, None);
    Ok(())
}

fn slice_starts_with(seq: &[InputId], prefix: &[InputId]) -> bool {
    prefix.len() <= seq.len() && seq[..prefix.len()] == *prefix
}

pub(crate) fn machine_input_boundary(driver: &DriverEntry, work_id: &InputId) -> RunApplyBoundary {
    match driver.ingress().handling_mode(work_id) {
        Some(meerkat_core::types::HandlingMode::Steer) => RunApplyBoundary::RunCheckpoint,
        _ => RunApplyBoundary::RunStart,
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

fn machine_validate_stage_drain_snapshot(
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

fn machine_validate_boundary_applied(
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

fn machine_validate_run_completed(
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

fn machine_staged_contributors(driver: &DriverEntry) -> Vec<InputId> {
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

fn machine_validate_run_failed(
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
    store: &dyn RuntimeStore,
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
    pub content_shape: ContentShape,
    pub handling_mode: HandlingMode,
    pub is_prompt: bool,
    pub lifecycle_state: InputLifecycleState,
    pub policy: PolicyDecision,
}

pub(crate) fn machine_build_recovered_ingress_entry(
    state: &InputState,
) -> Option<RecoveredIngressEntry> {
    let handling_mode = state
        .policy
        .as_ref()
        .map(|policy| crate::accept::handling_mode_from_policy(&policy.decision))
        .unwrap_or(HandlingMode::Queue);
    let content_shape = state
        .persisted_input
        .as_ref()
        .map(|input| ContentShape(input.kind_id().to_string()))
        .unwrap_or_else(|| ContentShape("unknown".into()));
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
        is_prompt: matches!(state.persisted_input.as_ref(), Some(Input::Prompt(_))),
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
    store: &dyn RuntimeStore,
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

fn machine_build_replay_plan(
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

async fn machine_stop_runtime(driver: &mut DriverEntry) -> Result<(), RuntimeDriverError> {
    match driver.runtime_state() {
        RuntimeState::Initializing
        | RuntimeState::Idle
        | RuntimeState::Attached
        | RuntimeState::Running
        | RuntimeState::Retired => {}
        from => {
            return Err(RuntimeDriverError::Internal(
                RuntimeStateTransitionError {
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

async fn machine_destroy(driver: &mut DriverEntry) -> Result<DestroyReport, RuntimeDriverError> {
    match driver.runtime_state() {
        RuntimeState::Initializing | RuntimeState::Destroyed => {
            return Err(RuntimeDriverError::Internal(
                RuntimeStateTransitionError {
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

async fn machine_retire(driver: &mut DriverEntry) -> Result<RetireReport, RuntimeDriverError> {
    match driver.runtime_state() {
        RuntimeState::Idle | RuntimeState::Attached | RuntimeState::Running => {}
        from => {
            return Err(RuntimeDriverError::Internal(
                RuntimeStateTransitionError {
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

async fn machine_reset(driver: &mut DriverEntry) -> Result<ResetReport, RuntimeDriverError> {
    match driver.runtime_state() {
        RuntimeState::Initializing
        | RuntimeState::Idle
        | RuntimeState::Attached
        | RuntimeState::Retired => {}
        from => {
            return Err(RuntimeDriverError::Internal(
                RuntimeStateTransitionError {
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

fn machine_prepare_bindings_projection(driver: &mut DriverEntry) -> Result<(), RuntimeDriverError> {
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
            RuntimeStateTransitionError {
                from,
                to: RuntimeState::Attached,
            }
            .to_string(),
        )),
    }
}

fn machine_executor_attach_projection(
    driver: &mut DriverEntry,
) -> Result<bool, RuntimeDriverError> {
    match driver.runtime_state() {
        RuntimeState::Idle => {
            driver.set_control_projection(RuntimeState::Attached, None, None);
            Ok(true)
        }
        RuntimeState::Attached => Ok(false),
        from => Err(RuntimeDriverError::Internal(
            RuntimeStateTransitionError {
                from,
                to: RuntimeState::Attached,
            }
            .to_string(),
        )),
    }
}

fn machine_unregister_session_projection(driver: &mut DriverEntry) {
    if matches!(driver.runtime_state(), RuntimeState::Attached) {
        driver.set_control_projection(RuntimeState::Idle, None, None);
    }
}

async fn machine_recycle_preserving_work(
    driver: &mut DriverEntry,
) -> Result<usize, RuntimeDriverError> {
    let target_phase = match driver.runtime_state() {
        RuntimeState::Idle | RuntimeState::Retired => RuntimeState::Idle,
        RuntimeState::Attached => RuntimeState::Attached,
        from => {
            return Err(RuntimeDriverError::Internal(
                RuntimeStateTransitionError {
                    from,
                    to: RuntimeState::Idle,
                }
                .to_string(),
            ));
        }
    };

    if driver.current_run_id().is_some() {
        return Err(RuntimeDriverError::Internal(
            RuntimeStateTransitionError {
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

#[derive(Debug, Default)]
struct MachineToolVisibilityOwner {
    state: StdRwLock<SessionToolVisibilityState>,
    next_revision: AtomicU64,
}

impl MachineToolVisibilityOwner {
    fn new() -> Self {
        Self::default()
    }
}

fn formal_projection_value<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|err| format!("\"<serialization error: {err}>\""))
}

impl ToolVisibilityOwner for MachineToolVisibilityOwner {
    fn visibility_state(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
        self.state
            .read()
            .map(|state| state.clone())
            .map_err(|_| ToolScopeApplyError::Owner {
                message: "machine visibility state lock poisoned".to_string(),
            })
    }

    fn replace_visibility_state(
        &self,
        visibility_state: SessionToolVisibilityState,
    ) -> Result<(), ToolScopeApplyError> {
        let mut state = self.state.write().map_err(|_| ToolScopeApplyError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        let next_revision = visibility_state
            .active_revision
            .max(visibility_state.staged_revision);
        *state = visibility_state;
        self.next_revision.store(next_revision, Ordering::SeqCst);
        Ok(())
    }

    fn stage_persistent_filter(
        &self,
        filter: ToolFilter,
        witnesses: std::collections::BTreeMap<String, ToolVisibilityWitness>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        let mut state = self.state.write().map_err(|_| ToolScopeStageError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        let revision = ToolScopeRevision(self.next_revision.fetch_add(1, Ordering::SeqCst) + 1);
        state.staged_filter = filter;
        state.filter_witnesses.extend(witnesses);
        state.staged_revision = revision.0;
        Ok(revision)
    }

    fn stage_requested_deferred_names(
        &self,
        names: std::collections::BTreeSet<String>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        let mut state = self.state.write().map_err(|_| ToolScopeStageError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        let revision = ToolScopeRevision(self.next_revision.fetch_add(1, Ordering::SeqCst) + 1);
        state.staged_requested_deferred_names = names;
        state.staged_revision = revision.0;
        Ok(revision)
    }

    fn request_deferred_tools(
        &self,
        names: std::collections::BTreeSet<String>,
        witnesses: std::collections::BTreeMap<String, ToolVisibilityWitness>,
    ) -> Result<ToolScopeRevision, ToolScopeStageError> {
        let mut state = self.state.write().map_err(|_| ToolScopeStageError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        let revision = ToolScopeRevision(self.next_revision.fetch_add(1, Ordering::SeqCst) + 1);
        state.staged_requested_deferred_names.extend(names);
        state.requested_witnesses.extend(witnesses);
        state.staged_revision = revision.0;
        Ok(revision)
    }

    fn boundary_applied(&self) -> Result<SessionToolVisibilityState, ToolScopeApplyError> {
        let mut state = self.state.write().map_err(|_| ToolScopeApplyError::Owner {
            message: "machine visibility state lock poisoned".to_string(),
        })?;
        state.active_filter = state.staged_filter.clone();
        state.active_requested_deferred_names = state.staged_requested_deferred_names.clone();
        state.active_revision = state.staged_revision;
        Ok(state.clone())
    }
}

/// Per-session state: driver + registration phase.
struct RuntimeSessionEntry {
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
    /// Per-session comms drain lifecycle, driven by machine authority.
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
            MeerkatMachineCommand::RegisterSession { .. }
            | MeerkatMachineCommand::UnregisterSession { .. }
            | MeerkatMachineCommand::EnsureSessionWithExecutor { .. }
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

    async fn execute_meerkat_machine_session_command(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineCommand::RegisterSession { session_id } => {
                // Guard: DestroyedShapeInvariant — a destroyed binding must
                // never be resurrected.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                self.register_session_inner(session_id).await;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::UnregisterSession { session_id } => {
                // Guard: session must exist before it can be unregistered.
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                self.unregister_session_inner(&session_id).await;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::EnsureSessionWithExecutor {
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
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::SetSilentIntents {
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
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::InterruptCurrentRun { session_id } => {
                // Guard: DestroyedShapeInvariant — no mutation on destroyed sessions.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                self.interrupt_current_run_inner(&session_id)
                    .await
                    .map(|()| MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::CancelAfterBoundary { session_id } => {
                // Guard: DestroyedShapeInvariant — no mutation on destroyed sessions.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                self.cancel_after_boundary_inner(&session_id)
                    .await
                    .map(|()| MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::StopRuntimeExecutor {
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
                    .map(|()| MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::ContainsSession { session_id } => {
                Ok(MeerkatMachineCommandResult::Bool(
                    self.sessions.read().await.contains_key(&session_id),
                ))
            }
            MeerkatMachineCommand::SessionHasExecutor { session_id } => {
                let sessions = self.sessions.read().await;
                Ok(MeerkatMachineCommandResult::Bool(
                    sessions
                        .get(&session_id)
                        .map(RuntimeSessionEntry::has_attachment_or_attaching)
                        .unwrap_or(false),
                ))
            }
            MeerkatMachineCommand::SessionHasComms { session_id } => {
                let slots = self.comms_drain_slots.read().await;
                Ok(MeerkatMachineCommandResult::Bool(
                    slots.contains_key(&session_id),
                ))
            }
            MeerkatMachineCommand::OpsLifecycleRegistry { session_id } => {
                let sessions = self.sessions.read().await;
                Ok(MeerkatMachineCommandResult::OpsLifecycleRegistry(
                    sessions
                        .get(&session_id)
                        .map(|e| Arc::clone(&e.ops_lifecycle)),
                ))
            }
            MeerkatMachineCommand::PrepareBindings { session_id } => {
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
                let mut driver = entry.driver.lock().await;
                machine_prepare_bindings_projection(&mut driver)?;
                drop(driver);
                Ok(MeerkatMachineCommandResult::Bindings(
                    meerkat_core::SessionRuntimeBindings {
                        session_id,
                        epoch_id: entry.epoch_id.clone(),
                        ops_lifecycle: Arc::clone(&entry.ops_lifecycle)
                            as Arc<dyn meerkat_core::OpsLifecycleRegistry>,
                        cursor_state: Arc::clone(&entry.cursor_state),
                        tool_visibility_owner: Arc::clone(&entry.tool_visibility_owner)
                            as Arc<dyn meerkat_core::ToolVisibilityOwner>,
                    },
                ))
            }
            MeerkatMachineCommand::InputState {
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
                Ok(MeerkatMachineCommandResult::InputState(
                    driver.as_driver().input_state(&input_id).cloned(),
                ))
            }
            MeerkatMachineCommand::ListActiveInputs { session_id } => {
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
                Ok(MeerkatMachineCommandResult::ActiveInputs(
                    driver.as_driver().active_input_ids(),
                ))
            }
            MeerkatMachineCommand::ReconfigureSessionLlmIdentity {
                session_id,
                previous_identity,
                previous_visibility_state,
                previous_capability_surface,
                previous_capability_surface_status,
                target_identity,
                target_capability_surface,
                next_visibility_state,
                next_capability_base_filter: _next_capability_base_filter,
                next_active_visibility_revision: _next_active_visibility_revision,
                tool_visibility_delta,
            } => self
                .reconfigure_session_llm_identity_inner(
                    &session_id,
                    *previous_identity,
                    *previous_visibility_state,
                    previous_capability_surface,
                    previous_capability_surface_status,
                    *target_identity,
                    *target_capability_surface,
                    *next_visibility_state,
                    *tool_visibility_delta,
                )
                .await
                .map(MeerkatMachineCommandResult::LlmReconfigured),
            MeerkatMachineCommand::StagePersistentFilter {
                session_id,
                filter,
                witnesses,
            } => {
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                let owner = {
                    let sessions = self.sessions.read().await;
                    Arc::clone(
                        &sessions
                            .get(&session_id)
                            .ok_or(RuntimeDriverError::NotReady {
                                state: RuntimeState::Destroyed,
                            })?
                            .tool_visibility_owner,
                    )
                };
                let revision = owner
                    .stage_persistent_filter(filter, witnesses)
                    .map_err(|err| RuntimeDriverError::Internal(err.to_string()))?;
                Ok(MeerkatMachineCommandResult::VisibilityRevision(revision))
            }
            MeerkatMachineCommand::RequestDeferredTools {
                session_id,
                names,
                witnesses,
            } => {
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }
                let owner = {
                    let sessions = self.sessions.read().await;
                    Arc::clone(
                        &sessions
                            .get(&session_id)
                            .ok_or(RuntimeDriverError::NotReady {
                                state: RuntimeState::Destroyed,
                            })?
                            .tool_visibility_owner,
                    )
                };
                let revision = owner
                    .request_deferred_tools(names, witnesses)
                    .map_err(|err| RuntimeDriverError::Internal(err.to_string()))?;
                Ok(MeerkatMachineCommandResult::VisibilityRevision(revision))
            }
            MeerkatMachineCommand::PublishCommittedVisibleSet {
                session_id,
                visibility_state,
            } => {
                // Guard: session must exist — publishing to an unknown session
                // has no target.
                let sessions = self.sessions.read().await;
                if !sessions.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                drop(sessions);

                // Guard: DestroyedShapeInvariant — no mutation on destroyed sessions.
                if matches!(
                    self.existing_session_runtime_state(&session_id).await,
                    Some(RuntimeState::Destroyed)
                ) {
                    return Err(RuntimeDriverError::Destroyed);
                }

                // Guard: VisibleSurfacesMatchAppliedStateInvariant —
                // the committed (active) revision must not lag behind the staged
                // revision. A lagging active revision means the visible set has
                // not caught up with staged mutations, violating the TLA+
                // invariant that visible_surfaces == {s : base_state[s] # None}.
                if visibility_state.active_revision < visibility_state.staged_revision {
                    return Err(RuntimeDriverError::ValidationFailed {
                        reason: format!(
                            "VisibleSurfacesMatchAppliedStateInvariant violated: \
                             active_revision ({}) < staged_revision ({})",
                            visibility_state.active_revision, visibility_state.staged_revision,
                        ),
                    });
                }

                if visibility_state.active_revision == visibility_state.staged_revision
                    && (visibility_state.active_filter != visibility_state.staged_filter
                        || visibility_state.active_requested_deferred_names
                            != visibility_state.staged_requested_deferred_names)
                {
                    return Err(RuntimeDriverError::ValidationFailed {
                        reason: "VisibleSurfacesMatchAppliedStateInvariant violated: equal revisions require equal active and staged visibility state".to_string(),
                    });
                }

                if !visibility_state
                    .active_requested_deferred_names
                    .is_subset(&visibility_state.staged_requested_deferred_names)
                {
                    return Err(RuntimeDriverError::ValidationFailed {
                        reason: "VisibleSurfacesMatchAppliedStateInvariant violated: active requested deferred names must remain a subset of staged requested deferred names".to_string(),
                    });
                }

                {
                    let sessions = self.sessions.read().await;
                    let owner = Arc::clone(
                        &sessions
                            .get(&session_id)
                            .ok_or(RuntimeDriverError::NotReady {
                                state: RuntimeState::Destroyed,
                            })?
                            .tool_visibility_owner,
                    );
                    owner
                        .replace_visibility_state(*visibility_state.clone())
                        .map_err(|err| RuntimeDriverError::Internal(err.to_string()))?;
                }

                Ok(MeerkatMachineCommandResult::VisibilityPublished(
                    *visibility_state,
                ))
            }
            _ => unreachable!("non-session command routed to session handler"),
        }
    }

    async fn execute_meerkat_machine_drain_command(
        self: &Arc<Self>,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineCommand::SetPeerIngressContext {
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
                Ok(MeerkatMachineCommandResult::Spawned(
                    self.update_peer_ingress_context_inner(&session_id, keep_alive, comms_runtime)
                        .await,
                ))
            }
            MeerkatMachineCommand::NotifyDrainExited { session_id, reason } => {
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
                Ok(MeerkatMachineCommandResult::Unit)
            }
            _ => unreachable!("non-drain command routed to drain handler"),
        }
    }

    async fn execute_meerkat_machine_drain_local_command(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineCommand::AbortAll => {
                let mut slots = self.comms_drain_slots.write().await;
                for (_, slot) in slots.iter_mut() {
                    abort_slot(slot);
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::Abort { session_id } => {
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
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::Wait { session_id } => {
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
                Ok(MeerkatMachineCommandResult::Unit)
            }
            _ => unreachable!("non-drain-local command routed to drain-local handler"),
        }
    }

    async fn execute_meerkat_machine_control_command(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeControlPlaneError> {
        match command {
            MeerkatMachineCommand::Ingest { runtime_id, input } => {
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

                let request_immediate_processing =
                    crate::accept::requests_immediate_processing(&input);
                let (outcome, signal) = {
                    let mut drv = driver.lock().await;
                    let result = drv
                        .as_driver_mut()
                        .accept_input(input)
                        .await
                        .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                    let signal =
                        Self::machine_owned_admission_signal(&result, request_immediate_processing);
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

                Ok(MeerkatMachineCommandResult::AcceptOutcome(outcome))
            }
            MeerkatMachineCommand::PublishEvent { event } => {
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
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::Retire { runtime_id } => {
                let (session_id, driver, completions, wake_tx) =
                    self.lookup_entry(&runtime_id).await?;
                let _ = session_id;

                let state = Self::driver_runtime_state(&driver).await;
                if matches!(state, RuntimeState::Destroyed | RuntimeState::Stopped) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }

                let mut drv = driver.lock().await;
                let mut report = machine_retire(&mut drv)
                    .await
                    .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                drop(drv);

                if report.inputs_pending_drain > 0 {
                    if let Some(ref tx) = wake_tx
                        && tx.send(()).await.is_ok()
                    {
                        return Ok(MeerkatMachineCommandResult::RetireReport(report));
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

                Ok(MeerkatMachineCommandResult::RetireReport(report))
            }
            MeerkatMachineCommand::Recycle { runtime_id } => {
                let (_session_id, driver, completions, wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                let state = Self::driver_runtime_state(&driver).await;
                if matches!(state, RuntimeState::Destroyed | RuntimeState::Running) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }

                let (transferred, active_after_recycle) = {
                    let mut drv = driver.lock().await;
                    let transferred = machine_recycle_preserving_work(&mut drv)
                        .await
                        .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;

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

                Ok(MeerkatMachineCommandResult::RecycleReport(RecycleReport {
                    inputs_transferred: transferred,
                }))
            }
            MeerkatMachineCommand::Reset { runtime_id } => {
                let (_session_id, driver, completions, _wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                let state = Self::driver_runtime_state(&driver).await;
                if matches!(state, RuntimeState::Destroyed | RuntimeState::Running) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }

                let mut drv = driver.lock().await;
                let report = machine_reset(&mut drv)
                    .await
                    .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                drop(drv);

                let mut comp = completions.lock().await;
                comp.resolve_all_terminated("runtime reset");

                Ok(MeerkatMachineCommandResult::ResetReport(report))
            }
            MeerkatMachineCommand::Recover { runtime_id } => {
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

                Ok(MeerkatMachineCommandResult::RecoveryReport(report))
            }
            MeerkatMachineCommand::Destroy { runtime_id } => {
                let (_session_id, driver, completions, _wake_tx) =
                    self.lookup_entry(&runtime_id).await?;

                let state = Self::driver_runtime_state(&driver).await;
                if matches!(state, RuntimeState::Destroyed) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }

                // Guard: runtime_is_bound — reject Destroy if the runtime was
                // never bound via PrepareBindings. The driver's Initializing
                // state means Initialize hasn't even been called. Beyond that,
                // the ephemeral driver transitions through PrepareBindings ->
                // Attached, so Initializing is the only state where the runtime
                // is definitely unbound. (The schema field `active_runtime_id`
                // maps to "has PrepareBindings been called and not reset".)
                if matches!(state, RuntimeState::Initializing) {
                    return Err(RuntimeControlPlaneError::InvalidState { state });
                }

                let mut drv = driver.lock().await;
                let report = machine_destroy(&mut drv)
                    .await
                    .map_err(|e| RuntimeControlPlaneError::Internal(e.to_string()))?;
                drop(drv);

                let mut comp = completions.lock().await;
                comp.resolve_all_terminated("runtime destroyed");

                Ok(MeerkatMachineCommandResult::DestroyReport(report))
            }
            MeerkatMachineCommand::RuntimeState { runtime_id } => {
                let session_id = Self::resolve_session_id(&runtime_id)?;
                let sessions = self.sessions.read().await;
                let entry = sessions
                    .get(&session_id)
                    .ok_or(RuntimeControlPlaneError::NotFound(runtime_id.clone()))?;
                Ok(MeerkatMachineCommandResult::RuntimeState(
                    entry.control_snapshot().phase,
                ))
            }
            MeerkatMachineCommand::LoadBoundaryReceipt {
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
                Ok(MeerkatMachineCommandResult::BoundaryReceipt(receipt))
            }
            _ => unreachable!("non-control command routed to control handler"),
        }
    }

    async fn execute_meerkat_machine_ingress_command(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineCommand::AcceptWithCompletion { session_id, input } => {
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

                let state = self
                    .existing_session_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                if matches!(
                    state,
                    RuntimeState::Retired | RuntimeState::Stopped | RuntimeState::Destroyed
                ) {
                    return Err(match state {
                        RuntimeState::Destroyed => RuntimeDriverError::Destroyed,
                        RuntimeState::Retired | RuntimeState::Stopped => {
                            RuntimeDriverError::NotReady { state }
                        }
                        _ => unreachable!("guard only matches retired/stopped/destroyed"),
                    });
                }

                let request_immediate_processing =
                    crate::accept::requests_immediate_processing(&input);
                let (outcome, signal, handle) = {
                    let mut driver = driver.lock().await;
                    let result = driver
                        .as_driver_mut()
                        .accept_input(input)
                        .await
                        .map_err(Self::normalize_destroyed_error)?;
                    let signal =
                        Self::machine_owned_admission_signal(&result, request_immediate_processing);

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
                            (result, signal, handle)
                        }
                        AcceptOutcome::Deduplicated { existing_id, .. } => {
                            let existing_state = driver.as_driver().input_state(existing_id);
                            let is_terminal = existing_state
                                .map(|s| s.current_state().is_terminal())
                                .unwrap_or(true);

                            if is_terminal {
                                (result, signal, None)
                            } else {
                                let handle = {
                                    let mut completions = completions.lock().await;
                                    completions.register(existing_id.clone())
                                };
                                (result, signal, Some(handle))
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

                Ok(MeerkatMachineCommandResult::AcceptWithCompletion {
                    outcome,
                    handle,
                    admission_signal: signal,
                })
            }
            MeerkatMachineCommand::AcceptWithoutWake { session_id, input } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    let entry = sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?;
                    entry.driver.clone()
                };

                let state = self
                    .existing_session_runtime_state(&session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                if matches!(
                    state,
                    RuntimeState::Retired | RuntimeState::Stopped | RuntimeState::Destroyed
                ) {
                    return Err(match state {
                        RuntimeState::Destroyed => RuntimeDriverError::Destroyed,
                        RuntimeState::Retired | RuntimeState::Stopped => {
                            RuntimeDriverError::NotReady { state }
                        }
                        _ => unreachable!("guard only matches retired/stopped/destroyed"),
                    });
                }

                let request_immediate_processing =
                    crate::accept::requests_immediate_processing(&input);
                let outcome = {
                    let mut driver = driver.lock().await;
                    let result = driver
                        .as_driver_mut()
                        .accept_input(input)
                        .await
                        .map_err(Self::normalize_destroyed_error)?;
                    let signal =
                        Self::machine_owned_admission_signal(&result, request_immediate_processing);
                    debug_assert!(
                        !signal.should_process_immediately(),
                        "queue-only admission unexpectedly requested immediate processing"
                    );
                    result
                };

                Ok(MeerkatMachineCommandResult::AcceptOutcome(outcome))
            }
            _ => unreachable!("non-ingress command routed to ingress handler"),
        }
    }

    async fn execute_meerkat_machine_legacy_run_command(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineCommand::Prepare { session_id, input } => {
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
                        return Err(Self::normalize_destroyed_error(
                            RuntimeDriverError::NotReady {
                                state: driver.as_driver().runtime_state(),
                            },
                        ));
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

                    let outcome = driver
                        .as_driver_mut()
                        .accept_input(input)
                        .await
                        .map_err(Self::normalize_destroyed_error)?;
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
                        return Err(Self::normalize_destroyed_error(
                            RuntimeDriverError::NotReady {
                                state: driver.as_driver().runtime_state(),
                            },
                        ));
                    }

                    let (dequeued_id, dequeued_input) = driver.dequeue_next().ok_or_else(|| {
                        RuntimeDriverError::Internal(
                            "accepted input was not queued for execution".into(),
                        )
                    })?;
                    if dequeued_id != input_id {
                        return Err(Self::normalize_destroyed_error(
                            RuntimeDriverError::NotReady {
                                state: driver.as_driver().runtime_state(),
                            },
                        ));
                    }

                    let run_id = RunId::new();
                    machine_begin_run(&mut driver, run_id.clone()).map_err(|err| {
                        RuntimeDriverError::Internal(format!("failed to start runtime run: {err}"))
                    })?;
                    if let Err(err) = driver.stage_input(&dequeued_id, &run_id) {
                        let _ = driver.rollback_staged(std::slice::from_ref(&dequeued_id));
                        let next_phase = crate::runtime_state::run_return_phase_from_pre_run_phase(
                            driver.pre_run_phase(),
                        );
                        let _ =
                            machine_apply_run_return_projection(&mut driver, &run_id, next_phase);
                        return Err(RuntimeDriverError::Internal(format!(
                            "failed to stage accepted input: {err}"
                        )));
                    }

                    MeerkatMachineLegacyRunPrepared {
                        input_id,
                        run_id,
                        primitive: crate::runtime_loop::input_to_primitive(
                            &dequeued_input,
                            dequeued_id,
                        ),
                    }
                };

                Ok(MeerkatMachineCommandResult::Prepared(prepared))
            }
            MeerkatMachineCommand::Commit {
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

                if let Err(err) = commit_runtime_loop_run(
                    &driver,
                    run_id,
                    vec![input_id],
                    output.receipt,
                    output.session_snapshot,
                )
                .await
                {
                    let should_unregister =
                        !err.to_string().contains("runtime boundary commit failed");
                    if should_unregister {
                        self.unregister_session_inner(&session_id).await;
                    }
                    return Err(RuntimeDriverError::Internal(format!(
                        "runtime commit failed: {err}"
                    )));
                }

                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::Fail {
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

                if let Err(run_err) = fail_runtime_loop_run(&driver, run_id, error).await {
                    self.unregister_session_inner(&session_id).await;
                    return Err(RuntimeDriverError::Internal(format!(
                        "failed to persist runtime failure snapshot: {run_err}"
                    )));
                }

                Ok(MeerkatMachineCommandResult::Unit)
            }
            _ => unreachable!("non-legacy-run command routed to legacy-run handler"),
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
        let control_projection = entry.control_projection_handle();
        let session_entry = RuntimeSessionEntry {
            control_projection,
            driver: Arc::new(Mutex::new(entry)),
            ops_lifecycle,
            epoch_id,
            cursor_state,
            completions: Arc::new(Mutex::new(crate::completion::CompletionRegistry::new())),
            tool_visibility_owner: Arc::new(MachineToolVisibilityOwner::new()),
            current_llm_identity: None,
            current_capability_surface: None,
            capability_surface_status: SessionLlmCapabilitySurfaceStatus::Unresolved,
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
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::SetSilentIntents {
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
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::EnsureSessionWithExecutor {
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
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::EnsureSessionWithExecutor {
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
                // the entry while we were rebuilding runtime state.
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
                    let control_projection = recovered_entry.control_projection_handle();
                    let driver = Arc::new(Mutex::new(recovered_entry));
                    let completions =
                        Arc::new(Mutex::new(crate::completion::CompletionRegistry::new()));
                    sessions.insert(
                        session_id.clone(),
                        RuntimeSessionEntry {
                            control_projection,
                            driver: driver.clone(),
                            ops_lifecycle: recovered_ops.clone(),
                            epoch_id: recovered_epoch,
                            cursor_state: recovered_cursors,
                            completions: completions.clone(),
                            tool_visibility_owner: Arc::new(MachineToolVisibilityOwner::new()),
                            current_llm_identity: None,
                            current_capability_surface: None,
                            capability_surface_status:
                                SessionLlmCapabilitySurfaceStatus::Unresolved,
                            phase: RegistrationPhase::Queuing,
                            detached_wake: None,
                        },
                    );
                    (driver, completions, recovered_ops)
                }
            };

        let should_wake = {
            let mut driver_guard = driver.lock().await;
            match machine_executor_attach_projection(&mut driver_guard) {
                Ok(true) => {}
                Ok(false) => {
                    tracing::warn!(
                        %session_id,
                        "runtime driver remained attached without a live published loop; republishing attachment"
                    );
                }
                Err(error) => {
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
                machine_unregister_session_projection(&mut driver_guard);
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
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::UnregisterSession {
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
            machine_unregister_session_projection(&mut driver);
            drop(driver);

            let mut completions = entry.completions.lock().await;
            completions.resolve_all_terminated("runtime session unregistered");
        }
    }

    /// Check whether a runtime driver is already registered for a session.
    pub async fn contains_session(&self, session_id: &SessionId) -> bool {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::ContainsSession {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::Bool(present)) => present,
            Ok(_) => {
                tracing::error!("contains_session: unexpected command result variant");
                false
            }
            Err(_) => false,
        }
    }

    /// Check whether a session has an active RuntimeLoop or attachment in
    /// progress. Returns `false` only for `Queuing` sessions (registered via
    /// `prepare_bindings()` with no executor) and unknown sessions.
    pub async fn session_has_executor(&self, session_id: &SessionId) -> bool {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::SessionHasExecutor {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::Bool(present)) => present,
            Ok(_) => {
                tracing::error!("session_has_executor: unexpected command result variant");
                false
            }
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
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::SessionHasComms {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::Bool(present)) => present,
            Ok(_) => {
                tracing::error!("session_has_comms: unexpected command result variant");
                false
            }
            Err(_) => false,
        }
    }

    /// Cancel the currently-running turn for a registered session.
    pub async fn interrupt_current_run(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        self.execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::InterruptCurrentRun {
                session_id: session_id.clone(),
            },
        )
        .await
        .map_err(MeerkatMachine::driver_error_from_command_error)
        .map(|_| ())
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

    /// Stage a durable session visibility filter through the machine-owned visibility state.
    pub async fn stage_persistent_filter(
        &self,
        session_id: &SessionId,
        filter: meerkat_core::ToolFilter,
        witnesses: std::collections::BTreeMap<String, meerkat_core::ToolVisibilityWitness>,
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
        names: std::collections::BTreeSet<String>,
        witnesses: std::collections::BTreeMap<String, meerkat_core::ToolVisibilityWitness>,
    ) -> Result<meerkat_core::ToolScopeRevision, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::RequestDeferredTools {
                    session_id: session_id.clone(),
                    names,
                    witnesses,
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

    fn llm_reconfigure_host(
        &self,
    ) -> Result<Arc<dyn SessionLlmReconfigureHost>, RuntimeDriverError> {
        self.llm_reconfigure_host
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                RuntimeDriverError::Internal(
                    "session llm reconfigure host is not configured".to_string(),
                )
            })
    }

    fn capability_delta(
        previous: Option<SessionLlmCapabilitySurface>,
        current: Option<SessionLlmCapabilitySurface>,
    ) -> SessionLlmCapabilityDelta {
        SessionLlmCapabilityDelta {
            changed: previous != current,
            previous,
            current,
        }
    }

    fn committed_visibility_allows(
        base_tool_names: &std::collections::BTreeSet<String>,
        visibility_state: &SessionToolVisibilityState,
        tool_name: &str,
    ) -> bool {
        if !base_tool_names.contains(tool_name) {
            return false;
        }

        meerkat_core::ToolScope::compose(&[
            visibility_state.capability_base_filter.clone(),
            visibility_state.inherited_base_filter.clone(),
            visibility_state.active_filter.clone(),
        ])
        .allows(tool_name)
    }

    fn derive_reconfigured_visibility_state(
        current: &SessionToolVisibilityState,
        target_capability_surface: &SessionLlmCapabilitySurface,
        base_tool_names: &std::collections::BTreeSet<String>,
    ) -> (SessionToolVisibilityState, SessionToolVisibilityDelta) {
        let previous_capability_base_filter = current.capability_base_filter.clone();
        let current_view_image_visible = Self::committed_visibility_allows(
            base_tool_names,
            current,
            meerkat_core::VIEW_IMAGE_TOOL_NAME,
        );

        let mut next = current.clone();
        next.capability_base_filter = meerkat_core::capability_base_filter_for_image_tool_results(
            target_capability_surface.image_tool_results,
        );

        let next_view_image_visible = Self::committed_visibility_allows(
            base_tool_names,
            &next,
            meerkat_core::VIEW_IMAGE_TOOL_NAME,
        );
        let committed_visible_set_changed = current_view_image_visible != next_view_image_visible;
        let revision_bumped = committed_visible_set_changed;
        if revision_bumped {
            next.active_revision = current.active_revision.max(current.staged_revision) + 1;
        }

        (
            next.clone(),
            SessionToolVisibilityDelta {
                previous_capability_base_filter,
                current_capability_base_filter: next.capability_base_filter,
                committed_visible_set_changed,
                revision_bumped,
            },
        )
    }

    async fn set_cached_session_llm_state(
        &self,
        session_id: &SessionId,
        current_identity: Option<meerkat_core::SessionLlmIdentity>,
        current_capability_surface: Option<SessionLlmCapabilitySurface>,
        capability_surface_status: SessionLlmCapabilitySurfaceStatus,
    ) {
        let mut sessions = self.sessions.write().await;
        if let Some(entry) = sessions.get_mut(session_id) {
            entry.current_llm_identity = current_identity;
            entry.current_capability_surface = current_capability_surface;
            entry.capability_surface_status = capability_surface_status;
        }
    }

    async fn cache_hydrated_session_llm_state(
        &self,
        session_id: &SessionId,
        hydrated: &HydratedSessionLlmState,
    ) -> Result<(), RuntimeDriverError> {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        entry
            .tool_visibility_owner
            .replace_visibility_state(hydrated.current_visibility_state.clone())
            .map_err(|err| RuntimeDriverError::Internal(err.to_string()))?;
        entry.current_llm_identity = Some(hydrated.current_identity.clone());
        entry.current_capability_surface = hydrated.current_capability_surface.clone();
        entry.capability_surface_status = hydrated.capability_surface_status;
        Ok(())
    }

    async fn replace_machine_visibility_state(
        &self,
        session_id: &SessionId,
        visibility_state: SessionToolVisibilityState,
    ) -> Result<(), RuntimeDriverError> {
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        entry
            .tool_visibility_owner
            .replace_visibility_state(visibility_state)
            .map_err(|err| RuntimeDriverError::Internal(err.to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    async fn rollback_reconfigure_failure(
        &self,
        host: &Arc<dyn SessionLlmReconfigureHost>,
        session_id: &SessionId,
        previous_identity: &meerkat_core::SessionLlmIdentity,
        previous_visibility_state: &SessionToolVisibilityState,
        previous_capability_surface: Option<SessionLlmCapabilitySurface>,
        previous_capability_surface_status: SessionLlmCapabilitySurfaceStatus,
        original_error: RuntimeDriverError,
    ) -> Result<SessionLlmReconfigureReport, RuntimeDriverError> {
        let rollback_result = async {
            host.apply_live_session_llm_identity(session_id, previous_identity)
                .await?;
            host.apply_live_session_tool_visibility_state(
                session_id,
                Some(previous_visibility_state.clone()),
            )
            .await?;
            Ok::<(), RuntimeDriverError>(())
        }
        .await;

        match rollback_result {
            Ok(()) => {
                self.set_cached_session_llm_state(
                    session_id,
                    Some(previous_identity.clone()),
                    previous_capability_surface,
                    previous_capability_surface_status,
                )
                .await;
                Err(original_error)
            }
            Err(rollback_error) => {
                let _ = host.discard_live_session(session_id).await;
                self.set_cached_session_llm_state(
                    session_id,
                    None,
                    None,
                    SessionLlmCapabilitySurfaceStatus::Unresolved,
                )
                .await;
                Err(RuntimeDriverError::Internal(format!(
                    "failed to rollback live llm reconfiguration after error ({original_error}): {rollback_error}"
                )))
            }
        }
    }

    async fn prepare_reconfigure_session_llm_command(
        &self,
        session_id: &SessionId,
        request: SessionLlmReconfigureRequest,
    ) -> Result<MeerkatMachineCommand, RuntimeDriverError> {
        let host = self.llm_reconfigure_host()?;
        let runtime_state = self
            .existing_session_runtime_state(session_id)
            .await
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        if !matches!(
            runtime_state,
            RuntimeState::Attached | RuntimeState::Running
        ) {
            return Err(RuntimeDriverError::NotReady {
                state: runtime_state,
            });
        }

        let hydrated = host.hydrate_session_llm_state(session_id).await?;
        self.cache_hydrated_session_llm_state(session_id, &hydrated)
            .await?;

        let resolved = host
            .resolve_target_session_llm_identity(&request, &hydrated.current_identity)
            .await?;

        let (next_visibility_state, tool_visibility_delta) =
            Self::derive_reconfigured_visibility_state(
                &hydrated.current_visibility_state,
                &resolved.target_capability_surface,
                &hydrated.base_tool_names,
            );
        let next_capability_base_filter = next_visibility_state.capability_base_filter.clone();
        let next_active_visibility_revision = next_visibility_state.active_revision;

        Ok(MeerkatMachineCommand::ReconfigureSessionLlmIdentity {
            session_id: session_id.clone(),
            previous_identity: Box::new(hydrated.current_identity),
            previous_visibility_state: Box::new(hydrated.current_visibility_state),
            previous_capability_surface: hydrated.current_capability_surface,
            previous_capability_surface_status: hydrated.capability_surface_status,
            target_identity: Box::new(resolved.target_identity),
            target_capability_surface: Box::new(resolved.target_capability_surface),
            next_visibility_state: Box::new(next_visibility_state),
            next_capability_base_filter,
            next_active_visibility_revision,
            tool_visibility_delta: Box::new(tool_visibility_delta),
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn reconfigure_session_llm_identity_inner(
        &self,
        session_id: &SessionId,
        previous_identity: meerkat_core::SessionLlmIdentity,
        previous_visibility_state: SessionToolVisibilityState,
        previous_capability_surface: Option<SessionLlmCapabilitySurface>,
        previous_capability_surface_status: SessionLlmCapabilitySurfaceStatus,
        target_identity: meerkat_core::SessionLlmIdentity,
        target_capability_surface: SessionLlmCapabilitySurface,
        next_visibility_state: SessionToolVisibilityState,
        tool_visibility_delta: SessionToolVisibilityDelta,
    ) -> Result<SessionLlmReconfigureReport, RuntimeDriverError> {
        let host = self.llm_reconfigure_host()?;
        let runtime_state = self
            .existing_session_runtime_state(session_id)
            .await
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        if !matches!(
            runtime_state,
            RuntimeState::Attached | RuntimeState::Running
        ) {
            return Err(RuntimeDriverError::NotReady {
                state: runtime_state,
            });
        }

        let capability_delta = Self::capability_delta(
            previous_capability_surface.clone(),
            Some(target_capability_surface.clone()),
        );

        host.apply_live_session_llm_identity(session_id, &target_identity)
            .await?;
        if let Err(error) = host
            .apply_live_session_tool_visibility_state(
                session_id,
                Some(next_visibility_state.clone()),
            )
            .await
        {
            return self
                .rollback_reconfigure_failure(
                    &host,
                    session_id,
                    &previous_identity,
                    &previous_visibility_state,
                    previous_capability_surface.clone(),
                    previous_capability_surface_status,
                    error,
                )
                .await;
        }

        if let Err(error) = host.persist_live_session(session_id).await {
            return self
                .rollback_reconfigure_failure(
                    &host,
                    session_id,
                    &previous_identity,
                    &previous_visibility_state,
                    previous_capability_surface.clone(),
                    previous_capability_surface_status,
                    error,
                )
                .await;
        }

        self.replace_machine_visibility_state(session_id, next_visibility_state)
            .await?;
        self.set_cached_session_llm_state(
            session_id,
            Some(target_identity.clone()),
            Some(target_capability_surface.clone()),
            SessionLlmCapabilitySurfaceStatus::Resolved,
        )
        .await;

        Ok(SessionLlmReconfigureReport {
            previous_identity,
            new_identity: target_identity,
            capability_delta,
            tool_visibility_delta,
            rollback_occurred: false,
        })
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
        self.execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::StopRuntimeExecutor {
                session_id: session_id.clone(),
                command,
            },
        )
        .await
        .map_err(MeerkatMachine::driver_error_from_command_error)
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

        let state_before_stop = self
            .existing_session_runtime_state(session_id)
            .await
            .unwrap_or(RuntimeState::Destroyed);

        if let Some(control_tx) = control_tx
            && control_tx.send(command.clone()).await.is_ok()
        {
            if matches!(
                (state_before_stop, &command),
                (
                    RuntimeState::Attached,
                    RunControlCommand::StopRuntimeExecutor { .. }
                )
            ) {
                let _ = tokio::time::timeout(std::time::Duration::from_millis(200), async {
                    loop {
                        match self.existing_session_runtime_state(session_id).await {
                            Some(RuntimeState::Stopped | RuntimeState::Destroyed) => break,
                            Some(RuntimeState::Attached) => {
                                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                            }
                            Some(_) | None => break,
                        }
                    }
                })
                .await;
            }

            return Ok(());
        }

        if matches!(command, RunControlCommand::StopRuntimeExecutor { .. }) {
            let mut driver = driver.lock().await;
            machine_stop_runtime(&mut driver).await?;
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
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Prepare {
                    session_id: session_id.clone(),
                    input,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::Prepared(prepared) => prepared,
            other => {
                return Err(RuntimeDriverError::Internal(format!(
                    "unexpected command result preparing legacy Meerkat run: {other:?}"
                )));
            }
        };

        match op(run_id.clone(), primitive).await {
            Ok((result, output)) => {
                self.execute_meerkat_machine_command(
                    None,
                    MeerkatMachineCommand::Commit {
                        session_id: session_id.clone(),
                        input_id,
                        run_id,
                        output,
                    },
                )
                .await
                .map_err(MeerkatMachine::driver_error_from_command_error)?;
                Ok(result)
            }
            Err(err) => {
                self.execute_meerkat_machine_command(
                    None,
                    MeerkatMachineCommand::Fail {
                        session_id: session_id.clone(),
                        run_id,
                        error: err.to_string(),
                    },
                )
                .await
                .map_err(MeerkatMachine::driver_error_from_command_error)?;
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
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::AcceptWithCompletion {
                    session_id: session_id.clone(),
                    input,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::AcceptWithCompletion {
                outcome,
                handle,
                admission_signal: _,
            } => Ok((outcome, handle)),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected command result for accept_input_with_completion: {other:?}"
            ))),
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
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::AcceptWithoutWake {
                    session_id: session_id.clone(),
                    input,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::AcceptOutcome(outcome) => Ok(outcome),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected command result for accept_input_without_wake: {other:?}"
            ))),
        }
    }

    /// Get the shared ops lifecycle registry for a session/runtime instance.
    pub async fn ops_lifecycle_registry(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::OpsLifecycleRegistry {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::OpsLifecycleRegistry(registry)) => registry,
            Ok(_) => {
                tracing::error!("ops_lifecycle_registry: unexpected command result variant");
                None
            }
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
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::PrepareBindings {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::Bindings(bindings)) => Ok(bindings),
            Ok(_) => {
                tracing::error!("prepare_bindings: unexpected command result variant");
                Err(RuntimeBindingsError::SessionNotFound(session_id))
            }
            Err(_) => Err(RuntimeBindingsError::SessionNotFound(session_id)),
        }
    }

    /// Capture a diagnostic snapshot of the current Meerkat runtime spine.
    ///
    /// This is an internal scaffolding aid for the MeerkatMachine refactor. It
    /// intentionally reflects current runtime truth without changing behavior.
    #[doc(hidden)]
    pub async fn meerkat_machine_spine_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Option<MeerkatMachineSpineSnapshot> {
        let (
            driver_handle,
            control_snapshot,
            completions_handle,
            ops_lifecycle,
            cursor_state,
            completions_present,
            ops_registry_present,
            attachment_live,
            detached_wake_pending,
            detached_wake_signaled,
            epoch_id,
            _visibility_state,
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
                entry.control_snapshot(),
                Arc::clone(&entry.completions),
                Arc::clone(&entry.ops_lifecycle),
                Arc::clone(&entry.cursor_state),
                true,
                true,
                entry.attachment_is_live(),
                detached_wake_pending,
                detached_wake_signaled,
                entry.epoch_id.clone(),
                entry.tool_visibility_owner.visibility_state().ok()?,
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
            phase: control_snapshot.phase,
            current_run_id: control_snapshot.current_run_id,
            pre_run_phase: control_snapshot.pre_run_phase,
        };

        let admission_order: Vec<MeerkatAdmittedInputSnapshot> = ingress
            .admission_order()
            .iter()
            .cloned()
            .map(|input_id| {
                let ledger_state = driver.ledger().get(&input_id);
                MeerkatAdmittedInputSnapshot {
                    content_shape: ingress.content_shape(&input_id),
                    request_id: ingress.request_id(&input_id),
                    reservation_key: ingress.reservation_key(&input_id),
                    handling_mode: ingress.handling_mode(&input_id),
                    lifecycle: ledger_state.map(crate::input_state::InputState::current_state),
                    terminal_outcome: ledger_state
                        .and_then(|state| state.terminal_outcome().cloned()),
                    last_run_id: ledger_state.and_then(|state| state.last_run_id().cloned()),
                    last_boundary_sequence: ledger_state
                        .and_then(crate::input_state::InputState::last_boundary_sequence),
                    is_prompt: ingress.is_prompt(&input_id),
                    input_id,
                }
            })
            .collect();

        let current_run_contributors = if let Some(control_run_id) = &control.current_run_id {
            admission_order
                .iter()
                .filter(|snapshot| {
                    snapshot.last_run_id.as_ref() == Some(control_run_id)
                        && matches!(
                            snapshot.lifecycle,
                            Some(
                                crate::input_state::InputLifecycleState::Staged
                                    | crate::input_state::InputLifecycleState::Applied
                                    | crate::input_state::InputLifecycleState::AppliedPendingConsumption
                            )
                        )
                })
                .map(|snapshot| snapshot.input_id.clone())
                .collect()
        } else {
            Vec::new()
        };

        let inputs = MeerkatInputsSnapshot {
            admission_order,
            queue: ingress.queue().to_vec(),
            steer_queue: ingress.steer_queue().to_vec(),
            current_run_id: control.current_run_id.clone(),
            current_run_contributors,
            post_admission_signal: format!("{:?}", driver.post_admission_signal()),
            silent_intent_overrides: driver.silent_comms_intents().into_iter().collect(),
        };
        let ledger = {
            let mut snapshot = MeerkatLedgerSnapshot {
                input_count: 0,
                non_terminal_count: 0,
                accepted_count: 0,
                queued_count: 0,
                staged_count: 0,
                applied_count: 0,
                applied_pending_consumption_count: 0,
                consumed_count: 0,
                superseded_count: 0,
                coalesced_count: 0,
                abandoned_count: 0,
            };

            for (_input_id, state) in driver.ledger().iter() {
                snapshot.input_count += 1;
                let lifecycle = state.current_state();
                if !lifecycle.is_terminal() {
                    snapshot.non_terminal_count += 1;
                }
                match lifecycle {
                    InputLifecycleState::Accepted => snapshot.accepted_count += 1,
                    InputLifecycleState::Queued => snapshot.queued_count += 1,
                    InputLifecycleState::Staged => snapshot.staged_count += 1,
                    InputLifecycleState::Applied => snapshot.applied_count += 1,
                    InputLifecycleState::AppliedPendingConsumption => {
                        snapshot.applied_pending_consumption_count += 1;
                    }
                    InputLifecycleState::Consumed => snapshot.consumed_count += 1,
                    InputLifecycleState::Superseded => snapshot.superseded_count += 1,
                    InputLifecycleState::Coalesced => snapshot.coalesced_count += 1,
                    InputLifecycleState::Abandoned => snapshot.abandoned_count += 1,
                }
            }

            snapshot
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
        let formal_state = {
            let mut available_fields = std::collections::BTreeMap::new();
            available_fields.insert(
                "session_id".into(),
                formal_projection_value(&Some(session_id.to_string())),
            );
            available_fields.insert(
                "active_runtime_id".into(),
                formal_projection_value(&Some(driver.runtime_id().to_string())),
            );
            available_fields.insert(
                "current_run_id".into(),
                formal_projection_value(&control.current_run_id.as_ref().map(ToString::to_string)),
            );
            available_fields.insert(
                "pre_run_phase".into(),
                formal_projection_value(&control.pre_run_phase.map(|phase| phase.to_string())),
            );
            available_fields.insert(
                "silent_intent_overrides".into(),
                formal_projection_value(
                    &driver
                        .silent_comms_intents()
                        .into_iter()
                        .collect::<BTreeSet<_>>(),
                ),
            );
            MeerkatFormalStateProjection {
                available_fields,
                unavailable_fields: vec!["active_fence_token".into()],
            }
        };

        Some(MeerkatMachineSpineSnapshot {
            binding,
            control,
            inputs,
            ledger,
            completion_waiters,
            ops,
            drain,
            formal_state,
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
            .execute_meerkat_machine_command(
                Some(Arc::clone(self)),
                MeerkatMachineCommand::SetPeerIngressContext {
                    session_id: session_id.clone(),
                    keep_alive,
                    comms_runtime,
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::Spawned(spawned)) => spawned,
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
            .execute_meerkat_machine_command(
                Some(Arc::clone(self)),
                MeerkatMachineCommand::SetPeerIngressContext {
                    session_id: session_id.clone(),
                    keep_alive,
                    comms_runtime,
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::Spawned(spawned)) => spawned,
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
                .execute_meerkat_machine_drain_local_command(MeerkatMachineCommand::Abort {
                    session_id: session_id.clone(),
                })
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
            .execute_meerkat_machine_command(
                Some(Arc::clone(self)),
                MeerkatMachineCommand::NotifyDrainExited {
                    session_id: session_id.clone(),
                    reason,
                },
            )
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
            .execute_meerkat_machine_command(None, MeerkatMachineCommand::AbortAll)
            .await;
    }

    /// Abort the comms drain task for a specific session.
    pub async fn abort_comms_drain(&self, session_id: &SessionId) {
        let _ = self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Abort {
                    session_id: session_id.clone(),
                },
            )
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
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Wait {
                    session_id: session_id.clone(),
                },
            )
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
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Ingest { runtime_id, input },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::AcceptOutcome(outcome) => Ok(outcome),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::accept_input: {other:?}"
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
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::RuntimeState { runtime_id },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::RuntimeState(state) => Ok(state),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::runtime_state: {other:?}"
            ))),
        }
    }

    async fn retire_runtime(
        &self,
        session_id: &SessionId,
    ) -> Result<RetireReport, RuntimeDriverError> {
        let runtime_id = MeerkatMachine::logical_runtime_id(session_id);
        match self
            .execute_meerkat_machine_command(None, MeerkatMachineCommand::Retire { runtime_id })
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::RetireReport(report) => Ok(report),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::retire_runtime: {other:?}"
            ))),
        }
    }

    async fn reset_runtime(
        &self,
        session_id: &SessionId,
    ) -> Result<ResetReport, RuntimeDriverError> {
        let runtime_id = MeerkatMachine::logical_runtime_id(session_id);
        match self
            .execute_meerkat_machine_command(None, MeerkatMachineCommand::Reset { runtime_id })
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::ResetReport(report) => Ok(report),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::reset_runtime: {other:?}"
            ))),
        }
    }

    async fn input_state(
        &self,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<InputState>, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::InputState {
                    session_id: session_id.clone(),
                    input_id: input_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::InputState(state) => Ok(state),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::input_state: {other:?}"
            ))),
        }
    }

    async fn list_active_inputs(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<InputId>, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::ListActiveInputs {
                    session_id: session_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::ActiveInputs(inputs) => Ok(inputs),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::list_active_inputs: {other:?}"
            ))),
        }
    }

    async fn reconfigure_session_llm_identity(
        &self,
        session_id: &SessionId,
        request: SessionLlmReconfigureRequest,
    ) -> Result<SessionLlmReconfigureReport, RuntimeDriverError> {
        let command = self
            .prepare_reconfigure_session_llm_command(session_id, request)
            .await?;
        match self
            .execute_meerkat_machine_command(None, command)
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::LlmReconfigured(report) => Ok(report),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::reconfigure_session_llm_identity: {other:?}"
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

    fn machine_owned_admission_signal(
        outcome: &AcceptOutcome,
        request_immediate_processing: bool,
    ) -> crate::driver::ephemeral::PostAdmissionSignal {
        crate::accept::post_admission_signal_from_accept_outcome(
            outcome,
            request_immediate_processing,
        )
    }

    fn driver_error_from_command_error(err: MeerkatMachineCommandError) -> RuntimeDriverError {
        match err {
            MeerkatMachineCommandError::Driver(err) => err,
            MeerkatMachineCommandError::Control(err) => {
                Self::driver_error_from_control_plane_error(err)
            }
        }
    }

    fn control_plane_error_from_command_error(
        err: MeerkatMachineCommandError,
    ) -> RuntimeControlPlaneError {
        match err {
            MeerkatMachineCommandError::Control(err) => err,
            MeerkatMachineCommandError::Driver(err) => {
                RuntimeControlPlaneError::Internal(err.to_string())
            }
        }
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
        let control = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .map(RuntimeSessionEntry::control_snapshot)
        }?;
        Some(control.phase)
    }

    async fn driver_runtime_state(driver: &SharedDriver) -> RuntimeState {
        let control_handle = {
            let driver = driver.lock().await;
            driver.control_projection_handle()
        };
        control_handle
            .read()
            .map(|guard| guard.phase)
            .unwrap_or_else(|poisoned| {
                tracing::error!("runtime control projection lock poisoned");
                poisoned.into_inner().phase
            })
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
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Ingest {
                    runtime_id: runtime_id.clone(),
                    input,
                },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::AcceptOutcome(outcome) => Ok(outcome),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for ingest: {other:?}"
            ))),
        }
    }

    async fn publish_event(
        &self,
        event: crate::runtime_event::RuntimeEventEnvelope,
    ) -> Result<(), RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_command(None, MeerkatMachineCommand::PublishEvent { event })
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::Unit => Ok(()),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for publish_event: {other:?}"
            ))),
        }
    }

    async fn retire(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Retire {
                    runtime_id: runtime_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::RetireReport(report) => Ok(report),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for retire: {other:?}"
            ))),
        }
    }

    async fn recycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RecycleReport, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Recycle {
                    runtime_id: runtime_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::RecycleReport(report) => Ok(report),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for recycle: {other:?}"
            ))),
        }
    }

    async fn reset(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<crate::traits::ResetReport, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Reset {
                    runtime_id: runtime_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::ResetReport(report) => Ok(report),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for reset: {other:?}"
            ))),
        }
    }

    async fn recover(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RecoveryReport, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Recover {
                    runtime_id: runtime_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::RecoveryReport(report) => Ok(report),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for recover: {other:?}"
            ))),
        }
    }

    async fn destroy(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<DestroyReport, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Destroy {
                    runtime_id: runtime_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::DestroyReport(report) => Ok(report),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for destroy: {other:?}"
            ))),
        }
    }

    async fn runtime_state(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RuntimeState, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::RuntimeState {
                    runtime_id: runtime_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::RuntimeState(state) => Ok(state),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for runtime_state: {other:?}"
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
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::LoadBoundaryReceipt {
                    runtime_id: runtime_id.clone(),
                    run_id: run_id.clone(),
                    sequence,
                },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::BoundaryReceipt(receipt) => Ok(receipt),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for load_boundary_receipt: {other:?}"
            ))),
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
#[path = "meerkat_machine_tests.rs"]
mod tests;
