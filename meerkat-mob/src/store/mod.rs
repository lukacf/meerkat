//! Mob store traits and implementations.

mod in_memory;
#[cfg(not(target_arch = "wasm32"))]
mod sqlite;

pub use in_memory::{InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobSpecStore};
#[cfg(not(target_arch = "wasm32"))]
pub use sqlite::{SqliteMobEventStore, SqliteMobRunStore, SqliteMobSpecStore, SqliteMobStores};

use crate::definition::MobDefinition;
use crate::event::{MobEvent, NewMobEvent};
use crate::ids::{FlowId, FrameId, LoopId, LoopInstanceId, MobId, RunId, StepId};
use crate::run::{
    FailureLedgerEntry, FrameSnapshot, LoopIterationLedgerEntry, LoopSnapshot, MobRun,
    MobRunStatus, StepLedgerEntry,
};
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
    /// # Frame support
    async fn cas_frame_state(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        expected: Option<&FrameSnapshot>,
        next: FrameSnapshot,
    ) -> Result<bool, MobStoreError>;

    /// CAS wrapper 2: grant node slot — atomically update run flow state + frame state.
    ///
    /// # Frame support
    /// Backends that do not support frame-aware atomic persistence may return
    /// `Err(MobError::NotYetImplemented(...))`.
    async fn cas_grant_node_slot(
        &self,
        run_id: &RunId,
        expected_run_state: &KernelState,
        next_run_state: KernelState,
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
    /// # Frame support
    /// Backends that do not support frame-aware atomic persistence may return
    /// `Err(MobError::NotYetImplemented(...))`.
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
    /// # Frame support
    /// Backends that do not support frame-aware atomic persistence may return
    /// `Err(MobError::NotYetImplemented(...))`.
    #[allow(clippy::too_many_arguments)]
    async fn cas_start_loop(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        expected_run_state: &KernelState,
        next_run_state: KernelState,
        frame_id: &FrameId,
        expected_frame: &FrameSnapshot,
        next_frame: FrameSnapshot,
        initial_loop: LoopSnapshot,
    ) -> Result<bool, MobStoreError>;

    /// CAS wrapper 5: register pending body frame — loop transition + run state update.
    ///
    /// # Frame support
    /// Backends that do not support frame-aware atomic persistence may return
    /// `Err(MobError::NotYetImplemented(...))`.
    async fn cas_loop_request_body_frame(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        expected_loop: &LoopSnapshot,
        next_loop: LoopSnapshot,
        expected_run_state: &KernelState,
        next_run_state: KernelState,
    ) -> Result<bool, MobStoreError>;

    /// CAS wrapper 6: body frame start — loop transition + register new frame + run state update.
    ///
    /// # Frame support
    /// Backends that do not support frame-aware atomic persistence may return
    /// `Err(MobError::NotYetImplemented(...))`.
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
        expected_run_state: &KernelState,
        next_run_state: KernelState,
    ) -> Result<bool, MobStoreError>;

    /// CAS wrapper 7: body frame completion — terminalize frame + loop state update + run state.
    ///
    /// # Frame support
    /// Backends that do not support frame-aware atomic persistence may return
    /// `Err(MobError::NotYetImplemented(...))`.
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
        expected_run_state: &KernelState,
        next_run_state: KernelState,
    ) -> Result<bool, MobStoreError>;

    /// CAS wrapper 8: loop completion — loop state + run state + parent frame update.
    ///
    /// # Frame support
    /// Backends that do not support frame-aware atomic persistence may return
    /// `Err(MobError::NotYetImplemented(...))`.
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
        expected_run_state: &KernelState,
        next_run_state: KernelState,
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
