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
use crate::event::{MemberRef, MobEvent, NewMobEvent};
use crate::ids::{
    AgentIdentity, FlowId, FrameId, Generation, LoopId, LoopInstanceId, MobId, RunId, StepId,
};
use crate::run::flow_run;
use crate::run::{
    FailureLedgerEntry, FrameSnapshot, LoopIterationLedgerEntry, LoopSnapshot, MobRun,
    MobRunStatus, StepLedgerEntry,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use meerkat_contracts::wire::supervisor_bridge::BridgeBootstrapToken;
use serde::{Deserialize, Serialize};

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
    pub protocol_version: u32,
}

impl SupervisorAuthorityRecord {
    /// Mint a fresh supervisor authority record.
    pub fn generate(protocol_version: u32) -> Self {
        let keypair = meerkat_comms::Keypair::generate();
        Self {
            secret_key: keypair.secret_bytes(),
            public_peer_id: keypair.public_key().to_peer_id().as_str(),
            epoch: 0,
            protocol_version,
        }
    }

    /// Reconstruct the signing keypair for runtime use.
    pub fn keypair(&self) -> meerkat_comms::Keypair {
        meerkat_comms::Keypair::from_secret(self.secret_key)
    }
}

/// Normalization status for a legacy external binding overlay.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExternalBindingOverlayStatus {
    /// The legacy external binding was normalized to a peer-only member ref.
    Normalized,
    /// Normalization failed and the member should surface as broken.
    Failed { reason: String },
}

/// Authoritative runtime metadata for an external binding overlay.
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
pub trait MobEventStore: Send + Sync {
    /// Append a new event to the store.
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobStoreError>;

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
    ) -> Result<(), MobStoreError>;

    /// Insert the mob-owned supervisor authority record if it is missing.
    ///
    /// Returns `true` when the caller won initialization, `false` when an
    /// existing record already owned the key.
    async fn put_supervisor_authority_if_absent(
        &self,
        mob_id: &MobId,
        record: &SupervisorAuthorityRecord,
    ) -> Result<bool, MobStoreError>;

    /// Delete the mob-owned supervisor authority record.
    async fn delete_supervisor_authority(&self, mob_id: &MobId) -> Result<(), MobStoreError>;

    /// List all authoritative external binding overlays for a mob.
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
    async fn cas_run_status(
        &self,
        run_id: &RunId,
        expected: MobRunStatus,
        next: MobRunStatus,
    ) -> Result<bool, MobStoreError>;
    async fn cas_flow_state(
        &self,
        run_id: &RunId,
        expected: &flow_run::State,
        next: &flow_run::State,
    ) -> Result<bool, MobStoreError>;
    async fn cas_run_snapshot(
        &self,
        run_id: &RunId,
        expected_status: MobRunStatus,
        expected_flow_state: &flow_run::State,
        next_status: MobRunStatus,
        next_flow_state: &flow_run::State,
    ) -> Result<bool, MobStoreError>;
    async fn append_step_entry(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
    ) -> Result<(), MobStoreError>;
    async fn append_step_entry_if_absent(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
    ) -> Result<bool, MobStoreError>;
    async fn put_step_output(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        output: serde_json::Value,
    ) -> Result<(), MobStoreError>;
    async fn append_failure_entry(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
    ) -> Result<(), MobStoreError>;

    /// Upsert a loop snapshot. Creates or overwrites the entry for `loop_instance_id`
    /// in `run.loops` and optionally records a `LoopIterationLedgerEntry`.
    ///
    /// Used by the sequential `FlowFrameEngine` to persist loop state so that
    /// `reconcile_run_state` can reconstruct in-progress loops after a crash.
    ///
    /// Implementations must treat `ledger_entry` as idempotent by logical
    /// iteration identity. Replaying the same `(loop_instance_id, iteration, frame_id)`
    /// on resume must not append a duplicate row.
    async fn upsert_loop_snapshot(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        snapshot: LoopSnapshot,
        ledger_entry: Option<LoopIterationLedgerEntry>,
    ) -> Result<(), MobStoreError>;

    // Phase 3: CAS wrappers for frame and loop state.

    /// CAS wrapper 1: frame state update.
    ///
    /// If `expected` is `None`, this is an insert (frame must not yet exist).
    /// If `expected` is `Some(snapshot)`, the current frame state must match.
    /// Returns `Ok(true)` on success, `Ok(false)` on mismatch.
    ///
    /// Implementations must atomically compare and replace the frame snapshot.
    /// Backends that cannot make that guarantee must return
    /// [`MobStoreError::FrameAtomicPersistenceUnavailable`] for this operation.
    async fn cas_frame_state(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        expected: Option<&FrameSnapshot>,
        next: FrameSnapshot,
    ) -> Result<bool, MobStoreError>;

    /// CAS wrapper 2: grant node slot — atomically update run flow state + frame state.
    ///
    /// Implementations must commit the run-state and frame-state changes in one
    /// atomic unit. Backends that cannot make that guarantee must return
    /// [`MobStoreError::FrameAtomicPersistenceUnavailable`].
    async fn cas_grant_node_slot(
        &self,
        run_id: &RunId,
        expected_run_state: &flow_run::State,
        next_run_state: flow_run::State,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
    ) -> Result<bool, MobStoreError>;

    /// CAS wrapper 3: complete step — update frame state and record step output.
    ///
    /// When `loop_context` is `None`, the output is stored in `root_step_outputs`.
    /// When `loop_context` is `Some((loop_id, iteration))`, the output is stored
    /// in `loop_iteration_outputs[loop_id][iteration]`.
    ///
    /// Implementations must commit the frame-state update and step-output write
    /// in one atomic unit. Backends that cannot make that guarantee must return
    /// [`MobStoreError::FrameAtomicPersistenceUnavailable`].
    #[allow(clippy::too_many_arguments)]
    async fn cas_complete_step_and_record_output(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
        step_output_key: String,
        step_output: serde_json::Value,
        loop_context: Option<(&LoopId, u64)>,
    ) -> Result<bool, MobStoreError>;

    /// CAS wrapper 4: start loop — register loop + update run state + parent frame.
    ///
    /// Implementations must register the loop and commit the run/frame updates
    /// in one atomic unit. Backends that cannot make that guarantee must return
    /// [`MobStoreError::FrameAtomicPersistenceUnavailable`].
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
    ) -> Result<bool, MobStoreError>;

    /// CAS wrapper 5: register pending body frame — loop transition + run state update.
    ///
    /// Implementations must commit the loop transition and run-state update in
    /// one atomic unit. Backends that cannot make that guarantee must return
    /// [`MobStoreError::FrameAtomicPersistenceUnavailable`].
    async fn cas_loop_request_body_frame(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        expected_loop: &LoopSnapshot,
        next_loop: LoopSnapshot,
        expected_run_state: &flow_run::State,
        next_run_state: flow_run::State,
    ) -> Result<bool, MobStoreError>;

    /// CAS wrapper 6: body frame start — loop transition + register new frame + run state update.
    ///
    /// Implementations must commit the loop transition, frame registration, and
    /// run-state update in one atomic unit. Backends that cannot make that
    /// guarantee must return [`MobStoreError::FrameAtomicPersistenceUnavailable`].
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
    ) -> Result<bool, MobStoreError>;

    /// CAS wrapper 7: body frame completion — terminalize frame + loop state update + run state.
    ///
    /// Implementations must commit the frame terminalization, loop update, and
    /// run-state update in one atomic unit. Backends that cannot make that
    /// guarantee must return [`MobStoreError::FrameAtomicPersistenceUnavailable`].
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
    ) -> Result<bool, MobStoreError>;

    /// CAS wrapper 8: loop completion — loop state + run state + parent frame update.
    ///
    /// Implementations must commit the loop state, run state, and parent-frame
    /// update in one atomic unit. Backends that cannot make that guarantee must
    /// return [`MobStoreError::FrameAtomicPersistenceUnavailable`].
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
    ) -> Result<bool, MobStoreError>;
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
