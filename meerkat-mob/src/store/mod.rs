//! Mob store traits and implementations.

#[cfg(all(test, not(target_arch = "wasm32")))]
mod identity_contract_tests;
mod in_memory;
mod realm_profile;
#[cfg(not(target_arch = "wasm32"))]
mod sqlite;

pub use in_memory::{
    InMemoryMobEventStore, InMemoryMobIdentityStatusStore, InMemoryMobIdentityStore,
    InMemoryMobRunStore, InMemoryMobRuntimeMetadataStore, InMemoryMobSpecStore,
    InMemoryRealmProfileStore,
};
pub use realm_profile::{RealmProfileStore, StoredRealmProfile};
#[cfg(not(target_arch = "wasm32"))]
pub use sqlite::{
    SqliteMobEventStore, SqliteMobIdentityStatusStore, SqliteMobIdentityStore, SqliteMobRunStore,
    SqliteMobRuntimeMetadataStore, SqliteMobSpecStore, SqliteMobStores, SqliteRealmProfileStore,
};

use crate::definition::MobDefinition;
use crate::event::{MemberRef, MobEvent, MobEventKind, NewMobEvent};
#[cfg(feature = "runtime-adapter")]
use crate::identity::DesiredSessionTarget;
use crate::identity::{
    IdentityActuationPermit, IdentityConvergenceStatus, IdentityDeclarationApplyPlan,
    IdentityDeclarationManifestApplyOutcome, IdentityDeclarationScopeHead, IdentityIntent,
    IdentityIntentRecord, IdentityLeaseClaim, IdentityLeaseClaimOutcome, IdentityLeaseRecord,
    IdentityOperationReceipt, IdentityOperationReceiptInsertOutcome, IdentityOperationSlot,
    IdentityOperationSubject, IdentityResourceObservation, IdentityStoredObservation,
    IdentityTargetObservationVersion,
};
use crate::ids::{
    AgentIdentity, FlowId, FrameId, Generation, LoopId, LoopInstanceId, MobId, PlacedSpawnId,
    RunId, StepId,
};
use crate::machines::mob_machine as mob_dsl;
use crate::run::flow_run;
use crate::run::{
    FailureLedgerEntry, FrameSnapshot, LoopIterationLedgerEntry, LoopSnapshot, MobRun,
    MobRunProvenanceAuthority, MobRunRemoteTurnIntent, MobRunRemoteTurnReceipt, MobRunStatus,
    StepLedgerEntry,
};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use meerkat_contracts::wire::WireControlScope;
use meerkat_contracts::wire::supervisor_bridge::{
    BridgeBootstrapToken, BridgeMemberIncarnation, BridgePeerSpec, BridgeProtocolVersion,
    MemberOperatorOp, MemberOperatorReply, SupervisorRotationOperationId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Receiver for append-driven structural mob events.
pub type MobEventReceiver = broadcast::Receiver<MobEvent>;

pub(crate) mod private {
    pub trait MobEventStoreSealed {}
}

pub(crate) fn terminal_event_identity(kind: &MobEventKind) -> Option<(&RunId, &FlowId)> {
    match kind {
        MobEventKind::FlowCompleted {
            run_id, flow_id, ..
        }
        | MobEventKind::FlowFailed {
            run_id, flow_id, ..
        }
        | MobEventKind::FlowCanceled { run_id, flow_id } => Some((run_id, flow_id)),
        _ => None,
    }
}

/// Identity member/wiring projection events are current structural anchors,
/// not disposable history. Until the store has compacted current-head
/// representations, timestamp pruning must retain the full reducer inputs so
/// live targets cannot become spuriously absent after restart.
pub(crate) fn identity_structural_projection_is_anchor(kind: &MobEventKind) -> bool {
    matches!(
        kind,
        MobEventKind::MemberSpawned(_)
            | MobEventKind::MemberRetirementStarted { .. }
            | MobEventKind::RemoteMemberRuntimeRetired { .. }
            | MobEventKind::RemoteMemberSupervisorRevoked { .. }
            | MobEventKind::MemberRetired { .. }
            | MobEventKind::MemberReset { .. }
            | MobEventKind::MemberSessionBindingRecovered(_)
            | MobEventKind::MembersWired { .. }
            | MobEventKind::MembersWiredBatch { .. }
            | MobEventKind::MembersUnwired { .. }
            | MobEventKind::MobReset
    )
}

pub(crate) fn step_failed_event_identity(kind: &MobEventKind) -> Option<(&RunId, &StepId, &str)> {
    match kind {
        MobEventKind::StepFailed {
            run_id,
            step_id,
            reason,
        } => Some((run_id, step_id, reason)),
        _ => None,
    }
}

/// Frame-aware atomic persistence operation required by the flow/frame store contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameAtomicOperation {
    CasFrameState,
    CasGrantNodeSlot,
    CasCompleteStepAndRecordOutput,
    CasStartLoop,
    CasLoopRequestBodyFrame,
    CasGrantBodyFrameStart,
    CasCompleteBodyFrame,
    CasCompleteLoop,
    PutRemoteTurnIntent,
    DeleteRemoteTurnIntent,
    PutRemoteTurnReceipt,
    DeleteRemoteTurnReceipt,
}

impl std::fmt::Display for FrameAtomicOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CasFrameState => write!(f, "cas_frame_state"),
            Self::CasGrantNodeSlot => write!(f, "cas_grant_node_slot"),
            Self::CasCompleteStepAndRecordOutput => {
                write!(f, "cas_complete_step_and_record_output")
            }
            Self::CasStartLoop => write!(f, "cas_start_loop"),
            Self::CasLoopRequestBodyFrame => write!(f, "cas_loop_request_body_frame"),
            Self::CasGrantBodyFrameStart => write!(f, "cas_grant_body_frame_start"),
            Self::CasCompleteBodyFrame => write!(f, "cas_complete_body_frame"),
            Self::CasCompleteLoop => write!(f, "cas_complete_loop"),
            Self::PutRemoteTurnIntent => write!(f, "put_remote_turn_intent"),
            Self::DeleteRemoteTurnIntent => write!(f, "delete_remote_turn_intent"),
            Self::PutRemoteTurnReceipt => write!(f, "put_remote_turn_receipt"),
            Self::DeleteRemoteTurnReceipt => write!(f, "delete_remote_turn_receipt"),
        }
    }
}

/// Errors from mob storage operations.
///
/// Scoped to storage concerns only — callers convert to [`MobError`](crate::MobError)
/// at the boundary via the `From` impl.
#[derive(Debug, thiserror::Error)]
pub enum MobStoreError {
    /// A write operation failed.
    #[error("Write failed: {0}")]
    WriteFailed(String),

    /// A read operation failed.
    #[error("Read failed: {0}")]
    ReadFailed(String),

    /// The requested entity was not found.
    #[error("Not found: {0}")]
    NotFound(String),

    /// A compare-and-swap precondition was not met.
    #[error("CAS conflict: {0}")]
    CasConflict(String),

    /// Durable identity authority is present but cannot safely authorize a
    /// mutation. Controllers must project RepairBlocked instead of retrying
    /// this as a transient transport failure.
    #[error("identity authority is repair-blocked: {detail}")]
    IdentityAuthorityBlocked {
        evidence_digest: Option<String>,
        detail: String,
    },

    /// A monotonic identity counter reached the end of the u64 domain.
    #[error("identity {counter} counter exhausted")]
    IdentityCounterExhausted { counter: String },

    /// Spec revision compare-and-swap failed (structured variant for typed conversion).
    #[error("spec revision conflict for mob {mob_id}: expected {expected:?}, actual {actual}")]
    SpecRevisionConflict {
        mob_id: crate::ids::MobId,
        expected: Option<u64>,
        actual: u64,
    },

    /// The backend cannot provide the requested frame-aware atomic operation.
    #[error("frame-aware atomic persistence unavailable for operation '{operation}'")]
    FrameAtomicPersistenceUnavailable { operation: FrameAtomicOperation },

    /// This storage composition cannot atomically fence identity authority and
    /// append the structural member or wiring event in one backend transaction.
    #[error("identity structural-event atomic persistence is unavailable")]
    IdentityMemberAtomicPersistenceUnavailable,

    /// This identity store cannot retain its authority serialization guard
    /// across a runtime target mutation.
    #[error("identity runtime write fencing is unavailable")]
    IdentityRuntimeWriteFenceUnavailable,

    /// Serialization or deserialization failed.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Result of the single target-local CAS that finalizes a level-triggered
/// identity member materialization.
///
/// The structural event is the member target. `Conflict` means the caller
/// must re-observe and reclassify; `RepairBlocked` is durable unsafe evidence;
/// `Backoff` is a transient store/clock failure. Status is deliberately absent
/// because it is an output projection and never participates in this write.
#[derive(Debug, Clone)]
pub(crate) enum IdentityMemberEventCommitOutcome {
    Applied {
        event: MobEvent,
    },
    AlreadyExact {
        event: MobEvent,
    },
    Conflict {
        current: Option<IdentityMemberTargetObservation>,
        detail: String,
    },
    RepairBlocked {
        evidence_digest: Option<String>,
        detail: String,
    },
    Backoff {
        detail: String,
    },
}

/// Result of one target-local identity wiring projection CAS.
///
/// This operation seals only the durable structural event projection. It does
/// not prove that comms trust or generated-machine wiring side effects are
/// physically realized. The actor must observe and actuate those resources at
/// their owning seam, then use this CAS as the durable structural projection.
/// Authority is revalidated in the same transaction that appends exactly one
/// existing `MembersWired` or `MembersUnwired` event. Status is deliberately
/// absent: it is an output projection only.
#[derive(Debug, Clone)]
pub(crate) enum IdentityWiringEventCommitOutcome {
    Applied {
        event: MobEvent,
    },
    AlreadyExact {
        current: IdentityWiringTargetObservation,
    },
    Conflict {
        current: Option<IdentityWiringTargetObservation>,
        detail: String,
    },
    RepairBlocked {
        evidence_digest: Option<String>,
        detail: String,
    },
    Backoff {
        detail: String,
    },
}

/// Raw structural observation of one identity's member-event target.
///
/// `Present` intentionally does not mean that the realization matches desired
/// material. The actor compares the fresh realization to `IdentityIntent` for
/// classification, while this value supplies only the resource-local CAS
/// version used by the final event append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IdentityMemberTargetObservation {
    Absent {
        absence_version: String,
    },
    Present {
        version: String,
        evidence: IdentityMemberTargetEvidence,
    },
    /// A current realization is held by an already-admitted recovery/cleanup
    /// sequence. No member write permit may be minted until that sequence
    /// reaches a fresh observable target.
    Recovering {
        version: String,
        detail: String,
    },
    Malformed {
        observed_version: Option<String>,
        detail: String,
    },
}

/// Desired-state-comparable evidence carried by the current structural
/// `MemberSpawned` realization. The authority digest seals the complete
/// current intent while the explicit fields make its session/material binding
/// auditable. Missing digest evidence is a legacy divergent realization, not
/// an implicit match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IdentityMemberTargetEvidence {
    pub identity: AgentIdentity,
    pub intent_authority_digest: Option<String>,
    pub session_id: meerkat_core::SessionId,
    pub role: crate::ids::ProfileName,
    pub runtime_mode: crate::runtime_mode::MobRuntimeMode,
    pub labels: BTreeMap<String, String>,
}

impl IdentityMemberTargetObservation {
    pub fn target_precondition(&self) -> Option<IdentityTargetObservationVersion> {
        match self {
            Self::Absent { absence_version } => Some(IdentityTargetObservationVersion::Absent {
                absence_version: absence_version.clone(),
            }),
            Self::Present { version, .. } => Some(IdentityTargetObservationVersion::Version {
                version: version.clone(),
            }),
            Self::Recovering { .. } | Self::Malformed { .. } => None,
        }
    }

    /// Compare fresh structural member evidence with the sole desired intent
    /// while retaining this target's exact CAS version. Callers must classify
    /// malformed intent authority independently before using this projection.
    #[must_use]
    pub fn resource_observation_against(
        &self,
        intent: &IdentityIntentRecord,
    ) -> IdentityResourceObservation {
        match self {
            Self::Absent { absence_version } => IdentityResourceObservation::Missing {
                absence_version: absence_version.clone(),
            },
            Self::Malformed {
                observed_version,
                detail,
            } => IdentityResourceObservation::Malformed {
                observed_version: observed_version.clone(),
                detail: detail.clone(),
            },
            Self::Recovering { detail, .. } => IdentityResourceObservation::Unavailable {
                detail: detail.clone(),
            },
            Self::Present { version, evidence } => {
                let matches = match &intent.intent {
                    IdentityIntent::Present {
                        identity,
                        session,
                        member,
                        ..
                    } => {
                        let expected_runtime_mode = desired_member_runtime_mode(member);
                        let expected_labels =
                            member.material.overlay.labels.clone().unwrap_or_default();
                        evidence.identity == *identity
                            && evidence.intent_authority_digest.as_deref()
                                == Some(intent.authority_digest.as_str())
                            && evidence.session_id == session.session_id
                            && evidence.role == member.material.profile_name
                            && evidence.runtime_mode == expected_runtime_mode
                            && evidence.labels == expected_labels
                    }
                    IdentityIntent::Absent { .. } => false,
                };
                if matches {
                    IdentityResourceObservation::Matching {
                        version: version.clone(),
                    }
                } else {
                    IdentityResourceObservation::Divergent {
                        version: version.clone(),
                        detail: "member realization does not match the current sealed intent authority and session/material projection".to_string(),
                    }
                }
            }
        }
    }
}

/// Fresh structural observation of all local wiring edges incident to one
/// identity.
///
/// The version is content-addressed from `(mob, identity, incident_edges)`.
/// Unrelated event-log writes and unrelated edges therefore cannot invalidate
/// a permit, while any semantic change to this target does. Returning to the
/// same exact edge set is a safe content-level ABA: the proposed idempotent
/// edge mutation sees the same target state it was authorized against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IdentityWiringTargetObservation {
    Absent {
        absence_version: String,
    },
    Present {
        version: String,
        incident_edges: BTreeSet<crate::identity::DesiredIdentityEdge>,
    },
    Malformed {
        observed_version: Option<String>,
        detail: String,
    },
}

impl IdentityWiringTargetObservation {
    pub fn target_precondition(&self) -> Option<IdentityTargetObservationVersion> {
        match self {
            Self::Absent { absence_version } => Some(IdentityTargetObservationVersion::Absent {
                absence_version: absence_version.clone(),
            }),
            Self::Present { version, .. } => Some(IdentityTargetObservationVersion::Version {
                version: version.clone(),
            }),
            Self::Malformed { .. } => None,
        }
    }

    #[must_use]
    pub fn incident_edges(&self) -> Option<&BTreeSet<crate::identity::DesiredIdentityEdge>> {
        match self {
            Self::Absent { .. } => Some(empty_identity_wiring_edges()),
            Self::Present { incident_edges, .. } => Some(incident_edges),
            Self::Malformed { .. } => None,
        }
    }

    #[must_use]
    pub fn contains(&self, edge: &crate::identity::DesiredIdentityEdge) -> bool {
        matches!(self, Self::Present { incident_edges, .. } if incident_edges.contains(edge))
    }
}

fn empty_identity_wiring_edges() -> &'static BTreeSet<crate::identity::DesiredIdentityEdge> {
    static EMPTY: std::sync::OnceLock<BTreeSet<crate::identity::DesiredIdentityEdge>> =
        std::sync::OnceLock::new();
    EMPTY.get_or_init(BTreeSet::new)
}

/// Store-owned wall clock for bounded identity leases and actuation fences.
///
/// The public mutation API deliberately does not accept a caller-authored
/// timestamp: advancing time is lease authority. Built-in stores use the
/// system clock; deterministic stores may inject an implementation for tests.
pub trait MobIdentityStoreClock: Send + Sync {
    fn now_ms(&self) -> Result<u64, MobStoreError>;
}

pub(crate) fn validate_identity_declaration_replay_request(
    mob_id: &MobId,
    scope_id: &crate::identity::IdentityDeclarationScopeId,
    operation_id: &meerkat_core::ops::OperationId,
    request_digest: &str,
) -> Result<(), MobStoreError> {
    let digest = request_digest.strip_prefix("sha256:");
    if mob_id.as_str().is_empty()
        || mob_id.as_str().trim() != mob_id.as_str()
        || scope_id.as_str().is_empty()
        || scope_id.as_str().trim() != scope_id.as_str()
        || operation_id.0.is_nil()
        || digest.is_none_or(|hex| {
            hex.len() != 64
                || !hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    {
        return Err(MobStoreError::Serialization(
            "invalid identity declaration replay key".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct SystemMobIdentityStoreClock;

impl MobIdentityStoreClock for SystemMobIdentityStoreClock {
    fn now_ms(&self) -> Result<u64, MobStoreError> {
        let millis = meerkat_core::time_compat::SystemTime::now()
            .duration_since(meerkat_core::time_compat::SystemTime::UNIX_EPOCH)
            .map_err(|error| {
                MobStoreError::Internal(format!("system clock before epoch: {error}"))
            })?
            .as_millis();
        u64::try_from(millis)
            .map_err(|_| MobStoreError::Internal("system clock millisecond overflow".to_string()))
    }
}

pub(crate) fn validate_mob_event_write_authority(kind: &MobEventKind) -> Result<(), MobStoreError> {
    if let MobEventKind::StepTargetFailed {
        error_report,
        error,
        ..
    } = kind
    {
        if error_report.is_some() || error.is_some() {
            return Err(MobStoreError::Internal(
                "step target terminal error metadata requires generated mob authority".to_string(),
            ));
        }
    }
    Ok(())
}

/// Persisted runtime-side supervisor authority for a mob.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SupervisorAuthorityRecord {
    /// Raw secret bytes for reconstructing the mob-owned supervisor keypair.
    pub secret_key: [u8; 32],
    /// Canonical peer id string for the corresponding public key.
    pub public_peer_id: String,
    /// Monotonic supervisor epoch for stale-authority rejection.
    pub epoch: u64,
    /// Protocol version carried on supervisor commands.
    pub protocol_version: BridgeProtocolVersion,
    /// Explicit pending rotation retained after a partial remote rotation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_rotation: Option<SupervisorPendingRotationRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SupervisorPendingRotationRecord {
    /// Stable mob-wide operation id reused for every member submission and
    /// every retry of this attempted authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<SupervisorRotationOperationId>,
    /// Raw secret bytes for reconstructing the attempted supervisor keypair.
    pub secret_key: [u8; 32],
    /// Canonical peer id string for the attempted supervisor public key.
    pub public_peer_id: String,
    /// Attempted supervisor epoch.
    pub epoch: u64,
    /// Protocol version carried on supervisor commands.
    pub protocol_version: BridgeProtocolVersion,
    /// Remote peer ids that already accepted this attempted authority.
    #[serde(default)]
    pub accepted_peer_ids: Vec<String>,
    /// Exact per-member target specs committed before the first submission.
    ///
    /// The supervisor bridge address may be ephemeral. Retries must use these
    /// durable specs rather than recomputing a target that can no longer match
    /// the member-owned operation receipt.
    #[serde(default)]
    pub member_targets: std::collections::BTreeMap<String, BridgePeerSpec>,
}

impl SupervisorAuthorityRecord {
    /// Create a fresh supervisor authority candidate.
    ///
    /// The generated `MobMachine` authority must admit the candidate before
    /// runtime code persists it or installs it into a bridge runtime.
    pub fn generate(protocol_version: BridgeProtocolVersion) -> Self {
        let keypair = meerkat_comms::Keypair::generate();
        Self {
            secret_key: keypair.secret_bytes(),
            public_peer_id: keypair.public_key().to_peer_id().as_str(),
            epoch: 0,
            protocol_version,
            pending_rotation: None,
        }
    }

    /// Reconstruct the signing keypair for runtime use.
    pub fn keypair(&self) -> meerkat_comms::Keypair {
        meerkat_comms::Keypair::from_secret(self.secret_key)
    }

    pub fn public_signing_key(&self) -> [u8; 32] {
        *self.keypair().public_key().as_bytes()
    }

    pub fn dsl_peer_id(&self) -> mob_dsl::PeerId {
        mob_dsl::PeerId::from(self.public_peer_id.clone())
    }

    pub fn dsl_signing_key(&self) -> mob_dsl::PeerSigningKey {
        mob_dsl::PeerSigningKey::from(self.public_signing_key())
    }

    pub fn dsl_protocol_version(&self) -> mob_dsl::SupervisorProtocolVersion {
        mob_dsl::SupervisorProtocolVersion::from(self.protocol_version)
    }

    pub fn dsl_recover_signal(&self) -> mob_dsl::MobMachineSignal {
        let pending = self.pending_rotation.as_ref();
        mob_dsl::MobMachineSignal::RecoverSupervisorAuthority {
            peer_id: self.dsl_peer_id(),
            signing_key: self.dsl_signing_key(),
            epoch: self.epoch,
            protocol_version: self.dsl_protocol_version(),
            pending_operation_id: pending
                .and_then(|pending| pending.operation_id)
                .map(|operation_id| operation_id.to_string()),
            pending_peer_id: pending.map(SupervisorPendingRotationRecord::dsl_peer_id),
            pending_signing_key: pending.map(SupervisorPendingRotationRecord::dsl_signing_key),
            pending_epoch: pending.map(|pending| pending.epoch),
            pending_protocol_version: pending
                .map(SupervisorPendingRotationRecord::dsl_protocol_version),
            pending_accepted_peer_ids: pending
                .map(SupervisorPendingRotationRecord::dsl_accepted_peer_ids)
                .unwrap_or_default(),
            pending_member_target_names: pending
                .map(SupervisorPendingRotationRecord::dsl_member_target_names)
                .unwrap_or_default(),
            pending_member_target_addresses: pending
                .map(SupervisorPendingRotationRecord::dsl_member_target_addresses)
                .unwrap_or_default(),
        }
    }

    pub fn dsl_provision_input(&self) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::ProvisionSupervisorAuthority {
            peer_id: self.dsl_peer_id(),
            signing_key: self.dsl_signing_key(),
            epoch: self.epoch,
            protocol_version: self.dsl_protocol_version(),
        }
    }

    pub fn dsl_record_pending_rotation_input(
        &self,
        pending: &SupervisorPendingRotationRecord,
        active_peer_ids: &std::collections::BTreeSet<String>,
    ) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::RecordSupervisorPendingRotation {
            current_peer_id: self.dsl_peer_id(),
            current_epoch: self.epoch,
            current_protocol_version: self.dsl_protocol_version(),
            operation_id: pending
                .operation_id
                .map(|operation_id| operation_id.to_string())
                .unwrap_or_default(),
            pending_peer_id: pending.dsl_peer_id(),
            pending_signing_key: pending.dsl_signing_key(),
            pending_epoch: pending.epoch,
            pending_protocol_version: pending.dsl_protocol_version(),
            accepted_peer_ids: pending.dsl_accepted_peer_ids(),
            active_peer_ids: active_peer_ids
                .iter()
                .cloned()
                .map(mob_dsl::PeerId::from)
                .collect(),
            member_target_names: pending.dsl_member_target_names(),
            member_target_addresses: pending.dsl_member_target_addresses(),
        }
    }

    pub fn dsl_commit_rotation_input(
        &self,
        operation_id: &str,
        next: &SupervisorAuthorityRecord,
    ) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::CommitSupervisorRotation {
            current_peer_id: self.dsl_peer_id(),
            current_epoch: self.epoch,
            current_protocol_version: self.dsl_protocol_version(),
            operation_id: operation_id.to_string(),
            next_peer_id: next.dsl_peer_id(),
            next_signing_key: next.dsl_signing_key(),
            next_epoch: next.epoch,
            next_protocol_version: next.dsl_protocol_version(),
        }
    }

    pub fn dsl_clear_authority_for_destroy_input(&self) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::ClearSupervisorAuthorityForDestroy {
            current_peer_id: self.dsl_peer_id(),
            current_signing_key: self.dsl_signing_key(),
            current_epoch: self.epoch,
            protocol_version: self.dsl_protocol_version(),
        }
    }

    pub fn dsl_restore_after_destroy_rollback_input(&self) -> mob_dsl::MobMachineInput {
        let pending = self.pending_rotation.as_ref();
        mob_dsl::MobMachineInput::RestoreSupervisorAuthorityAfterDestroyRollback {
            peer_id: self.dsl_peer_id(),
            signing_key: self.dsl_signing_key(),
            epoch: self.epoch,
            protocol_version: self.dsl_protocol_version(),
            pending_operation_id: pending
                .and_then(|pending| pending.operation_id)
                .map(|operation_id| operation_id.to_string()),
            pending_peer_id: pending.map(SupervisorPendingRotationRecord::dsl_peer_id),
            pending_signing_key: pending.map(SupervisorPendingRotationRecord::dsl_signing_key),
            pending_epoch: pending.map(|pending| pending.epoch),
            pending_protocol_version: pending
                .map(SupervisorPendingRotationRecord::dsl_protocol_version),
            pending_accepted_peer_ids: pending
                .map(SupervisorPendingRotationRecord::dsl_accepted_peer_ids)
                .unwrap_or_default(),
            pending_member_target_names: pending
                .map(SupervisorPendingRotationRecord::dsl_member_target_names)
                .unwrap_or_default(),
            pending_member_target_addresses: pending
                .map(SupervisorPendingRotationRecord::dsl_member_target_addresses)
                .unwrap_or_default(),
        }
    }

    pub fn without_pending_rotation(&self) -> Self {
        let mut record = self.clone();
        record.pending_rotation = None;
        record
    }
}

impl SupervisorPendingRotationRecord {
    pub fn from_authority(
        authority: &SupervisorAuthorityRecord,
        operation_id: SupervisorRotationOperationId,
        accepted_peer_ids: Vec<String>,
        member_targets: std::collections::BTreeMap<String, BridgePeerSpec>,
    ) -> Self {
        Self {
            operation_id: Some(operation_id),
            secret_key: authority.secret_key,
            public_peer_id: authority.public_peer_id.clone(),
            epoch: authority.epoch,
            protocol_version: authority.protocol_version,
            accepted_peer_ids,
            member_targets,
        }
    }

    pub fn authority_record(&self) -> SupervisorAuthorityRecord {
        SupervisorAuthorityRecord {
            secret_key: self.secret_key,
            public_peer_id: self.public_peer_id.clone(),
            epoch: self.epoch,
            protocol_version: self.protocol_version,
            pending_rotation: None,
        }
    }

    pub fn public_signing_key(&self) -> [u8; 32] {
        *meerkat_comms::Keypair::from_secret(self.secret_key)
            .public_key()
            .as_bytes()
    }

    pub fn dsl_peer_id(&self) -> mob_dsl::PeerId {
        mob_dsl::PeerId::from(self.public_peer_id.clone())
    }

    pub fn dsl_signing_key(&self) -> mob_dsl::PeerSigningKey {
        mob_dsl::PeerSigningKey::from(self.public_signing_key())
    }

    pub fn dsl_protocol_version(&self) -> mob_dsl::SupervisorProtocolVersion {
        mob_dsl::SupervisorProtocolVersion::from(self.protocol_version)
    }

    pub fn dsl_accepted_peer_ids(&self) -> std::collections::BTreeSet<mob_dsl::PeerId> {
        self.accepted_peer_ids
            .iter()
            .cloned()
            .map(mob_dsl::PeerId::from)
            .collect()
    }

    pub fn dsl_member_target_names(&self) -> std::collections::BTreeMap<mob_dsl::PeerId, String> {
        self.member_targets
            .iter()
            .map(|(member_peer_id, target)| {
                (
                    mob_dsl::PeerId::from(member_peer_id.clone()),
                    target.name.clone(),
                )
            })
            .collect()
    }

    pub fn dsl_member_target_addresses(
        &self,
    ) -> std::collections::BTreeMap<mob_dsl::PeerId, String> {
        self.member_targets
            .iter()
            .map(|(member_peer_id, target)| {
                (
                    mob_dsl::PeerId::from(member_peer_id.clone()),
                    target.address.clone(),
                )
            })
            .collect()
    }

    pub fn same_attempted_authority(&self, other: &Self) -> bool {
        self.operation_id == other.operation_id
            && self.secret_key == other.secret_key
            && self.public_peer_id == other.public_peer_id
            && self.epoch == other.epoch
            && self
                .protocol_version
                .same_protocol_as(other.protocol_version)
            && self.member_targets == other.member_targets
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorAuthorityPersistenceAuthority {
    peer_id: mob_dsl::PeerId,
    signing_key: mob_dsl::PeerSigningKey,
    epoch: u64,
    protocol_version: BridgeProtocolVersion,
    pending_peer_id: Option<mob_dsl::PeerId>,
    pending_signing_key: Option<mob_dsl::PeerSigningKey>,
    pending_epoch: Option<u64>,
    pending_protocol_version: Option<BridgeProtocolVersion>,
    pending_operation_id: Option<String>,
    pending_accepted_peer_ids: std::collections::BTreeSet<mob_dsl::PeerId>,
    pending_member_target_names: std::collections::BTreeMap<mob_dsl::PeerId, String>,
    pending_member_target_addresses: std::collections::BTreeMap<mob_dsl::PeerId, String>,
}

fn dsl_bridge_protocol_version_matches(
    dsl: &mob_dsl::SupervisorProtocolVersion,
    bridge: BridgeProtocolVersion,
) -> bool {
    dsl == &mob_dsl::SupervisorProtocolVersion::from(bridge)
}

fn optional_dsl_bridge_protocol_version_matches(
    dsl: Option<&mob_dsl::SupervisorProtocolVersion>,
    bridge: Option<BridgeProtocolVersion>,
) -> bool {
    match (dsl, bridge) {
        (Some(dsl), Some(bridge)) => dsl_bridge_protocol_version_matches(dsl, bridge),
        (None, None) => true,
        _ => false,
    }
}

fn optional_bridge_protocol_versions_match(
    left: Option<BridgeProtocolVersion>,
    right: Option<BridgeProtocolVersion>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.same_protocol_as(right),
        (None, None) => true,
        _ => false,
    }
}

impl SupervisorAuthorityPersistenceAuthority {
    pub fn from_transition(
        record: &SupervisorAuthorityRecord,
        transition: &mob_dsl::MobMachineTransition,
    ) -> Result<Self, MobStoreError> {
        let Some((
            peer_id,
            signing_key,
            epoch,
            effect_protocol,
            pending_operation_id,
            pending_peer_id,
            pending_signing_key,
            pending_epoch,
            effect_pending_protocol,
            pending_accepted_peer_ids,
            pending_member_target_names,
            pending_member_target_addresses,
        )) = transition.effects().iter().find_map(|effect| match effect {
            mob_dsl::MobMachineEffect::PersistSupervisorAuthority {
                peer_id,
                signing_key,
                epoch,
                protocol_version,
                pending_operation_id,
                pending_peer_id,
                pending_signing_key,
                pending_epoch,
                pending_protocol_version,
                pending_accepted_peer_ids,
                pending_member_target_names,
                pending_member_target_addresses,
            } => Some((
                peer_id.clone(),
                *signing_key,
                *epoch,
                protocol_version.clone(),
                pending_operation_id.clone(),
                pending_peer_id.clone(),
                *pending_signing_key,
                *pending_epoch,
                pending_protocol_version.clone(),
                pending_accepted_peer_ids.clone(),
                pending_member_target_names.clone(),
                pending_member_target_addresses.clone(),
            )),
            _ => None,
        })
        else {
            return Err(MobStoreError::Internal(
                "generated supervisor persistence authority effect is absent".to_string(),
            ));
        };
        let pending = record.pending_rotation.as_ref();
        if !dsl_bridge_protocol_version_matches(&effect_protocol, record.protocol_version)
            || !optional_dsl_bridge_protocol_version_matches(
                effect_pending_protocol.as_ref(),
                pending.map(|pending| pending.protocol_version),
            )
        {
            return Err(MobStoreError::Internal(format!(
                "generated supervisor persistence authority protocol does not match record peer={} epoch={}",
                record.public_peer_id, record.epoch
            )));
        }
        let effect = Self {
            peer_id,
            signing_key,
            epoch,
            protocol_version: record.protocol_version,
            pending_peer_id,
            pending_signing_key,
            pending_epoch,
            pending_protocol_version: pending.map(|pending| pending.protocol_version),
            pending_operation_id,
            pending_accepted_peer_ids,
            pending_member_target_names,
            pending_member_target_addresses,
        };
        effect.verify_record(record)?;
        Ok(effect)
    }

    pub fn verify_record(&self, record: &SupervisorAuthorityRecord) -> Result<(), MobStoreError> {
        let pending = record.pending_rotation.as_ref();
        let pending_peer_id = pending.map(SupervisorPendingRotationRecord::dsl_peer_id);
        let pending_signing_key = pending.map(SupervisorPendingRotationRecord::dsl_signing_key);
        let pending_epoch = pending.map(|pending| pending.epoch);
        let pending_protocol_version = pending.map(|pending| pending.protocol_version);
        let pending_operation_id = pending
            .and_then(|pending| pending.operation_id)
            .map(|operation_id| operation_id.to_string());
        let pending_accepted_peer_ids = pending
            .map(SupervisorPendingRotationRecord::dsl_accepted_peer_ids)
            .unwrap_or_default();
        let pending_member_target_names = pending
            .map(SupervisorPendingRotationRecord::dsl_member_target_names)
            .unwrap_or_default();
        let pending_member_target_addresses = pending
            .map(SupervisorPendingRotationRecord::dsl_member_target_addresses)
            .unwrap_or_default();
        if let Some(pending) = pending {
            for (member_peer_id, target) in &pending.member_targets {
                if target.peer_id != pending.public_peer_id
                    || target.pubkey != pending.public_signing_key()
                {
                    return Err(MobStoreError::Internal(format!(
                        "supervisor rotation target for member '{member_peer_id}' does not match pending authority peer={} epoch={}",
                        pending.public_peer_id, pending.epoch
                    )));
                }
            }
        }
        if self.peer_id == record.dsl_peer_id()
            && self.signing_key == record.dsl_signing_key()
            && self.epoch == record.epoch
            && self
                .protocol_version
                .same_protocol_as(record.protocol_version)
            && self.pending_peer_id == pending_peer_id
            && self.pending_signing_key == pending_signing_key
            && self.pending_epoch == pending_epoch
            && optional_bridge_protocol_versions_match(
                self.pending_protocol_version,
                pending_protocol_version,
            )
            && self.pending_operation_id == pending_operation_id
            && self.pending_accepted_peer_ids == pending_accepted_peer_ids
            && self.pending_member_target_names == pending_member_target_names
            && self.pending_member_target_addresses == pending_member_target_addresses
        {
            return Ok(());
        }

        Err(MobStoreError::Internal(format!(
            "generated supervisor persistence authority does not match record peer={} epoch={}",
            record.public_peer_id, record.epoch
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorAuthorityBridgeAuthority {
    peer_id: mob_dsl::PeerId,
    signing_key: mob_dsl::PeerSigningKey,
    epoch: u64,
    protocol_version: BridgeProtocolVersion,
}

impl SupervisorAuthorityBridgeAuthority {
    fn from_record(record: &SupervisorAuthorityRecord) -> Self {
        Self {
            peer_id: record.dsl_peer_id(),
            signing_key: record.dsl_signing_key(),
            epoch: record.epoch,
            protocol_version: record.protocol_version,
        }
    }

    pub fn from_persistence_authority(
        record: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityPersistenceAuthority,
    ) -> Result<Self, MobStoreError> {
        authority.verify_record(record)?;
        Ok(Self::from_record(record))
    }

    pub fn from_machine_state(
        record: &SupervisorAuthorityRecord,
        state: &mob_dsl::MobMachineState,
    ) -> Result<Self, MobStoreError> {
        let authority = Self::from_record(record);
        let current_matches = state.supervisor_authority_peer_id.as_ref()
            == Some(&authority.peer_id)
            && state.supervisor_authority_signing_key == Some(authority.signing_key)
            && state.supervisor_authority_epoch == Some(authority.epoch)
            && optional_dsl_bridge_protocol_version_matches(
                state.supervisor_authority_protocol_version.as_ref(),
                Some(authority.protocol_version),
            );
        let pending_matches = state.supervisor_pending_authority_peer_id.as_ref()
            == Some(&authority.peer_id)
            && state.supervisor_pending_authority_signing_key == Some(authority.signing_key)
            && state.supervisor_pending_authority_epoch == Some(authority.epoch)
            && optional_dsl_bridge_protocol_version_matches(
                state.supervisor_pending_authority_protocol_version.as_ref(),
                Some(authority.protocol_version),
            );
        if current_matches || pending_matches {
            return Ok(authority);
        }

        Err(MobStoreError::Internal(format!(
            "generated supervisor bridge authority is absent for record peer={} epoch={}",
            record.public_peer_id, record.epoch
        )))
    }

    pub fn verify_record(&self, record: &SupervisorAuthorityRecord) -> Result<(), MobStoreError> {
        if self.peer_id == record.dsl_peer_id()
            && self.signing_key == record.dsl_signing_key()
            && self.epoch == record.epoch
            && self
                .protocol_version
                .same_protocol_as(record.protocol_version)
        {
            return Ok(());
        }

        Err(MobStoreError::Internal(format!(
            "generated supervisor bridge authority does not match record peer={} epoch={}",
            record.public_peer_id, record.epoch
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorAuthorityDeletionAuthority {
    peer_id: mob_dsl::PeerId,
    signing_key: mob_dsl::PeerSigningKey,
    epoch: u64,
    protocol_version: BridgeProtocolVersion,
}

impl SupervisorAuthorityDeletionAuthority {
    pub fn from_transition(
        record: &SupervisorAuthorityRecord,
        transition: &mob_dsl::MobMachineTransition,
    ) -> Result<Self, MobStoreError> {
        let Some((peer_id, signing_key, epoch, effect_protocol)) =
            transition.effects().iter().find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::DeleteSupervisorAuthority {
                    peer_id,
                    signing_key,
                    epoch,
                    protocol_version,
                } => Some((
                    peer_id.clone(),
                    *signing_key,
                    *epoch,
                    protocol_version.clone(),
                )),
                _ => None,
            })
        else {
            return Err(MobStoreError::Internal(
                "generated supervisor deletion authority effect is absent".to_string(),
            ));
        };
        if !dsl_bridge_protocol_version_matches(&effect_protocol, record.protocol_version) {
            return Err(MobStoreError::Internal(format!(
                "generated supervisor deletion authority protocol does not match record peer={} epoch={}",
                record.public_peer_id, record.epoch
            )));
        }
        let effect = Self {
            peer_id,
            signing_key,
            epoch,
            protocol_version: record.protocol_version,
        };
        effect.verify_record(record)?;
        Ok(effect)
    }

    pub fn verify_record(&self, record: &SupervisorAuthorityRecord) -> Result<(), MobStoreError> {
        if self.peer_id == record.dsl_peer_id()
            && self.signing_key == record.dsl_signing_key()
            && self.epoch == record.epoch
            && self
                .protocol_version
                .same_protocol_as(record.protocol_version)
        {
            return Ok(());
        }

        Err(MobStoreError::Internal(format!(
            "generated supervisor deletion authority does not match record peer={} epoch={}",
            record.public_peer_id, record.epoch
        )))
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn supervisor_authority_persistence_authority_for_record(
    record: &SupervisorAuthorityRecord,
) -> Result<SupervisorAuthorityPersistenceAuthority, MobStoreError> {
    let mut authority = mob_dsl::MobMachineAuthority::new();
    let transition =
        mob_dsl::MobMachineMutator::apply(&mut authority, record.dsl_provision_input()).map_err(
            |error| {
                MobStoreError::Internal(format!(
                    "generated supervisor test persistence authority rejected record: {error}"
                ))
            },
        )?;
    SupervisorAuthorityPersistenceAuthority::from_transition(record, &transition)
}

#[cfg(any(test, feature = "test-support"))]
pub fn supervisor_authority_deletion_authority_for_record(
    record: &SupervisorAuthorityRecord,
) -> Result<SupervisorAuthorityDeletionAuthority, MobStoreError> {
    let mut authority = mob_dsl::MobMachineAuthority::new();
    authority
        .apply_signal(record.dsl_recover_signal())
        .map_err(|error| {
            MobStoreError::Internal(format!(
                "generated supervisor test deletion recovery rejected record: {error}"
            ))
        })?;
    mob_dsl::MobMachineMutator::apply(
        &mut authority,
        mob_dsl::MobMachineInput::BeginPlacedCompletionLifecycleQuiesce {
            intent: mob_dsl::PlacedCompletionLifecycleIntentKind::Destroy,
        },
    )
    .map_err(|error| {
        MobStoreError::Internal(format!(
            "generated supervisor test deletion quiesce rejected record: {error}"
        ))
    })?;
    mob_dsl::MobMachineMutator::apply(&mut authority, mob_dsl::MobMachineInput::Destroy).map_err(
        |error| {
            MobStoreError::Internal(format!(
                "generated supervisor test deletion destroy rejected record: {error}"
            ))
        },
    )?;
    let transition = mob_dsl::MobMachineMutator::apply(
        &mut authority,
        record.dsl_clear_authority_for_destroy_input(),
    )
    .map_err(|error| {
        MobStoreError::Internal(format!(
            "generated supervisor test deletion authority rejected record: {error}"
        ))
    })?;
    SupervisorAuthorityDeletionAuthority::from_transition(record, &transition)
}

/// Typed bind-phase mirror for a persisted bound-host record (FLAG-3).
///
/// Records are written only for committed binds, so `Requested` is
/// deliberately unrepresentable here — a kind with no producer is dead
/// vocabulary. Recovery therefore always restores the `Bound` fact.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MobHostBindPhaseRecord {
    Bound,
}

/// Typed mirror of the MobMachine's flattened host capability maps (§6.1
/// single enumeration). Field names stay aligned with the DSL state block;
/// this record is the durable carrier, not a second owner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MobHostCapabilityRecord {
    pub protocol_min: u64,
    pub protocol_max: u64,
    pub engine_version: String,
    pub durable_sessions: bool,
    pub autonomous_members: bool,
    pub hard_cancel_member: bool,
    #[serde(default)]
    pub tracked_input_cancel: bool,
    pub memory_store: bool,
    pub mcp: bool,
    pub resolvable_providers: std::collections::BTreeSet<String>,
    pub approval_forwarding: bool,
}

/// Persisted controlling-side record of one bound member host (FLAG-3).
///
/// One record per `(mob, host)`, written from transitions witnessed by
/// `HostRegistered` / `HostReboundRecorded` / `HostCapabilitiesRefreshed`
/// effects and deleted on
/// `HostRevoked`. Builder recovery replays it through
/// [`MobHostAuthorityRecord::dsl_recover_signal`]
/// (`MobMachineSignal::RecoverHostBinding`) so a controlling-host restart
/// re-learns its bound hosts instead of orphaning them into an operator
/// re-ceremony (the host daemon would keep answering `AlreadyBound` for a
/// mob the controlling side forgot). Carries NO bootstrap token (tokens are
/// never persisted) and no mob definition/roster/profile material (the
/// second-roster prohibition).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MobHostAuthorityRecord {
    /// Identity-first host id: the host's canonical comms `PeerId` string
    /// (D1 — never a display name).
    pub host_id: String,
    /// Canonical comms peer id used to address the host. Identity and
    /// transport addressing are separate facts even though host ids ARE
    /// peer ids today; `verify_record` pins the two equal so they cannot
    /// silently drift apart.
    pub peer_id: String,
    /// Host Ed25519 PUBLIC signing key. The controlling side never holds a
    /// host secret (contrast `SupervisorAuthorityRecord`, which owns its
    /// keypair).
    pub signing_key: [u8; 32],
    /// Advertised host endpoint address (token-free: advertised addresses
    /// never carry bootstrap tokens).
    pub endpoint: String,
    /// Host authority epoch recorded at bind/rebind.
    pub authority_epoch: u64,
    /// Durable incarnation of the controller-to-host binding. Legacy rows
    /// deserialize as generation zero; fresh binds monotonically advance it.
    #[serde(default)]
    pub binding_generation: u64,
    /// Typed bind-phase mirror (always `Bound`; see the enum doc).
    pub bind_phase: MobHostBindPhaseRecord,
    /// Typed mirror of the host's declared capabilities.
    pub capabilities: MobHostCapabilityRecord,
    /// Advertised ws/wss live base URL; `None` = live-incapable host (no
    /// shadow boolean).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_endpoint: Option<String>,
}

impl MobHostAuthorityRecord {
    pub fn dsl_host_id(&self) -> mob_dsl::HostId {
        mob_dsl::HostId::from(self.host_id.clone())
    }

    pub fn dsl_signing_key(&self) -> mob_dsl::PeerSigningKey {
        mob_dsl::PeerSigningKey::from(self.signing_key)
    }

    pub fn dsl_endpoint(&self) -> mob_dsl::PeerAddress {
        mob_dsl::PeerAddress::from(self.endpoint.clone())
    }

    /// Builder-recovery signal restoring the full bound-host fact set into
    /// the generated MobMachine (applied next to
    /// `RecoverSupervisorAuthority`).
    pub fn dsl_recover_signal(&self) -> mob_dsl::MobMachineSignal {
        mob_dsl::MobMachineSignal::RecoverHostBinding {
            host_id: self.dsl_host_id(),
            pubkey: self.dsl_signing_key(),
            endpoint: self.dsl_endpoint(),
            epoch: self.authority_epoch,
            binding_generation: self.binding_generation,
            protocol_min: self.capabilities.protocol_min,
            protocol_max: self.capabilities.protocol_max,
            engine_version: self.capabilities.engine_version.clone(),
            durable_sessions: self.capabilities.durable_sessions,
            autonomous_members: self.capabilities.autonomous_members,
            hard_cancel_member: self.capabilities.hard_cancel_member,
            tracked_input_cancel: self.capabilities.tracked_input_cancel,
            memory_store: self.capabilities.memory_store,
            mcp: self.capabilities.mcp,
            resolvable_providers: self.capabilities.resolvable_providers.clone(),
            approval_forwarding: self.capabilities.approval_forwarding,
            live_endpoint: self
                .live_endpoint
                .clone()
                .map(mob_dsl::LiveWsEndpointUrl::from),
        }
    }
}

/// Typed write permit for [`MobHostAuthorityRecord`], constructible only
/// from a MobMachine transition whose effects carry the matching
/// `HostRegistered` (bind commit), `HostReboundRecorded` (rebind), or
/// `HostCapabilitiesRefreshed` (same-binding full fact refresh) fact
/// for the record's host at the record's epoch.
///
/// The generated transition owns the exact host/epoch/generation target; the
/// prepared machine state and record construction share the full capability
/// and live-endpoint input so durable write precedes publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobHostAuthorityPersistenceAuthority {
    host_id: mob_dsl::HostId,
    epoch: u64,
    binding_generation: u64,
    /// Present only for authenticated bind-ACK recovery. Unlike the ordinary
    /// transition witness (whose generated effect carries only host/epoch/G),
    /// this durable authority must pin every capability and endpoint field.
    confirmed_record: Option<MobHostAuthorityRecord>,
}

impl MobHostAuthorityPersistenceAuthority {
    pub fn from_transition(
        record: &MobHostAuthorityRecord,
        transition: &mob_dsl::MobMachineTransition,
    ) -> Result<Self, MobStoreError> {
        let Some((host_id, epoch, binding_generation)) =
            transition.effects().iter().find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::HostRegistered {
                    host_id,
                    epoch,
                    binding_generation,
                }
                | mob_dsl::MobMachineEffect::HostReboundRecorded {
                    host_id,
                    epoch,
                    binding_generation,
                }
                | mob_dsl::MobMachineEffect::HostCapabilitiesRefreshed {
                    host_id,
                    epoch,
                    binding_generation,
                } => Some((host_id.clone(), *epoch, *binding_generation)),
                _ => None,
            })
        else {
            return Err(MobStoreError::Internal(
                "generated host binding persistence authority effect is absent".to_string(),
            ));
        };
        let authority = Self {
            host_id,
            epoch,
            binding_generation,
            confirmed_record: None,
        };
        authority.verify_record(record)?;
        Ok(authority)
    }

    /// Recovery permit minted from an authenticated structural BindHost ACK.
    /// The Started request pins descriptor identity/endpoint/epoch/G and the
    /// Confirmed row pins the complete capability record. The resulting
    /// permit can insert only this byte-for-byte record; it never overwrites.
    pub(crate) fn from_confirmed_bind_anchor(
        request: &crate::event::RemoteHostBindRequestEvent,
        record: &MobHostAuthorityRecord,
    ) -> Result<Self, MobStoreError> {
        if request.host_id != record.host_id
            || request.peer_id != record.peer_id
            || request.signing_key != record.signing_key
            || request.endpoint != record.endpoint
            || request.authority_epoch != record.authority_epoch
            || request.binding_generation != record.binding_generation
            || record.peer_id != record.host_id
            || record.bind_phase != MobHostBindPhaseRecord::Bound
        {
            return Err(MobStoreError::Internal(format!(
                "confirmed host bind authority does not match Started request for host '{}'",
                request.host_id
            )));
        }
        let authority = Self {
            host_id: record.dsl_host_id(),
            epoch: record.authority_epoch,
            binding_generation: record.binding_generation,
            confirmed_record: Some(record.clone()),
        };
        authority.verify_record(record)?;
        Ok(authority)
    }

    pub fn verify_record(&self, record: &MobHostAuthorityRecord) -> Result<(), MobStoreError> {
        if self.host_id == record.dsl_host_id()
            && self.epoch == record.authority_epoch
            && self.binding_generation == record.binding_generation
            && record.peer_id == record.host_id
            && self
                .confirmed_record
                .as_ref()
                .is_none_or(|confirmed| confirmed == record)
        {
            return Ok(());
        }

        Err(MobStoreError::Internal(format!(
            "generated host binding persistence authority does not match record host={} epoch={}",
            record.host_id, record.authority_epoch
        )))
    }
}

/// Typed deletion permit for [`MobHostAuthorityRecord`], constructible only
/// from a MobMachine transition whose effects carry the matching
/// `HostRevoked` fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobHostAuthorityDeletionAuthority {
    host_id: mob_dsl::HostId,
    binding_generation: u64,
}

impl MobHostAuthorityDeletionAuthority {
    pub fn from_transition(
        record: &MobHostAuthorityRecord,
        transition: &mob_dsl::MobMachineTransition,
    ) -> Result<Self, MobStoreError> {
        let Some((host_id, binding_generation)) =
            transition.effects().iter().find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::HostRevoked {
                    host_id,
                    binding_generation,
                } => Some((host_id.clone(), *binding_generation)),
                _ => None,
            })
        else {
            return Err(MobStoreError::Internal(
                "generated host binding deletion authority effect is absent".to_string(),
            ));
        };
        let authority = Self {
            host_id,
            binding_generation,
        };
        authority.verify_record(record)?;
        Ok(authority)
    }

    pub fn verify_record(&self, record: &MobHostAuthorityRecord) -> Result<(), MobStoreError> {
        if self.host_id == record.dsl_host_id()
            && self.binding_generation == record.binding_generation
        {
            return Ok(());
        }

        Err(MobStoreError::Internal(format!(
            "generated host binding deletion authority does not match record host={} epoch={}",
            record.host_id, record.authority_epoch
        )))
    }
}

/// Durable controlling-side capability FACTS for a host-materialized
/// member's operator (mob-tool) authority (multi-host §15.4, ADJ-15).
///
/// Stores facts, never a rehydratable sealed context: read-time consumers
/// RE-MINT the sealed `MobToolAuthorityContext` through the generated
/// authority ladder from these facts (no stored-context rehydration lane).
/// Written by the spawn ladder under the same transition witness as the
/// spec's authority context; deleted on spawn abort and member release.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MobMemberOperatorAuthorityRecord {
    /// Stable member identity the authority was minted for.
    pub agent_identity: String,
    /// Runtime generation the mint is bound to (a respawn re-mints).
    pub generation: u64,
    pub can_create_mobs: bool,
    #[serde(default)]
    pub can_mutate_profiles: bool,
    #[serde(default)]
    pub can_run_adaptive_packs: bool,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub managed_mob_scope: std::collections::BTreeSet<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub spawn_profile_scope: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
}

/// Durable controlling-side idempotency record for one member-originated
/// operator request.
///
/// The table key adds `mob_id` outside this self-describing record, yielding
/// `(mob_id, agent_identity, host_id, host_binding_generation,
/// member_session_id, generation, fence_token, request_id)`. `Pending`
/// is inserted atomically before the operator effect runs. A recovered
/// `Pending` is never permission to run the effect again: the responder
/// terminalizes it as an explicit indeterminate outcome unless a future
/// operation-specific reconciliation path can prove the original outcome.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MobMemberOperatorRequestKey {
    /// Stable requester identity.
    pub agent_identity: String,
    /// Runtime generation the requester used for this request.
    pub generation: u64,
    /// Runtime fence the requester used for this request.
    pub fence_token: u64,
    /// Host identity that originated the request.
    pub host_id: String,
    /// Host-binding generation that originated this request.
    pub host_binding_generation: u64,
    /// Exact host-resident member session that originated the request.
    pub member_session_id: String,
    /// Member-generated idempotency key, scoped by the full binding tuple.
    pub request_id: String,
}

impl MobMemberOperatorRequestKey {
    pub fn new(
        agent_identity: impl Into<String>,
        generation: u64,
        fence_token: u64,
        host_id: impl Into<String>,
        host_binding_generation: u64,
        member_session_id: impl Into<String>,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            agent_identity: agent_identity.into(),
            generation,
            fence_token,
            host_id: host_id.into(),
            host_binding_generation,
            member_session_id: member_session_id.into(),
            request_id: request_id.into(),
        }
    }
}

/// The persisted record deliberately keeps its key fields flat for on-disk
/// compatibility. Runtime and store APIs carry the cohesive typed key above,
/// preventing any caller from accidentally reordering or omitting one of the
/// admission-significant execution-fence atoms.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MobMemberOperatorRequestRecord {
    /// Stable requester identity.
    pub agent_identity: String,
    /// Runtime generation the requester used for this request.
    pub generation: u64,
    /// Runtime fence the requester used for this request. Generation and
    /// fence both participate in the key because either is independently
    /// admission-significant.
    pub fence_token: u64,
    /// Host identity that originated the request; binding generations are
    /// scoped to this host.
    pub host_id: String,
    /// Host-binding generation that originated this request. This remains
    /// part of the durable identity even when member generation/fence/session
    /// are reused by a replacement host.
    pub host_binding_generation: u64,
    /// Exact host-resident member session that originated the request.
    pub member_session_id: String,
    /// Member-generated idempotency key, scoped by the full binding tuple.
    pub request_id: String,
    /// SHA-256 over the typed [`MemberOperatorOp`] projection. One request key
    /// can never name two different operations.
    pub op_digest: String,
    /// Durable execution/reply phase.
    pub state: MobMemberOperatorRequestState,
}

/// Bounded durable negative memory per exact member incarnation. Duplicate
/// keys continue to replay at the ceiling; only a previously unseen key is
/// rejected, so exhaustion cannot authorize duplicate execution.
pub const MEMBER_OPERATOR_REQUEST_MAX_PER_INCARNATION: usize = 1024;
pub const MEMBER_OPERATOR_REQUEST_MAX_PER_MOB: usize = 4096;
pub const MEMBER_OPERATOR_REQUEST_ID_MAX_BYTES: usize = 128;
pub const MEMBER_OPERATOR_AGENT_IDENTITY_MAX_BYTES: usize = 256;

/// Exact machine-current placed residency used to authorize pruning of
/// durable member-operator negative memory. The actor constructs these only
/// from one serialized MobMachine state snapshot; stores never infer
/// residency currency from rows or wall-clock data.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MobMemberOperatorResidency {
    pub agent_identity: String,
    pub generation: u64,
    pub fence_token: u64,
    pub host_id: String,
    pub host_binding_generation: u64,
    pub member_session_id: String,
}

impl MobMemberOperatorResidency {
    fn from_record(record: &MobMemberOperatorRequestRecord) -> Self {
        Self {
            agent_identity: record.agent_identity.clone(),
            generation: record.generation,
            fence_token: record.fence_token,
            host_id: record.host_id.clone(),
            host_binding_generation: record.host_binding_generation,
            member_session_id: record.member_session_id.clone(),
        }
    }
}

/// Actor-minted authority for one stale-ledger pruning pass.
///
/// The current-residency set is closed at construction and deliberately has
/// no public constructor. Backends may delete a row only when its full tuple
/// is absent from this actor-projected set; current Pending and terminal rows
/// are therefore non-prunable regardless of quota pressure.
#[derive(Debug, Clone)]
pub struct MobMemberOperatorPruneAuthority {
    current_residencies: std::collections::BTreeSet<MobMemberOperatorResidency>,
}

impl MobMemberOperatorPruneAuthority {
    pub(crate) fn from_actor_current_residencies(
        current_residencies: std::collections::BTreeSet<MobMemberOperatorResidency>,
    ) -> Self {
        Self {
            current_residencies,
        }
    }

    fn preserves(&self, record: &MobMemberOperatorRequestRecord) -> bool {
        self.current_residencies
            .contains(&MobMemberOperatorResidency::from_record(record))
    }
}

/// Durable member-operator request phase.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum MobMemberOperatorRequestState {
    /// Persisted before any operator side effect.
    Pending,
    /// Final reply, replayed verbatim for the same operation digest.
    Terminal { reply: MemberOperatorReply },
}

/// Result of the atomic begin-if-absent operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobMemberOperatorRequestBegin {
    /// The caller inserted `Pending` and owns the one allowed execution.
    Started,
    /// The key already existed; the caller must replay/terminalize/conflict,
    /// never execute.
    Existing(MobMemberOperatorRequestRecord),
}

impl MobMemberOperatorRequestRecord {
    pub fn pending(key: MobMemberOperatorRequestKey, op_digest: impl Into<String>) -> Self {
        Self {
            agent_identity: key.agent_identity,
            generation: key.generation,
            fence_token: key.fence_token,
            host_id: key.host_id,
            host_binding_generation: key.host_binding_generation,
            member_session_id: key.member_session_id,
            request_id: key.request_id,
            op_digest: op_digest.into(),
            state: MobMemberOperatorRequestState::Pending,
        }
    }

    pub fn key(&self) -> MobMemberOperatorRequestKey {
        MobMemberOperatorRequestKey {
            agent_identity: self.agent_identity.clone(),
            generation: self.generation,
            fence_token: self.fence_token,
            host_id: self.host_id.clone(),
            host_binding_generation: self.host_binding_generation,
            member_session_id: self.member_session_id.clone(),
            request_id: self.request_id.clone(),
        }
    }

    pub fn terminal(&self, reply: MemberOperatorReply) -> Result<Self, MobStoreError> {
        if !matches!(self.state, MobMemberOperatorRequestState::Pending) {
            return Err(MobStoreError::CasConflict(format!(
                "member operator request '{}' is already terminal",
                self.request_id
            )));
        }
        if reply.request_id != self.request_id {
            return Err(MobStoreError::Internal(format!(
                "member operator reply request_id '{}' does not match ledger key '{}'",
                reply.request_id, self.request_id
            )));
        }
        let mut terminal = self.clone();
        terminal.state = MobMemberOperatorRequestState::Terminal { reply };
        Ok(terminal)
    }

    pub fn terminal_reply(&self) -> Option<&MemberOperatorReply> {
        match &self.state {
            MobMemberOperatorRequestState::Pending => None,
            MobMemberOperatorRequestState::Terminal { reply } => Some(reply),
        }
    }

    fn validate(&self) -> Result<(), MobStoreError> {
        if self.agent_identity.is_empty()
            || self.host_id.is_empty()
            || self.member_session_id.is_empty()
            || self.request_id.is_empty()
        {
            return Err(MobStoreError::Internal(
                "member operator request ledger keys must be non-empty".to_string(),
            ));
        }
        if self.host_binding_generation == 0 {
            return Err(MobStoreError::Internal(format!(
                "member operator request '{}' has zero host binding generation",
                self.request_id
            )));
        }
        if self.agent_identity.len() > MEMBER_OPERATOR_AGENT_IDENTITY_MAX_BYTES
            || self.request_id.len() > MEMBER_OPERATOR_REQUEST_ID_MAX_BYTES
        {
            return Err(MobStoreError::Internal(format!(
                "member operator request ledger key exceeds bounds (identity={} max={}, request_id={} max={})",
                self.agent_identity.len(),
                MEMBER_OPERATOR_AGENT_IDENTITY_MAX_BYTES,
                self.request_id.len(),
                MEMBER_OPERATOR_REQUEST_ID_MAX_BYTES,
            )));
        }
        if self.op_digest.len() != 64
            || !self
                .op_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(MobStoreError::Internal(format!(
                "member operator request '{}' carries an invalid SHA-256 operation digest",
                self.request_id
            )));
        }
        if let Some(reply) = self.terminal_reply()
            && reply.request_id != self.request_id
        {
            return Err(MobStoreError::Internal(format!(
                "member operator request '{}' stores a terminal reply for '{}'",
                self.request_id, reply.request_id
            )));
        }
        Ok(())
    }

    fn validate_pending(&self) -> Result<(), MobStoreError> {
        self.validate()?;
        if matches!(self.state, MobMemberOperatorRequestState::Pending) {
            Ok(())
        } else {
            Err(MobStoreError::Internal(format!(
                "member operator request '{}' begin record must be pending",
                self.request_id
            )))
        }
    }

    fn validate_terminal_transition(&self, next: &Self) -> Result<(), MobStoreError> {
        self.validate_pending()?;
        next.validate()?;
        if self.agent_identity != next.agent_identity
            || self.generation != next.generation
            || self.fence_token != next.fence_token
            || self.host_id != next.host_id
            || self.host_binding_generation != next.host_binding_generation
            || self.member_session_id != next.member_session_id
            || self.request_id != next.request_id
            || self.op_digest != next.op_digest
            || !matches!(next.state, MobMemberOperatorRequestState::Terminal { .. })
        {
            return Err(MobStoreError::Internal(format!(
                "member operator request '{}' terminal transition changed its key/digest or did not terminalize",
                self.request_id
            )));
        }
        Ok(())
    }
}

/// Compute the operation digest used by the durable upcall ledger.
///
/// `MemberOperatorOp` is a closed, derive-serialized typed enum. Its only
/// opaque JSON carrier is already stored as a canonical string, so this
/// projection is deterministic and preserves exact operation identity.
pub fn member_operator_op_digest(op: &MemberOperatorOp) -> Result<String, MobStoreError> {
    let encoded = serde_json::to_vec(op).map_err(|error| {
        MobStoreError::Serialization(format!(
            "member operator operation digest encoding failed: {error}"
        ))
    })?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

/// Current on-disk schema for the canonical placed-spawn carrier.
pub const PLACED_SPAWN_CARRIER_SCHEMA_VERSION: u32 = 4;

/// Conservative replay-payload ceiling below the 1 MiB supervisor-bridge
/// envelope. The remaining 256 KiB is reserved for authenticated routing and
/// framing overhead.
pub const PLACED_KICKOFF_INTENT_MAX_ENCODED_BYTES: usize = 768 * 1024;

/// Exact, private replay material for a placed autonomous member's kickoff.
///
/// This lives beside the canonical placed-spawn carrier rather than in
/// [`meerkat_contracts::wire::PortableMemberSpec`]: the portable spec owns the
/// member BUILD and deliberately excludes the first real turn. The intent is
/// immutable from Pending through Committed and host-binding promotion, so a
/// controller restart can resend the same model-visible content at the same
/// runtime idempotency/correlation identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobPlacedKickoffIntent {
    /// Canonical UUID text used as runtime input id, idempotency key, and
    /// interaction correlation.
    pub input_id: String,
    /// Durable delegated-objective causality stamped onto the kickoff turn.
    pub objective_id: meerkat_core::interaction::ObjectiveId,
    /// Exact model-visible kickoff content after fork/fallback composition.
    pub prompt: meerkat_core::types::ContentInput,
    /// Placed kickoff is queue-admitted; retaining the typed value makes an
    /// accidental replay-policy change detectable rather than implicit.
    pub handling_mode: meerkat_core::types::HandlingMode,
    /// Exact host-attached context. Empty today, retained so replay cannot
    /// silently drop a future non-empty carrier.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub injected_context: Vec<meerkat_core::types::ContentInput>,
}

impl MobPlacedKickoffIntent {
    pub fn validate(&self) -> Result<(), MobStoreError> {
        let input_id = uuid::Uuid::parse_str(&self.input_id).map_err(|error| {
            MobStoreError::Internal(format!(
                "placed kickoff input_id '{}' is not a UUID: {error}",
                self.input_id
            ))
        })?;
        if input_id.is_nil() || self.input_id != input_id.to_string() {
            return Err(MobStoreError::Internal(format!(
                "placed kickoff input_id '{}' is not a canonical non-nil UUID",
                self.input_id
            )));
        }
        if self.objective_id.0.is_nil() {
            return Err(MobStoreError::Internal(
                "placed kickoff objective_id must be non-nil".to_string(),
            ));
        }
        if self.handling_mode != meerkat_core::types::HandlingMode::Queue {
            return Err(MobStoreError::Internal(
                "placed kickoff handling_mode must be queue".to_string(),
            ));
        }
        let encoded_bytes = serde_json::to_vec(self)
            .map_err(|error| {
                MobStoreError::Serialization(format!(
                    "placed kickoff intent encoding failed: {error}"
                ))
            })?
            .len();
        if encoded_bytes > PLACED_KICKOFF_INTENT_MAX_ENCODED_BYTES {
            return Err(MobStoreError::Internal(format!(
                "placed kickoff intent is {encoded_bytes} bytes; maximum is {PLACED_KICKOFF_INTENT_MAX_ENCODED_BYTES}"
            )));
        }
        Ok(())
    }
}

/// Canonical controlling-side authority for one placed-member spawn attempt.
///
/// The digest-covered portable spec is the sole owner of profile, runtime
/// mode, addressability, labels, continuity, and per-member operator facts.
/// This record adds only lifecycle tuple facts that cannot be derived from the
/// spec. `Pending` never grants member-operator authority; only an exact
/// `Committed` carrier may be projected into an authority record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobPlacedSpawnCarrierRecord {
    pub schema_version: u32,
    /// Controller-minted ABA fence for this exact attempt.
    pub spawn_id: PlacedSpawnId,
    pub agent_identity: String,
    pub generation: u64,
    pub fence_token: u64,
    /// Pre-minted before any MaterializeMember dispatch. Cold cleanup aborts
    /// this exact operation after host absence/release is certified.
    pub provision_operation_id: meerkat_core::ops::OperationId,
    /// Exact owner registry/session that owns `provision_operation_id`.
    pub operation_owner_session_id: meerkat_core::SessionId,
    pub host_id: meerkat_core::comms::PeerId,
    /// Exact authenticated host-binding generation under which this attempt
    /// was materialized. It is immutable from Pending through the initial
    /// Committed transition and advances only through the dedicated committed
    /// carrier generation CAS after replacement-host authentication.
    pub host_binding_generation: u64,
    pub spec_digest: String,
    pub spec: meerkat_contracts::wire::PortableMemberSpec,
    /// Exact first-turn intent for a placed autonomous member. It is private
    /// runtime metadata, never part of the public member spec or event shape.
    /// Turn-driven placed members carry `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kickoff_intent: Option<MobPlacedKickoffIntent>,
    /// Presence-sensitive profile provenance not derivable from
    /// `spec.profile`: whether the live MemberSpawned payload carried a full
    /// effective override. Recovery rehydrates the value losslessly from the
    /// portable profile only when this bit is true.
    pub effective_profile_override_present: bool,
    /// Whether the digest-covered portable profile's model was selected by a
    /// field-scoped model override. When absent, false preserves the
    /// definition-owned model semantics.
    #[serde(default)]
    pub effective_model_override_present: bool,
    pub phase: PlacedSpawnCarrierPhase,
}

impl MobPlacedSpawnCarrierRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn pending(
        spawn_id: PlacedSpawnId,
        agent_identity: String,
        generation: u64,
        fence_token: u64,
        provision_operation_id: meerkat_core::ops::OperationId,
        operation_owner_session_id: meerkat_core::SessionId,
        host_id: meerkat_core::comms::PeerId,
        host_binding_generation: u64,
        spec_digest: String,
        spec: meerkat_contracts::wire::PortableMemberSpec,
        kickoff_intent: Option<MobPlacedKickoffIntent>,
        effective_profile_override_present: bool,
        effective_model_override_present: bool,
    ) -> Self {
        Self {
            schema_version: PLACED_SPAWN_CARRIER_SCHEMA_VERSION,
            spawn_id,
            agent_identity,
            generation,
            fence_token,
            provision_operation_id,
            operation_owner_session_id,
            host_id,
            host_binding_generation,
            spec_digest,
            spec,
            kickoff_intent,
            effective_profile_override_present,
            effective_model_override_present,
            phase: PlacedSpawnCarrierPhase::Pending,
        }
    }

    /// Validate all self-contained carrier facts. Mob-key binding is checked
    /// by [`Self::validate_for_mob`] at every store boundary.
    pub fn validate(&self) -> Result<(), MobStoreError> {
        if self.schema_version != PLACED_SPAWN_CARRIER_SCHEMA_VERSION {
            return Err(MobStoreError::Internal(format!(
                "unsupported placed-spawn carrier schema_version={}; expected {}",
                self.schema_version, PLACED_SPAWN_CARRIER_SCHEMA_VERSION
            )));
        }
        if self.spawn_id.as_uuid().is_nil()
            || self.agent_identity.is_empty()
            || self.provision_operation_id.0.is_nil()
            || self.operation_owner_session_id.0.is_nil()
            || self.host_id.as_uuid().is_nil()
            || self.host_binding_generation == 0
            || self.spec.mob_id.is_empty()
            || self.spec.profile_name.is_empty()
            || self.spec.agent_identity != self.agent_identity
            || !self
                .spec
                .definition_extract
                .profile_names
                .contains(&self.spec.profile_name)
            || self
                .spec
                .profile
                .skills
                .iter()
                .any(|skill| !self.spec.definition_extract.skills.contains_key(skill))
        {
            return Err(MobStoreError::Internal(format!(
                "invalid placed-spawn carrier for identity='{}' generation={} fence={}",
                self.agent_identity, self.generation, self.fence_token
            )));
        }
        let canonical_digest = meerkat_contracts::wire::portable_member_spec_digest(&self.spec)
            .map_err(|error| {
                MobStoreError::Serialization(format!(
                    "placed-spawn carrier canonical spec digest failed: {error}"
                ))
            })?;
        if self.spec_digest != canonical_digest {
            return Err(MobStoreError::Internal(format!(
                "placed-spawn carrier spec digest mismatch for identity='{}' generation={} fence={}",
                self.agent_identity, self.generation, self.fence_token
            )));
        }
        if self.spec.profile.runtime_mode != self.spec.overlay.runtime_mode {
            return Err(MobStoreError::Internal(format!(
                "placed-spawn carrier has ambiguous runtime mode for identity='{}'",
                self.agent_identity
            )));
        }
        let autonomous = matches!(
            self.spec.profile.runtime_mode,
            meerkat_contracts::wire::WireMobRuntimeMode::AutonomousHost
        );
        if self.kickoff_intent.is_some() != autonomous {
            return Err(MobStoreError::Internal(format!(
                "placed-spawn carrier kickoff intent presence does not match runtime mode for identity='{}'",
                self.agent_identity
            )));
        }
        if let Some(intent) = &self.kickoff_intent {
            intent.validate()?;
        }
        let rehydrated = crate::portable_profile::rehydrate_portable_profile(&self.spec.profile)
            .map_err(|error| {
                MobStoreError::Internal(format!(
                    "placed-spawn portable profile rehydration failed: {error}"
                ))
            })?;
        let projected = crate::portable_profile::project_portable_profile(
            &rehydrated,
            rehydrated.runtime_mode,
            &self.spec.definition_extract.models,
            &self.agent_identity,
            &self.spec.profile_name,
            Vec::new(),
        )
        .map_err(|error| {
            MobStoreError::Internal(format!(
                "placed-spawn portable profile round-trip projection failed: {error}"
            ))
        })?;
        if projected != self.spec.profile {
            return Err(MobStoreError::Internal(format!(
                "placed-spawn portable profile does not round-trip for identity='{}'",
                self.agent_identity
            )));
        }
        if let PlacedSpawnCarrierPhase::Committed(committed) = &self.phase {
            committed.validate(&self.spec)?;
        }
        Ok(())
    }

    /// Validate the carrier against the table partition it was loaded from or
    /// is about to mutate. This prevents a self-consistent row for mob A from
    /// being written under mob B's key.
    pub fn validate_for_mob(&self, mob_id: &MobId) -> Result<(), MobStoreError> {
        self.validate()?;
        if *mob_id != self.spec.mob_id {
            return Err(MobStoreError::Internal(format!(
                "placed-spawn carrier mob binding mismatch: row={} key={}",
                self.spec.mob_id, mob_id
            )));
        }
        Ok(())
    }

    /// Validate both table partition and identity key against the embedded
    /// canonical record. Backends call this immediately after every decode.
    pub fn validate_for_store_key(
        &self,
        mob_id: &MobId,
        agent_identity: &str,
    ) -> Result<(), MobStoreError> {
        self.validate_for_mob(mob_id)?;
        if self.agent_identity != agent_identity {
            return Err(MobStoreError::Internal(format!(
                "placed-spawn carrier identity binding mismatch: row={} key={agent_identity}",
                self.agent_identity
            )));
        }
        Ok(())
    }

    pub fn expected_phase(&self) -> mob_dsl::PlacedSpawnCarrierExpectedPhase {
        match self.phase {
            PlacedSpawnCarrierPhase::Pending => mob_dsl::PlacedSpawnCarrierExpectedPhase::Pending,
            PlacedSpawnCarrierPhase::Committed(_) => {
                mob_dsl::PlacedSpawnCarrierExpectedPhase::Committed
            }
        }
    }

    pub fn same_attempt_as(&self, other: &Self) -> bool {
        self.schema_version == other.schema_version
            && self.spawn_id == other.spawn_id
            && self.agent_identity == other.agent_identity
            && self.generation == other.generation
            && self.fence_token == other.fence_token
            && self.provision_operation_id == other.provision_operation_id
            && self.operation_owner_session_id == other.operation_owner_session_id
            && self.host_id == other.host_id
            && self.host_binding_generation == other.host_binding_generation
            && self.spec_digest == other.spec_digest
            && self.spec == other.spec
            && self.kickoff_intent == other.kickoff_intent
            && self.effective_profile_override_present == other.effective_profile_override_present
            && self.effective_model_override_present == other.effective_model_override_present
    }

    /// Rebuild the presence-sensitive event/roster override without reading a
    /// current mob definition or model catalog. The portable profile carries
    /// an explicit provider and all allowed resolved profile facts.
    pub fn rehydrated_effective_profile_override(
        &self,
    ) -> Result<Option<crate::profile::Profile>, MobStoreError> {
        if !self.effective_profile_override_present {
            return Ok(None);
        }
        crate::portable_profile::rehydrate_portable_profile(&self.spec.profile)
            .map(Some)
            .map_err(|error| {
                MobStoreError::Internal(format!(
                    "placed-spawn effective profile override rehydration failed: {error}"
                ))
            })
    }

    /// Rebuild the field-scoped model override from the digest-covered
    /// portable profile without consulting the current mob definition.
    #[must_use]
    pub fn rehydrated_effective_model_override(&self) -> Option<String> {
        self.effective_model_override_present
            .then(|| self.spec.profile.model.clone())
    }

    /// Project per-member operator facts only from an exact committed carrier.
    /// Mob-wide operator grants remain in their independent principal-keyed
    /// store and are never folded into this record.
    pub fn committed_operator_authority(&self) -> Option<MobMemberOperatorAuthorityRecord> {
        if !matches!(self.phase, PlacedSpawnCarrierPhase::Committed(_)) {
            return None;
        }
        let context = self.spec.overlay.mob_tool_authority_context.as_ref()?;
        Some(MobMemberOperatorAuthorityRecord {
            agent_identity: self.agent_identity.clone(),
            generation: self.generation,
            can_create_mobs: context.can_create_mobs,
            can_mutate_profiles: context.can_mutate_profiles,
            can_run_adaptive_packs: context.can_run_adaptive_packs,
            managed_mob_scope: context.managed_mob_scope.clone(),
            spawn_profile_scope: context.spawn_profile_scope.clone(),
        })
    }
}

/// Phase of the canonical placed-spawn carrier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "phase",
    content = "commit",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PlacedSpawnCarrierPhase {
    Pending,
    Committed(PlacedSpawnCommitRecord),
}

/// Exact authenticated host ACK facts for a committed placed member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacedSpawnCommitRecord {
    pub member_session_id: meerkat_core::SessionId,
    pub member_peer_endpoint: mob_dsl::MemberPeerEndpoint,
    pub ack_engine_version: String,
}

impl PlacedSpawnCommitRecord {
    fn validate(
        &self,
        spec: &meerkat_contracts::wire::PortableMemberSpec,
    ) -> Result<(), MobStoreError> {
        if self.member_session_id.0.is_nil() || self.ack_engine_version.is_empty() {
            return Err(MobStoreError::Internal(
                "committed placed-spawn carrier has incomplete host ACK facts".to_string(),
            ));
        }

        let endpoint = &self.member_peer_endpoint;
        let expected_name = meerkat_core::MemberCommsName::new(
            spec.mob_id.clone(),
            spec.profile_name.clone(),
            spec.agent_identity.clone(),
        )
        .map_err(|error| {
            MobStoreError::Internal(format!(
                "committed placed-spawn carrier has invalid member comms identity: {error}"
            ))
        })?
        .to_string();
        if endpoint.name.0 != expected_name {
            return Err(MobStoreError::Internal(format!(
                "committed placed-spawn carrier endpoint name '{}' does not match canonical '{}'",
                endpoint.name.0, expected_name
            )));
        }
        let canonical = meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey(
            endpoint.name.0.clone(),
            endpoint.peer_id.0.as_str(),
            endpoint.signing_key.0,
            endpoint.address.0.as_str(),
        )
        .map_err(|error| {
            MobStoreError::Internal(format!(
                "committed placed-spawn carrier has invalid member peer endpoint: {error}"
            ))
        })?;
        if canonical.name.as_str() != endpoint.name.0
            || canonical.peer_id.to_string() != endpoint.peer_id.0
            || canonical.address.to_string() != endpoint.address.0
            || canonical.pubkey != endpoint.signing_key.0
        {
            return Err(MobStoreError::Internal(
                "committed placed-spawn carrier member peer endpoint is not canonical".to_string(),
            ));
        }
        Ok(())
    }
}

/// Result of an atomic begin-if-absent operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeginPlacedSpawnResult {
    Inserted,
    ExistingExactPending,
    ExistingExactCommitted,
    Conflict,
}

/// Result of an exact pending-to-committed CAS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitPlacedSpawnResult {
    Committed,
    AlreadyCommittedExact,
    StillPending,
    Conflict,
}

/// Result of an exact committed carrier host-binding-generation CAS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromotePlacedSpawnBindingResult {
    Promoted,
    AlreadyPromotedExact,
    Conflict,
}

/// Result of an exact witness-backed carrier delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletePlacedSpawnResult {
    Deleted,
    AlreadyAbsent,
    Conflict,
}

/// Generated write permit for a pending placed-spawn carrier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobPlacedSpawnPendingPersistenceAuthority {
    spawn_id: mob_dsl::PlacedSpawnId,
    agent_identity: mob_dsl::AgentIdentity,
    generation: u64,
    fence_token: u64,
    spec_digest: String,
    host: mob_dsl::HostId,
    host_binding_generation: u64,
    effective_profile_override_present: bool,
    effective_model_override_present: bool,
    provision_operation_id: String,
    operation_owner_session_id: mob_dsl::SessionId,
}

impl MobPlacedSpawnPendingPersistenceAuthority {
    pub fn from_transition(
        record: &MobPlacedSpawnCarrierRecord,
        transition: &mob_dsl::MobMachineTransition,
    ) -> Result<Self, MobStoreError> {
        let Some((
            spawn_id,
            agent_identity,
            generation,
            fence_token,
            spec_digest,
            host,
            host_binding_generation,
            effective_profile_override_present,
            effective_model_override_present,
            provision_operation_id,
            operation_owner_session_id,
        )) = transition.effects().iter().find_map(|effect| match effect {
            mob_dsl::MobMachineEffect::PersistPendingPlacedSpawn {
                spawn_id,
                agent_identity,
                generation,
                fence_token,
                spec_digest,
                host,
                host_binding_generation,
                effective_profile_override_present,
                effective_model_override_present,
                provision_operation_id,
                operation_owner_session_id,
            } => Some((
                spawn_id.clone(),
                agent_identity.clone(),
                generation.0,
                fence_token.0,
                spec_digest.clone(),
                host.clone(),
                *host_binding_generation,
                *effective_profile_override_present,
                *effective_model_override_present,
                provision_operation_id.clone(),
                operation_owner_session_id.clone(),
            )),
            _ => None,
        })
        else {
            return Err(MobStoreError::Internal(
                "generated placed member spec persistence witness effect is absent".to_string(),
            ));
        };
        let authority = Self {
            spawn_id,
            agent_identity,
            generation,
            fence_token,
            spec_digest,
            host,
            host_binding_generation,
            effective_profile_override_present,
            effective_model_override_present,
            provision_operation_id,
            operation_owner_session_id,
        };
        authority.verify_record(record)?;
        Ok(authority)
    }

    pub fn verify_record(&self, record: &MobPlacedSpawnCarrierRecord) -> Result<(), MobStoreError> {
        record.validate()?;
        if record.spawn_id.to_string() == self.spawn_id.0
            && record.agent_identity == self.agent_identity.0
            && record.generation == self.generation
            && record.fence_token == self.fence_token
            && record.spec_digest == self.spec_digest
            && record.host_id.to_string() == self.host.0
            && record.host_binding_generation == self.host_binding_generation
            && record.effective_profile_override_present == self.effective_profile_override_present
            && record.effective_model_override_present == self.effective_model_override_present
            && record.provision_operation_id.to_string() == self.provision_operation_id
            && record.operation_owner_session_id.to_string() == self.operation_owner_session_id.0
            && matches!(record.phase, PlacedSpawnCarrierPhase::Pending)
        {
            return Ok(());
        }
        Err(MobStoreError::Internal(format!(
            "generated pending placed-spawn witness does not match identity={} generation={} fence={}",
            record.agent_identity, record.generation, record.fence_token
        )))
    }
}

/// Generated CAS permit for the exact pending-to-committed transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobPlacedSpawnCommitPersistenceAuthority {
    spawn_id: mob_dsl::PlacedSpawnId,
    agent_identity: mob_dsl::AgentIdentity,
    generation: u64,
    fence_token: u64,
    host: mob_dsl::HostId,
    host_binding_generation: u64,
    spec_digest: String,
    session_id: mob_dsl::SessionId,
    peer_endpoint: mob_dsl::MemberPeerEndpoint,
    ack_engine_version: String,
    provision_operation_id: String,
    operation_owner_session_id: mob_dsl::SessionId,
}

impl MobPlacedSpawnCommitPersistenceAuthority {
    pub fn from_transition(
        record: &MobPlacedSpawnCarrierRecord,
        transition: &mob_dsl::MobMachineTransition,
    ) -> Result<Self, MobStoreError> {
        let Some((
            spawn_id,
            agent_identity,
            generation,
            fence_token,
            host,
            host_binding_generation,
            spec_digest,
            session_id,
            peer_endpoint,
            ack_engine_version,
            provision_operation_id,
            operation_owner_session_id,
        )) = transition.effects().iter().find_map(|effect| match effect {
            mob_dsl::MobMachineEffect::CommitPlacedSpawnCarrier {
                spawn_id,
                agent_identity,
                generation,
                fence_token,
                host,
                host_binding_generation,
                spec_digest,
                member_session_id,
                member_peer_endpoint,
                ack_engine_version,
                provision_operation_id,
                operation_owner_session_id,
            } => Some((
                spawn_id.clone(),
                agent_identity.clone(),
                generation.0,
                fence_token.0,
                host.clone(),
                *host_binding_generation,
                spec_digest.clone(),
                member_session_id.clone(),
                member_peer_endpoint.clone(),
                ack_engine_version.clone(),
                provision_operation_id.clone(),
                operation_owner_session_id.clone(),
            )),
            _ => None,
        })
        else {
            return Err(MobStoreError::Internal(
                "generated placed-spawn commit persistence effect is absent".to_string(),
            ));
        };
        let authority = Self {
            spawn_id,
            agent_identity,
            generation,
            fence_token,
            host,
            host_binding_generation,
            spec_digest,
            session_id,
            peer_endpoint,
            ack_engine_version,
            provision_operation_id,
            operation_owner_session_id,
        };
        authority.verify_record(record)?;
        Ok(authority)
    }

    pub fn verify_record(&self, record: &MobPlacedSpawnCarrierRecord) -> Result<(), MobStoreError> {
        record.validate()?;
        let PlacedSpawnCarrierPhase::Committed(committed) = &record.phase else {
            return Err(MobStoreError::Internal(
                "placed-spawn commit witness requires a committed carrier".to_string(),
            ));
        };
        if self.spawn_id.0 == record.spawn_id.to_string()
            && self.agent_identity.0 == record.agent_identity
            && self.generation == record.generation
            && self.fence_token == record.fence_token
            && self.spec_digest == record.spec_digest
            && self.host.0 == record.host_id.to_string()
            && self.host_binding_generation == record.host_binding_generation
            && committed.member_session_id.to_string() == self.session_id.0
            && committed.member_peer_endpoint == self.peer_endpoint
            && committed.ack_engine_version == self.ack_engine_version
            && record.provision_operation_id.to_string() == self.provision_operation_id
            && record.operation_owner_session_id.to_string() == self.operation_owner_session_id.0
        {
            return Ok(());
        }
        Err(MobStoreError::Internal(format!(
            "generated committed placed-spawn witness does not match identity={} generation={} fence={}",
            record.agent_identity, record.generation, record.fence_token
        )))
    }
}

/// Generated CAS permit for advancing an exact committed placed-spawn
/// carrier from one authenticated host-binding generation to its replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobPlacedSpawnBindingPromotionAuthority {
    spawn_id: mob_dsl::PlacedSpawnId,
    agent_identity: mob_dsl::AgentIdentity,
    generation: u64,
    fence_token: u64,
    host: mob_dsl::HostId,
    expected_host_binding_generation: u64,
    host_binding_generation: u64,
}

impl MobPlacedSpawnBindingPromotionAuthority {
    pub fn from_transition(
        expected: &MobPlacedSpawnCarrierRecord,
        promoted: &MobPlacedSpawnCarrierRecord,
        transition: &mob_dsl::MobMachineTransition,
    ) -> Result<Self, MobStoreError> {
        let Some((
            spawn_id,
            agent_identity,
            generation,
            fence_token,
            host,
            expected_host_binding_generation,
            host_binding_generation,
        )) = transition.effects().iter().find_map(|effect| match effect {
            mob_dsl::MobMachineEffect::PromoteCommittedPlacedSpawnCarrierBinding {
                spawn_id,
                agent_identity,
                generation,
                fence_token,
                host,
                expected_host_binding_generation,
                host_binding_generation,
            } => Some((
                spawn_id.clone(),
                agent_identity.clone(),
                generation.0,
                fence_token.0,
                host.clone(),
                *expected_host_binding_generation,
                *host_binding_generation,
            )),
            _ => None,
        })
        else {
            return Err(MobStoreError::Internal(
                "generated placed-spawn carrier binding promotion effect is absent".to_string(),
            ));
        };
        let authority = Self {
            spawn_id,
            agent_identity,
            generation,
            fence_token,
            host,
            expected_host_binding_generation,
            host_binding_generation,
        };
        authority.verify_records(expected, promoted)?;
        Ok(authority)
    }

    pub fn verify_records(
        &self,
        expected: &MobPlacedSpawnCarrierRecord,
        promoted: &MobPlacedSpawnCarrierRecord,
    ) -> Result<(), MobStoreError> {
        expected.validate()?;
        promoted.validate()?;
        if !matches!(expected.phase, PlacedSpawnCarrierPhase::Committed(_))
            || !matches!(promoted.phase, PlacedSpawnCarrierPhase::Committed(_))
        {
            return Err(MobStoreError::Internal(
                "placed-spawn carrier binding promotion requires committed carriers".to_string(),
            ));
        }
        let mut expected_promoted = expected.clone();
        expected_promoted.host_binding_generation = self.host_binding_generation;
        if self.expected_host_binding_generation > 0
            && self.host_binding_generation > self.expected_host_binding_generation
            && expected.host_binding_generation == self.expected_host_binding_generation
            && promoted == &expected_promoted
            && self.spawn_id.0 == expected.spawn_id.to_string()
            && self.agent_identity.0 == expected.agent_identity
            && self.generation == expected.generation
            && self.fence_token == expected.fence_token
            && self.host.0 == expected.host_id.to_string()
        {
            return Ok(());
        }
        Err(MobStoreError::Internal(format!(
            "generated placed-spawn carrier binding promotion witness does not match identity={} generation={} fence={} binding_generation={}->{}",
            expected.agent_identity,
            expected.generation,
            expected.fence_token,
            expected.host_binding_generation,
            promoted.host_binding_generation
        )))
    }
}

/// Generated exact-delete permit. The expected phase is part of the
/// obligation, so cleanup for an aborted pending attempt cannot delete a
/// committed carrier, and cleanup for generation N cannot target N+1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobPlacedSpawnCleanupAuthority {
    obligation: mob_dsl::PlacedCarrierCleanupObligation,
}

impl MobPlacedSpawnCleanupAuthority {
    pub fn from_transition(
        record: &MobPlacedSpawnCarrierRecord,
        transition: &mob_dsl::MobMachineTransition,
    ) -> Result<Self, MobStoreError> {
        let Some(obligation) = transition.effects().iter().find_map(|effect| match effect {
            mob_dsl::MobMachineEffect::PlacedCarrierCleanupAuthorized { obligation } => {
                Some(obligation.clone())
            }
            _ => None,
        }) else {
            return Err(MobStoreError::Internal(
                "generated placed-carrier cleanup authority effect is absent".to_string(),
            ));
        };
        let authority = Self { obligation };
        authority.verify_record(record)?;
        Ok(authority)
    }

    pub fn verify_record(&self, record: &MobPlacedSpawnCarrierRecord) -> Result<(), MobStoreError> {
        record.validate()?;
        if self.obligation.agent_identity.0 == record.agent_identity
            && self.obligation.spawn_id.0 == record.spawn_id.to_string()
            && self.obligation.generation.0 == record.generation
            && self.obligation.fence_token.0 == record.fence_token
            && self.obligation.provision_operation_id == record.provision_operation_id.to_string()
            && self.obligation.operation_owner_session_id.0
                == record.operation_owner_session_id.to_string()
            && self.obligation.expected_phase == record.expected_phase()
        {
            return Ok(());
        }
        Err(MobStoreError::Internal(format!(
            "generated placed-carrier cleanup authority does not match identity={} generation={} fence={} phase={:?}",
            record.agent_identity,
            record.generation,
            record.fence_token,
            record.expected_phase()
        )))
    }
}

/// Durable projection of one principal's control-scope grant (§8).
///
/// Written ONLY under a MobMachine transition witness whose effects carry
/// the matching `GrantRecorded` (grant / full replace) or `GrantRevoked`
/// with a nonempty `remaining` (partial revoke rewrite); deleted only under
/// a full-revoke `GrantRevoked` witness (or the destroy-path scrub). Read
/// ONLY by resume recovery, which replays it through the machine's own
/// `GrantOperatorScopes` input. Never an enforcement source: the sealed
/// policy resolves MACHINE state, and this record cannot mint scopes.
///
/// Deliberately a runtime-metadata RECORD rather than a `MobEventKind`
/// (ADJ-P5-2): the frozen `ResetToRunning` transition keeps grants across a
/// live reset, while event replay truncates at `MobReset` — an event
/// realization would provably diverge restored state from live state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobOperatorGrantRecord {
    /// Validated principal id string (the machine map key).
    pub principal: String,
    /// Recorded scopes, sorted and deduped. The wire enum is THE serialized
    /// scope spelling; the machine `ControlScope` deliberately has no serde.
    pub scopes: Vec<WireControlScope>,
    /// Raw recorded expiry, verbatim (`None` = never expires). Replay
    /// restores it unchanged; only the enforcement seam evaluates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
}

impl MobOperatorGrantRecord {
    pub fn dsl_principal(&self) -> mob_dsl::PrincipalId {
        mob_dsl::PrincipalId::from(self.principal.clone())
    }

    /// The recorded scopes as the machine scope set (the set the transition
    /// witness compares against).
    pub fn control_scope_set(&self) -> std::collections::BTreeSet<mob_dsl::ControlScope> {
        self.scopes
            .iter()
            .copied()
            .map(mob_dsl::ControlScope::from)
            .collect()
    }

    /// Resume-recovery input: the frozen catalog has no grant recovery
    /// SIGNAL, so recovery rides the live `GrantOperatorScopes` input (the
    /// placed-member posture — recovery rides the same admission ladder the
    /// live write walked).
    pub fn dsl_grant_input(&self) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::GrantOperatorScopes {
            principal: self.dsl_principal(),
            scopes: self.control_scope_set(),
            expires_at_ms: self.expires_at_ms,
        }
    }
}

/// Durable member-event pump cursor (§7.4 phase 6, DEC-P6E-10). A PLAIN
/// runtime-metadata record, deliberately WITHOUT a transition-witness
/// permit (ADJ-P6-6, FLAG-P6E-9): the cursor is mechanical consumption
/// progress, not machine truth — losing it costs only re-reading events
/// (dedup'd downstream by full residency plus durable seq), and no DSL fact mirrors
/// it. Witness discipline stays reserved for machine-fact records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobMemberEventCursorRecord {
    /// Member identity the pump follows (the record key with the mob id).
    pub agent_identity: String,
    /// Authoritative MobMachine placement owner. Empty is the legacy
    /// pre-host-domain shape and is never reused for a live placed pump.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host_id: String,
    /// Exact host-binding generation under which this cursor was observed.
    /// `None` is the legacy pre-binding-generation shape and is never reused
    /// for a placed-member pump.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_binding_generation: Option<u64>,
    /// Exact member session hosted by this residency. Empty is the legacy
    /// pre-session-addressed shape and is never reused for a placed pump.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub member_session_id: String,
    /// Member generation the cursor was recorded under. A stale value is
    /// self-healing: the serving host resolves `g < G` by serving the
    /// current generation from seq 1 (the page's generation is the reset
    /// signal).
    pub generation: u64,
    /// Materialization fence paired with `generation`. `None` is the legacy
    /// pre-fence record shape and is never reused for a live pump.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fence_token: Option<u64>,
    /// Next durable `StoredEvent.seq` to poll from (gotcha 8: the durable
    /// seq domain only).
    pub next_seq: u64,
}

/// Exact session/placement incarnation against which an orphaned successful
/// member-live Open must be reconciled. Routes are reconstructed from current
/// roster + machine facts; the durable record carries only canonical identity
/// and fencing atoms, never a duplicate endpoint/key cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MobMemberLiveCleanupTargetRecord {
    Local {
        session_id: String,
        generation: u64,
    },
    Placed {
        expected_member: BridgeMemberIncarnation,
    },
}

/// Durable custody for a successful member-live Open whose exact result was
/// not acknowledged by its caller. This is mechanical cleanup progress, not
/// a competing lifecycle machine: the owning session's MeerkatMachine remains
/// the live-channel truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobMemberLiveCleanupRecord {
    pub cleanup_id: String,
    pub agent_identity: AgentIdentity,
    pub target: MobMemberLiveCleanupTargetRecord,
    /// Exact returned channel identity. Ambiguous Open failures without this
    /// identity remain caller-driven and may never mint cleanup custody.
    pub channel_id: String,
    pub reason: String,
}

/// Typed write permit for [`MobOperatorGrantRecord`], constructible only
/// from a MobMachine transition whose effects carry the matching
/// `GrantRecorded` fact (grant path) or, via
/// [`MobOperatorGrantPersistenceAuthority::from_revoke_transition`], the
/// matching partial-revoke `GrantRevoked` fact. Exact sibling of
/// [`MobHostAuthorityPersistenceAuthority`] (ADJ-15 pattern).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobOperatorGrantPersistenceAuthority {
    principal: mob_dsl::PrincipalId,
    scopes: std::collections::BTreeSet<mob_dsl::ControlScope>,
    expires_at_ms: Option<u64>,
}

impl MobOperatorGrantPersistenceAuthority {
    /// Grant-path witness: the transition's `GrantRecorded` effect must
    /// match the record's `(principal, scopes, expires_at_ms)` exactly.
    pub fn from_transition(
        record: &MobOperatorGrantRecord,
        transition: &mob_dsl::MobMachineTransition,
    ) -> Result<Self, MobStoreError> {
        let Some((principal, scopes, expires_at_ms)) =
            transition.effects().iter().find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::GrantRecorded {
                    principal,
                    scopes,
                    expires_at_ms,
                } => Some((principal.clone(), scopes.clone(), *expires_at_ms)),
                _ => None,
            })
        else {
            return Err(MobStoreError::Internal(
                "generated operator grant persistence witness effect is absent".to_string(),
            ));
        };
        let authority = Self {
            principal,
            scopes,
            expires_at_ms,
        };
        authority.verify_record(record)?;
        Ok(authority)
    }

    /// Partial-revoke witness: the transition's `GrantRevoked` effect with a
    /// NONEMPTY `remaining` must match the record's `(principal, scopes)`.
    /// A partial revoke retains the recorded expiry, but the effect does not
    /// carry it — so the permit validates the record's `expires_at_ms`
    /// against the transition's resulting `operator_grant_expiries` row
    /// (`post_state` is the prepared authority's state after apply, before
    /// commit).
    pub fn from_revoke_transition(
        record: &MobOperatorGrantRecord,
        transition: &mob_dsl::MobMachineTransition,
        post_state: &mob_dsl::MobMachineState,
    ) -> Result<Self, MobStoreError> {
        let Some((principal, remaining)) =
            transition.effects().iter().find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::GrantRevoked {
                    principal,
                    remaining,
                    ..
                } if !remaining.is_empty() => Some((principal.clone(), remaining.clone())),
                _ => None,
            })
        else {
            return Err(MobStoreError::Internal(
                "generated operator grant partial-revoke persistence witness effect is absent"
                    .to_string(),
            ));
        };
        let Some(expires_at_ms) = post_state.operator_grant_expiries.get(&principal).copied()
        else {
            return Err(MobStoreError::Internal(format!(
                "generated operator grant partial-revoke witness has no post-state expiry row for principal '{}'",
                principal.0
            )));
        };
        let authority = Self {
            principal,
            scopes: remaining,
            expires_at_ms,
        };
        authority.verify_record(record)?;
        Ok(authority)
    }

    pub fn verify_record(&self, record: &MobOperatorGrantRecord) -> Result<(), MobStoreError> {
        if self.principal == record.dsl_principal()
            && self.scopes == record.control_scope_set()
            && self.expires_at_ms == record.expires_at_ms
        {
            return Ok(());
        }
        Err(MobStoreError::Internal(format!(
            "generated operator grant persistence witness does not match record principal={}",
            record.principal
        )))
    }
}

/// Typed deletion permit for [`MobOperatorGrantRecord`], constructible only
/// from a MobMachine transition whose effects carry the matching full-revoke
/// `GrantRevoked` fact (`remaining` empty) for the principal. The
/// absent-grant no-op arm emits nothing and therefore can never mint a
/// deletion permit; the destroy-path scrub uses
/// [`MobRuntimeMetadataStore::delete_mob_operator_grants`] instead (revoke
/// transitions guard `Running` and are unreachable during destroy).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobOperatorGrantDeletionAuthority {
    principal: mob_dsl::PrincipalId,
}

impl MobOperatorGrantDeletionAuthority {
    pub fn from_transition(
        principal: &str,
        transition: &mob_dsl::MobMachineTransition,
    ) -> Result<Self, MobStoreError> {
        let Some(effect_principal) = transition.effects().iter().find_map(|effect| match effect {
            mob_dsl::MobMachineEffect::GrantRevoked {
                principal,
                remaining,
                ..
            } if remaining.is_empty() => Some(principal.clone()),
            _ => None,
        }) else {
            return Err(MobStoreError::Internal(
                "generated operator grant deletion witness effect is absent".to_string(),
            ));
        };
        let authority = Self {
            principal: effect_principal,
        };
        authority.verify_principal(principal)?;
        Ok(authority)
    }

    pub fn verify_principal(&self, principal: &str) -> Result<(), MobStoreError> {
        if self.principal.0 == principal {
            return Ok(());
        }
        Err(MobStoreError::Internal(format!(
            "generated operator grant deletion witness does not match principal={principal}"
        )))
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn mob_operator_grant_persistence_authority_for_record(
    record: &MobOperatorGrantRecord,
) -> Result<MobOperatorGrantPersistenceAuthority, MobStoreError> {
    let mut authority = mob_dsl::MobMachineAuthority::new();
    let transition = mob_dsl::MobMachineMutator::apply(&mut authority, record.dsl_grant_input())
        .map_err(|error| {
            MobStoreError::Internal(format!(
                "generated operator grant test persistence authority rejected record: {error}"
            ))
        })?;
    MobOperatorGrantPersistenceAuthority::from_transition(record, &transition)
}

#[cfg(any(test, feature = "test-support"))]
pub fn mob_operator_grant_deletion_authority_for_record(
    record: &MobOperatorGrantRecord,
) -> Result<MobOperatorGrantDeletionAuthority, MobStoreError> {
    let mut authority = mob_dsl::MobMachineAuthority::new();
    mob_dsl::MobMachineMutator::apply(&mut authority, record.dsl_grant_input()).map_err(
        |error| {
            MobStoreError::Internal(format!(
                "generated operator grant test deletion grant rejected record: {error}"
            ))
        },
    )?;
    let transition = mob_dsl::MobMachineMutator::apply(
        &mut authority,
        mob_dsl::MobMachineInput::RevokeOperatorScopes {
            principal: record.dsl_principal(),
            revoked: record.control_scope_set(),
            remaining: std::collections::BTreeSet::new(),
        },
    )
    .map_err(|error| {
        MobStoreError::Internal(format!(
            "generated operator grant test deletion revoke rejected record: {error}"
        ))
    })?;
    MobOperatorGrantDeletionAuthority::from_transition(record.principal.as_str(), &transition)
}

#[cfg(any(test, feature = "test-support"))]
pub fn mob_host_authority_persistence_authority_for_record(
    record: &MobHostAuthorityRecord,
) -> Result<MobHostAuthorityPersistenceAuthority, MobStoreError> {
    let mut authority = mob_dsl::MobMachineAuthority::new();
    mob_dsl::MobMachineMutator::apply(
        &mut authority,
        mob_dsl::MobMachineInput::BeginHostBind {
            host_id: record.dsl_host_id(),
            expected_endpoint: record.dsl_endpoint(),
            binding_generation: record.binding_generation.max(1),
        },
    )
    .map_err(|error| {
        MobStoreError::Internal(format!(
            "generated host binding test persistence begin rejected record: {error}"
        ))
    })?;
    let transition = mob_dsl::MobMachineMutator::apply(
        &mut authority,
        mob_dsl::MobMachineInput::CommitHostBind {
            host_id: record.dsl_host_id(),
            pubkey: record.dsl_signing_key(),
            endpoint: record.dsl_endpoint(),
            epoch: record.authority_epoch,
            binding_generation: record.binding_generation.max(1),
            protocol_min: record.capabilities.protocol_min,
            protocol_max: record.capabilities.protocol_max,
            engine_version: record.capabilities.engine_version.clone(),
            durable_sessions: record.capabilities.durable_sessions,
            autonomous_members: record.capabilities.autonomous_members,
            hard_cancel_member: record.capabilities.hard_cancel_member,
            tracked_input_cancel: record.capabilities.tracked_input_cancel,
            memory_store: record.capabilities.memory_store,
            mcp: record.capabilities.mcp,
            resolvable_providers: record.capabilities.resolvable_providers.clone(),
            approval_forwarding: record.capabilities.approval_forwarding,
            live_endpoint: record
                .live_endpoint
                .clone()
                .map(mob_dsl::LiveWsEndpointUrl::from),
        },
    )
    .map_err(|error| {
        MobStoreError::Internal(format!(
            "generated host binding test persistence commit rejected record: {error}"
        ))
    })?;
    MobHostAuthorityPersistenceAuthority::from_transition(record, &transition)
}

#[cfg(any(test, feature = "test-support"))]
pub fn mob_host_authority_deletion_authority_for_record(
    record: &MobHostAuthorityRecord,
) -> Result<MobHostAuthorityDeletionAuthority, MobStoreError> {
    let mut authority = mob_dsl::MobMachineAuthority::new();
    authority
        .apply_signal(record.dsl_recover_signal())
        .map_err(|error| {
            MobStoreError::Internal(format!(
                "generated host binding test deletion recovery rejected record: {error}"
            ))
        })?;
    let transition = mob_dsl::MobMachineMutator::apply(
        &mut authority,
        mob_dsl::MobMachineInput::RevokeHost {
            host_id: record.dsl_host_id(),
            binding_generation: record.binding_generation,
        },
    )
    .map_err(|error| {
        MobStoreError::Internal(format!(
            "generated host binding test deletion revoke rejected record: {error}"
        ))
    })?;
    MobHostAuthorityDeletionAuthority::from_transition(record, &transition)
}

/// Deterministic bound-host record fixture shared by the store test suites.
#[cfg(any(test, feature = "test-support"))]
pub fn sample_mob_host_authority_record(host_id: &str, epoch: u64) -> MobHostAuthorityRecord {
    MobHostAuthorityRecord {
        host_id: host_id.to_string(),
        peer_id: host_id.to_string(),
        signing_key: [7u8; 32],
        endpoint: format!("tcp://hosts/{host_id}"),
        authority_epoch: epoch,
        binding_generation: 1,
        bind_phase: MobHostBindPhaseRecord::Bound,
        capabilities: MobHostCapabilityRecord {
            protocol_min: 4,
            protocol_max: 4,
            engine_version: "0.7.22-test".to_string(),
            durable_sessions: true,
            autonomous_members: true,
            hard_cancel_member: false,
            tracked_input_cancel: true,
            memory_store: true,
            mcp: true,
            resolvable_providers: std::collections::BTreeSet::from(["anthropic".to_string()]),
            approval_forwarding: false,
        },
        live_endpoint: Some(format!("wss://hosts/{host_id}/live")),
    }
}

/// Projection status for legacy external binding metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExternalBindingOverlayStatus {
    /// The legacy external binding was normalized to a peer-only member ref.
    Normalized,
    /// Normalization failed and the member should surface as broken.
    Failed { reason: String },
}

/// Compatibility projection metadata for a legacy external binding.
///
/// This record never creates roster membership and is not restart authority for
/// member material, bridge binding, lifecycle status, or restore failure state.
/// Resume rebuilds those facts from `MemberSpawned`/`MemberRetired` events and
/// MobMachine-owned state; overlays remain persisted only for compatibility
/// projection, diagnostics, and cleanup of older runtimes that wrote them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalBindingOverlayRecord {
    /// Stable member identity.
    pub agent_identity: AgentIdentity,
    /// Generation the overlay applies to.
    pub generation: Generation,
    /// Peer-only runtime binding when normalization succeeds.
    ///
    /// Crate-private alongside `MemberRef` itself (finding A7): the pre-0.6
    /// bridge identity is never surfaced to external callers. Serde still
    /// carries the value through the persisted overlay record.
    pub(crate) normalized_member_ref: Option<MemberRef>,
    /// Optional bootstrap proof for re-establishing supervisor control.
    pub bootstrap_token: Option<BridgeBootstrapToken>,
    /// Current normalization status.
    pub status: ExternalBindingOverlayStatus,
    /// Last update time for conflict resolution and diagnostics.
    pub updated_at: DateTime<Utc>,
}

/// Trait for persisting and querying mob events.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MobEventStore: private::MobEventStoreSealed + Send + Sync {
    /// Append a new event to the store.
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobStoreError>;

    /// Append a terminal flow event only when no terminal event for the same
    /// mob/run/flow has already been persisted.
    async fn append_terminal_event_if_absent(
        &self,
        event: NewMobEvent,
    ) -> Result<Option<MobEvent>, MobStoreError>;

    /// Append one exact aggregate step-failure projection if its
    /// `(mob, run, step)` key is absent. An exact replay returns `None`; a
    /// different reason under the same key fails closed.
    ///
    /// Durable implementations override this with an atomic check-and-append.
    /// The default keeps crate-local test doubles source-compatible.
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
        let mut exact_replay = false;
        for existing in self.replay_all().await? {
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
        self.append(event).await.map(Some)
    }

    /// Append multiple events atomically.
    ///
    /// Implementations must ensure all-or-nothing semantics: either every
    /// event is persisted or none are. No default implementation is provided
    /// to force implementors to consider atomicity.
    async fn append_batch(&self, events: Vec<NewMobEvent>) -> Result<Vec<MobEvent>, MobStoreError>;

    /// Poll for events after a given cursor, up to a limit.
    async fn poll(&self, after_cursor: u64, limit: usize) -> Result<Vec<MobEvent>, MobStoreError>;

    /// Replay all events from the beginning.
    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobStoreError>;

    /// Return the latest persisted event cursor, or 0 when no events exist.
    async fn latest_cursor(&self) -> Result<u64, MobStoreError>;

    /// Subscribe to structural events appended through this store after this call.
    fn subscribe(&self) -> Result<MobEventReceiver, MobStoreError> {
        Err(MobStoreError::Internal(
            "mob event store does not support native event subscriptions".to_string(),
        ))
    }

    /// Delete all persisted events.
    async fn clear(&self) -> Result<(), MobStoreError>;

    /// Prune events older than a timestamp. Returns count of deleted events.
    async fn prune(&self, _older_than: DateTime<Utc>) -> Result<u64, MobStoreError> {
        Ok(0)
    }
}

fn runtime_metadata_capability_unavailable<T>(operation: &str) -> Result<T, MobStoreError> {
    Err(MobStoreError::Internal(format!(
        "MobRuntimeMetadataStore does not implement {operation}"
    )))
}

/// Trait for persisting authoritative runtime-side mob metadata.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MobRuntimeMetadataStore: Send + Sync {
    /// Load the mob-owned supervisor authority record.
    async fn load_supervisor_authority(
        &self,
        mob_id: &MobId,
    ) -> Result<Option<SupervisorAuthorityRecord>, MobStoreError>;

    /// Upsert the mob-owned supervisor authority record.
    async fn put_supervisor_authority(
        &self,
        mob_id: &MobId,
        record: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityPersistenceAuthority,
    ) -> Result<(), MobStoreError>;

    /// Compare-and-put the mob-owned supervisor authority record.
    ///
    /// Returns `true` when the persisted record exactly matched `expected`
    /// and was replaced by `record`, `false` when another writer changed or
    /// removed the record first.
    async fn compare_and_put_supervisor_authority(
        &self,
        mob_id: &MobId,
        expected: &SupervisorAuthorityRecord,
        record: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError>;

    /// Insert the mob-owned supervisor authority record if it is missing.
    ///
    /// Returns `true` when the caller won initialization, `false` when an
    /// existing record already owned the key.
    async fn put_supervisor_authority_if_absent(
        &self,
        mob_id: &MobId,
        record: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError>;

    /// Delete the mob-owned supervisor authority record.
    async fn delete_supervisor_authority(
        &self,
        mob_id: &MobId,
        expected: &SupervisorAuthorityRecord,
        authority: &SupervisorAuthorityDeletionAuthority,
    ) -> Result<bool, MobStoreError>;

    /// Load one mob-owned bound-host authority record.
    async fn load_mob_host_authority(
        &self,
        mob_id: &MobId,
        host_id: &str,
    ) -> Result<Option<MobHostAuthorityRecord>, MobStoreError> {
        let _ = (mob_id, host_id);
        runtime_metadata_capability_unavailable("load_mob_host_authority")
    }

    /// List every bound-host authority record for a mob (builder recovery),
    /// in deterministic host-id order.
    async fn list_mob_host_authorities(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobHostAuthorityRecord>, MobStoreError> {
        let _ = mob_id;
        runtime_metadata_capability_unavailable("list_mob_host_authorities")
    }

    /// Upsert one mob-owned bound-host authority record, keyed by
    /// `(mob_id, record.host_id)`.
    async fn put_mob_host_authority(
        &self,
        mob_id: &MobId,
        record: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityPersistenceAuthority,
    ) -> Result<(), MobStoreError> {
        let _ = (mob_id, record, authority);
        runtime_metadata_capability_unavailable("put_mob_host_authority")
    }

    /// Insert the bound-host record if the `(mob, host)` key is missing.
    ///
    /// Returns `true` when the caller won initialization, `false` when an
    /// existing record already owned the key.
    async fn put_mob_host_authority_if_absent(
        &self,
        mob_id: &MobId,
        record: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        let _ = (mob_id, record, authority);
        runtime_metadata_capability_unavailable("put_mob_host_authority_if_absent")
    }

    /// Compare-and-put the bound-host record.
    ///
    /// Returns `true` when the persisted record exactly matched `expected`
    /// and was replaced by `record`, `false` when another writer changed or
    /// removed the record first.
    async fn compare_and_put_mob_host_authority(
        &self,
        mob_id: &MobId,
        expected: &MobHostAuthorityRecord,
        record: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityPersistenceAuthority,
    ) -> Result<bool, MobStoreError> {
        let _ = (mob_id, expected, record, authority);
        runtime_metadata_capability_unavailable("compare_and_put_mob_host_authority")
    }

    /// Delete the bound-host record (host revoke).
    async fn delete_mob_host_authority(
        &self,
        mob_id: &MobId,
        expected: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityDeletionAuthority,
    ) -> Result<bool, MobStoreError> {
        let _ = (mob_id, expected, authority);
        runtime_metadata_capability_unavailable("delete_mob_host_authority")
    }

    /// Persist the non-prunable generation tombstone for a host revoke.
    /// Implementations monotonically retain `expected.binding_generation`;
    /// the transition-derived deletion witness proves the exact host tuple.
    async fn put_mob_host_binding_generation_highwater(
        &self,
        mob_id: &MobId,
        expected: &MobHostAuthorityRecord,
        authority: &MobHostAuthorityDeletionAuthority,
    ) -> Result<(), MobStoreError> {
        let _ = (mob_id, expected, authority);
        runtime_metadata_capability_unavailable("put_mob_host_binding_generation_highwater")
    }

    /// List non-prunable host binding-generation tombstones in deterministic
    /// host-id order for builder recovery.
    async fn list_mob_host_binding_generation_highwaters(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<(String, u64)>, MobStoreError> {
        let _ = mob_id;
        runtime_metadata_capability_unavailable("list_mob_host_binding_generation_highwaters")
    }

    /// Atomically insert a durable `Pending` member-operator request if its
    /// `(mob, identity, generation, fence_token, host_id,
    /// host_binding_generation, member_session_id, request_id)` key is absent,
    /// otherwise return the existing record. An existing record never grants
    /// execution.
    async fn begin_member_operator_request(
        &self,
        mob_id: &MobId,
        record: &MobMemberOperatorRequestRecord,
    ) -> Result<MobMemberOperatorRequestBegin, MobStoreError> {
        let _ = (mob_id, record);
        runtime_metadata_capability_unavailable("begin_member_operator_request")
    }

    /// Load one durable member-operator request ledger row.
    async fn load_member_operator_request(
        &self,
        mob_id: &MobId,
        key: &MobMemberOperatorRequestKey,
    ) -> Result<Option<MobMemberOperatorRequestRecord>, MobStoreError> {
        let _ = (mob_id, key);
        runtime_metadata_capability_unavailable("load_member_operator_request")
    }

    /// Compare-and-put the only legal phase transition: the exact persisted
    /// `Pending` record to a terminal record with the same key and op digest.
    /// Terminal replies are immutable.
    async fn compare_and_put_member_operator_request(
        &self,
        mob_id: &MobId,
        expected: &MobMemberOperatorRequestRecord,
        record: &MobMemberOperatorRequestRecord,
    ) -> Result<bool, MobStoreError> {
        let _ = (mob_id, expected, record);
        runtime_metadata_capability_unavailable("compare_and_put_member_operator_request")
    }

    /// List every request ledger row for a mob in deterministic
    /// `(identity, generation, fence_token, host, host generation, session,
    /// request_id)` order. This is an
    /// inspection and destroy-rollback surface; rows for a machine-current
    /// residency are never capacity-evicted.
    async fn list_member_operator_requests(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobMemberOperatorRequestRecord>, MobStoreError> {
        let _ = mob_id;
        runtime_metadata_capability_unavailable("list_member_operator_requests")
    }

    /// Delete only rows whose full requester residency is absent from the
    /// actor-minted current set. This is the sole normal-capacity reclamation
    /// path: a current Pending or terminal row is never evicted, while rows
    /// from superseded host/member incarnations cannot replay because machine
    /// admission precedes every ledger lookup.
    async fn prune_stale_member_operator_requests(
        &self,
        mob_id: &MobId,
        authority: &MobMemberOperatorPruneAuthority,
    ) -> Result<u64, MobStoreError> {
        let _ = (mob_id, authority);
        runtime_metadata_capability_unavailable("prune_stale_member_operator_requests")
    }

    /// Delete all request ledger rows at mob destroy. Normal reclamation uses
    /// [`Self::prune_stale_member_operator_requests`] under actor authority.
    async fn delete_member_operator_requests(&self, mob_id: &MobId) -> Result<u64, MobStoreError> {
        let _ = mob_id;
        runtime_metadata_capability_unavailable("delete_member_operator_requests")
    }

    /// Load the canonical carrier for one identity. Unknown carrier schema
    /// versions fail closed in the backend before this value is returned.
    async fn load_placed_spawn(
        &self,
        mob_id: &MobId,
        agent_identity: &str,
    ) -> Result<Option<MobPlacedSpawnCarrierRecord>, MobStoreError> {
        let _ = (mob_id, agent_identity);
        runtime_metadata_capability_unavailable("load_placed_spawn")
    }

    /// Deterministically list every placed-spawn carrier for recovery.
    async fn list_placed_spawns(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobPlacedSpawnCarrierRecord>, MobStoreError> {
        let _ = mob_id;
        runtime_metadata_capability_unavailable("list_placed_spawns")
    }

    /// Insert an exact pending attempt without overwriting an existing
    /// identity row. Existing exact state is classified for lost-ACK replay;
    /// any different tuple is an ABA conflict.
    async fn begin_placed_spawn_if_absent(
        &self,
        mob_id: &MobId,
        record: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnPendingPersistenceAuthority,
    ) -> Result<BeginPlacedSpawnResult, MobStoreError> {
        let _ = (mob_id, record, authority);
        runtime_metadata_capability_unavailable("begin_placed_spawn_if_absent")
    }

    /// Exact pending-to-committed CAS. Every CAS-significant tuple field is
    /// compared, including spawn id and phase.
    async fn compare_and_commit_placed_spawn(
        &self,
        mob_id: &MobId,
        expected_pending: &MobPlacedSpawnCarrierRecord,
        committed: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnCommitPersistenceAuthority,
    ) -> Result<CommitPlacedSpawnResult, MobStoreError> {
        let _ = (mob_id, expected_pending, committed, authority);
        runtime_metadata_capability_unavailable("compare_and_commit_placed_spawn")
    }

    /// Exact committed-carrier host-binding-generation CAS. The durable row
    /// changes only the binding generation; every logical carrier and ACK fact
    /// remains byte-for-byte identical.
    async fn compare_and_promote_placed_spawn_binding(
        &self,
        mob_id: &MobId,
        expected: &MobPlacedSpawnCarrierRecord,
        promoted: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnBindingPromotionAuthority,
    ) -> Result<PromotePlacedSpawnBindingResult, MobStoreError> {
        let _ = (mob_id, expected, promoted, authority);
        runtime_metadata_capability_unavailable("compare_and_promote_placed_spawn_binding")
    }

    /// Exact witness-backed carrier deletion.
    async fn compare_and_delete_placed_spawn(
        &self,
        mob_id: &MobId,
        expected: &MobPlacedSpawnCarrierRecord,
        authority: &MobPlacedSpawnCleanupAuthority,
    ) -> Result<DeletePlacedSpawnResult, MobStoreError> {
        let _ = (mob_id, expected, authority);
        runtime_metadata_capability_unavailable("compare_and_delete_placed_spawn")
    }

    /// Read per-member operator facts from the committed canonical carrier.
    /// Pending carriers never grant.
    async fn load_committed_placed_member_operator_authority(
        &self,
        mob_id: &MobId,
        agent_identity: &str,
        generation: u64,
    ) -> Result<Option<MobMemberOperatorAuthorityRecord>, MobStoreError> {
        Ok(self
            .load_placed_spawn(mob_id, agent_identity)
            .await?
            .filter(|record| record.generation == generation)
            .and_then(|record| record.committed_operator_authority()))
    }

    /// List every principal control-scope grant record for a mob (builder
    /// recovery), in deterministic principal order.
    async fn list_mob_operator_grants(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobOperatorGrantRecord>, MobStoreError> {
        let _ = mob_id;
        runtime_metadata_capability_unavailable("list_mob_operator_grants")
    }

    /// Upsert one grant record, keyed by `(mob_id, record.principal)` —
    /// a grant is a full replace of the principal's scope set and expiry.
    async fn put_mob_operator_grant(
        &self,
        mob_id: &MobId,
        record: &MobOperatorGrantRecord,
        authority: &MobOperatorGrantPersistenceAuthority,
    ) -> Result<(), MobStoreError> {
        let _ = (mob_id, record, authority);
        runtime_metadata_capability_unavailable("put_mob_operator_grant")
    }

    /// Delete one grant record (full revoke). Returns whether a record was
    /// removed.
    async fn delete_mob_operator_grant(
        &self,
        mob_id: &MobId,
        principal: &str,
        authority: &MobOperatorGrantDeletionAuthority,
    ) -> Result<bool, MobStoreError> {
        let _ = (mob_id, principal, authority);
        runtime_metadata_capability_unavailable("delete_mob_operator_grant")
    }

    /// Delete every grant record for a mob (destroy-path metadata scrub,
    /// sibling of [`Self::delete_external_binding_overlays`]). No per-row
    /// revoke witness exists at destroy — revoke transitions guard the
    /// `Running` phase — so the scrub sweeps the whole family under the
    /// `MobDestroyStorageFinalizing` fence. Returns the removed row count.
    async fn delete_mob_operator_grants(&self, mob_id: &MobId) -> Result<u64, MobStoreError> {
        let _ = mob_id;
        runtime_metadata_capability_unavailable("delete_mob_operator_grants")
    }

    /// List every member-event pump cursor record for a mob (pump resume,
    /// DEC-P6E-10). Defaults model a store without cursor durability —
    /// honest for these PLAIN records (losing a cursor only re-reads
    /// events; the pump dedups by `(generation, seq)`).
    async fn list_member_event_cursors(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobMemberEventCursorRecord>, MobStoreError> {
        let _ = mob_id;
        Ok(Vec::new())
    }

    /// Upsert one pump cursor record, keyed by `(mob_id, agent_identity)`.
    async fn put_member_event_cursor(
        &self,
        mob_id: &MobId,
        record: &MobMemberEventCursorRecord,
    ) -> Result<(), MobStoreError> {
        let _ = (mob_id, record);
        Ok(())
    }

    /// Delete one member's pump cursor (release / retire cleanup).
    async fn delete_member_event_cursor(
        &self,
        mob_id: &MobId,
        agent_identity: &str,
    ) -> Result<bool, MobStoreError> {
        let _ = (mob_id, agent_identity);
        Ok(false)
    }

    /// Delete every pump cursor for a mob (destroy-path metadata scrub).
    async fn delete_member_event_cursors(&self, mob_id: &MobId) -> Result<u64, MobStoreError> {
        let _ = mob_id;
        Ok(0)
    }

    /// Deterministically list every retained member-live cleanup obligation.
    async fn list_member_live_cleanup_records(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<MobMemberLiveCleanupRecord>, MobStoreError> {
        let _ = mob_id;
        // A legacy store cannot contain rows it never supported writing.
        Ok(Vec::new())
    }

    /// Insert one immutable cleanup record. Returns `false` only for an exact
    /// replay; conflicting reuse of a cleanup id fails closed.
    async fn put_member_live_cleanup_record_if_absent(
        &self,
        mob_id: &MobId,
        record: &MobMemberLiveCleanupRecord,
    ) -> Result<bool, MobStoreError> {
        let _ = (mob_id, record);
        runtime_metadata_capability_unavailable("put_member_live_cleanup_record_if_absent")
    }

    /// Delete the exact immutable record after physical+machine absence or a
    /// caller delivery acknowledgement.
    async fn delete_member_live_cleanup_record(
        &self,
        mob_id: &MobId,
        expected: &MobMemberLiveCleanupRecord,
    ) -> Result<bool, MobStoreError> {
        let _ = (mob_id, expected);
        runtime_metadata_capability_unavailable("delete_member_live_cleanup_record")
    }

    /// List all external binding compatibility projection records for a mob.
    async fn list_external_binding_overlays(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<ExternalBindingOverlayRecord>, MobStoreError>;

    /// Insert a new overlay if one does not already exist for the identity/generation key.
    ///
    /// Returns `true` when the record was inserted, `false` when an existing
    /// overlay already owns the key.
    async fn put_external_binding_overlay_if_absent(
        &self,
        mob_id: &MobId,
        record: &ExternalBindingOverlayRecord,
    ) -> Result<bool, MobStoreError>;

    /// Upsert an overlay for the identity/generation key.
    async fn upsert_external_binding_overlay(
        &self,
        mob_id: &MobId,
        record: &ExternalBindingOverlayRecord,
    ) -> Result<(), MobStoreError>;

    /// Delete the overlay for a specific identity/generation key.
    async fn delete_external_binding_overlay(
        &self,
        mob_id: &MobId,
        agent_identity: &AgentIdentity,
        generation: Generation,
    ) -> Result<(), MobStoreError>;

    /// Delete all overlays for the given mob.
    async fn delete_external_binding_overlays(&self, mob_id: &MobId) -> Result<(), MobStoreError>;
}

#[derive(Debug, Clone)]
pub(crate) struct IdentityMemberTargetState {
    pub(crate) observation: IdentityMemberTargetObservation,
    /// Present only while the current target is exactly the last
    /// `MemberSpawned` payload, with no later member-lifecycle mutation.
    pub(crate) exact_current_spawn: Option<MobEvent>,
}

fn identity_member_target_version(
    mob_id: &MobId,
    identity: &AgentIdentity,
    present: bool,
    latest_member_cursor: u64,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"meerkat.mob.identity-member-target.v1\0");
    digest.update((mob_id.as_str().len() as u64).to_be_bytes());
    digest.update(mob_id.as_str().as_bytes());
    digest.update((identity.as_str().len() as u64).to_be_bytes());
    digest.update(identity.as_str().as_bytes());
    digest.update([u8::from(present)]);
    digest.update(latest_member_cursor.to_be_bytes());
    format!("sha256:{:x}", digest.finalize())
}

fn malformed_member_target(
    mob_id: &MobId,
    identity: &AgentIdentity,
    cursor: u64,
    detail: impl Into<String>,
) -> IdentityMemberTargetState {
    IdentityMemberTargetState {
        observation: IdentityMemberTargetObservation::Malformed {
            observed_version: Some(identity_member_target_version(
                mob_id, identity, false, cursor,
            )),
            detail: detail.into(),
        },
        exact_current_spawn: None,
    }
}

fn validate_identity_member_spawn_event(
    mob_id: &MobId,
    identity: &AgentIdentity,
    event: &MobEvent,
) -> Result<(), String> {
    if event.mob_id != *mob_id {
        return Err("MemberSpawned event does not match the target mob".to_string());
    }
    let MobEventKind::MemberSpawned(spawned) = &event.kind else {
        return Err("identity member commit requires a MemberSpawned event".to_string());
    };
    if spawned.agent_identity != *identity
        || spawned.agent_runtime_id.identity != *identity
        || spawned.agent_runtime_id.generation != spawned.generation
    {
        return Err(
            "MemberSpawned identity, runtime identity, and generation are incoherent".to_string(),
        );
    }
    if spawned.fence_token.get() == 0 {
        return Err("MemberSpawned fencing token must be non-zero".to_string());
    }
    if spawned.bridge_member_ref().is_none() {
        return Err("MemberSpawned is missing its canonical bridge member reference".to_string());
    }
    Ok(())
}

/// Replay only member-lifecycle events relevant to one identity. The global
/// cursor is used solely as an immutable version atom after an event has been
/// selected as identity-local; unrelated mob events never perturb this CAS.
pub(crate) fn identity_member_target_state(
    events: &[MobEvent],
    mob_id: &MobId,
    identity: &AgentIdentity,
) -> IdentityMemberTargetState {
    let mut current_head: Option<(
        Generation,
        crate::ids::AgentRuntimeId,
        crate::ids::FenceToken,
    )> = None;
    let mut generation_highwater: Option<Generation> = None;
    let mut latest_member_cursor = 0;
    let mut exact_current_spawn = None;
    let mut current_evidence: Option<IdentityMemberTargetEvidence> = None;
    let mut lifecycle_recovery: Option<String> = None;

    for event in events.iter().filter(|event| event.mob_id == *mob_id) {
        match &event.kind {
            MobEventKind::MemberSpawned(spawned) if spawned.agent_identity == *identity => {
                if let Err(detail) = validate_identity_member_spawn_event(mob_id, identity, event) {
                    return malformed_member_target(mob_id, identity, event.cursor, detail);
                }
                if current_head.is_some() {
                    return malformed_member_target(
                        mob_id,
                        identity,
                        event.cursor,
                        "MemberSpawned overwrites a current member realization without terminal retirement",
                    );
                }
                if let Some(highwater) = generation_highwater {
                    if highwater.next() != Some(spawned.generation) {
                        return malformed_member_target(
                            mob_id,
                            identity,
                            event.cursor,
                            "MemberSpawned generation is not the exact successor of retired history",
                        );
                    }
                }
                current_head = Some((
                    spawned.generation,
                    spawned.agent_runtime_id.clone(),
                    spawned.fence_token,
                ));
                let Some(session_id) = spawned
                    .bridge_member_ref()
                    .and_then(MemberRef::bridge_session_id)
                    .cloned()
                else {
                    return malformed_member_target(
                        mob_id,
                        identity,
                        event.cursor,
                        "MemberSpawned is missing its canonical bridge session",
                    );
                };
                current_evidence = Some(IdentityMemberTargetEvidence {
                    identity: spawned.agent_identity.clone(),
                    intent_authority_digest: spawned
                        .identity_intent_authority_digest()
                        .map(str::to_string),
                    session_id,
                    role: spawned.role.clone(),
                    runtime_mode: spawned.runtime_mode,
                    labels: spawned.labels.clone(),
                });
                generation_highwater = Some(spawned.generation);
                latest_member_cursor = event.cursor;
                exact_current_spawn = Some(event.clone());
                lifecycle_recovery = None;
            }
            MobEventKind::MemberRetirementStarted {
                agent_identity,
                agent_runtime_id,
                generation,
                ..
            } if agent_identity == identity => {
                let Some((current_generation, current_runtime_id, _)) = &current_head else {
                    return malformed_member_target(
                        mob_id,
                        identity,
                        event.cursor,
                        "MemberRetirementStarted has no current member realization",
                    );
                };
                if current_generation != generation || current_runtime_id != agent_runtime_id {
                    return malformed_member_target(
                        mob_id,
                        identity,
                        event.cursor,
                        "MemberRetirementStarted does not name the exact current runtime",
                    );
                }
                latest_member_cursor = event.cursor;
                exact_current_spawn = None;
                lifecycle_recovery = Some(
                    "member retirement was durably admitted but has not completed".to_string(),
                );
            }
            MobEventKind::RemoteMemberRuntimeRetired {
                agent_identity,
                agent_runtime_id,
                fence_token,
                generation,
                ..
            }
            | MobEventKind::RemoteMemberSupervisorRevoked {
                agent_identity,
                agent_runtime_id,
                fence_token,
                generation,
                ..
            } if agent_identity == identity => {
                if generation_highwater.is_some_and(|highwater| *generation < highwater) {
                    // Delayed recovery progress from an older incarnation is
                    // a proven stale no-op, just like an older terminal.
                    continue;
                }
                let Some((current_generation, current_runtime_id, current_fence)) = &current_head
                else {
                    return malformed_member_target(
                        mob_id,
                        identity,
                        event.cursor,
                        "remote retirement progress has no current member realization",
                    );
                };
                if current_generation != generation
                    || current_runtime_id != agent_runtime_id
                    || current_fence != fence_token
                {
                    return malformed_member_target(
                        mob_id,
                        identity,
                        event.cursor,
                        "remote retirement progress does not name the exact current runtime",
                    );
                }
                latest_member_cursor = event.cursor;
                exact_current_spawn = None;
                lifecycle_recovery =
                    Some("remote member retirement progress is incomplete".to_string());
            }
            MobEventKind::MemberRetired {
                agent_identity,
                generation,
                ..
            } if agent_identity == identity
                && current_head
                    .as_ref()
                    .is_some_and(|(current_generation, _, _)| current_generation == generation) =>
            {
                current_head = None;
                current_evidence = None;
                latest_member_cursor = event.cursor;
                exact_current_spawn = None;
                lifecycle_recovery = None;
            }
            MobEventKind::MemberRetired {
                agent_identity,
                generation,
                ..
            } if agent_identity == identity => {
                if generation_highwater.is_some_and(|highwater| *generation < highwater) {
                    // A delayed older-generation terminal is a proven stale
                    // no-op and cannot erase the successor realization.
                    continue;
                }
                return malformed_member_target(
                    mob_id,
                    identity,
                    event.cursor,
                    "MemberRetired has no exact current generation to retire",
                );
            }
            MobEventKind::MemberReset {
                agent_identity,
                previous_generation,
                new_generation,
                fence_token,
                agent_runtime_id,
                ..
            } if agent_identity == identity => {
                let Some((current_generation, _, _)) = &current_head else {
                    return malformed_member_target(
                        mob_id,
                        identity,
                        event.cursor,
                        "MemberReset has no current member realization",
                    );
                };
                if current_generation != previous_generation
                    || previous_generation.next() != Some(*new_generation)
                    || agent_runtime_id.identity != *identity
                    || agent_runtime_id.generation != *new_generation
                    || fence_token.get() == 0
                {
                    return malformed_member_target(
                        mob_id,
                        identity,
                        event.cursor,
                        "MemberReset does not name an exact successor runtime",
                    );
                }
                current_head = Some((*new_generation, agent_runtime_id.clone(), *fence_token));
                generation_highwater = Some(*new_generation);
                latest_member_cursor = event.cursor;
                exact_current_spawn = None;
                lifecycle_recovery = None;
            }
            MobEventKind::MemberSessionBindingRecovered(recovered)
                if recovered.agent_identity == *identity =>
            {
                let Some((current_generation, current_runtime_id, _)) = &current_head else {
                    return malformed_member_target(
                        mob_id,
                        identity,
                        event.cursor,
                        "member session-binding recovery has no current member realization",
                    );
                };
                if current_generation != &recovered.agent_runtime_id.generation
                    || current_runtime_id != &recovered.agent_runtime_id
                    || recovered.bridge_session_id().is_none()
                {
                    return malformed_member_target(
                        mob_id,
                        identity,
                        event.cursor,
                        "member session-binding recovery does not name the exact current runtime and session",
                    );
                }
                if let (Some(evidence), Some(session_id)) =
                    (&mut current_evidence, recovered.bridge_session_id())
                {
                    evidence.session_id = session_id.clone();
                }
                latest_member_cursor = event.cursor;
                exact_current_spawn = None;
            }
            MobEventKind::MobReset => {
                current_head = None;
                current_evidence = None;
                generation_highwater = None;
                latest_member_cursor = event.cursor;
                exact_current_spawn = None;
                lifecycle_recovery = None;
            }
            _ => {}
        }
    }

    let present = current_head.is_some();
    let version = identity_member_target_version(mob_id, identity, present, latest_member_cursor);
    let observation = if let Some(detail) = lifecycle_recovery {
        IdentityMemberTargetObservation::Recovering { version, detail }
    } else if present {
        let Some(evidence) = current_evidence else {
            return malformed_member_target(
                mob_id,
                identity,
                latest_member_cursor,
                "present member realization is missing its desired-state evidence",
            );
        };
        IdentityMemberTargetObservation::Present { version, evidence }
    } else {
        IdentityMemberTargetObservation::Absent {
            absence_version: version,
        }
    };
    IdentityMemberTargetState {
        observation,
        exact_current_spawn,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct IdentityWiringTargetState {
    pub(crate) observation: IdentityWiringTargetObservation,
}

fn identity_wiring_target_version(
    mob_id: &MobId,
    identity: &AgentIdentity,
    reset_cursor: u64,
    incident_edges: &BTreeSet<crate::identity::DesiredIdentityEdge>,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"meerkat.mob.identity-wiring-target.v1\0");
    digest.update((mob_id.as_str().len() as u64).to_be_bytes());
    digest.update(mob_id.as_str().as_bytes());
    digest.update((identity.as_str().len() as u64).to_be_bytes());
    digest.update(identity.as_str().as_bytes());
    digest.update(reset_cursor.to_be_bytes());
    digest.update((incident_edges.len() as u64).to_be_bytes());
    for edge in incident_edges {
        digest.update((edge.a.as_str().len() as u64).to_be_bytes());
        digest.update(edge.a.as_str().as_bytes());
        digest.update((edge.b.as_str().len() as u64).to_be_bytes());
        digest.update(edge.b.as_str().as_bytes());
    }
    format!("sha256:{:x}", digest.finalize())
}

fn malformed_wiring_target(
    mob_id: &MobId,
    identity: &AgentIdentity,
    reset_cursor: u64,
    incident_edges: &BTreeSet<crate::identity::DesiredIdentityEdge>,
    detail: impl Into<String>,
) -> IdentityWiringTargetState {
    IdentityWiringTargetState {
        observation: IdentityWiringTargetObservation::Malformed {
            observed_version: Some(identity_wiring_target_version(
                mob_id,
                identity,
                reset_cursor,
                incident_edges,
            )),
            detail: detail.into(),
        },
    }
}

fn validate_stored_wiring_edge(
    identity: &AgentIdentity,
    a: &AgentIdentity,
    b: &AgentIdentity,
) -> Result<(), String> {
    if a == identity || b == identity {
        if a >= b {
            return Err(
                "identity wiring event carries a non-canonical or self-referential edge"
                    .to_string(),
            );
        }
    }
    Ok(())
}

/// Reduce the durable local structural wiring projection for one identity.
///
/// This deliberately does not route through `Roster::project`: roster member
/// presence is a separate realization fact and can legitimately be torn from
/// edge history. An edge remains observable until an exact `MembersUnwired`
/// event removes it, even if either endpoint is currently absent. Events
/// belonging to other mobs and decodable unrelated edge shapes are ignored.
/// Malformed raw rows are classified by the backend before this reducer runs.
pub(crate) fn identity_wiring_target_state(
    events: &[MobEvent],
    mob_id: &MobId,
    identity: &AgentIdentity,
) -> IdentityWiringTargetState {
    let mob_events = events
        .iter()
        .filter(|event| event.mob_id == *mob_id)
        .cloned()
        .collect::<Vec<_>>();
    let mut incident_edges = BTreeSet::new();
    let mut reset_cursor = 0;

    let apply_edge = |incident_edges: &mut BTreeSet<crate::identity::DesiredIdentityEdge>,
                      a: &AgentIdentity,
                      b: &AgentIdentity,
                      adding: bool| {
        if a != identity && b != identity {
            return Ok(());
        }
        validate_stored_wiring_edge(identity, a, b)?;
        let edge = crate::identity::DesiredIdentityEdge::new(a.clone(), b.clone())
            .map_err(|error| error.to_string())?;
        if adding {
            incident_edges.insert(edge);
        } else {
            incident_edges.remove(&edge);
        }
        Ok::<(), String>(())
    };
    for event in &mob_events {
        let result = match &event.kind {
            MobEventKind::MembersWired { a, b } => apply_edge(&mut incident_edges, a, b, true),
            MobEventKind::MembersUnwired { a, b } => apply_edge(&mut incident_edges, a, b, false),
            MobEventKind::MembersWiredBatch { edges } => edges
                .iter()
                .try_for_each(|edge| apply_edge(&mut incident_edges, &edge.a, &edge.b, true)),
            MobEventKind::MobReset => {
                incident_edges.clear();
                reset_cursor = event.cursor;
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(detail) = result {
            return malformed_wiring_target(
                mob_id,
                identity,
                reset_cursor,
                &incident_edges,
                detail,
            );
        }
    }

    let version = identity_wiring_target_version(mob_id, identity, reset_cursor, &incident_edges);
    let observation = if incident_edges.is_empty() {
        IdentityWiringTargetObservation::Absent {
            absence_version: version,
        }
    } else {
        IdentityWiringTargetObservation::Present {
            version,
            incident_edges,
        }
    };
    IdentityWiringTargetState { observation }
}

fn wiring_commit_repair_blocked(detail: impl Into<String>) -> IdentityWiringEventCommitOutcome {
    IdentityWiringEventCommitOutcome::RepairBlocked {
        evidence_digest: None,
        detail: detail.into(),
    }
}

#[allow(clippy::result_large_err)] // exact typed target outcome is required at this boundary
fn proposed_identity_wiring_edge(
    permit: &IdentityActuationPermit,
    event: &MobEvent,
) -> Result<(crate::identity::DesiredIdentityEdge, bool), IdentityWiringEventCommitOutcome> {
    if permit.target != crate::identity::IdentityActuatorTarget::Wiring
        || permit.mob_id != event.mob_id
    {
        return Err(wiring_commit_repair_blocked(
            "identity wiring permit is not scoped to the proposed structural event",
        ));
    }
    let (a, b, adding) = match &event.kind {
        MobEventKind::MembersWired { a, b } => (a, b, true),
        MobEventKind::MembersUnwired { a, b } => (a, b, false),
        _ => {
            return Err(wiring_commit_repair_blocked(
                "identity wiring commit requires one MembersWired or MembersUnwired event",
            ));
        }
    };
    if a >= b || (a != &permit.identity && b != &permit.identity) {
        return Err(wiring_commit_repair_blocked(
            "identity wiring event is non-canonical or not incident to the permitted identity",
        ));
    }
    let edge = crate::identity::DesiredIdentityEdge::new(a.clone(), b.clone())
        .map_err(|error| wiring_commit_repair_blocked(error.to_string()))?;
    Ok((edge, adding))
}

#[allow(clippy::result_large_err)] // exact typed target outcome is required at this boundary
pub(crate) fn validate_identity_wiring_commit_authority(
    permit: &IdentityActuationPermit,
    intent: &IdentityIntentRecord,
    lease: &IdentityLeaseRecord,
    event: &MobEvent,
    observed_at_ms: u64,
) -> Result<(crate::identity::DesiredIdentityEdge, bool), IdentityWiringEventCommitOutcome> {
    let (edge, adding) = proposed_identity_wiring_edge(permit, event)?;
    if let Err(error) = permit.validate_for_write(observed_at_ms) {
        return Err(IdentityWiringEventCommitOutcome::Conflict {
            current: None,
            detail: format!("identity wiring permit is no longer current: {error}"),
        });
    }
    if let Err(error) = intent.validate() {
        return Err(wiring_commit_repair_blocked(format!(
            "identity wiring commit observed malformed intent authority: {error}"
        )));
    }
    if intent.mob_id != permit.mob_id
        || intent.intent.identity() != &permit.identity
        || intent.intent_revision != permit.intent_revision
        || intent.intent_digest != permit.intent_digest
        || intent.authority_digest != permit.intent_authority_digest
    {
        return Err(IdentityWiringEventCommitOutcome::Conflict {
            current: None,
            detail: "identity wiring commit intent authority is stale".to_string(),
        });
    }
    if adding && edge.owner() != &permit.identity {
        return Err(wiring_commit_repair_blocked(
            "identity wiring addition requires the lexicographic edge owner's authority",
        ));
    }
    let cleanup_edges = match &intent.retirement_plan {
        crate::identity::IdentityRetirementPlan::Targets {
            incident_wiring, ..
        } => incident_wiring,
        crate::identity::IdentityRetirementPlan::NoKnownRealization => {
            empty_identity_wiring_edges()
        }
    };
    match (&intent.intent, adding) {
        (IdentityIntent::Present { owned_wiring, .. }, true) if owned_wiring.contains(&edge) => {}
        // Removal is a resource-local safety operation selected by the
        // stateless generated classifier from fresh realization evidence.
        // The store revalidates only the exact current intent, active lease,
        // owner-only addition/either-endpoint safety removal, and target
        // observation; it must not grow a shadow session/status classifier. A
        // still-desired Present edge may therefore be drained after unsafe
        // session evidence and will be re-established by a later fresh
        // ReconcileWiring decision if it becomes safe again.
        (IdentityIntent::Present { .. }, false) => {}
        (IdentityIntent::Absent { .. }, false) if cleanup_edges.contains(&edge) => {}
        (IdentityIntent::Absent { .. }, true) => {
            return Err(wiring_commit_repair_blocked(
                "MembersWired cannot be authorized by an absent identity intent",
            ));
        }
        _ => {
            return Err(wiring_commit_repair_blocked(
                "identity wiring event contradicts the store-sealed desired or cleanup edge set",
            ));
        }
    }
    if let Err(error) = lease.validate() {
        return Err(wiring_commit_repair_blocked(format!(
            "identity wiring commit observed malformed lease authority: {error}"
        )));
    }
    let Some(active) = &lease.active else {
        return Err(IdentityWiringEventCommitOutcome::Conflict {
            current: None,
            detail: "identity wiring commit observed no active lease".to_string(),
        });
    };
    if active.epoch != permit.lease_epoch
        || active.holder_id != permit.lease_holder_id
        || active.incarnation_id != permit.lease_incarnation_id
        || active.expires_at_ms != permit.lease_expires_at_ms
        || observed_at_ms >= active.expires_at_ms
    {
        return Err(IdentityWiringEventCommitOutcome::Conflict {
            current: None,
            detail: "identity wiring commit lease authority is stale, expired, or superseded"
                .to_string(),
        });
    }
    Ok((edge, adding))
}

fn member_commit_repair_blocked(detail: impl Into<String>) -> IdentityMemberEventCommitOutcome {
    IdentityMemberEventCommitOutcome::RepairBlocked {
        evidence_digest: None,
        detail: detail.into(),
    }
}

fn desired_member_runtime_mode(
    member: &crate::identity::DesiredMemberSpec,
) -> crate::runtime_mode::MobRuntimeMode {
    match member.material.overlay.runtime_mode {
        meerkat_contracts::wire::WireMobRuntimeMode::AutonomousHost => {
            crate::runtime_mode::MobRuntimeMode::AutonomousHost
        }
        meerkat_contracts::wire::WireMobRuntimeMode::TurnDriven => {
            crate::runtime_mode::MobRuntimeMode::TurnDriven
        }
    }
}

#[allow(clippy::result_large_err)] // exact typed target outcome is required at this boundary
pub(crate) fn validate_identity_member_commit_authority(
    permit: &IdentityActuationPermit,
    intent: &IdentityIntentRecord,
    lease: &IdentityLeaseRecord,
    event: &MobEvent,
    observed_at_ms: u64,
) -> Result<(), IdentityMemberEventCommitOutcome> {
    if permit.target != crate::identity::IdentityActuatorTarget::Member
        || permit.mob_id != event.mob_id
    {
        return Err(member_commit_repair_blocked(
            "identity member commit permit is not scoped to the proposed MemberSpawned event",
        ));
    }
    if let Err(detail) =
        validate_identity_member_spawn_event(&permit.mob_id, &permit.identity, event)
    {
        return Err(member_commit_repair_blocked(detail));
    }
    if let Err(error) = permit.validate_for_write(observed_at_ms) {
        return Err(IdentityMemberEventCommitOutcome::Conflict {
            current: None,
            detail: format!("identity member actuation permit is no longer current: {error}"),
        });
    }
    if let Err(error) = intent.validate() {
        return Err(member_commit_repair_blocked(format!(
            "identity member commit observed malformed intent authority: {error}"
        )));
    }
    if intent.mob_id != permit.mob_id
        || intent.intent.identity() != &permit.identity
        || intent.intent_revision != permit.intent_revision
        || intent.intent_digest != permit.intent_digest
        || intent.authority_digest != permit.intent_authority_digest
    {
        return Err(IdentityMemberEventCommitOutcome::Conflict {
            current: None,
            detail: "identity member commit intent authority is stale".to_string(),
        });
    }
    let IdentityIntent::Present {
        session, member, ..
    } = &intent.intent
    else {
        return Err(member_commit_repair_blocked(
            "MemberSpawned finalization cannot be authorized by an absent intent",
        ));
    };
    let MobEventKind::MemberSpawned(spawned) = &event.kind else {
        return Err(member_commit_repair_blocked(
            "identity member commit requires a MemberSpawned event",
        ));
    };
    let expected_runtime_mode = desired_member_runtime_mode(member);
    let expected_labels = member.material.overlay.labels.clone().unwrap_or_default();
    if spawned.identity_intent_authority_digest() != Some(intent.authority_digest.as_str())
        || spawned.role != member.material.profile_name
        || spawned.runtime_mode != expected_runtime_mode
        || spawned.labels != expected_labels
        || spawned
            .bridge_member_ref()
            .and_then(MemberRef::bridge_session_id)
            != Some(&session.session_id)
    {
        return Err(member_commit_repair_blocked(
            "MemberSpawned payload does not match the sealed desired member/session material",
        ));
    }
    if let Err(error) = lease.validate() {
        return Err(member_commit_repair_blocked(format!(
            "identity member commit observed malformed lease authority: {error}"
        )));
    }
    let Some(active) = &lease.active else {
        return Err(IdentityMemberEventCommitOutcome::Conflict {
            current: None,
            detail: "identity member commit observed no active lease".to_string(),
        });
    };
    if active.epoch != permit.lease_epoch
        || active.holder_id != permit.lease_holder_id
        || active.incarnation_id != permit.lease_incarnation_id
        || active.expires_at_ms != permit.lease_expires_at_ms
        || observed_at_ms >= active.expires_at_ms
    {
        return Err(IdentityMemberEventCommitOutcome::Conflict {
            current: None,
            detail: "identity member commit lease authority is stale, expired, or superseded"
                .to_string(),
        });
    }
    Ok(())
}

/// Atomic built-in capability joining the sole identity authority with the
/// existing structural member/wiring event log. Custom storage compositions
/// do not receive this capability because two independent trait objects cannot
/// make the required one-transaction guarantee.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub(crate) trait MobIdentityMemberStore: Send + Sync {
    async fn observe_identity_member_target(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityMemberTargetObservation, MobStoreError>;

    async fn commit_identity_member_spawned(
        &self,
        permit: &IdentityActuationPermit,
        event: &NewMobEvent,
    ) -> Result<IdentityMemberEventCommitOutcome, MobStoreError>;

    async fn observe_identity_wiring_target(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityWiringTargetObservation, MobStoreError>;

    async fn commit_identity_wiring_event(
        &self,
        permit: &IdentityActuationPermit,
        event: &NewMobEvent,
    ) -> Result<IdentityWiringEventCommitOutcome, MobStoreError>;
}

#[cfg(feature = "runtime-adapter")]
pub(crate) fn identity_runtime_target_observation_version(
    observed: &meerkat_runtime::RuntimeSessionLifecycleObservation,
) -> IdentityTargetObservationVersion {
    match observed.lifecycle().expected_version() {
        meerkat_runtime::store::MachineLifecycleExpectedVersion::Missing => {
            IdentityTargetObservationVersion::Absent {
                absence_version: format!("runtime-lifecycle-absent:{}", observed.runtime_id()),
            }
        }
        meerkat_runtime::store::MachineLifecycleExpectedVersion::Version(version) => {
            IdentityTargetObservationVersion::Version {
                version: version.as_str().to_string(),
            }
        }
    }
}

#[cfg(feature = "runtime-adapter")]
pub(crate) fn validate_identity_runtime_target_binding(
    permit: &IdentityActuationPermit,
    expected_session: &DesiredSessionTarget,
    observed: &meerkat_runtime::RuntimeSessionLifecycleObservation,
) -> Result<(), MobStoreError> {
    let expected_target = identity_runtime_target_observation_version(observed);
    if permit.target != crate::identity::IdentityActuatorTarget::Runtime
        || observed.session_id() != &expected_session.session_id
        || permit.target_observation != expected_target
    {
        return Err(MobStoreError::IdentityAuthorityBlocked {
            evidence_digest: observed.lifecycle().evidence_digest().map(str::to_string),
            detail: "runtime write fence is not bound to the exact runtime target observation"
                .to_string(),
        });
    }
    Ok(())
}

#[cfg(feature = "runtime-adapter")]
pub(crate) fn validate_identity_runtime_write_authority(
    permit: &IdentityActuationPermit,
    expected_session: &DesiredSessionTarget,
    intent: Option<&IdentityIntentRecord>,
    lease: Option<&IdentityLeaseRecord>,
    observed_at_ms: u64,
) -> Result<(), MobStoreError> {
    permit.validate_for_write(observed_at_ms).map_err(|error| {
        MobStoreError::CasConflict(format!(
            "identity runtime actuation permit is no longer current: {error}"
        ))
    })?;

    let intent = intent.ok_or_else(|| {
        MobStoreError::CasConflict(
            "identity runtime actuation observed no current intent".to_string(),
        )
    })?;
    intent
        .validate()
        .map_err(|error| MobStoreError::IdentityAuthorityBlocked {
            evidence_digest: None,
            detail: format!("identity runtime actuation observed malformed intent: {error}"),
        })?;
    if intent.mob_id != permit.mob_id
        || intent.intent.identity() != &permit.identity
        || intent.intent_revision != permit.intent_revision
        || intent.intent_digest != permit.intent_digest
        || intent.authority_digest != permit.intent_authority_digest
    {
        return Err(MobStoreError::CasConflict(
            "identity runtime actuation intent authority is stale".to_string(),
        ));
    }
    match &intent.intent {
        IdentityIntent::Present { session, .. } if session == expected_session => {}
        IdentityIntent::Present { .. } => {
            return Err(MobStoreError::CasConflict(
                "identity runtime actuation session target is stale".to_string(),
            ));
        }
        IdentityIntent::Absent { .. } => {
            return Err(MobStoreError::IdentityAuthorityBlocked {
                evidence_digest: None,
                detail: "runtime registration cannot be authorized by an absent identity intent"
                    .to_string(),
            });
        }
    }

    let lease = lease.ok_or_else(|| {
        MobStoreError::CasConflict(
            "identity runtime actuation observed no current lease".to_string(),
        )
    })?;
    lease
        .validate()
        .map_err(|error| MobStoreError::IdentityAuthorityBlocked {
            evidence_digest: None,
            detail: format!("identity runtime actuation observed malformed lease: {error}"),
        })?;
    let active = lease.active.as_ref().ok_or_else(|| {
        MobStoreError::CasConflict(
            "identity runtime actuation observed no active lease".to_string(),
        )
    })?;
    if active.epoch != permit.lease_epoch
        || active.holder_id != permit.lease_holder_id
        || active.incarnation_id != permit.lease_incarnation_id
        || active.expires_at_ms != permit.lease_expires_at_ms
        || observed_at_ms >= active.expires_at_ms
    {
        return Err(MobStoreError::CasConflict(
            "identity runtime actuation lease authority is stale, expired, or superseded"
                .to_string(),
        ));
    }
    Ok(())
}

#[cfg(feature = "runtime-adapter")]
pub(crate) fn identity_runtime_fence_error(
    error: MobStoreError,
) -> Result<meerkat_runtime::RuntimeStoreWriteFenceOutcome, meerkat_runtime::RuntimeStoreError> {
    match error {
        MobStoreError::CasConflict(detail) | MobStoreError::NotFound(detail) => {
            Ok(meerkat_runtime::RuntimeStoreWriteFenceOutcome::Conflict { reason: detail })
        }
        MobStoreError::IdentityAuthorityBlocked {
            evidence_digest,
            detail,
        } => Err(
            meerkat_runtime::RuntimeStoreError::MachineLifecycleRepairBlocked {
                evidence_digest,
                detail,
            },
        ),
        MobStoreError::Serialization(detail) => Err(
            meerkat_runtime::RuntimeStoreError::MachineLifecycleRepairBlocked {
                evidence_digest: None,
                detail,
            },
        ),
        other => Ok(meerkat_runtime::RuntimeStoreWriteFenceOutcome::Backoff {
            reason: other.to_string(),
        }),
    }
}

/// First-class persistence owner for level-triggered mob identity intent.
///
/// This is intentionally separate from [`MobRuntimeMetadataStore`]: intent,
/// leases, and immutable operation custody are semantic convergence inputs,
/// while runtime metadata contains observed-realization projections. Built-in
/// SQLite composition may share one database, but callers must inject this
/// capability explicitly through `MobStorage`.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MobIdentityStore: Send + Sync {
    /// Prepare an observation-bound runtime write fence.
    ///
    /// Built-in stores retain their identity-authority serialization guard
    /// continuously across each synchronous target callback. Custom stores
    /// fail closed unless they can provide the same atomic boundary.
    #[cfg(feature = "runtime-adapter")]
    fn prepare_runtime_write_fence(
        &self,
        _permit: IdentityActuationPermit,
        _expected_session: DesiredSessionTarget,
        _observed: &meerkat_runtime::RuntimeSessionLifecycleObservation,
    ) -> Result<Arc<dyn meerkat_runtime::RuntimeStoreWriteFence>, MobStoreError> {
        Err(MobStoreError::IdentityRuntimeWriteFenceUnavailable)
    }

    async fn observe_identity_declaration_scope(
        &self,
        mob_id: &MobId,
        scope_id: &crate::identity::IdentityDeclarationScopeId,
    ) -> Result<IdentityStoredObservation<IdentityDeclarationScopeHead>, MobStoreError>;

    async fn observe_identity_intent(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityStoredObservation<IdentityIntentRecord>, MobStoreError>;

    async fn list_identity_intents(
        &self,
        mob_id: &MobId,
    ) -> Result<
        BTreeMap<AgentIdentity, IdentityStoredObservation<IdentityIntentRecord>>,
        MobStoreError,
    >;

    /// Return the immutable original result for an exact declaration
    /// operation before the actor recompiles portable material. This protects
    /// lost-ACK replay from profile, skill, or base-prompt drift.
    ///
    /// The store validates that current scope/intent authority is a monotonic
    /// descendant of the receipt. A reused operation id with a different
    /// request digest is a CAS conflict; `None` means the exact slot is absent.
    async fn replay_identity_declaration(
        &self,
        mob_id: &MobId,
        scope_id: &crate::identity::IdentityDeclarationScopeId,
        operation_id: &meerkat_core::ops::OperationId,
        request_digest: &str,
    ) -> Result<Option<IdentityDeclarationManifestApplyOutcome>, MobStoreError>;

    /// One transaction validates the scope CAS, reuses or allocates targets,
    /// seals all affected intents, optionally installs a verified legacy
    /// lease high-water for a previously missing identity, advances the scope
    /// head, and inserts the immutable operation receipt. A legacy seed is
    /// accepted only when both intent and lease rows are missing; its snapshot
    /// fence is audit-only and never participates in checkpoint coherence.
    /// No prepared/shadow rows exist.
    async fn apply_identity_declaration(
        &self,
        mob_id: &MobId,
        plan: &IdentityDeclarationApplyPlan,
    ) -> Result<IdentityDeclarationManifestApplyOutcome, MobStoreError>;

    async fn observe_identity_lease(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityStoredObservation<IdentityLeaseRecord>, MobStoreError>;

    async fn claim_or_renew_identity_lease(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
        holder_id: &str,
        incarnation_id: &str,
        ttl_ms: u64,
    ) -> Result<IdentityLeaseClaimOutcome, MobStoreError>;

    async fn release_identity_lease(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
        expected: &IdentityLeaseClaim,
    ) -> Result<bool, MobStoreError>;

    /// Revalidate the desired-state and lease half of one ephemeral actuator
    /// permit against the store-owned clock. Target writers still own their
    /// resource-local observation CAS; this check prevents an expired,
    /// superseded, or previous-incarnation controller from reaching that
    /// writer at all.
    async fn validate_identity_actuation_permit(
        &self,
        permit: &IdentityActuationPermit,
    ) -> Result<(), MobStoreError>;

    async fn observe_identity_operation_receipt(
        &self,
        mob_id: &MobId,
        subject: &IdentityOperationSubject,
        slot: &IdentityOperationSlot,
    ) -> Result<IdentityStoredObservation<IdentityOperationReceipt>, MobStoreError>;

    async fn insert_identity_operation_receipt_if_absent(
        &self,
        receipt: &IdentityOperationReceipt,
        permit: &IdentityActuationPermit,
    ) -> Result<IdentityOperationReceiptInsertOutcome, MobStoreError>;
}

/// Replaceable identity convergence diagnostics.
///
/// This projection capability is intentionally separate from
/// [`MobIdentityStore`]. Status never participates in a desired-state CAS,
/// grants no repair authority, and is never read by the classifier.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MobIdentityStatusStore: Send + Sync {
    async fn load_identity_convergence_status(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<IdentityStoredObservation<IdentityConvergenceStatus>, MobStoreError>;

    async fn list_identity_convergence_statuses(
        &self,
        mob_id: &MobId,
    ) -> Result<
        BTreeMap<AgentIdentity, IdentityStoredObservation<IdentityConvergenceStatus>>,
        MobStoreError,
    >;

    async fn replace_identity_convergence_status(
        &self,
        mob_id: &MobId,
        status: &IdentityConvergenceStatus,
    ) -> Result<(), MobStoreError>;
}

/// Trait for persisting and querying flow runs.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MobRunStore: Send + Sync {
    async fn create_run(&self, run: MobRun) -> Result<(), MobStoreError>;
    async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, MobStoreError>;
    async fn list_runs(
        &self,
        mob_id: &MobId,
        flow_id: Option<&FlowId>,
    ) -> Result<Vec<MobRun>, MobStoreError>;
    /// Idempotently persist the complete replay material before the public
    /// record carrier and before remote delivery are permitted.
    async fn put_remote_turn_intent(
        &self,
        _run_id: &RunId,
        _intent: &MobRunRemoteTurnIntent,
    ) -> Result<bool, MobStoreError> {
        Err(MobStoreError::FrameAtomicPersistenceUnavailable {
            operation: FrameAtomicOperation::PutRemoteTurnIntent,
        })
    }
    async fn delete_remote_turn_intent(
        &self,
        _run_id: &RunId,
        _dispatch_sequence: u64,
    ) -> Result<bool, MobStoreError> {
        Err(MobStoreError::FrameAtomicPersistenceUnavailable {
            operation: FrameAtomicOperation::DeleteRemoteTurnIntent,
        })
    }
    async fn list_remote_turn_intents(
        &self,
        _run_id: &RunId,
    ) -> Result<Vec<MobRunRemoteTurnIntent>, MobStoreError> {
        Err(MobStoreError::FrameAtomicPersistenceUnavailable {
            operation: FrameAtomicOperation::PutRemoteTurnIntent,
        })
    }
    /// Idempotently persist one exact remote-turn receipt under its dispatch
    /// sequence. Returns `true` when inserted and `false` when an identical
    /// receipt already existed; conflicting reuse fails closed.
    async fn put_remote_turn_receipt(
        &self,
        _run_id: &RunId,
        _receipt: &MobRunRemoteTurnReceipt,
    ) -> Result<bool, MobStoreError> {
        Err(MobStoreError::FrameAtomicPersistenceUnavailable {
            operation: FrameAtomicOperation::PutRemoteTurnReceipt,
        })
    }
    async fn list_remote_turn_receipts(
        &self,
        _run_id: &RunId,
    ) -> Result<Vec<MobRunRemoteTurnReceipt>, MobStoreError> {
        Err(MobStoreError::FrameAtomicPersistenceUnavailable {
            operation: FrameAtomicOperation::PutRemoteTurnReceipt,
        })
    }
    async fn delete_remote_turn_receipt(
        &self,
        _run_id: &RunId,
        _dispatch_sequence: u64,
    ) -> Result<bool, MobStoreError> {
        Err(MobStoreError::FrameAtomicPersistenceUnavailable {
            operation: FrameAtomicOperation::DeleteRemoteTurnReceipt,
        })
    }
    async fn cas_flow_state_with_authority(
        &self,
        run_id: &RunId,
        expected: &flow_run::State,
        next: &flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError>;
    async fn cas_run_snapshot_with_authority(
        &self,
        run_id: &RunId,
        expected_status: MobRunStatus,
        expected_flow_state: &flow_run::State,
        next_status: MobRunStatus,
        next_flow_state: &flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError>;

    /// Atomically commit a terminal run snapshot AND append its terminal mob
    /// event, so terminal run-status truth and terminal-event truth cannot
    /// split.
    ///
    /// Returns `Ok(Some(event))` when the snapshot CAS won and the terminal
    /// event is committed, `Ok(None)` when the CAS lost (nothing appended).
    ///
    /// The default implementation validates the terminal event fail-closed
    /// BEFORE any mutation, then commits through the two underlying seams; it
    /// is only used by non-durable (in-memory / test-double) stores where a
    /// process death loses both sides together. The SQLite store overrides
    /// this with a true single-transaction commit that has no divergence
    /// window.
    #[allow(clippy::too_many_arguments)]
    async fn cas_run_snapshot_and_append_terminal_event_with_authority(
        &self,
        run_id: &RunId,
        expected_status: MobRunStatus,
        expected_flow_state: &flow_run::State,
        next_status: MobRunStatus,
        next_flow_state: &flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
        events: &dyn MobEventStore,
        terminal_event: NewMobEvent,
    ) -> Result<Option<MobEvent>, MobStoreError> {
        validate_mob_event_write_authority(&terminal_event.kind)?;
        if terminal_event_identity(&terminal_event.kind).is_none() {
            return Err(MobStoreError::Internal(
                "cas_run_snapshot_and_append_terminal_event_with_authority requires a terminal flow event".to_string(),
            ));
        }
        let transitioned = self
            .cas_run_snapshot_with_authority(
                run_id,
                expected_status,
                expected_flow_state,
                next_status,
                next_flow_state,
                authority_inputs,
            )
            .await?;
        if !transitioned {
            return Ok(None);
        }
        match events.append(terminal_event).await {
            Ok(stored) => Ok(Some(stored)),
            Err(append_error) => Err(MobStoreError::Internal(format!(
                "terminal run status persisted but terminal event append failed for run '{run_id}': {append_error}"
            ))),
        }
    }

    async fn append_step_entry_with_authority(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<(), MobStoreError>;
    async fn append_step_entry_if_absent_with_authority(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<bool, MobStoreError>;
    async fn append_failure_entry_with_authority(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<(), MobStoreError>;
    /// Append one exact failure-ledger projection if the step has no failure
    /// row yet. An exact replay returns `false`; conflicting metadata for the
    /// same step fails closed.
    ///
    /// Durable implementations override this with an atomic check-and-append.
    async fn append_failure_entry_if_absent_with_authority(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<bool, MobStoreError> {
        let run = self
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobStoreError::NotFound(format!("run not found: {run_id}")))?;
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
        self.append_failure_entry_with_authority(run_id, entry, authority)
            .await?;
        Ok(true)
    }

    async fn cas_frame_state_with_authority(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        expected: Option<&FrameSnapshot>,
        next: FrameSnapshot,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError>;

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
    ) -> Result<bool, MobStoreError>;

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
    ) -> Result<bool, MobStoreError>;

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
    ) -> Result<bool, MobStoreError>;

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
    ) -> Result<bool, MobStoreError>;

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
    ) -> Result<bool, MobStoreError>;

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
    ) -> Result<bool, MobStoreError>;

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
    ) -> Result<bool, MobStoreError>;
}

/// Wrap a run store so custom persistence cannot become flow lifecycle authority.
///
/// The inner store owns IO mechanics only. Every `MobRun` crossing this boundary
/// must validate against its persisted MobMachine authority log before callers
/// may consume lifecycle/status/flow-state projections.
pub(crate) fn authority_validating_mob_run_store(
    inner: Arc<dyn MobRunStore>,
) -> Arc<dyn MobRunStore> {
    Arc::new(AuthorityValidatingMobRunStore { inner })
}

struct AuthorityValidatingMobRunStore {
    inner: Arc<dyn MobRunStore>,
}

impl AuthorityValidatingMobRunStore {
    fn validate_run(run: &MobRun, context: &str) -> Result<(), MobStoreError> {
        run.validate_flow_authority_projection().map_err(|error| {
            MobStoreError::Internal(format!(
                "MobRunStore {context} returned run '{}' whose lifecycle projection is not authorized by MobMachine: {error}",
                run.run_id
            ))
        })
    }

    fn validate_listed_run(
        run: &MobRun,
        mob_id: &MobId,
        flow_id: Option<&FlowId>,
    ) -> Result<(), MobStoreError> {
        if &run.mob_id != mob_id {
            return Err(MobStoreError::Internal(format!(
                "MobRunStore list_runs returned run '{}' for mob '{}' while listing mob '{}'",
                run.run_id, run.mob_id, mob_id
            )));
        }
        if let Some(flow_id) = flow_id
            && &run.flow_id != flow_id
        {
            return Err(MobStoreError::Internal(format!(
                "MobRunStore list_runs returned run '{}' for flow '{}' while listing flow '{}'",
                run.run_id, run.flow_id, flow_id
            )));
        }
        Self::validate_run(run, "list_runs")
    }

    async fn validate_existing_run_by_id(
        &self,
        run_id: &RunId,
        context: &str,
    ) -> Result<(), MobStoreError> {
        let run = self.inner.get_run(run_id).await?.ok_or_else(|| {
            MobStoreError::Internal(format!(
                "MobRunStore {context} accepted mutation for run '{run_id}' but returned no run"
            ))
        })?;
        Self::validate_run(&run, context)
    }

    async fn validate_run_by_id_if_present(
        &self,
        run_id: &RunId,
        context: &str,
    ) -> Result<(), MobStoreError> {
        if let Some(run) = self.inner.get_run(run_id).await? {
            Self::validate_run(&run, context)?;
        }
        Ok(())
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobRunStore for AuthorityValidatingMobRunStore {
    async fn create_run(&self, run: MobRun) -> Result<(), MobStoreError> {
        Self::validate_run(&run, "create_run")?;
        let run_id = run.run_id.clone();
        self.inner.create_run(run).await?;
        self.validate_existing_run_by_id(&run_id, "create_run")
            .await
    }

    async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, MobStoreError> {
        let run = self.inner.get_run(run_id).await?;
        if let Some(run) = run.as_ref() {
            if &run.run_id != run_id {
                return Err(MobStoreError::Internal(format!(
                    "MobRunStore get_run returned run '{}' while reading run '{}'",
                    run.run_id, run_id
                )));
            }
            Self::validate_run(run, "get_run")?;
        }
        Ok(run)
    }

    async fn list_runs(
        &self,
        mob_id: &MobId,
        flow_id: Option<&FlowId>,
    ) -> Result<Vec<MobRun>, MobStoreError> {
        let runs = self.inner.list_runs(mob_id, flow_id).await?;
        for run in &runs {
            Self::validate_listed_run(run, mob_id, flow_id)?;
        }
        Ok(runs)
    }

    async fn put_remote_turn_intent(
        &self,
        run_id: &RunId,
        intent: &MobRunRemoteTurnIntent,
    ) -> Result<bool, MobStoreError> {
        if &intent.obligation.run_id != run_id {
            return Err(MobStoreError::Internal(format!(
                "remote-turn intent run '{}' does not match store key '{run_id}'",
                intent.obligation.run_id
            )));
        }
        let run = self
            .inner
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobStoreError::NotFound(format!("run '{run_id}'")))?;
        intent
            .validate_for(run_id, &run.mob_id)
            .map_err(MobStoreError::Internal)?;
        let inserted = self.inner.put_remote_turn_intent(run_id, intent).await?;
        self.validate_existing_run_by_id(run_id, "put_remote_turn_intent")
            .await?;
        Ok(inserted)
    }

    async fn delete_remote_turn_intent(
        &self,
        run_id: &RunId,
        dispatch_sequence: u64,
    ) -> Result<bool, MobStoreError> {
        let deleted = self
            .inner
            .delete_remote_turn_intent(run_id, dispatch_sequence)
            .await?;
        self.validate_existing_run_by_id(run_id, "delete_remote_turn_intent")
            .await?;
        Ok(deleted)
    }

    async fn list_remote_turn_intents(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<MobRunRemoteTurnIntent>, MobStoreError> {
        let intents = self.inner.list_remote_turn_intents(run_id).await?;
        let run = self
            .inner
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobStoreError::NotFound(format!("run '{run_id}'")))?;
        for intent in &intents {
            intent
                .validate_for(run_id, &run.mob_id)
                .map_err(MobStoreError::Internal)?;
        }
        Ok(intents)
    }

    async fn put_remote_turn_receipt(
        &self,
        run_id: &RunId,
        receipt: &MobRunRemoteTurnReceipt,
    ) -> Result<bool, MobStoreError> {
        if &receipt.obligation.run_id != run_id {
            return Err(MobStoreError::Internal(format!(
                "remote-turn receipt run '{}' does not match store key '{run_id}'",
                receipt.obligation.run_id
            )));
        }
        let run = self
            .inner
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobStoreError::NotFound(format!("run '{run_id}'")))?;
        let intent = self
            .inner
            .list_remote_turn_intents(run_id)
            .await?
            .into_iter()
            .find(|intent| intent.obligation == receipt.obligation)
            .ok_or_else(|| {
                MobStoreError::Internal(
                    "remote-turn receipt has no exact durable intent".to_string(),
                )
            })?;
        receipt
            .validate_for(run_id, &run.mob_id, &intent)
            .map_err(MobStoreError::Internal)?;
        let inserted = self.inner.put_remote_turn_receipt(run_id, receipt).await?;
        self.validate_existing_run_by_id(run_id, "put_remote_turn_receipt")
            .await?;
        Ok(inserted)
    }

    async fn list_remote_turn_receipts(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<MobRunRemoteTurnReceipt>, MobStoreError> {
        let receipts = self.inner.list_remote_turn_receipts(run_id).await?;
        let run = self
            .inner
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobStoreError::NotFound(format!("run '{run_id}'")))?;
        let intents = self.inner.list_remote_turn_intents(run_id).await?;
        for receipt in &receipts {
            receipt
                .validate_shape_for(run_id)
                .map_err(MobStoreError::Internal)?;
            let intent = intents
                .iter()
                .find(|intent| intent.obligation == receipt.obligation);
            if let Some(intent) = intent {
                receipt
                    .validate_for(run_id, &run.mob_id, intent)
                    .map_err(MobStoreError::Internal)?;
            }
        }
        Ok(receipts)
    }

    async fn delete_remote_turn_receipt(
        &self,
        run_id: &RunId,
        dispatch_sequence: u64,
    ) -> Result<bool, MobStoreError> {
        let deleted = self
            .inner
            .delete_remote_turn_receipt(run_id, dispatch_sequence)
            .await?;
        self.validate_existing_run_by_id(run_id, "delete_remote_turn_receipt")
            .await?;
        Ok(deleted)
    }

    async fn cas_flow_state_with_authority(
        &self,
        run_id: &RunId,
        expected: &flow_run::State,
        next: &flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let changed = self
            .inner
            .cas_flow_state_with_authority(run_id, expected, next, authority_inputs)
            .await?;
        if changed {
            self.validate_existing_run_by_id(run_id, "cas_flow_state_with_authority")
                .await?;
        } else {
            self.validate_run_by_id_if_present(run_id, "cas_flow_state_with_authority")
                .await?;
        }
        Ok(changed)
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
        let changed = self
            .inner
            .cas_run_snapshot_with_authority(
                run_id,
                expected_status,
                expected_flow_state,
                next_status,
                next_flow_state,
                authority_inputs,
            )
            .await?;
        if changed {
            self.validate_existing_run_by_id(run_id, "cas_run_snapshot_with_authority")
                .await?;
        } else {
            self.validate_run_by_id_if_present(run_id, "cas_run_snapshot_with_authority")
                .await?;
        }
        Ok(changed)
    }

    async fn cas_run_snapshot_and_append_terminal_event_with_authority(
        &self,
        run_id: &RunId,
        expected_status: MobRunStatus,
        expected_flow_state: &flow_run::State,
        next_status: MobRunStatus,
        next_flow_state: &flow_run::State,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
        events: &dyn MobEventStore,
        terminal_event: NewMobEvent,
    ) -> Result<Option<MobEvent>, MobStoreError> {
        // Forward to the inner store so durable backends keep their atomic
        // single-transaction override (the default implementation would
        // silently re-split the commit).
        let stored = self
            .inner
            .cas_run_snapshot_and_append_terminal_event_with_authority(
                run_id,
                expected_status,
                expected_flow_state,
                next_status,
                next_flow_state,
                authority_inputs,
                events,
                terminal_event,
            )
            .await?;
        if stored.is_some() {
            self.validate_existing_run_by_id(
                run_id,
                "cas_run_snapshot_and_append_terminal_event_with_authority",
            )
            .await?;
        } else {
            self.validate_run_by_id_if_present(
                run_id,
                "cas_run_snapshot_and_append_terminal_event_with_authority",
            )
            .await?;
        }
        Ok(stored)
    }

    async fn append_step_entry_with_authority(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<(), MobStoreError> {
        self.inner
            .append_step_entry_with_authority(run_id, entry, authority)
            .await?;
        self.validate_existing_run_by_id(run_id, "append_step_entry_with_authority")
            .await
    }

    async fn append_step_entry_if_absent_with_authority(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<bool, MobStoreError> {
        let inserted = self
            .inner
            .append_step_entry_if_absent_with_authority(run_id, entry, authority)
            .await?;
        if inserted {
            self.validate_existing_run_by_id(run_id, "append_step_entry_if_absent_with_authority")
                .await?;
        } else {
            self.validate_run_by_id_if_present(
                run_id,
                "append_step_entry_if_absent_with_authority",
            )
            .await?;
        }
        Ok(inserted)
    }

    async fn append_failure_entry_with_authority(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<(), MobStoreError> {
        self.inner
            .append_failure_entry_with_authority(run_id, entry, authority)
            .await?;
        self.validate_existing_run_by_id(run_id, "append_failure_entry_with_authority")
            .await
    }

    async fn append_failure_entry_if_absent_with_authority(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
        authority: MobRunProvenanceAuthority,
    ) -> Result<bool, MobStoreError> {
        let inserted = self
            .inner
            .append_failure_entry_if_absent_with_authority(run_id, entry, authority)
            .await?;
        if inserted {
            self.validate_existing_run_by_id(
                run_id,
                "append_failure_entry_if_absent_with_authority",
            )
            .await?;
        } else {
            self.validate_run_by_id_if_present(
                run_id,
                "append_failure_entry_if_absent_with_authority",
            )
            .await?;
        }
        Ok(inserted)
    }

    async fn cas_frame_state_with_authority(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        expected: Option<&FrameSnapshot>,
        next: FrameSnapshot,
        authority_inputs: Vec<mob_dsl::MobMachineInput>,
    ) -> Result<bool, MobStoreError> {
        let changed = self
            .inner
            .cas_frame_state_with_authority(run_id, frame_id, expected, next, authority_inputs)
            .await?;
        if changed {
            self.validate_existing_run_by_id(run_id, "cas_frame_state_with_authority")
                .await?;
        } else {
            self.validate_run_by_id_if_present(run_id, "cas_frame_state_with_authority")
                .await?;
        }
        Ok(changed)
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
        let changed = self
            .inner
            .cas_grant_node_slot_with_authority(
                run_id,
                expected_run_state,
                next_run_state,
                frame_id,
                expected_frame,
                next_frame,
                authority_inputs,
            )
            .await?;
        if changed {
            self.validate_existing_run_by_id(run_id, "cas_grant_node_slot_with_authority")
                .await?;
        } else {
            self.validate_run_by_id_if_present(run_id, "cas_grant_node_slot_with_authority")
                .await?;
        }
        Ok(changed)
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
        let changed = self
            .inner
            .cas_complete_step_and_record_output_with_authority(
                run_id,
                frame_id,
                expected_frame,
                next_frame,
                step_output_key,
                step_output,
                loop_context,
                authority_inputs,
            )
            .await?;
        if changed {
            self.validate_existing_run_by_id(
                run_id,
                "cas_complete_step_and_record_output_with_authority",
            )
            .await?;
        } else {
            self.validate_run_by_id_if_present(
                run_id,
                "cas_complete_step_and_record_output_with_authority",
            )
            .await?;
        }
        Ok(changed)
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
        let changed = self
            .inner
            .cas_start_loop_with_authority(
                run_id,
                loop_instance_id,
                expected_run_state,
                next_run_state,
                frame_id,
                expected_frame,
                next_frame,
                initial_loop,
                authority_inputs,
            )
            .await?;
        if changed {
            self.validate_existing_run_by_id(run_id, "cas_start_loop_with_authority")
                .await?;
        } else {
            self.validate_run_by_id_if_present(run_id, "cas_start_loop_with_authority")
                .await?;
        }
        Ok(changed)
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
        let changed = self
            .inner
            .cas_loop_request_body_frame_with_authority(
                run_id,
                loop_instance_id,
                expected_loop,
                next_loop,
                expected_run_state,
                next_run_state,
                authority_inputs,
            )
            .await?;
        if changed {
            self.validate_existing_run_by_id(run_id, "cas_loop_request_body_frame_with_authority")
                .await?;
        } else {
            self.validate_run_by_id_if_present(
                run_id,
                "cas_loop_request_body_frame_with_authority",
            )
            .await?;
        }
        Ok(changed)
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
        let changed = self
            .inner
            .cas_grant_body_frame_start_with_authority(
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
            )
            .await?;
        if changed {
            self.validate_existing_run_by_id(run_id, "cas_grant_body_frame_start_with_authority")
                .await?;
        } else {
            self.validate_run_by_id_if_present(run_id, "cas_grant_body_frame_start_with_authority")
                .await?;
        }
        Ok(changed)
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
        let changed = self
            .inner
            .cas_complete_body_frame_with_authority(
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
            )
            .await?;
        if changed {
            self.validate_existing_run_by_id(run_id, "cas_complete_body_frame_with_authority")
                .await?;
        } else {
            self.validate_run_by_id_if_present(run_id, "cas_complete_body_frame_with_authority")
                .await?;
        }
        Ok(changed)
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
        let changed = self
            .inner
            .cas_complete_loop_with_authority(
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
            )
            .await?;
        if changed {
            self.validate_existing_run_by_id(run_id, "cas_complete_loop_with_authority")
                .await?;
        } else {
            self.validate_run_by_id_if_present(run_id, "cas_complete_loop_with_authority")
                .await?;
        }
        Ok(changed)
    }
}

/// Trait for persisting and querying mob specs.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MobSpecStore: Send + Sync {
    /// Put a spec. Returns new revision.
    async fn put_spec(
        &self,
        mob_id: &MobId,
        definition: &MobDefinition,
        revision: Option<u64>,
    ) -> Result<u64, MobStoreError>;

    async fn get_spec(&self, mob_id: &MobId)
    -> Result<Option<(MobDefinition, u64)>, MobStoreError>;
    async fn list_specs(&self) -> Result<Vec<MobId>, MobStoreError>;
    async fn delete_spec(
        &self,
        mob_id: &MobId,
        revision: Option<u64>,
    ) -> Result<bool, MobStoreError>;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod mob_host_authority_record_tests {
    use super::*;

    #[test]
    fn record_serde_roundtrips_and_omits_absent_live_endpoint() {
        let record = sample_mob_host_authority_record("host-peer-1", 3);
        let value = serde_json::to_value(&record).expect("serialize record");
        assert_eq!(value["bind_phase"], serde_json::json!("bound"));
        assert_eq!(
            value["live_endpoint"],
            serde_json::json!("wss://hosts/host-peer-1/live")
        );
        let decoded: MobHostAuthorityRecord = serde_json::from_value(value).expect("decode record");
        assert_eq!(decoded, record);

        let live_incapable = MobHostAuthorityRecord {
            live_endpoint: None,
            ..record
        };
        let value = serde_json::to_value(&live_incapable).expect("serialize record");
        assert!(
            value.get("live_endpoint").is_none(),
            "absent live endpoint must be omitted (absence = live-incapable)"
        );
        let decoded: MobHostAuthorityRecord = serde_json::from_value(value).expect("decode record");
        assert_eq!(decoded, live_incapable);
    }

    #[test]
    fn recover_signal_restores_the_full_bound_host_fact_set() {
        let record = sample_mob_host_authority_record("host-peer-1", 3);
        let mut authority = mob_dsl::MobMachineAuthority::new();
        authority
            .apply_signal(record.dsl_recover_signal())
            .expect("recovery signal must fire on an untracked host");

        let state = authority.state();
        let host = record.dsl_host_id();
        assert!(state.mob_hosts.contains(&host));
        assert_eq!(
            state.host_bind_phase.get(&host),
            Some(&mob_dsl::HostBindPhase::Bound)
        );
        assert_eq!(
            state.host_public_keys.get(&host),
            Some(&record.dsl_signing_key())
        );
        assert_eq!(
            state.host_endpoints.get(&host),
            Some(&record.dsl_endpoint())
        );
        assert_eq!(state.host_authority_epochs.get(&host), Some(&3));
        assert_eq!(state.host_protocol_min.get(&host), Some(&4));
        assert_eq!(state.host_protocol_max.get(&host), Some(&4));
        assert_eq!(
            state.host_engine_versions.get(&host),
            Some(&"0.7.22-test".to_string())
        );
        assert_eq!(state.host_durable_sessions.get(&host), Some(&true));
        assert_eq!(state.host_autonomous_members.get(&host), Some(&true));
        assert_eq!(state.host_hard_cancel_member.get(&host), Some(&false));
        assert_eq!(state.host_tracked_input_cancel.get(&host), Some(&true));
        assert_eq!(state.host_memory_store.get(&host), Some(&true));
        assert_eq!(state.host_mcp.get(&host), Some(&true));
        assert_eq!(state.host_approval_forwarding.get(&host), Some(&false));
        assert!(
            state
                .host_resolvable_providers
                .get(&host)
                .expect("providers recovered")
                .contains("anthropic")
        );
        assert_eq!(
            state.host_live_endpoints.get(&host),
            Some(&mob_dsl::LiveWsEndpointUrl(
                "wss://hosts/host-peer-1/live".to_string()
            ))
        );

        // Double recovery for the same host fails loud on the guard.
        assert!(
            authority.apply_signal(record.dsl_recover_signal()).is_err(),
            "recovering an already-tracked host must be rejected"
        );
    }

    #[test]
    fn persistence_authority_is_transition_witnessed() {
        let record = sample_mob_host_authority_record("host-peer-1", 3);
        let authority = mob_host_authority_persistence_authority_for_record(&record)
            .expect("commit-witnessed persistence authority");
        authority.verify_record(&record).expect("record matches");

        // Wrong epoch: the witness refuses the record.
        let stale = MobHostAuthorityRecord {
            authority_epoch: 4,
            ..record.clone()
        };
        assert!(authority.verify_record(&stale).is_err());

        // Wrong host: refused.
        let other = sample_mob_host_authority_record("host-peer-2", 3);
        assert!(authority.verify_record(&other).is_err());

        // peer_id drifting away from host_id is refused (identity-first,
        // one fact — the split cannot silently diverge).
        let drifted = MobHostAuthorityRecord {
            peer_id: "some-other-peer".to_string(),
            ..record
        };
        assert!(authority.verify_record(&drifted).is_err());
    }

    #[test]
    fn persistence_authority_witnesses_rebind_transitions() {
        let bound = sample_mob_host_authority_record("host-peer-1", 3);
        let mut authority = mob_dsl::MobMachineAuthority::new();
        authority
            .apply_signal(bound.dsl_recover_signal())
            .expect("seed recovered binding");
        let rebound_record = MobHostAuthorityRecord {
            authority_epoch: 4,
            ..bound
        };
        let transition = mob_dsl::MobMachineMutator::apply(
            &mut authority,
            mob_dsl::MobMachineInput::HostRebound {
                host_id: rebound_record.dsl_host_id(),
                epoch: 4,
                binding_generation: rebound_record.binding_generation,
                protocol_min: rebound_record.capabilities.protocol_min,
                protocol_max: rebound_record.capabilities.protocol_max,
                engine_version: rebound_record.capabilities.engine_version.clone(),
                durable_sessions: rebound_record.capabilities.durable_sessions,
                autonomous_members: rebound_record.capabilities.autonomous_members,
                hard_cancel_member: rebound_record.capabilities.hard_cancel_member,
                tracked_input_cancel: rebound_record.capabilities.tracked_input_cancel,
                memory_store: rebound_record.capabilities.memory_store,
                mcp: rebound_record.capabilities.mcp,
                resolvable_providers: rebound_record.capabilities.resolvable_providers.clone(),
                approval_forwarding: rebound_record.capabilities.approval_forwarding,
                live_endpoint: rebound_record
                    .live_endpoint
                    .clone()
                    .map(mob_dsl::LiveWsEndpointUrl::from),
            },
        )
        .expect("rebind transition fires");
        let witness =
            MobHostAuthorityPersistenceAuthority::from_transition(&rebound_record, &transition)
                .expect("HostReboundRecorded witnesses the rebound record");
        witness
            .verify_record(&rebound_record)
            .expect("record matches");
    }

    #[test]
    fn deletion_authority_requires_the_revoke_effect() {
        let record = sample_mob_host_authority_record("host-peer-1", 3);
        let deletion = mob_host_authority_deletion_authority_for_record(&record)
            .expect("revoke-witnessed deletion authority");
        deletion.verify_record(&record).expect("record matches");

        let other = sample_mob_host_authority_record("host-peer-2", 3);
        assert!(deletion.verify_record(&other).is_err());

        // A non-revoke transition can never mint a deletion authority.
        let commit = mob_host_authority_persistence_authority_for_record(&record).is_ok();
        assert!(commit, "sanity: commit witness exists");
        let mut authority = mob_dsl::MobMachineAuthority::new();
        authority
            .apply_signal(record.dsl_recover_signal())
            .expect("seed recovered binding");
        let refresh = mob_dsl::MobMachineMutator::apply(
            &mut authority,
            mob_dsl::MobMachineInput::RefreshHostCapabilities {
                host_id: record.dsl_host_id(),
                epoch: record.authority_epoch,
                binding_generation: record.binding_generation,
                protocol_min: record.capabilities.protocol_min,
                protocol_max: record.capabilities.protocol_max,
                engine_version: record.capabilities.engine_version.clone(),
                durable_sessions: record.capabilities.durable_sessions,
                autonomous_members: record.capabilities.autonomous_members,
                hard_cancel_member: record.capabilities.hard_cancel_member,
                tracked_input_cancel: record.capabilities.tracked_input_cancel,
                memory_store: record.capabilities.memory_store,
                mcp: record.capabilities.mcp,
                resolvable_providers: record.capabilities.resolvable_providers.clone(),
                approval_forwarding: record.capabilities.approval_forwarding,
                live_endpoint: record
                    .live_endpoint
                    .clone()
                    .map(mob_dsl::LiveWsEndpointUrl::from),
            },
        )
        .expect("refresh transition fires");
        assert!(
            MobHostAuthorityDeletionAuthority::from_transition(&record, &refresh).is_err(),
            "a refresh transition must not mint a deletion authority"
        );
        MobHostAuthorityPersistenceAuthority::from_transition(&record, &refresh)
            .expect("exact full host refresh must witness the corresponding durable record");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod mob_operator_grant_record_tests {
    use super::*;

    fn sample_grant_record() -> MobOperatorGrantRecord {
        MobOperatorGrantRecord {
            principal: "console:luka".to_string(),
            scopes: vec![WireControlScope::List, WireControlScope::SendCommand],
            expires_at_ms: Some(1_234_567),
        }
    }

    #[test]
    fn record_serde_roundtrips_with_wire_scope_spelling() {
        let record = sample_grant_record();
        let value = serde_json::to_value(&record).expect("serialize record");
        assert_eq!(
            value["scopes"],
            serde_json::json!(["list", "send_command"]),
            "scopes serialize as the pinned wire snake_case spelling"
        );
        assert_eq!(value["expires_at_ms"], serde_json::json!(1_234_567));
        let decoded: MobOperatorGrantRecord = serde_json::from_value(value).expect("decode record");
        assert_eq!(decoded, record);

        let unexpiring = MobOperatorGrantRecord {
            expires_at_ms: None,
            ..record
        };
        let value = serde_json::to_value(&unexpiring).expect("serialize record");
        assert!(
            value.get("expires_at_ms").is_none(),
            "absent expiry must be omitted (absence = never expires)"
        );
        let decoded: MobOperatorGrantRecord = serde_json::from_value(value).expect("decode record");
        assert_eq!(decoded, unexpiring);
    }

    #[test]
    fn persistence_authority_is_grant_transition_witnessed() {
        let record = sample_grant_record();
        let authority = mob_operator_grant_persistence_authority_for_record(&record)
            .expect("grant-witnessed persistence authority");
        authority.verify_record(&record).expect("record matches");

        // Wrong principal: refused.
        let other_principal = MobOperatorGrantRecord {
            principal: "console:other".to_string(),
            ..record.clone()
        };
        assert!(authority.verify_record(&other_principal).is_err());

        // Wrong scope set: refused.
        let other_scopes = MobOperatorGrantRecord {
            scopes: vec![WireControlScope::AdminGrants],
            ..record.clone()
        };
        assert!(authority.verify_record(&other_scopes).is_err());

        // Wrong expiry: refused (raw data is witness-checked verbatim).
        let other_expiry = MobOperatorGrantRecord {
            expires_at_ms: None,
            ..record
        };
        assert!(authority.verify_record(&other_expiry).is_err());
    }

    #[test]
    fn partial_revoke_witnesses_the_rewrite_and_validates_retained_expiry() {
        let record = sample_grant_record();
        let mut authority = mob_dsl::MobMachineAuthority::new();
        mob_dsl::MobMachineMutator::apply(&mut authority, record.dsl_grant_input())
            .expect("grant accepted");
        let transition = mob_dsl::MobMachineMutator::apply(
            &mut authority,
            mob_dsl::MobMachineInput::RevokeOperatorScopes {
                principal: record.dsl_principal(),
                revoked: std::collections::BTreeSet::from([mob_dsl::ControlScope::SendCommand]),
                remaining: std::collections::BTreeSet::from([mob_dsl::ControlScope::List]),
            },
        )
        .expect("partial revoke accepted");
        let post_state = authority.state().clone();

        // The rewritten record: remaining scopes, expiry retained.
        let rewritten = MobOperatorGrantRecord {
            principal: record.principal.clone(),
            scopes: vec![WireControlScope::List],
            expires_at_ms: record.expires_at_ms,
        };
        let witness = MobOperatorGrantPersistenceAuthority::from_revoke_transition(
            &rewritten,
            &transition,
            &post_state,
        )
        .expect("partial revoke witnesses the rewritten record");
        witness.verify_record(&rewritten).expect("record matches");

        // A record claiming a DIFFERENT expiry than the machine's retained
        // row is refused: the post-state is the truth the permit checks.
        let drifted_expiry = MobOperatorGrantRecord {
            expires_at_ms: Some(9),
            ..rewritten.clone()
        };
        assert!(
            MobOperatorGrantPersistenceAuthority::from_revoke_transition(
                &drifted_expiry,
                &transition,
                &post_state,
            )
            .is_err()
        );

        // A partial-revoke transition can never mint a deletion authority.
        assert!(
            MobOperatorGrantDeletionAuthority::from_transition(
                rewritten.principal.as_str(),
                &transition
            )
            .is_err(),
            "a partial revoke must not witness a record deletion"
        );

        // Nor can it mint a grant-path persistence authority (no
        // GrantRecorded effect).
        assert!(
            MobOperatorGrantPersistenceAuthority::from_transition(&rewritten, &transition).is_err()
        );
    }

    #[test]
    fn deletion_authority_requires_the_full_revoke_effect() {
        let record = sample_grant_record();
        let deletion = mob_operator_grant_deletion_authority_for_record(&record)
            .expect("full-revoke-witnessed deletion authority");
        deletion
            .verify_principal(record.principal.as_str())
            .expect("principal matches");
        assert!(deletion.verify_principal("console:other").is_err());

        // A grant transition can never mint a deletion authority.
        let mut authority = mob_dsl::MobMachineAuthority::new();
        let grant_transition =
            mob_dsl::MobMachineMutator::apply(&mut authority, record.dsl_grant_input())
                .expect("grant accepted");
        assert!(
            MobOperatorGrantDeletionAuthority::from_transition(
                record.principal.as_str(),
                &grant_transition
            )
            .is_err(),
            "a grant transition must not mint a deletion authority"
        );

        // The absent-grant no-op revoke emits nothing: no witness at all.
        let mut fresh = mob_dsl::MobMachineAuthority::new();
        let noop = mob_dsl::MobMachineMutator::apply(
            &mut fresh,
            mob_dsl::MobMachineInput::RevokeOperatorScopes {
                principal: mob_dsl::PrincipalId::from("console:absent"),
                revoked: std::collections::BTreeSet::new(),
                remaining: std::collections::BTreeSet::new(),
            },
        )
        .expect("absent-grant revoke is a machine no-op");
        assert!(
            MobOperatorGrantDeletionAuthority::from_transition("console:absent", &noop).is_err(),
            "the no-op arm emits nothing and must not witness a deletion"
        );
    }

    #[test]
    fn recovery_input_replays_the_record_verbatim() {
        let record = sample_grant_record();
        let mut authority = mob_dsl::MobMachineAuthority::new();
        mob_dsl::MobMachineMutator::apply(&mut authority, record.dsl_grant_input())
            .expect("recovery grant input accepted on a fresh Running machine");
        let key = record.dsl_principal();
        assert_eq!(
            authority.state().operator_grant_scopes.get(&key),
            Some(&record.control_scope_set())
        );
        assert_eq!(
            authority.state().operator_grant_expiries.get(&key),
            Some(&record.expires_at_ms),
            "expiry replays verbatim; only the enforcement seam evaluates it"
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod runtime_metadata_store_compatibility_tests {
    use super::*;

    struct LegacyRuntimeMetadataStore;

    #[async_trait]
    impl MobRuntimeMetadataStore for LegacyRuntimeMetadataStore {
        async fn load_supervisor_authority(
            &self,
            _mob_id: &MobId,
        ) -> Result<Option<SupervisorAuthorityRecord>, MobStoreError> {
            Ok(None)
        }

        async fn put_supervisor_authority(
            &self,
            _mob_id: &MobId,
            _record: &SupervisorAuthorityRecord,
            _authority: &SupervisorAuthorityPersistenceAuthority,
        ) -> Result<(), MobStoreError> {
            Ok(())
        }

        async fn compare_and_put_supervisor_authority(
            &self,
            _mob_id: &MobId,
            _expected: &SupervisorAuthorityRecord,
            _record: &SupervisorAuthorityRecord,
            _authority: &SupervisorAuthorityPersistenceAuthority,
        ) -> Result<bool, MobStoreError> {
            Ok(false)
        }

        async fn put_supervisor_authority_if_absent(
            &self,
            _mob_id: &MobId,
            _record: &SupervisorAuthorityRecord,
            _authority: &SupervisorAuthorityPersistenceAuthority,
        ) -> Result<bool, MobStoreError> {
            Ok(false)
        }

        async fn delete_supervisor_authority(
            &self,
            _mob_id: &MobId,
            _expected: &SupervisorAuthorityRecord,
            _authority: &SupervisorAuthorityDeletionAuthority,
        ) -> Result<bool, MobStoreError> {
            Ok(false)
        }

        async fn list_external_binding_overlays(
            &self,
            _mob_id: &MobId,
        ) -> Result<Vec<ExternalBindingOverlayRecord>, MobStoreError> {
            Ok(Vec::new())
        }

        async fn put_external_binding_overlay_if_absent(
            &self,
            _mob_id: &MobId,
            _record: &ExternalBindingOverlayRecord,
        ) -> Result<bool, MobStoreError> {
            Ok(false)
        }

        async fn upsert_external_binding_overlay(
            &self,
            _mob_id: &MobId,
            _record: &ExternalBindingOverlayRecord,
        ) -> Result<(), MobStoreError> {
            Ok(())
        }

        async fn delete_external_binding_overlay(
            &self,
            _mob_id: &MobId,
            _agent_identity: &AgentIdentity,
            _generation: Generation,
        ) -> Result<(), MobStoreError> {
            Ok(())
        }

        async fn delete_external_binding_overlays(
            &self,
            _mob_id: &MobId,
        ) -> Result<(), MobStoreError> {
            Ok(())
        }
    }

    fn assert_capability_unavailable<T>(result: Result<T, MobStoreError>, operation: &str) {
        assert!(
            matches!(
                result,
                Err(MobStoreError::Internal(message))
                    if message.contains(operation) && message.contains("does not implement")
            ),
            "legacy metadata store must fail closed for {operation}"
        );
    }

    #[tokio::test]
    async fn legacy_runtime_metadata_implementors_compile_and_new_capabilities_fail_closed() {
        let store = LegacyRuntimeMetadataStore;
        let mob_id = MobId::from("legacy-mob");

        assert_capability_unavailable(
            store.load_mob_host_authority(&mob_id, "host-b").await,
            "load_mob_host_authority",
        );
        assert_capability_unavailable(
            store.list_member_operator_requests(&mob_id).await,
            "list_member_operator_requests",
        );
        assert_capability_unavailable(
            store.list_placed_spawns(&mob_id).await,
            "list_placed_spawns",
        );
        assert_capability_unavailable(
            store.list_mob_operator_grants(&mob_id).await,
            "list_mob_operator_grants",
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod supervisor_rotation_migration_tests {
    use super::*;

    #[test]
    fn recovery_rejects_wrapped_pending_supervisor_epoch() {
        let mut current = SupervisorAuthorityRecord::generate(BridgeProtocolVersion::V4);
        current.epoch = u64::MAX;
        let mut wrapped = SupervisorAuthorityRecord::generate(BridgeProtocolVersion::V4);
        wrapped.epoch = 0;
        current.pending_rotation = Some(SupervisorPendingRotationRecord::from_authority(
            &wrapped,
            SupervisorRotationOperationId::new(),
            Vec::new(),
            std::collections::BTreeMap::new(),
        ));

        let mut authority = mob_dsl::MobMachineAuthority::new();
        assert!(
            authority
                .apply_signal(current.dsl_recover_signal())
                .is_err(),
            "recovery must never treat epoch zero as the successor of u64::MAX"
        );
    }

    #[tokio::test]
    async fn legacy_pending_rotation_without_operation_id_is_cas_migrated_once() {
        let mut current = SupervisorAuthorityRecord::generate(BridgeProtocolVersion::V4);
        let mut next = SupervisorAuthorityRecord::generate(BridgeProtocolVersion::V4);
        next.epoch = current.epoch + 1;
        current.pending_rotation = Some(SupervisorPendingRotationRecord::from_authority(
            &next,
            SupervisorRotationOperationId::new(),
            vec![
                "peer-already-complete".to_string(),
                "peer-retired-before-upgrade".to_string(),
            ],
            std::collections::BTreeMap::new(),
        ));

        let mut legacy_json = serde_json::to_value(&current).expect("serialize authority");
        let legacy_pending = legacy_json
            .get_mut("pending_rotation")
            .and_then(serde_json::Value::as_object_mut)
            .expect("pending rotation object");
        legacy_pending.remove("operation_id");
        legacy_pending.remove("member_targets");
        let recovered: SupervisorAuthorityRecord =
            serde_json::from_value(legacy_json).expect("legacy record must deserialize");
        assert_eq!(
            recovered
                .pending_rotation
                .as_ref()
                .and_then(|pending| pending.operation_id),
            None
        );

        let mut authority =
            mob_dsl::MobMachineAuthority::recover_from_state(mob_dsl::MobMachineState::default())
                .expect("initialize authority");
        authority
            .apply_signal(recovered.dsl_recover_signal())
            .expect("legacy pending target must recover without inventing an id");
        assert_eq!(
            authority.state().supervisor_pending_authority_operation_id,
            None
        );

        let store = in_memory::InMemoryMobRuntimeMetadataStore::new();
        let mob_id = MobId::from("legacy-supervisor-operation-migration");
        store
            .seed_legacy_supervisor_authority(&mob_id, recovered.clone())
            .await;

        let operation_id = SupervisorRotationOperationId::new();
        let mut migrated = recovered.clone();
        let pending = migrated
            .pending_rotation
            .as_mut()
            .expect("pending rotation remains present");
        pending.operation_id = Some(operation_id);
        pending
            .accepted_peer_ids
            .retain(|peer_id| peer_id == "peer-already-complete");
        let pending_peer_id = pending.public_peer_id.clone();
        let pending_pubkey = pending.public_signing_key();
        pending.member_targets.insert(
            "peer-already-complete".to_string(),
            BridgePeerSpec {
                name: "legacy-supervisor".to_string(),
                peer_id: pending_peer_id,
                address: "inproc://legacy-supervisor".to_string(),
                pubkey: pending_pubkey,
            },
        );
        let pending = pending.clone();
        let active_peer_ids = ["peer-already-complete".to_string()].into_iter().collect();
        let mut invalid_prune = migrated.clone();
        invalid_prune
            .pending_rotation
            .as_mut()
            .expect("invalid migration pending rotation")
            .accepted_peer_ids
            .clear();
        let invalid_pending = invalid_prune
            .pending_rotation
            .as_ref()
            .expect("invalid migration pending rotation")
            .clone();
        assert!(
            mob_dsl::MobMachineMutator::apply(
                &mut authority,
                invalid_prune.dsl_record_pending_rotation_input(&invalid_pending, &active_peer_ids),
            )
            .is_err(),
            "migration must not let the shell prune an accepted peer that remains active"
        );
        let transition = mob_dsl::MobMachineMutator::apply(
            &mut authority,
            migrated.dsl_record_pending_rotation_input(&pending, &active_peer_ids),
        )
        .expect(
            "migration must prune inactive legacy evidence and install one stable operation id",
        );
        let persistence_authority =
            SupervisorAuthorityPersistenceAuthority::from_transition(&migrated, &transition)
                .expect("migration transition must authorize the exact persisted record");
        assert!(
            store
                .compare_and_put_supervisor_authority(
                    &mob_id,
                    &recovered,
                    &migrated,
                    &persistence_authority,
                )
                .await
                .expect("migration CAS must succeed")
        );

        let persisted = store
            .load_supervisor_authority(&mob_id)
            .await
            .expect("load migrated authority")
            .expect("migrated authority remains stored");
        assert_eq!(persisted, migrated);
        assert_eq!(
            persisted
                .pending_rotation
                .as_ref()
                .map(|pending| pending.accepted_peer_ids.as_slice()),
            Some(["peer-already-complete".to_string()].as_slice())
        );
        assert_eq!(
            persisted
                .pending_rotation
                .as_ref()
                .and_then(|pending| pending.operation_id),
            Some(operation_id)
        );
        let expected_operation_id = operation_id.to_string();
        assert_eq!(
            authority
                .state()
                .supervisor_pending_authority_operation_id
                .as_deref(),
            Some(expected_operation_id.as_str())
        );
    }
}
