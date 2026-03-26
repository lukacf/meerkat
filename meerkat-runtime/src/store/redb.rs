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
    use crate::runtime_state::RuntimeState;
    use crate::store::{RuntimeStore, RuntimeStoreError, SessionDelta, authoritative_receipt};
    use meerkat_store::redb_store::write_session_snapshot_in_txn;

    /// Table: (runtime_id + input_id bytes) → InputState JSON
    const INPUT_STATES: TableDefinition<&[u8], &[u8]> =
        TableDefinition::new("runtime_input_states");

    /// Table: (runtime_id + run_id + sequence bytes) → Receipt JSON
    const BOUNDARY_RECEIPTS: TableDefinition<&[u8], &[u8]> =
        TableDefinition::new("runtime_boundary_receipts");
    /// Table: runtime_id → serialized session snapshot JSON.
    const SESSION_SNAPSHOTS: TableDefinition<&[u8], &[u8]> =
        TableDefinition::new("runtime_session_snapshots");
    /// Table: runtime_id → RuntimeState JSON
    const RUNTIME_STATES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("runtime_states");

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
                let _ = write_txn.open_table(RUNTIME_STATES).map_err(|e| {
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
        async fn commit_session_boundary(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: SessionDelta,
            run_id: RunId,
            boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
            contributing_input_ids: Vec<InputId>,
            input_updates: Vec<InputState>,
        ) -> Result<RunBoundaryReceipt, RuntimeStoreError> {
            let db = self.db.clone();
            let rid = runtime_id.clone();
            let snapshot_bytes = session_delta.session_snapshot.clone();
            let session_snapshot =
                serde_json::from_slice::<meerkat_core::Session>(&session_delta.session_snapshot)
                    .map_err(|e| RuntimeStoreError::WriteFailed(e.to_string()))?;
            tokio::task::spawn_blocking(move || {
                let write_txn = db.begin_write().map_err(|e| {
                    RuntimeStoreError::WriteFailed(format!("Failed to begin write: {e}"))
                })?;
                let receipt = {
                    let mut max_sequence = None;
                    {
                        let receipts_table =
                            write_txn.open_table(BOUNDARY_RECEIPTS).map_err(|e| {
                                RuntimeStoreError::WriteFailed(format!("Failed to open table: {e}"))
                            })?;
                        let iter = receipts_table.iter().map_err(|e| {
                            RuntimeStoreError::WriteFailed(format!("Failed to iterate: {e}"))
                        })?;
                        let receipt_prefix = {
                            let mut prefix = runtime_prefix(&rid);
                            prefix.extend_from_slice(run_id.0.as_bytes());
                            prefix
                        };
                        for row in iter {
                            let (key, _) = row.map_err(|e| {
                                RuntimeStoreError::WriteFailed(format!("Failed to read row: {e}"))
                            })?;
                            let key_bytes = key.value();
                            if key_bytes.starts_with(&receipt_prefix) && key_bytes.len() >= 8 {
                                let seq_offset = key_bytes.len() - 8;
                                let mut seq_bytes = [0_u8; 8];
                                seq_bytes.copy_from_slice(&key_bytes[seq_offset..]);
                                let seq = u64::from_be_bytes(seq_bytes);
                                max_sequence =
                                    Some(max_sequence.map_or(seq, |cur: u64| cur.max(seq)));
                            }
                        }
                    }

                    authoritative_receipt(
                        Some(&session_delta),
                        run_id,
                        boundary,
                        contributing_input_ids,
                        max_sequence.map(|seq| seq + 1).unwrap_or(0),
                    )?
                };
                let mut input_updates = input_updates;
                for state in &mut input_updates {
                    state
                        .authority_mut()
                        .stamp_receipt_metadata(receipt.run_id.clone(), receipt.sequence);
                }
                let input_jsons: Vec<(Vec<u8>, Vec<u8>)> = input_updates
                    .iter()
                    .map(|s| {
                        let key = input_state_key(&rid, &s.input_id);
                        let val = serde_json::to_vec(s)
                            .map_err(|e| RuntimeStoreError::WriteFailed(e.to_string()));
                        val.map(|v| (key, v))
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                {
                    write_session_snapshot_in_txn(&write_txn, &session_snapshot)
                        .map_err(|e| RuntimeStoreError::WriteFailed(e.to_string()))?;
                    let mut snapshots_table =
                        write_txn.open_table(SESSION_SNAPSHOTS).map_err(|e| {
                            RuntimeStoreError::WriteFailed(format!("Failed to open table: {e}"))
                        })?;
                    let snapshot_key = runtime_prefix(&rid);
                    snapshots_table
                        .insert(snapshot_key.as_slice(), snapshot_bytes.as_slice())
                        .map_err(|e| {
                            RuntimeStoreError::WriteFailed(format!("Failed to insert: {e}"))
                        })?;

                    let receipt_json = serde_json::to_vec(&receipt)
                        .map_err(|e| RuntimeStoreError::WriteFailed(e.to_string()))?;
                    let rkey = receipt_key(&rid, &receipt.run_id, receipt.sequence);
                    let mut receipts_table =
                        write_txn.open_table(BOUNDARY_RECEIPTS).map_err(|e| {
                            RuntimeStoreError::WriteFailed(format!("Failed to open table: {e}"))
                        })?;
                    receipts_table
                        .insert(rkey.as_slice(), receipt_json.as_slice())
                        .map_err(|e| {
                            RuntimeStoreError::WriteFailed(format!("Failed to insert: {e}"))
                        })?;

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
                Ok(receipt)
            })
            .await
            .map_err(|e| RuntimeStoreError::Internal(format!("Task join failed: {e}")))?
        }

        async fn atomic_apply(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: Option<SessionDelta>,
            receipt: RunBoundaryReceipt,
            input_updates: Vec<InputState>,
            _session_store_key: Option<meerkat_core::types::SessionId>,
        ) -> Result<(), RuntimeStoreError> {
            let db = self.db.clone();
            let rid = runtime_id.clone();
            let receipt_json = serde_json::to_vec(&receipt)
                .map_err(|e| RuntimeStoreError::WriteFailed(e.to_string()))?;
            let session_snapshot_bytes = session_delta.as_ref().map(|d| d.session_snapshot.clone());
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
            let session_snapshot = session_delta
                .map(|d| {
                    serde_json::from_slice::<meerkat_core::Session>(&d.session_snapshot)
                        .map_err(|e| RuntimeStoreError::WriteFailed(e.to_string()))
                })
                .transpose()?;

            tokio::task::spawn_blocking(move || {
                let write_txn = db.begin_write().map_err(|e| {
                    RuntimeStoreError::WriteFailed(format!("Failed to begin write: {e}"))
                })?;
                {
                    if let Some(session) = session_snapshot.as_ref() {
                        write_session_snapshot_in_txn(&write_txn, session)
                            .map_err(|e| RuntimeStoreError::WriteFailed(e.to_string()))?;
                        if let Some(snapshot_bytes) = session_snapshot_bytes.as_ref() {
                            let mut snapshots_table =
                                write_txn.open_table(SESSION_SNAPSHOTS).map_err(|e| {
                                    RuntimeStoreError::WriteFailed(format!(
                                        "Failed to open table: {e}"
                                    ))
                                })?;
                            let snapshot_key = runtime_prefix(&rid);
                            snapshots_table
                                .insert(snapshot_key.as_slice(), snapshot_bytes.as_slice())
                                .map_err(|e| {
                                    RuntimeStoreError::WriteFailed(format!("Failed to insert: {e}"))
                                })?;
                        }
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

        async fn load_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
            let db = self.db.clone();
            let key = runtime_prefix(runtime_id);

            tokio::task::spawn_blocking(move || {
                let read_txn = db.begin_read().map_err(|e| {
                    RuntimeStoreError::ReadFailed(format!("Failed to begin read: {e}"))
                })?;
                let table = read_txn.open_table(SESSION_SNAPSHOTS).map_err(|e| {
                    RuntimeStoreError::ReadFailed(format!("Failed to open table: {e}"))
                })?;
                match table.get(key.as_slice()) {
                    Ok(Some(data)) => Ok(Some(data.value().to_vec())),
                    Ok(None) => Ok(None),
                    Err(e) => Err(RuntimeStoreError::ReadFailed(format!(
                        "Failed to read session snapshot: {e}"
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

        async fn persist_runtime_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            state: RuntimeState,
        ) -> Result<(), RuntimeStoreError> {
            let db = self.db.clone();
            let key = runtime_prefix(runtime_id);
            let value = serde_json::to_vec(&state)
                .map_err(|e| RuntimeStoreError::WriteFailed(e.to_string()))?;

            tokio::task::spawn_blocking(move || {
                let write_txn = db.begin_write().map_err(|e| {
                    RuntimeStoreError::WriteFailed(format!("Failed to begin write: {e}"))
                })?;
                {
                    let mut table = write_txn.open_table(RUNTIME_STATES).map_err(|e| {
                        RuntimeStoreError::WriteFailed(format!("Failed to open table: {e}"))
                    })?;
                    table
                        .insert(key.as_slice(), value.as_slice())
                        .map_err(|e| {
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

        async fn load_runtime_state(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<RuntimeState>, RuntimeStoreError> {
            let db = self.db.clone();
            let key = runtime_prefix(runtime_id);

            tokio::task::spawn_blocking(move || {
                let read_txn = db.begin_read().map_err(|e| {
                    RuntimeStoreError::ReadFailed(format!("Failed to begin read: {e}"))
                })?;
                let table = read_txn.open_table(RUNTIME_STATES).map_err(|e| {
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
                        "Failed to get runtime state: {e}"
                    ))),
                }
            })
            .await
            .map_err(|e| RuntimeStoreError::Internal(format!("Task join failed: {e}")))?
        }

        async fn atomic_lifecycle_commit(
            &self,
            runtime_id: &LogicalRuntimeId,
            runtime_state: RuntimeState,
            input_states: &[InputState],
        ) -> Result<(), RuntimeStoreError> {
            let db = self.db.clone();
            let rid = runtime_id.clone();
            let runtime_key = runtime_prefix(&rid);
            let runtime_value = serde_json::to_vec(&runtime_state)
                .map_err(|e| RuntimeStoreError::WriteFailed(e.to_string()))?;
            let input_jsons: Vec<(Vec<u8>, Vec<u8>)> = input_states
                .iter()
                .map(|s| {
                    let key = input_state_key(&rid, &s.input_id);
                    let val = serde_json::to_vec(s)
                        .map_err(|e| RuntimeStoreError::WriteFailed(e.to_string()));
                    val.map(|v| (key, v))
                })
                .collect::<Result<Vec<_>, _>>()?;

            tokio::task::spawn_blocking(move || {
                let write_txn = db.begin_write().map_err(|e| {
                    RuntimeStoreError::WriteFailed(format!("Failed to begin write: {e}"))
                })?;
                {
                    // Write runtime state
                    let mut rt_table = write_txn.open_table(RUNTIME_STATES).map_err(|e| {
                        RuntimeStoreError::WriteFailed(format!("Failed to open table: {e}"))
                    })?;
                    rt_table
                        .insert(runtime_key.as_slice(), runtime_value.as_slice())
                        .map_err(|e| {
                            RuntimeStoreError::WriteFailed(format!("Failed to insert: {e}"))
                        })?;

                    // Write input states
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
                // Single atomic commit
                write_txn.commit().map_err(|e| {
                    RuntimeStoreError::WriteFailed(format!("Failed to commit: {e}"))
                })?;
                Ok(())
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
            let session_snapshot = serde_json::to_vec(&meerkat_core::Session::new()).unwrap();

            store
                .atomic_apply(
                    &rid,
                    Some(SessionDelta { session_snapshot }),
                    receipt,
                    vec![state],
                    None,
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
                    None,
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

        #[tokio::test]
        async fn commit_session_boundary_returns_authoritative_receipt() {
            let db = temp_db();
            let store = RedbRuntimeStore::new(db).unwrap();
            let rid = LogicalRuntimeId::new("test");
            let run_id = RunId::new();
            let input_id = InputId::new();
            let session_snapshot = serde_json::to_vec(&meerkat_core::Session::new()).unwrap();

            let receipt = store
                .commit_session_boundary(
                    &rid,
                    SessionDelta { session_snapshot },
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
        async fn load_session_snapshot_roundtrip() {
            let db = temp_db();
            let store = RedbRuntimeStore::new(db).unwrap();
            let rid = LogicalRuntimeId::new("test");
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
}

#[cfg(feature = "redb-store")]
pub use inner::RedbRuntimeStore;
