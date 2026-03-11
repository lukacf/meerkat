//! PersistentRuntimeDriver — wraps EphemeralRuntimeDriver + RuntimeStore.
//!
//! Provides durable-before-ack guarantee: InputState is persisted via
//! RuntimeStore BEFORE returning AcceptOutcome. Delegates state machine
//! logic to the ephemeral driver.

use std::sync::Arc;

use meerkat_core::lifecycle::{InputId, RunEvent};

use crate::accept::AcceptOutcome;
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_state::InputState;
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
}

#[async_trait::async_trait]
impl RuntimeDriver for PersistentRuntimeDriver {
    async fn accept_input(&mut self, input: Input) -> Result<AcceptOutcome, RuntimeDriverError> {
        // Delegate to ephemeral for state machine logic
        let outcome = self.inner.accept_input(input).await?;

        // Durable-before-ack: persist InputState before returning
        if let AcceptOutcome::Accepted { ref input_id, .. } = outcome
            && let Some(state) = self.inner.input_state(input_id)
        {
            self.store
                .persist_input_state(&self.runtime_id, state)
                .await
                .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
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
        // Delegate to inner first (updates state machine and input lifecycle)
        self.inner.on_run_event(event.clone()).await?;

        // For BoundaryApplied, persist receipt + updated input states atomically
        if let RunEvent::BoundaryApplied { ref receipt, .. } = event {
            let input_updates: Vec<InputState> = receipt
                .contributing_input_ids
                .iter()
                .filter_map(|id| self.inner.input_state(id).cloned())
                .collect();

            self.store
                .atomic_apply(&self.runtime_id, None, receipt.clone(), input_updates)
                .await
                .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;
        }

        Ok(())
    }

    async fn on_runtime_control(
        &mut self,
        command: RuntimeControlCommand,
    ) -> Result<(), RuntimeDriverError> {
        self.inner.on_runtime_control(command).await
    }

    async fn recover(&mut self) -> Result<RecoveryReport, RuntimeDriverError> {
        // §24 full recovery: load durable InputState from store
        let stored_states = self
            .store
            .load_input_states(&self.runtime_id)
            .await
            .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?;

        // Inject stored states into the ephemeral driver's ledger.
        // Uses recover() which also rebuilds the idempotency index for
        // dedup correctness and filters out Ephemeral inputs.
        for state in stored_states {
            // Only inject non-terminal states (terminal ones are already resolved)
            if !state.is_terminal() {
                let ledger = &mut self.inner;
                if ledger.input_state(&state.input_id).is_none() {
                    ledger.ledger_mut().recover(state);
                }
            }
        }

        // Then run ephemeral recovery logic (requeue Accepted/Staged)
        self.inner.recover().await
    }

    async fn retire(&mut self) -> Result<crate::traits::RetireReport, RuntimeDriverError> {
        EphemeralRuntimeDriver::retire(&mut self.inner)
    }

    async fn reset(&mut self) -> Result<crate::traits::ResetReport, RuntimeDriverError> {
        EphemeralRuntimeDriver::reset(&mut self.inner)
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
        let input_id = InputId::new();
        let state = InputState::new_accepted(input_id.clone());
        store.persist_input_state(&rid, &state).await.unwrap();

        // Create a fresh driver (simulating restart)
        let mut driver = PersistentRuntimeDriver::new(rid, store);

        // Recover
        let report = driver.recover().await.unwrap();
        assert_eq!(report.inputs_recovered, 1);

        // State should now be in the driver
        assert!(driver.input_state(&input_id).is_some());
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
}
