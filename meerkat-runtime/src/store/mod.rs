//! RuntimeStore — atomic persistence for runtime state.
//!
//! Machine-owned runtime commands durably persist [`RunBoundaryReceipt`] values
//! atomically with their session and input-state effects.

pub mod memory;
#[cfg(feature = "sqlite-store")]
pub mod sqlite;

use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};

use crate::identifiers::LogicalRuntimeId;
use crate::input_state::StoredInputState;
use crate::runtime_state::RuntimeState;

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

/// Machine-owned lifecycle commit token.
///
/// This token has no public constructor. RuntimeStore implementors can persist
/// the selected state, but callers outside the machine/driver commit path
/// cannot select arbitrary lifecycle truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MachineLifecycleCommit {
    runtime_state: RuntimeState,
}

impl MachineLifecycleCommit {
    pub(crate) fn new(runtime_state: RuntimeState) -> Self {
        Self { runtime_state }
    }

    /// Runtime state selected by the owning MeerkatMachine transition.
    pub fn runtime_state(self) -> RuntimeState {
        self.runtime_state
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
        input_updates: Vec<StoredInputState>,
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

    /// Persist a single input state (for durable-before-ack).
    async fn persist_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        state: &StoredInputState,
    ) -> Result<(), RuntimeStoreError>;

    /// Load a single input state.
    async fn load_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, RuntimeStoreError>;

    /// Load the last persisted runtime state, if any.
    async fn load_runtime_state(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeState>, RuntimeStoreError>;

    /// Atomically commit machine-owned lifecycle state changes.
    ///
    /// Writes runtime state + all input state updates in a single atomic
    /// operation. `MachineLifecycleCommit` has no public constructor, so this
    /// cannot be used by compatibility callers to pick runtime truth.
    async fn commit_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        commit: MachineLifecycleCommit,
        input_states: &[StoredInputState],
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
}

pub use memory::InMemoryRuntimeStore;
#[cfg(feature = "sqlite-store")]
pub use sqlite::SqliteRuntimeStore;
