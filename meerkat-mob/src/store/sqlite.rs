//! SQLite-backed store implementations.
//!
//! Unlike the redb backend, SQLite uses WAL mode with no exclusive file lock,
//! allowing the same database to be reopened after drop within the same process.

use super::{MobEventStore, MobRunStore, MobSpecStore};
use crate::definition::MobDefinition;
use crate::error::MobError;
use crate::event::{MobEvent, NewMobEvent};
use crate::ids::{FlowId, MobId, RunId, StepId};
use crate::run::{FailureLedgerEntry, MobRun, MobRunStatus, StepLedgerEntry};
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

fn storage_error(error: impl std::fmt::Display) -> MobError {
    MobError::StorageError(Box::<dyn std::error::Error + Send + Sync>::from(
        error.to_string(),
    ))
}

fn encode_json<T: Serialize>(value: &T) -> Result<Vec<u8>, MobError> {
    serde_json::to_vec(value).map_err(storage_error)
}

fn decode_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, MobError> {
    serde_json::from_slice(bytes).map_err(storage_error)
}

fn cursor_to_i64(value: u64) -> Result<i64, MobError> {
    i64::try_from(value)
        .map_err(|_| MobError::Internal(format!("cursor value {value} exceeds i64::MAX")))
}

fn i64_to_cursor(value: i64) -> u64 {
    // SQLite INTEGER is signed; cursors start at 1 and are monotonic.
    // Negative values should never appear, but clamp to 0 defensively.
    u64::try_from(value).unwrap_or(0)
}

fn open_connection(path: &Path) -> Result<Connection, MobError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(storage_error)?;
    }
    let conn = Connection::open(path).map_err(storage_error)?;
    conn.busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
        .map_err(storage_error)?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(storage_error)?;
    conn.pragma_update(None, "synchronous", "FULL")
        .map_err(storage_error)?;
    conn.execute_batch(CREATE_SCHEMA_SQL)
        .map_err(storage_error)?;
    Ok(conn)
}

fn begin_immediate(conn: &mut Connection) -> Result<Transaction<'_>, MobError> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_error)
}

async fn run_sqlite_task<T>(
    task: impl FnOnce() -> Result<T, MobError> + Send + 'static,
) -> Result<T, MobError>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|error| MobError::Internal(format!("sqlite task join failed: {error}")))?
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
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobError> {
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
            .map_err(storage_error)?;
            set_next_cursor(&tx, cursor.saturating_add(1))?;
            tx.commit().map_err(storage_error)?;
            Ok(stored)
        })
        .await
    }

    async fn append_batch(&self, batch: Vec<NewMobEvent>) -> Result<Vec<MobEvent>, MobError> {
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
                .map_err(storage_error)?;
                results.push(stored);
                cursor = cursor.saturating_add(1);
            }
            set_next_cursor(&tx, cursor)?;
            tx.commit().map_err(storage_error)?;
            Ok(results)
        })
        .await
    }

    async fn poll(&self, after_cursor: u64, limit: usize) -> Result<Vec<MobEvent>, MobError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT event_json FROM mob_events WHERE cursor > ?1 ORDER BY cursor LIMIT ?2",
                )
                .map_err(storage_error)?;
            let rows = stmt
                .query_map(
                    params![
                        cursor_to_i64(after_cursor)?,
                        i64::try_from(limit)
                            .map_err(|_| storage_error("limit exceeds i64::MAX"))?
                    ],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .map_err(storage_error)?;
            let mut result = Vec::new();
            for row in rows {
                let bytes = row.map_err(storage_error)?;
                result.push(decode_json(&bytes)?);
            }
            Ok(result)
        })
        .await
    }

    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare("SELECT event_json FROM mob_events ORDER BY cursor")
                .map_err(storage_error)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(storage_error)?;
            let mut result = Vec::new();
            for row in rows {
                let bytes = row.map_err(storage_error)?;
                result.push(decode_json(&bytes)?);
            }
            Ok(result)
        })
        .await
    }

    async fn clear(&self) -> Result<(), MobError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute("DELETE FROM mob_events", [])
                .map_err(storage_error)?;
            tx.execute(
                "INSERT OR REPLACE INTO mob_event_meta (key, value) VALUES (?1, ?2)",
                params![EVENT_CURSOR_KEY, 1i64],
            )
            .map_err(storage_error)?;
            tx.commit().map_err(storage_error)?;
            Ok(())
        })
        .await
    }

    async fn prune(&self, older_than: DateTime<Utc>) -> Result<u64, MobError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            // Read all events, delete those older than the threshold.
            // Events store timestamp inside JSON, so we must deserialize to check.
            let mut stmt = tx
                .prepare("SELECT cursor, event_json FROM mob_events ORDER BY cursor")
                .map_err(storage_error)?;
            let rows: Vec<(i64, Vec<u8>)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(storage_error)?
                .collect::<Result<_, _>>()
                .map_err(storage_error)?;
            drop(stmt);

            let mut removed = 0u64;
            for (cursor_val, bytes) in rows {
                let event: MobEvent = decode_json(&bytes)?;
                if event.timestamp < older_than {
                    tx.execute(
                        "DELETE FROM mob_events WHERE cursor = ?1",
                        params![cursor_val],
                    )
                    .map_err(storage_error)?;
                    removed = removed.saturating_add(1);
                }
            }
            tx.commit().map_err(storage_error)?;
            Ok(removed)
        })
        .await
    }
}

fn get_next_cursor(conn: &Connection) -> Result<u64, MobError> {
    let result: Option<i64> = conn
        .query_row(
            "SELECT value FROM mob_event_meta WHERE key = ?1",
            params![EVENT_CURSOR_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage_error)?;
    Ok(result.map_or(1, i64_to_cursor))
}

fn next_event_cursor(tx: &Transaction<'_>) -> Result<u64, MobError> {
    get_next_cursor(tx)
}

fn set_next_cursor(conn: &Connection, value: u64) -> Result<(), MobError> {
    conn.execute(
        "INSERT OR REPLACE INTO mob_event_meta (key, value) VALUES (?1, ?2)",
        params![EVENT_CURSOR_KEY, cursor_to_i64(value)?],
    )
    .map_err(storage_error)?;
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
    async fn create_run(&self, run: MobRun) -> Result<(), MobError> {
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
                .map_err(storage_error)?
                .unwrap_or(false);
            if exists {
                return Err(MobError::Internal(format!(
                    "run already exists: {}",
                    run.run_id
                )));
            }

            let encoded = encode_json(&run)?;
            tx.execute(
                "INSERT INTO mob_runs (run_id, run_json) VALUES (?1, ?2)",
                params![key, encoded],
            )
            .map_err(storage_error)?;
            tx.commit().map_err(storage_error)?;
            Ok(())
        })
        .await
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, MobError> {
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
                .map_err(storage_error)?;
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
    ) -> Result<Vec<MobRun>, MobError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let flow_id = flow_id.cloned();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare("SELECT run_json FROM mob_runs")
                .map_err(storage_error)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(storage_error)?;
            let mut runs = Vec::new();
            for row in rows {
                let bytes = row.map_err(storage_error)?;
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
    ) -> Result<bool, MobError> {
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
                .map_err(storage_error)?;
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
            .map_err(storage_error)?;
            tx.commit().map_err(storage_error)?;
            Ok(true)
        })
        .await
    }

    async fn cas_flow_state(
        &self,
        run_id: &RunId,
        expected: &KernelState,
        next: &KernelState,
    ) -> Result<bool, MobError> {
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
                .map_err(storage_error)?;
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
            .map_err(storage_error)?;
            tx.commit().map_err(storage_error)?;
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
    ) -> Result<bool, MobError> {
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
                .map_err(storage_error)?;
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
            .map_err(storage_error)?;
            tx.commit().map_err(storage_error)?;
            Ok(true)
        })
        .await
    }

    async fn append_step_entry(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
    ) -> Result<(), MobError> {
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
                .map_err(storage_error)?;
            let Some(bytes) = bytes else {
                return Err(MobError::RunNotFound(run_id));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            run.step_ledger.push(entry);
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(storage_error)?;
            tx.commit().map_err(storage_error)?;
            Ok(())
        })
        .await
    }

    async fn append_step_entry_if_absent(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
    ) -> Result<bool, MobError> {
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
                .map_err(storage_error)?;
            let Some(bytes) = bytes else {
                return Err(MobError::RunNotFound(run_id));
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
            .map_err(storage_error)?;
            tx.commit().map_err(storage_error)?;
            Ok(true)
        })
        .await
    }

    async fn put_step_output(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        output: serde_json::Value,
    ) -> Result<(), MobError> {
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
                .map_err(storage_error)?;
            let Some(bytes) = bytes else {
                return Err(MobError::RunNotFound(run_id));
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
                return Err(MobError::Internal(format!(
                    "cannot set output for unknown step '{step_id}' in run '{run_id}'"
                )));
            }
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(storage_error)?;
            tx.commit().map_err(storage_error)?;
            Ok(())
        })
        .await
    }

    async fn append_failure_entry(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
    ) -> Result<(), MobError> {
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
                .map_err(storage_error)?;
            let Some(bytes) = bytes else {
                return Err(MobError::RunNotFound(run_id));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            run.failure_ledger.push(entry);
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(storage_error)?;
            tx.commit().map_err(storage_error)?;
            Ok(())
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
    ) -> Result<u64, MobError> {
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
                .map_err(storage_error)?;
            let current_revision = match current {
                Some(bytes) => decode_json::<StoredSpec>(&bytes)?.revision,
                None => 0,
            };

            if let Some(expected) = revision
                && expected != current_revision
            {
                return Err(MobError::SpecRevisionConflict {
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
            .map_err(storage_error)?;
            tx.commit().map_err(storage_error)?;
            Ok(next_revision)
        })
        .await
    }

    async fn get_spec(&self, mob_id: &MobId) -> Result<Option<(MobDefinition, u64)>, MobError> {
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
                .map_err(storage_error)?;
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

    async fn list_specs(&self) -> Result<Vec<MobId>, MobError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare("SELECT mob_id FROM mob_specs")
                .map_err(storage_error)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(storage_error)?;
            let mut result = Vec::new();
            for row in rows {
                let id = row.map_err(storage_error)?;
                result.push(MobId::from(id));
            }
            Ok(result)
        })
        .await
    }

    async fn delete_spec(&self, mob_id: &MobId, revision: Option<u64>) -> Result<bool, MobError> {
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
                .map_err(storage_error)?;
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
                .map_err(storage_error)?;
            tx.commit().map_err(storage_error)?;
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
                    },
                );
                flows
            },
            topology: None,
            supervisor: None,
            limits: None,
            spawn_policy: None,
            event_router: None,
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
        assert!(matches!(conflict, MobError::SpecRevisionConflict { .. }));

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
