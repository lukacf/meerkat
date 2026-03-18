//! In-memory runtime implementation of the shared async-operation lifecycle seam.

use std::collections::HashMap;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

#[cfg(target_arch = "wasm32")]
use crate::tokio;
use meerkat_core::ops_lifecycle::{
    OperationCompletionWatch, OperationId, OperationLifecycleSnapshot, OperationPeerHandle,
    OperationProgressUpdate, OperationResult, OperationSpec, OperationStatus,
    OperationTerminalOutcome, OpsLifecycleError, OpsLifecycleRegistry,
};

#[derive(Debug)]
struct OperationRecord {
    spec: OperationSpec,
    status: OperationStatus,
    peer_ready: bool,
    #[allow(dead_code)]
    peer_handle: Option<OperationPeerHandle>,
    progress_count: u32,
    watchers: Vec<tokio::sync::oneshot::Sender<OperationTerminalOutcome>>,
    terminal_outcome: Option<OperationTerminalOutcome>,
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
        }
    }

    fn snapshot(&self) -> OperationLifecycleSnapshot {
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
        self.terminal_outcome = Some(outcome.clone());
        for watcher in std::mem::take(&mut self.watchers) {
            let _ = watcher.send(outcome.clone());
        }
    }
}

#[derive(Debug, Default)]
struct RegistryState {
    operations: HashMap<OperationId, OperationRecord>,
}

/// Per-runtime shared registry for async operation lifecycle truth.
#[derive(Debug, Default)]
pub struct RuntimeOpsLifecycleRegistry {
    state: RwLock<RegistryState>,
}

impl RuntimeOpsLifecycleRegistry {
    pub fn new() -> Self {
        Self::default()
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
}

impl OpsLifecycleRegistry for RuntimeOpsLifecycleRegistry {
    fn register_operation(&self, spec: OperationSpec) -> Result<(), OpsLifecycleError> {
        let mut state = self.write_state()?;
        let operation_id = spec.id.clone();
        if state.operations.contains_key(&operation_id) {
            return Err(OpsLifecycleError::AlreadyRegistered(operation_id));
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
        for record in state.operations.values_mut() {
            if !record.status.is_terminal() {
                record.resolve_terminal(
                    OperationStatus::Terminated,
                    OperationTerminalOutcome::Terminated {
                        reason: reason.clone(),
                    },
                );
            }
        }
        Ok(())
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
}
