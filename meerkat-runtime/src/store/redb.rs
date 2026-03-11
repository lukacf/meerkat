//! RedbRuntimeStore — redb-backed RuntimeStore with atomic guarantees.
//!
//! Shares `Arc<redb::Database>` with `RedbSessionStore`. A single redb
//! write transaction covers session delta + receipt + InputState updates
//! for §19 atomic persistence.

#[cfg(feature = "redb-store")]
mod inner {
    use std::sync::Arc;

    use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
    use redb::{Database, ReadableTable, TableDefinition};

    use crate::identifiers::LogicalRuntimeId;
    use crate::input_state::InputState;
    use crate::store::{RuntimeStore, RuntimeStoreError, SessionDelta};

    /// Table: (runtime_id + input_id bytes) → InputState JSON
    const INPUT_STATES: TableDefinition<&[u8], &[u8]> =
        TableDefinition::new("runtime_input_states");

    /// Table: (runtime_id + run_id + sequence bytes) → Receipt JSON
    const BOUNDARY_RECEIPTS: TableDefinition<&[u8], &[u8]> =
        TableDefinition::new("runtime_boundary_receipts");

    /// Table: runtime_id → session snapshot bytes
    const SESSION_SNAPSHOTS: TableDefinition<&[u8], &[u8]> =
        TableDefinition::new("runtime_session_snapshots");

    /// Key prefix length for runtime ID (stored as UTF-8 length-prefixed).
    fn runtime_prefix(runtime_id: &LogicalRuntimeId) -> Vec<u8> {
        let bytes = runtime_id.0.as_bytes();
        let mut prefix = Vec::with_capacity(2 + bytes.len());
        prefix.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        prefix.extend_from_slice(bytes);
        prefix
    }

    fn input_state_key(runtime_id: &LogicalRuntimeId, input_id: &InputId) -> Vec<u8> {
        let mut key = runtime_prefix(runtime_id);
        key.extend_from_slice(input_id.0.as_bytes());
        key
    }

    fn receipt_key(runtime_id: &LogicalRuntimeId, run_id: &RunId, sequence: u64) -> Vec<u8> {
        let mut key = runtime_prefix(runtime_id);
        key.extend_from_slice(run_id.0.as_bytes());
        key.extend_from_slice(&sequence.to_be_bytes());
        key
    }

    /// Redb-backed runtime store sharing the same database as RedbSessionStore.
    pub struct RedbRuntimeStore {
        db: Arc<Database>,
    }

    impl RedbRuntimeStore {
        /// Create from a shared database (obtained via `RedbSessionStore::database()`).
        pub fn new(db: Arc<Database>) -> Result<Self, RuntimeStoreError> {
            // Ensure tables exist
            let write_txn = db.begin_write().map_err(|e| {
                RuntimeStoreError::WriteFailed(format!("Failed to begin write: {e}"))
            })?;
            {
                let _ = write_txn.open_table(INPUT_STATES).map_err(|e| {
                    RuntimeStoreError::WriteFailed(format!("Failed to create table: {e}"))
                })?;
                let _ = write_txn.open_table(BOUNDARY_RECEIPTS).map_err(|e| {
                    RuntimeStoreError::WriteFailed(format!("Failed to create table: {e}"))
                })?;
                let _ = write_txn.open_table(SESSION_SNAPSHOTS).map_err(|e| {
                    RuntimeStoreError::WriteFailed(format!("Failed to create table: {e}"))
                })?;
            }
            write_txn
                .commit()
                .map_err(|e| RuntimeStoreError::WriteFailed(format!("Failed to commit: {e}")))?;

            Ok(Self { db })
        }

        /// Access the underlying database.
        pub fn database(&self) -> Arc<Database> {
            self.db.clone()
        }
    }

    #[async_trait::async_trait]
    impl RuntimeStore for RedbRuntimeStore {
        async fn atomic_apply(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: Option<SessionDelta>,
            receipt: RunBoundaryReceipt,
            input_updates: Vec<InputState>,
        ) -> Result<(), RuntimeStoreError> {
            let db = self.db.clone();
            let rid = runtime_id.clone();
            let receipt_json = serde_json::to_vec(&receipt)
                .map_err(|e| RuntimeStoreError::WriteFailed(e.to_string()))?;
            let input_jsons: Vec<(Vec<u8>, Vec<u8>)> = input_updates
                .iter()
                .map(|s| {
                    let key = input_state_key(&rid, &s.input_id);
                    let val = serde_json::to_vec(s)
                        .map_err(|e| RuntimeStoreError::WriteFailed(e.to_string()));
                    val.map(|v| (key, v))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let rkey = receipt_key(&rid, &receipt.run_id, receipt.sequence);
            let session_data = session_delta.map(|d| d.session_snapshot);
            let rid_for_session = rid.clone();

            tokio::task::spawn_blocking(move || {
                let write_txn = db.begin_write().map_err(|e| {
                    RuntimeStoreError::WriteFailed(format!("Failed to begin write: {e}"))
                })?;
                {
                    // Session snapshot
                    if let Some(data) = session_data {
                        let mut table = write_txn.open_table(SESSION_SNAPSHOTS).map_err(|e| {
                            RuntimeStoreError::WriteFailed(format!("Failed to open table: {e}"))
                        })?;
                        let skey = runtime_prefix(&rid_for_session);
                        table
                            .insert(skey.as_slice(), data.as_slice())
                            .map_err(|e| {
                                RuntimeStoreError::WriteFailed(format!("Failed to insert: {e}"))
                            })?;
                    }

                    // Receipt
                    let mut receipts_table =
                        write_txn.open_table(BOUNDARY_RECEIPTS).map_err(|e| {
                            RuntimeStoreError::WriteFailed(format!("Failed to open table: {e}"))
                        })?;
                    receipts_table
                        .insert(rkey.as_slice(), receipt_json.as_slice())
                        .map_err(|e| {
                            RuntimeStoreError::WriteFailed(format!("Failed to insert: {e}"))
                        })?;

                    // Input states
                    let mut states_table = write_txn.open_table(INPUT_STATES).map_err(|e| {
                        RuntimeStoreError::WriteFailed(format!("Failed to open table: {e}"))
                    })?;
                    for (key, val) in &input_jsons {
                        states_table
                            .insert(key.as_slice(), val.as_slice())
                            .map_err(|e| {
                                RuntimeStoreError::WriteFailed(format!("Failed to insert: {e}"))
                            })?;
                    }
                }
                write_txn.commit().map_err(|e| {
                    RuntimeStoreError::WriteFailed(format!("Failed to commit: {e}"))
                })?;
                Ok(())
            })
            .await
            .map_err(|e| RuntimeStoreError::Internal(format!("Task join failed: {e}")))?
        }

        async fn load_input_states(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Vec<InputState>, RuntimeStoreError> {
            let db = self.db.clone();
            let prefix = runtime_prefix(runtime_id);

            tokio::task::spawn_blocking(move || {
                let read_txn = db.begin_read().map_err(|e| {
                    RuntimeStoreError::ReadFailed(format!("Failed to begin read: {e}"))
                })?;
                let table = read_txn.open_table(INPUT_STATES).map_err(|e| {
                    RuntimeStoreError::ReadFailed(format!("Failed to open table: {e}"))
                })?;

                let mut states = Vec::new();
                let iter = table.iter().map_err(|e| {
                    RuntimeStoreError::ReadFailed(format!("Failed to iterate: {e}"))
                })?;
                for entry in iter {
                    let entry = entry.map_err(|e| {
                        RuntimeStoreError::ReadFailed(format!("Failed to read entry: {e}"))
                    })?;
                    let key = entry.0.value();
                    if key.starts_with(&prefix) {
                        let state: InputState =
                            serde_json::from_slice(entry.1.value()).map_err(|e| {
                                RuntimeStoreError::ReadFailed(format!("Failed to deserialize: {e}"))
                            })?;
                        states.push(state);
                    }
                }
                Ok(states)
            })
            .await
            .map_err(|e| RuntimeStoreError::Internal(format!("Task join failed: {e}")))?
        }

        async fn load_boundary_receipt(
            &self,
            runtime_id: &LogicalRuntimeId,
            run_id: &RunId,
            sequence: u64,
        ) -> Result<Option<RunBoundaryReceipt>, RuntimeStoreError> {
            let db = self.db.clone();
            let key = receipt_key(runtime_id, run_id, sequence);

            tokio::task::spawn_blocking(move || {
                let read_txn = db.begin_read().map_err(|e| {
                    RuntimeStoreError::ReadFailed(format!("Failed to begin read: {e}"))
                })?;
                let table = read_txn.open_table(BOUNDARY_RECEIPTS).map_err(|e| {
                    RuntimeStoreError::ReadFailed(format!("Failed to open table: {e}"))
                })?;
                match table.get(key.as_slice()) {
                    Ok(Some(data)) => {
                        let receipt = serde_json::from_slice(data.value()).map_err(|e| {
                            RuntimeStoreError::ReadFailed(format!("Failed to deserialize: {e}"))
                        })?;
                        Ok(Some(receipt))
                    }
                    Ok(None) => Ok(None),
                    Err(e) => Err(RuntimeStoreError::ReadFailed(format!(
                        "Failed to read: {e}"
                    ))),
                }
            })
            .await
            .map_err(|e| RuntimeStoreError::Internal(format!("Task join failed: {e}")))?
        }

        async fn persist_input_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            state: &InputState,
        ) -> Result<(), RuntimeStoreError> {
            let db = self.db.clone();
            let key = input_state_key(runtime_id, &state.input_id);
            let val = serde_json::to_vec(state)
                .map_err(|e| RuntimeStoreError::WriteFailed(e.to_string()))?;

            tokio::task::spawn_blocking(move || {
                let write_txn = db.begin_write().map_err(|e| {
                    RuntimeStoreError::WriteFailed(format!("Failed to begin write: {e}"))
                })?;
                {
                    let mut table = write_txn.open_table(INPUT_STATES).map_err(|e| {
                        RuntimeStoreError::WriteFailed(format!("Failed to open table: {e}"))
                    })?;
                    table.insert(key.as_slice(), val.as_slice()).map_err(|e| {
                        RuntimeStoreError::WriteFailed(format!("Failed to insert: {e}"))
                    })?;
                }
                write_txn.commit().map_err(|e| {
                    RuntimeStoreError::WriteFailed(format!("Failed to commit: {e}"))
                })?;
                Ok(())
            })
            .await
            .map_err(|e| RuntimeStoreError::Internal(format!("Task join failed: {e}")))?
        }

        async fn load_input_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            input_id: &InputId,
        ) -> Result<Option<InputState>, RuntimeStoreError> {
            let db = self.db.clone();
            let key = input_state_key(runtime_id, input_id);

            tokio::task::spawn_blocking(move || {
                let read_txn = db.begin_read().map_err(|e| {
                    RuntimeStoreError::ReadFailed(format!("Failed to begin read: {e}"))
                })?;
                let table = read_txn.open_table(INPUT_STATES).map_err(|e| {
                    RuntimeStoreError::ReadFailed(format!("Failed to open table: {e}"))
                })?;
                match table.get(key.as_slice()) {
                    Ok(Some(data)) => {
                        let state = serde_json::from_slice(data.value()).map_err(|e| {
                            RuntimeStoreError::ReadFailed(format!("Failed to deserialize: {e}"))
                        })?;
                        Ok(Some(state))
                    }
                    Ok(None) => Ok(None),
                    Err(e) => Err(RuntimeStoreError::ReadFailed(format!(
                        "Failed to read: {e}"
                    ))),
                }
            })
            .await
            .map_err(|e| RuntimeStoreError::Internal(format!("Task join failed: {e}")))?
        }
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used)]
    mod tests {
        use super::*;
        use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;

        fn temp_db() -> Arc<Database> {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("test.redb");
            let db = Database::create(&path).unwrap();
            // Keep dir alive by leaking it (test only)
            std::mem::forget(dir);
            Arc::new(db)
        }

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
            let db = temp_db();
            let store = RedbRuntimeStore::new(db).unwrap();
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
                    receipt,
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
        async fn persist_and_load_single() {
            let db = temp_db();
            let store = RedbRuntimeStore::new(db).unwrap();
            let rid = LogicalRuntimeId::new("test");
            let input_id = InputId::new();
            let state = InputState::new_accepted(input_id.clone());

            store.persist_input_state(&rid, &state).await.unwrap();

            let loaded = store.load_input_state(&rid, &input_id).await.unwrap();
            assert!(loaded.is_some());
            assert_eq!(loaded.unwrap().input_id, input_id);
        }

        #[tokio::test]
        async fn load_nonexistent() {
            let db = temp_db();
            let store = RedbRuntimeStore::new(db).unwrap();
            let rid = LogicalRuntimeId::new("test");

            let states = store.load_input_states(&rid).await.unwrap();
            assert!(states.is_empty());

            let receipt = store
                .load_boundary_receipt(&rid, &RunId::new(), 0)
                .await
                .unwrap();
            assert!(receipt.is_none());
        }

        #[tokio::test]
        async fn multiple_runtimes_isolated() {
            let db = temp_db();
            let store = RedbRuntimeStore::new(db).unwrap();
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
        async fn atomic_apply_all_in_one_transaction() {
            let db = temp_db();
            let store = RedbRuntimeStore::new(db.clone()).unwrap();
            let rid = LogicalRuntimeId::new("test");
            let run_id = RunId::new();

            let id1 = InputId::new();
            let id2 = InputId::new();

            store
                .atomic_apply(
                    &rid,
                    None,
                    make_receipt(run_id.clone(), 0),
                    vec![
                        InputState::new_accepted(id1.clone()),
                        InputState::new_accepted(id2.clone()),
                    ],
                )
                .await
                .unwrap();

            // Both states should be visible
            let states = store.load_input_states(&rid).await.unwrap();
            assert_eq!(states.len(), 2);

            // Receipt should be visible
            let receipt = store.load_boundary_receipt(&rid, &run_id, 0).await.unwrap();
            assert!(receipt.is_some());
        }
    }
}

#[cfg(feature = "redb-store")]
pub use inner::RedbRuntimeStore;
