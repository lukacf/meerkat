//! Canonical async-operation lifecycle seam for shared child/background work.

use serde::{Deserialize, Serialize};

#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::oneshot;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::oneshot;

use crate::comms::TrustedPeerSpec;
pub use crate::ops::{OperationId, OperationResult};
use crate::types::SessionId;

/// Default maximum number of completed operations to retain before eviction.
pub const DEFAULT_MAX_COMPLETED: usize = 256;

/// The kind of async operation tracked by the shared lifecycle registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    MobMemberChild,
    BackgroundToolOp,
}

impl OperationKind {
    /// Whether this kind can expose a peer-ready handoff.
    pub fn expects_peer_channel(self) -> bool {
        matches!(self, Self::MobMemberChild)
    }
}

/// Lifecycle-relevant registration payload for an operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationSpec {
    pub id: OperationId,
    pub kind: OperationKind,
    pub owner_session_id: SessionId,
    pub display_name: String,
    pub source_label: String,
    pub child_session_id: Option<SessionId>,
    pub expect_peer_channel: bool,
}

/// Peer-facing connection handoff surfaced once an operation is ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationPeerHandle {
    pub peer_name: String,
    pub trusted_peer: TrustedPeerSpec,
}

/// Progress update for a long-running async operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationProgressUpdate {
    pub message: String,
    pub percent: Option<f32>,
}

/// Terminal lifecycle outcome recorded for an operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome_type", rename_all = "snake_case")]
pub enum OperationTerminalOutcome {
    Completed(OperationResult),
    Failed { error: String },
    Cancelled { reason: Option<String> },
    Retired,
    Terminated { reason: String },
}

/// Current lifecycle status for an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Absent,
    Provisioning,
    Running,
    Retiring,
    Completed,
    Failed,
    Cancelled,
    Retired,
    Terminated,
}

impl OperationStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Retired | Self::Terminated
        )
    }

    pub fn allows_terminalization(self) -> bool {
        matches!(self, Self::Provisioning | Self::Running | Self::Retiring)
    }
}

/// Public snapshot of one operation's lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationLifecycleSnapshot {
    pub id: OperationId,
    pub kind: OperationKind,
    pub display_name: String,
    pub status: OperationStatus,
    pub peer_ready: bool,
    pub progress_count: u32,
    pub watcher_count: u32,
    pub terminal_outcome: Option<OperationTerminalOutcome>,
    pub child_session_id: Option<SessionId>,
    /// Peer handle info (exposed when peer_ready is true).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub peer_handle: Option<OperationPeerHandle>,
    /// Wall-clock epoch millis when the operation was registered.
    #[serde(default)]
    pub created_at_ms: u64,
    /// Wall-clock epoch millis when provisioning succeeded (entered Running).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub started_at_ms: Option<u64>,
    /// Wall-clock epoch millis when the operation reached terminal state.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub completed_at_ms: Option<u64>,
    /// Monotonic elapsed millis from creation to terminal (computed from Instant).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub elapsed_ms: Option<u64>,
}

/// One watcher for a terminal lifecycle outcome.
pub struct OperationCompletionWatch {
    rx: oneshot::Receiver<OperationTerminalOutcome>,
}

impl OperationCompletionWatch {
    /// Create a pending watch and its sender.
    pub fn channel() -> (
        oneshot::Sender<OperationTerminalOutcome>,
        OperationCompletionWatch,
    ) {
        let (tx, rx) = oneshot::channel();
        (tx, Self { rx })
    }

    /// Await the operation's terminal outcome.
    pub async fn wait(self) -> OperationTerminalOutcome {
        match self.rx.await {
            Ok(outcome) => outcome,
            Err(_) => OperationTerminalOutcome::Terminated {
                reason: "operation completion watch dropped".into(),
            },
        }
    }

    /// Create a watch that is already resolved.
    pub fn already_resolved(outcome: OperationTerminalOutcome) -> Self {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(outcome);
        Self { rx }
    }
}

/// Errors returned by the shared lifecycle registry.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum OpsLifecycleError {
    #[error("operation already registered: {0}")]
    AlreadyRegistered(OperationId),
    #[error("operation not found: {0}")]
    NotFound(OperationId),
    #[error("invalid lifecycle transition for {id}: {status:?} -> {action}")]
    InvalidTransition {
        id: OperationId,
        status: OperationStatus,
        action: &'static str,
    },
    #[error("operation does not expect a peer handoff: {0}")]
    PeerNotExpected(OperationId),
    #[error("operation is already peer-ready: {0}")]
    AlreadyPeerReady(OperationId),
    #[error("max concurrent operations exceeded (limit: {limit}, active: {active})")]
    MaxConcurrentExceeded { limit: usize, active: usize },
    #[error("operation not supported: {0}")]
    Unsupported(String),
    #[error("internal lifecycle registry error: {0}")]
    Internal(String),
}

/// Authority-validated result of `wait_all()`.
///
/// Carries the per-operation outcomes alongside an authority-derived
/// obligation token (`satisfied`). The obligation proves the authority
/// validated every operation as terminal before emitting `WaitAllSatisfied`.
#[derive(Debug)]
pub struct WaitAllResult {
    /// Per-operation terminal outcomes.
    pub outcomes: Vec<(OperationId, OperationTerminalOutcome)>,
    /// Authority-validated obligation token for the ops_barrier_satisfaction protocol.
    pub satisfied: WaitAllSatisfied,
}

/// Authority-validated obligation token emitted by the `WaitAllSatisfied` effect.
///
/// Created only by the `OpsLifecycleRegistry::wait_all()` implementation after
/// the authority validates all operations are terminal. Core-owned so it can be
/// consumed by the `protocol_ops_barrier_satisfaction` helper without crossing
/// crate boundaries.
#[derive(Debug)]
pub struct WaitAllSatisfied {
    /// The operation IDs validated as terminal by the authority.
    pub operation_ids: Vec<OperationId>,
}

/// Shared async-operation lifecycle registry.
pub trait OpsLifecycleRegistry: Send + Sync {
    fn register_operation(&self, spec: OperationSpec) -> Result<(), OpsLifecycleError>;
    fn provisioning_succeeded(&self, id: &OperationId) -> Result<(), OpsLifecycleError>;
    fn provisioning_failed(&self, id: &OperationId, error: String)
    -> Result<(), OpsLifecycleError>;
    fn peer_ready(
        &self,
        id: &OperationId,
        peer: OperationPeerHandle,
    ) -> Result<(), OpsLifecycleError>;
    fn register_watcher(
        &self,
        id: &OperationId,
    ) -> Result<OperationCompletionWatch, OpsLifecycleError>;
    fn report_progress(
        &self,
        id: &OperationId,
        update: OperationProgressUpdate,
    ) -> Result<(), OpsLifecycleError>;
    fn complete_operation(
        &self,
        id: &OperationId,
        result: OperationResult,
    ) -> Result<(), OpsLifecycleError>;
    fn fail_operation(&self, id: &OperationId, error: String) -> Result<(), OpsLifecycleError>;
    fn cancel_operation(
        &self,
        id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), OpsLifecycleError>;
    fn request_retire(&self, id: &OperationId) -> Result<(), OpsLifecycleError>;
    fn mark_retired(&self, id: &OperationId) -> Result<(), OpsLifecycleError>;
    fn snapshot(&self, id: &OperationId) -> Option<OperationLifecycleSnapshot>;
    fn list_operations(&self) -> Vec<OperationLifecycleSnapshot>;
    fn terminate_owner(&self, reason: String) -> Result<(), OpsLifecycleError>;

    /// Drain all completed operations from the registry, returning their outcomes.
    fn collect_completed(
        &self,
    ) -> Result<Vec<(OperationId, OperationTerminalOutcome)>, OpsLifecycleError> {
        Err(OpsLifecycleError::Unsupported("collect_completed".into()))
    }

    /// Register a completion watcher for each ID and await all of them.
    ///
    /// Returns a [`WaitAllResult`] containing per-operation outcomes and an
    /// authority-validated obligation token. The authority validates that every
    /// operation is terminal before emitting `WaitAllSatisfied`.
    fn wait_all(
        &self,
        _ids: &[OperationId],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<WaitAllResult, OpsLifecycleError>> + Send + '_>,
    > {
        Box::pin(std::future::ready(Err(OpsLifecycleError::Unsupported(
            "wait_all".into(),
        ))))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn already_resolved_watch_returns_terminal_outcome() {
        let watch = OperationCompletionWatch::already_resolved(OperationTerminalOutcome::Retired);
        assert_eq!(watch.wait().await, OperationTerminalOutcome::Retired);
    }

    #[test]
    fn operation_kind_peer_expectation_matches_contract() {
        assert!(OperationKind::MobMemberChild.expects_peer_channel());
        assert!(!OperationKind::BackgroundToolOp.expects_peer_channel());
    }

    #[test]
    fn terminal_statuses_match_contract() {
        assert!(OperationStatus::Completed.is_terminal());
        assert!(OperationStatus::Failed.is_terminal());
        assert!(OperationStatus::Cancelled.is_terminal());
        assert!(OperationStatus::Retired.is_terminal());
        assert!(OperationStatus::Terminated.is_terminal());
        assert!(!OperationStatus::Running.is_terminal());
        assert!(OperationStatus::Running.allows_terminalization());
        assert!(OperationStatus::Retiring.allows_terminalization());
        assert!(!OperationStatus::Completed.allows_terminalization());
    }
}
