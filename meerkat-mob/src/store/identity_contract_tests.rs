//! Backend-neutral conformance tests for the durable identity authority store.
//!
//! Keep semantic assertions here rather than in an implementation module. New
//! backends should instantiate the macro at the bottom with a fresh harness.
//! Raw-row corruption is an optional harness extension because not every
//! backend exposes physical test hooks; normal contract behavior is required
//! of every backend.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::{
    IdentityMemberEventCommitOutcome, IdentityMemberTargetObservation,
    IdentityWiringEventCommitOutcome, InMemoryMobEventStore, InMemoryMobIdentityStore,
    MobEventStore, MobIdentityMemberStore, MobIdentityStore, MobIdentityStoreClock, MobStoreError,
    SqliteMobIdentityStore, SqliteMobStores,
};
use crate::event::{MemberRef, MemberSpawnedEvent, MobEventKind, NewMobEvent};
use crate::identity::{
    DesiredIdentityEdge, DesiredMemberMaterial, DesiredMemberOverlay,
    DesiredSessionAuthorityPolicy, DesiredSessionTarget, IDENTITY_OPERATION_RECEIPT_SCHEMA_VERSION,
    IdentityActuationPermit, IdentityActuatorTarget, IdentityDeclarationApplyPlan,
    IdentityDeclarationManifest, IdentityDeclarationManifestApplyDisposition,
    IdentityDeclarationMemberPlan, IdentityDeclarationScopeId,
    IdentityDeclarationScopePrecondition, IdentityIntent, IdentityIntentApplyDisposition,
    IdentityIntentRecord, IdentityLeaseClaim, IdentityLeaseClaimOutcome, IdentityLegacyImport,
    IdentityOperationKind, IdentityOperationReceipt, IdentityOperationReceiptInsertOutcome,
    IdentityOperationReceiptPayload, IdentityOperationSlot, IdentityOperationSubject,
    IdentityResourceObservation, IdentityRetirementPlan, IdentityStoredObservation,
    IdentityTargetObservationVersion,
};
use crate::ids::{AgentIdentity, AgentRuntimeId, FenceToken, Generation, MobId, ProfileName};
use meerkat_contracts::wire::{PortableDefinitionExtract, PortableProfile, PortableSystemPrompt};
use meerkat_core::lifecycle::InputId;
use meerkat_core::ops::OperationId;
use meerkat_core::{
    ContentInput, Provider, Session, SessionCheckpointAuthorityBase, SessionCheckpointProvenance,
    SessionCheckpointRevision, SessionCheckpointStamp, SessionGeneration, SessionId,
    SessionLineageId,
};
#[cfg(feature = "runtime-adapter")]
use meerkat_runtime::{
    InMemoryRuntimeStore, MeerkatMachine, RuntimeStore, RuntimeStoreWriteFenceOutcome,
};
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
struct ContractClock {
    now_ms: AtomicU64,
}

impl ContractClock {
    fn new(now_ms: u64) -> Self {
        Self {
            now_ms: AtomicU64::new(now_ms),
        }
    }

    fn set(&self, now_ms: u64) {
        self.now_ms.store(now_ms, Ordering::SeqCst);
    }
}

impl MobIdentityStoreClock for ContractClock {
    fn now_ms(&self) -> Result<u64, MobStoreError> {
        Ok(self.now_ms.load(Ordering::SeqCst))
    }
}

enum ContractStoreSource {
    InMemory(InMemoryMobIdentityStore),
    Sqlite {
        path: PathBuf,
        store_instance_id: String,
    },
}

struct ContractHarness {
    source: ContractStoreSource,
    clock: Arc<ContractClock>,
    _temp_dir: Option<tempfile::TempDir>,
}

impl ContractHarness {
    fn store(&self) -> Arc<dyn MobIdentityStore> {
        match &self.source {
            ContractStoreSource::InMemory(store) => Arc::new(store.clone()),
            ContractStoreSource::Sqlite {
                path,
                store_instance_id,
            } => Arc::new(SqliteMobIdentityStore::with_clock(
                path.clone(),
                store_instance_id.clone(),
                self.clock.clone(),
            )),
        }
    }

    /// A fresh handle over the same durable state, modeling process restart.
    fn reopen(&self) -> Arc<dyn MobIdentityStore> {
        self.store()
    }

    fn set_time(&self, now_ms: u64) {
        self.clock.set(now_ms);
    }

    fn raw_sqlite_path(&self) -> Option<&PathBuf> {
        match &self.source {
            ContractStoreSource::Sqlite { path, .. } => Some(path),
            ContractStoreSource::InMemory(_) => None,
        }
    }

    /// Optional physical corruption hook. Backends with an opaque storage
    /// engine can add an equivalent hook without changing the shared tests.
    fn corrupt_intent_row(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
        corruption: RawIntentCorruption,
    ) -> bool {
        let Some(path) = self.raw_sqlite_path() else {
            return false;
        };
        let conn = Connection::open(path).unwrap();
        let bytes = match corruption {
            RawIntentCorruption::Malformed => b"{".to_vec(),
            RawIntentCorruption::UnsupportedSchema => {
                let bytes: Vec<u8> = conn
                    .query_row(
                        "SELECT CAST(record_json AS BLOB) FROM mob_identity_intents
                         WHERE mob_id = ?1 AND agent_identity = ?2",
                        params![mob_id.as_str(), identity.as_str()],
                        |row| row.get(0),
                    )
                    .unwrap();
                let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                value["schema_version"] = serde_json::Value::from(u64::from(u32::MAX));
                serde_json::to_vec(&value).unwrap()
            }
            RawIntentCorruption::KeyMismatch { source_identity } => conn
                .query_row(
                    "SELECT CAST(record_json AS BLOB) FROM mob_identity_intents
                     WHERE mob_id = ?1 AND agent_identity = ?2",
                    params![mob_id.as_str(), source_identity.as_str()],
                    |row| row.get(0),
                )
                .unwrap(),
        };
        assert_eq!(
            conn.execute(
                "UPDATE mob_identity_intents SET record_json = ?3
                 WHERE mob_id = ?1 AND agent_identity = ?2",
                params![mob_id.as_str(), identity.as_str(), bytes],
            )
            .unwrap(),
            1
        );
        true
    }
}

enum RawIntentCorruption {
    Malformed,
    UnsupportedSchema,
    KeyMismatch { source_identity: AgentIdentity },
}

fn in_memory_harness() -> ContractHarness {
    let clock = Arc::new(ContractClock::new(1_000));
    let store = InMemoryMobIdentityStore::with_clock(clock.clone());
    ContractHarness {
        source: ContractStoreSource::InMemory(store),
        clock,
        _temp_dir: None,
    }
}

fn sqlite_harness() -> ContractHarness {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("identity-contract.db");
    let _stores = SqliteMobStores::open(&path).unwrap();
    let path = std::fs::canonicalize(path).unwrap();
    let store_instance_id: String = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT value FROM mob_identity_store_meta WHERE key = 'store_instance_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    ContractHarness {
        source: ContractStoreSource::Sqlite {
            path,
            store_instance_id,
        },
        clock: Arc::new(ContractClock::new(1_000)),
        _temp_dir: Some(temp_dir),
    }
}

struct MemberCommitHarness {
    identity: Arc<dyn MobIdentityStore>,
    member: Arc<dyn MobIdentityMemberStore>,
    events: Arc<dyn MobEventStore>,
    clock: Arc<ContractClock>,
    reopen_member: Arc<dyn Fn() -> Arc<dyn MobIdentityMemberStore> + Send + Sync>,
    _temp_dir: Option<tempfile::TempDir>,
}

fn in_memory_member_commit_harness() -> MemberCommitHarness {
    let clock = Arc::new(ContractClock::new(1_000));
    let events = Arc::new(InMemoryMobEventStore::new());
    let identity = InMemoryMobIdentityStore::paired_with_event_store(&events, clock.clone());
    let reopen_identity = identity.clone();
    MemberCommitHarness {
        identity: Arc::new(identity.clone()),
        member: Arc::new(identity),
        events,
        clock,
        reopen_member: Arc::new(move || Arc::new(reopen_identity.clone())),
        _temp_dir: None,
    }
}

fn sqlite_member_commit_harness() -> MemberCommitHarness {
    let temp_dir = tempfile::tempdir().unwrap();
    let stores =
        SqliteMobStores::open(temp_dir.path().join("identity-member-contract.db")).unwrap();
    let clock = Arc::new(ContractClock::new(1_000));
    let identity = stores.identity_store_with_clock(clock.clone());
    let path = std::fs::canonicalize(temp_dir.path().join("identity-member-contract.db")).unwrap();
    MemberCommitHarness {
        identity: Arc::new(identity.clone()),
        member: Arc::new(identity),
        events: Arc::new(stores.event_store()),
        clock,
        reopen_member: Arc::new(move || {
            Arc::new(SqliteMobStores::open(&path).unwrap().identity_store())
        }),
        _temp_dir: Some(temp_dir),
    }
}

fn material(model: impl Into<String>) -> DesiredMemberMaterial {
    DesiredMemberMaterial {
        profile_name: ProfileName::from("default"),
        profile: PortableProfile {
            model: model.into(),
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

#[derive(Clone)]
struct MemberFixture {
    identity: AgentIdentity,
    material: DesiredMemberMaterial,
    initial_message: Option<ContentInput>,
    candidate: Option<DesiredSessionTarget>,
    legacy_import: Option<IdentityLegacyImport>,
}

fn member(identity: &str, model: impl Into<String>) -> MemberFixture {
    MemberFixture {
        identity: AgentIdentity::from(identity),
        material: material(model),
        initial_message: None,
        candidate: None,
        legacy_import: None,
    }
}

fn legacy_member(
    identity: &str,
    continuity_epoch_highwater: u64,
    snapshot_fence_audit: u64,
) -> (MemberFixture, SessionCheckpointStamp) {
    let session_id = SessionId::new();
    let legacy_session = Session::with_id(session_id.clone());
    let source_blob = serde_json::to_vec(&legacy_session).unwrap();
    let checkpoint = SessionCheckpointStamp::recovery_migration(
        &legacy_session,
        &source_blob,
        SessionGeneration::INITIAL,
        SessionCheckpointRevision::new(859),
    )
    .unwrap();
    let target = DesiredSessionTarget {
        session_id,
        lineage_id: checkpoint.lineage_id().clone(),
        lineage_generation: checkpoint.generation(),
        authority_policy: DesiredSessionAuthorityPolicy::RequireExisting,
    };
    let legacy_import = IdentityLegacyImport::AdoptVerifiedLegacy {
        session: target.clone(),
        checkpoint: checkpoint.clone(),
        continuity_epoch_highwater,
        snapshot_fence_audit,
    };
    (
        MemberFixture {
            identity: AgentIdentity::from(identity),
            material: material("legacy-model"),
            initial_message: None,
            candidate: Some(target),
            legacy_import: Some(legacy_import),
        },
        checkpoint,
    )
}

fn declaration_plan(
    scope: &str,
    operation_id: OperationId,
    expected_scope: IdentityDeclarationScopePrecondition,
    members: Vec<MemberFixture>,
    wiring: BTreeSet<DesiredIdentityEdge>,
) -> IdentityDeclarationApplyPlan {
    let mut declarations = BTreeMap::new();
    let mut compiled = BTreeMap::new();
    for fixture in members {
        let candidate = fixture.candidate.unwrap_or_else(|| DesiredSessionTarget {
            session_id: SessionId::new(),
            lineage_id: SessionLineageId::new(format!(
                "identity-contract-lineage-{}",
                uuid::Uuid::new_v4()
            ))
            .unwrap(),
            lineage_generation: SessionGeneration::INITIAL,
            authority_policy: DesiredSessionAuthorityPolicy::CreateIfAbsent,
        });
        declarations.insert(
            fixture.identity.clone(),
            crate::identity::IdentityMemberDeclaration {
                material: crate::identity::IdentityMemberMaterialDeclaration::Resolved {
                    material: fixture.material.clone(),
                },
                session_authority_policy: candidate.authority_policy,
                initial_message: fixture.initial_message.clone(),
                legacy_import: fixture.legacy_import.clone(),
            },
        );
        compiled.insert(
            fixture.identity,
            IdentityDeclarationMemberPlan {
                material: fixture.material,
                session_authority_policy: candidate.authority_policy,
                initial_message: fixture.initial_message.clone(),
                candidate_session_id: candidate.session_id,
                candidate_lineage_id: candidate.lineage_id,
                candidate_initial_delivery_id: fixture
                    .initial_message
                    .as_ref()
                    .map(|_| InputId::new()),
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

async fn valid_intent(
    store: &dyn MobIdentityStore,
    mob_id: &MobId,
    identity: &AgentIdentity,
) -> IdentityIntentRecord {
    match store
        .observe_identity_intent(mob_id, identity)
        .await
        .unwrap()
    {
        IdentityStoredObservation::Valid(record) => record,
        other => panic!("expected valid identity intent, observed {other:?}"),
    }
}

fn initial_delivery_receipt(
    mob_id: &MobId,
    record: &IdentityIntentRecord,
    audit_lease_epoch: u64,
) -> IdentityOperationReceipt {
    let IdentityIntent::Present {
        identity,
        session,
        member,
        ..
    } = &record.intent
    else {
        panic!("initial-delivery receipt requires Present intent");
    };
    let delivery = member.initial_delivery.as_ref().unwrap();
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
        audit_lease_epoch: Some(audit_lease_epoch),
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

fn retirement_receipt(
    mob_id: &MobId,
    record: &IdentityIntentRecord,
    audit_lease_epoch: u64,
) -> IdentityOperationReceipt {
    assert!(matches!(record.intent, IdentityIntent::Absent { .. }));
    let tombstone_generation = record.tombstone_generation.unwrap();
    let mut receipt = IdentityOperationReceipt {
        schema_version: IDENTITY_OPERATION_RECEIPT_SCHEMA_VERSION,
        mob_id: mob_id.clone(),
        subject: IdentityOperationSubject::Identity {
            identity: record.intent.identity().clone(),
        },
        effect_kind: IdentityOperationKind::RetirementProven,
        slot: IdentityOperationSlot::RetirementProven {
            tombstone_generation,
        },
        receipt_id: OperationId::new(),
        intent_revision: Some(record.intent_revision),
        intent_digest: Some(record.intent_digest.clone()),
        intent_authority_digest: Some(record.authority_digest.clone()),
        tombstone_generation: Some(tombstone_generation),
        audit_lease_epoch: Some(audit_lease_epoch),
        request_digest: String::new(),
        payload: IdentityOperationReceiptPayload::RetirementProven {
            absent_authority_digest: record.authority_digest.clone(),
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
    target: IdentityActuatorTarget,
) -> IdentityActuationPermit {
    IdentityActuationPermit {
        mob_id: mob_id.clone(),
        identity: record.intent.identity().clone(),
        target,
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

fn acquired(outcome: IdentityLeaseClaimOutcome) -> IdentityLeaseClaim {
    match outcome {
        IdentityLeaseClaimOutcome::Acquired(claim) => claim,
        other => panic!("expected lease acquisition, got {other:?}"),
    }
}

fn member_permit(
    mob_id: &MobId,
    record: &IdentityIntentRecord,
    claim: &IdentityLeaseClaim,
    target_observation: IdentityTargetObservationVersion,
) -> IdentityActuationPermit {
    IdentityActuationPermit {
        mob_id: mob_id.clone(),
        identity: record.intent.identity().clone(),
        target: IdentityActuatorTarget::Member,
        intent_revision: record.intent_revision,
        intent_digest: record.intent_digest.clone(),
        intent_authority_digest: record.authority_digest.clone(),
        lease_epoch: claim.epoch,
        lease_holder_id: claim.holder_id.clone(),
        lease_incarnation_id: claim.incarnation_id.clone(),
        lease_expires_at_ms: claim.expires_at_ms,
        target_observation,
    }
}

fn wiring_permit(
    mob_id: &MobId,
    record: &IdentityIntentRecord,
    claim: &IdentityLeaseClaim,
    target_observation: IdentityTargetObservationVersion,
) -> IdentityActuationPermit {
    IdentityActuationPermit {
        mob_id: mob_id.clone(),
        identity: record.intent.identity().clone(),
        target: IdentityActuatorTarget::Wiring,
        intent_revision: record.intent_revision,
        intent_digest: record.intent_digest.clone(),
        intent_authority_digest: record.authority_digest.clone(),
        lease_epoch: claim.epoch,
        lease_holder_id: claim.holder_id.clone(),
        lease_incarnation_id: claim.incarnation_id.clone(),
        lease_expires_at_ms: claim.expires_at_ms,
        target_observation,
    }
}

#[cfg(feature = "runtime-adapter")]
fn runtime_permit(
    mob_id: &MobId,
    record: &IdentityIntentRecord,
    claim: &IdentityLeaseClaim,
    target_observation: IdentityTargetObservationVersion,
) -> IdentityActuationPermit {
    IdentityActuationPermit {
        mob_id: mob_id.clone(),
        identity: record.intent.identity().clone(),
        target: IdentityActuatorTarget::Runtime,
        intent_revision: record.intent_revision,
        intent_digest: record.intent_digest.clone(),
        intent_authority_digest: record.authority_digest.clone(),
        lease_epoch: claim.epoch,
        lease_holder_id: claim.holder_id.clone(),
        lease_incarnation_id: claim.incarnation_id.clone(),
        lease_expires_at_ms: claim.expires_at_ms,
        target_observation,
    }
}

#[cfg(feature = "runtime-adapter")]
async fn missing_runtime_observation(
    session_id: &SessionId,
) -> meerkat_runtime::RuntimeSessionLifecycleObservation {
    let runtime_store = Arc::new(InMemoryRuntimeStore::new()) as Arc<dyn RuntimeStore>;
    let blob_store = Arc::new(meerkat_store::MemoryBlobStore::new());
    let machine = MeerkatMachine::persistent(runtime_store, blob_store);
    machine
        .observe_cold_runtime_lifecycle(session_id)
        .await
        .unwrap()
}

fn member_spawn_event(
    mob_id: &MobId,
    record: &IdentityIntentRecord,
    claim: &IdentityLeaseClaim,
) -> NewMobEvent {
    member_spawn_event_at_generation(mob_id, record, claim, Generation::INITIAL)
}

fn member_spawn_event_at_generation(
    mob_id: &MobId,
    record: &IdentityIntentRecord,
    claim: &IdentityLeaseClaim,
    generation: Generation,
) -> NewMobEvent {
    let IdentityIntent::Present {
        identity,
        session,
        member,
        ..
    } = &record.intent
    else {
        panic!("MemberSpawned fixture requires Present intent");
    };
    let mut spawned = MemberSpawnedEvent::new(
        identity.clone(),
        generation,
        FenceToken::new(claim.epoch),
        AgentRuntimeId::new(identity.clone(), generation),
        member.material.profile_name.clone(),
    );
    spawned.runtime_mode = match member.material.overlay.runtime_mode {
        meerkat_contracts::wire::WireMobRuntimeMode::AutonomousHost => {
            crate::runtime_mode::MobRuntimeMode::AutonomousHost
        }
        meerkat_contracts::wire::WireMobRuntimeMode::TurnDriven => {
            crate::runtime_mode::MobRuntimeMode::TurnDriven
        }
    };
    spawned.labels = member.material.overlay.labels.clone().unwrap_or_default();
    spawned = spawned
        .with_bridge_member_ref(Some(MemberRef::from_bridge_session_id(
            session.session_id.clone(),
        )))
        .with_identity_intent_authority_digest(Some(record.authority_digest.clone()));
    NewMobEvent {
        mob_id: mob_id.clone(),
        timestamp: None,
        kind: MobEventKind::MemberSpawned(spawned),
    }
}

fn wiring_event(mob_id: &MobId, edge: &DesiredIdentityEdge, adding: bool) -> NewMobEvent {
    NewMobEvent {
        mob_id: mob_id.clone(),
        timestamp: None,
        kind: if adding {
            MobEventKind::MembersWired {
                a: edge.a.clone(),
                b: edge.b.clone(),
            }
        } else {
            MobEventKind::MembersUnwired {
                a: edge.a.clone(),
                b: edge.b.clone(),
            }
        },
    }
}

async fn materialize_member_for_wiring(
    harness: &MemberCommitHarness,
    mob_id: &MobId,
    record: &IdentityIntentRecord,
    claim: &IdentityLeaseClaim,
) {
    let identity = record.intent.identity();
    let observed = harness
        .member
        .observe_identity_member_target(mob_id, identity)
        .await
        .unwrap();
    let permit = member_permit(
        mob_id,
        record,
        claim,
        observed.target_precondition().unwrap(),
    );
    assert!(matches!(
        harness
            .member
            .commit_identity_member_spawned(&permit, &member_spawn_event(mob_id, record, claim))
            .await
            .unwrap(),
        IdentityMemberEventCommitOutcome::Applied { .. }
            | IdentityMemberEventCommitOutcome::AlreadyExact { .. }
    ));
}

async fn contract_empty_scope_restart_and_cas(harness: ContractHarness) {
    let store = harness.store();
    let restarted = harness.reopen();
    let mob_id = MobId::from("contract-empty-scope");
    let first = declaration_plan(
        "scope-a",
        OperationId::new(),
        IdentityDeclarationScopePrecondition::Missing,
        Vec::new(),
        BTreeSet::new(),
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
        first_outcome
    );

    let second = declaration_plan(
        "scope-a",
        OperationId::new(),
        IdentityDeclarationScopePrecondition::Revision { revision: 1 },
        Vec::new(),
        BTreeSet::new(),
    );
    assert_eq!(
        restarted
            .apply_identity_declaration(&mob_id, &second)
            .await
            .unwrap()
            .scope_revision,
        2
    );

    let contender_a = declaration_plan(
        "scope-a",
        OperationId::new(),
        IdentityDeclarationScopePrecondition::Revision { revision: 2 },
        Vec::new(),
        BTreeSet::new(),
    );
    let contender_b = declaration_plan(
        "scope-a",
        OperationId::new(),
        IdentityDeclarationScopePrecondition::Revision { revision: 2 },
        Vec::new(),
        BTreeSet::new(),
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

async fn contract_lost_ack_replays_original_after_later_mutation(harness: ContractHarness) {
    let store = harness.store();
    let mob_id = MobId::from("contract-lost-ack");
    let identity = AgentIdentity::from("member-a");
    let first = declaration_plan(
        "scope-a",
        OperationId::new(),
        IdentityDeclarationScopePrecondition::Missing,
        vec![member("member-a", "model-a")],
        BTreeSet::new(),
    );
    let first_outcome = store
        .apply_identity_declaration(&mob_id, &first)
        .await
        .unwrap();
    let second = declaration_plan(
        "scope-a",
        OperationId::new(),
        IdentityDeclarationScopePrecondition::Revision { revision: 1 },
        vec![member("member-a", "model-b")],
        BTreeSet::new(),
    );
    store
        .apply_identity_declaration(&mob_id, &second)
        .await
        .unwrap();

    let mut compiler_drifted_plan = first.clone();
    compiler_drifted_plan
        .members
        .get_mut(&identity)
        .unwrap()
        .material = material("model-from-new-profile-compiler");
    assert_eq!(
        harness
            .reopen()
            .replay_identity_declaration(
                &mob_id,
                compiler_drifted_plan.scope_id(),
                compiler_drifted_plan.operation_id(),
                compiler_drifted_plan.request_digest(),
            )
            .await
            .unwrap(),
        Some(first_outcome.clone()),
        "receipt replay must happen before compiler/profile drift is consulted"
    );
    assert!(matches!(
        harness
            .reopen()
            .replay_identity_declaration(
                &mob_id,
                first.scope_id(),
                first.operation_id(),
                second.request_digest(),
            )
            .await,
        Err(MobStoreError::CasConflict(_))
    ));
    assert_eq!(
        harness
            .reopen()
            .replay_identity_declaration(
                &mob_id,
                first.scope_id(),
                &OperationId::new(),
                first.request_digest(),
            )
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        harness
            .reopen()
            .apply_identity_declaration(&mob_id, &first)
            .await
            .unwrap(),
        first_outcome,
        "lost-ACK replay must return the immutable original result"
    );
    let current = valid_intent(store.as_ref(), &mob_id, &identity).await;
    assert!(matches!(
        &current.intent,
        IdentityIntent::Present { member, .. } if member.material.profile.model == "model-b"
    ));
    assert_eq!(current.intent_revision, 2);
}

async fn contract_scope_ownership_cannot_be_stolen(harness: ContractHarness) {
    let store = harness.store();
    let mob_id = MobId::from("contract-scope-ownership");
    let alpha = AgentIdentity::from("alpha");
    let beta = AgentIdentity::from("beta");
    store
        .apply_identity_declaration(
            &mob_id,
            &declaration_plan(
                "scope-a",
                OperationId::new(),
                IdentityDeclarationScopePrecondition::Missing,
                vec![member("alpha", "model-a")],
                BTreeSet::new(),
            ),
        )
        .await
        .unwrap();
    store
        .apply_identity_declaration(
            &mob_id,
            &declaration_plan(
                "scope-b",
                OperationId::new(),
                IdentityDeclarationScopePrecondition::Missing,
                vec![member("beta", "model-b")],
                BTreeSet::new(),
            ),
        )
        .await
        .unwrap();

    store
        .apply_identity_declaration(
            &mob_id,
            &declaration_plan(
                "scope-b",
                OperationId::new(),
                IdentityDeclarationScopePrecondition::Revision { revision: 1 },
                Vec::new(),
                BTreeSet::new(),
            ),
        )
        .await
        .unwrap();
    assert!(matches!(
        valid_intent(store.as_ref(), &mob_id, &alpha).await.intent,
        IdentityIntent::Present { .. }
    ));
    assert!(matches!(
        valid_intent(store.as_ref(), &mob_id, &beta).await.intent,
        IdentityIntent::Absent { .. }
    ));

    let steal = declaration_plan(
        "scope-b",
        OperationId::new(),
        IdentityDeclarationScopePrecondition::Revision { revision: 2 },
        vec![member("alpha", "stolen")],
        BTreeSet::new(),
    );
    assert!(matches!(
        store.apply_identity_declaration(&mob_id, &steal).await,
        Err(MobStoreError::CasConflict(_))
    ));
    assert_eq!(
        valid_intent(store.as_ref(), &mob_id, &alpha)
            .await
            .declaration_scope,
        Some(IdentityDeclarationScopeId::new("scope-a").unwrap())
    );
}

async fn contract_unchanged_manifest_preserves_row_revision_coherence(harness: ContractHarness) {
    let store = harness.store();
    let mob_id = MobId::from("contract-unchanged");
    let identity = AgentIdentity::from("member-a");
    let first = declaration_plan(
        "scope-a",
        OperationId::new(),
        IdentityDeclarationScopePrecondition::Missing,
        vec![member("member-a", "model-a")],
        BTreeSet::new(),
    );
    store
        .apply_identity_declaration(&mob_id, &first)
        .await
        .unwrap();
    let first_record = valid_intent(store.as_ref(), &mob_id, &identity).await;

    let second = declaration_plan(
        "scope-a",
        OperationId::new(),
        IdentityDeclarationScopePrecondition::Revision { revision: 1 },
        vec![member("member-a", "model-a")],
        BTreeSet::new(),
    );
    let outcome = store
        .apply_identity_declaration(&mob_id, &second)
        .await
        .unwrap();
    assert_eq!(
        outcome.disposition,
        IdentityDeclarationManifestApplyDisposition::Unchanged
    );
    assert_eq!(outcome.scope_revision, 2);
    assert_eq!(
        outcome.identities[&identity].disposition,
        IdentityIntentApplyDisposition::Unchanged
    );
    let current = valid_intent(store.as_ref(), &mob_id, &identity).await;
    assert_eq!(current, first_record);
    assert_eq!(current.intent_revision, 1);
    assert_eq!(current.declaration_revision, Some(1));
    let scope = match store
        .observe_identity_declaration_scope(
            &mob_id,
            &IdentityDeclarationScopeId::new("scope-a").unwrap(),
        )
        .await
        .unwrap()
    {
        IdentityStoredObservation::Valid(head) => head,
        other => panic!("expected valid scope head, observed {other:?}"),
    };
    assert_eq!(scope.revision, 2);
    assert!(current.declaration_revision.unwrap() <= scope.revision);
}

async fn contract_omission_seals_one_tombstone_and_incident_wiring(harness: ContractHarness) {
    let store = harness.store();
    let mob_id = MobId::from("contract-omission");
    let alpha = AgentIdentity::from("alpha");
    let beta = AgentIdentity::from("beta");
    let edge = DesiredIdentityEdge::new(alpha.clone(), beta.clone()).unwrap();
    store
        .apply_identity_declaration(
            &mob_id,
            &declaration_plan(
                "scope-a",
                OperationId::new(),
                IdentityDeclarationScopePrecondition::Missing,
                vec![member("alpha", "model-a"), member("beta", "model-b")],
                BTreeSet::from([edge.clone()]),
            ),
        )
        .await
        .unwrap();
    store
        .apply_identity_declaration(
            &mob_id,
            &declaration_plan(
                "scope-a",
                OperationId::new(),
                IdentityDeclarationScopePrecondition::Revision { revision: 1 },
                vec![member("alpha", "model-a")],
                BTreeSet::new(),
            ),
        )
        .await
        .unwrap();

    let retired = valid_intent(store.as_ref(), &mob_id, &beta).await;
    assert!(matches!(retired.intent, IdentityIntent::Absent { .. }));
    assert_eq!(retired.tombstone_generation, Some(1));
    assert!(matches!(
        &retired.retirement_plan,
        IdentityRetirementPlan::Targets { incident_wiring, .. }
            if incident_wiring == &BTreeSet::from([edge])
    ));
    let retired_revision = retired.intent_revision;

    let third = declaration_plan(
        "scope-a",
        OperationId::new(),
        IdentityDeclarationScopePrecondition::Revision { revision: 2 },
        vec![member("alpha", "model-a")],
        BTreeSet::new(),
    );
    let third_outcome = store
        .apply_identity_declaration(&mob_id, &third)
        .await
        .unwrap();
    assert_eq!(
        third_outcome.disposition,
        IdentityDeclarationManifestApplyDisposition::Unchanged
    );
    let retired_again = valid_intent(store.as_ref(), &mob_id, &beta).await;
    assert_eq!(retired_again.tombstone_generation, Some(1));
    assert_eq!(retired_again.intent_revision, retired_revision);
    let rows = store.list_identity_intents(&mob_id).await.unwrap();
    assert_eq!(
        rows.values()
            .filter(|row| matches!(
                row,
                IdentityStoredObservation::Valid(IdentityIntentRecord {
                    intent: IdentityIntent::Absent { .. },
                    ..
                })
            ))
            .count(),
        1
    );
}

async fn contract_recreate_requires_exact_retirement_proof(harness: ContractHarness) {
    harness.set_time(1_000);
    let store = harness.store();
    let mob_id = MobId::from("contract-recreate");
    let identity = AgentIdentity::from("member-a");
    let first = declaration_plan(
        "scope-a",
        OperationId::new(),
        IdentityDeclarationScopePrecondition::Missing,
        vec![member("member-a", "model-a")],
        BTreeSet::new(),
    );
    let first_outcome = store
        .apply_identity_declaration(&mob_id, &first)
        .await
        .unwrap();
    let IdentityIntent::Present {
        session: old_target,
        ..
    } = &first_outcome.identities[&identity].intent
    else {
        panic!("expected Present outcome");
    };
    store
        .apply_identity_declaration(
            &mob_id,
            &declaration_plan(
                "scope-a",
                OperationId::new(),
                IdentityDeclarationScopePrecondition::Revision { revision: 1 },
                Vec::new(),
                BTreeSet::new(),
            ),
        )
        .await
        .unwrap();
    let absent = valid_intent(store.as_ref(), &mob_id, &identity).await;

    let recreate = declaration_plan(
        "scope-a",
        OperationId::new(),
        IdentityDeclarationScopePrecondition::Revision { revision: 2 },
        vec![member("member-a", "model-new")],
        BTreeSet::new(),
    );
    assert!(matches!(
        store.apply_identity_declaration(&mob_id, &recreate).await,
        Err(MobStoreError::CasConflict(_))
    ));

    let claim = acquired(
        store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "incarnation-a", 100)
            .await
            .unwrap(),
    );
    let receipt = retirement_receipt(&mob_id, &absent, claim.epoch);
    let permit = receipt_permit(
        &mob_id,
        &absent,
        &claim,
        IdentityActuatorTarget::RetirementReceipt,
    );
    assert!(matches!(
        store
            .insert_identity_operation_receipt_if_absent(&receipt, &permit)
            .await
            .unwrap(),
        IdentityOperationReceiptInsertOutcome::Inserted(_)
    ));

    let recreated = store
        .apply_identity_declaration(&mob_id, &recreate)
        .await
        .unwrap();
    let IdentityIntent::Present {
        session: new_target,
        ..
    } = &recreated.identities[&identity].intent
    else {
        panic!("expected recreated Present outcome");
    };
    assert_ne!(new_target.session_id, old_target.session_id);
    assert_ne!(new_target.lineage_id, old_target.lineage_id);
    assert_eq!(
        recreated.identities[&identity].tombstone_generation,
        Some(1)
    );
}

async fn contract_lease_renew_takeover_release_preserves_highwater(harness: ContractHarness) {
    harness.set_time(1_000);
    let store = harness.store();
    let restarted = harness.reopen();
    let mob_id = MobId::from("contract-lease");
    let identity = AgentIdentity::from("member-a");
    let first = acquired(
        store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-a", 100)
            .await
            .unwrap(),
    );
    harness.set_time(1_040);
    let renewed = match restarted
        .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-a", 100)
        .await
        .unwrap()
    {
        IdentityLeaseClaimOutcome::Renewed(claim) => claim,
        other => panic!("expected renewal, got {other:?}"),
    };
    assert_eq!(renewed.epoch, first.epoch);
    assert!(matches!(
        store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-b", 100)
            .await
            .unwrap(),
        IdentityLeaseClaimOutcome::HeldByOther(_)
    ));

    harness.set_time(renewed.expires_at_ms);
    let takeover = acquired(
        restarted
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-b", 100)
            .await
            .unwrap(),
    );
    assert_eq!(takeover.epoch, renewed.epoch + 1);
    assert!(
        !store
            .release_identity_lease(&mob_id, &identity, &renewed)
            .await
            .unwrap()
    );
    assert!(
        restarted
            .release_identity_lease(&mob_id, &identity, &takeover)
            .await
            .unwrap()
    );
    match store
        .observe_identity_lease(&mob_id, &identity)
        .await
        .unwrap()
    {
        IdentityStoredObservation::Valid(record) => {
            assert_eq!(record.epoch_highwater, takeover.epoch);
            assert!(record.active.is_none());
        }
        other => panic!("expected released lease record, observed {other:?}"),
    }
    let next = acquired(
        store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-c", 100)
            .await
            .unwrap(),
    );
    assert_eq!(next.epoch, takeover.epoch + 1);
}

async fn contract_verified_legacy_import_is_atomic_and_replay_never_resets_lease(
    harness: ContractHarness,
) {
    harness.set_time(1_000);
    let store = harness.store();
    let mob_id = MobId::from("contract-legacy-import");
    let identity = AgentIdentity::from("parent-1");
    let operation_id = OperationId::new();
    let (legacy_fixture, checkpoint) = legacy_member("parent-1", 14_462, 11_130);
    assert_eq!(
        checkpoint.provenance(),
        SessionCheckpointProvenance::RecoveryMigration
    );
    assert!(matches!(
        checkpoint.authority_base(),
        SessionCheckpointAuthorityBase::Legacy {
            observed_generation,
            observed_checkpoint_revision,
            ..
        } if *observed_generation == SessionGeneration::INITIAL
            && *observed_checkpoint_revision == SessionCheckpointRevision::new(859)
    ));
    let plan = declaration_plan(
        "legacy-scope",
        operation_id.clone(),
        IdentityDeclarationScopePrecondition::Missing,
        vec![legacy_fixture.clone()],
        BTreeSet::new(),
    );
    let outcome = store
        .apply_identity_declaration(&mob_id, &plan)
        .await
        .unwrap();
    let record = valid_intent(store.as_ref(), &mob_id, &identity).await;
    let IdentityIntent::Present {
        session, member, ..
    } = &record.intent
    else {
        panic!("legacy adoption must seal Present intent");
    };
    assert_eq!(&session.session_id, checkpoint.session_id());
    assert_eq!(&session.lineage_id, checkpoint.lineage_id());
    assert_eq!(session.lineage_generation, checkpoint.generation());
    assert_eq!(
        session.authority_policy,
        DesiredSessionAuthorityPolicy::RequireExisting
    );
    assert!(member.initial_delivery.is_none());

    match store
        .observe_identity_lease(&mob_id, &identity)
        .await
        .unwrap()
    {
        IdentityStoredObservation::Valid(lease) => {
            assert_eq!(lease.epoch_highwater, 14_462);
            assert!(lease.active.is_none());
        }
        other => panic!("legacy adoption did not seed an inactive lease: {other:?}"),
    }
    let first_claim = acquired(
        store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "new-incarnation", 100)
            .await
            .unwrap(),
    );
    assert_eq!(first_claim.epoch, 14_463);

    assert_eq!(
        harness
            .reopen()
            .apply_identity_declaration(&mob_id, &plan)
            .await
            .unwrap(),
        outcome,
        "lost-ACK replay must return the sealed outcome"
    );
    assert!(matches!(
        store
            .observe_identity_lease(&mob_id, &identity)
            .await
            .unwrap(),
        IdentityStoredObservation::Valid(lease)
            if lease.epoch_highwater == first_claim.epoch
                && lease.active.as_ref() == Some(&first_claim)
    ));

    let mut different_audit = legacy_fixture;
    let Some(IdentityLegacyImport::AdoptVerifiedLegacy {
        snapshot_fence_audit,
        ..
    }) = &mut different_audit.legacy_import
    else {
        panic!("legacy fixture lost its import evidence");
    };
    *snapshot_fence_audit = 11_131;
    let conflicting_reuse = declaration_plan(
        "legacy-scope",
        operation_id,
        IdentityDeclarationScopePrecondition::Missing,
        vec![different_audit],
        BTreeSet::new(),
    );
    assert_ne!(plan.request_digest(), conflicting_reuse.request_digest());
    assert!(matches!(
        store
            .apply_identity_declaration(&mob_id, &conflicting_reuse)
            .await,
        Err(MobStoreError::CasConflict(_))
    ));
    assert!(matches!(
        store
            .observe_identity_lease(&mob_id, &identity)
            .await
            .unwrap(),
        IdentityStoredObservation::Valid(lease)
            if lease.epoch_highwater == first_claim.epoch
                && lease.active.as_ref() == Some(&first_claim)
    ));
}

async fn contract_legacy_import_rejects_existing_authority_and_target_collisions(
    harness: ContractHarness,
) {
    harness.set_time(1_000);
    let store = harness.store();

    let lease_mob = MobId::from("contract-legacy-existing-lease");
    let lease_identity = AgentIdentity::from("parent-with-lease");
    let preexisting_claim = acquired(
        store
            .claim_or_renew_identity_lease(
                &lease_mob,
                &lease_identity,
                "old-controller",
                "old-incarnation",
                100,
            )
            .await
            .unwrap(),
    );
    let (mut lease_fixture, _) = legacy_member("parent-with-lease", 14_462, 11_130);
    lease_fixture.identity = lease_identity.clone();
    let lease_collision = declaration_plan(
        "legacy-scope",
        OperationId::new(),
        IdentityDeclarationScopePrecondition::Missing,
        vec![lease_fixture],
        BTreeSet::new(),
    );
    assert!(matches!(
        store
            .apply_identity_declaration(&lease_mob, &lease_collision)
            .await,
        Err(MobStoreError::CasConflict(_))
    ));
    assert!(matches!(
        store
            .observe_identity_intent(&lease_mob, &lease_identity)
            .await
            .unwrap(),
        IdentityStoredObservation::Missing
    ));
    assert!(matches!(
        store
            .observe_identity_declaration_scope(
                &lease_mob,
                &IdentityDeclarationScopeId::new("legacy-scope").unwrap(),
            )
            .await
            .unwrap(),
        IdentityStoredObservation::Missing
    ));
    assert!(matches!(
        store
            .observe_identity_lease(&lease_mob, &lease_identity)
            .await
            .unwrap(),
        IdentityStoredObservation::Valid(lease)
            if lease.active.as_ref() == Some(&preexisting_claim)
    ));

    let target_mob = MobId::from("contract-legacy-target-collision");
    let (first_fixture, _) = legacy_member("parent-a", 10, 7);
    let mut second_fixture = first_fixture.clone();
    second_fixture.identity = AgentIdentity::from("parent-b");
    store
        .apply_identity_declaration(
            &target_mob,
            &declaration_plan(
                "scope-a",
                OperationId::new(),
                IdentityDeclarationScopePrecondition::Missing,
                vec![first_fixture],
                BTreeSet::new(),
            ),
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .apply_identity_declaration(
                &target_mob,
                &declaration_plan(
                    "scope-b",
                    OperationId::new(),
                    IdentityDeclarationScopePrecondition::Missing,
                    vec![second_fixture],
                    BTreeSet::new(),
                ),
            )
            .await,
        Err(MobStoreError::CasConflict(_))
    ));
    let second_identity = AgentIdentity::from("parent-b");
    assert!(matches!(
        store
            .observe_identity_intent(&target_mob, &second_identity)
            .await
            .unwrap(),
        IdentityStoredObservation::Missing
    ));
    assert!(matches!(
        store
            .observe_identity_lease(&target_mob, &second_identity)
            .await
            .unwrap(),
        IdentityStoredObservation::Missing
    ));
}

async fn contract_receipt_insert_is_fenced_but_replay_survives_takeover(harness: ContractHarness) {
    harness.set_time(1_000);
    let store = harness.store();
    let mob_id = MobId::from("contract-receipt");
    let identity = AgentIdentity::from("member-a");
    let mut first_member = member("member-a", "model-a");
    first_member.initial_message = Some(ContentInput::from("deliver exactly once"));
    store
        .apply_identity_declaration(
            &mob_id,
            &declaration_plan(
                "scope-a",
                OperationId::new(),
                IdentityDeclarationScopePrecondition::Missing,
                vec![first_member],
                BTreeSet::new(),
            ),
        )
        .await
        .unwrap();
    let first_record = valid_intent(store.as_ref(), &mob_id, &identity).await;
    let claim_a = acquired(
        store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-a", 100)
            .await
            .unwrap(),
    );
    let stale_intent_receipt = initial_delivery_receipt(&mob_id, &first_record, claim_a.epoch);
    let stale_intent_permit = receipt_permit(
        &mob_id,
        &first_record,
        &claim_a,
        IdentityActuatorTarget::InitialDeliveryReceipt,
    );

    let mut updated_member = member("member-a", "model-b");
    updated_member.initial_message = Some(ContentInput::from("deliver exactly once"));
    store
        .apply_identity_declaration(
            &mob_id,
            &declaration_plan(
                "scope-a",
                OperationId::new(),
                IdentityDeclarationScopePrecondition::Revision { revision: 1 },
                vec![updated_member],
                BTreeSet::new(),
            ),
        )
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

    let current = valid_intent(store.as_ref(), &mob_id, &identity).await;
    harness.set_time(claim_a.expires_at_ms);
    let claim_b = acquired(
        store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-b", 100)
            .await
            .unwrap(),
    );
    let receipt = initial_delivery_receipt(&mob_id, &current, claim_b.epoch);
    let stale_lease_permit = receipt_permit(
        &mob_id,
        &current,
        &claim_a,
        IdentityActuatorTarget::InitialDeliveryReceipt,
    );
    assert!(matches!(
        store
            .insert_identity_operation_receipt_if_absent(&receipt, &stale_lease_permit)
            .await,
        Err(MobStoreError::CasConflict(_))
    ));
    let pre_renew_permit = receipt_permit(
        &mob_id,
        &current,
        &claim_b,
        IdentityActuatorTarget::InitialDeliveryReceipt,
    );
    harness.set_time(claim_b.renewed_at_ms + 1);
    let renewed_b = match store
        .claim_or_renew_identity_lease(
            &mob_id,
            &identity,
            &claim_b.holder_id,
            &claim_b.incarnation_id,
            200,
        )
        .await
        .unwrap()
    {
        IdentityLeaseClaimOutcome::Renewed(claim) => claim,
        other => panic!("expected same-incarnation lease renewal, got {other:?}"),
    };
    assert_eq!(renewed_b.epoch, claim_b.epoch);
    assert_ne!(renewed_b.expires_at_ms, claim_b.expires_at_ms);
    assert!(matches!(
        store
            .insert_identity_operation_receipt_if_absent(&receipt, &pre_renew_permit)
            .await,
        Err(MobStoreError::CasConflict(_))
    ));
    let current_permit = receipt_permit(
        &mob_id,
        &current,
        &renewed_b,
        IdentityActuatorTarget::InitialDeliveryReceipt,
    );
    let mut wrong_target_receipt = receipt.clone();
    let IdentityOperationSlot::InitialDelivery { session_id, .. } = &mut wrong_target_receipt.slot
    else {
        panic!("initial delivery fixture has the wrong slot");
    };
    *session_id = SessionId::new();
    wrong_target_receipt.request_digest = wrong_target_receipt.canonical_request_digest().unwrap();
    wrong_target_receipt.validate().unwrap();
    assert!(matches!(
        store
            .insert_identity_operation_receipt_if_absent(&wrong_target_receipt, &current_permit,)
            .await,
        Err(MobStoreError::CasConflict(_))
    ));
    let inserted = match store
        .insert_identity_operation_receipt_if_absent(&receipt, &current_permit)
        .await
        .unwrap()
    {
        IdentityOperationReceiptInsertOutcome::Inserted(receipt) => receipt,
        other => panic!("expected receipt insert, got {other:?}"),
    };

    harness.set_time(renewed_b.expires_at_ms);
    let claim_c = acquired(
        harness
            .reopen()
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-c", 100)
            .await
            .unwrap(),
    );
    assert!(claim_c.epoch > claim_b.epoch);
    assert!(matches!(
        harness
            .reopen()
            .insert_identity_operation_receipt_if_absent(&receipt, &stale_lease_permit)
            .await
            .unwrap(),
        IdentityOperationReceiptInsertOutcome::ExistingExact(existing) if existing == inserted
    ));
    let subject = IdentityOperationSubject::Identity {
        identity: identity.clone(),
    };
    assert!(matches!(
        harness
            .reopen()
            .observe_identity_operation_receipt(&mob_id, &subject, &receipt.slot)
            .await
            .unwrap(),
        IdentityStoredObservation::Valid(existing) if existing == inserted
    ));
}

async fn contract_deterministic_operation_sequences(harness: ContractHarness) {
    let store = harness.store();
    for seed in 0_u64..12 {
        let mob_id = MobId::from(format!("contract-sequence-{seed}").as_str());
        let identities = [
            AgentIdentity::from("alpha"),
            AgentIdentity::from("beta"),
            AgentIdentity::from("gamma"),
        ];
        let mut state = seed.wrapping_add(1);
        let mut live_mask = 0b111_u8;
        let mut prior: Option<(
            IdentityDeclarationApplyPlan,
            crate::identity::IdentityDeclarationManifestApplyOutcome,
        )> = None;
        for step in 0_u64..10 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            if step > 0 && state.trailing_zeros() >= 2 {
                let candidate = ((state >> 8) % 3) as u8;
                live_mask &= !(1 << candidate);
            }
            let mut members = Vec::new();
            for (index, identity) in identities.iter().enumerate() {
                if live_mask & (1 << index) != 0 {
                    members.push(member(
                        identity.as_str(),
                        format!("model-{}-{}", (state >> (index * 7)) & 3, step % 3),
                    ));
                }
            }
            let wiring = if live_mask & 0b011 == 0b011 && state & 1 == 0 {
                BTreeSet::from([DesiredIdentityEdge::new(
                    identities[0].clone(),
                    identities[1].clone(),
                )
                .unwrap()])
            } else {
                BTreeSet::new()
            };
            let plan = declaration_plan(
                "scope-sequence",
                OperationId::new(),
                if step == 0 {
                    IdentityDeclarationScopePrecondition::Missing
                } else {
                    IdentityDeclarationScopePrecondition::Revision { revision: step }
                },
                members,
                wiring,
            );
            let outcome = store
                .apply_identity_declaration(&mob_id, &plan)
                .await
                .unwrap();
            outcome.validate().unwrap();
            assert_eq!(outcome.scope_revision, step + 1);

            let head = match store
                .observe_identity_declaration_scope(
                    &mob_id,
                    &IdentityDeclarationScopeId::new("scope-sequence").unwrap(),
                )
                .await
                .unwrap()
            {
                IdentityStoredObservation::Valid(head) => head,
                other => panic!("expected valid sequence head, got {other:?}"),
            };
            assert_eq!(head.revision, step + 1);
            assert_eq!(
                head.declared_member_count,
                u64::from(live_mask.count_ones())
            );
            for (identity, observation) in store.list_identity_intents(&mob_id).await.unwrap() {
                let IdentityStoredObservation::Valid(record) = observation else {
                    panic!("operation sequence lost total row classification for {identity}");
                };
                record.validate().unwrap();
                assert_eq!(record.intent.identity(), &identity);
                assert_eq!(
                    record.declaration_scope,
                    Some(IdentityDeclarationScopeId::new("scope-sequence").unwrap())
                );
                assert!(record.declaration_revision.unwrap() <= head.revision);
            }

            if step % 3 == 2
                && let Some((prior_plan, prior_outcome)) = &prior
            {
                assert_eq!(
                    harness
                        .reopen()
                        .apply_identity_declaration(&mob_id, prior_plan)
                        .await
                        .unwrap(),
                    *prior_outcome
                );
            }
            prior = Some((plan, outcome));
        }
    }
}

async fn sqlite_raw_rows_are_total_observations(corruption: RawIntentCorruption) {
    let harness = sqlite_harness();
    let store = harness.store();
    let mob_id = MobId::from("contract-raw-observation");
    let alpha = AgentIdentity::from("alpha");
    let beta = AgentIdentity::from("beta");
    store
        .apply_identity_declaration(
            &mob_id,
            &declaration_plan(
                "scope-a",
                OperationId::new(),
                IdentityDeclarationScopePrecondition::Missing,
                vec![member("alpha", "model-a"), member("beta", "model-b")],
                BTreeSet::new(),
            ),
        )
        .await
        .unwrap();
    let expect_unsupported = matches!(&corruption, RawIntentCorruption::UnsupportedSchema);
    assert!(harness.corrupt_intent_row(&mob_id, &alpha, corruption));
    let observation = store
        .observe_identity_intent(&mob_id, &alpha)
        .await
        .unwrap();
    observation.validate_evidence().unwrap();
    match observation {
        IdentityStoredObservation::Unsupported { .. } if expect_unsupported => {}
        IdentityStoredObservation::Malformed { .. } if !expect_unsupported => {}
        other => panic!("corrupt physical row was collapsed to {other:?}"),
    }
    let listed = store.list_identity_intents(&mob_id).await.unwrap();
    assert!(matches!(
        listed.get(&alpha),
        Some(
            IdentityStoredObservation::Unsupported { .. }
                | IdentityStoredObservation::Malformed { .. }
        )
    ));
    assert!(matches!(
        listed.get(&beta),
        Some(IdentityStoredObservation::Valid(_))
    ));
}

async fn contract_member_spawn_is_one_atomic_target_local_cas(harness: MemberCommitHarness) {
    let mob_id = MobId::from("contract-member-cas");
    let identity = AgentIdentity::from("member-a");
    let first = declaration_plan(
        "scope-a",
        OperationId::new(),
        IdentityDeclarationScopePrecondition::Missing,
        vec![member("member-a", "model-a")],
        BTreeSet::new(),
    );
    harness
        .identity
        .apply_identity_declaration(&mob_id, &first)
        .await
        .unwrap();
    let first_record = valid_intent(harness.identity.as_ref(), &mob_id, &identity).await;
    let first_claim = acquired(
        harness
            .identity
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "incarnation-a", 100)
            .await
            .unwrap(),
    );
    let absent = harness
        .member
        .observe_identity_member_target(&mob_id, &identity)
        .await
        .unwrap();
    assert!(matches!(
        absent.resource_observation_against(&first_record),
        IdentityResourceObservation::Missing { .. }
    ));
    let first_permit = member_permit(
        &mob_id,
        &first_record,
        &first_claim,
        absent.target_precondition().unwrap(),
    );
    let first_event = member_spawn_event(&mob_id, &first_record, &first_claim);

    let mut stale_incarnation = first_permit.clone();
    stale_incarnation.lease_incarnation_id = "incarnation-stale".to_string();
    assert!(matches!(
        harness
            .member
            .commit_identity_member_spawned(&stale_incarnation, &first_event)
            .await
            .unwrap(),
        IdentityMemberEventCommitOutcome::Conflict { .. }
    ));

    let mut stale_target = first_permit.clone();
    stale_target.target_observation = IdentityTargetObservationVersion::Absent {
        absence_version: "sha256:stale-target".to_string(),
    };
    assert!(matches!(
        harness
            .member
            .commit_identity_member_spawned(&stale_target, &first_event)
            .await
            .unwrap(),
        IdentityMemberEventCommitOutcome::Conflict { .. }
    ));

    harness
        .identity
        .apply_identity_declaration(
            &mob_id,
            &declaration_plan(
                "scope-a",
                OperationId::new(),
                IdentityDeclarationScopePrecondition::Revision { revision: 1 },
                vec![member("member-a", "model-b")],
                BTreeSet::new(),
            ),
        )
        .await
        .unwrap();

    assert!(matches!(
        harness
            .member
            .commit_identity_member_spawned(&first_permit, &first_event)
            .await
            .unwrap(),
        IdentityMemberEventCommitOutcome::Conflict { .. }
    ));

    let current_record = valid_intent(harness.identity.as_ref(), &mob_id, &identity).await;
    let current_absent = harness
        .member
        .observe_identity_member_target(&mob_id, &identity)
        .await
        .unwrap();
    let current_permit = member_permit(
        &mob_id,
        &current_record,
        &first_claim,
        current_absent.target_precondition().unwrap(),
    );
    let current_event = member_spawn_event(&mob_id, &current_record, &first_claim);
    let mut wrong_material = current_event.clone();
    let MobEventKind::MemberSpawned(spawned) = &mut wrong_material.kind else {
        unreachable!();
    };
    spawned
        .labels
        .insert("wrong".to_string(), "material".to_string());
    assert!(matches!(
        harness
            .member
            .commit_identity_member_spawned(&current_permit, &wrong_material)
            .await
            .unwrap(),
        IdentityMemberEventCommitOutcome::RepairBlocked { .. }
    ));

    harness.clock.set(first_claim.expires_at_ms);
    assert!(matches!(
        harness
            .member
            .commit_identity_member_spawned(&current_permit, &current_event)
            .await
            .unwrap(),
        IdentityMemberEventCommitOutcome::Conflict { .. }
    ));
    let takeover = acquired(
        harness
            .identity
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "incarnation-b", 100)
            .await
            .unwrap(),
    );
    assert!(takeover.epoch > first_claim.epoch);
    let takeover_target = harness
        .member
        .observe_identity_member_target(&mob_id, &identity)
        .await
        .unwrap();
    let takeover_permit = member_permit(
        &mob_id,
        &current_record,
        &takeover,
        takeover_target.target_precondition().unwrap(),
    );
    let takeover_event = member_spawn_event(&mob_id, &current_record, &takeover);
    assert!(matches!(
        harness
            .member
            .commit_identity_member_spawned(&takeover_permit, &takeover_event)
            .await
            .unwrap(),
        IdentityMemberEventCommitOutcome::Applied { .. }
    ));

    let present = harness
        .member
        .observe_identity_member_target(&mob_id, &identity)
        .await
        .unwrap();
    assert!(matches!(
        present.resource_observation_against(&current_record),
        IdentityResourceObservation::Matching { .. }
    ));
    let IdentityMemberTargetObservation::Present {
        version,
        mut evidence,
    } = present.clone()
    else {
        panic!("applied MemberSpawned did not become present");
    };
    evidence.intent_authority_digest = None;
    assert!(matches!(
        IdentityMemberTargetObservation::Present { version, evidence }
            .resource_observation_against(&current_record),
        IdentityResourceObservation::Divergent { .. }
    ));

    // A lost ACK may retry its stale Absent permit, but target-local CAS must
    // force re-observation before exact replay is acknowledged.
    assert!(matches!(
        harness
            .member
            .commit_identity_member_spawned(&takeover_permit, &takeover_event)
            .await
            .unwrap(),
        IdentityMemberEventCommitOutcome::Conflict { .. }
    ));
    let exact_permit = member_permit(
        &mob_id,
        &current_record,
        &takeover,
        present.target_precondition().unwrap(),
    );
    assert!(matches!(
        harness
            .member
            .commit_identity_member_spawned(&exact_permit, &takeover_event)
            .await
            .unwrap(),
        IdentityMemberEventCommitOutcome::AlreadyExact { .. }
    ));
    assert_eq!(
        harness
            .events
            .replay_all()
            .await
            .unwrap()
            .into_iter()
            .filter(|event| matches!(event.kind, MobEventKind::MemberSpawned(_)))
            .count(),
        1
    );

    let IdentityIntent::Present {
        session, member, ..
    } = &current_record.intent
    else {
        unreachable!();
    };
    harness
        .events
        .append(NewMobEvent {
            mob_id: mob_id.clone(),
            timestamp: None,
            kind: MobEventKind::MemberRetirementStarted {
                agent_identity: identity.clone(),
                agent_runtime_id: AgentRuntimeId::initial(identity.clone()),
                generation: Generation::INITIAL,
                role: member.material.profile_name.clone(),
                releasing: Some(session.session_id.clone()),
                session_id: Some(session.session_id.clone()),
                retiring_peer_endpoint: None,
                preserve_machine_topology: false,
            },
        })
        .await
        .unwrap();
    let retiring = harness
        .member
        .observe_identity_member_target(&mob_id, &identity)
        .await
        .unwrap();
    assert!(matches!(
        retiring,
        IdentityMemberTargetObservation::Recovering { .. }
    ));
    assert!(matches!(
        retiring.resource_observation_against(&current_record),
        IdentityResourceObservation::Unavailable { .. }
    ));
    assert!(retiring.target_precondition().is_none());

    harness.clock.set(takeover.expires_at_ms);
    let successor = acquired(
        harness
            .identity
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "incarnation-c", 100)
            .await
            .unwrap(),
    );
    assert!(successor.epoch > takeover.epoch);
    assert!(matches!(
        harness
            .member
            .commit_identity_member_spawned(&exact_permit, &takeover_event)
            .await
            .unwrap(),
        IdentityMemberEventCommitOutcome::Conflict { .. }
    ));

    harness
        .events
        .append(NewMobEvent {
            mob_id: mob_id.clone(),
            timestamp: None,
            kind: MobEventKind::MemberRetired {
                agent_identity: identity.clone(),
                generation: Generation::INITIAL,
                role: member.material.profile_name.clone(),
            },
        })
        .await
        .unwrap();
    let retired = harness
        .member
        .observe_identity_member_target(&mob_id, &identity)
        .await
        .unwrap();
    assert!(matches!(
        retired.resource_observation_against(&current_record),
        IdentityResourceObservation::Missing { .. }
    ));
    let successor_generation = Generation::INITIAL.next().unwrap();
    let rematerialize_permit = member_permit(
        &mob_id,
        &current_record,
        &successor,
        retired.target_precondition().unwrap(),
    );
    let rematerialize_event = member_spawn_event_at_generation(
        &mob_id,
        &current_record,
        &successor,
        successor_generation,
    );
    assert!(matches!(
        harness
            .member
            .commit_identity_member_spawned(&rematerialize_permit, &rematerialize_event)
            .await
            .unwrap(),
        IdentityMemberEventCommitOutcome::Applied { .. }
    ));
    let rematerialized = harness
        .member
        .observe_identity_member_target(&mob_id, &identity)
        .await
        .unwrap();
    assert!(matches!(
        rematerialized.resource_observation_against(&current_record),
        IdentityResourceObservation::Matching { .. }
    ));
    for kind in [
        MobEventKind::RemoteMemberRuntimeRetired {
            agent_identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::initial(identity.clone()),
            fence_token: FenceToken::new(takeover.epoch),
            generation: Generation::INITIAL,
        },
        MobEventKind::RemoteMemberSupervisorRevoked {
            agent_identity: identity.clone(),
            agent_runtime_id: AgentRuntimeId::initial(identity.clone()),
            fence_token: FenceToken::new(takeover.epoch),
            generation: Generation::INITIAL,
        },
    ] {
        harness
            .events
            .append(NewMobEvent {
                mob_id: mob_id.clone(),
                timestamp: None,
                kind,
            })
            .await
            .unwrap();
    }
    assert_eq!(
        harness
            .member
            .observe_identity_member_target(&mob_id, &identity)
            .await
            .unwrap(),
        rematerialized,
    );
}

async fn contract_wiring_is_one_atomic_identity_local_cas(harness: MemberCommitHarness) {
    let mob_id = MobId::from("contract-wiring-cas");
    let alpha = AgentIdentity::from("alpha");
    let beta = AgentIdentity::from("beta");
    let gamma = AgentIdentity::from("gamma");
    let delta = AgentIdentity::from("delta");
    let alpha_beta = DesiredIdentityEdge::new(alpha.clone(), beta.clone()).unwrap();
    let delta_gamma = DesiredIdentityEdge::new(delta.clone(), gamma.clone()).unwrap();

    harness
        .identity
        .apply_identity_declaration(
            &mob_id,
            &declaration_plan(
                "scope-wiring",
                OperationId::new(),
                IdentityDeclarationScopePrecondition::Missing,
                vec![
                    member("alpha", "model-a"),
                    member("beta", "model-b"),
                    member("gamma", "model-g"),
                    member("delta", "model-d"),
                ],
                BTreeSet::from([alpha_beta.clone(), delta_gamma.clone()]),
            ),
        )
        .await
        .unwrap();

    // Simulate a torn legacy ordering: an edge projection was durably
    // appended before either endpoint's member realization. Wiring truth is
    // reduced from wiring events themselves, never inferred from roster
    // presence, so the edge remains exactly observable after restart.
    harness
        .events
        .append(wiring_event(&mob_id, &delta_gamma, true))
        .await
        .unwrap();

    let mut records = BTreeMap::new();
    let mut claims = BTreeMap::new();
    for identity in [&alpha, &beta, &gamma, &delta] {
        let record = valid_intent(harness.identity.as_ref(), &mob_id, identity).await;
        let claim = acquired(
            harness
                .identity
                .claim_or_renew_identity_lease(
                    &mob_id,
                    identity,
                    "controller",
                    "incarnation-a",
                    100,
                )
                .await
                .unwrap(),
        );
        materialize_member_for_wiring(&harness, &mob_id, &record, &claim).await;
        records.insert(identity.clone(), record);
        claims.insert(identity.clone(), claim);
    }

    let alpha_record = records.get(&alpha).unwrap();
    let alpha_claim = claims.get(&alpha).unwrap();
    let alpha_absent = harness
        .member
        .observe_identity_wiring_target(&mob_id, &alpha)
        .await
        .unwrap();
    assert!(!alpha_absent.contains(&alpha_beta));
    let alpha_permit = wiring_permit(
        &mob_id,
        alpha_record,
        alpha_claim,
        alpha_absent.target_precondition().unwrap(),
    );
    let alpha_event = wiring_event(&mob_id, &alpha_beta, true);

    // The edge has one semantic owner. The non-owning endpoint's derived
    // incident cleanup evidence cannot authorize desired edge creation.
    let beta_record = records.get(&beta).unwrap();
    let beta_claim = claims.get(&beta).unwrap();
    let beta_absent = harness
        .member
        .observe_identity_wiring_target(&mob_id, &beta)
        .await
        .unwrap();
    let beta_add = wiring_permit(
        &mob_id,
        beta_record,
        beta_claim,
        beta_absent.target_precondition().unwrap(),
    );
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&beta_add, &alpha_event)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::RepairBlocked { .. }
    ));

    let mut stale_incarnation = alpha_permit.clone();
    stale_incarnation.lease_incarnation_id = "incarnation-stale".to_string();
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&stale_incarnation, &alpha_event)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::Conflict { .. }
    ));
    let mut stale_target = alpha_permit.clone();
    stale_target.target_observation = IdentityTargetObservationVersion::Absent {
        absence_version: "sha256:stale-wiring-target".to_string(),
    };
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&stale_target, &alpha_event)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::Conflict { .. }
    ));
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(
                &alpha_permit,
                &wiring_event(&mob_id, &alpha_beta, false),
            )
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::AlreadyExact { .. }
    ));

    // A different identity-local edge may change the global event cursor but
    // must not invalidate alpha's content-addressed target witness.
    let delta_record = records.get(&delta).unwrap();
    let delta_claim = claims.get(&delta).unwrap();
    let delta_observed = harness
        .member
        .observe_identity_wiring_target(&mob_id, &delta)
        .await
        .unwrap();
    let delta_permit = wiring_permit(
        &mob_id,
        delta_record,
        delta_claim,
        delta_observed.target_precondition().unwrap(),
    );
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(
                &delta_permit,
                &wiring_event(&mob_id, &delta_gamma, true),
            )
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::AlreadyExact { .. }
    ));
    assert_eq!(
        harness
            .member
            .observe_identity_wiring_target(&mob_id, &alpha)
            .await
            .unwrap()
            .target_precondition(),
        alpha_absent.target_precondition(),
    );
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&alpha_permit, &alpha_event)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::Applied { .. }
    ));

    let alpha_present = harness
        .member
        .observe_identity_wiring_target(&mob_id, &alpha)
        .await
        .unwrap();
    let beta_present = harness
        .member
        .observe_identity_wiring_target(&mob_id, &beta)
        .await
        .unwrap();
    assert!(alpha_present.contains(&alpha_beta));
    assert!(beta_present.contains(&alpha_beta));
    assert_eq!(
        alpha_present.incident_edges(),
        beta_present.incident_edges()
    );

    // A fresh generated cleanup decision may drain a still-desired Present
    // edge when another realization (for example the session) is unsafe. An
    // addition remains owner-only, but either incident endpoint may perform
    // this resource-local safety removal under its own exact intent/lease and
    // target CAS. A later fresh Ensure pass can restore the desired edge.
    let desired_remove = wiring_permit(
        &mob_id,
        beta_record,
        beta_claim,
        beta_present.target_precondition().unwrap(),
    );
    let desired_remove_event = wiring_event(&mob_id, &alpha_beta, false);
    let mut stale_desired_remove_intent = desired_remove.clone();
    stale_desired_remove_intent.intent_digest = format!("sha256:{}", "f".repeat(64));
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&stale_desired_remove_intent, &desired_remove_event,)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::Conflict { .. }
    ));
    let mut stale_desired_remove_lease = desired_remove.clone();
    stale_desired_remove_lease.lease_incarnation_id = "incarnation-stale".to_string();
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&stale_desired_remove_lease, &desired_remove_event,)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::Conflict { .. }
    ));
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&desired_remove, &desired_remove_event)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::Applied { .. }
    ));
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&desired_remove, &desired_remove_event)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::Conflict { .. }
    ));
    let desired_removed = harness
        .member
        .observe_identity_wiring_target(&mob_id, &alpha)
        .await
        .unwrap();
    let desired_readd = wiring_permit(
        &mob_id,
        alpha_record,
        alpha_claim,
        desired_removed.target_precondition().unwrap(),
    );
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&desired_readd, &alpha_event)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::Applied { .. }
    ));
    assert_eq!(
        harness
            .member
            .observe_identity_wiring_target(&mob_id, &alpha)
            .await
            .unwrap(),
        alpha_present
    );

    // Member terminality is an independent observation. A torn retirement
    // without an exact unwire must not erase the retained cleanup edge.
    harness
        .events
        .append(NewMobEvent {
            mob_id: mob_id.clone(),
            timestamp: None,
            kind: MobEventKind::MemberRetired {
                agent_identity: beta.clone(),
                generation: Generation::INITIAL,
                role: ProfileName::from("model-b"),
            },
        })
        .await
        .unwrap();
    assert_eq!(
        harness
            .member
            .observe_identity_wiring_target(&mob_id, &alpha)
            .await
            .unwrap(),
        alpha_present,
    );

    // Lost ACK replay must re-observe first; the stale absent witness cannot
    // silently report success, while the exact current target can.
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&alpha_permit, &alpha_event)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::Conflict { .. }
    ));
    let exact_add = wiring_permit(
        &mob_id,
        alpha_record,
        alpha_claim,
        alpha_present.target_precondition().unwrap(),
    );
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&exact_add, &alpha_event)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::AlreadyExact { .. }
    ));

    // Timestamp pruning may discard ordinary observability history, but it
    // must retain current structural wiring anchors until a compacted head
    // exists. Reopen must reconstruct the exact same identity-local target.
    let alpha_member_before_prune = harness
        .member
        .observe_identity_member_target(&mob_id, &alpha)
        .await
        .unwrap();
    let _ = harness
        .events
        .prune(chrono::Utc::now() + chrono::Duration::days(1))
        .await
        .unwrap();
    assert_eq!(
        (harness.reopen_member)()
            .observe_identity_wiring_target(&mob_id, &alpha)
            .await
            .unwrap(),
        alpha_present,
    );
    assert_eq!(
        (harness.reopen_member)()
            .observe_identity_member_target(&mob_id, &alpha)
            .await
            .unwrap(),
        alpha_member_before_prune,
    );

    // MobReset is an epoch boundary for the structural graph. It clears the
    // reduced edge set and changes the target version, so no pre-reset permit
    // can be replayed into the new epoch. A fresh add establishes the edge in
    // the new epoch and survives pruning/reopen with the reset anchor intact.
    harness
        .events
        .append(NewMobEvent {
            mob_id: mob_id.clone(),
            timestamp: None,
            kind: MobEventKind::MobReset,
        })
        .await
        .unwrap();
    let reset_absent = harness
        .member
        .observe_identity_wiring_target(&mob_id, &alpha)
        .await
        .unwrap();
    assert!(!reset_absent.contains(&alpha_beta));
    assert_ne!(
        reset_absent.target_precondition(),
        alpha_absent.target_precondition()
    );
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&exact_add, &alpha_event)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::Conflict { .. }
    ));
    let reset_add = wiring_permit(
        &mob_id,
        alpha_record,
        alpha_claim,
        reset_absent.target_precondition().unwrap(),
    );
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&reset_add, &alpha_event)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::Applied { .. }
    ));
    let alpha_present_after_reset = harness
        .member
        .observe_identity_wiring_target(&mob_id, &alpha)
        .await
        .unwrap();
    assert!(alpha_present_after_reset.contains(&alpha_beta));
    let _ = harness
        .events
        .prune(chrono::Utc::now() + chrono::Duration::days(1))
        .await
        .unwrap();
    assert_eq!(
        (harness.reopen_member)()
            .observe_identity_wiring_target(&mob_id, &alpha)
            .await
            .unwrap(),
        alpha_present_after_reset,
    );

    // Remove alpha-beta from desired state. The old intent authority cannot
    // act, and an expired old incarnation cannot remove it after takeover.
    harness
        .identity
        .apply_identity_declaration(
            &mob_id,
            &declaration_plan(
                "scope-wiring",
                OperationId::new(),
                IdentityDeclarationScopePrecondition::Revision { revision: 1 },
                vec![
                    member("alpha", "model-a"),
                    member("beta", "model-b"),
                    member("gamma", "model-g"),
                    member("delta", "model-d"),
                ],
                BTreeSet::from([delta_gamma.clone()]),
            ),
        )
        .await
        .unwrap();
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&exact_add, &alpha_event)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::Conflict { .. }
    ));
    let revised_alpha = valid_intent(harness.identity.as_ref(), &mob_id, &alpha).await;
    let current_target = harness
        .member
        .observe_identity_wiring_target(&mob_id, &alpha)
        .await
        .unwrap();
    let stale_remove = wiring_permit(
        &mob_id,
        &revised_alpha,
        alpha_claim,
        current_target.target_precondition().unwrap(),
    );
    let remove_event = wiring_event(&mob_id, &alpha_beta, false);
    harness.clock.set(alpha_claim.expires_at_ms);
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&stale_remove, &remove_event)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::Conflict { .. }
    ));
    let takeover = acquired(
        harness
            .identity
            .claim_or_renew_identity_lease(&mob_id, &alpha, "controller", "incarnation-b", 100)
            .await
            .unwrap(),
    );
    assert!(takeover.epoch > alpha_claim.epoch);
    let takeover_remove = wiring_permit(
        &mob_id,
        &revised_alpha,
        &takeover,
        current_target.target_precondition().unwrap(),
    );
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&takeover_remove, &remove_event)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::Applied { .. }
    ));

    let alpha_removed = harness
        .member
        .observe_identity_wiring_target(&mob_id, &alpha)
        .await
        .unwrap();
    assert!(!alpha_removed.contains(&alpha_beta));
    assert_eq!(
        (harness.reopen_member)()
            .observe_identity_wiring_target(&mob_id, &alpha)
            .await
            .unwrap(),
        alpha_removed,
    );
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&takeover_remove, &remove_event)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::Conflict { .. }
    ));
    let exact_remove = wiring_permit(
        &mob_id,
        &revised_alpha,
        &takeover,
        alpha_removed.target_precondition().unwrap(),
    );
    assert!(matches!(
        harness
            .member
            .commit_identity_wiring_event(&exact_remove, &remove_event)
            .await
            .unwrap(),
        IdentityWiringEventCommitOutcome::AlreadyExact { .. }
    ));

    let events = harness.events.replay_all().await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, MobEventKind::MembersWired { .. }))
            .count(),
        4
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, MobEventKind::MembersUnwired { .. }))
            .count(),
        2
    );
}

#[cfg(feature = "runtime-adapter")]
async fn contract_runtime_write_fence_revalidates_authority(harness: ContractHarness) {
    let store = harness.store();
    let mob_id = MobId::from("contract-runtime-fence");
    let identity = AgentIdentity::from("runtime-member");
    store
        .apply_identity_declaration(
            &mob_id,
            &declaration_plan(
                "scope-runtime",
                OperationId::new(),
                IdentityDeclarationScopePrecondition::Missing,
                vec![member("runtime-member", "model-a")],
                BTreeSet::new(),
            ),
        )
        .await
        .unwrap();
    let record = valid_intent(store.as_ref(), &mob_id, &identity).await;
    let IdentityIntent::Present { session, .. } = &record.intent else {
        panic!("runtime fence fixture requires Present intent");
    };
    let expected_session = session.clone();
    let claim = acquired(
        store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "incarnation-a", 100)
            .await
            .unwrap(),
    );
    let observed = missing_runtime_observation(&expected_session.session_id).await;
    let permit = runtime_permit(
        &mob_id,
        &record,
        &claim,
        super::identity_runtime_target_observation_version(&observed),
    );

    let mut wrong_target = permit.clone();
    wrong_target.target_observation = IdentityTargetObservationVersion::Version {
        version: "sha256:stale-runtime-target".to_string(),
    };
    assert!(matches!(
        store.prepare_runtime_write_fence(wrong_target, expected_session.clone(), &observed),
        Err(MobStoreError::IdentityAuthorityBlocked { .. })
    ));
    let mut wrong_session = expected_session.clone();
    wrong_session.session_id = SessionId::new();
    assert!(matches!(
        store.prepare_runtime_write_fence(permit.clone(), wrong_session, &observed),
        Err(MobStoreError::IdentityAuthorityBlocked { .. })
    ));

    let fence = store
        .prepare_runtime_write_fence(permit, expected_session, &observed)
        .unwrap();
    let calls = AtomicU64::new(0);
    assert_eq!(
        fence
            .execute_if_current(Box::new(|| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }))
            .unwrap(),
        RuntimeStoreWriteFenceOutcome::Applied
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    harness.set_time(claim.expires_at_ms);
    assert!(matches!(
        fence
            .execute_if_current(Box::new(|| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }))
            .unwrap(),
        RuntimeStoreWriteFenceOutcome::Conflict { .. }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let takeover = acquired(
        store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "incarnation-b", 100)
            .await
            .unwrap(),
    );
    assert!(takeover.epoch > claim.epoch);
    let takeover_permit = runtime_permit(
        &mob_id,
        &record,
        &takeover,
        super::identity_runtime_target_observation_version(&observed),
    );
    let takeover_fence = store
        .prepare_runtime_write_fence(takeover_permit, session.clone(), &observed)
        .unwrap();
    assert_eq!(
        takeover_fence
            .execute_if_current(Box::new(|| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }))
            .unwrap(),
        RuntimeStoreWriteFenceOutcome::Applied
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(
        store
            .release_identity_lease(&mob_id, &identity, &takeover)
            .await
            .unwrap()
    );
    let successor = acquired(
        store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "incarnation-c", 100)
            .await
            .unwrap(),
    );
    assert!(successor.epoch > takeover.epoch);
    assert!(matches!(
        takeover_fence
            .execute_if_current(Box::new(|| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }))
            .unwrap(),
        RuntimeStoreWriteFenceOutcome::Conflict { .. }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[cfg(feature = "runtime-adapter")]
#[tokio::test]
async fn sqlite_runtime_write_fence_is_nonblocking_and_held_across_callback() {
    let harness = sqlite_harness();
    let store = harness.store();
    let mob_id = MobId::from("contract-runtime-sqlite-guard");
    let identity = AgentIdentity::from("runtime-member");
    store
        .apply_identity_declaration(
            &mob_id,
            &declaration_plan(
                "scope-runtime",
                OperationId::new(),
                IdentityDeclarationScopePrecondition::Missing,
                vec![member("runtime-member", "model-a")],
                BTreeSet::new(),
            ),
        )
        .await
        .unwrap();
    let record = valid_intent(store.as_ref(), &mob_id, &identity).await;
    let IdentityIntent::Present { session, .. } = &record.intent else {
        panic!("runtime fence fixture requires Present intent");
    };
    let claim = acquired(
        store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "incarnation-a", 100)
            .await
            .unwrap(),
    );
    let observed = missing_runtime_observation(&session.session_id).await;
    let permit = runtime_permit(
        &mob_id,
        &record,
        &claim,
        super::identity_runtime_target_observation_version(&observed),
    );
    let fence = store
        .prepare_runtime_write_fence(permit, session.clone(), &observed)
        .unwrap();
    let path = harness.raw_sqlite_path().unwrap().clone();

    let callback_observed_guard = std::cell::Cell::new(false);
    assert_eq!(
        fence
            .execute_if_current(Box::new(|| {
                let mut competing = Connection::open(&path).map_err(|error| {
                    meerkat_runtime::RuntimeStoreError::Internal(error.to_string())
                })?;
                competing
                    .busy_timeout(std::time::Duration::ZERO)
                    .map_err(|error| {
                        meerkat_runtime::RuntimeStoreError::Internal(error.to_string())
                    })?;
                let error = match competing
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                {
                    Ok(_) => panic!("identity authority guard was released before the callback"),
                    Err(error) => error,
                };
                assert!(matches!(
                    error,
                    rusqlite::Error::SqliteFailure(sqlite_error, _)
                        if matches!(
                            sqlite_error.code,
                            rusqlite::ErrorCode::DatabaseBusy
                                | rusqlite::ErrorCode::DatabaseLocked
                        )
                ));
                callback_observed_guard.set(true);
                Ok(())
            }))
            .unwrap(),
        RuntimeStoreWriteFenceOutcome::Applied
    );
    assert!(callback_observed_guard.get());

    let mut held = Connection::open(&path).unwrap();
    held.busy_timeout(std::time::Duration::ZERO).unwrap();
    let held_tx = held
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let calls = AtomicU64::new(0);
    assert!(matches!(
        fence
            .execute_if_current(Box::new(|| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }))
            .unwrap(),
        RuntimeStoreWriteFenceOutcome::Backoff { .. }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    drop(held_tx);

    assert!(harness.corrupt_intent_row(&mob_id, &identity, RawIntentCorruption::Malformed,));
    assert!(matches!(
        fence.execute_if_current(Box::new(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })),
        Err(meerkat_runtime::RuntimeStoreError::MachineLifecycleRepairBlocked { .. })
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn sqlite_future_lease_schema_blocks_mutation_without_rewrite() {
    let harness = sqlite_harness();
    let store = harness.store();
    let mob_id = MobId::from("contract-future-lease-schema");
    let identity = AgentIdentity::from("member-a");
    let claim = acquired(
        store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "incarnation-a", 100)
            .await
            .unwrap(),
    );
    let path = harness.raw_sqlite_path().unwrap();
    let conn = Connection::open(path).unwrap();
    let current: Vec<u8> = conn
        .query_row(
            "SELECT CAST(record_json AS BLOB) FROM mob_identity_leases
             WHERE mob_id = ?1 AND agent_identity = ?2",
            params![mob_id.as_str(), identity.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    let mut future: serde_json::Value = serde_json::from_slice(&current).unwrap();
    future["schema_version"] = serde_json::Value::from(u32::MAX);
    let future = serde_json::to_vec(&future).unwrap();
    assert_eq!(
        conn.execute(
            "UPDATE mob_identity_leases SET record_json = ?3
             WHERE mob_id = ?1 AND agent_identity = ?2",
            params![mob_id.as_str(), identity.as_str(), &future],
        )
        .unwrap(),
        1
    );
    drop(conn);

    assert!(matches!(
        store
            .observe_identity_lease(&mob_id, &identity)
            .await
            .unwrap(),
        IdentityStoredObservation::Unsupported { .. }
    ));
    harness.set_time(claim.renewed_at_ms + 1);
    assert!(matches!(
        store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "incarnation-a", 100,)
            .await,
        Err(MobStoreError::IdentityAuthorityBlocked { .. })
    ));
    let retained: Vec<u8> = Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT CAST(record_json AS BLOB) FROM mob_identity_leases
             WHERE mob_id = ?1 AND agent_identity = ?2",
            params![mob_id.as_str(), identity.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, future);
}

macro_rules! identity_store_contract {
    ($module:ident, $factory:ident) => {
        mod $module {
            use super::*;

            #[tokio::test]
            async fn empty_scope_restart_and_cas() {
                contract_empty_scope_restart_and_cas(super::$factory()).await;
            }

            #[tokio::test]
            async fn lost_ack_replays_original_after_later_mutation() {
                contract_lost_ack_replays_original_after_later_mutation(super::$factory()).await;
            }

            #[tokio::test]
            async fn scope_ownership_cannot_be_stolen() {
                contract_scope_ownership_cannot_be_stolen(super::$factory()).await;
            }

            #[tokio::test]
            async fn unchanged_manifest_preserves_row_revision_coherence() {
                contract_unchanged_manifest_preserves_row_revision_coherence(super::$factory())
                    .await;
            }

            #[tokio::test]
            async fn omission_seals_one_tombstone_and_incident_wiring() {
                contract_omission_seals_one_tombstone_and_incident_wiring(super::$factory()).await;
            }

            #[tokio::test]
            async fn recreate_requires_exact_retirement_proof() {
                contract_recreate_requires_exact_retirement_proof(super::$factory()).await;
            }

            #[tokio::test]
            async fn lease_renew_takeover_release_preserves_highwater() {
                contract_lease_renew_takeover_release_preserves_highwater(super::$factory()).await;
            }

            #[tokio::test]
            async fn verified_legacy_import_is_atomic_and_replay_never_resets_lease() {
                contract_verified_legacy_import_is_atomic_and_replay_never_resets_lease(
                    super::$factory(),
                )
                .await;
            }

            #[tokio::test]
            async fn legacy_import_rejects_existing_authority_and_target_collisions() {
                contract_legacy_import_rejects_existing_authority_and_target_collisions(
                    super::$factory(),
                )
                .await;
            }

            #[tokio::test]
            async fn receipt_insert_is_fenced_but_replay_survives_takeover() {
                contract_receipt_insert_is_fenced_but_replay_survives_takeover(super::$factory())
                    .await;
            }

            #[tokio::test]
            async fn deterministic_operation_sequences() {
                contract_deterministic_operation_sequences(super::$factory()).await;
            }

            #[cfg(feature = "runtime-adapter")]
            #[tokio::test]
            async fn runtime_write_fence_revalidates_authority() {
                contract_runtime_write_fence_revalidates_authority(super::$factory()).await;
            }
        }
    };
}

identity_store_contract!(in_memory, in_memory_harness);
identity_store_contract!(sqlite, sqlite_harness);

#[tokio::test]
async fn in_memory_member_spawn_is_one_atomic_target_local_cas() {
    contract_member_spawn_is_one_atomic_target_local_cas(in_memory_member_commit_harness()).await;
}

#[tokio::test]
async fn sqlite_member_spawn_is_one_atomic_target_local_cas() {
    contract_member_spawn_is_one_atomic_target_local_cas(sqlite_member_commit_harness()).await;
}

#[tokio::test]
async fn in_memory_wiring_is_one_atomic_identity_local_cas() {
    contract_wiring_is_one_atomic_identity_local_cas(in_memory_member_commit_harness()).await;
}

#[tokio::test]
async fn sqlite_wiring_is_one_atomic_identity_local_cas() {
    contract_wiring_is_one_atomic_identity_local_cas(sqlite_member_commit_harness()).await;
}

#[test]
fn legacy_import_rejects_exhausted_highwater_and_initial_delivery() {
    let (mut fixture, _) = legacy_member("parent-1", u64::MAX, 11_130);
    let legacy_import = fixture.legacy_import.as_ref().unwrap();
    assert!(matches!(
        legacy_import.validate(),
        Err(crate::identity::IdentityIntentError::CounterExhausted {
            counter: "legacy lease epoch highwater"
        })
    ));

    let IdentityLegacyImport::AdoptVerifiedLegacy {
        continuity_epoch_highwater,
        ..
    } = fixture.legacy_import.as_mut().unwrap();
    *continuity_epoch_highwater = 14_462;
    let declaration = crate::identity::IdentityMemberDeclaration {
        material: crate::identity::IdentityMemberMaterialDeclaration::Resolved {
            material: fixture.material,
        },
        session_authority_policy: DesiredSessionAuthorityPolicy::RequireExisting,
        initial_message: Some(ContentInput::from("must not redeliver")),
        legacy_import: fixture.legacy_import,
    };
    assert!(matches!(
        declaration.validate(),
        Err(crate::identity::IdentityIntentError::InvalidLegacyImport)
    ));
}

#[tokio::test]
async fn sqlite_malformed_row_is_not_missing() {
    sqlite_raw_rows_are_total_observations(RawIntentCorruption::Malformed).await;
}

#[tokio::test]
async fn sqlite_unsupported_row_is_not_missing() {
    sqlite_raw_rows_are_total_observations(RawIntentCorruption::UnsupportedSchema).await;
}

#[tokio::test]
async fn sqlite_key_mismatched_row_is_not_missing() {
    sqlite_raw_rows_are_total_observations(RawIntentCorruption::KeyMismatch {
        source_identity: AgentIdentity::from("beta"),
    })
    .await;
}
