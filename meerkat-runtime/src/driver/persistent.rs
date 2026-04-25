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
use crate::input_state::{InputAbandonReason, InputLifecycleState, InputState, StoredInputState};
use crate::runtime_event::RuntimeEventEnvelope;
use crate::runtime_state::RuntimeState;
use crate::store::RuntimeStore;
use crate::traits::{DestroyReport, RecoveryReport, RuntimeDriver, RuntimeDriverError};

use super::ephemeral::{EphemeralRuntimeDriver, SharedIngressDslAuthority};

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
        }
    }

    /// Get immutable reference to the inner ephemeral driver.
    pub fn inner_ref(&self) -> &EphemeralRuntimeDriver {
        &self.inner
    }

    /// Get the logical runtime ID for this driver.
    pub fn runtime_id(&self) -> &LogicalRuntimeId {
        &self.runtime_id
    }

    /// Set the list of comms intents that should be silently accepted (delegates to inner).
    pub fn set_silent_comms_intents(&mut self, intents: Vec<String>) {
        self.inner.set_silent_comms_intents(intents);
    }

    pub fn silent_comms_intents(&self) -> Vec<String> {
        self.inner.silent_comms_intents()
    }

    /// Check if the runtime is idle (delegates to inner).
    pub fn is_idle(&self) -> bool {
        self.inner.is_idle()
    }

    /// Check if the runtime is idle or attached (delegates to inner).
    pub fn is_idle_or_attached(&self) -> bool {
        self.inner.is_idle_or_attached()
    }

    /// Map runtime state for persistence.
    ///
    /// Attached must never be persisted — on recovery, the executor is
    /// re-attached by the surface. Map Attached to Idle for store operations.
    fn runtime_state_for_persistence(&self) -> RuntimeState {
        match self.inner.runtime_state() {
            RuntimeState::Attached => RuntimeState::Idle,
            other => other,
        }
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
    #[doc(hidden)]
    pub fn contract_set_control_projection(
        &mut self,
        next_phase: RuntimeState,
        current_run_id: Option<RunId>,
        pre_run_phase: Option<RuntimeState>,
    ) {
        self.set_control_projection(next_phase, current_run_id, pre_run_phase);
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

    /// Dequeue next input (delegates to inner).
    pub fn dequeue_next(&mut self) -> Option<(InputId, Input)> {
        self.inner.dequeue_next()
    }

    /// Dequeue a specific input by ID (delegates to inner).
    pub fn dequeue_by_id(&mut self, input_id: &InputId) -> Option<(InputId, Input)> {
        self.inner.dequeue_by_id(input_id)
    }

    pub fn has_queued_input_outside(&self, excluded: &[InputId]) -> bool {
        self.inner.has_queued_input_outside(excluded)
    }

    pub(crate) fn defer_queued_inputs_behind_backlog(&mut self, input_ids: &[InputId]) {
        self.inner.defer_queued_inputs_behind_backlog(input_ids);
    }

    pub(crate) fn absorb_post_admission_effects(
        &mut self,
        effects: &[crate::meerkat_machine::dsl::MeerkatMachineEffect],
    ) {
        self.inner.absorb_post_admission_effects(effects);
    }

    pub(crate) fn resolve_admission_for_runtime_idle(
        &self,
        input: &Input,
        runtime_idle: bool,
    ) -> crate::accept::ResolvedAdmission {
        self.inner
            .resolve_admission_for_runtime_idle(input, runtime_idle)
    }

    pub(crate) fn resolve_admission(&self, input: &Input) -> crate::accept::ResolvedAdmission {
        self.inner.resolve_admission(input)
    }

    pub(crate) async fn accept_resolved_input(
        &mut self,
        input: Input,
        resolved: crate::accept::ResolvedAdmission,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        let checkpoint = self.inner.rollback_snapshot();
        let input_for_recovery = input.clone();

        let mut outcome = self.inner.accept_resolved_input(input, resolved).await?;

        if let AcceptOutcome::Accepted {
            ref input_id,
            ref mut state,
            ..
        } = outcome
            && let Some(mut bundle) = self.inner.stored_input_state(input_id)
        {
            let mut input_for_recovery = input_for_recovery.clone();
            if let Err(err) =
                externalize_input_images(self.blob_store.as_ref(), &mut input_for_recovery).await
            {
                self.inner.restore_rollback_snapshot(checkpoint);
                return Err(RuntimeDriverError::Internal(format!(
                    "failed to externalize runtime input images: {err}"
                )));
            }
            bundle.state.persisted_input = Some(input_for_recovery);
            self.inner.ledger_mut().accept(bundle.state.clone());
            if let Err(err) = self.persist_state(&bundle).await {
                self.inner.restore_rollback_snapshot(checkpoint);
                return Err(err);
            }
            *state = bundle.state;
        }

        Ok(outcome)
    }

    /// Stage input (delegates to inner).
    pub fn stage_input(
        &mut self,
        input_id: &InputId,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        self.inner.stage_input(input_id, run_id)
    }

    /// Stage a batch of inputs atomically (delegates to inner).
    pub fn stage_batch(
        &mut self,
        input_ids: &[InputId],
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        self.inner.stage_batch(input_ids, run_id)
    }

    pub(crate) fn machine_realize_stage_batch(
        &mut self,
        input_ids: &[InputId],
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        self.inner.machine_realize_stage_batch(input_ids, run_id)
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

    /// Consume applied inputs without completing a runtime run.
    pub fn consume_inputs(
        &mut self,
        input_ids: &[InputId],
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        self.inner.consume_inputs(input_ids, run_id)
    }

    async fn persist_state(&self, state: &StoredInputState) -> Result<(), RuntimeDriverError> {
        self.store
            .persist_input_state(&self.runtime_id, state)
            .await
            .map_err(|e| RuntimeDriverError::Internal(e.to_string()))
    }

    pub async fn abandon_pending_inputs(
        &mut self,
        reason: InputAbandonReason,
    ) -> Result<usize, RuntimeDriverError> {
        let checkpoint = self.inner.rollback_snapshot();
        let abandoned = self.inner.abandon_pending_inputs(reason);
        let input_states = self.inner.stored_input_states_snapshot();
        if let Err(err) = self
            .store
            .atomic_lifecycle_commit(
                &self.runtime_id,
                self.runtime_state_for_persistence(),
                &input_states,
            )
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
    pub async fn recycle_preserving_work(
        &mut self,
        target_phase: RuntimeState,
    ) -> Result<usize, RuntimeDriverError> {
        let checkpoint = self.inner.rollback_snapshot();
        let input_states = self.inner.stored_input_states_snapshot();
        if let Err(err) = self
            .store
            .atomic_lifecycle_commit(
                &self.runtime_id,
                self.runtime_state_for_persistence(),
                &input_states,
            )
            .await
        {
            self.inner.restore_rollback_snapshot(checkpoint);
            return Err(RuntimeDriverError::Internal(format!(
                "recycle persist failed: {err}"
            )));
        }

        let _ = self.inner.recycle_preserving_work()?;
        self.inner.set_control_projection(target_phase, None, None);
        Ok(self.inner.active_input_ids().len())
    }

    pub async fn finalize_retire(
        &mut self,
    ) -> Result<crate::traits::RetireReport, RuntimeDriverError> {
        let report = self.inner.finalize_retire();
        let input_states = self.inner.stored_input_states_snapshot();
        self.store
            .atomic_lifecycle_commit(
                &self.runtime_id,
                self.runtime_state_for_persistence(),
                &input_states,
            )
            .await
            .map_err(|err| RuntimeDriverError::Internal(format!("retire persist failed: {err}")))?;
        Ok(report)
    }

    /// Low-level retire realization shim for external contract tests.
    ///
    /// Callers are responsible for setting the control projection first.
    #[doc(hidden)]
    pub async fn contract_finalize_retire(
        &mut self,
    ) -> Result<crate::traits::RetireReport, RuntimeDriverError> {
        self.finalize_retire().await
    }

    pub async fn finalize_reset(
        &mut self,
    ) -> Result<crate::traits::ResetReport, RuntimeDriverError> {
        let report = self.inner.reset_cleanup();
        let input_states = self.inner.stored_input_states_snapshot();
        self.store
            .atomic_lifecycle_commit(
                &self.runtime_id,
                self.runtime_state_for_persistence(),
                &input_states,
            )
            .await
            .map_err(|err| RuntimeDriverError::Internal(format!("reset persist failed: {err}")))?;
        Ok(report)
    }

    /// Low-level reset realization shim for external contract tests.
    ///
    /// Callers are responsible for setting the post-reset control projection.
    #[doc(hidden)]
    pub async fn contract_finalize_reset(
        &mut self,
    ) -> Result<crate::traits::ResetReport, RuntimeDriverError> {
        self.finalize_reset().await
    }

    pub async fn destroy(&mut self) -> Result<DestroyReport, RuntimeDriverError> {
        let abandoned = self.inner.destroy_cleanup();
        let input_states = self.inner.stored_input_states_snapshot();
        // Persist the intent-carrying terminal phase (`Destroyed`),
        // not the pre-destroy shell phase from
        // `runtime_state_for_persistence()`. `e5c5ecaf3` removed the
        // shell `set_control_projection(Destroyed, ...)` write (the DSL
        // Destroy input is the canonical phase-update path now), so
        // `runtime_state_for_persistence()` still reads the pre-destroy
        // shell phase at this point. Without the explicit `Destroyed`
        // here, the store records the wrong phase and cold restarts
        // (`cold_reregister_preserves_destroyed_runtime_state`) resurrect
        // the session as non-Destroyed.
        if let Err(err) = self
            .store
            .atomic_lifecycle_commit(&self.runtime_id, RuntimeState::Destroyed, &input_states)
            .await
        {
            return Err(RuntimeDriverError::Internal(format!(
                "destroy persist failed: {err}"
            )));
        }
        Ok(DestroyReport {
            inputs_abandoned: abandoned,
        })
    }

    /// Low-level destroy realization shim for external contract tests.
    ///
    /// Callers are responsible for setting the destroyed control projection.
    #[doc(hidden)]
    pub async fn contract_destroy(&mut self) -> Result<DestroyReport, RuntimeDriverError> {
        self.destroy().await
    }

    pub async fn finalize_stop_runtime(&mut self) -> Result<(), RuntimeDriverError> {
        self.inner.stop_runtime_cleanup();
        let input_states = self.inner.stored_input_states_snapshot();
        self.store
            .atomic_lifecycle_commit(
                &self.runtime_id,
                self.runtime_state_for_persistence(),
                &input_states,
            )
            .await
            .map_err(|err| RuntimeDriverError::Internal(format!("stop persist failed: {err}")))?;
        Ok(())
    }

    /// Low-level stop realization shim for external contract tests.
    ///
    /// Callers are responsible for setting the stopped control projection.
    #[doc(hidden)]
    pub async fn contract_finalize_stop_runtime(&mut self) -> Result<(), RuntimeDriverError> {
        self.finalize_stop_runtime().await
    }

    pub async fn boundary_applied(
        &mut self,
        run_id: RunId,
        receipt: RunBoundaryReceipt,
        session_snapshot: Option<Vec<u8>>,
    ) -> Result<(), RuntimeDriverError> {
        self.machine_realize_boundary_applied(&run_id, &receipt, session_snapshot.as_ref())
            .await
    }

    pub(crate) async fn machine_realize_boundary_applied(
        &mut self,
        run_id: &RunId,
        receipt: &RunBoundaryReceipt,
        session_snapshot: Option<&Vec<u8>>,
    ) -> Result<(), RuntimeDriverError> {
        let checkpoint = self.inner.rollback_snapshot();
        self.inner
            .machine_realize_boundary_applied(run_id, receipt)?;
        if self
            .store
            .load_boundary_receipt(&self.runtime_id, &receipt.run_id, receipt.sequence)
            .await
            .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?
            .is_some()
        {
            return Ok(());
        }
        let input_updates: Vec<StoredInputState> = receipt
            .contributing_input_ids
            .iter()
            .filter_map(|id| self.inner.stored_input_state(id))
            .collect();

        self.store
            .atomic_apply(
                &self.runtime_id,
                session_snapshot.map(|session_snapshot| crate::store::SessionDelta {
                    session_snapshot: session_snapshot.clone(),
                }),
                receipt.clone(),
                input_updates,
                None,
            )
            .await
            .map_err(|e| {
                self.inner.restore_rollback_snapshot(checkpoint);
                RuntimeDriverError::Internal(format!("runtime boundary commit failed: {e}"))
            })?;
        Ok(())
    }

    pub async fn run_completed(
        &mut self,
        run_id: RunId,
        consumed_input_ids: Vec<InputId>,
    ) -> Result<(), RuntimeDriverError> {
        self.machine_realize_run_completed(&run_id, &consumed_input_ids)
            .await
    }

    pub(crate) async fn machine_realize_run_completed(
        &mut self,
        run_id: &RunId,
        consumed_input_ids: &[InputId],
    ) -> Result<(), RuntimeDriverError> {
        let checkpoint = self.inner.rollback_snapshot();
        self.inner
            .machine_realize_run_completed(run_id, consumed_input_ids)?;
        let input_states = self.inner.stored_input_states_snapshot();
        if let Err(err) = self
            .store
            .atomic_lifecycle_commit(
                &self.runtime_id,
                self.runtime_state_for_persistence(),
                &input_states,
            )
            .await
        {
            self.inner.restore_rollback_snapshot(checkpoint);
            return Err(RuntimeDriverError::Internal(format!(
                "terminal event persist failed: {err}"
            )));
        }
        Ok(())
    }

    pub async fn run_failed(
        &mut self,
        run_id: RunId,
        contributing_input_ids: Vec<InputId>,
        replay_plan: super::ephemeral::ReplayQueuedContributorsPlan,
        error: String,
        recoverable: bool,
    ) -> Result<(), RuntimeDriverError> {
        self.machine_realize_run_failed(
            &run_id,
            &contributing_input_ids,
            &replay_plan,
            &error,
            recoverable,
        )
        .await
    }

    pub(crate) async fn machine_realize_run_failed(
        &mut self,
        run_id: &RunId,
        contributing_input_ids: &[InputId],
        replay_plan: &super::ephemeral::ReplayQueuedContributorsPlan,
        error: &str,
        recoverable: bool,
    ) -> Result<(), RuntimeDriverError> {
        let checkpoint = self.inner.rollback_snapshot();
        self.inner
            .machine_realize_run_failed(run_id, contributing_input_ids, replay_plan)?;
        tracing::debug!(
            run_id = ?run_id,
            recoverable,
            error,
            "persistent driver realized machine-owned failed-run replay"
        );
        let input_states = self.inner.stored_input_states_snapshot();
        if let Err(err) = self
            .store
            .atomic_lifecycle_commit(
                &self.runtime_id,
                self.runtime_state_for_persistence(),
                &input_states,
            )
            .await
        {
            self.inner.restore_rollback_snapshot(checkpoint);
            return Err(RuntimeDriverError::Internal(format!(
                "terminal event persist failed: {err}"
            )));
        }
        Ok(())
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl RuntimeDriver for PersistentRuntimeDriver {
    async fn accept_input(&mut self, input: Input) -> Result<AcceptOutcome, RuntimeDriverError> {
        let resolved = self.resolve_admission(&input);
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

        // Persist recovered state atomically
        let input_states = self.inner.stored_input_states_snapshot();
        self.store
            .atomic_lifecycle_commit(
                &self.runtime_id,
                self.runtime_state_for_persistence(),
                &input_states,
            )
            .await
            .map_err(|e| RuntimeDriverError::Internal(format!("recovery persist failed: {e}")))?;
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
