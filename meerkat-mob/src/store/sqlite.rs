//! SQLite-backed store implementations.
//!
//! SQLite uses WAL mode with no exclusive file lock,
//! allowing the same database to be reopened after drop within the same process.

use super::in_memory::{
    IdentityAuthorityState, apply_identity_declaration_locked, validate_manifest_replay_state,
};
use super::realm_profile::{RealmProfileStore, StoredRealmProfile};
use super::{
    BeginPlacedSpawnResult, CommitPlacedSpawnResult, DeletePlacedSpawnResult,
    ExternalBindingOverlayRecord, IdentityMemberEventCommitOutcome,
    IdentityMemberTargetObservation, IdentityMemberTargetState, IdentityWiringEventCommitOutcome,
    IdentityWiringTargetObservation, IdentityWiringTargetState, MobEventStore,
    MobHostAuthorityDeletionAuthority, MobHostAuthorityPersistenceAuthority,
    MobHostAuthorityRecord, MobIdentityMemberStore, MobIdentityStatusStore, MobIdentityStore,
    MobIdentityStoreClock, MobMemberEventCursorRecord, MobMemberLiveCleanupRecord,
    MobMemberOperatorPruneAuthority, MobMemberOperatorRequestBegin, MobMemberOperatorRequestKey,
    MobMemberOperatorRequestRecord, MobOperatorGrantDeletionAuthority,
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
use crate::error::MobError;
use crate::event::{
    MobEvent, MobEventKind, NewMobEvent, decode_stored_mob_event, encode_stored_mob_event,
};
#[cfg(feature = "runtime-adapter")]
use crate::identity::DesiredSessionTarget;
use crate::identity::{
    IDENTITY_DECLARATION_SCOPE_SCHEMA_VERSION, IDENTITY_INTENT_SCHEMA_VERSION,
    IDENTITY_LEASE_MAX_TTL_MS, IDENTITY_LEASE_SCHEMA_VERSION,
    IDENTITY_OPERATION_RECEIPT_SCHEMA_VERSION, IdentityActuationPermit, IdentityActuatorTarget,
    IdentityConvergenceStatus, IdentityDeclarationApplyPlan,
    IdentityDeclarationManifestApplyOutcome, IdentityDeclarationScopeHead,
    IdentityDeclarationScopeId, IdentityIntent, IdentityIntentError, IdentityIntentRecord,
    IdentityLeaseClaim, IdentityLeaseClaimOutcome, IdentityLeaseRecord, IdentityOperationKind,
    IdentityOperationReceipt, IdentityOperationReceiptInsertOutcome,
    IdentityOperationReceiptPayload, IdentityOperationSlot, IdentityOperationSubject,
    IdentityRetirementPlan, IdentityStoredObservation, IdentityTargetObservationVersion,
};
use crate::ids::{
    AgentIdentity, FlowId, FrameId, Generation, LoopId, LoopInstanceId, MobId, RunId, StepId,
};
use crate::machines::mob_machine as mob_dsl;
use crate::profile::Profile;
use crate::run::flow_run;
use crate::run::{
    FailureLedgerEntry, FrameSnapshot, LoopIterationLedgerEntry, LoopSnapshot, MobRun,
    MobRunProvenanceAuthority, MobRunRemoteTurnIntent, MobRunRemoteTurnReceipt, MobRunStatus,
    StepLedgerEntry, mob_machine_run_status_is_terminal,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use notify::{RecursiveMode, Watcher};
use rusqlite::{
    Connection, ErrorCode, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::thread;
use std::time::Duration;
use tokio::sync::broadcast;

const EVENT_SUBSCRIPTION_CHANNEL_CAPACITY: usize = 4096;
const EVENT_WATCH_CATCH_UP_LIMIT: usize = 1024;
const EVENT_WATCH_POLL_FALLBACK_MS: u64 = 250;
const IDENTITY_STORE_INSTANCE_KEY: &str = "store_instance_id";
const IDENTITY_RECEIPT_SLOT_KEY_VERSION: i64 = 1;

const CREATE_SCHEMA_SQL: &str = r"
CREATE TABLE IF NOT EXISTS mob_events (
    cursor INTEGER PRIMARY KEY,
    mob_id TEXT,
    event_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_event_meta (
    key TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_runs (
    run_id TEXT PRIMARY KEY,
    run_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_run_remote_turn_intents (
    run_id TEXT NOT NULL,
    dispatch_sequence BLOB NOT NULL,
    intent_json BLOB NOT NULL,
    PRIMARY KEY (run_id, dispatch_sequence)
);
CREATE TABLE IF NOT EXISTS mob_run_remote_turn_receipts (
    run_id TEXT NOT NULL,
    dispatch_sequence BLOB NOT NULL,
    receipt_json BLOB NOT NULL,
    PRIMARY KEY (run_id, dispatch_sequence)
);
CREATE TABLE IF NOT EXISTS mob_specs (
    mob_id TEXT PRIMARY KEY,
    spec_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_runtime_supervisors (
    mob_id TEXT PRIMARY KEY,
    record_json BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_runtime_host_authorities (
    mob_id TEXT NOT NULL,
    host_id TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, host_id)
);
CREATE TABLE IF NOT EXISTS mob_runtime_host_binding_generation_highwaters (
    mob_id TEXT NOT NULL,
    host_id TEXT NOT NULL,
    binding_generation BLOB NOT NULL,
    PRIMARY KEY (mob_id, host_id)
);
CREATE TABLE IF NOT EXISTS mob_runtime_member_operator_requests (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    generation BLOB NOT NULL,
    fence_token BLOB NOT NULL,
    host_id TEXT NOT NULL,
    binding_generation BLOB NOT NULL,
    member_session_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity, generation, fence_token, host_id, binding_generation, member_session_id, request_id)
);
CREATE TABLE IF NOT EXISTS mob_runtime_placed_spawns (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity)
);
CREATE TABLE IF NOT EXISTS mob_runtime_operator_grants (
    mob_id TEXT NOT NULL,
    principal TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, principal)
);
CREATE TABLE IF NOT EXISTS mob_runtime_member_event_cursors (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity)
);
CREATE TABLE IF NOT EXISTS mob_runtime_member_live_cleanups (
    mob_id TEXT NOT NULL,
    cleanup_id TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, cleanup_id)
);
CREATE TABLE IF NOT EXISTS mob_runtime_binding_overlays (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    generation INTEGER NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity, generation)
);
CREATE TABLE IF NOT EXISTS mob_identity_declaration_scopes (
    mob_id TEXT NOT NULL,
    scope_id TEXT NOT NULL,
    head_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, scope_id)
);
CREATE TABLE IF NOT EXISTS mob_identity_store_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS mob_identity_intents (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    declaration_scope TEXT,
    session_id TEXT,
    lineage_id TEXT,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity)
);
CREATE UNIQUE INDEX IF NOT EXISTS mob_identity_intents_session
    ON mob_identity_intents (session_id)
    WHERE session_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS mob_identity_intents_lineage
    ON mob_identity_intents (lineage_id)
    WHERE lineage_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS mob_identity_intents_scope
    ON mob_identity_intents (mob_id, declaration_scope);
CREATE TABLE IF NOT EXISTS mob_identity_leases (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    epoch_highwater BLOB NOT NULL,
    active_holder_id TEXT,
    active_incarnation_id TEXT,
    active_epoch BLOB,
    active_renewed_at_ms BLOB,
    active_expires_at_ms BLOB,
    authority_digest TEXT NOT NULL,
    record_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity)
);
CREATE TABLE IF NOT EXISTS mob_identity_operation_receipts (
    mob_id TEXT NOT NULL,
    subject_kind TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    slot_kind TEXT NOT NULL,
    slot_key_version INTEGER NOT NULL,
    slot_digest TEXT NOT NULL,
    subject_json BLOB NOT NULL,
    slot_json BLOB NOT NULL,
    subject_scope_id TEXT,
    subject_identity TEXT,
    effect_kind TEXT NOT NULL,
    receipt_json BLOB NOT NULL,
    PRIMARY KEY (
        mob_id, subject_kind, subject_id, slot_kind,
        slot_key_version, slot_digest
    )
);
CREATE INDEX IF NOT EXISTS mob_identity_receipts_scope
    ON mob_identity_operation_receipts (mob_id, subject_scope_id);
CREATE INDEX IF NOT EXISTS mob_identity_receipts_identity
    ON mob_identity_operation_receipts (mob_id, subject_identity);
CREATE TABLE IF NOT EXISTS mob_identity_statuses (
    mob_id TEXT NOT NULL,
    agent_identity TEXT NOT NULL,
    status_json BLOB NOT NULL,
    PRIMARY KEY (mob_id, agent_identity)
);
CREATE TABLE IF NOT EXISTS realm_profiles (
    name TEXT PRIMARY KEY,
    profile_json BLOB NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
)";

fn se(error: impl std::fmt::Display) -> MobStoreError {
    MobStoreError::Internal(error.to_string())
}

fn encode_json<T: Serialize>(value: &T) -> Result<Vec<u8>, MobStoreError> {
    serde_json::to_vec(value).map_err(|e| MobStoreError::Serialization(e.to_string()))
}

fn decode_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, MobStoreError> {
    serde_json::from_slice(bytes).map_err(|e| MobStoreError::Serialization(e.to_string()))
}

fn identity_raw_evidence_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn identity_contract_error(error: IdentityIntentError) -> MobStoreError {
    MobStoreError::Serialization(error.to_string())
}

fn identity_authority_blocked(
    evidence_digest: Option<String>,
    detail: impl Into<String>,
) -> MobStoreError {
    MobStoreError::IdentityAuthorityBlocked {
        evidence_digest,
        detail: detail.into(),
    }
}

fn classify_identity_blob<T>(
    storage_type: &str,
    bytes: &[u8],
    expected_schema_version: Option<u32>,
    validate: impl FnOnce(&T) -> Result<(), IdentityIntentError>,
    physical_key_matches: impl FnOnce(&T) -> Result<(), String>,
) -> IdentityStoredObservation<T>
where
    T: DeserializeOwned,
{
    let evidence_digest = identity_raw_evidence_digest(bytes);
    if storage_type != "blob" {
        return IdentityStoredObservation::Malformed {
            evidence_digest,
            detail: format!(
                "identity row uses SQLite storage class '{storage_type}', expected blob"
            ),
        };
    }
    let json = match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(value) => value,
        Err(error) => {
            return IdentityStoredObservation::Malformed {
                evidence_digest,
                detail: format!("identity row JSON is malformed: {error}"),
            };
        }
    };
    if let Some(expected) = expected_schema_version
        && let Some(observed) = json
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
        && observed != u64::from(expected)
    {
        return IdentityStoredObservation::Unsupported {
            evidence_digest,
            detail: format!("unsupported identity row schema version {observed}"),
        };
    }
    let value = match serde_json::from_value::<T>(json) {
        Ok(value) => value,
        Err(error) => {
            return IdentityStoredObservation::Malformed {
                evidence_digest,
                detail: format!("identity row JSON does not match its current schema: {error}"),
            };
        }
    };
    if let Err(detail) = physical_key_matches(&value) {
        return IdentityStoredObservation::Malformed {
            evidence_digest,
            detail,
        };
    }
    match validate(&value) {
        Ok(()) => IdentityStoredObservation::Valid(value),
        Err(error @ IdentityIntentError::UnsupportedSchemaVersion { .. }) => {
            IdentityStoredObservation::Unsupported {
                evidence_digest,
                detail: error.to_string(),
            }
        }
        Err(error) => IdentityStoredObservation::Malformed {
            evidence_digest,
            detail: error.to_string(),
        },
    }
}

fn require_identity_authority<T>(
    observation: IdentityStoredObservation<T>,
    row_name: &str,
) -> Result<Option<T>, MobStoreError> {
    match observation {
        IdentityStoredObservation::Missing => Ok(None),
        IdentityStoredObservation::Valid(value) => Ok(Some(value)),
        IdentityStoredObservation::Unsupported {
            evidence_digest,
            detail,
        }
        | IdentityStoredObservation::Malformed {
            evidence_digest,
            detail,
        } => Err(identity_authority_blocked(
            Some(evidence_digest),
            format!("{row_name}: {detail}"),
        )),
    }
}

fn validate_identity_store_text(field: &str, value: &str) -> Result<(), MobStoreError> {
    if value.is_empty() || value.trim() != value {
        return Err(MobStoreError::Serialization(format!(
            "{field} must be nonempty canonical text"
        )));
    }
    Ok(())
}

fn decode_optional_identity_text(
    storage_type: &str,
    bytes: Option<Vec<u8>>,
    field: &str,
) -> Result<Option<String>, String> {
    match (storage_type, bytes) {
        ("null", None) => Ok(None),
        ("text", Some(bytes)) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|error| format!("{field} is not valid UTF-8 text: {error}")),
        (observed, _) => Err(format!(
            "{field} uses SQLite storage class '{observed}', expected text or null"
        )),
    }
}

fn decode_required_identity_text(
    storage_type: &str,
    bytes: Vec<u8>,
    field: &str,
) -> Result<String, String> {
    if storage_type != "text" {
        return Err(format!(
            "{field} uses SQLite storage class '{storage_type}', expected text"
        ));
    }
    String::from_utf8(bytes).map_err(|error| format!("{field} is not valid UTF-8 text: {error}"))
}

fn identity_intent_projection(
    record: &IdentityIntentRecord,
) -> (Option<String>, Option<String>, Option<String>) {
    let target = match &record.retirement_plan {
        IdentityRetirementPlan::Targets { session, .. } => Some(session),
        IdentityRetirementPlan::NoKnownRealization => match &record.intent {
            IdentityIntent::Present { session, .. } => Some(session),
            IdentityIntent::Absent { .. } => None,
        },
    };
    (
        record
            .declaration_scope
            .as_ref()
            .map(|scope| scope.as_str().to_string()),
        target.map(|target| target.session_id.to_string()),
        target.map(|target| target.lineage_id.as_str().to_string()),
    )
}

fn identity_intent_physical_key_matches(
    record: &IdentityIntentRecord,
    mob_id: &MobId,
    identity: &AgentIdentity,
    declaration_scope: Option<&str>,
    session_id: Option<&str>,
    lineage_id: Option<&str>,
) -> Result<(), String> {
    if record.mob_id != *mob_id || record.intent.identity() != identity {
        return Err(
            "identity intent record does not match its physical mob/identity key".to_string(),
        );
    }
    let expected = identity_intent_projection(record);
    if expected.0.as_deref() != declaration_scope
        || expected.1.as_deref() != session_id
        || expected.2.as_deref() != lineage_id
    {
        return Err(
            "identity intent record does not match its derived scope/session/lineage keys"
                .to_string(),
        );
    }
    Ok(())
}

fn identity_operation_kind_key(kind: IdentityOperationKind) -> &'static str {
    match kind {
        IdentityOperationKind::ApplyDeclarationManifest => "apply_declaration_manifest",
        IdentityOperationKind::SessionCreationConsumed => "session_creation_consumed",
        IdentityOperationKind::RetirementProven => "retirement_proven",
        IdentityOperationKind::ExternalBinding => "external_binding",
        IdentityOperationKind::InitialDelivery => "initial_delivery",
    }
}

fn identity_receipt_keys(
    subject: &IdentityOperationSubject,
    slot: &IdentityOperationSlot,
) -> Result<(String, String, Vec<u8>, Vec<u8>), MobStoreError> {
    let subject_text = serde_json::to_string(subject)
        .map_err(|error| MobStoreError::Serialization(error.to_string()))?;
    let slot_text = serde_json::to_string(slot)
        .map_err(|error| MobStoreError::Serialization(error.to_string()))?;
    Ok((
        subject_text.clone(),
        slot_text.clone(),
        subject_text.into_bytes(),
        slot_text.into_bytes(),
    ))
}

fn identity_receipt_subject_projection(
    subject: &IdentityOperationSubject,
) -> (Option<&str>, Option<&str>) {
    match subject {
        IdentityOperationSubject::DeclarationScope { scope_id } => (Some(scope_id.as_str()), None),
        IdentityOperationSubject::Identity { identity } => (None, Some(identity.as_str())),
    }
}

fn identity_receipt_sql_key(
    subject: &IdentityOperationSubject,
    slot: &IdentityOperationSlot,
) -> Result<(&'static str, String, &'static str, i64, String), MobStoreError> {
    #[derive(Serialize)]
    struct SlotDigestMaterial<'a> {
        domain: &'static str,
        version: i64,
        slot: &'a IdentityOperationSlot,
    }
    let (subject_kind, subject_id) = match subject {
        IdentityOperationSubject::DeclarationScope { scope_id } => {
            ("declaration_scope", scope_id.as_str().to_string())
        }
        IdentityOperationSubject::Identity { identity } => {
            ("identity", identity.as_str().to_string())
        }
    };
    let slot_kind = identity_operation_kind_key(slot.kind());
    let material = serde_json::to_vec(&SlotDigestMaterial {
        domain: "meerkat.identity.operation_slot_key.v1",
        version: IDENTITY_RECEIPT_SLOT_KEY_VERSION,
        slot,
    })
    .map_err(|error| MobStoreError::Serialization(error.to_string()))?;
    Ok((
        subject_kind,
        subject_id,
        slot_kind,
        IDENTITY_RECEIPT_SLOT_KEY_VERSION,
        identity_raw_evidence_digest(&material),
    ))
}

fn identity_lease_authority_digest(
    mob_id: &MobId,
    identity: &AgentIdentity,
    record: &IdentityLeaseRecord,
) -> Result<String, MobStoreError> {
    #[derive(Serialize)]
    struct LeaseAuthorityMaterial<'a> {
        domain: &'static str,
        mob_id: &'a MobId,
        identity: &'a AgentIdentity,
        schema_version: u32,
        epoch_highwater: u64,
        active: &'a Option<IdentityLeaseClaim>,
    }
    let bytes = serde_json::to_vec(&LeaseAuthorityMaterial {
        domain: "meerkat.identity.lease.authority.v1",
        mob_id,
        identity,
        schema_version: record.schema_version,
        epoch_highwater: record.epoch_highwater,
        active: &record.active,
    })
    .map_err(|error| MobStoreError::Serialization(error.to_string()))?;
    Ok(identity_raw_evidence_digest(&bytes))
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
        ) => {
            let Some(delivery) = &member.initial_delivery else {
                return false;
            };
            *tombstone_generation == normalized_tombstone
                && session_id == &session.session_id
                && lineage_id == &session.lineage_id
                && *lineage_generation == session.lineage_generation
                && delivery_generation == payload_generation
                && *delivery_generation == delivery.delivery_generation
                && delivery_id == &delivery.delivery_id
                && message_digest == &delivery.message_digest
        }
        _ => false,
    }
}

fn validate_member_operator_request_row(
    record: &MobMemberOperatorRequestRecord,
    key: &MobMemberOperatorRequestKey,
) -> Result<(), MobStoreError> {
    record.validate()?;
    if record.key().eq(key) {
        return Ok(());
    }
    Err(MobStoreError::Internal(format!(
        "member operator request row key does not match record identity/generation/fence/host/binding_generation/session/request_id: row=({},{},{},{},{},{},{}) record=({},{},{},{},{},{},{})",
        key.agent_identity,
        key.generation,
        key.fence_token,
        key.host_id,
        key.host_binding_generation,
        key.member_session_id,
        key.request_id,
        record.agent_identity,
        record.generation,
        record.fence_token,
        record.host_id,
        record.host_binding_generation,
        record.member_session_id,
        record.request_id
    )))
}

fn cursor_to_i64(value: u64) -> Result<i64, MobStoreError> {
    i64::try_from(value)
        .map_err(|_| MobStoreError::Internal(format!("cursor value {value} exceeds i64::MAX")))
}

/// Full-domain, order-preserving SQLite key for newly introduced u64
/// identity/sequence columns. SQLite INTEGER is signed and would reject the
/// upper half of the public u64 domain; fixed-width big-endian blobs compare
/// lexicographically in the same order as the source integers.
fn u64_to_sql_key(value: u64) -> Vec<u8> {
    value.to_be_bytes().to_vec()
}

fn sql_key_to_u64(bytes: &[u8], field: &str) -> Result<u64, MobStoreError> {
    let encoded: [u8; std::mem::size_of::<u64>()] = bytes.try_into().map_err(|_| {
        MobStoreError::Internal(format!(
            "{field} SQLite key has {} bytes; expected {}",
            bytes.len(),
            std::mem::size_of::<u64>()
        ))
    })?;
    Ok(u64::from_be_bytes(encoded))
}

fn i64_to_cursor(value: i64) -> u64 {
    // SQLite INTEGER is signed; cursors start at 1 and are monotonic.
    // Negative values should never appear, but clamp to 0 defensively.
    u64::try_from(value).unwrap_or(0)
}

/// Read only the stable routing envelope shared by every current stored mob
/// event. This deliberately does not decode `kind`: future or malformed event
/// payloads can still be attributed to their physical mob without poisoning
/// unrelated identity observations.
fn stored_event_envelope_mob_id(bytes: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<serde_json::Value>(bytes).ok()?;
    let mob_id = value.get("event")?.get("mob_id")?.as_str()?.to_string();
    (!mob_id.is_empty() && mob_id.trim() == mob_id).then_some(mob_id)
}

/// Add and backfill the derived mob route for legacy event tables. The event
/// bytes remain the durable source; this column is only a physical locality
/// key. Completely unreadable legacy bytes stay NULL and therefore remain
/// conservatively visible to every identity observation.
fn migration_0002_event_route(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    let has_mob_id = {
        let mut statement = tx.prepare("PRAGMA table_info(mob_events)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        columns.iter().any(|column| column == "mob_id")
    };
    if !has_mob_id {
        tx.execute("ALTER TABLE mob_events ADD COLUMN mob_id TEXT", [])?;
    }
    tx.execute(
        "CREATE INDEX IF NOT EXISTS mob_events_mob_cursor ON mob_events (mob_id, cursor)",
        [],
    )?;
    Ok(())
}

/// Idempotent NULL-route heal, run on every open (not a one-shot migration):
/// binaries predating the route column keep inserting rows with a NULL
/// `mob_id`, and those rows must regain their physical locality key on the
/// next open. Statement-at-a-time (no transaction of its own) is safe
/// because each UPDATE is keyed and idempotent; rows whose event bytes are
/// unreadable stay NULL and remain conservatively visible to every identity
/// observation.
fn heal_unrouted_events(conn: &Connection) -> Result<(), MobStoreError> {
    let unrouted = {
        let mut statement = conn
            .prepare(
                "SELECT cursor, CAST(event_json AS BLOB)
                 FROM mob_events
                 WHERE mob_id IS NULL
                 ORDER BY cursor",
            )
            .map_err(se)?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(se)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(se)?
    };
    for (cursor, bytes) in unrouted {
        if let Some(mob_id) = stored_event_envelope_mob_id(&bytes) {
            conn.execute(
                "UPDATE mob_events SET mob_id = ?2 WHERE cursor = ?1 AND mob_id IS NULL",
                params![cursor, mob_id],
            )
            .map_err(se)?;
        }
    }
    Ok(())
}

fn migration_0003_member_operator_execution_fence(
    tx: &Transaction<'_>,
) -> Result<(), rusqlite::Error> {
    let (has_host_id, has_binding_generation, has_member_session_id, primary_key) = {
        let mut stmt = tx.prepare("PRAGMA table_info(mob_runtime_member_operator_requests)")?;
        let columns = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
        })?;
        let mut found_host_id = false;
        let mut found_binding_generation = false;
        let mut found_member_session_id = false;
        let mut primary_key = Vec::new();
        for column in columns {
            let (name, key_position) = column?;
            match name.as_str() {
                "host_id" => found_host_id = true,
                "binding_generation" => found_binding_generation = true,
                "member_session_id" => found_member_session_id = true,
                _ => {}
            }
            if key_position > 0 {
                primary_key.push((key_position, name));
            }
        }
        primary_key.sort_by_key(|(position, _)| *position);
        (
            found_host_id,
            found_binding_generation,
            found_member_session_id,
            primary_key
                .into_iter()
                .map(|(_, name)| name)
                .collect::<Vec<_>>(),
        )
    };
    let expected_primary_key = [
        "mob_id",
        "agent_identity",
        "generation",
        "fence_token",
        "host_id",
        "binding_generation",
        "member_session_id",
        "request_id",
    ];
    if !has_host_id
        || !has_binding_generation
        || !has_member_session_id
        || !primary_key
            .iter()
            .map(String::as_str)
            .eq(expected_primary_key)
    {
        // Pre-fence rows cannot be attributed to a host generation. The wire
        // now rejects every pre-fence request before ledger access, so keeping
        // those rows would only create ambiguous replay keys; rebuild this
        // bounded negative-memory table empty under the exact residency key.
        tx.execute_batch(
            "ALTER TABLE mob_runtime_member_operator_requests
                 RENAME TO mob_runtime_member_operator_requests_pre_execution_fence;
             CREATE TABLE mob_runtime_member_operator_requests (
                 mob_id TEXT NOT NULL,
                 agent_identity TEXT NOT NULL,
                 generation BLOB NOT NULL,
                 fence_token BLOB NOT NULL,
                 host_id TEXT NOT NULL,
                 binding_generation BLOB NOT NULL,
                 member_session_id TEXT NOT NULL,
                 request_id TEXT NOT NULL,
                 record_json BLOB NOT NULL,
                 PRIMARY KEY (
                     mob_id, agent_identity, generation, fence_token, host_id,
                     binding_generation, member_session_id, request_id
                 )
             );
             DROP TABLE mob_runtime_member_operator_requests_pre_execution_fence;",
        )?;
    }
    Ok(())
}

fn migration_0001_mob_schema(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
    tx.execute_batch(CREATE_SCHEMA_SQL)
}

/// The mob store bundle's schema domain in the per-file migration ledger.
///
/// Migrations 0002/0003 lift the historical per-open upgrade functions
/// (event-route column backfill, member-operator fence-table rebuild) into
/// once-per-file migrations; their internal shape probes keep them
/// convergent on files of any vintage.
pub const MOB_DOMAIN: meerkat_sqlite::SchemaDomain = meerkat_sqlite::SchemaDomain {
    name: "mob",
    migrations: &[
        meerkat_sqlite::Migration {
            version: 1,
            name: "base-schema",
            apply: migration_0001_mob_schema,
        },
        meerkat_sqlite::Migration {
            version: 2,
            name: "event-route",
            apply: migration_0002_event_route,
        },
        meerkat_sqlite::Migration {
            version: 3,
            name: "member-operator-execution-fence",
            apply: migration_0003_member_operator_execution_fence,
        },
    ],
};

/// Per-operation connection: fence guard lives exactly as long as the
/// connection it admits.
struct MobConn {
    conn: Connection,
    _guard: meerkat_sqlite::OperationGuard,
}

impl std::ops::Deref for MobConn {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.conn
    }
}

impl std::ops::DerefMut for MobConn {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

fn open_connection(path: &Path) -> Result<MobConn, MobStoreError> {
    let guard = meerkat_sqlite::OperationGuard::for_database(path).map_err(se)?;
    let mut conn =
        meerkat_sqlite::open(path, meerkat_sqlite::ConnectionProfile::PRIMARY).map_err(se)?;
    meerkat_sqlite::apply_domain_migrations(&mut conn, &MOB_DOMAIN).map_err(se)?;
    // Data reconciliation (not schema): reconcile the durable cursor
    // allocator with MAX(cursor) and heal NULL event routes on every open.
    repair_next_event_cursor(&conn)?;
    heal_unrouted_events(&conn)?;
    Ok(MobConn {
        conn,
        _guard: guard,
    })
}

/// Open an existing mob database for passive observation without creating the
/// primary database or running migrations. Event-watch catch-up may race
/// terminal storage removal, so it must never use the create-capable writer
/// connection.
fn open_existing_read_connection(path: &Path) -> Result<Connection, MobStoreError> {
    // Deliberately unguarded: passive observation may hold this connection
    // across long watch windows, and readers must not stall the exclusive
    // maintenance fence; reader quiescence during maintenance happens at the
    // SQLite layer.
    meerkat_sqlite::open(path, meerkat_sqlite::ConnectionProfile::ReadOnly).map_err(se)
}

fn initialize_identity_store_instance(conn: &Connection) -> Result<String, MobStoreError> {
    let candidate = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT OR IGNORE INTO mob_identity_store_meta (key, value) VALUES (?1, ?2)",
        params![IDENTITY_STORE_INSTANCE_KEY, candidate],
    )
    .map_err(se)?;
    conn.query_row(
        "SELECT value FROM mob_identity_store_meta WHERE key = ?1",
        params![IDENTITY_STORE_INSTANCE_KEY],
        |row| row.get(0),
    )
    .map_err(se)
}

fn verify_identity_store_instance(
    conn: &Connection,
    expected_store_instance_id: &str,
) -> Result<(), MobStoreError> {
    let actual: String = conn
        .query_row(
            "SELECT value FROM mob_identity_store_meta WHERE key = ?1",
            params![IDENTITY_STORE_INSTANCE_KEY],
            |row| row.get(0),
        )
        .map_err(|error| {
            MobStoreError::ReadFailed(format!(
                "identity store instance marker is unavailable: {error}"
            ))
        })?;
    if actual != expected_store_instance_id {
        return Err(identity_authority_blocked(
            None,
            "identity store database was replaced while this store handle was live",
        ));
    }
    Ok(())
}

fn open_existing_identity_read_connection(
    path: &Path,
    expected_store_instance_id: &str,
) -> Result<Connection, MobStoreError> {
    let conn = open_existing_read_connection(path).map_err(|error| {
        MobStoreError::ReadFailed(format!("identity store database is unavailable: {error}"))
    })?;
    verify_identity_store_instance(&conn, expected_store_instance_id)?;
    Ok(conn)
}

fn open_existing_identity_write_connection(
    path: &Path,
    _expected_store_instance_id: &str,
) -> Result<MobConn, MobStoreError> {
    let guard = meerkat_sqlite::OperationGuard::for_database(path).map_err(se)?;
    let conn = meerkat_sqlite::open(
        path,
        meerkat_sqlite::ConnectionProfile::Primary { create: false },
    )
    .map_err(|error| {
        MobStoreError::WriteFailed(format!("identity store database is unavailable: {error}"))
    })?;
    Ok(MobConn {
        conn,
        _guard: guard,
    })
}

fn begin_identity_immediate<'connection>(
    conn: &'connection mut Connection,
    expected_store_instance_id: &str,
) -> Result<Transaction<'connection>, MobStoreError> {
    let tx = begin_immediate(conn)?;
    // Verify inside the same writer transaction that will sample the lease
    // clock and perform the mutation. A replaced database can never restart
    // epoch or idempotency authority through this handle.
    verify_identity_store_instance(&tx, expected_store_instance_id)?;
    Ok(tx)
}

fn begin_immediate(conn: &mut Connection) -> Result<Transaction<'_>, MobStoreError> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(se)
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn latest_event_cursor_sync(path: &Path) -> Result<u64, MobStoreError> {
    let conn = open_existing_read_connection(path)?;
    let cursor: Option<i64> = conn
        .query_row("SELECT MAX(cursor) FROM mob_events", [], |row| row.get(0))
        .optional()
        .map_err(se)?
        .flatten();
    Ok(cursor.map_or(0, i64_to_cursor))
}

fn poll_events_sync(
    path: &Path,
    after_cursor: u64,
    limit: usize,
) -> Result<Vec<MobEvent>, MobStoreError> {
    let conn = open_existing_read_connection(path)?;
    poll_events_from_connection(&conn, after_cursor, limit)
}

fn poll_events_from_connection(
    conn: &Connection,
    after_cursor: u64,
    limit: usize,
) -> Result<Vec<MobEvent>, MobStoreError> {
    let mut stmt = conn
        .prepare("SELECT event_json FROM mob_events WHERE cursor > ?1 ORDER BY cursor LIMIT ?2")
        .map_err(se)?;
    let rows = stmt
        .query_map(
            params![
                cursor_to_i64(after_cursor)?,
                i64::try_from(limit).map_err(|_| se("limit exceeds i64::MAX"))?
            ],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .map_err(se)?;
    let mut result = Vec::new();
    for row in rows {
        let bytes = row.map_err(se)?;
        result.push(
            decode_stored_mob_event(&bytes)
                .map_err(|e| MobStoreError::Serialization(e.to_string()))?,
        );
    }
    Ok(result)
}

fn load_run_bytes(tx: &Transaction<'_>, key: &str) -> Result<Option<Vec<u8>>, MobStoreError> {
    tx.query_row(
        "SELECT run_json FROM mob_runs WHERE run_id = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(se)
}

fn write_run_json(tx: &Transaction<'_>, key: &str, run: &MobRun) -> Result<(), MobStoreError> {
    let encoded = encode_json(run)?;
    tx.execute(
        "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
        params![encoded, key],
    )
    .map_err(se)?;
    Ok(())
}

fn append_loop_iteration_ledger_if_absent(run: &mut MobRun, entry: LoopIterationLedgerEntry) {
    if !run.loop_iteration_ledger.iter().any(|existing| {
        existing.loop_instance_id == entry.loop_instance_id
            && existing.iteration == entry.iteration
            && existing.frame_id == entry.frame_id
    }) {
        run.loop_iteration_ledger.push(entry);
    }
}

async fn run_sqlite_task<T>(
    task: impl FnOnce() -> Result<T, MobStoreError> + Send + 'static,
) -> Result<T, MobStoreError>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|error| MobStoreError::Internal(format!("sqlite task join failed: {error}")))?
}

// ---------------------------------------------------------------------------
// SqliteMobStores — unified handle (stores only the path)
// ---------------------------------------------------------------------------

static SQLITE_EVENT_BUSES: OnceLock<Mutex<HashMap<PathBuf, Weak<SqliteMobEventBus>>>> =
    OnceLock::new();

fn sqlite_event_buses() -> &'static Mutex<HashMap<PathBuf, Weak<SqliteMobEventBus>>> {
    SQLITE_EVENT_BUSES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn sqlite_event_bus_for_path(path: PathBuf) -> Result<Arc<SqliteMobEventBus>, MobStoreError> {
    let mut buses = lock_unpoisoned(sqlite_event_buses());
    if let Some(bus) = buses.get(&path).and_then(Weak::upgrade) {
        return Ok(bus);
    }

    let bus = SqliteMobEventBus::new(path.clone())?;
    buses.insert(path, Arc::downgrade(&bus));
    Ok(bus)
}

struct SqliteMobEventBus {
    path: PathBuf,
    event_tx: broadcast::Sender<MobEvent>,
    latest_broadcast_cursor: Mutex<u64>,
    catch_up_lock: Mutex<()>,
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
}

impl std::fmt::Debug for SqliteMobEventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteMobEventBus")
            .field("path", &self.path)
            .finish()
    }
}

impl SqliteMobEventBus {
    fn new(path: PathBuf) -> Result<Arc<Self>, MobStoreError> {
        let latest_cursor = latest_event_cursor_sync(&path)?;
        let (event_tx, _event_rx) = broadcast::channel(EVENT_SUBSCRIPTION_CHANNEL_CAPACITY);
        let bus = Arc::new(Self {
            path,
            event_tx,
            latest_broadcast_cursor: Mutex::new(latest_cursor),
            catch_up_lock: Mutex::new(()),
            watcher: Mutex::new(None),
        });
        bus.start_external_watch();
        Ok(bus)
    }

    fn subscribe(&self) -> super::MobEventReceiver {
        self.event_tx.subscribe()
    }

    fn publish_committed(&self, event: MobEvent) {
        self.publish_committed_batch(std::slice::from_ref(&event));
    }

    fn publish_committed_batch(&self, events: &[MobEvent]) {
        let current_cursor = *lock_unpoisoned(&self.latest_broadcast_cursor);
        let mut expected_cursor = current_cursor.saturating_add(1);
        let mut has_gap = false;
        for event in events.iter().filter(|event| event.cursor > current_cursor) {
            if event.cursor > expected_cursor {
                has_gap = true;
                break;
            }
            expected_cursor = event.cursor.saturating_add(1);
        }
        if has_gap {
            if let Err(error) = self.publish_available_from_storage() {
                tracing::warn!(
                    error = %error,
                    path = %self.path.display(),
                    "sqlite mob event gap catch-up failed before direct publish",
                );
            }
        }
        self.publish_committed_batch_unchecked(events);
    }

    fn publish_committed_batch_unchecked(&self, events: &[MobEvent]) {
        let mut cursor = lock_unpoisoned(&self.latest_broadcast_cursor);
        for event in events {
            if event.cursor <= *cursor {
                continue;
            }
            *cursor = event.cursor;
            let _ = self.event_tx.send(event.clone());
        }
    }

    fn publish_available_from_storage(&self) -> Result<(), MobStoreError> {
        let _catch_up_guard = lock_unpoisoned(&self.catch_up_lock);
        if !self.path.try_exists().map_err(se)? {
            // A successful mob destroy removes the SQLite database while
            // existing handle clones may still keep this event bus alive. The
            // watcher can wake on that delete event; avoid reopening the path,
            // because opening SQLite would create a fresh empty database.
            return Ok(());
        }
        loop {
            let after_cursor = *lock_unpoisoned(&self.latest_broadcast_cursor);
            let batch = match poll_events_sync(&self.path, after_cursor, EVENT_WATCH_CATCH_UP_LIMIT)
            {
                Ok(batch) => batch,
                Err(error) => {
                    if !self.path.try_exists().map_err(se)? {
                        // The file disappeared after the preflight existence
                        // check. This is normal terminal destroy, and the
                        // no-create reader guarantees catch-up cannot resurrect
                        // the database in this window.
                        return Ok(());
                    }
                    return Err(error);
                }
            };
            if batch.is_empty() {
                return Ok(());
            }
            let is_complete = batch.len() < EVENT_WATCH_CATCH_UP_LIMIT;
            self.publish_committed_batch_unchecked(&batch);
            if is_complete {
                return Ok(());
            }
        }
    }

    fn start_external_watch(self: &Arc<Self>) {
        let Some(parent) = self.path.parent().map(Path::to_path_buf) else {
            return;
        };
        let watched_paths = sqlite_watch_paths(&self.path);
        let (wake_tx, wake_rx) = std::sync::mpsc::channel::<()>();
        let thread_bus = Arc::downgrade(self);
        let thread_builder = thread::Builder::new().name("sqlite-mob-event-watch".to_string());
        if let Err(error) = thread_builder.spawn(move || {
            loop {
                let received_wake = match wake_rx
                    .recv_timeout(Duration::from_millis(EVENT_WATCH_POLL_FALLBACK_MS))
                {
                    Ok(()) => true,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => false,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                };
                if received_wake {
                    thread::sleep(Duration::from_millis(10));
                }
                while wake_rx.try_recv().is_ok() {}
                let Some(bus) = thread_bus.upgrade() else {
                    break;
                };
                if !received_wake && bus.event_tx.receiver_count() == 0 {
                    continue;
                }
                if let Err(error) = bus.publish_available_from_storage() {
                    tracing::warn!(
                        error = %error,
                        path = %bus.path.display(),
                        "sqlite mob event watch catch-up failed",
                    );
                }
            }
        }) {
            tracing::warn!(
                error = %error,
                path = %self.path.display(),
                "failed to start sqlite mob event watch thread",
            );
            return;
        }

        let callback_wake_tx = wake_tx.clone();
        let callback_parent = parent.clone();
        let mut watcher =
            match notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                match result {
                    Ok(event)
                        if sqlite_watch_event_relevant(
                            &event,
                            &callback_parent,
                            &watched_paths,
                        ) =>
                    {
                        let _ = callback_wake_tx.send(());
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(
                            error = %error,
                            "sqlite mob event filesystem watch reported an error",
                        );
                    }
                }
            }) {
                Ok(watcher) => watcher,
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        path = %self.path.display(),
                        "failed to create sqlite mob event filesystem watcher",
                    );
                    return;
                }
            };

        if let Err(error) = watcher.watch(&parent, RecursiveMode::NonRecursive) {
            tracing::warn!(
                error = %error,
                path = %parent.display(),
                "failed to watch sqlite mob event directory",
            );
            return;
        }

        *lock_unpoisoned(&self.watcher) = Some(watcher);
    }
}

fn sqlite_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn sqlite_watch_paths(path: &Path) -> Vec<PathBuf> {
    vec![
        path.to_path_buf(),
        sqlite_sidecar_path(path, "-wal"),
        sqlite_sidecar_path(path, "-shm"),
    ]
}

fn sqlite_watch_event_relevant(
    event: &notify::Event,
    parent: &Path,
    watched_paths: &[PathBuf],
) -> bool {
    if matches!(event.kind, notify::EventKind::Access(_)) {
        return false;
    }

    if event.paths.is_empty() {
        return true;
    }

    event.paths.iter().any(|path| {
        path == parent
            || watched_paths.iter().any(|watched| {
                path == watched
                    || (path.parent() == watched.parent()
                        && path.file_name() == watched.file_name())
            })
    })
}

/// Shared bundle that produces event/run/spec stores all pointing to the same db file.
#[derive(Debug, Clone)]
pub struct SqliteMobStores {
    path: PathBuf,
    event_bus: Arc<SqliteMobEventBus>,
    identity_store_instance_id: String,
}

impl SqliteMobStores {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MobError> {
        let path = path.as_ref().to_path_buf();
        // Validate the path works by opening and immediately closing.
        let conn = open_connection(&path)?;
        let identity_store_instance_id = initialize_identity_store_instance(&conn)?;
        let path = std::fs::canonicalize(&path).map_err(se)?;
        let event_bus = sqlite_event_bus_for_path(path.clone())?;
        Ok(Self {
            path,
            event_bus,
            identity_store_instance_id,
        })
    }

    pub fn event_store(&self) -> SqliteMobEventStore {
        SqliteMobEventStore {
            path: self.path.clone(),
            event_bus: Arc::clone(&self.event_bus),
        }
    }

    pub fn run_store(&self) -> SqliteMobRunStore {
        SqliteMobRunStore {
            path: self.path.clone(),
        }
    }

    pub fn spec_store(&self) -> SqliteMobSpecStore {
        SqliteMobSpecStore {
            path: self.path.clone(),
        }
    }

    pub fn runtime_metadata_store(&self) -> SqliteMobRuntimeMetadataStore {
        SqliteMobRuntimeMetadataStore {
            path: self.path.clone(),
        }
    }

    pub fn identity_store(&self) -> SqliteMobIdentityStore {
        SqliteMobIdentityStore {
            path: self.path.clone(),
            store_instance_id: self.identity_store_instance_id.clone(),
            clock: Arc::new(SystemMobIdentityStoreClock),
            event_bus: Some(Arc::clone(&self.event_bus)),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn identity_store_with_clock(
        &self,
        clock: Arc<dyn MobIdentityStoreClock>,
    ) -> SqliteMobIdentityStore {
        SqliteMobIdentityStore {
            path: self.path.clone(),
            store_instance_id: self.identity_store_instance_id.clone(),
            clock,
            event_bus: Some(Arc::clone(&self.event_bus)),
        }
    }

    pub fn identity_status_store(&self) -> SqliteMobIdentityStatusStore {
        SqliteMobIdentityStatusStore {
            path: self.path.clone(),
            store_instance_id: self.identity_store_instance_id.clone(),
        }
    }

    pub fn realm_profile_store(&self) -> SqliteRealmProfileStore {
        SqliteRealmProfileStore {
            path: self.path.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// SqliteMobIdentityStore / SqliteMobIdentityStatusStore
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SqliteMobIdentityStore {
    path: PathBuf,
    store_instance_id: String,
    clock: Arc<dyn MobIdentityStoreClock>,
    event_bus: Option<Arc<SqliteMobEventBus>>,
}

#[cfg(feature = "runtime-adapter")]
struct SqliteIdentityRuntimeWriteFence {
    path: PathBuf,
    store_instance_id: String,
    clock: Arc<dyn MobIdentityStoreClock>,
    permit: IdentityActuationPermit,
    expected_session: DesiredSessionTarget,
}

#[cfg(feature = "runtime-adapter")]
impl meerkat_runtime::RuntimeStoreWriteFence for SqliteIdentityRuntimeWriteFence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), meerkat_runtime::RuntimeStoreError> + '_>,
    ) -> Result<meerkat_runtime::RuntimeStoreWriteFenceOutcome, meerkat_runtime::RuntimeStoreError>
    {
        // Shared guard: while the exclusive maintenance fence is held the
        // probe backs off instead of writing through offline maintenance.
        let _guard = match meerkat_sqlite::OperationGuard::for_database(&self.path) {
            Ok(guard) => guard,
            Err(error) => {
                return Ok(meerkat_runtime::RuntimeStoreWriteFenceOutcome::Backoff {
                    reason: format!(
                        "identity runtime write fence deferred to the maintenance fence: {error}"
                    ),
                });
            }
        };
        // Maintenance profile: read-write on the existing file, zero busy
        // timeout for nonblocking admission, no pragma mutation.
        let mut conn = match meerkat_sqlite::open(
            &self.path,
            meerkat_sqlite::ConnectionProfile::Maintenance { write: true },
        ) {
            Ok(conn) => conn,
            Err(error) => {
                return Ok(meerkat_runtime::RuntimeStoreWriteFenceOutcome::Backoff {
                    reason: format!(
                        "identity runtime write fence could not open the store: {error}"
                    ),
                });
            }
        };
        let tx = match conn.transaction_with_behavior(TransactionBehavior::Immediate) {
            Ok(tx) => tx,
            Err(error) if meerkat_sqlite::is_busy_or_locked(&error) => {
                return Ok(meerkat_runtime::RuntimeStoreWriteFenceOutcome::Backoff {
                    reason: "identity runtime write fence authority guard is contended".to_string(),
                });
            }
            Err(error) => {
                return identity_runtime_fence_error(MobStoreError::WriteFailed(format!(
                    "identity runtime write fence could not begin its authority transaction: {error}"
                )));
            }
        };
        if let Err(error) = verify_identity_store_instance(&tx, &self.store_instance_id) {
            return identity_runtime_fence_error(error);
        }
        let intent = match query_identity_intent_observation(
            &tx,
            &self.permit.mob_id,
            &self.permit.identity,
        )
        .and_then(|observation| require_identity_authority(observation, "identity intent"))
        {
            Ok(intent) => intent,
            Err(error) => return identity_runtime_fence_error(error),
        };
        let lease =
            match query_identity_lease_observation(&tx, &self.permit.mob_id, &self.permit.identity)
                .and_then(|observation| require_identity_authority(observation, "identity lease"))
            {
                Ok(lease) => lease,
                Err(error) => return identity_runtime_fence_error(error),
            };
        let observed_at_ms = match self.clock.now_ms() {
            Ok(observed_at_ms) => observed_at_ms,
            Err(error) => return identity_runtime_fence_error(error),
        };
        if let Err(error) = validate_identity_runtime_write_authority(
            &self.permit,
            &self.expected_session,
            intent.as_ref(),
            lease.as_ref(),
            observed_at_ms,
        ) {
            return identity_runtime_fence_error(error);
        }
        operation()?;
        Ok(meerkat_runtime::RuntimeStoreWriteFenceOutcome::Applied)
    }
}

impl std::fmt::Debug for SqliteMobIdentityStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteMobIdentityStore")
            .field("path", &self.path)
            .field("store_instance_id", &self.store_instance_id)
            .field("clock", &"<dyn MobIdentityStoreClock>")
            .field("member_event_capability", &self.event_bus.is_some())
            .finish()
    }
}

impl SqliteMobIdentityStore {
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn with_clock(
        path: PathBuf,
        store_instance_id: String,
        clock: Arc<dyn MobIdentityStoreClock>,
    ) -> Self {
        Self {
            path,
            store_instance_id,
            clock,
            event_bus: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SqliteMobIdentityStatusStore {
    path: PathBuf,
    store_instance_id: String,
}

fn sqlite_identity_write_error(error: rusqlite::Error, context: &str) -> MobStoreError {
    if matches!(
        &error,
        rusqlite::Error::SqliteFailure(sqlite_error, _)
            if sqlite_error.code == ErrorCode::ConstraintViolation
                && matches!(
                    sqlite_error.extended_code,
                    rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
                        | rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                )
    ) {
        MobStoreError::CasConflict(format!("{context}: {error}"))
    } else {
        MobStoreError::WriteFailed(format!("{context}: {error}"))
    }
}

fn query_identity_scope_observation(
    conn: &Connection,
    mob_id: &MobId,
    scope_id: &IdentityDeclarationScopeId,
) -> Result<IdentityStoredObservation<IdentityDeclarationScopeHead>, MobStoreError> {
    let row: Option<(String, Vec<u8>)> = conn
        .query_row(
            "SELECT typeof(head_json), CAST(head_json AS BLOB)
             FROM mob_identity_declaration_scopes
             WHERE mob_id = ?1 AND scope_id = ?2",
            params![mob_id.as_str(), scope_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(se)?;
    let Some((storage_type, bytes)) = row else {
        return Ok(IdentityStoredObservation::Missing);
    };
    Ok(classify_identity_blob(
        &storage_type,
        &bytes,
        Some(IDENTITY_DECLARATION_SCOPE_SCHEMA_VERSION),
        IdentityDeclarationScopeHead::validate,
        |head| {
            if head.mob_id == *mob_id && head.scope_id == *scope_id {
                Ok(())
            } else {
                Err("identity declaration scope head does not match its physical key".to_string())
            }
        },
    ))
}

fn query_identity_intent_observation(
    conn: &Connection,
    mob_id: &MobId,
    identity: &AgentIdentity,
) -> Result<IdentityStoredObservation<IdentityIntentRecord>, MobStoreError> {
    let row: Option<(
        String,
        Option<Vec<u8>>,
        String,
        Option<Vec<u8>>,
        String,
        Option<Vec<u8>>,
        String,
        Vec<u8>,
    )> = conn
        .query_row(
            "SELECT typeof(declaration_scope), CAST(declaration_scope AS BLOB),
                    typeof(session_id), CAST(session_id AS BLOB),
                    typeof(lineage_id), CAST(lineage_id AS BLOB),
                    typeof(record_json), CAST(record_json AS BLOB)
             FROM mob_identity_intents
             WHERE mob_id = ?1 AND agent_identity = ?2",
            params![mob_id.as_str(), identity.as_str()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .optional()
        .map_err(se)?;
    let Some((
        declaration_scope_type,
        declaration_scope_bytes,
        session_id_type,
        session_id_bytes,
        lineage_id_type,
        lineage_id_bytes,
        storage_type,
        bytes,
    )) = row
    else {
        return Ok(IdentityStoredObservation::Missing);
    };
    let declaration_scope = decode_optional_identity_text(
        &declaration_scope_type,
        declaration_scope_bytes,
        "identity intent declaration_scope",
    );
    let session_id = decode_optional_identity_text(
        &session_id_type,
        session_id_bytes,
        "identity intent session_id",
    );
    let lineage_id = decode_optional_identity_text(
        &lineage_id_type,
        lineage_id_bytes,
        "identity intent lineage_id",
    );
    let projection_error = declaration_scope
        .as_ref()
        .err()
        .or_else(|| session_id.as_ref().err())
        .or_else(|| lineage_id.as_ref().err())
        .cloned();
    let declaration_scope = declaration_scope.unwrap_or(None);
    let session_id = session_id.unwrap_or(None);
    let lineage_id = lineage_id.unwrap_or(None);
    Ok(classify_identity_blob(
        &storage_type,
        &bytes,
        Some(IDENTITY_INTENT_SCHEMA_VERSION),
        IdentityIntentRecord::validate,
        |record| {
            if let Some(detail) = projection_error {
                return Err(detail);
            }
            identity_intent_physical_key_matches(
                record,
                mob_id,
                identity,
                declaration_scope.as_deref(),
                session_id.as_deref(),
                lineage_id.as_deref(),
            )
        },
    ))
}

struct LoadedIdentityLeaseRow {
    observation: IdentityStoredObservation<IdentityLeaseRecord>,
}

fn query_identity_lease_row(
    conn: &Connection,
    mob_id: &MobId,
    identity: &AgentIdentity,
) -> Result<Option<LoadedIdentityLeaseRow>, MobStoreError> {
    type LeaseSqlRow = (
        Vec<u8>,
        Option<String>,
        Option<String>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        String,
        String,
        Vec<u8>,
    );
    let row: Option<LeaseSqlRow> = conn
        .query_row(
            "SELECT epoch_highwater, active_holder_id, active_incarnation_id,
                    active_epoch, active_renewed_at_ms, active_expires_at_ms,
                    authority_digest, typeof(record_json), CAST(record_json AS BLOB)
             FROM mob_identity_leases
             WHERE mob_id = ?1 AND agent_identity = ?2",
            params![mob_id.as_str(), identity.as_str()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()
        .map_err(se)?;
    let Some((
        epoch_highwater,
        active_holder_id,
        active_incarnation_id,
        active_epoch,
        active_renewed_at_ms,
        active_expires_at_ms,
        authority_digest,
        storage_type,
        bytes,
    )) = row
    else {
        return Ok(None);
    };
    let evidence_digest = identity_raw_evidence_digest(&bytes);
    let authority = (|| -> Result<IdentityLeaseRecord, String> {
        let epoch_highwater = sql_key_to_u64(&epoch_highwater, "identity lease epoch highwater")
            .map_err(|error| error.to_string())?;
        let active = match (
            active_holder_id,
            active_incarnation_id,
            active_epoch,
            active_renewed_at_ms,
            active_expires_at_ms,
        ) {
            (None, None, None, None, None) => None,
            (Some(holder_id), Some(incarnation_id), Some(epoch), Some(renewed), Some(expires)) => {
                Some(IdentityLeaseClaim {
                    holder_id,
                    incarnation_id,
                    epoch: sql_key_to_u64(&epoch, "identity lease active epoch")
                        .map_err(|error| error.to_string())?,
                    renewed_at_ms: sql_key_to_u64(&renewed, "identity lease renewed time")
                        .map_err(|error| error.to_string())?,
                    expires_at_ms: sql_key_to_u64(&expires, "identity lease expiry time")
                        .map_err(|error| error.to_string())?,
                })
            }
            _ => {
                return Err(
                    "identity lease normalized active-claim columns are partially populated"
                        .to_string(),
                );
            }
        };
        let record = IdentityLeaseRecord {
            schema_version: IDENTITY_LEASE_SCHEMA_VERSION,
            epoch_highwater,
            active,
        };
        record.validate().map_err(|error| error.to_string())?;
        let expected_digest = identity_lease_authority_digest(mob_id, identity, &record)
            .map_err(|error| error.to_string())?;
        if authority_digest != expected_digest {
            return Err(
                "identity lease normalized authority seal does not match its physical mob/identity key"
                    .to_string(),
            );
        }
        Ok(record)
    })();
    let observation = match &authority {
        Ok(authority) => classify_identity_blob(
            &storage_type,
            &bytes,
            Some(IDENTITY_LEASE_SCHEMA_VERSION),
            IdentityLeaseRecord::validate,
            |record| {
                if record == authority {
                    Ok(())
                } else {
                    Err(
                        "identity lease projection differs from its sealed normalized authority"
                            .to_string(),
                    )
                }
            },
        ),
        Err(detail) => IdentityStoredObservation::Malformed {
            evidence_digest: evidence_digest.clone(),
            detail: detail.clone(),
        },
    };
    Ok(Some(LoadedIdentityLeaseRow { observation }))
}

fn query_identity_lease_observation(
    conn: &Connection,
    mob_id: &MobId,
    identity: &AgentIdentity,
) -> Result<IdentityStoredObservation<IdentityLeaseRecord>, MobStoreError> {
    Ok(query_identity_lease_row(conn, mob_id, identity)?
        .map_or(IdentityStoredObservation::Missing, |row| row.observation))
}

fn query_identity_lease_authority(
    conn: &Connection,
    mob_id: &MobId,
    identity: &AgentIdentity,
) -> Result<Option<IdentityLeaseRecord>, MobStoreError> {
    let Some(row) = query_identity_lease_row(conn, mob_id, identity)? else {
        return Ok(None);
    };
    match row.observation {
        IdentityStoredObservation::Valid(record) => Ok(Some(record)),
        IdentityStoredObservation::Unsupported {
            evidence_digest,
            detail,
        }
        | IdentityStoredObservation::Malformed {
            evidence_digest,
            detail,
        } => Err(identity_authority_blocked(
            Some(evidence_digest),
            format!("identity lease: {detail}"),
        )),
        IdentityStoredObservation::Missing => Err(identity_authority_blocked(
            None,
            "identity lease row disappeared while loading authority",
        )),
    }
}

enum IdentityStructuralEventLog {
    Valid(Vec<MobEvent>),
    Malformed {
        evidence_digest: String,
        detail: String,
    },
}

fn query_identity_structural_event_log(
    conn: &Connection,
    mob_id: &MobId,
) -> Result<IdentityStructuralEventLog, MobStoreError> {
    let mut statement = conn
        .prepare(
            "SELECT cursor, typeof(mob_id), CAST(mob_id AS BLOB),
                    typeof(event_json), event_json
             FROM mob_events
             WHERE mob_id = ?1
                OR mob_id IS NULL
                OR typeof(mob_id) <> 'text'
                OR length(mob_id) = 0
                OR trim(mob_id) <> mob_id
             ORDER BY cursor",
        )
        .map_err(se)?;
    let rows = statement
        .query_map(params![mob_id.as_str()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, rusqlite::types::Value>(4)?,
            ))
        })
        .map_err(se)?;
    let mut events = Vec::new();
    let mut resettable_malformed: Option<(String, String)> = None;
    let mut unordered_malformed: Option<(String, String)> = None;
    for row in rows {
        let (physical_cursor, route_type, route_bytes, storage_type, raw) = row.map_err(se)?;
        let raw_bytes = match raw {
            rusqlite::types::Value::Blob(bytes) => bytes,
            rusqlite::types::Value::Text(text) => text.into_bytes(),
            rusqlite::types::Value::Integer(value) => value.to_string().into_bytes(),
            rusqlite::types::Value::Real(value) => value.to_string().into_bytes(),
            rusqlite::types::Value::Null => Vec::new(),
        };
        let evidence_digest = identity_raw_evidence_digest(&raw_bytes);
        let physical_route = match (route_type.as_str(), route_bytes) {
            ("text", Some(bytes)) => match String::from_utf8(bytes) {
                Ok(route) if !route.is_empty() && route.trim() == route => Ok(Some(route)),
                Ok(_) => Err("mob event row has a non-canonical physical mob route".to_string()),
                Err(error) => Err(format!(
                    "mob event physical mob route is not UTF-8: {error}"
                )),
            },
            ("null", None) => Ok(None),
            (observed, _) => Err(format!(
                "mob event physical mob route uses SQLite storage class '{observed}', expected text or null"
            )),
        };
        if physical_route
            .as_ref()
            .ok()
            .and_then(|route| route.as_deref())
            .is_some_and(|route| route != mob_id.as_str())
        {
            continue;
        }
        if !matches!(physical_route.as_ref(), Ok(Some(_)))
            && stored_event_envelope_mob_id(&raw_bytes)
                .as_deref()
                .is_some_and(|route| route != mob_id.as_str())
        {
            continue;
        }
        let physical_route = match physical_route {
            Ok(route) => route,
            Err(detail) => {
                resettable_malformed.get_or_insert_with(|| (evidence_digest.clone(), detail));
                None
            }
        };

        let cursor = match u64::try_from(physical_cursor)
            .ok()
            .filter(|cursor| *cursor > 0)
        {
            Some(cursor) => cursor,
            None => {
                unordered_malformed.get_or_insert_with(|| {
                    (
                        evidence_digest,
                        format!("mob event row has invalid physical cursor {physical_cursor}"),
                    )
                });
                continue;
            }
        };
        if storage_type != "blob" {
            resettable_malformed.get_or_insert_with(|| {
                (
                    evidence_digest,
                    format!(
                        "mob event row uses SQLite storage class '{storage_type}', expected blob"
                    ),
                )
            });
            continue;
        }
        let decoded = match decode_stored_mob_event(&raw_bytes) {
            Ok(decoded) => decoded,
            Err(error) => {
                resettable_malformed.get_or_insert_with(|| {
                    (
                        evidence_digest,
                        format!("mob event row is malformed or unsupported: {error}"),
                    )
                });
                continue;
            }
        };
        if decoded.mob_id != *mob_id {
            if physical_route.is_some() {
                resettable_malformed.get_or_insert_with(|| {
                    (
                        evidence_digest,
                        format!(
                            "mob event physical mob route '{}' differs from payload mob '{}'",
                            mob_id, decoded.mob_id
                        ),
                    )
                });
            }
            continue;
        }
        if decoded.cursor != cursor {
            resettable_malformed.get_or_insert_with(|| {
                (
                    evidence_digest,
                    format!(
                        "mob event row cursor mismatch: physical={cursor} payload={}",
                        decoded.cursor
                    ),
                )
            });
            continue;
        }
        if matches!(&decoded.kind, MobEventKind::MobReset) {
            // A valid, physically ordered reset is a content epoch boundary.
            // Earlier target-routed or unattributable structural garbage can
            // no longer affect the post-reset realization.
            events.clear();
            resettable_malformed = None;
        }
        events.push(decoded);
    }
    if let Some((evidence_digest, detail)) = unordered_malformed {
        return Ok(IdentityStructuralEventLog::Malformed {
            evidence_digest,
            detail,
        });
    }
    if let Some((evidence_digest, detail)) = resettable_malformed {
        return Ok(IdentityStructuralEventLog::Malformed {
            evidence_digest,
            detail,
        });
    }
    Ok(IdentityStructuralEventLog::Valid(events))
}

fn query_identity_member_target_state(
    conn: &Connection,
    mob_id: &MobId,
    identity: &AgentIdentity,
) -> Result<IdentityMemberTargetState, MobStoreError> {
    match query_identity_structural_event_log(conn, mob_id)? {
        IdentityStructuralEventLog::Valid(events) => {
            Ok(identity_member_target_state(&events, mob_id, identity))
        }
        IdentityStructuralEventLog::Malformed {
            evidence_digest,
            detail,
        } => Ok(IdentityMemberTargetState {
            observation: IdentityMemberTargetObservation::Malformed {
                observed_version: Some(evidence_digest),
                detail,
            },
            exact_current_spawn: None,
        }),
    }
}

fn query_identity_wiring_target_state(
    conn: &Connection,
    mob_id: &MobId,
    identity: &AgentIdentity,
) -> Result<IdentityWiringTargetState, MobStoreError> {
    match query_identity_structural_event_log(conn, mob_id)? {
        IdentityStructuralEventLog::Valid(events) => {
            Ok(identity_wiring_target_state(&events, mob_id, identity))
        }
        IdentityStructuralEventLog::Malformed {
            evidence_digest,
            detail,
        } => Ok(IdentityWiringTargetState {
            observation: IdentityWiringTargetObservation::Malformed {
                observed_version: Some(evidence_digest),
                detail,
            },
        }),
    }
}

fn query_identity_receipt_observation(
    conn: &Connection,
    mob_id: &MobId,
    subject: &IdentityOperationSubject,
    slot: &IdentityOperationSlot,
) -> Result<IdentityStoredObservation<IdentityOperationReceipt>, MobStoreError> {
    let (subject_kind, subject_id, slot_kind, slot_key_version, slot_digest) =
        identity_receipt_sql_key(subject, slot)?;
    let row: Option<(
        String,
        Vec<u8>,
        String,
        Vec<u8>,
        Option<String>,
        Option<String>,
        String,
        String,
        Vec<u8>,
    )> = conn
        .query_row(
            "SELECT typeof(subject_json), CAST(subject_json AS BLOB),
                    typeof(slot_json), CAST(slot_json AS BLOB),
                    subject_scope_id, subject_identity, effect_kind,
                    typeof(receipt_json), CAST(receipt_json AS BLOB)
             FROM mob_identity_operation_receipts
             WHERE mob_id = ?1
               AND subject_kind = ?2 AND subject_id = ?3
               AND slot_kind = ?4 AND slot_key_version = ?5 AND slot_digest = ?6",
            params![
                mob_id.as_str(),
                subject_kind,
                subject_id,
                slot_kind,
                slot_key_version,
                slot_digest,
            ],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()
        .map_err(se)?;
    let Some((
        subject_type,
        stored_subject_key,
        slot_type,
        stored_slot_key,
        subject_scope_id,
        subject_identity,
        effect_kind,
        receipt_type,
        receipt_bytes,
    )) = row
    else {
        return Ok(IdentityStoredObservation::Missing);
    };
    if subject_type != "blob"
        || slot_type != "blob"
        || stored_subject_key != serde_json::to_vec(subject).map_err(se)?
        || stored_slot_key != serde_json::to_vec(slot).map_err(se)?
    {
        return Ok(IdentityStoredObservation::Malformed {
            evidence_digest: identity_raw_evidence_digest(&receipt_bytes),
            detail: "identity operation receipt key is not canonical blob JSON".to_string(),
        });
    }
    let (expected_scope, expected_identity) = identity_receipt_subject_projection(subject);
    Ok(classify_identity_blob(
        &receipt_type,
        &receipt_bytes,
        Some(IDENTITY_OPERATION_RECEIPT_SCHEMA_VERSION),
        IdentityOperationReceipt::validate,
        |receipt| {
            if receipt.mob_id != *mob_id
                || receipt.subject != *subject
                || receipt.slot != *slot
                || receipt.effect_kind != slot.kind()
                || subject_scope_id.as_deref() != expected_scope
                || subject_identity.as_deref() != expected_identity
                || effect_kind != identity_operation_kind_key(receipt.effect_kind)
            {
                Err("identity operation receipt does not match its physical key".to_string())
            } else {
                Ok(())
            }
        },
    ))
}

fn query_identity_status_observation(
    conn: &Connection,
    mob_id: &MobId,
    identity: &AgentIdentity,
) -> Result<IdentityStoredObservation<IdentityConvergenceStatus>, MobStoreError> {
    let row: Option<(String, Vec<u8>)> = conn
        .query_row(
            "SELECT typeof(status_json), CAST(status_json AS BLOB)
             FROM mob_identity_statuses
             WHERE mob_id = ?1 AND agent_identity = ?2",
            params![mob_id.as_str(), identity.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(se)?;
    let Some((storage_type, bytes)) = row else {
        return Ok(IdentityStoredObservation::Missing);
    };
    Ok(classify_identity_blob(
        &storage_type,
        &bytes,
        None,
        |_| Ok(()),
        |status: &IdentityConvergenceStatus| {
            if status.identity == *identity {
                Ok(())
            } else {
                Err("identity convergence status does not match its physical key".to_string())
            }
        },
    ))
}

fn write_identity_scope_head(
    tx: &Transaction<'_>,
    head: &IdentityDeclarationScopeHead,
) -> Result<(), MobStoreError> {
    tx.execute(
        "INSERT INTO mob_identity_declaration_scopes (mob_id, scope_id, head_json)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(mob_id, scope_id) DO UPDATE SET head_json = excluded.head_json",
        params![
            head.mob_id.as_str(),
            head.scope_id.as_str(),
            encode_json(head)?
        ],
    )
    .map_err(|error| sqlite_identity_write_error(error, "write identity scope head"))?;
    Ok(())
}

fn write_identity_intent(
    tx: &Transaction<'_>,
    mob_id: &MobId,
    identity: &AgentIdentity,
    record: &IdentityIntentRecord,
) -> Result<(), MobStoreError> {
    record.validate().map_err(identity_contract_error)?;
    if record.mob_id != *mob_id || record.intent.identity() != identity {
        return Err(identity_authority_blocked(
            Some(identity_raw_evidence_digest(&encode_json(record)?)),
            "identity intent write does not match its physical mob/identity key",
        ));
    }
    let (declaration_scope, session_id, lineage_id) = identity_intent_projection(record);
    tx.execute(
        "INSERT INTO mob_identity_intents
             (mob_id, agent_identity, declaration_scope, session_id, lineage_id, record_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(mob_id, agent_identity) DO UPDATE SET
             declaration_scope = excluded.declaration_scope,
             session_id = excluded.session_id,
             lineage_id = excluded.lineage_id,
             record_json = excluded.record_json",
        params![
            mob_id.as_str(),
            identity.as_str(),
            declaration_scope,
            session_id,
            lineage_id,
            encode_json(record)?,
        ],
    )
    .map_err(|error| sqlite_identity_write_error(error, "write identity intent"))?;
    Ok(())
}

fn write_identity_lease(
    tx: &Transaction<'_>,
    mob_id: &MobId,
    identity: &AgentIdentity,
    record: &IdentityLeaseRecord,
) -> Result<(), MobStoreError> {
    let record_json = encode_json(record)?;
    let authority_digest = identity_lease_authority_digest(mob_id, identity, record)?;
    let (active_holder_id, active_incarnation_id, active_epoch, active_renewed, active_expires) =
        record
            .active
            .as_ref()
            .map_or((None, None, None, None, None), |active| {
                (
                    Some(active.holder_id.as_str()),
                    Some(active.incarnation_id.as_str()),
                    Some(u64_to_sql_key(active.epoch)),
                    Some(u64_to_sql_key(active.renewed_at_ms)),
                    Some(u64_to_sql_key(active.expires_at_ms)),
                )
            });
    tx.execute(
        "INSERT INTO mob_identity_leases
             (mob_id, agent_identity, epoch_highwater, active_holder_id,
              active_incarnation_id, active_epoch, active_renewed_at_ms,
              active_expires_at_ms, authority_digest, record_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(mob_id, agent_identity) DO UPDATE SET
             epoch_highwater = excluded.epoch_highwater,
             active_holder_id = excluded.active_holder_id,
             active_incarnation_id = excluded.active_incarnation_id,
             active_epoch = excluded.active_epoch,
             active_renewed_at_ms = excluded.active_renewed_at_ms,
             active_expires_at_ms = excluded.active_expires_at_ms,
             authority_digest = excluded.authority_digest,
             record_json = excluded.record_json",
        params![
            mob_id.as_str(),
            identity.as_str(),
            u64_to_sql_key(record.epoch_highwater),
            active_holder_id,
            active_incarnation_id,
            active_epoch,
            active_renewed,
            active_expires,
            authority_digest,
            record_json,
        ],
    )
    .map_err(|error| sqlite_identity_write_error(error, "write identity lease"))?;
    Ok(())
}

fn insert_identity_receipt(
    tx: &Transaction<'_>,
    receipt: &IdentityOperationReceipt,
) -> Result<(), MobStoreError> {
    let (_, _, subject_key, slot_key) = identity_receipt_keys(&receipt.subject, &receipt.slot)?;
    let (subject_kind, subject_id, slot_kind, slot_key_version, slot_digest) =
        identity_receipt_sql_key(&receipt.subject, &receipt.slot)?;
    let (subject_scope_id, subject_identity) =
        identity_receipt_subject_projection(&receipt.subject);
    tx.execute(
        "INSERT INTO mob_identity_operation_receipts
             (mob_id, subject_kind, subject_id, slot_kind, slot_key_version, slot_digest,
              subject_json, slot_json, subject_scope_id, subject_identity, effect_kind,
              receipt_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            receipt.mob_id.as_str(),
            subject_kind,
            subject_id,
            slot_kind,
            slot_key_version,
            slot_digest,
            subject_key,
            slot_key,
            subject_scope_id,
            subject_identity,
            identity_operation_kind_key(receipt.effect_kind),
            encode_json(receipt)?,
        ],
    )
    .map_err(|error| sqlite_identity_write_error(error, "insert identity operation receipt"))?;
    Ok(())
}

fn load_identity_authority_state_for_manifest(
    tx: &Transaction<'_>,
    mob_id: &MobId,
    plan: &IdentityDeclarationApplyPlan,
) -> Result<IdentityAuthorityState, MobStoreError> {
    let mut state = IdentityAuthorityState::default();
    if let Some(head) = require_identity_authority(
        query_identity_scope_observation(tx, mob_id, plan.scope_id())?,
        "identity declaration scope head",
    )? {
        state
            .scope_heads
            .insert((mob_id.clone(), plan.scope_id().clone()), head);
    }

    let declared_identities = plan.members.keys().cloned().collect::<BTreeSet<_>>();
    let candidate_session_ids = plan
        .members
        .values()
        .map(|member| member.candidate_session_target().session_id.to_string())
        .chain(
            plan.legacy_imports()
                .values()
                .map(|legacy_import| legacy_import.session().session_id.to_string()),
        )
        .collect::<BTreeSet<_>>();
    let candidate_lineage_ids = plan
        .members
        .values()
        .map(|member| {
            member
                .candidate_session_target()
                .lineage_id
                .as_str()
                .to_string()
        })
        .chain(
            plan.legacy_imports()
                .values()
                .map(|legacy_import| legacy_import.session().lineage_id.as_str().to_string()),
        )
        .collect::<BTreeSet<_>>();
    let mut stmt = tx
        .prepare(
            "SELECT typeof(agent_identity), CAST(agent_identity AS BLOB),
                    typeof(declaration_scope), CAST(declaration_scope AS BLOB),
                    typeof(session_id), CAST(session_id AS BLOB),
                    typeof(lineage_id), CAST(lineage_id AS BLOB),
                    typeof(record_json), CAST(record_json AS BLOB)
             FROM mob_identity_intents
             WHERE mob_id = ?1",
        )
        .map_err(se)?;
    let rows = stmt
        .query_map(params![mob_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<Vec<u8>>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<Vec<u8>>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<Vec<u8>>>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, Vec<u8>>(9)?,
            ))
        })
        .map_err(se)?;
    for row in rows {
        let (
            identity_type,
            identity_bytes,
            declaration_scope_type,
            declaration_scope_bytes,
            session_id_type,
            session_id_bytes,
            lineage_id_type,
            lineage_id_bytes,
            storage_type,
            bytes,
        ) = row.map_err(se)?;
        let identity_key =
            decode_required_identity_text(&identity_type, identity_bytes.clone(), "agent_identity");
        let identity_key_error = identity_key.as_ref().err().cloned();
        let identity = AgentIdentity::from(identity_key.unwrap_or_else(|_| {
            format!(
                "malformed-physical-identity:{}",
                identity_raw_evidence_digest(&identity_bytes)
            )
        }));
        let declaration_scope = decode_optional_identity_text(
            &declaration_scope_type,
            declaration_scope_bytes,
            "identity intent declaration_scope",
        );
        let session_id = decode_optional_identity_text(
            &session_id_type,
            session_id_bytes,
            "identity intent session_id",
        );
        let lineage_id = decode_optional_identity_text(
            &lineage_id_type,
            lineage_id_bytes,
            "identity intent lineage_id",
        );
        let projection_error = identity_key_error
            .or_else(|| declaration_scope.as_ref().err().cloned())
            .or_else(|| session_id.as_ref().err().cloned())
            .or_else(|| lineage_id.as_ref().err().cloned());
        let declaration_scope = declaration_scope.unwrap_or(None);
        let session_id = session_id.unwrap_or(None);
        let lineage_id = lineage_id.unwrap_or(None);
        let observation = classify_identity_blob(
            &storage_type,
            &bytes,
            Some(IDENTITY_INTENT_SCHEMA_VERSION),
            IdentityIntentRecord::validate,
            |record| {
                if let Some(detail) = projection_error {
                    return Err(detail);
                }
                identity_intent_physical_key_matches(
                    record,
                    mob_id,
                    &identity,
                    declaration_scope.as_deref(),
                    session_id.as_deref(),
                    lineage_id.as_deref(),
                )
            },
        );
        match observation {
            IdentityStoredObservation::Valid(record) => {
                state.intents.insert((mob_id.clone(), identity), record);
            }
            IdentityStoredObservation::Unsupported {
                evidence_digest,
                detail,
            }
            | IdentityStoredObservation::Malformed {
                evidence_digest,
                detail,
            } => {
                let relevant = declared_identities.contains(&identity)
                    || declaration_scope.as_deref() == Some(plan.scope_id().as_str())
                    || session_id
                        .as_ref()
                        .is_some_and(|value| candidate_session_ids.contains(value))
                    || lineage_id
                        .as_ref()
                        .is_some_and(|value| candidate_lineage_ids.contains(value));
                if relevant {
                    return Err(identity_authority_blocked(
                        Some(evidence_digest),
                        format!("relevant identity intent row '{identity}' is unsafe: {detail}"),
                    ));
                }
            }
            IdentityStoredObservation::Missing => unreachable!("query row cannot be missing"),
        }
    }
    drop(stmt);

    let relevant_identities = state
        .intents
        .iter()
        .filter_map(|((stored_mob_id, identity), record)| {
            (stored_mob_id == mob_id
                && (declared_identities.contains(identity)
                    || record.declaration_scope.as_ref() == Some(plan.scope_id())))
            .then_some(identity.clone())
        })
        .chain(declared_identities.iter().cloned())
        .collect::<BTreeSet<_>>();
    let mut stmt = tx
        .prepare(
            "SELECT subject_kind, subject_id, slot_kind, slot_key_version, slot_digest,
                    typeof(subject_json), CAST(subject_json AS BLOB),
                    typeof(slot_json), CAST(slot_json AS BLOB),
                    subject_scope_id, subject_identity, effect_kind,
                    typeof(receipt_json), CAST(receipt_json AS BLOB)
             FROM mob_identity_operation_receipts
             WHERE mob_id = ?1",
        )
        .map_err(se)?;
    let rows = stmt
        .query_map(params![mob_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Vec<u8>>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Vec<u8>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, Vec<u8>>(13)?,
            ))
        })
        .map_err(se)?;
    for row in rows {
        let (
            subject_kind,
            subject_id,
            slot_kind,
            slot_key_version,
            slot_digest,
            subject_type,
            subject_bytes,
            slot_type,
            slot_bytes,
            subject_scope_id,
            subject_identity,
            effect_kind,
            receipt_type,
            receipt_bytes,
        ) = row.map_err(se)?;
        let relevant = (subject_kind == "declaration_scope"
            && subject_id == plan.scope_id().as_str())
            || (subject_kind == "identity"
                && relevant_identities.contains(&AgentIdentity::from(subject_id.as_str())))
            || subject_scope_id.as_deref() == Some(plan.scope_id().as_str())
            || subject_identity.as_ref().is_some_and(|value| {
                relevant_identities.contains(&AgentIdentity::from(value.as_str()))
            });
        let subject = serde_json::from_slice::<IdentityOperationSubject>(&subject_bytes);
        let slot = serde_json::from_slice::<IdentityOperationSlot>(&slot_bytes);
        let (Ok(subject), Ok(slot)) = (subject, slot) else {
            if relevant {
                return Err(identity_authority_blocked(
                    Some(identity_raw_evidence_digest(&receipt_bytes)),
                    "relevant identity receipt has a malformed physical subject/slot key",
                ));
            }
            continue;
        };
        let canonical_subject = serde_json::to_vec(&subject).map_err(se)?;
        let canonical_slot = serde_json::to_vec(&slot).map_err(se)?;
        let (expected_scope, expected_identity) = identity_receipt_subject_projection(&subject);
        let (
            expected_subject_kind,
            expected_subject_id,
            expected_slot_kind,
            expected_slot_key_version,
            expected_slot_digest,
        ) = identity_receipt_sql_key(&subject, &slot)?;
        let observation = if subject_type != "blob"
            || slot_type != "blob"
            || subject_bytes != canonical_subject
            || slot_bytes != canonical_slot
            || subject_kind != expected_subject_kind
            || subject_id != expected_subject_id
            || slot_kind != expected_slot_kind
            || slot_key_version != expected_slot_key_version
            || slot_digest != expected_slot_digest
        {
            IdentityStoredObservation::Malformed {
                evidence_digest: identity_raw_evidence_digest(&receipt_bytes),
                detail: "identity receipt has a noncanonical physical subject/slot key".to_string(),
            }
        } else {
            classify_identity_blob(
                &receipt_type,
                &receipt_bytes,
                Some(IDENTITY_OPERATION_RECEIPT_SCHEMA_VERSION),
                IdentityOperationReceipt::validate,
                |receipt| {
                    if receipt.mob_id != *mob_id
                        || receipt.subject != subject
                        || receipt.slot != slot
                        || receipt.effect_kind != slot.kind()
                        || subject_scope_id.as_deref() != expected_scope
                        || subject_identity.as_deref() != expected_identity
                        || effect_kind != identity_operation_kind_key(receipt.effect_kind)
                    {
                        Err("identity receipt does not match its physical key".to_string())
                    } else {
                        Ok(())
                    }
                },
            )
        };
        match observation {
            IdentityStoredObservation::Valid(receipt) => {
                let (subject_key, slot_key, _, _) =
                    identity_receipt_keys(&receipt.subject, &receipt.slot)?;
                state
                    .receipts
                    .insert((mob_id.clone(), subject_key, slot_key), receipt);
            }
            IdentityStoredObservation::Unsupported {
                evidence_digest,
                detail,
            }
            | IdentityStoredObservation::Malformed {
                evidence_digest,
                detail,
            } if relevant => {
                return Err(identity_authority_blocked(
                    Some(evidence_digest),
                    format!("relevant identity operation receipt is unsafe: {detail}"),
                ));
            }
            IdentityStoredObservation::Unsupported { .. }
            | IdentityStoredObservation::Malformed { .. } => {}
            IdentityStoredObservation::Missing => unreachable!("query row cannot be missing"),
        }
    }

    let manifest_receipt_already_exists = state.receipts.values().any(|receipt| {
        matches!(
            (&receipt.subject, &receipt.slot),
            (
                IdentityOperationSubject::DeclarationScope { scope_id },
                IdentityOperationSlot::ApplyDeclarationManifest {
                    scope_id: slot_scope,
                    mutation_id,
                },
            ) if scope_id == plan.scope_id()
                && slot_scope == plan.scope_id()
                && mutation_id == plan.operation_id()
        )
    });
    if !manifest_receipt_already_exists {
        for identity in plan.legacy_imports().keys() {
            if let Some(lease) = query_identity_lease_authority(tx, mob_id, identity)? {
                state
                    .leases
                    .insert((mob_id.clone(), identity.clone()), lease);
            }
        }
    }
    Ok(state)
}

#[async_trait]
impl MobIdentityStore for SqliteMobIdentityStore {
    #[cfg(feature = "runtime-adapter")]
    fn prepare_runtime_write_fence(
        &self,
        permit: IdentityActuationPermit,
        expected_session: DesiredSessionTarget,
        observed: &meerkat_runtime::RuntimeSessionLifecycleObservation,
    ) -> Result<Arc<dyn meerkat_runtime::RuntimeStoreWriteFence>, MobStoreError> {
        validate_identity_runtime_target_binding(&permit, &expected_session, observed)?;
        Ok(Arc::new(SqliteIdentityRuntimeWriteFence {
            path: self.path.clone(),
            store_instance_id: self.store_instance_id.clone(),
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
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let mob_id = mob_id.clone();
        let scope_id = scope_id.clone();
        run_sqlite_task(move || {
            let conn = open_existing_identity_read_connection(&path, &store_instance_id)?;
            query_identity_scope_observation(&conn, &mob_id, &scope_id)
        })
        .await
    }

    async fn observe_identity_intent(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityStoredObservation<IdentityIntentRecord>, MobStoreError> {
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let mob_id = mob_id.clone();
        let identity = identity.clone();
        run_sqlite_task(move || {
            let conn = open_existing_identity_read_connection(&path, &store_instance_id)?;
            query_identity_intent_observation(&conn, &mob_id, &identity)
        })
        .await
    }

    async fn list_identity_intents(
        &self,
        mob_id: &MobId,
    ) -> Result<
        BTreeMap<AgentIdentity, IdentityStoredObservation<IdentityIntentRecord>>,
        MobStoreError,
    > {
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_existing_identity_read_connection(&path, &store_instance_id)?;
            let mut stmt = conn
                .prepare(
                    "SELECT typeof(agent_identity), CAST(agent_identity AS BLOB),
                            typeof(declaration_scope), CAST(declaration_scope AS BLOB),
                            typeof(session_id), CAST(session_id AS BLOB),
                            typeof(lineage_id), CAST(lineage_id AS BLOB),
                            typeof(record_json), CAST(record_json AS BLOB)
                     FROM mob_identity_intents
                     WHERE mob_id = ?1
                     ORDER BY agent_identity",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<Vec<u8>>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<Vec<u8>>>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<Vec<u8>>>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, Vec<u8>>(9)?,
                    ))
                })
                .map_err(se)?;
            let mut observations = BTreeMap::new();
            for row in rows {
                let (
                    identity_type,
                    identity_bytes,
                    declaration_scope_type,
                    declaration_scope_bytes,
                    session_id_type,
                    session_id_bytes,
                    lineage_id_type,
                    lineage_id_bytes,
                    storage_type,
                    bytes,
                ) = row.map_err(se)?;
                let identity_key = decode_required_identity_text(
                    &identity_type,
                    identity_bytes.clone(),
                    "agent_identity",
                );
                let identity_key_error = identity_key.as_ref().err().cloned();
                let identity = AgentIdentity::from(identity_key.unwrap_or_else(|_| {
                    format!(
                        "malformed-physical-identity:{}",
                        identity_raw_evidence_digest(&identity_bytes)
                    )
                }));
                let declaration_scope = decode_optional_identity_text(
                    &declaration_scope_type,
                    declaration_scope_bytes,
                    "identity intent declaration_scope",
                );
                let session_id = decode_optional_identity_text(
                    &session_id_type,
                    session_id_bytes,
                    "identity intent session_id",
                );
                let lineage_id = decode_optional_identity_text(
                    &lineage_id_type,
                    lineage_id_bytes,
                    "identity intent lineage_id",
                );
                let projection_error = identity_key_error
                    .or_else(|| declaration_scope.as_ref().err().cloned())
                    .or_else(|| session_id.as_ref().err().cloned())
                    .or_else(|| lineage_id.as_ref().err().cloned());
                let declaration_scope = declaration_scope.unwrap_or(None);
                let session_id = session_id.unwrap_or(None);
                let lineage_id = lineage_id.unwrap_or(None);
                let observation = classify_identity_blob(
                    &storage_type,
                    &bytes,
                    Some(IDENTITY_INTENT_SCHEMA_VERSION),
                    IdentityIntentRecord::validate,
                    |record| {
                        if let Some(detail) = projection_error {
                            return Err(detail);
                        }
                        identity_intent_physical_key_matches(
                            record,
                            &mob_id,
                            &identity,
                            declaration_scope.as_deref(),
                            session_id.as_deref(),
                            lineage_id.as_deref(),
                        )
                    },
                );
                observations.insert(identity, observation);
            }
            Ok(observations)
        })
        .await
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
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let mob_id = mob_id.clone();
        let scope_id = scope_id.clone();
        let operation_id = operation_id.clone();
        let request_digest = request_digest.to_string();
        run_sqlite_task(move || {
            let mut conn = open_existing_identity_read_connection(&path, &store_instance_id)?;
            let tx = conn.transaction().map_err(se)?;
            let subject = IdentityOperationSubject::DeclarationScope {
                scope_id: scope_id.clone(),
            };
            let slot = IdentityOperationSlot::ApplyDeclarationManifest {
                scope_id: scope_id.clone(),
                mutation_id: operation_id.clone(),
            };
            let Some(receipt) = require_identity_authority(
                query_identity_receipt_observation(&tx, &mob_id, &subject, &slot)?,
                "identity declaration replay receipt",
            )?
            else {
                tx.commit().map_err(se)?;
                return Ok(None);
            };
            let IdentityOperationReceiptPayload::ApplyDeclarationManifest { outcome } =
                receipt.payload
            else {
                return Err(identity_authority_blocked(
                    None,
                    "identity declaration replay slot contains a non-manifest receipt",
                ));
            };
            if outcome.request_digest != request_digest {
                return Err(MobStoreError::CasConflict(format!(
                    "identity declaration operation '{operation_id}' was reused with different content"
                )));
            }

            let mut state = IdentityAuthorityState::default();
            if let Some(head) = require_identity_authority(
                query_identity_scope_observation(&tx, &mob_id, &scope_id)?,
                "identity declaration replay scope head",
            )? {
                state
                    .scope_heads
                    .insert((mob_id.clone(), scope_id.clone()), head);
            }
            for identity in outcome.identities.keys() {
                if let Some(record) = require_identity_authority(
                    query_identity_intent_observation(&tx, &mob_id, identity)?,
                    "identity declaration replay intent",
                )? {
                    state
                        .intents
                        .insert((mob_id.clone(), identity.clone()), record);
                }
            }
            validate_manifest_replay_state(&state, &mob_id, &outcome)?;
            tx.commit().map_err(se)?;
            Ok(Some(outcome))
        })
        .await
    }

    async fn apply_identity_declaration(
        &self,
        mob_id: &MobId,
        plan: &IdentityDeclarationApplyPlan,
    ) -> Result<IdentityDeclarationManifestApplyOutcome, MobStoreError> {
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let mob_id = mob_id.clone();
        let plan = plan.clone();
        run_sqlite_task(move || {
            let mut conn = open_existing_identity_write_connection(&path, &store_instance_id)?;
            let tx = begin_identity_immediate(&mut conn, &store_instance_id)?;
            let mut state = load_identity_authority_state_for_manifest(&tx, &mob_id, &plan)?;
            let prior_scope_heads = state.scope_heads.clone();
            let prior_intents = state.intents.clone();
            let prior_leases = state.leases.clone();
            let prior_receipts = state.receipts.clone();
            let outcome = apply_identity_declaration_locked(&mut state, &mob_id, &plan)?;

            for ((stored_mob_id, identity), record) in &state.intents {
                if stored_mob_id == &mob_id
                    && prior_intents.get(&(stored_mob_id.clone(), identity.clone())) != Some(record)
                {
                    write_identity_intent(&tx, &mob_id, identity, record)?;
                }
            }
            for ((stored_mob_id, identity), record) in &state.leases {
                if stored_mob_id == &mob_id
                    && prior_leases.get(&(stored_mob_id.clone(), identity.clone())) != Some(record)
                {
                    write_identity_lease(&tx, &mob_id, identity, record)?;
                }
            }
            for ((stored_mob_id, _), head) in &state.scope_heads {
                if stored_mob_id == &mob_id
                    && prior_scope_heads.get(&(stored_mob_id.clone(), head.scope_id.clone()))
                        != Some(head)
                {
                    write_identity_scope_head(&tx, head)?;
                }
            }
            for (key, receipt) in &state.receipts {
                if key.0 == mob_id && prior_receipts.get(key) != Some(receipt) {
                    insert_identity_receipt(&tx, receipt)?;
                }
            }
            tx.commit().map_err(se)?;
            Ok(outcome)
        })
        .await
    }

    async fn observe_identity_lease(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityStoredObservation<IdentityLeaseRecord>, MobStoreError> {
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let mob_id = mob_id.clone();
        let identity = identity.clone();
        run_sqlite_task(move || {
            let conn = open_existing_identity_read_connection(&path, &store_instance_id)?;
            query_identity_lease_observation(&conn, &mob_id, &identity)
        })
        .await
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
                IdentityIntentError::InvalidLeaseLifetime,
            ));
        }
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let clock = Arc::clone(&self.clock);
        let mob_id = mob_id.clone();
        let identity = identity.clone();
        let holder_id = holder_id.to_string();
        let incarnation_id = incarnation_id.to_string();
        run_sqlite_task(move || {
            let mut conn = open_existing_identity_write_connection(&path, &store_instance_id)?;
            let tx = begin_identity_immediate(&mut conn, &store_instance_id)?;
            // Time is sampled only after the writer lock is held, so a queued
            // claimant cannot author a lease that was already stale at commit.
            let observed_at_ms = clock.now_ms()?;
            let expires_at_ms = observed_at_ms.checked_add(ttl_ms).ok_or_else(|| {
                MobStoreError::WriteFailed("identity lease expiry overflow".to_string())
            })?;
            let current = query_identity_lease_authority(&tx, &mob_id, &identity)?;
            if let Some(record) = &current
                && let Some(active) = &record.active
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
                    holder_id,
                    incarnation_id,
                    epoch: active.epoch,
                    renewed_at_ms: observed_at_ms,
                    expires_at_ms,
                };
                let record = IdentityLeaseRecord {
                    schema_version: IDENTITY_LEASE_SCHEMA_VERSION,
                    epoch_highwater: record.epoch_highwater,
                    active: Some(claim.clone()),
                };
                record.validate().map_err(identity_contract_error)?;
                write_identity_lease(&tx, &mob_id, &identity, &record)?;
                tx.commit().map_err(se)?;
                return Ok(IdentityLeaseClaimOutcome::Renewed(claim));
            }

            let epoch = current
                .as_ref()
                .map_or(0, |record| record.epoch_highwater)
                .checked_add(1)
                .ok_or_else(|| MobStoreError::IdentityCounterExhausted {
                    counter: "identity lease epoch".to_string(),
                })?;
            let claim = IdentityLeaseClaim {
                holder_id,
                incarnation_id,
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
            write_identity_lease(&tx, &mob_id, &identity, &record)?;
            tx.commit().map_err(se)?;
            Ok(IdentityLeaseClaimOutcome::Acquired(claim))
        })
        .await
    }

    async fn release_identity_lease(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
        expected: &IdentityLeaseClaim,
    ) -> Result<bool, MobStoreError> {
        let expected_record = IdentityLeaseRecord {
            schema_version: IDENTITY_LEASE_SCHEMA_VERSION,
            epoch_highwater: expected.epoch,
            active: Some(expected.clone()),
        };
        expected_record
            .validate()
            .map_err(identity_contract_error)?;
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let mob_id = mob_id.clone();
        let identity = identity.clone();
        let expected = expected.clone();
        run_sqlite_task(move || {
            let mut conn = open_existing_identity_write_connection(&path, &store_instance_id)?;
            let tx = begin_identity_immediate(&mut conn, &store_instance_id)?;
            let Some(current) = query_identity_lease_authority(&tx, &mob_id, &identity)? else {
                return Ok(false);
            };
            if current.active.as_ref() != Some(&expected) {
                return Ok(false);
            }
            let released = IdentityLeaseRecord {
                schema_version: IDENTITY_LEASE_SCHEMA_VERSION,
                epoch_highwater: current.epoch_highwater,
                active: None,
            };
            released.validate().map_err(identity_contract_error)?;
            write_identity_lease(&tx, &mob_id, &identity, &released)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    async fn validate_identity_actuation_permit(
        &self,
        permit: &IdentityActuationPermit,
    ) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let clock = Arc::clone(&self.clock);
        let permit = permit.clone();
        run_sqlite_task(move || {
            let observed_at_ms = clock.now_ms()?;
            permit.validate_for_write(observed_at_ms).map_err(|error| {
                MobStoreError::CasConflict(format!(
                    "identity actuation permit is no longer current: {error}"
                ))
            })?;
            let conn = open_existing_identity_read_connection(&path, &store_instance_id)?;
            let intent = require_identity_authority(
                query_identity_intent_observation(&conn, &permit.mob_id, &permit.identity)?,
                "identity intent",
            )?
            .ok_or_else(|| {
                MobStoreError::CasConflict(
                    "identity actuation observed no current intent".to_string(),
                )
            })?;
            if intent.intent_revision != permit.intent_revision
                || intent.intent_digest != permit.intent_digest
                || intent.authority_digest != permit.intent_authority_digest
            {
                return Err(MobStoreError::CasConflict(
                    "identity actuation intent authority is stale".to_string(),
                ));
            }
            let lease = query_identity_lease_authority(&conn, &permit.mob_id, &permit.identity)?
                .ok_or_else(|| {
                    MobStoreError::CasConflict(
                        "identity actuation observed no current lease".to_string(),
                    )
                })?;
            let active = lease.active.ok_or_else(|| {
                MobStoreError::CasConflict(
                    "identity actuation observed no active lease".to_string(),
                )
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
        })
        .await
    }

    async fn observe_identity_operation_receipt(
        &self,
        mob_id: &MobId,
        subject: &IdentityOperationSubject,
        slot: &IdentityOperationSlot,
    ) -> Result<IdentityStoredObservation<IdentityOperationReceipt>, MobStoreError> {
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let mob_id = mob_id.clone();
        let subject = subject.clone();
        let slot = slot.clone();
        run_sqlite_task(move || {
            let conn = open_existing_identity_read_connection(&path, &store_instance_id)?;
            query_identity_receipt_observation(&conn, &mob_id, &subject, &slot)
        })
        .await
    }

    async fn insert_identity_operation_receipt_if_absent(
        &self,
        receipt: &IdentityOperationReceipt,
        permit: &IdentityActuationPermit,
    ) -> Result<IdentityOperationReceiptInsertOutcome, MobStoreError> {
        receipt.validate().map_err(identity_contract_error)?;
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let clock = Arc::clone(&self.clock);
        let receipt = receipt.clone();
        let permit = permit.clone();
        run_sqlite_task(move || {
            let mut conn = open_existing_identity_write_connection(&path, &store_instance_id)?;
            let tx = begin_identity_immediate(&mut conn, &store_instance_id)?;
            if let Some(existing) = require_identity_authority(
                query_identity_receipt_observation(
                    &tx,
                    &receipt.mob_id,
                    &receipt.subject,
                    &receipt.slot,
                )?,
                "identity operation receipt",
            )? {
                return Ok(if existing.request_digest == receipt.request_digest {
                    IdentityOperationReceiptInsertOutcome::ExistingExact(existing)
                } else {
                    IdentityOperationReceiptInsertOutcome::Conflict(existing)
                });
            }

            let observed_at_ms = clock.now_ms()?;
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
                || identity_receipt_target(&receipt) != Some(permit.target)
                || receipt.intent_revision != Some(permit.intent_revision)
                || receipt.intent_digest.as_ref() != Some(&permit.intent_digest)
                || receipt.intent_authority_digest.as_ref()
                    != Some(&permit.intent_authority_digest)
                || !matches!(
                    permit.target_observation,
                    IdentityTargetObservationVersion::InsertIfAbsent
                )
            {
                return Err(MobStoreError::CasConflict(
                    "identity receipt insertion permit does not match the immutable receipt slot"
                        .to_string(),
                ));
            }

            let intent = require_identity_authority(
                query_identity_intent_observation(&tx, &receipt.mob_id, identity)?,
                "identity intent",
            )?
            .ok_or_else(|| {
                MobStoreError::CasConflict(
                    "identity receipt insertion observed no current intent".to_string(),
                )
            })?;
            if intent.intent_revision != permit.intent_revision
                || intent.intent_digest != permit.intent_digest
                || intent.authority_digest != permit.intent_authority_digest
                || !identity_actuator_receipt_matches_intent(&receipt, &intent)
                || receipt
                    .audit_lease_epoch
                    .is_some_and(|epoch| epoch != permit.lease_epoch)
            {
                return Err(MobStoreError::CasConflict(
                    "identity receipt insertion intent authority is stale".to_string(),
                ));
            }

            let lease = query_identity_lease_authority(&tx, &receipt.mob_id, identity)?
            .ok_or_else(|| {
                MobStoreError::CasConflict(
                    "identity receipt insertion observed no current lease".to_string(),
                )
            })?;
            let active = lease.active.ok_or_else(|| {
                MobStoreError::CasConflict(
                    "identity receipt insertion observed no active lease".to_string(),
                )
            })?;
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
            insert_identity_receipt(&tx, &receipt)?;
            tx.commit().map_err(se)?;
            Ok(IdentityOperationReceiptInsertOutcome::Inserted(receipt))
        })
        .await
    }
}

#[async_trait]
impl MobIdentityMemberStore for SqliteMobIdentityStore {
    async fn observe_identity_member_target(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityMemberTargetObservation, MobStoreError> {
        if self.event_bus.is_none() {
            return Err(MobStoreError::IdentityMemberAtomicPersistenceUnavailable);
        }
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let mob_id = mob_id.clone();
        let identity = identity.clone();
        run_sqlite_task(move || {
            let conn = open_existing_identity_read_connection(&path, &store_instance_id)?;
            Ok(query_identity_member_target_state(&conn, &mob_id, &identity)?.observation)
        })
        .await
    }

    async fn commit_identity_member_spawned(
        &self,
        permit: &IdentityActuationPermit,
        event: &NewMobEvent,
    ) -> Result<IdentityMemberEventCommitOutcome, MobStoreError> {
        let Some(event_bus) = self.event_bus.clone() else {
            return Err(MobStoreError::IdentityMemberAtomicPersistenceUnavailable);
        };
        if let Err(error) = validate_mob_event_write_authority(&event.kind) {
            return Ok(IdentityMemberEventCommitOutcome::RepairBlocked {
                evidence_digest: None,
                detail: error.to_string(),
            });
        }
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let clock = Arc::clone(&self.clock);
        let permit = permit.clone();
        let event = event.clone();
        let result = run_sqlite_task(move || {
            let mut conn = open_existing_identity_write_connection(&path, &store_instance_id)?;
            let tx = begin_identity_immediate(&mut conn, &store_instance_id)?;
            // Sample only after BEGIN IMMEDIATE has acquired the writer lock.
            let observed_at_ms = match clock.now_ms() {
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
                mob_id: event.mob_id,
                kind: event.kind,
            };

            let intent =
                match query_identity_intent_observation(&tx, &permit.mob_id, &permit.identity)? {
                    IdentityStoredObservation::Valid(intent) => intent,
                    IdentityStoredObservation::Missing => {
                        return Ok(IdentityMemberEventCommitOutcome::Conflict {
                            current: None,
                            detail: "identity member commit observed no current intent".to_string(),
                        });
                    }
                    IdentityStoredObservation::Unsupported {
                        evidence_digest,
                        detail,
                    }
                    | IdentityStoredObservation::Malformed {
                        evidence_digest,
                        detail,
                    } => {
                        return Ok(IdentityMemberEventCommitOutcome::RepairBlocked {
                            evidence_digest: Some(evidence_digest),
                            detail: format!(
                                "identity member commit observed unsafe intent authority: {detail}"
                            ),
                        });
                    }
                };
            let lease =
                match query_identity_lease_observation(&tx, &permit.mob_id, &permit.identity)? {
                    IdentityStoredObservation::Valid(lease) => lease,
                    IdentityStoredObservation::Missing => {
                        return Ok(IdentityMemberEventCommitOutcome::Conflict {
                            current: None,
                            detail: "identity member commit observed no current lease".to_string(),
                        });
                    }
                    IdentityStoredObservation::Unsupported {
                        evidence_digest,
                        detail,
                    }
                    | IdentityStoredObservation::Malformed {
                        evidence_digest,
                        detail,
                    } => {
                        return Ok(IdentityMemberEventCommitOutcome::RepairBlocked {
                            evidence_digest: Some(evidence_digest),
                            detail: format!(
                                "identity member commit observed unsafe lease authority: {detail}"
                            ),
                        });
                    }
                };
            if let Err(outcome) = validate_identity_member_commit_authority(
                &permit,
                &intent,
                &lease,
                &proposed,
                observed_at_ms,
            ) {
                return Ok(outcome);
            }

            let current =
                query_identity_member_target_state(&tx, &permit.mob_id, &permit.identity)?;
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
                tx.commit().map_err(se)?;
                return Ok(IdentityMemberEventCommitOutcome::Conflict {
                    current: Some(current.observation),
                    detail: "identity member target observation is stale".to_string(),
                });
            }
            if let Some(existing) = current.exact_current_spawn
                && existing.kind == proposed.kind
            {
                tx.commit().map_err(se)?;
                return Ok(IdentityMemberEventCommitOutcome::AlreadyExact { event: existing });
            }
            if !matches!(
                permit.target_observation,
                IdentityTargetObservationVersion::Absent { .. }
            ) {
                tx.commit().map_err(se)?;
                return Ok(IdentityMemberEventCommitOutcome::RepairBlocked {
                    evidence_digest: current
                        .observation
                        .target_precondition()
                        .and_then(|target| match target {
                            IdentityTargetObservationVersion::Version { version } => Some(version),
                            _ => None,
                        }),
                    detail: "MemberSpawned append requires an exact absent-target witness"
                        .to_string(),
                });
            }

            let cursor = next_event_cursor(&tx)?;
            if cursor == 0 || cursor >= i64::MAX as u64 {
                return Ok(IdentityMemberEventCommitOutcome::RepairBlocked {
                    evidence_digest: None,
                    detail: "mob event cursor is exhausted".to_string(),
                });
            }
            let stored = MobEvent {
                cursor,
                timestamp: proposed.timestamp,
                mob_id: proposed.mob_id,
                kind: proposed.kind,
            };
            let encoded = match encode_stored_mob_event(&stored) {
                Ok(encoded) => encoded,
                Err(error) => {
                    return Ok(IdentityMemberEventCommitOutcome::RepairBlocked {
                        evidence_digest: None,
                        detail: format!("MemberSpawned event is not serializable: {error}"),
                    });
                }
            };
            tx.execute(
                "INSERT INTO mob_events (cursor, mob_id, event_json) VALUES (?1, ?2, ?3)",
                params![cursor_to_i64(cursor)?, stored.mob_id.as_str(), encoded],
            )
            .map_err(se)?;
            let Some(next_cursor) = cursor.checked_add(1) else {
                return Ok(IdentityMemberEventCommitOutcome::RepairBlocked {
                    evidence_digest: None,
                    detail: "mob event cursor is exhausted".to_string(),
                });
            };
            set_next_cursor(&tx, next_cursor)?;
            tx.commit().map_err(se)?;
            Ok(IdentityMemberEventCommitOutcome::Applied { event: stored })
        })
        .await;

        let outcome = match result {
            Ok(outcome) => outcome,
            Err(MobStoreError::IdentityAuthorityBlocked {
                evidence_digest,
                detail,
            }) => IdentityMemberEventCommitOutcome::RepairBlocked {
                evidence_digest,
                detail,
            },
            Err(error) => IdentityMemberEventCommitOutcome::Backoff {
                detail: error.to_string(),
            },
        };
        if let IdentityMemberEventCommitOutcome::Applied { event } = &outcome {
            // Publish only after the SQLite transaction committed. A failed
            // transaction is never observable through the append bus.
            event_bus.publish_committed(event.clone());
        }
        Ok(outcome)
    }

    async fn observe_identity_wiring_target(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityWiringTargetObservation, MobStoreError> {
        if self.event_bus.is_none() {
            return Err(MobStoreError::IdentityMemberAtomicPersistenceUnavailable);
        }
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let mob_id = mob_id.clone();
        let identity = identity.clone();
        run_sqlite_task(move || {
            let conn = open_existing_identity_read_connection(&path, &store_instance_id)?;
            Ok(query_identity_wiring_target_state(&conn, &mob_id, &identity)?.observation)
        })
        .await
    }

    async fn commit_identity_wiring_event(
        &self,
        permit: &IdentityActuationPermit,
        event: &NewMobEvent,
    ) -> Result<IdentityWiringEventCommitOutcome, MobStoreError> {
        let Some(event_bus) = self.event_bus.clone() else {
            return Err(MobStoreError::IdentityMemberAtomicPersistenceUnavailable);
        };
        if let Err(error) = validate_mob_event_write_authority(&event.kind) {
            return Ok(IdentityWiringEventCommitOutcome::RepairBlocked {
                evidence_digest: None,
                detail: error.to_string(),
            });
        }
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let clock = Arc::clone(&self.clock);
        let permit = permit.clone();
        let event = event.clone();
        let result = run_sqlite_task(move || {
            let mut conn = open_existing_identity_write_connection(&path, &store_instance_id)?;
            let tx = begin_identity_immediate(&mut conn, &store_instance_id)?;
            let observed_at_ms = match clock.now_ms() {
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
                mob_id: event.mob_id,
                kind: event.kind,
            };
            let intent =
                match query_identity_intent_observation(&tx, &permit.mob_id, &permit.identity)? {
                    IdentityStoredObservation::Valid(intent) => intent,
                    IdentityStoredObservation::Missing => {
                        return Ok(IdentityWiringEventCommitOutcome::Conflict {
                            current: None,
                            detail: "identity wiring commit observed no current intent".to_string(),
                        });
                    }
                    IdentityStoredObservation::Unsupported {
                        evidence_digest,
                        detail,
                    }
                    | IdentityStoredObservation::Malformed {
                        evidence_digest,
                        detail,
                    } => {
                        return Ok(IdentityWiringEventCommitOutcome::RepairBlocked {
                            evidence_digest: Some(evidence_digest),
                            detail: format!(
                                "identity wiring commit observed unsafe intent authority: {detail}"
                            ),
                        });
                    }
                };
            let lease =
                match query_identity_lease_observation(&tx, &permit.mob_id, &permit.identity)? {
                    IdentityStoredObservation::Valid(lease) => lease,
                    IdentityStoredObservation::Missing => {
                        return Ok(IdentityWiringEventCommitOutcome::Conflict {
                            current: None,
                            detail: "identity wiring commit observed no current lease".to_string(),
                        });
                    }
                    IdentityStoredObservation::Unsupported {
                        evidence_digest,
                        detail,
                    }
                    | IdentityStoredObservation::Malformed {
                        evidence_digest,
                        detail,
                    } => {
                        return Ok(IdentityWiringEventCommitOutcome::RepairBlocked {
                            evidence_digest: Some(evidence_digest),
                            detail: format!(
                                "identity wiring commit observed unsafe lease authority: {detail}"
                            ),
                        });
                    }
                };
            let (edge, adding) = match validate_identity_wiring_commit_authority(
                &permit,
                &intent,
                &lease,
                &proposed,
                observed_at_ms,
            ) {
                Ok(validated) => validated,
                Err(outcome) => return Ok(outcome),
            };

            let current =
                query_identity_wiring_target_state(&tx, &permit.mob_id, &permit.identity)?;
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
            if current.observation.target_precondition().as_ref()
                != Some(&permit.target_observation)
            {
                tx.commit().map_err(se)?;
                return Ok(IdentityWiringEventCommitOutcome::Conflict {
                    current: Some(current.observation),
                    detail: "identity wiring target observation is stale".to_string(),
                });
            }
            if current.observation.contains(&edge) == adding {
                tx.commit().map_err(se)?;
                return Ok(IdentityWiringEventCommitOutcome::AlreadyExact {
                    current: current.observation,
                });
            }

            let cursor = next_event_cursor(&tx)?;
            if cursor == 0 || cursor >= i64::MAX as u64 {
                return Ok(IdentityWiringEventCommitOutcome::RepairBlocked {
                    evidence_digest: None,
                    detail: "mob event cursor is exhausted".to_string(),
                });
            }
            let stored = MobEvent {
                cursor,
                timestamp: proposed.timestamp,
                mob_id: proposed.mob_id,
                kind: proposed.kind,
            };
            let encoded = match encode_stored_mob_event(&stored) {
                Ok(encoded) => encoded,
                Err(error) => {
                    return Ok(IdentityWiringEventCommitOutcome::RepairBlocked {
                        evidence_digest: None,
                        detail: format!("identity wiring event is not serializable: {error}"),
                    });
                }
            };
            tx.execute(
                "INSERT INTO mob_events (cursor, mob_id, event_json) VALUES (?1, ?2, ?3)",
                params![cursor_to_i64(cursor)?, stored.mob_id.as_str(), encoded],
            )
            .map_err(se)?;
            let Some(next_cursor) = cursor.checked_add(1) else {
                return Ok(IdentityWiringEventCommitOutcome::RepairBlocked {
                    evidence_digest: None,
                    detail: "mob event cursor is exhausted".to_string(),
                });
            };
            set_next_cursor(&tx, next_cursor)?;
            tx.commit().map_err(se)?;
            Ok(IdentityWiringEventCommitOutcome::Applied { event: stored })
        })
        .await;

        let outcome = match result {
            Ok(outcome) => outcome,
            Err(MobStoreError::IdentityAuthorityBlocked {
                evidence_digest,
                detail,
            }) => IdentityWiringEventCommitOutcome::RepairBlocked {
                evidence_digest,
                detail,
            },
            Err(error) => IdentityWiringEventCommitOutcome::Backoff {
                detail: error.to_string(),
            },
        };
        if let IdentityWiringEventCommitOutcome::Applied { event } = &outcome {
            event_bus.publish_committed(event.clone());
        }
        Ok(outcome)
    }
}

#[async_trait]
impl MobIdentityStatusStore for SqliteMobIdentityStatusStore {
    async fn load_identity_convergence_status(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityStoredObservation<IdentityConvergenceStatus>, MobStoreError> {
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let mob_id = mob_id.clone();
        let identity = identity.clone();
        run_sqlite_task(move || {
            let conn = open_existing_identity_read_connection(&path, &store_instance_id)?;
            query_identity_status_observation(&conn, &mob_id, &identity)
        })
        .await
    }

    async fn list_identity_convergence_statuses(
        &self,
        mob_id: &MobId,
    ) -> Result<
        BTreeMap<AgentIdentity, IdentityStoredObservation<IdentityConvergenceStatus>>,
        MobStoreError,
    > {
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_existing_identity_read_connection(&path, &store_instance_id)?;
            let mut stmt = conn
                .prepare(
                    "SELECT agent_identity, typeof(status_json), CAST(status_json AS BLOB)
                     FROM mob_identity_statuses
                     WHERE mob_id = ?1
                     ORDER BY agent_identity",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                })
                .map_err(se)?;
            let mut observations = BTreeMap::new();
            for row in rows {
                let (identity, storage_type, bytes) = row.map_err(se)?;
                let identity = AgentIdentity::from(identity);
                let observation = classify_identity_blob(
                    &storage_type,
                    &bytes,
                    None,
                    |_| Ok(()),
                    |status: &IdentityConvergenceStatus| {
                        if status.identity == identity {
                            Ok(())
                        } else {
                            Err(
                                "identity convergence status does not match its physical key"
                                    .to_string(),
                            )
                        }
                    },
                );
                observations.insert(identity, observation);
            }
            Ok(observations)
        })
        .await
    }

    async fn replace_identity_convergence_status(
        &self,
        mob_id: &MobId,
        status: &IdentityConvergenceStatus,
    ) -> Result<(), MobStoreError> {
        validate_identity_store_text("mob_id", mob_id.as_str())?;
        validate_identity_store_text("identity", status.identity.as_str())?;
        let path = self.path.clone();
        let store_instance_id = self.store_instance_id.clone();
        let mob_id = mob_id.clone();
        let status = status.clone();
        run_sqlite_task(move || {
            let mut conn = open_existing_identity_write_connection(&path, &store_instance_id)?;
            let tx = begin_identity_immediate(&mut conn, &store_instance_id)?;
            tx.execute(
                "INSERT INTO mob_identity_statuses (mob_id, agent_identity, status_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(mob_id, agent_identity) DO UPDATE SET
                     status_json = excluded.status_json",
                params![
                    mob_id.as_str(),
                    status.identity.as_str(),
                    encode_json(&status)?,
                ],
            )
            .map_err(|error| {
                sqlite_identity_write_error(error, "replace identity convergence status")
            })?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// SqliteMobRuntimeMetadataStore
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SqliteMobRuntimeMetadataStore {
    path: PathBuf,
}

impl std::fmt::Debug for SqliteMobRuntimeMetadataStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteMobRuntimeMetadataStore")
            .field("path", &self.path)
            .finish()
    }
}

#[async_trait]
impl MobRuntimeMetadataStore for SqliteMobRuntimeMetadataStore {
    async fn load_supervisor_authority(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<SupervisorAuthorityRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let row: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT record_json FROM mob_runtime_supervisors WHERE mob_id = ?1",
                    params![mob_id.as_str()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            row.map(|bytes| decode_json(&bytes)).transpose()
        })
        .await
    }

    async fn put_supervisor_authority(
        &self,
        mob_id: &MobId,
        record: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityPersistenceAuthority,
    ) -> Result<(), MobStoreError> {
        authority.verify_record(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "INSERT INTO mob_runtime_supervisors (mob_id, record_json) VALUES (?1, ?2)
                 ON CONFLICT(mob_id) DO UPDATE SET record_json = excluded.record_json",
                params![mob_id.as_str(), encode_json(&record)?],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn compare_and_put_supervisor_authority(
        &self,
        mob_id: &MobId,
        expected: &SupervisorAuthorityRecord,
        record: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected = expected.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "UPDATE mob_runtime_supervisors
                     SET record_json = ?2
                     WHERE mob_id = ?1 AND record_json = ?3",
                    params![
                        mob_id.as_str(),
                        encode_json(&record)?,
                        encode_json(&expected)?
                    ],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn put_supervisor_authority_if_absent(
        &self,
        mob_id: &MobId,
        record: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "INSERT OR IGNORE INTO mob_runtime_supervisors (mob_id, record_json) VALUES (?1, ?2)",
                    params![mob_id.as_str(), encode_json(&record)?],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn delete_supervisor_authority(
        &self,
        mob_id: &MobId,
        expected: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityDeletionAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(expected)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected = expected.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "DELETE FROM mob_runtime_supervisors WHERE mob_id = ?1 AND record_json = ?2",
                    params![mob_id.as_str(), encode_json(&expected)?],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn load_mob_host_authority(
        &self,
        mob_id: &MobId,
        host_id: &str,
    ) -> Result<Option<MobHostAuthorityRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let host_id = host_id.to_string();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let row: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT record_json FROM mob_runtime_host_authorities
                     WHERE mob_id = ?1 AND host_id = ?2",
                    params![mob_id.as_str(), host_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            row.map(|bytes| decode_json(&bytes)).transpose()
        })
        .await
    }

    async fn list_mob_host_authorities(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobHostAuthorityRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT record_json FROM mob_runtime_host_authorities
                     WHERE mob_id = ?1
                     ORDER BY host_id",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut records = Vec::new();
            for row in rows {
                let bytes = row.map_err(se)?;
                records.push(decode_json(&bytes)?);
            }
            Ok(records)
        })
        .await
    }

    async fn put_mob_host_authority(
        &self,
        mob_id: &MobId,
        record: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityPersistenceAuthority,
    ) -> Result<(), MobStoreError> {
        authority.verify_record(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "INSERT INTO mob_runtime_host_authorities (mob_id, host_id, record_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(mob_id, host_id) DO UPDATE SET record_json = excluded.record_json",
                params![mob_id.as_str(), record.host_id, encode_json(&record)?],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn put_mob_host_authority_if_absent(
        &self,
        mob_id: &MobId,
        record: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "INSERT OR IGNORE INTO mob_runtime_host_authorities (mob_id, host_id, record_json)
                     VALUES (?1, ?2, ?3)",
                    params![mob_id.as_str(), record.host_id, encode_json(&record)?],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn compare_and_put_mob_host_authority(
        &self,
        mob_id: &MobId,
        expected: &MobHostAuthorityRecord,
        record: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected = expected.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "UPDATE mob_runtime_host_authorities
                     SET record_json = ?3
                     WHERE mob_id = ?1 AND host_id = ?2 AND record_json = ?4",
                    params![
                        mob_id.as_str(),
                        record.host_id,
                        encode_json(&record)?,
                        encode_json(&expected)?
                    ],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn delete_mob_host_authority(
        &self,
        mob_id: &MobId,
        expected: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityDeletionAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_record(expected)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected = expected.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "DELETE FROM mob_runtime_host_authorities
                     WHERE mob_id = ?1 AND host_id = ?2 AND record_json = ?3",
                    params![mob_id.as_str(), expected.host_id, encode_json(&expected)?],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn put_mob_host_binding_generation_highwater(
        &self,
        mob_id: &MobId,
        expected: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityDeletionAuthority,
    ) -> Result<(), MobStoreError> {
        authority.verify_record(expected)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let host_id = expected.host_id.clone();
        let binding_generation = u64_to_sql_key(expected.binding_generation);
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "INSERT INTO mob_runtime_host_binding_generation_highwaters
                    (mob_id, host_id, binding_generation)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(mob_id, host_id) DO UPDATE SET binding_generation =
                    CASE
                        WHEN excluded.binding_generation > mob_runtime_host_binding_generation_highwaters.binding_generation
                        THEN excluded.binding_generation
                        ELSE mob_runtime_host_binding_generation_highwaters.binding_generation
                    END",
                params![mob_id.as_str(), host_id, binding_generation],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn list_mob_host_binding_generation_highwaters(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<(String, u64)>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT host_id, binding_generation
                     FROM mob_runtime_host_binding_generation_highwaters
                     WHERE mob_id = ?1
                     ORDER BY host_id",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
                })
                .map_err(se)?;
            let mut highwaters = Vec::new();
            for row in rows {
                let (host_id, encoded) = row.map_err(se)?;
                highwaters.push((host_id, sql_key_to_u64(&encoded, "binding_generation")?));
            }
            Ok(highwaters)
        })
        .await
    }

    async fn begin_member_operator_request(
        &self,
        mob_id: &MobId,
        record: &MobMemberOperatorRequestRecord,
    ) -> Result<MobMemberOperatorRequestBegin, MobStoreError> {
        record.validate_pending()?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let generation = u64_to_sql_key(record.generation);
            let fence_token = u64_to_sql_key(record.fence_token);
            let binding_generation = u64_to_sql_key(record.host_binding_generation);
            let existing_bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT record_json FROM mob_runtime_member_operator_requests
                     WHERE mob_id = ?1 AND agent_identity = ?2
                       AND generation = ?3 AND fence_token = ?4
                       AND host_id = ?5 AND binding_generation = ?6
                       AND member_session_id = ?7 AND request_id = ?8",
                    params![
                        mob_id.as_str(),
                        &record.agent_identity,
                        generation,
                        fence_token,
                        &record.host_id,
                        binding_generation,
                        &record.member_session_id,
                        &record.request_id,
                    ],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let result = if let Some(bytes) = existing_bytes {
                let existing: MobMemberOperatorRequestRecord = decode_json(&bytes)?;
                validate_member_operator_request_row(&existing, &record.key())?;
                MobMemberOperatorRequestBegin::Existing(existing)
            } else {
                let incarnation_rows: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM mob_runtime_member_operator_requests
                         WHERE mob_id = ?1 AND agent_identity = ?2
                           AND generation = ?3 AND fence_token = ?4
                           AND host_id = ?5 AND binding_generation = ?6
                           AND member_session_id = ?7",
                        params![
                            mob_id.as_str(),
                            &record.agent_identity,
                            generation,
                            fence_token,
                            &record.host_id,
                            binding_generation,
                            &record.member_session_id,
                        ],
                        |row| row.get(0),
                    )
                    .map_err(se)?;
                if incarnation_rows
                    >= i64::try_from(super::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION)
                        .unwrap_or(i64::MAX)
                {
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
                let mob_rows: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM mob_runtime_member_operator_requests
                         WHERE mob_id = ?1",
                        params![mob_id.as_str()],
                        |row| row.get(0),
                    )
                    .map_err(se)?;
                if mob_rows
                    >= i64::try_from(super::MEMBER_OPERATOR_REQUEST_MAX_PER_MOB)
                        .unwrap_or(i64::MAX)
                {
                    return Err(MobStoreError::WriteFailed(format!(
                        "member operator request quota exhausted for mob '{}' (max {})",
                        mob_id,
                        super::MEMBER_OPERATOR_REQUEST_MAX_PER_MOB,
                    )));
                }
                tx.execute(
                    "INSERT INTO mob_runtime_member_operator_requests
                     (mob_id, agent_identity, generation, fence_token, host_id, binding_generation, member_session_id, request_id, record_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        mob_id.as_str(),
                        &record.agent_identity,
                        generation,
                        fence_token,
                        &record.host_id,
                        binding_generation,
                        &record.member_session_id,
                        &record.request_id,
                        encode_json(&record)?,
                    ],
                )
                .map_err(se)?;
                MobMemberOperatorRequestBegin::Started
            };
            tx.commit().map_err(se)?;
            Ok(result)
        })
        .await
    }

    async fn load_member_operator_request(
        &self,
        mob_id: &MobId,
        key: &MobMemberOperatorRequestKey,
    ) -> Result<Option<MobMemberOperatorRequestRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let key = key.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let row: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT record_json FROM mob_runtime_member_operator_requests
                     WHERE mob_id = ?1 AND agent_identity = ?2
                       AND generation = ?3 AND fence_token = ?4
                       AND host_id = ?5 AND binding_generation = ?6
                       AND member_session_id = ?7 AND request_id = ?8",
                    params![
                        mob_id.as_str(),
                        &key.agent_identity,
                        u64_to_sql_key(key.generation),
                        u64_to_sql_key(key.fence_token),
                        &key.host_id,
                        u64_to_sql_key(key.host_binding_generation),
                        &key.member_session_id,
                        &key.request_id,
                    ],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let record: Option<MobMemberOperatorRequestRecord> =
                row.map(|bytes| decode_json(&bytes)).transpose()?;
            if let Some(record) = &record {
                validate_member_operator_request_row(record, &key)?;
            }
            Ok(record)
        })
        .await
    }

    async fn compare_and_put_member_operator_request(
        &self,
        mob_id: &MobId,
        expected: &MobMemberOperatorRequestRecord,
        record: &MobMemberOperatorRequestRecord,
    ) -> Result<bool, MobStoreError> {
        expected.validate_terminal_transition(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected = expected.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "UPDATE mob_runtime_member_operator_requests
                     SET record_json = ?1
                     WHERE mob_id = ?2 AND agent_identity = ?3
                       AND generation = ?4 AND fence_token = ?5
                       AND host_id = ?6 AND binding_generation = ?7
                       AND member_session_id = ?8 AND request_id = ?9
                       AND record_json = ?10",
                    params![
                        encode_json(&record)?,
                        mob_id.as_str(),
                        expected.agent_identity,
                        u64_to_sql_key(expected.generation),
                        u64_to_sql_key(expected.fence_token),
                        expected.host_id,
                        u64_to_sql_key(expected.host_binding_generation),
                        expected.member_session_id,
                        expected.request_id,
                        encode_json(&expected)?,
                    ],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn list_member_operator_requests(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobMemberOperatorRequestRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT agent_identity, generation, fence_token, host_id, binding_generation, member_session_id, request_id, record_json
                     FROM mob_runtime_member_operator_requests
                     WHERE mob_id = ?1
                     ORDER BY agent_identity, generation, fence_token, host_id, binding_generation, member_session_id, request_id",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Vec<u8>>(7)?,
                    ))
                })
                .map_err(se)?;
            let mut records = Vec::new();
            for row in rows {
                let (
                    agent_identity,
                    generation,
                    fence_token,
                    host_id,
                    binding_generation,
                    member_session_id,
                    request_id,
                    bytes,
                ) =
                    row.map_err(se)?;
                let generation = sql_key_to_u64(
                    &generation,
                    &format!("member operator request '{request_id}' generation"),
                )?;
                let fence_token = sql_key_to_u64(
                    &fence_token,
                    &format!("member operator request '{request_id}' fence token"),
                )?;
                let binding_generation = sql_key_to_u64(
                    &binding_generation,
                    &format!(
                        "member operator request '{request_id}' binding generation"
                    ),
                )?;
                let record: MobMemberOperatorRequestRecord = decode_json(&bytes)?;
                let key = MobMemberOperatorRequestKey::new(
                    agent_identity,
                    generation,
                    fence_token,
                    host_id,
                    binding_generation,
                    member_session_id,
                    request_id,
                );
                validate_member_operator_request_row(&record, &key)?;
                records.push(record);
            }
            Ok(records)
        })
        .await
    }

    async fn prune_stale_member_operator_requests(
        &self,
        mob_id: &MobId,
        authority: &MobMemberOperatorPruneAuthority,
    ) -> Result<u64, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let authority = authority.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let stale = {
                let mut stmt = tx
                    .prepare(
                        "SELECT agent_identity, generation, fence_token, host_id,
                                binding_generation, member_session_id, request_id, record_json
                         FROM mob_runtime_member_operator_requests
                         WHERE mob_id = ?1",
                    )
                    .map_err(se)?;
                let rows = stmt
                    .query_map(params![mob_id.as_str()], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Vec<u8>>(1)?,
                            row.get::<_, Vec<u8>>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Vec<u8>>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, Vec<u8>>(7)?,
                        ))
                    })
                    .map_err(se)?;
                let mut stale = Vec::new();
                for row in rows {
                    let (
                        agent_identity,
                        generation,
                        fence_token,
                        host_id,
                        binding_generation,
                        member_session_id,
                        request_id,
                        bytes,
                    ) = row.map_err(se)?;
                    let generation = sql_key_to_u64(
                        &generation,
                        &format!("member operator request '{request_id}' generation"),
                    )?;
                    let fence_token = sql_key_to_u64(
                        &fence_token,
                        &format!("member operator request '{request_id}' fence token"),
                    )?;
                    let binding_generation = sql_key_to_u64(
                        &binding_generation,
                        &format!("member operator request '{request_id}' binding generation"),
                    )?;
                    let record: MobMemberOperatorRequestRecord = decode_json(&bytes)?;
                    let key = MobMemberOperatorRequestKey::new(
                        agent_identity,
                        generation,
                        fence_token,
                        host_id,
                        binding_generation,
                        member_session_id,
                        request_id,
                    );
                    validate_member_operator_request_row(&record, &key)?;
                    if !authority.preserves(&record) {
                        stale.push(record);
                    }
                }
                stale
            };

            let mut deleted = 0usize;
            for record in stale {
                deleted += tx
                    .execute(
                        "DELETE FROM mob_runtime_member_operator_requests
                         WHERE mob_id = ?1 AND agent_identity = ?2
                           AND generation = ?3 AND fence_token = ?4
                           AND host_id = ?5 AND binding_generation = ?6
                           AND member_session_id = ?7 AND request_id = ?8",
                        params![
                            mob_id.as_str(),
                            record.agent_identity,
                            u64_to_sql_key(record.generation),
                            u64_to_sql_key(record.fence_token),
                            record.host_id,
                            u64_to_sql_key(record.host_binding_generation),
                            record.member_session_id,
                            record.request_id,
                        ],
                    )
                    .map_err(se)?;
            }
            tx.commit().map_err(se)?;
            u64::try_from(deleted).map_err(|_| {
                MobStoreError::Internal(
                    "member operator request SQLite prune count overflow".to_string(),
                )
            })
        })
        .await
    }

    async fn delete_member_operator_requests(&self, mob_id: &MobId) -> Result<u64, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let deleted = tx
                .execute(
                    "DELETE FROM mob_runtime_member_operator_requests WHERE mob_id = ?1",
                    params![mob_id.as_str()],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            u64::try_from(deleted).map_err(|_| {
                MobStoreError::Internal("member operator request delete count overflow".to_string())
            })
        })
        .await
    }

    async fn load_placed_spawn(
        &self,
        mob_id: &MobId,
        agent_identity: &str,
    ) -> Result<Option<MobPlacedSpawnCarrierRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let agent_identity = agent_identity.to_string();
        run_sqlite_task(move || {
            let conn = open_existing_read_connection(&path)?;
            let row: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT record_json FROM mob_runtime_placed_spawns
                     WHERE mob_id = ?1 AND agent_identity = ?2",
                    params![mob_id.as_str(), agent_identity],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let record: Option<MobPlacedSpawnCarrierRecord> =
                row.map(|bytes| decode_json(&bytes)).transpose()?;
            if let Some(record) = &record {
                record.validate_for_store_key(&mob_id, &agent_identity)?;
            }
            Ok(record)
        })
        .await
    }

    async fn list_placed_spawns(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobPlacedSpawnCarrierRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_existing_read_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT agent_identity, record_json FROM mob_runtime_placed_spawns
                     WHERE mob_id = ?1 ORDER BY agent_identity",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
                })
                .map_err(se)?;
            let mut records = Vec::new();
            for row in rows {
                let (agent_identity, bytes) = row.map_err(se)?;
                let record: MobPlacedSpawnCarrierRecord = decode_json(&bytes)?;
                record.validate_for_store_key(&mob_id, &agent_identity)?;
                records.push(record);
            }
            Ok(records)
        })
        .await
    }

    async fn begin_placed_spawn_if_absent(
        &self,
        mob_id: &MobId,
        record: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnPendingPersistenceAuthority,
    ) -> Result<BeginPlacedSpawnResult, MobStoreError> {
        record.validate_for_mob(mob_id)?;
        authority.verify_record(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let row: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT record_json FROM mob_runtime_placed_spawns
                     WHERE mob_id = ?1 AND agent_identity = ?2",
                    params![mob_id.as_str(), record.agent_identity],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let result = if let Some(bytes) = row {
                let existing: MobPlacedSpawnCarrierRecord = decode_json(&bytes)?;
                existing.validate_for_store_key(&mob_id, &record.agent_identity)?;
                if !existing.same_attempt_as(&record) {
                    BeginPlacedSpawnResult::Conflict
                } else {
                    match existing.phase {
                        PlacedSpawnCarrierPhase::Pending => {
                            BeginPlacedSpawnResult::ExistingExactPending
                        }
                        PlacedSpawnCarrierPhase::Committed(_) => {
                            BeginPlacedSpawnResult::ExistingExactCommitted
                        }
                    }
                }
            } else {
                tx.execute(
                    "INSERT INTO mob_runtime_placed_spawns
                     (mob_id, agent_identity, record_json) VALUES (?1, ?2, ?3)",
                    params![
                        mob_id.as_str(),
                        record.agent_identity,
                        encode_json(&record)?
                    ],
                )
                .map_err(se)?;
                BeginPlacedSpawnResult::Inserted
            };
            tx.commit().map_err(se)?;
            Ok(result)
        })
        .await
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
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected_pending = expected_pending.clone();
        let committed = committed.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let row: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT record_json FROM mob_runtime_placed_spawns
                     WHERE mob_id = ?1 AND agent_identity = ?2",
                    params![mob_id.as_str(), expected_pending.agent_identity],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let result = match row {
                None => CommitPlacedSpawnResult::Conflict,
                Some(bytes) => {
                    let existing: MobPlacedSpawnCarrierRecord = decode_json(&bytes)?;
                    existing.validate_for_store_key(&mob_id, &expected_pending.agent_identity)?;
                    if existing == committed {
                        CommitPlacedSpawnResult::AlreadyCommittedExact
                    } else if existing == expected_pending {
                        tx.execute(
                            "UPDATE mob_runtime_placed_spawns SET record_json = ?3
                             WHERE mob_id = ?1 AND agent_identity = ?2",
                            params![
                                mob_id.as_str(),
                                expected_pending.agent_identity,
                                encode_json(&committed)?
                            ],
                        )
                        .map_err(se)?;
                        CommitPlacedSpawnResult::Committed
                    } else if existing.same_attempt_as(&expected_pending)
                        && matches!(existing.phase, PlacedSpawnCarrierPhase::Pending)
                    {
                        CommitPlacedSpawnResult::StillPending
                    } else {
                        CommitPlacedSpawnResult::Conflict
                    }
                }
            };
            tx.commit().map_err(se)?;
            Ok(result)
        })
        .await
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
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected = expected.clone();
        let promoted = promoted.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let row: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT record_json FROM mob_runtime_placed_spawns
                     WHERE mob_id = ?1 AND agent_identity = ?2",
                    params![mob_id.as_str(), expected.agent_identity],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let result = match row {
                None => PromotePlacedSpawnBindingResult::Conflict,
                Some(bytes) => {
                    let existing: MobPlacedSpawnCarrierRecord = decode_json(&bytes)?;
                    existing.validate_for_store_key(&mob_id, &expected.agent_identity)?;
                    if existing == promoted {
                        PromotePlacedSpawnBindingResult::AlreadyPromotedExact
                    } else if existing == expected {
                        tx.execute(
                            "UPDATE mob_runtime_placed_spawns SET record_json = ?3
                             WHERE mob_id = ?1 AND agent_identity = ?2",
                            params![
                                mob_id.as_str(),
                                expected.agent_identity,
                                encode_json(&promoted)?
                            ],
                        )
                        .map_err(se)?;
                        PromotePlacedSpawnBindingResult::Promoted
                    } else {
                        PromotePlacedSpawnBindingResult::Conflict
                    }
                }
            };
            tx.commit().map_err(se)?;
            Ok(result)
        })
        .await
    }

    async fn compare_and_delete_placed_spawn(
        &self,
        mob_id: &MobId,
        expected: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnCleanupAuthority,
    ) -> Result<DeletePlacedSpawnResult, MobStoreError> {
        expected.validate_for_mob(mob_id)?;
        authority.verify_record(expected)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected = expected.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let row: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT record_json FROM mob_runtime_placed_spawns
                     WHERE mob_id = ?1 AND agent_identity = ?2",
                    params![mob_id.as_str(), expected.agent_identity],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let result = match row {
                None => DeletePlacedSpawnResult::AlreadyAbsent,
                Some(bytes) => {
                    let existing: MobPlacedSpawnCarrierRecord = decode_json(&bytes)?;
                    existing.validate_for_store_key(&mob_id, &expected.agent_identity)?;
                    if existing != expected {
                        DeletePlacedSpawnResult::Conflict
                    } else {
                        tx.execute(
                            "DELETE FROM mob_runtime_placed_spawns
                             WHERE mob_id = ?1 AND agent_identity = ?2",
                            params![mob_id.as_str(), expected.agent_identity],
                        )
                        .map_err(se)?;
                        DeletePlacedSpawnResult::Deleted
                    }
                }
            };
            tx.commit().map_err(se)?;
            Ok(result)
        })
        .await
    }

    async fn list_mob_operator_grants(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobOperatorGrantRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT record_json FROM mob_runtime_operator_grants
                     WHERE mob_id = ?1
                     ORDER BY principal",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut records = Vec::new();
            for row in rows {
                let bytes = row.map_err(se)?;
                records.push(decode_json(&bytes)?);
            }
            Ok(records)
        })
        .await
    }

    async fn put_mob_operator_grant(
        &self,
        mob_id: &MobId,
        record: &MobOperatorGrantRecord,
        authority: &MobOperatorGrantPersistenceAuthority,
    ) -> Result<(), MobStoreError> {
        authority.verify_record(record)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "INSERT INTO mob_runtime_operator_grants (mob_id, principal, record_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(mob_id, principal) DO UPDATE SET record_json = excluded.record_json",
                params![mob_id.as_str(), record.principal, encode_json(&record)?],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn delete_mob_operator_grant(
        &self,
        mob_id: &MobId,
        principal: &str,
        authority: &MobOperatorGrantDeletionAuthority,
    ) -> Result<bool, MobStoreError> {
        authority.verify_principal(principal)?;
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let principal = principal.to_string();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "DELETE FROM mob_runtime_operator_grants
                     WHERE mob_id = ?1 AND principal = ?2",
                    params![mob_id.as_str(), principal],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn delete_mob_operator_grants(&self, mob_id: &MobId) -> Result<u64, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "DELETE FROM mob_runtime_operator_grants WHERE mob_id = ?1",
                    params![mob_id.as_str()],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed as u64)
        })
        .await
    }

    async fn list_member_event_cursors(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobMemberEventCursorRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT record_json FROM mob_runtime_member_event_cursors
                     WHERE mob_id = ?1
                     ORDER BY agent_identity",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut records = Vec::new();
            for row in rows {
                let bytes = row.map_err(se)?;
                records.push(decode_json(&bytes)?);
            }
            Ok(records)
        })
        .await
    }

    async fn put_member_event_cursor(
        &self,
        mob_id: &MobId,
        record: &MobMemberEventCursorRecord,
    ) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "INSERT INTO mob_runtime_member_event_cursors (mob_id, agent_identity, record_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(mob_id, agent_identity) DO UPDATE SET record_json = excluded.record_json",
                params![mob_id.as_str(), record.agent_identity, encode_json(&record)?],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn delete_member_event_cursor(
        &self,
        mob_id: &MobId,
        agent_identity: &str,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let agent_identity = agent_identity.to_string();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "DELETE FROM mob_runtime_member_event_cursors
                     WHERE mob_id = ?1 AND agent_identity = ?2",
                    params![mob_id.as_str(), agent_identity],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn delete_member_event_cursors(&self, mob_id: &MobId) -> Result<u64, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "DELETE FROM mob_runtime_member_event_cursors WHERE mob_id = ?1",
                    params![mob_id.as_str()],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed as u64)
        })
        .await
    }

    async fn list_member_live_cleanup_records(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobMemberLiveCleanupRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT record_json FROM mob_runtime_member_live_cleanups
                     WHERE mob_id = ?1 ORDER BY cleanup_id",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut records = Vec::new();
            for row in rows {
                records.push(decode_json(&row.map_err(se)?)?);
            }
            Ok(records)
        })
        .await
    }

    async fn put_member_live_cleanup_record_if_absent(
        &self,
        mob_id: &MobId,
        record: &MobMemberLiveCleanupRecord,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let encoded = encode_json(&record)?;
            let changed = tx
                .execute(
                    "INSERT OR IGNORE INTO mob_runtime_member_live_cleanups
                     (mob_id, cleanup_id, record_json) VALUES (?1, ?2, ?3)",
                    params![mob_id.as_str(), record.cleanup_id.as_str(), encoded],
                )
                .map_err(se)?;
            if changed == 0 {
                let existing: Vec<u8> = tx
                    .query_row(
                        "SELECT record_json FROM mob_runtime_member_live_cleanups
                         WHERE mob_id = ?1 AND cleanup_id = ?2",
                        params![mob_id.as_str(), record.cleanup_id.as_str()],
                        |row| row.get(0),
                    )
                    .map_err(se)?;
                let existing: MobMemberLiveCleanupRecord = decode_json(&existing)?;
                if existing != record {
                    return Err(MobStoreError::Internal(format!(
                        "member-live cleanup id '{}' was reused for a conflicting record",
                        record.cleanup_id
                    )));
                }
            }
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn delete_member_live_cleanup_record(
        &self,
        mob_id: &MobId,
        expected: &MobMemberLiveCleanupRecord,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let expected = expected.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let expected_encoded = encode_json(&expected)?;
            let changed = tx
                .execute(
                    "DELETE FROM mob_runtime_member_live_cleanups
                     WHERE mob_id = ?1 AND cleanup_id = ?2 AND record_json = ?3",
                    params![
                        mob_id.as_str(),
                        expected.cleanup_id.as_str(),
                        expected_encoded.as_slice()
                    ],
                )
                .map_err(se)?;
            if changed == 0 {
                let existing: Option<Vec<u8>> = tx
                    .query_row(
                        "SELECT record_json FROM mob_runtime_member_live_cleanups
                         WHERE mob_id = ?1 AND cleanup_id = ?2",
                        params![mob_id.as_str(), expected.cleanup_id.as_str()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(se)?;
                if existing.is_some_and(|bytes| bytes != expected_encoded) {
                    return Err(MobStoreError::Internal(format!(
                        "member-live cleanup id '{}' no longer matches its expected immutable record",
                        expected.cleanup_id
                    )));
                }
            }
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn list_external_binding_overlays(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<ExternalBindingOverlayRecord>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT record_json
                     FROM mob_runtime_binding_overlays
                     WHERE mob_id = ?1
                     ORDER BY agent_identity, generation",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![mob_id.as_str()], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut records = Vec::new();
            for row in rows {
                let bytes = row.map_err(se)?;
                records.push(decode_json(&bytes)?);
            }
            Ok(records)
        })
        .await
    }

    async fn put_external_binding_overlay_if_absent(
        &self,
        mob_id: &MobId,
        record: &ExternalBindingOverlayRecord,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let changed = tx
                .execute(
                    "INSERT OR IGNORE INTO mob_runtime_binding_overlays
                     (mob_id, agent_identity, generation, record_json)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        mob_id.as_str(),
                        record.agent_identity.as_str(),
                        i64::try_from(record.generation.get()).map_err(|_| {
                            MobStoreError::Internal(format!(
                                "generation {} exceeds i64::MAX",
                                record.generation.get()
                            ))
                        })?,
                        encode_json(&record)?,
                    ],
                )
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn upsert_external_binding_overlay(
        &self,
        mob_id: &MobId,
        record: &ExternalBindingOverlayRecord,
    ) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let record = record.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "INSERT INTO mob_runtime_binding_overlays
                 (mob_id, agent_identity, generation, record_json)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(mob_id, agent_identity, generation)
                 DO UPDATE SET record_json = excluded.record_json",
                params![
                    mob_id.as_str(),
                    record.agent_identity.as_str(),
                    i64::try_from(record.generation.get()).map_err(|_| {
                        MobStoreError::Internal(format!(
                            "generation {} exceeds i64::MAX",
                            record.generation.get()
                        ))
                    })?,
                    encode_json(&record)?,
                ],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn delete_external_binding_overlay(
        &self,
        mob_id: &MobId,
        agent_identity: &AgentIdentity,
        generation: Generation,
    ) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let agent_identity = agent_identity.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "DELETE FROM mob_runtime_binding_overlays
                 WHERE mob_id = ?1 AND agent_identity = ?2 AND generation = ?3",
                params![
                    mob_id.as_str(),
                    agent_identity.as_str(),
                    i64::try_from(generation.get()).map_err(|_| {
                        MobStoreError::Internal(format!(
                            "generation {} exceeds i64::MAX",
                            generation.get()
                        ))
                    })?,
                ],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn delete_external_binding_overlays(&self, mob_id: &MobId) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "DELETE FROM mob_runtime_binding_overlays WHERE mob_id = ?1",
                params![mob_id.as_str()],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// SqliteMobEventStore
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SqliteMobEventStore {
    path: PathBuf,
    event_bus: Arc<SqliteMobEventBus>,
}

impl std::fmt::Debug for SqliteMobEventStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteMobEventStore")
            .field("path", &self.path)
            .finish()
    }
}

const EVENT_CURSOR_KEY: &str = "next_cursor";

impl private::MobEventStoreSealed for SqliteMobEventStore {}

#[async_trait]
impl MobEventStore for SqliteMobEventStore {
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobStoreError> {
        validate_mob_event_write_authority(&event.kind)?;

        let path = self.path.clone();
        let stored = run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let cursor = next_event_cursor(&tx)?;
            let stored = MobEvent {
                cursor,
                timestamp: event.timestamp.unwrap_or_else(Utc::now),
                mob_id: event.mob_id,
                kind: event.kind,
            };
            let encoded = encode_stored_mob_event(&stored)
                .map_err(|e| MobStoreError::Serialization(e.to_string()))?;
            tx.execute(
                "INSERT INTO mob_events (cursor, mob_id, event_json) VALUES (?1, ?2, ?3)",
                params![cursor_to_i64(cursor)?, stored.mob_id.as_str(), encoded],
            )
            .map_err(se)?;
            set_next_cursor(&tx, checked_event_cursor_successor(cursor)?)?;
            tx.commit().map_err(se)?;
            Ok(stored)
        })
        .await?;
        self.event_bus.publish_committed(stored.clone());
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

        let path = self.path.clone();
        let stored = run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let mut stmt = tx
                .prepare("SELECT event_json FROM mob_events ORDER BY cursor")
                .map_err(se)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            for row in rows {
                let bytes = row.map_err(se)?;
                let existing = decode_stored_mob_event(&bytes)
                    .map_err(|e| MobStoreError::Serialization(e.to_string()))?;
                if existing.mob_id == mob_id
                    && terminal_event_identity(&existing.kind).is_some_and(
                        |(existing_run_id, existing_flow_id)| {
                            existing_run_id == &run_id && existing_flow_id == &flow_id
                        },
                    )
                {
                    return Ok(None);
                }
            }
            drop(stmt);

            let cursor = next_event_cursor(&tx)?;
            let stored = MobEvent {
                cursor,
                timestamp: event.timestamp.unwrap_or_else(Utc::now),
                mob_id: event.mob_id,
                kind: event.kind,
            };
            let encoded = encode_stored_mob_event(&stored)
                .map_err(|e| MobStoreError::Serialization(e.to_string()))?;
            tx.execute(
                "INSERT INTO mob_events (cursor, mob_id, event_json) VALUES (?1, ?2, ?3)",
                params![cursor_to_i64(cursor)?, stored.mob_id.as_str(), encoded],
            )
            .map_err(se)?;
            set_next_cursor(&tx, checked_event_cursor_successor(cursor)?)?;
            tx.commit().map_err(se)?;
            Ok(Some(stored))
        })
        .await?;
        if let Some(stored) = stored.as_ref() {
            self.event_bus.publish_committed(stored.clone());
        }
        Ok(stored)
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

        let path = self.path.clone();
        let stored = run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let mut stmt = tx
                .prepare("SELECT event_json FROM mob_events ORDER BY cursor")
                .map_err(se)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut exact_replay = false;
            for row in rows {
                let bytes = row.map_err(se)?;
                let existing = decode_stored_mob_event(&bytes)
                    .map_err(|e| MobStoreError::Serialization(e.to_string()))?;
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
            drop(stmt);
            if exact_replay {
                return Ok(None);
            }

            let cursor = next_event_cursor(&tx)?;
            let stored = MobEvent {
                cursor,
                timestamp: event.timestamp.unwrap_or_else(Utc::now),
                mob_id: event.mob_id,
                kind: event.kind,
            };
            let encoded = encode_stored_mob_event(&stored)
                .map_err(|e| MobStoreError::Serialization(e.to_string()))?;
            tx.execute(
                "INSERT INTO mob_events (cursor, mob_id, event_json) VALUES (?1, ?2, ?3)",
                params![cursor_to_i64(cursor)?, stored.mob_id.as_str(), encoded],
            )
            .map_err(se)?;
            set_next_cursor(&tx, checked_event_cursor_successor(cursor)?)?;
            tx.commit().map_err(se)?;
            Ok(Some(stored))
        })
        .await?;
        if let Some(stored) = stored.as_ref() {
            self.event_bus.publish_committed(stored.clone());
        }
        Ok(stored)
    }

    async fn append_batch(&self, batch: Vec<NewMobEvent>) -> Result<Vec<MobEvent>, MobStoreError> {
        for event in &batch {
            validate_mob_event_write_authority(&event.kind)?;
        }

        let path = self.path.clone();
        let stored = run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let mut cursor = next_event_cursor(&tx)?;
            let mut results = Vec::with_capacity(batch.len());
            for event in batch {
                let stored = MobEvent {
                    cursor,
                    timestamp: event.timestamp.unwrap_or_else(Utc::now),
                    mob_id: event.mob_id,
                    kind: event.kind,
                };
                let encoded = encode_stored_mob_event(&stored)
                    .map_err(|e| MobStoreError::Serialization(e.to_string()))?;
                tx.execute(
                    "INSERT INTO mob_events (cursor, mob_id, event_json) VALUES (?1, ?2, ?3)",
                    params![cursor_to_i64(cursor)?, stored.mob_id.as_str(), encoded],
                )
                .map_err(se)?;
                results.push(stored);
                cursor = checked_event_cursor_successor(cursor)?;
            }
            set_next_cursor(&tx, cursor)?;
            tx.commit().map_err(se)?;
            Ok(results)
        })
        .await?;
        self.event_bus.publish_committed_batch(&stored);
        Ok(stored)
    }

    async fn poll(&self, after_cursor: u64, limit: usize) -> Result<Vec<MobEvent>, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || poll_events_sync(&path, after_cursor, limit)).await
    }

    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let conn = open_existing_read_connection(&path)?;
            let mut stmt = conn
                .prepare("SELECT event_json FROM mob_events ORDER BY cursor")
                .map_err(se)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut result = Vec::new();
            for row in rows {
                let bytes = row.map_err(se)?;
                result.push(
                    decode_stored_mob_event(&bytes)
                        .map_err(|e| MobStoreError::Serialization(e.to_string()))?,
                );
            }
            Ok(result)
        })
        .await
    }

    async fn latest_cursor(&self) -> Result<u64, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || latest_event_cursor_sync(&path)).await
    }

    fn subscribe(&self) -> Result<super::MobEventReceiver, MobStoreError> {
        Ok(self.event_bus.subscribe())
    }

    async fn clear(&self) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute("DELETE FROM mob_events", []).map_err(se)?;
            tx.execute(
                "INSERT OR REPLACE INTO mob_event_meta (key, value) VALUES (?1, ?2)",
                params![EVENT_CURSOR_KEY, 1i64],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn prune(&self, older_than: DateTime<Utc>) -> Result<u64, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            // Read all events, delete those older than the threshold.
            // Events store timestamp inside JSON, so we must deserialize to check.
            let mut stmt = tx
                .prepare("SELECT cursor, event_json FROM mob_events ORDER BY cursor")
                .map_err(se)?;
            let rows: Vec<(i64, Vec<u8>)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(se)?
                .collect::<Result<_, _>>()
                .map_err(se)?;
            drop(stmt);

            let mut removed = 0u64;
            for (cursor_val, bytes) in rows {
                let event: MobEvent = decode_stored_mob_event(&bytes)
                    .map_err(|e| MobStoreError::Serialization(e.to_string()))?;
                if event.timestamp < older_than
                    && !identity_structural_projection_is_anchor(&event.kind)
                {
                    tx.execute(
                        "DELETE FROM mob_events WHERE cursor = ?1",
                        params![cursor_val],
                    )
                    .map_err(se)?;
                    removed = removed.saturating_add(1);
                }
            }
            tx.commit().map_err(se)?;
            Ok(removed)
        })
        .await
    }
}

fn get_next_cursor(conn: &Connection) -> Result<u64, MobStoreError> {
    repair_next_event_cursor(conn)
}

/// Reconcile the cached allocator high-water with the physical event table.
/// Missing, malformed, or regressed metadata can never turn into a permanent
/// primary-key conflict. A valid metadata value above MAX(cursor) is retained
/// so pruning never reuses a previously issued cursor.
fn repair_next_event_cursor(conn: &Connection) -> Result<u64, MobStoreError> {
    let physical_max: Option<i64> = conn
        .query_row("SELECT MAX(cursor) FROM mob_events", [], |row| row.get(0))
        .map_err(se)?;
    let physical_floor = match physical_max {
        Some(max) if max > 0 => {
            let cursor = u64::try_from(max).map_err(se)?;
            if cursor >= i64::MAX as u64 {
                i64::MAX as u64
            } else {
                cursor + 1
            }
        }
        _ => 1,
    };
    let stored: Option<rusqlite::types::Value> = conn
        .query_row(
            "SELECT value FROM mob_event_meta WHERE key = ?1",
            params![EVENT_CURSOR_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(se)?;
    let stored_next = match &stored {
        Some(rusqlite::types::Value::Integer(value)) if *value > 0 => u64::try_from(*value).ok(),
        _ => None,
    };
    let next = stored_next.map_or(physical_floor, |stored| stored.max(physical_floor));
    if stored_next != Some(next) {
        set_next_cursor(conn, next)?;
    }
    Ok(next)
}

fn next_event_cursor(tx: &Transaction<'_>) -> Result<u64, MobStoreError> {
    let cursor = get_next_cursor(tx)?;
    if cursor >= i64::MAX as u64 {
        return Err(MobStoreError::Internal(
            "mob event cursor is exhausted".to_string(),
        ));
    }
    Ok(cursor)
}

fn checked_event_cursor_successor(cursor: u64) -> Result<u64, MobStoreError> {
    cursor
        .checked_add(1)
        .filter(|next| i64::try_from(*next).is_ok())
        .ok_or_else(|| MobStoreError::Internal("mob event cursor is exhausted".to_string()))
}

fn set_next_cursor(conn: &Connection, value: u64) -> Result<(), MobStoreError> {
    conn.execute(
        "INSERT OR REPLACE INTO mob_event_meta (key, value) VALUES (?1, ?2)",
        params![EVENT_CURSOR_KEY, cursor_to_i64(value)?],
    )
    .map_err(se)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// SqliteMobRunStore
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SqliteMobRunStore {
    path: PathBuf,
}

#[derive(Debug, Clone, Copy)]
enum MissingRunCasBehavior {
    ReturnFalse,
    NotFound,
}

impl std::fmt::Debug for SqliteMobRunStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteMobRunStore")
            .field("path", &self.path)
            .finish()
    }
}

impl SqliteMobRunStore {
    async fn update_run_with_authority_if<F>(
        &self,
        run_id: &RunId,
        missing_behavior: MissingRunCasBehavior,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
        update: F,
    ) -> Result<bool, MobStoreError>
    where
        F: FnOnce(&mut MobRun) -> Result<bool, MobStoreError> + Send + 'static,
    {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes = load_run_bytes(&tx, &key)?;
            let Some(bytes) = bytes else {
                return match missing_behavior {
                    MissingRunCasBehavior::ReturnFalse => Ok(false),
                    MissingRunCasBehavior::NotFound => {
                        Err(MobStoreError::NotFound(format!("run not found: {run_id}")))
                    }
                };
            };
            let mut run: MobRun = decode_json(&bytes)?;
            if !update(&mut run)? {
                return Ok(false);
            }
            run.append_flow_authority_inputs(authority_inputs)
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            run.validate_flow_authority_projection()
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            write_run_json(&tx, &key, &run)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }
}

#[async_trait]
impl MobRunStore for SqliteMobRunStore {
    async fn create_run(&self, run: MobRun) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let key = run.run_id.to_string();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            let exists: bool = tx
                .query_row(
                    "SELECT 1 FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |_| Ok(true),
                )
                .optional()
                .map_err(se)?
                .unwrap_or(false);
            if exists {
                return Err(MobStoreError::Internal(format!(
                    "run already exists: {}",
                    run.run_id
                )));
            }
            run.validate_flow_authority_projection()
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;

            let encoded = encode_json(&run)?;
            tx.execute(
                "INSERT INTO mob_runs (run_id, run_json) VALUES (?1, ?2)",
                params![key, encoded],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        run_sqlite_task(move || {
            let conn = open_existing_read_connection(&path)?;
            let bytes: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            match bytes {
                Some(b) => Ok(Some(decode_json(&b)?)),
                None => Ok(None),
            }
        })
        .await
    }

    async fn list_runs(
        &self,
        mob_id: &MobId,
        flow_id: Option<&FlowId>,
    ) -> Result<Vec<MobRun>, MobStoreError> {
        let path = self.path.clone();
        let mob_id = mob_id.clone();
        let flow_id = flow_id.cloned();
        run_sqlite_task(move || {
            let conn = open_existing_read_connection(&path)?;
            let mut stmt = conn.prepare("SELECT run_json FROM mob_runs").map_err(se)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut runs = Vec::new();
            for row in rows {
                let bytes = row.map_err(se)?;
                let run: MobRun = decode_json(&bytes)?;
                if run.mob_id == mob_id && flow_id.as_ref().is_none_or(|fid| run.flow_id == *fid) {
                    runs.push(run);
                }
            }
            Ok(runs)
        })
        .await
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
        let path = self.path.clone();
        let run_key = run_id.to_string();
        let sequence = u64_to_sql_key(intent.obligation.dispatch_sequence);
        let intent = intent.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let run_json: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![run_key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(run_json) = run_json else {
                return Err(MobStoreError::NotFound(format!(
                    "run not found: {}",
                    intent.obligation.run_id
                )));
            };
            let run: MobRun = decode_json(&run_json)?;
            intent
                .validate_for(&run.run_id, &run.mob_id)
                .map_err(MobStoreError::Internal)?;
            let existing: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT intent_json FROM mob_run_remote_turn_intents \
                     WHERE run_id = ?1 AND dispatch_sequence = ?2",
                    params![run_key, sequence],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            if let Some(bytes) = existing {
                let existing: MobRunRemoteTurnIntent = decode_json(&bytes)?;
                if existing != intent {
                    return Err(MobStoreError::Internal(format!(
                        "remote-turn intent sequence {} conflicts for run '{}'",
                        intent.obligation.dispatch_sequence, intent.obligation.run_id
                    )));
                }
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO mob_run_remote_turn_intents \
                 (run_id, dispatch_sequence, intent_json) VALUES (?1, ?2, ?3)",
                params![run_key, sequence, encode_json(&intent)?],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    async fn delete_remote_turn_intent(
        &self,
        run_id: &RunId,
        dispatch_sequence: u64,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let run_key = run_id.to_string();
        let sequence = u64_to_sql_key(dispatch_sequence);
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            Ok(conn
                .execute(
                    "DELETE FROM mob_run_remote_turn_intents \
                     WHERE run_id = ?1 AND dispatch_sequence = ?2",
                    params![run_key, sequence],
                )
                .map_err(se)?
                > 0)
        })
        .await
    }

    async fn list_remote_turn_intents(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<MobRunRemoteTurnIntent>, MobStoreError> {
        let path = self.path.clone();
        let run_key = run_id.to_string();
        run_sqlite_task(move || {
            let conn = open_existing_read_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT intent_json FROM mob_run_remote_turn_intents \
                     WHERE run_id = ?1 ORDER BY dispatch_sequence ASC",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![run_key], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut intents = Vec::new();
            for row in rows {
                intents.push(decode_json(&row.map_err(se)?)?);
            }
            Ok(intents)
        })
        .await
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
        let path = self.path.clone();
        let run_key = run_id.to_string();
        let sequence = u64_to_sql_key(receipt.obligation.dispatch_sequence);
        let receipt = receipt.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let run_json: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![run_key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(run_json) = run_json else {
                return Err(MobStoreError::NotFound(format!(
                    "run not found: {}",
                    receipt.obligation.run_id
                )));
            };
            let run: MobRun = decode_json(&run_json)?;
            let intent_json: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT intent_json FROM mob_run_remote_turn_intents \
                     WHERE run_id = ?1 AND dispatch_sequence = ?2",
                    params![run_key, sequence],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let intent: MobRunRemoteTurnIntent = intent_json
                .map(|bytes| decode_json(&bytes))
                .transpose()?
                .ok_or_else(|| {
                    MobStoreError::Internal(
                        "remote-turn receipt has no exact durable intent".to_string(),
                    )
                })?;
            receipt
                .validate_for(&run.run_id, &run.mob_id, &intent)
                .map_err(MobStoreError::Internal)?;
            let existing: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT receipt_json FROM mob_run_remote_turn_receipts \
                     WHERE run_id = ?1 AND dispatch_sequence = ?2",
                    params![run_key, sequence],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            if let Some(bytes) = existing {
                let existing: MobRunRemoteTurnReceipt = decode_json(&bytes)?;
                if existing != receipt {
                    return Err(MobStoreError::Internal(format!(
                        "remote-turn receipt sequence {} conflicts for run '{}'",
                        receipt.obligation.dispatch_sequence, receipt.obligation.run_id
                    )));
                }
                return Ok(false);
            }
            let bytes = encode_json(&receipt)?;
            tx.execute(
                "INSERT INTO mob_run_remote_turn_receipts \
                 (run_id, dispatch_sequence, receipt_json) VALUES (?1, ?2, ?3)",
                params![run_key, sequence, bytes],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    async fn list_remote_turn_receipts(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<MobRunRemoteTurnReceipt>, MobStoreError> {
        let path = self.path.clone();
        let run_key = run_id.to_string();
        run_sqlite_task(move || {
            let conn = open_existing_read_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT receipt_json FROM mob_run_remote_turn_receipts \
                     WHERE run_id = ?1 ORDER BY dispatch_sequence ASC",
                )
                .map_err(se)?;
            let rows = stmt
                .query_map(params![run_key], |row| row.get::<_, Vec<u8>>(0))
                .map_err(se)?;
            let mut receipts = Vec::new();
            for row in rows {
                receipts.push(decode_json(&row.map_err(se)?)?);
            }
            Ok(receipts)
        })
        .await
    }

    async fn delete_remote_turn_receipt(
        &self,
        run_id: &RunId,
        dispatch_sequence: u64,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let run_key = run_id.to_string();
        let sequence = u64_to_sql_key(dispatch_sequence);
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            Ok(conn
                .execute(
                    "DELETE FROM mob_run_remote_turn_receipts \
                     WHERE run_id = ?1 AND dispatch_sequence = ?2",
                    params![run_key, sequence],
                )
                .map_err(se)?
                > 0)
        })
        .await
    }

    async fn cas_flow_state_with_authority(
        &self,
        run_id: &RunId,
        expected: &flow_run::State,
        next: &flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let expected = expected.clone();
        let next = next.clone();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::ReturnFalse,
            authority_inputs,
            move |run| {
                if run.flow_state != expected {
                    return Ok(false);
                }
                run.flow_state = next;
                Ok(true)
            },
        )
        .await
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
        let expected_flow_state = expected_flow_state.clone();
        let next_flow_state = next_flow_state.clone();
        let terminality_run_id = run_id.clone();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::ReturnFalse,
            authority_inputs,
            move |run| {
                let current_terminal =
                    mob_machine_run_status_is_terminal(&terminality_run_id, &run.status)
                        .map_err(|error| MobStoreError::Internal(error.to_string()))?;
                if run.status != expected_status
                    || current_terminal
                    || run.flow_state != expected_flow_state
                {
                    return Ok(false);
                }
                let terminal =
                    mob_machine_run_status_is_terminal(&terminality_run_id, &next_status)
                        .map_err(|error| MobStoreError::Internal(error.to_string()))?;
                run.status = next_status;
                run.flow_state = next_flow_state;
                if terminal && run.completed_at.is_none() {
                    run.completed_at = Some(Utc::now());
                }
                Ok(true)
            },
        )
        .await
    }

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
        // The mob run table and the mob event log live in the same SQLite
        // database file, so the terminal snapshot and the terminal event are
        // committed in ONE transaction — no divergence window between terminal
        // run-status truth and terminal-event truth. The `_events` handle is
        // unused: any event store for this storage bundle shares this database
        // path, and the committed event is broadcast on the per-path bus.
        validate_mob_event_write_authority(&terminal_event.kind)?;
        if terminal_event_identity(&terminal_event.kind).is_none() {
            return Err(MobStoreError::Internal(
                "cas_run_snapshot_and_append_terminal_event_with_authority requires a terminal flow event".to_string(),
            ));
        }
        let path = self.path.clone();
        let key = run_id.to_string();
        let terminality_run_id = run_id.clone();
        let expected_flow_state = expected_flow_state.clone();
        let next_flow_state = next_flow_state.clone();
        let event_bus = sqlite_event_bus_for_path(self.path.clone())?;
        let stored = run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes = load_run_bytes(&tx, &key)?;
            let Some(bytes) = bytes else {
                return Ok(None);
            };
            let mut run: MobRun = decode_json(&bytes)?;
            let current_terminal =
                mob_machine_run_status_is_terminal(&terminality_run_id, &run.status)
                    .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            if run.status != expected_status
                || current_terminal
                || run.flow_state != expected_flow_state
            {
                return Ok(None);
            }
            let terminal = mob_machine_run_status_is_terminal(&terminality_run_id, &next_status)
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            run.status = next_status;
            run.flow_state = next_flow_state;
            if terminal && run.completed_at.is_none() {
                run.completed_at = Some(Utc::now());
            }
            run.append_flow_authority_inputs(authority_inputs)
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            run.validate_flow_authority_projection()
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            write_run_json(&tx, &key, &run)?;

            let cursor = next_event_cursor(&tx)?;
            let stored = MobEvent {
                cursor,
                timestamp: terminal_event.timestamp.unwrap_or_else(Utc::now),
                mob_id: terminal_event.mob_id,
                kind: terminal_event.kind,
            };
            let encoded = encode_stored_mob_event(&stored)
                .map_err(|e| MobStoreError::Serialization(e.to_string()))?;
            tx.execute(
                "INSERT INTO mob_events (cursor, mob_id, event_json) VALUES (?1, ?2, ?3)",
                params![cursor_to_i64(cursor)?, stored.mob_id.as_str(), encoded],
            )
            .map_err(se)?;
            set_next_cursor(&tx, checked_event_cursor_successor(cursor)?)?;
            tx.commit().map_err(se)?;
            Ok(Some(stored))
        })
        .await?;
        if let Some(stored) = stored.as_ref() {
            event_bus.publish_committed(stored.clone());
        }
        Ok(stored)
    }

    async fn append_step_entry_with_authority(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            authority
                .validate_step_entry(&run, &entry)
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            run.step_ledger.push(entry);
            run.validate_flow_authority_projection()
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn append_step_entry_if_absent_with_authority(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            authority
                .validate_step_entry(&run, &entry)
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            let is_duplicate = run.step_ledger.iter().any(|existing| {
                existing.step_id == entry.step_id
                    && existing.agent_identity == entry.agent_identity
                    && existing.status == entry.status
            });
            if is_duplicate {
                return Ok(false);
            }
            run.step_ledger.push(entry);
            run.validate_flow_authority_projection()
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    async fn append_failure_entry_with_authority(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<(), MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            authority
                .validate_failure_entry(&run, &entry)
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            run.failure_ledger.push(entry);
            run.validate_flow_authority_projection()
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
    }

    async fn append_failure_entry_if_absent_with_authority(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = run_id.to_string();
        let run_id = run_id.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;
            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT run_json FROM mob_runs WHERE run_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Err(MobStoreError::NotFound(format!("run not found: {run_id}")));
            };
            let mut run: MobRun = decode_json(&bytes)?;
            authority
                .validate_failure_entry(&run, &entry)
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
            run.failure_ledger.push(entry);
            run.validate_flow_authority_projection()
                .map_err(|error| MobStoreError::Internal(error.to_string()))?;
            let encoded = encode_json(&run)?;
            tx.execute(
                "UPDATE mob_runs SET run_json = ?1 WHERE run_id = ?2",
                params![encoded, key],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }

    async fn cas_frame_state_with_authority(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        expected: Option<&FrameSnapshot>,
        next: FrameSnapshot,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let frame_id = frame_id.clone();
        let expected = expected.cloned();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::NotFound,
            authority_inputs,
            move |run| {
                let current = run.frames.get(&frame_id);
                let matches = match (expected.as_ref(), current) {
                    (None, None) => true,
                    (Some(exp), Some(cur)) => exp == cur,
                    _ => false,
                };
                if !matches {
                    return Ok(false);
                }
                run.frames.insert(frame_id, next);
                Ok(true)
            },
        )
        .await
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
        let expected_run_state = expected_run_state.clone();
        let frame_id = frame_id.clone();
        let expected_frame = expected_frame.clone();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::NotFound,
            authority_inputs,
            move |run| {
                if run.flow_state != expected_run_state {
                    return Ok(false);
                }
                if run.frames.get(&frame_id) != Some(&expected_frame) {
                    return Ok(false);
                }
                run.flow_state = next_run_state;
                run.frames.insert(frame_id, next_frame);
                Ok(true)
            },
        )
        .await
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
        let frame_id = frame_id.clone();
        let expected_frame = expected_frame.clone();
        let loop_context = loop_context.map(|(loop_id, iteration)| (loop_id.clone(), iteration));
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::NotFound,
            authority_inputs,
            move |run| {
                if run.frames.get(&frame_id) != Some(&expected_frame) {
                    return Ok(false);
                }
                run.frames.insert(frame_id, next_frame);
                match loop_context {
                    None => {
                        run.root_step_outputs
                            .insert(StepId::from(step_output_key.as_str()), step_output);
                    }
                    Some((loop_id, iteration)) => {
                        let iteration_index = usize::try_from(iteration).map_err(|_| {
                            MobStoreError::Internal(format!(
                                "loop iteration index {iteration} exceeds usize::MAX on this target"
                            ))
                        })?;
                        let outputs = run.loop_iteration_outputs.entry(loop_id).or_default();
                        while outputs.len() <= iteration_index {
                            outputs.push(indexmap::IndexMap::new());
                        }
                        outputs[iteration_index]
                            .insert(StepId::from(step_output_key.as_str()), step_output);
                    }
                }
                Ok(true)
            },
        )
        .await
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
        let loop_instance_id = loop_instance_id.clone();
        let expected_run_state = expected_run_state.clone();
        let frame_id = frame_id.clone();
        let expected_frame = expected_frame.clone();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::NotFound,
            authority_inputs,
            move |run| {
                if run.flow_state != expected_run_state {
                    return Ok(false);
                }
                if run.frames.get(&frame_id) != Some(&expected_frame) {
                    return Ok(false);
                }
                if run.loops.contains_key(&loop_instance_id) {
                    return Ok(false);
                }
                run.flow_state = next_run_state;
                run.frames.insert(frame_id, next_frame);
                run.loops.insert(loop_instance_id, initial_loop);
                Ok(true)
            },
        )
        .await
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
        let loop_instance_id = loop_instance_id.clone();
        let expected_loop = expected_loop.clone();
        let expected_run_state = expected_run_state.clone();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::NotFound,
            authority_inputs,
            move |run| {
                if run.flow_state != expected_run_state {
                    return Ok(false);
                }
                if run.loops.get(&loop_instance_id) != Some(&expected_loop) {
                    return Ok(false);
                }
                run.flow_state = next_run_state;
                run.loops.insert(loop_instance_id, next_loop);
                Ok(true)
            },
        )
        .await
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
        let loop_instance_id = loop_instance_id.clone();
        let expected_loop = expected_loop.clone();
        let frame_id = frame_id.clone();
        let expected_run_state = expected_run_state.clone();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::NotFound,
            authority_inputs,
            move |run| {
                if run.flow_state != expected_run_state {
                    return Ok(false);
                }
                if run.loops.get(&loop_instance_id) != Some(&expected_loop) {
                    return Ok(false);
                }
                if run.frames.contains_key(&frame_id) {
                    return Ok(false);
                }
                run.flow_state = next_run_state;
                run.loops.insert(loop_instance_id, next_loop);
                run.frames.insert(frame_id, initial_frame);
                append_loop_iteration_ledger_if_absent(run, ledger_entry);
                Ok(true)
            },
        )
        .await
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
        let loop_instance_id = loop_instance_id.clone();
        let expected_loop = expected_loop.clone();
        let frame_id = frame_id.clone();
        let expected_frame = expected_frame.clone();
        let expected_run_state = expected_run_state.clone();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::NotFound,
            authority_inputs,
            move |run| {
                if run.flow_state != expected_run_state {
                    return Ok(false);
                }
                if run.loops.get(&loop_instance_id) != Some(&expected_loop) {
                    return Ok(false);
                }
                if run.frames.get(&frame_id) != Some(&expected_frame) {
                    return Ok(false);
                }
                run.flow_state = next_run_state;
                run.loops.insert(loop_instance_id, next_loop);
                run.frames.insert(frame_id, next_frame);
                Ok(true)
            },
        )
        .await
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
        let loop_instance_id = loop_instance_id.clone();
        let expected_loop = expected_loop.clone();
        let frame_id = frame_id.clone();
        let expected_frame = expected_frame.clone();
        let expected_run_state = expected_run_state.clone();
        self.update_run_with_authority_if(
            run_id,
            MissingRunCasBehavior::NotFound,
            authority_inputs,
            move |run| {
                if run.flow_state != expected_run_state {
                    return Ok(false);
                }
                if run.loops.get(&loop_instance_id) != Some(&expected_loop) {
                    return Ok(false);
                }
                if run.frames.get(&frame_id) != Some(&expected_frame) {
                    return Ok(false);
                }
                run.flow_state = next_run_state;
                run.loops.insert(loop_instance_id, next_loop);
                run.frames.insert(frame_id, next_frame);
                Ok(true)
            },
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// SqliteMobSpecStore
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SqliteMobSpecStore {
    path: PathBuf,
}

impl std::fmt::Debug for SqliteMobSpecStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteMobSpecStore")
            .field("path", &self.path)
            .finish()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredSpec {
    definition: MobDefinition,
    revision: u64,
}

#[async_trait]
impl MobSpecStore for SqliteMobSpecStore {
    async fn put_spec(
        &self,
        mob_id: &MobId,
        definition: &MobDefinition,
        revision: Option<u64>,
    ) -> Result<u64, MobStoreError> {
        let path = self.path.clone();
        let key = mob_id.to_string();
        let mob_id = mob_id.clone();
        let definition = definition.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            let current: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT spec_json FROM mob_specs WHERE mob_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let current_revision = match current {
                Some(bytes) => decode_json::<StoredSpec>(&bytes)?.revision,
                None => 0,
            };

            if let Some(expected) = revision
                && expected != current_revision
            {
                return Err(MobStoreError::SpecRevisionConflict {
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
            tx.execute(
                "INSERT OR REPLACE INTO mob_specs (mob_id, spec_json) VALUES (?1, ?2)",
                params![key, encoded],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(next_revision)
        })
        .await
    }

    async fn get_spec(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<(MobDefinition, u64)>, MobStoreError> {
        let path = self.path.clone();
        let key = mob_id.to_string();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let bytes: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT spec_json FROM mob_specs WHERE mob_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            match bytes {
                Some(b) => {
                    let stored: StoredSpec = decode_json(&b)?;
                    Ok(Some((stored.definition, stored.revision)))
                }
                None => Ok(None),
            }
        })
        .await
    }

    async fn list_specs(&self) -> Result<Vec<MobId>, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn.prepare("SELECT mob_id FROM mob_specs").map_err(se)?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(se)?;
            let mut result = Vec::new();
            for row in rows {
                let id = row.map_err(se)?;
                result.push(MobId::from(id));
            }
            Ok(result)
        })
        .await
    }

    async fn delete_spec(
        &self,
        mob_id: &MobId,
        revision: Option<u64>,
    ) -> Result<bool, MobStoreError> {
        let path = self.path.clone();
        let key = mob_id.to_string();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            let bytes: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT spec_json FROM mob_specs WHERE mob_id = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(se)?;
            let Some(bytes) = bytes else {
                return Ok(false);
            };
            let stored: StoredSpec = decode_json(&bytes)?;
            if let Some(expected) = revision
                && expected != stored.revision
            {
                return Ok(false);
            }

            tx.execute("DELETE FROM mob_specs WHERE mob_id = ?1", params![key])
                .map_err(se)?;
            tx.commit().map_err(se)?;
            Ok(true)
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// SqliteRealmProfileStore
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SqliteRealmProfileStore {
    path: PathBuf,
}

impl SqliteRealmProfileStore {
    /// Open a standalone realm profile store at the given database path.
    ///
    /// Creates the parent directory and initializes the schema if needed.
    pub fn open(db_path: &std::path::Path) -> Result<Self, MobStoreError> {
        // Same opener as every operation on this store (which shares the
        // bundle opener): one connection policy, one schema domain.
        let conn = open_connection(db_path)?;
        drop(conn);
        Ok(Self {
            path: db_path.to_path_buf(),
        })
    }
}

impl std::fmt::Debug for SqliteRealmProfileStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteRealmProfileStore")
            .field("path", &self.path)
            .finish()
    }
}

#[async_trait]
impl RealmProfileStore for SqliteRealmProfileStore {
    async fn create(
        &self,
        name: &str,
        profile: &Profile,
    ) -> Result<StoredRealmProfile, MobStoreError> {
        let path = self.path.clone();
        let name = name.to_string();
        let profile = profile.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            let exists: bool = tx
                .query_row(
                    "SELECT 1 FROM realm_profiles WHERE name = ?1",
                    params![name],
                    |_| Ok(true),
                )
                .optional()
                .map_err(se)?
                .unwrap_or(false);

            if exists {
                return Err(MobStoreError::CasConflict(format!(
                    "realm profile already exists: {name}"
                )));
            }

            let now = Utc::now();
            let now_str = now.to_rfc3339();
            let profile_json = encode_json(&profile)?;

            tx.execute(
                "INSERT INTO realm_profiles (name, profile_json, revision, created_at, updated_at) VALUES (?1, ?2, 1, ?3, ?4)",
                params![name, profile_json, now_str, now_str],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;

            Ok(StoredRealmProfile {
                name,
                profile,
                revision: 1,
                created_at: now,
                updated_at: now,
            })
        })
        .await
    }

    async fn get(&self, name: &str) -> Result<Option<StoredRealmProfile>, MobStoreError> {
        let path = self.path.clone();
        let name = name.to_string();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let row: Option<(Vec<u8>, i64, String, String)> = conn
                .query_row(
                    "SELECT profile_json, revision, created_at, updated_at FROM realm_profiles WHERE name = ?1",
                    params![name],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(se)?;

            match row {
                Some((bytes, revision, created_at_str, updated_at_str)) => {
                    let profile: Profile = decode_json(&bytes)?;
                    let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
                        .map_err(|e| MobStoreError::Serialization(e.to_string()))?
                        .with_timezone(&Utc);
                    let updated_at = chrono::DateTime::parse_from_rfc3339(&updated_at_str)
                        .map_err(|e| MobStoreError::Serialization(e.to_string()))?
                        .with_timezone(&Utc);
                    Ok(Some(StoredRealmProfile {
                        name,
                        profile,
                        revision: revision as u64,
                        created_at,
                        updated_at,
                    }))
                }
                None => Ok(None),
            }
        })
        .await
    }

    async fn list(&self) -> Result<Vec<StoredRealmProfile>, MobStoreError> {
        let path = self.path.clone();
        run_sqlite_task(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare("SELECT name, profile_json, revision, created_at, updated_at FROM realm_profiles ORDER BY name")
                .map_err(se)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })
                .map_err(se)?;

            let mut result = Vec::new();
            for row in rows {
                let (name, bytes, revision, created_at_str, updated_at_str) = row.map_err(se)?;
                let profile: Profile = decode_json(&bytes)?;
                let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
                    .map_err(|e| MobStoreError::Serialization(e.to_string()))?
                    .with_timezone(&Utc);
                let updated_at = chrono::DateTime::parse_from_rfc3339(&updated_at_str)
                    .map_err(|e| MobStoreError::Serialization(e.to_string()))?
                    .with_timezone(&Utc);
                result.push(StoredRealmProfile {
                    name,
                    profile,
                    revision: revision as u64,
                    created_at,
                    updated_at,
                });
            }
            Ok(result)
        })
        .await
    }

    async fn update(
        &self,
        name: &str,
        profile: &Profile,
        expected_revision: u64,
    ) -> Result<StoredRealmProfile, MobStoreError> {
        let path = self.path.clone();
        let name = name.to_string();
        let profile = profile.clone();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            let row: Option<(i64, String)> = tx
                .query_row(
                    "SELECT revision, created_at FROM realm_profiles WHERE name = ?1",
                    params![name],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(se)?;

            let (current_revision, created_at_str) = row.ok_or_else(|| {
                MobStoreError::NotFound(format!("realm profile not found: {name}"))
            })?;

            if current_revision as u64 != expected_revision {
                return Err(MobStoreError::CasConflict(format!(
                    "realm profile '{name}' revision conflict: expected {expected_revision}, actual {current_revision}"
                )));
            }

            let next_revision = expected_revision + 1;
            let now = Utc::now();
            let now_str = now.to_rfc3339();
            let profile_json = encode_json(&profile)?;

            tx.execute(
                "UPDATE realm_profiles SET profile_json = ?1, revision = ?2, updated_at = ?3 WHERE name = ?4",
                params![profile_json, next_revision as i64, now_str, name],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;

            let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
                .map_err(|e| MobStoreError::Serialization(e.to_string()))?
                .with_timezone(&Utc);

            Ok(StoredRealmProfile {
                name,
                profile,
                revision: next_revision,
                created_at,
                updated_at: now,
            })
        })
        .await
    }

    async fn delete(
        &self,
        name: &str,
        expected_revision: u64,
    ) -> Result<StoredRealmProfile, MobStoreError> {
        let path = self.path.clone();
        let name = name.to_string();
        run_sqlite_task(move || {
            let mut conn = open_connection(&path)?;
            let tx = begin_immediate(&mut conn)?;

            let row: Option<(Vec<u8>, i64, String, String)> = tx
                .query_row(
                    "SELECT profile_json, revision, created_at, updated_at FROM realm_profiles WHERE name = ?1",
                    params![name],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(se)?;

            let (bytes, current_revision, created_at_str, updated_at_str) = row.ok_or_else(|| {
                MobStoreError::NotFound(format!("realm profile not found: {name}"))
            })?;

            if current_revision as u64 != expected_revision {
                return Err(MobStoreError::CasConflict(format!(
                    "realm profile '{name}' revision conflict: expected {expected_revision}, actual {current_revision}"
                )));
            }

            let profile: Profile = decode_json(&bytes)?;
            tx.execute(
                "DELETE FROM realm_profiles WHERE name = ?1",
                params![name],
            )
            .map_err(se)?;
            tx.commit().map_err(se)?;

            let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
                .map_err(|e| MobStoreError::Serialization(e.to_string()))?
                .with_timezone(&Utc);
            let updated_at = chrono::DateTime::parse_from_rfc3339(&updated_at_str)
                .map_err(|e| MobStoreError::Serialization(e.to_string()))?
                .with_timezone(&Utc);

            Ok(StoredRealmProfile {
                name,
                profile,
                revision: expected_revision,
                created_at,
                updated_at,
            })
        })
        .await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::definition::{BackendConfig, FlowSpec, WiringRules};
    use crate::event::{MemberRef, MobEventKind};
    use crate::identity::{
        DesiredMemberMaterial, DesiredMemberOverlay, DesiredSessionAuthorityPolicy,
        IDENTITY_OPERATION_RECEIPT_SCHEMA_VERSION, IdentityDeclarationManifest,
        IdentityDeclarationMemberPlan, IdentityDeclarationScopePrecondition,
        IdentityMemberDeclaration, IdentityMemberMaterialDeclaration,
        IdentityOperationReceiptPayload,
    };
    use crate::ids::{AgentIdentity, Generation, ProfileName};
    use crate::profile::{Profile, ProfileBinding, ToolConfig};
    use crate::run::StepRunStatus;
    use crate::store::ExternalBindingOverlayStatus;
    use futures::future::join_all;
    use indexmap::IndexMap;
    use meerkat_contracts::wire::{
        PortableDefinitionExtract, PortableProfile, PortableSystemPrompt,
    };
    use meerkat_core::lifecycle::InputId;
    use meerkat_core::ops::OperationId;
    use meerkat_core::{ContentInput, Provider, SessionId, SessionLineageId};
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

    fn sqlite_identity_material(model: &str) -> DesiredMemberMaterial {
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

    fn sqlite_identity_plan(
        scope: &str,
        operation_id: OperationId,
        expected_scope: IdentityDeclarationScopePrecondition,
        members: Vec<(AgentIdentity, DesiredMemberMaterial, Option<ContentInput>)>,
    ) -> IdentityDeclarationApplyPlan {
        let mut declarations = BTreeMap::new();
        let mut compiled = BTreeMap::new();
        for (identity, material, initial_message) in members {
            declarations.insert(
                identity.clone(),
                IdentityMemberDeclaration {
                    material: IdentityMemberMaterialDeclaration::Resolved {
                        material: material.clone(),
                    },
                    session_authority_policy: DesiredSessionAuthorityPolicy::CreateIfAbsent,
                    initial_message: initial_message.clone(),
                    legacy_import: None,
                },
            );
            compiled.insert(
                identity,
                IdentityDeclarationMemberPlan {
                    material,
                    session_authority_policy: DesiredSessionAuthorityPolicy::CreateIfAbsent,
                    initial_message: initial_message.clone(),
                    candidate_session_id: SessionId::new(),
                    candidate_lineage_id: SessionLineageId::new(format!(
                        "sqlite-identity-lineage-{}",
                        uuid::Uuid::new_v4()
                    ))
                    .unwrap(),
                    candidate_initial_delivery_id: initial_message.map(|_| InputId::new()),
                },
            );
        }
        let manifest = IdentityDeclarationManifest {
            scope_id: IdentityDeclarationScopeId::new(scope).unwrap(),
            operation_id,
            expected_scope,
            members: declarations,
            wiring: BTreeSet::new(),
        };
        IdentityDeclarationApplyPlan::from_compiled_manifest(&manifest, compiled).unwrap()
    }

    fn sqlite_initial_delivery_receipt(
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
            panic!("initial delivery fixture requires a present intent");
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

    fn sqlite_receipt_permit(
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

    fn default_bridge_protocol_version()
    -> meerkat_contracts::wire::supervisor_bridge::BridgeProtocolVersion {
        meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_default_protocol_version()
    }

    fn temp_db_path() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mob.db");
        (dir, path)
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

    fn seed_operator_requests(
        path: &Path,
        mob_id: &MobId,
        records: &[MobMemberOperatorRequestRecord],
    ) {
        let mut conn = open_connection(path).expect("open quota seed connection");
        let tx = begin_immediate(&mut conn).expect("begin quota seed transaction");
        {
            let mut insert = tx
                .prepare(
                    "INSERT INTO mob_runtime_member_operator_requests
                     (mob_id, agent_identity, generation, fence_token, host_id, binding_generation, member_session_id, request_id, record_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )
                .expect("prepare quota seed insert");
            for record in records {
                record.validate_pending().expect("valid quota seed row");
                insert
                    .execute(params![
                        mob_id.as_str(),
                        &record.agent_identity,
                        u64_to_sql_key(record.generation),
                        u64_to_sql_key(record.fence_token),
                        &record.host_id,
                        u64_to_sql_key(record.host_binding_generation),
                        &record.member_session_id,
                        &record.request_id,
                        encode_json(record).expect("encode quota seed row"),
                    ])
                    .expect("insert quota seed row");
            }
        }
        tx.commit().expect("commit quota seed transaction");
    }

    fn operator_request_count(path: &Path, mob_id: &MobId) -> usize {
        let conn = open_connection(path).expect("open quota count connection");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mob_runtime_member_operator_requests WHERE mob_id = ?1",
                params![mob_id.as_str()],
                |row| row.get(0),
            )
            .expect("count operator request rows");
        usize::try_from(count).expect("non-negative operator request count")
    }

    fn sample_definition() -> MobDefinition {
        let mut profiles = std::collections::BTreeMap::new();
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
        definition.flows = {
            let mut flows = std::collections::BTreeMap::new();
            flows.insert(
                FlowId::from("flow-a"),
                FlowSpec::new(None, IndexMap::new(), None),
            );
            flows
        };
        definition
    }

    fn sample_run(status: MobRunStatus) -> MobRun {
        MobRun::authority_backed_for_steps(
            RunId::new(),
            MobId::from("mob"),
            FlowId::from("flow-a"),
            [crate::ids::StepId::from("step-1")],
            status,
            serde_json::json!({"a":1}),
        )
        .expect("authority-backed sample run")
    }

    #[tokio::test]
    async fn sqlite_identity_empty_scope_reopens_and_replays_exact_outcome() {
        let (_dir, path) = temp_db_path();
        let mob_id = MobId::from("sqlite-identity-empty-scope");
        let first = sqlite_identity_plan(
            "provider-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Missing,
            Vec::new(),
        );
        let stores = SqliteMobStores::open(&path).unwrap();
        let first_outcome = stores
            .identity_store()
            .apply_identity_declaration(&mob_id, &first)
            .await
            .unwrap();
        assert_eq!(first_outcome.scope_revision, 1);
        drop(stores);

        let reopened = SqliteMobStores::open(&path).unwrap().identity_store();
        assert_eq!(
            reopened
                .apply_identity_declaration(&mob_id, &first)
                .await
                .unwrap(),
            first_outcome
        );
        let second = sqlite_identity_plan(
            "provider-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Revision { revision: 1 },
            Vec::new(),
        );
        assert_eq!(
            reopened
                .apply_identity_declaration(&mob_id, &second)
                .await
                .unwrap()
                .scope_revision,
            2
        );
    }

    #[tokio::test]
    async fn sqlite_identity_session_and_lineage_allocation_is_global_to_database() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().identity_store();
        let first_identity = AgentIdentity::from("member-a");
        let first = sqlite_identity_plan(
            "provider-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Missing,
            vec![(
                first_identity.clone(),
                sqlite_identity_material("model-a"),
                None,
            )],
        );
        let first_outcome = store
            .apply_identity_declaration(&MobId::from("mob-a"), &first)
            .await
            .unwrap();
        let IdentityIntent::Present { session, .. } =
            &first_outcome.identities[&first_identity].intent
        else {
            panic!("present intent");
        };

        let second_identity = AgentIdentity::from("member-b");
        let mut second = sqlite_identity_plan(
            "provider-b",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Missing,
            vec![(
                second_identity.clone(),
                sqlite_identity_material("model-b"),
                None,
            )],
        );
        let member = second.members.get_mut(&second_identity).unwrap();
        member.candidate_session_id = session.session_id.clone();
        member.candidate_lineage_id = session.lineage_id.clone();
        assert!(matches!(
            store
                .apply_identity_declaration(&MobId::from("mob-b"), &second)
                .await,
            Err(MobStoreError::CasConflict(_))
        ));
    }

    #[tokio::test]
    async fn sqlite_identity_live_handle_never_recreates_a_deleted_database() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).unwrap();
        let store = stores.identity_store();
        drop(stores);
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(
            store
                .observe_identity_intent(
                    &MobId::from("deleted-db"),
                    &AgentIdentity::from("member-a"),
                )
                .await,
            Err(MobStoreError::ReadFailed(_))
        ));
        assert!(
            !path.exists(),
            "identity read must not recreate the database"
        );
    }

    #[tokio::test]
    async fn sqlite_identity_unrelated_malformed_row_is_total_and_does_not_block_allocation() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).unwrap();
        let mob_id = MobId::from("sqlite-identity-malformed");
        let malformed_bytes = b"{".to_vec();
        {
            let conn = open_connection(&stores.path).unwrap();
            conn.execute(
                "INSERT INTO mob_identity_intents
                     (mob_id, agent_identity, declaration_scope, session_id, lineage_id, record_json)
                 VALUES (?1, ?2, ?3, NULL, NULL, ?4)",
                params![mob_id.as_str(), "unrelated-garbage", "other-scope", malformed_bytes],
            )
            .unwrap();
        }
        let identity = AgentIdentity::from("member-a");
        let plan = sqlite_identity_plan(
            "provider-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Missing,
            vec![(identity.clone(), sqlite_identity_material("model-a"), None)],
        );
        stores
            .identity_store()
            .apply_identity_declaration(&mob_id, &plan)
            .await
            .unwrap();
        assert!(matches!(
            stores
                .identity_store()
                .observe_identity_intent(&mob_id, &identity)
                .await
                .unwrap(),
            IdentityStoredObservation::Valid(_)
        ));
        match stores
            .identity_store()
            .observe_identity_intent(&mob_id, &AgentIdentity::from("unrelated-garbage"))
            .await
            .unwrap()
        {
            IdentityStoredObservation::Malformed {
                evidence_digest, ..
            } => assert_eq!(evidence_digest, identity_raw_evidence_digest(b"{")),
            other => panic!("expected malformed raw row, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn sqlite_identity_future_schema_is_unsupported_before_typed_decode() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).unwrap();
        let mob_id = MobId::from("sqlite-identity-future-schema");
        let bytes = br#"{"schema_version":2,"future_shape":true}"#.to_vec();
        {
            let conn = open_connection(&stores.path).unwrap();
            conn.execute(
                "INSERT INTO mob_identity_intents
                     (mob_id, agent_identity, declaration_scope, session_id, lineage_id, record_json)
                 VALUES (?1, ?2, NULL, NULL, NULL, ?3)",
                params![mob_id.as_str(), "future-member", bytes],
            )
            .unwrap();
        }
        match stores
            .identity_store()
            .observe_identity_intent(&mob_id, &AgentIdentity::from("future-member"))
            .await
            .unwrap()
        {
            IdentityStoredObservation::Unsupported {
                evidence_digest, ..
            } => assert_eq!(
                evidence_digest,
                identity_raw_evidence_digest(br#"{"schema_version":2,"future_shape":true}"#)
            ),
            other => panic!("expected unsupported future schema, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn sqlite_identity_lease_takeover_strictly_advances_epoch() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).unwrap();
        let clock = Arc::new(TestIdentityClock::new(100));
        let store = SqliteMobIdentityStore::with_clock(
            stores.path.clone(),
            stores.identity_store_instance_id.clone(),
            clock.clone(),
        );
        let mob_id = MobId::from("sqlite-identity-lease");
        let identity = AgentIdentity::from("member-a");
        let first = match store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-a", 10)
            .await
            .unwrap()
        {
            IdentityLeaseClaimOutcome::Acquired(claim) => claim,
            other => panic!("expected acquire, got {other:?}"),
        };
        assert!(matches!(
            store
                .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-b", 10)
                .await
                .unwrap(),
            IdentityLeaseClaimOutcome::HeldByOther(_)
        ));
        clock.set(first.expires_at_ms);
        let takeover = match store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-b", 10)
            .await
            .unwrap()
        {
            IdentityLeaseClaimOutcome::Acquired(claim) => claim,
            other => panic!("expected takeover, got {other:?}"),
        };
        assert_eq!(takeover.epoch, first.epoch + 1);
        assert!(
            !store
                .release_identity_lease(&mob_id, &identity, &first)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn sqlite_identity_lease_refuses_to_overwrite_malformed_authority() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).unwrap();
        let clock = Arc::new(TestIdentityClock::new(100));
        let store = SqliteMobIdentityStore::with_clock(
            stores.path.clone(),
            stores.identity_store_instance_id.clone(),
            clock.clone(),
        );
        let mob_id = MobId::from("sqlite-identity-lease-projection-repair");
        let identity = AgentIdentity::from("member-a");
        let first = match store
            .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-a", 10)
            .await
            .unwrap()
        {
            IdentityLeaseClaimOutcome::Acquired(claim) => claim,
            other => panic!("expected acquire, got {other:?}"),
        };
        {
            let conn = open_connection(&stores.path).unwrap();
            conn.execute(
                "UPDATE mob_identity_leases SET record_json = ?3
                 WHERE mob_id = ?1 AND agent_identity = ?2",
                params![mob_id.as_str(), identity.as_str(), b"{".as_slice()],
            )
            .unwrap();
        }
        assert!(matches!(
            store
                .observe_identity_lease(&mob_id, &identity)
                .await
                .unwrap(),
            IdentityStoredObservation::Malformed { .. }
        ));
        clock.set(first.expires_at_ms);
        assert!(matches!(
            store
                .claim_or_renew_identity_lease(&mob_id, &identity, "controller", "inc-b", 10)
                .await,
            Err(MobStoreError::IdentityAuthorityBlocked { .. })
        ));
        assert!(matches!(
            store
                .observe_identity_lease(&mob_id, &identity)
                .await
                .unwrap(),
            IdentityStoredObservation::Malformed { .. }
        ));
    }

    #[tokio::test]
    async fn sqlite_identity_receipt_insert_fences_stale_intent_and_holder_but_replays() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).unwrap();
        let clock = Arc::new(TestIdentityClock::new(100));
        let store = SqliteMobIdentityStore::with_clock(
            stores.path.clone(),
            stores.identity_store_instance_id.clone(),
            clock.clone(),
        );
        let mob_id = MobId::from("sqlite-identity-receipt-fence");
        let identity = AgentIdentity::from("member-a");
        let first = sqlite_identity_plan(
            "provider-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Missing,
            vec![(
                identity.clone(),
                sqlite_identity_material("model-a"),
                Some(ContentInput::from("deliver once")),
            )],
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
        let stale_intent_receipt = sqlite_initial_delivery_receipt(&mob_id, &first_record);
        let stale_intent_permit = sqlite_receipt_permit(&mob_id, &first_record, &claim_a);

        let update = sqlite_identity_plan(
            "provider-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Revision { revision: 1 },
            vec![(
                identity.clone(),
                sqlite_identity_material("model-b"),
                Some(ContentInput::from("deliver once")),
            )],
        );
        store
            .apply_identity_declaration(&mob_id, &update)
            .await
            .unwrap();
        assert!(matches!(
            store
                .insert_identity_operation_receipt_if_absent(
                    &stale_intent_receipt,
                    &stale_intent_permit,
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
            other => panic!("expected current intent, got {other:?}"),
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
        let receipt = sqlite_initial_delivery_receipt(&mob_id, &current_record);
        let stale_lease_permit = sqlite_receipt_permit(&mob_id, &current_record, &claim_a);
        assert!(matches!(
            store
                .insert_identity_operation_receipt_if_absent(&receipt, &stale_lease_permit)
                .await,
            Err(MobStoreError::CasConflict(_))
        ));
        let current_permit = sqlite_receipt_permit(&mob_id, &current_record, &claim_b);
        assert!(matches!(
            store
                .insert_identity_operation_receipt_if_absent(&receipt, &current_permit)
                .await
                .unwrap(),
            IdentityOperationReceiptInsertOutcome::Inserted(_)
        ));
        clock.set(claim_b.expires_at_ms);
        let _ = store
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

    #[tokio::test]
    async fn sqlite_identity_cross_mob_transplant_is_malformed_and_cannot_authorize_cleanup() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).unwrap();
        let store = stores.identity_store();
        let donor_mob = MobId::from("sqlite-identity-donor");
        let recipient_mob = MobId::from("sqlite-identity-recipient");
        let identity = AgentIdentity::from("member-a");
        let present = sqlite_identity_plan(
            "provider-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Missing,
            vec![(identity.clone(), sqlite_identity_material("model-a"), None)],
        );
        store
            .apply_identity_declaration(&donor_mob, &present)
            .await
            .unwrap();
        let absent = sqlite_identity_plan(
            "provider-a",
            OperationId::new(),
            IdentityDeclarationScopePrecondition::Revision { revision: 1 },
            Vec::new(),
        );
        store
            .apply_identity_declaration(&donor_mob, &absent)
            .await
            .unwrap();
        let donor_record = match store
            .observe_identity_intent(&donor_mob, &identity)
            .await
            .unwrap()
        {
            IdentityStoredObservation::Valid(record) => record,
            other => panic!("expected donor tombstone, got {other:?}"),
        };
        assert!(matches!(donor_record.intent, IdentityIntent::Absent { .. }));
        assert!(matches!(
            donor_record.retirement_plan,
            IdentityRetirementPlan::Targets { .. }
        ));

        {
            let conn = Connection::open(&path).unwrap();
            conn.execute(
                "UPDATE mob_identity_intents SET mob_id = ?1
                 WHERE mob_id = ?2 AND agent_identity = ?3",
                params![
                    recipient_mob.as_str(),
                    donor_mob.as_str(),
                    identity.as_str()
                ],
            )
            .unwrap();
        }

        let transplanted = store
            .observe_identity_intent(&recipient_mob, &identity)
            .await
            .unwrap();
        assert!(matches!(
            transplanted,
            IdentityStoredObservation::Malformed { .. }
        ));
        assert!(matches!(
            store
                .list_identity_intents(&recipient_mob)
                .await
                .unwrap()
                .get(&identity),
            Some(IdentityStoredObservation::Malformed { .. })
        ));

        let claim = match store
            .claim_or_renew_identity_lease(
                &recipient_mob,
                &identity,
                "controller",
                "recipient-incarnation",
                1_000,
            )
            .await
            .unwrap()
        {
            IdentityLeaseClaimOutcome::Acquired(claim) => claim,
            other => panic!("expected recipient lease claim, got {other:?}"),
        };
        let permit = IdentityActuationPermit {
            mob_id: recipient_mob,
            identity,
            target: IdentityActuatorTarget::Wiring,
            intent_revision: donor_record.intent_revision,
            intent_digest: donor_record.intent_digest,
            intent_authority_digest: donor_record.authority_digest,
            lease_epoch: claim.epoch,
            lease_holder_id: claim.holder_id,
            lease_incarnation_id: claim.incarnation_id,
            lease_expires_at_ms: claim.expires_at_ms,
            target_observation: IdentityTargetObservationVersion::Absent {
                absence_version: identity_raw_evidence_digest(b"recipient-wiring-absent"),
            },
        };
        assert!(matches!(
            store.validate_identity_actuation_permit(&permit).await,
            Err(MobStoreError::IdentityAuthorityBlocked { .. })
        ));
    }

    #[tokio::test]
    async fn sqlite_identity_wiring_is_mob_local_and_reset_heals_target_garbage() {
        let (_dir, path) = temp_db_path();
        let target_mob = MobId::from("sqlite-wiring-target");
        let malformed_mob = MobId::from("sqlite-wiring-malformed");
        let identity = AgentIdentity::from("member-a");
        let stores = SqliteMobStores::open(&path).unwrap();
        let encoded = encode_stored_mob_event(&MobEvent {
            cursor: 1,
            timestamp: Utc::now(),
            mob_id: malformed_mob.clone(),
            kind: MobEventKind::MobCompleted,
        })
        .unwrap();
        let mut malformed_value = serde_json::from_slice::<serde_json::Value>(&encoded).unwrap();
        malformed_value["event"]["kind"]["type"] =
            serde_json::Value::String("future_structural_event".to_string());
        let malformed_bytes = serde_json::to_vec(&malformed_value).unwrap();
        let malformed_digest = identity_raw_evidence_digest(&malformed_bytes);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute(
                "INSERT INTO mob_events (cursor, mob_id, event_json) VALUES (?1, NULL, ?2)",
                params![1i64, &malformed_bytes],
            )
            .unwrap();
        }
        drop(stores);

        let stores = SqliteMobStores::open(&path).unwrap();
        let physical_route: String = Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT mob_id FROM mob_events WHERE cursor = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(physical_route, malformed_mob.as_str());
        assert!(matches!(
            stores
                .identity_store()
                .observe_identity_wiring_target(&target_mob, &identity)
                .await
                .unwrap(),
            IdentityWiringTargetObservation::Absent { .. }
        ));
        match stores
            .identity_store()
            .observe_identity_wiring_target(&malformed_mob, &identity)
            .await
            .unwrap()
        {
            IdentityWiringTargetObservation::Malformed {
                observed_version, ..
            } => assert_eq!(observed_version.as_deref(), Some(malformed_digest.as_str())),
            other => panic!("expected target-local malformed event evidence, got {other:?}"),
        }

        let reset = stores
            .event_store()
            .append(NewMobEvent {
                mob_id: malformed_mob.clone(),
                timestamp: None,
                kind: MobEventKind::MobReset,
            })
            .await
            .unwrap();
        assert_eq!(reset.cursor, 2);
        assert!(matches!(
            stores
                .identity_store()
                .observe_identity_wiring_target(&malformed_mob, &identity)
                .await
                .unwrap(),
            IdentityWiringTargetObservation::Absent { .. }
        ));
    }

    #[tokio::test]
    async fn sqlite_event_cursor_repairs_regressed_meta_in_transaction_and_missing_meta_on_open() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).unwrap();
        let events = stores.event_store();
        let first = events
            .append(NewMobEvent {
                mob_id: MobId::from("cursor-repair"),
                timestamp: None,
                kind: MobEventKind::MobCompleted,
            })
            .await
            .unwrap();
        assert_eq!(first.cursor, 1);

        {
            let mut conn = Connection::open(&path).unwrap();
            conn.execute(
                "UPDATE mob_event_meta SET value = 1 WHERE key = ?1",
                params![EVENT_CURSOR_KEY],
            )
            .unwrap();
            let tx = begin_immediate(&mut conn).unwrap();
            assert_eq!(next_event_cursor(&tx).unwrap(), 2);
            tx.commit().unwrap();
            assert_eq!(get_next_cursor(&conn).unwrap(), 2);
        }
        let second = events
            .append(NewMobEvent {
                mob_id: MobId::from("cursor-repair"),
                timestamp: None,
                kind: MobEventKind::MobCompleted,
            })
            .await
            .unwrap();
        assert_eq!(second.cursor, 2);

        {
            let conn = Connection::open(&path).unwrap();
            conn.execute(
                "DELETE FROM mob_event_meta WHERE key = ?1",
                params![EVENT_CURSOR_KEY],
            )
            .unwrap();
        }
        drop(events);
        drop(stores);

        let reopened = SqliteMobStores::open(&path).unwrap();
        let repaired_meta: i64 = Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT value FROM mob_event_meta WHERE key = ?1",
                params![EVENT_CURSOR_KEY],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(repaired_meta, 3);
        let third = reopened
            .event_store()
            .append(NewMobEvent {
                mob_id: MobId::from("cursor-repair"),
                timestamp: None,
                kind: MobEventKind::MobCompleted,
            })
            .await
            .unwrap();
        assert_eq!(third.cursor, 3);
        assert_eq!(
            reopened
                .event_store()
                .replay_all()
                .await
                .unwrap()
                .into_iter()
                .map(|event| event.cursor)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[tokio::test]
    async fn test_sqlite_event_store_append_poll_replay_prune() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().event_store();
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

    #[test]
    fn test_sqlite_event_bus_catch_up_does_not_recreate_deleted_database() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).unwrap();
        assert!(
            path.exists(),
            "opening the store should create the database"
        );

        std::fs::remove_file(&path).unwrap();
        assert!(
            !path.exists(),
            "test setup should remove the database before watcher catch-up"
        );

        stores.event_bus.publish_available_from_storage().unwrap();
        assert!(
            !path.exists(),
            "watcher catch-up must not recreate intentionally deleted mob storage"
        );
    }

    #[test]
    fn test_sqlite_event_bus_catch_up_open_does_not_recreate_database_deleted_after_precheck() {
        let (_dir, path) = temp_db_path();
        let _stores = SqliteMobStores::open(&path).unwrap();
        assert!(
            path.try_exists().unwrap(),
            "watcher preflight should observe the database before the simulated race"
        );

        std::fs::remove_file(&path).unwrap();
        let error = poll_events_sync(&path, 0, EVENT_WATCH_CATCH_UP_LIMIT)
            .expect_err("open-existing catch-up must reject a concurrently deleted database");

        assert!(
            !path.exists(),
            "passive catch-up must not recreate a database deleted after its preflight check: {error}"
        );
    }

    #[tokio::test]
    async fn test_sqlite_terminal_reads_do_not_recreate_deleted_database() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).unwrap();
        let event_store = stores.event_store();
        let run_store = stores.run_store();
        let runtime_store = stores.runtime_metadata_store();
        std::fs::remove_file(&path).unwrap();

        assert!(event_store.poll(0, 1).await.is_err());
        assert!(event_store.replay_all().await.is_err());
        assert!(event_store.latest_cursor().await.is_err());
        let run_id = RunId::new();
        assert!(run_store.get_run(&run_id).await.is_err());
        assert!(
            run_store
                .list_runs(&MobId::from("deleted-mob"), None)
                .await
                .is_err()
        );
        assert!(run_store.list_remote_turn_intents(&run_id).await.is_err());
        assert!(run_store.list_remote_turn_receipts(&run_id).await.is_err());
        assert!(
            runtime_store
                .load_placed_spawn(&MobId::from("deleted-mob"), "deleted-agent")
                .await
                .is_err()
        );
        assert!(
            runtime_store
                .list_placed_spawns(&MobId::from("deleted-mob"))
                .await
                .is_err()
        );
        assert!(
            !path.exists(),
            "terminal storage reads must not recreate removed mob storage"
        );
    }

    #[tokio::test]
    async fn test_sqlite_event_store_subscribe_receives_appended_events() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().event_store();
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
    async fn test_sqlite_event_store_subscribe_receives_appends_from_separate_open() {
        let (_dir, path) = temp_db_path();
        let subscriber_store = SqliteMobStores::open(&path).unwrap().event_store();
        let writer_store = SqliteMobStores::open(&path).unwrap().event_store();
        let mut rx = subscriber_store.subscribe().expect("subscribe");

        let stored = writer_store
            .append(NewMobEvent {
                mob_id: MobId::from("mob"),
                timestamp: None,
                kind: MobEventKind::MobCompleted,
            })
            .await
            .unwrap();

        let observed = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("subscription should receive event from separately opened writer")
            .expect("subscription should stay open");

        assert_eq!(observed.cursor, stored.cursor);
        assert!(matches!(observed.kind, MobEventKind::MobCompleted));
    }

    #[tokio::test]
    async fn test_sqlite_event_store_subscribe_receives_raw_sqlite_append() {
        let (_dir, path) = temp_db_path();
        let subscriber_store = SqliteMobStores::open(&path).unwrap().event_store();
        let mut rx = subscriber_store.subscribe().expect("subscribe");
        let stored = MobEvent {
            cursor: 1,
            timestamp: Utc::now(),
            mob_id: MobId::from("mob"),
            kind: MobEventKind::MobCompleted,
        };
        let encoded = encode_stored_mob_event(&stored).unwrap();
        let writer_path = path.clone();

        run_sqlite_task(move || {
            let mut conn = open_connection(&writer_path)?;
            let tx = begin_immediate(&mut conn)?;
            tx.execute(
                "INSERT INTO mob_events (cursor, event_json) VALUES (?1, ?2)",
                params![cursor_to_i64(stored.cursor)?, encoded],
            )
            .map_err(se)?;
            set_next_cursor(&tx, stored.cursor.saturating_add(1))?;
            tx.commit().map_err(se)?;
            Ok(())
        })
        .await
        .unwrap();

        let observed = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("subscription should receive raw sqlite append")
            .expect("subscription should stay open");

        assert_eq!(observed.cursor, 1);
        assert!(matches!(observed.kind, MobEventKind::MobCompleted));
    }

    #[tokio::test]
    async fn test_sqlite_event_store_rejects_pre_0_6_unversioned_history() {
        let (_dir, path) = temp_db_path();
        let raw_event = serde_json::to_vec(&MobEvent {
            cursor: 1,
            timestamp: Utc::now(),
            mob_id: MobId::from("mob"),
            kind: MobEventKind::MobCompleted,
        })
        .unwrap();

        {
            let conn = open_connection(&path).unwrap();
            conn.execute(
                "INSERT INTO mob_events (cursor, event_json) VALUES (?1, ?2)",
                params![1i64, raw_event],
            )
            .unwrap();
        }

        let store = SqliteMobStores::open(&path).unwrap().event_store();
        let error = store
            .replay_all()
            .await
            .expect_err("pre-0.6 unversioned history must be rejected");
        match error {
            MobStoreError::Serialization(message) => {
                assert!(message.contains("pre-0.6 mob event history is unsupported"));
            }
            other => panic!("expected serialization error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sqlite_run_store_cas_and_dedup() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().run_store();
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
        let (failed_flow_state, failed_authority_input) = run
            .flow_run_command_projection_for_test(
                crate::run::MobMachineFlowRunCommand::TerminalizeFailed(
                    crate::run::flow_run::inputs::TerminalizeFailed {},
                ),
            )
            .expect("project failed run state");

        store.create_run(run).await.unwrap();

        let s1 = store.clone();
        let rid1 = run_id.clone();
        let state1 = expected_flow_state.clone();
        let completed_state = completed_flow_state.clone();
        let completed_input = completed_authority_input.clone();
        let s2 = store.clone();
        let rid2 = run_id.clone();
        let state2 = expected_flow_state.clone();
        let failed_state = failed_flow_state.clone();
        let failed_input = failed_authority_input.clone();
        let outcomes = join_all(vec![
            tokio::spawn(async move {
                s1.cas_run_snapshot_with_authority(
                    &rid1,
                    MobRunStatus::Running,
                    &state1,
                    MobRunStatus::Completed,
                    &completed_state,
                    vec![completed_input],
                )
                .await
                .unwrap()
            }),
            tokio::spawn(async move {
                s2.cas_run_snapshot_with_authority(
                    &rid2,
                    MobRunStatus::Running,
                    &state2,
                    MobRunStatus::Failed,
                    &failed_state,
                    vec![failed_input],
                )
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

        let ledger_run = sample_run(MobRunStatus::Running);
        let ledger_run_id = ledger_run.run_id.clone();
        let step_id = StepId::from("step-1");
        let ledger_expected_flow_state = ledger_run.flow_state.clone();
        let (dispatched_flow_state, dispatched_authority_input) = ledger_run
            .flow_run_command_projection_for_test(
                crate::run::MobMachineFlowRunCommand::DispatchStep(
                    crate::run::flow_run::inputs::DispatchStep {
                        step_id: step_id.clone(),
                    },
                ),
            )
            .expect("project dispatched step state");
        store.create_run(ledger_run).await.unwrap();
        assert!(
            store
                .cas_flow_state_with_authority(
                    &ledger_run_id,
                    &ledger_expected_flow_state,
                    &dispatched_flow_state,
                    vec![dispatched_authority_input.clone()],
                )
                .await
                .unwrap()
        );
        let dispatched_authority =
            MobRunProvenanceAuthority::from_flow_authority_input(dispatched_authority_input)
                .expect("dispatch input is provenance authority");

        let entry = StepLedgerEntry {
            step_id,
            agent_identity: AgentIdentity::flow_system_provenance(),
            status: StepRunStatus::Dispatched,
            output: None,
            timestamp: Utc::now(),
        };

        assert!(
            store
                .append_step_entry_if_absent_with_authority(
                    &ledger_run_id,
                    entry.clone(),
                    dispatched_authority.clone(),
                )
                .await
                .unwrap()
        );
        assert!(
            !store
                .append_step_entry_if_absent_with_authority(
                    &ledger_run_id,
                    entry,
                    dispatched_authority.clone(),
                )
                .await
                .unwrap()
        );
        store
            .append_step_entry_with_authority(
                &ledger_run_id,
                StepLedgerEntry {
                    step_id: StepId::from("step-1"),
                    agent_identity: AgentIdentity::flow_system_provenance(),
                    status: StepRunStatus::Dispatched,
                    output: None,
                    timestamp: Utc::now(),
                },
                dispatched_authority,
            )
            .await
            .expect_err("one dispatch authority must not authorize duplicate step ledger rows");
    }

    #[tokio::test]
    async fn test_sqlite_run_store_create_rejects_preset_provenance_ledgers_without_authority() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().run_store();
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
    async fn test_sqlite_run_store_snapshot_rejects_missing_authority_without_mutation() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().run_store();
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
    async fn test_sqlite_terminal_snapshot_and_event_commit_atomically() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).unwrap();
        let store = stores.run_store();
        let events = stores.event_store();

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

        let stored = store
            .cas_run_snapshot_and_append_terminal_event_with_authority(
                &run_id,
                MobRunStatus::Running,
                &expected_flow_state,
                MobRunStatus::Completed,
                &completed_flow_state,
                vec![completed_authority_input.clone()],
                &events,
                crate::event::NewMobEvent {
                    mob_id: MobId::from("mob"),
                    timestamp: None,
                    kind: crate::event::MobEventKind::FlowCompleted {
                        run_id: run_id.clone(),
                        flow_id: FlowId::from("flow-a"),
                        structured_output: None,
                    },
                },
            )
            .await
            .expect("atomic terminal commit");
        assert!(stored.is_some(), "winning CAS must commit the event");

        // Both sides committed: run is terminal AND the terminal event exists.
        let run = store.get_run(&run_id).await.unwrap().unwrap();
        assert_eq!(run.status, MobRunStatus::Completed);
        assert!(run.completed_at.is_some());
        let replayed = events.replay_all().await.unwrap();
        assert!(replayed.iter().any(|event| matches!(
            &event.kind,
            crate::event::MobEventKind::FlowCompleted { run_id: event_run, .. } if event_run == &run_id
        )));

        // A second (stale) attempt loses the CAS and appends NOTHING — the
        // terminal event cannot exist twice or without its snapshot.
        let lost = store
            .cas_run_snapshot_and_append_terminal_event_with_authority(
                &run_id,
                MobRunStatus::Running,
                &expected_flow_state,
                MobRunStatus::Completed,
                &completed_flow_state,
                vec![completed_authority_input],
                &events,
                crate::event::NewMobEvent {
                    mob_id: MobId::from("mob"),
                    timestamp: None,
                    kind: crate::event::MobEventKind::FlowCompleted {
                        run_id: run_id.clone(),
                        flow_id: FlowId::from("flow-a"),
                        structured_output: None,
                    },
                },
            )
            .await
            .expect("lost CAS is not an error");
        assert!(lost.is_none());
        assert_eq!(
            events.replay_all().await.unwrap().len(),
            1,
            "a lost CAS must not append a terminal event"
        );
    }

    #[tokio::test]
    async fn test_sqlite_spec_store_revision_conflict() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().spec_store();
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
        assert!(matches!(
            conflict,
            MobStoreError::SpecRevisionConflict { .. }
        ));

        let loaded = store.get_spec(&MobId::from("mob")).await.unwrap();
        assert!(loaded.is_some());
    }

    #[tokio::test]
    async fn test_sqlite_member_operator_request_ledger_survives_reopen_and_replays_terminal() {
        use meerkat_contracts::wire::supervisor_bridge::{
            MemberOperatorOp, MemberOperatorOutcome, MemberOperatorReply, WireOpaqueJson,
        };

        let (_dir, path) = temp_db_path();
        let mob_id = MobId::from("mob-upcall-ledger");
        let op = MemberOperatorOp::MobRunFlow {
            flow_id: "release".to_string(),
            params: None,
        };
        let pending = MobMemberOperatorRequestRecord::pending(
            MobMemberOperatorRequestKey::new(
                "worker-1",
                u64::MAX - 1,
                u64::MAX,
                "host-a",
                u64::MAX - 2,
                "member-session-a",
                "req-reopen-1",
            ),
            crate::store::member_operator_op_digest(&op).expect("operation digest"),
        );

        {
            let store = SqliteMobStores::open(&path)
                .expect("open initial store")
                .runtime_metadata_store();
            assert_eq!(
                store
                    .begin_member_operator_request(&mob_id, &pending)
                    .await
                    .expect("persist Pending"),
                MobMemberOperatorRequestBegin::Started
            );
        }

        let terminal_reply = MemberOperatorReply {
            request_id: pending.request_id.clone(),
            outcome: MemberOperatorOutcome::Completed {
                result: WireOpaqueJson::from_value(&serde_json::json!({"ok": true})),
            },
        };
        let terminal = pending
            .terminal(terminal_reply.clone())
            .expect("legal terminal transition");
        {
            let reopened = SqliteMobStores::open(&path)
                .expect("reopen after Pending")
                .runtime_metadata_store();
            assert_eq!(
                reopened
                    .load_member_operator_request(&mob_id, &pending.key())
                    .await
                    .expect("load reopened Pending"),
                Some(pending.clone())
            );
            assert!(
                reopened
                    .compare_and_put_member_operator_request(&mob_id, &pending, &terminal)
                    .await
                    .expect("terminal CAS")
            );
        }

        let reopened = SqliteMobStores::open(&path)
            .expect("reopen after Terminal")
            .runtime_metadata_store();
        let loaded = reopened
            .load_member_operator_request(&mob_id, &pending.key())
            .await
            .expect("load reopened Terminal")
            .expect("terminal row survives");
        assert_eq!(loaded, terminal);
        assert_eq!(loaded.terminal_reply(), Some(&terminal_reply));
        assert_eq!(
            reopened
                .list_member_operator_requests(&mob_id)
                .await
                .expect("list full-domain terminal row"),
            vec![terminal.clone()]
        );
        assert_eq!(
            reopened
                .begin_member_operator_request(&mob_id, &pending)
                .await
                .expect("duplicate begin reads terminal"),
            MobMemberOperatorRequestBegin::Existing(loaded)
        );
    }

    #[tokio::test]
    async fn sqlite_member_operator_request_key_separates_host_generation_and_member_session() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path)
            .expect("open execution-fence key store")
            .runtime_metadata_store();
        let mob_id = MobId::from("sqlite-operator-execution-fence-key");
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
                    .expect("insert exact SQLite execution-fence key"),
                MobMemberOperatorRequestBegin::Started,
            );
        }
        assert!(matches!(
            store
                .begin_member_operator_request(&mob_id, &base)
                .await
                .expect("exact SQLite duplicate replays"),
            MobMemberOperatorRequestBegin::Existing(existing) if existing == base
        ));

        let reopened = SqliteMobStores::open(&path)
            .expect("reopen execution-fence key store")
            .runtime_metadata_store();
        for record in [&base, &host_generation_two, &replacement_session] {
            assert_eq!(
                reopened
                    .load_member_operator_request(&mob_id, &record.key())
                    .await
                    .expect("load exact SQLite execution-fence key"),
                Some(record.clone()),
            );
        }
        assert_eq!(
            reopened
                .list_member_operator_requests(&mob_id)
                .await
                .expect("list distinct SQLite execution-fence rows")
                .len(),
            3,
        );
    }

    #[tokio::test]
    async fn sqlite_migrates_pre_execution_fence_ledger_by_dropping_ambiguous_rows() {
        let (_dir, path) = temp_db_path();
        {
            let conn = Connection::open(&path).expect("open legacy member-operator database");
            conn.execute_batch(
                "CREATE TABLE mob_runtime_member_operator_requests (
                     mob_id TEXT NOT NULL,
                     agent_identity TEXT NOT NULL,
                     generation BLOB NOT NULL,
                     fence_token BLOB NOT NULL,
                     request_id TEXT NOT NULL,
                     record_json BLOB NOT NULL,
                     PRIMARY KEY (
                         mob_id, agent_identity, generation, fence_token, request_id
                     )
                 );",
            )
            .expect("create pre-execution-fence ledger");
            conn.execute(
                "INSERT INTO mob_runtime_member_operator_requests
                 (mob_id, agent_identity, generation, fence_token, request_id, record_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    "legacy-mob",
                    "member-a",
                    u64_to_sql_key(7),
                    u64_to_sql_key(11),
                    "ambiguous-request",
                    b"{}".as_slice(),
                ],
            )
            .expect("seed ambiguous pre-fence row");
        }

        let stores = SqliteMobStores::open(&path).expect("open and migrate legacy ledger");
        assert!(
            stores
                .runtime_metadata_store()
                .list_member_operator_requests(&MobId::from("legacy-mob"))
                .await
                .expect("list migrated ledger")
                .is_empty(),
            "a pre-fence row cannot be safely attributed and must not replay"
        );

        let conn = Connection::open(&path).expect("inspect migrated ledger schema");
        let mut statement = conn
            .prepare("PRAGMA table_info(mob_runtime_member_operator_requests)")
            .expect("prepare table-info inspection");
        let columns = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
            })
            .expect("read migrated table info")
            .map(|row| row.expect("read migrated column"))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(columns.get("host_id"), Some(&5));
        assert_eq!(columns.get("binding_generation"), Some(&6));
        assert_eq!(columns.get("member_session_id"), Some(&7));
        assert_eq!(columns.get("request_id"), Some(&8));
    }

    #[tokio::test]
    async fn sqlite_rebuilds_execution_fence_columns_when_they_are_not_in_the_primary_key() {
        let (_dir, path) = temp_db_path();
        {
            let conn = Connection::open(&path).expect("open malformed member-operator database");
            conn.execute_batch(
                "CREATE TABLE mob_runtime_member_operator_requests (
                     mob_id TEXT NOT NULL,
                     agent_identity TEXT NOT NULL,
                     generation BLOB NOT NULL,
                     fence_token BLOB NOT NULL,
                     host_id TEXT NOT NULL,
                     binding_generation BLOB NOT NULL,
                     member_session_id TEXT NOT NULL,
                     request_id TEXT NOT NULL,
                     record_json BLOB NOT NULL,
                     PRIMARY KEY (
                         mob_id, agent_identity, generation, fence_token, request_id
                     )
                 );",
            )
            .expect("create ledger whose fence columns are not key columns");
            conn.execute(
                "INSERT INTO mob_runtime_member_operator_requests
                 (mob_id, agent_identity, generation, fence_token, host_id,
                  binding_generation, member_session_id, request_id, record_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    "malformed-key-mob",
                    "member-a",
                    u64_to_sql_key(7),
                    u64_to_sql_key(11),
                    "host-a",
                    u64_to_sql_key(3),
                    "member-session-a",
                    "ambiguous-request",
                    b"{}".as_slice(),
                ],
            )
            .expect("seed row under malformed execution-fence key");
        }

        let stores = SqliteMobStores::open(&path).expect("open and rebuild malformed ledger");
        assert!(
            stores
                .runtime_metadata_store()
                .list_member_operator_requests(&MobId::from("malformed-key-mob"))
                .await
                .expect("list rebuilt malformed ledger")
                .is_empty(),
            "rows written without the full tuple in the primary key are ambiguous and must drop"
        );

        let conn = Connection::open(&path).expect("inspect rebuilt malformed ledger schema");
        let mut statement = conn
            .prepare("PRAGMA table_info(mob_runtime_member_operator_requests)")
            .expect("prepare rebuilt table-info inspection");
        let primary_key = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(5)?, row.get::<_, String>(1)?))
            })
            .expect("read rebuilt table info")
            .map(|row| row.expect("read rebuilt column"))
            .filter(|(position, _)| *position > 0)
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            primary_key.into_values().collect::<Vec<_>>(),
            vec![
                "mob_id",
                "agent_identity",
                "generation",
                "fence_token",
                "host_id",
                "binding_generation",
                "member_session_id",
                "request_id",
            ],
        );
    }

    #[test]
    fn sqlite_u64_blob_keys_round_trip_and_sort_the_full_domain() {
        let (_dir, path) = temp_db_path();
        let conn = open_connection(&path).expect("open full-domain key store");
        let expected = [1, i64::MAX as u64, i64::MAX as u64 + 1, u64::MAX];
        for value in expected {
            conn.execute(
                "INSERT INTO mob_run_remote_turn_intents \
                 (run_id, dispatch_sequence, intent_json) VALUES (?1, ?2, ?3)",
                params!["full-domain-run", u64_to_sql_key(value), Vec::<u8>::new()],
            )
            .expect("insert full-domain dispatch key");
        }
        let mut statement = conn
            .prepare(
                "SELECT dispatch_sequence FROM mob_run_remote_turn_intents \
                 WHERE run_id = ?1 ORDER BY dispatch_sequence ASC",
            )
            .expect("prepare ordered full-domain key query");
        let rows = statement
            .query_map(params!["full-domain-run"], |row| row.get::<_, Vec<u8>>(0))
            .expect("query full-domain keys");
        let actual = rows
            .map(|row| {
                let key = row.expect("read full-domain key");
                sql_key_to_u64(&key, "test dispatch sequence").expect("decode full-domain key")
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_sqlite_member_operator_incarnation_quota_is_atomic_and_replays_at_ceiling() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).expect("open SQLite quota store");
        let mob_id = MobId::from("operator-incarnation-quota");
        let oversized = MobMemberOperatorRequestRecord::pending(
            MobMemberOperatorRequestKey::new(
                "member-a",
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
            stores
                .runtime_metadata_store()
                .begin_member_operator_request(&mob_id, &oversized)
                .await
                .is_err(),
            "SQLite must reject oversized untrusted idempotency keys before persistence"
        );
        assert_eq!(operator_request_count(&path, &mob_id), 0);
        let seed = (0..crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION - 1)
            .map(|sequence| operator_request("member-a", 1, sequence))
            .collect::<Vec<_>>();
        seed_operator_requests(&path, &mob_id, &seed);

        let candidate_a = operator_request(
            "member-a",
            1,
            crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION - 1,
        );
        let candidate_b = operator_request(
            "member-a",
            1,
            crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION,
        );
        let store_a = stores.runtime_metadata_store();
        let store_b = stores.runtime_metadata_store();
        let (result_a, result_b) = tokio::join!(
            store_a.begin_member_operator_request(&mob_id, &candidate_a),
            store_b.begin_member_operator_request(&mob_id, &candidate_b),
        );

        assert_eq!(
            [&result_a, &result_b]
                .into_iter()
                .filter(|result| matches!(result, Ok(MobMemberOperatorRequestBegin::Started)))
                .count(),
            1,
            "BEGIN IMMEDIATE must serialize the count+insert so only one contender owns execution"
        );
        assert_eq!(
            [&result_a, &result_b]
                .into_iter()
                .filter(|result| matches!(result, Err(MobStoreError::WriteFailed(_))))
                .count(),
            1,
            "the serialized loser must observe the committed ceiling and fail closed"
        );
        assert_eq!(
            operator_request_count(&path, &mob_id),
            crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION,
            "concurrent begins must not oversubscribe the incarnation ceiling"
        );

        let winner = if matches!(result_a, Ok(MobMemberOperatorRequestBegin::Started)) {
            &candidate_a
        } else {
            &candidate_b
        };
        assert_eq!(
            stores
                .runtime_metadata_store()
                .begin_member_operator_request(&mob_id, winner)
                .await
                .expect("the winning key must replay at the ceiling"),
            MobMemberOperatorRequestBegin::Existing(winner.clone()),
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_sqlite_member_operator_global_quota_is_atomic_and_replays_at_ceiling() {
        let (_dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).expect("open SQLite quota store");
        let mob_id = MobId::from("operator-global-quota");
        let seed = (0..crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_MOB - 1)
            .map(|sequence| {
                let incarnation =
                    sequence / crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION;
                let request = sequence % crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION;
                operator_request(&format!("member-{incarnation}"), 1, request)
            })
            .collect::<Vec<_>>();
        seed_operator_requests(&path, &mob_id, &seed);

        let candidate_a = operator_request("member-overflow-a", 1, 0);
        let candidate_b = operator_request("member-overflow-b", 1, 0);
        let store_a = stores.runtime_metadata_store();
        let store_b = stores.runtime_metadata_store();
        let (result_a, result_b) = tokio::join!(
            store_a.begin_member_operator_request(&mob_id, &candidate_a),
            store_b.begin_member_operator_request(&mob_id, &candidate_b),
        );

        assert_eq!(
            [&result_a, &result_b]
                .into_iter()
                .filter(|result| matches!(result, Ok(MobMemberOperatorRequestBegin::Started)))
                .count(),
            1,
            "only one concurrent contender may claim the final per-mob slot"
        );
        assert_eq!(
            [&result_a, &result_b]
                .into_iter()
                .filter(|result| matches!(result, Err(MobStoreError::WriteFailed(_))))
                .count(),
            1,
            "the other contender must observe the committed global ceiling"
        );
        assert_eq!(
            operator_request_count(&path, &mob_id),
            crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_MOB,
            "concurrent begins must not oversubscribe the per-mob ceiling"
        );

        let winner = if matches!(result_a, Ok(MobMemberOperatorRequestBegin::Started)) {
            &candidate_a
        } else {
            &candidate_b
        };
        assert_eq!(
            stores
                .runtime_metadata_store()
                .begin_member_operator_request(&mob_id, winner)
                .await
                .expect("the globally winning key must replay at the ceiling"),
            MobMemberOperatorRequestBegin::Existing(winner.clone()),
        );
    }

    #[tokio::test]
    async fn sqlite_recovery_prune_frees_stale_global_capacity_and_preserves_current_rows() {
        use meerkat_contracts::wire::supervisor_bridge::{
            MemberOperatorOutcome, MemberOperatorReply, WireOpaqueJson,
        };

        let (_dir, path) = temp_db_path();
        let mob_id = MobId::from("sqlite-operator-recovery-prune");
        let current = (0..crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION)
            .map(|sequence| operator_request("member-current", 1, sequence));
        let stale = (0..3).flat_map(|incarnation| {
            (0..crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION).map(move |sequence| {
                operator_request(&format!("member-stale-{incarnation}"), 1, sequence)
            })
        });
        let rows = current.chain(stale).collect::<Vec<_>>();
        seed_operator_requests(&path, &mob_id, &rows);

        let current_pending = operator_request("member-current", 1, 0);
        let current_terminal = current_pending
            .terminal(MemberOperatorReply {
                request_id: current_pending.request_id.clone(),
                outcome: MemberOperatorOutcome::Completed {
                    result: WireOpaqueJson::from_value(&serde_json::json!({"ok": true})),
                },
            })
            .expect("terminalize current SQLite replay row");
        {
            let store = SqliteMobStores::open(&path)
                .expect("open before simulated crash")
                .runtime_metadata_store();
            assert!(
                store
                    .compare_and_put_member_operator_request(
                        &mob_id,
                        &current_pending,
                        &current_terminal,
                    )
                    .await
                    .expect("persist current SQLite terminal")
            );
            assert!(
                store
                    .begin_member_operator_request(&mob_id, &operator_request("member-new", 1, 0),)
                    .await
                    .is_err(),
                "the recovered store starts at the global fail-closed ceiling"
            );
        }

        let recovered = SqliteMobStores::open(&path)
            .expect("reopen after simulated crash")
            .runtime_metadata_store();
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
            recovered
                .prune_stale_member_operator_requests(&mob_id, &authority)
                .await
                .expect("recovered actor-authorized SQLite prune"),
            3 * crate::store::MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION as u64,
        );
        assert_eq!(
            recovered
                .begin_member_operator_request(&mob_id, &current_pending)
                .await
                .expect("current SQLite terminal still replays"),
            MobMemberOperatorRequestBegin::Existing(current_terminal),
        );
        assert!(
            recovered
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
            "recovery pruning must preserve the current incarnation ceiling"
        );
        assert_eq!(
            recovered
                .begin_member_operator_request(&mob_id, &operator_request("member-new", 1, 0))
                .await
                .expect("recovery pruning frees global SQLite capacity"),
            MobMemberOperatorRequestBegin::Started,
        );
    }

    #[tokio::test]
    async fn test_sqlite_runtime_metadata_store_roundtrips_supervisor_and_overlay_records() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path)
            .unwrap()
            .runtime_metadata_store();
        let mob_id = MobId::from("mob");
        let supervisor = SupervisorAuthorityRecord::generate(default_bridge_protocol_version());
        let supervisor_authority =
            crate::store::supervisor_authority_persistence_authority_for_record(&supervisor)
                .unwrap();
        let overlay = ExternalBindingOverlayRecord {
            agent_identity: AgentIdentity::from("worker-1"),
            generation: Generation::new(2),
            normalized_member_ref: Some(MemberRef::BackendPeer {
                peer_id: "peer-worker-1".to_string(),
                address: "tcp://worker-1".to_string(),
                bootstrap_token: None,
                session_id: None,
                pubkey: [7u8; 32],
            }),
            bootstrap_token: None,
            status: ExternalBindingOverlayStatus::Normalized,
            updated_at: Utc::now(),
        };

        store
            .put_supervisor_authority(&mob_id, &supervisor, &supervisor_authority)
            .await
            .unwrap();
        let loaded_supervisor = store
            .load_supervisor_authority(&mob_id)
            .await
            .unwrap()
            .expect("supervisor should persist");
        assert_eq!(loaded_supervisor, supervisor);

        assert!(
            store
                .put_external_binding_overlay_if_absent(&mob_id, &overlay)
                .await
                .unwrap(),
            "first overlay insert should win"
        );
        assert!(
            !store
                .put_external_binding_overlay_if_absent(&mob_id, &overlay)
                .await
                .unwrap(),
            "duplicate overlay insert should be ignored"
        );

        let overlays = store.list_external_binding_overlays(&mob_id).await.unwrap();
        assert_eq!(overlays, vec![overlay.clone()]);

        let failed_overlay = ExternalBindingOverlayRecord {
            status: ExternalBindingOverlayStatus::Failed {
                reason: "normalization failed".to_string(),
            },
            normalized_member_ref: None,
            updated_at: Utc::now(),
            ..overlay
        };
        store
            .upsert_external_binding_overlay(&mob_id, &failed_overlay)
            .await
            .unwrap();
        let overlays = store.list_external_binding_overlays(&mob_id).await.unwrap();
        assert_eq!(overlays, vec![failed_overlay]);

        store
            .delete_external_binding_overlays(&mob_id)
            .await
            .unwrap();
        assert!(
            store
                .list_external_binding_overlays(&mob_id)
                .await
                .unwrap()
                .is_empty(),
            "overlay delete should clear all records for the mob"
        );

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
                .load_supervisor_authority(&mob_id)
                .await
                .unwrap()
                .is_none(),
            "supervisor delete should remove the stored record"
        );
    }

    #[tokio::test]
    async fn test_sqlite_runtime_metadata_store_put_supervisor_if_absent_preserves_existing_record()
    {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path)
            .unwrap()
            .runtime_metadata_store();
        let mob_id = MobId::from("mob");
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
    async fn test_sqlite_runtime_metadata_store_compare_and_put_supervisor_authority() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path)
            .unwrap()
            .runtime_metadata_store();
        let mob_id = MobId::from("mob-cas");
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
    async fn test_sqlite_runtime_metadata_store_roundtrips_mob_host_authority_records() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path)
            .unwrap()
            .runtime_metadata_store();
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

        // Rebind CAS to the next epoch under a rebind-witnessed authority.
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

        // Durable across reopen (the controlling-restart fact FLAG-3 exists
        // for).
        drop(store);
        let reopened = SqliteMobStores::open(&path)
            .unwrap()
            .runtime_metadata_store();
        assert_eq!(
            reopened.list_mob_host_authorities(&mob_id).await.unwrap(),
            vec![host_b_rebound.clone(), host_c],
            "host bindings must survive a store reopen"
        );

        // Revoke: delete requires the exact expected record + revoke
        // witness; the other mob's row survives (A14 isolation).
        let deletion =
            crate::store::mob_host_authority_deletion_authority_for_record(&host_b_rebound)
                .unwrap();
        reopened
            .put_mob_host_binding_generation_highwater(&mob_id, &host_b_rebound, &deletion)
            .await
            .unwrap();
        assert!(
            !reopened
                .delete_mob_host_authority(&mob_id, &host_b, &deletion)
                .await
                .unwrap(),
            "stale expected record must not delete"
        );
        assert!(
            reopened
                .delete_mob_host_authority(&mob_id, &host_b_rebound, &deletion)
                .await
                .unwrap()
        );
        assert!(
            reopened
                .load_mob_host_authority(&mob_id, "host-peer-b")
                .await
                .unwrap()
                .is_none()
        );
        drop(reopened);
        let reopened = SqliteMobStores::open(&path)
            .unwrap()
            .runtime_metadata_store();
        assert_eq!(
            reopened
                .list_mob_host_binding_generation_highwaters(&mob_id)
                .await
                .unwrap(),
            vec![("host-peer-b".to_string(), host_b_rebound.binding_generation)],
            "the generation tombstone survives both active-row deletion and reopen",
        );
        assert_eq!(
            reopened
                .list_mob_host_authorities(&other_mob)
                .await
                .unwrap(),
            vec![host_b],
            "another mob's binding for the same host must survive"
        );
    }

    #[tokio::test]
    async fn test_sqlite_stores_share_single_database_path() {
        let (_dir, path) = temp_db_path();
        let shared = SqliteMobStores::open(&path).unwrap();
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
        assert!(fetched_run.is_some(), "run store should share same db path");

        let definition = sample_definition();
        let revision = specs
            .put_spec(&MobId::from("mob"), &definition, None)
            .await
            .unwrap();
        assert_eq!(revision, 1);
        assert_eq!(events.replay_all().await.unwrap().len(), 1);
    }

    #[tokio::test]
    #[ignore] // integration_real: large keyset stress test
    async fn integration_real_sqlite_event_store_clear_and_prune_large_keyset() {
        let (_dir, path) = temp_db_path();
        let store = SqliteMobStores::open(&path).unwrap().event_store();
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

    /// Regression test: durable persistence must allow
    /// reopening the same database path after drop within the same process.
    /// SQLite WAL mode does not have this limitation.
    #[tokio::test]
    async fn test_sqlite_reopen_after_drop_same_path() {
        let (_dir, path) = temp_db_path();

        // First open: write data
        {
            let stores = SqliteMobStores::open(&path).unwrap();
            let events = stores.event_store();
            events
                .append(NewMobEvent {
                    mob_id: MobId::from("mob"),
                    timestamp: None,
                    kind: MobEventKind::MobCompleted,
                })
                .await
                .unwrap();
            // stores + events dropped here
        }

        // Second open: must succeed and see prior data
        {
            let stores = SqliteMobStores::open(&path).unwrap();
            let events = stores.event_store();
            let all = events.replay_all().await.unwrap();
            assert_eq!(all.len(), 1, "data from first open must survive reopen");
        }
    }

    // -----------------------------------------------------------------------
    // RealmProfileStore contract tests (SQLite)
    // -----------------------------------------------------------------------

    use crate::store::realm_profile::contract_tests;

    fn sqlite_realm_profile_store() -> (tempfile::TempDir, SqliteRealmProfileStore) {
        let (dir, path) = temp_db_path();
        let stores = SqliteMobStores::open(&path).unwrap();
        (dir, stores.realm_profile_store())
    }

    #[tokio::test]
    async fn realm_profile_create_and_get() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_create_and_get(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_get_nonexistent() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_get_nonexistent(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_create_duplicate_fails() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_create_duplicate_fails(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_update_correct_revision() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_update_with_correct_revision(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_update_wrong_revision() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_update_with_wrong_revision(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_update_nonexistent() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_update_nonexistent(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_delete_correct_revision() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_delete_with_correct_revision(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_delete_wrong_revision() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_delete_with_wrong_revision(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_delete_nonexistent() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_delete_nonexistent(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_list() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_list(&store).await;
    }

    #[tokio::test]
    async fn realm_profile_list_empty() {
        let (_dir, store) = sqlite_realm_profile_store();
        contract_tests::test_list_empty(&store).await;
    }

    // -----------------------------------------------------------------------
    // SqliteRealmProfileStore::open() standalone constructor tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn sqlite_realm_profile_store_open_creates_directory() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("realm_profiles.db");
        assert!(!nested.parent().unwrap().exists());
        let _store = SqliteRealmProfileStore::open(&nested).unwrap();
        assert!(nested.parent().unwrap().exists());
    }

    #[tokio::test]
    async fn sqlite_realm_profile_store_open_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("realm_profiles.db");

        // First open: create a profile.
        {
            let store = SqliteRealmProfileStore::open(&db_path).unwrap();
            let profile = Profile {
                model: "claude-sonnet-4-5".into(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
                skills: Vec::new(),
                tools: ToolConfig::default(),
                peer_description: String::new(),
                external_addressable: false,
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            };
            store.create("test-profile", &profile).await.unwrap();
        }

        // Second open: must see the profile from the first session.
        {
            let store = SqliteRealmProfileStore::open(&db_path).unwrap();
            let got = store.get("test-profile").await.unwrap();
            assert!(got.is_some(), "profile must survive reopen");
            assert_eq!(got.unwrap().profile.model, "claude-sonnet-4-5");
        }
    }

    #[tokio::test]
    async fn sqlite_realm_profile_store_open_contract_create_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("realm_profiles.db");
        let store = SqliteRealmProfileStore::open(&db_path).unwrap();
        contract_tests::test_create_and_get(&store).await;
    }

    #[tokio::test]
    async fn sqlite_realm_profile_store_open_contract_get_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("realm_profiles.db");
        let store = SqliteRealmProfileStore::open(&db_path).unwrap();
        contract_tests::test_get_nonexistent(&store).await;
    }

    #[tokio::test]
    async fn sqlite_realm_profile_store_open_contract_list() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("realm_profiles.db");
        let store = SqliteRealmProfileStore::open(&db_path).unwrap();
        contract_tests::test_list(&store).await;
    }
}
