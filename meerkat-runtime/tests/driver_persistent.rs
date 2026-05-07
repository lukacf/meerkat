#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use meerkat_core::BlobStore;
use meerkat_core::lifecycle::{InputId, RunId, run_receipt::RunBoundaryReceipt};
use meerkat_core::types::{ContentBlock, ImageData, SessionId};
use meerkat_runtime::input_state::{InputStateSeed, InputTerminalOutcome, StoredInputState};
use meerkat_runtime::store::RuntimeStoreError;
use meerkat_runtime::{
    InMemoryRuntimeStore, Input, InputDurability, InputHeader, InputOrigin, InputState,
    InputVisibility, LogicalRuntimeId, PersistentRuntimeDriver, PromptInput, RuntimeDriver,
    RuntimeState, RuntimeStore, SessionDelta,
};
use meerkat_store::MemoryBlobStore;

fn memory_blob_store() -> Arc<dyn BlobStore> {
    Arc::new(MemoryBlobStore::new())
}

fn stamp_runtime_semantics(state: &mut InputState) {
    let Some(input) = state.persisted_input.as_ref() else {
        return;
    };
    let policy = meerkat_runtime::DefaultPolicyTable::resolve(input, true);
    let policy_version = policy.policy_version;
    state.runtime_semantics = Some(
        meerkat_runtime::ingress_types::RuntimeInputSemantics::from_policy_and_kind(
            &policy,
            input.kind(),
        ),
    );
    state.policy = Some(meerkat_runtime::input_state::PolicySnapshot {
        version: policy_version,
        decision: policy,
    });
}

fn stored_accepted(mut state: InputState) -> StoredInputState {
    stamp_runtime_semantics(&mut state);
    StoredInputState {
        seed: InputStateSeed::new_accepted(),
        state,
    }
}

struct FailPersistInputStore {
    inner: Arc<InMemoryRuntimeStore>,
    fail_persist_input_state: AtomicBool,
    fail_atomic_apply: AtomicBool,
    fail_commit_machine_lifecycle: AtomicBool,
    fail_load_input_states_for: Option<LogicalRuntimeId>,
    fail_load_boundary_receipt_for: Option<LogicalRuntimeId>,
    fail_load_runtime_state_for: Option<LogicalRuntimeId>,
    runtime_state_overrides: Mutex<HashMap<LogicalRuntimeId, RuntimeState>>,
}

impl FailPersistInputStore {
    fn new(inner: Arc<InMemoryRuntimeStore>) -> Self {
        Self {
            inner,
            fail_persist_input_state: AtomicBool::new(true),
            fail_atomic_apply: AtomicBool::new(false),
            fail_commit_machine_lifecycle: AtomicBool::new(false),
            fail_load_input_states_for: None,
            fail_load_boundary_receipt_for: None,
            fail_load_runtime_state_for: None,
            runtime_state_overrides: Mutex::new(HashMap::new()),
        }
    }

    fn passthrough(inner: Arc<InMemoryRuntimeStore>) -> Self {
        Self {
            inner,
            fail_persist_input_state: AtomicBool::new(false),
            fail_atomic_apply: AtomicBool::new(false),
            fail_commit_machine_lifecycle: AtomicBool::new(false),
            fail_load_input_states_for: None,
            fail_load_boundary_receipt_for: None,
            fail_load_runtime_state_for: None,
            runtime_state_overrides: Mutex::new(HashMap::new()),
        }
    }

    fn fail_commit_machine_lifecycle_once(inner: Arc<InMemoryRuntimeStore>) -> Self {
        Self {
            inner,
            fail_persist_input_state: AtomicBool::new(false),
            fail_atomic_apply: AtomicBool::new(false),
            fail_commit_machine_lifecycle: AtomicBool::new(true),
            fail_load_input_states_for: None,
            fail_load_boundary_receipt_for: None,
            fail_load_runtime_state_for: None,
            runtime_state_overrides: Mutex::new(HashMap::new()),
        }
    }

    fn fail_load_input_states_for(
        inner: Arc<InMemoryRuntimeStore>,
        runtime_id: LogicalRuntimeId,
    ) -> Self {
        Self {
            inner,
            fail_persist_input_state: AtomicBool::new(false),
            fail_atomic_apply: AtomicBool::new(false),
            fail_commit_machine_lifecycle: AtomicBool::new(false),
            fail_load_input_states_for: Some(runtime_id),
            fail_load_boundary_receipt_for: None,
            fail_load_runtime_state_for: None,
            runtime_state_overrides: Mutex::new(HashMap::new()),
        }
    }

    fn fail_load_boundary_receipt_for(
        inner: Arc<InMemoryRuntimeStore>,
        runtime_id: LogicalRuntimeId,
    ) -> Self {
        Self {
            inner,
            fail_persist_input_state: AtomicBool::new(false),
            fail_atomic_apply: AtomicBool::new(false),
            fail_commit_machine_lifecycle: AtomicBool::new(false),
            fail_load_input_states_for: None,
            fail_load_boundary_receipt_for: Some(runtime_id),
            fail_load_runtime_state_for: None,
            runtime_state_overrides: Mutex::new(HashMap::new()),
        }
    }

    fn fail_load_runtime_state_for(
        inner: Arc<InMemoryRuntimeStore>,
        runtime_id: LogicalRuntimeId,
    ) -> Self {
        Self {
            inner,
            fail_persist_input_state: AtomicBool::new(false),
            fail_atomic_apply: AtomicBool::new(false),
            fail_commit_machine_lifecycle: AtomicBool::new(false),
            fail_load_input_states_for: None,
            fail_load_boundary_receipt_for: None,
            fail_load_runtime_state_for: Some(runtime_id),
            runtime_state_overrides: Mutex::new(HashMap::new()),
        }
    }

    fn seed_runtime_state_projection(
        &self,
        runtime_id: LogicalRuntimeId,
        runtime_state: RuntimeState,
    ) {
        self.runtime_state_overrides
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(runtime_id, runtime_state);
    }
}

#[async_trait]
impl RuntimeStore for FailPersistInputStore {
    async fn commit_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: SessionDelta,
    ) -> Result<(), RuntimeStoreError> {
        self.inner
            .commit_session_snapshot(runtime_id, session_delta)
            .await
    }

    async fn atomic_apply(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: Option<SessionDelta>,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<StoredInputState>,
        session_store_key: Option<meerkat_core::types::SessionId>,
    ) -> Result<(), RuntimeStoreError> {
        if self.fail_atomic_apply.swap(false, Ordering::SeqCst) {
            return Err(RuntimeStoreError::WriteFailed(
                "synthetic atomic_apply failure".into(),
            ));
        }
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
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<StoredInputState>, RuntimeStoreError> {
        if self.fail_load_input_states_for.as_ref() == Some(runtime_id) {
            return Err(RuntimeStoreError::ReadFailed(
                "synthetic legacy input-state load failure".into(),
            ));
        }
        self.inner.load_input_states(runtime_id).await
    }

    async fn load_boundary_receipt(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<Option<RunBoundaryReceipt>, RuntimeStoreError> {
        if self.fail_load_boundary_receipt_for.as_ref() == Some(runtime_id) {
            return Err(RuntimeStoreError::ReadFailed(
                "synthetic legacy boundary-receipt load failure".into(),
            ));
        }
        self.inner
            .load_boundary_receipt(runtime_id, run_id, sequence)
            .await
    }

    async fn load_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        self.inner.load_session_snapshot(runtime_id).await
    }

    async fn persist_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        state: &StoredInputState,
    ) -> Result<(), RuntimeStoreError> {
        if self.fail_persist_input_state.swap(false, Ordering::SeqCst) {
            return Err(RuntimeStoreError::WriteFailed(
                "synthetic persist_input_state failure".into(),
            ));
        }
        self.inner.persist_input_state(runtime_id, state).await
    }

    async fn load_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, RuntimeStoreError> {
        self.inner.load_input_state(runtime_id, input_id).await
    }

    async fn load_runtime_state(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeState>, RuntimeStoreError> {
        if self.fail_load_runtime_state_for.as_ref() == Some(runtime_id) {
            return Err(RuntimeStoreError::ReadFailed(
                "synthetic legacy runtime-state load failure".into(),
            ));
        }
        if let Some(runtime_state) = self
            .runtime_state_overrides
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(runtime_id)
            .copied()
        {
            return Ok(Some(runtime_state));
        }
        self.inner.load_runtime_state(runtime_id).await
    }

    async fn commit_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        commit: meerkat_runtime::store::MachineLifecycleCommit,
        input_states: &[StoredInputState],
    ) -> Result<(), RuntimeStoreError> {
        if self
            .fail_commit_machine_lifecycle
            .swap(false, Ordering::SeqCst)
        {
            return Err(RuntimeStoreError::WriteFailed(
                "synthetic commit_machine_lifecycle failure".into(),
            ));
        }
        self.runtime_state_overrides
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(runtime_id);
        self.inner
            .commit_machine_lifecycle(runtime_id, commit, input_states)
            .await
    }

    async fn persist_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        snapshot: &meerkat_runtime::PersistedOpsSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        self.inner.persist_ops_lifecycle(runtime_id, snapshot).await
    }

    async fn load_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<meerkat_runtime::PersistedOpsSnapshot>, RuntimeStoreError> {
        self.inner.load_ops_lifecycle(runtime_id).await
    }
}

fn make_prompt(text: &str) -> Input {
    Input::Prompt(PromptInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Operator,
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        text: text.into(),
        blocks: None,
        turn_metadata: None,
    })
}

fn make_multimodal_prompt(text: &str, label: &str) -> Input {
    Input::Prompt(PromptInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Operator,
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        text: text.into(),
        blocks: Some(vec![
            ContentBlock::Text {
                text: text.to_string(),
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Inline {
                    data: format!("base64-{label}"),
                },
            },
        ]),
        turn_metadata: None,
    })
}

#[tokio::test]
async fn durable_before_ack() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");
    let mut driver = PersistentRuntimeDriver::new(rid.clone(), store.clone(), memory_blob_store());

    let input = make_prompt("hello");
    let input_id = input.id().clone();
    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());

    // Verify state was persisted to store BEFORE we returned
    let stored = store.load_input_state(&rid, &input_id).await.unwrap();
    assert!(stored.is_some());
    assert!(stored.unwrap().state.persisted_input.is_some());
}

#[tokio::test]
async fn dedup_not_persisted() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");
    let mut driver = PersistentRuntimeDriver::new(rid.clone(), store.clone(), memory_blob_store());

    let key = meerkat_runtime::identifiers::IdempotencyKey::new("req-1");
    let mut input1 = make_prompt("hello");
    if let Input::Prompt(ref mut p) = input1 {
        p.header.idempotency_key = Some(key.clone());
    }
    driver.accept_input(input1).await.unwrap();

    let mut input2 = make_prompt("hello again");
    if let Input::Prompt(ref mut p) = input2 {
        p.header.idempotency_key = Some(key);
    }
    let outcome = driver.accept_input(input2).await.unwrap();
    assert!(outcome.is_deduplicated());

    // Only one state in store
    let states = store.load_input_states(&rid).await.unwrap();
    assert_eq!(states.len(), 1);
}

#[tokio::test]
async fn recover_from_store() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");

    // Pre-populate store with a state (simulating crash recovery)
    let input = make_prompt("hello");
    let input_id = input.id().clone();
    let mut state = InputState::new_accepted(input_id.clone());
    state.persisted_input = Some(input.clone());
    state.durability = Some(InputDurability::Durable);
    store
        .persist_input_state(&rid, &stored_accepted(state))
        .await
        .unwrap();

    // Create a fresh driver (simulating restart)
    let mut driver = PersistentRuntimeDriver::new(rid, store, memory_blob_store());

    // Recover
    let report = driver.recover().await.unwrap();
    assert_eq!(report.inputs_recovered, 1);

    // State should now be in the driver
    assert!(driver.input_state(&input_id).is_some());
    let dequeued = driver.dequeue_next();
    assert!(
        dequeued.is_some(),
        "Recovered queued input should be re-enqueued"
    );
    let (queued_id, queued_input) = dequeued.unwrap();
    assert_eq!(queued_id, input_id);
    assert_eq!(queued_input.id(), &input_id);
}

#[tokio::test]
async fn recover_ignores_legacy_session_alias_input_states() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let session_id = SessionId::new();
    let canonical_rid = LogicalRuntimeId::for_session(&session_id);
    let legacy_rid = LogicalRuntimeId::legacy_session_uuid_alias(&session_id);

    let canonical_input = make_prompt("canonical input");
    let canonical_input_id = canonical_input.id().clone();
    let mut canonical_state = InputState::new_accepted(canonical_input_id.clone());
    canonical_state.persisted_input = Some(canonical_input);
    canonical_state.durability = Some(InputDurability::Durable);
    store
        .persist_input_state(&canonical_rid, &stored_accepted(canonical_state))
        .await
        .unwrap();

    let legacy_input = make_prompt("legacy input");
    let legacy_input_id = legacy_input.id().clone();
    let mut legacy_state = InputState::new_accepted(legacy_input_id.clone());
    legacy_state.persisted_input = Some(legacy_input);
    legacy_state.durability = Some(InputDurability::Durable);
    store
        .persist_input_state(&legacy_rid, &stored_accepted(legacy_state))
        .await
        .unwrap();

    let mut driver = PersistentRuntimeDriver::new(canonical_rid, store, memory_blob_store());
    let report = driver.recover().await.unwrap();

    assert_eq!(report.inputs_recovered, 1);
    assert!(driver.input_state(&canonical_input_id).is_some());
    assert!(
        driver.input_state(&legacy_input_id).is_none(),
        "legacy raw session alias input state must not drive recovery"
    );
}

#[tokio::test]
async fn recover_rebuilds_dedup_index() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");
    let key = meerkat_runtime::identifiers::IdempotencyKey::new("dedup-key");

    // Pre-populate store with a state that has an idempotency key
    let mut input = make_prompt("dedup original");
    if let Input::Prompt(ref mut p) = input {
        p.header.idempotency_key = Some(key.clone());
    }
    let input_id = input.id().clone();
    let mut state = InputState::new_accepted(input_id.clone());
    state.idempotency_key = Some(key.clone());
    state.durability = Some(InputDurability::Durable);
    state.persisted_input = Some(input);
    store
        .persist_input_state(&rid, &stored_accepted(state))
        .await
        .unwrap();

    // Create a fresh driver and recover
    let mut driver = PersistentRuntimeDriver::new(rid, store, memory_blob_store());
    driver.recover().await.unwrap();

    // Now try to accept a new input with the same idempotency key
    let mut dup_input = make_prompt("duplicate");
    if let Input::Prompt(ref mut p) = dup_input {
        p.header.idempotency_key = Some(key);
    }
    let outcome = driver.accept_input(dup_input).await.unwrap();
    assert!(
        outcome.is_deduplicated(),
        "After recovery, dedup index should be rebuilt so duplicates are caught"
    );
}

#[tokio::test]
async fn recover_filters_ephemeral_inputs() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");

    // Pre-populate with an ephemeral input state
    let input_id = InputId::new();
    let mut state = InputState::new_accepted(input_id.clone());
    state.durability = Some(InputDurability::Ephemeral);
    store
        .persist_input_state(&rid, &stored_accepted(state))
        .await
        .unwrap();

    // Create fresh driver and recover
    let mut driver = PersistentRuntimeDriver::new(rid, store, memory_blob_store());
    let report = driver.recover().await.unwrap();

    // Ephemeral input should NOT be recovered (it shouldn't survive restart)
    assert!(
        driver.input_state(&input_id).is_none(),
        "Ephemeral inputs should be filtered during recovery"
    );
    assert_eq!(report.inputs_recovered, 0);
}

#[tokio::test]
async fn durable_runtime_input_externalizes_inline_images_before_ack() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");
    let mut driver = PersistentRuntimeDriver::new(rid.clone(), store.clone(), memory_blob_store());

    let input = make_multimodal_prompt("hello", "driver");
    let input_id = input.id().clone();
    let outcome = driver.accept_input(input).await.unwrap();
    assert!(outcome.is_accepted());

    let stored = store
        .load_input_state(&rid, &input_id)
        .await
        .unwrap()
        .expect("persisted input should exist");
    let persisted_input = stored
        .state
        .persisted_input
        .expect("accepted durable input should be persisted");
    match persisted_input {
        Input::Prompt(prompt) => {
            let blocks = prompt.blocks.expect("multimodal blocks should persist");
            assert!(
                blocks.iter().any(|block| matches!(
                    block,
                    ContentBlock::Image {
                        data: ImageData::Blob { .. },
                        ..
                    }
                )),
                "persisted runtime input should externalize image bytes"
            );
            assert!(
                !blocks.iter().any(|block| matches!(
                    block,
                    ContentBlock::Image {
                        data: ImageData::Inline { .. },
                        ..
                    }
                )),
                "persisted runtime input must not retain inline image bytes"
            );
        }
        other => panic!("expected prompt input, got {other:?}"),
    }
}

#[tokio::test]
async fn durable_accept_failure_restores_canonical_ingress_state() {
    let inner = Arc::new(InMemoryRuntimeStore::new());
    let store: Arc<dyn RuntimeStore> = Arc::new(FailPersistInputStore::new(inner.clone()));
    let rid = LogicalRuntimeId::new("test");
    let mut driver = PersistentRuntimeDriver::new(rid.clone(), store, memory_blob_store());

    let input = make_prompt("hello");
    let input_id = input.id().clone();
    let retry_input = input.clone();

    let err = driver
        .accept_input(input)
        .await
        .expect_err("persist should fail");
    let err_text = err.to_string();
    assert!(
        err_text.contains("synthetic persist_input_state failure"),
        "unexpected error: {err_text}"
    );
    assert!(
        driver.input_state(&input_id).is_none(),
        "failed durable admission must not leave canonical input state behind"
    );
    assert!(
        driver.dequeue_next().is_none(),
        "failed durable admission must not leave a queued phantom input"
    );
    assert!(
        inner
            .load_input_state(&rid, &input_id)
            .await
            .unwrap()
            .is_none(),
        "failed durable admission must not persist input state"
    );

    let outcome = driver.accept_input(retry_input).await.unwrap();
    assert!(
        outcome.is_accepted(),
        "retry after failed durable admission should succeed cleanly"
    );
}

#[tokio::test]
async fn recovery_lifecycle_commit_failure_restores_recovered_projection() {
    let inner = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");
    let input = make_prompt("recover rollback");
    let input_id = input.id().clone();
    let mut state = InputState::new_accepted(input_id.clone());
    state.persisted_input = Some(input);
    state.durability = Some(InputDurability::Durable);
    inner
        .persist_input_state(&rid, &stored_accepted(state))
        .await
        .unwrap();

    let store: Arc<dyn RuntimeStore> = Arc::new(
        FailPersistInputStore::fail_commit_machine_lifecycle_once(inner.clone()),
    );
    let mut driver = PersistentRuntimeDriver::new(rid.clone(), store, memory_blob_store());

    let err = driver
        .recover()
        .await
        .expect_err("recovery lifecycle commit should fail");
    assert!(
        err.to_string()
            .contains("synthetic commit_machine_lifecycle failure"),
        "unexpected error: {err}",
    );
    assert!(
        driver.input_state(&input_id).is_none(),
        "failed recovery must not leave recovered input in the live driver",
    );
    assert!(
        driver.dequeue_next().is_none(),
        "failed recovery must not leave recovered queue projection",
    );
    let stored = inner
        .load_input_state(&rid, &input_id)
        .await
        .unwrap()
        .expect("durable recovery seed should remain");
    assert_eq!(
        stored.seed.phase,
        meerkat_runtime::input_state::InputLifecycleState::Accepted,
        "failed recovery must not rewrite durable input lifecycle",
    );
}

#[tokio::test]
async fn recovery_rejecting_later_row_restores_partial_recovered_projection() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");

    let valid_input = make_prompt("valid recovered row");
    let valid_id = valid_input.id().clone();
    let mut valid_state = InputState::new_accepted(valid_id.clone());
    valid_state.persisted_input = Some(valid_input);
    valid_state.durability = Some(InputDurability::Durable);
    store
        .persist_input_state(&rid, &stored_accepted(valid_state))
        .await
        .unwrap();

    let invalid_input = make_prompt("unstamped recovered row");
    let invalid_id = invalid_input.id().clone();
    let mut invalid_state = InputState::new_accepted(invalid_id.clone());
    invalid_state.persisted_input = Some(invalid_input);
    invalid_state.durability = Some(InputDurability::Durable);
    store
        .persist_input_state(
            &rid,
            &StoredInputState {
                state: invalid_state,
                seed: InputStateSeed::new_accepted(),
            },
        )
        .await
        .unwrap();

    let mut driver = PersistentRuntimeDriver::new(rid, store, memory_blob_store());
    let err = driver
        .recover()
        .await
        .expect_err("unstamped later row should fail recovery");

    assert!(
        err.to_string()
            .contains("missing runtime execution semantics stamp"),
        "unexpected error: {err}",
    );
    assert!(
        driver.input_state(&valid_id).is_none(),
        "failed recovery must roll back already admitted recovered rows",
    );
    assert!(
        driver.input_state(&invalid_id).is_none(),
        "failed recovery must not retain the rejected row",
    );
    assert!(
        driver.dequeue_next().is_none(),
        "failed recovery must not leave recovered queue projection",
    );
}

#[tokio::test]
async fn recover_allows_legacy_unstamped_terminal_rows() {
    use meerkat_runtime::input_state::InputLifecycleState;

    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");

    let input = make_prompt("legacy terminal row");
    let input_id = input.id().clone();
    let mut state = InputState::new_accepted(input_id.clone());
    state.persisted_input = Some(input);
    state.durability = Some(InputDurability::Durable);
    state.terminal_outcome = Some(InputTerminalOutcome::Consumed);
    store
        .persist_input_state(
            &rid,
            &StoredInputState {
                state,
                seed: InputStateSeed {
                    phase: InputLifecycleState::Consumed,
                    last_run_id: None,
                    last_boundary_sequence: None,
                    terminal_outcome: Some(InputTerminalOutcome::Consumed),
                    attempt_count: 0,
                },
            },
        )
        .await
        .unwrap();

    let mut driver = PersistentRuntimeDriver::new(rid.clone(), store.clone(), memory_blob_store());
    driver
        .recover()
        .await
        .expect("legacy unstamped terminal row should not block recovery");

    assert!(
        driver.input_state(&input_id).is_some(),
        "terminal history should remain queryable after recovery"
    );
    assert_eq!(
        driver.input_phase(&input_id),
        Some(InputLifecycleState::Consumed)
    );
    assert!(
        driver.active_input_ids().is_empty(),
        "terminal rows must not become active"
    );
    assert!(
        driver.dequeue_next().is_none(),
        "terminal rows must not enter runtime queues"
    );

    let stored = store
        .load_input_state(&rid, &input_id)
        .await
        .unwrap()
        .expect("terminal row should remain persisted");
    assert_eq!(stored.seed.phase, InputLifecycleState::Consumed);
    assert_eq!(stored.state.runtime_semantics, None);
}

#[tokio::test]
async fn recover_consumes_committed_applied_pending_inputs() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;

    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");
    let input = make_prompt("already committed");
    let input_id = input.id().clone();
    let run_id = RunId::new();

    let mut state = InputState::new_accepted(input_id.clone());
    state.persisted_input = Some(input);
    state.durability = Some(InputDurability::Durable);
    // Simulate Accepted → Queued → Staged → Applied → AppliedPendingConsumption
    // by seeding the DSL-owned phase + run association alongside the shell.
    use meerkat_runtime::input_state::InputLifecycleState;
    state.attempt_count = 1;
    stamp_runtime_semantics(&mut state);
    let stored = StoredInputState {
        state,
        seed: InputStateSeed {
            phase: InputLifecycleState::AppliedPendingConsumption,
            last_run_id: Some(run_id.clone()),
            last_boundary_sequence: Some(0),
            terminal_outcome: None,
            attempt_count: 1,
        },
    };
    store.persist_input_state(&rid, &stored).await.unwrap();
    store
        .atomic_apply(
            &rid,
            None,
            RunBoundaryReceipt {
                run_id: run_id.clone(),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![input_id.clone()],
                conversation_digest: None,
                message_count: 1,
                sequence: 0,
            },
            vec![stored.clone()],
            None,
        )
        .await
        .unwrap();

    let mut driver = PersistentRuntimeDriver::new(rid, store, memory_blob_store());
    driver.recover().await.unwrap();

    let recovered = driver.input_state(&input_id);
    assert!(
        recovered.is_some(),
        "committed input should remain queryable after recovery"
    );
    assert_eq!(
        driver.inner_ref().input_phase(&input_id),
        Some(InputLifecycleState::Consumed)
    );
    assert!(
        driver.active_input_ids().is_empty(),
        "committed applied inputs should not stay active after recovery"
    );
    assert!(
        driver.dequeue_next().is_none(),
        "committed applied inputs should not be replayed after recovery"
    );
}

#[tokio::test]
async fn recover_duplicate_legacy_input_row_keeps_canonical_boundary_receipt() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
    use meerkat_runtime::input_state::InputLifecycleState;

    let store = Arc::new(InMemoryRuntimeStore::new());
    let session_id = SessionId::new();
    let canonical_rid = LogicalRuntimeId::for_session(&session_id);
    let legacy_rid = LogicalRuntimeId::legacy_session_uuid_alias(&session_id);
    let input = make_prompt("already committed under canonical alias");
    let input_id = input.id().clone();
    let run_id = RunId::new();

    let mut canonical_state = InputState::new_accepted(input_id.clone());
    canonical_state.persisted_input = Some(input.clone());
    canonical_state.durability = Some(InputDurability::Durable);
    canonical_state.attempt_count = 1;
    stamp_runtime_semantics(&mut canonical_state);
    let canonical_stored = StoredInputState {
        state: canonical_state.clone(),
        seed: InputStateSeed {
            phase: InputLifecycleState::AppliedPendingConsumption,
            last_run_id: Some(run_id.clone()),
            last_boundary_sequence: Some(0),
            terminal_outcome: None,
            attempt_count: 1,
        },
    };
    store
        .atomic_apply(
            &canonical_rid,
            None,
            RunBoundaryReceipt {
                run_id: run_id.clone(),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![input_id.clone()],
                conversation_digest: None,
                message_count: 1,
                sequence: 0,
            },
            vec![canonical_stored.clone()],
            None,
        )
        .await
        .unwrap();

    let mut legacy_state = canonical_state;
    legacy_state.updated_at = canonical_stored.state.updated_at + chrono::Duration::milliseconds(1);
    let legacy_stored = StoredInputState {
        state: legacy_state,
        seed: canonical_stored.seed.clone(),
    };
    store
        .persist_input_state(&legacy_rid, &legacy_stored)
        .await
        .unwrap();

    let mut driver = PersistentRuntimeDriver::new(canonical_rid, store, memory_blob_store());
    driver.recover().await.unwrap();

    assert_eq!(
        driver.inner_ref().input_phase(&input_id),
        Some(InputLifecycleState::Consumed),
        "duplicate legacy row must still consult the canonical boundary receipt"
    );
    assert!(
        driver.dequeue_next().is_none(),
        "canonical committed input must not be replayed because the newer duplicate row came from the legacy alias"
    );
}

#[tokio::test]
async fn recover_prefers_canonical_duplicate_over_newer_stale_legacy_row() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
    use meerkat_runtime::input_state::InputLifecycleState;

    let store = Arc::new(InMemoryRuntimeStore::new());
    let session_id = SessionId::new();
    let canonical_rid = LogicalRuntimeId::for_session(&session_id);
    let legacy_rid = LogicalRuntimeId::legacy_session_uuid_alias(&session_id);
    let input = make_prompt("canonical applied row beats stale legacy accepted row");
    let input_id = input.id().clone();
    let run_id = RunId::new();

    let mut canonical_state = InputState::new_accepted(input_id.clone());
    canonical_state.persisted_input = Some(input.clone());
    canonical_state.durability = Some(InputDurability::Durable);
    canonical_state.attempt_count = 1;
    stamp_runtime_semantics(&mut canonical_state);
    let canonical_stored = StoredInputState {
        state: canonical_state.clone(),
        seed: InputStateSeed {
            phase: InputLifecycleState::AppliedPendingConsumption,
            last_run_id: Some(run_id.clone()),
            last_boundary_sequence: Some(0),
            terminal_outcome: None,
            attempt_count: 1,
        },
    };
    store
        .atomic_apply(
            &canonical_rid,
            None,
            RunBoundaryReceipt {
                run_id: run_id.clone(),
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![input_id.clone()],
                conversation_digest: None,
                message_count: 1,
                sequence: 0,
            },
            vec![canonical_stored.clone()],
            None,
        )
        .await
        .unwrap();

    let mut legacy_state = canonical_state;
    legacy_state.updated_at = canonical_stored.state.updated_at + chrono::Duration::milliseconds(1);
    store
        .persist_input_state(&legacy_rid, &stored_accepted(legacy_state))
        .await
        .unwrap();

    let mut driver = PersistentRuntimeDriver::new(canonical_rid, store, memory_blob_store());
    driver.recover().await.unwrap();

    assert_eq!(
        driver.inner_ref().input_phase(&input_id),
        Some(InputLifecycleState::Consumed),
        "canonical applied row must not be replaced by a newer stale legacy row"
    );
    assert!(
        driver.dequeue_next().is_none(),
        "newer stale legacy row must not replay a canonically committed input"
    );
}

#[tokio::test]
async fn recover_ignores_legacy_boundary_receipt_load_error_after_canonical_miss() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_runtime::input_state::InputLifecycleState;

    let inner = Arc::new(InMemoryRuntimeStore::new());
    let session_id = SessionId::new();
    let canonical_rid = LogicalRuntimeId::for_session(&session_id);
    let legacy_rid = LogicalRuntimeId::legacy_session_uuid_alias(&session_id);
    let input = make_prompt("canonical applied row with missing receipt");
    let input_id = input.id().clone();
    let run_id = RunId::new();

    let mut state = InputState::new_accepted(input_id.clone());
    state.persisted_input = Some(input);
    state.durability = Some(InputDurability::Durable);
    state.attempt_count = 1;
    stamp_runtime_semantics(&mut state);
    inner
        .persist_input_state(
            &canonical_rid,
            &StoredInputState {
                state,
                seed: InputStateSeed {
                    phase: InputLifecycleState::AppliedPendingConsumption,
                    last_run_id: Some(run_id),
                    last_boundary_sequence: Some(0),
                    terminal_outcome: None,
                    attempt_count: 1,
                },
            },
        )
        .await
        .unwrap();

    let store = Arc::new(FailPersistInputStore::fail_load_boundary_receipt_for(
        inner, legacy_rid,
    ));
    let mut driver = PersistentRuntimeDriver::new(canonical_rid, store, memory_blob_store());

    let report = driver
        .recover()
        .await
        .expect("legacy receipt read failure must not poison canonical missing-receipt recovery");

    assert_eq!(report.inputs_recovered, 1);
    assert_eq!(
        driver.inner_ref().input_phase(&input_id),
        Some(InputLifecycleState::Queued),
        "canonical missing receipt should recover by requeueing the input"
    );
    assert_eq!(
        driver.dequeue_next().map(|(queued_id, _)| queued_id),
        Some(input_id),
        "requeued canonical input should remain available for replay"
    );
}

#[tokio::test]
async fn recover_treats_canonical_boundary_receipt_miss_as_authoritative() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
    use meerkat_runtime::input_state::InputLifecycleState;

    let store = Arc::new(InMemoryRuntimeStore::new());
    let session_id = SessionId::new();
    let canonical_rid = LogicalRuntimeId::for_session(&session_id);
    let legacy_rid = LogicalRuntimeId::legacy_session_uuid_alias(&session_id);
    let input = make_prompt("canonical missing receipt must not consume from legacy receipt");
    let input_id = input.id().clone();
    let run_id = RunId::new();

    let mut state = InputState::new_accepted(input_id.clone());
    state.persisted_input = Some(input);
    state.durability = Some(InputDurability::Durable);
    state.attempt_count = 1;
    stamp_runtime_semantics(&mut state);
    store
        .persist_input_state(
            &canonical_rid,
            &StoredInputState {
                state,
                seed: InputStateSeed {
                    phase: InputLifecycleState::AppliedPendingConsumption,
                    last_run_id: Some(run_id.clone()),
                    last_boundary_sequence: Some(0),
                    terminal_outcome: None,
                    attempt_count: 1,
                },
            },
        )
        .await
        .unwrap();
    store
        .atomic_apply(
            &legacy_rid,
            None,
            RunBoundaryReceipt {
                run_id,
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: vec![input_id.clone()],
                conversation_digest: None,
                message_count: 1,
                sequence: 0,
            },
            vec![],
            None,
        )
        .await
        .unwrap();

    let mut driver = PersistentRuntimeDriver::new(canonical_rid, store, memory_blob_store());
    let report = driver
        .recover()
        .await
        .expect("canonical receipt miss should not be poisoned by legacy receipt presence");

    assert_eq!(report.inputs_recovered, 1);
    assert_eq!(
        driver.inner_ref().input_phase(&input_id),
        Some(InputLifecycleState::Queued),
        "canonical receipt miss should requeue instead of consuming from stale legacy receipt"
    );
    assert_eq!(
        driver.dequeue_next().map(|(queued_id, _)| queued_id),
        Some(input_id),
        "canonical missing-receipt input should remain queued for replay"
    );
}

#[tokio::test]
async fn recover_ignores_legacy_input_state_load_error_after_canonical_states() {
    let inner = Arc::new(InMemoryRuntimeStore::new());
    let session_id = SessionId::new();
    let canonical_rid = LogicalRuntimeId::for_session(&session_id);
    let legacy_rid = LogicalRuntimeId::legacy_session_uuid_alias(&session_id);
    let input = make_prompt("canonical survives unreadable legacy alias");
    let input_id = input.id().clone();

    let mut state = InputState::new_accepted(input_id.clone());
    state.persisted_input = Some(input);
    state.durability = Some(InputDurability::Durable);
    inner
        .persist_input_state(&canonical_rid, &stored_accepted(state))
        .await
        .unwrap();

    let store = Arc::new(FailPersistInputStore::fail_load_input_states_for(
        inner, legacy_rid,
    ));
    let mut driver = PersistentRuntimeDriver::new(canonical_rid, store, memory_blob_store());

    let report = driver
        .recover()
        .await
        .expect("legacy input-state read failure must not poison canonical recovery");

    assert_eq!(report.inputs_recovered, 1);
    assert!(
        driver.input_state(&input_id).is_some(),
        "canonical input state should recover even when legacy alias load fails"
    );
}

#[tokio::test]
async fn recover_ignores_legacy_input_state_load_error_after_empty_canonical_read() {
    let inner = Arc::new(InMemoryRuntimeStore::new());
    let session_id = SessionId::new();
    let canonical_rid = LogicalRuntimeId::for_session(&session_id);
    let legacy_rid = LogicalRuntimeId::legacy_session_uuid_alias(&session_id);
    let store = Arc::new(FailPersistInputStore::fail_load_input_states_for(
        inner, legacy_rid,
    ));
    let mut driver = PersistentRuntimeDriver::new(canonical_rid, store, memory_blob_store());

    let report = driver
        .recover()
        .await
        .expect("legacy input-state read failure must not poison empty canonical recovery");

    assert_eq!(report.inputs_recovered, 0);
    assert!(
        driver.active_input_ids().is_empty(),
        "empty canonical recovery should stay empty when legacy alias load fails"
    );
}

#[tokio::test]
async fn recover_ignores_legacy_runtime_state_load_error_after_canonical_miss() {
    let inner = Arc::new(InMemoryRuntimeStore::new());
    let session_id = SessionId::new();
    let canonical_rid = LogicalRuntimeId::for_session(&session_id);
    let legacy_rid = LogicalRuntimeId::legacy_session_uuid_alias(&session_id);
    let store = Arc::new(FailPersistInputStore::fail_load_runtime_state_for(
        inner, legacy_rid,
    ));
    let mut driver = PersistentRuntimeDriver::new(canonical_rid, store, memory_blob_store());

    let report = driver
        .recover()
        .await
        .expect("legacy runtime-state read failure must not poison a canonical miss");

    assert_eq!(report.inputs_recovered, 0);
    assert_eq!(
        driver.runtime_state(),
        RuntimeState::Idle,
        "canonical runtime-state miss should retain the fresh idle runtime state"
    );
}

#[tokio::test]
async fn driver_persistent_recovery_persists_machine_lifecycle_truth_not_terminal_projection() {
    let inner = Arc::new(InMemoryRuntimeStore::new());
    let store = Arc::new(FailPersistInputStore::passthrough(inner));
    let rid = LogicalRuntimeId::new("test");
    store.seed_runtime_state_projection(rid.clone(), RuntimeState::Destroyed);

    let mut driver = PersistentRuntimeDriver::new(rid.clone(), store.clone(), memory_blob_store());
    let report = driver.recover().await.unwrap();

    assert_eq!(report.inputs_recovered, 0);
    assert_eq!(
        driver.runtime_state(),
        RuntimeState::Idle,
        "recovery must not realize destroyed lifecycle truth from a runtime-state projection",
    );
    assert_eq!(
        store.load_runtime_state(&rid).await.unwrap(),
        Some(RuntimeState::Idle),
        "recovery durable projection must be rewritten from machine truth, not the loaded terminal projection",
    );
}

#[tokio::test]
async fn driver_persistent_recovery_fails_closed_for_terminal_projection_with_active_inputs() {
    let inner = Arc::new(InMemoryRuntimeStore::new());
    let store = Arc::new(FailPersistInputStore::passthrough(inner.clone()));
    let rid = LogicalRuntimeId::new("test");
    let input = make_prompt("terminal projection conflict");
    let input_id = input.id().clone();
    let mut state = InputState::new_accepted(input_id.clone());
    state.persisted_input = Some(input);
    state.durability = Some(InputDurability::Durable);
    store
        .persist_input_state(&rid, &stored_accepted(state))
        .await
        .unwrap();
    store.seed_runtime_state_projection(rid.clone(), RuntimeState::Destroyed);

    let mut driver = PersistentRuntimeDriver::new(rid.clone(), store.clone(), memory_blob_store());
    let error = driver
        .recover()
        .await
        .expect_err("terminal runtime-state projection with active inputs must fail closed");
    assert!(
        error
            .to_string()
            .contains("runtime-state projection 'destroyed' conflicts with 1 active inputs"),
        "unexpected error: {error}",
    );
    assert_eq!(
        driver.runtime_state(),
        RuntimeState::Idle,
        "failed recovery must not force destroyed state from the store projection",
    );
    assert!(
        driver.input_state(&input_id).is_none(),
        "failed recovery must roll back active inputs after detecting the projection conflict",
    );
    assert_eq!(
        store.load_runtime_state(&rid).await.unwrap(),
        Some(RuntimeState::Destroyed),
        "failed recovery must not repair or overwrite durable lifecycle projection rows",
    );
}
