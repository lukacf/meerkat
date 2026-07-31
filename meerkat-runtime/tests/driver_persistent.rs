#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use chrono::Utc;
use meerkat_core::BlobStore;
use meerkat_core::lifecycle::{InputId, RunId, run_receipt::RunBoundaryReceipt};
use meerkat_core::types::{ContentBlock, ImageData, SessionId};
use meerkat_runtime::input_state::{InputStatePersistenceRecord, InputStateSeed, StoredInputState};
use meerkat_runtime::store::{
    FencedInputStateBatchCasOutcome, InputStateBatchCasImplementationProfile,
    InputStateBatchCasOutcome, InputStateRow, PreparedRecoveryInputSnapshot,
    RecoveryInputSetRevision, RecoveryInputStateMutation, RuntimeStoreError,
    RuntimeStoreWriteFence, load_runtime_state,
};
use meerkat_runtime::{
    EphemeralRuntimeDriver, InMemoryRuntimeStore, Input, InputDurability, InputHeader, InputOrigin,
    InputState, InputVisibility, LogicalRuntimeId, MeerkatMachine, PersistentRuntimeDriver,
    PromptInput, RuntimeDriver, RuntimeState, RuntimeStore, SerializedSessionSnapshot,
    SessionServiceRuntimeExt,
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
        meerkat_runtime::ingress_types::RuntimeInputSemantics::try_from_generated_admission(
            input, true,
        )
        .expect("generated admission semantics"),
    );
    state.policy = Some(meerkat_runtime::input_state::PolicySnapshot {
        version: policy_version,
        decision: policy,
    });
}

fn stored_accepted(mut state: InputState) -> StoredInputState {
    stamp_runtime_semantics(&mut state);
    let mut seed = InputStateSeed::new_accepted();
    seed.recovery_lane = Some(meerkat_core::types::HandlingMode::Queue);
    StoredInputState { seed, state }
}

fn persistable(stored: StoredInputState) -> InputStatePersistenceRecord {
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new(format!(
        "persistence-record-{}",
        stored.state.input_id
    )));
    driver
        .recover_input_state_persistence_record(stored)
        .expect("test input-state seed should pass generated recovery authority")
}

struct FailPersistInputStore {
    inner: Arc<InMemoryRuntimeStore>,
    fail_persist_input_state: AtomicBool,
    fail_atomic_apply: AtomicBool,
    fail_commit_machine_lifecycle: AtomicBool,
}

impl FailPersistInputStore {
    fn new(inner: Arc<InMemoryRuntimeStore>) -> Self {
        Self {
            inner,
            fail_persist_input_state: AtomicBool::new(true),
            fail_atomic_apply: AtomicBool::new(false),
            fail_commit_machine_lifecycle: AtomicBool::new(false),
        }
    }

    fn passthrough(inner: Arc<InMemoryRuntimeStore>) -> Self {
        Self {
            inner,
            fail_persist_input_state: AtomicBool::new(false),
            fail_atomic_apply: AtomicBool::new(false),
            fail_commit_machine_lifecycle: AtomicBool::new(false),
        }
    }

    fn fail_commit_machine_lifecycle_once(inner: Arc<InMemoryRuntimeStore>) -> Self {
        Self {
            inner,
            fail_persist_input_state: AtomicBool::new(false),
            fail_atomic_apply: AtomicBool::new(false),
            fail_commit_machine_lifecycle: AtomicBool::new(true),
        }
    }
}

async fn persist_destroyed_runtime_lifecycle(
    store: Arc<FailPersistInputStore>,
) -> (SessionId, LogicalRuntimeId) {
    let session_id = SessionId::new();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let adapter = MeerkatMachine::persistent(store as Arc<dyn RuntimeStore>, memory_blob_store());
    adapter
        .register_session(session_id.clone())
        .await
        .expect("register session");
    meerkat_runtime::traits::RuntimeControlPlane::destroy(&adapter, &runtime_id)
        .await
        .expect("generated destroy should persist lifecycle");
    (session_id, runtime_id)
}

#[async_trait]
impl RuntimeStore for FailPersistInputStore {
    fn session_authority_ops(&self) -> &dyn meerkat_runtime::store::RuntimeSessionAuthorityOps {
        self.inner.session_authority_ops()
    }

    fn session_persistence_profile(
        &self,
    ) -> meerkat_runtime::store::RuntimeSessionPersistenceProfile {
        meerkat_runtime::store::RuntimeSessionPersistenceProfile::WholeBlobV1
    }

    fn supports_compaction_projection_outbox(&self) -> bool {
        self.inner.supports_compaction_projection_outbox()
    }

    fn input_state_batch_cas_implementation_profile(
        &self,
    ) -> InputStateBatchCasImplementationProfile {
        self.inner.input_state_batch_cas_implementation_profile()
    }

    async fn observe_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<meerkat_runtime::store::MachineLifecycleObservation, RuntimeStoreError> {
        self.inner.observe_machine_lifecycle(runtime_id).await
    }

    async fn compare_and_swap_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: meerkat_runtime::store::MachineLifecycleExpectedVersion,
        replacement: meerkat_runtime::store::MachineLifecycleCommit,
    ) -> Result<meerkat_runtime::store::MachineLifecycleCasOutcome, RuntimeStoreError> {
        if self
            .fail_commit_machine_lifecycle
            .swap(false, Ordering::SeqCst)
        {
            return Err(RuntimeStoreError::WriteFailed(
                "synthetic commit_machine_lifecycle failure".into(),
            ));
        }
        self.inner
            .compare_and_swap_machine_lifecycle(runtime_id, expected, replacement)
            .await
    }

    async fn commit_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: SerializedSessionSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        self.inner
            .commit_session_snapshot(runtime_id, session_delta)
            .await
    }

    async fn commit_prepared_whole_blob_rewrite_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        boundary: meerkat_runtime::store::PreparedWholeBlobRewriteStoreParts,
    ) -> Result<meerkat_runtime::store::WholeBlobStoreAuthority, RuntimeStoreError> {
        self.inner
            .commit_prepared_whole_blob_rewrite_boundary(runtime_id, boundary)
            .await
    }

    async fn atomic_apply(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: Option<SerializedSessionSnapshot>,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
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
    ) -> Result<Vec<InputStateRow>, RuntimeStoreError> {
        self.inner.load_input_states(runtime_id).await
    }

    async fn load_input_states_with_versions(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<PreparedRecoveryInputSnapshot, RuntimeStoreError> {
        self.inner.load_input_states_with_versions(runtime_id).await
    }

    async fn load_boundary_receipt(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<Option<RunBoundaryReceipt>, RuntimeStoreError> {
        self.inner
            .load_boundary_receipt(runtime_id, run_id, sequence)
            .await
    }

    async fn load_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<std::sync::Arc<Vec<u8>>>, RuntimeStoreError> {
        self.inner.load_session_snapshot(runtime_id).await
    }

    async fn load_pending_compaction_projections(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<meerkat_core::CompactionProjectionIntent>, RuntimeStoreError> {
        self.inner
            .load_pending_compaction_projections(runtime_id)
            .await
    }

    async fn mark_compaction_projection_finalized(
        &self,
        runtime_id: &LogicalRuntimeId,
        projection: &meerkat_core::CompactionProjectionId,
    ) -> Result<(), RuntimeStoreError> {
        self.inner
            .mark_compaction_projection_finalized(runtime_id, projection)
            .await
    }

    async fn clear_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        self.inner.clear_session_snapshot(runtime_id).await
    }

    async fn replace_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
        replacement: Vec<u8>,
    ) -> Result<bool, RuntimeStoreError> {
        self.inner
            .replace_session_snapshot_if_current(runtime_id, expected_current, replacement)
            .await
    }

    async fn clear_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        self.inner
            .clear_session_snapshot_if_current(runtime_id, expected_current)
            .await
    }

    async fn persist_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        state: &InputStatePersistenceRecord,
    ) -> Result<(), RuntimeStoreError> {
        if self.fail_persist_input_state.swap(false, Ordering::SeqCst) {
            return Err(RuntimeStoreError::WriteFailed(
                "synthetic persist_input_state failure".into(),
            ));
        }
        self.inner.persist_input_state(runtime_id, state).await
    }

    async fn persist_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        states: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError> {
        if self.fail_persist_input_state.swap(false, Ordering::SeqCst) {
            return Err(RuntimeStoreError::WriteFailed(
                "synthetic persist_input_state failure".into(),
            ));
        }
        self.inner
            .persist_input_states_atomically(runtime_id, states)
            .await
    }

    async fn compare_and_swap_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &[StoredInputState],
        replacements: &[InputStatePersistenceRecord],
    ) -> Result<InputStateBatchCasOutcome, RuntimeStoreError> {
        self.inner
            .compare_and_swap_input_states_atomically(runtime_id, expected, replacements)
            .await
    }

    async fn compare_and_swap_input_states_atomically_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &[StoredInputState],
        replacements: &[InputStatePersistenceRecord],
        write_fence: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<FencedInputStateBatchCasOutcome, RuntimeStoreError> {
        self.inner
            .compare_and_swap_input_states_atomically_with_fence(
                runtime_id,
                expected,
                replacements,
                write_fence,
            )
            .await
    }

    async fn compare_and_swap_recovery_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: RecoveryInputSetRevision,
        mutations: &[RecoveryInputStateMutation],
    ) -> Result<InputStateBatchCasOutcome, RuntimeStoreError> {
        self.inner
            .compare_and_swap_recovery_input_states_atomically(
                runtime_id,
                expected_revision,
                mutations,
            )
            .await
    }

    async fn compare_and_swap_recovery_input_states_atomically_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: RecoveryInputSetRevision,
        mutations: &[RecoveryInputStateMutation],
        write_fence: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<FencedInputStateBatchCasOutcome, RuntimeStoreError> {
        self.inner
            .compare_and_swap_recovery_input_states_atomically_with_fence(
                runtime_id,
                expected_revision,
                mutations,
                write_fence,
            )
            .await
    }

    async fn load_input_state_by_idempotency_key(
        &self,
        runtime_id: &LogicalRuntimeId,
        key: &meerkat_runtime::identifiers::IdempotencyKey,
    ) -> Result<Option<meerkat_runtime::store::ExactInputStateObservation>, RuntimeStoreError> {
        self.inner
            .load_input_state_by_idempotency_key(runtime_id, key)
            .await
    }

    async fn load_input_states_by_ids(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_ids: &[InputId],
    ) -> Result<Vec<Option<StoredInputState>>, RuntimeStoreError> {
        self.inner
            .load_input_states_by_ids(runtime_id, input_ids)
            .await
    }

    async fn load_pending_terminal_owner_ids_page(
        &self,
        runtime_id: &LogicalRuntimeId,
        after: Option<&InputId>,
        limit: usize,
    ) -> Result<Vec<InputId>, RuntimeStoreError> {
        self.inner
            .load_pending_terminal_owner_ids_page(runtime_id, after, limit)
            .await
    }

    async fn load_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, RuntimeStoreError> {
        self.inner.load_input_state(runtime_id, input_id).await
    }

    async fn load_machine_lifecycle_record(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        self.inner.load_machine_lifecycle_record(runtime_id).await
    }

    async fn commit_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        commit: meerkat_runtime::store::MachineLifecycleCommit,
        input_states: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError> {
        if self
            .fail_commit_machine_lifecycle
            .swap(false, Ordering::SeqCst)
        {
            return Err(RuntimeStoreError::WriteFailed(
                "synthetic commit_machine_lifecycle failure".into(),
            ));
        }
        self.inner
            .commit_machine_lifecycle(runtime_id, commit, input_states)
            .await
    }

    async fn commit_unregister_finalization(
        &self,
        runtime_id: &LogicalRuntimeId,
        finalization: meerkat_runtime::store::UnregisterFinalizationCommit,
    ) -> Result<(), RuntimeStoreError> {
        if self
            .fail_commit_machine_lifecycle
            .swap(false, Ordering::SeqCst)
        {
            return Err(RuntimeStoreError::WriteFailed(
                "synthetic commit_machine_lifecycle failure".into(),
            ));
        }
        self.inner
            .commit_unregister_finalization(runtime_id, finalization)
            .await
    }

    async fn persist_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        snapshot: &meerkat_runtime::PersistedOpsSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        self.inner.persist_ops_lifecycle(runtime_id, snapshot).await
    }

    async fn initialize_ops_lifecycle_if_absent(
        &self,
        runtime_id: &LogicalRuntimeId,
        candidate: &meerkat_runtime::PersistedOpsSnapshot,
    ) -> Result<meerkat_runtime::PersistedOpsSnapshot, RuntimeStoreError> {
        self.inner
            .initialize_ops_lifecycle_if_absent(runtime_id, candidate)
            .await
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
        injected_context: Vec::new(),
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
        content: text.into(),
        typed_turn_appends: Vec::new(),
        turn_metadata: None,
    })
}

fn make_multimodal_prompt(text: &str, label: &str) -> Input {
    Input::Prompt(PromptInput {
        injected_context: Vec::new(),
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
        content: meerkat_core::types::ContentInput::Blocks(vec![
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
        typed_turn_appends: Vec::new(),
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
    let states = store.load_input_states_strict(&rid).await.unwrap();
    assert_eq!(states.len(), 1);
}

#[tokio::test]
async fn recover_from_store() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let session_id = SessionId::new();
    let rid = LogicalRuntimeId::for_session(&session_id);

    // Pre-populate store with a state (simulating crash recovery)
    let input = make_prompt("hello");
    let input_id = input.id().clone();
    let mut state = InputState::new_accepted(input_id.clone());
    state.persisted_input = Some(input.clone());
    state.durability = Some(InputDurability::Durable);
    store
        .persist_input_state(&rid, &persistable(stored_accepted(state)))
        .await
        .unwrap();

    let adapter = MeerkatMachine::persistent(store as Arc<dyn RuntimeStore>, memory_blob_store());
    adapter
        .register_session(session_id.clone())
        .await
        .expect("registration-authorized recovery must adopt durable input work");

    let recovered = adapter
        .input_state(&session_id, &input_id)
        .await
        .unwrap()
        .expect("recovered input remains queryable");
    assert_eq!(
        recovered.seed.phase,
        meerkat_runtime::input_state::InputLifecycleState::Queued
    );
    assert_eq!(
        adapter.list_active_inputs(&session_id).await.unwrap(),
        vec![input_id],
        "recovered queued input must remain active for replay"
    );
}

#[tokio::test]
async fn recover_rebuilds_dedup_index() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let session_id = SessionId::new();
    let rid = LogicalRuntimeId::for_session(&session_id);
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
        .persist_input_state(&rid, &persistable(stored_accepted(state)))
        .await
        .unwrap();

    let adapter = MeerkatMachine::persistent(store as Arc<dyn RuntimeStore>, memory_blob_store());
    adapter
        .register_session(session_id.clone())
        .await
        .expect("registration-authorized recovery must rebuild the dedup index");

    // Now try to accept a new input with the same idempotency key
    let mut dup_input = make_prompt("duplicate");
    if let Input::Prompt(ref mut p) = dup_input {
        p.header.idempotency_key = Some(key);
    }
    let outcome = adapter.accept_input(&session_id, dup_input).await.unwrap();
    assert!(
        outcome.is_deduplicated(),
        "After recovery, dedup index should be rebuilt so duplicates are caught"
    );
}

#[tokio::test]
async fn recover_discards_machine_classified_ephemeral_inputs() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let session_id = SessionId::new();
    let rid = LogicalRuntimeId::for_session(&session_id);

    // Pre-populate with an ephemeral input state
    let mut input = make_prompt("ephemeral recovered input");
    if let Input::Prompt(ref mut prompt) = input {
        prompt.header.durability = InputDurability::Ephemeral;
    }
    let input_id = input.id().clone();
    let mut state = InputState::new_accepted(input_id.clone());
    state.persisted_input = Some(input);
    state.durability = Some(InputDurability::Ephemeral);
    store
        .persist_input_state(&rid, &persistable(stored_accepted(state)))
        .await
        .unwrap();

    let adapter = MeerkatMachine::persistent(store as Arc<dyn RuntimeStore>, memory_blob_store());
    adapter
        .register_session(session_id.clone())
        .await
        .expect("registration-authorized recovery must discard ephemeral input work");

    // Generated recovery durability authority discards ephemeral rows before
    // the ledger or queue projections can recover them.
    assert!(
        adapter
            .input_state(&session_id, &input_id)
            .await
            .unwrap()
            .is_none(),
        "Ephemeral inputs should be filtered during recovery"
    );
    assert!(
        adapter
            .list_active_inputs(&session_id)
            .await
            .unwrap()
            .is_empty()
    );
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
            let meerkat_core::types::ContentInput::Blocks(blocks) = prompt.content else {
                panic!("multimodal blocks should persist");
            };
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
        driver.contract_dequeue_next_for_recovery_tests().is_none(),
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
    let session_id = SessionId::new();
    let rid = LogicalRuntimeId::for_session(&session_id);
    let input = make_prompt("recover rollback");
    let input_id = input.id().clone();
    let mut state = InputState::new_accepted(input_id.clone());
    state.persisted_input = Some(input);
    state.durability = Some(InputDurability::Durable);
    inner
        .persist_input_state(&rid, &persistable(stored_accepted(state)))
        .await
        .unwrap();

    let store: Arc<dyn RuntimeStore> = Arc::new(
        FailPersistInputStore::fail_commit_machine_lifecycle_once(inner.clone()),
    );
    let adapter = MeerkatMachine::persistent(store, memory_blob_store());
    let err = adapter
        .register_session(session_id.clone())
        .await
        .expect_err("recovery lifecycle commit should fail");
    assert!(
        err.to_string()
            .contains("synthetic commit_machine_lifecycle failure"),
        "unexpected error: {err}",
    );
    assert!(
        !adapter.contains_session(&session_id).await,
        "failed registration recovery must not retain a live runtime entry",
    );
    let stored = inner
        .load_input_state(&rid, &input_id)
        .await
        .unwrap()
        .expect("durable recovery seed should remain");
    assert_eq!(
        stored.seed.phase,
        meerkat_runtime::input_state::InputLifecycleState::Queued,
        "failed recovery must not rewrite durable input lifecycle after generated persistence normalization",
    );
}

#[tokio::test]
async fn persistence_record_rejects_unstamped_recovered_row_before_store_write() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let rid = LogicalRuntimeId::new("test");

    let valid_input = make_prompt("valid recovered row");
    let valid_id = valid_input.id().clone();
    let mut valid_state = InputState::new_accepted(valid_id.clone());
    valid_state.persisted_input = Some(valid_input);
    valid_state.durability = Some(InputDurability::Durable);
    store
        .persist_input_state(&rid, &persistable(stored_accepted(valid_state)))
        .await
        .unwrap();

    let invalid_input = make_prompt("unstamped recovered row");
    let invalid_id = invalid_input.id().clone();
    let mut invalid_state = InputState::new_accepted(invalid_id.clone());
    invalid_state.persisted_input = Some(invalid_input);
    invalid_state.durability = Some(InputDurability::Durable);
    let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("unstamped-record"));
    let err = driver
        .recover_input_state_persistence_record(StoredInputState {
            state: invalid_state,
            seed: InputStateSeed::new_accepted(),
        })
        .expect_err("unstamped later row should fail before store write");

    assert!(
        err.to_string()
            .contains("missing recovered admission witness"),
        "unexpected error: {err}",
    );
    assert!(
        driver.input_state(&invalid_id).is_none(),
        "failed persistence-record recovery must not retain the rejected row",
    );
    assert!(
        driver.contract_dequeue_next_for_recovery_tests().is_none(),
        "failed persistence-record recovery must not leave recovered queue projection",
    );
    assert!(
        store
            .load_input_state(&rid, &valid_id)
            .await
            .unwrap()
            .is_some(),
        "valid generated-authority record should remain persisted",
    );
}

#[tokio::test]
async fn recover_consumes_committed_applied_pending_inputs() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;

    let store = Arc::new(InMemoryRuntimeStore::new());
    let session_id = SessionId::new();
    let rid = LogicalRuntimeId::for_session(&session_id);
    let input = make_prompt("already committed");
    let input_id = input.id().clone();
    let run_id = RunId::new();

    let mut state = InputState::new_accepted(input_id.clone());
    state.persisted_input = Some(input);
    state.durability = Some(InputDurability::Durable);
    // Simulate Accepted → Queued → Staged → Applied → AppliedPendingConsumption
    // by seeding the DSL-owned phase + run association alongside the shell.
    use meerkat_runtime::input_state::InputLifecycleState;
    stamp_runtime_semantics(&mut state);
    let stored = StoredInputState {
        state,
        seed: InputStateSeed {
            phase: InputLifecycleState::AppliedPendingConsumption,
            last_run_id: Some(run_id.clone()),
            last_boundary_sequence: Some(0),
            terminal_outcome: None,
            attempt_count: 1,
            admission_sequence: None,
            recovery_lane: Some(meerkat_core::types::HandlingMode::Queue),
        },
    };
    store
        .persist_input_state(&rid, &persistable(stored.clone()))
        .await
        .unwrap();
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
            vec![persistable(stored.clone())],
            None,
        )
        .await
        .unwrap();

    let adapter = MeerkatMachine::persistent(store as Arc<dyn RuntimeStore>, memory_blob_store());
    adapter
        .register_session(session_id.clone())
        .await
        .expect("registration-authorized recovery must adopt the committed input");
    let recovered = adapter
        .input_state(&session_id, &input_id)
        .await
        .unwrap()
        .expect("committed input should remain queryable after recovery");
    assert_eq!(recovered.seed.phase, InputLifecycleState::Consumed);
    assert!(
        adapter
            .list_active_inputs(&session_id)
            .await
            .unwrap()
            .is_empty(),
        "committed applied inputs should not stay active after recovery"
    );
}

#[tokio::test]
async fn driver_persistent_recovery_replaces_terminal_process_projection() {
    let inner = Arc::new(InMemoryRuntimeStore::new());
    let store = Arc::new(FailPersistInputStore::passthrough(inner));
    let (session_id, rid) = persist_destroyed_runtime_lifecycle(Arc::clone(&store)).await;

    let adapter =
        MeerkatMachine::persistent(store.clone() as Arc<dyn RuntimeStore>, memory_blob_store());
    adapter
        .register_session(session_id.clone())
        .await
        .expect("registration-authorized recovery must replace the dead process projection");

    assert_eq!(
        adapter.runtime_state(&session_id).await.unwrap(),
        RuntimeState::Idle,
        "cold recovery must not adopt a dead process phase as live authority",
    );
    assert_eq!(
        load_runtime_state(store.as_ref(), &rid).await.unwrap(),
        Some(RuntimeState::Idle),
        "the exact lifecycle CAS must publish a fresh unbound Idle shell",
    );
}

#[tokio::test]
async fn driver_persistent_recovery_normalizes_phase_and_recovers_durable_input_independently() {
    let inner = Arc::new(InMemoryRuntimeStore::new());
    let store = Arc::new(FailPersistInputStore::passthrough(inner.clone()));
    let (session_id, rid) = persist_destroyed_runtime_lifecycle(Arc::clone(&store)).await;
    let input = make_prompt("terminal projection conflict");
    let input_id = input.id().clone();
    let mut state = InputState::new_accepted(input_id.clone());
    state.persisted_input = Some(input);
    state.durability = Some(InputDurability::Durable);
    store
        .persist_input_state(&rid, &persistable(stored_accepted(state)))
        .await
        .unwrap();

    let adapter =
        MeerkatMachine::persistent(store.clone() as Arc<dyn RuntimeStore>, memory_blob_store());
    adapter
        .register_session(session_id.clone())
        .await
        .expect("registration-authorized recovery must preserve durable input work");
    assert_eq!(
        adapter.runtime_state(&session_id).await.unwrap(),
        RuntimeState::Idle,
        "cold recovery must not force destroyed state from the store projection",
    );
    assert!(
        adapter
            .input_state(&session_id, &input_id)
            .await
            .unwrap()
            .is_some(),
        "durable input work must survive lifecycle-shell normalization",
    );
    assert_eq!(
        load_runtime_state(store.as_ref(), &rid).await.unwrap(),
        Some(RuntimeState::Idle),
        "runtime lifecycle convergence is independent of later input-recovery refusal",
    );
}
