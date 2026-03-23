//! PersistentRuntimeDriver — wraps EphemeralRuntimeDriver + RuntimeStore.
//!
//! Provides durable-before-ack guarantee: InputState is persisted via
//! RuntimeStore BEFORE returning AcceptOutcome. Delegates state machine
//! logic to the ephemeral driver.

use std::sync::Arc;

use chrono::Utc;
use meerkat_core::lifecycle::{InputId, RunEvent};

use crate::accept::AcceptOutcome;
use crate::driver::ephemeral::handling_mode_from_policy;
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_state::{
    InputAbandonReason, InputLifecycleState, InputState, InputStateHistoryEntry,
    InputTerminalOutcome,
};
use crate::runtime_event::RuntimeEventEnvelope;
use crate::runtime_ingress_authority::ContentShape;
use crate::runtime_state::RuntimeState;
use crate::store::RuntimeStore;
use crate::traits::{
    DestroyReport, RecoveryReport, RuntimeControlCommand, RuntimeDriver, RuntimeDriverError,
};
use meerkat_core::types::HandlingMode;

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

    /// Dequeue a specific input by ID (delegates to inner).
    pub fn dequeue_by_id(&mut self, input_id: &InputId) -> Option<(InputId, Input)> {
        self.inner.dequeue_by_id(input_id)
    }

    /// Look up the persisted input for a given ID (delegates to inner).
    pub fn persisted_input(&self, input_id: &InputId) -> Option<&Input> {
        self.inner.persisted_input(input_id)
    }

    pub fn has_queued_input_outside(&self, excluded: &[InputId]) -> bool {
        self.inner.has_queued_input_outside(excluded)
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
    pub(crate) fn forget_input(&mut self, input_id: &InputId) {
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

    /// Recycle the in-memory driver shell while preserving canonical pending
    /// work from durable runtime truth.
    ///
    /// Unlike `reset()`, this must not abandon queued/staged work.
    pub async fn recycle_preserving_work(&mut self) -> Result<usize, RuntimeDriverError> {
        let silent_intents = self.inner.silent_comms_intents();
        self.inner = EphemeralRuntimeDriver::new(self.runtime_id.clone());
        self.inner.set_silent_comms_intents(silent_intents);
        let _ = RuntimeDriver::recover(self).await?;
        Ok(self.inner.active_input_ids().len())
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
            if matches!(
                state.current_state(),
                InputLifecycleState::Accepted | InputLifecycleState::Staged
            ) {
                // Accepted/Staged are pre-run in-flight states. On recovery they
                // must re-enter the queue explicitly so ingress/ledger/queue
                // truth stays aligned before Recover effects are evaluated.
                let now = Utc::now();
                let from = state.current_state();
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
                            reason: Some("recovery: pre-run state normalized to queued".into()),
                        });
                        h
                    },
                    now,
                );
                *state.authority_mut() = auth;
            }

            // Admit to ingress authority so Recover can see this input.
            if self.inner.input_state(&state.input_id).is_none() {
                let handling_mode = state
                    .policy
                    .as_ref()
                    .map(|p| handling_mode_from_policy(&p.decision))
                    .unwrap_or(HandlingMode::Queue);
                let content_shape = state
                    .persisted_input
                    .as_ref()
                    .map(|i| ContentShape(i.kind_id().to_string()))
                    .unwrap_or_else(|| ContentShape("unknown".into()));
                let policy = match state.policy.as_ref() {
                    Some(p) => p.decision.clone(),
                    None => match state.persisted_input.as_ref() {
                        Some(input) => {
                            crate::policy_table::DefaultPolicyTable::resolve(input, true)
                        }
                        None => {
                            // No policy and no payload — load into ledger for dedup
                            // but skip ingress admission (nothing to route).
                            self.inner.ledger_mut().recover(state);
                            continue;
                        }
                    },
                };
                let request_id = None;
                let reservation_key = None;

                let inserted = self.inner.ledger_mut().recover(state.clone());
                if !inserted {
                    // Filtered by ledger recover (e.g. ephemeral durability): do not
                    // admit to ingress, otherwise ingress queue truth can outlive
                    // canonical ledger truth.
                    continue;
                }

                if let Some(input) = state.persisted_input.clone() {
                    recovered_payloads.push((state.input_id.clone(), input));
                }

                let lifecycle_state = state.current_state();

                if let Err(err) = self.inner.admit_recovered_to_ingress(
                    state.input_id.clone(),
                    content_shape,
                    handling_mode,
                    lifecycle_state,
                    policy,
                    request_id,
                    reservation_key,
                ) {
                    return Err(RuntimeDriverError::Internal(format!(
                        "failed to admit recovered input '{}' to ingress authority: {err}",
                        state.input_id
                    )));
                }
            }
        }

        // Then run ephemeral recovery logic to finalize ingress recovery,
        // execute any remaining per-input recovery effects, and rebuild the
        // physical queue projections from canonical ingress truth.
        let report = self.inner.recover().await?;

        for (input_id, _input) in recovered_payloads {
            let should_requeue = self.inner.input_state(&input_id).is_some_and(|state| {
                state.current_state() == crate::input_state::InputLifecycleState::Queued
            });
            if should_requeue && !self.inner.has_queued_input(&input_id) {
                return Err(RuntimeDriverError::Internal(format!(
                    "persistent recover left queued input '{input_id}' out of the runtime queue projection"
                )));
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
            // is terminal but active inputs exist, fail closed as store
            // corruption instead of mutating queue projections in shell code.
            if runtime_state.is_terminal() {
                let active = self.inner.active_input_ids();
                if !active.is_empty() {
                    return Err(RuntimeDriverError::Internal(format!(
                        "store corruption: terminal runtime '{}' has {} active inputs",
                        runtime_state,
                        active.len()
                    )));
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
