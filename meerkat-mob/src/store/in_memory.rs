//! In-memory store implementations.

use super::realm_profile::{RealmProfileStore, StoredRealmProfile};
use super::{
    ExternalBindingOverlayRecord, MobEventStore, MobRunStore, MobRuntimeMetadataStore,
    MobSpecStore, MobStoreError, SupervisorAuthorityRecord,
};
use crate::definition::MobDefinition;
use crate::event::{MobEvent, NewMobEvent};
use crate::ids::{
    AgentIdentity, FlowId, FrameId, Generation, LoopId, LoopInstanceId, MobId, RunId, StepId,
};
use crate::profile::Profile;
use crate::run::flow_run;
use crate::run::{
    FailureLedgerEntry, FrameSnapshot, LoopIterationLedgerEntry, LoopSnapshot, MobRun,
    MobRunStatus, StepLedgerEntry,
};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};

const EVENT_SUBSCRIPTION_CHANNEL_CAPACITY: usize = 4096;

/// In-memory event store for tests and ephemeral mobs.
#[derive(Debug)]
pub struct InMemoryMobEventStore {
    events: Arc<RwLock<Vec<MobEvent>>>,
    event_tx: broadcast::Sender<MobEvent>,
}

impl Default for InMemoryMobEventStore {
    fn default() -> Self {
        let (event_tx, _event_rx) = broadcast::channel(EVENT_SUBSCRIPTION_CHANNEL_CAPACITY);
        Self {
            events: Arc::new(RwLock::new(Vec::new())),
            event_tx,
        }
    }
}

impl InMemoryMobEventStore {
    pub fn new() -> Self {
        Self::default()
    }
}

type ExternalBindingOverlayMap = BTreeMap<
    (MobId, crate::ids::AgentIdentity, crate::ids::Generation),
    ExternalBindingOverlayRecord,
>;

/// In-memory runtime metadata store for tests and ephemeral mobs.
#[derive(Debug, Default)]
pub struct InMemoryMobRuntimeMetadataStore {
    supervisor_records: Arc<RwLock<BTreeMap<MobId, SupervisorAuthorityRecord>>>,
    external_binding_overlays: Arc<RwLock<ExternalBindingOverlayMap>>,
}

impl InMemoryMobRuntimeMetadataStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobRuntimeMetadataStore for InMemoryMobRuntimeMetadataStore {
    async fn load_supervisor_authority(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<SupervisorAuthorityRecord>, MobStoreError> {
        Ok(self.supervisor_records.read().await.get(mob_id).cloned())
    }

    async fn put_supervisor_authority(
        &self,
        mob_id: &MobId,
        record: &SupervisorAuthorityRecord,
    ) -> Result<(), MobStoreError> {
        self.supervisor_records
            .write()
            .await
            .insert(mob_id.clone(), record.clone());
        Ok(())
    }

    async fn put_supervisor_authority_if_absent(
        &self,
        mob_id: &MobId,
        record: &SupervisorAuthorityRecord,
    ) -> Result<bool, MobStoreError> {
        let mut guard = self.supervisor_records.write().await;
        if guard.contains_key(mob_id) {
            return Ok(false);
        }
        guard.insert(mob_id.clone(), record.clone());
        Ok(true)
    }

    async fn delete_supervisor_authority(&self, mob_id: &MobId) -> Result<(), MobStoreError> {
        self.supervisor_records.write().await.remove(mob_id);
        Ok(())
    }

    async fn list_external_binding_overlays(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<ExternalBindingOverlayRecord>, MobStoreError> {
        Ok(self
            .external_binding_overlays
            .read()
            .await
            .iter()
            .filter(|((stored_mob_id, _, _), _)| stored_mob_id == mob_id)
            .map(|(_, record)| record.clone())
            .collect())
    }

    async fn put_external_binding_overlay_if_absent(
        &self,
        mob_id: &MobId,
        record: &ExternalBindingOverlayRecord,
    ) -> Result<bool, MobStoreError> {
        let key = (
            mob_id.clone(),
            record.agent_identity.clone(),
            record.generation,
        );
        let mut overlays = self.external_binding_overlays.write().await;
        if overlays.contains_key(&key) {
            return Ok(false);
        }
        overlays.insert(key, record.clone());
        Ok(true)
    }

    async fn upsert_external_binding_overlay(
        &self,
        mob_id: &MobId,
        record: &ExternalBindingOverlayRecord,
    ) -> Result<(), MobStoreError> {
        let key = (
            mob_id.clone(),
            record.agent_identity.clone(),
            record.generation,
        );
        self.external_binding_overlays
            .write()
            .await
            .insert(key, record.clone());
        Ok(())
    }

    async fn delete_external_binding_overlay(
        &self,
        mob_id: &MobId,
        agent_identity: &AgentIdentity,
        generation: Generation,
    ) -> Result<(), MobStoreError> {
        self.external_binding_overlays.write().await.remove(&(
            mob_id.clone(),
            agent_identity.clone(),
            generation,
        ));
        Ok(())
    }

    async fn delete_external_binding_overlays(&self, mob_id: &MobId) -> Result<(), MobStoreError> {
        self.external_binding_overlays
            .write()
            .await
            .retain(|(stored_mob_id, _, _), _| stored_mob_id != mob_id);
        Ok(())
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobEventStore for InMemoryMobEventStore {
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobStoreError> {
        let mut events = self.events.write().await;
        let cursor = events.last().map_or(1, |existing| existing.cursor + 1);
        let stored = MobEvent {
            cursor,
            timestamp: event.timestamp.unwrap_or_else(Utc::now),
            mob_id: event.mob_id,
            kind: event.kind,
        };
        events.push(stored.clone());
        drop(events);
        let _ = self.event_tx.send(stored.clone());
        Ok(stored)
    }

    async fn append_batch(&self, batch: Vec<NewMobEvent>) -> Result<Vec<MobEvent>, MobStoreError> {
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
        drop(events);
        for event in &results {
            let _ = self.event_tx.send(event.clone());
        }
        Ok(results)
    }

    async fn poll(&self, after_cursor: u64, limit: usize) -> Result<Vec<MobEvent>, MobStoreError> {
        let events = self.events.read().await;
        Ok(events
            .iter()
            .filter(|event| event.cursor > after_cursor)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobStoreError> {
        Ok(self.events.read().await.clone())
    }

    async fn latest_cursor(&self) -> Result<u64, MobStoreError> {
        Ok(self
            .events
            .read()
            .await
            .last()
            .map_or(0, |event| event.cursor))
    }

    fn subscribe(&self) -> Result<super::MobEventReceiver, MobStoreError> {
        Ok(self.event_tx.subscribe())
    }

    async fn clear(&self) -> Result<(), MobStoreError> {
        self.events.write().await.clear();
        Ok(())
    }

    async fn prune(&self, older_than: DateTime<Utc>) -> Result<u64, MobStoreError> {
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

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobRunStore for InMemoryMobRunStore {
    async fn create_run(&self, run: MobRun) -> Result<(), MobStoreError> {
        let mut runs = self.runs.write().await;
        if runs.contains_key(&run.run_id) {
            return Err(MobStoreError::Internal(format!(
                "run already exists: {}",
                run.run_id
            )));
        }
        runs.insert(run.run_id.clone(), run);
        Ok(())
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, MobStoreError> {
        Ok(self.runs.read().await.get(run_id).cloned())
    }

    async fn list_runs(
        &self,
        mob_id: &MobId,
        flow_id: Option<&FlowId>,
    ) -> Result<Vec<MobRun>, MobStoreError> {
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
    ) -> Result<bool, MobStoreError> {
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

    async fn cas_flow_state(
        &self,
        run_id: &RunId,
        expected: &flow_run::State,
        next: &flow_run::State,
    ) -> Result<bool, MobStoreError> {
        let mut runs = self.runs.write().await;
        let Some(run) = runs.get_mut(run_id) else {
            return Ok(false);
        };
        if &run.flow_state != expected {
            return Ok(false);
        }
        run.flow_state = next.clone();
        Ok(true)
    }

    async fn cas_run_snapshot(
        &self,
        run_id: &RunId,
        expected_status: MobRunStatus,
        expected_flow_state: &flow_run::State,
        next_status: MobRunStatus,
        next_flow_state: &flow_run::State,
    ) -> Result<bool, MobStoreError> {
        let mut runs = self.runs.write().await;
        let Some(run) = runs.get_mut(run_id) else {
            return Ok(false);
        };
        if run.status != expected_status
            || run.status.is_terminal()
            || &run.flow_state != expected_flow_state
        {
            return Ok(false);
        }
        let terminal = next_status.is_terminal();
        run.status = next_status;
        run.flow_state = next_flow_state.clone();
        if terminal && run.completed_at.is_none() {
            run.completed_at = Some(Utc::now());
        }
        Ok(true)
    }

    async fn append_step_entry(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
    ) -> Result<(), MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        run.step_ledger.push(entry);
        Ok(())
    }

    async fn append_step_entry_if_absent(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
    ) -> Result<bool, MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        let is_duplicate = run.step_ledger.iter().any(|existing| {
            existing.step_id == entry.step_id
                && existing.agent_identity == entry.agent_identity
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
    ) -> Result<(), MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        if let Some(entry) = run
            .step_ledger
            .iter_mut()
            .rev()
            .find(|entry| &entry.step_id == step_id)
        {
            entry.output = Some(output);
            return Ok(());
        }
        Err(MobStoreError::Internal(format!(
            "cannot set output for unknown step '{step_id}' in run '{run_id}'"
        )))
    }

    async fn append_failure_entry(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
    ) -> Result<(), MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        run.failure_ledger.push(entry);
        Ok(())
    }

    async fn upsert_loop_snapshot(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        snapshot: LoopSnapshot,
        ledger_entry: Option<LoopIterationLedgerEntry>,
    ) -> Result<(), MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        run.loops.insert(loop_instance_id.clone(), snapshot);
        if let Some(entry) = ledger_entry
            && !run.loop_iteration_ledger.iter().any(|existing| {
                existing.loop_instance_id == entry.loop_instance_id
                    && existing.iteration == entry.iteration
                    && existing.frame_id == entry.frame_id
            })
        {
            run.loop_iteration_ledger.push(entry);
        }
        Ok(())
    }

    async fn cas_frame_state(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        expected: Option<&FrameSnapshot>,
        next: FrameSnapshot,
    ) -> Result<bool, MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        let current = run.frames.get(frame_id);
        match (expected, current) {
            (None, None) => {
                // Insert new frame
                run.frames.insert(frame_id.clone(), next);
                Ok(true)
            }
            (Some(exp), Some(cur)) if exp == cur => {
                run.frames.insert(frame_id.clone(), next);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn cas_grant_node_slot(
        &self,
        run_id: &RunId,
        expected_run_state: &flow_run::State,
        next_run_state: flow_run::State,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
    ) -> Result<bool, MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        // Check all expectations atomically
        if &run.flow_state != expected_run_state {
            return Ok(false);
        }
        if run.frames.get(frame_id) != Some(expected_frame) {
            return Ok(false);
        }
        // Apply all updates
        run.flow_state = next_run_state;
        run.frames.insert(frame_id.clone(), next_frame);
        Ok(true)
    }

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
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        if run.frames.get(frame_id) != Some(expected_frame) {
            return Ok(false);
        }
        run.frames.insert(frame_id.clone(), next_frame);
        match loop_context {
            None => {
                run.root_step_outputs.insert(
                    crate::ids::StepId::from(step_output_key.as_str()),
                    step_output,
                );
            }
            Some((loop_id, iteration)) => {
                let iteration_index = usize::try_from(iteration).map_err(|_| {
                    MobStoreError::Internal(format!(
                        "loop iteration index {iteration} exceeds usize::MAX on this target"
                    ))
                })?;
                let vec = run
                    .loop_iteration_outputs
                    .entry(loop_id.clone())
                    .or_default();
                while vec.len() <= iteration_index {
                    vec.push(IndexMap::new());
                }
                vec[iteration_index].insert(
                    crate::ids::StepId::from(step_output_key.as_str()),
                    step_output,
                );
            }
        }
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_start_loop(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        expected_run_state: &flow_run::State,
        next_run_state: flow_run::State,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
        initial_loop: LoopSnapshot,
    ) -> Result<bool, MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        if &run.flow_state != expected_run_state {
            return Ok(false);
        }
        if run.frames.get(frame_id) != Some(expected_frame) {
            return Ok(false);
        }
        // Loop must not already exist
        if run.loops.contains_key(loop_instance_id) {
            return Ok(false);
        }
        run.flow_state = next_run_state;
        run.frames.insert(frame_id.clone(), next_frame);
        run.loops.insert(loop_instance_id.clone(), initial_loop);
        Ok(true)
    }

    async fn cas_loop_request_body_frame(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        expected_loop: &LoopSnapshot,
        next_loop: LoopSnapshot,
        expected_run_state: &flow_run::State,
        next_run_state: flow_run::State,
    ) -> Result<bool, MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        if &run.flow_state != expected_run_state {
            return Ok(false);
        }
        if run.loops.get(loop_instance_id) != Some(expected_loop) {
            return Ok(false);
        }
        run.flow_state = next_run_state;
        run.loops.insert(loop_instance_id.clone(), next_loop);
        Ok(true)
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
        expected_run_state: &flow_run::State,
        next_run_state: flow_run::State,
    ) -> Result<bool, MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        if &run.flow_state != expected_run_state {
            return Ok(false);
        }
        if run.loops.get(loop_instance_id) != Some(expected_loop) {
            return Ok(false);
        }
        // Frame must not already exist
        if run.frames.contains_key(frame_id) {
            return Ok(false);
        }
        run.flow_state = next_run_state;
        run.loops.insert(loop_instance_id.clone(), next_loop);
        run.frames.insert(frame_id.clone(), initial_frame);
        if !run.loop_iteration_ledger.iter().any(|existing| {
            existing.loop_instance_id == ledger_entry.loop_instance_id
                && existing.iteration == ledger_entry.iteration
                && existing.frame_id == ledger_entry.frame_id
        }) {
            run.loop_iteration_ledger.push(ledger_entry);
        }
        Ok(true)
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
        expected_run_state: &flow_run::State,
        next_run_state: flow_run::State,
    ) -> Result<bool, MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        if &run.flow_state != expected_run_state {
            return Ok(false);
        }
        if run.loops.get(loop_instance_id) != Some(expected_loop) {
            return Ok(false);
        }
        if run.frames.get(frame_id) != Some(expected_frame) {
            return Ok(false);
        }
        run.flow_state = next_run_state;
        run.loops.insert(loop_instance_id.clone(), next_loop);
        run.frames.insert(frame_id.clone(), next_frame);
        Ok(true)
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
        expected_run_state: &flow_run::State,
        next_run_state: flow_run::State,
    ) -> Result<bool, MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        if &run.flow_state != expected_run_state {
            return Ok(false);
        }
        if run.loops.get(loop_instance_id) != Some(expected_loop) {
            return Ok(false);
        }
        if run.frames.get(frame_id) != Some(expected_frame) {
            return Ok(false);
        }
        run.flow_state = next_run_state;
        run.loops.insert(loop_instance_id.clone(), next_loop);
        run.frames.insert(frame_id.clone(), next_frame);
        Ok(true)
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

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobSpecStore for InMemoryMobSpecStore {
    async fn put_spec(
        &self,
        mob_id: &MobId,
        definition: &MobDefinition,
        revision: Option<u64>,
    ) -> Result<u64, MobStoreError> {
        let mut specs = self.specs.write().await;
        let current_revision = specs.get(mob_id).map_or(0, |(_, rev)| *rev);
        if let Some(expected) = revision
            && expected != current_revision
        {
            return Err(MobStoreError::SpecRevisionConflict {
                mob_id: mob_id.clone(),
                expected: revision,
                actual: current_revision,
            });
        }

        let next_revision = current_revision + 1;
        specs.insert(mob_id.clone(), (definition.clone(), next_revision));
        Ok(next_revision)
    }

    async fn get_spec(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<(MobDefinition, u64)>, MobStoreError> {
        Ok(self.specs.read().await.get(mob_id).cloned())
    }

    async fn list_specs(&self) -> Result<Vec<MobId>, MobStoreError> {
        Ok(self.specs.read().await.keys().cloned().collect())
    }

    async fn delete_spec(
        &self,
        mob_id: &MobId,
        revision: Option<u64>,
    ) -> Result<bool, MobStoreError> {
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

/// In-memory realm profile store with CAS semantics.
#[derive(Debug, Default)]
pub struct InMemoryRealmProfileStore {
    profiles: Arc<RwLock<BTreeMap<String, StoredRealmProfile>>>,
}

impl InMemoryRealmProfileStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl RealmProfileStore for InMemoryRealmProfileStore {
    async fn create(
        &self,
        name: &str,
        profile: &Profile,
    ) -> Result<StoredRealmProfile, MobStoreError> {
        let mut profiles = self.profiles.write().await;
        if profiles.contains_key(name) {
            return Err(MobStoreError::CasConflict(format!(
                "realm profile already exists: {name}"
            )));
        }
        let now = Utc::now();
        let stored = StoredRealmProfile {
            name: name.to_string(),
            profile: profile.clone(),
            revision: 1,
            created_at: now,
            updated_at: now,
        };
        profiles.insert(name.to_string(), stored.clone());
        Ok(stored)
    }

    async fn get(&self, name: &str) -> Result<Option<StoredRealmProfile>, MobStoreError> {
        Ok(self.profiles.read().await.get(name).cloned())
    }

    async fn list(&self) -> Result<Vec<StoredRealmProfile>, MobStoreError> {
        Ok(self.profiles.read().await.values().cloned().collect())
    }

    async fn update(
        &self,
        name: &str,
        profile: &Profile,
        expected_revision: u64,
    ) -> Result<StoredRealmProfile, MobStoreError> {
        let mut profiles = self.profiles.write().await;
        let existing = profiles
            .get(name)
            .ok_or_else(|| MobStoreError::NotFound(format!("realm profile not found: {name}")))?;
        if existing.revision != expected_revision {
            return Err(MobStoreError::CasConflict(format!(
                "realm profile '{name}' revision conflict: expected {expected_revision}, actual {}",
                existing.revision
            )));
        }
        let updated = StoredRealmProfile {
            name: name.to_string(),
            profile: profile.clone(),
            revision: expected_revision + 1,
            created_at: existing.created_at,
            updated_at: Utc::now(),
        };
        profiles.insert(name.to_string(), updated.clone());
        Ok(updated)
    }

    async fn delete(
        &self,
        name: &str,
        expected_revision: u64,
    ) -> Result<StoredRealmProfile, MobStoreError> {
        let mut profiles = self.profiles.write().await;
        let existing = profiles
            .get(name)
            .ok_or_else(|| MobStoreError::NotFound(format!("realm profile not found: {name}")))?;
        if existing.revision != expected_revision {
            return Err(MobStoreError::CasConflict(format!(
                "realm profile '{name}' revision conflict: expected {expected_revision}, actual {}",
                existing.revision
            )));
        }
        let removed = existing.clone();
        profiles.remove(name);
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{BackendConfig, MobDefinition, WiringRules};
    use crate::event::MobEventKind;
    use crate::ids::{AgentIdentity, ProfileName};
    use crate::profile::{Profile, ProfileBinding, ToolConfig};
    use crate::run::StepRunStatus;
    use futures::future::join_all;
    use std::collections::BTreeMap;

    fn default_bridge_protocol_version() -> u32 {
        meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_default_protocol_version()
    }

    fn sample_definition() -> MobDefinition {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("worker"),
            ProfileBinding::Inline(Profile {
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
            }),
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
            spawn_policy: None,
            event_router: None,
            owner_bridge_session_id: None,
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
            flow_state: MobRun::flow_state_for_steps([StepId::from("step-1")]).unwrap(),
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
            loop_iteration_outputs: BTreeMap::new(),
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
    async fn test_event_store_subscribe_receives_appended_events() {
        let store = InMemoryMobEventStore::new();
        let mut rx = store.subscribe().expect("subscribe");

        let stored = store
            .append(NewMobEvent {
                mob_id: MobId::from("mob"),
                timestamp: None,
                kind: MobEventKind::MobCompleted,
            })
            .await
            .unwrap();

        let observed = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("subscription should receive appended event")
            .expect("subscription should stay open");

        assert_eq!(observed.cursor, stored.cursor);
        assert!(matches!(observed.kind, MobEventKind::MobCompleted));
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
            .filter_map(std::result::Result::ok)
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
            agent_identity: AgentIdentity::from("worker-1"),
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
            MobStoreError::SpecRevisionConflict {
                expected: Some(1),
                actual: 2,
                ..
            }
        ));

        assert!(!store.delete_spec(&mob_id, Some(1)).await.unwrap());
        assert!(store.delete_spec(&mob_id, Some(2)).await.unwrap());
    }

    #[tokio::test]
    async fn test_runtime_metadata_store_roundtrips_supervisor_and_overlay_records() {
        let store = InMemoryMobRuntimeMetadataStore::new();
        let mob_id = MobId::from("mob-runtime");
        let supervisor = SupervisorAuthorityRecord::generate(default_bridge_protocol_version());
        store
            .put_supervisor_authority(&mob_id, &supervisor)
            .await
            .unwrap();
        assert_eq!(
            store.load_supervisor_authority(&mob_id).await.unwrap(),
            Some(supervisor.clone())
        );

        let overlay = ExternalBindingOverlayRecord {
            agent_identity: crate::AgentIdentity::from("worker-1"),
            generation: crate::Generation::INITIAL,
            normalized_member_ref: Some(crate::event::MemberRef::BackendPeer {
                peer_id: "ed25519:test-worker-1".to_string(),
                address: "inproc://worker-1".to_string(),
                bootstrap_token: None,
                session_id: None,
            }),
            bootstrap_token: None,
            status: crate::store::ExternalBindingOverlayStatus::Normalized,
            updated_at: Utc::now(),
        };
        assert!(
            store
                .put_external_binding_overlay_if_absent(&mob_id, &overlay)
                .await
                .unwrap()
        );
        assert!(
            !store
                .put_external_binding_overlay_if_absent(&mob_id, &overlay)
                .await
                .unwrap()
        );
        let overlays = store.list_external_binding_overlays(&mob_id).await.unwrap();
        assert_eq!(overlays, vec![overlay.clone()]);

        store
            .delete_external_binding_overlays(&mob_id)
            .await
            .unwrap();
        store.delete_supervisor_authority(&mob_id).await.unwrap();
        assert!(
            store
                .list_external_binding_overlays(&mob_id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .load_supervisor_authority(&mob_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_runtime_metadata_store_put_supervisor_if_absent_preserves_existing_record() {
        let store = InMemoryMobRuntimeMetadataStore::new();
        let mob_id = MobId::from("mob-runtime");
        let first = SupervisorAuthorityRecord::generate(default_bridge_protocol_version());
        let second = SupervisorAuthorityRecord::generate(default_bridge_protocol_version());

        assert!(
            store
                .put_supervisor_authority_if_absent(&mob_id, &first)
                .await
                .unwrap()
        );
        assert!(
            !store
                .put_supervisor_authority_if_absent(&mob_id, &second)
                .await
                .unwrap()
        );
        assert_eq!(
            store.load_supervisor_authority(&mob_id).await.unwrap(),
            Some(first)
        );
    }

    // -----------------------------------------------------------------------
    // RealmProfileStore contract tests
    // -----------------------------------------------------------------------

    use crate::store::realm_profile::contract_tests;

    #[tokio::test]
    async fn realm_profile_create_and_get() {
        let store = InMemoryRealmProfileStore::new();
        contract_tests::test_create_and_get(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_get_nonexistent() {
        let store = InMemoryRealmProfileStore::new();
        contract_tests::test_get_nonexistent(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_create_duplicate_fails() {
        let store = InMemoryRealmProfileStore::new();
        contract_tests::test_create_duplicate_fails(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_update_correct_revision() {
        let store = InMemoryRealmProfileStore::new();
        contract_tests::test_update_with_correct_revision(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_update_wrong_revision() {
        let store = InMemoryRealmProfileStore::new();
        contract_tests::test_update_with_wrong_revision(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_update_nonexistent() {
        let store = InMemoryRealmProfileStore::new();
        contract_tests::test_update_nonexistent(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_delete_correct_revision() {
        let store = InMemoryRealmProfileStore::new();
        contract_tests::test_delete_with_correct_revision(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_delete_wrong_revision() {
        let store = InMemoryRealmProfileStore::new();
        contract_tests::test_delete_with_wrong_revision(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_delete_nonexistent() {
        let store = InMemoryRealmProfileStore::new();
        contract_tests::test_delete_nonexistent(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_list() {
        let store = InMemoryRealmProfileStore::new();
        contract_tests::test_list(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_list_empty() {
        let store = InMemoryRealmProfileStore::new();
        contract_tests::test_list_empty(&store).await;
    }
}
