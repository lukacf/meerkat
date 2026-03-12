//! RuntimeStore — atomic persistence for runtime state.
//!
//! §19: "CoreExecutor::apply MUST durably persist the RunBoundaryReceipt atomically
//! with the boundary side effects before returning success."

pub mod memory;
#[cfg(feature = "redb-store")]
pub mod redb;

use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
use sha2::{Digest, Sha256};

use crate::identifiers::LogicalRuntimeId;
use crate::input_state::InputState;
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

fn authoritative_receipt(
    session_delta: Option<&SessionDelta>,
    run_id: RunId,
    boundary: RunApplyBoundary,
    contributing_input_ids: Vec<InputId>,
    sequence: u64,
) -> Result<RunBoundaryReceipt, RuntimeStoreError> {
    let (conversation_digest, message_count) = match session_delta {
        Some(delta) => {
            let session: meerkat_core::Session = serde_json::from_slice(&delta.session_snapshot)
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
            let encoded_messages = serde_json::to_vec(session.messages())
                .map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))?;
            (
                Some(format!("{:x}", Sha256::digest(encoded_messages))),
                session.messages().len(),
            )
        }
        None => (None, 0),
    };

    Ok(RunBoundaryReceipt {
        run_id,
        boundary,
        contributing_input_ids,
        conversation_digest,
        message_count,
        sequence,
    })
}

/// Atomic persistence interface for runtime state.
///
/// Implementations:
/// - `InMemoryRuntimeStore` — in-memory, no durability (ephemeral/testing)
/// - `RedbRuntimeStore` — shared redb database, single-transaction atomicity (Phase 4)
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait RuntimeStore: Send + Sync {
    /// Atomically persist session delta + authoritative receipt + input state updates.
    ///
    /// The receipt MUST be minted by the durable commit seam itself, not by the
    /// caller, so returned success carries the exact proof that was stored.
    async fn commit_session_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: SessionDelta,
        run_id: RunId,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        input_updates: Vec<InputState>,
    ) -> Result<RunBoundaryReceipt, RuntimeStoreError>;

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

    /// Persist the runtime state itself (for durable retire/stop semantics).
    async fn persist_runtime_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        state: RuntimeState,
    ) -> Result<(), RuntimeStoreError>;

    /// Load the last persisted runtime state, if any.
    async fn load_runtime_state(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeState>, RuntimeStoreError>;
}

pub use memory::InMemoryRuntimeStore;
#[cfg(feature = "redb-store")]
pub use redb::RedbRuntimeStore;
