//! PersistentRuntimeDriver — wraps EphemeralRuntimeDriver + RuntimeStore.
//!
//! Provides durable-before-ack guarantee: InputState is persisted via
//! RuntimeStore BEFORE returning AcceptOutcome. Delegates state machine
//! logic to the ephemeral driver.

use std::sync::Arc;

use chrono::Utc;
use meerkat_core::lifecycle::{InputId, RunEvent};

use crate::accept::AcceptOutcome;
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_state::{
    InputAbandonReason, InputLifecycleState, InputState, InputStateHistoryEntry,
    InputTerminalOutcome,
};
use crate::runtime_event::RuntimeEventEnvelope;
use crate::runtime_state::RuntimeState;
use crate::store::RuntimeStore;
use crate::traits::{
    DestroyReport, RecoveryReport, RuntimeControlCommand, RuntimeDriver, RuntimeDriverError,
};

use super::ephemeral::EphemeralRuntimeDriver;

/// Persistent runtime driver — durable InputState via RuntimeStore.
pub struct PersistentRuntimeDriver {
    /// Underlying ephemeral driver for state machine logic.
    inner: EphemeralRuntimeDriver,
    /// Durable store for InputState + receipts.
    store: Arc<dyn RuntimeStore>,
    /// Runtime ID for store operations.
    runtime_id: LogicalRuntimeId,
}

impl PersistentRuntimeDriver {
    /// Create a new persistent runtime driver.
    pub fn new(runtime_id: LogicalRuntimeId, store: Arc<dyn RuntimeStore>) -> Self {
        Self {
            inner: EphemeralRuntimeDriver::new(runtime_id.clone()),
            store,
            runtime_id,
        }
    }

    /// Get immutable reference to the inner ephemeral driver.
    pub fn inner_ref(&self) -> &EphemeralRuntimeDriver {
        &self.inner
    }

    /// Set the list of comms intents that should be silently accepted (delegates to inner).
    pub fn set_silent_comms_intents(&mut self, intents: Vec<String>) {
        self.inner.set_silent_comms_intents(intents);
    }

    /// Check if the runtime is idle (delegates to inner).
    pub fn is_idle(&self) -> bool {
        self.inner.is_idle()
    }

    /// Check if the runtime is idle or attached (delegates to inner).
    pub fn is_idle_or_attached(&self) -> bool {
        self.inner.is_idle_or_attached()
    }

    /// Attach an executor (Idle → Attached). Delegates to inner.
    pub fn attach(&mut self) -> Result<(), crate::runtime_state::RuntimeStateTransitionError> {
        self.inner.attach()
    }

    /// Detach an executor (Attached → Idle). Delegates to inner.
    pub fn detach(
        &mut self,
    ) -> Result<Option<RuntimeState>, crate::runtime_state::RuntimeStateTransitionError> {
        self.inner.detach()
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

    /// Start a new run (delegates to inner).
    pub fn start_run(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
    ) -> Result<(), crate::runtime_state::RuntimeStateTransitionError> {
        self.inner.start_run(run_id)
    }

    /// Complete a run (delegates to inner).
    pub fn complete_run(
        &mut self,
    ) -> Result<meerkat_core::lifecycle::RunId, crate::runtime_state::RuntimeStateTransitionError>
    {
        self.inner.complete_run()
    }

    /// Get pending events (delegates to inner).
    pub fn drain_events(&mut self) -> Vec<RuntimeEventEnvelope> {
        self.inner.drain_events()
    }

    /// Check and clear wake flag (delegates to inner).
    pub fn take_wake_requested(&mut self) -> bool {
        self.inner.take_wake_requested()
    }

    /// Check and clear immediate processing flag (delegates to inner).
    pub fn take_process_requested(&mut self) -> bool {
        self.inner.take_process_requested()
    }

    /// Dequeue next input (delegates to inner).
    pub fn dequeue_next(&mut self) -> Option<(InputId, Input)> {
        self.inner.dequeue_next()
    }

    pub fn has_queued_input_outside(&self, excluded: &[InputId]) -> bool {
        self.inner.has_queued_input_outside(excluded)
    }

    /// Requeue an input at the front of the queue (delegates to inner).
    pub fn enqueue_front_input(&mut self, input_id: InputId, input: Input) {
        self.inner.enqueue_front_input(input_id, input);
    }

    /// Stage input (delegates to inner).
    pub fn stage_input(
        &mut self,
        input_id: &InputId,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<(), crate::input_lifecycle_authority::InputLifecycleError> {
        self.inner.stage_input(input_id, run_id)
    }

    /// Apply input (delegates to inner).
    pub fn apply_input(
        &mut self,
        input_id: &InputId,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<(), crate::input_lifecycle_authority::InputLifecycleError> {
        self.inner.apply_input(input_id, run_id)
    }

    /// Roll back staged inputs (delegates to inner).
    pub fn rollback_staged(
        &mut self,
        input_ids: &[InputId],
    ) -> Result<(), crate::input_lifecycle_authority::InputLifecycleError> {
        self.inner.rollback_staged(input_ids)
    }

    /// Consume applied inputs without completing a runtime run.
    pub fn consume_inputs(
        &mut self,
        input_ids: &[InputId],
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<(), crate::input_lifecycle_authority::InputLifecycleError> {
        self.inner.consume_inputs(input_ids, run_id)
    }

    /// Remove a previously accepted input from the ledger/queue.
    pub fn forget_input(&mut self, input_id: &InputId) {
        self.inner.forget_input(input_id);
    }

    async fn persist_state(&self, state: &InputState) -> Result<(), RuntimeDriverError> {
        self.store
            .persist_input_state(&self.runtime_id, state)
            .await
            .map_err(|e| RuntimeDriverError::Internal(e.to_string()))
    }

    pub async fn abandon_pending_inputs(
        &mut self,
        reason: InputAbandonReason,
    ) -> Result<usize, RuntimeDriverError> {
        let checkpoint = self.inner.clone();
        let abandoned = self.inner.abandon_pending_inputs(reason);
        let input_states = self.inner.input_states_snapshot();
        if let Err(err) = self
            .store
            .atomic_lifecycle_commit(
                &self.runtime_id,
                self.runtime_state_for_persistence(),
                &input_states,
            )
            .await
        {
            self.inner = checkpoint;
            return Err(RuntimeDriverError::Internal(format!(
                "pending input abandon persist failed: {err}"
            )));
        }
        Ok(abandoned)
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl RuntimeDriver for PersistentRuntimeDriver {
    async fn accept_input(&mut self, input: Input) -> Result<AcceptOutcome, RuntimeDriverError> {
        let input_for_recovery = input.clone();

        // Delegate to ephemeral for state machine logic
        let mut outcome = self.inner.accept_input(input).await?;

        // Durable-before-ack: persist InputState before returning
        if let AcceptOutcome::Accepted {
            ref input_id,
            ref mut state,
            ..
        } = outcome
            && let Some(inner_state) = self.inner.input_state(input_id).cloned()
        {
            let mut persisted = inner_state;
            persisted.persisted_input = Some(input_for_recovery);
            self.inner.ledger_mut().accept(persisted.clone());
            if let Err(err) = self.persist_state(&persisted).await {
                self.forget_input(input_id);
                return Err(err);
            }
            *state = persisted;
        }

        Ok(outcome)
    }

    async fn on_runtime_event(
        &mut self,
        event: RuntimeEventEnvelope,
    ) -> Result<(), RuntimeDriverError> {
        self.inner.on_runtime_event(event).await
    }

    async fn on_run_event(&mut self, event: RunEvent) -> Result<(), RuntimeDriverError> {
        match event {
            // BoundaryApplied persists the receipt and the applied state atomically.
            RunEvent::BoundaryApplied {
                ref receipt,
                ref session_snapshot,
                ..
            } => {
                let checkpoint = self.inner.clone();
                self.inner.on_run_event(event.clone()).await?;
                if self
                    .store
                    .load_boundary_receipt(&self.runtime_id, &receipt.run_id, receipt.sequence)
                    .await
                    .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?
                    .is_some()
                {
                    return Ok(());
                }
                let input_updates: Vec<InputState> = receipt
                    .contributing_input_ids
                    .iter()
                    .filter_map(|id| self.inner.input_state(id).cloned())
                    .collect();

                self.store
                    .atomic_apply(
                        &self.runtime_id,
                        session_snapshot.clone().map(|session_snapshot| {
                            crate::store::SessionDelta { session_snapshot }
                        }),
                        receipt.clone(),
                        input_updates,
                        None, // session_store_key — caller provides if dual-store needed
                    )
                    .await
                    .map_err(|e| {
                        self.inner = checkpoint;
                        RuntimeDriverError::Internal(format!("runtime boundary commit failed: {e}"))
                    })?;
            }
            RunEvent::RunCompleted { .. }
            | RunEvent::RunFailed { .. }
            | RunEvent::RunCancelled { .. } => {
                let checkpoint = self.inner.clone();
                self.inner.on_run_event(event).await?;
                let input_states = self.inner.input_states_snapshot();
                if let Err(err) = self
                    .store
                    .atomic_lifecycle_commit(
                        &self.runtime_id,
                        self.runtime_state_for_persistence(),
                        &input_states,
                    )
                    .await
                {
                    self.inner = checkpoint;
                    return Err(RuntimeDriverError::Internal(format!(
                        "terminal event persist failed: {err}"
                    )));
                }
            }
            _ => {
                self.inner.on_run_event(event).await?;
            }
        }

        Ok(())
    }

    async fn on_runtime_control(
        &mut self,
        command: RuntimeControlCommand,
    ) -> Result<(), RuntimeDriverError> {
        let checkpoint = self.inner.clone();
        self.inner.on_runtime_control(command).await?;
        let input_states = self.inner.input_states_snapshot();
        if let Err(err) = self
            .store
            .atomic_lifecycle_commit(
                &self.runtime_id,
                self.runtime_state_for_persistence(),
                &input_states,
            )
            .await
        {
            self.inner = checkpoint;
            return Err(RuntimeDriverError::Internal(format!(
                "control op persist failed: {err}"
            )));
        }
        Ok(())
    }

    async fn recover(&mut self) -> Result<RecoveryReport, RuntimeDriverError> {
        // §24 full recovery: load durable InputState from store
        let stored_states = self
            .store
            .load_input_states(&self.runtime_id)
            .await
            .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;

        let mut recovered_payloads = Vec::new();

        // Inject stored states into the ephemeral driver's ledger.
        // Uses recover() which also rebuilds the idempotency index for
        // dedup correctness and filters out Ephemeral inputs.
        for mut state in stored_states {
            if matches!(
                state.current_state(),
                InputLifecycleState::Applied | InputLifecycleState::AppliedPendingConsumption
            ) {
                let has_receipt =
                    match (state.last_run_id().cloned(), state.last_boundary_sequence()) {
                        (Some(run_id), Some(sequence)) => self
                            .store
                            .load_boundary_receipt(&self.runtime_id, &run_id, sequence)
                            .await
                            .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?
                            .is_some(),
                        _ => false,
                    };
                let now = Utc::now();
                let from = state.current_state();
                if has_receipt {
                    let auth = crate::input_lifecycle_authority::InputLifecycleAuthority::restore(
                        InputLifecycleState::Consumed,
                        Some(InputTerminalOutcome::Consumed),
                        state.last_run_id().cloned(),
                        state.last_boundary_sequence(),
                        {
                            let mut h = state.history().to_vec();
                            h.push(InputStateHistoryEntry {
                                timestamp: now,
                                from,
                                to: InputLifecycleState::Consumed,
                                reason: Some("recovery: boundary receipt already committed".into()),
                            });
                            h
                        },
                        now,
                    );
                    *state.authority_mut() = auth;
                } else {
                    let auth = crate::input_lifecycle_authority::InputLifecycleAuthority::restore(
                        InputLifecycleState::Queued,
                        None,
                        state.last_run_id().cloned(),
                        state.last_boundary_sequence(),
                        {
                            let mut h = state.history().to_vec();
                            h.push(InputStateHistoryEntry {
                                timestamp: now,
                                from,
                                to: InputLifecycleState::Queued,
                                reason: Some("recovery: missing boundary receipt".into()),
                            });
                            h
                        },
                        now,
                    );
                    *state.authority_mut() = auth;
                }
            }

            if let Some(input) = state.persisted_input.clone() {
                recovered_payloads.push((state.input_id.clone(), input));
            }
            let ledger = &mut self.inner;
            if ledger.input_state(&state.input_id).is_none() {
                ledger.ledger_mut().recover(state);
            }
        }

        // Then run ephemeral recovery logic (requeue Accepted/Staged)
        let report = self.inner.recover().await?;

        for (input_id, input) in recovered_payloads {
            let should_requeue = self.inner.input_state(&input_id).is_some_and(|state| {
                state.current_state() == crate::input_state::InputLifecycleState::Queued
            });
            if should_requeue && !self.inner.has_queued_input(&input_id) {
                self.inner.enqueue_recovered_input(input_id, input);
            }
        }

        if let Some(runtime_state) = self
            .store
            .load_runtime_state(&self.runtime_id)
            .await
            .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?
        {
            match runtime_state {
                RuntimeState::Retired if self.inner.runtime_state() != RuntimeState::Retired => {
                    EphemeralRuntimeDriver::retire(&mut self.inner)?;
                }
                RuntimeState::Stopped
                    if self.inner.runtime_state() != RuntimeState::Stopped
                        && self.inner.runtime_state() != RuntimeState::Destroyed =>
                {
                    // Never revive Destroyed as Stopped
                    self.inner
                        .on_runtime_control(RuntimeControlCommand::Stop)
                        .await?;
                }
                RuntimeState::Destroyed
                    if self.inner.runtime_state() != RuntimeState::Destroyed =>
                {
                    self.inner.destroy()?;
                }
                _ => {}
            }

            // Terminal states must not have active inputs. If persisted state
            // is terminal but active inputs exist, treat as store corruption:
            // terminalize those inputs instead of resurrecting work.
            if runtime_state.is_terminal() {
                let active = self.inner.active_input_ids();
                if !active.is_empty() {
                    tracing::warn!(
                        runtime_id = %self.runtime_id,
                        active_count = active.len(),
                        persisted_state = %runtime_state,
                        "terminal runtime has active inputs — terminalizing as corrupted"
                    );
                    let abandoned = self
                        .inner
                        .abandon_all_non_terminal(InputAbandonReason::Destroyed);
                    self.inner.queue_mut().drain();
                    tracing::warn!(
                        runtime_id = %self.runtime_id,
                        abandoned,
                        "force-abandoned active inputs from terminal runtime"
                    );
                }
            }
        }

        // Persist recovered state atomically
        let input_states = self.inner.input_states_snapshot();
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

    async fn retire(&mut self) -> Result<crate::traits::RetireReport, RuntimeDriverError> {
        let checkpoint = self.inner.clone();
        let report = EphemeralRuntimeDriver::retire(&mut self.inner)?;
        let input_states = self.inner.input_states_snapshot();
        if let Err(err) = self
            .store
            .atomic_lifecycle_commit(
                &self.runtime_id,
                self.runtime_state_for_persistence(),
                &input_states,
            )
            .await
        {
            self.inner = checkpoint;
            return Err(RuntimeDriverError::Internal(format!(
                "retire persist failed: {err}"
            )));
        }
        Ok(report)
    }

    async fn reset(&mut self) -> Result<crate::traits::ResetReport, RuntimeDriverError> {
        let checkpoint = self.inner.clone();
        let report = EphemeralRuntimeDriver::reset(&mut self.inner)?;
        let input_states = self.inner.input_states_snapshot();
        if let Err(err) = self
            .store
            .atomic_lifecycle_commit(
                &self.runtime_id,
                self.runtime_state_for_persistence(),
                &input_states,
            )
            .await
        {
            self.inner = checkpoint;
            return Err(RuntimeDriverError::Internal(format!(
                "reset persist failed: {err}"
            )));
        }
        Ok(report)
    }

    async fn destroy(&mut self) -> Result<DestroyReport, RuntimeDriverError> {
        let abandoned = self.inner.destroy()?;
        let input_states = self.inner.input_states_snapshot();
        if let Err(err) = self
            .store
            .atomic_lifecycle_commit(
                &self.runtime_id,
                self.runtime_state_for_persistence(),
                &input_states,
            )
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

    fn runtime_state(&self) -> RuntimeState {
        self.inner.runtime_state()
    }

    fn input_state(&self, input_id: &InputId) -> Option<&InputState> {
        self.inner.input_state(input_id)
    }

    fn active_input_ids(&self) -> Vec<InputId> {
        self.inner.active_input_ids()
    }
}
