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
use crate::store::{InputStateBatchCasOutcome, MachineLifecycleCommit, RuntimeStore};
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
    pub(crate) async fn recover_inputs_after_runtime_authority(
        &mut self,
        recovered_unregister_progress: Option<&crate::store::MachineUnregisterProgressSnapshot>,
    ) -> Result<RecoveryReport, RuntimeDriverError> {
        let report = crate::meerkat_machine::machine_recover_persistent_inputs(
            self.store.as_ref(),
            &self.runtime_id,
            &mut self.inner,
            recovered_unregister_progress,
        )
        .await?;

        let input_states = self.inner.authorized_stored_input_states_snapshot()?;
        self.store
            .persist_input_states_atomically(&self.runtime_id, &input_states)
            .await
            .map_err(|err| {
                RuntimeDriverError::Internal(format!("recovered input persistence failed: {err}"))
            })?;
        Ok(report)
    }

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

    pub(crate) fn inner_mut(&mut self) -> &mut EphemeralRuntimeDriver {
        &mut self.inner
    }

    #[cfg(test)]
    pub(crate) async fn compare_and_swap_interaction_terminal_outbox_inputs(
        &self,
        expected: &[StoredInputState],
        input_ids: &[InputId],
    ) -> Result<InputStateBatchCasOutcome, RuntimeDriverError> {
        let mut replacements = Vec::with_capacity(input_ids.len());
        for input_id in input_ids {
            let replacement = self
                .inner
                .authorized_stored_input_state(input_id)?
                .ok_or_else(|| {
                    RuntimeDriverError::Internal(format!(
                        "interaction terminal outbox input {input_id} disappeared before compare-and-swap"
                    ))
                })?;
            replacements.push(replacement);
        }
        self.store
            .compare_and_swap_input_states_atomically(&self.runtime_id, expected, &replacements)
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "interaction terminal outbox batch compare-and-swap failed: {error}"
                ))
            })
    }

    pub(crate) async fn compare_and_swap_interaction_terminal_outbox_replacements(
        &self,
        expected: &[StoredInputState],
        replacements: &[crate::input_state::InputStatePersistenceRecord],
    ) -> Result<InputStateBatchCasOutcome, RuntimeDriverError> {
        self.store
            .compare_and_swap_input_states_atomically(&self.runtime_id, expected, replacements)
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "interaction terminal outbox batch compare-and-swap failed: {error}"
                ))
            })
    }

    pub(crate) async fn committed_session_snapshot_for_terminal_recovery(
        &self,
    ) -> Result<Option<Vec<u8>>, RuntimeDriverError> {
        self.store
            .load_session_snapshot(&self.runtime_id)
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "interaction terminal recovery failed to load committed session snapshot: {error}"
                ))
            })
    }

    pub(crate) async fn durable_input_states_for_terminal_recovery(
        &self,
    ) -> Result<Vec<StoredInputState>, RuntimeDriverError> {
        self.store
            .load_input_states(&self.runtime_id)
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "interaction terminal recovery failed to load durable input states: {error}"
                ))
            })
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

    pub(crate) async fn load_pending_compaction_projections(
        &self,
    ) -> Result<Vec<meerkat_core::CompactionProjectionIntent>, RuntimeDriverError> {
        self.store
            .load_pending_compaction_projections(&self.runtime_id)
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "failed to load compaction projection outbox: {error}"
                ))
            })
    }

    pub(crate) async fn mark_compaction_projection_finalized(
        &self,
        projection: &meerkat_core::CompactionProjectionId,
    ) -> Result<(), RuntimeDriverError> {
        self.store
            .mark_compaction_projection_finalized(&self.runtime_id, projection)
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "failed to finalize compaction projection outbox: {error}"
                ))
            })
    }

    pub(crate) async fn load_compaction_checkpoint_snapshot(
        &self,
    ) -> Result<Option<Vec<u8>>, RuntimeDriverError> {
        self.store
            .load_session_snapshot(&self.runtime_id)
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "failed to load authoritative compaction checkpoint snapshot: {error}"
                ))
            })
    }

    pub(crate) async fn commit_compaction_checkpoint_snapshot(
        &self,
        session_snapshot: Vec<u8>,
    ) -> Result<(), RuntimeDriverError> {
        self.store
            .commit_session_snapshot(
                &self.runtime_id,
                crate::store::SessionDelta { session_snapshot },
            )
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "failed to prepare authoritative compaction checkpoint snapshot: {error}"
                ))
            })
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
        crate::meerkat_machine::classify_runtime_lifecycle_durable_state_with_pre_run_phase(
            inner.runtime_state(),
            inner.pre_run_phase(),
        )
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

    fn lifecycle_commit_for_persistence_with_supervisor_authority(
        &self,
        supervisor_authority: crate::store::SupervisorAuthoritySnapshot,
    ) -> Result<MachineLifecycleCommit, RuntimeDriverError> {
        Ok(
            MachineLifecycleCommit::new_with_binding_and_unregister_progress(
                Self::runtime_state_for_persistence_from_inner(&self.inner)?,
                self.inner.machine_lifecycle_binding_facts(),
                supervisor_authority,
                Self::unregister_progress_for_persistence_from_inner(&self.inner),
            ),
        )
    }

    fn lifecycle_commit_for_persistence_from_inner(
        inner: &EphemeralRuntimeDriver,
    ) -> Result<MachineLifecycleCommit, RuntimeDriverError> {
        Ok(
            MachineLifecycleCommit::new_with_binding_and_unregister_progress(
                Self::runtime_state_for_persistence_from_inner(inner)?,
                inner.machine_lifecycle_binding_facts(),
                inner.supervisor_authority_snapshot(),
                Self::unregister_progress_for_persistence_from_inner(inner),
            ),
        )
    }

    /// Project a committed final `UnregisterSession` for durable storage.
    ///
    /// The live entry deliberately keeps `registration_phase = Draining` as a
    /// same-process rematerialization tombstone until exact entry removal. That
    /// mechanical fence is not durable unregister progress: final generated
    /// authority has cleared the session binding and all drain obligations, so
    /// persisting a progress row would make a later process replay a completed
    /// teardown and reject fresh registration.
    fn lifecycle_commit_for_completed_unregister(
        &self,
    ) -> Result<MachineLifecycleCommit, RuntimeDriverError> {
        let completed = {
            let authority = self.inner.shared_dsl_authority();
            let authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = authority.state();
            state.registration_phase == crate::meerkat_machine::dsl::RegistrationPhase::Draining
                && state.session_id.is_none()
                && state.active_runtime_id.is_none()
                && state.active_fence_token.is_none()
                && state.active_runtime_generation.is_none()
                && state.active_runtime_epoch_id.is_none()
                && !state.unregister_runtime_loop_drain_pending
                && !state.unregister_comms_drain_exit_pending
                && !state.unregister_completion_waiter_drain_pending
        };
        if !completed {
            return Err(RuntimeDriverError::Internal(
                "completed unregister persistence requires the generated final lifecycle image"
                    .to_string(),
            ));
        }
        Ok(
            MachineLifecycleCommit::new_with_binding_and_unregister_progress(
                Self::runtime_state_for_persistence_from_inner(&self.inner)?,
                self.inner.machine_lifecycle_binding_facts(),
                self.inner.supervisor_authority_snapshot(),
                None,
            ),
        )
    }

    fn unregister_progress_for_persistence_from_inner(
        inner: &EphemeralRuntimeDriver,
    ) -> Option<crate::store::MachineUnregisterProgressSnapshot> {
        let authority = inner.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = authority.state();
        (state.registration_phase == crate::meerkat_machine::dsl::RegistrationPhase::Draining).then(
            || {
                crate::store::MachineUnregisterProgressSnapshot::new(
                    state.unregister_runtime_loop_drain_pending,
                    state.unregister_comms_drain_exit_pending,
                    state.unregister_completion_waiter_drain_pending,
                    state.unregister_runtime_loop_forced_abort,
                    state.unregister_comms_drain_forced_abort,
                )
            },
        )
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
            match crate::meerkat_machine::classify_runtime_lifecycle_durable_state_with_pre_run_phase(
                target_state,
                self.inner.pre_run_phase(),
            ) {
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

    pub(crate) async fn publish_service_turn_terminal(
        &mut self,
        checkpoint: super::ephemeral::EphemeralDriverRollbackSnapshot,
        target_state: RuntimeState,
        session_snapshot: Vec<u8>,
        receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
        owner_session_id: meerkat_core::types::SessionId,
    ) -> Result<(), RuntimeDriverError> {
        let commit = match self.lifecycle_commit_for_persistence() {
            Ok(commit) => commit,
            Err(error) => {
                self.inner.restore_rollback_snapshot(checkpoint);
                return Err(RuntimeDriverError::Internal(format!(
                    "service turn terminal receipt lifecycle classification failed: {error}"
                )));
            }
        };
        let target_durable_state =
            match crate::meerkat_machine::classify_runtime_lifecycle_durable_state(target_state) {
                Ok(target_durable_state) => target_durable_state,
                Err(error) => {
                    self.inner.restore_rollback_snapshot(checkpoint);
                    return Err(RuntimeDriverError::Internal(format!(
                        "service turn terminal receipt target classification failed: {error}"
                    )));
                }
            };
        if commit.runtime_state() != target_durable_state {
            self.inner.restore_rollback_snapshot(checkpoint);
            return Err(RuntimeDriverError::Internal(format!(
                "service turn terminal receipt durable target {target_durable_state:?} disagreed with generated lifecycle {:?}",
                commit.runtime_state()
            )));
        }
        if let Err(error) = self
            .store
            .atomic_apply_with_machine_lifecycle(
                &self.runtime_id,
                crate::store::SessionDelta { session_snapshot },
                receipt,
                commit,
                Vec::new(),
                owner_session_id,
            )
            .await
        {
            self.inner.restore_rollback_snapshot(checkpoint);
            return Err(RuntimeDriverError::Internal(format!(
                "service turn terminal receipt persist failed: {error}"
            )));
        }
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

    pub(crate) async fn commit_unregister_finalization(
        &mut self,
        context: &str,
        retired_ops_epoch: &meerkat_core::RuntimeEpochId,
        authority: crate::meerkat_machine::DeleteOpsFinalizationAuthority,
    ) -> Result<(), RuntimeDriverError> {
        let input_states = self.inner.authorized_stored_input_states_snapshot()?;
        let commit = self.lifecycle_commit_for_completed_unregister()?;
        let finalization = crate::store::UnregisterFinalizationCommit::new(
            commit,
            input_states,
            retired_ops_epoch.clone(),
            authority,
        );
        self.store
            .commit_unregister_finalization(&self.runtime_id, finalization)
            .await
            .map_err(|err| match err {
                crate::store::RuntimeStoreError::UnregisterFinalizationOutcomeUnknown(reason) => {
                    RuntimeDriverError::UnregisterFinalizationOutcomeUnknown {
                        reason: format!("{context} lifecycle+ops finalization: {reason}"),
                    }
                }
                err => RuntimeDriverError::Internal(format!(
                    "{context} lifecycle+ops finalization failed: {err}"
                )),
            })
    }

    pub(crate) async fn persist_completed_unregister_machine_lifecycle(
        &mut self,
        context: &str,
        _authority: crate::meerkat_machine::RetainOpsFinalizationAuthority,
    ) -> Result<(), RuntimeDriverError> {
        let input_states = self.inner.authorized_stored_input_states_snapshot()?;
        let commit = self.lifecycle_commit_for_completed_unregister()?;
        self.store
            .commit_machine_lifecycle(&self.runtime_id, commit, &input_states)
            .await
            .map_err(|error| {
                // The generic lifecycle commit contract is atomic, but unlike
                // commit_unregister_finalization it does not distinguish a
                // definitely-uncommitted error from a lost acknowledgement.
                // RetainSnapshot finalization must therefore treat every
                // error as ambiguous: rolling local authority back to
                // Draining could overwrite a terminal image that already
                // committed durably.
                RuntimeDriverError::UnregisterFinalizationOutcomeUnknown {
                    reason: format!(
                        "{context} retained lifecycle finalization acknowledgement unavailable: {error}"
                    ),
                }
            })
    }

    /// Persist a previewed closed supervisor projection alongside the current
    /// machine lifecycle. This lets the supervisor saga commit durable truth
    /// before changing the shared live authority, avoiding a whole-authority
    /// rollback across asynchronous store I/O (peer ingress may concurrently
    /// mutate unrelated generated fields).
    pub(crate) async fn persist_current_machine_lifecycle_with_supervisor_authority(
        &mut self,
        context: &str,
        supervisor_authority: crate::store::SupervisorAuthoritySnapshot,
    ) -> Result<(), RuntimeDriverError> {
        let input_states = self.inner.authorized_stored_input_states_snapshot()?;
        let commit =
            self.lifecycle_commit_for_persistence_with_supervisor_authority(supervisor_authority)?;
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

    pub(crate) fn machine_realize_terminal_failure_applied(
        &mut self,
        run_id: &meerkat_core::lifecycle::RunId,
        input_ids: &[InputId],
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        self.inner
            .machine_realize_terminal_failure_applied(run_id, input_ids)
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

    pub(crate) async fn abandon_queued_input(
        &mut self,
        input_id: &meerkat_core::lifecycle::InputId,
        reason: InputAbandonReason,
    ) -> Result<bool, RuntimeDriverError> {
        let checkpoint = self.inner.rollback_snapshot();
        let abandoned = match self.inner.abandon_queued_input(input_id, reason) {
            Ok(abandoned) => abandoned,
            Err(error) => {
                self.inner.restore_rollback_snapshot(checkpoint);
                return Err(error);
            }
        };
        if !abandoned {
            return Ok(false);
        }
        let (checkpoint, input_states, commit) =
            self.lifecycle_persistence_payload_with_rollback(checkpoint, "tracked input cancel")?;
        if let Err(error) = self
            .store
            .commit_machine_lifecycle(&self.runtime_id, commit, &input_states)
            .await
        {
            self.inner.restore_rollback_snapshot(checkpoint);
            return Err(RuntimeDriverError::Internal(format!(
                "tracked input cancel persist failed: {error}"
            )));
        }
        Ok(true)
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
        owner_session_id: &meerkat_core::types::SessionId,
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
                Some(owner_session_id.clone()),
            )
            .await
            .map_err(|e| {
                RuntimeDriverError::Internal(format!(
                    "runtime completed-boundary commit failed: {e}"
                ))
            })
    }

    /// Persist a failed-run realization whose generated input transitions and
    /// directed terminal outboxes have already been staged in `inner` by the
    /// shared `DriverEntry` owner. Keeping this persistence step after the
    /// shared realization makes the queued/abandoned split and its exact
    /// terminal recipient batch one atomic store commit.
    pub(crate) async fn persist_machine_realized_run_failed(
        &mut self,
        realization: crate::meerkat_machine::driver::MachineRunFailureRealization,
    ) -> Result<(), RuntimeDriverError> {
        let crate::meerkat_machine::driver::MachineRunFailureRealization {
            run_id,
            contributing_input_ids,
            replay_plan,
            terminal_error,
            runtime_apply_failure,
            recoverable,
            applied_commit,
        } = realization;
        let checkpoint = self.inner.rollback_snapshot();
        let failure_cause = runtime_apply_failure.as_ref().map(|failure| failure.kind);
        tracing::debug!(
            run_id = ?run_id,
            contributors = contributing_input_ids.len(),
            replay_kind = replay_plan.notice_kind,
            recoverable,
            error = terminal_error,
            failure_cause = ?failure_cause,
            "persistent driver realized machine-owned failed-run replay"
        );
        let (checkpoint, input_states, commit) = self
            .lifecycle_persistence_payload_with_rollback(checkpoint, "failed-run terminal event")?;
        let persist_result = if let Some(applied_commit) = applied_commit {
            let session = match serde_json::from_slice::<meerkat_core::Session>(
                &applied_commit.session_snapshot,
            ) {
                Ok(session) => session,
                Err(error) => {
                    self.inner.restore_rollback_snapshot(checkpoint);
                    return Err(RuntimeDriverError::Internal(format!(
                        "machine-terminal session snapshot was not a Session: {error}"
                    )));
                }
            };
            if session.id() != &applied_commit.owner_session_id {
                self.inner.restore_rollback_snapshot(checkpoint);
                return Err(RuntimeDriverError::Internal(format!(
                    "machine-terminal session owner changed after validation: generated {}, snapshot {}",
                    applied_commit.owner_session_id,
                    session.id()
                )));
            }
            self.store
                .atomic_apply_with_machine_lifecycle(
                    &self.runtime_id,
                    crate::store::SessionDelta {
                        session_snapshot: applied_commit.session_snapshot,
                    },
                    applied_commit.receipt,
                    commit,
                    input_states,
                    applied_commit.owner_session_id,
                )
                .await
        } else {
            self.store
                .commit_machine_lifecycle(&self.runtime_id, commit, &input_states)
                .await
        };
        if let Err(err) = persist_result {
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
        let report = crate::meerkat_machine::machine_recover_persistent_driver(
            self.store.as_ref(),
            &self.runtime_id,
            &mut self.inner,
        )
        .await?;

        let input_states = self.inner.authorized_stored_input_states_snapshot()?;
        self.store
            .persist_input_states_atomically(&self.runtime_id, &input_states)
            .await
            .map_err(|err| {
                RuntimeDriverError::Internal(format!("recovered input persistence failed: {err}"))
            })?;
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

    fn stored_input_states_snapshot(&self) -> Result<Vec<StoredInputState>, RuntimeDriverError> {
        self.inner.stored_input_states_snapshot()
    }

    fn input_id_for_idempotency_key(&self, idempotency_key: &str) -> Option<InputId> {
        self.inner.input_id_for_idempotency_key(idempotency_key)
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
            injected_context: Vec::new(),
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

    #[tokio::test]
    async fn retiring_active_run_persists_retired_before_dropping_live_witness() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let runtime_id = LogicalRuntimeId::new("retire-active-run-durability");
        let runtime_store: Arc<dyn RuntimeStore> = store.clone();
        let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let mut driver =
            PersistentRuntimeDriver::new(runtime_id.clone(), runtime_store, blob_store);
        let run_id = RunId::new();

        driver
            .contract_begin_run_authority(run_id.clone())
            .expect("contract run admission");
        assert_eq!(driver.runtime_state(), RuntimeState::Running);
        assert_eq!(driver.inner_ref().current_run_id(), Some(run_id));
        assert!(driver.inner_ref().pre_run_phase().is_some());

        let session_id = driver.inner_ref().session_authority_id_for_recovery();
        {
            let authority = driver.inner_ref().shared_dsl_authority();
            let mut authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                crate::meerkat_machine::dsl::MeerkatMachineInput::Retire { session_id },
            )
            .expect("machine-authorized mid-run retire transition");
        }
        driver.sync_control_projection_from_dsl_authority();
        assert_eq!(driver.runtime_state(), RuntimeState::Retired);
        assert!(
            driver.inner_ref().pre_run_phase().is_some(),
            "Retire commits before the live run witness is dropped"
        );

        driver
            .realize_retire_lifecycle()
            .await
            .expect("mid-run retire must durably commit");

        assert_eq!(driver.runtime_state(), RuntimeState::Retired);
        assert_eq!(
            crate::store::load_runtime_state(store.as_ref(), &runtime_id)
                .await
                .expect("reload durable lifecycle"),
            Some(RuntimeState::Retired)
        );
    }

    #[tokio::test]
    async fn interaction_terminal_outbox_delegator_swaps_exact_rows_and_reports_stale() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let store_trait: Arc<dyn RuntimeStore> = store.clone();
        let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let rid = LogicalRuntimeId::new("interaction-outbox-cas-delegator");
        let mut driver = PersistentRuntimeDriver::new(rid.clone(), store_trait, blob_store);

        let mut input_ids = Vec::new();
        for text in ["first", "second"] {
            let input = make_prompt(text);
            input_ids.push(input.id().clone());
            assert!(driver.accept_input(input).await.unwrap().is_accepted());
        }
        // The persistent accept path intentionally previews and durably
        // commits an isolated staged driver before realizing the same
        // admission in the live driver.  Capture the CAS witness from the
        // durable store, as recovery adoption does, instead of assuming the
        // two independently timestamped admission shells are byte-identical.
        let expected = store.load_input_states(&rid).await.unwrap();
        for input_id in &input_ids {
            driver
                .inner_mut()
                .ledger_mut()
                .get_mut(input_id)
                .unwrap()
                .recovery_count = 1;
        }

        assert_eq!(
            driver
                .compare_and_swap_interaction_terminal_outbox_inputs(&expected, &input_ids)
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Swapped
        );
        assert!(
            store
                .load_input_states(&rid)
                .await
                .unwrap()
                .iter()
                .all(|row| row.state.recovery_count == 1)
        );

        for input_id in &input_ids {
            driver
                .inner_mut()
                .ledger_mut()
                .get_mut(input_id)
                .unwrap()
                .recovery_count = 2;
        }
        assert_eq!(
            driver
                .compare_and_swap_interaction_terminal_outbox_inputs(&expected, &input_ids)
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Stale
        );
        assert!(
            store
                .load_input_states(&rid)
                .await
                .unwrap()
                .iter()
                .all(|row| row.state.recovery_count == 1),
            "a stale delegator CAS must not mutate any durable row"
        );
    }

    #[tokio::test]
    async fn recover_atomically_rewrites_cold_running_lifecycle_to_idle() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let runtime_id = LogicalRuntimeId::new("rewrite-cold-running-lifecycle");
        store
            .commit_machine_lifecycle(
                &runtime_id,
                MachineLifecycleCommit::new_with_binding(
                    RuntimeState::Running,
                    crate::store::MachineLifecycleBindingFacts::new(
                        Some("rt:cold-running".to_string()),
                        Some(9),
                        Some(2),
                        Some("epoch-cold-running".to_string()),
                    ),
                    crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                ),
                &[],
            )
            .await
            .expect("seed legacy cold Running lifecycle");

        let runtime_store: Arc<dyn RuntimeStore> = store.clone();
        let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let mut driver =
            PersistentRuntimeDriver::new(runtime_id.clone(), runtime_store, blob_store);

        driver
            .recover()
            .await
            .expect("cold Running recovery should converge durably");

        assert_eq!(driver.runtime_state(), RuntimeState::Idle);
        assert_eq!(
            crate::store::load_runtime_state(store.as_ref(), &runtime_id)
                .await
                .expect("reload durable lifecycle"),
            Some(RuntimeState::Idle),
            "recovery acknowledgement must mean the torn lifecycle row is repaired"
        );
    }

    #[tokio::test]
    async fn exact_batch_cas_fences_stale_two_handle_finalization_and_publication_writes() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let store_trait: Arc<dyn RuntimeStore> = store.clone();
        let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let rid = LogicalRuntimeId::new("interaction-outbox-two-handle-phase-fence");
        let mut owner =
            PersistentRuntimeDriver::new(rid.clone(), store_trait.clone(), blob_store.clone());
        let mut input_ids = Vec::new();
        for text in ["first", "second"] {
            let input = make_prompt(text);
            input_ids.push(input.id().clone());
            assert!(owner.accept_input(input).await.unwrap().is_accepted());
        }

        // First owner acquires the durable batch witness.
        let initial = store.load_input_states(&rid).await.unwrap();
        for input_id in &input_ids {
            owner
                .inner_mut()
                .ledger_mut()
                .get_mut(input_id)
                .unwrap()
                .recovery_count = 10;
        }
        assert_eq!(
            owner
                .compare_and_swap_interaction_terminal_outbox_inputs(&initial, &input_ids)
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Swapped
        );
        let owner_witness = store.load_input_states(&rid).await.unwrap();

        // A second store handle takes over before Candidate -> Finalized.
        let mut takeover =
            PersistentRuntimeDriver::new(rid.clone(), store_trait.clone(), blob_store.clone());
        RuntimeDriver::recover(&mut takeover).await.unwrap();
        let takeover_expected = store.load_input_states(&rid).await.unwrap();
        for input_id in &input_ids {
            takeover
                .inner_mut()
                .ledger_mut()
                .get_mut(input_id)
                .unwrap()
                .recovery_count = 20;
        }
        assert_eq!(
            takeover
                .compare_and_swap_interaction_terminal_outbox_inputs(
                    &takeover_expected,
                    &input_ids,
                )
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Swapped
        );
        for input_id in &input_ids {
            owner
                .inner_mut()
                .ledger_mut()
                .get_mut(input_id)
                .unwrap()
                .recovery_count = 30;
        }
        assert_eq!(
            owner
                .compare_and_swap_interaction_terminal_outbox_inputs(&owner_witness, &input_ids)
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Stale,
            "the superseded owner must not overwrite takeover at finalization"
        );
        assert!(
            store
                .load_input_states(&rid)
                .await
                .unwrap()
                .iter()
                .all(|row| row.state.recovery_count == 20)
        );

        // The takeover owner finalizes, then a third handle takes ownership
        // before Finalized -> Published. The old finalizer's receipt write is
        // fenced by its exact pre-publication witness.
        let takeover_witness = store.load_input_states(&rid).await.unwrap();
        for input_id in &input_ids {
            takeover
                .inner_mut()
                .ledger_mut()
                .get_mut(input_id)
                .unwrap()
                .recovery_count = 40;
        }
        assert_eq!(
            takeover
                .compare_and_swap_interaction_terminal_outbox_inputs(&takeover_witness, &input_ids,)
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Swapped
        );
        let finalized_witness = store.load_input_states(&rid).await.unwrap();
        let mut publisher = PersistentRuntimeDriver::new(rid.clone(), store_trait, blob_store);
        RuntimeDriver::recover(&mut publisher).await.unwrap();
        let publisher_expected = store.load_input_states(&rid).await.unwrap();
        for input_id in &input_ids {
            publisher
                .inner_mut()
                .ledger_mut()
                .get_mut(input_id)
                .unwrap()
                .recovery_count = 50;
        }
        assert_eq!(
            publisher
                .compare_and_swap_interaction_terminal_outbox_inputs(
                    &publisher_expected,
                    &input_ids,
                )
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Swapped
        );
        for input_id in &input_ids {
            takeover
                .inner_mut()
                .ledger_mut()
                .get_mut(input_id)
                .unwrap()
                .recovery_count = 60;
        }
        assert_eq!(
            takeover
                .compare_and_swap_interaction_terminal_outbox_inputs(
                    &finalized_witness,
                    &input_ids,
                )
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Stale,
            "the superseded finalizer must not overwrite takeover at publication"
        );
        assert!(
            store
                .load_input_states(&rid)
                .await
                .unwrap()
                .iter()
                .all(|row| row.state.recovery_count == 50)
        );
    }
}
