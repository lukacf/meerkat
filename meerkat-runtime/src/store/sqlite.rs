//! SQLite-backed RuntimeStore with atomic cross-table commits.

#[cfg(feature = "sqlite-store")]
mod inner {
    use std::path::{Path, PathBuf};

    use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
    use meerkat_store::sqlite_store::{
        begin_immediate_transaction, open_connection, write_session_snapshot_in_txn,
    };
    use rusqlite::{Connection, OptionalExtension, Transaction, params};

    use crate::identifiers::LogicalRuntimeId;
    use crate::input_state::InputState;
    use crate::runtime_state::RuntimeState;
    use crate::store::{RuntimeStore, RuntimeStoreError, SessionDelta, authoritative_receipt};

    const CREATE_RUNTIME_SCHEMA_SQL: &str = r"
CREATE TABLE IF NOT EXISTS runtime_input_states (
    runtime_id TEXT NOT NULL,
    input_id TEXT NOT NULL,
    state_json BLOB NOT NULL,
    PRIMARY KEY (runtime_id, input_id)
);
CREATE TABLE IF NOT EXISTS runtime_boundary_receipts (
    runtime_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    receipt_json BLOB NOT NULL,
    PRIMARY KEY (runtime_id, run_id, sequence)
);
CREATE TABLE IF NOT EXISTS runtime_session_snapshots (
    runtime_id TEXT PRIMARY KEY,
    session_snapshot BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS runtime_states (
    runtime_id TEXT PRIMARY KEY,
    runtime_state_json BLOB NOT NULL
)";

    fn ensure_runtime_schema(conn: &Connection) -> Result<(), RuntimeStoreError> {
        conn.execute_batch(CREATE_RUNTIME_SCHEMA_SQL)
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))
    }

    fn open_runtime_connection(path: &Path) -> Result<Connection, RuntimeStoreError> {
        let conn =
            open_connection(path).map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        ensure_runtime_schema(&conn)?;
        Ok(conn)
    }

    fn begin_runtime_transaction(
        conn: &mut Connection,
    ) -> Result<Transaction<'_>, RuntimeStoreError> {
        begin_immediate_transaction(conn)
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))
    }

    fn runtime_id_text(runtime_id: &LogicalRuntimeId) -> &str {
        &runtime_id.0
    }

    fn next_receipt_sequence(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
    ) -> Result<u64, RuntimeStoreError> {
        let next: i64 = tx
            .query_row(
                r"
                SELECT COALESCE(MAX(sequence), -1) + 1
                FROM runtime_boundary_receipts
                WHERE runtime_id = ?1 AND run_id = ?2
                ",
                params![runtime_id_text(runtime_id), run_id.0.to_string()],
                |row| row.get(0),
            )
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        u64::try_from(next).map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))
    }

    fn upsert_runtime_snapshot(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        snapshot: &[u8],
    ) -> Result<(), RuntimeStoreError> {
        tx.execute(
            r"
            INSERT INTO runtime_session_snapshots (runtime_id, session_snapshot)
            VALUES (?1, ?2)
            ON CONFLICT(runtime_id) DO UPDATE SET session_snapshot = excluded.session_snapshot
            ",
            params![runtime_id_text(runtime_id), snapshot],
        )
        .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        Ok(())
    }

    fn insert_receipt(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        receipt: &RunBoundaryReceipt,
    ) -> Result<(), RuntimeStoreError> {
        let receipt_json = serde_json::to_vec(receipt)
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        tx.execute(
            r"
            INSERT INTO runtime_boundary_receipts (runtime_id, run_id, sequence, receipt_json)
            VALUES (?1, ?2, ?3, ?4)
            ",
            params![
                runtime_id_text(runtime_id),
                receipt.run_id.0.to_string(),
                i64::try_from(receipt.sequence).unwrap_or(i64::MAX),
                receipt_json,
            ],
        )
        .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        Ok(())
    }

    fn upsert_input_states(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        input_states: &[InputState],
    ) -> Result<(), RuntimeStoreError> {
        for state in input_states {
            let state_json = serde_json::to_vec(state)
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
            tx.execute(
                r"
                INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(runtime_id, input_id) DO UPDATE SET state_json = excluded.state_json
                ",
                params![
                    runtime_id_text(runtime_id),
                    state.input_id.0.to_string(),
                    state_json
                ],
            )
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        }
        Ok(())
    }

    fn upsert_runtime_state(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        runtime_state: &RuntimeState,
    ) -> Result<(), RuntimeStoreError> {
        let state_json = serde_json::to_vec(runtime_state)
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        tx.execute(
            r"
            INSERT INTO runtime_states (runtime_id, runtime_state_json)
            VALUES (?1, ?2)
            ON CONFLICT(runtime_id) DO UPDATE SET runtime_state_json = excluded.runtime_state_json
            ",
            params![runtime_id_text(runtime_id), state_json],
        )
        .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
        Ok(())
    }

    /// SQLite-backed runtime store sharing the same sqlite file as `SqliteSessionStore`.
    pub struct SqliteRuntimeStore {
        path: PathBuf,
    }

    impl SqliteRuntimeStore {
        pub fn new(path: impl Into<PathBuf>) -> Result<Self, RuntimeStoreError> {
            let path = path.into();
            let conn = open_runtime_connection(&path)?;
            drop(conn);
            Ok(Self { path })
        }

        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    #[async_trait::async_trait]
    impl RuntimeStore for SqliteRuntimeStore {
        async fn commit_session_boundary(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: SessionDelta,
            run_id: RunId,
            boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
            contributing_input_ids: Vec<InputId>,
            input_updates: Vec<InputState>,
        ) -> Result<RunBoundaryReceipt, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let session_snapshot_bytes = session_delta.session_snapshot.clone();
            tokio::task::spawn_blocking(move || {
                let session_snapshot = serde_json::from_slice::<meerkat_core::Session>(
                    &session_delta.session_snapshot,
                )
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let sequence = next_receipt_sequence(&tx, &runtime_id, &run_id)?;
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

                write_session_snapshot_in_txn(&tx, &session_snapshot)
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                upsert_runtime_snapshot(&tx, &runtime_id, &session_snapshot_bytes)?;
                insert_receipt(&tx, &runtime_id, &receipt)?;
                upsert_input_states(&tx, &runtime_id, &input_updates)?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(receipt)
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn atomic_apply(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: Option<SessionDelta>,
            receipt: RunBoundaryReceipt,
            input_updates: Vec<InputState>,
            _session_store_key: Option<meerkat_core::types::SessionId>,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let session_snapshot = session_delta
                    .as_ref()
                    .map(|delta| {
                        serde_json::from_slice::<meerkat_core::Session>(&delta.session_snapshot)
                            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))
                    })
                    .transpose()?;

                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;

                if let Some(session) = session_snapshot.as_ref() {
                    write_session_snapshot_in_txn(&tx, session)
                        .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                    if let Some(delta) = session_delta.as_ref() {
                        upsert_runtime_snapshot(&tx, &runtime_id, &delta.session_snapshot)?;
                    }
                }

                insert_receipt(&tx, &runtime_id, &receipt)?;
                upsert_input_states(&tx, &runtime_id, &input_updates)?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_input_states(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Vec<InputState>, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                let mut stmt = conn
                    .prepare(
                        r"
                        SELECT state_json
                        FROM runtime_input_states
                        WHERE runtime_id = ?1
                        ORDER BY input_id ASC
                        ",
                    )
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                let rows = stmt
                    .query_map(params![runtime_id_text(&runtime_id)], |row| {
                        row.get::<_, Vec<u8>>(0)
                    })
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                rows.map(|row| {
                    let bytes =
                        row.map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                    serde_json::from_slice(&bytes)
                        .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
                })
                .collect()
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_boundary_receipt(
            &self,
            runtime_id: &LogicalRuntimeId,
            run_id: &RunId,
            sequence: u64,
        ) -> Result<Option<RunBoundaryReceipt>, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let run_id = run_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                conn.query_row(
                    r"
                    SELECT receipt_json
                    FROM runtime_boundary_receipts
                    WHERE runtime_id = ?1 AND run_id = ?2 AND sequence = ?3
                    ",
                    params![
                        runtime_id_text(&runtime_id),
                        run_id.0.to_string(),
                        i64::try_from(sequence).unwrap_or(i64::MAX)
                    ],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional()
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?
                .map(|bytes| {
                    serde_json::from_slice(&bytes)
                        .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
                })
                .transpose()
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                conn.query_row(
                    "SELECT session_snapshot FROM runtime_session_snapshots WHERE runtime_id = ?1",
                    params![runtime_id_text(&runtime_id)],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional()
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn persist_input_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            state: &InputState,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let state = state.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                upsert_input_states(&tx, &runtime_id, &[state])?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_input_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            input_id: &InputId,
        ) -> Result<Option<InputState>, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let input_id = input_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                conn.query_row(
                    r"
                    SELECT state_json
                    FROM runtime_input_states
                    WHERE runtime_id = ?1 AND input_id = ?2
                    ",
                    params![runtime_id_text(&runtime_id), input_id.0.to_string()],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional()
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?
                .map(|bytes| {
                    serde_json::from_slice(&bytes)
                        .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
                })
                .transpose()
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn persist_runtime_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            state: RuntimeState,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                upsert_runtime_state(&tx, &runtime_id, &state)?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_runtime_state(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<RuntimeState>, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                conn.query_row(
                    "SELECT runtime_state_json FROM runtime_states WHERE runtime_id = ?1",
                    params![runtime_id_text(&runtime_id)],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional()
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?
                .map(|bytes| {
                    serde_json::from_slice(&bytes)
                        .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
                })
                .transpose()
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn atomic_lifecycle_commit(
            &self,
            runtime_id: &LogicalRuntimeId,
            runtime_state: RuntimeState,
            input_states: &[InputState],
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let input_states = input_states.to_vec();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                upsert_runtime_state(&tx, &runtime_id, &runtime_state)?;
                upsert_input_states(&tx, &runtime_id, &input_states)?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }
    }

    #[cfg(test)]
    #[allow(clippy::expect_used, clippy::unwrap_used)]
    mod tests {
        use tempfile::TempDir;

        use super::*;
        use crate::identifiers::LogicalRuntimeId;
        use crate::input_state::InputState;
        use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
        use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};

        fn temp_store() -> (TempDir, SqliteRuntimeStore) {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("sessions.sqlite3");
            let store = SqliteRuntimeStore::new(path).unwrap();
            (dir, store)
        }

        fn runtime_id() -> LogicalRuntimeId {
            LogicalRuntimeId("runtime-1".to_string())
        }

        fn input_state() -> InputState {
            InputState::new_accepted(InputId::new())
        }

        #[tokio::test]
        async fn commit_session_boundary_roundtrip() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let session = serde_json::to_vec(&meerkat_core::Session::new()).unwrap();
            let receipt = store
                .commit_session_boundary(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: session.clone(),
                    },
                    RunId(uuid::Uuid::new_v4()),
                    RunApplyBoundary::RunStart,
                    vec![],
                    vec![input_state()],
                )
                .await
                .unwrap();

            assert_eq!(receipt.sequence, 0);
            assert!(
                store
                    .load_session_snapshot(&runtime_id)
                    .await
                    .unwrap()
                    .is_some()
            );
            assert_eq!(store.load_input_states(&runtime_id).await.unwrap().len(), 1);
        }

        #[tokio::test]
        async fn atomic_apply_is_atomic_on_receipt_conflict() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let receipt = RunBoundaryReceipt {
                run_id: RunId(uuid::Uuid::new_v4()),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![],
                conversation_digest: None,
                message_count: 0,
                sequence: 0,
            };

            store
                .atomic_apply(
                    &runtime_id,
                    None,
                    receipt.clone(),
                    vec![input_state()],
                    None,
                )
                .await
                .unwrap();

            let session = serde_json::to_vec(&meerkat_core::Session::new()).unwrap();
            let err = store
                .atomic_apply(
                    &runtime_id,
                    Some(SessionDelta {
                        session_snapshot: session,
                    }),
                    receipt,
                    vec![input_state()],
                    None,
                )
                .await
                .expect_err("duplicate receipt should fail");

            assert!(matches!(err, RuntimeStoreError::WriteFailed(_)));
            assert!(
                store
                    .load_session_snapshot(&runtime_id)
                    .await
                    .unwrap()
                    .is_none()
            );
            let states = store.load_input_states(&runtime_id).await.unwrap();
            assert_eq!(states.len(), 1);
        }

        #[tokio::test]
        async fn atomic_lifecycle_commit_persists_both_parts() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let runtime_state = RuntimeState::Stopped;
            store
                .atomic_lifecycle_commit(&runtime_id, runtime_state, &[input_state()])
                .await
                .unwrap();

            assert!(
                store
                    .load_runtime_state(&runtime_id)
                    .await
                    .unwrap()
                    .is_some()
            );
            assert_eq!(store.load_input_states(&runtime_id).await.unwrap().len(), 1);
        }
    }
}

#[cfg(feature = "sqlite-store")]
pub use inner::SqliteRuntimeStore;
