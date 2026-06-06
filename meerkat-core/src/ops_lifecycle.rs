//! Canonical async-operation lifecycle seam for shared child/background work.

use serde::{Deserialize, Serialize};

use std::future::Future;
use std::pin::Pin;

use crate::comms::{PeerAddress, PeerId, TrustedPeerDescriptor};
use crate::lifecycle::{RunId, WaitRequestId};
pub use crate::ops::{OperationId, OperationResult};
use crate::runtime_epoch::EpochCursorState;
use crate::types::SessionId;

/// Default maximum number of completed operations to retain before eviction.
pub const DEFAULT_MAX_COMPLETED: usize = 256;

/// The kind of async operation tracked by the shared lifecycle registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    MobMemberChild,
    BackgroundToolOp,
    BackgroundToolCapacitySlot,
}

impl OperationKind {
    /// Closed variant set mirrored from the generated MeerkatMachine named type.
    pub const ALL: [Self; 3] = [
        Self::MobMemberChild,
        Self::BackgroundToolOp,
        Self::BackgroundToolCapacitySlot,
    ];

    /// Generated named-type variant spelling used by drift ratchets.
    pub const fn generated_variant(self) -> &'static str {
        match self {
            Self::MobMemberChild => "MobMemberChild",
            Self::BackgroundToolOp => "BackgroundToolOp",
            Self::BackgroundToolCapacitySlot => "BackgroundToolCapacitySlot",
        }
    }

    /// Whether this kind can expose a peer-ready handoff.
    pub fn expects_peer_channel(self) -> bool {
        matches!(self, Self::MobMemberChild)
    }
}

/// Generated-authority-owned source identity for an async operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OperationSource {
    SessionChild {
        session_id: SessionId,
    },
    BackendPeer {
        peer_id: PeerId,
        address: PeerAddress,
    },
}

impl OperationSource {
    pub fn session_child(session_id: SessionId) -> Self {
        Self::SessionChild { session_id }
    }

    pub fn backend_peer(peer_id: PeerId, address: PeerAddress) -> Self {
        Self::BackendPeer { peer_id, address }
    }
}

/// Lifecycle-relevant registration payload for an operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationSpec {
    pub id: OperationId,
    pub kind: OperationKind,
    /// Compatibility owner binding field.
    ///
    /// Under the identity-first Mob regime this carries the canonical bridge
    /// session binding, even though the stored field name still says
    /// `owner_session_id`.
    pub owner_session_id: SessionId,
    pub display_name: String,
    pub source_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_source: Option<OperationSource>,
    pub child_session_id: Option<SessionId>,
    pub expect_peer_channel: bool,
}

impl OperationSpec {
    /// Canonical owner bridge binding for this operation.
    pub fn owner_bridge_session_id(&self) -> &SessionId {
        &self.owner_session_id
    }

    /// Compatibility alias retained while older surfaces still speak in
    /// session-centric terms.
    pub fn owner_session_id(&self) -> &SessionId {
        &self.owner_session_id
    }
}

/// Peer-facing connection handoff surfaced once an operation is ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationPeerHandle {
    pub peer_name: crate::comms::PeerName,
    pub trusted_peer: TrustedPeerDescriptor,
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
    Aborted { reason: Option<String> },
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
    Aborted,
    Cancelled,
    Retired,
    Terminated,
}

impl OperationStatus {
    /// Stable string representation for app-facing surfaces.
    ///
    /// Unlike `Debug` format, this is an explicit mapping that won't
    /// produce uncontrolled strings when new variants are added.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Provisioning => "provisioning",
            Self::Running => "running",
            Self::Retiring => "retiring",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Aborted => "aborted",
            Self::Cancelled => "cancelled",
            Self::Retired => "retired",
            Self::Terminated => "terminated",
        }
    }
}

/// Public result class for an operation lifecycle snapshot.
///
/// This is the typed domain value emitted by generated operation lifecycle
/// authority before shell/tool surfaces project it into their presentation
/// structs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationPublicResultClass {
    MissingAuthority,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl OperationPublicResultClass {
    /// Closed variant set mirrored from the generated MeerkatMachine named type.
    pub const ALL: [Self; 5] = [
        Self::MissingAuthority,
        Self::Running,
        Self::Completed,
        Self::Failed,
        Self::Cancelled,
    ];

    /// Generated named-type variant spelling used by drift ratchets.
    pub const fn generated_variant(self) -> &'static str {
        match self {
            Self::MissingAuthority => "MissingAuthority",
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }
}

/// Generated classification for completion-feed entries that should wake the
/// owning agent as detached background job completions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationCompletionWakeClass {
    Wake,
    Ignore,
}

impl OperationCompletionWakeClass {
    /// Closed variant set mirrored from the generated MeerkatMachine named type.
    pub const ALL: [Self; 2] = [Self::Wake, Self::Ignore];

    /// Generated named-type variant spelling used by drift ratchets.
    pub const fn generated_variant(self) -> &'static str {
        match self {
            Self::Wake => "Wake",
            Self::Ignore => "Ignore",
        }
    }
}

/// Operation lifecycle action observed by generated transition feedback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationLifecycleAction {
    Start,
    Fail,
    PeerReady,
    ProgressReported,
    Complete,
    Abort,
    Cancel,
    RetireRequested,
    RetireCompleted,
    Terminate,
}

impl OperationLifecycleAction {
    /// Closed variant set mirrored from the generated MeerkatMachine named type.
    pub const ALL: [Self; 10] = [
        Self::Start,
        Self::Fail,
        Self::PeerReady,
        Self::ProgressReported,
        Self::Complete,
        Self::Abort,
        Self::Cancel,
        Self::RetireRequested,
        Self::RetireCompleted,
        Self::Terminate,
    ];

    /// Generated named-type variant spelling used by drift ratchets.
    pub const fn generated_variant(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Fail => "Fail",
            Self::PeerReady => "PeerReady",
            Self::ProgressReported => "ProgressReported",
            Self::Complete => "Complete",
            Self::Abort => "Abort",
            Self::Cancel => "Cancel",
            Self::RetireRequested => "RetireRequested",
            Self::RetireCompleted => "RetireCompleted",
            Self::Terminate => "Terminate",
        }
    }
}

/// Public snapshot of one operation's lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OperationLifecycleSnapshot {
    pub id: OperationId,
    pub kind: OperationKind,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_source: Option<OperationSource>,
    pub status: OperationStatus,
    /// Generated terminality classification for `status`.
    pub terminal: bool,
    /// Generated public result classification for `status`.
    pub public_result_class: OperationPublicResultClass,
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

/// One registry-owned watcher for a terminal lifecycle outcome.
///
/// This is a read-only waiter returned by [`OpsLifecycleRegistry`]. It does not
/// expose terminal-outcome send authority to callers; registry implementations
/// must resolve it only after generated operation lifecycle authority has
/// accepted the terminal transition.
pub type OperationCompletionWatch = Pin<
    Box<
        dyn Future<Output = Result<OperationTerminalOutcome, OperationCompletionWatchError>>
            + Send
            + 'static,
    >,
>;

/// Mechanical failure while waiting for operation completion plumbing.
///
/// This is intentionally not an [`OperationTerminalOutcome`]: a dropped waiter
/// channel is not async-operation terminal truth.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OperationCompletionWatchError {
    #[error("operation completion channel closed without an authorized terminal outcome")]
    ChannelClosed,
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
    #[error("wait_all already active")]
    WaitAlreadyActive,
    #[error("wait_all not active for request: {0}")]
    WaitNotActive(WaitRequestId),
    #[error("wait_all contains duplicate operation id: {0}")]
    DuplicateWaitOperation(OperationId),
    #[error("internal lifecycle registry error: {0}")]
    Internal(String),
}

/// Authority-owned result of `wait_all()`.
///
/// Carries the per-operation outcomes alongside an authority-derived
/// obligation token (`satisfied`). The obligation proves the authority owned
/// the wait request lifecycle and emitted `WaitAllSatisfied` when the tracked
/// barrier set became terminal.
#[derive(Debug)]
pub struct WaitAllResult {
    /// Per-operation terminal outcomes.
    pub outcomes: Vec<(OperationId, OperationTerminalOutcome)>,
    /// Authority-validated obligation token for the ops_barrier_satisfaction protocol.
    pub satisfied: WaitAllSatisfied,
}

/// Authority-owned obligation token emitted by the `WaitAllSatisfied` effect.
///
/// Created only by the `OpsLifecycleRegistry::wait_all()` implementation after
/// the authority resolves an outstanding wait request. Core-owned so it can be
/// consumed by the `protocol_ops_barrier_satisfaction` helper without crossing
/// crate boundaries.
#[derive(Debug)]
pub struct WaitAllSatisfied {
    /// The authority-owned wait request that reached satisfaction.
    pub wait_request_id: WaitRequestId,
    /// The run whose turn-state barrier was satisfied.
    pub run_id: RunId,
    /// The operation IDs validated as terminal by the authority.
    pub operation_ids: Vec<OperationId>,
}

/// Completion-feed consumer cursor owned by generated machine authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionCursorConsumer {
    /// Cursor advanced after the agent boundary has applied background notices.
    AgentApplied,
    /// Cursor advanced after the runtime loop has observed feed entries.
    RuntimeObserved,
    /// Cursor advanced after the runtime loop has injected detached continuation.
    RuntimeInjected,
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
    fn abort_provisioning(
        &self,
        id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), OpsLifecycleError>;
    fn cancel_operation(
        &self,
        id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), OpsLifecycleError>;
    fn request_retire(&self, id: &OperationId) -> Result<(), OpsLifecycleError>;
    fn mark_retired(&self, id: &OperationId) -> Result<(), OpsLifecycleError>;
    fn snapshot(
        &self,
        id: &OperationId,
    ) -> Result<Option<OperationLifecycleSnapshot>, OpsLifecycleError>;
    fn list_operations(&self) -> Result<Vec<OperationLifecycleSnapshot>, OpsLifecycleError>;
    fn terminate_owner(&self, reason: String) -> Result<(), OpsLifecycleError>;

    /// Classify operation terminality through the registry's generated
    /// lifecycle authority.
    fn classify_operation_terminality(
        &self,
        _id: &OperationId,
        _status: OperationStatus,
    ) -> Result<bool, OpsLifecycleError> {
        Err(OpsLifecycleError::Unsupported(
            "classify_operation_terminality".into(),
        ))
    }

    /// Classify operation public result projection through generated lifecycle
    /// authority.
    fn classify_operation_public_result(
        &self,
        _id: &OperationId,
    ) -> Result<OperationPublicResultClass, OpsLifecycleError> {
        Err(OpsLifecycleError::Unsupported(
            "classify_operation_public_result".into(),
        ))
    }

    /// Classify whether a completion-feed entry should wake the owning agent
    /// as a detached background job completion.
    fn classify_operation_completion_wake(
        &self,
        _id: &OperationId,
        _kind: OperationKind,
    ) -> Result<OperationCompletionWakeClass, OpsLifecycleError> {
        Err(OpsLifecycleError::Unsupported(
            "classify_operation_completion_wake".into(),
        ))
    }

    /// Classify whether a generated invalid-transition rejection is an
    /// idempotent success for the requested lifecycle action.
    fn classify_operation_transition_idempotence(
        &self,
        _id: &OperationId,
        _action: OperationLifecycleAction,
    ) -> Result<bool, OpsLifecycleError> {
        Err(OpsLifecycleError::Unsupported(
            "classify_operation_transition_idempotence".into(),
        ))
    }

    /// Register an operation while applying a caller-supplied generated
    /// admission limit for this registration.
    fn register_operation_with_admission_limit(
        &self,
        _spec: OperationSpec,
        _max_concurrent: Option<usize>,
    ) -> Result<(), OpsLifecycleError> {
        Err(OpsLifecycleError::Unsupported(
            "register_operation_with_admission_limit".into(),
        ))
    }

    /// Drain all completed operations from the registry, returning their outcomes.
    fn collect_completed(
        &self,
    ) -> Result<Vec<(OperationId, OperationTerminalOutcome)>, OpsLifecycleError> {
        Err(OpsLifecycleError::Unsupported("collect_completed".into()))
    }

    /// Return the canonical completion feed, if this registry supports it.
    ///
    /// Runtime-backed registries return a feed handle that consumers (agent
    /// boundary, idle wake) use for cursor-based completion delivery.
    /// Returns `None` for registries that don't support the feed protocol.
    fn completion_feed(
        &self,
    ) -> Option<std::sync::Arc<dyn crate::completion_feed::CompletionFeed>> {
        None
    }

    /// Read the generated completion-consumer cursor for this registry.
    ///
    /// `Ok(None)` means this registry has no generated cursor authority.
    /// `Err(OpsLifecycleError::Internal(_))` means the cursor authority is
    /// corrupt (e.g. a poisoned registry lock) and must not be laundered into
    /// the `None`/no-authority meaning by callers.
    fn completion_cursor(
        &self,
        _consumer: CompletionCursorConsumer,
    ) -> Result<Option<crate::completion_feed::CompletionSeq>, OpsLifecycleError> {
        Ok(None)
    }

    /// Advance a completion-consumer cursor through generated authority.
    ///
    /// Runtime-backed registries update `projection` only after the generated
    /// transition emits the matching cursor-advanced effect. The projection is
    /// an epoch-local cache, never the source of cursor truth.
    fn advance_completion_cursor(
        &self,
        _consumer: CompletionCursorConsumer,
        _cursor: crate::completion_feed::CompletionSeq,
        _projection: Option<&EpochCursorState>,
    ) -> Result<crate::completion_feed::CompletionSeq, OpsLifecycleError> {
        Err(OpsLifecycleError::Unsupported(
            "advance_completion_cursor".into(),
        ))
    }

    /// Register an authority-owned barrier wait and await its completion.
    ///
    /// Returns a [`WaitAllResult`] containing per-operation outcomes and an
    /// authority-owned obligation token. The runtime may host the async future,
    /// but wait completion truth comes from the registry authority emitting
    /// `WaitAllSatisfied`, not from shell watcher timing alone.
    fn wait_all(
        &self,
        _run_id: &RunId,
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

    #[test]
    fn operation_kind_peer_expectation_matches_contract() {
        assert!(OperationKind::MobMemberChild.expects_peer_channel());
        assert!(!OperationKind::BackgroundToolOp.expects_peer_channel());
        assert!(!OperationKind::BackgroundToolCapacitySlot.expects_peer_channel());
    }
}
