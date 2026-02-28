//! redb-backed store implementations.

use super::{MobEventStore, MobRunStore, MobSpecStore};
use crate::definition::MobDefinition;
use crate::error::MobError;
use crate::event::{MobEvent, NewMobEvent};
use crate::ids::{FlowId, MobId, RunId, StepId};
use crate::run::{FailureLedgerEntry, MobRun, MobRunStatus, StepLedgerEntry};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Serialize, de::DeserializeOwned};
use std::path::Path;
use std::sync::Arc;

const EVENTS_TABLE: TableDefinition<u64, &[u8]> = TableDefinition::new("mob_events");
const EVENT_META_TABLE: TableDefinition<&str, u64> = TableDefinition::new("mob_event_meta");
const RUNS_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("mob_runs");
const SPECS_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("mob_specs");
const EVENT_CURSOR_KEY: &str = "next_cursor";
const DELETE_BATCH_SIZE: usize = 256;

fn storage_error<E>(error: E) -> MobError
where
    E: std::error::Error + Send + Sync + 'static,
{
    MobError::StorageError(Box::new(error))
}

fn encode_json<T: Serialize>(value: &T) -> Result<Vec<u8>, MobError> {
    serde_json::to_vec(value).map_err(storage_error)
}

fn decode_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, MobError> {
    serde_json::from_slice(bytes).map_err(storage_error)
}

fn run_key(run_id: &RunId) -> Vec<u8> {
    run_id.to_string().into_bytes()
}

fn mob_key(mob_id: &MobId) -> Vec<u8> {
    mob_id.to_string().into_bytes()
}

fn open_db(path: impl AsRef<Path>) -> Result<Arc<Database>, MobError> {
    let db = Database::create(path).map_err(storage_error)?;
    let write_txn = db.begin_write().map_err(storage_error)?;
    {
        let _ = write_txn.open_table(EVENTS_TABLE).map_err(storage_error)?;
        let _ = write_txn
            .open_table(EVENT_META_TABLE)
            .map_err(storage_error)?;
        let _ = write_txn.open_table(RUNS_TABLE).map_err(storage_error)?;
        let _ = write_txn.open_table(SPECS_TABLE).map_err(storage_error)?;
    }
    write_txn.commit().map_err(storage_error)?;

    Ok(Arc::new(db))
}

async fn run_redb_task<T>(
    task: impl FnOnce() -> Result<T, MobError> + Send + 'static,
) -> Result<T, MobError>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|error| MobError::Internal(format!("redb task join failed: {error}")))?
}

#[derive(Clone)]
pub struct RedbMobEventStore {
    db: Arc<Database>,
}

impl std::fmt::Debug for RedbMobEventStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedbMobEventStore")
            .field("db", &"<redb::Database>")
            .finish()
    }
}

impl RedbMobEventStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MobError> {
        Ok(Self::from_db(open_db(path)?))
    }

    fn from_db(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl MobEventStore for RedbMobEventStore {
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobError> {
        let db = self.db.clone();
        run_redb_task(move || {
            let write_txn = db.begin_write().map_err(storage_error)?;
            let cursor = {
                let mut meta = write_txn
                    .open_table(EVENT_META_TABLE)
                    .map_err(storage_error)?;
                let cursor = meta
                    .get(EVENT_CURSOR_KEY)
                    .map_err(storage_error)?
                    .map_or(1, |value| value.value());
                meta.insert(EVENT_CURSOR_KEY, cursor.saturating_add(1))
                    .map_err(storage_error)?;
                cursor
            };

            let stored = MobEvent {
                cursor,
                timestamp: event.timestamp.unwrap_or_else(Utc::now),
                mob_id: event.mob_id,
                kind: event.kind,
            };
            let encoded = encode_json(&stored)?;

            {
                let mut table = write_txn.open_table(EVENTS_TABLE).map_err(storage_error)?;
                table
                    .insert(cursor, encoded.as_slice())
                    .map_err(storage_error)?;
            }

            write_txn.commit().map_err(storage_error)?;
            Ok(stored)
        })
        .await
    }

    async fn append_batch(&self, batch: Vec<NewMobEvent>) -> Result<Vec<MobEvent>, MobError> {
        let db = self.db.clone();
        run_redb_task(move || {
            let write_txn = db.begin_write().map_err(storage_error)?;
            let mut results = Vec::with_capacity(batch.len());
            {
                let mut meta = write_txn
                    .open_table(EVENT_META_TABLE)
                    .map_err(storage_error)?;
                let mut table = write_txn.open_table(EVENTS_TABLE).map_err(storage_error)?;

                let mut cursor = meta
                    .get(EVENT_CURSOR_KEY)
                    .map_err(storage_error)?
                    .map_or(1, |value| value.value());

                for event in batch {
                    let stored = MobEvent {
                        cursor,
                        timestamp: event.timestamp.unwrap_or_else(Utc::now),
                        mob_id: event.mob_id,
                        kind: event.kind,
                    };
                    let encoded = encode_json(&stored)?;
                    table
                        .insert(cursor, encoded.as_slice())
                        .map_err(storage_error)?;
                    results.push(stored);
                    cursor = cursor.saturating_add(1);
                }

                meta.insert(EVENT_CURSOR_KEY, cursor)
                    .map_err(storage_error)?;
            }
            write_txn.commit().map_err(storage_error)?;
            Ok(results)
        })
        .await
    }

    async fn poll(&self, after_cursor: u64, limit: usize) -> Result<Vec<MobEvent>, MobError> {
        let db = self.db.clone();
        run_redb_task(move || {
            let read_txn = db.begin_read().map_err(storage_error)?;
            let table = read_txn.open_table(EVENTS_TABLE).map_err(storage_error)?;
            let mut result = Vec::new();

            let start = after_cursor.saturating_add(1);
            let iter = table.range(start..).map_err(storage_error)?;
            for entry in iter.take(limit) {
                let (_, value) = entry.map_err(storage_error)?;
                result.push(decode_json(value.value())?);
            }

            Ok(result)
        })
        .await
    }

    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobError> {
        let db = self.db.clone();
        run_redb_task(move || {
            let read_txn = db.begin_read().map_err(storage_error)?;
            let table = read_txn.open_table(EVENTS_TABLE).map_err(storage_error)?;
            let mut result = Vec::new();
            let iter = table.iter().map_err(storage_error)?;

            for entry in iter {
                let (_, value) = entry.map_err(storage_error)?;
                result.push(decode_json(value.value())?);
            }

            Ok(result)
        })
        .await
    }

    async fn clear(&self) -> Result<(), MobError> {
        let db = self.db.clone();
        run_redb_task(move || {
            let write_txn = db.begin_write().map_err(storage_error)?;
            {
                let mut table = write_txn.open_table(EVENTS_TABLE).map_err(storage_error)?;
                loop {
                    let mut batch = Vec::with_capacity(DELETE_BATCH_SIZE);
                    let iter = table.iter().map_err(storage_error)?;
                    for entry in iter.take(DELETE_BATCH_SIZE) {
                        let (key, _) = entry.map_err(storage_error)?;
                        batch.push(key.value());
                    }
                    if batch.is_empty() {
                        break;
                    }
                    for key in batch {
                        let _ = table.remove(key).map_err(storage_error)?;
                    }
                }
            }
            {
                let mut meta = write_txn
                    .open_table(EVENT_META_TABLE)
                    .map_err(storage_error)?;
                meta.insert(EVENT_CURSOR_KEY, 1).map_err(storage_error)?;
            }
            write_txn.commit().map_err(storage_error)?;
            Ok(())
        })
        .await
    }

    async fn prune(&self, older_than: DateTime<Utc>) -> Result<u64, MobError> {
        let db = self.db.clone();
        run_redb_task(move || {
            let write_txn = db.begin_write().map_err(storage_error)?;
            let mut removed_count = 0u64;
            {
                let mut table = write_txn.open_table(EVENTS_TABLE).map_err(storage_error)?;
                loop {
                    let mut stale = Vec::with_capacity(DELETE_BATCH_SIZE);
                    let iter = table.iter().map_err(storage_error)?;
                    for entry in iter {
                        let (key, value) = entry.map_err(storage_error)?;
                        let event: MobEvent = decode_json(value.value())?;
                        if event.timestamp < older_than {
                            stale.push(key.value());
                        }
                        if stale.len() >= DELETE_BATCH_SIZE {
                            break;
                        }
                    }
                    if stale.is_empty() {
                        break;
                    }
                    for key in stale {
                        let _ = table.remove(key).map_err(storage_error)?;
                        removed_count = removed_count.saturating_add(1);
                    }
                }
            }

            write_txn.commit().map_err(storage_error)?;
            Ok(removed_count)
        })
        .await
    }
}

#[derive(Clone)]
pub struct RedbMobRunStore {
    db: Arc<Database>,
}

impl std::fmt::Debug for RedbMobRunStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedbMobRunStore")
            .field("db", &"<redb::Database>")
            .finish()
    }
}

impl RedbMobRunStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MobError> {
        Ok(Self::from_db(open_db(path)?))
    }

    fn from_db(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl MobRunStore for RedbMobRunStore {
    async fn create_run(&self, run: MobRun) -> Result<(), MobError> {
        let db = self.db.clone();
        let key = run_key(&run.run_id);
        run_redb_task(move || {
            let write_txn = db.begin_write().map_err(storage_error)?;
            {
                let mut table = write_txn.open_table(RUNS_TABLE).map_err(storage_error)?;
                if table.get(key.as_slice()).map_err(storage_error)?.is_some() {
                    return Err(MobError::Internal(format!(
                        "run already exists: {}",
                        run.run_id
                    )));
                }

                let encoded = encode_json(&run)?;
                table
                    .insert(key.as_slice(), encoded.as_slice())
                    .map_err(storage_error)?;
            }
            write_txn.commit().map_err(storage_error)?;
            Ok(())
        })
        .await
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, MobError> {
        let db = self.db.clone();
        let key = run_key(run_id);
        run_redb_task(move || {
            let read_txn = db.begin_read().map_err(storage_error)?;
            let table = read_txn.open_table(RUNS_TABLE).map_err(storage_error)?;
            match table.get(key.as_slice()).map_err(storage_error)? {
                Some(value) => Ok(Some(decode_json(value.value())?)),
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
        let db = self.db.clone();
        let mob_id = mob_id.clone();
        let flow_id = flow_id.cloned();
        run_redb_task(move || {
            let read_txn = db.begin_read().map_err(storage_error)?;
            let table = read_txn.open_table(RUNS_TABLE).map_err(storage_error)?;
            let iter = table.iter().map_err(storage_error)?;
            let mut runs = Vec::new();

            for entry in iter {
                let (_, value) = entry.map_err(storage_error)?;
                let run: MobRun = decode_json(value.value())?;
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
        let db = self.db.clone();
        let key = run_key(run_id);
        run_redb_task(move || {
            let write_txn = db.begin_write().map_err(storage_error)?;
            let cas_result = {
                let mut table = write_txn.open_table(RUNS_TABLE).map_err(storage_error)?;
                let stored = table
                    .get(key.as_slice())
                    .map_err(storage_error)?
                    .map(|value| value.value().to_vec());
                let Some(stored) = stored else {
                    return Ok(false);
                };
                let mut run: MobRun = decode_json(&stored)?;

                if run.status != expected || run.status.is_terminal() {
                    return Ok(false);
                }

                let terminal = next.is_terminal();
                run.status = next;
                if terminal && run.completed_at.is_none() {
                    run.completed_at = Some(Utc::now());
                }

                let encoded = encode_json(&run)?;
                table
                    .insert(key.as_slice(), encoded.as_slice())
                    .map_err(storage_error)?;
                true
            };
            write_txn.commit().map_err(storage_error)?;
            Ok(cas_result)
        })
        .await
    }

    async fn append_step_entry(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
    ) -> Result<(), MobError> {
        let db = self.db.clone();
        let key = run_key(run_id);
        let run_id = run_id.clone();
        run_redb_task(move || {
            let write_txn = db.begin_write().map_err(storage_error)?;
            {
                let mut table = write_txn.open_table(RUNS_TABLE).map_err(storage_error)?;
                let stored = table
                    .get(key.as_slice())
                    .map_err(storage_error)?
                    .map(|value| value.value().to_vec());
                let Some(stored) = stored else {
                    return Err(MobError::RunNotFound(run_id));
                };
                let mut run: MobRun = decode_json(&stored)?;
                run.step_ledger.push(entry);

                let encoded = encode_json(&run)?;
                table
                    .insert(key.as_slice(), encoded.as_slice())
                    .map_err(storage_error)?;
            }
            write_txn.commit().map_err(storage_error)?;
            Ok(())
        })
        .await
    }

    async fn append_step_entry_if_absent(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
    ) -> Result<bool, MobError> {
        let db = self.db.clone();
        let key = run_key(run_id);
        let run_id = run_id.clone();
        run_redb_task(move || {
            let write_txn = db.begin_write().map_err(storage_error)?;
            let was_inserted = {
                let mut table = write_txn.open_table(RUNS_TABLE).map_err(storage_error)?;
                let stored = table
                    .get(key.as_slice())
                    .map_err(storage_error)?
                    .map(|value| value.value().to_vec());
                let Some(stored) = stored else {
                    return Err(MobError::RunNotFound(run_id));
                };
                let mut run: MobRun = decode_json(&stored)?;

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
                table
                    .insert(key.as_slice(), encoded.as_slice())
                    .map_err(storage_error)?;
                true
            };
            write_txn.commit().map_err(storage_error)?;
            Ok(was_inserted)
        })
        .await
    }

    async fn put_step_output(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        output: serde_json::Value,
    ) -> Result<(), MobError> {
        let db = self.db.clone();
        let key = run_key(run_id);
        let run_id = run_id.clone();
        let step_id = step_id.clone();
        run_redb_task(move || {
            let write_txn = db.begin_write().map_err(storage_error)?;
            {
                let mut table = write_txn.open_table(RUNS_TABLE).map_err(storage_error)?;
                let stored = table
                    .get(key.as_slice())
                    .map_err(storage_error)?
                    .map(|value| value.value().to_vec());
                let Some(stored) = stored else {
                    return Err(MobError::RunNotFound(run_id.clone()));
                };
                let mut run: MobRun = decode_json(&stored)?;

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
                table
                    .insert(key.as_slice(), encoded.as_slice())
                    .map_err(storage_error)?;
            }
            write_txn.commit().map_err(storage_error)?;
            Ok(())
        })
        .await
    }

    async fn append_failure_entry(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
    ) -> Result<(), MobError> {
        let db = self.db.clone();
        let key = run_key(run_id);
        let run_id = run_id.clone();
        run_redb_task(move || {
            let write_txn = db.begin_write().map_err(storage_error)?;
            {
                let mut table = write_txn.open_table(RUNS_TABLE).map_err(storage_error)?;
                let stored = table
                    .get(key.as_slice())
                    .map_err(storage_error)?
                    .map(|value| value.value().to_vec());
                let Some(stored) = stored else {
                    return Err(MobError::RunNotFound(run_id));
                };
                let mut run: MobRun = decode_json(&stored)?;
                run.failure_ledger.push(entry);

                let encoded = encode_json(&run)?;
                table
                    .insert(key.as_slice(), encoded.as_slice())
                    .map_err(storage_error)?;
            }
            write_txn.commit().map_err(storage_error)?;
            Ok(())
        })
        .await
    }
}

#[derive(Clone)]
pub struct RedbMobSpecStore {
    db: Arc<Database>,
}

impl std::fmt::Debug for RedbMobSpecStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedbMobSpecStore")
            .field("db", &"<redb::Database>")
            .finish()
    }
}

impl RedbMobSpecStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MobError> {
        Ok(Self::from_db(open_db(path)?))
    }

    fn from_db(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[derive(Clone)]
pub struct RedbMobStores {
    db: Arc<Database>,
}

impl std::fmt::Debug for RedbMobStores {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedbMobStores")
            .field("db", &"<redb::Database>")
            .finish()
    }
}

impl RedbMobStores {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MobError> {
        Ok(Self { db: open_db(path)? })
    }

    pub fn event_store(&self) -> RedbMobEventStore {
        RedbMobEventStore::from_db(self.db.clone())
    }

    pub fn run_store(&self) -> RedbMobRunStore {
        RedbMobRunStore::from_db(self.db.clone())
    }

    pub fn spec_store(&self) -> RedbMobSpecStore {
        RedbMobSpecStore::from_db(self.db.clone())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredSpec {
    definition: MobDefinition,
    revision: u64,
}

#[async_trait]
impl MobSpecStore for RedbMobSpecStore {
    async fn put_spec(
        &self,
        mob_id: &MobId,
        definition: &MobDefinition,
        revision: Option<u64>,
    ) -> Result<u64, MobError> {
        let db = self.db.clone();
        let key = mob_key(mob_id);
        let mob_id = mob_id.clone();
        let definition = definition.clone();

        run_redb_task(move || {
            let write_txn = db.begin_write().map_err(storage_error)?;
            let next_revision = {
                let mut table = write_txn.open_table(SPECS_TABLE).map_err(storage_error)?;
                let current_revision = match table.get(key.as_slice()).map_err(storage_error)? {
                    Some(value) => decode_json::<StoredSpec>(value.value())?.revision,
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
                table
                    .insert(key.as_slice(), encoded.as_slice())
                    .map_err(storage_error)?;
                next_revision
            };
            write_txn.commit().map_err(storage_error)?;
            Ok(next_revision)
        })
        .await
    }

    async fn get_spec(&self, mob_id: &MobId) -> Result<Option<(MobDefinition, u64)>, MobError> {
        let db = self.db.clone();
        let key = mob_key(mob_id);
        run_redb_task(move || {
            let read_txn = db.begin_read().map_err(storage_error)?;
            let table = read_txn.open_table(SPECS_TABLE).map_err(storage_error)?;
            match table.get(key.as_slice()).map_err(storage_error)? {
                Some(value) => {
                    let stored: StoredSpec = decode_json(value.value())?;
                    Ok(Some((stored.definition, stored.revision)))
                }
                None => Ok(None),
            }
        })
        .await
    }

    async fn list_specs(&self) -> Result<Vec<MobId>, MobError> {
        let db = self.db.clone();
        run_redb_task(move || {
            let read_txn = db.begin_read().map_err(storage_error)?;
            let table = read_txn.open_table(SPECS_TABLE).map_err(storage_error)?;
            let iter = table.iter().map_err(storage_error)?;
            let mut result = Vec::new();

            for entry in iter {
                let (key, _) = entry.map_err(storage_error)?;
                let id = String::from_utf8(key.value().to_vec()).map_err(storage_error)?;
                result.push(MobId::from(id));
            }

            Ok(result)
        })
        .await
    }

    async fn delete_spec(&self, mob_id: &MobId, revision: Option<u64>) -> Result<bool, MobError> {
        let db = self.db.clone();
        let key = mob_key(mob_id);
        run_redb_task(move || {
            let write_txn = db.begin_write().map_err(storage_error)?;
            let removed = {
                let mut table = write_txn.open_table(SPECS_TABLE).map_err(storage_error)?;
                let stored = table
                    .get(key.as_slice())
                    .map_err(storage_error)?
                    .map(|value| value.value().to_vec());
                let Some(stored) = stored else {
                    return Ok(false);
                };
                let stored: StoredSpec = decode_json(&stored)?;

                if let Some(expected) = revision
                    && expected != stored.revision
                {
                    return Ok(false);
                }

                let _ = table.remove(key.as_slice()).map_err(storage_error)?;
                true
            };
            write_txn.commit().map_err(storage_error)?;
            Ok(removed)
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
        let path = dir.path().join("mob.redb");
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
            activation_params: serde_json::json!({"a":1}),
            created_at: Utc::now(),
            completed_at: None,
            step_ledger: Vec::new(),
            failure_ledger: Vec::new(),
        }
    }

    #[tokio::test]
    async fn test_redb_event_store_append_poll_replay_prune() {
        let (_dir, path) = temp_db_path();
        let store = RedbMobEventStore::open(&path).unwrap();
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
    #[ignore] // integration_real: hits real redb with large keyset (~19s)
    async fn integration_real_redb_event_store_clear_and_prune_large_keyset() {
        let (_dir, path) = temp_db_path();
        let store = RedbMobEventStore::open(&path).unwrap();
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

    #[tokio::test]
    async fn test_redb_run_store_cas_and_dedup() {
        let (_dir, path) = temp_db_path();
        let store = RedbMobRunStore::open(&path).unwrap();
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
    async fn test_redb_spec_store_revision_conflict() {
        let (_dir, path) = temp_db_path();
        let store = RedbMobSpecStore::open(&path).unwrap();
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
    async fn test_redb_stores_share_single_database_handle() {
        let (_dir, path) = temp_db_path();
        let shared = RedbMobStores::open(&path).unwrap();
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
        assert!(
            fetched_run.is_some(),
            "run store should share same open db lifecycle"
        );

        let definition = sample_definition();
        let revision = specs
            .put_spec(&MobId::from("mob"), &definition, None)
            .await
            .unwrap();
        assert_eq!(revision, 1);
        assert_eq!(events.replay_all().await.unwrap().len(), 1);
    }
}
