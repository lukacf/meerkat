#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Phase 2 contract tests for ops lifecycle persistence and recovery.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use meerkat_core::completion_feed::CompletionFeed;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
use meerkat_core::lifecycle::{CoreExecutor, RunId};
use meerkat_core::ops_lifecycle::{
    OperationKind, OperationResult, OperationSpec, OpsLifecycleRegistry,
};
use meerkat_core::runtime_epoch::{EpochCursorState, RuntimeEpochId};
use meerkat_core::types::SessionId;
use meerkat_runtime::RuntimeStore; // needed for trait method calls
use meerkat_runtime::{
    OpsLifecyclePersistenceRequest, PersistedOpsSnapshot, RuntimeOpsLifecycleRegistry,
};

fn bg_spec(name: &str) -> OperationSpec {
    OperationSpec {
        id: meerkat_core::ops_lifecycle::OperationId::new(),
        kind: OperationKind::BackgroundToolOp,
        owner_session_id: SessionId::new(),
        display_name: name.into(),
        source_label: "test-persistence".into(),
        child_session_id: None,
        expect_peer_channel: false,
    }
}

fn op_result(id: &meerkat_core::ops_lifecycle::OperationId) -> OperationResult {
    OperationResult {
        id: id.clone(),
        content: "done".into(),
        is_error: false,
        duration_ms: 42,
        tokens_used: 0,
    }
}

// ─── Serde round-trip ───

#[test]
fn persisted_ops_snapshot_serde_round_trip() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::new());
    let epoch_id = RuntimeEpochId::new();

    // Register and complete an operation
    let spec = bg_spec("serde-roundtrip");
    let op_id = spec.id.clone();
    registry.register_operation(spec).unwrap();
    registry.provisioning_succeeded(&op_id).unwrap();
    registry
        .complete_operation(&op_id, op_result(&op_id))
        .unwrap();

    let snapshot = registry.capture_persistence_snapshot(epoch_id.clone(), &cursor_state);

    // Serialize and deserialize
    let json = serde_json::to_string(&snapshot).expect("serialize");
    let restored: PersistedOpsSnapshot = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(restored.epoch_id, epoch_id);
    assert_eq!(restored.completion_entries.len(), 1);
    assert_eq!(restored.completion_entries[0].operation_id, op_id);
    assert_eq!(restored.operation_specs.len(), 1);
    assert!(restored.operation_specs.contains_key(&op_id));
}

#[test]
fn persisted_ops_snapshot_preserves_cursor_values() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::from_recovered(5, 3, 2));

    let snapshot = registry.capture_persistence_snapshot(RuntimeEpochId::new(), &cursor_state);

    assert_eq!(snapshot.cursors.agent_applied_cursor, 5);
    assert_eq!(snapshot.cursors.runtime_observed_seq, 3);
    assert_eq!(snapshot.cursors.runtime_last_injected_seq, 2);
}

// ─── Recovery — feed visibility ───

#[test]
fn recovered_registry_contains_terminal_completion_entries() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::new());

    // Register 3 ops, complete 2
    let specs: Vec<_> = (0..3).map(|i| bg_spec(&format!("op-{i}"))).collect();
    for spec in &specs {
        registry.register_operation(spec.clone()).unwrap();
        registry.provisioning_succeeded(&spec.id).unwrap();
    }
    registry
        .complete_operation(&specs[0].id, op_result(&specs[0].id))
        .unwrap();
    registry
        .complete_operation(&specs[1].id, op_result(&specs[1].id))
        .unwrap();
    // specs[2] stays in-progress (non-terminal)

    let snapshot = registry.capture_persistence_snapshot(RuntimeEpochId::new(), &cursor_state);

    // Recover
    let recovered = RuntimeOpsLifecycleRegistry::from_recovered(snapshot);
    let feed = recovered
        .completion_feed()
        .expect("recovered registry should have feed");

    // Feed should contain the 2 terminal entries
    let batch = feed.list_since(0);
    assert_eq!(
        batch.entries.len(),
        2,
        "recovered feed should contain 2 terminal entries, got {}",
        batch.entries.len()
    );
}

#[test]
fn recovered_registry_strips_non_terminal_ops() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::new());

    let terminal_spec = bg_spec("terminal");
    let nonterminal_spec = bg_spec("in-progress");

    registry.register_operation(terminal_spec.clone()).unwrap();
    registry.provisioning_succeeded(&terminal_spec.id).unwrap();
    registry
        .complete_operation(&terminal_spec.id, op_result(&terminal_spec.id))
        .unwrap();

    registry
        .register_operation(nonterminal_spec.clone())
        .unwrap();
    registry
        .provisioning_succeeded(&nonterminal_spec.id)
        .unwrap();

    let snapshot = registry.capture_persistence_snapshot(RuntimeEpochId::new(), &cursor_state);
    let recovered = RuntimeOpsLifecycleRegistry::from_recovered(snapshot);

    // Terminal op present
    assert!(
        recovered.snapshot(&terminal_spec.id).is_some(),
        "terminal op should survive recovery"
    );
    // Non-terminal op stripped
    assert!(
        recovered.snapshot(&nonterminal_spec.id).is_none(),
        "non-terminal op should NOT survive recovery"
    );
}

#[test]
fn recovered_feed_list_since_persisted_cursor_returns_unsurfaced() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    // Simulate: agent has seen up to cursor=0, two completions arrived
    let cursor_state = Arc::new(EpochCursorState::new()); // all zeros

    let spec_a = bg_spec("unsurfaced-a");
    let spec_b = bg_spec("unsurfaced-b");
    registry.register_operation(spec_a.clone()).unwrap();
    registry.provisioning_succeeded(&spec_a.id).unwrap();
    registry
        .complete_operation(&spec_a.id, op_result(&spec_a.id))
        .unwrap();

    registry.register_operation(spec_b.clone()).unwrap();
    registry.provisioning_succeeded(&spec_b.id).unwrap();
    registry
        .complete_operation(&spec_b.id, op_result(&spec_b.id))
        .unwrap();

    let snapshot = registry.capture_persistence_snapshot(RuntimeEpochId::new(), &cursor_state);

    // Recover
    let recovered = RuntimeOpsLifecycleRegistry::from_recovered(snapshot);
    let feed = recovered
        .completion_feed()
        .expect("recovered registry should have feed");

    // list_since(0) should return both entries (cursor was 0, entries are seq 1 and 2)
    let batch = feed.list_since(0);
    assert_eq!(
        batch.entries.len(),
        2,
        "list_since(0) should return 2 unsurfaced entries"
    );

    // list_since(watermark) should return nothing
    let batch_at_watermark = feed.list_since(batch.watermark);
    assert!(
        batch_at_watermark.entries.is_empty(),
        "list_since(watermark) should return nothing"
    );
}

#[test]
fn recovered_registry_clears_wait_state() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::new());

    let spec = bg_spec("wait-clear");
    registry.register_operation(spec.clone()).unwrap();
    registry.provisioning_succeeded(&spec.id).unwrap();
    registry
        .complete_operation(&spec.id, op_result(&spec.id))
        .unwrap();

    let snapshot = registry.capture_persistence_snapshot(RuntimeEpochId::new(), &cursor_state);
    let recovered = RuntimeOpsLifecycleRegistry::from_recovered(snapshot);

    // Should be usable — no panic from stale wait state
    let new_spec = bg_spec("post-recovery-op");
    let result = recovered.register_operation(new_spec);
    assert!(
        result.is_ok(),
        "recovered registry should accept new operations"
    );
}

#[test]
fn recovered_epoch_id_matches_persisted() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::new());
    let epoch_id = RuntimeEpochId::new();

    let snapshot = registry.capture_persistence_snapshot(epoch_id.clone(), &cursor_state);
    assert_eq!(snapshot.epoch_id, epoch_id, "persisted epoch_id must match");
}

#[test]
fn recovered_feed_watermark_matches_persisted_sequence() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::new());

    // Complete 3 ops to advance the sequence
    for i in 0..3 {
        let spec = bg_spec(&format!("watermark-{i}"));
        let id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&id).unwrap();
        registry.complete_operation(&id, op_result(&id)).unwrap();
    }

    let snapshot = registry.capture_persistence_snapshot(RuntimeEpochId::new(), &cursor_state);
    let recovered = RuntimeOpsLifecycleRegistry::from_recovered(snapshot);
    let feed = recovered.completion_feed().unwrap();

    // Watermark should be at least 3 (3 completions)
    assert!(
        feed.watermark() >= 3,
        "recovered feed watermark ({}) should be >= 3",
        feed.watermark()
    );
}

// ─── Snapshot captures all feed entries, not just authority-retained ops ───

#[test]
fn snapshot_captures_entries_beyond_authority_retention() {
    // Use a registry with max_completed=2 so authority evicts early
    let registry = meerkat_runtime::RuntimeOpsLifecycleRegistry::with_config(
        meerkat_runtime::OpsLifecycleConfig {
            max_completed: 2,
            max_concurrent: None,
        },
    );
    let cursor_state = Arc::new(EpochCursorState::new());

    // Complete 4 ops — authority keeps last 2, but feed buffer keeps all 4
    for i in 0..4 {
        let spec = bg_spec(&format!("evict-{i}"));
        let id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&id).unwrap();
        registry.complete_operation(&id, op_result(&id)).unwrap();
    }

    let snapshot = registry.capture_persistence_snapshot(RuntimeEpochId::new(), &cursor_state);

    // Snapshot should contain all 4 completion entries (from feed buffer),
    // even though authority only retains 2 terminal ops
    assert_eq!(
        snapshot.completion_entries.len(),
        4,
        "snapshot should capture all feed entries ({} found), not just authority-retained",
        snapshot.completion_entries.len()
    );

    // Authority state should only have 2 ops (eviction)
    assert_eq!(
        snapshot.authority_state.operation_count(),
        2,
        "authority should retain max_completed=2 ops"
    );
}

#[test]
fn terminal_persistence_queue_captures_burst_without_loss_for_recovery() {
    const TERMINAL_COUNT: usize = 32;

    let registry = RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::new());
    let epoch_id = RuntimeEpochId::new();
    let (persist_tx, mut persist_rx) =
        tokio::sync::mpsc::unbounded_channel::<OpsLifecyclePersistenceRequest>();
    let snapshots = Arc::new(Mutex::new(Vec::new()));
    let worker_snapshots = Arc::clone(&snapshots);
    let worker = std::thread::spawn(move || {
        while let Some(request) = persist_rx.blocking_recv() {
            worker_snapshots
                .lock()
                .unwrap()
                .push(request.snapshot().clone());
            request.complete(Ok(()));
        }
    });
    registry.set_persistence_channel(persist_tx, epoch_id.clone(), Arc::clone(&cursor_state));

    for i in 0..TERMINAL_COUNT {
        let spec = bg_spec(&format!("queued-terminal-{i}"));
        let id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&id).unwrap();
        registry.complete_operation(&id, op_result(&id)).unwrap();
    }
    drop(registry);
    worker.join().unwrap();

    let snapshots = snapshots.lock().unwrap();

    assert_eq!(
        snapshots.len(),
        TERMINAL_COUNT,
        "every terminal transition must queue a snapshot"
    );
    let latest_snapshot = snapshots
        .last()
        .expect("at least one terminal snapshot")
        .clone();
    assert_eq!(latest_snapshot.epoch_id, epoch_id);
    assert_eq!(
        latest_snapshot.completion_entries.len(),
        TERMINAL_COUNT,
        "latest persisted snapshot must include the full terminal completion feed"
    );

    let recovered = RuntimeOpsLifecycleRegistry::from_recovered(latest_snapshot);
    let recovered_feed = recovered
        .completion_feed()
        .expect("recovered registry should expose completion feed");
    let batch = recovered_feed.list_since(0);
    assert_eq!(
        batch.entries.len(),
        TERMINAL_COUNT,
        "restart recovery must see every terminal completion from the queued snapshot"
    );
}

#[test]
fn terminal_persistence_channel_closed_fails_terminal_transition() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::new());
    let (persist_tx, persist_rx) =
        tokio::sync::mpsc::unbounded_channel::<OpsLifecyclePersistenceRequest>();
    drop(persist_rx);
    registry.set_persistence_channel(persist_tx, RuntimeEpochId::new(), cursor_state);

    let spec = bg_spec("closed-persist-channel");
    let id = spec.id.clone();
    registry.register_operation(spec).unwrap();
    registry.provisioning_succeeded(&id).unwrap();

    let err = registry
        .complete_operation(&id, op_result(&id))
        .expect_err("closed persistence channel must fail the terminal transition");
    assert!(
        err.to_string().contains("persistence channel closed"),
        "unexpected error: {err}"
    );
}

struct FailingOpsLifecycleStore {
    inner: meerkat_runtime::InMemoryRuntimeStore,
    persist_calls: AtomicUsize,
}

impl FailingOpsLifecycleStore {
    fn new() -> Self {
        Self {
            inner: meerkat_runtime::InMemoryRuntimeStore::new(),
            persist_calls: AtomicUsize::new(0),
        }
    }

    fn persist_calls(&self) -> usize {
        self.persist_calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl RuntimeStore for FailingOpsLifecycleStore {
    async fn commit_session_boundary(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        session_delta: meerkat_runtime::SessionDelta,
        run_id: RunId,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::lifecycle::InputId>,
        input_updates: Vec<meerkat_runtime::input_state::StoredInputState>,
    ) -> Result<meerkat_core::lifecycle::RunBoundaryReceipt, meerkat_runtime::RuntimeStoreError>
    {
        self.inner
            .commit_session_boundary(
                runtime_id,
                session_delta,
                run_id,
                boundary,
                contributing_input_ids,
                input_updates,
            )
            .await
    }

    async fn atomic_apply(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        session_delta: Option<meerkat_runtime::SessionDelta>,
        receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
        input_updates: Vec<meerkat_runtime::input_state::StoredInputState>,
        session_store_key: Option<SessionId>,
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        self.inner
            .atomic_apply(
                runtime_id,
                session_delta,
                receipt,
                input_updates,
                session_store_key,
            )
            .await
    }

    async fn load_input_states(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
    ) -> Result<
        Vec<meerkat_runtime::input_state::StoredInputState>,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner.load_input_states(runtime_id).await
    }

    async fn load_boundary_receipt(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<
        Option<meerkat_core::lifecycle::RunBoundaryReceipt>,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner
            .load_boundary_receipt(runtime_id, run_id, sequence)
            .await
    }

    async fn load_session_snapshot(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, meerkat_runtime::RuntimeStoreError> {
        self.inner.load_session_snapshot(runtime_id).await
    }

    async fn persist_input_state(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        state: &meerkat_runtime::input_state::StoredInputState,
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        self.inner.persist_input_state(runtime_id, state).await
    }

    async fn load_input_state(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) -> Result<
        Option<meerkat_runtime::input_state::StoredInputState>,
        meerkat_runtime::RuntimeStoreError,
    > {
        self.inner.load_input_state(runtime_id, input_id).await
    }

    async fn persist_runtime_state(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        state: meerkat_runtime::RuntimeState,
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        self.inner.persist_runtime_state(runtime_id, state).await
    }

    async fn load_runtime_state(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
    ) -> Result<Option<meerkat_runtime::RuntimeState>, meerkat_runtime::RuntimeStoreError> {
        self.inner.load_runtime_state(runtime_id).await
    }

    async fn atomic_lifecycle_commit(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        runtime_state: meerkat_runtime::RuntimeState,
        input_states: &[meerkat_runtime::input_state::StoredInputState],
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        self.inner
            .atomic_lifecycle_commit(runtime_id, runtime_state, input_states)
            .await
    }

    async fn persist_ops_lifecycle(
        &self,
        _runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        _snapshot: &PersistedOpsSnapshot,
    ) -> Result<(), meerkat_runtime::RuntimeStoreError> {
        self.persist_calls.fetch_add(1, Ordering::SeqCst);
        Err(meerkat_runtime::RuntimeStoreError::WriteFailed(
            "synthetic ops lifecycle persist failure".to_string(),
        ))
    }

    async fn load_ops_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
    ) -> Result<Option<PersistedOpsSnapshot>, meerkat_runtime::RuntimeStoreError> {
        self.inner.load_ops_lifecycle(runtime_id).await
    }
}

#[tokio::test]
async fn terminal_transition_surfaces_store_write_failure_after_persistence_request_is_accepted() {
    let store = Arc::new(FailingOpsLifecycleStore::new());
    let adapter = Arc::new(meerkat_runtime::MeerkatMachine::persistent_without_blobs(
        Arc::clone(&store) as Arc<dyn RuntimeStore>,
    ));
    let session_id = SessionId::new();
    adapter
        .register_session_with_executor(session_id.clone(), Box::new(NoopExecutor))
        .await;
    let bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("persistent session should prepare bindings");

    let spec = bg_spec("durable-store-rejection");
    let op_id = spec.id.clone();
    bindings.ops_lifecycle.register_operation(spec).unwrap();
    bindings
        .ops_lifecycle
        .provisioning_succeeded(&op_id)
        .unwrap();

    let err = bindings
        .ops_lifecycle
        .complete_operation(&op_id, op_result(&op_id))
        .expect_err("terminal transition must fail when the durable store rejects the snapshot");
    assert!(
        err.to_string()
            .contains("synthetic ops lifecycle persist failure"),
        "unexpected error: {err}"
    );
    assert_eq!(
        store.persist_calls(),
        1,
        "terminal transition should wait for the accepted persistence request"
    );
}

// ─── Regression: cold ensure_session_with_executor recovers persisted epoch ───

/// Simple executor for recovery tests.
struct NoopExecutor;

#[async_trait::async_trait]
impl CoreExecutor for NoopExecutor {
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        Ok(CoreApplyOutput {
            receipt: meerkat_core::RunBoundaryReceipt {
                run_id,
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                conversation_digest: None,
                message_count: 0,
                sequence: 0,
            },
            session_snapshot: None,
            terminal: None,
            run_result: None,
        })
    }
    async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

/// Regression test: cold ensure_session_with_executor creates an entry
/// with the shared recovery helper, not a bare fresh registry.
/// Uses ephemeral adapter (no store) to verify the code path works.
#[tokio::test]
async fn cold_ensure_session_with_executor_uses_shared_recovery_path() {
    let adapter = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
    let session_id = SessionId::new();

    // Go straight to register_session_with_executor — no prior register_session.
    adapter
        .register_session_with_executor(session_id.clone(), Box::new(NoopExecutor))
        .await;

    // The session should be registered with valid bindings.
    let bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("session should be registered after cold attach");

    assert_eq!(bindings.session_id, session_id);

    // Register an op — it should be visible through the same registry.
    let spec = bg_spec("cold-attach-verify");
    let op_id = spec.id.clone();
    bindings.ops_lifecycle.register_operation(spec).unwrap();

    let direct = adapter
        .ops_lifecycle_registry(&session_id)
        .await
        .expect("registry should exist");
    assert!(
        direct.snapshot(&op_id).is_some(),
        "cold attach-first registry must be the same instance as prepare_bindings"
    );
}

/// Regression test: cold persistent adapter with pre-persisted snapshot
/// recovers epoch through both register_session and
/// register_session_with_executor paths.
#[tokio::test]
async fn cold_persistent_adapter_recovers_persisted_epoch() {
    let store = Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());

    let session_id = SessionId::new();

    // Phase 1: Persist a snapshot manually.
    let registry = meerkat_runtime::RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::new());
    let epoch_id = RuntimeEpochId::new();

    let spec = bg_spec("persistent-recovery");
    let op_id = spec.id.clone();
    registry.register_operation(spec).unwrap();
    registry.provisioning_succeeded(&op_id).unwrap();
    registry
        .complete_operation(&op_id, op_result(&op_id))
        .unwrap();

    let snapshot = registry.capture_persistence_snapshot(epoch_id.clone(), &cursor_state);
    let runtime_id = meerkat_runtime::identifiers::LogicalRuntimeId::new(session_id.to_string());
    store
        .persist_ops_lifecycle(&runtime_id, &snapshot)
        .await
        .unwrap();

    // Phase 2: Cold persistent adapter — register_session path.
    let adapter = meerkat_runtime::MeerkatMachine::persistent_without_blobs(
        Arc::clone(&store) as Arc<dyn RuntimeStore>
    );
    adapter.register_session(session_id.clone()).await;

    let bindings = adapter.prepare_bindings(session_id.clone()).await.unwrap();
    assert_eq!(
        bindings.epoch_id, epoch_id,
        "register_session must recover the persisted epoch_id"
    );

    let feed = bindings.ops_lifecycle.completion_feed().unwrap();
    let batch = feed.list_since(0);
    assert_eq!(
        batch.entries.len(),
        1,
        "register_session must recover persisted completion entries"
    );
    assert_eq!(batch.entries[0].operation_id, op_id);
}

/// Same test but via the ensure_session_with_executor cold path.
#[tokio::test]
async fn cold_ensure_session_with_executor_recovers_persisted_epoch() {
    let store = Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());

    let session_id = SessionId::new();

    // Persist a snapshot
    let registry = meerkat_runtime::RuntimeOpsLifecycleRegistry::new();
    let cursor_state = Arc::new(EpochCursorState::new());
    let epoch_id = RuntimeEpochId::new();

    let spec = bg_spec("executor-attach-recovery");
    let op_id = spec.id.clone();
    registry.register_operation(spec).unwrap();
    registry.provisioning_succeeded(&op_id).unwrap();
    registry
        .complete_operation(&op_id, op_result(&op_id))
        .unwrap();

    let snapshot = registry.capture_persistence_snapshot(epoch_id.clone(), &cursor_state);
    let runtime_id = meerkat_runtime::identifiers::LogicalRuntimeId::new(session_id.to_string());
    store
        .persist_ops_lifecycle(&runtime_id, &snapshot)
        .await
        .unwrap();

    // Cold attach-first — no prior register_session
    let adapter = Arc::new(meerkat_runtime::MeerkatMachine::persistent_without_blobs(
        Arc::clone(&store) as Arc<dyn RuntimeStore>,
    ));
    adapter
        .register_session_with_executor(session_id.clone(), Box::new(NoopExecutor))
        .await;

    // Verify epoch recovered
    let bindings = adapter.prepare_bindings(session_id.clone()).await.unwrap();
    assert_eq!(
        bindings.epoch_id, epoch_id,
        "cold ensure_session_with_executor must recover the persisted epoch_id"
    );

    let feed = bindings.ops_lifecycle.completion_feed().unwrap();
    let batch = feed.list_since(0);
    assert!(
        !batch.entries.is_empty(),
        "cold ensure_session_with_executor must recover persisted completion entries"
    );
    assert_eq!(batch.entries[0].operation_id, op_id);
}
