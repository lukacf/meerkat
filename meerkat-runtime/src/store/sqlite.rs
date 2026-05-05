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
    use crate::input_state::StoredInputState;
    use crate::runtime_state::RuntimeState;
    use crate::store::{
        AuthOAuthFlowSnapshotUpdate, MachineLifecycleCommit, RuntimeStore, RuntimeStoreError,
        SessionDelta,
    };

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
);
CREATE TABLE IF NOT EXISTS runtime_ops_lifecycle (
    runtime_id TEXT PRIMARY KEY,
    state_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS runtime_auth_oauth_flow_state (
    id TEXT PRIMARY KEY,
    state_json BLOB NOT NULL
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

    const AUTH_OAUTH_FLOW_STATE_ID: &str = "auth_oauth_flow_state";

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
        input_states: &[StoredInputState],
    ) -> Result<(), RuntimeStoreError> {
        for bundle in input_states {
            let state_json = serde_json::to_vec(bundle)
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
            tx.execute(
                r"
                INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(runtime_id, input_id) DO UPDATE SET state_json = excluded.state_json
                ",
                params![
                    runtime_id_text(runtime_id),
                    bundle.state.input_id.0.to_string(),
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
        fn auth_authority_key(&self) -> Option<String> {
            let path = std::fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone());
            Some(format!("sqlite:{}", path.display()))
        }

        fn persist_auth_oauth_flow_snapshot(
            &self,
            snapshot_json: &[u8],
        ) -> Result<(), RuntimeStoreError> {
            let mut conn = open_runtime_connection(&self.path)?;
            let tx = begin_runtime_transaction(&mut conn)?;
            tx.execute(
                r"
                INSERT INTO runtime_auth_oauth_flow_state (id, state_json)
                VALUES (?1, ?2)
                ON CONFLICT(id) DO UPDATE SET state_json = excluded.state_json
                ",
                params![AUTH_OAUTH_FLOW_STATE_ID, snapshot_json],
            )
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
            tx.commit()
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
            Ok(())
        }

        fn load_auth_oauth_flow_snapshot(&self) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
            let conn = open_runtime_connection(&self.path)?;
            conn.query_row(
                r"
                SELECT state_json
                FROM runtime_auth_oauth_flow_state
                WHERE id = ?1
                ",
                params![AUTH_OAUTH_FLOW_STATE_ID],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
        }

        fn update_auth_oauth_flow_snapshot(
            &self,
            update: &mut AuthOAuthFlowSnapshotUpdate<'_>,
        ) -> Result<(), RuntimeStoreError> {
            let mut conn = open_runtime_connection(&self.path)?;
            let tx = begin_runtime_transaction(&mut conn)?;
            let current = tx
                .query_row(
                    r"
                    SELECT state_json
                    FROM runtime_auth_oauth_flow_state
                    WHERE id = ?1
                    ",
                    params![AUTH_OAUTH_FLOW_STATE_ID],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional()
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
            let next = update(current.as_deref())?;
            tx.execute(
                r"
                INSERT INTO runtime_auth_oauth_flow_state (id, state_json)
                VALUES (?1, ?2)
                ON CONFLICT(id) DO UPDATE SET state_json = excluded.state_json
                ",
                params![AUTH_OAUTH_FLOW_STATE_ID, next],
            )
            .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
            tx.commit()
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
            Ok(())
        }

        async fn commit_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: SessionDelta,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let session = serde_json::from_slice::<meerkat_core::Session>(
                    &session_delta.session_snapshot,
                )
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                write_session_snapshot_in_txn(&tx, &session)
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                upsert_runtime_snapshot(&tx, &runtime_id, &session_delta.session_snapshot)?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn atomic_apply(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: Option<SessionDelta>,
            receipt: RunBoundaryReceipt,
            input_updates: Vec<StoredInputState>,
            session_store_key: Option<meerkat_core::types::SessionId>,
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
                if let (Some(session), Some(session_store_key)) =
                    (session_snapshot.as_ref(), session_store_key.as_ref())
                    && session.id() != session_store_key
                {
                    return Err(RuntimeStoreError::SessionKeyMismatch {
                        expected: session_store_key.clone(),
                        actual: session.id().clone(),
                    });
                }

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
        ) -> Result<Vec<StoredInputState>, RuntimeStoreError> {
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
            state: &StoredInputState,
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
        ) -> Result<Option<StoredInputState>, RuntimeStoreError> {
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

        async fn commit_machine_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
            commit: MachineLifecycleCommit,
            input_states: &[StoredInputState],
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let runtime_state = commit.runtime_state();
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

        async fn persist_ops_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
            snapshot: &crate::ops_lifecycle::PersistedOpsSnapshot,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let snapshot = snapshot.clone();
            tokio::task::spawn_blocking(move || {
                let state_json = serde_json::to_vec(&snapshot)
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                tx.execute(
                    r"
                    INSERT INTO runtime_ops_lifecycle (runtime_id, state_json)
                    VALUES (?1, ?2)
                    ON CONFLICT(runtime_id) DO UPDATE SET state_json = excluded.state_json
                    ",
                    params![runtime_id_text(&runtime_id), state_json],
                )
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_ops_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<crate::ops_lifecycle::PersistedOpsSnapshot>, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let conn = open_runtime_connection(&path)?;
                conn.query_row(
                    "SELECT state_json FROM runtime_ops_lifecycle WHERE runtime_id = ?1",
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
    }

    #[cfg(test)]
    #[allow(clippy::expect_used, clippy::unwrap_used)]
    mod tests {
        use tempfile::TempDir;

        use super::*;
        use crate::identifiers::LogicalRuntimeId;
        use crate::input_state::StoredInputState;
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

        fn input_state() -> StoredInputState {
            StoredInputState::new_accepted(InputId::new())
        }

        fn receipt_row_count(store: &SqliteRuntimeStore) -> usize {
            let conn = open_runtime_connection(store.path()).unwrap();
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM runtime_boundary_receipts",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            usize::try_from(count).unwrap()
        }

        #[tokio::test]
        async fn atomic_apply_roundtrip() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let session = serde_json::to_vec(&meerkat_core::Session::new()).unwrap();
            let receipt = RunBoundaryReceipt {
                run_id: RunId(uuid::Uuid::new_v4()),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![],
                conversation_digest: Some("machine-owned-digest".to_string()),
                message_count: 42,
                sequence: 5,
            };
            store
                .atomic_apply(
                    &runtime_id,
                    Some(SessionDelta {
                        session_snapshot: session.clone(),
                    }),
                    receipt.clone(),
                    vec![input_state()],
                    None,
                )
                .await
                .unwrap();

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
        async fn commit_session_snapshot_does_not_write_boundary_receipt() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let session = serde_json::to_vec(&meerkat_core::Session::new()).unwrap();

            store
                .commit_session_snapshot(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: session,
                    },
                )
                .await
                .unwrap();

            assert!(
                store
                    .load_session_snapshot(&runtime_id)
                    .await
                    .unwrap()
                    .is_some()
            );
            assert_eq!(receipt_row_count(&store), 0);
            assert!(
                store
                    .load_input_states(&runtime_id)
                    .await
                    .unwrap()
                    .is_empty()
            );
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
        async fn atomic_apply_rejects_mismatched_session_store_key() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let session = meerkat_core::Session::new();
            let wrong_session_id = meerkat_core::Session::new().id().clone();
            let snapshot = serde_json::to_vec(&session).unwrap();

            let err = store
                .atomic_apply(
                    &runtime_id,
                    Some(SessionDelta {
                        session_snapshot: snapshot,
                    }),
                    RunBoundaryReceipt {
                        run_id: RunId(uuid::Uuid::new_v4()),
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: vec![],
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    vec![input_state()],
                    Some(wrong_session_id),
                )
                .await
                .expect_err("mismatched session_store_key should fail");

            assert!(matches!(err, RuntimeStoreError::SessionKeyMismatch { .. }));
            assert!(
                store
                    .load_session_snapshot(&runtime_id)
                    .await
                    .unwrap()
                    .is_none()
            );
        }

        #[tokio::test]
        async fn commit_machine_lifecycle_persists_both_parts() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let runtime_state = RuntimeState::Stopped;
            store
                .commit_machine_lifecycle(
                    &runtime_id,
                    MachineLifecycleCommit::new(runtime_state),
                    &[input_state()],
                )
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
