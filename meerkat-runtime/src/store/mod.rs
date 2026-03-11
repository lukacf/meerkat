//! RuntimeStore — atomic persistence for runtime state.
//!
//! §19: "CoreExecutor::apply MUST durably persist the RunBoundaryReceipt atomically
//! with the boundary side effects before returning success."

pub mod memory;
#[cfg(feature = "redb-store")]
pub mod redb;

use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};

use crate::identifiers::LogicalRuntimeId;
use crate::input_state::InputState;

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
    /// Not found.
    #[error("Not found: {0}")]
    NotFound(String),
    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Describes session-level writes to be committed atomically with receipts.
#[derive(Debug, Clone)]
pub struct SessionDelta {
    /// Serialized session snapshot (opaque to RuntimeStore).
    pub session_snapshot: Vec<u8>,
}

/// Atomic persistence interface for runtime state.
///
/// Implementations:
/// - `InMemoryRuntimeStore` — in-memory, no durability (ephemeral/testing)
/// - `RedbRuntimeStore` — shared redb database, single-transaction atomicity (Phase 4)
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait RuntimeStore: Send + Sync {
    /// Atomically persist session delta + receipt + input state updates.
    ///
    /// All three writes MUST commit in a single atomic operation.
    /// If any write fails, none should be visible.
    async fn atomic_apply(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: Option<SessionDelta>,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputState>,
    ) -> Result<(), RuntimeStoreError>;

    /// Load all input states for a runtime.
    async fn load_input_states(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<InputState>, RuntimeStoreError>;

    /// Load a specific boundary receipt.
    async fn load_boundary_receipt(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<Option<RunBoundaryReceipt>, RuntimeStoreError>;

    /// Persist a single input state (for durable-before-ack).
    async fn persist_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        state: &InputState,
    ) -> Result<(), RuntimeStoreError>;

    /// Load a single input state.
    async fn load_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_id: &InputId,
    ) -> Result<Option<InputState>, RuntimeStoreError>;
}

pub use memory::InMemoryRuntimeStore;
#[cfg(feature = "redb-store")]
pub use redb::RedbRuntimeStore;
