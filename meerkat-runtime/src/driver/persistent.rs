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
use crate::traits::{RecoveryReport, RuntimeControlCommand, RuntimeDriver, RuntimeDriverError};

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

    /// Check if the runtime is idle (delegates to inner).
    pub fn is_idle(&self) -> bool {
        self.inner.is_idle()
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

    /// Stage input (delegates to inner).
    pub fn stage_input(
        &mut self,
        input_id: &InputId,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<(), crate::input_machine::InputStateMachineError> {
        self.inner.stage_input(input_id, run_id)
    }

    /// Apply input (delegates to inner).
    pub fn apply_input(
        &mut self,
        input_id: &InputId,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<(), crate::input_machine::InputStateMachineError> {
        self.inner.apply_input(input_id, run_id)
    }

    /// Roll back staged inputs (delegates to inner).
    pub fn rollback_staged(
        &mut self,
        input_ids: &[InputId],
    ) -> Result<(), crate::input_machine::InputStateMachineError> {
        self.inner.rollback_staged(input_ids)
    }

    /// Consume applied inputs without completing a runtime run.
    pub fn consume_inputs(
        &mut self,
        input_ids: &[InputId],
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<(), crate::input_machine::InputStateMachineError> {
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
                        self.inner.runtime_state(),
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
            .atomic_lifecycle_commit(&self.runtime_id, self.inner.runtime_state(), &input_states)
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
                state.current_state,
                InputLifecycleState::Applied | InputLifecycleState::AppliedPendingConsumption
            ) {
                let has_receipt = match (state.last_run_id.clone(), state.last_boundary_sequence) {
                    (Some(run_id), Some(sequence)) => self
                        .store
                        .load_boundary_receipt(&self.runtime_id, &run_id, sequence)
                        .await
                        .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?
                        .is_some(),
                    _ => false,
                };
                let now = Utc::now();
                if has_receipt {
                    state.history.push(InputStateHistoryEntry {
                        timestamp: now,
                        from: state.current_state,
                        to: InputLifecycleState::Consumed,
                        reason: Some("recovery: boundary receipt already committed".into()),
                    });
                    state.current_state = InputLifecycleState::Consumed;
                    state.terminal_outcome = Some(InputTerminalOutcome::Consumed);
                    state.updated_at = now;
                } else {
                    state.history.push(InputStateHistoryEntry {
                        timestamp: now,
                        from: state.current_state,
                        to: InputLifecycleState::Queued,
                        reason: Some("recovery: missing boundary receipt".into()),
                    });
                    state.current_state = InputLifecycleState::Queued;
                    state.updated_at = now;
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
                state.current_state == crate::input_state::InputLifecycleState::Queued
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
            .atomic_lifecycle_commit(&self.runtime_id, self.inner.runtime_state(), &input_states)
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
            .atomic_lifecycle_commit(&self.runtime_id, self.inner.runtime_state(), &input_states)
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
            .atomic_lifecycle_commit(&self.runtime_id, self.inner.runtime_state(), &input_states)
            .await
        {
            self.inner = checkpoint;
            return Err(RuntimeDriverError::Internal(format!(
                "reset persist failed: {err}"
            )));
        }
        Ok(report)
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input::*;
    use crate::store::InMemoryRuntimeStore;
    use chrono::Utc;

    fn make_prompt(text: &str) -> Input {
        Input::Prompt(PromptInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            text: text.into(),
            turn_metadata: None,
        })
    }

    #[tokio::test]
    async fn durable_before_ack() {
        let store = Arc::new(InMemoryRuntimeStore::new());
        let rid = LogicalRuntimeId::new("test");
        let mut driver = PersistentRuntimeDriver::new(rid.clone(), store.clone());

        let input = make_prompt("hello");
        let input_id = input.id().clone();
        let outcome = driver.accept_input(input).await.unwrap();
        assert!(outcome.is_accepted());

        // Verify state was persisted to store BEFORE we returned
        let stored = store.load_input_state(&rid, &input_id).await.unwrap();
        assert!(stored.is_some());
        assert!(stored.unwrap().persisted_input.is_some());
    }

    #[tokio::test]
    async fn dedup_not_persisted() {
        let store = Arc::new(InMemoryRuntimeStore::new());
        let rid = LogicalRuntimeId::new("test");
        let mut driver = PersistentRuntimeDriver::new(rid.clone(), store.clone());

        let key = crate::identifiers::IdempotencyKey::new("req-1");
        let mut input1 = make_prompt("hello");
        if let Input::Prompt(ref mut p) = input1 {
            p.header.idempotency_key = Some(key.clone());
        }
        driver.accept_input(input1).await.unwrap();

        let mut input2 = make_prompt("hello again");
        if let Input::Prompt(ref mut p) = input2 {
            p.header.idempotency_key = Some(key);
        }
        let outcome = driver.accept_input(input2).await.unwrap();
        assert!(outcome.is_deduplicated());

        // Only one state in store
        let states = store.load_input_states(&rid).await.unwrap();
        assert_eq!(states.len(), 1);
    }

    #[tokio::test]
    async fn recover_from_store() {
        let store = Arc::new(InMemoryRuntimeStore::new());
        let rid = LogicalRuntimeId::new("test");

        // Pre-populate store with a state (simulating crash recovery)
        let input = make_prompt("hello");
        let input_id = input.id().clone();
        let mut state = InputState::new_accepted(input_id.clone());
        state.persisted_input = Some(input.clone());
        state.durability = Some(InputDurability::Durable);
        store.persist_input_state(&rid, &state).await.unwrap();

        // Create a fresh driver (simulating restart)
        let mut driver = PersistentRuntimeDriver::new(rid, store);

        // Recover
        let report = driver.recover().await.unwrap();
        assert_eq!(report.inputs_recovered, 1);

        // State should now be in the driver
        assert!(driver.input_state(&input_id).is_some());
        let dequeued = driver.dequeue_next();
        assert!(
            dequeued.is_some(),
            "Recovered queued input should be re-enqueued"
        );
        let (queued_id, queued_input) = dequeued.unwrap();
        assert_eq!(queued_id, input_id);
        assert_eq!(queued_input.id(), &input_id);
    }

    #[tokio::test]
    async fn recover_rebuilds_dedup_index() {
        let store = Arc::new(InMemoryRuntimeStore::new());
        let rid = LogicalRuntimeId::new("test");
        let key = crate::identifiers::IdempotencyKey::new("dedup-key");

        // Pre-populate store with a state that has an idempotency key
        let input_id = InputId::new();
        let mut state = InputState::new_accepted(input_id.clone());
        state.idempotency_key = Some(key.clone());
        state.durability = Some(InputDurability::Durable);
        store.persist_input_state(&rid, &state).await.unwrap();

        // Create a fresh driver and recover
        let mut driver = PersistentRuntimeDriver::new(rid, store);
        driver.recover().await.unwrap();

        // Now try to accept a new input with the same idempotency key
        let mut dup_input = make_prompt("duplicate");
        if let Input::Prompt(ref mut p) = dup_input {
            p.header.idempotency_key = Some(key);
        }
        let outcome = driver.accept_input(dup_input).await.unwrap();
        assert!(
            outcome.is_deduplicated(),
            "After recovery, dedup index should be rebuilt so duplicates are caught"
        );
    }

    #[tokio::test]
    async fn recover_filters_ephemeral_inputs() {
        let store = Arc::new(InMemoryRuntimeStore::new());
        let rid = LogicalRuntimeId::new("test");

        // Pre-populate with an ephemeral input state
        let input_id = InputId::new();
        let mut state = InputState::new_accepted(input_id.clone());
        state.durability = Some(InputDurability::Ephemeral);
        store.persist_input_state(&rid, &state).await.unwrap();

        // Create fresh driver and recover
        let mut driver = PersistentRuntimeDriver::new(rid, store);
        let report = driver.recover().await.unwrap();

        // Ephemeral input should NOT be recovered (it shouldn't survive restart)
        assert!(
            driver.input_state(&input_id).is_none(),
            "Ephemeral inputs should be filtered during recovery"
        );
        assert_eq!(report.inputs_recovered, 0);
    }

    #[tokio::test]
    async fn boundary_applied_persists_atomically() {
        use meerkat_core::lifecycle::RunId;
        use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
        use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;

        let store = Arc::new(InMemoryRuntimeStore::new());
        let rid = LogicalRuntimeId::new("test");
        let mut driver = PersistentRuntimeDriver::new(rid.clone(), store.clone());

        // Accept and manually process an input
        let input = make_prompt("hello");
        let input_id = input.id().clone();
        driver.accept_input(input).await.unwrap();

        let run_id = RunId::new();
        driver.start_run(run_id.clone()).unwrap();
        driver.stage_input(&input_id, &run_id).unwrap();

        // Fire BoundaryApplied — this should persist atomically
        let receipt = RunBoundaryReceipt {
            run_id: run_id.clone(),
            boundary: RunApplyBoundary::RunStart,
            contributing_input_ids: vec![input_id.clone()],
            conversation_digest: None,
            message_count: 1,
            sequence: 0,
        };
        driver
            .on_run_event(meerkat_core::lifecycle::RunEvent::BoundaryApplied {
                run_id: run_id.clone(),
                receipt: receipt.clone(),
                session_snapshot: Some(b"session-data".to_vec()),
            })
            .await
            .unwrap();

        // Verify the receipt was persisted via atomic_apply
        let loaded = store.load_boundary_receipt(&rid, &run_id, 0).await.unwrap();
        assert!(
            loaded.is_some(),
            "BoundaryApplied should persist the receipt via atomic_apply"
        );
    }

    #[tokio::test]
    async fn retire_preserves_inputs_for_drain() {
        let store = Arc::new(InMemoryRuntimeStore::new());
        let rid = LogicalRuntimeId::new("test");
        let mut driver = PersistentRuntimeDriver::new(rid.clone(), store.clone());

        let input = make_prompt("hello");
        let input_id = input.id().clone();
        driver.accept_input(input).await.unwrap();

        let report = driver.retire().await.unwrap();
        assert_eq!(report.inputs_abandoned, 0);
        assert_eq!(report.inputs_pending_drain, 1);

        // Input is still queued, not abandoned
        let stored = store
            .load_input_state(&rid, &input_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.current_state,
            crate::input_state::InputLifecycleState::Queued
        );
    }

    #[tokio::test]
    async fn reset_persists_abandoned_inputs() {
        let store = Arc::new(InMemoryRuntimeStore::new());
        let rid = LogicalRuntimeId::new("test");
        let mut driver = PersistentRuntimeDriver::new(rid.clone(), store.clone());

        let input = make_prompt("hello");
        let input_id = input.id().clone();
        driver.accept_input(input).await.unwrap();

        let report = driver.reset().await.unwrap();
        assert_eq!(report.inputs_abandoned, 1);

        let stored = store
            .load_input_state(&rid, &input_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.current_state,
            crate::input_state::InputLifecycleState::Abandoned
        );
    }

    #[tokio::test]
    async fn recover_consumes_committed_applied_pending_inputs() {
        use meerkat_core::lifecycle::RunId;
        use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
        use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;

        let store = Arc::new(InMemoryRuntimeStore::new());
        let rid = LogicalRuntimeId::new("test");
        let input = make_prompt("already committed");
        let input_id = input.id().clone();
        let run_id = RunId::new();

        let mut state = InputState::new_accepted(input_id.clone());
        state.persisted_input = Some(input);
        state.durability = Some(InputDurability::Durable);
        state.current_state = InputLifecycleState::AppliedPendingConsumption;
        state.last_run_id = Some(run_id.clone());
        state.last_boundary_sequence = Some(0);
        store.persist_input_state(&rid, &state).await.unwrap();
        store
            .atomic_apply(
                &rid,
                None,
                RunBoundaryReceipt {
                    run_id: run_id.clone(),
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: vec![input_id.clone()],
                    conversation_digest: None,
                    message_count: 1,
                    sequence: 0,
                },
                vec![state.clone()],
            )
            .await
            .unwrap();

        let mut driver = PersistentRuntimeDriver::new(rid, store);
        driver.recover().await.unwrap();

        let recovered = driver.input_state(&input_id);
        assert!(
            recovered.is_some(),
            "committed input should remain queryable after recovery"
        );
        let Some(recovered) = recovered else {
            unreachable!("asserted some recovery state above");
        };
        assert_eq!(recovered.current_state, InputLifecycleState::Consumed);
        assert!(
            driver.active_input_ids().is_empty(),
            "committed applied inputs should not stay active after recovery"
        );
        assert!(
            driver.dequeue_next().is_none(),
            "committed applied inputs should not be replayed after recovery"
        );
    }
}
