//! Mob store traits and implementations.

mod in_memory;
mod redb;

pub use in_memory::{InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobSpecStore};
pub use redb::{RedbMobEventStore, RedbMobRunStore, RedbMobSpecStore};

use crate::definition::MobDefinition;
use crate::error::MobError;
use crate::event::{MobEvent, NewMobEvent};
use crate::ids::{FlowId, MobId, RunId, StepId};
use crate::run::{FailureLedgerEntry, MobRun, MobRunStatus, StepLedgerEntry};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// Trait for persisting and querying mob events.
#[async_trait]
pub trait MobEventStore: Send + Sync {
    /// Append a new event to the store.
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobError>;

    /// Poll for events after a given cursor, up to a limit.
    async fn poll(&self, after_cursor: u64, limit: usize) -> Result<Vec<MobEvent>, MobError>;

    /// Replay all events from the beginning.
    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobError>;

    /// Delete all persisted events.
    async fn clear(&self) -> Result<(), MobError>;

    /// Prune events older than a timestamp. Returns count of deleted events.
    async fn prune(&self, _older_than: DateTime<Utc>) -> Result<u64, MobError> {
        Ok(0)
    }
}

/// Trait for persisting and querying flow runs.
#[async_trait]
pub trait MobRunStore: Send + Sync {
    async fn create_run(&self, run: MobRun) -> Result<(), MobError>;
    async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, MobError>;
    async fn list_runs(
        &self,
        mob_id: &MobId,
        flow_id: Option<&FlowId>,
    ) -> Result<Vec<MobRun>, MobError>;
    async fn cas_run_status(
        &self,
        run_id: &RunId,
        expected: MobRunStatus,
        next: MobRunStatus,
    ) -> Result<bool, MobError>;
    async fn append_step_entry(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
    ) -> Result<(), MobError>;
    async fn append_step_entry_if_absent(
        &self,
        run_id: &RunId,
        entry: StepLedgerEntry,
    ) -> Result<bool, MobError>;
    async fn put_step_output(
        &self,
        run_id: &RunId,
        step_id: &StepId,
        output: serde_json::Value,
    ) -> Result<(), MobError>;
    async fn append_failure_entry(
        &self,
        run_id: &RunId,
        entry: FailureLedgerEntry,
    ) -> Result<(), MobError>;
}

/// Trait for persisting and querying mob specs.
#[async_trait]
pub trait MobSpecStore: Send + Sync {
    /// Put a spec. Returns new revision.
    async fn put_spec(
        &self,
        mob_id: &MobId,
        definition: &MobDefinition,
        revision: Option<u64>,
    ) -> Result<u64, MobError>;

    async fn get_spec(&self, mob_id: &MobId) -> Result<Option<(MobDefinition, u64)>, MobError>;
    async fn list_specs(&self) -> Result<Vec<MobId>, MobError>;
    async fn delete_spec(&self, mob_id: &MobId, revision: Option<u64>) -> Result<bool, MobError>;
}
