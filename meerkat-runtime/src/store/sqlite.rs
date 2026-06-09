//! SQLite-backed RuntimeStore with atomic cross-table commits.

#[cfg(feature = "sqlite-store")]
mod inner {
    use std::path::{Path, PathBuf};

    use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
    use meerkat_store::sqlite_store::{begin_immediate_transaction, open_connection};
    use rusqlite::{Connection, OptionalExtension, Transaction, params};

    use crate::identifiers::LogicalRuntimeId;
    use crate::input_state::{InputStatePersistenceRecord, StoredInputState};
    use crate::store::{
        AuthOAuthFlowSnapshotUpdate, MachineLifecycleCommit, MachineLifecycleSnapshot,
        MachineLifecycleStoreRecord, RuntimeStore, RuntimeStoreError, SessionDelta,
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

    /// Deserialize a persisted session-snapshot row through typed serde.
    /// `Session::deserialize` validates the mandatory envelope version against
    /// the generated persistence version authority, so a missing or
    /// non-current (v0/v1) row fails closed here instead of silently
    /// defaulting or upgrading on read.
    fn deserialize_persisted_session(
        bytes: &[u8],
    ) -> Result<meerkat_core::Session, RuntimeStoreError> {
        serde_json::from_slice(bytes).map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
    }

    /// Deserialize a persisted `StoredInputState` row through typed serde.
    /// `StoredInputState::deserialize` validates the mandatory
    /// `stored_input_state_version` byte against the generated persistence
    /// version authority, so a missing or non-current row fails closed.
    fn deserialize_persisted_input_state(
        bytes: &[u8],
    ) -> Result<StoredInputState, RuntimeStoreError> {
        serde_json::from_slice(bytes).map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
    }

    /// Encode a `u64` boundary-receipt sequence into the durable `INTEGER`
    /// column as the reinterpreted two's-complement `i64` bit pattern.
    ///
    /// This is a total, injective bijection over the full `u64` domain: every
    /// distinct sequence maps to a distinct stored key (values above
    /// `i64::MAX` wrap into the negative `i64` range rather than saturating to
    /// a single alias). The `sequence` column participates only in exact-match
    /// equality lookups against a composite primary key — never in ordered
    /// range scans — so the lack of monotonic ORDER BY ordering for the
    /// wrapped high half is irrelevant to identity. Decode symmetrically with
    /// [`decode_receipt_sequence`].
    fn encode_receipt_sequence(sequence: u64) -> i64 {
        i64::from_ne_bytes(sequence.to_ne_bytes())
    }

    /// Inverse of [`encode_receipt_sequence`]: reinterpret the stored `i64`
    /// bit pattern back into the original `u64` sequence.
    #[cfg(test)]
    fn decode_receipt_sequence(stored: i64) -> u64 {
        u64::from_ne_bytes(stored.to_ne_bytes())
    }

    fn is_runtime_placeholder_session(session: &meerkat_core::Session) -> bool {
        session.transcript_history_state().ok().flatten().is_none()
            && matches!(
                session.messages(),
                [] | [meerkat_core::types::Message::System(_)]
            )
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
                encode_receipt_sequence(receipt.sequence),
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

    fn upsert_machine_lifecycle_snapshot(
        tx: &Transaction<'_>,
        runtime_id: &LogicalRuntimeId,
        snapshot: &MachineLifecycleSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        let state_json = MachineLifecycleStoreRecord::from_snapshot(snapshot).encode()?;
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
                let incoming =
                    serde_json::from_slice(&session_delta.session_snapshot)
                        .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let previous = tx
                    .query_row(
                        "SELECT session_snapshot FROM runtime_session_snapshots WHERE runtime_id = ?1",
                        params![runtime_id_text(&runtime_id)],
                        |row| row.get::<_, Vec<u8>>(0),
                    )
                    .optional()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?
                    .map(|bytes| deserialize_persisted_session(&bytes))
                    .transpose()?;
                meerkat_core::session_store::run_boundary_snapshot_save_guard(
                    &incoming,
                    previous.as_ref(),
                )
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                upsert_runtime_snapshot(&tx, &runtime_id, &session_delta.session_snapshot)?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn commit_session_transcript_rewrite_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: SessionDelta,
            commit: &meerkat_core::TranscriptRewriteCommit,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let commit = commit.clone();
            tokio::task::spawn_blocking(move || {
                let incoming = serde_json::from_slice::<meerkat_core::Session>(
                    &session_delta.session_snapshot,
                )
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let previous = tx
                    .query_row(
                        "SELECT session_snapshot FROM runtime_session_snapshots WHERE runtime_id = ?1",
                        params![runtime_id_text(&runtime_id)],
                        |row| row.get::<_, Vec<u8>>(0),
                    )
                    .optional()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?
                    .map(|bytes| deserialize_persisted_session(&bytes))
                    .transpose()?;
                meerkat_core::session_store::transcript_rewrite_save_guard(
                    &incoming,
                    previous.as_ref(),
                    &commit,
                )
                .map_err(|err| match err {
                    meerkat_core::SessionStoreError::TranscriptRevisionConflict {
                        expected, actual, ..
                    } => RuntimeStoreError::TranscriptRevisionConflict { expected, actual },
                    other => RuntimeStoreError::WriteFailed(other.to_string()),
                })?;
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
            input_updates: Vec<InputStatePersistenceRecord>,
            session_store_key: Option<meerkat_core::types::SessionId>,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let input_updates = input_updates
                .into_iter()
                .map(InputStatePersistenceRecord::into_stored)
                .collect::<Vec<_>>();
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

                // The supersession verdict keys the entire commit: if the
                // incoming session snapshot is classified as superseded (the
                // persisted head is already a valid append-extension of it),
                // the snapshot write is skipped AND so are the receipt + input
                // writes, so receipt/input ordering identity never advances
                // past the retained session truth.
                let mut session_snapshot_superseded = false;
                if let Some(session) = session_snapshot.as_ref() {
                    let mut persist_session_snapshot = true;
                    let previous = tx
                        .query_row(
                            "SELECT session_snapshot FROM runtime_session_snapshots WHERE runtime_id = ?1",
                            params![runtime_id_text(&runtime_id)],
                            |row| row.get::<_, Vec<u8>>(0),
                        )
                        .optional()
                        .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?
                        .map(|bytes| deserialize_persisted_session(&bytes))
                        .transpose()?;
                    if let Err(err) = meerkat_core::session_store::run_boundary_snapshot_save_guard(
                        session,
                        previous.as_ref(),
                    ) {
                        if previous.as_ref().is_some_and(is_runtime_placeholder_session) {
                            persist_session_snapshot = true;
                        } else if previous.as_ref().is_some_and(|previous| {
                            meerkat_core::session_store::run_boundary_snapshot_save_guard(
                                previous,
                                Some(session),
                            )
                            .is_ok()
                        }) {
                            persist_session_snapshot = false;
                            session_snapshot_superseded = true;
                        } else {
                            return Err(RuntimeStoreError::WriteFailed(err.to_string()));
                        }
                    }
                    if persist_session_snapshot
                        && let Some(delta) = session_delta.as_ref()
                    {
                        upsert_runtime_snapshot(&tx, &runtime_id, &delta.session_snapshot)?;
                    }
                }

                // When the session snapshot was superseded and skipped, the
                // boundary receipt and input-state updates must also be skipped:
                // advancing them against a retained (older) session snapshot
                // would split receipt/input ordering identity from session
                // truth. Commit the (no-op) transaction so the read lock is
                // released cleanly.
                if session_snapshot_superseded {
                    tx.commit()
                        .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                    return Ok(());
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
                    deserialize_persisted_input_state(&bytes)
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
                        encode_receipt_sequence(sequence)
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

        async fn clear_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                tx.execute(
                    "DELETE FROM runtime_session_snapshots WHERE runtime_id = ?1",
                    params![runtime_id_text(&runtime_id)],
                )
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(())
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn replace_session_snapshot_if_current(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected_current: &[u8],
            replacement: Vec<u8>,
        ) -> Result<bool, RuntimeStoreError> {
            let _: meerkat_core::Session = serde_json::from_slice(&replacement)
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let expected_current = expected_current.to_vec();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let current = tx
                    .query_row(
                        "SELECT session_snapshot FROM runtime_session_snapshots WHERE runtime_id = ?1",
                        params![runtime_id_text(&runtime_id)],
                        |row| row.get::<_, Vec<u8>>(0),
                    )
                    .optional()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                if current.as_deref() != Some(expected_current.as_slice()) {
                    return Ok(false);
                }
                upsert_runtime_snapshot(&tx, &runtime_id, &replacement)?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(true)
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn clear_session_snapshot_if_current(
            &self,
            runtime_id: &LogicalRuntimeId,
            expected_current: &[u8],
        ) -> Result<bool, RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let expected_current = expected_current.to_vec();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                let current = tx
                    .query_row(
                        "SELECT session_snapshot FROM runtime_session_snapshots WHERE runtime_id = ?1",
                        params![runtime_id_text(&runtime_id)],
                        |row| row.get::<_, Vec<u8>>(0),
                    )
                    .optional()
                    .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
                if current.as_deref() != Some(expected_current.as_slice()) {
                    return Ok(false);
                }
                tx.execute(
                    "DELETE FROM runtime_session_snapshots WHERE runtime_id = ?1",
                    params![runtime_id_text(&runtime_id)],
                )
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                tx.commit()
                    .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
                Ok(true)
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn persist_input_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            state: &InputStatePersistenceRecord,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let state = state.clone_stored();
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
                .map(|bytes| deserialize_persisted_input_state(&bytes))
                .transpose()
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn load_machine_lifecycle_record(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
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
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))
            })
            .await
            .map_err(|err| RuntimeStoreError::Internal(format!("Task join failed: {err}")))?
        }

        async fn commit_machine_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
            commit: MachineLifecycleCommit,
            input_states: &[InputStatePersistenceRecord],
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            let snapshot = commit.into_snapshot();
            let input_states = input_states
                .iter()
                .map(InputStatePersistenceRecord::clone_stored)
                .collect::<Vec<_>>();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                upsert_machine_lifecycle_snapshot(&tx, &runtime_id, &snapshot)?;
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

        async fn delete_ops_lifecycle(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<(), RuntimeStoreError> {
            let path = self.path.clone();
            let runtime_id = runtime_id.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = open_runtime_connection(&path)?;
                let tx = begin_runtime_transaction(&mut conn)?;
                tx.execute(
                    "DELETE FROM runtime_ops_lifecycle WHERE runtime_id = ?1",
                    params![runtime_id_text(&runtime_id)],
                )
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
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
        use crate::runtime_state::RuntimeState;
        use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
        use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
        use meerkat_core::session_store::SessionStore as _;
        use meerkat_core::types::{AssistantMessage, Message, StopReason, UserMessage};
        use meerkat_core::{Session, TranscriptRewriteReason, TranscriptRewriteSelection};
        use meerkat_store::SqliteSessionStore;

        fn temp_store() -> (TempDir, SqliteRuntimeStore) {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("sessions.sqlite3");
            let store = SqliteRuntimeStore::new(path).unwrap();
            (dir, store)
        }

        fn runtime_id() -> LogicalRuntimeId {
            LogicalRuntimeId("runtime-1".to_string())
        }

        fn input_state() -> InputStatePersistenceRecord {
            InputStatePersistenceRecord::from_machine_snapshot(StoredInputState::new_accepted(
                InputId::new(),
            ))
            .expect("accepted test input state seed must be machine-authorized")
        }

        fn session_with_one_turn() -> Session {
            let mut session = Session::new();
            session.push(Message::User(UserMessage::text("hello".to_string())));
            session.push(Message::Assistant(AssistantMessage {
                content: "verbose answer".to_string(),
                tool_calls: Vec::new(),
                stop_reason: StopReason::EndTurn,
                usage: meerkat_core::types::Usage::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            }));
            session
        }

        fn session_with_user(content: &str) -> Session {
            let mut session = Session::new();
            session.push(Message::User(UserMessage::text(content.to_string())));
            session
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
        async fn commit_session_snapshot_does_not_write_session_projection_row() {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("sessions.sqlite3");
            let store = SqliteRuntimeStore::new(path.clone()).unwrap();
            let runtime_id = runtime_id();
            let session = meerkat_core::Session::new();
            let session_id = session.id().clone();

            store
                .commit_session_snapshot(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&session).unwrap(),
                    },
                )
                .await
                .unwrap();

            let session_store = SqliteSessionStore::open(path).unwrap();
            assert!(
                session_store.load(&session_id).await.unwrap().is_none(),
                "runtime snapshot commits must not contaminate the SessionStore projection row before checkpoint continuity validation"
            );
            assert!(
                store
                    .load_session_snapshot(&runtime_id)
                    .await
                    .unwrap()
                    .is_some()
            );
        }

        #[tokio::test]
        async fn commit_session_snapshot_rejects_stale_runtime_parent() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let accepted = session_with_user("accepted runtime turn");
            let mut stale = Session::with_id(accepted.id().clone());
            stale.push(Message::User(UserMessage::text(
                "stale runtime turn".to_string(),
            )));
            let accepted_snapshot = serde_json::to_vec(&accepted).unwrap();

            store
                .commit_session_snapshot(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: accepted_snapshot.clone(),
                    },
                )
                .await
                .unwrap();

            let err = store
                .commit_session_snapshot(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&stale).unwrap(),
                    },
                )
                .await
                .expect_err("stale non-continuation must not overwrite runtime snapshot");

            assert!(matches!(err, RuntimeStoreError::WriteFailed(_)));
            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(accepted_snapshot)
            );
        }

        #[tokio::test]
        async fn atomic_apply_keeps_current_snapshot_when_incoming_is_superseded() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let incoming = session_with_user("turn input");
            let mut current = incoming.clone();
            current.push(Message::Assistant(AssistantMessage {
                content: "peer response already applied".to_string(),
                tool_calls: Vec::new(),
                stop_reason: StopReason::EndTurn,
                usage: meerkat_core::types::Usage::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            }));
            let current_snapshot = serde_json::to_vec(&current).unwrap();
            let receipt = RunBoundaryReceipt {
                run_id: RunId(uuid::Uuid::new_v4()),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![],
                conversation_digest: Some("machine-owned-digest".to_string()),
                message_count: 2,
                sequence: 11,
            };

            store
                .commit_session_snapshot(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: current_snapshot.clone(),
                    },
                )
                .await
                .unwrap();

            store
                .atomic_apply(
                    &runtime_id,
                    Some(SessionDelta {
                        session_snapshot: serde_json::to_vec(&incoming).unwrap(),
                    }),
                    receipt.clone(),
                    vec![input_state()],
                    Some(incoming.id().clone()),
                )
                .await
                .unwrap();

            assert_eq!(
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(current_snapshot)
            );
            // The session snapshot was classified superseded and skipped, so the
            // boundary receipt + input-state writes must NOT advance against the
            // retained (more-advanced) session snapshot.
            assert_eq!(
                store
                    .load_boundary_receipt(&runtime_id, &receipt.run_id, receipt.sequence)
                    .await
                    .unwrap(),
                None
            );
            assert!(
                store
                    .load_input_states(&runtime_id)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }

        #[tokio::test]
        async fn atomic_apply_allows_first_generated_snapshot_after_placeholder() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let mut placeholder = Session::new();
            placeholder.set_system_prompt("base system".to_string());
            let mut incoming = Session::with_id(placeholder.id().clone());
            incoming.set_system_prompt("base system".to_string());
            incoming.push(Message::User(UserMessage::text(
                "verbose first turn".to_string(),
            )));
            let parent_revision = incoming.transcript_revision().unwrap();
            incoming
                .commit_transcript_rewrite(
                    TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    vec![Message::User(UserMessage::compaction_summary(
                        "[Context compacted] first turn",
                    ))],
                    TranscriptRewriteReason::new("compaction"),
                    Some("meerkat-core".to_string()),
                    Some(parent_revision),
                )
                .unwrap();
            let incoming_snapshot = serde_json::to_vec(&incoming).unwrap();
            let receipt = RunBoundaryReceipt {
                run_id: RunId(uuid::Uuid::new_v4()),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![],
                conversation_digest: Some("machine-owned-digest".to_string()),
                message_count: incoming.messages().len(),
                sequence: 12,
            };

            store
                .commit_session_snapshot(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&placeholder).unwrap(),
                    },
                )
                .await
                .unwrap();

            store
                .atomic_apply(
                    &runtime_id,
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
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(incoming_snapshot)
            );
            assert_eq!(
                store
                    .load_boundary_receipt(&runtime_id, &receipt.run_id, receipt.sequence)
                    .await
                    .unwrap(),
                Some(receipt)
            );
        }

        #[tokio::test]
        async fn atomic_apply_allows_generated_compaction_before_retained_tail() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let mut previous = Session::new();
            previous.set_system_prompt("runtime system before context refresh".to_string());
            previous.push(Message::User(UserMessage::text(
                "Turn 1 request".to_string(),
            )));
            previous.push(Message::Assistant(AssistantMessage {
                content: "Turn 1 answer".to_string(),
                tool_calls: Vec::new(),
                stop_reason: StopReason::EndTurn,
                usage: meerkat_core::types::Usage::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            }));

            let mut incoming = Session::with_id(previous.id().clone());
            incoming.set_system_prompt("runtime system after context refresh".to_string());
            incoming.push(Message::User(UserMessage::text(
                "Verbose context that will be compacted".to_string(),
            )));
            for message in previous.messages()[1..].iter().cloned() {
                incoming.push(message);
            }
            incoming.push(Message::Assistant(AssistantMessage {
                content: "Turn 2 generated answer".to_string(),
                tool_calls: Vec::new(),
                stop_reason: StopReason::EndTurn,
                usage: meerkat_core::types::Usage::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
            }));
            let parent_revision = incoming.transcript_revision().unwrap();
            incoming
                .commit_transcript_rewrite(
                    TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    vec![Message::User(UserMessage::compaction_summary(
                        "[Context compacted] Earlier runtime context".to_string(),
                    ))],
                    TranscriptRewriteReason::new("compaction"),
                    Some("meerkat-core".to_string()),
                    Some(parent_revision),
                )
                .unwrap();
            let incoming_snapshot = serde_json::to_vec(&incoming).unwrap();
            let receipt = RunBoundaryReceipt {
                run_id: RunId(uuid::Uuid::new_v4()),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![],
                conversation_digest: Some("machine-owned-digest".to_string()),
                message_count: incoming.messages().len(),
                sequence: 13,
            };

            store
                .commit_session_snapshot(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&previous).unwrap(),
                    },
                )
                .await
                .unwrap();

            store
                .atomic_apply(
                    &runtime_id,
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
                store.load_session_snapshot(&runtime_id).await.unwrap(),
                Some(incoming_snapshot)
            );
            assert_eq!(
                store
                    .load_boundary_receipt(&runtime_id, &receipt.run_id, receipt.sequence)
                    .await
                    .unwrap(),
                Some(receipt)
            );
        }

        #[tokio::test]
        async fn transcript_rewrite_snapshot_rejects_stale_runtime_parent() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let original = session_with_one_turn();
            store
                .commit_session_snapshot(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&original).unwrap(),
                    },
                )
                .await
                .unwrap();

            let parent_revision = original.transcript_revision().unwrap();
            let mut first_rewrite = original.clone();
            let first_commit = first_rewrite
                .commit_transcript_rewrite(
                    TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    vec![Message::Assistant(AssistantMessage {
                        content: "first compact answer".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: meerkat_core::types::Usage::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    })],
                    TranscriptRewriteReason::new("compaction"),
                    Some("sqlite-test".to_string()),
                    Some(parent_revision.clone()),
                )
                .unwrap();
            store
                .commit_session_transcript_rewrite_snapshot(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&first_rewrite).unwrap(),
                    },
                    &first_commit,
                )
                .await
                .unwrap();

            let mut stale_rewrite = original;
            let stale_commit = stale_rewrite
                .commit_transcript_rewrite(
                    TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                    vec![Message::Assistant(AssistantMessage {
                        content: "stale compact answer".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: meerkat_core::types::Usage::default(),
                        created_at: meerkat_core::types::message_timestamp_now(),
                    })],
                    TranscriptRewriteReason::new("compaction"),
                    Some("sqlite-test".to_string()),
                    Some(parent_revision),
                )
                .unwrap();
            let err = store
                .commit_session_transcript_rewrite_snapshot(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&stale_rewrite).unwrap(),
                    },
                    &stale_commit,
                )
                .await
                .expect_err("stale rewrite parent should be rejected atomically");
            assert!(matches!(
                err,
                RuntimeStoreError::TranscriptRevisionConflict { .. }
            ));

            let stored = store
                .load_session_snapshot(&runtime_id)
                .await
                .unwrap()
                .unwrap();
            let stored: Session = serde_json::from_slice(&stored).unwrap();
            assert_eq!(stored.transcript_revision().unwrap(), first_commit.revision);
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
            let binding = crate::store::MachineLifecycleBindingFacts::new(
                Some("rt:session:sqlite".to_string()),
                Some(11),
                None,
                Some("epoch-sqlite".to_string()),
            );
            store
                .commit_machine_lifecycle(
                    &runtime_id,
                    MachineLifecycleCommit::new_with_binding(runtime_state, binding.clone()),
                    &[input_state()],
                )
                .await
                .unwrap();

            assert!(
                crate::store::load_runtime_state(&store, &runtime_id)
                    .await
                    .unwrap()
                    .is_some()
            );
            let lifecycle = crate::store::load_machine_lifecycle(&store, &runtime_id)
                .await
                .unwrap()
                .expect("machine lifecycle snapshot");
            assert_eq!(lifecycle.runtime_state(), runtime_state);
            assert_eq!(lifecycle.binding(), &binding);
            assert_eq!(store.load_input_states(&runtime_id).await.unwrap().len(), 1);
        }

        #[tokio::test]
        async fn legacy_runtime_state_row_is_not_lifecycle_authority() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let legacy_state_json = serde_json::to_vec(&RuntimeState::Retired).unwrap();
            let conn = open_runtime_connection(&store.path).unwrap();
            conn.execute(
                r"
                INSERT INTO runtime_states (runtime_id, runtime_state_json)
                VALUES (?1, ?2)
                ",
                params![runtime_id_text(&runtime_id), legacy_state_json],
            )
            .unwrap();

            assert!(matches!(
                crate::store::load_runtime_state(&store, &runtime_id).await,
                Err(RuntimeStoreError::ReadFailed(_))
            ));
            assert!(matches!(
                crate::store::load_machine_lifecycle(&store, &runtime_id).await,
                Err(RuntimeStoreError::ReadFailed(_))
            ));
        }

        #[tokio::test]
        async fn legacy_machine_lifecycle_snapshot_row_is_not_lifecycle_authority() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let runtime_state = RuntimeState::Retired;
            let legacy_snapshot_json = serde_json::to_vec(&serde_json::json!({
                "runtime_state": runtime_state,
                "binding": {
                    "agent_runtime_id": "rt:session:legacy",
                    "fence_token": 23,
                    "runtime_generation": 7,
                    "runtime_epoch_id": "epoch-legacy"
                }
            }))
            .unwrap();
            let conn = open_runtime_connection(&store.path).unwrap();
            conn.execute(
                r"
                INSERT INTO runtime_states (runtime_id, runtime_state_json)
                VALUES (?1, ?2)
                ",
                params![runtime_id_text(&runtime_id), legacy_snapshot_json],
            )
            .unwrap();

            assert!(matches!(
                crate::store::load_runtime_state(&store, &runtime_id).await,
                Err(RuntimeStoreError::ReadFailed(_))
            ));
            assert!(matches!(
                crate::store::load_machine_lifecycle(&store, &runtime_id).await,
                Err(RuntimeStoreError::ReadFailed(_))
            ));
        }

        #[test]
        fn receipt_sequence_encoding_is_injective_across_i64_boundary() {
            // Distinct u64 sequences straddling the i64::MAX boundary must map
            // to distinct durable keys (no saturation aliasing) and decode
            // symmetrically.
            let probes: [u64; 6] = [
                0,
                1,
                i64::MAX as u64 - 1,
                i64::MAX as u64,
                i64::MAX as u64 + 1,
                u64::MAX,
            ];
            let mut seen = std::collections::HashSet::new();
            for sequence in probes {
                let encoded = encode_receipt_sequence(sequence);
                assert!(
                    seen.insert(encoded),
                    "sequence {sequence} aliased an already-stored key {encoded}"
                );
                assert_eq!(
                    decode_receipt_sequence(encoded),
                    sequence,
                    "round-trip failed for sequence {sequence}"
                );
            }
        }

        fn receipt_with_sequence(run_id: RunId, sequence: u64) -> RunBoundaryReceipt {
            RunBoundaryReceipt {
                run_id,
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![],
                conversation_digest: Some("machine-owned-digest".to_string()),
                message_count: 1,
                sequence,
            }
        }

        #[tokio::test]
        async fn boundary_receipts_straddling_i64_max_persist_and_read_distinctly() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();
            let run_id = RunId(uuid::Uuid::new_v4());
            let low = receipt_with_sequence(run_id.clone(), i64::MAX as u64);
            let high = receipt_with_sequence(run_id.clone(), i64::MAX as u64 + 1);

            store
                .atomic_apply(&runtime_id, None, low.clone(), vec![], None)
                .await
                .unwrap();
            store
                .atomic_apply(&runtime_id, None, high.clone(), vec![], None)
                .await
                .unwrap();

            // Two distinct sequences must produce two distinct durable rows,
            // not collapse onto one i64::MAX key.
            assert_eq!(receipt_row_count(&store), 2);
            assert_eq!(
                store
                    .load_boundary_receipt(&runtime_id, &run_id, low.sequence)
                    .await
                    .unwrap(),
                Some(low)
            );
            assert_eq!(
                store
                    .load_boundary_receipt(&runtime_id, &run_id, high.sequence)
                    .await
                    .unwrap(),
                Some(high)
            );
        }

        /// A session blob without the mandatory envelope `version` byte (the
        /// pre-typed-owner v0 shape) must FAIL CLOSED through the runtime-store
        /// read helper — it never silently defaults or upgrades on read.
        #[test]
        fn deserialize_persisted_session_rejects_missing_version_row() {
            let v0_blob = serde_json::json!({
                "id": "00000000-0000-0000-0000-000000000012",
                "messages": [],
                "created_at": { "secs_since_epoch": 1727784000, "nanos_since_epoch": 0 },
                "updated_at": { "secs_since_epoch": 1727784000, "nanos_since_epoch": 0 },
                "metadata": {}
            });
            let bytes = serde_json::to_vec(&v0_blob).unwrap();

            let err = deserialize_persisted_session(&bytes)
                .expect_err("missing-version session row must fail closed");
            assert!(
                err.to_string().contains("version"),
                "unexpected error: {err}"
            );
        }

        /// A session blob carrying the retired legacy envelope version (v1)
        /// must FAIL CLOSED with the typed generated-authority rejection.
        #[test]
        fn deserialize_persisted_session_rejects_legacy_v1_version_row() {
            let v1_blob = serde_json::json!({
                "version": 1,
                "id": "00000000-0000-0000-0000-000000000012",
                "messages": [],
                "created_at": { "secs_since_epoch": 1727784000, "nanos_since_epoch": 0 },
                "updated_at": { "secs_since_epoch": 1727784000, "nanos_since_epoch": 0 },
                "metadata": {}
            });
            let bytes = serde_json::to_vec(&v1_blob).unwrap();

            let err = deserialize_persisted_session(&bytes)
                .expect_err("legacy v1 session row must fail closed");
            assert!(
                err.to_string()
                    .contains("generated session persistence version authority rejected"),
                "unexpected error: {err}"
            );
        }

        /// End-to-end: a raw v0 session row written directly into the
        /// `runtime_session_snapshots` table must FAIL the runtime-store
        /// `previous`-snapshot read path (here exercised by
        /// `commit_session_snapshot`) — the read path never silently accepts a
        /// pre-version row.
        #[tokio::test]
        async fn runtime_store_read_path_rejects_v0_session_row() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();

            // Write a v0-shaped row (no envelope `version`) directly into the
            // durable table.
            let v0_blob = serde_json::json!({
                "id": "00000000-0000-0000-0000-000000000012",
                "messages": [],
                "created_at": { "secs_since_epoch": 1727784000, "nanos_since_epoch": 0 },
                "updated_at": { "secs_since_epoch": 1727784000, "nanos_since_epoch": 0 },
                "metadata": {}
            });
            let v0_bytes = serde_json::to_vec(&v0_blob).unwrap();
            {
                let mut conn = open_runtime_connection(store.path()).unwrap();
                let tx = begin_runtime_transaction(&mut conn).unwrap();
                upsert_runtime_snapshot(&tx, &runtime_id, &v0_bytes).unwrap();
                tx.commit().unwrap();
            }

            // A subsequent boundary commit reads the persisted previous row;
            // the v0 row must surface a read failure, not a silent default.
            let mut incoming = Session::new();
            incoming.push(Message::User(UserMessage::text("hello".to_string())));
            let err = store
                .commit_session_snapshot(
                    &runtime_id,
                    SessionDelta {
                        session_snapshot: serde_json::to_vec(&incoming).unwrap(),
                    },
                )
                .await
                .expect_err("v0 previous row must fail the read path closed");
            assert!(
                err.to_string().contains("version"),
                "unexpected error: {err}"
            );
        }

        /// A `StoredInputState` row without the mandatory
        /// `stored_input_state_version` byte (the pre-version v0 shape) written
        /// directly into `runtime_input_states` must FAIL CLOSED on read.
        #[tokio::test]
        async fn runtime_store_read_path_rejects_v0_input_state_row() {
            let (_dir, store) = temp_store();
            let runtime_id = runtime_id();

            let input_id = InputId::new();
            let bundle = StoredInputState::new_accepted(input_id.clone());
            let mut row = serde_json::to_value(&bundle).unwrap();
            // Drop the version byte to simulate a pre-version (v0) row.
            row.as_object_mut()
                .unwrap()
                .remove("stored_input_state_version");
            let row_bytes = serde_json::to_vec(&row).unwrap();
            {
                let mut conn = open_runtime_connection(store.path()).unwrap();
                let tx = begin_runtime_transaction(&mut conn).unwrap();
                tx.execute(
                    r"
                    INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
                    VALUES (?1, ?2, ?3)
                    ",
                    params![
                        runtime_id_text(&runtime_id),
                        input_id.0.to_string(),
                        row_bytes
                    ],
                )
                .unwrap();
                tx.commit().unwrap();
            }

            let err = store
                .load_input_state(&runtime_id, &input_id)
                .await
                .expect_err("v0 input-state row must fail the read path closed");
            assert!(
                err.to_string().contains("stored_input_state_version"),
                "unexpected error: {err}"
            );

            let err = store
                .load_input_states(&runtime_id)
                .await
                .expect_err("v0 input-state row must fail the bulk read path closed");
            assert!(
                err.to_string().contains("stored_input_state_version"),
                "unexpected error: {err}"
            );
        }
    }
}

#[cfg(feature = "sqlite-store")]
pub use inner::SqliteRuntimeStore;
