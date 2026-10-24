//! The mob store traits over one host-store-journaled storage.
//!
//! Every method runs the in-memory store's own implementation, so the
//! semantics are exactly the in-memory stores'. A reading method locks the
//! scopes it reads; a writing method self-fences, locks the scopes it
//! writes, runs, and commits what it touched as one batch before it answers.
//! Next to each method: the scopes it locks (M mob, I identity, R runs,
//! S statuses) and the entry groups it exports.

use std::sync::Arc;

use async_trait::async_trait;

use super::*;
use crate::forked_participant::{
    ForkedParticipantCapabilityId, ForkedParticipantRef, ForkedParticipantRequestId,
};
use crate::store::in_memory::identity_receipt_key;
// The in-memory event store's sealed half, called through by the adapter.
use crate::store::private::MobEventStoreSealed as _;

fn mob_read() -> Locks {
    Locks {
        mob: true,
        ..Locks::default()
    }
}

fn identity_read() -> Locks {
    Locks {
        identity: true,
        ..Locks::default()
    }
}

fn mob_and_identity_read() -> Locks {
    Locks {
        mob: true,
        identity: true,
        ..Locks::default()
    }
}

fn run_read() -> Locks {
    Locks {
        runs: true,
        ..Locks::default()
    }
}

fn status_read() -> Locks {
    Locks {
        statuses: true,
        ..Locks::default()
    }
}

fn appended() -> Touch {
    Touch {
        events: Some(EventsTouch::Appended),
        ..Touch::default()
    }
}

fn delivery(intent: &MobExternalDeliveryIntent) -> Touch {
    Touch {
        delivery: Some((
            intent.mob_id.clone(),
            intent.identity.idempotency_key.clone(),
        )),
        ..Touch::default()
    }
}

fn runtime_metadata() -> Touch {
    Touch {
        runtime_metadata: true,
        ..Touch::default()
    }
}

fn run(run_id: &RunId) -> Touch {
    Touch {
        run: Some(run_id.clone()),
        ..Touch::default()
    }
}

// ---------------------------------------------------------------------------
// Events (scope M)
// ---------------------------------------------------------------------------

/// [`MobEventStore`] over the storage's mob scope.
#[derive(Debug, Clone)]
pub struct SpiMobEventStore {
    bundle: Arc<Bundle>,
}

impl SpiMobEventStore {
    pub(super) fn new(bundle: Arc<Bundle>) -> Self {
        Self { bundle }
    }
}

#[async_trait]
impl private::MobEventStoreSealed for SpiMobEventStore {
    /// The in-memory store's own process gate (no store state).
    fn definition_resume_gate(&self) -> Arc<tokio::sync::RwLock<()>> {
        self.bundle.events.definition_resume_gate()
    }

    /// M: events appended.
    async fn append_definition_epoch(
        &self,
        event: NewMobEvent,
        authority: &private::MobDefinitionEpochPersistenceAuthority,
    ) -> Result<private::MobDefinitionEpochAppendOutcome, MobStoreError> {
        self.bundle
            .write(
                appended(),
                self.bundle.events.append_definition_epoch(event, authority),
            )
            .await
    }

    /// M: read.
    async fn creation_record(&self) -> Result<Option<MobCreationRecord>, MobStoreError> {
        self.bundle
            .read(mob_read(), self.bundle.events.creation_record())
            .await
    }

    /// M: events appended, creation record.
    async fn append_creation(
        &self,
        event: NewMobEvent,
        incarnation: meerkat_core::service::MobIncarnationId,
        provenance: &crate::MobProvenance,
    ) -> Result<MobEvent, MobStoreError> {
        let touch = Touch {
            creation: true,
            ..appended()
        };
        self.bundle
            .write(
                touch,
                self.bundle
                    .events
                    .append_creation(event, incarnation, provenance),
            )
            .await
    }

    /// M: creation record.
    async fn assign_legacy_incarnation(
        &self,
        mob_id: &MobId,
        incarnation: meerkat_core::service::MobIncarnationId,
    ) -> Result<MobCreationRecord, MobStoreError> {
        let touch = Touch {
            creation: true,
            ..Touch::default()
        };
        self.bundle
            .write(
                touch,
                self.bundle
                    .events
                    .assign_legacy_incarnation(mob_id, incarnation),
            )
            .await
    }
}

#[async_trait]
impl MobEventStore for SpiMobEventStore {
    /// M: the one external delivery the intent names.
    async fn begin_external_delivery(
        &self,
        intent: &MobExternalDeliveryIntent,
    ) -> Result<MobExternalDeliveryBeginOutcome, MobStoreError> {
        self.bundle
            .write(
                delivery(intent),
                self.bundle.events.begin_external_delivery(intent),
            )
            .await
    }

    /// M: the one external delivery the intent names.
    async fn complete_external_delivery(
        &self,
        intent: &MobExternalDeliveryIntent,
        terminal: &MobExternalDeliveryTerminal,
    ) -> Result<MobExternalDeliveryCompleteOutcome, MobStoreError> {
        self.bundle
            .write(
                delivery(intent),
                self.bundle
                    .events
                    .complete_external_delivery(intent, terminal),
            )
            .await
    }

    /// M: the one external delivery the commit's intent names.
    async fn claim_external_delivery_realization(
        &self,
        commit: &MobExternalDeliveryRealizationCommit,
    ) -> Result<MobExternalDeliveryClaimOutcome, MobStoreError> {
        self.bundle
            .write(
                delivery(commit.intent()),
                self.bundle
                    .events
                    .claim_external_delivery_realization(commit),
            )
            .await
    }

    /// M: the one external delivery the commit's intent names.
    async fn complete_external_delivery_realization(
        &self,
        commit: &MobExternalDeliveryRealizerCompletionCommit,
    ) -> Result<MobExternalDeliveryCompleteOutcome, MobStoreError> {
        self.bundle
            .write(
                delivery(commit.intent()),
                self.bundle
                    .events
                    .complete_external_delivery_realization(commit),
            )
            .await
    }

    /// M: the one external delivery the intent names.
    async fn abandon_external_delivery(
        &self,
        intent: &MobExternalDeliveryIntent,
        terminal: &MobExternalDeliveryTerminal,
    ) -> Result<MobExternalDeliveryAbandonOutcome, MobStoreError> {
        self.bundle
            .write(
                delivery(intent),
                self.bundle
                    .events
                    .abandon_external_delivery(intent, terminal),
            )
            .await
    }

    /// M: the one external delivery the commit's intent names.
    async fn resolve_external_delivery_after_realizer_fenced(
        &self,
        commit: &MobExternalDeliveryFencedResolutionCommit,
    ) -> Result<MobExternalDeliveryAbandonOutcome, MobStoreError> {
        self.bundle
            .write(
                delivery(commit.intent()),
                self.bundle
                    .events
                    .resolve_external_delivery_after_realizer_fenced(commit),
            )
            .await
    }

    /// M: the one external delivery the intent names.
    async fn schedule_external_delivery_repair(
        &self,
        intent: &MobExternalDeliveryIntent,
    ) -> Result<MobExternalDeliveryRepairOutcome, MobStoreError> {
        self.bundle
            .write(
                delivery(intent),
                self.bundle.events.schedule_external_delivery_repair(intent),
            )
            .await
    }

    /// M: read.
    async fn load_external_delivery(
        &self,
        mob_id: &MobId,
        idempotency_key: &str,
    ) -> Result<Option<MobExternalDeliveryRecord>, MobStoreError> {
        self.bundle
            .read(
                mob_read(),
                self.bundle
                    .events
                    .load_external_delivery(mob_id, idempotency_key),
            )
            .await
    }

    /// M: events appended.
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobStoreError> {
        self.bundle
            .write(appended(), self.bundle.events.append(event))
            .await
    }

    /// M: events appended.
    async fn append_terminal_event_if_absent(
        &self,
        event: NewMobEvent,
    ) -> Result<Option<MobEvent>, MobStoreError> {
        self.bundle
            .write(
                appended(),
                self.bundle.events.append_terminal_event_if_absent(event),
            )
            .await
    }

    /// M: events appended (the in-memory check-and-append, under one lock).
    async fn append_step_failed_event_if_absent(
        &self,
        event: NewMobEvent,
    ) -> Result<Option<MobEvent>, MobStoreError> {
        self.bundle
            .write(
                appended(),
                self.bundle.events.append_step_failed_event_if_absent(event),
            )
            .await
    }

    /// M: events appended (the in-memory check-and-append, under one lock).
    async fn append_fork_job_terminal_if_absent(
        &self,
        event: NewMobEvent,
    ) -> Result<Option<MobEvent>, MobStoreError> {
        self.bundle
            .write(
                appended(),
                self.bundle.events.append_fork_job_terminal_if_absent(event),
            )
            .await
    }

    /// M: events appended, all of them in one batch.
    async fn append_batch(&self, events: Vec<NewMobEvent>) -> Result<Vec<MobEvent>, MobStoreError> {
        self.bundle
            .write(appended(), self.bundle.events.append_batch(events))
            .await
    }

    /// M: read.
    async fn poll(&self, after_cursor: u64, limit: usize) -> Result<Vec<MobEvent>, MobStoreError> {
        self.bundle
            .read(mob_read(), self.bundle.events.poll(after_cursor, limit))
            .await
    }

    /// M: read.
    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobStoreError> {
        self.bundle
            .read(mob_read(), self.bundle.events.replay_all())
            .await
    }

    /// M: read.
    async fn latest_cursor(&self) -> Result<u64, MobStoreError> {
        self.bundle
            .read(mob_read(), self.bundle.events.latest_cursor())
            .await
    }

    /// Events reach subscribers only after their batch committed.
    fn subscribe(&self) -> Result<MobEventReceiver, MobStoreError> {
        Ok(self.bundle.committed_events.subscribe())
    }

    /// M: the whole log rewritten (cleared), the creation record finalized.
    async fn clear(&self) -> Result<(), MobStoreError> {
        let touch = Touch {
            events: Some(EventsTouch::Rewritten),
            creation: true,
            ..Touch::default()
        };
        self.bundle.write(touch, self.bundle.events.clear()).await
    }

    /// M: the whole log rewritten (pruned).
    async fn prune(&self, older_than: DateTime<Utc>) -> Result<u64, MobStoreError> {
        let touch = Touch {
            events: Some(EventsTouch::Rewritten),
            ..Touch::default()
        };
        self.bundle
            .write(touch, self.bundle.events.prune(older_than))
            .await
    }
}

// ---------------------------------------------------------------------------
// Runtime metadata (scope M; every write re-exports the eleven metadata maps)
// ---------------------------------------------------------------------------

/// [`MobRuntimeMetadataStore`] over the storage's mob scope. Every write
/// exports the whole runtime metadata (eleven maps, bounded by the mob's
/// members, hosts and pending operator requests); the journal writes only
/// the records whose bytes changed.
#[derive(Debug, Clone)]
pub struct SpiMobRuntimeMetadataStore {
    bundle: Arc<Bundle>,
}

impl SpiMobRuntimeMetadataStore {
    pub(super) fn new(bundle: Arc<Bundle>) -> Self {
        Self { bundle }
    }
}

macro_rules! metadata_read {
    ($self:ident, $call:expr) => {
        $self.bundle.read(mob_read(), $call).await
    };
}

macro_rules! metadata_write {
    ($self:ident, $call:expr) => {
        $self.bundle.write(runtime_metadata(), $call).await
    };
}

#[async_trait]
impl MobRuntimeMetadataStore for SpiMobRuntimeMetadataStore {
    async fn load_supervisor_authority(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<SupervisorAuthorityRecord>, MobStoreError> {
        metadata_read!(
            self,
            self.bundle
                .runtime_metadata
                .load_supervisor_authority(mob_id)
        )
    }

    async fn put_supervisor_authority(
        &self,
        mob_id: &MobId,
        record: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityPersistenceAuthority,
    ) -> Result<(), MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .put_supervisor_authority(mob_id, record, authority)
        )
    }

    async fn compare_and_put_supervisor_authority(
        &self,
        mob_id: &MobId,
        expected: &SupervisorAuthorityRecord,
        record: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .compare_and_put_supervisor_authority(mob_id, expected, record, authority)
        )
    }

    async fn put_supervisor_authority_if_absent(
        &self,
        mob_id: &MobId,
        record: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .put_supervisor_authority_if_absent(mob_id, record, authority)
        )
    }

    async fn delete_supervisor_authority(
        &self,
        mob_id: &MobId,
        expected: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityDeletionAuthority,
    ) -> Result<bool, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .delete_supervisor_authority(mob_id, expected, authority)
        )
    }

    async fn load_mob_host_authority(
        &self,
        mob_id: &MobId,
        host_id: &str,
    ) -> Result<Option<MobHostAuthorityRecord>, MobStoreError> {
        metadata_read!(
            self,
            self.bundle
                .runtime_metadata
                .load_mob_host_authority(mob_id, host_id)
        )
    }

    async fn list_mob_host_authorities(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobHostAuthorityRecord>, MobStoreError> {
        metadata_read!(
            self,
            self.bundle
                .runtime_metadata
                .list_mob_host_authorities(mob_id)
        )
    }

    async fn put_mob_host_authority(
        &self,
        mob_id: &MobId,
        record: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityPersistenceAuthority,
    ) -> Result<(), MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .put_mob_host_authority(mob_id, record, authority)
        )
    }

    async fn put_mob_host_authority_if_absent(
        &self,
        mob_id: &MobId,
        record: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .put_mob_host_authority_if_absent(mob_id, record, authority)
        )
    }

    async fn compare_and_put_mob_host_authority(
        &self,
        mob_id: &MobId,
        expected: &MobHostAuthorityRecord,
        record: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .compare_and_put_mob_host_authority(mob_id, expected, record, authority)
        )
    }

    async fn delete_mob_host_authority(
        &self,
        mob_id: &MobId,
        expected: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityDeletionAuthority,
    ) -> Result<bool, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .delete_mob_host_authority(mob_id, expected, authority)
        )
    }

    async fn put_mob_host_binding_generation_highwater(
        &self,
        mob_id: &MobId,
        expected: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityDeletionAuthority,
    ) -> Result<(), MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .put_mob_host_binding_generation_highwater(mob_id, expected, authority)
        )
    }

    async fn list_mob_host_binding_generation_highwaters(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<(String, u64)>, MobStoreError> {
        metadata_read!(
            self,
            self.bundle
                .runtime_metadata
                .list_mob_host_binding_generation_highwaters(mob_id)
        )
    }

    async fn begin_member_operator_request(
        &self,
        mob_id: &MobId,
        record: &MobMemberOperatorRequestRecord,
    ) -> Result<MobMemberOperatorRequestBegin, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .begin_member_operator_request(mob_id, record)
        )
    }

    async fn load_member_operator_request(
        &self,
        mob_id: &MobId,
        key: &MobMemberOperatorRequestKey,
    ) -> Result<Option<MobMemberOperatorRequestRecord>, MobStoreError> {
        metadata_read!(
            self,
            self.bundle
                .runtime_metadata
                .load_member_operator_request(mob_id, key)
        )
    }

    async fn compare_and_put_member_operator_request(
        &self,
        mob_id: &MobId,
        expected: &MobMemberOperatorRequestRecord,
        record: &MobMemberOperatorRequestRecord,
    ) -> Result<bool, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .compare_and_put_member_operator_request(mob_id, expected, record)
        )
    }

    async fn list_member_operator_requests(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobMemberOperatorRequestRecord>, MobStoreError> {
        metadata_read!(
            self,
            self.bundle
                .runtime_metadata
                .list_member_operator_requests(mob_id)
        )
    }

    async fn prune_stale_member_operator_requests(
        &self,
        mob_id: &MobId,
        authority: &MobMemberOperatorPruneAuthority,
    ) -> Result<u64, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .prune_stale_member_operator_requests(mob_id, authority)
        )
    }

    async fn delete_member_operator_requests(&self, mob_id: &MobId) -> Result<u64, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .delete_member_operator_requests(mob_id)
        )
    }

    async fn load_placed_spawn(
        &self,
        mob_id: &MobId,
        agent_identity: &str,
    ) -> Result<Option<MobPlacedSpawnCarrierRecord>, MobStoreError> {
        metadata_read!(
            self,
            self.bundle
                .runtime_metadata
                .load_placed_spawn(mob_id, agent_identity)
        )
    }

    async fn list_placed_spawns(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobPlacedSpawnCarrierRecord>, MobStoreError> {
        metadata_read!(
            self,
            self.bundle.runtime_metadata.list_placed_spawns(mob_id)
        )
    }

    async fn begin_placed_spawn_if_absent(
        &self,
        mob_id: &MobId,
        record: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnPendingPersistenceAuthority,
    ) -> Result<BeginPlacedSpawnResult, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .begin_placed_spawn_if_absent(mob_id, record, authority)
        )
    }

    async fn compare_and_commit_placed_spawn(
        &self,
        mob_id: &MobId,
        expected_pending: &MobPlacedSpawnCarrierRecord,
        committed: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnCommitPersistenceAuthority,
    ) -> Result<CommitPlacedSpawnResult, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .compare_and_commit_placed_spawn(mob_id, expected_pending, committed, authority)
        )
    }

    async fn compare_and_promote_placed_spawn_binding(
        &self,
        mob_id: &MobId,
        expected: &MobPlacedSpawnCarrierRecord,
        promoted: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnBindingPromotionAuthority,
    ) -> Result<PromotePlacedSpawnBindingResult, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .compare_and_promote_placed_spawn_binding(mob_id, expected, promoted, authority)
        )
    }

    async fn compare_and_delete_placed_spawn(
        &self,
        mob_id: &MobId,
        expected: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnCleanupAuthority,
    ) -> Result<DeletePlacedSpawnResult, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .compare_and_delete_placed_spawn(mob_id, expected, authority)
        )
    }

    async fn list_mob_operator_grants(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobOperatorGrantRecord>, MobStoreError> {
        metadata_read!(
            self,
            self.bundle
                .runtime_metadata
                .list_mob_operator_grants(mob_id)
        )
    }

    async fn put_mob_operator_grant(
        &self,
        mob_id: &MobId,
        record: &MobOperatorGrantRecord,
        authority: &MobOperatorGrantPersistenceAuthority,
    ) -> Result<(), MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .put_mob_operator_grant(mob_id, record, authority)
        )
    }

    async fn delete_mob_operator_grant(
        &self,
        mob_id: &MobId,
        principal: &str,
        authority: &MobOperatorGrantDeletionAuthority,
    ) -> Result<bool, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .delete_mob_operator_grant(mob_id, principal, authority)
        )
    }

    async fn delete_mob_operator_grants(&self, mob_id: &MobId) -> Result<u64, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .delete_mob_operator_grants(mob_id)
        )
    }

    async fn list_member_event_cursors(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobMemberEventCursorRecord>, MobStoreError> {
        metadata_read!(
            self,
            self.bundle
                .runtime_metadata
                .list_member_event_cursors(mob_id)
        )
    }

    async fn put_member_event_cursor(
        &self,
        mob_id: &MobId,
        record: &MobMemberEventCursorRecord,
    ) -> Result<(), MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .put_member_event_cursor(mob_id, record)
        )
    }

    async fn delete_member_event_cursor(
        &self,
        mob_id: &MobId,
        agent_identity: &str,
    ) -> Result<bool, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .delete_member_event_cursor(mob_id, agent_identity)
        )
    }

    async fn delete_member_event_cursors(&self, mob_id: &MobId) -> Result<u64, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .delete_member_event_cursors(mob_id)
        )
    }

    async fn list_member_live_cleanup_records(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobMemberLiveCleanupRecord>, MobStoreError> {
        metadata_read!(
            self,
            self.bundle
                .runtime_metadata
                .list_member_live_cleanup_records(mob_id)
        )
    }

    async fn put_member_live_cleanup_record_if_absent(
        &self,
        mob_id: &MobId,
        record: &MobMemberLiveCleanupRecord,
    ) -> Result<bool, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .put_member_live_cleanup_record_if_absent(mob_id, record)
        )
    }

    async fn delete_member_live_cleanup_record(
        &self,
        mob_id: &MobId,
        expected: &MobMemberLiveCleanupRecord,
    ) -> Result<bool, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .delete_member_live_cleanup_record(mob_id, expected)
        )
    }

    async fn list_forked_participant_member_associations(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobForkedParticipantMemberAssociation>, MobStoreError> {
        metadata_read!(
            self,
            self.bundle
                .runtime_metadata
                .list_forked_participant_member_associations(mob_id)
        )
    }

    async fn put_forked_participant_member_association(
        &self,
        mob_id: &MobId,
        record: &MobForkedParticipantMemberAssociation,
    ) -> Result<(), MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .put_forked_participant_member_association(mob_id, record)
        )
    }

    async fn delete_forked_participant_member_association(
        &self,
        mob_id: &MobId,
        expected: &MobForkedParticipantMemberAssociation,
    ) -> Result<bool, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .delete_forked_participant_member_association(mob_id, expected)
        )
    }

    async fn list_external_binding_overlays(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<ExternalBindingOverlayRecord>, MobStoreError> {
        metadata_read!(
            self,
            self.bundle
                .runtime_metadata
                .list_external_binding_overlays(mob_id)
        )
    }

    async fn put_external_binding_overlay_if_absent(
        &self,
        mob_id: &MobId,
        record: &ExternalBindingOverlayRecord,
    ) -> Result<bool, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .put_external_binding_overlay_if_absent(mob_id, record)
        )
    }

    async fn upsert_external_binding_overlay(
        &self,
        mob_id: &MobId,
        record: &ExternalBindingOverlayRecord,
    ) -> Result<(), MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .upsert_external_binding_overlay(mob_id, record)
        )
    }

    async fn compare_and_set_external_direct_bind(
        &self,
        mob_id: &MobId,
        expected: &ExternalBindingOverlayRecord,
        desired: &ExternalBindingOverlayRecord,
    ) -> Result<bool, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .compare_and_set_external_direct_bind(mob_id, expected, desired)
        )
    }

    async fn compare_and_delete_external_direct_bind(
        &self,
        mob_id: &MobId,
        expected: &ExternalBindingOverlayRecord,
    ) -> Result<bool, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .compare_and_delete_external_direct_bind(mob_id, expected)
        )
    }

    async fn delete_external_binding_overlay(
        &self,
        mob_id: &MobId,
        agent_identity: &AgentIdentity,
        generation: Generation,
    ) -> Result<(), MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .delete_external_binding_overlay(mob_id, agent_identity, generation)
        )
    }

    async fn delete_external_binding_overlays(&self, mob_id: &MobId) -> Result<(), MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .delete_external_binding_overlays(mob_id)
        )
    }

    async fn put_callback_claim_verifier_if_absent(
        &self,
        mob_id: &MobId,
        verifier: &crate::MobClaimVerifier,
    ) -> Result<bool, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .put_callback_claim_verifier_if_absent(mob_id, verifier)
        )
    }

    async fn load_callback_claim_verifier(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<crate::MobClaimVerifier>, MobStoreError> {
        metadata_read!(
            self,
            self.bundle
                .runtime_metadata
                .load_callback_claim_verifier(mob_id)
        )
    }

    async fn delete_callback_claim_verifier(&self, mob_id: &MobId) -> Result<bool, MobStoreError> {
        metadata_write!(
            self,
            self.bundle
                .runtime_metadata
                .delete_callback_claim_verifier(mob_id)
        )
    }
}

// ---------------------------------------------------------------------------
// Identity authority (scope I) and the identity member transaction (M + I)
// ---------------------------------------------------------------------------

/// [`MobIdentityStore`] over the storage's identity scope, and the built-in
/// identity member transaction (`MobIdentityMemberStore`) over the mob and
/// identity scopes in one batch.
#[derive(Debug, Clone)]
pub struct SpiMobIdentityStore {
    bundle: Arc<Bundle>,
}

impl SpiMobIdentityStore {
    pub(super) fn new(bundle: Arc<Bundle>) -> Self {
        Self { bundle }
    }
}

/// A runtime write fence that holds the identity scope's lock across the
/// fenced runtime write, so the in-memory fence validates only identity
/// authority the host acknowledged (no commit of that scope is in flight).
/// While a commit of that scope is in flight it answers Backoff, and the
/// runtime retries it for the host's declared longest commit
/// ([`meerkat_core::host_store::FenceTiming::max_commit_latency`]). That
/// bound covers one host commit: an identity write holds the scope through
/// its commit retries (the realm's retry budget) and a re-acquire, so while
/// the host is in trouble fenced runtime writes report Backoff past the
/// bound. They never wait longer than it.
#[cfg(feature = "runtime-adapter")]
pub(super) struct SpiIdentityRuntimeWriteFence {
    cell: Arc<meerkat_runtime::store::spi::ScopeCell>,
    inner: Arc<dyn meerkat_runtime::RuntimeStoreWriteFence>,
    backoff_bound: std::time::Duration,
}

#[cfg(feature = "runtime-adapter")]
impl SpiIdentityRuntimeWriteFence {
    /// The fence over `bundle`'s identity scope around `inner`, retried for
    /// the host's declared longest commit.
    pub(super) fn over(
        bundle: &Bundle,
        inner: Arc<dyn meerkat_runtime::RuntimeStoreWriteFence>,
    ) -> Self {
        Self {
            cell: bundle.realm.cell(&bundle.identity_scope),
            inner,
            backoff_bound: bundle.realm.timing().max_commit_latency(),
        }
    }
}

#[cfg(feature = "runtime-adapter")]
impl meerkat_runtime::RuntimeStoreWriteFence for SpiIdentityRuntimeWriteFence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), meerkat_runtime::RuntimeStoreError> + '_>,
    ) -> Result<meerkat_runtime::RuntimeStoreWriteFenceOutcome, meerkat_runtime::RuntimeStoreError>
    {
        let Ok(state) = self.cell.try_lock() else {
            return Ok(meerkat_runtime::RuntimeStoreWriteFenceOutcome::Backoff {
                reason: "the identity authority has a host store commit in flight".to_string(),
            });
        };
        if let Some(cause) = state.poison() {
            return Ok(meerkat_runtime::RuntimeStoreWriteFenceOutcome::Conflict {
                reason: format!("the identity authority's host store scope is poisoned: {cause}"),
            });
        }
        let outcome = self.inner.execute_if_current(operation);
        drop(state);
        outcome
    }

    fn backoff_retry_bound(&self) -> std::time::Duration {
        self.backoff_bound
    }
}

#[async_trait]
impl MobIdentityStore for SpiMobIdentityStore {
    #[cfg(feature = "runtime-adapter")]
    fn prepare_runtime_write_fence(
        &self,
        permit: IdentityActuationPermit,
        expected_session: DesiredSessionTarget,
        observed: &meerkat_runtime::RuntimeSessionLifecycleObservation,
    ) -> Result<Arc<dyn meerkat_runtime::RuntimeStoreWriteFence>, MobStoreError> {
        let inner =
            self.bundle
                .identity
                .prepare_runtime_write_fence(permit, expected_session, observed)?;
        Ok(Arc::new(SpiIdentityRuntimeWriteFence::over(
            &self.bundle,
            inner,
        )))
    }

    /// I: read.
    async fn observe_identity_intent(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityStoredObservation<IdentityIntentRecord>, MobStoreError> {
        self.bundle
            .read(
                identity_read(),
                self.bundle
                    .identity
                    .observe_identity_intent(mob_id, identity),
            )
            .await
    }

    /// I: every intent (an adoption recomputes other identities' wiring),
    /// and the one adoption receipt the request names.
    async fn adopt_member_identity_declaration(
        &self,
        request: &AdoptMemberIdentityDeclaration,
        compiled: &IdentityIntentRecord,
    ) -> Result<IdentityAdoptionOutcome, MobStoreError> {
        let touch = Touch {
            intents: true,
            adoption_receipt: Some((request.mob_id.clone(), request.request_id.clone())),
            ..Touch::default()
        };
        self.bundle
            .write(
                touch,
                self.bundle
                    .identity
                    .adopt_member_identity_declaration(request, compiled),
            )
            .await
    }

    /// I: every intent and the one mutation receipt the request names.
    async fn apply_member_tool_declaration(
        &self,
        request: &ApplyMemberToolDeclaration,
    ) -> Result<MemberToolCommitOutcome, MobStoreError> {
        let touch = Touch {
            intents: true,
            mutation_receipt: Some((request.mob_id.clone(), request.request_id.clone())),
            ..Touch::default()
        };
        self.bundle
            .write(
                touch,
                self.bundle.identity.apply_member_tool_declaration(request),
            )
            .await
    }

    /// I: every intent and the one resolution receipt the request names.
    async fn resolve_identity_convergence_block(
        &self,
        request: &ResolveIdentityConvergenceBlock,
        actual_active_revision: u64,
    ) -> Result<IdentityConvergenceResolutionOutcome, MobStoreError> {
        let touch = Touch {
            intents: true,
            resolution_receipt: Some((request.mob_id.clone(), request.request_id.clone())),
            ..Touch::default()
        };
        self.bundle
            .write(
                touch,
                self.bundle
                    .identity
                    .resolve_identity_convergence_block(request, actual_active_revision),
            )
            .await
    }

    /// I: read.
    async fn list_identity_intents(
        &self,
        mob_id: &MobId,
    ) -> Result<
        BTreeMap<AgentIdentity, IdentityStoredObservation<IdentityIntentRecord>>,
        MobStoreError,
    > {
        self.bundle
            .read(
                identity_read(),
                self.bundle.identity.list_identity_intents(mob_id),
            )
            .await
    }

    /// I: read.
    async fn scan_identity_intents_page(
        &self,
        mob_id: &MobId,
        after: Option<&IdentityIntentScanCursor>,
        limit: usize,
    ) -> Result<IdentityIntentScanPage, MobStoreError> {
        self.bundle
            .read(
                identity_read(),
                self.bundle
                    .identity
                    .scan_identity_intents_page(mob_id, after, limit),
            )
            .await
    }

    /// I: read.
    async fn observe_identity_lease(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityStoredObservation<IdentityLeaseRecord>, MobStoreError> {
        self.bundle
            .read(
                identity_read(),
                self.bundle
                    .identity
                    .observe_identity_lease(mob_id, identity),
            )
            .await
    }

    /// I: every lease (bounded by the mob's identities).
    async fn claim_or_renew_identity_lease(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
        holder_id: &str,
        incarnation_id: &str,
        ttl_ms: u64,
    ) -> Result<IdentityLeaseClaimOutcome, MobStoreError> {
        let touch = Touch {
            leases: true,
            ..Touch::default()
        };
        self.bundle
            .write(
                touch,
                self.bundle.identity.claim_or_renew_identity_lease(
                    mob_id,
                    identity,
                    holder_id,
                    incarnation_id,
                    ttl_ms,
                ),
            )
            .await
    }

    /// I: every lease.
    async fn release_identity_lease(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
        expected: &IdentityLeaseClaim,
    ) -> Result<bool, MobStoreError> {
        let touch = Touch {
            leases: true,
            ..Touch::default()
        };
        self.bundle
            .write(
                touch,
                self.bundle
                    .identity
                    .release_identity_lease(mob_id, identity, expected),
            )
            .await
    }

    /// I: read.
    async fn validate_identity_actuation_permit(
        &self,
        permit: &IdentityActuationPermit,
    ) -> Result<(), MobStoreError> {
        self.bundle
            .read(
                identity_read(),
                self.bundle
                    .identity
                    .validate_identity_actuation_permit(permit),
            )
            .await
    }

    /// I: read.
    async fn observe_identity_operation_receipt(
        &self,
        mob_id: &MobId,
        subject: &IdentityOperationSubject,
        slot: &IdentityOperationSlot,
    ) -> Result<IdentityStoredObservation<IdentityOperationReceipt>, MobStoreError> {
        self.bundle
            .read(
                identity_read(),
                self.bundle
                    .identity
                    .observe_identity_operation_receipt(mob_id, subject, slot),
            )
            .await
    }

    /// I: the one receipt the receipt's slot names (keyed as the in-memory
    /// store keys it).
    async fn insert_identity_operation_receipt_if_absent(
        &self,
        receipt: &IdentityOperationReceipt,
        permit: &IdentityActuationPermit,
    ) -> Result<IdentityOperationReceiptInsertOutcome, MobStoreError> {
        let touch = Touch {
            receipt: Some(identity_receipt_key(
                &receipt.mob_id,
                &receipt.subject,
                &receipt.slot,
            )?),
            ..Touch::default()
        };
        self.bundle
            .write(
                touch,
                self.bundle
                    .identity
                    .insert_identity_operation_receipt_if_absent(receipt, permit),
            )
            .await
    }
}

#[async_trait]
impl MobIdentityMemberStore for SpiMobIdentityStore {
    /// M + I: read (the member target folds events and intents).
    async fn observe_identity_member_target(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityMemberTargetObservation, MobStoreError> {
        self.bundle
            .read(
                mob_and_identity_read(),
                self.bundle
                    .identity
                    .observe_identity_member_target(mob_id, identity),
            )
            .await
    }

    /// M + I in one batch: the member event appended under identity
    /// authority validated in the same locked state. The in-memory
    /// transaction writes no identity entry; the identity scope is locked so
    /// the validation reads only acknowledged authority.
    async fn commit_identity_member_spawned(
        &self,
        permit: &IdentityActuationPermit,
        event: &NewMobEvent,
    ) -> Result<IdentityMemberEventCommitOutcome, MobStoreError> {
        let touch = Touch {
            intents: true,
            leases: true,
            ..appended()
        };
        self.bundle
            .write(
                touch,
                self.bundle
                    .identity
                    .commit_identity_member_spawned(permit, event),
            )
            .await
    }

    /// M + I: read.
    async fn observe_identity_wiring_target(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityWiringTargetObservation, MobStoreError> {
        self.bundle
            .read(
                mob_and_identity_read(),
                self.bundle
                    .identity
                    .observe_identity_wiring_target(mob_id, identity),
            )
            .await
    }

    /// M + I in one batch, as [`Self::commit_identity_member_spawned`].
    async fn commit_identity_wiring_event(
        &self,
        permit: &IdentityActuationPermit,
        event: &NewMobEvent,
    ) -> Result<IdentityWiringEventCommitOutcome, MobStoreError> {
        let touch = Touch {
            intents: true,
            leases: true,
            ..appended()
        };
        self.bundle
            .write(
                touch,
                self.bundle
                    .identity
                    .commit_identity_wiring_event(permit, event),
            )
            .await
    }

    /// M + I: read (the journal and the intent rows, from one locked state).
    async fn read_member_binding_journal(
        &self,
        mob_id: &MobId,
    ) -> Result<MemberBindingJournalRead, MobStoreError> {
        self.bundle
            .read(
                mob_and_identity_read(),
                self.bundle.identity.read_member_binding_journal(mob_id),
            )
            .await
    }
}

// ---------------------------------------------------------------------------
// Identity convergence statuses (scope S)
// ---------------------------------------------------------------------------

/// [`MobIdentityStatusStore`] over the storage's status scope.
#[derive(Debug, Clone)]
pub struct SpiMobIdentityStatusStore {
    bundle: Arc<Bundle>,
}

impl SpiMobIdentityStatusStore {
    pub(super) fn new(bundle: Arc<Bundle>) -> Self {
        Self { bundle }
    }
}

#[async_trait]
impl MobIdentityStatusStore for SpiMobIdentityStatusStore {
    /// S: read.
    async fn load_identity_convergence_status(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityStoredObservation<IdentityConvergenceStatus>, MobStoreError> {
        self.bundle
            .read(
                status_read(),
                self.bundle
                    .identity_status
                    .load_identity_convergence_status(mob_id, identity),
            )
            .await
    }

    /// S: read.
    async fn list_identity_convergence_statuses(
        &self,
        mob_id: &MobId,
    ) -> Result<
        BTreeMap<AgentIdentity, IdentityStoredObservation<IdentityConvergenceStatus>>,
        MobStoreError,
    > {
        self.bundle
            .read(
                status_read(),
                self.bundle
                    .identity_status
                    .list_identity_convergence_statuses(mob_id),
            )
            .await
    }

    /// S: every status (bounded by the mob's identities).
    async fn replace_identity_convergence_status(
        &self,
        mob_id: &MobId,
        status: &IdentityConvergenceStatus,
    ) -> Result<(), MobStoreError> {
        let touch = Touch {
            statuses: true,
            ..Touch::default()
        };
        self.bundle
            .write(
                touch,
                self.bundle
                    .identity_status
                    .replace_identity_convergence_status(mob_id, status),
            )
            .await
    }
}

// ---------------------------------------------------------------------------
// Flow runs (scope R)
// ---------------------------------------------------------------------------

/// [`MobRunStore`] over the storage's run scope. Every write names one run
/// and exports that run (or that run's remote turn intents or receipts)
/// only. `cas_run_snapshot_and_append_terminal_event_with_authority` commits
/// the terminal run and its terminal event in one batch over the run and
/// mob scopes, as the SQLite store does in one transaction.
#[derive(Debug, Clone)]
pub struct SpiMobRunStore {
    bundle: Arc<Bundle>,
}

impl SpiMobRunStore {
    pub(super) fn new(bundle: Arc<Bundle>) -> Self {
        Self { bundle }
    }
}

#[async_trait]
impl MobRunStore for SpiMobRunStore {
    /// R: the created run.
    async fn create_run(&self, run_record: MobRun) -> Result<(), MobStoreError> {
        let run_id = run_record.run_id.clone();
        self.bundle
            .write(run(&run_id), self.bundle.runs.create_run(run_record))
            .await
    }

    /// R: read.
    async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, MobStoreError> {
        self.bundle
            .read(run_read(), self.bundle.runs.get_run(run_id))
            .await
    }

    /// R: read.
    async fn list_runs(
        &self,
        mob_id: &MobId,
        flow_id: Option<&FlowId>,
    ) -> Result<Vec<MobRun>, MobStoreError> {
        self.bundle
            .read(run_read(), self.bundle.runs.list_runs(mob_id, flow_id))
            .await
    }

    /// R: the run's remote turn intents.
    async fn put_remote_turn_intent(
        &self,
        run_id: &RunId,
        intent: &MobRunRemoteTurnIntent,
    ) -> Result<bool, MobStoreError> {
        let touch = Touch {
            turn_intents: Some(run_id.clone()),
            ..Touch::default()
        };
        self.bundle
            .write(
                touch,
                self.bundle.runs.put_remote_turn_intent(run_id, intent),
            )
            .await
    }

    /// R: the run's remote turn intents.
    async fn delete_remote_turn_intent(
        &self,
        run_id: &RunId,
        dispatch_sequence: u64,
    ) -> Result<bool, MobStoreError> {
        let touch = Touch {
            turn_intents: Some(run_id.clone()),
            ..Touch::default()
        };
        self.bundle
            .write(
                touch,
                self.bundle
                    .runs
                    .delete_remote_turn_intent(run_id, dispatch_sequence),
            )
            .await
    }

    /// R: read.
    async fn list_remote_turn_intents(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<MobRunRemoteTurnIntent>, MobStoreError> {
        self.bundle
            .read(
                run_read(),
                self.bundle.runs.list_remote_turn_intents(run_id),
            )
            .await
    }

    /// R: the run's remote turn receipts.
    async fn put_remote_turn_receipt(
        &self,
        run_id: &RunId,
        receipt: &MobRunRemoteTurnReceipt,
    ) -> Result<bool, MobStoreError> {
        let touch = Touch {
            turn_receipts: Some(run_id.clone()),
            ..Touch::default()
        };
        self.bundle
            .write(
                touch,
                self.bundle.runs.put_remote_turn_receipt(run_id, receipt),
            )
            .await
    }

    /// R: read.
    async fn list_remote_turn_receipts(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<MobRunRemoteTurnReceipt>, MobStoreError> {
        self.bundle
            .read(
                run_read(),
                self.bundle.runs.list_remote_turn_receipts(run_id),
            )
            .await
    }

    /// R: the run's remote turn receipts.
    async fn delete_remote_turn_receipt(
        &self,
        run_id: &RunId,
        dispatch_sequence: u64,
    ) -> Result<bool, MobStoreError> {
        let touch = Touch {
            turn_receipts: Some(run_id.clone()),
            ..Touch::default()
        };
        self.bundle
            .write(
                touch,
                self.bundle
                    .runs
                    .delete_remote_turn_receipt(run_id, dispatch_sequence),
            )
            .await
    }

    /// R: the run.
    async fn cas_flow_state_with_authority(
        &self,
        run_id: &RunId,
        expected: &flow_run::State,
        next: &flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        self.bundle
            .write(
                run(run_id),
                self.bundle.runs.cas_flow_state_with_authority(
                    run_id,
                    expected,
                    next,
                    authority_inputs,
                ),
            )
            .await
    }

    /// R: the run.
    async fn cas_run_snapshot_with_authority(
        &self,
        run_id: &RunId,
        expected_status: MobRunStatus,
        expected_flow_state: &flow_run::State,
        next_status: MobRunStatus,
        next_flow_state: &flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        self.bundle
            .write(
                run(run_id),
                self.bundle.runs.cas_run_snapshot_with_authority(
                    run_id,
                    expected_status,
                    expected_flow_state,
                    next_status,
                    next_flow_state,
                    authority_inputs,
                ),
            )
            .await
    }

    /// R + M in one batch: the terminal run snapshot and its terminal event,
    /// so terminal run truth and terminal event truth never split. The
    /// event goes to this storage's own log, which every event store of the
    /// storage shares; `events` is not used, as in the SQLite store.
    async fn cas_run_snapshot_and_append_terminal_event_with_authority(
        &self,
        run_id: &RunId,
        expected_status: MobRunStatus,
        expected_flow_state: &flow_run::State,
        next_status: MobRunStatus,
        next_flow_state: &flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
        _events: &dyn MobEventStore,
        terminal_event: NewMobEvent,
    ) -> Result<Option<MobEvent>, MobStoreError> {
        // Validated before any change, as the trait's default does.
        validate_mob_event_write_authority(&terminal_event.kind)?;
        if terminal_event_identity(&terminal_event.kind).is_none() {
            return Err(MobStoreError::Internal(
                "cas_run_snapshot_and_append_terminal_event_with_authority requires a terminal flow event".to_string(),
            ));
        }
        let touch = Touch {
            run: Some(run_id.clone()),
            ..appended()
        };
        let bundle = &self.bundle;
        self.bundle
            .write_whole(touch, async move {
                let transitioned = bundle
                    .runs
                    .cas_run_snapshot_with_authority(
                        run_id,
                        expected_status,
                        expected_flow_state,
                        next_status,
                        next_flow_state,
                        authority_inputs,
                    )
                    .await
                    .map_err(WriteFailure::Failed)?;
                if !transitioned {
                    return Ok(None);
                }
                // The run is terminal in memory now: an event refused here
                // must not let the run commit alone.
                match bundle.events.append(terminal_event).await {
                    Ok(stored) => Ok(Some(stored)),
                    Err(error) => Err(WriteFailure::Torn(MobStoreError::Internal(format!(
                        "the terminal event of run '{run_id}' was refused after its terminal \
                         snapshot won: {error}; neither is committed, and this storage's run \
                         and mob scopes refuse every call until the realm is reopened"
                    )))),
                }
            })
            .await
    }

    /// R: the run.
    async fn append_step_entry_with_authority(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<(), MobStoreError> {
        self.bundle
            .write(
                run(run_id),
                self.bundle
                    .runs
                    .append_step_entry_with_authority(run_id, entry, authority),
            )
            .await
    }

    /// R: the run.
    async fn append_step_entry_if_absent_with_authority(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<bool, MobStoreError> {
        self.bundle
            .write(
                run(run_id),
                self.bundle
                    .runs
                    .append_step_entry_if_absent_with_authority(run_id, entry, authority),
            )
            .await
    }

    /// R: the run.
    async fn append_failure_entry_with_authority(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<(), MobStoreError> {
        self.bundle
            .write(
                run(run_id),
                self.bundle
                    .runs
                    .append_failure_entry_with_authority(run_id, entry, authority),
            )
            .await
    }

    /// R: the run.
    async fn append_failure_entry_if_absent_with_authority(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<bool, MobStoreError> {
        self.bundle
            .write(
                run(run_id),
                self.bundle
                    .runs
                    .append_failure_entry_if_absent_with_authority(run_id, entry, authority),
            )
            .await
    }

    /// R: the run.
    async fn cas_frame_state_with_authority(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        expected: Option<&FrameSnapshot>,
        next: FrameSnapshot,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        self.bundle
            .write(
                run(run_id),
                self.bundle.runs.cas_frame_state_with_authority(
                    run_id,
                    frame_id,
                    expected,
                    next,
                    authority_inputs,
                ),
            )
            .await
    }

    /// R: the run.
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
        self.bundle
            .write(
                run(run_id),
                self.bundle.runs.cas_grant_node_slot_with_authority(
                    run_id,
                    expected_run_state,
                    next_run_state,
                    frame_id,
                    expected_frame,
                    next_frame,
                    authority_inputs,
                ),
            )
            .await
    }

    /// R: the run.
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
        self.bundle
            .write(
                run(run_id),
                self.bundle
                    .runs
                    .cas_complete_step_and_record_output_with_authority(
                        run_id,
                        frame_id,
                        expected_frame,
                        next_frame,
                        step_output_key,
                        step_output,
                        loop_context,
                        authority_inputs,
                    ),
            )
            .await
    }

    /// R: the run.
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
        self.bundle
            .write(
                run(run_id),
                self.bundle.runs.cas_start_loop_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_run_state,
                    next_run_state,
                    frame_id,
                    expected_frame,
                    next_frame,
                    initial_loop,
                    authority_inputs,
                ),
            )
            .await
    }

    /// R: the run.
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
        self.bundle
            .write(
                run(run_id),
                self.bundle.runs.cas_loop_request_body_frame_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop,
                    expected_run_state,
                    next_run_state,
                    authority_inputs,
                ),
            )
            .await
    }

    /// R: the run.
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
        self.bundle
            .write(
                run(run_id),
                self.bundle.runs.cas_grant_body_frame_start_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop,
                    frame_id,
                    initial_frame,
                    ledger_entry,
                    expected_run_state,
                    next_run_state,
                    authority_inputs,
                ),
            )
            .await
    }

    /// R: the run.
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
        self.bundle
            .write(
                run(run_id),
                self.bundle.runs.cas_complete_body_frame_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop,
                    frame_id,
                    expected_frame,
                    next_frame,
                    expected_run_state,
                    next_run_state,
                    authority_inputs,
                ),
            )
            .await
    }

    /// R: the run.
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
        self.bundle
            .write(
                run(run_id),
                self.bundle.runs.cas_complete_loop_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop,
                    frame_id,
                    expected_frame,
                    next_frame,
                    expected_run_state,
                    next_run_state,
                    authority_inputs,
                ),
            )
            .await
    }
}

// ---------------------------------------------------------------------------
// Specs, realm profiles, forked participants (scope M)
// ---------------------------------------------------------------------------

/// [`MobSpecStore`] over the storage's mob scope.
#[derive(Debug, Clone)]
pub struct SpiMobSpecStore {
    bundle: Arc<Bundle>,
}

impl SpiMobSpecStore {
    pub(super) fn new(bundle: Arc<Bundle>) -> Self {
        Self { bundle }
    }
}

fn specs() -> Touch {
    Touch {
        specs: true,
        ..Touch::default()
    }
}

#[async_trait]
impl MobSpecStore for SpiMobSpecStore {
    /// M: every spec (one per mob).
    async fn put_spec(
        &self,
        mob_id: &MobId,
        definition: &MobDefinition,
        revision: Option<u64>,
    ) -> Result<u64, MobStoreError> {
        self.bundle
            .write(
                specs(),
                self.bundle.specs.put_spec(mob_id, definition, revision),
            )
            .await
    }

    /// M: read.
    async fn get_spec(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<(MobDefinition, u64)>, MobStoreError> {
        self.bundle
            .read(mob_read(), self.bundle.specs.get_spec(mob_id))
            .await
    }

    /// M: read.
    async fn list_specs(&self) -> Result<Vec<MobId>, MobStoreError> {
        self.bundle
            .read(mob_read(), self.bundle.specs.list_specs())
            .await
    }

    /// M: every spec.
    async fn delete_spec(
        &self,
        mob_id: &MobId,
        revision: Option<u64>,
    ) -> Result<bool, MobStoreError> {
        self.bundle
            .write(specs(), self.bundle.specs.delete_spec(mob_id, revision))
            .await
    }
}

/// [`RealmProfileStore`] over the storage's mob scope.
#[derive(Debug, Clone)]
pub struct SpiRealmProfileStore {
    bundle: Arc<Bundle>,
}

impl SpiRealmProfileStore {
    pub(super) fn new(bundle: Arc<Bundle>) -> Self {
        Self { bundle }
    }
}

fn profiles() -> Touch {
    Touch {
        profiles: true,
        ..Touch::default()
    }
}

#[async_trait]
impl RealmProfileStore for SpiRealmProfileStore {
    /// M: every profile.
    async fn create(
        &self,
        name: &str,
        profile: &crate::profile::Profile,
    ) -> Result<StoredRealmProfile, MobStoreError> {
        self.bundle
            .write(profiles(), self.bundle.realm_profiles.create(name, profile))
            .await
    }

    /// M: read.
    async fn get(&self, name: &str) -> Result<Option<StoredRealmProfile>, MobStoreError> {
        self.bundle
            .read(mob_read(), self.bundle.realm_profiles.get(name))
            .await
    }

    /// M: read.
    async fn list(&self) -> Result<Vec<StoredRealmProfile>, MobStoreError> {
        self.bundle
            .read(mob_read(), self.bundle.realm_profiles.list())
            .await
    }

    /// M: every profile.
    async fn update(
        &self,
        name: &str,
        profile: &crate::profile::Profile,
        expected_revision: u64,
    ) -> Result<StoredRealmProfile, MobStoreError> {
        self.bundle
            .write(
                profiles(),
                self.bundle
                    .realm_profiles
                    .update(name, profile, expected_revision),
            )
            .await
    }

    /// M: every profile.
    async fn delete(
        &self,
        name: &str,
        expected_revision: u64,
    ) -> Result<StoredRealmProfile, MobStoreError> {
        self.bundle
            .write(
                profiles(),
                self.bundle.realm_profiles.delete(name, expected_revision),
            )
            .await
    }
}

/// [`ForkedParticipantStore`] over the storage's mob scope.
#[derive(Debug, Clone)]
pub struct SpiForkedParticipantStore {
    bundle: Arc<Bundle>,
}

impl SpiForkedParticipantStore {
    pub(super) fn new(bundle: Arc<Bundle>) -> Self {
        Self { bundle }
    }
}

fn forked() -> Touch {
    Touch {
        forked: true,
        ..Touch::default()
    }
}

#[async_trait]
impl ForkedParticipantStore for SpiForkedParticipantStore {
    /// M: every forked participant record.
    async fn insert_reserved(
        &self,
        record: &ForkedParticipantRecord,
    ) -> Result<ForkedParticipantRecord, MobStoreError> {
        self.bundle
            .write(
                forked(),
                self.bundle.forked_participants.insert_reserved(record),
            )
            .await
    }

    /// M: read.
    async fn load_by_capability_id(
        &self,
        capability_id: &ForkedParticipantCapabilityId,
    ) -> Result<Option<ForkedParticipantRecord>, MobStoreError> {
        self.bundle
            .read(
                mob_read(),
                self.bundle
                    .forked_participants
                    .load_by_capability_id(capability_id),
            )
            .await
    }

    /// M: read.
    async fn load_by_request_id(
        &self,
        request_id: &ForkedParticipantRequestId,
    ) -> Result<Option<ForkedParticipantRecord>, MobStoreError> {
        self.bundle
            .read(
                mob_read(),
                self.bundle
                    .forked_participants
                    .load_by_request_id(request_id),
            )
            .await
    }

    /// M: read.
    async fn load_by_fork_session_id(
        &self,
        fork_session_id: &meerkat_core::SessionId,
    ) -> Result<Option<ForkedParticipantRecord>, MobStoreError> {
        self.bundle
            .read(
                mob_read(),
                self.bundle
                    .forked_participants
                    .load_by_fork_session_id(fork_session_id),
            )
            .await
    }

    /// M: read.
    async fn load_exact(
        &self,
        capability: &ForkedParticipantRef,
    ) -> Result<ForkedParticipantRecord, MobStoreError> {
        self.bundle
            .read(
                mob_read(),
                self.bundle.forked_participants.load_exact(capability),
            )
            .await
    }

    /// M: every forked participant record.
    async fn commit(
        &self,
        record: &ForkedParticipantRecord,
    ) -> Result<ForkedParticipantRecord, MobStoreError> {
        self.bundle
            .write(forked(), self.bundle.forked_participants.commit(record))
            .await
    }

    /// M: read.
    async fn list_all(&self) -> Result<Vec<ForkedParticipantRecord>, MobStoreError> {
        self.bundle
            .read(mob_read(), self.bundle.forked_participants.list_all())
            .await
    }
}
