//! In-memory store implementations.

use super::realm_profile::{RealmProfileStore, StoredRealmProfile};
use super::{
    BeginPlacedSpawnResult, CommitPlacedSpawnResult, DeletePlacedSpawnResult,
    ExternalBindingOverlayRecord, IdentityMemberEventCommitOutcome,
    IdentityMemberTargetObservation, IdentityWiringEventCommitOutcome,
    IdentityWiringTargetObservation, MobEventStore, MobHostAuthorityDeletionAuthority,
    MobHostAuthorityPersistenceAuthority, MobHostAuthorityRecord, MobIdentityMemberStore,
    MobIdentityStatusStore, MobIdentityStore, MobIdentityStoreClock, MobMemberEventCursorRecord,
    MobMemberLiveCleanupRecord, MobMemberOperatorPruneAuthority, MobMemberOperatorRequestBegin,
    MobMemberOperatorRequestKey, MobMemberOperatorRequestRecord, MobOperatorGrantDeletionAuthority,
    MobOperatorGrantPersistenceAuthority, MobOperatorGrantRecord,
    MobPlacedSpawnBindingPromotionAuthority, MobPlacedSpawnCarrierRecord,
    MobPlacedSpawnCleanupAuthority, MobPlacedSpawnCommitPersistenceAuthority,
    MobPlacedSpawnPendingPersistenceAuthority, MobRunStore, MobRuntimeMetadataStore, MobSpecStore,
    MobStoreError, PlacedSpawnCarrierPhase, PromotePlacedSpawnBindingResult,
    SupervisorAuthorityDeletionAuthority, SupervisorAuthorityPersistenceAuthority,
    SupervisorAuthorityRecord, SystemMobIdentityStoreClock, identity_member_target_state,
    identity_structural_projection_is_anchor, identity_wiring_target_state, private,
    step_failed_event_identity, terminal_event_identity,
    validate_identity_declaration_replay_request, validate_identity_member_commit_authority,
    validate_identity_wiring_commit_authority, validate_mob_event_write_authority,
};
#[cfg(feature = "runtime-adapter")]
use super::{
    identity_runtime_fence_error, validate_identity_runtime_target_binding,
    validate_identity_runtime_write_authority,
};
use crate::definition::MobDefinition;
use crate::event::{MobEvent, MobEventKind, NewMobEvent};
#[cfg(feature = "runtime-adapter")]
use crate::identity::DesiredSessionTarget;
use crate::identity::{
    DesiredInitialDelivery, DesiredMemberSpec, DesiredSessionAuthorityPolicy,
    IDENTITY_DECLARATION_SCOPE_SCHEMA_VERSION, IDENTITY_INTENT_SCHEMA_VERSION,
    IDENTITY_LEASE_MAX_TTL_MS, IDENTITY_LEASE_SCHEMA_VERSION,
    IDENTITY_OPERATION_RECEIPT_SCHEMA_VERSION, IdentityActuationPermit, IdentityActuatorTarget,
    IdentityConvergenceStatus, IdentityDeclarationApplyPlan,
    IdentityDeclarationManifestApplyDisposition, IdentityDeclarationManifestApplyOutcome,
    IdentityDeclarationScopeHead, IdentityDeclarationScopeId, IdentityDeclarationScopePrecondition,
    IdentityIntent, IdentityIntentApplyDisposition, IdentityIntentApplyOutcome,
    IdentityIntentRecord, IdentityLeaseClaim, IdentityLeaseClaimOutcome, IdentityLeaseRecord,
    IdentityOperationKind, IdentityOperationReceipt, IdentityOperationReceiptInsertOutcome,
    IdentityOperationReceiptPayload, IdentityOperationSlot, IdentityOperationSubject,
    IdentityRetirementPlan, IdentityStoredObservation,
};
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
use serde::Serialize;
use sha2::{Digest, Sha256};
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
    aggregate: Arc<RwLock<InMemoryMobAggregateState>>,
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
            aggregate: Arc::new(RwLock::new(InMemoryMobAggregateState::default())),
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

#[derive(Debug, Default)]
pub(super) struct IdentityAuthorityState {
    pub(super) scope_heads:
        BTreeMap<(MobId, IdentityDeclarationScopeId), IdentityDeclarationScopeHead>,
    pub(super) intents: BTreeMap<(MobId, AgentIdentity), IdentityIntentRecord>,
    pub(super) leases: BTreeMap<(MobId, AgentIdentity), IdentityLeaseRecord>,
    pub(super) receipts: BTreeMap<(MobId, String, String), IdentityOperationReceipt>,
}

/// One in-memory lock for the narrow identity slice's cross-resource atomic
/// writes: current intent/lease validation plus one structural member or
/// wiring event append. Ordinary event and identity operations use the same
/// aggregate so the final CAS cannot race either side.
#[derive(Debug, Default)]
struct InMemoryMobAggregateState {
    events: Vec<MobEvent>,
    identity: IdentityAuthorityState,
}

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

/// In-memory first-class identity authority store. One lock preserves
/// atomicity across scope head, many intents, a one-time legacy lease seed,
/// and the immutable manifest receipt; these maps are not independent
/// lifecycle machines.
#[derive(Clone)]
pub struct InMemoryMobIdentityStore {
    aggregate: Arc<RwLock<InMemoryMobAggregateState>>,
    event_tx: Option<broadcast::Sender<MobEvent>>,
    clock: Arc<dyn MobIdentityStoreClock>,
}

#[cfg(feature = "runtime-adapter")]
struct InMemoryIdentityRuntimeWriteFence {
    aggregate: Arc<RwLock<InMemoryMobAggregateState>>,
    clock: Arc<dyn MobIdentityStoreClock>,
    permit: IdentityActuationPermit,
    expected_session: DesiredSessionTarget,
}

#[cfg(feature = "runtime-adapter")]
impl meerkat_runtime::RuntimeStoreWriteFence for InMemoryIdentityRuntimeWriteFence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), meerkat_runtime::RuntimeStoreError> + '_>,
    ) -> Result<meerkat_runtime::RuntimeStoreWriteFenceOutcome, meerkat_runtime::RuntimeStoreError>
    {
        let aggregate = match self.aggregate.try_read() {
            Ok(aggregate) => aggregate,
            Err(error) => {
                return Ok(meerkat_runtime::RuntimeStoreWriteFenceOutcome::Backoff {
                    reason: format!(
                        "identity runtime write fence could not acquire the authority guard: {error}"
                    ),
                });
            }
        };
        let observed_at_ms = match self.clock.now_ms() {
            Ok(observed_at_ms) => observed_at_ms,
            Err(error) => return identity_runtime_fence_error(error),
        };
        let key = (self.permit.mob_id.clone(), self.permit.identity.clone());
        if let Err(error) = validate_identity_runtime_write_authority(
            &self.permit,
            &self.expected_session,
            aggregate.identity.intents.get(&key),
            aggregate.identity.leases.get(&key),
            observed_at_ms,
        ) {
            return identity_runtime_fence_error(error);
        }
        operation()?;
        Ok(meerkat_runtime::RuntimeStoreWriteFenceOutcome::Applied)
    }
}

/// Replaceable in-memory identity diagnostics, deliberately isolated from
/// the desired-state authority lock.
#[derive(Debug, Clone, Default)]
pub struct InMemoryMobIdentityStatusStore {
    statuses: Arc<RwLock<BTreeMap<(MobId, AgentIdentity), IdentityConvergenceStatus>>>,
}

impl InMemoryMobIdentityStatusStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl std::fmt::Debug for InMemoryMobIdentityStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InMemoryMobIdentityStore")
            .field("aggregate", &self.aggregate)
            .field("member_event_capability", &self.event_tx.is_some())
            .field("clock", &"<dyn MobIdentityStoreClock>")
            .finish()
    }
}

impl Default for InMemoryMobIdentityStore {
    fn default() -> Self {
        Self {
            aggregate: Arc::new(RwLock::new(InMemoryMobAggregateState::default())),
            event_tx: None,
            clock: Arc::new(SystemMobIdentityStoreClock),
        }
    }
}

impl InMemoryMobIdentityStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_clock(clock: Arc<dyn MobIdentityStoreClock>) -> Self {
        Self {
            aggregate: Arc::new(RwLock::new(InMemoryMobAggregateState::default())),
            event_tx: None,
            clock,
        }
    }

    /// Pair identity authority with this exact in-memory structural event log.
    /// This is crate-local so custom compositions cannot accidentally claim a
    /// cross-store transaction that they do not implement.
    pub(crate) fn paired_with_event_store(
        events: &InMemoryMobEventStore,
        clock: Arc<dyn MobIdentityStoreClock>,
    ) -> Self {
        Self {
            aggregate: Arc::clone(&events.aggregate),
            event_tx: Some(events.event_tx.clone()),
            clock,
        }
    }
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

fn identity_contract_error(error: crate::identity::IdentityIntentError) -> MobStoreError {
    MobStoreError::Serialization(error.to_string())
}

fn identity_authority_error(error: crate::identity::IdentityIntentError) -> MobStoreError {
    MobStoreError::IdentityAuthorityBlocked {
        evidence_digest: None,
        detail: error.to_string(),
    }
}

fn identity_authority_blocked(detail: impl Into<String>) -> MobStoreError {
    MobStoreError::IdentityAuthorityBlocked {
        evidence_digest: None,
        detail: detail.into(),
    }
}

fn validate_identity_store_text(field: &'static str, value: &str) -> Result<(), MobStoreError> {
    if value.is_empty() || value.trim() != value {
        return Err(identity_contract_error(
            crate::identity::IdentityIntentError::InvalidText { field },
        ));
    }
    Ok(())
}

fn identity_receipt_key(
    mob_id: &MobId,
    subject: &IdentityOperationSubject,
    slot: &IdentityOperationSlot,
) -> Result<(MobId, String, String), MobStoreError> {
    let subject = serde_json::to_string(subject)
        .map_err(|error| MobStoreError::Serialization(error.to_string()))?;
    let slot = serde_json::to_string(slot)
        .map_err(|error| MobStoreError::Serialization(error.to_string()))?;
    Ok((mob_id.clone(), subject, slot))
}

fn identity_apply_outcome(
    disposition: IdentityIntentApplyDisposition,
    record: &IdentityIntentRecord,
) -> IdentityIntentApplyOutcome {
    IdentityIntentApplyOutcome {
        disposition,
        identity: record.intent.identity().clone(),
        intent_revision: record.intent_revision,
        declaration_scope: record.declaration_scope.clone(),
        declaration_revision: record.declaration_revision,
        tombstone_generation: record.tombstone_generation,
        initial_delivery_generation_highwater: record.initial_delivery_generation_highwater,
        intent_digest: record.intent_digest.clone(),
        authority_digest: record.authority_digest.clone(),
        intent: record.intent.clone(),
    }
}

fn next_identity_counter(value: u64, counter: &'static str) -> Result<u64, MobStoreError> {
    value
        .checked_add(1)
        .ok_or_else(|| MobStoreError::IdentityCounterExhausted {
            counter: counter.to_string(),
        })
}

fn seal_identity_intent_record(
    mob_id: &MobId,
    mut record: IdentityIntentRecord,
) -> Result<IdentityIntentRecord, MobStoreError> {
    if record.mob_id != *mob_id {
        return Err(identity_authority_blocked(
            "identity intent draft does not match its physical mob key",
        ));
    }
    record.intent_digest = record.intent.digest().map_err(identity_contract_error)?;
    record.authority_digest = record
        .canonical_authority_digest()
        .map_err(identity_contract_error)?;
    record.validate().map_err(identity_contract_error)?;
    Ok(record)
}

fn has_matching_retirement_proof(
    state: &IdentityAuthorityState,
    mob_id: &MobId,
    record: &IdentityIntentRecord,
) -> Result<bool, MobStoreError> {
    let Some(tombstone_generation) = record.tombstone_generation else {
        return Ok(false);
    };
    let subject = IdentityOperationSubject::Identity {
        identity: record.intent.identity().clone(),
    };
    let slot = IdentityOperationSlot::RetirementProven {
        tombstone_generation,
    };
    let key = identity_receipt_key(mob_id, &subject, &slot)?;
    let Some(receipt) = state.receipts.get(&key) else {
        return Ok(false);
    };
    receipt.validate().map_err(identity_authority_error)?;
    Ok(matches!(
        &receipt.payload,
        IdentityOperationReceiptPayload::RetirementProven {
            absent_authority_digest
        } if absent_authority_digest == &record.authority_digest
    ))
}

fn identity_evidence_digest<T: Serialize>(value: &T) -> Result<String, MobStoreError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| MobStoreError::Serialization(error.to_string()))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn identity_receipt_target(receipt: &IdentityOperationReceipt) -> Option<IdentityActuatorTarget> {
    match receipt.effect_kind {
        IdentityOperationKind::SessionCreationConsumed => {
            Some(IdentityActuatorTarget::SessionCreationReceipt)
        }
        IdentityOperationKind::RetirementProven => Some(IdentityActuatorTarget::RetirementReceipt),
        IdentityOperationKind::ExternalBinding => {
            Some(IdentityActuatorTarget::ExternalBindingReceipt)
        }
        IdentityOperationKind::InitialDelivery => {
            Some(IdentityActuatorTarget::InitialDeliveryReceipt)
        }
        IdentityOperationKind::ApplyDeclarationManifest => None,
    }
}

fn identity_actuator_receipt_matches_intent(
    receipt: &IdentityOperationReceipt,
    intent: &IdentityIntentRecord,
) -> bool {
    use crate::identity::IdentityOperationReceiptPayload as Payload;

    let normalized_tombstone = intent.tombstone_generation.unwrap_or(0);
    match (&receipt.slot, &receipt.payload, &intent.intent) {
        (
            IdentityOperationSlot::SessionCreationConsumed {
                tombstone_generation,
                session_id,
                lineage_id,
                lineage_generation,
            },
            Payload::SessionCreationConsumed { checkpoint },
            IdentityIntent::Present { session, .. },
        ) => {
            *tombstone_generation == normalized_tombstone
                && session_id == &session.session_id
                && lineage_id == &session.lineage_id
                && *lineage_generation == session.lineage_generation
                && checkpoint.session_id() == &session.session_id
                && checkpoint.lineage_id() == &session.lineage_id
                && checkpoint.generation() == session.lineage_generation
        }
        (
            IdentityOperationSlot::RetirementProven {
                tombstone_generation,
            },
            Payload::RetirementProven {
                absent_authority_digest,
            },
            IdentityIntent::Absent { .. },
        ) => {
            *tombstone_generation == normalized_tombstone
                && absent_authority_digest == &intent.authority_digest
        }
        (
            IdentityOperationSlot::ExternalBinding {
                tombstone_generation,
                remote_signing_identity,
                controller_signing_identity,
            },
            Payload::ExternalBinding {
                expected_address,
                expected_identity,
                expected_controller_identity,
                ..
            },
            IdentityIntent::Present { member, .. },
        ) => {
            let crate::identity::DesiredExecution::External { address, identity } =
                member.execution()
            else {
                return false;
            };
            *tombstone_generation == normalized_tombstone
                && expected_address == address
                && expected_identity == identity
                && remote_signing_identity == identity
                && controller_signing_identity == expected_controller_identity
        }
        (
            IdentityOperationSlot::InitialDelivery {
                tombstone_generation,
                session_id,
                lineage_id,
                lineage_generation,
                delivery_generation,
            },
            Payload::InitialDelivery {
                delivery_generation: payload_generation,
                delivery_id,
                message_digest,
            },
            IdentityIntent::Present {
                session, member, ..
            },
        ) => member.initial_delivery.as_ref().is_some_and(|delivery| {
            *tombstone_generation == normalized_tombstone
                && session_id == &session.session_id
                && lineage_id == &session.lineage_id
                && *lineage_generation == session.lineage_generation
                && *delivery_generation == delivery.delivery_generation
                && payload_generation == delivery_generation
                && delivery_id == &delivery.delivery_id
                && message_digest == &delivery.message_digest
        }),
        _ => false,
    }
}

fn malformed_identity_record<T: Clone + Serialize>(
    value: &T,
    detail: impl Into<String>,
) -> Result<IdentityStoredObservation<T>, MobStoreError> {
    Ok(IdentityStoredObservation::Malformed {
        evidence_digest: identity_evidence_digest(value)?,
        detail: detail.into(),
    })
}

fn classify_identity_record<T, F>(
    value: &T,
    validate: F,
) -> Result<IdentityStoredObservation<T>, MobStoreError>
where
    T: Clone + Serialize,
    F: FnOnce(&T) -> Result<(), crate::identity::IdentityIntentError>,
{
    Ok(match validate(value) {
        Ok(()) => IdentityStoredObservation::Valid(value.clone()),
        Err(error @ crate::identity::IdentityIntentError::UnsupportedSchemaVersion { .. }) => {
            IdentityStoredObservation::Unsupported {
                evidence_digest: identity_evidence_digest(value)?,
                detail: error.to_string(),
            }
        }
        Err(error) => IdentityStoredObservation::Malformed {
            evidence_digest: identity_evidence_digest(value)?,
            detail: error.to_string(),
        },
    })
}

pub(super) fn validate_manifest_replay_state(
    state: &IdentityAuthorityState,
    mob_id: &MobId,
    outcome: &IdentityDeclarationManifestApplyOutcome,
) -> Result<(), MobStoreError> {
    outcome.validate().map_err(identity_authority_error)?;
    let scope_key = (mob_id.clone(), outcome.scope_id.clone());
    let head = state.scope_heads.get(&scope_key).ok_or_else(|| {
        identity_authority_blocked(format!(
            "identity declaration replay found no current head for scope '{}'",
            outcome.scope_id.as_str()
        ))
    })?;
    head.validate().map_err(identity_authority_error)?;
    if head.mob_id != *mob_id
        || head.scope_id != outcome.scope_id
        || head.revision < outcome.scope_revision
    {
        return Err(identity_authority_blocked(format!(
            "identity declaration replay observed a regressed or misplaced scope head '{}'",
            outcome.scope_id.as_str()
        )));
    }
    for (identity, prior) in &outcome.identities {
        let current = state
            .intents
            .get(&(mob_id.clone(), identity.clone()))
            .ok_or_else(|| {
                identity_authority_blocked(format!(
                    "identity declaration replay found no current row for '{identity}'"
                ))
            })?;
        current.validate().map_err(identity_authority_error)?;
        if current.mob_id != *mob_id
            || current.intent.identity() != identity
            || current.declaration_scope.as_ref() != Some(&outcome.scope_id)
            || current.intent_revision < prior.intent_revision
            || current.tombstone_generation.unwrap_or(0) < prior.tombstone_generation.unwrap_or(0)
            || current.initial_delivery_generation_highwater
                < prior.initial_delivery_generation_highwater
            || current.declaration_revision.is_none_or(|revision| {
                prior
                    .declaration_revision
                    .is_none_or(|prior_revision| revision < prior_revision)
            })
        {
            return Err(identity_authority_blocked(format!(
                "identity declaration replay observed regressed authority for '{identity}'"
            )));
        }
    }
    Ok(())
}

fn ensure_session_target_unallocated(
    state: &IdentityAuthorityState,
    candidate: &crate::identity::DesiredSessionTarget,
    staged: &[crate::identity::DesiredSessionTarget],
) -> Result<(), MobStoreError> {
    let collides = |existing: &crate::identity::DesiredSessionTarget| {
        existing.session_id == candidate.session_id || existing.lineage_id == candidate.lineage_id
    };
    if staged.iter().any(collides) {
        return Err(MobStoreError::CasConflict(
            "identity declaration allocated a duplicate session or lineage target".to_string(),
        ));
    }
    for ((_, key_identity), record) in &state.intents {
        let _ = key_identity;
        let target = match &record.retirement_plan {
            IdentityRetirementPlan::Targets { session, .. } => Some(session),
            IdentityRetirementPlan::NoKnownRealization => match &record.intent {
                IdentityIntent::Present { session, .. } => Some(session),
                IdentityIntent::Absent { .. } => None,
            },
        };
        if target.is_some_and(collides) {
            return Err(MobStoreError::CasConflict(
                "identity declaration session or lineage target was already allocated".to_string(),
            ));
        }
    }
    Ok(())
}

pub(super) fn apply_identity_declaration_locked(
    state: &mut IdentityAuthorityState,
    mob_id: &MobId,
    plan: &IdentityDeclarationApplyPlan,
) -> Result<IdentityDeclarationManifestApplyOutcome, MobStoreError> {
    validate_identity_store_text("mob_id", mob_id.as_str())?;
    plan.validate().map_err(identity_contract_error)?;

    let manifest_subject = IdentityOperationSubject::DeclarationScope {
        scope_id: plan.scope_id.clone(),
    };
    let manifest_slot = IdentityOperationSlot::ApplyDeclarationManifest {
        scope_id: plan.scope_id.clone(),
        mutation_id: plan.operation_id.clone(),
    };
    let manifest_receipt_key = identity_receipt_key(mob_id, &manifest_subject, &manifest_slot)?;
    if let Some(existing) = state.receipts.get(&manifest_receipt_key) {
        existing.validate().map_err(identity_authority_error)?;
        if let IdentityOperationReceiptPayload::ApplyDeclarationManifest { outcome } =
            &existing.payload
            && outcome.request_digest == plan.request_digest
        {
            validate_manifest_replay_state(state, mob_id, outcome)?;
            return Ok(outcome.clone());
        }
        return Err(MobStoreError::CasConflict(format!(
            "identity declaration operation '{}' was reused with different content",
            plan.operation_id
        )));
    }

    let scope_key = (mob_id.clone(), plan.scope_id.clone());
    let current_scope = state.scope_heads.get(&scope_key).cloned();
    if let Some(head) = &current_scope {
        head.validate().map_err(identity_authority_error)?;
        if &head.mob_id != mob_id || head.scope_id != plan.scope_id {
            return Err(identity_authority_blocked(
                "identity declaration scope head does not match its store key",
            ));
        }
    }
    let mut scoped_rows = BTreeMap::new();
    for ((stored_mob_id, identity), record) in &state.intents {
        if stored_mob_id == mob_id && record.declaration_scope.as_ref() == Some(&plan.scope_id) {
            record.validate().map_err(identity_authority_error)?;
            if record.mob_id != *mob_id || record.intent.identity() != identity {
                return Err(identity_authority_blocked(format!(
                    "identity intent row key '{mob_id}/{identity}' does not match record mob/identity '{}/{}'",
                    record.mob_id,
                    record.intent.identity()
                )));
            }
            scoped_rows.insert(identity.clone(), record.clone());
        }
    }
    let has_manifest_history = current_scope.is_none()
        && state.receipts.values().any(|receipt| {
            matches!(
                (&receipt.subject, &receipt.payload),
                (
                    IdentityOperationSubject::DeclarationScope { scope_id },
                    IdentityOperationReceiptPayload::ApplyDeclarationManifest { .. }
                ) if scope_id == &plan.scope_id && receipt.mob_id == *mob_id
            )
        });
    if current_scope.is_none() && (!scoped_rows.is_empty() || has_manifest_history) {
        return Err(identity_authority_blocked(format!(
            "identity declaration scope head '{}' is missing while durable scope history remains",
            plan.scope_id.as_str()
        )));
    }
    if let Some(head) = &current_scope {
        for record in scoped_rows.values() {
            if record
                .declaration_revision
                .is_some_and(|revision| revision > head.revision)
            {
                return Err(identity_authority_blocked(format!(
                    "identity declaration row revision exceeds scope head '{}' revision",
                    plan.scope_id.as_str()
                )));
            }
        }
    }

    let current_scope_revision = current_scope.as_ref().map(|head| head.revision);
    let precondition_matches = match plan.expected_scope {
        IdentityDeclarationScopePrecondition::Any => true,
        IdentityDeclarationScopePrecondition::Missing => current_scope_revision.is_none(),
        IdentityDeclarationScopePrecondition::Revision { revision } => {
            current_scope_revision == Some(revision)
        }
    };
    if !precondition_matches {
        return Err(MobStoreError::CasConflict(format!(
            "identity declaration scope '{}' expected {:?}, observed {:?}",
            plan.scope_id.as_str(),
            plan.expected_scope,
            current_scope_revision
        )));
    }
    let next_scope_revision = next_identity_counter(
        current_scope_revision.unwrap_or(0),
        "declaration scope revision",
    )?;

    let mut staged_rows = BTreeMap::new();
    let mut staged_leases = BTreeMap::new();
    let mut staged_session_targets = Vec::new();
    let mut outcomes = BTreeMap::new();
    let mut desired_changed = current_scope.is_none();

    for (identity, member_plan) in &plan.members {
        let key = (mob_id.clone(), identity.clone());
        let current = state.intents.get(&key).cloned();
        let legacy_import = plan.legacy_imports.get(identity);
        if legacy_import.is_some() {
            if current.is_some() {
                return Err(MobStoreError::CasConflict(format!(
                    "legacy identity adoption for '{identity}' requires a missing intent row"
                )));
            }
            if state.leases.contains_key(&key) {
                return Err(MobStoreError::CasConflict(format!(
                    "legacy identity adoption for '{identity}' requires a missing lease row"
                )));
            }
        }
        if let Some(current) = &current {
            current.validate().map_err(identity_authority_error)?;
            if current.mob_id != *mob_id || current.intent.identity() != identity {
                return Err(identity_authority_blocked(format!(
                    "identity intent row key '{mob_id}/{identity}' does not match record mob/identity '{}/{}'",
                    current.mob_id,
                    current.intent.identity()
                )));
            }
            match current.declaration_scope.as_ref() {
                Some(scope_id) if scope_id == &plan.scope_id => {}
                Some(scope_id) => {
                    return Err(MobStoreError::CasConflict(format!(
                        "identity '{identity}' is owned by declaration scope '{}'",
                        scope_id.as_str()
                    )));
                }
                None => {
                    return Err(MobStoreError::CasConflict(format!(
                        "identity '{identity}' is owned by an unscoped migration row and has no explicit transfer receipt"
                    )));
                }
            }
        }

        let (mut session, prior_delivery, tombstone_generation, prior_highwater, allocated) =
            match &current {
                Some(current) => match &current.intent {
                    IdentityIntent::Present {
                        session, member, ..
                    } => (
                        session.clone(),
                        member.initial_delivery.clone(),
                        current.tombstone_generation,
                        current.initial_delivery_generation_highwater,
                        false,
                    ),
                    IdentityIntent::Absent { .. } => {
                        if !has_matching_retirement_proof(state, mob_id, current)? {
                            return Err(MobStoreError::CasConflict(format!(
                                "identity '{identity}' cannot be recreated before its exact retirement proof is sealed"
                            )));
                        }
                        if matches!(
                            member_plan.session_authority_policy,
                            DesiredSessionAuthorityPolicy::RequireExisting
                        ) {
                            return Err(MobStoreError::WriteFailed(format!(
                                "identity '{identity}' cannot allocate a new RequireExisting session target"
                            )));
                        }
                        (
                            member_plan.candidate_session_target(),
                            None,
                            current.tombstone_generation,
                            current.initial_delivery_generation_highwater,
                            true,
                        )
                    }
                },
                None if legacy_import.is_some() => {
                    let legacy_import = legacy_import.ok_or_else(|| {
                        MobStoreError::Internal(
                            "legacy identity adoption disappeared during validation".to_string(),
                        )
                    })?;
                    (legacy_import.session().clone(), None, None, 0, true)
                }
                None => {
                    if matches!(
                        member_plan.session_authority_policy,
                        DesiredSessionAuthorityPolicy::RequireExisting
                    ) {
                        return Err(MobStoreError::WriteFailed(format!(
                            "new identity '{identity}' cannot allocate a RequireExisting session target"
                        )));
                    }
                    (member_plan.candidate_session_target(), None, None, 0, true)
                }
            };
        if allocated {
            ensure_session_target_unallocated(state, &session, &staged_session_targets)?;
            staged_session_targets.push(session.clone());
        }
        session.authority_policy = member_plan.session_authority_policy;

        if let Some(legacy_import) = legacy_import {
            let lease = IdentityLeaseRecord {
                schema_version: IDENTITY_LEASE_SCHEMA_VERSION,
                epoch_highwater: legacy_import.continuity_epoch_highwater(),
                active: None,
            };
            lease.validate().map_err(identity_contract_error)?;
            staged_leases.insert(identity.clone(), lease);
        }

        let (initial_delivery, delivery_highwater) = match &member_plan.initial_message {
            None => (None, prior_highwater),
            Some(message) => {
                let candidate_id = member_plan
                    .candidate_initial_delivery_id
                    .as_ref()
                    .ok_or_else(|| {
                        MobStoreError::WriteFailed(
                            "initial delivery candidate id is missing".to_string(),
                        )
                    })?;
                let candidate_for_digest =
                    DesiredInitialDelivery::new(1, candidate_id.clone(), message.clone())
                        .map_err(identity_contract_error)?;
                if let Some(existing) = prior_delivery
                    && existing.message_digest == candidate_for_digest.message_digest
                {
                    (Some(existing), prior_highwater)
                } else {
                    let generation =
                        next_identity_counter(prior_highwater, "initial delivery generation")?;
                    (
                        Some(
                            DesiredInitialDelivery::new(
                                generation,
                                candidate_id.clone(),
                                message.clone(),
                            )
                            .map_err(identity_contract_error)?,
                        ),
                        generation,
                    )
                }
            }
        };

        let owned_wiring = plan
            .wiring
            .iter()
            .filter(|edge| edge.owner() == identity)
            .cloned()
            .collect();
        let incident_wiring = plan
            .wiring
            .iter()
            .filter(|edge| &edge.a == identity || &edge.b == identity)
            .cloned()
            .collect();
        let intent = IdentityIntent::Present {
            identity: identity.clone(),
            session: session.clone(),
            member: Box::new(DesiredMemberSpec {
                material: member_plan.material.clone(),
                initial_delivery,
            }),
            owned_wiring,
        };
        let retirement_plan = IdentityRetirementPlan::Targets {
            session,
            execution: member_plan.material.execution.clone(),
            incident_wiring,
        };
        let unchanged = current.as_ref().is_some_and(|current| {
            current.intent == intent
                && current.retirement_plan == retirement_plan
                && current.initial_delivery_generation_highwater == delivery_highwater
        });
        let record = if unchanged {
            current.ok_or_else(|| {
                MobStoreError::Internal(
                    "unchanged identity declaration unexpectedly had no current row".to_string(),
                )
            })?
        } else {
            desired_changed = true;
            let intent_revision = next_identity_counter(
                current.as_ref().map_or(0, |record| record.intent_revision),
                "intent revision",
            )?;
            seal_identity_intent_record(
                mob_id,
                IdentityIntentRecord {
                    schema_version: IDENTITY_INTENT_SCHEMA_VERSION,
                    mob_id: mob_id.clone(),
                    intent_revision,
                    declaration_scope: Some(plan.scope_id.clone()),
                    declaration_revision: Some(next_scope_revision),
                    tombstone_generation,
                    initial_delivery_generation_highwater: delivery_highwater,
                    retirement_plan,
                    intent_digest: String::new(),
                    authority_digest: String::new(),
                    intent,
                },
            )?
        };
        if !unchanged {
            staged_rows.insert(identity.clone(), record.clone());
        }
        outcomes.insert(
            identity.clone(),
            identity_apply_outcome(
                if unchanged {
                    IdentityIntentApplyDisposition::Unchanged
                } else {
                    IdentityIntentApplyDisposition::Applied
                },
                &record,
            ),
        );
    }

    for (identity, current) in scoped_rows {
        if plan.members.contains_key(&identity) {
            continue;
        }
        let (record, disposition) = match &current.intent {
            IdentityIntent::Absent { .. } => {
                (current.clone(), IdentityIntentApplyDisposition::Unchanged)
            }
            IdentityIntent::Present { .. } => {
                desired_changed = true;
                let tombstone_generation = next_identity_counter(
                    current.tombstone_generation.unwrap_or(0),
                    "tombstone generation",
                )?;
                let intent_revision =
                    next_identity_counter(current.intent_revision, "intent revision")?;
                let record = seal_identity_intent_record(
                    mob_id,
                    IdentityIntentRecord {
                        schema_version: IDENTITY_INTENT_SCHEMA_VERSION,
                        mob_id: mob_id.clone(),
                        intent_revision,
                        declaration_scope: Some(plan.scope_id.clone()),
                        declaration_revision: Some(next_scope_revision),
                        tombstone_generation: Some(tombstone_generation),
                        initial_delivery_generation_highwater: current
                            .initial_delivery_generation_highwater,
                        retirement_plan: current.retirement_plan.clone(),
                        intent_digest: String::new(),
                        authority_digest: String::new(),
                        intent: IdentityIntent::Absent {
                            identity: identity.clone(),
                        },
                    },
                )?;
                staged_rows.insert(identity.clone(), record.clone());
                (record, IdentityIntentApplyDisposition::Applied)
            }
        };
        outcomes.insert(
            identity.clone(),
            identity_apply_outcome(disposition, &record),
        );
    }

    let mut outcome = IdentityDeclarationManifestApplyOutcome {
        disposition: if desired_changed {
            IdentityDeclarationManifestApplyDisposition::Applied
        } else {
            IdentityDeclarationManifestApplyDisposition::Unchanged
        },
        scope_id: plan.scope_id.clone(),
        scope_revision: next_scope_revision,
        request_digest: plan.request_digest.clone(),
        compiled_manifest_digest: String::new(),
        identities: outcomes,
    };
    outcome.compiled_manifest_digest = outcome
        .canonical_compiled_manifest_digest()
        .map_err(identity_contract_error)?;
    outcome.validate().map_err(identity_contract_error)?;

    let mut scope_head = IdentityDeclarationScopeHead {
        schema_version: IDENTITY_DECLARATION_SCOPE_SCHEMA_VERSION,
        mob_id: mob_id.clone(),
        scope_id: plan.scope_id.clone(),
        revision: next_scope_revision,
        operation_id: plan.operation_id.clone(),
        request_digest: plan.request_digest.clone(),
        compiled_manifest_digest: outcome.compiled_manifest_digest.clone(),
        declared_member_count: u64::try_from(plan.members.len()).map_err(|_| {
            MobStoreError::WriteFailed("identity declaration member count overflow".to_string())
        })?,
        authority_digest: String::new(),
    };
    scope_head.authority_digest = scope_head
        .canonical_authority_digest()
        .map_err(identity_contract_error)?;
    scope_head.validate().map_err(identity_contract_error)?;

    let mut receipt = IdentityOperationReceipt {
        schema_version: IDENTITY_OPERATION_RECEIPT_SCHEMA_VERSION,
        mob_id: mob_id.clone(),
        subject: manifest_subject,
        effect_kind: IdentityOperationKind::ApplyDeclarationManifest,
        slot: manifest_slot,
        receipt_id: plan.operation_id.clone(),
        intent_revision: None,
        intent_digest: None,
        intent_authority_digest: None,
        tombstone_generation: None,
        audit_lease_epoch: None,
        request_digest: String::new(),
        payload: IdentityOperationReceiptPayload::ApplyDeclarationManifest {
            outcome: outcome.clone(),
        },
    };
    receipt.request_digest = receipt
        .canonical_request_digest()
        .map_err(identity_contract_error)?;
    receipt.validate().map_err(identity_contract_error)?;

    for (identity, record) in staged_rows {
        state.intents.insert((mob_id.clone(), identity), record);
    }
    for (identity, record) in staged_leases {
        state.leases.insert((mob_id.clone(), identity), record);
    }
    state.scope_heads.insert(scope_key, scope_head);
    state.receipts.insert(manifest_receipt_key, receipt);
    Ok(outcome)
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobIdentityStore for InMemoryMobIdentityStore {
    #[cfg(feature = "runtime-adapter")]
    fn prepare_runtime_write_fence(
        &self,
        permit: IdentityActuationPermit,
        expected_session: DesiredSessionTarget,
        observed: &meerkat_runtime::RuntimeSessionLifecycleObservation,
    ) -> Result<Arc<dyn meerkat_runtime::RuntimeStoreWriteFence>, MobStoreError> {
        validate_identity_runtime_target_binding(&permit, &expected_session, observed)?;
        Ok(Arc::new(InMemoryIdentityRuntimeWriteFence {
            aggregate: Arc::clone(&self.aggregate),
            clock: Arc::clone(&self.clock),
            permit,
            expected_session,
        }))
    }

    async fn observe_identity_declaration_scope(
        &self,
        mob_id: &MobId,
        scope_id: &IdentityDeclarationScopeId,
    ) -> Result<IdentityStoredObservation<IdentityDeclarationScopeHead>, MobStoreError> {
        let aggregate = self.aggregate.read().await;
        let state = &aggregate.identity;
        let Some(head) = state.scope_heads.get(&(mob_id.clone(), scope_id.clone())) else {
            return Ok(IdentityStoredObservation::Missing);
        };
        if head.mob_id != *mob_id || head.scope_id != *scope_id {
            return malformed_identity_record(
                head,
                "identity declaration scope head does not match its physical key",
            );
        }
        classify_identity_record(head, IdentityDeclarationScopeHead::validate)
    }

    async fn observe_identity_intent(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityStoredObservation<IdentityIntentRecord>, MobStoreError> {
        let aggregate = self.aggregate.read().await;
        let state = &aggregate.identity;
        let Some(record) = state.intents.get(&(mob_id.clone(), identity.clone())) else {
            return Ok(IdentityStoredObservation::Missing);
        };
        if record.mob_id != *mob_id || record.intent.identity() != identity {
            return malformed_identity_record(
                record,
                "identity intent record does not match its physical mob/identity key",
            );
        }
        classify_identity_record(record, IdentityIntentRecord::validate)
    }

    async fn list_identity_intents(
        &self,
        mob_id: &MobId,
    ) -> Result<
        BTreeMap<AgentIdentity, IdentityStoredObservation<IdentityIntentRecord>>,
        MobStoreError,
    > {
        let aggregate = self.aggregate.read().await;
        let state = &aggregate.identity;
        let mut observations = BTreeMap::new();
        for ((stored_mob_id, identity), record) in &state.intents {
            if stored_mob_id != mob_id {
                continue;
            }
            let observation = if record.mob_id != *mob_id || record.intent.identity() != identity {
                malformed_identity_record(
                    record,
                    "identity intent record does not match its physical mob/identity key",
                )?
            } else {
                classify_identity_record(record, IdentityIntentRecord::validate)?
            };
            observations.insert(identity.clone(), observation);
        }
        Ok(observations)
    }

    async fn replay_identity_declaration(
        &self,
        mob_id: &MobId,
        scope_id: &IdentityDeclarationScopeId,
        operation_id: &meerkat_core::ops::OperationId,
        request_digest: &str,
    ) -> Result<Option<IdentityDeclarationManifestApplyOutcome>, MobStoreError> {
        validate_identity_declaration_replay_request(
            mob_id,
            scope_id,
            operation_id,
            request_digest,
        )?;
        let subject = IdentityOperationSubject::DeclarationScope {
            scope_id: scope_id.clone(),
        };
        let slot = IdentityOperationSlot::ApplyDeclarationManifest {
            scope_id: scope_id.clone(),
            mutation_id: operation_id.clone(),
        };
        let key = identity_receipt_key(mob_id, &subject, &slot)?;
        let aggregate = self.aggregate.read().await;
        let state = &aggregate.identity;
        let Some(receipt) = state.receipts.get(&key) else {
            return Ok(None);
        };
        receipt.validate().map_err(identity_authority_error)?;
        let IdentityOperationReceiptPayload::ApplyDeclarationManifest { outcome } =
            &receipt.payload
        else {
            return Err(identity_authority_blocked(
                "identity declaration replay slot contains a non-manifest receipt",
            ));
        };
        if outcome.request_digest != request_digest {
            return Err(MobStoreError::CasConflict(format!(
                "identity declaration operation '{operation_id}' was reused with different content"
            )));
        }
        validate_manifest_replay_state(state, mob_id, outcome)?;
        Ok(Some(outcome.clone()))
    }

    async fn apply_identity_declaration(
        &self,
        mob_id: &MobId,
        plan: &IdentityDeclarationApplyPlan,
    ) -> Result<IdentityDeclarationManifestApplyOutcome, MobStoreError> {
        let mut aggregate = self.aggregate.write().await;
        apply_identity_declaration_locked(&mut aggregate.identity, mob_id, plan)
    }

    async fn observe_identity_lease(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityStoredObservation<IdentityLeaseRecord>, MobStoreError> {
        let aggregate = self.aggregate.read().await;
        let state = &aggregate.identity;
        let Some(record) = state.leases.get(&(mob_id.clone(), identity.clone())) else {
            return Ok(IdentityStoredObservation::Missing);
        };
        classify_identity_record(record, IdentityLeaseRecord::validate)
    }

    async fn claim_or_renew_identity_lease(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
        holder_id: &str,
        incarnation_id: &str,
        ttl_ms: u64,
    ) -> Result<IdentityLeaseClaimOutcome, MobStoreError> {
        validate_identity_store_text("mob_id", mob_id.as_str())?;
        validate_identity_store_text("identity", identity.as_str())?;
        validate_identity_store_text("holder_id", holder_id)?;
        validate_identity_store_text("incarnation_id", incarnation_id)?;
        if ttl_ms == 0 || ttl_ms > IDENTITY_LEASE_MAX_TTL_MS {
            return Err(identity_contract_error(
                crate::identity::IdentityIntentError::InvalidLeaseLifetime,
            ));
        }
        let key = (mob_id.clone(), identity.clone());
        let mut aggregate = self.aggregate.write().await;
        let state = &mut aggregate.identity;
        let observed_at_ms = self.clock.now_ms()?;
        let expires_at_ms = observed_at_ms.checked_add(ttl_ms).ok_or_else(|| {
            MobStoreError::WriteFailed("identity lease expiry overflow".to_string())
        })?;
        let current = state.leases.get(&key).cloned();
        if let Some(record) = &current {
            record.validate().map_err(identity_authority_error)?;
            if let Some(active) = &record.active
                && observed_at_ms < active.expires_at_ms
            {
                if active.holder_id != holder_id || active.incarnation_id != incarnation_id {
                    return Ok(IdentityLeaseClaimOutcome::HeldByOther(active.clone()));
                }
                if observed_at_ms < active.renewed_at_ms {
                    return Err(MobStoreError::WriteFailed(
                        "identity lease clock regressed before the last renewal".to_string(),
                    ));
                }
                let claim = IdentityLeaseClaim {
                    holder_id: holder_id.to_string(),
                    incarnation_id: incarnation_id.to_string(),
                    epoch: active.epoch,
                    renewed_at_ms: observed_at_ms,
                    expires_at_ms,
                };
                let record = IdentityLeaseRecord {
                    schema_version: IDENTITY_LEASE_SCHEMA_VERSION,
                    epoch_highwater: current
                        .as_ref()
                        .map_or(claim.epoch, |record| record.epoch_highwater),
                    active: Some(claim.clone()),
                };
                record.validate().map_err(identity_contract_error)?;
                state.leases.insert(key, record);
                return Ok(IdentityLeaseClaimOutcome::Renewed(claim));
            }
        }

        let epoch = next_identity_counter(
            current.as_ref().map_or(0, |record| record.epoch_highwater),
            "identity lease epoch",
        )?;
        let claim = IdentityLeaseClaim {
            holder_id: holder_id.to_string(),
            incarnation_id: incarnation_id.to_string(),
            epoch,
            renewed_at_ms: observed_at_ms,
            expires_at_ms,
        };
        let record = IdentityLeaseRecord {
            schema_version: IDENTITY_LEASE_SCHEMA_VERSION,
            epoch_highwater: epoch,
            active: Some(claim.clone()),
        };
        record.validate().map_err(identity_contract_error)?;
        state.leases.insert(key, record);
        Ok(IdentityLeaseClaimOutcome::Acquired(claim))
    }

    async fn release_identity_lease(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
        expected: &IdentityLeaseClaim,
    ) -> Result<bool, MobStoreError> {
        validate_identity_store_text("mob_id", mob_id.as_str())?;
        validate_identity_store_text("identity", identity.as_str())?;
        let expected_record = IdentityLeaseRecord {
            schema_version: IDENTITY_LEASE_SCHEMA_VERSION,
            epoch_highwater: expected.epoch,
            active: Some(expected.clone()),
        };
        expected_record
            .validate()
            .map_err(identity_contract_error)?;
        let key = (mob_id.clone(), identity.clone());
        let mut aggregate = self.aggregate.write().await;
        let state = &mut aggregate.identity;
        let Some(current) = state.leases.get(&key).cloned() else {
            return Ok(false);
        };
        current.validate().map_err(identity_authority_error)?;
        if current.active.as_ref() != Some(expected) {
            return Ok(false);
        }
        let released = IdentityLeaseRecord {
            schema_version: IDENTITY_LEASE_SCHEMA_VERSION,
            epoch_highwater: current.epoch_highwater,
            active: None,
        };
        released.validate().map_err(identity_contract_error)?;
        state.leases.insert(key, released);
        Ok(true)
    }

    async fn validate_identity_actuation_permit(
        &self,
        permit: &IdentityActuationPermit,
    ) -> Result<(), MobStoreError> {
        let observed_at_ms = self.clock.now_ms()?;
        permit.validate_for_write(observed_at_ms).map_err(|error| {
            MobStoreError::CasConflict(format!(
                "identity actuation permit is no longer current: {error}"
            ))
        })?;
        let aggregate = self.aggregate.read().await;
        let state = &aggregate.identity;
        let intent = state
            .intents
            .get(&(permit.mob_id.clone(), permit.identity.clone()))
            .ok_or_else(|| {
                MobStoreError::CasConflict(
                    "identity actuation observed no current intent".to_string(),
                )
            })?;
        intent.validate().map_err(identity_authority_error)?;
        if intent.mob_id != permit.mob_id
            || intent.intent.identity() != &permit.identity
            || intent.intent_revision != permit.intent_revision
            || intent.intent_digest != permit.intent_digest
            || intent.authority_digest != permit.intent_authority_digest
        {
            return Err(MobStoreError::CasConflict(
                "identity actuation intent authority is stale".to_string(),
            ));
        }
        let lease = state
            .leases
            .get(&(permit.mob_id.clone(), permit.identity.clone()))
            .ok_or_else(|| {
                MobStoreError::CasConflict(
                    "identity actuation observed no current lease".to_string(),
                )
            })?;
        lease.validate().map_err(identity_authority_error)?;
        let active = lease.active.as_ref().ok_or_else(|| {
            MobStoreError::CasConflict("identity actuation observed no active lease".to_string())
        })?;
        if active.epoch != permit.lease_epoch
            || active.holder_id != permit.lease_holder_id
            || active.incarnation_id != permit.lease_incarnation_id
            || active.expires_at_ms != permit.lease_expires_at_ms
            || observed_at_ms >= active.expires_at_ms
        {
            return Err(MobStoreError::CasConflict(
                "identity actuation lease authority is stale".to_string(),
            ));
        }
        Ok(())
    }

    async fn observe_identity_operation_receipt(
        &self,
        mob_id: &MobId,
        subject: &IdentityOperationSubject,
        slot: &IdentityOperationSlot,
    ) -> Result<IdentityStoredObservation<IdentityOperationReceipt>, MobStoreError> {
        let key = identity_receipt_key(mob_id, subject, slot)?;
        let aggregate = self.aggregate.read().await;
        let state = &aggregate.identity;
        let Some(receipt) = state.receipts.get(&key) else {
            return Ok(IdentityStoredObservation::Missing);
        };
        if receipt.mob_id != *mob_id || receipt.subject != *subject || receipt.slot != *slot {
            return malformed_identity_record(
                receipt,
                "identity operation receipt does not match its physical key",
            );
        }
        classify_identity_record(receipt, IdentityOperationReceipt::validate)
    }

    async fn insert_identity_operation_receipt_if_absent(
        &self,
        receipt: &IdentityOperationReceipt,
        permit: &IdentityActuationPermit,
    ) -> Result<IdentityOperationReceiptInsertOutcome, MobStoreError> {
        receipt.validate().map_err(identity_contract_error)?;
        let key = identity_receipt_key(&receipt.mob_id, &receipt.subject, &receipt.slot)?;
        let mut aggregate = self.aggregate.write().await;
        let state = &mut aggregate.identity;
        if let Some(existing) = state.receipts.get(&key) {
            existing.validate().map_err(identity_authority_error)?;
            return Ok(if existing.request_digest == receipt.request_digest {
                IdentityOperationReceiptInsertOutcome::ExistingExact(existing.clone())
            } else {
                IdentityOperationReceiptInsertOutcome::Conflict(existing.clone())
            });
        }

        let observed_at_ms = self.clock.now_ms()?;
        permit.validate_for_write(observed_at_ms).map_err(|error| {
            MobStoreError::CasConflict(format!(
                "identity receipt insertion permit is no longer current: {error}"
            ))
        })?;
        let IdentityOperationSubject::Identity { identity } = &receipt.subject else {
            return Err(MobStoreError::WriteFailed(
                "declaration/apply receipts must be inserted by their owning desired-state transaction"
                    .to_string(),
            ));
        };
        if receipt.mob_id != permit.mob_id
            || identity != &permit.identity
            || identity_receipt_target(receipt) != Some(permit.target)
            || receipt.intent_revision != Some(permit.intent_revision)
            || receipt.intent_digest.as_ref() != Some(&permit.intent_digest)
            || receipt.intent_authority_digest.as_ref() != Some(&permit.intent_authority_digest)
            || !matches!(
                permit.target_observation,
                crate::identity::IdentityTargetObservationVersion::InsertIfAbsent
            )
        {
            return Err(MobStoreError::CasConflict(
                "identity receipt insertion permit does not match the immutable receipt slot"
                    .to_string(),
            ));
        }

        let intent = state
            .intents
            .get(&(receipt.mob_id.clone(), identity.clone()))
            .ok_or_else(|| {
                MobStoreError::CasConflict(
                    "identity receipt insertion observed no current intent".to_string(),
                )
            })?;
        intent.validate().map_err(identity_authority_error)?;
        if intent.mob_id != receipt.mob_id
            || intent.intent.identity() != identity
            || intent.intent_revision != permit.intent_revision
            || intent.intent_digest != permit.intent_digest
            || intent.authority_digest != permit.intent_authority_digest
            || !identity_actuator_receipt_matches_intent(receipt, intent)
            || receipt
                .audit_lease_epoch
                .is_some_and(|epoch| epoch != permit.lease_epoch)
        {
            return Err(MobStoreError::CasConflict(
                "identity receipt insertion intent authority is stale".to_string(),
            ));
        }

        let lease = state
            .leases
            .get(&(receipt.mob_id.clone(), identity.clone()))
            .ok_or_else(|| {
                MobStoreError::CasConflict(
                    "identity receipt insertion observed no current lease".to_string(),
                )
            })?;
        lease.validate().map_err(identity_authority_error)?;
        let Some(active) = &lease.active else {
            return Err(MobStoreError::CasConflict(
                "identity receipt insertion observed no active lease".to_string(),
            ));
        };
        if active.epoch != permit.lease_epoch
            || active.holder_id != permit.lease_holder_id
            || active.incarnation_id != permit.lease_incarnation_id
            || active.expires_at_ms != permit.lease_expires_at_ms
            || observed_at_ms >= active.expires_at_ms
        {
            return Err(MobStoreError::CasConflict(
                "identity receipt insertion lease authority is stale".to_string(),
            ));
        }
        state.receipts.insert(key, receipt.clone());
        Ok(IdentityOperationReceiptInsertOutcome::Inserted(
            receipt.clone(),
        ))
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobIdentityMemberStore for InMemoryMobIdentityStore {
    async fn observe_identity_member_target(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityMemberTargetObservation, MobStoreError> {
        if self.event_tx.is_none() {
            return Err(MobStoreError::IdentityMemberAtomicPersistenceUnavailable);
        }
        let aggregate = self.aggregate.read().await;
        Ok(identity_member_target_state(&aggregate.events, mob_id, identity).observation)
    }

    async fn commit_identity_member_spawned(
        &self,
        permit: &IdentityActuationPermit,
        event: &NewMobEvent,
    ) -> Result<IdentityMemberEventCommitOutcome, MobStoreError> {
        let Some(event_tx) = &self.event_tx else {
            return Err(MobStoreError::IdentityMemberAtomicPersistenceUnavailable);
        };
        if let Err(error) = validate_mob_event_write_authority(&event.kind) {
            return Ok(IdentityMemberEventCommitOutcome::RepairBlocked {
                evidence_digest: None,
                detail: error.to_string(),
            });
        }

        let mut aggregate = self.aggregate.write().await;
        // The clock is sampled only after this aggregate's writer lock is
        // held. A queued writer therefore cannot commit a lease that expired
        // while it waited for the target-local CAS.
        let observed_at_ms = match self.clock.now_ms() {
            Ok(observed_at_ms) => observed_at_ms,
            Err(error) => {
                return Ok(IdentityMemberEventCommitOutcome::Backoff {
                    detail: error.to_string(),
                });
            }
        };
        let proposed = MobEvent {
            cursor: 0,
            timestamp: event.timestamp.unwrap_or_else(Utc::now),
            mob_id: event.mob_id.clone(),
            kind: event.kind.clone(),
        };
        let Some(intent) = aggregate
            .identity
            .intents
            .get(&(permit.mob_id.clone(), permit.identity.clone()))
        else {
            return Ok(IdentityMemberEventCommitOutcome::Conflict {
                current: None,
                detail: "identity member commit observed no current intent".to_string(),
            });
        };
        let Some(lease) = aggregate
            .identity
            .leases
            .get(&(permit.mob_id.clone(), permit.identity.clone()))
        else {
            return Ok(IdentityMemberEventCommitOutcome::Conflict {
                current: None,
                detail: "identity member commit observed no current lease".to_string(),
            });
        };
        if let Err(outcome) = validate_identity_member_commit_authority(
            permit,
            intent,
            lease,
            &proposed,
            observed_at_ms,
        ) {
            return Ok(outcome);
        }

        let current =
            identity_member_target_state(&aggregate.events, &permit.mob_id, &permit.identity);
        if let IdentityMemberTargetObservation::Malformed {
            observed_version,
            detail,
        } = &current.observation
        {
            return Ok(IdentityMemberEventCommitOutcome::RepairBlocked {
                evidence_digest: observed_version.clone(),
                detail: detail.clone(),
            });
        }
        let expected = current.observation.target_precondition();
        if expected.as_ref() != Some(&permit.target_observation) {
            return Ok(IdentityMemberEventCommitOutcome::Conflict {
                current: Some(current.observation),
                detail: "identity member target observation is stale".to_string(),
            });
        }
        if let Some(existing) = current.exact_current_spawn
            && existing.kind == proposed.kind
        {
            return Ok(IdentityMemberEventCommitOutcome::AlreadyExact { event: existing });
        }
        if !matches!(
            permit.target_observation,
            crate::identity::IdentityTargetObservationVersion::Absent { .. }
        ) {
            return Ok(IdentityMemberEventCommitOutcome::RepairBlocked {
                evidence_digest: current
                    .observation
                    .target_precondition()
                    .and_then(|target| match target {
                        crate::identity::IdentityTargetObservationVersion::Version { version } => {
                            Some(version)
                        }
                        _ => None,
                    }),
                detail: "MemberSpawned append requires an exact absent-target witness".to_string(),
            });
        }

        let Some(cursor) = aggregate
            .events
            .last()
            .map_or(Some(1), |existing| existing.cursor.checked_add(1))
        else {
            return Ok(IdentityMemberEventCommitOutcome::RepairBlocked {
                evidence_digest: None,
                detail: "mob event cursor is exhausted".to_string(),
            });
        };
        let stored = MobEvent {
            cursor,
            timestamp: proposed.timestamp,
            mob_id: proposed.mob_id,
            kind: proposed.kind,
        };
        aggregate.events.push(stored.clone());
        drop(aggregate);
        let _ = event_tx.send(stored.clone());
        Ok(IdentityMemberEventCommitOutcome::Applied { event: stored })
    }

    async fn observe_identity_wiring_target(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityWiringTargetObservation, MobStoreError> {
        if self.event_tx.is_none() {
            return Err(MobStoreError::IdentityMemberAtomicPersistenceUnavailable);
        }
        let aggregate = self.aggregate.read().await;
        Ok(identity_wiring_target_state(&aggregate.events, mob_id, identity).observation)
    }

    async fn commit_identity_wiring_event(
        &self,
        permit: &IdentityActuationPermit,
        event: &NewMobEvent,
    ) -> Result<IdentityWiringEventCommitOutcome, MobStoreError> {
        let Some(event_tx) = &self.event_tx else {
            return Err(MobStoreError::IdentityMemberAtomicPersistenceUnavailable);
        };
        if let Err(error) = validate_mob_event_write_authority(&event.kind) {
            return Ok(IdentityWiringEventCommitOutcome::RepairBlocked {
                evidence_digest: None,
                detail: error.to_string(),
            });
        }

        let mut aggregate = self.aggregate.write().await;
        let observed_at_ms = match self.clock.now_ms() {
            Ok(observed_at_ms) => observed_at_ms,
            Err(error) => {
                return Ok(IdentityWiringEventCommitOutcome::Backoff {
                    detail: error.to_string(),
                });
            }
        };
        let proposed = MobEvent {
            cursor: 0,
            timestamp: event.timestamp.unwrap_or_else(Utc::now),
            mob_id: event.mob_id.clone(),
            kind: event.kind.clone(),
        };
        let Some(intent) = aggregate
            .identity
            .intents
            .get(&(permit.mob_id.clone(), permit.identity.clone()))
        else {
            return Ok(IdentityWiringEventCommitOutcome::Conflict {
                current: None,
                detail: "identity wiring commit observed no current intent".to_string(),
            });
        };
        let Some(lease) = aggregate
            .identity
            .leases
            .get(&(permit.mob_id.clone(), permit.identity.clone()))
        else {
            return Ok(IdentityWiringEventCommitOutcome::Conflict {
                current: None,
                detail: "identity wiring commit observed no current lease".to_string(),
            });
        };
        let (edge, adding) = match validate_identity_wiring_commit_authority(
            permit,
            intent,
            lease,
            &proposed,
            observed_at_ms,
        ) {
            Ok(validated) => validated,
            Err(outcome) => return Ok(outcome),
        };

        let current =
            identity_wiring_target_state(&aggregate.events, &permit.mob_id, &permit.identity);
        if let IdentityWiringTargetObservation::Malformed {
            observed_version,
            detail,
        } = &current.observation
        {
            return Ok(IdentityWiringEventCommitOutcome::RepairBlocked {
                evidence_digest: observed_version.clone(),
                detail: detail.clone(),
            });
        }
        if current.observation.target_precondition().as_ref() != Some(&permit.target_observation) {
            return Ok(IdentityWiringEventCommitOutcome::Conflict {
                current: Some(current.observation),
                detail: "identity wiring target observation is stale".to_string(),
            });
        }
        if current.observation.contains(&edge) == adding {
            return Ok(IdentityWiringEventCommitOutcome::AlreadyExact {
                current: current.observation,
            });
        }

        let Some(cursor) = aggregate
            .events
            .last()
            .map_or(Some(1), |existing| existing.cursor.checked_add(1))
        else {
            return Ok(IdentityWiringEventCommitOutcome::RepairBlocked {
                evidence_digest: None,
                detail: "mob event cursor is exhausted".to_string(),
            });
        };
        let stored = MobEvent {
            cursor,
            timestamp: proposed.timestamp,
            mob_id: proposed.mob_id,
            kind: proposed.kind,
        };
        aggregate.events.push(stored.clone());
        drop(aggregate);
        let _ = event_tx.send(stored.clone());
        Ok(IdentityWiringEventCommitOutcome::Applied { event: stored })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobIdentityStatusStore for InMemoryMobIdentityStatusStore {
    async fn load_identity_convergence_status(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityStoredObservation<IdentityConvergenceStatus>, MobStoreError> {
        Ok(self
            .statuses
            .read()
            .await
            .get(&(mob_id.clone(), identity.clone()))
            .cloned()
            .map_or(
                IdentityStoredObservation::Missing,
                IdentityStoredObservation::Valid,
            ))
    }

    async fn list_identity_convergence_statuses(
        &self,
        mob_id: &MobId,
    ) -> Result<
        BTreeMap<AgentIdentity, IdentityStoredObservation<IdentityConvergenceStatus>>,
        MobStoreError,
    > {
        Ok(self
            .statuses
            .read()
            .await
            .iter()
            .filter(|((stored_mob_id, _), _)| stored_mob_id == mob_id)
            .map(|((_, identity), status)| {
                (
                    identity.clone(),
                    IdentityStoredObservation::Valid(status.clone()),
                )
            })
            .collect())
    }

    async fn replace_identity_convergence_status(
        &self,
        mob_id: &MobId,
        status: &IdentityConvergenceStatus,
    ) -> Result<(), MobStoreError> {
        self.statuses
            .write()
            .await
            .insert((mob_id.clone(), status.identity.clone()), status.clone());
        Ok(())
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

        let mut aggregate = self.aggregate.write().await;
        let events = &mut aggregate.events;
        let cursor = events.last().map_or(1, |existing| existing.cursor + 1);
        let stored = MobEvent {
            cursor,
            timestamp: event.timestamp.unwrap_or_else(Utc::now),
            mob_id: event.mob_id,
            kind: event.kind,
        };
        events.push(stored.clone());
        drop(aggregate);
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

        let mut aggregate = self.aggregate.write().await;
        let events = &mut aggregate.events;
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
        drop(aggregate);
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

        let mut aggregate = self.aggregate.write().await;
        let events = &mut aggregate.events;
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
        drop(aggregate);
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

        let mut aggregate = self.aggregate.write().await;
        let events = &mut aggregate.events;
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
        drop(aggregate);
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
        let aggregate = self.aggregate.read().await;
        let events = &aggregate.events;
        Ok(events
            .iter()
            .filter(|event| event.cursor > after_cursor)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobStoreError> {
        Ok(self.aggregate.read().await.events.clone())
    }

    async fn latest_cursor(&self) -> Result<u64, MobStoreError> {
        #[cfg(any(test, feature = "test-support"))]
        if Self::consume_read_failure(&self.active_reconciliation_latest_cursor_failures) {
            return Err(MobStoreError::Internal(
                "forced mob event store reconciliation latest-cursor failure".to_string(),
            ));
        }
        Ok(self
            .aggregate
            .read()
            .await
            .events
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

        self.aggregate.write().await.events.clear();
        Ok(())
    }

    async fn prune(&self, older_than: DateTime<Utc>) -> Result<u64, MobStoreError> {
        let mut aggregate = self.aggregate.write().await;
        let events = &mut aggregate.events;
        let before = events.len();
        events.retain(|event| {
            identity_structural_projection_is_anchor(&event.kind) || event.timestamp >= older_than
        });
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
    use crate::identity::{
        DesiredMemberMaterial, DesiredMemberOverlay, DesiredSessionTarget,
        IdentityDeclarationManifest, IdentityDeclarationMemberPlan, IdentityMemberDeclaration,
        IdentityMemberMaterialDeclaration, IdentityTargetObservationVersion,
    };
    use crate::ids::{AgentIdentity, Generation, ProfileName};
    use crate::profile::{Profile, ProfileBinding, ToolConfig};
    use crate::run::StepRunStatus;
    use futures::future::join_all;
    use meerkat_contracts::wire::{
        PortableDefinitionExtract, PortableProfile, PortableSystemPrompt,
    };
    use meerkat_core::lifecycle::InputId;
    use meerkat_core::ops::OperationId;
    use meerkat_core::{ContentInput, Provider, SessionId, SessionLineageId};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Debug)]
    struct TestIdentityClock {
        now_ms: AtomicU64,
    }

    impl TestIdentityClock {
        fn new(now_ms: u64) -> Self {
            Self {
                now_ms: AtomicU64::new(now_ms),
            }
        }

        fn set(&self, now_ms: u64) {
            self.now_ms.store(now_ms, Ordering::SeqCst);
        }
    }

    impl MobIdentityStoreClock for TestIdentityClock {
        fn now_ms(&self) -> Result<u64, MobStoreError> {
            Ok(self.now_ms.load(Ordering::SeqCst))
        }
    }

    fn identity_material(model: &str) -> DesiredMemberMaterial {
        DesiredMemberMaterial {
            profile_name: ProfileName::from("default"),
            profile: PortableProfile {
                model: model.to_string(),
                provider: Provider::OpenAI,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
                skills: Vec::new(),
                tools: Default::default(),
                peer_description: String::new(),
                external_addressable: false,
                runtime_mode: Default::default(),
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            },
            definition_extract: PortableDefinitionExtract {
                profile_names: vec!["default".to_string()],
                ..PortableDefinitionExtract::default()
            },
            overlay: DesiredMemberOverlay {
                context: None,
                labels: None,
                additional_instructions: None,
                system_prompt: PortableSystemPrompt::Disable,
                tool_access_policy: None,
                auth_binding: None,
                budget_limits: None,
                runtime_mode: Default::default(),
            },
            required_env_keys: Vec::new(),
            required_local_callback_tools: Vec::new(),
            execution: crate::identity::DesiredExecution::ControllingSession,
        }
    }

    fn identity_declaration_plan(
        scope: &str,
        operation_id: OperationId,
        expected_scope: IdentityDeclarationScopePrecondition,
        members: Vec<(
            AgentIdentity,
            DesiredMemberMaterial,
            Option<ContentInput>,
            Option<DesiredSessionTarget>,
        )>,
        wiring: std::collections::BTreeSet<crate::identity::DesiredIdentityEdge>,
    ) -> IdentityDeclarationApplyPlan {
        let mut declarations = BTreeMap::new();
        let mut compiled = BTreeMap::new();
        for (identity, material, initial_message, candidate) in members {
            let candidate = candidate.unwrap_or_else(|| DesiredSessionTarget {
                session_id: SessionId::new(),
                lineage_id: SessionLineageId::new(format!(
                    "identity-lineage-{}",
                    uuid::Uuid::new_v4()
                ))
                .unwrap(),
                lineage_generation: meerkat_core::SessionGeneration::INITIAL,
                authority_policy: DesiredSessionAuthorityPolicy::CreateIfAbsent,
            });
            declarations.insert(
                identity.clone(),
                IdentityMemberDeclaration {
                    material: IdentityMemberMaterialDeclaration::Resolved {
                        material: material.clone(),
                    },
                    session_authority_policy: candidate.authority_policy,
                    initial_message: initial_message.clone(),
                    legacy_import: None,
                },
            );
            compiled.insert(
                identity,
                IdentityDeclarationMemberPlan {
                    material,
                    session_authority_policy: candidate.authority_policy,
                    initial_message: initial_message.clone(),
                    candidate_session_id: candidate.session_id,
                    candidate_lineage_id: candidate.lineage_id,
                    candidate_initial_delivery_id: initial_message.map(|_| InputId::new()),
                },
            );
        }
        let manifest = IdentityDeclarationManifest {
            scope_id: IdentityDeclarationScopeId::new(scope).unwrap(),
            operation_id,
            expected_scope,
            members: declarations,
            wiring,
        };
        IdentityDeclarationApplyPlan::from_compiled_manifest(&manifest, compiled).unwrap()
    }

    fn valid_initial_delivery_receipt(
        mob_id: &MobId,
        record: &IdentityIntentRecord,
    ) -> IdentityOperationReceipt {
        let IdentityIntent::Present {
            identity,
            session,
            member,
            ..
        } = &record.intent
        else {
            panic!("initial-delivery fixture requires a present intent");
        };
        let delivery = member
            .initial_delivery
            .as_ref()
            .expect("initial delivery fixture");
        let mut receipt = IdentityOperationReceipt {
            schema_version: IDENTITY_OPERATION_RECEIPT_SCHEMA_VERSION,
            mob_id: mob_id.clone(),
            subject: IdentityOperationSubject::Identity {
                identity: identity.clone(),
            },
            effect_kind: IdentityOperationKind::InitialDelivery,
            slot: IdentityOperationSlot::InitialDelivery {
                tombstone_generation: record.tombstone_generation.unwrap_or(0),
                session_id: session.session_id.clone(),
                lineage_id: session.lineage_id.clone(),
                lineage_generation: session.lineage_generation,
                delivery_generation: delivery.delivery_generation,
            },
            receipt_id: OperationId::new(),
            intent_revision: Some(record.intent_revision),
            intent_digest: Some(record.intent_digest.clone()),
            intent_authority_digest: Some(record.authority_digest.clone()),
            tombstone_generation: record.tombstone_generation,
            audit_lease_epoch: None,
            request_digest: String::new(),
            payload: IdentityOperationReceiptPayload::InitialDelivery {
                delivery_generation: delivery.delivery_generation,
                delivery_id: delivery.delivery_id.clone(),
                message_digest: delivery.message_digest.clone(),
            },
        };
        receipt.request_digest = receipt.canonical_request_digest().unwrap();
        receipt.validate().unwrap();
        receipt
    }

    fn receipt_permit(
        mob_id: &MobId,
        record: &IdentityIntentRecord,
        claim: &IdentityLeaseClaim,
    ) -> IdentityActuationPermit {
        IdentityActuationPermit {
            mob_id: mob_id.clone(),
            identity: record.intent.identity().clone(),
            target: IdentityActuatorTarget::InitialDeliveryReceipt,
            intent_revision: record.intent_revision,
            intent_digest: record.intent_digest.clone(),
            intent_authority_digest: record.authority_digest.clone(),
            lease_epoch: claim.epoch,
            lease_holder_id: claim.holder_id.clone(),
            lease_incarnation_id: claim.incarnation_id.clone(),
            lease_expires_at_ms: claim.expires_at_ms,
            target_observation: IdentityTargetObservationVersion::InsertIfAbsent,
        }
    }

    #[tokio::test]
    async fn identity_store_empty_scope_replays_lost_ack_and_preserves_restart_cas() {
        let store = InMemoryMobIdentityStore::new();
        let restarted = store.clone();
        let mob_id = MobId::from("identity-empty-scope");
        let first = identity_declaration_plan(
            "provider-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Missing,
            Vec::new(),
            Default::default(),
        );
        let first_outcome = store
            .apply_identity_declaration(&mob_id, &first)
            .await
            .unwrap();
        assert_eq!(first_outcome.scope_revision, 1);
        assert_eq!(
            restarted
                .apply_identity_declaration(&mob_id, &first)
                .await
                .unwrap(),
            first_outcome,
            "lost-ack replay returns the immutable original outcome"
        );

        let second = identity_declaration_plan(
            "provider-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Revision { revision: 1 },
            Vec::new(),
            Default::default(),
        );
        assert_eq!(
            restarted
                .apply_identity_declaration(&mob_id, &second)
                .await
                .unwrap()
                .scope_revision,
            2
        );

        let contender_a = identity_declaration_plan(
            "provider-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Revision { revision: 2 },
            Vec::new(),
            Default::default(),
        );
        let contender_b = identity_declaration_plan(
            "provider-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Revision { revision: 2 },
            Vec::new(),
            Default::default(),
        );
        let (a, b) = tokio::join!(
            store.apply_identity_declaration(&mob_id, &contender_a),
            restarted.apply_identity_declaration(&mob_id, &contender_b)
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        assert!(matches!(
            a.err().or_else(|| b.err()),
            Some(MobStoreError::CasConflict(_))
        ));
    }

    #[tokio::test]
    async fn identity_store_missing_scope_head_and_counter_exhaustion_are_repair_blocked() {
        let store = InMemoryMobIdentityStore::new();
        let mob_id = MobId::from("identity-scope-corruption");
        let scope_id = IdentityDeclarationScopeId::new("provider-a").unwrap();
        let first = identity_declaration_plan(
            scope_id.as_str(),
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Missing,
            Vec::new(),
            Default::default(),
        );
        store
            .apply_identity_declaration(&mob_id, &first)
            .await
            .unwrap();
        let removed = store
            .aggregate
            .write()
            .await
            .identity
            .scope_heads
            .remove(&(mob_id.clone(), scope_id.clone()))
            .unwrap();
        assert!(matches!(
            store.apply_identity_declaration(&mob_id, &first).await,
            Err(MobStoreError::IdentityAuthorityBlocked { .. })
        ));

        let mut exhausted = removed;
        exhausted.revision = u64::MAX;
        exhausted.authority_digest = exhausted.canonical_authority_digest().unwrap();
        store
            .aggregate
            .write()
            .await
            .identity
            .scope_heads
            .insert((mob_id.clone(), scope_id), exhausted);
        let next = identity_declaration_plan(
            "provider-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Revision { revision: u64::MAX },
            Vec::new(),
            Default::default(),
        );
        assert!(matches!(
            store.apply_identity_declaration(&mob_id, &next).await,
            Err(MobStoreError::IdentityCounterExhausted { .. })
        ));
    }

    #[tokio::test]
    async fn identity_store_lost_ack_replay_rejects_valid_shaped_highwater_regression() {
        let store = InMemoryMobIdentityStore::new();
        let mob_id = MobId::from("identity-replay-regression");
        let identity = AgentIdentity::from("member-a");
        let first = identity_declaration_plan(
            "scope-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Missing,
            vec![(
                identity.clone(),
                identity_material("model-a"),
                Some(ContentInput::from("deliver once")),
                None,
            )],
            Default::default(),
        );
        store
            .apply_identity_declaration(&mob_id, &first)
            .await
            .unwrap();
        let key = (mob_id.clone(), identity.clone());
        let mut aggregate = store.aggregate.write().await;
        let mut regressed = aggregate.identity.intents.get(&key).unwrap().clone();
        let IdentityIntent::Present { member, .. } = &mut regressed.intent else {
            panic!("present fixture");
        };
        member.initial_delivery = None;
        regressed.intent_revision += 1;
        regressed.initial_delivery_generation_highwater = 0;
        let regressed = seal_identity_intent_record(&mob_id, regressed).unwrap();
        aggregate.identity.intents.insert(key, regressed);
        drop(aggregate);

        assert!(matches!(
            store.apply_identity_declaration(&mob_id, &first).await,
            Err(MobStoreError::IdentityAuthorityBlocked { .. })
        ));
    }

    #[tokio::test]
    async fn identity_store_scope_omission_is_local_and_other_scope_cannot_steal() {
        let store = InMemoryMobIdentityStore::new();
        let mob_id = MobId::from("identity-scopes");
        let a1 = AgentIdentity::from("a-1");
        let a2 = AgentIdentity::from("a-2");
        let b1 = AgentIdentity::from("b-1");
        let plan_a = identity_declaration_plan(
            "scope-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Missing,
            vec![
                (a1.clone(), identity_material("model-a1"), None, None),
                (a2.clone(), identity_material("model-a2"), None, None),
            ],
            Default::default(),
        );
        store
            .apply_identity_declaration(&mob_id, &plan_a)
            .await
            .unwrap();
        let plan_b = identity_declaration_plan(
            "scope-b",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Missing,
            vec![(b1.clone(), identity_material("model-b1"), None, None)],
            Default::default(),
        );
        store
            .apply_identity_declaration(&mob_id, &plan_b)
            .await
            .unwrap();

        let omit_a2 = identity_declaration_plan(
            "scope-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Revision { revision: 1 },
            vec![(a1.clone(), identity_material("model-a1"), None, None)],
            Default::default(),
        );
        store
            .apply_identity_declaration(&mob_id, &omit_a2)
            .await
            .unwrap();
        assert!(matches!(
            store.observe_identity_intent(&mob_id, &a2).await.unwrap(),
            IdentityStoredObservation::Valid(IdentityIntentRecord {
                intent: IdentityIntent::Absent { .. },
                ..
            })
        ));
        assert!(matches!(
            store.observe_identity_intent(&mob_id, &b1).await.unwrap(),
            IdentityStoredObservation::Valid(IdentityIntentRecord {
                intent: IdentityIntent::Present { .. },
                ..
            })
        ));

        let steal = identity_declaration_plan(
            "scope-b",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Revision { revision: 1 },
            vec![
                (b1, identity_material("model-b1"), None, None),
                (a1, identity_material("stolen"), None, None),
            ],
            Default::default(),
        );
        assert!(matches!(
            store.apply_identity_declaration(&mob_id, &steal).await,
            Err(MobStoreError::CasConflict(_))
        ));
    }

    #[tokio::test]
    async fn identity_store_reuses_current_target_and_rejects_duplicate_allocation() {
        let store = InMemoryMobIdentityStore::new();
        let mob_id = MobId::from("identity-targets");
        let identity = AgentIdentity::from("member-a");
        let first = identity_declaration_plan(
            "scope-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Missing,
            vec![(identity.clone(), identity_material("model-a"), None, None)],
            Default::default(),
        );
        let first_outcome = store
            .apply_identity_declaration(&mob_id, &first)
            .await
            .unwrap();
        let IdentityIntent::Present { session, .. } = &first_outcome.identities[&identity].intent
        else {
            panic!("present outcome");
        };
        let target = session.clone();

        let update = identity_declaration_plan(
            "scope-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Revision { revision: 1 },
            vec![(identity.clone(), identity_material("model-b"), None, None)],
            Default::default(),
        );
        let update_outcome = store
            .apply_identity_declaration(&mob_id, &update)
            .await
            .unwrap();
        assert!(matches!(
            &update_outcome.identities[&identity].intent,
            IdentityIntent::Present { session, .. } if session == &target
        ));

        let collision = identity_declaration_plan(
            "scope-b",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Missing,
            vec![(
                AgentIdentity::from("member-b"),
                identity_material("model-c"),
                None,
                Some(target),
            )],
            Default::default(),
        );
        assert!(matches!(
            store.apply_identity_declaration(&mob_id, &collision).await,
            Err(MobStoreError::CasConflict(_))
        ));
    }

    #[tokio::test]
    async fn identity_store_classifies_unsupported_malformed_and_key_mismatched_rows() {
        let store = InMemoryMobIdentityStore::new();
        let mob_id = MobId::from("identity-observation-totality");
        let identity = AgentIdentity::from("member-a");
        let plan = identity_declaration_plan(
            "scope-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Missing,
            vec![(identity.clone(), identity_material("model-a"), None, None)],
            Default::default(),
        );
        store
            .apply_identity_declaration(&mob_id, &plan)
            .await
            .unwrap();

        let key = (mob_id.clone(), identity.clone());
        let valid = store.aggregate.read().await.identity.intents[&key].clone();

        let mut unsupported = valid.clone();
        unsupported.schema_version = IDENTITY_INTENT_SCHEMA_VERSION + 1;
        store
            .aggregate
            .write()
            .await
            .identity
            .intents
            .insert(key.clone(), unsupported);
        assert!(matches!(
            store.observe_identity_intent(&mob_id, &identity).await.unwrap(),
            IdentityStoredObservation::Unsupported { evidence_digest, .. }
                if evidence_digest.starts_with("sha256:")
        ));

        let mut malformed = valid.clone();
        malformed.authority_digest = format!("sha256:{}", "0".repeat(64));
        store
            .aggregate
            .write()
            .await
            .identity
            .intents
            .insert(key.clone(), malformed);
        assert!(matches!(
            store.observe_identity_intent(&mob_id, &identity).await.unwrap(),
            IdentityStoredObservation::Malformed { evidence_digest, .. }
                if evidence_digest.starts_with("sha256:")
        ));

        let mismatched_identity = AgentIdentity::from("physical-key-b");
        store
            .aggregate
            .write()
            .await
            .identity
            .intents
            .insert((mob_id.clone(), mismatched_identity.clone()), valid.clone());
        assert!(matches!(
            store
                .observe_identity_intent(&mob_id, &mismatched_identity)
                .await
                .unwrap(),
            IdentityStoredObservation::Malformed { evidence_digest, .. }
                if evidence_digest.starts_with("sha256:")
        ));
        assert!(matches!(
            store.list_identity_intents(&mob_id).await.unwrap().get(&mismatched_identity),
            Some(IdentityStoredObservation::Malformed { evidence_digest, .. })
                if evidence_digest.starts_with("sha256:")
        ));

        // Moving a fully valid donor row under another mob's physical key
        // must remain malformed and cannot authorize that mob even when the
        // identity and all desired content are otherwise identical.
        let recipient_mob = MobId::from("identity-observation-transplant-recipient");
        store
            .aggregate
            .write()
            .await
            .identity
            .intents
            .insert((recipient_mob.clone(), identity.clone()), valid.clone());
        assert!(matches!(
            store
                .observe_identity_intent(&recipient_mob, &identity)
                .await
                .unwrap(),
            IdentityStoredObservation::Malformed { evidence_digest, .. }
                if evidence_digest.starts_with("sha256:")
        ));
        assert!(matches!(
            store
                .list_identity_intents(&recipient_mob)
                .await
                .unwrap()
                .get(&identity),
            Some(IdentityStoredObservation::Malformed { evidence_digest, .. })
                if evidence_digest.starts_with("sha256:")
        ));
        let recipient_claim = match store
            .claim_or_renew_identity_lease(
                &recipient_mob,
                &identity,
                "controller",
                "recipient-incarnation",
                100,
            )
            .await
            .unwrap()
        {
            IdentityLeaseClaimOutcome::Acquired(claim) => claim,
            other => panic!("expected recipient lease, got {other:?}"),
        };
        assert!(matches!(
            store
                .validate_identity_actuation_permit(&receipt_permit(
                    &recipient_mob,
                    &valid,
                    &recipient_claim,
                ))
                .await,
            Err(MobStoreError::CasConflict(_))
        ));
        assert!(!matches!(
            store
                .observe_identity_intent(&mob_id, &identity)
                .await
                .unwrap(),
            IdentityStoredObservation::Missing
        ));
    }

    #[tokio::test]
    async fn identity_store_lease_takeover_strictly_advances_and_stale_release_loses() {
        let clock = Arc::new(TestIdentityClock::new(100));
        let store = InMemoryMobIdentityStore::with_clock(clock.clone());
        let mob_id = MobId::from("identity-lease");
        let identity = AgentIdentity::from("member-a");
        let first = match store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-a", 10)
            .await
            .unwrap()
        {
            IdentityLeaseClaimOutcome::Acquired(claim) => claim,
            other => panic!("expected acquire, got {other:?}"),
        };
        clock.set(105);
        let renewed = match store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-a", 10)
            .await
            .unwrap()
        {
            IdentityLeaseClaimOutcome::Renewed(claim) => claim,
            other => panic!("expected renew, got {other:?}"),
        };
        assert_eq!(renewed.epoch, first.epoch);
        assert!(matches!(
            store
                .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-b", 10)
                .await
                .unwrap(),
            IdentityLeaseClaimOutcome::HeldByOther(_)
        ));
        clock.set(renewed.expires_at_ms);
        let takeover = match store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-b", 10)
            .await
            .unwrap()
        {
            IdentityLeaseClaimOutcome::Acquired(claim) => claim,
            other => panic!("expected takeover, got {other:?}"),
        };
        assert_eq!(takeover.epoch, renewed.epoch + 1);
        assert!(
            !store
                .release_identity_lease(&mob_id, &identity, &renewed)
                .await
                .unwrap()
        );
        assert!(
            store
                .release_identity_lease(&mob_id, &identity, &takeover)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn identity_receipt_insert_rejects_stale_intent_and_stale_lease_but_replay_is_independent()
     {
        let clock = Arc::new(TestIdentityClock::new(100));
        let store = InMemoryMobIdentityStore::with_clock(clock.clone());
        let mob_id = MobId::from("identity-receipt-fence");
        let identity = AgentIdentity::from("member-a");
        let first = identity_declaration_plan(
            "scope-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Missing,
            vec![(
                identity.clone(),
                identity_material("model-a"),
                Some(ContentInput::from("deliver once")),
                None,
            )],
            Default::default(),
        );
        store
            .apply_identity_declaration(&mob_id, &first)
            .await
            .unwrap();
        let first_record = match store
            .observe_identity_intent(&mob_id, &identity)
            .await
            .unwrap()
        {
            IdentityStoredObservation::Valid(record) => record,
            other => panic!("expected intent, got {other:?}"),
        };
        let claim_a = match store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-a", 10)
            .await
            .unwrap()
        {
            IdentityLeaseClaimOutcome::Acquired(claim) => claim,
            other => panic!("expected acquire, got {other:?}"),
        };
        let stale_intent_receipt = valid_initial_delivery_receipt(&mob_id, &first_record);
        let stale_intent_permit = receipt_permit(&mob_id, &first_record, &claim_a);

        let update = identity_declaration_plan(
            "scope-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Revision { revision: 1 },
            vec![(
                identity.clone(),
                identity_material("model-b"),
                Some(ContentInput::from("deliver once")),
                None,
            )],
            Default::default(),
        );
        store
            .apply_identity_declaration(&mob_id, &update)
            .await
            .unwrap();
        assert!(matches!(
            store
                .insert_identity_operation_receipt_if_absent(
                    &stale_intent_receipt,
                    &stale_intent_permit
                )
                .await,
            Err(MobStoreError::CasConflict(_))
        ));

        let current_record = match store
            .observe_identity_intent(&mob_id, &identity)
            .await
            .unwrap()
        {
            IdentityStoredObservation::Valid(record) => record,
            other => panic!("expected intent, got {other:?}"),
        };
        clock.set(claim_a.expires_at_ms);
        let claim_b = match store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-b", 10)
            .await
            .unwrap()
        {
            IdentityLeaseClaimOutcome::Acquired(claim) => claim,
            other => panic!("expected takeover, got {other:?}"),
        };
        let receipt = valid_initial_delivery_receipt(&mob_id, &current_record);
        let stale_lease_permit = receipt_permit(&mob_id, &current_record, &claim_a);
        assert!(matches!(
            store
                .insert_identity_operation_receipt_if_absent(&receipt, &stale_lease_permit)
                .await,
            Err(MobStoreError::CasConflict(_))
        ));
        let current_permit = receipt_permit(&mob_id, &current_record, &claim_b);
        assert!(matches!(
            store
                .insert_identity_operation_receipt_if_absent(&receipt, &current_permit)
                .await
                .unwrap(),
            IdentityOperationReceiptInsertOutcome::Inserted(_)
        ));

        clock.set(claim_b.expires_at_ms);
        let _claim_c = store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-c", 10)
            .await
            .unwrap();
        assert!(matches!(
            store
                .insert_identity_operation_receipt_if_absent(&receipt, &stale_lease_permit)
                .await
                .unwrap(),
            IdentityOperationReceiptInsertOutcome::ExistingExact(_)
        ));
    }

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
