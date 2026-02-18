//! Mob run state machine and ledger types.
//!
//! A [`MobRun`] tracks the lifecycle of a single flow activation, including
//! step dispatch outcomes in the [`StepLedgerEntry`] and failure history in
//! the [`FailureLedgerEntry`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Run status
// ---------------------------------------------------------------------------

/// Lifecycle state of a mob run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Run created but not yet started.
    Pending,
    /// Run is actively executing steps.
    Running,
    /// All steps completed successfully.
    Completed,
    /// At least one step failed fatally.
    Failed,
    /// Run was explicitly cancelled.
    Cancelled,
}

impl RunStatus {
    /// Returns `true` if this is a terminal state.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Check whether transitioning from `self` to `to` is valid.
    pub fn can_transition_to(self, to: Self) -> bool {
        matches!(
            (self, to),
            (Self::Pending, Self::Running)
                | (Self::Running, Self::Completed | Self::Failed | Self::Cancelled)
                | (Self::Pending, Self::Cancelled)
        )
    }
}

// ---------------------------------------------------------------------------
// Mob run
// ---------------------------------------------------------------------------

/// State of a single flow execution.
///
/// Contains the step ledger (per-target dispatch outcomes) and failure ledger
/// (per-attempt failure history). Status transitions are enforced via
/// compare-and-swap in the run store.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MobRun {
    /// Unique identifier for this run.
    pub run_id: String,
    /// Mob spec this run belongs to.
    pub mob_id: String,
    /// Flow being executed.
    pub flow_id: String,
    /// Spec revision at the time of activation.
    pub spec_revision: u64,
    /// Current lifecycle state.
    pub status: RunStatus,
    /// Per-target step dispatch outcomes.
    pub step_ledger: Vec<StepLedgerEntry>,
    /// Per-attempt failure history.
    pub failure_ledger: Vec<FailureLedgerEntry>,
    /// When this run was created.
    pub created_at: DateTime<Utc>,
    /// When this run was last updated.
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Step ledger
// ---------------------------------------------------------------------------

/// Status of a single step dispatch to a specific target meerkat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepEntryStatus {
    /// Step is queued but not yet dispatched.
    Pending,
    /// Step has been dispatched to the target.
    Dispatched,
    /// Target completed successfully.
    Completed,
    /// Target returned a failure.
    Failed,
    /// Step timed out waiting for the target.
    TimedOut,
    /// Step was skipped (condition evaluated to false).
    Skipped,
}

/// A single entry in the step ledger, tracking one dispatch to one target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepLedgerEntry {
    /// Step ID within the flow.
    pub step_id: String,
    /// Target meerkat instance ID.
    pub target_meerkat_id: String,
    /// Current status of this dispatch.
    pub status: StepEntryStatus,
    /// Attempt number (1-based).
    pub attempt: u32,
    /// When the dispatch was sent.
    pub dispatched_at: Option<DateTime<Utc>>,
    /// When the response was received.
    pub completed_at: Option<DateTime<Utc>>,
    /// Result payload from the target.
    pub result: Option<serde_json::Value>,
    /// Error message if the dispatch failed.
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Failure ledger
// ---------------------------------------------------------------------------

/// A record of a single failed attempt in the failure history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FailureLedgerEntry {
    /// Step ID within the flow.
    pub step_id: String,
    /// Target meerkat instance ID.
    pub target_meerkat_id: String,
    /// Attempt number (1-based).
    pub attempt: u32,
    /// Error description.
    pub error: String,
    /// When the failure occurred.
    pub failed_at: DateTime<Utc>,
}
