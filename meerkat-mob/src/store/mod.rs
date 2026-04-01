//! Mob store traits and implementations.

mod in_memory;
#[cfg(not(target_arch = "wasm32"))]
mod sqlite;

pub use in_memory::{InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobSpecStore};
#[cfg(not(target_arch = "wasm32"))]
pub use sqlite::{SqliteMobEventStore, SqliteMobRunStore, SqliteMobSpecStore, SqliteMobStores};

use crate::definition::MobDefinition;
use crate::event::{MobEvent, NewMobEvent};
use crate::ids::{FlowId, MobId, RunId, StepId};
use crate::run::{FailureLedgerEntry, MobRun, MobRunStatus, StepLedgerEntry};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use meerkat_machine_kernels::KernelState;

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

    /// Serialization or deserialization failed.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
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
        expected: &KernelState,
        next: &KernelState,
    ) -> Result<bool, MobStoreError>;
    async fn cas_run_snapshot(
        &self,
        run_id: &RunId,
        expected_status: MobRunStatus,
        expected_flow_state: &KernelState,
        next_status: MobRunStatus,
        next_flow_state: &KernelState,
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
