//! PersistentRuntimeDriver — wraps EphemeralRuntimeDriver + RuntimeStore.
//!
//! Provides durable-before-ack guarantee: InputState is persisted via
//! RuntimeStore BEFORE returning AcceptOutcome. Delegates state machine
//! logic to the ephemeral driver.

use std::sync::Arc;
use std::sync::RwLock as StdRwLock;

use meerkat_core::BlobStore;
use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};

use crate::accept::AcceptOutcome;
use crate::identifiers::LogicalRuntimeId;
use crate::input::{Input, externalize_input_images};
use crate::input_state::{
    InputAbandonReason, InputLifecycleState, InputState, InputStatePersistenceRecord,
    StoredInputState,
};
use crate::runtime_event::RuntimeEventEnvelope;
use crate::runtime_state::RuntimeState;
use crate::store::{MachineLifecycleCommit, RuntimeStore};
use crate::traits::{DestroyReport, RecoveryReport, RuntimeDriver, RuntimeDriverError};

use super::ephemeral::{
    EphemeralDriverRollbackSnapshot, EphemeralRuntimeDriver, SharedIngressDslAuthority,
};

/// Persistent runtime driver — durable InputState via RuntimeStore.
pub struct PersistentRuntimeDriver {
    /// Underlying ephemeral driver for state machine logic.
    inner: EphemeralRuntimeDriver,
    /// Durable store for InputState + receipts.
    store: Arc<dyn RuntimeStore>,
    /// Blob store used to externalize durable input payloads.
    blob_store: Arc<dyn BlobStore>,
    /// Runtime ID for store operations.
    runtime_id: LogicalRuntimeId,
    /// Test-only fault injection: forces the input-state snapshot step of
    /// [`Self::commit_lifecycle_with_rollback`] to fail so tests can pin the
    /// checkpoint-restore contract for that arm.
    #[cfg(test)]
    pub(crate) force_input_snapshot_failure_for_test: bool,
}

impl PersistentRuntimeDriver {
    /// Create a new persistent runtime driver.
    pub fn new(
        runtime_id: LogicalRuntimeId,
        store: Arc<dyn RuntimeStore>,
        blob_store: Arc<dyn BlobStore>,
    ) -> Self {
        Self::new_with_control(
            runtime_id,
            store,
            blob_store,
            Arc::new(StdRwLock::new(
                crate::driver::ephemeral::RuntimeControlProjection::default(),
            )),
            crate::driver::ephemeral::new_ingress_dsl_authority(),
        )
    }

    pub(crate) fn new_with_control(
        runtime_id: LogicalRuntimeId,
        store: Arc<dyn RuntimeStore>,
        blob_store: Arc<dyn BlobStore>,
        control: Arc<StdRwLock<crate::driver::ephemeral::RuntimeControlProjection>>,
        dsl: SharedIngressDslAuthority,
    ) -> Self {
        Self {
            inner: EphemeralRuntimeDriver::new_with_control_and_dsl(
                runtime_id.clone(),
                control,
                dsl,
            ),
            store,
            blob_store,
            runtime_id,
            #[cfg(test)]
            force_input_snapshot_failure_for_test: false,
        }
    }

    /// Get immutable reference to the inner ephemeral driver.
    pub fn inner_ref(&self) -> &EphemeralRuntimeDriver {
        &self.inner
    }

    pub(crate) fn rollback_snapshot(&self) -> EphemeralDriverRollbackSnapshot {
        self.inner.rollback_snapshot()
    }

    pub(crate) fn restore_rollback_snapshot(&mut self, snapshot: EphemeralDriverRollbackSnapshot) {
        self.inner.restore_rollback_snapshot(snapshot);
    }

    /// Get the logical runtime ID for this driver.
    pub fn runtime_id(&self) -> &LogicalRuntimeId {
        &self.runtime_id
    }

    pub fn silent_comms_intents(&self) -> Vec<String> {
        self.inner.silent_comms_intents()
    }

    /// Check if the runtime is idle (delegates to inner).
    pub fn is_idle(&self) -> bool {
        self.inner.is_idle()
    }

    /// Ask generated MeerkatMachine authority for the store-visible lifecycle.
    fn runtime_state_for_persistence(&self) -> Result<RuntimeState, RuntimeDriverError> {
        Self::runtime_state_for_persistence_from_inner(&self.inner)
    }

    fn runtime_state_for_persistence_from_inner(
        inner: &EphemeralRuntimeDriver,
    ) -> Result<RuntimeState, RuntimeDriverError> {
        crate::meerkat_machine::classify_runtime_lifecycle_durable_state(inner.runtime_state())
            .map_err(|err| {
                RuntimeDriverError::Internal(format!(
                    "generated runtime lifecycle durability classification failed: {err}"
                ))
            })
    }

    fn lifecycle_commit_for_persistence(
        &self,
    ) -> Result<MachineLifecycleCommit, RuntimeDriverError> {
        Self::lifecycle_commit_for_persistence_from_inner(&self.inner)
    }

    fn lifecycle_commit_for_persistence_from_inner(
        inner: &EphemeralRuntimeDriver,
    ) -> Result<MachineLifecycleCommit, RuntimeDriverError> {
        Ok(MachineLifecycleCommit::new_with_binding(
            Self::runtime_state_for_persistence_from_inner(inner)?,
            inner.machine_lifecycle_binding_facts(),
        ))
    }

    /// Snapshot + classify the lifecycle persistence payload, restoring the
    /// caller's checkpoint on failure.
    ///
    /// Contract (Dogma K11): every fallible step between a staged `&mut` DSL
    /// transition and the rollback-guarded durable commit restores the
    /// caller's checkpoint. A bare `?` here would leave the staged lifecycle
    /// live in driver state while reporting failure to the caller. The
    /// checkpoint is returned on success so the durable commit arm can keep
    /// using it.
    fn lifecycle_persistence_payload_with_rollback(
        &mut self,
        checkpoint: super::ephemeral::EphemeralDriverRollbackSnapshot,
        context: &str,
    ) -> Result<
        (
            super::ephemeral::EphemeralDriverRollbackSnapshot,
            Vec<InputStatePersistenceRecord>,
            MachineLifecycleCommit,
        ),
        RuntimeDriverError,
    > {
        let input_states_result = self.inner.authorized_stored_input_states_snapshot();
        #[cfg(test)]
        let input_states_result = if self.force_input_snapshot_failure_for_test {
            Err(RuntimeDriverError::Internal(
                "forced input-state snapshot failure for checkpoint-restore contract test"
                    .to_string(),
            ))
        } else {
            input_states_result
        };
        let input_states = match input_states_result {
            Ok(input_states) => input_states,
            Err(err) => {
                self.inner.restore_rollback_snapshot(checkpoint);
                return Err(RuntimeDriverError::Internal(format!(
                    "{context} input-state snapshot failed: {err}"
                )));
            }
        };
        let commit = match self.lifecycle_commit_for_persistence() {
            Ok(commit) => commit,
            Err(err) => {
                self.inner.restore_rollback_snapshot(checkpoint);
                return Err(RuntimeDriverError::Internal(format!(
                    "{context} lifecycle commit classification failed: {err}"
                )));
            }
        };
        Ok((checkpoint, input_states, commit))
    }

    async fn commit_lifecycle_with_rollback(
        &mut self,
        checkpoint: super::ephemeral::EphemeralDriverRollbackSnapshot,
        target_state: RuntimeState,
        context: &str,
    ) -> Result<(), RuntimeDriverError> {
        // Contract: every fallible step between the staged DSL transition and
        // the durable commit restores the caller's checkpoint on failure. A
        // bare `?` here would leave the staged lifecycle (e.g. Destroy) live
        // in driver state while reporting failure to the caller.
        let (checkpoint, input_states, commit) =
            self.lifecycle_persistence_payload_with_rollback(checkpoint, context)?;
        let target_durable_state =
            match crate::meerkat_machine::classify_runtime_lifecycle_durable_state(target_state) {
                Ok(target_durable_state) => target_durable_state,
                Err(err) => {
                    self.inner.restore_rollback_snapshot(checkpoint);
                    return Err(RuntimeDriverError::Internal(format!(
                        "{context} generated target lifecycle durability classification failed: {err}"
                    )));
                }
            };
        if commit.runtime_state() != target_durable_state {
            self.inner.restore_rollback_snapshot(checkpoint);
            return Err(RuntimeDriverError::Internal(format!(
                "{context} durable persist target {target_durable_state:?} from live {target_state:?} disagreed with generated lifecycle commit {:?}",
                commit.runtime_state()
            )));
        }
        if let Err(err) = self
            .store
            .commit_machine_lifecycle(&self.runtime_id, commit, &input_states)
            .await
        {
            self.inner.restore_rollback_snapshot(checkpoint);
            return Err(RuntimeDriverError::Internal(format!(
                "{context} persist failed: {err}"
            )));
        }
        Ok(())
    }

    pub(crate) async fn publish_service_turn_terminal_lifecycle(
        &mut self,
        checkpoint: super::ephemeral::EphemeralDriverRollbackSnapshot,
        target_state: RuntimeState,
    ) -> Result<(), RuntimeDriverError> {
        self.commit_lifecycle_with_rollback(
            checkpoint,
            target_state,
            "service turn terminal receipt",
        )
        .await?;
        self.inner.sync_control_projection_from_dsl_authority();
        Ok(())
    }

    pub(crate) fn set_control_projection(
        &mut self,
        next_phase: RuntimeState,
        current_run_id: Option<RunId>,
        pre_run_phase: Option<RuntimeState>,
    ) {
        self.inner
            .set_control_projection(next_phase, current_run_id, pre_run_phase);
    }

    /// Low-level control projection shim for external contract tests.
    ///
    /// This does not decide lifecycle legality; it only applies an already
    /// chosen MeerkatMachine control projection to the concrete driver shell.
    pub(crate) fn sync_control_projection_from_dsl_authority(&mut self) {
        self.inner.sync_control_projection_from_dsl_authority();
    }

    pub(crate) async fn persist_current_machine_lifecycle(
        &mut self,
        context: &str,
    ) -> Result<(), RuntimeDriverError> {
        let input_states = self.inner.authorized_stored_input_states_snapshot()?;
        let commit = self.lifecycle_commit_for_persistence()?;
        self.store
            .commit_machine_lifecycle(&self.runtime_id, commit, &input_states)
            .await
            .map_err(|err| {
                RuntimeDriverError::Internal(format!("{context} lifecycle persist failed: {err}"))
            })
    }

    /// Contract helper for external tests that need to start a run through the
    /// same DSL authority used by the runtime loop.
    #[doc(hidden)]
    pub fn contract_begin_run_authority(
        &mut self,
        run_id: RunId,
    ) -> Result<(), RuntimeDriverError> {
        self.inner.contract_begin_run_authority(run_id)
    }

    /// Test-only authority override for crate-unit tests that need to seed
    /// impossible or already-realized runtime phases.
    #[cfg(test)]
    #[doc(hidden)]
    pub(crate) fn contract_force_runtime_authority(
        &mut self,
        next_phase: RuntimeState,
        current_run_id: Option<RunId>,
        pre_run_phase: Option<RuntimeState>,
    ) {
        self.inner
            .contract_force_runtime_authority(next_phase, current_run_id, pre_run_phase);
    }

    /// Get pending events (delegates to inner).
    pub fn drain_events(&mut self) -> Vec<RuntimeEventEnvelope> {
        self.inner.drain_events()
    }

    /// Drain the typed post-admission signal (delegates to inner).
    pub fn take_post_admission_signal(&mut self) -> crate::driver::ephemeral::PostAdmissionSignal {
        self.inner.take_post_admission_signal()
    }

    /// Inspect the current typed post-admission signal without draining it.
    pub fn post_admission_signal(&self) -> crate::driver::ephemeral::PostAdmissionSignal {
        self.inner.post_admission_signal()
    }

    /// Check and clear wake flag (backward-compat, delegates to inner).
    pub fn take_wake_requested(&mut self) -> bool {
        self.inner.take_wake_requested()
    }

    /// Check and clear immediate processing flag (backward-compat, delegates to inner).
    pub fn take_process_requested(&mut self) -> bool {
        self.inner.take_process_requested()
    }

    /// Contract helper for recovery/queue-projection tests. Production runtime
    /// execution must use generated batch authority via `dequeue_batch_exact`.
    #[cfg(any(test, debug_assertions, feature = "test-support"))]
    #[doc(hidden)]
    pub fn contract_dequeue_next_for_recovery_tests(&mut self) -> Option<(InputId, Input)> {
        self.inner.contract_dequeue_next_for_recovery_tests()
    }

    pub(crate) fn dequeue_batch_exact(
        &mut self,
        batch: &crate::meerkat_machine::driver::AuthorizedRuntimeLoopBatch,
    ) -> Result<Vec<(InputId, Input)>, RuntimeDriverError> {
        self.inner.dequeue_batch_exact(batch)
    }

    pub fn has_queued_input_outside(&self, excluded: &[InputId]) -> bool {
        self.inner.has_queued_input_outside(excluded)
    }

    pub(crate) fn defer_queued_inputs_behind_backlog(
        &mut self,
        input_ids: &[InputId],
    ) -> Result<(), RuntimeDriverError> {
        self.inner.defer_queued_inputs_behind_backlog(input_ids)
    }

    pub(crate) fn absorb_post_admission_effects(
        &mut self,
        effects: &[crate::meerkat_machine::dsl::MeerkatMachineEffect],
    ) {
        self.inner.absorb_post_admission_effects(effects);
    }

    pub(crate) fn resolve_admission(
        &self,
        input: &Input,
    ) -> Result<crate::accept::ResolvedAdmission, RuntimeDriverError> {
        self.inner.resolve_admission(input)
    }

    pub(crate) fn resolve_admission_with_active_turn_boundary(
        &self,
        input: &Input,
        active_turn_boundary_available: bool,
    ) -> Result<crate::accept::ResolvedAdmission, RuntimeDriverError> {
        self.inner
            .resolve_admission_with_active_turn_boundary(input, active_turn_boundary_available)
    }

    pub(crate) fn resolve_admission_without_wake_with_active_turn_boundary(
        &self,
        input: &Input,
        active_turn_boundary_available: bool,
    ) -> Result<crate::accept::ResolvedAdmission, RuntimeDriverError> {
        self.inner
            .resolve_admission_without_wake_with_active_turn_boundary(
                input,
                active_turn_boundary_available,
            )
    }

    pub(crate) async fn accept_resolved_input(
        &mut self,
        input: Input,
        resolved: crate::accept::ResolvedAdmission,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        let mut staged = self.inner.clone_with_isolated_dsl_authority();
        staged.ensure_contract_session_authority()?;
        let staged_resolved = if resolved.authority().without_wake() {
            staged.resolve_admission_without_wake_with_active_turn_boundary(
                &input,
                resolved.authority().active_turn_boundary_available(),
            )?
        } else {
            staged.resolve_admission_with_active_turn_boundary(
                &input,
                resolved.authority().active_turn_boundary_available(),
            )?
        };
        if !resolved.semantically_equivalent_to(&staged_resolved) {
            return Err(RuntimeDriverError::Internal(format!(
                "staged admission resolution diverged from preview: preview={resolved:?}, staged={staged_resolved:?}"
            )));
        }
        let flags = staged_resolved.coarse_flags();
        let staged_outcome = staged
            .accept_resolved_input(input.clone(), staged_resolved)
            .await?;

        let AcceptOutcome::Accepted {
            input_id: staged_input_id,
            ..
        } = staged_outcome
        else {
            return self.inner.accept_resolved_input(input, resolved).await;
        };

        staged.machine_apply_accept_with_completion_signal(&staged_input_id, flags)?;
        let Some(mut staged_bundle) = staged.stored_input_state(&staged_input_id) else {
            return Err(RuntimeDriverError::Internal(format!(
                "generated input lifecycle phase missing for accepted input {staged_input_id}"
            )));
        };
        let mut input_for_recovery = input.clone();
        externalize_input_images(self.blob_store.as_ref(), &mut input_for_recovery)
            .await
            .map_err(|err| {
                RuntimeDriverError::Internal(format!(
                    "failed to externalize runtime input images: {err}"
                ))
            })?;
        staged_bundle.state.persisted_input = Some(input_for_recovery.clone());
        self.persist_state(&staged_bundle).await?;

        self.inner.ensure_contract_session_authority()?;
        let mut outcome = self.inner.accept_resolved_input(input, resolved).await?;
        if let AcceptOutcome::Accepted {
            ref input_id,
            ref mut state,
            ref mut seed,
            ..
        } = outcome
        {
            if input_id != &staged_input_id {
                return Err(RuntimeDriverError::Internal(format!(
                    "staged accepted input {staged_input_id} differed from committed input {input_id}"
                )));
            }
            self.inner
                .machine_apply_accept_with_completion_signal(input_id, flags)?;
            let Some(mut bundle) = self.inner.stored_input_state(input_id) else {
                return Err(RuntimeDriverError::Internal(format!(
                    "generated input lifecycle phase missing for accepted input {input_id}"
                )));
            };
            bundle.state.persisted_input = Some(input_for_recovery);
            self.inner.ledger_mut().accept(bundle.state.clone());
            *state = bundle.state;
            *seed = bundle.seed;
        }

        Ok(outcome)
    }

    pub(crate) async fn preview_accept_resolved_input(
        &self,
        input: Input,
        resolved: &crate::accept::ResolvedAdmission,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        let mut staged = self.inner.clone_with_isolated_dsl_authority();
        staged.ensure_contract_session_authority()?;
        let staged_resolved = if resolved.authority().without_wake() {
            staged.resolve_admission_without_wake_with_active_turn_boundary(
                &input,
                resolved.authority().active_turn_boundary_available(),
            )?
        } else {
            staged.resolve_admission_with_active_turn_boundary(
                &input,
                resolved.authority().active_turn_boundary_available(),
            )?
        };
        if !resolved.semantically_equivalent_to(&staged_resolved) {
            return Err(RuntimeDriverError::Internal(format!(
                "staged admission preview diverged from caller resolution: preview={resolved:?}, staged={staged_resolved:?}"
            )));
        }
        staged.accept_resolved_input(input, staged_resolved).await
    }

    pub(crate) fn machine_realize_authorized_stage_batch(
        &mut self,
        authority: crate::meerkat_machine::driver::AuthorizedStageForRun,
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        self.inner.machine_realize_authorized_stage_batch(authority)
    }

    /// Apply input (delegates to inner).
    pub fn apply_input(
        &mut self,
        input_id: &InputId,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        self.inner.apply_input(input_id, run_id)
    }

    /// Roll back staged inputs (delegates to inner).
    pub fn rollback_staged(
        &mut self,
        input_ids: &[InputId],
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        self.inner.rollback_staged(input_ids)
    }

    async fn persist_state(&self, state: &StoredInputState) -> Result<(), RuntimeDriverError> {
        let state = InputStatePersistenceRecord::from_machine_snapshot(state.clone())
            .map_err(RuntimeDriverError::Internal)?;
        self.store
            .persist_input_state(&self.runtime_id, &state)
            .await
            .map_err(|e| RuntimeDriverError::Internal(e.to_string()))
    }

    pub(crate) async fn abandon_pending_inputs(
        &mut self,
        reason: InputAbandonReason,
    ) -> Result<usize, RuntimeDriverError> {
        let checkpoint = self.inner.rollback_snapshot();
        let abandoned = match self.inner.abandon_pending_inputs(reason) {
            Ok(abandoned) => abandoned,
            Err(err) => {
                self.inner.restore_rollback_snapshot(checkpoint);
                return Err(err);
            }
        };
        let (checkpoint, input_states, commit) =
            self.lifecycle_persistence_payload_with_rollback(checkpoint, "pending input abandon")?;
        if let Err(err) = self
            .store
            .commit_machine_lifecycle(&self.runtime_id, commit, &input_states)
            .await
        {
            self.inner.restore_rollback_snapshot(checkpoint);
            return Err(RuntimeDriverError::Internal(format!(
                "pending input abandon persist failed: {err}"
            )));
        }
        Ok(abandoned)
    }

    /// Recycle the in-memory driver shell while preserving canonical pending
    /// work from durable runtime truth.
    ///
    /// Unlike `reset()`, this must not abandon queued/staged work.
    pub(crate) async fn recycle_preserving_work(&mut self) -> Result<usize, RuntimeDriverError> {
        let checkpoint = self.inner.rollback_snapshot();
        let transferred = match self.inner.recycle_preserving_work() {
            Ok(transferred) => transferred,
            Err(err) => {
                self.inner.restore_rollback_snapshot(checkpoint);
                return Err(err);
            }
        };
        let (checkpoint, input_states, commit) =
            self.lifecycle_persistence_payload_with_rollback(checkpoint, "recycle")?;
        if let Err(err) = self
            .store
            .commit_machine_lifecycle(&self.runtime_id, commit, &input_states)
            .await
        {
            self.inner.restore_rollback_snapshot(checkpoint);
            return Err(RuntimeDriverError::Internal(format!(
                "recycle persist failed: {err}"
            )));
        }

        self.inner.sync_control_projection_from_dsl_authority();
        Ok(transferred)
    }

    pub(crate) async fn realize_retire_lifecycle(
        &mut self,
    ) -> Result<crate::traits::RetireReport, RuntimeDriverError> {
        let checkpoint = self.inner.rollback_snapshot();
        let report = self.inner.finalize_retire();
        // Restore the checkpoint on classification failure: an early `?` here
        // would leave the finalized retire state live without rollback.
        let target_state = match self.runtime_state_for_persistence() {
            Ok(target_state) => target_state,
            Err(err) => {
                self.inner.restore_rollback_snapshot(checkpoint);
                return Err(err);
            }
        };
        self.commit_lifecycle_with_rollback(checkpoint, target_state, "retire")
            .await?;
        self.inner.sync_control_projection_from_dsl_authority();
        Ok(report)
    }

    pub(crate) async fn realize_reset_lifecycle(
        &mut self,
    ) -> Result<crate::traits::ResetReport, RuntimeDriverError> {
        let checkpoint = self.inner.rollback_snapshot();
        let report = match self.inner.reset_cleanup() {
            Ok(report) => report,
            Err(err) => {
                self.inner.restore_rollback_snapshot(checkpoint);
                return Err(err);
            }
        };
        // Restore the checkpoint on classification failure: an early `?` here
        // would leave the reset-cleaned state live without rollback.
        let target_state = match self.runtime_state_for_persistence() {
            Ok(target_state) => target_state,
            Err(err) => {
                self.inner.restore_rollback_snapshot(checkpoint);
                return Err(err);
            }
        };
        self.commit_lifecycle_with_rollback(checkpoint, target_state, "reset")
            .await?;
        self.inner.sync_control_projection_from_dsl_authority();
        Ok(report)
    }

    pub(crate) fn prepare_destroy_lifecycle(
        &mut self,
    ) -> Result<(EphemeralDriverRollbackSnapshot, DestroyReport), RuntimeDriverError> {
        let checkpoint = self.inner.rollback_snapshot();
        let abandoned = match self.inner.destroy_cleanup() {
            Ok(abandoned) => abandoned,
            Err(err) => {
                self.inner.restore_rollback_snapshot(checkpoint);
                return Err(err);
            }
        };
        Ok((
            checkpoint,
            DestroyReport {
                inputs_abandoned: abandoned,
            },
        ))
    }

    pub(crate) async fn commit_prepared_destroy_lifecycle(
        &mut self,
        checkpoint: EphemeralDriverRollbackSnapshot,
    ) -> Result<(), RuntimeDriverError> {
        // Resolve the durable target BEFORE handing the checkpoint to the
        // commit helper: an early `?` here would otherwise leave the staged
        // destroy state live without restoring the checkpoint (driver-side
        // shadow truth with no rollback).
        let target_state = match self.runtime_state_for_persistence() {
            Ok(target_state) => target_state,
            Err(err) => {
                self.inner.restore_rollback_snapshot(checkpoint);
                return Err(err);
            }
        };
        self.commit_lifecycle_with_rollback(checkpoint, target_state, "destroy")
            .await
    }

    pub(crate) fn rollback_prepared_destroy_lifecycle(
        &mut self,
        checkpoint: EphemeralDriverRollbackSnapshot,
    ) {
        self.inner.restore_rollback_snapshot(checkpoint);
    }

    pub(crate) async fn finalize_runtime_executor_exit(
        &mut self,
    ) -> Result<(), RuntimeDriverError> {
        let checkpoint = self.inner.rollback_snapshot();
        if let Err(err) = self.inner.apply_runtime_executor_exited_authority() {
            self.inner.restore_rollback_snapshot(checkpoint);
            return Err(err);
        }
        if let Err(err) = self.inner.stop_runtime_cleanup() {
            self.inner.restore_rollback_snapshot(checkpoint);
            return Err(err);
        }
        // Resolve the durable target BEFORE handing the checkpoint to the
        // commit helper, so a classification failure restores the staged
        // executor-exit state instead of leaving it live without rollback.
        let target_state = match self.runtime_state_for_persistence() {
            Ok(target_state) => target_state,
            Err(err) => {
                self.inner.restore_rollback_snapshot(checkpoint);
                return Err(err);
            }
        };
        self.commit_lifecycle_with_rollback(checkpoint, target_state, "stop")
            .await?;
        self.inner.sync_control_projection_from_dsl_authority();
        Ok(())
    }

    pub(crate) fn machine_realize_boundary_applied_in_memory(
        &mut self,
        run_id: &RunId,
        receipt: &RunBoundaryReceipt,
    ) -> Result<(), RuntimeDriverError> {
        self.inner.machine_realize_boundary_applied(run_id, receipt)
    }

    pub(crate) fn machine_realize_run_completed_in_memory(
        &mut self,
        run_id: &RunId,
        consumed_input_ids: &[InputId],
    ) -> Result<(), RuntimeDriverError> {
        self.inner
            .machine_realize_run_completed(run_id, consumed_input_ids)
    }

    pub(crate) async fn machine_realize_live_boundary_context_injected(
        &mut self,
        run_id: &RunId,
        input_ids: &[InputId],
        stage_authority: crate::meerkat_machine::driver::AuthorizedStageForRun,
        session_snapshot: Option<Vec<u8>>,
    ) -> Result<(), RuntimeDriverError> {
        let checkpoint = self.inner.rollback_snapshot();
        let receipt = match self.inner.machine_realize_live_boundary_context_injected(
            run_id,
            input_ids,
            stage_authority,
        ) {
            Ok(receipt) => receipt,
            Err(err) => {
                self.inner.restore_rollback_snapshot(checkpoint);
                return Err(err);
            }
        };
        let input_updates = match self.inner.authorized_stored_input_states_snapshot() {
            Ok(input_updates) => input_updates,
            Err(err) => {
                self.inner.restore_rollback_snapshot(checkpoint);
                return Err(err);
            }
        };
        if let Err(err) = self
            .store
            .atomic_apply(
                &self.runtime_id,
                session_snapshot
                    .as_ref()
                    .map(|session_snapshot| crate::store::SessionDelta {
                        session_snapshot: session_snapshot.clone(),
                    }),
                receipt.clone(),
                input_updates,
                session_snapshot
                    .as_deref()
                    .and_then(|snapshot| {
                        serde_json::from_slice::<meerkat_core::Session>(snapshot).ok()
                    })
                    .map(|session| session.id().clone()),
            )
            .await
        {
            self.inner.restore_rollback_snapshot(checkpoint);
            return Err(RuntimeDriverError::Internal(format!(
                "runtime live-boundary context commit failed: {err}"
            )));
        }
        Ok(())
    }

    pub(crate) async fn machine_commit_completed_boundary_snapshot(
        &mut self,
        receipt: &RunBoundaryReceipt,
        session_snapshot: Option<&Vec<u8>>,
    ) -> Result<(), RuntimeDriverError> {
        let input_updates = self.inner.authorized_stored_input_states_snapshot()?;
        self.store
            .atomic_apply(
                &self.runtime_id,
                session_snapshot.map(|session_snapshot| crate::store::SessionDelta {
                    session_snapshot: session_snapshot.clone(),
                }),
                receipt.clone(),
                input_updates,
                session_snapshot
                    .and_then(|snapshot| {
                        serde_json::from_slice::<meerkat_core::Session>(snapshot).ok()
                    })
                    .map(|session| session.id().clone()),
            )
            .await
            .map_err(|e| {
                RuntimeDriverError::Internal(format!(
                    "runtime completed-boundary commit failed: {e}"
                ))
            })
    }

    pub(crate) async fn machine_realize_run_failed(
        &mut self,
        run_id: &RunId,
        contributing_input_ids: &[InputId],
        replay_plan: &super::ephemeral::ReplayQueuedContributorsPlan,
        terminal_error: &str,
        runtime_apply_failure: Option<&meerkat_core::lifecycle::CoreApplyFailureCause>,
        recoverable: bool,
    ) -> Result<(), RuntimeDriverError> {
        let checkpoint = self.inner.rollback_snapshot();
        if let Err(err) =
            self.inner
                .machine_realize_run_failed(run_id, contributing_input_ids, replay_plan)
        {
            self.inner.restore_rollback_snapshot(checkpoint);
            return Err(err);
        }
        let failure_cause = runtime_apply_failure.map(|failure| failure.kind);
        tracing::debug!(
            run_id = ?run_id,
            recoverable,
            error = terminal_error,
            failure_cause = ?failure_cause,
            "persistent driver realized machine-owned failed-run replay"
        );
        let (checkpoint, input_states, commit) = self
            .lifecycle_persistence_payload_with_rollback(checkpoint, "failed-run terminal event")?;
        if let Err(err) = self
            .store
            .commit_machine_lifecycle(&self.runtime_id, commit, &input_states)
            .await
        {
            self.inner.restore_rollback_snapshot(checkpoint);
            return Err(RuntimeDriverError::Internal(format!(
                "terminal event persist failed: {err}"
            )));
        }
        Ok(())
    }

    pub(crate) async fn machine_realize_run_cancelled(
        &mut self,
        run_id: &RunId,
        contributing_input_ids: &[InputId],
    ) -> Result<(), RuntimeDriverError> {
        let checkpoint = self.inner.rollback_snapshot();
        if let Err(err) = self
            .inner
            .machine_realize_run_cancelled(run_id, contributing_input_ids)
        {
            self.inner.restore_rollback_snapshot(checkpoint);
            return Err(err);
        }
        tracing::debug!(
            run_id = ?run_id,
            contributors = contributing_input_ids.len(),
            "persistent driver realized machine-owned cancelled run"
        );
        let (checkpoint, input_states, commit) = self.lifecycle_persistence_payload_with_rollback(
            checkpoint,
            "cancelled-run terminal event",
        )?;
        if let Err(err) = self
            .store
            .commit_machine_lifecycle(&self.runtime_id, commit, &input_states)
            .await
        {
            self.inner.restore_rollback_snapshot(checkpoint);
            return Err(RuntimeDriverError::Internal(format!(
                "terminal cancellation persist failed: {err}"
            )));
        }
        Ok(())
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl RuntimeDriver for PersistentRuntimeDriver {
    async fn accept_input(&mut self, input: Input) -> Result<AcceptOutcome, RuntimeDriverError> {
        let resolved = self.resolve_admission(&input)?;
        self.accept_resolved_input(input, resolved).await
    }

    async fn on_runtime_event(
        &mut self,
        event: RuntimeEventEnvelope,
    ) -> Result<(), RuntimeDriverError> {
        self.inner.on_runtime_event(event).await
    }

    async fn recover(&mut self) -> Result<RecoveryReport, RuntimeDriverError> {
        let mut staged = self.inner.clone_with_isolated_dsl_authority();
        let report = crate::meerkat_machine::machine_recover_persistent_driver(
            self.store.as_ref(),
            &self.runtime_id,
            &mut staged,
        )
        .await?;

        let input_states = staged.authorized_stored_input_states_snapshot()?;
        let commit = Self::lifecycle_commit_for_persistence_from_inner(&staged)?;
        self.store
            .commit_machine_lifecycle(&self.runtime_id, commit, &input_states)
            .await
            .map_err(|err| {
                RuntimeDriverError::Internal(format!("recovery persist failed: {err}"))
            })?;
        let _ = crate::meerkat_machine::machine_recover_persistent_driver(
            self.store.as_ref(),
            &self.runtime_id,
            &mut self.inner,
        )
        .await?;
        Ok(report)
    }

    fn runtime_state(&self) -> RuntimeState {
        self.inner.runtime_state()
    }

    fn input_state(&self, input_id: &InputId) -> Option<&InputState> {
        self.inner.input_state(input_id)
    }

    fn input_phase(&self, input_id: &InputId) -> Option<InputLifecycleState> {
        self.inner.input_phase(input_id)
    }

    fn input_last_run_id(&self, input_id: &InputId) -> Option<RunId> {
        self.inner.input_last_run_id(input_id)
    }

    fn input_last_boundary_sequence(&self, input_id: &InputId) -> Option<u64> {
        self.inner.input_last_boundary_sequence(input_id)
    }

    fn stored_input_state(&self, input_id: &InputId) -> Option<StoredInputState> {
        self.inner.stored_input_state(input_id)
    }

    fn active_input_ids(&self) -> Vec<InputId> {
        self.inner.active_input_ids()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::Utc;
    use meerkat_core::lifecycle::InputId;

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
            content: text.into(),
            typed_turn_appends: Vec::new(),
            turn_metadata: None,
        })
    }

    /// Dogma K11 (Persistent destroy / driver-side shadow truth): every
    /// fallible step of `commit_lifecycle_with_rollback` AFTER the caller has
    /// staged a DSL lifecycle transition must restore the caller's checkpoint.
    /// The input-state snapshot read used to escape with a bare `?`, leaving
    /// the staged lifecycle live in driver state while reporting failure.
    #[tokio::test]
    async fn commit_lifecycle_snapshot_failure_restores_checkpoint() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let rid = LogicalRuntimeId::new("commit-lifecycle-rollback-contract");
        let mut driver = PersistentRuntimeDriver::new(rid, store, blob_store);

        // Checkpoint BEFORE any state mutation (the caller's pre-stage view).
        let checkpoint = driver.rollback_snapshot();

        // Mutate driver state past the checkpoint (stands in for a staged
        // Destroy/lifecycle transition awaiting durable commit).
        let input = make_prompt("staged work");
        let input_id = input.id().clone();
        let outcome = driver.accept_input(input).await.unwrap();
        assert!(outcome.is_accepted());
        assert!(driver.input_phase(&input_id).is_some());

        // Inject a failure into the input-state snapshot step.
        driver.force_input_snapshot_failure_for_test = true;
        let target_state = driver.inner_ref().runtime_state();
        let result = driver
            .commit_lifecycle_with_rollback(checkpoint, target_state, "test destroy")
            .await;

        // The failure must propagate typed AND the staged driver state must be
        // rolled back to the checkpoint — no half-destroyed shadow truth.
        assert!(result.is_err(), "forced snapshot failure must propagate");
        assert!(
            driver.input_phase(&input_id).is_none(),
            "staged driver state must be restored to the pre-stage checkpoint"
        );
        assert!(driver.active_input_ids().is_empty());
    }

    /// Same K11 checkpoint-restore contract for `abandon_pending_inputs`: the
    /// input-state snapshot / lifecycle-commit classification steps between
    /// the staged `&mut` abandon and the durable commit used to escape with a
    /// bare `?`, leaving the abandon applied in memory while reporting
    /// failure (and never persisting it).
    #[tokio::test]
    async fn abandon_pending_inputs_snapshot_failure_restores_checkpoint() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let rid = LogicalRuntimeId::new("abandon-rollback-contract");
        let mut driver = PersistentRuntimeDriver::new(rid, store, blob_store);

        // Accept a pending input so the abandon has staged work to mutate.
        let input = make_prompt("pending work");
        let input_id = input.id().clone();
        let outcome = driver.accept_input(input).await.unwrap();
        assert!(outcome.is_accepted());
        assert!(driver.input_phase(&input_id).is_some());

        // Inject a failure into the input-state snapshot step that runs after
        // the staged abandon mutation.
        driver.force_input_snapshot_failure_for_test = true;
        let result = driver
            .abandon_pending_inputs(InputAbandonReason::Reset)
            .await;

        assert!(result.is_err(), "forced snapshot failure must propagate");
        assert!(
            driver.input_phase(&input_id).is_some(),
            "staged abandon must be rolled back: the pending input must still be live"
        );
    }
}
