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
        self.inner.on_run_event(event).await
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

        // Inject stored states into the ephemeral driver's ledger
        for state in stored_states {
            // Only inject non-terminal states (terminal ones are already resolved)
            if !state.is_terminal() {
                // The ephemeral ledger may already have some states from
                // accept_input calls that survived in memory. Stored states
                // take precedence as they represent the durable truth.
                let ledger = &mut self.inner;
                // Access the ledger directly — we need to rebuild from store
                if ledger.input_state(&state.input_id).is_none() {
                    // State exists in store but not in memory — add it
                    ledger.ledger_mut().accept(state);
                }
            }
        }

        // Then run ephemeral recovery logic (requeue Accepted/Staged)
        self.inner.recover().await
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
}
