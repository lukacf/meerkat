//! RuntimeStore — atomic persistence for runtime state.
//!
//! Machine-owned runtime commands durably persist [`RunBoundaryReceipt`] values
//! atomically with their session and input-state effects.

pub mod memory;
#[cfg(feature = "sqlite-store")]
pub mod sqlite;

use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};

use crate::identifiers::LogicalRuntimeId;
use crate::input_state::{InputStatePersistenceRecord, StoredInputState};
use crate::runtime_state::RuntimeState;

const MACHINE_LIFECYCLE_STORE_RECORD_VERSION: u16 = 1;

/// Errors from RuntimeStore operations.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum RuntimeStoreError {
    /// Write failed.
    #[error("Store write failed: {0}")]
    WriteFailed(String),
    /// Read failed.
    #[error("Store read failed: {0}")]
    ReadFailed(String),
    /// The explicit session-store key does not match the serialized session.
    #[error("Session store key mismatch: expected {expected}, actual {actual}")]
    SessionKeyMismatch {
        expected: meerkat_core::types::SessionId,
        actual: meerkat_core::types::SessionId,
    },
    /// Not found.
    #[error("Not found: {0}")]
    NotFound(String),
    /// Operation is not supported by this store implementation.
    #[error("Unsupported store operation: {0}")]
    Unsupported(String),
    /// Runtime snapshot CAS rejected a stale transcript rewrite.
    #[error("Transcript revision conflict: expected {expected}, actual {actual}")]
    TranscriptRevisionConflict { expected: String, actual: String },
    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Transactional updater for the runtime-owned OAuth login-flow payload snapshot.
pub type AuthOAuthFlowSnapshotUpdate<'a> =
    dyn FnMut(Option<&[u8]>) -> Result<Vec<u8>, RuntimeStoreError> + 'a;

/// Describes a serialized session snapshot for boundary and snapshot-only commits.
#[derive(Debug, Clone)]
pub struct SessionDelta {
    /// Serialized session snapshot (opaque to RuntimeStore).
    pub session_snapshot: Vec<u8>,
}

/// Runtime binding facts selected by generated MeerkatMachine authority.
///
/// RuntimeStore implementations persist and read these facts as part of a
/// machine lifecycle snapshot. The commit token that writes these facts stays
/// crate-private so compatibility callers cannot mint replacement lifecycle
/// truth.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MachineLifecycleBindingFacts {
    agent_runtime_id: Option<String>,
    fence_token: Option<u64>,
    runtime_generation: Option<u64>,
    runtime_epoch_id: Option<String>,
}

impl MachineLifecycleBindingFacts {
    pub(crate) fn new(
        agent_runtime_id: Option<String>,
        fence_token: Option<u64>,
        runtime_generation: Option<u64>,
        runtime_epoch_id: Option<String>,
    ) -> Self {
        Self {
            agent_runtime_id,
            fence_token,
            runtime_generation,
            runtime_epoch_id,
        }
    }

    pub fn agent_runtime_id(&self) -> Option<&str> {
        self.agent_runtime_id.as_deref()
    }

    pub fn fence_token(&self) -> Option<u64> {
        self.fence_token
    }

    pub fn runtime_generation(&self) -> Option<u64> {
        self.runtime_generation
    }

    pub fn runtime_epoch_id(&self) -> Option<&str> {
        self.runtime_epoch_id.as_deref()
    }
}

/// Durable read-back shape for machine-owned lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineLifecycleSnapshot {
    runtime_state: RuntimeState,
    binding: MachineLifecycleBindingFacts,
}

impl MachineLifecycleSnapshot {
    pub(crate) fn new(runtime_state: RuntimeState, binding: MachineLifecycleBindingFacts) -> Self {
        Self {
            runtime_state,
            binding,
        }
    }

    /// Runtime state selected by the owning MeerkatMachine transition.
    pub fn runtime_state(&self) -> RuntimeState {
        self.runtime_state
    }

    /// Runtime binding facts selected by the owning MeerkatMachine transition.
    pub fn binding(&self) -> &MachineLifecycleBindingFacts {
        &self.binding
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct MachineLifecycleBindingFactsStoreWire {
    agent_runtime_id: Option<String>,
    fence_token: Option<u64>,
    runtime_generation: Option<u64>,
    runtime_epoch_id: Option<String>,
}

impl From<&MachineLifecycleBindingFacts> for MachineLifecycleBindingFactsStoreWire {
    fn from(binding: &MachineLifecycleBindingFacts) -> Self {
        Self {
            agent_runtime_id: binding.agent_runtime_id().map(ToOwned::to_owned),
            fence_token: binding.fence_token(),
            runtime_generation: binding.runtime_generation(),
            runtime_epoch_id: binding.runtime_epoch_id().map(ToOwned::to_owned),
        }
    }
}

impl From<MachineLifecycleBindingFactsStoreWire> for MachineLifecycleBindingFacts {
    fn from(binding: MachineLifecycleBindingFactsStoreWire) -> Self {
        Self::new(
            binding.agent_runtime_id,
            binding.fence_token,
            binding.runtime_generation,
            binding.runtime_epoch_id,
        )
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct MachineLifecycleSnapshotStoreWire {
    record_version: u16,
    runtime_state: RuntimeState,
    binding: MachineLifecycleBindingFactsStoreWire,
}

impl From<&MachineLifecycleSnapshot> for MachineLifecycleSnapshotStoreWire {
    fn from(snapshot: &MachineLifecycleSnapshot) -> Self {
        Self {
            record_version: MACHINE_LIFECYCLE_STORE_RECORD_VERSION,
            runtime_state: snapshot.runtime_state(),
            binding: snapshot.binding().into(),
        }
    }
}

impl TryFrom<MachineLifecycleSnapshotStoreWire> for MachineLifecycleSnapshot {
    type Error = RuntimeStoreError;

    fn try_from(record: MachineLifecycleSnapshotStoreWire) -> Result<Self, Self::Error> {
        if record.record_version != MACHINE_LIFECYCLE_STORE_RECORD_VERSION {
            return Err(RuntimeStoreError::ReadFailed(format!(
                "unsupported machine lifecycle store record version {}",
                record.record_version
            )));
        }
        Ok(Self::new(record.runtime_state, record.binding.into()))
    }
}

fn decode_machine_lifecycle_store_record(
    bytes: &[u8],
) -> Result<MachineLifecycleSnapshot, RuntimeStoreError> {
    let record = serde_json::from_slice::<MachineLifecycleSnapshotStoreWire>(bytes)
        .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
    MachineLifecycleSnapshot::try_from(record)
}

/// Load the last persisted runtime-state projection from a generated lifecycle
/// record.
///
/// This is a projection of [`MachineLifecycleCommit`] authority. Store
/// implementations provide only opaque record bytes; the runtime crate owns the
/// decoding and rejects compatibility rows that are not machine lifecycle
/// records.
pub async fn load_runtime_state(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
) -> Result<Option<RuntimeState>, RuntimeStoreError> {
    Ok(load_machine_lifecycle(store, runtime_id)
        .await?
        .map(|snapshot| snapshot.runtime_state()))
}

pub(crate) async fn load_machine_lifecycle(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
) -> Result<Option<MachineLifecycleSnapshot>, RuntimeStoreError> {
    store
        .load_machine_lifecycle_record(runtime_id)
        .await?
        .map(|bytes| decode_machine_lifecycle_store_record(&bytes))
        .transpose()
}

/// Declared durable store record for generated machine lifecycle truth.
///
/// Stores receive this record from [`MachineLifecycleCommit`] and may persist
/// its encoded form. Loading must decode this exact record shape; compatibility
/// runtime-state projections are not lifecycle authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineLifecycleStoreRecord {
    snapshot: MachineLifecycleSnapshot,
}

impl MachineLifecycleStoreRecord {
    pub(crate) fn from_snapshot(snapshot: &MachineLifecycleSnapshot) -> Self {
        Self {
            snapshot: snapshot.clone(),
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, RuntimeStoreError> {
        let wire = MachineLifecycleSnapshotStoreWire::from(&self.snapshot);
        serde_json::to_vec(&wire).map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))
    }
}

/// Machine-owned lifecycle commit token.
///
/// This token has no public constructor. RuntimeStore implementors can persist
/// the selected state and binding facts, but callers outside the machine/driver
/// commit path cannot select arbitrary lifecycle truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineLifecycleCommit {
    snapshot: MachineLifecycleSnapshot,
}

impl MachineLifecycleCommit {
    pub(crate) fn new_with_binding(
        runtime_state: RuntimeState,
        binding: MachineLifecycleBindingFacts,
    ) -> Self {
        Self {
            snapshot: MachineLifecycleSnapshot::new(runtime_state, binding),
        }
    }

    /// Runtime state selected by the owning MeerkatMachine transition.
    pub fn runtime_state(&self) -> RuntimeState {
        self.snapshot.runtime_state()
    }

    /// Durable lifecycle snapshot selected by the owning MeerkatMachine transition.
    pub fn snapshot(&self) -> &MachineLifecycleSnapshot {
        &self.snapshot
    }

    /// Durable record selected by the owning MeerkatMachine transition.
    pub fn store_record(&self) -> MachineLifecycleStoreRecord {
        MachineLifecycleStoreRecord::from_snapshot(&self.snapshot)
    }

    pub(crate) fn into_snapshot(self) -> MachineLifecycleSnapshot {
        self.snapshot
    }
}

/// Atomic persistence interface for runtime state.
///
/// Implementations:
/// - `InMemoryRuntimeStore` — in-memory, no durability (ephemeral/testing)
/// - `SqliteRuntimeStore` — SQLite-backed durable runtime state
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait RuntimeStore: Send + Sync {
    /// Stable key for process-local auth/OAuth authority reuse across reopened
    /// handles for the same durable store.
    fn auth_authority_key(&self) -> Option<String> {
        None
    }

    /// Persist the runtime-owned OAuth login-flow payload snapshot.
    ///
    /// The AuthMachine owns admission/consume semantics; this payload snapshot
    /// carries the PKCE verifier and device-code correlation data needed to
    /// rehydrate active flows after a persistent runtime process restart.
    fn persist_auth_oauth_flow_snapshot(
        &self,
        snapshot_json: &[u8],
    ) -> Result<(), RuntimeStoreError> {
        let _ = snapshot_json;
        Err(RuntimeStoreError::Unsupported(
            "persist_auth_oauth_flow_snapshot".into(),
        ))
    }

    /// Load the runtime-owned OAuth login-flow payload snapshot, if present.
    fn load_auth_oauth_flow_snapshot(&self) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "load_auth_oauth_flow_snapshot".into(),
        ))
    }

    /// Atomically update the runtime-owned OAuth login-flow payload snapshot.
    ///
    /// Stores that support OAuth snapshots must override this with a lock,
    /// transaction, or compare-and-swap boundary. A load/compute/persist
    /// fallback is not safe for admission, capacity, or consume claims.
    fn update_auth_oauth_flow_snapshot(
        &self,
        _update: &mut AuthOAuthFlowSnapshotUpdate<'_>,
    ) -> Result<(), RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "update_auth_oauth_flow_snapshot".into(),
        ))
    }

    /// Atomically persist a session snapshot that is not a run boundary.
    ///
    /// Session-control snapshots update durable session authority without
    /// producing a [`RunBoundaryReceipt`].
    async fn commit_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: SessionDelta,
    ) -> Result<(), RuntimeStoreError>;

    /// Atomically persist a same-session transcript rewrite snapshot.
    ///
    /// Store implementations that support transcript edits must compare the
    /// currently persisted session transcript revision with `commit.parent_revision`
    /// inside the same lock or transaction that writes `session_delta`.
    async fn commit_session_transcript_rewrite_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: SessionDelta,
        commit: &meerkat_core::TranscriptRewriteCommit,
    ) -> Result<(), RuntimeStoreError> {
        let _ = (runtime_id, session_delta, commit);
        Err(RuntimeStoreError::Unsupported(
            "commit_session_transcript_rewrite_snapshot".into(),
        ))
    }

    /// Atomically persist session delta + receipt + input state updates.
    ///
    /// All three writes MUST commit in a single atomic operation.
    /// If any write fails, none should be visible.
    /// Atomically persist session delta + receipt + input state updates.
    ///
    /// All writes MUST commit in a single atomic operation.
    /// If `session_store_key` is `Some`, validates that the snapshot belongs
    /// to that session and, for stores that physically share a `SessionStore`
    /// table, writes that table in the same transaction. Runtime snapshot
    /// authority remains keyed only by `runtime_id`; `session_store_key` must
    /// not create a raw session UUID runtime alias.
    async fn atomic_apply(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: Option<SessionDelta>,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: Option<meerkat_core::types::SessionId>,
    ) -> Result<(), RuntimeStoreError>;

    /// Load all input states for a runtime.
    async fn load_input_states(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<StoredInputState>, RuntimeStoreError>;

    /// Load a specific boundary receipt.
    async fn load_boundary_receipt(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<Option<RunBoundaryReceipt>, RuntimeStoreError>;

    /// Load the latest committed session snapshot for a runtime, if any.
    async fn load_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError>;

    /// Remove the latest committed session snapshot for a runtime.
    ///
    /// This is used only as a fail-closed quarantine path after a compatibility
    /// projection write rejects a runtime snapshot that was already staged as
    /// runtime authority and the service cannot restore the previous snapshot.
    async fn clear_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError>;

    /// Replace the latest committed session snapshot only if it still matches
    /// `expected_current`.
    ///
    /// Used by fail-closed recovery after a compatibility projection rejected
    /// a runtime snapshot. Implementations must compare and write atomically so
    /// recovery cannot overwrite a newer runtime-authoritative snapshot.
    async fn replace_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
        replacement: Vec<u8>,
    ) -> Result<bool, RuntimeStoreError>;

    /// Remove the latest committed session snapshot only if it still matches
    /// `expected_current`.
    ///
    /// This is the conditional variant of the fail-closed quarantine path.
    async fn clear_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
    ) -> Result<bool, RuntimeStoreError>;

    /// Persist a single input state (for durable-before-ack).
    async fn persist_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        state: &InputStatePersistenceRecord,
    ) -> Result<(), RuntimeStoreError>;

    /// Load a single input state.
    async fn load_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, RuntimeStoreError>;

    /// Load the last persisted machine lifecycle record bytes, if any.
    ///
    /// Implementations return only the opaque bytes previously obtained from
    /// [`MachineLifecycleCommit::store_record`]. The runtime crate decodes
    /// these bytes through `load_runtime_state` or internal recovery helpers;
    /// stores must not promote compatibility rows or bare runtime states into
    /// lifecycle authority.
    async fn load_machine_lifecycle_record(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError>;

    /// Atomically commit machine-owned lifecycle state changes.
    ///
    /// Writes runtime state, generated runtime binding facts, and all input
    /// state updates in a single atomic operation. `MachineLifecycleCommit` has
    /// no public constructor, so this cannot be used by compatibility callers
    /// to pick runtime truth.
    async fn commit_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        commit: MachineLifecycleCommit,
        input_states: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError>;

    /// Persist a snapshot of the ops lifecycle registry state.
    async fn persist_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        snapshot: &crate::ops_lifecycle::PersistedOpsSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        let _ = (runtime_id, snapshot);
        Err(RuntimeStoreError::Unsupported(
            "persist_ops_lifecycle".into(),
        ))
    }

    /// Load a previously persisted ops lifecycle snapshot.
    async fn load_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<crate::ops_lifecycle::PersistedOpsSnapshot>, RuntimeStoreError> {
        let _ = runtime_id;
        Err(RuntimeStoreError::Unsupported("load_ops_lifecycle".into()))
    }

    /// Delete a previously persisted ops lifecycle snapshot.
    async fn delete_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        let _ = runtime_id;
        Err(RuntimeStoreError::Unsupported(
            "delete_ops_lifecycle".into(),
        ))
    }
}

pub use memory::InMemoryRuntimeStore;
#[cfg(feature = "sqlite-store")]
pub use sqlite::SqliteRuntimeStore;
