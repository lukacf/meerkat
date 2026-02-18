use super::{MobEventStore, MobRunStore, MobSpecStore};
use crate::error::{MobError, MobResult};
use crate::model::{
    FailureLedgerEntry, MobEvent, MobRun, MobRunFilter, MobRunStatus, MobSpec, MobSpecRevision,
    NewMobEvent, StepLedgerEntry,
};
use async_trait::async_trait;
use chrono::Utc;
use redb::{Database, ReadableTable, TableDefinition};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

const SPECS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("mob_specs");
const RUNS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("mob_runs");
const EVENTS_TABLE: TableDefinition<u64, &[u8]> = TableDefinition::new("mob_events");
const META_TABLE: TableDefinition<&str, u64> = TableDefinition::new("mob_meta");
const META_CURSOR_KEY: &str = "next_cursor";

fn map_db_err(err: impl std::fmt::Display) -> MobError {
    MobError::Store(err.to_string())
}

fn parse_json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> MobResult<T> {
    serde_json::from_slice(bytes).map_err(|err| MobError::Store(err.to_string()))
}

fn encode_json<T: serde::Serialize>(value: &T) -> MobResult<Vec<u8>> {
    serde_json::to_vec(value).map_err(|err| MobError::Store(err.to_string()))
}

pub struct RedbMobSpecStore {
    db: Arc<Database>,
}

impl RedbMobSpecStore {
    pub fn open(path: impl AsRef<Path>) -> MobResult<Self> {
        let db = Database::create(path).map_err(map_db_err)?;
        let write = db.begin_write().map_err(map_db_err)?;
        {
            let _ = write.open_table(SPECS_TABLE).map_err(map_db_err)?;
        }
        write.commit().map_err(map_db_err)?;
        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl MobSpecStore for RedbMobSpecStore {
    async fn put_spec(
        &self,
        spec: MobSpec,
        expected_revision: Option<MobSpecRevision>,
    ) -> MobResult<()> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let write = db.begin_write().map_err(map_db_err)?;
            {
                let mut table = write.open_table(SPECS_TABLE).map_err(map_db_err)?;

                if let Some(expected) = expected_revision {
                    let current = table
                        .get(spec.mob_id.as_str())
                        .map_err(map_db_err)?
                        .map(|raw| parse_json::<MobSpec>(raw.value()))
                        .transpose()?;

                    if current.as_ref().map(|s| s.revision) != Some(expected) {
                        return Err(MobError::SpecRevisionConflict {
                            mob_id: spec.mob_id.to_string(),
                            expected: Some(expected),
                            current: current.map(|s| s.revision),
                        });
                    }
                }

                let payload = encode_json(&spec)?;
                table
                    .insert(spec.mob_id.as_str(), payload.as_slice())
                    .map_err(map_db_err)?;
            }
            write.commit().map_err(map_db_err)?;
            Ok(())
        })
        .await
        .map_err(|err| MobError::Store(err.to_string()))?
    }

    async fn get_spec(&self, mob_id: &str) -> MobResult<Option<MobSpec>> {
        let db = self.db.clone();
        let mob_id = mob_id.to_string();
        tokio::task::spawn_blocking(move || {
            let read = db.begin_read().map_err(map_db_err)?;
            let table = read.open_table(SPECS_TABLE).map_err(map_db_err)?;
            let result = table.get(mob_id.as_str()).map_err(map_db_err)?;
            result
                .map(|value| parse_json::<MobSpec>(value.value()))
                .transpose()
        })
        .await
        .map_err(|err| MobError::Store(err.to_string()))?
    }

    async fn list_specs(&self) -> MobResult<Vec<MobSpec>> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let read = db.begin_read().map_err(map_db_err)?;
            let table = read.open_table(SPECS_TABLE).map_err(map_db_err)?;
            let iter = table.iter().map_err(map_db_err)?;

            let mut specs = Vec::new();
            for row in iter {
                let (_, value) = row.map_err(map_db_err)?;
                specs.push(parse_json::<MobSpec>(value.value())?);
            }
            specs.sort_by(|a, b| a.mob_id.cmp(&b.mob_id));
            Ok(specs)
        })
        .await
        .map_err(|err| MobError::Store(err.to_string()))?
    }

    async fn delete_spec(&self, mob_id: &str) -> MobResult<()> {
        let db = self.db.clone();
        let mob_id = mob_id.to_string();
        tokio::task::spawn_blocking(move || {
            let write = db.begin_write().map_err(map_db_err)?;
            {
                let mut table = write.open_table(SPECS_TABLE).map_err(map_db_err)?;
                let _ = table.remove(mob_id.as_str()).map_err(map_db_err)?;
            }
            write.commit().map_err(map_db_err)?;
            Ok(())
        })
        .await
        .map_err(|err| MobError::Store(err.to_string()))?
    }
}

pub struct RedbMobRunStore {
    db: Arc<Database>,
}

impl RedbMobRunStore {
    pub fn open(path: impl AsRef<Path>) -> MobResult<Self> {
        let db = Database::create(path).map_err(map_db_err)?;
        let write = db.begin_write().map_err(map_db_err)?;
        {
            let _ = write.open_table(RUNS_TABLE).map_err(map_db_err)?;
        }
        write.commit().map_err(map_db_err)?;
        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl MobRunStore for RedbMobRunStore {
    async fn create_run(&self, run: MobRun) -> MobResult<()> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let write = db.begin_write().map_err(map_db_err)?;
            {
                let mut table = write.open_table(RUNS_TABLE).map_err(map_db_err)?;
                if table.get(run.run_id.as_str()).map_err(map_db_err)?.is_some() {
                    return Err(MobError::Store(format!("run '{}' already exists", run.run_id)));
                }
                let payload = encode_json(&run)?;
                table
                    .insert(run.run_id.as_str(), payload.as_slice())
                    .map_err(map_db_err)?;
            }
            write.commit().map_err(map_db_err)?;
            Ok(())
        })
        .await
        .map_err(|err| MobError::Store(err.to_string()))?
    }

    async fn cas_run_status(
        &self,
        run_id: &str,
        expected: MobRunStatus,
        next: MobRunStatus,
    ) -> MobResult<bool> {
        let db = self.db.clone();
        let run_id = run_id.to_string();
        tokio::task::spawn_blocking(move || {
            let write = db.begin_write().map_err(map_db_err)?;
            let mut changed = false;
            {
                let mut table = write.open_table(RUNS_TABLE).map_err(map_db_err)?;
                let current_bytes = table
                    .get(run_id.as_str())
                    .map_err(map_db_err)?
                    .ok_or_else(|| MobError::RunNotFound {
                        run_id: run_id.clone(),
                    })?
                    .value()
                    .to_vec();
                let mut run = parse_json::<MobRun>(&current_bytes)?;
                if run.status == expected {
                    run.status = next;
                    run.updated_at = Utc::now();
                    if matches!(next, MobRunStatus::Completed | MobRunStatus::Failed | MobRunStatus::Canceled) {
                        run.completed_at = Some(run.updated_at);
                    }
                    let payload = encode_json(&run)?;
                    table
                        .insert(run_id.as_str(), payload.as_slice())
                        .map_err(map_db_err)?;
                    changed = true;
                }
            }
            write.commit().map_err(map_db_err)?;
            Ok(changed)
        })
        .await
        .map_err(|err| MobError::Store(err.to_string()))?
    }

    async fn append_step_entry(&self, run_id: &str, entry: StepLedgerEntry) -> MobResult<()> {
        let db = self.db.clone();
        let run_id = run_id.to_string();
        tokio::task::spawn_blocking(move || {
            let write = db.begin_write().map_err(map_db_err)?;
            {
                let mut table = write.open_table(RUNS_TABLE).map_err(map_db_err)?;
                let current_bytes = table
                    .get(run_id.as_str())
                    .map_err(map_db_err)?
                    .ok_or_else(|| MobError::RunNotFound {
                        run_id: run_id.clone(),
                    })?
                    .value()
                    .to_vec();
                let mut run = parse_json::<MobRun>(&current_bytes)?;
                run.step_ledger.push(entry);
                run.updated_at = Utc::now();
                let payload = encode_json(&run)?;
                table
                    .insert(run_id.as_str(), payload.as_slice())
                    .map_err(map_db_err)?;
            }
            write.commit().map_err(map_db_err)?;
            Ok(())
        })
        .await
        .map_err(|err| MobError::Store(err.to_string()))?
    }

    async fn append_step_entry_if_absent(
        &self,
        run_id: &str,
        logical_key: &str,
        entry: StepLedgerEntry,
    ) -> MobResult<bool> {
        let db = self.db.clone();
        let run_id = run_id.to_string();
        let logical_key = logical_key.to_string();
        tokio::task::spawn_blocking(move || {
            let write = db.begin_write().map_err(map_db_err)?;
            let mut appended = false;
            {
                let mut table = write.open_table(RUNS_TABLE).map_err(map_db_err)?;
                let current_bytes = table
                    .get(run_id.as_str())
                    .map_err(map_db_err)?
                    .ok_or_else(|| MobError::RunNotFound {
                        run_id: run_id.clone(),
                    })?
                    .value()
                    .to_vec();
                let mut run = parse_json::<MobRun>(&current_bytes)?;
                let exists = run
                    .step_ledger
                    .iter()
                    .any(|item| item.logical_key == logical_key && !item.logical_key.is_empty());
                if !exists {
                    run.step_ledger.push(entry);
                    run.updated_at = Utc::now();
                    let payload = encode_json(&run)?;
                    table
                        .insert(run_id.as_str(), payload.as_slice())
                        .map_err(map_db_err)?;
                    appended = true;
                }
            }
            write.commit().map_err(map_db_err)?;
            Ok(appended)
        })
        .await
        .map_err(|err| MobError::Store(err.to_string()))?
    }

    async fn append_failure_entry(
        &self,
        run_id: &str,
        entry: FailureLedgerEntry,
    ) -> MobResult<()> {
        let db = self.db.clone();
        let run_id = run_id.to_string();
        tokio::task::spawn_blocking(move || {
            let write = db.begin_write().map_err(map_db_err)?;
            {
                let mut table = write.open_table(RUNS_TABLE).map_err(map_db_err)?;
                let current_bytes = table
                    .get(run_id.as_str())
                    .map_err(map_db_err)?
                    .ok_or_else(|| MobError::RunNotFound {
                        run_id: run_id.clone(),
                    })?
                    .value()
                    .to_vec();
                let mut run = parse_json::<MobRun>(&current_bytes)?;
                run.failure_ledger.push(entry);
                run.updated_at = Utc::now();
                let payload = encode_json(&run)?;
                table
                    .insert(run_id.as_str(), payload.as_slice())
                    .map_err(map_db_err)?;
            }
            write.commit().map_err(map_db_err)?;
            Ok(())
        })
        .await
        .map_err(|err| MobError::Store(err.to_string()))?
    }

    async fn put_step_output(
        &self,
        run_id: &str,
        step_id: &str,
        output: serde_json::Value,
    ) -> MobResult<()> {
        let db = self.db.clone();
        let run_id = run_id.to_string();
        let step_id = step_id.to_string();
        tokio::task::spawn_blocking(move || {
            let write = db.begin_write().map_err(map_db_err)?;
            {
                let mut table = write.open_table(RUNS_TABLE).map_err(map_db_err)?;
                let current_bytes = table
                    .get(run_id.as_str())
                    .map_err(map_db_err)?
                    .ok_or_else(|| MobError::RunNotFound {
                        run_id: run_id.clone(),
                    })?
                    .value()
                    .to_vec();
                let mut run = parse_json::<MobRun>(&current_bytes)?;
                run.step_outputs.insert(step_id.into(), output);
                run.updated_at = Utc::now();
                let payload = encode_json(&run)?;
                table
                    .insert(run_id.as_str(), payload.as_slice())
                    .map_err(map_db_err)?;
            }
            write.commit().map_err(map_db_err)?;
            Ok(())
        })
        .await
        .map_err(|err| MobError::Store(err.to_string()))?
    }

    async fn get_run(&self, run_id: &str) -> MobResult<Option<MobRun>> {
        let db = self.db.clone();
        let run_id = run_id.to_string();
        tokio::task::spawn_blocking(move || {
            let read = db.begin_read().map_err(map_db_err)?;
            let table = read.open_table(RUNS_TABLE).map_err(map_db_err)?;
            let result = table.get(run_id.as_str()).map_err(map_db_err)?;
            result
                .map(|value| parse_json::<MobRun>(value.value()))
                .transpose()
        })
        .await
        .map_err(|err| MobError::Store(err.to_string()))?
    }

    async fn list_runs(&self, filter: MobRunFilter) -> MobResult<Vec<MobRun>> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let read = db.begin_read().map_err(map_db_err)?;
            let table = read.open_table(RUNS_TABLE).map_err(map_db_err)?;
            let iter = table.iter().map_err(map_db_err)?;

            let mut runs = Vec::new();
            for row in iter {
                let (_, value) = row.map_err(map_db_err)?;
                runs.push(parse_json::<MobRun>(value.value())?);
            }

            if let Some(status) = filter.status {
                runs.retain(|run| run.status == status);
            }
            if let Some(mob_id) = filter.mob_id {
                runs.retain(|run| run.mob_id == mob_id);
            }
            if let Some(flow_id) = filter.flow_id {
                runs.retain(|run| run.flow_id == flow_id);
            }

            runs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            let offset = filter.offset.unwrap_or(0);
            let limit = filter.limit.unwrap_or(50);
            Ok(runs.into_iter().skip(offset).take(limit).collect())
        })
        .await
        .map_err(|err| MobError::Store(err.to_string()))?
    }

    async fn put_run(&self, run: MobRun) -> MobResult<()> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let write = db.begin_write().map_err(map_db_err)?;
            {
                let mut table = write.open_table(RUNS_TABLE).map_err(map_db_err)?;
                let payload = encode_json(&run)?;
                table
                    .insert(run.run_id.as_str(), payload.as_slice())
                    .map_err(map_db_err)?;
            }
            write.commit().map_err(map_db_err)?;
            Ok(())
        })
        .await
        .map_err(|err| MobError::Store(err.to_string()))?
    }
}

pub struct RedbMobEventStore {
    db: Arc<Database>,
}

impl RedbMobEventStore {
    pub fn open(path: impl AsRef<Path>) -> MobResult<Self> {
        let db = Database::create(path).map_err(map_db_err)?;
        let write = db.begin_write().map_err(map_db_err)?;
        {
            let _ = write.open_table(EVENTS_TABLE).map_err(map_db_err)?;
            let mut meta = write.open_table(META_TABLE).map_err(map_db_err)?;
            if meta.get(META_CURSOR_KEY).map_err(map_db_err)?.is_none() {
                meta.insert(META_CURSOR_KEY, 1).map_err(map_db_err)?;
            }
        }
        write.commit().map_err(map_db_err)?;
        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl MobEventStore for RedbMobEventStore {
    async fn append_event(&self, event: NewMobEvent) -> MobResult<u64> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let write = db.begin_write().map_err(map_db_err)?;
            let cursor;
            {
                let mut events = write.open_table(EVENTS_TABLE).map_err(map_db_err)?;
                let mut meta = write.open_table(META_TABLE).map_err(map_db_err)?;
                cursor = meta
                    .get(META_CURSOR_KEY)
                    .map_err(map_db_err)?
                    .map(|v| v.value())
                    .unwrap_or(1);
                let payload = encode_json(&MobEvent {
                    cursor,
                    timestamp: event.timestamp,
                    category: event.category,
                    mob_id: event.mob_id,
                    run_id: event.run_id,
                    flow_id: event.flow_id,
                    step_id: event.step_id,
                    meerkat_id: event.meerkat_id,
                    kind: event.kind,
                    payload: event.payload,
                })?;
                events.insert(cursor, payload.as_slice()).map_err(map_db_err)?;
                meta.insert(META_CURSOR_KEY, cursor + 1).map_err(map_db_err)?;
            }
            write.commit().map_err(map_db_err)?;
            Ok(cursor)
        })
        .await
        .map_err(|err| MobError::Store(err.to_string()))?
    }

    async fn poll_events(
        &self,
        cursor: Option<u64>,
        limit: Option<usize>,
    ) -> MobResult<(u64, Vec<MobEvent>)> {
        let db = self.db.clone();
        let start = cursor.unwrap_or(0);
        let max = limit.unwrap_or(100);

        tokio::task::spawn_blocking(move || {
            let read = db.begin_read().map_err(map_db_err)?;
            let events = read.open_table(EVENTS_TABLE).map_err(map_db_err)?;
            let range = events.range((start + 1)..=u64::MAX).map_err(map_db_err)?;

            let mut out = Vec::new();
            let mut next_cursor = start;
            for row in range {
                let (key, value) = row.map_err(map_db_err)?;
                let event = parse_json::<MobEvent>(value.value())?;
                next_cursor = key.value();
                out.push(event);
                if out.len() >= max {
                    break;
                }
            }

            Ok((next_cursor, out))
        })
        .await
        .map_err(|err| MobError::Store(err.to_string()))?
    }

    async fn prune_events(&self, ttl_secs_by_category: &BTreeMap<String, u64>) -> MobResult<usize> {
        if ttl_secs_by_category.is_empty() {
            return Ok(0);
        }

        let db = self.db.clone();
        let ttl = ttl_secs_by_category.clone();
        tokio::task::spawn_blocking(move || {
            let write = db.begin_write().map_err(map_db_err)?;
            let mut removed = 0usize;
            {
                let mut events = write.open_table(EVENTS_TABLE).map_err(map_db_err)?;
                let iter = events.iter().map_err(map_db_err)?;
                let mut keys_to_remove = Vec::new();
                let now = Utc::now();
                for row in iter {
                    let (key, value) = row.map_err(map_db_err)?;
                    let event = parse_json::<MobEvent>(value.value())?;
                    let category = serde_json::to_string(&event.category)
                        .map_err(|err| MobError::Store(err.to_string()))?;
                    let normalized = category.trim_matches('"').to_string();
                    if let Some(ttl_secs) = ttl.get(&normalized) {
                        let age = now.signed_duration_since(event.timestamp).num_seconds();
                        if age >= *ttl_secs as i64 {
                            keys_to_remove.push(key.value());
                        }
                    }
                }

                for key in keys_to_remove {
                    let _ = events.remove(key).map_err(map_db_err)?;
                    removed += 1;
                }
            }
            write.commit().map_err(map_db_err)?;
            Ok(removed)
        })
        .await
        .map_err(|err| MobError::Store(err.to_string()))?
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::model::{MobEventCategory, MobEventKind, NewMobEvent, RetentionSpec, TopologySpec};
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn spec_store_roundtrip() {
        let dir = TempDir::new().unwrap();
        let store = RedbMobSpecStore::open(dir.path().join("specs.redb")).unwrap();

        let spec = MobSpec {
            mob_id: "invoice".into(),
            revision: 1,
            roles: BTreeMap::new(),
            topology: TopologySpec::default(),
            flows: BTreeMap::new(),
            prompts: BTreeMap::new(),
            schemas: BTreeMap::new(),
            tool_bundles: BTreeMap::new(),
            resolvers: BTreeMap::new(),
            supervisor: crate::model::SupervisorSpec::default(),
            limits: crate::model::LimitsSpec::default(),
            retention: RetentionSpec::default(),
            applied_at: Utc::now(),
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            store.put_spec(spec.clone(), None).await.unwrap();
            let loaded = store.get_spec("invoice").await.unwrap().unwrap();
            assert_eq!(loaded.mob_id, spec.mob_id);
            assert_eq!(loaded.revision, 1);
        });
    }

    #[test]
    fn run_store_roundtrip() {
        let dir = TempDir::new().unwrap();
        let store = RedbMobRunStore::open(dir.path().join("runs.redb")).unwrap();

        let run = MobRun::new(
            "run-1".into(),
            "invoice".into(),
            "triage".into(),
            1,
            json!({}),
        );

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            store.create_run(run).await.unwrap();
            let ok = store
                .cas_run_status("run-1", MobRunStatus::Pending, MobRunStatus::Running)
                .await
                .unwrap();
            assert!(ok);
            let loaded = store.get_run("run-1").await.unwrap().unwrap();
            assert_eq!(loaded.status, MobRunStatus::Running);
        });
    }

    #[test]
    fn event_store_cursor_monotonic() {
        let dir = TempDir::new().unwrap();
        let store = RedbMobEventStore::open(dir.path().join("events.redb")).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let c1 = store
                .append_event(NewMobEvent {
                    timestamp: Utc::now(),
                    category: MobEventCategory::Flow,
                    mob_id: "invoice".into(),
                    run_id: None,
                    flow_id: None,
                    step_id: None,
                    meerkat_id: None,
                    kind: MobEventKind::RunActivated,
                    payload: json!({}),
                })
                .await
                .unwrap();
            let c2 = store
                .append_event(NewMobEvent {
                    timestamp: Utc::now(),
                    category: MobEventCategory::Flow,
                    mob_id: "invoice".into(),
                    run_id: None,
                    flow_id: None,
                    step_id: None,
                    meerkat_id: None,
                    kind: MobEventKind::RunCompleted,
                    payload: json!({}),
                })
                .await
                .unwrap();
            assert!(c2 > c1);

            let (cursor, events) = store.poll_events(None, Some(10)).await.unwrap();
            assert_eq!(events.len(), 2);
            assert_eq!(cursor, c2);
        });
    }
}
