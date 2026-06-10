//! In-memory store implementations.

use super::realm_profile::{RealmProfileStore, StoredRealmProfile};
use super::{
    ExternalBindingOverlayRecord, MobEventStore, MobRunStore, MobRuntimeMetadataStore,
    MobSpecStore, MobStoreError, SupervisorAuthorityDeletionAuthority,
    SupervisorAuthorityPersistenceAuthority, SupervisorAuthorityRecord, private,
    terminal_event_identity, validate_mob_event_write_authority,
};
use crate::definition::MobDefinition;
#[cfg(any(test, feature = "test-support"))]
use crate::event::MobEventKind;
use crate::event::{MobEvent, NewMobEvent};
use crate::ids::{
    AgentIdentity, FlowId, FrameId, Generation, LoopId, LoopInstanceId, MobId, RunId, StepId,
};
use crate::machines::mob_machine as mob_dsl;
use crate::profile::Profile;
use crate::run::flow_run;
use crate::run::{
    FailureLedgerEntry, FlowAuthorityInputRecord, FrameSnapshot, LoopIterationLedgerEntry,
    LoopSnapshot, MobRun, MobRunProvenanceAuthority, MobRunStatus, StepLedgerEntry,
    mob_machine_run_status_is_terminal,
};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use std::collections::BTreeMap;
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{RwLock, broadcast};

const EVENT_SUBSCRIPTION_CHANNEL_CAPACITY: usize = 4096;

fn append_flow_authority_inputs(
    run: &mut MobRun,
    authority_inputs: Vec<mob_dsl::MobMachineInput>,
) -> Result<(), MobStoreError> {
    run.append_flow_authority_inputs(authority_inputs)
        .map_err(|error| MobStoreError::Internal(error.to_string()))?;
    validate_authorized_run_projection(run)
}

fn validate_authorized_run_projection(run: &MobRun) -> Result<(), MobStoreError> {
    run.validate_flow_authority_projection()
        .map_err(|error| MobStoreError::Internal(error.to_string()))
}

fn validate_flow_authority_inputs(
    run_id: &RunId,
    authority_inputs: &[mob_dsl::MobMachineInput],
) -> Result<(), MobStoreError> {
    if authority_inputs.is_empty() {
        return Err(MobStoreError::Internal(format!(
            "run '{run_id}' store mutation missing MobMachine authority input"
        )));
    }
    for input in authority_inputs.iter().cloned() {
        FlowAuthorityInputRecord::from_machine_input(input)
            .map_err(|error| MobStoreError::Internal(error.to_string()))?;
    }
    Ok(())
}

fn apply_flow_authority_update<F>(
    run: &MobRun,
    authority_inputs: Vec<mob_dsl::MobMachineInput>,
    update: F,
) -> Result<MobRun, MobStoreError>
where
    F: FnOnce(&mut MobRun) -> Result<(), MobStoreError>,
{
    validate_flow_authority_inputs(&run.run_id, &authority_inputs)?;
    let mut next_run = run.clone();
    update(&mut next_run)?;
    append_flow_authority_inputs(&mut next_run, authority_inputs)?;
    Ok(next_run)
}

/// In-memory event store for tests and ephemeral mobs.
#[derive(Debug)]
pub struct InMemoryMobEventStore {
    events: Arc<RwLock<Vec<MobEvent>>>,
    event_tx: broadcast::Sender<MobEvent>,
    #[cfg(any(test, feature = "test-support"))]
    fail_clear: AtomicBool,
    #[cfg(any(test, feature = "test-support"))]
    fail_member_retired_appends: AtomicBool,
}

impl Default for InMemoryMobEventStore {
    fn default() -> Self {
        let (event_tx, _event_rx) = broadcast::channel(EVENT_SUBSCRIPTION_CHANNEL_CAPACITY);
        Self {
            events: Arc::new(RwLock::new(Vec::new())),
            event_tx,
            #[cfg(any(test, feature = "test-support"))]
            fail_clear: AtomicBool::new(false),
            #[cfg(any(test, feature = "test-support"))]
            fail_member_retired_appends: AtomicBool::new(false),
        }
    }
}

impl InMemoryMobEventStore {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn fail_clear_until_allowed(&self) {
        self.fail_clear.store(true, Ordering::Relaxed);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn allow_clear(&self) {
        self.fail_clear.store(false, Ordering::Relaxed);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn fail_member_retired_appends(&self) {
        self.fail_member_retired_appends
            .store(true, Ordering::Relaxed);
    }

    #[cfg(any(test, feature = "test-support"))]
    fn forced_append_error(&self, event: &NewMobEvent) -> Option<MobStoreError> {
        if self.fail_member_retired_appends.load(Ordering::Relaxed)
            && matches!(event.kind, MobEventKind::MemberRetired { .. })
        {
            Some(MobStoreError::Internal(
                "forced mob event store member retired append failure".to_string(),
            ))
        } else {
            None
        }
    }
}

impl private::MobEventStoreSealed for InMemoryMobEventStore {}

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
        authority: &SupervisorAuthorityPersistenceAuthority,
    ) -> Result<(), MobStoreError> {
        authority.verify_record(record)?;
        self.supervisor_records
            .write()
            .await
            .insert(mob_id.clone(), record.clone());
        Ok(())
    }

    async fn compare_and_put_supervisor_authority(
        &self,
        mob_id: &MobId,
        expected: &SupervisorAuthorityRecord,
        record: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(record)?;
        let mut guard = self.supervisor_records.write().await;
        if guard.get(mob_id) != Some(expected) {
            return Ok(false);
        }
        guard.insert(mob_id.clone(), record.clone());
        Ok(true)
    }

    async fn put_supervisor_authority_if_absent(
        &self,
        mob_id: &MobId,
        record: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(record)?;
        let mut guard = self.supervisor_records.write().await;
        if guard.contains_key(mob_id) {
            return Ok(false);
        }
        guard.insert(mob_id.clone(), record.clone());
        Ok(true)
    }

    async fn delete_supervisor_authority(
        &self,
        mob_id: &MobId,
        expected: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityDeletionAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(expected)?;
        let mut guard = self.supervisor_records.write().await;
        if guard.get(mob_id) != Some(expected) {
            return Ok(false);
        }
        guard.remove(mob_id);
        Ok(true)
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
        validate_mob_event_write_authority(&event.kind)?;

        #[cfg(any(test, feature = "test-support"))]
        if let Some(error) = self.forced_append_error(&event) {
            return Err(error);
        }

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

    async fn append_terminal_event_if_absent(
        &self,
        event: NewMobEvent,
    ) -> Result<Option<MobEvent>, MobStoreError> {
        let Some((run_id, flow_id)) = terminal_event_identity(&event.kind) else {
            return Err(MobStoreError::Internal(
                "append_terminal_event_if_absent requires a terminal flow event".to_string(),
            ));
        };
        let run_id = run_id.clone();
        let flow_id = flow_id.clone();
        let mob_id = event.mob_id.clone();

        let mut events = self.events.write().await;
        if events.iter().any(|existing| {
            existing.mob_id == mob_id
                && terminal_event_identity(&existing.kind).is_some_and(
                    |(existing_run_id, existing_flow_id)| {
                        existing_run_id == &run_id && existing_flow_id == &flow_id
                    },
                )
        }) {
            return Ok(None);
        }

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
        Ok(Some(stored))
    }

    async fn append_batch(&self, batch: Vec<NewMobEvent>) -> Result<Vec<MobEvent>, MobStoreError> {
        for event in &batch {
            validate_mob_event_write_authority(&event.kind)?;
        }

        #[cfg(any(test, feature = "test-support"))]
        if let Some(error) = batch
            .iter()
            .find_map(|event| self.forced_append_error(event))
        {
            return Err(error);
        }

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
        #[cfg(any(test, feature = "test-support"))]
        if self.fail_clear.load(Ordering::Relaxed) {
            return Err(MobStoreError::Internal(
                "forced mob event store clear failure".to_string(),
            ));
        }

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
        validate_authorized_run_projection(&run)?;
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

    async fn cas_flow_state_with_authority(
        &self,
        run_id: &RunId,
        expected: &flow_run::State,
        next: &flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let mut runs = self.runs.write().await;
        let Some(run) = runs.get_mut(run_id) else {
            return Ok(false);
        };
        if &run.flow_state != expected {
            return Ok(false);
        }
        let next_state = next.clone();
        let next_run = apply_flow_authority_update(run, authority_inputs, move |candidate| {
            candidate.flow_state = next_state;
            Ok(())
        })?;
        *run = next_run;
        Ok(true)
    }

    async fn cas_run_snapshot_with_authority(
        &self,
        run_id: &RunId,
        expected_status: MobRunStatus,
        expected_flow_state: &flow_run::State,
        next_status: MobRunStatus,
        next_flow_state: &flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let mut runs = self.runs.write().await;
        let Some(run) = runs.get_mut(run_id) else {
            return Ok(false);
        };
        let current_terminal = mob_machine_run_status_is_terminal(run_id, &run.status)
            .map_err(|error| MobStoreError::Internal(error.to_string()))?;
        if run.status != expected_status
            || current_terminal
            || &run.flow_state != expected_flow_state
        {
            return Ok(false);
        }
        let next_flow_state = next_flow_state.clone();
        let terminal = mob_machine_run_status_is_terminal(run_id, &next_status)
            .map_err(|error| MobStoreError::Internal(error.to_string()))?;
        let next_run = apply_flow_authority_update(run, authority_inputs, move |candidate| {
            candidate.status = next_status;
            candidate.flow_state = next_flow_state;
            if terminal && candidate.completed_at.is_none() {
                candidate.completed_at = Some(Utc::now());
            }
            Ok(())
        })?;
        *run = next_run;
        Ok(true)
    }

    async fn append_step_entry_with_authority(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<(), MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        authority
            .validate_step_entry(run, &entry)
            .map_err(|error| MobStoreError::Internal(error.to_string()))?;
        let mut candidate = run.clone();
        candidate.step_ledger.push(entry);
        validate_authorized_run_projection(&candidate)?;
        *run = candidate;
        Ok(())
    }

    async fn append_step_entry_if_absent_with_authority(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<bool, MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        authority
            .validate_step_entry(run, &entry)
            .map_err(|error| MobStoreError::Internal(error.to_string()))?;
        let is_duplicate = run.step_ledger.iter().any(|existing| {
            existing.step_id == entry.step_id
                && existing.agent_identity == entry.agent_identity
                && existing.status == entry.status
        });
        if is_duplicate {
            return Ok(false);
        }
        let mut candidate = run.clone();
        candidate.step_ledger.push(entry);
        validate_authorized_run_projection(&candidate)?;
        *run = candidate;
        Ok(true)
    }

    async fn append_failure_entry_with_authority(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<(), MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        authority
            .validate_failure_entry(run, &entry)
            .map_err(|error| MobStoreError::Internal(error.to_string()))?;
        let mut candidate = run.clone();
        candidate.failure_ledger.push(entry);
        validate_authorized_run_projection(&candidate)?;
        *run = candidate;
        Ok(())
    }

    async fn cas_frame_state_with_authority(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        expected: Option<&FrameSnapshot>,
        next: FrameSnapshot,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        let current = run.frames.get(frame_id);
        match (expected, current) {
            (None, None) => {
                let frame_id = frame_id.clone();
                let next_run =
                    apply_flow_authority_update(run, authority_inputs, move |candidate| {
                        candidate.frames.insert(frame_id, next);
                        Ok(())
                    })?;
                *run = next_run;
                Ok(true)
            }
            (Some(exp), Some(cur)) if exp == cur => {
                let frame_id = frame_id.clone();
                let next_run =
                    apply_flow_authority_update(run, authority_inputs, move |candidate| {
                        candidate.frames.insert(frame_id, next);
                        Ok(())
                    })?;
                *run = next_run;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_grant_node_slot_with_authority(
        &self,
        run_id: &RunId,
        expected_run_state: &flow_run::State,
        next_run_state: flow_run::State,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
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
        let frame_id = frame_id.clone();
        let next_run = apply_flow_authority_update(run, authority_inputs, move |candidate| {
            candidate.flow_state = next_run_state;
            candidate.frames.insert(frame_id, next_frame);
            Ok(())
        })?;
        *run = next_run;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_complete_step_and_record_output_with_authority(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
        step_output_key: String,
        step_output: serde_json::Value,
        loop_context: Option<(&LoopId, u64)>,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        if run.frames.get(frame_id) != Some(expected_frame) {
            return Ok(false);
        }
        let frame_id = frame_id.clone();
        let loop_context = loop_context.map(|(loop_id, iteration)| (loop_id.clone(), iteration));
        let next_run = apply_flow_authority_update(run, authority_inputs, move |candidate| {
            candidate.frames.insert(frame_id, next_frame);
            match loop_context {
                None => {
                    candidate.root_step_outputs.insert(
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
                    let vec = candidate
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
            Ok(())
        })?;
        *run = next_run;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_start_loop_with_authority(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        expected_run_state: &flow_run::State,
        next_run_state: flow_run::State,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
        initial_loop: LoopSnapshot,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
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
        if run.loops.contains_key(loop_instance_id) {
            return Ok(false);
        }
        let frame_id = frame_id.clone();
        let loop_instance_id = loop_instance_id.clone();
        let next_run = apply_flow_authority_update(run, authority_inputs, move |candidate| {
            candidate.flow_state = next_run_state;
            candidate.frames.insert(frame_id, next_frame);
            candidate.loops.insert(loop_instance_id, initial_loop);
            Ok(())
        })?;
        *run = next_run;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_loop_request_body_frame_with_authority(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        expected_loop: &LoopSnapshot,
        next_loop: LoopSnapshot,
        expected_run_state: &flow_run::State,
        next_run_state: flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
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
        let loop_instance_id = loop_instance_id.clone();
        let next_run = apply_flow_authority_update(run, authority_inputs, move |candidate| {
            candidate.flow_state = next_run_state;
            candidate.loops.insert(loop_instance_id, next_loop);
            Ok(())
        })?;
        *run = next_run;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_grant_body_frame_start_with_authority(
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
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
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
        if run.frames.contains_key(frame_id) {
            return Ok(false);
        }
        let frame_id = frame_id.clone();
        let loop_instance_id = loop_instance_id.clone();
        let next_run = apply_flow_authority_update(run, authority_inputs, move |candidate| {
            candidate.flow_state = next_run_state;
            candidate.loops.insert(loop_instance_id, next_loop);
            candidate.frames.insert(frame_id, initial_frame);
            if !candidate.loop_iteration_ledger.iter().any(|existing| {
                existing.loop_instance_id == ledger_entry.loop_instance_id
                    && existing.iteration == ledger_entry.iteration
                    && existing.frame_id == ledger_entry.frame_id
            }) {
                candidate.loop_iteration_ledger.push(ledger_entry);
            }
            Ok(())
        })?;
        *run = next_run;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_complete_body_frame_with_authority(
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
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
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
        let frame_id = frame_id.clone();
        let loop_instance_id = loop_instance_id.clone();
        let next_run = apply_flow_authority_update(run, authority_inputs, move |candidate| {
            candidate.flow_state = next_run_state;
            candidate.loops.insert(loop_instance_id, next_loop);
            candidate.frames.insert(frame_id, next_frame);
            Ok(())
        })?;
        *run = next_run;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    async fn cas_complete_loop_with_authority(
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
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
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
        let frame_id = frame_id.clone();
        let loop_instance_id = loop_instance_id.clone();
        let next_run = apply_flow_authority_update(run, authority_inputs, move |candidate| {
            candidate.flow_state = next_run_state;
            candidate.loops.insert(loop_instance_id, next_loop);
            candidate.frames.insert(frame_id, next_frame);
            Ok(())
        })?;
        *run = next_run;
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
    use crate::ids::{AgentIdentity, Generation, ProfileName};
    use crate::profile::{Profile, ProfileBinding, ToolConfig};
    use crate::run::StepRunStatus;
    use futures::future::join_all;
    use std::collections::BTreeMap;

    fn default_bridge_protocol_version()
    -> meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion {
        meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_default_protocol_version()
    }

    fn sample_definition() -> MobDefinition {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("worker"),
            ProfileBinding::Inline(Profile {
                model: "model".to_string(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
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
        let mut definition = MobDefinition::explicit("mob");
        definition.profiles = profiles;
        definition
    }

    fn sample_run(status: MobRunStatus) -> MobRun {
        MobRun::authority_backed_for_steps(
            RunId::new(),
            MobId::from("mob"),
            FlowId::from("flow-a"),
            [StepId::from("step-1")],
            status,
            serde_json::json!({"a":1}),
        )
        .expect("authority-backed sample run")
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
    async fn test_event_store_test_support_failures_fail_closed() {
        let store = InMemoryMobEventStore::new();

        store.fail_clear_until_allowed();
        let clear_error = store.clear().await.expect_err("clear should fail closed");
        assert!(
            clear_error
                .to_string()
                .contains("forced mob event store clear failure")
        );
        store.allow_clear();
        store.clear().await.expect("clear recovery should succeed");

        store.fail_member_retired_appends();
        let append_error = store
            .append(NewMobEvent {
                mob_id: MobId::from("mob"),
                timestamp: None,
                kind: MobEventKind::MemberRetired {
                    agent_identity: AgentIdentity::from("worker-1"),
                    generation: Generation::new(1),
                    role: ProfileName::from("worker"),
                },
            })
            .await
            .expect_err("member-retired append should fail closed");
        assert!(
            append_error
                .to_string()
                .contains("forced mob event store member retired append failure")
        );

        let stored = store
            .append(NewMobEvent {
                mob_id: MobId::from("mob"),
                timestamp: None,
                kind: MobEventKind::MobCompleted,
            })
            .await
            .expect("unrelated append should still succeed");
        assert!(matches!(stored.kind, MobEventKind::MobCompleted));
    }

    #[tokio::test]
    async fn test_event_store_rejects_step_target_failure_terminal_metadata() {
        let store = InMemoryMobEventStore::new();

        let error = store
            .append(NewMobEvent {
                mob_id: MobId::from("mob"),
                timestamp: None,
                kind: MobEventKind::StepTargetFailed {
                    run_id: RunId::new(),
                    step_id: StepId::from("review"),
                    target: crate::ids::AgentRuntimeId::initial(AgentIdentity::from("reviewer")),
                    reason: "LLM failure terminal turn".to_string(),
                    error_report: None,
                    error: Some(meerkat_core::TurnErrorMetadata::terminal(
                        meerkat_core::TurnTerminalCauseKind::LlmFailure,
                        meerkat_core::TurnTerminalOutcome::Failed,
                        "LLM failure terminal turn",
                    )),
                },
            })
            .await
            .expect_err("terminal metadata without generated mob authority must fail closed");

        assert!(
            error
                .to_string()
                .contains("requires generated mob authority")
        );
    }

    #[tokio::test]
    async fn test_run_store_status_cas_single_winner() {
        let store = Arc::new(InMemoryMobRunStore::new());
        let run = sample_run(MobRunStatus::Running);
        let run_id = run.run_id.clone();
        let expected_flow_state = run.flow_state.clone();
        let (completed_flow_state, completed_authority_input) = run
            .flow_run_command_projection_for_test(
                crate::run::MobMachineFlowRunCommand::TerminalizeCompleted(
                    crate::run::flow_run::inputs::TerminalizeCompleted {},
                ),
            )
            .expect("project completed run state");
        store.create_run(run).await.unwrap();

        let tasks = (0..10).map(|_| {
            let store = Arc::clone(&store);
            let run_id = run_id.clone();
            let expected_flow_state = expected_flow_state.clone();
            let completed_flow_state = completed_flow_state.clone();
            let completed_authority_input = completed_authority_input.clone();
            tokio::spawn(async move {
                store
                    .cas_run_snapshot_with_authority(
                        &run_id,
                        MobRunStatus::Running,
                        &expected_flow_state,
                        MobRunStatus::Completed,
                        &completed_flow_state,
                        vec![completed_authority_input],
                    )
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
    async fn test_run_store_snapshot_rejects_missing_authority_without_mutation() {
        let store = InMemoryMobRunStore::new();
        let run = sample_run(MobRunStatus::Running);
        let run_id = run.run_id.clone();
        let expected_flow_state = run.flow_state.clone();
        let authority_input_count = run.flow_authority_inputs.len();
        store.create_run(run).await.unwrap();

        let error = store
            .cas_run_snapshot_with_authority(
                &run_id,
                MobRunStatus::Running,
                &expected_flow_state,
                MobRunStatus::Completed,
                &expected_flow_state,
                Vec::new(),
            )
            .await
            .expect_err("missing machine authority must reject snapshot CAS");
        assert!(
            error
                .to_string()
                .contains("store mutation missing MobMachine authority input")
        );

        let stored = store.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(stored.status, MobRunStatus::Running);
        assert!(stored.completed_at.is_none());
        assert_eq!(stored.flow_authority_inputs.len(), authority_input_count);
    }

    #[tokio::test]
    async fn test_run_store_step_dedup_and_ledgers() {
        let store = InMemoryMobRunStore::new();
        let run = sample_run(MobRunStatus::Running);
        let run_id = run.run_id.clone();
        let step_id = StepId::from("step-1");
        let expected_flow_state = run.flow_state.clone();
        let (dispatched_flow_state, dispatched_authority_input) = run
            .flow_run_command_projection_for_test(
                crate::run::MobMachineFlowRunCommand::DispatchStep(
                    crate::run::flow_run::inputs::DispatchStep {
                        step_id: step_id.clone(),
                    },
                ),
            )
            .expect("project dispatched step state");
        store.create_run(run).await.unwrap();
        assert!(
            store
                .cas_flow_state_with_authority(
                    &run_id,
                    &expected_flow_state,
                    &dispatched_flow_state,
                    vec![dispatched_authority_input.clone()],
                )
                .await
                .unwrap()
        );
        let dispatched_authority =
            MobRunProvenanceAuthority::from_flow_authority_input(dispatched_authority_input)
                .expect("dispatch input is provenance authority");

        let step_entry = StepLedgerEntry {
            step_id: step_id.clone(),
            agent_identity: AgentIdentity::flow_system_provenance(),
            status: StepRunStatus::Dispatched,
            output: None,
            timestamp: Utc::now(),
        };

        assert!(
            store
                .append_step_entry_if_absent_with_authority(
                    &run_id,
                    step_entry.clone(),
                    dispatched_authority.clone(),
                )
                .await
                .unwrap()
        );
        assert!(
            !store
                .append_step_entry_if_absent_with_authority(
                    &run_id,
                    step_entry,
                    dispatched_authority.clone(),
                )
                .await
                .unwrap()
        );
        store
            .append_step_entry_with_authority(
                &run_id,
                StepLedgerEntry {
                    step_id: step_id.clone(),
                    agent_identity: AgentIdentity::flow_system_provenance(),
                    status: StepRunStatus::Dispatched,
                    output: None,
                    timestamp: Utc::now(),
                },
                dispatched_authority,
            )
            .await
            .expect_err("one dispatch authority must not authorize duplicate step ledger rows");

        let dispatched_run = store.get_run(&run_id).await.unwrap().unwrap();
        let (failed_flow_state, failed_authority_input) = dispatched_run
            .flow_run_command_projection_for_test(crate::run::MobMachineFlowRunCommand::FailStep(
                crate::run::flow_run::inputs::FailStep {
                    step_id: step_id.clone(),
                },
            ))
            .expect("project failed step state");
        assert!(
            store
                .cas_flow_state_with_authority(
                    &run_id,
                    &dispatched_run.flow_state,
                    &failed_flow_state,
                    vec![failed_authority_input.clone()],
                )
                .await
                .unwrap()
        );
        let failed_authority =
            MobRunProvenanceAuthority::from_flow_authority_input(failed_authority_input)
                .expect("fail input is provenance authority");

        let rejected_typed_failure_entry = FailureLedgerEntry {
            step_id: step_id.clone(),
            reason: "failed".to_string(),
            error_report: None,
            error: Some(meerkat_core::TurnErrorMetadata::terminal(
                meerkat_core::TurnTerminalCauseKind::LlmFailure,
                meerkat_core::TurnTerminalOutcome::Failed,
                "LLM failure terminal turn",
            )),
            timestamp: Utc::now(),
        };
        let error = store
            .append_failure_entry_with_authority(
                &run_id,
                rejected_typed_failure_entry,
                failed_authority.clone(),
            )
            .await
            .expect_err("failure ledger terminal metadata must fail closed");
        assert!(
            error
                .to_string()
                .contains("requires generated mob authority")
        );

        let failure_entry = FailureLedgerEntry {
            step_id: step_id.clone(),
            reason: "failed".to_string(),
            error_report: None,
            error: None,
            timestamp: Utc::now(),
        };
        store
            .append_failure_entry_with_authority(
                &run_id,
                failure_entry.clone(),
                failed_authority.clone(),
            )
            .await
            .unwrap();
        store
            .append_failure_entry_with_authority(&run_id, failure_entry, failed_authority)
            .await
            .expect_err("one fail authority must not authorize duplicate failure ledger rows");

        let stored = store.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(stored.step_ledger.len(), 1);
        assert_eq!(stored.failure_ledger.len(), 1);
    }

    #[tokio::test]
    async fn test_run_store_create_rejects_preset_provenance_ledgers_without_authority() {
        let store = InMemoryMobRunStore::new();
        let mut run = sample_run(MobRunStatus::Running);
        run.step_ledger.push(StepLedgerEntry {
            step_id: StepId::from("step-1"),
            agent_identity: AgentIdentity::from("worker-1"),
            status: StepRunStatus::Dispatched,
            output: None,
            timestamp: Utc::now(),
        });

        let error = store
            .create_run(run)
            .await
            .expect_err("raw step provenance must be rejected");
        assert!(error.to_string().contains("step ledger entry"));

        let mut run = sample_run(MobRunStatus::Running);
        run.failure_ledger.push(FailureLedgerEntry {
            step_id: StepId::from("step-1"),
            reason: "caller-injected failure".to_string(),
            error_report: None,
            error: None,
            timestamp: Utc::now(),
        });

        let error = store
            .create_run(run)
            .await
            .expect_err("raw failure provenance must be rejected");
        assert!(error.to_string().contains("failure ledger entry"));

        let mut run = sample_run(MobRunStatus::Running);
        run.schema_version = crate::run::mob_run_schema_version() - 1;
        let error = store
            .create_run(run)
            .await
            .expect_err("caller-controlled schema version must be rejected");
        assert!(error.to_string().contains("schema_version"));
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
        let supervisor_authority =
            crate::store::supervisor_authority_persistence_authority_for_record(&supervisor)
                .unwrap();
        store
            .put_supervisor_authority(&mob_id, &supervisor, &supervisor_authority)
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
                pubkey: None,
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
        let delete_authority =
            crate::store::supervisor_authority_deletion_authority_for_record(&supervisor).unwrap();
        assert!(
            store
                .delete_supervisor_authority(&mob_id, &supervisor, &delete_authority)
                .await
                .unwrap()
        );
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
        let first_authority =
            crate::store::supervisor_authority_persistence_authority_for_record(&first).unwrap();
        let second_authority =
            crate::store::supervisor_authority_persistence_authority_for_record(&second).unwrap();

        assert!(
            store
                .put_supervisor_authority_if_absent(&mob_id, &first, &first_authority)
                .await
                .unwrap()
        );
        assert!(
            !store
                .put_supervisor_authority_if_absent(&mob_id, &second, &second_authority)
                .await
                .unwrap()
        );
        assert_eq!(
            store.load_supervisor_authority(&mob_id).await.unwrap(),
            Some(first)
        );
    }

    #[tokio::test]
    async fn test_runtime_metadata_store_compare_and_put_supervisor_authority() {
        let store = InMemoryMobRuntimeMetadataStore::new();
        let mob_id = MobId::from("mob-runtime-cas");
        let first = SupervisorAuthorityRecord::generate(default_bridge_protocol_version());
        let second = SupervisorAuthorityRecord::generate(default_bridge_protocol_version());
        let third = SupervisorAuthorityRecord::generate(default_bridge_protocol_version());
        let first_authority =
            crate::store::supervisor_authority_persistence_authority_for_record(&first).unwrap();
        let second_authority =
            crate::store::supervisor_authority_persistence_authority_for_record(&second).unwrap();
        let third_authority =
            crate::store::supervisor_authority_persistence_authority_for_record(&third).unwrap();

        store
            .put_supervisor_authority(&mob_id, &first, &first_authority)
            .await
            .unwrap();
        assert!(
            !store
                .compare_and_put_supervisor_authority(&mob_id, &second, &third, &third_authority)
                .await
                .unwrap(),
            "mismatched expected authority must not update"
        );
        assert_eq!(
            store.load_supervisor_authority(&mob_id).await.unwrap(),
            Some(first.clone())
        );
        assert!(
            store
                .compare_and_put_supervisor_authority(&mob_id, &first, &second, &second_authority)
                .await
                .unwrap(),
            "matching expected authority should update"
        );
        assert_eq!(
            store.load_supervisor_authority(&mob_id).await.unwrap(),
            Some(second)
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
