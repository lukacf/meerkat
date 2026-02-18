//! Mob-level event types.
//!
//! [`MobEvent`] captures lifecycle transitions, topology observations,
//! supervisor actions, reconcile reports, and degradation warnings.
//! Events carry correlation fields for linking to per-meerkat agent events.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Retention category for event TTL management.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionCategory {
    /// Long-lived audit trail events.
    Audit,
    /// Medium-lived operational events.
    Ops,
    /// Short-lived debug events.
    Debug,
}

/// A mob-level event with correlation fields.
///
/// Events are emitted during mob lifecycle transitions and captured by the
/// [`MobEventStore`](crate::store::MobEventStore) for cursor-based polling.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MobEvent {
    /// Monotonically increasing event sequence number (assigned by the store).
    pub cursor: u64,
    /// When this event was emitted.
    pub timestamp: DateTime<Utc>,
    /// Mob ID this event belongs to.
    pub mob_id: String,
    /// Run ID, if applicable.
    pub run_id: Option<String>,
    /// Flow ID, if applicable.
    pub flow_id: Option<String>,
    /// Step ID, if applicable.
    pub step_id: Option<String>,
    /// Retention category for TTL management.
    pub retention: RetentionCategory,
    /// Event payload.
    pub kind: MobEventKind,
}

/// The specific kind of mob event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum MobEventKind {
    /// A run has started executing.
    RunStarted,
    /// A run completed successfully.
    RunCompleted,
    /// A run failed.
    RunFailed {
        /// Error description.
        reason: String,
    },
    /// A run was cancelled.
    RunCancelled,

    /// A step has started dispatching.
    StepStarted {
        /// Number of targets dispatched to.
        target_count: usize,
    },
    /// A step completed successfully.
    StepCompleted {
        /// Number of responses collected.
        collected: usize,
    },
    /// A step failed.
    StepFailed {
        /// Error description.
        reason: String,
    },
    /// A step timed out.
    StepTimedOut {
        /// Number of responses collected before timeout.
        collected: usize,
        /// Number of responses expected.
        expected: usize,
    },

    /// A dispatch was sent to a target.
    DispatchSent {
        /// Target meerkat ID.
        target_meerkat_id: String,
    },
    /// A response was collected from a target.
    ResponseCollected {
        /// Target meerkat ID.
        target_meerkat_id: String,
    },

    /// Topology policy violation detected.
    TopologyViolation {
        /// Source role.
        from_role: String,
        /// Destination role.
        to_role: String,
        /// Human-readable description.
        description: String,
    },
    /// Communication blocked by topology policy.
    TopologyBlocked {
        /// Source role.
        from_role: String,
        /// Destination role.
        to_role: String,
    },

    /// Supervisor detected an issue and escalated.
    SupervisorEscalation {
        /// Description of the escalation.
        description: String,
    },

    /// Reconcile report.
    ReconcileReport {
        /// Number of meerkats spawned.
        spawned: usize,
        /// Number of meerkats retired.
        retired: usize,
        /// Number of meerkats unchanged.
        unchanged: usize,
    },

    /// A degradation warning (e.g., tool bundle unavailable).
    DegradationWarning {
        /// Warning message.
        message: String,
    },
}
