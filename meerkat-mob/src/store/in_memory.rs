//! In-memory store implementations.

use super::{MobEventStore, MobRunStore, MobSpecStore};
use crate::definition::MobDefinition;
use crate::error::MobError;
use crate::event::{MobEvent, NewMobEvent};
use crate::ids::{FlowId, MobId, RunId, StepId};
use crate::run::{FailureLedgerEntry, MobRun, MobRunStatus, StepLedgerEntry};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// In-memory event store for tests and ephemeral mobs.
#[derive(Debug, Default)]
pub struct InMemoryMobEventStore {
    events: Arc<RwLock<Vec<MobEvent>>>,
}

impl InMemoryMobEventStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl MobEventStore for InMemoryMobEventStore {
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobError> {
        let mut events = self.events.write().await;
        let cursor = events.last().map_or(1, |existing| existing.cursor + 1);
        let stored = MobEvent {
            cursor,
            timestamp: event.timestamp.unwrap_or_else(Utc::now),
            mob_id: event.mob_id,
            kind: event.kind,
        };
        events.push(stored.clone());
        Ok(stored)
    }

    async fn append_batch(&self, batch: Vec<NewMobEvent>) -> Result<Vec<MobEvent>, MobError> {
        let mut events = self.events.write().await;
        let mut results = Vec::with_capacity(batch.len());
        for event in batch {
            let cursor = events.last().map_or(1, |existing| existing.cursor + 1);
            let stored = MobEvent {
                cursor,
                timestamp: event.timestamp.unwrap_or_else(Utc::now),
                mob_id: event.mob_id,
                kind: event.kind,
            };
            events.push(stored.clone());
            results.push(stored);
        }
        Ok(results)
    }

    async fn poll(&self, after_cursor: u64, limit: usize) -> Result<Vec<MobEvent>, MobError> {
        let events = self.events.read().await;
        Ok(events
            .iter()
            .filter(|event| event.cursor > after_cursor)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobError> {
        Ok(self.events.read().await.clone())
    }

    async fn clear(&self) -> Result<(), MobError> {
        self.events.write().await.clear();
        Ok(())
    }

    async fn prune(&self, older_than: DateTime<Utc>) -> Result<u64, MobError> {
        let mut events = self.events.write().await;
        let before = events.len();
        events.retain(|event| event.timestamp >= older_than);
        Ok((before - events.len()) as u64)
    }
}

/// In-memory run store.
#[derive(Debug, Default)]
pub struct InMemoryMobRunStore {
    runs: Arc<RwLock<IndexMap<RunId, MobRun>>>,
}

impl InMemoryMobRunStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl MobRunStore for InMemoryMobRunStore {
    async fn create_run(&self, run: MobRun) -> Result<(), MobError> {
        let mut runs = self.runs.write().await;
        if runs.contains_key(&run.run_id) {
            return Err(MobError::Internal(format!(
                "run already exists: {}",
                run.run_id
            )));
        }
        runs.insert(run.run_id.clone(), run);
        Ok(())
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, MobError> {
        Ok(self.runs.read().await.get(run_id).cloned())
    }

    async fn list_runs(
        &self,
        mob_id: &MobId,
        flow_id: Option<&FlowId>,
    ) -> Result<Vec<MobRun>, MobError> {
        Ok(self
            .runs
            .read()
            .await
            .values()
            .filter(|run| {
                run.mob_id == *mob_id
                    && flow_id.is_none_or(|expected_flow_id| run.flow_id == *expected_flow_id)
            })
            .cloned()
            .collect())
    }

    async fn cas_run_status(
        &self,
        run_id: &RunId,
        expected: MobRunStatus,
        next: MobRunStatus,
    ) -> Result<bool, MobError> {
        let mut runs = self.runs.write().await;
        let Some(run) = runs.get_mut(run_id) else {
            return Ok(false);
        };
        if run.status != expected || run.status.is_terminal() {
            return Ok(false);
        }
        let terminal = next.is_terminal();
        run.status = next;
        if terminal && run.completed_at.is_none() {
            run.completed_at = Some(Utc::now());
        }
        Ok(true)
    }

    async fn append_step_entry(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
    ) -> Result<(), MobError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        run.step_ledger.push(entry);
        Ok(())
    }

    async fn append_step_entry_if_absent(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
    ) -> Result<bool, MobError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        let is_duplicate = run.step_ledger.iter().any(|existing| {
            existing.step_id == entry.step_id
                && existing.meerkat_id == entry.meerkat_id
                && existing.status == entry.status
        });
        if is_duplicate {
            return Ok(false);
        }
        run.step_ledger.push(entry);
        Ok(true)
    }

    async fn put_step_output(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        output: serde_json::Value,
    ) -> Result<(), MobError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        if let Some(entry) = run
            .step_ledger
            .iter_mut()
            .rev()
            .find(|entry| &entry.step_id == step_id)
        {
            entry.output = Some(output);
            return Ok(());
        }
        Err(MobError::Internal(format!(
            "cannot set output for unknown step '{}' in run '{}'",
            step_id, run_id
        )))
    }

    async fn append_failure_entry(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
    ) -> Result<(), MobError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        run.failure_ledger.push(entry);
        Ok(())
    }
}

/// In-memory spec store with revision CAS semantics.
#[derive(Debug, Default)]
pub struct InMemoryMobSpecStore {
    specs: Arc<RwLock<BTreeMap<MobId, (MobDefinition, u64)>>>,
}

impl InMemoryMobSpecStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl MobSpecStore for InMemoryMobSpecStore {
    async fn put_spec(
        &self,
        mob_id: &MobId,
        definition: &MobDefinition,
        revision: Option<u64>,
    ) -> Result<u64, MobError> {
        let mut specs = self.specs.write().await;
        let current_revision = specs.get(mob_id).map_or(0, |(_, rev)| *rev);
        if let Some(expected) = revision
            && expected != current_revision
        {
            return Err(MobError::SpecRevisionConflict {
                mob_id: mob_id.clone(),
                expected: revision,
                actual: current_revision,
            });
        }

        let next_revision = current_revision + 1;
        specs.insert(mob_id.clone(), (definition.clone(), next_revision));
        Ok(next_revision)
    }

    async fn get_spec(&self, mob_id: &MobId) -> Result<Option<(MobDefinition, u64)>, MobError> {
        Ok(self.specs.read().await.get(mob_id).cloned())
    }

    async fn list_specs(&self) -> Result<Vec<MobId>, MobError> {
        Ok(self.specs.read().await.keys().cloned().collect())
    }

    async fn delete_spec(&self, mob_id: &MobId, revision: Option<u64>) -> Result<bool, MobError> {
        let mut specs = self.specs.write().await;
        let Some((_, current_revision)) = specs.get(mob_id) else {
            return Ok(false);
        };

        if let Some(expected) = revision
            && expected != *current_revision
        {
            return Ok(false);
        }

        specs.remove(mob_id);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{BackendConfig, MobDefinition, WiringRules};
    use crate::event::MobEventKind;
    use crate::ids::{MeerkatId, ProfileName};
    use crate::profile::{Profile, ToolConfig};
    use crate::run::StepRunStatus;
    use futures::future::join_all;
    use std::collections::BTreeMap;

    fn sample_definition() -> MobDefinition {
        let mut profiles = BTreeMap::new();
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
            },
        );
        MobDefinition {
            id: MobId::from("mob"),
            orchestrator: None,
            profiles,
            mcp_servers: BTreeMap::new(),
            wiring: WiringRules::default(),
            skills: BTreeMap::new(),
            backend: BackendConfig::default(),
            flows: BTreeMap::new(),
            topology: None,
            supervisor: None,
            limits: None,
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
    async fn test_event_store_prune() {
        let store = InMemoryMobEventStore::new();
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

        let removed = store
            .prune(now - chrono::Duration::minutes(1))
            .await
            .unwrap();
        assert_eq!(removed, 1);
        assert_eq!(store.replay_all().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_run_store_status_cas_single_winner() {
        let store = Arc::new(InMemoryMobRunStore::new());
        let run = sample_run(MobRunStatus::Running);
        let run_id = run.run_id.clone();
        store.create_run(run).await.unwrap();

        let tasks = (0..10).map(|_| {
            let store = Arc::clone(&store);
            let run_id = run_id.clone();
            tokio::spawn(async move {
                store
                    .cas_run_status(&run_id, MobRunStatus::Running, MobRunStatus::Completed)
                    .await
                    .unwrap()
            })
        });
        let outcomes = join_all(tasks).await;
        let wins = outcomes
            .into_iter()
            .filter_map(|result| result.ok())
            .filter(|cas_result| *cas_result)
            .count();
        assert_eq!(wins, 1);

        let stored = store.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(stored.status, MobRunStatus::Completed);
        assert!(stored.completed_at.is_some());
    }

    #[tokio::test]
    async fn test_run_store_step_dedup_and_ledgers() {
        let store = InMemoryMobRunStore::new();
        let run = sample_run(MobRunStatus::Running);
        let run_id = run.run_id.clone();
        store.create_run(run).await.unwrap();

        let step_entry = StepLedgerEntry {
            step_id: StepId::from("s1"),
            meerkat_id: MeerkatId::from("worker-1"),
            status: StepRunStatus::Dispatched,
            output: None,
            timestamp: Utc::now(),
        };

        assert!(
            store
                .append_step_entry_if_absent(&run_id, step_entry.clone())
                .await
                .unwrap()
        );
        assert!(
            !store
                .append_step_entry_if_absent(&run_id, step_entry)
                .await
                .unwrap()
        );

        store
            .put_step_output(&run_id, &StepId::from("s1"), serde_json::json!({"ok":true}))
            .await
            .unwrap();
        store
            .append_failure_entry(
                &run_id,
                FailureLedgerEntry {
                    step_id: StepId::from("s1"),
                    reason: "failed".to_string(),
                    timestamp: Utc::now(),
                },
            )
            .await
            .unwrap();

        let stored = store.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(stored.step_ledger.len(), 1);
        assert_eq!(stored.failure_ledger.len(), 1);
        assert_eq!(
            stored.step_ledger[0].output,
            Some(serde_json::json!({"ok":true}))
        );
    }

    #[tokio::test]
    async fn test_spec_store_revision_conflict_behavior() {
        let store = InMemoryMobSpecStore::new();
        let definition = sample_definition();
        let mob_id = MobId::from("mob");

        let rev1 = store.put_spec(&mob_id, &definition, None).await.unwrap();
        assert_eq!(rev1, 1);

        let rev2 = store
            .put_spec(&mob_id, &definition, Some(rev1))
            .await
            .unwrap();
        assert_eq!(rev2, 2);

        let conflict = store
            .put_spec(&mob_id, &definition, Some(1))
            .await
            .unwrap_err();
        assert!(matches!(
            conflict,
            MobError::SpecRevisionConflict {
                expected: Some(1),
                actual: 2,
                ..
            }
        ));

        assert!(!store.delete_spec(&mob_id, Some(1)).await.unwrap());
        assert!(store.delete_spec(&mob_id, Some(2)).await.unwrap());
    }
}
