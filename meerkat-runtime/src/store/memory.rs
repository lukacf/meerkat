//! InMemoryRuntimeStore — in-memory implementation for testing/ephemeral.
//!
//! Uses `tokio::sync::Mutex` per the in-memory concurrency rule.
//! All mutations complete inside one lock acquisition (no lock held across .await).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use indexmap::IndexMap;
use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::Mutex;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::Mutex;

use super::{
    AuthOAuthFlowSnapshotUpdate, MachineLifecycleCommit, RuntimeStore, RuntimeStoreError,
    SessionDelta,
};
use crate::identifiers::LogicalRuntimeId;
use crate::input_state::StoredInputState;
use crate::ops_lifecycle::PersistedOpsSnapshot;
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
    /// runtime_id → (input_id → StoredInputState). IndexMap for deterministic iteration order.
    input_states: HashMap<String, IndexMap<InputId, StoredInputState>>,
    /// Receipt storage.
    receipts: HashMap<ReceiptKey, RunBoundaryReceipt>,
    /// Runtime session snapshots keyed by canonical runtime id.
    sessions: HashMap<String, Vec<u8>>,
    /// Persisted runtime state.
    runtime_states: HashMap<String, RuntimeState>,
    /// Persisted ops lifecycle snapshots.
    ops_lifecycle_snapshots: HashMap<String, PersistedOpsSnapshot>,
}

/// In-memory runtime store. Thread-safe via `tokio::sync::Mutex`.
#[derive(Debug, Clone)]
pub struct InMemoryRuntimeStore {
    inner: Arc<Mutex<Inner>>,
    auth_oauth_flow_snapshot: Arc<StdMutex<Option<Vec<u8>>>>,
}

impl InMemoryRuntimeStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
            auth_oauth_flow_snapshot: Arc::new(StdMutex::new(None)),
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
    fn persist_auth_oauth_flow_snapshot(
        &self,
        snapshot_json: &[u8],
    ) -> Result<(), RuntimeStoreError> {
        *self
            .auth_oauth_flow_snapshot
            .lock()
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))? =
            Some(snapshot_json.to_vec());
        Ok(())
    }

    fn load_auth_oauth_flow_snapshot(&self) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        self.auth_oauth_flow_snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
    }

    fn update_auth_oauth_flow_snapshot(
        &self,
        update: &mut AuthOAuthFlowSnapshotUpdate<'_>,
    ) -> Result<(), RuntimeStoreError> {
        let mut snapshot = self
            .auth_oauth_flow_snapshot
            .lock()
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        let next = update(snapshot.as_deref())?;
        *snapshot = Some(next);
        Ok(())
    }

    async fn commit_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: SessionDelta,
    ) -> Result<(), RuntimeStoreError> {
        let _: meerkat_core::Session = serde_json::from_slice(&session_delta.session_snapshot)
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        let mut inner = self.inner.lock().await;
        inner
            .sessions
            .insert(runtime_id.0.clone(), session_delta.session_snapshot);
        Ok(())
    }

    async fn atomic_apply(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: Option<SessionDelta>,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<StoredInputState>,
        session_store_key: Option<meerkat_core::types::SessionId>,
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;

        // All writes in one lock acquisition (atomic for in-memory)
        let rid = runtime_id.0.clone();

        // Session delta
        if let Some(delta) = session_delta {
            if let Some(session_store_key) = session_store_key {
                let session: meerkat_core::Session =
                    serde_json::from_slice(&delta.session_snapshot)
                        .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                if session.id() != &session_store_key {
                    return Err(RuntimeStoreError::SessionKeyMismatch {
                        expected: session_store_key,
                        actual: session.id().clone(),
                    });
                }
            }
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
        for bundle in input_updates {
            states.insert(bundle.state.input_id.clone(), bundle);
        }

        Ok(())
    }

    async fn load_input_states(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<StoredInputState>, RuntimeStoreError> {
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
        state: &StoredInputState,
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        let states = inner.input_states.entry(runtime_id.0.clone()).or_default();
        states.insert(state.state.input_id.clone(), state.clone());
        Ok(())
    }

    async fn load_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        let state = inner
            .input_states
            .get(&runtime_id.0)
            .and_then(|m| m.get(input_id).cloned());
        Ok(state)
    }

    async fn load_runtime_state(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeState>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        Ok(inner.runtime_states.get(&runtime_id.0).copied())
    }

    async fn commit_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        commit: MachineLifecycleCommit,
        input_states: &[StoredInputState],
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        let rid = runtime_id.0.clone();

        // Single lock acquisition — atomic for in-memory
        inner
            .runtime_states
            .insert(rid.clone(), commit.runtime_state());
        let states = inner.input_states.entry(rid).or_default();
        for bundle in input_states {
            states.insert(bundle.state.input_id.clone(), bundle.clone());
        }

        Ok(())
    }

    async fn persist_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        snapshot: &PersistedOpsSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        inner
            .ops_lifecycle_snapshots
            .insert(runtime_id.0.clone(), snapshot.clone());
        Ok(())
    }

    async fn load_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<PersistedOpsSnapshot>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        Ok(inner.ops_lifecycle_snapshots.get(&runtime_id.0).cloned())
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

        let bundle = StoredInputState::new_accepted(input_id.clone());
        let receipt = make_receipt(run_id.clone(), 0);

        store
            .atomic_apply(
                &rid,
                Some(SessionDelta {
                    session_snapshot: b"session-data".to_vec(),
                }),
                receipt.clone(),
                vec![bundle],
                None,
            )
            .await
            .unwrap();

        // Load input states
        let states = store.load_input_states(&rid).await.unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].state.input_id, input_id);

        // Load receipt
        let loaded = store.load_boundary_receipt(&rid, &run_id, 0).await.unwrap();
        assert!(loaded.is_some());
    }

    #[tokio::test]
    async fn persist_and_load_single_state() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("test");
        let input_id = InputId::new();
        let bundle = StoredInputState::new_accepted(input_id.clone());

        store.persist_input_state(&rid, &bundle).await.unwrap();

        let loaded = store.load_input_state(&rid, &input_id).await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().state.input_id, input_id);
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
        let bundle1 = StoredInputState::new_accepted(input_id.clone());
        store
            .atomic_apply(
                &rid,
                None,
                make_receipt(RunId::new(), 0),
                vec![bundle1],
                None,
            )
            .await
            .unwrap();

        // Second write with updated seed phase
        let mut bundle2 = StoredInputState::new_accepted(input_id.clone());
        bundle2.seed.phase = crate::input_state::InputLifecycleState::Queued;
        store
            .atomic_apply(
                &rid,
                None,
                make_receipt(RunId::new(), 1),
                vec![bundle2],
                None,
            )
            .await
            .unwrap();

        let states = store.load_input_states(&rid).await.unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(
            states[0].seed.phase,
            crate::input_state::InputLifecycleState::Queued
        );
    }

    #[tokio::test]
    async fn atomic_apply_validates_session_store_key_without_aliasing_snapshot() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("runtime-key");
        let session = meerkat_core::Session::new();
        let session_id = session.id().clone();
        let snapshot = serde_json::to_vec(&session).unwrap();

        store
            .atomic_apply(
                &rid,
                Some(SessionDelta {
                    session_snapshot: snapshot.clone(),
                }),
                make_receipt(RunId::new(), 0),
                vec![],
                Some(session_id.clone()),
            )
            .await
            .unwrap();

        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(snapshot)
        );
        assert!(
            store
                .load_session_snapshot(&LogicalRuntimeId::legacy_session_uuid_alias(&session_id))
                .await
                .unwrap()
                .is_none(),
            "session_store_key must validate the snapshot identity, not create a raw UUID runtime alias"
        );
    }

    #[tokio::test]
    async fn atomic_apply_rejects_mismatched_session_store_key() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("runtime-key");
        let session = meerkat_core::Session::new();
        let wrong_session_id = meerkat_core::Session::new().id().clone();
        let snapshot = serde_json::to_vec(&session).unwrap();

        let err = store
            .atomic_apply(
                &rid,
                Some(SessionDelta {
                    session_snapshot: snapshot,
                }),
                make_receipt(RunId::new(), 0),
                vec![],
                Some(wrong_session_id),
            )
            .await
            .expect_err("mismatched session_store_key should fail");

        assert!(matches!(err, RuntimeStoreError::SessionKeyMismatch { .. }));
        assert!(store.load_session_snapshot(&rid).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn atomic_apply_persists_machine_owned_receipt() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("test");
        let run_id = RunId::new();
        let input_id = InputId::new();
        let session = meerkat_core::Session::new();
        let snapshot = serde_json::to_vec(&session).unwrap();
        let receipt = RunBoundaryReceipt {
            run_id: run_id.clone(),
            boundary: RunApplyBoundary::Immediate,
            contributing_input_ids: vec![input_id.clone()],
            conversation_digest: Some("machine-owned-digest".to_string()),
            message_count: 42,
            sequence: 7,
        };

        store
            .atomic_apply(
                &rid,
                Some(SessionDelta {
                    session_snapshot: snapshot,
                }),
                receipt.clone(),
                vec![StoredInputState::new_accepted(input_id)],
                None,
            )
            .await
            .unwrap();

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
            .persist_input_state(&rid1, &StoredInputState::new_accepted(InputId::new()))
            .await
            .unwrap();
        store
            .persist_input_state(&rid2, &StoredInputState::new_accepted(InputId::new()))
            .await
            .unwrap();
        store
            .persist_input_state(&rid2, &StoredInputState::new_accepted(InputId::new()))
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
                None,
            )
            .await
            .unwrap();

        let loaded = store.load_session_snapshot(&rid).await.unwrap();
        assert_eq!(loaded, Some(snapshot));
    }
}
