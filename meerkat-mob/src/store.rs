//! Store traits for mob specs, runs, and events.
//!
//! Defines [`MobSpecStore`], [`MobRunStore`], and [`MobEventStore`] with
//! in-memory implementations for testing and development.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use tokio::sync::RwLock;

use crate::error::MobError;
use crate::event::MobEvent;
use crate::run::{FailureLedgerEntry, MobRun, RunStatus, StepLedgerEntry};
use crate::spec::MobSpec;

// ---------------------------------------------------------------------------
// MobSpecStore
// ---------------------------------------------------------------------------

/// Trait for persisting and retrieving mob specs.
///
/// Supports CAS updates via the `spec_revision` field on [`MobSpec`].
#[async_trait]
pub trait MobSpecStore: Send + Sync {
    /// Store a new spec or update an existing one.
    ///
    /// On create, `spec_revision` is set to 1.
    /// On update, `spec_revision` must match the stored revision (CAS).
    /// Returns the stored spec with its (possibly bumped) revision.
    async fn put(&self, spec: MobSpec) -> Result<MobSpec, MobError>;

    /// Retrieve a spec by mob ID.
    async fn get(&self, mob_id: &str) -> Result<Option<MobSpec>, MobError>;

    /// List all stored specs.
    async fn list(&self) -> Result<Vec<MobSpec>, MobError>;

    /// Delete a spec by mob ID. Returns `true` if it existed.
    async fn delete(&self, mob_id: &str) -> Result<bool, MobError>;
}

// ---------------------------------------------------------------------------
// MobRunStore
// ---------------------------------------------------------------------------

/// Trait for persisting and retrieving mob runs.
///
/// Supports compare-and-swap status transitions to prevent concurrent
/// mutation inconsistencies.
#[async_trait]
pub trait MobRunStore: Send + Sync {
    /// Create a new run. The `run_id` must be unique.
    async fn create(&self, run: MobRun) -> Result<(), MobError>;

    /// Retrieve a run by ID.
    async fn get(&self, run_id: &str) -> Result<Option<MobRun>, MobError>;

    /// List runs, optionally filtered by mob ID and/or status.
    async fn list(
        &self,
        mob_id: Option<&str>,
        status: Option<RunStatus>,
    ) -> Result<Vec<MobRun>, MobError>;

    /// Transition the run status via compare-and-swap.
    ///
    /// Returns `Ok(())` if the transition was applied, or
    /// [`MobError::InvalidTransition`] if the current status does not match
    /// `expected_status`.
    async fn cas_status(
        &self,
        run_id: &str,
        expected_status: RunStatus,
        new_status: RunStatus,
    ) -> Result<(), MobError>;

    /// Append an entry to the step ledger.
    async fn append_step_entry(
        &self,
        run_id: &str,
        entry: StepLedgerEntry,
    ) -> Result<(), MobError>;

    /// Append an entry to the failure ledger.
    async fn append_failure_entry(
        &self,
        run_id: &str,
        entry: FailureLedgerEntry,
    ) -> Result<(), MobError>;
}

// ---------------------------------------------------------------------------
// MobEventStore
// ---------------------------------------------------------------------------

/// Trait for appending and polling mob events.
///
/// Events are assigned monotonically increasing cursor values on append.
/// Supports cursor-based polling for consumers and TTL-based pruning.
#[async_trait]
pub trait MobEventStore: Send + Sync {
    /// Append an event and assign it a cursor value.
    ///
    /// The store sets `event.cursor` to the next sequence number.
    async fn append(&self, event: MobEvent) -> Result<u64, MobError>;

    /// Poll events after the given cursor (exclusive).
    ///
    /// Returns events in cursor order, limited to `limit` if specified.
    async fn poll(
        &self,
        mob_id: &str,
        after_cursor: Option<u64>,
        limit: Option<usize>,
    ) -> Result<Vec<MobEvent>, MobError>;

    /// Prune events older than the given timestamp.
    ///
    /// Returns the number of events removed.
    async fn prune_before(&self, before: DateTime<Utc>) -> Result<usize, MobError>;
}

// ===========================================================================
// In-memory implementations
// ===========================================================================

// ---------------------------------------------------------------------------
// InMemoryMobSpecStore
// ---------------------------------------------------------------------------

/// In-memory implementation of [`MobSpecStore`] for testing.
#[derive(Debug, Default)]
pub struct InMemoryMobSpecStore {
    specs: RwLock<BTreeMap<String, MobSpec>>,
}

impl InMemoryMobSpecStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl MobSpecStore for InMemoryMobSpecStore {
    async fn put(&self, mut spec: MobSpec) -> Result<MobSpec, MobError> {
        let mut store = self.specs.write().await;
        if let Some(existing) = store.get(&spec.mob_id) {
            // CAS check
            if spec.spec_revision != existing.spec_revision {
                return Err(MobError::SpecConflict {
                    expected: spec.spec_revision,
                    actual: existing.spec_revision,
                });
            }
            spec.spec_revision += 1;
        } else {
            spec.spec_revision = 1;
        }
        store.insert(spec.mob_id.clone(), spec.clone());
        Ok(spec)
    }

    async fn get(&self, mob_id: &str) -> Result<Option<MobSpec>, MobError> {
        let store = self.specs.read().await;
        Ok(store.get(mob_id).cloned())
    }

    async fn list(&self) -> Result<Vec<MobSpec>, MobError> {
        let store = self.specs.read().await;
        Ok(store.values().cloned().collect())
    }

    async fn delete(&self, mob_id: &str) -> Result<bool, MobError> {
        let mut store = self.specs.write().await;
        Ok(store.remove(mob_id).is_some())
    }
}

// ---------------------------------------------------------------------------
// InMemoryMobRunStore
// ---------------------------------------------------------------------------

/// In-memory implementation of [`MobRunStore`] for testing.
#[derive(Debug, Default)]
pub struct InMemoryMobRunStore {
    runs: RwLock<BTreeMap<String, MobRun>>,
}

impl InMemoryMobRunStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl MobRunStore for InMemoryMobRunStore {
    async fn create(&self, run: MobRun) -> Result<(), MobError> {
        let mut store = self.runs.write().await;
        if store.contains_key(&run.run_id) {
            return Err(MobError::StoreError(format!(
                "run already exists: {}",
                run.run_id
            )));
        }
        store.insert(run.run_id.clone(), run);
        Ok(())
    }

    async fn get(&self, run_id: &str) -> Result<Option<MobRun>, MobError> {
        let store = self.runs.read().await;
        Ok(store.get(run_id).cloned())
    }

    async fn list(
        &self,
        mob_id: Option<&str>,
        status: Option<RunStatus>,
    ) -> Result<Vec<MobRun>, MobError> {
        let store = self.runs.read().await;
        let runs: Vec<MobRun> = store
            .values()
            .filter(|r| {
                mob_id.is_none_or(|id| r.mob_id == id)
                    && status.is_none_or(|s| r.status == s)
            })
            .cloned()
            .collect();
        Ok(runs)
    }

    async fn cas_status(
        &self,
        run_id: &str,
        expected_status: RunStatus,
        new_status: RunStatus,
    ) -> Result<(), MobError> {
        let mut store = self.runs.write().await;
        let run = store
            .get_mut(run_id)
            .ok_or_else(|| MobError::RunNotFound {
                run_id: run_id.to_string(),
            })?;

        if run.status != expected_status {
            return Err(MobError::InvalidTransition {
                from: run.status,
                to: new_status,
            });
        }

        if !expected_status.can_transition_to(new_status) {
            return Err(MobError::InvalidTransition {
                from: expected_status,
                to: new_status,
            });
        }

        run.status = new_status;
        run.updated_at = Utc::now();
        Ok(())
    }

    async fn append_step_entry(
        &self,
        run_id: &str,
        entry: StepLedgerEntry,
    ) -> Result<(), MobError> {
        let mut store = self.runs.write().await;
        let run = store
            .get_mut(run_id)
            .ok_or_else(|| MobError::RunNotFound {
                run_id: run_id.to_string(),
            })?;
        run.step_ledger.push(entry);
        run.updated_at = Utc::now();
        Ok(())
    }

    async fn append_failure_entry(
        &self,
        run_id: &str,
        entry: FailureLedgerEntry,
    ) -> Result<(), MobError> {
        let mut store = self.runs.write().await;
        let run = store
            .get_mut(run_id)
            .ok_or_else(|| MobError::RunNotFound {
                run_id: run_id.to_string(),
            })?;
        run.failure_ledger.push(entry);
        run.updated_at = Utc::now();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// InMemoryMobEventStore
// ---------------------------------------------------------------------------

/// In-memory implementation of [`MobEventStore`] for testing.
#[derive(Debug, Default)]
pub struct InMemoryMobEventStore {
    events: RwLock<Vec<MobEvent>>,
    next_cursor: RwLock<u64>,
}

impl InMemoryMobEventStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self {
            events: RwLock::new(Vec::new()),
            next_cursor: RwLock::new(1),
        }
    }
}

#[async_trait]
impl MobEventStore for InMemoryMobEventStore {
    async fn append(&self, mut event: MobEvent) -> Result<u64, MobError> {
        let mut cursor_guard = self.next_cursor.write().await;
        let cursor = *cursor_guard;
        event.cursor = cursor;
        *cursor_guard += 1;
        drop(cursor_guard);

        let mut events = self.events.write().await;
        events.push(event);
        Ok(cursor)
    }

    async fn poll(
        &self,
        mob_id: &str,
        after_cursor: Option<u64>,
        limit: Option<usize>,
    ) -> Result<Vec<MobEvent>, MobError> {
        let events = self.events.read().await;
        let iter = events
            .iter()
            .filter(|e| e.mob_id == mob_id)
            .filter(|e| after_cursor.is_none_or(|c| e.cursor > c));

        let result: Vec<MobEvent> = if let Some(limit) = limit {
            iter.take(limit).cloned().collect()
        } else {
            iter.cloned().collect()
        };
        Ok(result)
    }

    async fn prune_before(&self, before: DateTime<Utc>) -> Result<usize, MobError> {
        let mut events = self.events.write().await;
        let original_len = events.len();
        events.retain(|e| e.timestamp >= before);
        Ok(original_len - events.len())
    }
}
