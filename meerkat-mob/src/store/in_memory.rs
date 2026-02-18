use super::{MobEventStore, MobRunStore, MobSpecStore};
use crate::error::{MobError, MobResult};
use crate::model::{
    FailureLedgerEntry, MobEvent, MobRun, MobRunFilter, MobRunStatus, MobSpec, MobSpecRevision,
    StepLedgerEntry,
};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;

#[derive(Default)]
pub struct InMemoryMobSpecStore {
    specs: RwLock<HashMap<String, MobSpec>>,
}

#[async_trait]
impl MobSpecStore for InMemoryMobSpecStore {
    async fn put_spec(
        &self,
        spec: MobSpec,
        expected_revision: Option<MobSpecRevision>,
    ) -> MobResult<()> {
        let mut specs = self.specs.write().await;
        let current = specs.get(&spec.mob_id).map(|existing| existing.revision);

        if let Some(expected) = expected_revision
            && current != Some(expected)
        {
            return Err(MobError::SpecRevisionConflict {
                mob_id: spec.mob_id,
                expected: Some(expected),
                current,
            });
        }

        specs.insert(spec.mob_id.clone(), spec);
        Ok(())
    }

    async fn get_spec(&self, mob_id: &str) -> MobResult<Option<MobSpec>> {
        Ok(self.specs.read().await.get(mob_id).cloned())
    }

    async fn list_specs(&self) -> MobResult<Vec<MobSpec>> {
        let mut specs: Vec<MobSpec> = self.specs.read().await.values().cloned().collect();
        specs.sort_by(|a, b| a.mob_id.cmp(&b.mob_id));
        Ok(specs)
    }

    async fn delete_spec(&self, mob_id: &str) -> MobResult<()> {
        self.specs.write().await.remove(mob_id);
        Ok(())
    }
}

#[derive(Default)]
pub struct InMemoryMobRunStore {
    runs: RwLock<HashMap<String, MobRun>>,
}

#[async_trait]
impl MobRunStore for InMemoryMobRunStore {
    async fn create_run(&self, run: MobRun) -> MobResult<()> {
        let mut runs = self.runs.write().await;
        if runs.contains_key(&run.run_id) {
            return Err(MobError::Store(format!(
                "run '{}' already exists",
                run.run_id
            )));
        }
        runs.insert(run.run_id.clone(), run);
        Ok(())
    }

    async fn cas_run_status(
        &self,
        run_id: &str,
        expected: MobRunStatus,
        next: MobRunStatus,
    ) -> MobResult<bool> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobError::RunNotFound {
                run_id: run_id.to_string(),
            })?;

        if run.status != expected {
            return Ok(false);
        }

        run.status = next;
        run.updated_at = Utc::now();
        if matches!(next, MobRunStatus::Completed | MobRunStatus::Failed | MobRunStatus::Canceled) {
            run.completed_at = Some(run.updated_at);
        }
        Ok(true)
    }

    async fn append_step_entry(&self, run_id: &str, entry: StepLedgerEntry) -> MobResult<()> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobError::RunNotFound {
                run_id: run_id.to_string(),
            })?;
        run.step_ledger.push(entry);
        run.updated_at = Utc::now();
        Ok(())
    }

    async fn append_failure_entry(
        &self,
        run_id: &str,
        entry: FailureLedgerEntry,
    ) -> MobResult<()> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobError::RunNotFound {
                run_id: run_id.to_string(),
            })?;
        run.failure_ledger.push(entry);
        run.updated_at = Utc::now();
        Ok(())
    }

    async fn put_step_output(
        &self,
        run_id: &str,
        step_id: &str,
        output: serde_json::Value,
    ) -> MobResult<()> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobError::RunNotFound {
                run_id: run_id.to_string(),
            })?;
        run.step_outputs.insert(step_id.to_string(), output);
        run.updated_at = Utc::now();
        Ok(())
    }

    async fn get_run(&self, run_id: &str) -> MobResult<Option<MobRun>> {
        Ok(self.runs.read().await.get(run_id).cloned())
    }

    async fn list_runs(&self, filter: MobRunFilter) -> MobResult<Vec<MobRun>> {
        let runs = self.runs.read().await;
        let mut items: Vec<MobRun> = runs.values().cloned().collect();

        if let Some(status) = filter.status {
            items.retain(|run| run.status == status);
        }
        if let Some(mob_id) = filter.mob_id {
            items.retain(|run| run.mob_id == mob_id);
        }
        if let Some(flow_id) = filter.flow_id {
            items.retain(|run| run.flow_id == flow_id);
        }

        items.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        let offset = filter.offset.unwrap_or(0);
        let limit = filter.limit.unwrap_or(50);

        let paged = items.into_iter().skip(offset).take(limit).collect();
        Ok(paged)
    }

    async fn put_run(&self, run: MobRun) -> MobResult<()> {
        self.runs.write().await.insert(run.run_id.clone(), run);
        Ok(())
    }
}

#[derive(Default)]
pub struct InMemoryMobEventStore {
    events: RwLock<Vec<MobEvent>>,
    cursor: AtomicU64,
}

#[async_trait]
impl MobEventStore for InMemoryMobEventStore {
    async fn append_event(&self, mut event: MobEvent) -> MobResult<u64> {
        let cursor = self.cursor.fetch_add(1, Ordering::SeqCst) + 1;
        event.cursor = cursor;
        self.events.write().await.push(event);
        Ok(cursor)
    }

    async fn poll_events(
        &self,
        cursor: Option<u64>,
        limit: Option<usize>,
    ) -> MobResult<(u64, Vec<MobEvent>)> {
        let start = cursor.unwrap_or(0);
        let max = limit.unwrap_or(100);
        let events = self.events.read().await;

        let mut selected = Vec::new();
        let mut next_cursor = start;

        for event in events.iter() {
            if event.cursor > start {
                selected.push(event.clone());
                next_cursor = event.cursor;
                if selected.len() >= max {
                    break;
                }
            }
        }

        Ok((next_cursor, selected))
    }

    async fn prune_events(&self, ttl_secs_by_category: &BTreeMap<String, u64>) -> MobResult<usize> {
        if ttl_secs_by_category.is_empty() {
            return Ok(0);
        }

        let now = Utc::now();
        let mut events = self.events.write().await;
        let before = events.len();

        events.retain(|event| {
            let category = serde_json::to_string(&event.category).unwrap_or_default();
            let normalized = category.trim_matches('"').to_string();
            let ttl = ttl_secs_by_category.get(&normalized);
            match ttl {
                Some(secs) => {
                    let age = now.signed_duration_since(event.timestamp).num_seconds();
                    age < *secs as i64
                }
                None => true,
            }
        });

        Ok(before.saturating_sub(events.len()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::model::{
        MobEventCategory, MobEventKind, MobRun, MobRunStatus, RetentionSpec, StepRunStatus,
    };
    use serde_json::json;

    fn sample_run(run_id: &str) -> MobRun {
        MobRun::new(
            run_id.to_string(),
            "invoice".to_string(),
            "triage".to_string(),
            1,
            json!({}),
        )
    }

    #[tokio::test]
    async fn spec_store_cas_revision() {
        let store = InMemoryMobSpecStore::default();
        let spec = MobSpec {
            mob_id: "invoice".into(),
            revision: 1,
            roles: BTreeMap::new(),
            topology: crate::model::TopologySpec::default(),
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
        store.put_spec(spec.clone(), None).await.unwrap();

        let mut v2 = spec;
        v2.revision = 2;
        let err = store.put_spec(v2.clone(), Some(2)).await.unwrap_err();
        assert!(matches!(err, MobError::SpecRevisionConflict { .. }));

        store.put_spec(v2, Some(1)).await.unwrap();
    }

    #[tokio::test]
    async fn run_store_cas_status_updates() {
        let store = InMemoryMobRunStore::default();
        store.create_run(sample_run("r1")).await.unwrap();

        let ok = store
            .cas_run_status("r1", MobRunStatus::Pending, MobRunStatus::Running)
            .await
            .unwrap();
        assert!(ok);

        let no = store
            .cas_run_status("r1", MobRunStatus::Pending, MobRunStatus::Completed)
            .await
            .unwrap();
        assert!(!no);

        let run = store.get_run("r1").await.unwrap().unwrap();
        assert_eq!(run.status, MobRunStatus::Running);
    }

    #[tokio::test]
    async fn run_store_append_ledgers() {
        let store = InMemoryMobRunStore::default();
        store.create_run(sample_run("r2")).await.unwrap();

        store
            .append_step_entry(
                "r2",
                StepLedgerEntry {
                    timestamp: Utc::now(),
                    step_id: "s1".into(),
                    target_meerkat: "reviewer/1".into(),
                    attempt: 1,
                    status: StepRunStatus::Completed,
                    detail: json!({"result":"ok"}),
                },
            )
            .await
            .unwrap();

        store
            .append_failure_entry(
                "r2",
                FailureLedgerEntry {
                    timestamp: Utc::now(),
                    step_id: Some("s2".into()),
                    target_meerkat: Some("reviewer/2".into()),
                    error: "failed".into(),
                },
            )
            .await
            .unwrap();

        let run = store.get_run("r2").await.unwrap().unwrap();
        assert_eq!(run.step_ledger.len(), 1);
        assert_eq!(run.failure_ledger.len(), 1);
    }

    #[tokio::test]
    async fn event_store_cursor_polling() {
        let store = InMemoryMobEventStore::default();

        for i in 0..3 {
            store
                .append_event(MobEvent {
                    cursor: 0,
                    timestamp: Utc::now(),
                    category: MobEventCategory::Flow,
                    mob_id: "invoice".into(),
                    run_id: Some("r1".into()),
                    flow_id: Some("triage".into()),
                    step_id: Some(format!("s{i}")),
                    meerkat_id: None,
                    kind: MobEventKind::StepCompleted,
                    payload: json!({"i": i}),
                })
                .await
                .unwrap();
        }

        let (cursor, first) = store.poll_events(None, Some(2)).await.unwrap();
        assert_eq!(first.len(), 2);
        assert_eq!(cursor, 2);

        let (cursor2, second) = store.poll_events(Some(cursor), Some(10)).await.unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(cursor2, 3);
    }
}
