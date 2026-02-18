//! MobService trait -- the public API for mob orchestration.
//!
//! [`MobService`] defines all operations for managing mob specs, activating
//! flows, querying runs, and polling events. All methods return
//! `Result<T, MobError>`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use crate::error::MobError;
use crate::event::MobEvent;
use crate::resolver::MeerkatIdentity;
use crate::run::{MobRun, RunStatus};
use crate::spec::MobSpec;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// Request to apply (create or update) a mob spec.
#[derive(Debug, Clone)]
pub struct ApplySpecRequest {
    /// The spec to apply.
    pub spec: MobSpec,
}

/// Request to activate a flow within a mob.
#[derive(Debug, Clone)]
pub struct ActivateRequest {
    /// Mob ID to activate.
    pub mob_id: String,
    /// Flow ID to execute.
    pub flow_id: String,
    /// Activation parameters (passed to conditions and step dispatch).
    pub params: serde_json::Value,
}

/// Result of activating a flow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActivateResult {
    /// Unique run ID for this activation.
    pub run_id: String,
    /// Initial run status (always `Pending`).
    pub status: RunStatus,
    /// Spec revision at activation time.
    pub spec_revision: u64,
}

/// Request to list runs with optional filters.
#[derive(Debug, Clone, Default)]
pub struct ListRunsRequest {
    /// Filter by mob ID.
    pub mob_id: Option<String>,
    /// Filter by status.
    pub status: Option<RunStatus>,
    /// Maximum number of runs to return.
    pub limit: Option<usize>,
}

/// Request to list mob specs.
#[derive(Debug, Clone, Default)]
pub struct ListSpecsRequest {
    /// Maximum number of specs to return.
    pub limit: Option<usize>,
}

/// Request to poll events.
#[derive(Debug, Clone)]
pub struct PollEventsRequest {
    /// Mob ID to poll events for.
    pub mob_id: String,
    /// Cursor to resume from (exclusive). `None` = start from beginning.
    pub after_cursor: Option<u64>,
    /// Maximum number of events to return.
    pub limit: Option<usize>,
}

/// Request to reconcile a mob's meerkats.
#[derive(Debug, Clone)]
pub struct ReconcileRequest {
    /// Mob ID to reconcile.
    pub mob_id: String,
    /// Reconcile mode.
    pub mode: ReconcileMode,
}

/// Reconcile mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileMode {
    /// Only report differences, do not apply changes.
    ReportOnly,
    /// Apply changes (spawn missing, retire orphaned).
    Apply,
}

/// Result of a reconcile operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReconcileResult {
    /// Number of meerkats spawned.
    pub spawned: usize,
    /// Number of meerkats retired.
    pub retired: usize,
    /// Number of meerkats unchanged.
    pub unchanged: usize,
    /// Meerkats that were spawned.
    pub spawned_ids: Vec<String>,
    /// Meerkats that were retired.
    pub retired_ids: Vec<String>,
}

/// Request to list meerkats for a mob.
#[derive(Debug, Clone)]
pub struct ListMeerkatsRequest {
    /// Mob ID to list meerkats for.
    pub mob_id: String,
    /// Optional role filter.
    pub role: Option<String>,
}

// ---------------------------------------------------------------------------
// MobService trait
// ---------------------------------------------------------------------------

/// The public API for mob orchestration.
///
/// All surface crates (CLI, MCP server, REST) delegate to this trait for
/// mob operations. Implementations are responsible for spec management,
/// flow activation, run lifecycle, and event emission.
#[async_trait]
pub trait MobService: Send + Sync {
    /// Apply (create or update) a mob spec.
    ///
    /// Returns the applied spec with its new revision number.
    async fn apply_spec(&self, req: ApplySpecRequest) -> Result<MobSpec, MobError>;

    /// Get a mob spec by ID.
    async fn get_spec(&self, mob_id: &str) -> Result<MobSpec, MobError>;

    /// List all mob specs.
    async fn list_specs(&self, req: ListSpecsRequest) -> Result<Vec<MobSpec>, MobError>;

    /// Delete a mob spec by ID.
    async fn delete_spec(&self, mob_id: &str) -> Result<(), MobError>;

    /// Activate a flow within a mob, creating a new run.
    async fn activate(&self, req: ActivateRequest) -> Result<ActivateResult, MobError>;

    /// Get a run by ID.
    async fn get_run(&self, run_id: &str) -> Result<MobRun, MobError>;

    /// List runs with optional filters.
    async fn list_runs(&self, req: ListRunsRequest) -> Result<Vec<MobRun>, MobError>;

    /// Cancel a running or pending run.
    async fn cancel_run(&self, run_id: &str) -> Result<MobRun, MobError>;

    /// List meerkat instances for a mob.
    async fn list_meerkats(
        &self,
        req: ListMeerkatsRequest,
    ) -> Result<Vec<MeerkatIdentity>, MobError>;

    /// Reconcile desired vs. actual meerkat instances.
    async fn reconcile(&self, req: ReconcileRequest) -> Result<ReconcileResult, MobError>;

    /// Poll mob events after a cursor position.
    async fn poll_events(&self, req: PollEventsRequest) -> Result<Vec<MobEvent>, MobError>;
}
