mod in_memory;
mod redb;

pub use in_memory::{InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobSpecStore};
pub use redb::{RedbMobEventStore, RedbMobRunStore, RedbMobSpecStore};

use crate::error::MobResult;
use crate::model::{
    FailureLedgerEntry, MobEvent, MobRun, MobRunFilter, MobRunStatus, MobSpec, MobSpecRevision,
    NewMobEvent, StepLedgerEntry,
};
use async_trait::async_trait;

#[async_trait]
pub trait MobSpecStore: Send + Sync {
    async fn put_spec(
        &self,
        spec: MobSpec,
        expected_revision: Option<MobSpecRevision>,
    ) -> MobResult<()>;

    async fn get_spec(&self, mob_id: &str) -> MobResult<Option<MobSpec>>;

    async fn list_specs(&self) -> MobResult<Vec<MobSpec>>;

    async fn delete_spec(&self, mob_id: &str) -> MobResult<()>;
}

#[async_trait]
pub trait MobRunStore: Send + Sync {
    async fn create_run(&self, run: MobRun) -> MobResult<()>;

    async fn cas_run_status(
        &self,
        run_id: &str,
        expected: MobRunStatus,
        next: MobRunStatus,
    ) -> MobResult<bool>;

    async fn append_step_entry(&self, run_id: &str, entry: StepLedgerEntry) -> MobResult<()>;

    /// Atomically appends a step entry when `logical_key` has not been committed yet.
    ///
    /// Returns `true` when appended, `false` when the key already exists.
    async fn append_step_entry_if_absent(
        &self,
        run_id: &str,
        logical_key: &str,
        entry: StepLedgerEntry,
    ) -> MobResult<bool>;

    async fn append_failure_entry(&self, run_id: &str, entry: FailureLedgerEntry)
        -> MobResult<()>;

    async fn put_step_output(
        &self,
        run_id: &str,
        step_id: &str,
        output: serde_json::Value,
    ) -> MobResult<()>;

    async fn get_run(&self, run_id: &str) -> MobResult<Option<MobRun>>;

    async fn list_runs(&self, filter: MobRunFilter) -> MobResult<Vec<MobRun>>;

    async fn put_run(&self, run: MobRun) -> MobResult<()>;
}

#[async_trait]
pub trait MobEventStore: Send + Sync {
    async fn append_event(&self, event: NewMobEvent) -> MobResult<u64>;

    async fn poll_events(&self, cursor: Option<u64>, limit: Option<usize>)
        -> MobResult<(u64, Vec<MobEvent>)>;

    async fn prune_events(
        &self,
        ttl_secs_by_category: &std::collections::BTreeMap<String, u64>,
    ) -> MobResult<usize>;
}
