//! Mob store traits and implementations.

mod in_memory;
mod realm_profile;
#[cfg(not(target_arch = "wasm32"))]
mod sqlite;

pub use in_memory::{
    InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobRuntimeMetadataStore,
    InMemoryMobSpecStore, InMemoryRealmProfileStore,
};
pub use realm_profile::{RealmProfileStore, StoredRealmProfile};
#[cfg(not(target_arch = "wasm32"))]
pub use sqlite::{
    SqliteMobEventStore, SqliteMobRunStore, SqliteMobRuntimeMetadataStore, SqliteMobSpecStore,
    SqliteMobStores, SqliteRealmProfileStore,
};

use crate::definition::MobDefinition;
use crate::event::{MemberRef, MobEvent, MobEventKind, NewMobEvent};
use crate::ids::{
    AgentIdentity, FlowId, FrameId, Generation, LoopId, LoopInstanceId, MobId, RunId, StepId,
};
use crate::machines::mob_machine as mob_dsl;
use crate::run::flow_run;
use crate::run::{
    FailureLedgerEntry, FrameSnapshot, LoopIterationLedgerEntry, LoopSnapshot, MobRun,
    MobRunProvenanceAuthority, MobRunStatus, StepLedgerEntry,
};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use meerkat_contracts::wire::supervisor_bridge::{BridgeBootstrapToken, BridgeProtocolVersion};
use serde::{Deserialize, Serialize};
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

    /// Serialization or deserialization failed.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
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
            pending_peer_id: pending.map(SupervisorPendingRotationRecord::dsl_peer_id),
            pending_signing_key: pending.map(SupervisorPendingRotationRecord::dsl_signing_key),
            pending_epoch: pending.map(|pending| pending.epoch),
            pending_protocol_version: pending
                .map(SupervisorPendingRotationRecord::dsl_protocol_version),
            pending_accepted_peer_ids: pending
                .map(SupervisorPendingRotationRecord::dsl_accepted_peer_ids)
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

    pub fn dsl_clear_pending_rotation_input(&self) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::ClearSupervisorPendingRotation {
            current_peer_id: self.dsl_peer_id(),
            current_epoch: self.epoch,
            protocol_version: self.dsl_protocol_version(),
        }
    }

    pub fn dsl_record_pending_rotation_input(
        &self,
        pending: &SupervisorPendingRotationRecord,
    ) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::RecordSupervisorPendingRotation {
            current_peer_id: self.dsl_peer_id(),
            current_epoch: self.epoch,
            pending_peer_id: pending.dsl_peer_id(),
            pending_signing_key: pending.dsl_signing_key(),
            pending_epoch: pending.epoch,
            protocol_version: pending.dsl_protocol_version(),
            accepted_peer_ids: pending.dsl_accepted_peer_ids(),
        }
    }

    pub fn dsl_commit_rotation_input(
        &self,
        next: &SupervisorAuthorityRecord,
    ) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::CommitSupervisorRotation {
            current_peer_id: self.dsl_peer_id(),
            current_epoch: self.epoch,
            next_peer_id: next.dsl_peer_id(),
            next_signing_key: next.dsl_signing_key(),
            next_epoch: next.epoch,
            protocol_version: next.dsl_protocol_version(),
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
            pending_peer_id: pending.map(SupervisorPendingRotationRecord::dsl_peer_id),
            pending_signing_key: pending.map(SupervisorPendingRotationRecord::dsl_signing_key),
            pending_epoch: pending.map(|pending| pending.epoch),
            pending_protocol_version: pending
                .map(SupervisorPendingRotationRecord::dsl_protocol_version),
            pending_accepted_peer_ids: pending
                .map(SupervisorPendingRotationRecord::dsl_accepted_peer_ids)
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
        accepted_peer_ids: Vec<String>,
    ) -> Self {
        Self {
            secret_key: authority.secret_key,
            public_peer_id: authority.public_peer_id.clone(),
            epoch: authority.epoch,
            protocol_version: authority.protocol_version,
            accepted_peer_ids,
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

    pub fn same_attempted_authority(&self, other: &Self) -> bool {
        self.secret_key == other.secret_key
            && self.public_peer_id == other.public_peer_id
            && self.epoch == other.epoch
            && self
                .protocol_version
                .same_protocol_as(other.protocol_version)
    }

    pub fn remove_accepted_peer_ids(&mut self, peer_ids: &[String]) -> bool {
        let original_len = self.accepted_peer_ids.len();
        self.accepted_peer_ids
            .retain(|peer_id| !peer_ids.iter().any(|candidate| candidate == peer_id));
        self.accepted_peer_ids.len() != original_len
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
    pending_accepted_peer_ids: std::collections::BTreeSet<mob_dsl::PeerId>,
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
            pending_peer_id,
            pending_signing_key,
            pending_epoch,
            effect_pending_protocol,
            pending_accepted_peer_ids,
        )) = transition.effects().iter().find_map(|effect| match effect {
            mob_dsl::MobMachineEffect::PersistSupervisorAuthority {
                peer_id,
                signing_key,
                epoch,
                protocol_version,
                pending_peer_id,
                pending_signing_key,
                pending_epoch,
                pending_protocol_version,
                pending_accepted_peer_ids,
            } => Some((
                peer_id.clone(),
                *signing_key,
                *epoch,
                protocol_version.clone(),
                pending_peer_id.clone(),
                *pending_signing_key,
                *pending_epoch,
                pending_protocol_version.clone(),
                pending_accepted_peer_ids.clone(),
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
            pending_accepted_peer_ids,
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
        let pending_accepted_peer_ids = pending
            .map(SupervisorPendingRotationRecord::dsl_accepted_peer_ids)
            .unwrap_or_default();
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
            && self.pending_accepted_peer_ids == pending_accepted_peer_ids
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
