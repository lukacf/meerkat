//! In-memory runtime implementation of the shared async-operation lifecycle seam.

use std::collections::{HashMap, VecDeque};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Instant, SystemTime};

#[cfg(target_arch = "wasm32")]
use crate::tokio;
use meerkat_core::ops_lifecycle::{
    DEFAULT_MAX_COMPLETED, OperationCompletionWatch, OperationId, OperationLifecycleSnapshot,
    OperationPeerHandle, OperationProgressUpdate, OperationResult, OperationSpec, OperationStatus,
    OperationTerminalOutcome, OpsLifecycleError, OpsLifecycleRegistry,
};

/// Internal record with monotonic timestamps for elapsed computation.
#[derive(Debug)]
struct OperationRecord {
    spec: OperationSpec,
    status: OperationStatus,
    peer_ready: bool,
    peer_handle: Option<OperationPeerHandle>,
    progress_count: u32,
    watchers: Vec<tokio::sync::oneshot::Sender<OperationTerminalOutcome>>,
    terminal_outcome: Option<OperationTerminalOutcome>,
    // Monotonic timestamps for elapsed computation
    created_at: Instant,
    started_at: Option<Instant>,
    completed_at: Option<Instant>,
    // Wall-clock anchor captured at creation for epoch millis
    created_at_wall: SystemTime,
}

impl OperationRecord {
    fn new(spec: OperationSpec) -> Self {
        Self {
            spec,
            status: OperationStatus::Provisioning,
            peer_ready: false,
            peer_handle: None,
            progress_count: 0,
            watchers: Vec::new(),
            terminal_outcome: None,
            created_at: Instant::now(),
            started_at: None,
            completed_at: None,
            created_at_wall: SystemTime::now(),
        }
    }

    fn epoch_millis(wall_anchor: &SystemTime) -> u64 {
        wall_anchor
            .duration_since(SystemTime::UNIX_EPOCH)
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

    fn snapshot(&self) -> OperationLifecycleSnapshot {
        let created_at_ms = Self::epoch_millis(&self.created_at_wall);
        let started_at_ms = self.started_at.map(|i| self.epoch_millis_for_instant(i));
        let completed_at_ms = self.completed_at.map(|i| self.epoch_millis_for_instant(i));
        let elapsed_ms = self.completed_at.map(|completed| {
            completed
                .saturating_duration_since(self.created_at)
                .as_millis() as u64
        });

        OperationLifecycleSnapshot {
            id: self.spec.id.clone(),
            kind: self.spec.kind,
            display_name: self.spec.display_name.clone(),
            status: self.status,
            peer_ready: self.peer_ready,
            progress_count: self.progress_count,
            watcher_count: self.watchers.len() as u32,
            terminal_outcome: self.terminal_outcome.clone(),
            child_session_id: self.spec.child_session_id.clone(),
            peer_handle: self.peer_handle.clone(),
            created_at_ms,
            started_at_ms,
            completed_at_ms,
            elapsed_ms,
        }
    }

    fn invalid_transition(&self, action: &'static str) -> OpsLifecycleError {
        OpsLifecycleError::InvalidTransition {
            id: self.spec.id.clone(),
            status: self.status,
            action,
        }
    }

    fn resolve_terminal(&mut self, status: OperationStatus, outcome: OperationTerminalOutcome) {
        self.status = status;
        self.completed_at = Some(Instant::now());
        self.terminal_outcome = Some(outcome.clone());
        for watcher in std::mem::take(&mut self.watchers) {
            let _ = watcher.send(outcome.clone());
        }
    }
}

#[derive(Debug)]
struct RegistryState {
    operations: HashMap<OperationId, OperationRecord>,
    /// FIFO ordering of completed operation IDs for bounded eviction.
    completed_order: VecDeque<OperationId>,
    /// Maximum number of completed operations to retain.
    max_completed: usize,
    /// Maximum concurrent non-terminal operations (None = unlimited).
    max_concurrent: Option<usize>,
}

impl RegistryState {
    fn new(max_completed: usize, max_concurrent: Option<usize>) -> Self {
        Self {
            operations: HashMap::new(),
            completed_order: VecDeque::new(),
            max_completed,
            max_concurrent,
        }
    }

    /// Count operations in non-terminal states.
    fn active_count(&self) -> usize {
        self.operations
            .values()
            .filter(|r| !r.status.is_terminal())
            .count()
    }

    /// Evict oldest completed operations if over the retention limit.
    fn evict_completed(&mut self) {
        while self.completed_order.len() > self.max_completed {
            if let Some(evicted_id) = self.completed_order.pop_front()
                && self
                    .operations
                    .get(&evicted_id)
                    .is_some_and(|r| r.status.is_terminal())
            {
                self.operations.remove(&evicted_id);
            }
        }
    }
}

impl Default for RegistryState {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_COMPLETED, None)
    }
}

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
#[derive(Debug)]
pub struct RuntimeOpsLifecycleRegistry {
    state: RwLock<RegistryState>,
}

impl Default for RuntimeOpsLifecycleRegistry {
    fn default() -> Self {
        Self {
            state: RwLock::new(RegistryState::default()),
        }
    }
}

impl RuntimeOpsLifecycleRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: OpsLifecycleConfig) -> Self {
        Self {
            state: RwLock::new(RegistryState::new(
                config.max_completed,
                config.max_concurrent,
            )),
        }
    }

    fn read_state(&self) -> Result<RwLockReadGuard<'_, RegistryState>, OpsLifecycleError> {
        self.state
            .read()
            .map_err(|_| OpsLifecycleError::Internal("ops lifecycle registry poisoned".into()))
    }

    fn write_state(&self) -> Result<RwLockWriteGuard<'_, RegistryState>, OpsLifecycleError> {
        self.state
            .write()
            .map_err(|_| OpsLifecycleError::Internal("ops lifecycle registry poisoned".into()))
    }

    fn record_mut<'a>(
        state: &'a mut RegistryState,
        id: &OperationId,
    ) -> Result<&'a mut OperationRecord, OpsLifecycleError> {
        state
            .operations
            .get_mut(id)
            .ok_or_else(|| OpsLifecycleError::NotFound(id.clone()))
    }

    /// Register a completion watcher for each ID and await all of them.
    pub async fn wait_all(
        &self,
        ids: &[OperationId],
    ) -> Result<Vec<(OperationId, OperationTerminalOutcome)>, OpsLifecycleError> {
        let watches: Vec<(OperationId, OperationCompletionWatch)> = {
            let mut state = self.write_state()?;
            ids.iter()
                .map(|id| {
                    let record = Self::record_mut(&mut state, id)?;
                    if let Some(outcome) = &record.terminal_outcome {
                        Ok((
                            id.clone(),
                            OperationCompletionWatch::already_resolved(outcome.clone()),
                        ))
                    } else {
                        let (tx, watch) = OperationCompletionWatch::channel();
                        record.watchers.push(tx);
                        Ok((id.clone(), watch))
                    }
                })
                .collect::<Result<Vec<_>, OpsLifecycleError>>()?
        };

        let mut results = Vec::with_capacity(watches.len());
        for (id, watch) in watches {
            let outcome = watch.wait().await;
            results.push((id, outcome));
        }
        Ok(results)
    }
}

impl OpsLifecycleRegistry for RuntimeOpsLifecycleRegistry {
    fn register_operation(&self, spec: OperationSpec) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        let operation_id = spec.id.clone();
        if state.operations.contains_key(&operation_id) {
            return Err(OpsLifecycleError::AlreadyRegistered(operation_id));
        }
        if let Some(limit) = state.max_concurrent {
            let active = state.active_count();
            if active >= limit {
                return Err(OpsLifecycleError::MaxConcurrentExceeded { limit, active });
            }
        }
        state
            .operations
            .insert(operation_id, OperationRecord::new(spec));
        Ok(())
    }

    fn provisioning_succeeded(&self, id: &OperationId) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        let record = Self::record_mut(&mut state, id)?;
        if record.status != OperationStatus::Provisioning {
            return Err(record.invalid_transition("provisioning_succeeded"));
        }
        record.status = OperationStatus::Running;
        record.started_at = Some(Instant::now());
        Ok(())
    }

    fn provisioning_failed(
        &self,
        id: &OperationId,
        error: String,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        let record = Self::record_mut(&mut state, id)?;
        if !record.status.allows_terminalization() {
            return Err(record.invalid_transition("provisioning_failed"));
        }
        record.resolve_terminal(
            OperationStatus::Failed,
            OperationTerminalOutcome::Failed { error },
        );
        state.completed_order.push_back(id.clone());
        state.evict_completed();
        Ok(())
    }

    fn peer_ready(
        &self,
        id: &OperationId,
        peer: OperationPeerHandle,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        let record = Self::record_mut(&mut state, id)?;
        if !record.spec.expect_peer_channel || !record.spec.kind.expects_peer_channel() {
            return Err(OpsLifecycleError::PeerNotExpected(id.clone()));
        }
        if record.peer_ready {
            return Err(OpsLifecycleError::AlreadyPeerReady(id.clone()));
        }
        if !matches!(
            record.status,
            OperationStatus::Running | OperationStatus::Retiring
        ) {
            return Err(record.invalid_transition("peer_ready"));
        }
        record.peer_ready = true;
        record.peer_handle = Some(peer);
        Ok(())
    }

    fn register_watcher(
        &self,
        id: &OperationId,
    ) -> Result<OperationCompletionWatch, OpsLifecycleError> {
        let mut state = self.write_state()?;
        let record = Self::record_mut(&mut state, id)?;
        if let Some(outcome) = &record.terminal_outcome {
            return Ok(OperationCompletionWatch::already_resolved(outcome.clone()));
        }
        let (tx, watch) = OperationCompletionWatch::channel();
        record.watchers.push(tx);
        Ok(watch)
    }

    fn report_progress(
        &self,
        id: &OperationId,
        _update: OperationProgressUpdate,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        let record = Self::record_mut(&mut state, id)?;
        if !matches!(
            record.status,
            OperationStatus::Running | OperationStatus::Retiring
        ) {
            return Err(record.invalid_transition("report_progress"));
        }
        record.progress_count += 1;
        Ok(())
    }

    fn complete_operation(
        &self,
        id: &OperationId,
        result: OperationResult,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        let record = Self::record_mut(&mut state, id)?;
        if !record.status.allows_terminalization() {
            return Err(record.invalid_transition("complete_operation"));
        }
        record.resolve_terminal(
            OperationStatus::Completed,
            OperationTerminalOutcome::Completed(result),
        );
        state.completed_order.push_back(id.clone());
        state.evict_completed();
        Ok(())
    }

    fn fail_operation(&self, id: &OperationId, error: String) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        let record = Self::record_mut(&mut state, id)?;
        if !record.status.allows_terminalization() {
            return Err(record.invalid_transition("fail_operation"));
        }
        record.resolve_terminal(
            OperationStatus::Failed,
            OperationTerminalOutcome::Failed { error },
        );
        state.completed_order.push_back(id.clone());
        state.evict_completed();
        Ok(())
    }

    fn cancel_operation(
        &self,
        id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        let record = Self::record_mut(&mut state, id)?;
        if !record.status.allows_terminalization() {
            return Err(record.invalid_transition("cancel_operation"));
        }
        record.resolve_terminal(
            OperationStatus::Cancelled,
            OperationTerminalOutcome::Cancelled { reason },
        );
        state.completed_order.push_back(id.clone());
        state.evict_completed();
        Ok(())
    }

    fn request_retire(&self, id: &OperationId) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        let record = Self::record_mut(&mut state, id)?;
        if record.status != OperationStatus::Running {
            return Err(record.invalid_transition("request_retire"));
        }
        record.status = OperationStatus::Retiring;
        Ok(())
    }

    fn mark_retired(&self, id: &OperationId) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        let record = Self::record_mut(&mut state, id)?;
        if !matches!(
            record.status,
            OperationStatus::Running | OperationStatus::Retiring
        ) {
            return Err(record.invalid_transition("mark_retired"));
        }
        record.resolve_terminal(OperationStatus::Retired, OperationTerminalOutcome::Retired);
        state.completed_order.push_back(id.clone());
        state.evict_completed();
        Ok(())
    }

    fn snapshot(&self, id: &OperationId) -> Option<OperationLifecycleSnapshot> {
        self.read_state()
            .ok()
            .and_then(|state| state.operations.get(id).map(OperationRecord::snapshot))
    }

    fn list_operations(&self) -> Vec<OperationLifecycleSnapshot> {
        let mut snapshots = self
            .read_state()
            .map(|state| {
                state
                    .operations
                    .values()
                    .map(OperationRecord::snapshot)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        snapshots.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        snapshots
    }

    fn terminate_owner(&self, reason: String) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        let to_terminate: Vec<OperationId> = state
            .operations
            .iter()
            .filter(|(_, r)| !r.status.is_terminal())
            .map(|(id, _)| id.clone())
            .collect();
        for id in &to_terminate {
            if let Some(record) = state.operations.get_mut(id) {
                record.resolve_terminal(
                    OperationStatus::Terminated,
                    OperationTerminalOutcome::Terminated {
                        reason: reason.clone(),
                    },
                );
            }
            state.completed_order.push_back(id.clone());
        }
        state.evict_completed();
        Ok(())
    }

    fn collect_completed(
        &self,
    ) -> Result<Vec<(OperationId, OperationTerminalOutcome)>, OpsLifecycleError> {
        let mut state = self.write_state()?;
        let mut collected = Vec::new();
        let completed_ids: Vec<OperationId> = state.completed_order.drain(..).collect();
        for id in completed_ids {
            if let Some(record) = state.operations.remove(&id)
                && let Some(outcome) = record.terminal_outcome
            {
                collected.push((id, outcome));
            }
        }
        Ok(collected)
    }

    fn wait_all(
        &self,
        ids: &[OperationId],
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        Vec<(OperationId, OperationTerminalOutcome)>,
                        OpsLifecycleError,
                    >,
                > + Send
                + '_,
        >,
    > {
        // Register watchers synchronously under the lock, then await them.
        // This avoids borrowing `ids` across the await point.
        let watches: Result<Vec<(OperationId, OperationCompletionWatch)>, OpsLifecycleError> = {
            let mut state = match self.write_state() {
                Ok(s) => s,
                Err(e) => return Box::pin(std::future::ready(Err(e))),
            };
            ids.iter()
                .map(|id| {
                    let record = Self::record_mut(&mut state, id)?;
                    if let Some(outcome) = &record.terminal_outcome {
                        Ok((
                            id.clone(),
                            OperationCompletionWatch::already_resolved(outcome.clone()),
                        ))
                    } else {
                        let (tx, watch) = OperationCompletionWatch::channel();
                        record.watchers.push(tx);
                        Ok((id.clone(), watch))
                    }
                })
                .collect()
        };

        Box::pin(async move {
            let watches = watches?;
            let mut results = Vec::with_capacity(watches.len());
            for (id, watch) in watches {
                let outcome = watch.wait().await;
                results.push((id, outcome));
            }
            Ok(results)
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::comms::TrustedPeerSpec;
    use meerkat_core::ops_lifecycle::{OperationKind, OpsLifecycleRegistry};
    use meerkat_core::types::SessionId;

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

        let results = registry
            .wait_all(&[id_a.clone(), id_b.clone()])
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, id_a);
        assert!(matches!(
            results[0].1,
            OperationTerminalOutcome::Completed(_)
        ));
        assert_eq!(results[1].0, id_b);
        assert!(matches!(
            results[1].1,
            OperationTerminalOutcome::Failed { .. }
        ));
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
