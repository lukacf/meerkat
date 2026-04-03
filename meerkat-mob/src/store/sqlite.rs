//! SQLite-backed store implementations.
//!
//! Unlike the redb backend, SQLite uses WAL mode with no exclusive file lock,
//! allowing the same database to be reopened after drop within the same process.

use super::{MobEventStore, MobRunStore, MobSpecStore, MobStoreError};
use crate::definition::MobDefinition;
use crate::error::MobError;
use crate::event::{MobEvent, NewMobEvent};
use crate::ids::{FlowId, FrameId, LoopId, LoopInstanceId, MobId, RunId, StepId};
use crate::run::{
    FailureLedgerEntry, FrameSnapshot, LoopIterationLedgerEntry, LoopSnapshot, MobRun,
    MobRunStatus, StepLedgerEntry,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use meerkat_machine_kernels::KernelState;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};
use std::time::Duration;

const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;

const CREATE_SCHEMA_SQL: &str = r"
CREATE TABLE IF NOT EXISTS mob_events (
    cursor INTEGER PRIMARY KEY,
    event_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_event_meta (
    key TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_runs (
    run_id TEXT PRIMARY KEY,
    run_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_specs (
    mob_id TEXT PRIMARY KEY,
    spec_json BLOB NOT NULL
)";

fn se(error: impl std::fmt::Display) -> MobStoreError {
    MobStoreError::Internal(error.to_string())
}

fn encode_json<T: Serialize>(value: &T) -> Result<Vec<u8>, MobStoreError> {
    serde_json::to_vec(value).map_err(|e| MobStoreError::Serialization(e.to_string()))
}

fn decode_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, MobStoreError> {
    serde_json::from_slice(bytes).map_err(|e| MobStoreError::Serialization(e.to_string()))
}

fn cursor_to_i64(value: u64) -> Result<i64, MobStoreError> {
    i64::try_from(value)
        .map_err(|_| MobStoreError::Internal(format!("cursor value {value} exceeds i64::MAX")))
}

fn i64_to_cursor(value: i64) -> u64 {
    // SQLite INTEGER is signed; cursors start at 1 and are monotonic.
    // Negative values should never appear, but clamp to 0 defensively.
    u64::try_from(value).unwrap_or(0)
}

fn open_connection(path: &Path) -> Result<Connection, MobStoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(se)?;
    }
    let conn = Connection::open(path).map_err(se)?;
    conn.busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
        .map_err(se)?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(se)?;
    conn.pragma_update(None, "synchronous", "FULL")
        .map_err(se)?;
    conn.execute_batch(CREATE_SCHEMA_SQL).map_err(se)?;
    Ok(conn)
}

fn begin_immediate(conn: &mut Connection) -> Result<Transaction<'_>, MobStoreError> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(se)
}

fn load_run_bytes(tx: &Transaction<'_>, key: &str) -> Result<Option<Vec<u8>>, MobStoreError> {
    tx.query_row(
        "SELECT run_json FROM mob_runs WHERE run_id = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(se)
}

fn write_run_json(tx: &Transaction<'_>, key: &str, run: &MobRun) -> Result<(), MobStoreError> {
    let encoded = encode_json(run)?;
    tx.execute(
        "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
        params![encoded, key],
    )
    .map_err(se)?;
    Ok(())
}

fn append_loop_iteration_ledger_if_absent(run: &mut MobRun, entry: LoopIterationLedgerEntry) {
    if !run.loop_iteration_ledger.iter().any(|existing| {
        existing.loop_instance_id == entry.loop_instance_id
            && existing.iteration == entry.iteration
            && existing.frame_id == entry.frame_id
    }) {
        run.loop_iteration_ledger.push(entry);
    }
}

async fn run_sqlite_task<T>(
    task: impl FnOnce() -> Result<T, MobStoreError> + Send + 'static,
) -> Result<T, MobStoreError>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|error| MobStoreError::Internal(format!("sqlite task join failed: {error}")))?
}

// ---------------------------------------------------------------------------
// SqliteMobStores — unified handle (stores only the path)
// ---------------------------------------------------------------------------

/// Shared bundle that produces event/run/spec stores all pointing to the same db file.
#[derive(Debug, Clone)]
pub struct SqliteMobStores {
    path: PathBuf,
}

impl SqliteMobStores {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MobError> {
        let path = path.as_ref().to_path_buf();
        // Validate the path works by opening and immediately closing.
        let _conn = open_connection(&path)?;
        Ok(Self { path })
    }

    pub fn event_store(&self) -> SqliteMobEventStore {
        SqliteMobEventStore {
            path: self.path.clone(),
        }
    }

    pub fn run_store(&self) -> SqliteMobRunStore {
        SqliteMobRunStore {
            path: self.path.clone(),
        }
    }

    pub fn spec_store(&self) -> SqliteMobSpecStore {
        SqliteMobSpecStore {
            path: self.path.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// SqliteMobEventStore
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SqliteMobEventStore {
    path: PathBuf,
}

impl std::fmt::Debug for SqliteMobEventStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteMobEventStore")
            .field("path", &self.path)
            .finish()
    }
}

const EVENT_CURSOR_KEY: &str = "next_cursor";

#[async_trait]
impl MobEventStore for SqliteMobEventStore {
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let cursor = next_event_cursor(&tx)?;
            let stored = MobEvent {
                cursor,
                timestamp: event.timestamp.unwrap_or_else(Utc::now),
                mob_id: event.mob_id,
                kind: event.kind,
            };
            let encoded = encode_json(&stored)?;
            tx.execute(
                "INSERT INTO mob_events (cursor, event_json) VALUES (?1, ?2)",
                params![cursor_to_i64(cursor)?, encoded],
            )
            .map_err(se)?;
            set_next_cursor(&tx, cursor.saturating_add(1))?;
            tx.commit().map_err(se)?;
            Ok(stored)
        })
        .await
    }

    async fn append_batch(&self, batch: Vec<NewMobEvent>) -> Result<Vec<MobEvent>, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let mut cursor = get_next_cursor(&tx)?;
            let mut results = Vec::with_capacity(batch.len());
            for event in batch {
                let stored = MobEvent {
                    cursor,
                    timestamp: event.timestamp.unwrap_or_else(Utc::now),
                    mob_id: event.mob_id,
                    kind: event.kind,
                };
                let encoded = encode_json(&stored)?;
                tx.execute(
                    "INSERT INTO mob_events (cursor, event_json) VALUES (?1, ?2)",
                    params![cursor_to_i64(cursor)?, encoded],
                )
                .map_err(se)?;
                results.push(stored);
                cursor = cursor.saturating_add(1);
            }
            set_next_cursor(&tx, cursor)?;
            tx.commit().map_err(se)?;
            Ok(results)
        })
        .await
    }

    async fn poll(&self, after_cursor: u64, limit: usize) -> Result<Vec<MobEvent>, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT event_json FROM mob_events WHERE cursor > ?1 ORDER BY cursor LIMIT ?2",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(
                    params![
                        cursor_to_i64(after_cursor)?,
                        i64::try_from(limit).map_err(|_| se("limit exceeds i64::MAX"))?
                    ],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .map_err(se)?;
            let mut result = Vec::new();
            for row in rows {
                let bytes = row.map_err(se)?;
                result.push(decode_json(&bytes)?);
            }
            Ok(result)
        })
        .await
    }

    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare("SELECT event_json FROM mob_events ORDER BY cursor")
                .map_err(se)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut result = Vec::new();
            for row in rows {
                let bytes = row.map_err(se)?;
                result.push(decode_json(&bytes)?);
            }
            Ok(result)
        })
        .await
    }

    async fn clear(&self) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute("DELETE FROM mob_events", []).map_err(se)?;
            tx.execute(
                "INSERT OR REPLACE INTO mob_event_meta (key, value) VALUES (?1, ?2)",
                params![EVENT_CURSOR_KEY, 1i64],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn prune(&self, older_than: DateTime<Utc>) -> Result<u64, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            // Read all events, delete those older than the threshold.
            // Events store timestamp inside JSON, so we must deserialize to check.
            let mut stmt = tx
                .prepare("SELECT cursor, event_json FROM mob_events ORDER BY cursor")
                .map_err(se)?;
            let rows: Vec<(i64, Vec<u8>)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(se)?
                .collect::<Result<_, _>>()
                .map_err(se)?;
            drop(stmt);

            let mut removed = 0u64;
            for (cursor_val, bytes) in rows {
                let event: MobEvent = decode_json(&bytes)?;
                if event.timestamp < older_than {
                    tx.execute(
                        "DELETE FROM mob_events WHERE cursor = ?1",
                        params![cursor_val],
                    )
                    .map_err(se)?;
                    removed = removed.saturating_add(1);
                }
            }
            tx.commit().map_err(se)?;
            Ok(removed)
        })
        .await
    }
}

fn get_next_cursor(conn: &Connection) -> Result<u64, MobStoreError> {
    let result: Option<i64> = conn
        .query_row(
            "SELECT value FROM mob_event_meta WHERE key = ?1",
            params![EVENT_CURSOR_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(se)?;
    Ok(result.map_or(1, i64_to_cursor))
}

fn next_event_cursor(tx: &Transaction<'_>) -> Result<u64, MobStoreError> {
    get_next_cursor(tx)
}

fn set_next_cursor(conn: &Connection, value: u64) -> Result<(), MobStoreError> {
    conn.execute(
        "INSERT OR REPLACE INTO mob_event_meta (key, value) VALUES (?1, ?2)",
        params![EVENT_CURSOR_KEY, cursor_to_i64(value)?],
    )
    .map_err(se)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// SqliteMobRunStore
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SqliteMobRunStore {
    path: PathBuf,
}

impl std::fmt::Debug for SqliteMobRunStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteMobRunStore")
            .field("path", &self.path)
            .finish()
    }
}

#[async_trait]
impl MobRunStore for SqliteMobRunStore {
    async fn create_run(&self, run: MobRun) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let key = run.run_id.to_string();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            let exists: bool = tx
                .query_row(
                    "SELECT 1 FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |_| Ok(true),
                )
                .optional()
                .map_err(se)?
                .unwrap_or(false);
            if exists {
                return Err(MobStoreError::Internal(format!(
                    "run already exists: {}",
                    run.run_id
                )));
            }

            let encoded = encode_json(&run)?;
            tx.execute(
                "INSERT INTO mob_runs (run_id, run_json) VALUES (?1, ?2)",
                params![key, encoded],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let bytes: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            match bytes {
                Some(b) => Ok(Some(decode_json(&b)?)),
                None => Ok(None),
            }
        })
        .await
    }

    async fn list_runs(
        &self,
        mob_id: &MobId,
        flow_id: Option<&FlowId>,
    ) -> Result<Vec<MobRun>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let flow_id = flow_id.cloned();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn.prepare("SELECT run_json FROM mob_runs").map_err(se)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut runs = Vec::new();
            for row in rows {
                let bytes = row.map_err(se)?;
                let run: MobRun = decode_json(&bytes)?;
                if run.mob_id == mob_id && flow_id.as_ref().is_none_or(|fid| run.flow_id == *fid) {
                    runs.push(run);
                }
            }
            Ok(runs)
        })
        .await
    }

    async fn cas_run_status(
        &self,
        run_id: &RunId,
        expected: MobRunStatus,
        next: MobRunStatus,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Ok(false);
            };
            let mut run: MobRun = decode_json(&bytes)?;
            if run.status != expected || run.status.is_terminal() {
                return Ok(false);
            }
            let terminal = next.is_terminal();
            run.status = next;
            if terminal && run.completed_at.is_none() {
                run.completed_at = Some(Utc::now());
            }
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    async fn cas_flow_state(
        &self,
        run_id: &RunId,
        expected: &KernelState,
        next: &KernelState,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let expected = expected.clone();
        let next = next.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Ok(false);
            };
            let mut run: MobRun = decode_json(&bytes)?;
            if run.flow_state != expected {
                return Ok(false);
            }
            run.flow_state = next;
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    async fn cas_run_snapshot(
        &self,
        run_id: &RunId,
        expected_status: MobRunStatus,
        expected_flow_state: &KernelState,
        next_status: MobRunStatus,
        next_flow_state: &KernelState,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let expected_flow_state = expected_flow_state.clone();
        let next_flow_state = next_flow_state.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Ok(false);
            };
            let mut run: MobRun = decode_json(&bytes)?;
            if run.status != expected_status
                || run.status.is_terminal()
                || run.flow_state != expected_flow_state
            {
                return Ok(false);
            }
            let terminal = next_status.is_terminal();
            run.status = next_status;
            run.flow_state = next_flow_state;
            if terminal && run.completed_at.is_none() {
                run.completed_at = Some(Utc::now());
            }
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    async fn append_step_entry(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
    ) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            run.step_ledger.push(entry);
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn append_step_entry_if_absent(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            let is_duplicate = run.step_ledger.iter().any(|existing| {
                existing.step_id == entry.step_id
                    && existing.meerkat_id == entry.meerkat_id
                    && existing.status == entry.status
            });
            if is_duplicate {
                return Ok(false);
            }
            run.step_ledger.push(entry);
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    async fn put_step_output(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        output: serde_json::Value,
    ) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        let step_id = step_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            if let Some(entry) = run
                .step_ledger
                .iter_mut()
                .rev()
                .find(|entry| entry.step_id == step_id)
            {
                entry.output = Some(output);
            } else {
                return Err(MobStoreError::Internal(format!(
                    "cannot set output for unknown step '{step_id}' in run '{run_id}'"
                )));
            }
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn append_failure_entry(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
    ) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            run.failure_ledger.push(entry);
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn upsert_loop_snapshot(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        snapshot: LoopSnapshot,
        ledger_entry: Option<LoopIterationLedgerEntry>,
    ) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        let loop_instance_id = loop_instance_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes = load_run_bytes(&tx, &key)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            run.loops.insert(loop_instance_id, snapshot);
            if let Some(entry) = ledger_entry {
                append_loop_iteration_ledger_if_absent(&mut run, entry);
            }
            write_run_json(&tx, &key, &run)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn cas_frame_state(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        expected: Option<&FrameSnapshot>,
        next: FrameSnapshot,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        let frame_id = frame_id.clone();
        let expected = expected.cloned();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes = load_run_bytes(&tx, &key)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            let current = run.frames.get(&frame_id);
            let matches = match (expected.as_ref(), current) {
                (None, None) => true,
                (Some(exp), Some(cur)) => exp == cur,
                _ => false,
            };
            if !matches {
                return Ok(false);
            }
            run.frames.insert(frame_id, next);
            write_run_json(&tx, &key, &run)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    async fn cas_grant_node_slot(
        &self,
        run_id: &RunId,
        expected_run_state: &KernelState,
        next_run_state: KernelState,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        let expected_run_state = expected_run_state.clone();
        let frame_id = frame_id.clone();
        let expected_frame = expected_frame.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes = load_run_bytes(&tx, &key)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            if run.flow_state != expected_run_state {
                return Ok(false);
            }
            if run.frames.get(&frame_id) != Some(&expected_frame) {
                return Ok(false);
            }
            run.flow_state = next_run_state;
            run.frames.insert(frame_id, next_frame);
            write_run_json(&tx, &key, &run)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_complete_step_and_record_output(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
        step_output_key: String,
        step_output: serde_json::Value,
        loop_context: Option<(&LoopId, u64)>,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        let frame_id = frame_id.clone();
        let expected_frame = expected_frame.clone();
        let loop_context = loop_context.map(|(loop_id, iteration)| (loop_id.clone(), iteration));
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes = load_run_bytes(&tx, &key)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            if run.frames.get(&frame_id) != Some(&expected_frame) {
                return Ok(false);
            }
            run.frames.insert(frame_id, next_frame);
            match loop_context {
                None => {
                    run.root_step_outputs
                        .insert(StepId::from(step_output_key.as_str()), step_output);
                }
                Some((loop_id, iteration)) => {
                    let iteration_index = usize::try_from(iteration).map_err(|_| {
                        MobStoreError::Internal(format!(
                            "loop iteration index {iteration} exceeds usize::MAX on this target"
                        ))
                    })?;
                    let outputs = run.loop_iteration_outputs.entry(loop_id).or_default();
                    while outputs.len() <= iteration_index {
                        outputs.push(indexmap::IndexMap::new());
                    }
                    outputs[iteration_index]
                        .insert(StepId::from(step_output_key.as_str()), step_output);
                }
            }
            write_run_json(&tx, &key, &run)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_start_loop(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        expected_run_state: &KernelState,
        next_run_state: KernelState,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
        initial_loop: LoopSnapshot,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        let loop_instance_id = loop_instance_id.clone();
        let expected_run_state = expected_run_state.clone();
        let frame_id = frame_id.clone();
        let expected_frame = expected_frame.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes = load_run_bytes(&tx, &key)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            if run.flow_state != expected_run_state {
                return Ok(false);
            }
            if run.frames.get(&frame_id) != Some(&expected_frame) {
                return Ok(false);
            }
            if run.loops.contains_key(&loop_instance_id) {
                return Ok(false);
            }
            run.flow_state = next_run_state;
            run.frames.insert(frame_id, next_frame);
            run.loops.insert(loop_instance_id, initial_loop);
            write_run_json(&tx, &key, &run)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    async fn cas_loop_request_body_frame(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        expected_loop: &LoopSnapshot,
        next_loop: LoopSnapshot,
        expected_run_state: &KernelState,
        next_run_state: KernelState,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        let loop_instance_id = loop_instance_id.clone();
        let expected_loop = expected_loop.clone();
        let expected_run_state = expected_run_state.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes = load_run_bytes(&tx, &key)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            if run.flow_state != expected_run_state {
                return Ok(false);
            }
            if run.loops.get(&loop_instance_id) != Some(&expected_loop) {
                return Ok(false);
            }
            run.flow_state = next_run_state;
            run.loops.insert(loop_instance_id, next_loop);
            write_run_json(&tx, &key, &run)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_grant_body_frame_start(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        expected_loop: &LoopSnapshot,
        next_loop: LoopSnapshot,
        frame_id: &FrameId,
        initial_frame: FrameSnapshot,
        ledger_entry: LoopIterationLedgerEntry,
        expected_run_state: &KernelState,
        next_run_state: KernelState,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        let loop_instance_id = loop_instance_id.clone();
        let expected_loop = expected_loop.clone();
        let frame_id = frame_id.clone();
        let expected_run_state = expected_run_state.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes = load_run_bytes(&tx, &key)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            if run.flow_state != expected_run_state {
                return Ok(false);
            }
            if run.loops.get(&loop_instance_id) != Some(&expected_loop) {
                return Ok(false);
            }
            if run.frames.contains_key(&frame_id) {
                return Ok(false);
            }
            run.flow_state = next_run_state;
            run.loops.insert(loop_instance_id, next_loop);
            run.frames.insert(frame_id, initial_frame);
            append_loop_iteration_ledger_if_absent(&mut run, ledger_entry);
            write_run_json(&tx, &key, &run)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_complete_body_frame(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        expected_loop: &LoopSnapshot,
        next_loop: LoopSnapshot,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
        expected_run_state: &KernelState,
        next_run_state: KernelState,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        let loop_instance_id = loop_instance_id.clone();
        let expected_loop = expected_loop.clone();
        let frame_id = frame_id.clone();
        let expected_frame = expected_frame.clone();
        let expected_run_state = expected_run_state.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes = load_run_bytes(&tx, &key)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            if run.flow_state != expected_run_state {
                return Ok(false);
            }
            if run.loops.get(&loop_instance_id) != Some(&expected_loop) {
                return Ok(false);
            }
            if run.frames.get(&frame_id) != Some(&expected_frame) {
                return Ok(false);
            }
            run.flow_state = next_run_state;
            run.loops.insert(loop_instance_id, next_loop);
            run.frames.insert(frame_id, next_frame);
            write_run_json(&tx, &key, &run)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_complete_loop(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        expected_loop: &LoopSnapshot,
        next_loop: LoopSnapshot,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
        expected_run_state: &KernelState,
        next_run_state: KernelState,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        let loop_instance_id = loop_instance_id.clone();
        let expected_loop = expected_loop.clone();
        let frame_id = frame_id.clone();
        let expected_frame = expected_frame.clone();
        let expected_run_state = expected_run_state.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes = load_run_bytes(&tx, &key)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            if run.flow_state != expected_run_state {
                return Ok(false);
            }
            if run.loops.get(&loop_instance_id) != Some(&expected_loop) {
                return Ok(false);
            }
            if run.frames.get(&frame_id) != Some(&expected_frame) {
                return Ok(false);
            }
            run.flow_state = next_run_state;
            run.loops.insert(loop_instance_id, next_loop);
            run.frames.insert(frame_id, next_frame);
            write_run_json(&tx, &key, &run)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// SqliteMobSpecStore
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SqliteMobSpecStore {
    path: PathBuf,
}

impl std::fmt::Debug for SqliteMobSpecStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteMobSpecStore")
            .field("path", &self.path)
            .finish()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredSpec {
    definition: MobDefinition,
    revision: u64,
}

#[async_trait]
impl MobSpecStore for SqliteMobSpecStore {
    async fn put_spec(
        &self,
        mob_id: &MobId,
        definition: &MobDefinition,
        revision: Option<u64>,
    ) -> Result<u64, MobStoreError> {
        let path = self.path.clone();
        let key = mob_id.to_string();
        let mob_id = mob_id.clone();
        let definition = definition.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            let current: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT spec_json FROM mob_specs WHERE mob_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let current_revision = match current {
                Some(bytes) => decode_json::<StoredSpec>(&bytes)?.revision,
                None => 0,
            };

            if let Some(expected) = revision
                && expected != current_revision
            {
                return Err(MobStoreError::SpecRevisionConflict {
                    mob_id,
                    expected: revision,
                    actual: current_revision,
                });
            }

            let next_revision = current_revision + 1;
            let payload = StoredSpec {
                definition,
                revision: next_revision,
            };
            let encoded = encode_json(&payload)?;
            tx.execute(
                "INSERT OR REPLACE INTO mob_specs (mob_id, spec_json) VALUES (?1, ?2)",
                params![key, encoded],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(next_revision)
        })
        .await
    }

    async fn get_spec(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<(MobDefinition, u64)>, MobStoreError> {
        let path = self.path.clone();
        let key = mob_id.to_string();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let bytes: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT spec_json FROM mob_specs WHERE mob_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            match bytes {
                Some(b) => {
                    let stored: StoredSpec = decode_json(&b)?;
                    Ok(Some((stored.definition, stored.revision)))
                }
                None => Ok(None),
            }
        })
        .await
    }

    async fn list_specs(&self) -> Result<Vec<MobId>, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn.prepare("SELECT mob_id FROM mob_specs").map_err(se)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(se)?;
            let mut result = Vec::new();
            for row in rows {
                let id = row.map_err(se)?;
                result.push(MobId::from(id));
            }
            Ok(result)
        })
        .await
    }

    async fn delete_spec(
        &self,
        mob_id: &MobId,
        revision: Option<u64>,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = mob_id.to_string();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT spec_json FROM mob_specs WHERE mob_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Ok(false);
            };
            let stored: StoredSpec = decode_json(&bytes)?;
            if let Some(expected) = revision
                && expected != stored.revision
            {
                return Ok(false);
            }

            tx.execute("DELETE FROM mob_specs WHERE mob_id = ?1", params![key])
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::definition::{BackendConfig, FlowSpec, WiringRules};
    use crate::event::MobEventKind;
    use crate::ids::{MeerkatId, ProfileName};
    use crate::profile::{Profile, ToolConfig};
    use crate::run::StepRunStatus;
    use futures::future::join_all;
    use indexmap::IndexMap;

    fn temp_db_path() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mob.db");
        (dir, path)
    }

    fn sample_definition() -> MobDefinition {
        let mut profiles = std::collections::BTreeMap::new();
        profiles.insert(
            ProfileName::from("worker"),
            Profile {
                model: "model".to_string(),
                skills: Vec::new(),
                tools: ToolConfig::default(),
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            },
        );
        MobDefinition {
            id: MobId::from("mob"),
            orchestrator: None,
            profiles,
            mcp_servers: std::collections::BTreeMap::new(),
            wiring: WiringRules::default(),
            skills: std::collections::BTreeMap::new(),
            backend: BackendConfig::default(),
            flows: {
                let mut flows = std::collections::BTreeMap::new();
                flows.insert(
                    FlowId::from("flow-a"),
                    FlowSpec {
                        description: None,
                        steps: IndexMap::new(),
                        root: None,
                    },
                );
                flows
            },
            topology: None,
            supervisor: None,
            limits: None,
            spawn_policy: None,
            event_router: None,
            owner_session_id: None,
            session_cleanup_policy: crate::definition::SessionCleanupPolicy::Manual,
            is_implicit: false,
        }
    }

    fn sample_run(status: MobRunStatus) -> MobRun {
        MobRun {
            run_id: RunId::new(),
            mob_id: MobId::from("mob"),
            flow_id: FlowId::from("flow-a"),
            status,
            flow_state: MobRun::flow_state_for_steps([crate::ids::StepId::from("step-1")]).unwrap(),
            activation_params: serde_json::json!({"a":1}),
            created_at: Utc::now(),
            completed_at: None,
            step_ledger: Vec::new(),
            failure_ledger: Vec::new(),
            frames: std::collections::BTreeMap::new(),
            loops: std::collections::BTreeMap::new(),
            loop_iteration_ledger: Vec::new(),
            schema_version: 4,
            root_step_outputs: IndexMap::new(),
            loop_iteration_outputs: std::collections::BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn test_sqlite_event_store_append_poll_replay_prune() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().event_store();
        let now = Utc::now();

        store
            .append(NewMobEvent {
                mob_id: MobId::from("mob"),
                timestamp: Some(now - chrono::Duration::minutes(10)),
                kind: MobEventKind::MobCompleted,
            })
            .await
            .unwrap();
        store
            .append(NewMobEvent {
                mob_id: MobId::from("mob"),
                timestamp: Some(now),
                kind: MobEventKind::MobCompleted,
            })
            .await
            .unwrap();

        let polled = store.poll(0, 10).await.unwrap();
        assert_eq!(polled.len(), 2);

        let removed = store
            .prune(now - chrono::Duration::minutes(1))
            .await
            .unwrap();
        assert_eq!(removed, 1);

        let replayed = store.replay_all().await.unwrap();
        assert_eq!(replayed.len(), 1);
    }

    #[tokio::test]
    async fn test_sqlite_run_store_cas_and_dedup() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().run_store();
        let run = sample_run(MobRunStatus::Running);
        let run_id = run.run_id.clone();

        store.create_run(run).await.unwrap();

        let s1 = store.clone();
        let rid1 = run_id.clone();
        let s2 = store.clone();
        let rid2 = run_id.clone();
        let outcomes = join_all(vec![
            tokio::spawn(async move {
                s1.cas_run_status(&rid1, MobRunStatus::Running, MobRunStatus::Completed)
                    .await
                    .unwrap()
            }),
            tokio::spawn(async move {
                s2.cas_run_status(&rid2, MobRunStatus::Running, MobRunStatus::Failed)
                    .await
                    .unwrap()
            }),
        ])
        .await;

        let winners = outcomes
            .into_iter()
            .map(|join| join.unwrap())
            .filter(|value| *value)
            .count();
        assert_eq!(winners, 1);

        let entry = StepLedgerEntry {
            step_id: StepId::from("step-a"),
            meerkat_id: MeerkatId::from("worker-1"),
            status: StepRunStatus::Completed,
            output: None,
            timestamp: Utc::now(),
        };

        assert!(
            store
                .append_step_entry_if_absent(&run_id, entry.clone())
                .await
                .unwrap()
        );
        assert!(
            !store
                .append_step_entry_if_absent(&run_id, entry)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn test_sqlite_spec_store_revision_conflict() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().spec_store();
        let definition = sample_definition();

        let revision = store
            .put_spec(&MobId::from("mob"), &definition, None)
            .await
            .unwrap();
        assert_eq!(revision, 1);

        let conflict = store
            .put_spec(&MobId::from("mob"), &definition, Some(0))
            .await
            .expect_err("revision conflict expected");
        assert!(matches!(
            conflict,
            MobStoreError::SpecRevisionConflict { .. }
        ));

        let loaded = store.get_spec(&MobId::from("mob")).await.unwrap();
        assert!(loaded.is_some());
    }

    #[tokio::test]
    async fn test_sqlite_stores_share_single_database_path() {
        let (_dir, path) = temp_db_path();
        let shared = SqliteMobStores::open(&path).unwrap();
        let events = shared.event_store();
        let runs = shared.run_store();
        let specs = shared.spec_store();

        events
            .append(NewMobEvent {
                mob_id: MobId::from("mob"),
                timestamp: None,
                kind: MobEventKind::MobCompleted,
            })
            .await
            .unwrap();

        let run = sample_run(MobRunStatus::Pending);
        let run_id = run.run_id.clone();
        runs.create_run(run).await.unwrap();
        let fetched_run = runs.get_run(&run_id).await.unwrap();
        assert!(fetched_run.is_some(), "run store should share same db path");

        let definition = sample_definition();
        let revision = specs
            .put_spec(&MobId::from("mob"), &definition, None)
            .await
            .unwrap();
        assert_eq!(revision, 1);
        assert_eq!(events.replay_all().await.unwrap().len(), 1);
    }

    #[tokio::test]
    #[ignore] // integration_real: large keyset stress test
    async fn integration_real_sqlite_event_store_clear_and_prune_large_keyset() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().event_store();
        let now = Utc::now();

        for i in 0..2_000 {
            store
                .append(NewMobEvent {
                    mob_id: MobId::from("mob"),
                    timestamp: Some(now - chrono::Duration::minutes((i % 10) as i64)),
                    kind: MobEventKind::MobCompleted,
                })
                .await
                .unwrap();
        }

        let removed = store
            .prune(now - chrono::Duration::minutes(5))
            .await
            .unwrap();
        assert!(removed > 0, "expected stale events to be pruned");

        store.clear().await.unwrap();
        let replayed = store.replay_all().await.unwrap();
        assert!(
            replayed.is_empty(),
            "clear should remove all persisted events"
        );
    }

    /// Regression test: redb held an exclusive file lock that prevented
    /// reopening the same database path after drop within the same process.
    /// SQLite WAL mode does not have this limitation.
    #[tokio::test]
    async fn test_sqlite_reopen_after_drop_same_path() {
        let (_dir, path) = temp_db_path();

        // First open: write data
        {
            let stores = SqliteMobStores::open(&path).unwrap();
            let events = stores.event_store();
            events
                .append(NewMobEvent {
                    mob_id: MobId::from("mob"),
                    timestamp: None,
                    kind: MobEventKind::MobCompleted,
                })
                .await
                .unwrap();
            // stores + events dropped here
        }

        // Second open: must succeed and see prior data
        {
            let stores = SqliteMobStores::open(&path).unwrap();
            let events = stores.event_store();
            let all = events.replay_all().await.unwrap();
            assert_eq!(all.len(), 1, "data from first open must survive reopen");
        }
    }
}
