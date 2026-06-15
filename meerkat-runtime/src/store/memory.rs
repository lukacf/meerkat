//! InMemoryRuntimeStore — in-memory implementation for testing/ephemeral.
//!
//! Uses `tokio::sync::Mutex` per the in-memory concurrency rule.
//! All mutations complete inside one lock acquisition (no lock held across .await).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use indexmap::IndexMap;
use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::Mutex;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::Mutex;

use super::{
    AuthOAuthFlowSnapshotUpdate, MachineLifecycleCommit, MachineLifecycleSnapshot,
    MachineLifecycleStoreRecord, RuntimeStore, RuntimeStoreError, SessionDelta,
};
use crate::identifiers::LogicalRuntimeId;
use crate::input_state::{InputStatePersistenceRecord, StoredInputState};
use crate::ops_lifecycle::PersistedOpsSnapshot;

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
    /// Canonical runtime ids whose projection fallback is quarantined.
    ///
    /// Mirrors the durable SQLite `runtime_projection_quarantine` table: set
    /// when a rejected runtime snapshot is cleared via
    /// `clear_session_snapshot_if_current`, cleared whenever a live snapshot is
    /// written for the runtime.
    projection_quarantine: HashSet<String>,
    /// Persisted machine lifecycle snapshots.
    runtime_lifecycle: HashMap<String, MachineLifecycleSnapshot>,
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

fn is_runtime_placeholder_session(session: &meerkat_core::Session) -> bool {
    session.transcript_history_state().ok().flatten().is_none()
        && matches!(
            session.messages(),
            [] | [meerkat_core::types::Message::System(_)]
        )
}

/// Deserialize a persisted session-snapshot blob through typed serde, matching
/// the SQLite runtime store read path. `Session::deserialize` validates the
/// mandatory envelope version against the generated persistence version
/// authority, so a missing or non-current (v0/v1) row fails closed instead of
/// silently defaulting or upgrading on read.
fn deserialize_persisted_session(bytes: &[u8]) -> Result<meerkat_core::Session, RuntimeStoreError> {
    serde_json::from_slice(bytes).map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
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
        let incoming: meerkat_core::Session =
            serde_json::from_slice(&session_delta.session_snapshot)
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        let mut inner = self.inner.lock().await;
        let previous = inner
            .sessions
            .get(&runtime_id.0)
            .map(|snapshot| deserialize_persisted_session(snapshot))
            .transpose()?;
        meerkat_core::session_store::run_boundary_snapshot_save_guard(&incoming, previous.as_ref())
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        inner
            .sessions
            .insert(runtime_id.0.clone(), session_delta.session_snapshot);
        inner.projection_quarantine.remove(&runtime_id.0);
        Ok(())
    }

    async fn commit_session_transcript_rewrite_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: SessionDelta,
        commit: &meerkat_core::TranscriptRewriteCommit,
    ) -> Result<(), RuntimeStoreError> {
        let incoming: meerkat_core::Session =
            serde_json::from_slice(&session_delta.session_snapshot)
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        let mut inner = self.inner.lock().await;
        let previous = inner
            .sessions
            .get(&runtime_id.0)
            .map(|snapshot| deserialize_persisted_session(snapshot))
            .transpose()?;
        meerkat_core::session_store::transcript_rewrite_save_guard(
            &incoming,
            previous.as_ref(),
            commit,
        )
        .map_err(|err| match err {
            meerkat_core::SessionStoreError::TranscriptRevisionConflict {
                expected,
                actual,
                ..
            } => RuntimeStoreError::TranscriptRevisionConflict { expected, actual },
            other => RuntimeStoreError::WriteFailed(other.to_string()),
        })?;
        inner
            .sessions
            .insert(runtime_id.0.clone(), session_delta.session_snapshot);
        inner.projection_quarantine.remove(&runtime_id.0);
        Ok(())
    }

    async fn atomic_apply(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: Option<SessionDelta>,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: Option<meerkat_core::types::SessionId>,
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;

        // All writes in one lock acquisition (atomic for in-memory)
        let rid = runtime_id.0.clone();

        // Session delta. The supersession verdict computed here keys the
        // entire commit: if the incoming session snapshot is classified as
        // superseded (the persisted head is already a valid append-extension
        // of it), the snapshot write is skipped AND so are the receipt + input
        // writes, so receipt/input ordering identity never advances past the
        // retained session truth.
        let mut session_snapshot_superseded = false;
        if let Some(delta) = session_delta {
            let incoming_session =
                serde_json::from_slice::<meerkat_core::Session>(&delta.session_snapshot);
            let mut persist_session_snapshot = true;
            match (incoming_session, session_store_key) {
                (Ok(incoming_session), session_store_key) => {
                    if let Some(session_store_key) = session_store_key
                        && incoming_session.id() != &session_store_key
                    {
                        return Err(RuntimeStoreError::SessionKeyMismatch {
                            expected: session_store_key,
                            actual: incoming_session.id().clone(),
                        });
                    }
                    let previous_session = inner
                        .sessions
                        .get(&rid)
                        .and_then(|snapshot| deserialize_persisted_session(snapshot).ok());
                    if let Err(err) = meerkat_core::session_store::run_boundary_snapshot_save_guard(
                        &incoming_session,
                        previous_session.as_ref(),
                    ) {
                        if previous_session
                            .as_ref()
                            .is_some_and(is_runtime_placeholder_session)
                        {
                            persist_session_snapshot = true;
                        } else if previous_session.as_ref().is_some_and(|previous_session| {
                            meerkat_core::session_store::run_boundary_snapshot_save_guard(
                                previous_session,
                                Some(&incoming_session),
                            )
                            .is_ok()
                        }) {
                            persist_session_snapshot = false;
                            session_snapshot_superseded = true;
                        } else {
                            return Err(RuntimeStoreError::WriteFailed(err.to_string()));
                        }
                    }
                }
                (Err(err), Some(session_store_key)) => {
                    return Err(RuntimeStoreError::WriteFailed(format!(
                        "session snapshot for {session_store_key} is not a Session: {err}"
                    )));
                }
                (Err(err), None) => {
                    return Err(RuntimeStoreError::WriteFailed(format!(
                        "session snapshot is not a Session: {err}"
                    )));
                }
            }
            if persist_session_snapshot {
                inner.sessions.insert(rid.clone(), delta.session_snapshot);
                inner.projection_quarantine.remove(&rid);
            }
        }

        // When the session snapshot was superseded and skipped, the boundary
        // receipt and input-state updates for that boundary must also be
        // skipped: advancing them against a retained (older) session snapshot
        // would split receipt/input ordering identity from session truth.
        if session_snapshot_superseded {
            return Ok(());
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
        for record in input_updates {
            let bundle = record.into_stored();
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

    async fn clear_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        inner.sessions.remove(&runtime_id.0);
        Ok(())
    }

    async fn replace_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
        replacement: Vec<u8>,
    ) -> Result<bool, RuntimeStoreError> {
        let _: meerkat_core::Session = serde_json::from_slice(&replacement)
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        let mut inner = self.inner.lock().await;
        let Some(current) = inner.sessions.get_mut(&runtime_id.0) else {
            return Ok(false);
        };
        if current.as_slice() != expected_current {
            return Ok(false);
        }
        *current = replacement;
        inner.projection_quarantine.remove(&runtime_id.0);
        Ok(true)
    }

    async fn clear_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        let Some(current) = inner.sessions.get(&runtime_id.0) else {
            return Ok(false);
        };
        if current.as_slice() != expected_current {
            return Ok(false);
        }
        inner.sessions.remove(&runtime_id.0);
        // Record the in-memory quarantine marker atomically with the snapshot
        // removal, mirroring the durable SQLite path.
        inner.projection_quarantine.insert(runtime_id.0.clone());
        Ok(true)
    }

    async fn is_runtime_projection_quarantined(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<bool, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        Ok(inner.projection_quarantine.contains(&runtime_id.0))
    }

    async fn persist_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        state: &InputStatePersistenceRecord,
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        let states = inner.input_states.entry(runtime_id.0.clone()).or_default();
        let bundle = state.as_stored();
        states.insert(bundle.state.input_id.clone(), bundle.clone());
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

    async fn load_machine_lifecycle_record(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        let inner = self.inner.lock().await;
        inner
            .runtime_lifecycle
            .get(&runtime_id.0)
            .map(|snapshot| MachineLifecycleStoreRecord::from_snapshot(snapshot).encode())
            .transpose()
    }

    async fn commit_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        commit: MachineLifecycleCommit,
        input_states: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        let rid = runtime_id.0.clone();

        // Single lock acquisition — atomic for in-memory
        inner
            .runtime_lifecycle
            .insert(rid.clone(), commit.into_snapshot());
        let states = inner.input_states.entry(rid).or_default();
        for record in input_states {
            let bundle = record.as_stored();
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

    async fn delete_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        let mut inner = self.inner.lock().await;
        inner.ops_lifecycle_snapshots.remove(&runtime_id.0);
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::store::MachineLifecycleBindingFacts;
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

    fn persistable(bundle: StoredInputState) -> InputStatePersistenceRecord {
        InputStatePersistenceRecord::from_machine_snapshot(bundle).unwrap()
    }

    fn session_with_user(content: &str) -> meerkat_core::Session {
        let mut session = meerkat_core::Session::new();
        session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text(content.to_string()),
        ));
        session
    }

    #[tokio::test]
    async fn atomic_apply_roundtrip() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("test-runtime");
        let run_id = RunId::new();
        let input_id = InputId::new();

        let bundle = StoredInputState::new_accepted(input_id.clone());
        let receipt = make_receipt(run_id.clone(), 0);

        let session = session_with_user("hello");
        let session_snapshot = serde_json::to_vec(&session).unwrap();

        store
            .atomic_apply(
                &rid,
                Some(SessionDelta { session_snapshot }),
                receipt.clone(),
                vec![persistable(bundle)],
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
    async fn atomic_apply_rejects_non_session_snapshot_without_owner_context() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("test-runtime");
        let run_id = RunId::new();
        let input_id = InputId::new();

        let bundle = StoredInputState::new_accepted(input_id);
        let receipt = make_receipt(run_id, 0);

        // Owner-context absence is not a license to store arbitrary bytes as a
        // session snapshot: a non-deserializable snapshot must fail closed.
        let err = store
            .atomic_apply(
                &rid,
                Some(SessionDelta {
                    session_snapshot: b"session-data".to_vec(),
                }),
                receipt,
                vec![persistable(bundle)],
                None,
            )
            .await
            .expect_err("non-Session snapshot must be rejected");

        match err {
            RuntimeStoreError::WriteFailed(message) => {
                assert!(
                    message.contains("not a Session"),
                    "unexpected WriteFailed message: {message}"
                );
            }
            other => panic!("expected WriteFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn persist_and_load_single_state() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("test");
        let input_id = InputId::new();
        let bundle = StoredInputState::new_accepted(input_id.clone());

        store
            .persist_input_state(&rid, &persistable(bundle))
            .await
            .unwrap();

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
                vec![persistable(bundle1)],
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
                vec![persistable(bundle2)],
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
                vec![persistable(StoredInputState::new_accepted(input_id))],
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
            .persist_input_state(
                &rid1,
                &persistable(StoredInputState::new_accepted(InputId::new())),
            )
            .await
            .unwrap();
        store
            .persist_input_state(
                &rid2,
                &persistable(StoredInputState::new_accepted(InputId::new())),
            )
            .await
            .unwrap();
        store
            .persist_input_state(
                &rid2,
                &persistable(StoredInputState::new_accepted(InputId::new())),
            )
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

    #[tokio::test]
    async fn commit_session_snapshot_rejects_stale_runtime_parent() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("runtime-stale-parent");
        let accepted = session_with_user("accepted runtime turn");
        let mut stale = meerkat_core::Session::with_id(accepted.id().clone());
        stale.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("stale runtime turn".to_string()),
        ));
        let accepted_snapshot = serde_json::to_vec(&accepted).unwrap();

        store
            .commit_session_snapshot(
                &rid,
                SessionDelta {
                    session_snapshot: accepted_snapshot.clone(),
                },
            )
            .await
            .unwrap();

        let err = store
            .commit_session_snapshot(
                &rid,
                SessionDelta {
                    session_snapshot: serde_json::to_vec(&stale).unwrap(),
                },
            )
            .await
            .expect_err("stale non-continuation must not overwrite runtime snapshot");

        assert!(matches!(err, RuntimeStoreError::WriteFailed(_)));
        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(accepted_snapshot)
        );
    }

    #[tokio::test]
    async fn atomic_apply_keeps_current_snapshot_when_incoming_is_superseded() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("runtime-superseded-terminal");
        let incoming = session_with_user("turn input");
        let mut current = incoming.clone();
        current.push(meerkat_core::types::Message::BlockAssistant(
            meerkat_core::types::BlockAssistantMessage {
                blocks: vec![meerkat_core::types::AssistantBlock::Text {
                    text: "peer response already applied".to_string(),
                    meta: None,
                }],
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                created_at: meerkat_core::types::message_timestamp_now(),
            },
        ));
        let current_snapshot = serde_json::to_vec(&current).unwrap();
        let receipt = make_receipt(RunId::new(), 11);

        store
            .commit_session_snapshot(
                &rid,
                SessionDelta {
                    session_snapshot: current_snapshot.clone(),
                },
            )
            .await
            .unwrap();

        store
            .atomic_apply(
                &rid,
                Some(SessionDelta {
                    session_snapshot: serde_json::to_vec(&incoming).unwrap(),
                }),
                receipt.clone(),
                vec![],
                Some(incoming.id().clone()),
            )
            .await
            .unwrap();

        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(current_snapshot)
        );
        // The session snapshot was classified superseded and skipped, so the
        // boundary receipt for that boundary must NOT advance against the
        // retained (more-advanced) session snapshot.
        assert_eq!(
            store
                .load_boundary_receipt(&rid, &receipt.run_id, receipt.sequence)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn atomic_apply_skips_inputs_when_session_snapshot_superseded() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("runtime-superseded-inputs");
        let incoming = session_with_user("turn input");
        let mut current = incoming.clone();
        current.push(meerkat_core::types::Message::BlockAssistant(
            meerkat_core::types::BlockAssistantMessage {
                blocks: vec![meerkat_core::types::AssistantBlock::Text {
                    text: "peer response already applied".to_string(),
                    meta: None,
                }],
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                created_at: meerkat_core::types::message_timestamp_now(),
            },
        ));
        let current_snapshot = serde_json::to_vec(&current).unwrap();
        let receipt = make_receipt(RunId::new(), 21);
        let input_id = InputId::new();
        let bundle = StoredInputState::new_accepted(input_id.clone());

        store
            .commit_session_snapshot(
                &rid,
                SessionDelta {
                    session_snapshot: current_snapshot.clone(),
                },
            )
            .await
            .unwrap();

        store
            .atomic_apply(
                &rid,
                Some(SessionDelta {
                    session_snapshot: serde_json::to_vec(&incoming).unwrap(),
                }),
                receipt.clone(),
                vec![persistable(bundle)],
                Some(incoming.id().clone()),
            )
            .await
            .unwrap();

        // Snapshot retained, receipt + input-state writes skipped as a unit.
        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(current_snapshot)
        );
        assert_eq!(
            store
                .load_boundary_receipt(&rid, &receipt.run_id, receipt.sequence)
                .await
                .unwrap(),
            None
        );
        assert!(store.load_input_states(&rid).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn atomic_apply_allows_first_generated_snapshot_after_placeholder() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("runtime-placeholder");
        let mut placeholder = meerkat_core::Session::new();
        placeholder.set_system_prompt("base system".to_string());
        let mut incoming = meerkat_core::Session::with_id(placeholder.id().clone());
        incoming.set_system_prompt("base system".to_string());
        incoming.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("verbose first turn".to_string()),
        ));
        let parent_revision = incoming.transcript_revision().unwrap();
        incoming
            .commit_transcript_rewrite(
                meerkat_core::TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![meerkat_core::types::Message::User(
                    meerkat_core::types::UserMessage::compaction_summary(
                        "[Context compacted] first turn",
                    ),
                )],
                meerkat_core::TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(parent_revision),
            )
            .unwrap();
        let incoming_snapshot = serde_json::to_vec(&incoming).unwrap();
        let receipt = make_receipt(RunId::new(), 12);

        store
            .commit_session_snapshot(
                &rid,
                SessionDelta {
                    session_snapshot: serde_json::to_vec(&placeholder).unwrap(),
                },
            )
            .await
            .unwrap();

        store
            .atomic_apply(
                &rid,
                Some(SessionDelta {
                    session_snapshot: incoming_snapshot.clone(),
                }),
                receipt.clone(),
                vec![],
                Some(incoming.id().clone()),
            )
            .await
            .unwrap();

        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(incoming_snapshot)
        );
        assert_eq!(
            store
                .load_boundary_receipt(&rid, &receipt.run_id, receipt.sequence)
                .await
                .unwrap(),
            Some(receipt)
        );
    }

    #[tokio::test]
    async fn atomic_apply_allows_generated_compaction_before_retained_tail() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("runtime-compaction-tail");
        let mut previous = meerkat_core::Session::new();
        previous.set_system_prompt("runtime system before context refresh".to_string());
        previous.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text("Turn 1 request".to_string()),
        ));
        previous.push(meerkat_core::types::Message::BlockAssistant(
            meerkat_core::types::BlockAssistantMessage {
                blocks: vec![meerkat_core::types::AssistantBlock::Text {
                    text: "Turn 1 answer".to_string(),
                    meta: None,
                }],
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                created_at: meerkat_core::types::message_timestamp_now(),
            },
        ));

        let mut incoming = meerkat_core::Session::with_id(previous.id().clone());
        incoming.set_system_prompt("runtime system after context refresh".to_string());
        incoming.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text(
                "Verbose context that will be compacted".to_string(),
            ),
        ));
        for message in previous.messages()[1..].iter().cloned() {
            incoming.push(message);
        }
        incoming.push(meerkat_core::types::Message::BlockAssistant(
            meerkat_core::types::BlockAssistantMessage {
                blocks: vec![meerkat_core::types::AssistantBlock::Text {
                    text: "Turn 2 generated answer".to_string(),
                    meta: None,
                }],
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                created_at: meerkat_core::types::message_timestamp_now(),
            },
        ));
        let parent_revision = incoming.transcript_revision().unwrap();
        incoming
            .commit_transcript_rewrite(
                meerkat_core::TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![meerkat_core::types::Message::User(
                    meerkat_core::types::UserMessage::compaction_summary(
                        "[Context compacted] Earlier runtime context".to_string(),
                    ),
                )],
                meerkat_core::TranscriptRewriteReason::new("compaction"),
                Some("meerkat-core".to_string()),
                Some(parent_revision),
            )
            .unwrap();
        let incoming_snapshot = serde_json::to_vec(&incoming).unwrap();
        let receipt = make_receipt(RunId::new(), 13);

        store
            .commit_session_snapshot(
                &rid,
                SessionDelta {
                    session_snapshot: serde_json::to_vec(&previous).unwrap(),
                },
            )
            .await
            .unwrap();

        store
            .atomic_apply(
                &rid,
                Some(SessionDelta {
                    session_snapshot: incoming_snapshot.clone(),
                }),
                receipt.clone(),
                vec![],
                Some(incoming.id().clone()),
            )
            .await
            .unwrap();

        assert_eq!(
            store.load_session_snapshot(&rid).await.unwrap(),
            Some(incoming_snapshot)
        );
        assert_eq!(
            store
                .load_boundary_receipt(&rid, &receipt.run_id, receipt.sequence)
                .await
                .unwrap(),
            Some(receipt)
        );
    }

    #[tokio::test]
    async fn commit_machine_lifecycle_persists_binding_facts() {
        use crate::runtime_state::RuntimeState;

        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("runtime-binding");
        let binding = MachineLifecycleBindingFacts::new(
            Some("rt:session:abc".to_string()),
            Some(7),
            Some(3),
            Some("epoch-1".to_string()),
        );

        store
            .commit_machine_lifecycle(
                &rid,
                MachineLifecycleCommit::new_with_binding(RuntimeState::Retired, binding.clone()),
                &[],
            )
            .await
            .unwrap();

        let lifecycle = crate::store::load_machine_lifecycle(&store, &rid)
            .await
            .unwrap()
            .expect("machine lifecycle snapshot");
        assert_eq!(lifecycle.runtime_state(), RuntimeState::Retired);
        assert_eq!(lifecycle.binding(), &binding);
        assert_eq!(
            crate::store::load_runtime_state(&store, &rid)
                .await
                .unwrap(),
            Some(RuntimeState::Retired)
        );
    }

    #[tokio::test]
    async fn clear_session_snapshot_if_current_sets_quarantine_marker_cleared_on_write() {
        let store = InMemoryRuntimeStore::new();
        let rid = LogicalRuntimeId::new("runtime-quarantine");
        let rejected = serde_json::to_vec(&session_with_user("rejected")).unwrap();

        assert!(!store.is_runtime_projection_quarantined(&rid).await.unwrap());
        store
            .commit_session_snapshot(
                &rid,
                SessionDelta {
                    session_snapshot: rejected.clone(),
                },
            )
            .await
            .unwrap();
        assert!(
            store
                .clear_session_snapshot_if_current(&rid, &rejected)
                .await
                .unwrap()
        );
        assert!(
            store.is_runtime_projection_quarantined(&rid).await.unwrap(),
            "clearing the rejected snapshot must record the in-memory quarantine marker"
        );

        // A live snapshot write reclaims runtime authority and clears the marker.
        store
            .commit_session_snapshot(
                &rid,
                SessionDelta {
                    session_snapshot: serde_json::to_vec(&session_with_user("revived")).unwrap(),
                },
            )
            .await
            .unwrap();
        assert!(
            !store.is_runtime_projection_quarantined(&rid).await.unwrap(),
            "a live snapshot write must clear the in-memory quarantine marker"
        );
    }
}
