//! InMemoryRuntimeStore — in-memory implementation for testing/ephemeral.
//!
//! Uses `tokio::sync::Mutex` per the in-memory concurrency rule.
//! All mutations complete inside one lock acquisition (no lock held across .await).

use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::Mutex;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::Mutex;

use super::{RuntimeStore, RuntimeStoreError, SessionDelta, authoritative_receipt};
use crate::identifiers::LogicalRuntimeId;
use crate::input_state::InputState;
use crate::runtime_state::RuntimeState;

/// Receipt key: (runtime_id, run_id, sequence).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReceiptKey {
    runtime_id: String,
    run_id: RunId,
    sequence: u64,
}

/// Inner state protected by the mutex.
#[derive(Debug, Default)]
struct Inner {
    /// runtime_id → (input_id → InputState). IndexMap for deterministic iteration order.
    input_states: HashMap<String, IndexMap<InputId, InputState>>,
    /// Receipt storage.
    receipts: HashMap<ReceiptKey, RunBoundaryReceipt>,
    /// Session snapshots (opaque bytes).
    sessions: HashMap<String, Vec<u8>>,
    /// Persisted runtime state.
    runtime_states: HashMap<String, RuntimeState>,
}

/// In-memory runtime store. Thread-safe via `tokio::sync::Mutex`.
#[derive(Debug, Clone)]
pub struct InMemoryRuntimeStore {
    inner: Arc<Mutex<Inner>>,
}

impl InMemoryRuntimeStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }
}

impl Default for InMemoryRuntimeStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl RuntimeStore for InMemoryRuntimeStore {
    async fn commit_session_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: SessionDelta,
        run_id: RunId,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        input_updates: Vec<InputState>,
    ) -> Result<RunBoundaryReceipt, RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        let rid = runtime_id.0.clone();
        let sequence = inner
            .receipts
            .keys()
            .filter(|key| key.runtime_id == rid && key.run_id == run_id)
            .map(|key| key.sequence)
            .max()
            .map(|seq| seq + 1)
            .unwrap_or(0);
        let receipt = authoritative_receipt(
            Some(&session_delta),
            run_id,
            boundary,
            contributing_input_ids,
            sequence,
        )?;
        let mut input_updates = input_updates;
        for state in &mut input_updates {
            state.last_run_id = Some(receipt.run_id.clone());
            state.last_boundary_sequence = Some(receipt.sequence);
        }

        inner
            .sessions
            .insert(rid.clone(), session_delta.session_snapshot);
        let key = ReceiptKey {
            runtime_id: rid.clone(),
            run_id: receipt.run_id.clone(),
            sequence: receipt.sequence,
        };
        inner.receipts.insert(key, receipt.clone());

        let states = inner.input_states.entry(rid).or_default();
        for state in input_updates {
            states.insert(state.input_id.clone(), state);
        }

        Ok(receipt)
    }

    async fn atomic_apply(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: Option<SessionDelta>,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputState>,
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;

        // All writes in one lock acquisition (atomic for in-memory)
        let rid = runtime_id.0.clone();

        // Session delta
        if let Some(delta) = session_delta {
            inner.sessions.insert(rid.clone(), delta.session_snapshot);
        }

        // Receipt
        let key = ReceiptKey {
            runtime_id: rid.clone(),
            run_id: receipt.run_id.clone(),
            sequence: receipt.sequence,
        };
        inner.receipts.insert(key, receipt);

        // Input states
        let states = inner.input_states.entry(rid).or_default();
        for state in input_updates {
            states.insert(state.input_id.clone(), state);
        }

        Ok(())
    }

    async fn load_input_states(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<InputState>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        let states = inner
            .input_states
            .get(&runtime_id.0)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default();
        Ok(states)
    }

    async fn load_boundary_receipt(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<Option<RunBoundaryReceipt>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        let key = ReceiptKey {
            runtime_id: runtime_id.0.clone(),
            run_id: run_id.clone(),
            sequence,
        };
        Ok(inner.receipts.get(&key).cloned())
    }

    async fn load_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        Ok(inner.sessions.get(&runtime_id.0).cloned())
    }

    async fn persist_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        state: &InputState,
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        let states = inner.input_states.entry(runtime_id.0.clone()).or_default();
        states.insert(state.input_id.clone(), state.clone());
        Ok(())
    }

    async fn load_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_id: &InputId,
    ) -> Result<Option<InputState>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        let state = inner
            .input_states
            .get(&runtime_id.0)
            .and_then(|m| m.get(input_id).cloned());
        Ok(state)
    }

    async fn persist_runtime_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        state: RuntimeState,
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        inner.runtime_states.insert(runtime_id.0.clone(), state);
        Ok(())
    }

    async fn load_runtime_state(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeState>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        Ok(inner.runtime_states.get(&runtime_id.0).copied())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;

    fn make_receipt(run_id: RunId, seq: u64) -> RunBoundaryReceipt {
        RunBoundaryReceipt {
            run_id,
            boundary: RunApplyBoundary::RunStart,
            contributing_input_ids: vec![],
            conversation_digest: None,
            message_count: 0,
            sequence: seq,
        }
    }

    #[tokio::test]
    async fn atomic_apply_roundtrip() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("test-runtime");
        let run_id = RunId::new();
        let input_id = InputId::new();

        let state = InputState::new_accepted(input_id.clone());
        let receipt = make_receipt(run_id.clone(), 0);

        store
            .atomic_apply(
                &rid,
                Some(SessionDelta {
                    session_snapshot: b"session-data".to_vec(),
                }),
                receipt.clone(),
                vec![state],
            )
            .await
            .unwrap();

        // Load input states
        let states = store.load_input_states(&rid).await.unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].input_id, input_id);

        // Load receipt
        let loaded = store.load_boundary_receipt(&rid, &run_id, 0).await.unwrap();
        assert!(loaded.is_some());
    }

    #[tokio::test]
    async fn persist_and_load_single_state() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("test");
        let input_id = InputId::new();
        let state = InputState::new_accepted(input_id.clone());

        store.persist_input_state(&rid, &state).await.unwrap();

        let loaded = store.load_input_state(&rid, &input_id).await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().input_id, input_id);
    }

    #[tokio::test]
    async fn load_nonexistent_returns_none() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("test");

        let states = store.load_input_states(&rid).await.unwrap();
        assert!(states.is_empty());

        let state = store.load_input_state(&rid, &InputId::new()).await.unwrap();
        assert!(state.is_none());

        let receipt = store
            .load_boundary_receipt(&rid, &RunId::new(), 0)
            .await
            .unwrap();
        assert!(receipt.is_none());
    }

    #[tokio::test]
    async fn atomic_apply_updates_existing() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("test");
        let input_id = InputId::new();

        // First write
        let state1 = InputState::new_accepted(input_id.clone());
        store
            .atomic_apply(&rid, None, make_receipt(RunId::new(), 0), vec![state1])
            .await
            .unwrap();

        // Second write with updated state
        let mut state2 = InputState::new_accepted(input_id.clone());
        state2.current_state = crate::input_state::InputLifecycleState::Queued;
        store
            .atomic_apply(&rid, None, make_receipt(RunId::new(), 1), vec![state2])
            .await
            .unwrap();

        let states = store.load_input_states(&rid).await.unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(
            states[0].current_state,
            crate::input_state::InputLifecycleState::Queued
        );
    }

    #[tokio::test]
    async fn commit_session_boundary_returns_authoritative_receipt() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("test");
        let run_id = RunId::new();
        let input_id = InputId::new();
        let session = meerkat_core::Session::new();
        let snapshot = serde_json::to_vec(&session).unwrap();

        let receipt = store
            .commit_session_boundary(
                &rid,
                SessionDelta {
                    session_snapshot: snapshot,
                },
                run_id.clone(),
                RunApplyBoundary::Immediate,
                vec![input_id.clone()],
                vec![InputState::new_accepted(input_id)],
            )
            .await
            .unwrap();

        assert_eq!(receipt.sequence, 0);
        assert_eq!(receipt.run_id, run_id);
        assert!(receipt.conversation_digest.is_some());
        let loaded = store
            .load_boundary_receipt(&rid, &receipt.run_id, receipt.sequence)
            .await
            .unwrap();
        assert!(loaded.is_some(), "receipt should be persisted");
        let Some(loaded) = loaded else {
            unreachable!("asserted above");
        };
        assert_eq!(loaded, receipt);
    }

    #[tokio::test]
    async fn multiple_runtimes_isolated() {
        let store = InMemoryRuntimeStore::new();
        let rid1 = LogicalRuntimeId::new("runtime-1");
        let rid2 = LogicalRuntimeId::new("runtime-2");

        store
            .persist_input_state(&rid1, &InputState::new_accepted(InputId::new()))
            .await
            .unwrap();
        store
            .persist_input_state(&rid2, &InputState::new_accepted(InputId::new()))
            .await
            .unwrap();
        store
            .persist_input_state(&rid2, &InputState::new_accepted(InputId::new()))
            .await
            .unwrap();

        let s1 = store.load_input_states(&rid1).await.unwrap();
        let s2 = store.load_input_states(&rid2).await.unwrap();
        assert_eq!(s1.len(), 1);
        assert_eq!(s2.len(), 2);
    }

    #[tokio::test]
    async fn load_session_snapshot_roundtrip() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("runtime");
        let snapshot = serde_json::to_vec(&meerkat_core::Session::new()).unwrap();

        store
            .atomic_apply(
                &rid,
                Some(SessionDelta {
                    session_snapshot: snapshot.clone(),
                }),
                make_receipt(RunId::new(), 0),
                vec![],
            )
            .await
            .unwrap();

        let loaded = store.load_session_snapshot(&rid).await.unwrap();
        assert_eq!(loaded, Some(snapshot));
    }
}
