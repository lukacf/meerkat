//! In-memory store implementations.

use super::realm_profile::{RealmProfileStore, StoredRealmProfile};
use super::{
    BeginPlacedSpawnResult, CommitPlacedSpawnResult, DeletePlacedSpawnResult,
    ExternalBindingOverlayRecord, MobEventStore, MobHostAuthorityDeletionAuthority,
    MobHostAuthorityPersistenceAuthority, MobHostAuthorityRecord, MobMemberEventCursorRecord,
    MobMemberLiveCleanupRecord, MobMemberOperatorPruneAuthority, MobMemberOperatorRequestBegin,
    MobMemberOperatorRequestKey, MobMemberOperatorRequestRecord, MobOperatorGrantDeletionAuthority,
    MobOperatorGrantPersistenceAuthority, MobOperatorGrantRecord,
    MobPlacedSpawnBindingPromotionAuthority, MobPlacedSpawnCarrierRecord,
    MobPlacedSpawnCleanupAuthority, MobPlacedSpawnCommitPersistenceAuthority,
    MobPlacedSpawnPendingPersistenceAuthority, MobRunStore, MobRuntimeMetadataStore, MobSpecStore,
    MobStoreError, PlacedSpawnCarrierPhase, PromotePlacedSpawnBindingResult,
    SupervisorAuthorityDeletionAuthority, SupervisorAuthorityPersistenceAuthority,
    SupervisorAuthorityRecord, private, step_failed_event_identity, terminal_event_identity,
    validate_mob_event_write_authority,
};
use crate::definition::MobDefinition;
use crate::event::{MobEvent, MobEventKind, NewMobEvent};
use crate::ids::{
    AgentIdentity, FlowId, FrameId, Generation, LoopId, LoopInstanceId, MobId, RunId, StepId,
};
use crate::machines::mob_machine as mob_dsl;
use crate::profile::Profile;
use crate::run::flow_run;
use crate::run::{
    FailureLedgerEntry, FlowAuthorityInputRecord, FrameSnapshot, LoopIterationLedgerEntry,
    LoopSnapshot, MobRun, MobRunProvenanceAuthority, MobRunRemoteTurnIntent,
    MobRunRemoteTurnReceipt, MobRunStatus, StepLedgerEntry, mob_machine_run_status_is_terminal,
};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use std::collections::BTreeMap;
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    #[cfg(any(test, feature = "test-support"))]
    fail_next_members_unwired_after_append: AtomicBool,
    #[cfg(any(test, feature = "test-support"))]
    fail_next_members_unwired_before_append: AtomicBool,
    #[cfg(any(test, feature = "test-support"))]
    reconciliation_latest_cursor_failures_to_arm: AtomicU64,
    #[cfg(any(test, feature = "test-support"))]
    reconciliation_poll_failures_to_arm: AtomicU64,
    #[cfg(any(test, feature = "test-support"))]
    active_reconciliation_latest_cursor_failures: AtomicU64,
    #[cfg(any(test, feature = "test-support"))]
    active_reconciliation_poll_failures: AtomicU64,
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
            #[cfg(any(test, feature = "test-support"))]
            fail_next_members_unwired_after_append: AtomicBool::new(false),
            #[cfg(any(test, feature = "test-support"))]
            fail_next_members_unwired_before_append: AtomicBool::new(false),
            #[cfg(any(test, feature = "test-support"))]
            reconciliation_latest_cursor_failures_to_arm: AtomicU64::new(0),
            #[cfg(any(test, feature = "test-support"))]
            reconciliation_poll_failures_to_arm: AtomicU64::new(0),
            #[cfg(any(test, feature = "test-support"))]
            active_reconciliation_latest_cursor_failures: AtomicU64::new(0),
            #[cfg(any(test, feature = "test-support"))]
            active_reconciliation_poll_failures: AtomicU64::new(0),
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
    pub fn fail_next_members_unwired_after_append(&self) {
        self.fail_next_members_unwired_after_append
            .store(true, Ordering::Relaxed);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn fail_next_members_unwired_before_append(&self) {
        self.fail_next_members_unwired_before_append
            .store(true, Ordering::Relaxed);
    }

    /// Arm failures only after the next fault-injected MembersUnwired append
    /// error. The operation's pre-mutation cursor-floor read remains healthy;
    /// only the exact post-error reconciliation is affected.
    #[cfg(any(test, feature = "test-support"))]
    pub fn fail_reconciliation_latest_cursor_reads_after_next_members_unwired_error(
        &self,
        failures: u64,
    ) {
        self.reconciliation_latest_cursor_failures_to_arm
            .store(failures, Ordering::Relaxed);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn fail_reconciliation_poll_reads_after_next_members_unwired_error(&self, failures: u64) {
        self.reconciliation_poll_failures_to_arm
            .store(failures, Ordering::Relaxed);
    }

    #[cfg(any(test, feature = "test-support"))]
    fn arm_members_unwired_reconciliation_read_failures(&self) {
        self.active_reconciliation_latest_cursor_failures.store(
            self.reconciliation_latest_cursor_failures_to_arm
                .swap(0, Ordering::Relaxed),
            Ordering::Relaxed,
        );
        self.active_reconciliation_poll_failures.store(
            self.reconciliation_poll_failures_to_arm
                .swap(0, Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }

    #[cfg(any(test, feature = "test-support"))]
    fn consume_read_failure(counter: &AtomicU64) -> bool {
        counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                if remaining > 0 {
                    Some(remaining - 1)
                } else {
                    None
                }
            })
            .is_ok()
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

type MemberOperatorRequestMap =
    BTreeMap<(MobId, MobMemberOperatorRequestKey), MobMemberOperatorRequestRecord>;

/// In-memory runtime metadata store for tests and ephemeral mobs.
#[derive(Debug, Default)]
pub struct InMemoryMobRuntimeMetadataStore {
    supervisor_records: Arc<RwLock<BTreeMap<MobId, SupervisorAuthorityRecord>>>,
    host_authority_records: Arc<RwLock<BTreeMap<(MobId, String), MobHostAuthorityRecord>>>,
    host_binding_generation_highwaters: Arc<RwLock<BTreeMap<(MobId, String), u64>>>,
    member_operator_requests: Arc<RwLock<MemberOperatorRequestMap>>,
    placed_spawns: Arc<RwLock<BTreeMap<(MobId, String), MobPlacedSpawnCarrierRecord>>>,
    operator_grants: Arc<RwLock<BTreeMap<(MobId, String), MobOperatorGrantRecord>>>,
    member_event_cursors: Arc<RwLock<BTreeMap<(MobId, String), MobMemberEventCursorRecord>>>,
    member_live_cleanup_records: Arc<RwLock<BTreeMap<(MobId, String), MobMemberLiveCleanupRecord>>>,
    external_binding_overlays: Arc<RwLock<ExternalBindingOverlayMap>>,
}

impl InMemoryMobRuntimeMetadataStore {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub(crate) async fn seed_legacy_supervisor_authority(
        &self,
        mob_id: &MobId,
        record: SupervisorAuthorityRecord,
    ) {
        self.supervisor_records
            .write()
            .await
            .insert(mob_id.clone(), record);
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

    async fn load_mob_host_authority(
        &self,
        mob_id: &MobId,
        host_id: &str,
    ) -> Result<Option<MobHostAuthorityRecord>, MobStoreError> {
        Ok(self
            .host_authority_records
            .read()
            .await
            .get(&(mob_id.clone(), host_id.to_string()))
            .cloned())
    }

    async fn list_mob_host_authorities(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobHostAuthorityRecord>, MobStoreError> {
        Ok(self
            .host_authority_records
            .read()
            .await
            .iter()
            .filter(|((stored_mob_id, _), _)| stored_mob_id == mob_id)
            .map(|(_, record)| record.clone())
            .collect())
    }

    async fn put_mob_host_authority(
        &self,
        mob_id: &MobId,
        record: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityPersistenceAuthority,
    ) -> Result<(), MobStoreError> {
        authority.verify_record(record)?;
        self.host_authority_records
            .write()
            .await
            .insert((mob_id.clone(), record.host_id.clone()), record.clone());
        Ok(())
    }

    async fn put_mob_host_authority_if_absent(
        &self,
        mob_id: &MobId,
        record: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(record)?;
        let key = (mob_id.clone(), record.host_id.clone());
        let mut guard = self.host_authority_records.write().await;
        if guard.contains_key(&key) {
            return Ok(false);
        }
        guard.insert(key, record.clone());
        Ok(true)
    }

    async fn compare_and_put_mob_host_authority(
        &self,
        mob_id: &MobId,
        expected: &MobHostAuthorityRecord,
        record: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(record)?;
        let key = (mob_id.clone(), record.host_id.clone());
        let mut guard = self.host_authority_records.write().await;
        if guard.get(&key) != Some(expected) {
            return Ok(false);
        }
        guard.insert(key, record.clone());
        Ok(true)
    }

    async fn delete_mob_host_authority(
        &self,
        mob_id: &MobId,
        expected: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityDeletionAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(expected)?;
        let key = (mob_id.clone(), expected.host_id.clone());
        let mut guard = self.host_authority_records.write().await;
        if guard.get(&key) != Some(expected) {
            return Ok(false);
        }
        guard.remove(&key);
        Ok(true)
    }

    async fn put_mob_host_binding_generation_highwater(
        &self,
        mob_id: &MobId,
        expected: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityDeletionAuthority,
    ) -> Result<(), MobStoreError> {
        authority.verify_record(expected)?;
        let mut guard = self.host_binding_generation_highwaters.write().await;
        let highwater = guard
            .entry((mob_id.clone(), expected.host_id.clone()))
            .or_insert(0);
        *highwater = (*highwater).max(expected.binding_generation);
        Ok(())
    }

    async fn list_mob_host_binding_generation_highwaters(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<(String, u64)>, MobStoreError> {
        Ok(self
            .host_binding_generation_highwaters
            .read()
            .await
            .iter()
            .filter(|((stored_mob_id, _), _)| stored_mob_id == mob_id)
            .map(|((_, host_id), generation)| (host_id.clone(), *generation))
            .collect())
    }

    async fn begin_member_operator_request(
        &self,
        mob_id: &MobId,
        record: &MobMemberOperatorRequestRecord,
    ) -> Result<MobMemberOperatorRequestBegin, MobStoreError> {
        record.validate_pending()?;
        let key = (mob_id.clone(), record.key());
        let mut guard = self.member_operator_requests.write().await;
        if let Some(existing) = guard.get(&key) {
            existing.validate()?;
            return Ok(MobMemberOperatorRequestBegin::Existing(existing.clone()));
        }
        let incarnation_rows = guard
            .keys()
            .filter(|(stored_mob, stored_key)| {
                stored_mob == mob_id
                    && stored_key.agent_identity == record.agent_identity
                    && stored_key.generation == record.generation
                    && stored_key.fence_token == record.fence_token
                    && stored_key.host_id == record.host_id
                    && stored_key.host_binding_generation == record.host_binding_generation
                    && stored_key.member_session_id == record.member_session_id
            })
            .count();
        if incarnation_rows >= super::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION {
            return Err(MobStoreError::WriteFailed(format!(
                "member operator request quota exhausted for '{}' generation {} fence {} host '{}' binding generation {} session '{}' (max {})",
                record.agent_identity,
                record.generation,
                record.fence_token,
                record.host_id,
                record.host_binding_generation,
                record.member_session_id,
                super::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION,
            )));
        }
        let mob_rows = guard
            .keys()
            .filter(|(stored_mob, ..)| stored_mob == mob_id)
            .count();
        if mob_rows >= super::MEMBER_OPERATOR_REQUEST_MAX_PER_MOB {
            return Err(MobStoreError::WriteFailed(format!(
                "member operator request quota exhausted for mob '{}' (max {})",
                mob_id,
                super::MEMBER_OPERATOR_REQUEST_MAX_PER_MOB,
            )));
        }
        guard.insert(key, record.clone());
        Ok(MobMemberOperatorRequestBegin::Started)
    }

    async fn load_member_operator_request(
        &self,
        mob_id: &MobId,
        key: &MobMemberOperatorRequestKey,
    ) -> Result<Option<MobMemberOperatorRequestRecord>, MobStoreError> {
        let record = self
            .member_operator_requests
            .read()
            .await
            .get(&(mob_id.clone(), key.clone()))
            .cloned();
        if let Some(record) = &record {
            record.validate()?;
        }
        Ok(record)
    }

    async fn compare_and_put_member_operator_request(
        &self,
        mob_id: &MobId,
        expected: &MobMemberOperatorRequestRecord,
        record: &MobMemberOperatorRequestRecord,
    ) -> Result<bool, MobStoreError> {
        expected.validate_terminal_transition(record)?;
        let key = (mob_id.clone(), expected.key());
        let mut guard = self.member_operator_requests.write().await;
        if guard.get(&key) != Some(expected) {
            return Ok(false);
        }
        guard.insert(key, record.clone());
        Ok(true)
    }

    async fn list_member_operator_requests(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobMemberOperatorRequestRecord>, MobStoreError> {
        let records = self
            .member_operator_requests
            .read()
            .await
            .iter()
            .filter(|((stored_mob_id, _), _)| stored_mob_id == mob_id)
            .map(|(_, record)| record.clone())
            .collect::<Vec<_>>();
        for record in &records {
            record.validate()?;
        }
        Ok(records)
    }

    async fn prune_stale_member_operator_requests(
        &self,
        mob_id: &MobId,
        authority: &MobMemberOperatorPruneAuthority,
    ) -> Result<u64, MobStoreError> {
        let mut guard = self.member_operator_requests.write().await;
        for ((stored_mob_id, ..), record) in guard.iter() {
            if stored_mob_id == mob_id {
                record.validate()?;
            }
        }
        let before = guard.len();
        guard.retain(|(stored_mob_id, ..), record| {
            stored_mob_id != mob_id || authority.preserves(record)
        });
        u64::try_from(before - guard.len()).map_err(|_| {
            MobStoreError::Internal("member operator request prune count overflow".to_string())
        })
    }

    async fn delete_member_operator_requests(&self, mob_id: &MobId) -> Result<u64, MobStoreError> {
        let mut guard = self.member_operator_requests.write().await;
        let before = guard.len();
        guard.retain(|(stored_mob_id, _), _| stored_mob_id != mob_id);
        u64::try_from(before - guard.len()).map_err(|_| {
            MobStoreError::Internal("member operator request delete count overflow".to_string())
        })
    }

    async fn load_placed_spawn(
        &self,
        mob_id: &MobId,
        agent_identity: &str,
    ) -> Result<Option<MobPlacedSpawnCarrierRecord>, MobStoreError> {
        let record = self
            .placed_spawns
            .read()
            .await
            .get(&(mob_id.clone(), agent_identity.to_string()))
            .cloned();
        if let Some(record) = &record {
            record.validate_for_store_key(mob_id, agent_identity)?;
        }
        Ok(record)
    }

    async fn list_placed_spawns(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobPlacedSpawnCarrierRecord>, MobStoreError> {
        let mut keyed_records = self
            .placed_spawns
            .read()
            .await
            .iter()
            .filter(|((stored_mob_id, _), _)| stored_mob_id == mob_id)
            .map(|((_, agent_identity), record)| (agent_identity.clone(), record.clone()))
            .collect::<Vec<_>>();
        keyed_records.sort_by(|left, right| left.0.cmp(&right.0));
        for (agent_identity, record) in &keyed_records {
            record.validate_for_store_key(mob_id, agent_identity)?;
        }
        let records = keyed_records
            .into_iter()
            .map(|(_, record)| record)
            .collect::<Vec<_>>();
        Ok(records)
    }

    async fn begin_placed_spawn_if_absent(
        &self,
        mob_id: &MobId,
        record: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnPendingPersistenceAuthority,
    ) -> Result<BeginPlacedSpawnResult, MobStoreError> {
        record.validate_for_mob(mob_id)?;
        authority.verify_record(record)?;
        let key = (mob_id.clone(), record.agent_identity.clone());
        let mut guard = self.placed_spawns.write().await;
        let Some(existing) = guard.get(&key) else {
            guard.insert(key, record.clone());
            return Ok(BeginPlacedSpawnResult::Inserted);
        };
        existing.validate_for_store_key(mob_id, &record.agent_identity)?;
        if !existing.same_attempt_as(record) {
            return Ok(BeginPlacedSpawnResult::Conflict);
        }
        Ok(match existing.phase {
            PlacedSpawnCarrierPhase::Pending => BeginPlacedSpawnResult::ExistingExactPending,
            PlacedSpawnCarrierPhase::Committed(_) => BeginPlacedSpawnResult::ExistingExactCommitted,
        })
    }

    async fn compare_and_commit_placed_spawn(
        &self,
        mob_id: &MobId,
        expected_pending: &MobPlacedSpawnCarrierRecord,
        committed: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnCommitPersistenceAuthority,
    ) -> Result<CommitPlacedSpawnResult, MobStoreError> {
        expected_pending.validate_for_mob(mob_id)?;
        committed.validate_for_mob(mob_id)?;
        authority.verify_record(committed)?;
        if !matches!(expected_pending.phase, PlacedSpawnCarrierPhase::Pending)
            || !expected_pending.same_attempt_as(committed)
        {
            return Err(MobStoreError::Internal(
                "placed-spawn commit CAS changed its attempt tuple or expected non-pending state"
                    .to_string(),
            ));
        }
        let key = (mob_id.clone(), expected_pending.agent_identity.clone());
        let mut guard = self.placed_spawns.write().await;
        let Some(existing) = guard.get(&key) else {
            return Ok(CommitPlacedSpawnResult::Conflict);
        };
        existing.validate_for_store_key(mob_id, &expected_pending.agent_identity)?;
        if existing == committed {
            return Ok(CommitPlacedSpawnResult::AlreadyCommittedExact);
        }
        if existing == expected_pending {
            guard.insert(key, committed.clone());
            return Ok(CommitPlacedSpawnResult::Committed);
        }
        if existing.same_attempt_as(expected_pending)
            && matches!(existing.phase, PlacedSpawnCarrierPhase::Pending)
        {
            return Ok(CommitPlacedSpawnResult::StillPending);
        }
        Ok(CommitPlacedSpawnResult::Conflict)
    }

    async fn compare_and_promote_placed_spawn_binding(
        &self,
        mob_id: &MobId,
        expected: &MobPlacedSpawnCarrierRecord,
        promoted: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnBindingPromotionAuthority,
    ) -> Result<PromotePlacedSpawnBindingResult, MobStoreError> {
        expected.validate_for_mob(mob_id)?;
        promoted.validate_for_mob(mob_id)?;
        authority.verify_records(expected, promoted)?;
        let key = (mob_id.clone(), expected.agent_identity.clone());
        let mut guard = self.placed_spawns.write().await;
        let Some(existing) = guard.get(&key) else {
            return Ok(PromotePlacedSpawnBindingResult::Conflict);
        };
        existing.validate_for_store_key(mob_id, &expected.agent_identity)?;
        if existing == promoted {
            return Ok(PromotePlacedSpawnBindingResult::AlreadyPromotedExact);
        }
        if existing != expected {
            return Ok(PromotePlacedSpawnBindingResult::Conflict);
        }
        guard.insert(key, promoted.clone());
        Ok(PromotePlacedSpawnBindingResult::Promoted)
    }

    async fn compare_and_delete_placed_spawn(
        &self,
        mob_id: &MobId,
        expected: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnCleanupAuthority,
    ) -> Result<DeletePlacedSpawnResult, MobStoreError> {
        expected.validate_for_mob(mob_id)?;
        authority.verify_record(expected)?;
        let key = (mob_id.clone(), expected.agent_identity.clone());
        let mut guard = self.placed_spawns.write().await;
        match guard.get(&key) {
            None => Ok(DeletePlacedSpawnResult::AlreadyAbsent),
            Some(current) => {
                current.validate_for_store_key(mob_id, &expected.agent_identity)?;
                if current != expected {
                    return Ok(DeletePlacedSpawnResult::Conflict);
                }
                guard.remove(&key);
                Ok(DeletePlacedSpawnResult::Deleted)
            }
        }
    }

    async fn list_mob_operator_grants(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobOperatorGrantRecord>, MobStoreError> {
        Ok(self
            .operator_grants
            .read()
            .await
            .iter()
            .filter(|((stored_mob_id, _), _)| stored_mob_id == mob_id)
            .map(|(_, record)| record.clone())
            .collect())
    }

    async fn put_mob_operator_grant(
        &self,
        mob_id: &MobId,
        record: &MobOperatorGrantRecord,
        authority: &MobOperatorGrantPersistenceAuthority,
    ) -> Result<(), MobStoreError> {
        authority.verify_record(record)?;
        self.operator_grants
            .write()
            .await
            .insert((mob_id.clone(), record.principal.clone()), record.clone());
        Ok(())
    }

    async fn delete_mob_operator_grant(
        &self,
        mob_id: &MobId,
        principal: &str,
        authority: &MobOperatorGrantDeletionAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_principal(principal)?;
        Ok(self
            .operator_grants
            .write()
            .await
            .remove(&(mob_id.clone(), principal.to_string()))
            .is_some())
    }

    async fn delete_mob_operator_grants(&self, mob_id: &MobId) -> Result<u64, MobStoreError> {
        let mut guard = self.operator_grants.write().await;
        let before = guard.len();
        guard.retain(|(stored_mob_id, _), _| stored_mob_id != mob_id);
        Ok((before - guard.len()) as u64)
    }

    async fn list_member_event_cursors(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobMemberEventCursorRecord>, MobStoreError> {
        Ok(self
            .member_event_cursors
            .read()
            .await
            .iter()
            .filter(|((stored_mob_id, _), _)| stored_mob_id == mob_id)
            .map(|(_, record)| record.clone())
            .collect())
    }

    async fn put_member_event_cursor(
        &self,
        mob_id: &MobId,
        record: &MobMemberEventCursorRecord,
    ) -> Result<(), MobStoreError> {
        self.member_event_cursors.write().await.insert(
            (mob_id.clone(), record.agent_identity.clone()),
            record.clone(),
        );
        Ok(())
    }

    async fn delete_member_event_cursor(
        &self,
        mob_id: &MobId,
        agent_identity: &str,
    ) -> Result<bool, MobStoreError> {
        Ok(self
            .member_event_cursors
            .write()
            .await
            .remove(&(mob_id.clone(), agent_identity.to_string()))
            .is_some())
    }

    async fn delete_member_event_cursors(&self, mob_id: &MobId) -> Result<u64, MobStoreError> {
        let mut guard = self.member_event_cursors.write().await;
        let before = guard.len();
        guard.retain(|(stored_mob_id, _), _| stored_mob_id != mob_id);
        Ok((before - guard.len()) as u64)
    }

    async fn list_member_live_cleanup_records(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobMemberLiveCleanupRecord>, MobStoreError> {
        Ok(self
            .member_live_cleanup_records
            .read()
            .await
            .iter()
            .filter(|((stored_mob_id, _), _)| stored_mob_id == mob_id)
            .map(|(_, record)| record.clone())
            .collect())
    }

    async fn put_member_live_cleanup_record_if_absent(
        &self,
        mob_id: &MobId,
        record: &MobMemberLiveCleanupRecord,
    ) -> Result<bool, MobStoreError> {
        let key = (mob_id.clone(), record.cleanup_id.clone());
        let mut records = self.member_live_cleanup_records.write().await;
        match records.get(&key) {
            Some(existing) if existing == record => Ok(false),
            Some(_) => Err(MobStoreError::Internal(format!(
                "member-live cleanup id '{}' was reused for a conflicting record",
                record.cleanup_id
            ))),
            None => {
                records.insert(key, record.clone());
                Ok(true)
            }
        }
    }

    async fn delete_member_live_cleanup_record(
        &self,
        mob_id: &MobId,
        expected: &MobMemberLiveCleanupRecord,
    ) -> Result<bool, MobStoreError> {
        let key = (mob_id.clone(), expected.cleanup_id.clone());
        let mut records = self.member_live_cleanup_records.write().await;
        match records.get(&key) {
            None => Ok(false),
            Some(current) if current == expected => {
                records.remove(&key);
                Ok(true)
            }
            Some(_) => Err(MobStoreError::Internal(format!(
                "member-live cleanup id '{}' no longer matches its expected immutable record",
                expected.cleanup_id
            ))),
        }
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
        if matches!(&event.kind, MobEventKind::MembersUnwired { .. })
            && self
                .fail_next_members_unwired_before_append
                .swap(false, Ordering::Relaxed)
        {
            self.arm_members_unwired_reconciliation_read_failures();
            return Err(MobStoreError::Internal(
                "forced mob event store MembersUnwired failure before append".to_string(),
            ));
        }

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
        #[cfg(any(test, feature = "test-support"))]
        if matches!(&stored.kind, MobEventKind::MembersUnwired { .. })
            && self
                .fail_next_members_unwired_after_append
                .swap(false, Ordering::Relaxed)
        {
            self.arm_members_unwired_reconciliation_read_failures();
            return Err(MobStoreError::Internal(
                "forced mob event store MembersUnwired lost append acknowledgement".to_string(),
            ));
        }
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

    async fn append_step_failed_event_if_absent(
        &self,
        event: NewMobEvent,
    ) -> Result<Option<MobEvent>, MobStoreError> {
        validate_mob_event_write_authority(&event.kind)?;
        let Some((run_id, step_id, _)) = step_failed_event_identity(&event.kind) else {
            return Err(MobStoreError::Internal(
                "append_step_failed_event_if_absent requires a StepFailed event".to_string(),
            ));
        };
        let run_id = run_id.clone();
        let step_id = step_id.clone();
        let mob_id = event.mob_id.clone();

        let mut events = self.events.write().await;
        let mut exact_replay = false;
        for existing in events.iter() {
            if existing.mob_id != mob_id {
                continue;
            }
            let Some((existing_run_id, existing_step_id, _)) =
                step_failed_event_identity(&existing.kind)
            else {
                continue;
            };
            if existing_run_id == &run_id && existing_step_id == &step_id {
                if existing.kind != event.kind {
                    return Err(MobStoreError::Internal(format!(
                        "StepFailed event conflict for run '{run_id}' step '{step_id}'"
                    )));
                }
                exact_replay = true;
            }
        }
        if exact_replay {
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
        #[cfg(any(test, feature = "test-support"))]
        if Self::consume_read_failure(&self.active_reconciliation_poll_failures) {
            return Err(MobStoreError::Internal(
                "forced mob event store reconciliation poll failure".to_string(),
            ));
        }
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
        #[cfg(any(test, feature = "test-support"))]
        if Self::consume_read_failure(&self.active_reconciliation_latest_cursor_failures) {
            return Err(MobStoreError::Internal(
                "forced mob event store reconciliation latest-cursor failure".to_string(),
            ));
        }
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
    remote_turn_intents: Arc<RwLock<BTreeMap<(RunId, u64), MobRunRemoteTurnIntent>>>,
    remote_turn_receipts: Arc<RwLock<BTreeMap<(RunId, u64), MobRunRemoteTurnReceipt>>>,
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

    async fn put_remote_turn_intent(
        &self,
        run_id: &RunId,
        intent: &MobRunRemoteTurnIntent,
    ) -> Result<bool, MobStoreError> {
        if &intent.obligation.run_id != run_id || intent.obligation.dispatch_sequence == 0 {
            return Err(MobStoreError::Internal(format!(
                "remote-turn intent does not match run '{run_id}' or has sequence zero"
            )));
        }
        let runs = self.runs.read().await;
        let run = runs
            .get(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run '{run_id}'")))?;
        intent
            .validate_for(run_id, &run.mob_id)
            .map_err(MobStoreError::Internal)?;
        drop(runs);
        let key = (run_id.clone(), intent.obligation.dispatch_sequence);
        let mut intents = self.remote_turn_intents.write().await;
        match intents.get(&key) {
            Some(existing) if existing == intent => Ok(false),
            Some(_) => Err(MobStoreError::Internal(format!(
                "remote-turn intent sequence {} conflicts for run '{run_id}'",
                intent.obligation.dispatch_sequence
            ))),
            None => {
                intents.insert(key, intent.clone());
                Ok(true)
            }
        }
    }

    async fn delete_remote_turn_intent(
        &self,
        run_id: &RunId,
        dispatch_sequence: u64,
    ) -> Result<bool, MobStoreError> {
        Ok(self
            .remote_turn_intents
            .write()
            .await
            .remove(&(run_id.clone(), dispatch_sequence))
            .is_some())
    }

    async fn list_remote_turn_intents(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<MobRunRemoteTurnIntent>, MobStoreError> {
        Ok(self
            .remote_turn_intents
            .read()
            .await
            .iter()
            .filter(|((intent_run_id, _), _)| intent_run_id == run_id)
            .map(|(_, intent)| intent.clone())
            .collect())
    }

    async fn put_remote_turn_receipt(
        &self,
        run_id: &RunId,
        receipt: &MobRunRemoteTurnReceipt,
    ) -> Result<bool, MobStoreError> {
        if &receipt.obligation.run_id != run_id || receipt.obligation.dispatch_sequence == 0 {
            return Err(MobStoreError::Internal(format!(
                "remote-turn receipt does not match run '{run_id}' or has sequence zero"
            )));
        }
        let runs = self.runs.read().await;
        let run = runs
            .get(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run '{run_id}'")))?;
        let intents = self.remote_turn_intents.read().await;
        let intent = intents
            .get(&(run_id.clone(), receipt.obligation.dispatch_sequence))
            .ok_or_else(|| {
                MobStoreError::Internal(
                    "remote-turn receipt has no exact durable intent".to_string(),
                )
            })?;
        receipt
            .validate_for(run_id, &run.mob_id, intent)
            .map_err(MobStoreError::Internal)?;
        drop(intents);
        drop(runs);
        let key = (run_id.clone(), receipt.obligation.dispatch_sequence);
        let mut receipts = self.remote_turn_receipts.write().await;
        match receipts.get(&key) {
            Some(existing) if existing == receipt => Ok(false),
            Some(_) => Err(MobStoreError::Internal(format!(
                "remote-turn receipt sequence {} conflicts for run '{run_id}'",
                receipt.obligation.dispatch_sequence
            ))),
            None => {
                receipts.insert(key, receipt.clone());
                Ok(true)
            }
        }
    }

    async fn list_remote_turn_receipts(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<MobRunRemoteTurnReceipt>, MobStoreError> {
        Ok(self
            .remote_turn_receipts
            .read()
            .await
            .iter()
            .filter(|((receipt_run_id, _), _)| receipt_run_id == run_id)
            .map(|(_, receipt)| receipt.clone())
            .collect())
    }

    async fn delete_remote_turn_receipt(
        &self,
        run_id: &RunId,
        dispatch_sequence: u64,
    ) -> Result<bool, MobStoreError> {
        Ok(self
            .remote_turn_receipts
            .write()
            .await
            .remove(&(run_id.clone(), dispatch_sequence))
            .is_some())
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

    async fn append_failure_entry_if_absent_with_authority(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<bool, MobStoreError> {
        let mut runs = self.runs.write().await;
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
        authority
            .validate_failure_entry(run, &entry)
            .map_err(|error| MobStoreError::Internal(error.to_string()))?;
        if let Some(existing) = run
            .failure_ledger
            .iter()
            .find(|existing| existing.step_id == entry.step_id)
        {
            if existing.reason == entry.reason
                && existing.error_report == entry.error_report
                && existing.error == entry.error
            {
                return Ok(false);
            }
            return Err(MobStoreError::Internal(format!(
                "failure ledger conflict for run '{run_id}' step '{}'",
                entry.step_id
            )));
        }
        let mut candidate = run.clone();
        candidate.failure_ledger.push(entry);
        validate_authorized_run_projection(&candidate)?;
        *run = candidate;
        Ok(true)
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

    fn operator_request(
        identity: &str,
        generation: u64,
        sequence: usize,
    ) -> MobMemberOperatorRequestRecord {
        MobMemberOperatorRequestRecord::pending(
            MobMemberOperatorRequestKey::new(
                identity,
                generation,
                1,
                "host-a",
                1,
                "member-session-a",
                format!("request-{sequence}"),
            ),
            "0".repeat(64),
        )
    }

    #[tokio::test]
    async fn member_operator_request_key_separates_host_generation_and_member_session() {
        let mob_id = MobId::from("operator-execution-fence-key");
        let store = InMemoryMobRuntimeMetadataStore::new();
        let base = MobMemberOperatorRequestRecord::pending(
            MobMemberOperatorRequestKey::new(
                "member-a",
                7,
                11,
                "host-a",
                1,
                "member-session-a",
                "same-request-id",
            ),
            "0".repeat(64),
        );
        let mut host_generation_two = base.clone();
        host_generation_two.host_binding_generation = 2;
        let mut replacement_session = host_generation_two.clone();
        replacement_session.member_session_id = "member-session-b".to_string();

        for record in [&base, &host_generation_two, &replacement_session] {
            assert_eq!(
                store
                    .begin_member_operator_request(&mob_id, record)
                    .await
                    .expect("insert exact execution-fence key"),
                MobMemberOperatorRequestBegin::Started,
            );
        }
        assert!(matches!(
            store
                .begin_member_operator_request(&mob_id, &base)
                .await
                .expect("exact duplicate replays"),
            MobMemberOperatorRequestBegin::Existing(existing) if existing == base
        ));

        for record in [&base, &host_generation_two, &replacement_session] {
            assert_eq!(
                store
                    .load_member_operator_request(&mob_id, &record.key())
                    .await
                    .expect("load exact execution-fence key"),
                Some(record.clone()),
            );
        }
        assert_eq!(
            store
                .list_member_operator_requests(&mob_id)
                .await
                .expect("list distinct execution-fence rows")
                .len(),
            3,
        );
    }

    #[tokio::test]
    async fn member_operator_request_ledger_bounds_keys_and_both_quotas() {
        let mob_id = MobId::from("operator-quota");
        let store = InMemoryMobRuntimeMetadataStore::new();
        let oversized = MobMemberOperatorRequestRecord::pending(
            MobMemberOperatorRequestKey::new(
                "member",
                1,
                1,
                "host-a",
                1,
                "member-session-a",
                "x".repeat(crate::store::MEMBER_OPERATOR_REQUEST_ID_MAX_BYTES + 1),
            ),
            "0".repeat(64),
        );
        assert!(
            store
                .begin_member_operator_request(&mob_id, &oversized)
                .await
                .is_err(),
            "oversized idempotency keys must fail before persistence"
        );

        for sequence in 0..crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION {
            assert!(matches!(
                store
                    .begin_member_operator_request(
                        &mob_id,
                        &operator_request("member-a", 1, sequence),
                    )
                    .await
                    .expect("row below per-incarnation ceiling"),
                MobMemberOperatorRequestBegin::Started
            ));
        }
        assert!(
            store
                .begin_member_operator_request(
                    &mob_id,
                    &operator_request(
                        "member-a",
                        1,
                        crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION,
                    ),
                )
                .await
                .is_err(),
            "a new key at the per-incarnation ceiling must be rejected"
        );
        assert!(matches!(
            store
                .begin_member_operator_request(&mob_id, &operator_request("member-a", 1, 0),)
                .await
                .expect("duplicates replay even at the ceiling"),
            MobMemberOperatorRequestBegin::Existing(_)
        ));

        for member in 1..4 {
            for sequence in 0..crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION {
                store
                    .begin_member_operator_request(
                        &mob_id,
                        &operator_request(&format!("member-{member}"), 1, sequence),
                    )
                    .await
                    .expect("row below global ceiling");
            }
        }
        assert!(
            store
                .begin_member_operator_request(
                    &mob_id,
                    &operator_request("member-global-overflow", 1, 0),
                )
                .await
                .is_err(),
            "cross-incarnation growth must stop at the per-mob hard ceiling"
        );
        assert!(matches!(
            store
                .begin_member_operator_request(&mob_id, &operator_request("member-3", 1, 0),)
                .await
                .expect("global ceiling still replays existing keys"),
            MobMemberOperatorRequestBegin::Existing(_)
        ));
    }

    #[tokio::test]
    async fn member_operator_prune_reclaims_stale_rows_but_preserves_current_replay_and_caps() {
        use meerkat_contracts::wire::supervisor_bridge::{
            MemberOperatorOutcome, MemberOperatorReply, WireOpaqueJson,
        };

        let mob_id = MobId::from("operator-prune-capacity");
        let store = InMemoryMobRuntimeMetadataStore::new();
        for sequence in 0..crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION {
            store
                .begin_member_operator_request(
                    &mob_id,
                    &operator_request("member-current", 1, sequence),
                )
                .await
                .expect("seed current incarnation to its ceiling");
        }
        let current_pending = operator_request("member-current", 1, 0);
        let current_terminal = current_pending
            .terminal(MemberOperatorReply {
                request_id: current_pending.request_id.clone(),
                outcome: MemberOperatorOutcome::Completed {
                    result: WireOpaqueJson::from_value(&serde_json::json!({"ok": true})),
                },
            })
            .expect("terminalize current replay row");
        assert!(
            store
                .compare_and_put_member_operator_request(
                    &mob_id,
                    &current_pending,
                    &current_terminal,
                )
                .await
                .expect("persist current terminal")
        );
        for stale in 0..3 {
            for sequence in 0..crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION {
                store
                    .begin_member_operator_request(
                        &mob_id,
                        &operator_request(&format!("member-stale-{stale}"), 1, sequence),
                    )
                    .await
                    .expect("seed stale incarnation to the global ceiling");
            }
        }
        assert!(
            store
                .begin_member_operator_request(&mob_id, &operator_request("member-new", 1, 0))
                .await
                .is_err(),
            "the global cap must remain fail closed before actor-authorized pruning"
        );

        let authority = MobMemberOperatorPruneAuthority::from_actor_current_residencies(
            std::collections::BTreeSet::from([
                crate::store::MobMemberOperatorResidency {
                    agent_identity: "member-current".to_string(),
                    generation: 1,
                    fence_token: 1,
                    host_id: "host-a".to_string(),
                    host_binding_generation: 1,
                    member_session_id: "member-session-a".to_string(),
                },
                crate::store::MobMemberOperatorResidency {
                    agent_identity: "member-new".to_string(),
                    generation: 1,
                    fence_token: 1,
                    host_id: "host-a".to_string(),
                    host_binding_generation: 1,
                    member_session_id: "member-session-a".to_string(),
                },
            ]),
        );
        assert_eq!(
            store
                .prune_stale_member_operator_requests(&mob_id, &authority)
                .await
                .expect("prune stale in-memory incarnations"),
            3 * crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION as u64,
        );
        assert_eq!(
            store
                .begin_member_operator_request(&mob_id, &current_pending)
                .await
                .expect("current terminal still replays after prune"),
            MobMemberOperatorRequestBegin::Existing(current_terminal),
        );
        assert!(
            store
                .begin_member_operator_request(
                    &mob_id,
                    &operator_request(
                        "member-current",
                        1,
                        crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION,
                    ),
                )
                .await
                .is_err(),
            "pruning must not weaken the current incarnation ceiling"
        );
        assert_eq!(
            store
                .begin_member_operator_request(&mob_id, &operator_request("member-new", 1, 0))
                .await
                .expect("stale rows must free global capacity"),
            MobMemberOperatorRequestBegin::Started,
        );
    }

    fn default_bridge_protocol_version()
    -> meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion {
        meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_default_protocol_version()
    }

    fn sample_definition() -> MobDefinition {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("worker"),
            ProfileBinding::Inline(Box::new(Profile {
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
            })),
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
                    remote_turn_obligation: None,
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
                pubkey: [7u8; 32],
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

    #[tokio::test]
    async fn test_runtime_metadata_store_roundtrips_mob_host_authority_records() {
        let store = InMemoryMobRuntimeMetadataStore::new();
        let mob_id = MobId::from("mob-hosts");
        let other_mob = MobId::from("mob-other");
        let host_b = crate::store::sample_mob_host_authority_record("host-peer-b", 1);
        let host_c = crate::store::sample_mob_host_authority_record("host-peer-c", 2);
        let host_b_authority =
            crate::store::mob_host_authority_persistence_authority_for_record(&host_b).unwrap();
        let host_c_authority =
            crate::store::mob_host_authority_persistence_authority_for_record(&host_c).unwrap();

        assert!(
            store
                .put_mob_host_authority_if_absent(&mob_id, &host_b, &host_b_authority)
                .await
                .unwrap()
        );
        assert!(
            !store
                .put_mob_host_authority_if_absent(&mob_id, &host_b, &host_b_authority)
                .await
                .unwrap(),
            "duplicate (mob, host) insert must be ignored"
        );
        store
            .put_mob_host_authority(&mob_id, &host_c, &host_c_authority)
            .await
            .unwrap();
        store
            .put_mob_host_authority(&other_mob, &host_b, &host_b_authority)
            .await
            .unwrap();

        assert_eq!(
            store
                .load_mob_host_authority(&mob_id, "host-peer-b")
                .await
                .unwrap(),
            Some(host_b.clone())
        );
        assert_eq!(
            store.list_mob_host_authorities(&mob_id).await.unwrap(),
            vec![host_b.clone(), host_c.clone()],
            "listing is mob-scoped and host-id ordered"
        );

        // Rebind: CAS to the next epoch under a rebind-witnessed authority.
        let host_b_rebound = MobHostAuthorityRecord {
            authority_epoch: 2,
            live_endpoint: None,
            ..host_b.clone()
        };
        let rebound_authority =
            crate::store::mob_host_authority_persistence_authority_for_record(&host_b_rebound)
                .unwrap();
        assert!(
            !store
                .compare_and_put_mob_host_authority(
                    &mob_id,
                    &host_b_rebound,
                    &host_b_rebound,
                    &rebound_authority
                )
                .await
                .unwrap(),
            "mismatched expected record must not update"
        );
        assert!(
            store
                .compare_and_put_mob_host_authority(
                    &mob_id,
                    &host_b,
                    &host_b_rebound,
                    &rebound_authority
                )
                .await
                .unwrap()
        );
        assert_eq!(
            store
                .load_mob_host_authority(&mob_id, "host-peer-b")
                .await
                .unwrap(),
            Some(host_b_rebound.clone())
        );

        // Revoke: delete requires the exact expected record + a revoke
        // witness; other mobs' rows survive (A14 isolation).
        let deletion =
            crate::store::mob_host_authority_deletion_authority_for_record(&host_b_rebound)
                .unwrap();
        store
            .put_mob_host_binding_generation_highwater(&mob_id, &host_b_rebound, &deletion)
            .await
            .unwrap();
        assert!(
            !store
                .delete_mob_host_authority(&mob_id, &host_b, &deletion)
                .await
                .unwrap(),
            "stale expected record must not delete"
        );
        assert!(
            store
                .delete_mob_host_authority(&mob_id, &host_b_rebound, &deletion)
                .await
                .unwrap()
        );
        assert!(
            store
                .load_mob_host_authority(&mob_id, "host-peer-b")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .list_mob_host_binding_generation_highwaters(&mob_id)
                .await
                .unwrap(),
            vec![("host-peer-b".to_string(), host_b_rebound.binding_generation)],
            "the non-prunable generation tombstone survives active-row deletion",
        );
        assert_eq!(
            store.list_mob_host_authorities(&other_mob).await.unwrap(),
            vec![host_b],
            "another mob's binding for the same host must survive"
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
