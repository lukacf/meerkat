//! In-memory runtime implementation of the shared async-operation lifecycle seam.
//!
//! All canonical lifecycle state mutations are delegated to
//! [`OpsLifecycleAuthority`] via [`OpsLifecycleMutator::apply`]. This shell
//! layer owns I/O concerns: watcher channels, timestamps, peer handles, and
//! snapshot assembly.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::task::{Context, Poll};

#[cfg(target_arch = "wasm32")]
use crate::tokio;
use meerkat_core::lifecycle::{RunId, WaitRequestId};
use meerkat_core::ops_lifecycle::{
    DEFAULT_MAX_COMPLETED, OperationCompletionWatch, OperationId, OperationLifecycleSnapshot,
    OperationPeerHandle, OperationProgressUpdate, OperationResult, OperationSpec,
    OperationTerminalOutcome, OpsLifecycleError, OpsLifecycleRegistry, WaitAllResult,
    WaitAllSatisfied,
};
use meerkat_core::time_compat::{Instant, SystemTime, UNIX_EPOCH};

use crate::ops_lifecycle_authority::{
    OpsLifecycleAuthority, OpsLifecycleEffect, OpsLifecycleInput, OpsLifecycleMutator,
};

// ---------------------------------------------------------------------------
// Shell-only per-operation record (not part of canonical machine state)
// ---------------------------------------------------------------------------

/// Shell-owned data for a single operation. Canonical lifecycle state lives in
/// the authority; this struct holds I/O concerns that the authority has no
/// knowledge of.
#[derive(Debug)]
struct ShellRecord {
    spec: OperationSpec,
    peer_handle: Option<OperationPeerHandle>,
    watchers: Vec<tokio::sync::oneshot::Sender<OperationTerminalOutcome>>,
    // Monotonic timestamps for elapsed computation
    created_at: Instant,
    started_at: Option<Instant>,
    completed_at: Option<Instant>,
    // Wall-clock anchor captured at creation for epoch millis
    created_at_wall: SystemTime,
}

#[derive(Debug)]
struct PendingWaitState {
    wait_request_id: WaitRequestId,
    sender: tokio::sync::oneshot::Sender<WaitAllSatisfied>,
}

impl ShellRecord {
    fn new(spec: OperationSpec) -> Self {
        Self {
            spec,
            peer_handle: None,
            watchers: Vec::new(),
            created_at: Instant::now(),
            started_at: None,
            completed_at: None,
            created_at_wall: SystemTime::now(),
        }
    }

    fn epoch_millis(wall_anchor: &SystemTime) -> u64 {
        wall_anchor
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn epoch_millis_for_instant(&self, instant: Instant) -> u64 {
        // Compute wall time for a given instant using the wall-clock anchor:
        // wall_time = created_at_wall + (instant - created_at)
        let offset = instant.saturating_duration_since(self.created_at);
        let wall = self.created_at_wall + offset;
        Self::epoch_millis(&wall)
    }

    /// Notify all watchers with the given terminal outcome and drain the list.
    fn notify_watchers(&mut self, outcome: &OperationTerminalOutcome) {
        for watcher in std::mem::take(&mut self.watchers) {
            let _ = watcher.send(outcome.clone());
        }
    }

    /// Mark the completion timestamp.
    fn mark_completed(&mut self) {
        self.completed_at = Some(Instant::now());
    }
}

// ---------------------------------------------------------------------------
// Combined shell state: authority + shell records
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ShellState {
    authority: OpsLifecycleAuthority,
    records: HashMap<OperationId, ShellRecord>,
    pending_wait: Option<PendingWaitState>,
}

impl ShellState {
    fn new(max_completed: usize, max_concurrent: Option<usize>) -> Self {
        Self {
            authority: OpsLifecycleAuthority::new(max_completed, max_concurrent),
            records: HashMap::new(),
            pending_wait: None,
        }
    }

    /// Build a snapshot by combining authority canonical state with shell data.
    fn snapshot(&self, id: &OperationId) -> Option<OperationLifecycleSnapshot> {
        let canonical = self.authority.operation(id)?;
        let shell = self.records.get(id)?;

        let created_at_ms = ShellRecord::epoch_millis(&shell.created_at_wall);
        let started_at_ms = shell.started_at.map(|i| shell.epoch_millis_for_instant(i));
        let completed_at_ms = shell
            .completed_at
            .map(|i| shell.epoch_millis_for_instant(i));
        let elapsed_ms = shell.completed_at.map(|completed| {
            completed
                .saturating_duration_since(shell.created_at)
                .as_millis() as u64
        });

        Some(OperationLifecycleSnapshot {
            id: shell.spec.id.clone(),
            kind: canonical.kind(),
            display_name: shell.spec.display_name.clone(),
            status: canonical.status(),
            peer_ready: canonical.peer_ready(),
            progress_count: canonical.progress_count(),
            watcher_count: shell.watchers.len() as u32,
            terminal_outcome: canonical.terminal_outcome().cloned(),
            child_session_id: shell.spec.child_session_id.clone(),
            peer_handle: shell.peer_handle.clone(),
            created_at_ms,
            started_at_ms,
            completed_at_ms,
            elapsed_ms,
        })
    }

    /// Execute authority effects on shell state.
    ///
    /// **Important:** callers must patch the real terminal outcome on the
    /// authority (via `patch_terminal_outcome`) *before* calling this method.
    /// `NotifyOpWatcher` effects read the patched outcome from the authority
    /// rather than using the placeholder embedded in the effect.
    fn execute_effects(&mut self, effects: &[OpsLifecycleEffect]) {
        for effect in effects {
            match effect {
                OpsLifecycleEffect::NotifyOpWatcher { operation_id, .. } => {
                    // Read the real (patched) outcome from the authority.
                    let outcome = self
                        .authority
                        .operation(operation_id)
                        .and_then(|op| op.terminal_outcome().cloned());
                    if let Some(outcome) = outcome
                        && let Some(shell) = self.records.get_mut(operation_id)
                    {
                        let watcher_count = shell.watchers.len() as u32;
                        shell.notify_watchers(&outcome);
                        shell.mark_completed();
                        self.authority.watchers_drained(operation_id, watcher_count);
                    }
                }
                OpsLifecycleEffect::ExposeOperationPeer { .. } => {
                    // Peer handle is stored in shell record by the calling method
                    // after authority.apply() succeeds. Nothing else to do here.
                }
                OpsLifecycleEffect::RetainTerminalRecord { .. } => {
                    // The authority handles completed_order tracking internally.
                    // Shell record stays in place until evicted.
                }
                OpsLifecycleEffect::EvictCompletedRecord { operation_id } => {
                    self.records.remove(operation_id);
                    self.authority.remove_operation(operation_id);
                }
                OpsLifecycleEffect::SubmitOpEvent { .. } => {
                    // Future: emit observability events. Currently a no-op.
                }
                OpsLifecycleEffect::WaitAllSatisfied {
                    wait_request_id,
                    operation_ids,
                } => {
                    if let Some(pending_wait) = self.pending_wait.take() {
                        if pending_wait.wait_request_id == *wait_request_id {
                            let _ = pending_wait.sender.send(WaitAllSatisfied {
                                wait_request_id: wait_request_id.clone(),
                                operation_ids: operation_ids.clone(),
                            });
                        } else {
                            self.pending_wait = Some(pending_wait);
                        }
                    }
                }
            }
        }
    }

    fn shell_record_mut(
        &mut self,
        id: &OperationId,
    ) -> Result<&mut ShellRecord, OpsLifecycleError> {
        self.records
            .get_mut(id)
            .ok_or_else(|| OpsLifecycleError::NotFound(id.clone()))
    }

    fn collect_wait_outcomes(
        &self,
        operation_ids: &[OperationId],
    ) -> Result<Vec<(OperationId, OperationTerminalOutcome)>, OpsLifecycleError> {
        operation_ids
            .iter()
            .map(|operation_id| {
                let outcome = self
                    .authority
                    .operation(operation_id)
                    .and_then(|op| op.terminal_outcome().cloned())
                    .ok_or_else(|| {
                        OpsLifecycleError::Internal(format!(
                            "wait_all completed without terminal outcome for {operation_id}"
                        ))
                    })?;
                Ok((operation_id.clone(), outcome))
            })
            .collect()
    }
}

impl Default for ShellState {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_COMPLETED, None)
    }
}

// ---------------------------------------------------------------------------
// Public configuration & registry
// ---------------------------------------------------------------------------

/// Configuration for [`RuntimeOpsLifecycleRegistry`].
#[derive(Debug, Clone)]
pub struct OpsLifecycleConfig {
    /// Maximum number of completed operations to retain (default: 256).
    pub max_completed: usize,
    /// Maximum concurrent non-terminal operations (None = unlimited).
    pub max_concurrent: Option<usize>,
}

impl Default for OpsLifecycleConfig {
    fn default() -> Self {
        Self {
            max_completed: DEFAULT_MAX_COMPLETED,
            max_concurrent: None,
        }
    }
}

/// Per-runtime shared registry for async operation lifecycle truth.
///
/// All canonical lifecycle state mutations are delegated to
/// [`OpsLifecycleAuthority`]. This shell manages I/O concerns: watcher
/// channels, timestamps, peer handles, and snapshot assembly.
#[derive(Debug)]
pub struct RuntimeOpsLifecycleRegistry {
    state: RwLock<ShellState>,
}

impl Default for RuntimeOpsLifecycleRegistry {
    fn default() -> Self {
        Self {
            state: RwLock::new(ShellState::default()),
        }
    }
}

impl RuntimeOpsLifecycleRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: OpsLifecycleConfig) -> Self {
        Self {
            state: RwLock::new(ShellState::new(config.max_completed, config.max_concurrent)),
        }
    }

    fn read_state(&self) -> Result<RwLockReadGuard<'_, ShellState>, OpsLifecycleError> {
        self.state
            .read()
            .map_err(|_| OpsLifecycleError::Internal("ops lifecycle registry poisoned".into()))
    }

    fn write_state(&self) -> Result<RwLockWriteGuard<'_, ShellState>, OpsLifecycleError> {
        self.state
            .write()
            .map_err(|_| OpsLifecycleError::Internal("ops lifecycle registry poisoned".into()))
    }

    fn cancel_wait_all_internal(
        &self,
        wait_request_id: &WaitRequestId,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        match state.authority.apply(OpsLifecycleInput::CancelWaitAll {
            wait_request_id: wait_request_id.clone(),
        }) {
            Ok(_) => {
                state.pending_wait = None;
                Ok(())
            }
            Err(OpsLifecycleError::WaitNotActive(_)) => {
                state.pending_wait = None;
                Ok(())
            }
            Err(err) => Err(err),
        }
    }
}

enum WaitAllFutureState {
    Ready(Option<Result<WaitAllResult, OpsLifecycleError>>),
    Waiting(tokio::sync::oneshot::Receiver<WaitAllSatisfied>),
    Done,
}

struct WaitAllFuture<'a> {
    registry: &'a RuntimeOpsLifecycleRegistry,
    wait_request_id: WaitRequestId,
    operation_ids: Vec<OperationId>,
    state: WaitAllFutureState,
}

impl Future for WaitAllFuture<'_> {
    type Output = Result<WaitAllResult, OpsLifecycleError>;

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match &mut self.state {
            WaitAllFutureState::Ready(result) => {
                let ready = result.take().unwrap_or_else(|| {
                    Err(OpsLifecycleError::Internal(
                        "wait_all future polled after completion".into(),
                    ))
                });
                self.state = WaitAllFutureState::Done;
                Poll::Ready(ready)
            }
            WaitAllFutureState::Waiting(receiver) => match std::pin::Pin::new(receiver).poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Ok(satisfied)) => {
                    let outcomes = match self.registry.read_state() {
                        Ok(state) => state.collect_wait_outcomes(&self.operation_ids),
                        Err(err) => Err(err),
                    };
                    self.state = WaitAllFutureState::Done;
                    Poll::Ready(outcomes.map(|outcomes| WaitAllResult {
                        outcomes,
                        satisfied,
                    }))
                }
                Poll::Ready(Err(_)) => {
                    self.state = WaitAllFutureState::Done;
                    Poll::Ready(Err(OpsLifecycleError::Internal(
                        "wait_all completion channel dropped".into(),
                    )))
                }
            },
            WaitAllFutureState::Done => Poll::Ready(Err(OpsLifecycleError::Internal(
                "wait_all future polled after completion".into(),
            ))),
        }
    }
}

impl Drop for WaitAllFuture<'_> {
    fn drop(&mut self) {
        if matches!(self.state, WaitAllFutureState::Waiting(_)) {
            let _ = self
                .registry
                .cancel_wait_all_internal(&self.wait_request_id);
        }
    }
}

impl OpsLifecycleRegistry for RuntimeOpsLifecycleRegistry {
    fn register_operation(&self, spec: OperationSpec) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        let operation_id = spec.id.clone();
        let kind = spec.kind;

        // Delegate to authority for guard checks and canonical state insertion.
        let transition = state
            .authority
            .apply(OpsLifecycleInput::RegisterOperation {
                operation_id: operation_id.clone(),
                kind,
            })?;

        // Insert shell record.
        state.records.insert(operation_id, ShellRecord::new(spec));

        // Execute effects (none expected for register, but be correct).
        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn provisioning_succeeded(&self, id: &OperationId) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state
            .authority
            .apply(OpsLifecycleInput::ProvisioningSucceeded {
                operation_id: id.clone(),
            })?;

        // Shell concern: record the started timestamp.
        if let Some(shell) = state.records.get_mut(id) {
            shell.started_at = Some(Instant::now());
        }

        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn provisioning_failed(
        &self,
        id: &OperationId,
        error: String,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state
            .authority
            .apply(OpsLifecycleInput::ProvisioningFailed {
                operation_id: id.clone(),
            })?;

        // Patch the real terminal outcome (authority uses placeholder).
        state
            .authority
            .patch_terminal_outcome(id, OperationTerminalOutcome::Failed { error });

        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn peer_ready(
        &self,
        id: &OperationId,
        peer: OperationPeerHandle,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state.authority.apply(OpsLifecycleInput::PeerReady {
            operation_id: id.clone(),
        })?;

        // Shell concern: store the peer handle.
        if let Some(shell) = state.records.get_mut(id) {
            shell.peer_handle = Some(peer);
        }

        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn register_watcher(
        &self,
        id: &OperationId,
    ) -> Result<OperationCompletionWatch, OpsLifecycleError> {
        let mut state = self.write_state()?;

        // Check authority for terminal outcome first (already-resolved path).
        let canonical = state
            .authority
            .operation(id)
            .ok_or_else(|| OpsLifecycleError::NotFound(id.clone()))?;

        if let Some(outcome) = canonical.terminal_outcome() {
            return Ok(OperationCompletionWatch::already_resolved(outcome.clone()));
        }

        // Delegate to authority for watcher_count bookkeeping.
        let _transition = state.authority.apply(OpsLifecycleInput::RegisterWatcher {
            operation_id: id.clone(),
        })?;

        // Shell concern: create the channel and store the sender.
        let shell = state.shell_record_mut(id)?;
        let (tx, watch) = OperationCompletionWatch::channel();
        shell.watchers.push(tx);
        Ok(watch)
    }

    fn report_progress(
        &self,
        id: &OperationId,
        _update: OperationProgressUpdate,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state.authority.apply(OpsLifecycleInput::ProgressReported {
            operation_id: id.clone(),
        })?;

        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn complete_operation(
        &self,
        id: &OperationId,
        result: OperationResult,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state
            .authority
            .apply(OpsLifecycleInput::CompleteOperation {
                operation_id: id.clone(),
            })?;

        // Patch the real terminal outcome (authority uses placeholder).
        state
            .authority
            .patch_terminal_outcome(id, OperationTerminalOutcome::Completed(result));

        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn fail_operation(&self, id: &OperationId, error: String) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state.authority.apply(OpsLifecycleInput::FailOperation {
            operation_id: id.clone(),
        })?;

        // Patch the real terminal outcome.
        state
            .authority
            .patch_terminal_outcome(id, OperationTerminalOutcome::Failed { error });

        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn abort_provisioning(
        &self,
        id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state
            .authority
            .apply(OpsLifecycleInput::AbortProvisioning {
                operation_id: id.clone(),
            })?;

        state
            .authority
            .patch_terminal_outcome(id, OperationTerminalOutcome::Aborted { reason });

        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn cancel_operation(
        &self,
        id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state.authority.apply(OpsLifecycleInput::CancelOperation {
            operation_id: id.clone(),
        })?;

        // Patch the real terminal outcome.
        state
            .authority
            .patch_terminal_outcome(id, OperationTerminalOutcome::Cancelled { reason });

        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn request_retire(&self, id: &OperationId) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state.authority.apply(OpsLifecycleInput::RetireRequested {
            operation_id: id.clone(),
        })?;

        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn mark_retired(&self, id: &OperationId) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state.authority.apply(OpsLifecycleInput::RetireCompleted {
            operation_id: id.clone(),
        })?;

        // Patch the real terminal outcome.
        state
            .authority
            .patch_terminal_outcome(id, OperationTerminalOutcome::Retired);

        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn snapshot(&self, id: &OperationId) -> Option<OperationLifecycleSnapshot> {
        self.read_state().ok().and_then(|state| state.snapshot(id))
    }

    fn list_operations(&self) -> Vec<OperationLifecycleSnapshot> {
        let mut snapshots = self
            .read_state()
            .map(|state| {
                state
                    .authority
                    .operations()
                    .filter_map(|(id, _)| state.snapshot(id))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        snapshots.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        snapshots
    }

    fn terminate_owner(&self, reason: String) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;

        let transition = state.authority.apply(OpsLifecycleInput::OwnerTerminated)?;

        // Patch all terminal outcomes with the real reason.
        // The authority set placeholder empty-string reasons; we patch the real
        // reason into each newly-terminated operation.
        for effect in &transition.effects {
            if let OpsLifecycleEffect::NotifyOpWatcher { operation_id, .. } = effect {
                state.authority.patch_terminal_outcome(
                    operation_id,
                    OperationTerminalOutcome::Terminated {
                        reason: reason.clone(),
                    },
                );
            }
        }

        state.execute_effects(&transition.effects);
        Ok(())
    }

    fn collect_completed(
        &self,
    ) -> Result<Vec<(OperationId, OperationTerminalOutcome)>, OpsLifecycleError> {
        let mut state = self.write_state()?;

        let collected = state.authority.drain_completed();

        // Remove corresponding shell records.
        for (id, _) in &collected {
            state.records.remove(id);
        }

        Ok(collected)
    }

    fn wait_all(
        &self,
        _run_id: &RunId,
        ids: &[OperationId],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<WaitAllResult, OpsLifecycleError>> + Send + '_>,
    > {
        let wait_request_id = WaitRequestId::new();
        let owned_ids = ids.to_vec();

        let state = match self.write_state() {
            Ok(mut state) => {
                let transition = match state.authority.apply(OpsLifecycleInput::BeginWaitAll {
                    wait_request_id: wait_request_id.clone(),
                    operation_ids: owned_ids.clone(),
                }) {
                    Ok(transition) => transition,
                    Err(err) => {
                        return Box::pin(WaitAllFuture {
                            registry: self,
                            wait_request_id,
                            operation_ids: owned_ids,
                            state: WaitAllFutureState::Ready(Some(Err(err))),
                        });
                    }
                };

                let satisfied = transition.effects.iter().find_map(|effect| match effect {
                    OpsLifecycleEffect::WaitAllSatisfied {
                        wait_request_id,
                        operation_ids,
                    } => Some(WaitAllSatisfied {
                        wait_request_id: wait_request_id.clone(),
                        operation_ids: operation_ids.clone(),
                    }),
                    _ => None,
                });

                state.execute_effects(&transition.effects);

                if let Some(satisfied) = satisfied {
                    WaitAllFutureState::Ready(Some(state.collect_wait_outcomes(&owned_ids).map(
                        |outcomes| WaitAllResult {
                            outcomes,
                            satisfied,
                        },
                    )))
                } else {
                    if state.pending_wait.is_some() {
                        return Box::pin(WaitAllFuture {
                            registry: self,
                            wait_request_id,
                            operation_ids: owned_ids,
                            state: WaitAllFutureState::Ready(Some(Err(
                                OpsLifecycleError::Internal(
                                    "wait_all started while a pending wait sender already existed"
                                        .into(),
                                ),
                            ))),
                        });
                    }
                    let (sender, receiver) = tokio::sync::oneshot::channel();
                    state.pending_wait = Some(PendingWaitState {
                        wait_request_id: wait_request_id.clone(),
                        sender,
                    });
                    WaitAllFutureState::Waiting(receiver)
                }
            }
            Err(err) => WaitAllFutureState::Ready(Some(Err(err))),
        };

        Box::pin(WaitAllFuture {
            registry: self,
            wait_request_id,
            operation_ids: owned_ids,
            state,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::comms::TrustedPeerSpec;
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::ops_lifecycle::{OperationKind, OpsLifecycleRegistry};
    use meerkat_core::types::SessionId;
    use uuid::Uuid;

    fn test_run_id() -> RunId {
        RunId(Uuid::from_u128(1))
    }

    fn background_spec(name: &str) -> OperationSpec {
        OperationSpec {
            id: OperationId::new(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: SessionId::new(),
            display_name: name.into(),
            source_label: "test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        }
    }

    #[tokio::test]
    async fn late_watchers_resolve_immediately() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("late");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();
        registry
            .complete_operation(
                &op_id,
                OperationResult {
                    id: op_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        let watch = registry.register_watcher(&op_id).unwrap();
        match watch.wait().await {
            OperationTerminalOutcome::Completed(result) => assert_eq!(result.content, "done"),
            other => panic!("expected completed outcome, got {other:?}"),
        }
    }

    #[test]
    fn peer_ready_requires_peer_expectation() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("no-peer");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();

        let result = registry.peer_ready(
            &op_id,
            OperationPeerHandle {
                peer_name: "peer".into(),
                trusted_peer: TrustedPeerSpec::new("peer", "peer-id", "inproc://peer").unwrap(),
            },
        );
        assert!(matches!(result, Err(OpsLifecycleError::PeerNotExpected(_))));
    }

    #[tokio::test]
    async fn multi_listener_completion() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("multi");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();

        let watch1 = registry.register_watcher(&op_id).unwrap();
        let watch2 = registry.register_watcher(&op_id).unwrap();
        let watch3 = registry.register_watcher(&op_id).unwrap();

        registry
            .complete_operation(
                &op_id,
                OperationResult {
                    id: op_id.clone(),
                    content: "multi-done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        for watch in [watch1, watch2, watch3] {
            match watch.wait().await {
                OperationTerminalOutcome::Completed(result) => {
                    assert_eq!(result.content, "multi-done");
                }
                other => panic!("expected completed, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn wait_all_returns_all_outcomes() {
        let registry = RuntimeOpsLifecycleRegistry::new();

        let spec_a = background_spec("a");
        let id_a = spec_a.id.clone();
        registry.register_operation(spec_a).unwrap();
        registry.provisioning_succeeded(&id_a).unwrap();

        let spec_b = background_spec("b");
        let id_b = spec_b.id.clone();
        registry.register_operation(spec_b).unwrap();
        registry.provisioning_succeeded(&id_b).unwrap();

        registry
            .complete_operation(
                &id_a,
                OperationResult {
                    id: id_a.clone(),
                    content: "a-done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();
        registry.fail_operation(&id_b, "b-error".into()).unwrap();

        let wait_result = registry
            .wait_all(&test_run_id(), &[id_a.clone(), id_b.clone()])
            .await
            .unwrap();
        assert_eq!(wait_result.outcomes.len(), 2);
        assert_eq!(wait_result.outcomes[0].0, id_a);
        assert!(matches!(
            wait_result.outcomes[0].1,
            OperationTerminalOutcome::Completed(_)
        ));
        assert_eq!(wait_result.outcomes[1].0, id_b);
        assert!(matches!(
            wait_result.outcomes[1].1,
            OperationTerminalOutcome::Failed { .. }
        ));
        // Authority-derived obligation carries the awaited IDs
        assert_eq!(wait_result.satisfied.operation_ids.len(), 2);
        assert_ne!(wait_result.satisfied.wait_request_id.to_string(), "");
    }

    /// Exercises the trait `wait_all` path (via `dyn OpsLifecycleRegistry`)
    /// which must submit WaitAll through the authority for cross-machine handoff.
    #[tokio::test]
    async fn wait_all_trait_path_submits_through_authority() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("trait-wait");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();
        registry
            .complete_operation(
                &op_id,
                OperationResult {
                    id: op_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        // Call through trait object to exercise the trait impl, not the inherent method.
        let trait_ref: &dyn OpsLifecycleRegistry = &registry;
        let wait_result = trait_ref
            .wait_all(&test_run_id(), std::slice::from_ref(&op_id))
            .await
            .unwrap();
        assert_eq!(wait_result.outcomes.len(), 1);
        assert!(matches!(
            wait_result.outcomes[0].1,
            OperationTerminalOutcome::Completed(_)
        ));
        // Obligation carries the validated ID
        assert_eq!(wait_result.satisfied.operation_ids, vec![op_id]);
        assert_ne!(wait_result.satisfied.wait_request_id.to_string(), "");
    }

    #[tokio::test]
    async fn wait_all_resolves_from_authority_owned_wait_request() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let run_id = test_run_id();

        let spec = background_spec("pending");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();

        let wait_fut = registry.wait_all(&run_id, std::slice::from_ref(&op_id));
        tokio::pin!(wait_fut);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), &mut wait_fut)
                .await
                .is_err()
        );

        let active_wait_request_id = {
            let state = registry.read_state().unwrap();
            let wait_request_id = match state.authority.wait_request_id().cloned() {
                Some(wait_request_id) => wait_request_id,
                None => panic!("wait request should be active"),
            };
            assert_eq!(
                state.authority.wait_operation_ids(),
                std::slice::from_ref(&op_id)
            );
            wait_request_id
        };

        registry
            .complete_operation(
                &op_id,
                OperationResult {
                    id: op_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        let wait_result = wait_fut.await.unwrap();
        assert_eq!(
            wait_result.satisfied.wait_request_id,
            active_wait_request_id
        );
        assert_eq!(wait_result.satisfied.operation_ids, vec![op_id.clone()]);
        assert!(matches!(
            wait_result.outcomes.as_slice(),
            [(returned_id, OperationTerminalOutcome::Completed(_))] if *returned_id == op_id
        ));
        assert!(
            registry
                .read_state()
                .unwrap()
                .authority
                .wait_request_id()
                .is_none()
        );
    }

    #[tokio::test]
    async fn dropping_wait_all_future_cancels_active_wait_request() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let run_id = test_run_id();

        let spec = background_spec("cancelled-wait");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();

        let wait_fut = registry.wait_all(&run_id, std::slice::from_ref(&op_id));
        drop(wait_fut);

        let state = registry.read_state().unwrap();
        assert!(state.authority.wait_request_id().is_none());
        assert!(state.authority.wait_operation_ids().is_empty());
    }

    #[test]
    fn collect_completed_drains_terminal_operations() {
        let registry = RuntimeOpsLifecycleRegistry::new();

        let spec_a = background_spec("a");
        let id_a = spec_a.id.clone();
        registry.register_operation(spec_a).unwrap();
        registry.provisioning_succeeded(&id_a).unwrap();
        registry
            .complete_operation(
                &id_a,
                OperationResult {
                    id: id_a.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        let spec_b = background_spec("b");
        let id_b = spec_b.id.clone();
        registry.register_operation(spec_b).unwrap();

        let collected = registry.collect_completed().unwrap();
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].0, id_a);

        assert!(registry.snapshot(&id_a).is_none());
        assert!(registry.snapshot(&id_b).is_some());

        let collected2 = registry.collect_completed().unwrap();
        assert!(collected2.is_empty());
    }

    #[test]
    fn bounded_completed_retention_evicts_oldest() {
        let registry = RuntimeOpsLifecycleRegistry::with_config(OpsLifecycleConfig {
            max_completed: 3,
            max_concurrent: None,
        });

        let mut ids = Vec::new();
        for i in 0..5 {
            let spec = background_spec(&format!("op-{i}"));
            let id = spec.id.clone();
            registry.register_operation(spec).unwrap();
            registry.provisioning_succeeded(&id).unwrap();
            registry
                .complete_operation(
                    &id,
                    OperationResult {
                        id: id.clone(),
                        content: format!("done-{i}"),
                        is_error: false,
                        duration_ms: 1,
                        tokens_used: 0,
                    },
                )
                .unwrap();
            ids.push(id);
        }

        assert!(registry.snapshot(&ids[0]).is_none());
        assert!(registry.snapshot(&ids[1]).is_none());
        assert!(registry.snapshot(&ids[2]).is_some());
        assert!(registry.snapshot(&ids[3]).is_some());
        assert!(registry.snapshot(&ids[4]).is_some());
    }

    #[test]
    fn max_concurrent_enforcement() {
        let registry = RuntimeOpsLifecycleRegistry::with_config(OpsLifecycleConfig {
            max_completed: DEFAULT_MAX_COMPLETED,
            max_concurrent: Some(2),
        });

        let spec_a = background_spec("a");
        let id_a = spec_a.id.clone();
        registry.register_operation(spec_a).unwrap();

        let spec_b = background_spec("b");
        registry.register_operation(spec_b).unwrap();

        let spec_c = background_spec("c");
        let result = registry.register_operation(spec_c);
        assert!(matches!(
            result,
            Err(OpsLifecycleError::MaxConcurrentExceeded {
                limit: 2,
                active: 2,
            })
        ));

        registry.provisioning_succeeded(&id_a).unwrap();
        registry
            .complete_operation(
                &id_a,
                OperationResult {
                    id: id_a.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();

        let spec_d = background_spec("d");
        assert!(registry.register_operation(spec_d).is_ok());
    }

    #[test]
    fn snapshot_includes_timestamps() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = background_spec("timed");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();

        let snap1 = registry.snapshot(&op_id).unwrap();
        assert!(snap1.created_at_ms > 0);
        assert!(snap1.started_at_ms.is_none());
        assert!(snap1.completed_at_ms.is_none());
        assert!(snap1.elapsed_ms.is_none());

        registry.provisioning_succeeded(&op_id).unwrap();
        let snap2 = registry.snapshot(&op_id).unwrap();
        assert!(snap2.started_at_ms.is_some());
        assert!(snap2.started_at_ms.unwrap() >= snap2.created_at_ms);

        registry
            .complete_operation(
                &op_id,
                OperationResult {
                    id: op_id.clone(),
                    content: "done".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .unwrap();
        let snap3 = registry.snapshot(&op_id).unwrap();
        assert!(snap3.completed_at_ms.is_some());
        assert!(snap3.elapsed_ms.is_some());
        assert!(snap3.completed_at_ms.unwrap() >= snap3.started_at_ms.unwrap());
    }

    #[test]
    fn snapshot_includes_peer_handle() {
        let registry = RuntimeOpsLifecycleRegistry::new();
        let spec = OperationSpec {
            id: OperationId::new(),
            kind: OperationKind::MobMemberChild,
            owner_session_id: SessionId::new(),
            display_name: "peer-test".into(),
            source_label: "test".into(),
            child_session_id: Some(SessionId::new()),
            expect_peer_channel: true,
        };
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();

        let snap1 = registry.snapshot(&op_id).unwrap();
        assert!(snap1.peer_handle.is_none());

        let handle = OperationPeerHandle {
            peer_name: "member-x".into(),
            trusted_peer: TrustedPeerSpec::new("member-x", "peer-id", "inproc://x").unwrap(),
        };
        registry.peer_ready(&op_id, handle).unwrap();

        let snap2 = registry.snapshot(&op_id).unwrap();
        assert_eq!(snap2.peer_handle.as_ref().unwrap().peer_name, "member-x");
    }
}
