#![allow(
    clippy::expect_used,
    clippy::large_futures,
    clippy::panic,
    clippy::unwrap_used
)]

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use chrono::Utc;
use meerkat_core::BlobStore;
use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
use meerkat_core::lifecycle::{
    InputId, RunBoundaryReceipt, RunBoundaryReceiptDraft, RunId, run_primitive::RunApplyBoundary,
};
use meerkat_core::types::SessionId;
use meerkat_runtime::input_state::{
    InputAbandonReason, InputStatePersistenceRecord, InputTerminalOutcome, StoredInputState,
};
use meerkat_runtime::store::load_runtime_state;
use meerkat_runtime::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, LogicalRuntimeId,
    MeerkatMachine, PromptInput, RuntimeDriverError, RuntimeState, RuntimeStore, RuntimeStoreError,
    SerializedSessionSnapshot, SessionServiceRuntimeExt,
};
use meerkat_store::MemoryBlobStore;

fn memory_blob_store() -> Arc<dyn BlobStore> {
    Arc::new(MemoryBlobStore::new())
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

async fn wait_for_atomic_bool(flag: &AtomicBool, context: &'static str) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if flag.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect(context);
}

async fn wait_for_atomic_usize_at_least(
    value: &AtomicUsize,
    expected: usize,
    context: &'static str,
) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if value.load(Ordering::SeqCst) >= expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect(context);
}

async fn wait_for_input_state(
    adapter: &MeerkatMachine,
    sid: &SessionId,
    input_id: &InputId,
    context: &'static str,
    matches: impl Fn(&StoredInputState) -> bool,
) -> StoredInputState {
    wait_for_input_state_within(
        adapter,
        sid,
        input_id,
        Duration::from_secs(2),
        context,
        matches,
    )
    .await
}

/// [`wait_for_input_state`] with an explicit bound, for callers whose predicate
/// is the thing under test rather than incidental setup. Widening the bound can
/// only delay a pass: the predicate is a machine-owned state, so exceeding the
/// bound means the state was never reached, never that the test ran out of
/// patience for a state that would have been wrong anyway.
async fn wait_for_input_state_within(
    adapter: &MeerkatMachine,
    sid: &SessionId,
    input_id: &InputId,
    within: Duration,
    context: &'static str,
    matches: impl Fn(&StoredInputState) -> bool,
) -> StoredInputState {
    tokio::time::timeout(within, async {
        loop {
            if let Some(state) = adapter
                .input_state(sid, input_id)
                .await
                .expect("input state read should succeed")
                && matches(&state)
            {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect(context)
}

async fn wait_for_runtime_state(
    adapter: &MeerkatMachine,
    sid: &SessionId,
    expected: RuntimeState,
    context: &'static str,
) -> RuntimeState {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let state = adapter
                .runtime_state(sid)
                .await
                .expect("runtime state read should succeed");
            if state == expected {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect(context)
}

struct HarnessRuntimeStore {
    inner: meerkat_runtime::store::InMemoryRuntimeStore,
    fail_atomic_apply: AtomicBool,
    fail_recovery_read: AtomicBool,
    fail_durable_tail_recovery_source: AtomicBool,
    fail_commit_machine_lifecycle_now: AtomicBool,
    /// Fail commit_machine_lifecycle after N successful calls (None = never fail).
    fail_commit_machine_lifecycle_after: Option<usize>,
    /// Delay commit_machine_lifecycle after N successful calls (None = never delay).
    delay_commit_machine_lifecycle_after: Option<usize>,
    commit_machine_lifecycle_delay: Duration,
    commit_machine_lifecycle_calls: AtomicUsize,
    load_input_states_delay: Duration,
    fail_persist_input_state_after: Option<usize>,
    persist_input_state_calls: AtomicUsize,
    fail_lifecycle_load_for: Option<LogicalRuntimeId>,
}

impl HarnessRuntimeStore {
    fn new() -> Self {
        Self {
            inner: meerkat_runtime::store::InMemoryRuntimeStore::new(),
            fail_atomic_apply: AtomicBool::new(false),
            fail_recovery_read: AtomicBool::new(false),
            fail_durable_tail_recovery_source: AtomicBool::new(false),
            fail_commit_machine_lifecycle_now: AtomicBool::new(false),
            fail_commit_machine_lifecycle_after: None,
            delay_commit_machine_lifecycle_after: None,
            commit_machine_lifecycle_delay: Duration::ZERO,
            commit_machine_lifecycle_calls: AtomicUsize::new(0),
            load_input_states_delay: Duration::ZERO,
            fail_persist_input_state_after: None,
            persist_input_state_calls: AtomicUsize::new(0),
            fail_lifecycle_load_for: None,
        }
    }

    fn failing_atomic_apply() -> Self {
        Self {
            fail_atomic_apply: AtomicBool::new(true),
            ..Self::new()
        }
    }

    fn set_fail_atomic_apply(&self, fail: bool) {
        self.fail_atomic_apply.store(fail, Ordering::SeqCst);
    }

    fn set_fail_recovery_read(&self, fail: bool) {
        self.fail_recovery_read.store(fail, Ordering::SeqCst);
    }

    fn set_fail_durable_tail_recovery_source(&self, fail: bool) {
        self.fail_durable_tail_recovery_source
            .store(fail, Ordering::SeqCst);
    }

    fn delayed_recover(delay: Duration) -> Self {
        Self {
            load_input_states_delay: delay,
            ..Self::new()
        }
    }

    fn fail_lifecycle_load_for(runtime_id: LogicalRuntimeId) -> Self {
        Self {
            fail_lifecycle_load_for: Some(runtime_id),
            ..Self::new()
        }
    }

    fn failing_lifecycle_commit() -> Self {
        Self::failing_lifecycle_commit_after(1)
    }

    fn failing_lifecycle_commit_after(successful_calls: usize) -> Self {
        Self {
            fail_commit_machine_lifecycle_after: Some(successful_calls),
            ..Self::new()
        }
    }

    fn failing_terminal_snapshot() -> Self {
        Self {
            // Recovery calls commit_machine_lifecycle once (call 0 succeeds),
            // the terminal event call (call 1) fails.
            fail_commit_machine_lifecycle_after: Some(1),
            ..Self::new()
        }
    }

    fn delayed_terminal_lifecycle_commit(delay: Duration) -> Self {
        Self {
            delay_commit_machine_lifecycle_after: Some(1),
            commit_machine_lifecycle_delay: delay,
            ..Self::new()
        }
    }

    fn commit_machine_lifecycle_calls(&self) -> usize {
        self.commit_machine_lifecycle_calls.load(Ordering::SeqCst)
    }

    fn set_fail_commit_machine_lifecycle_now(&self, fail: bool) {
        self.fail_commit_machine_lifecycle_now
            .store(fail, Ordering::SeqCst);
    }

    async fn before_machine_lifecycle_commit(&self) -> Result<(), RuntimeStoreError> {
        let call_index = self
            .commit_machine_lifecycle_calls
            .fetch_add(1, Ordering::SeqCst);
        if self
            .fail_commit_machine_lifecycle_now
            .load(Ordering::SeqCst)
        {
            return Err(RuntimeStoreError::WriteFailed(
                "synthetic commit_machine_lifecycle failure".to_string(),
            ));
        }
        if self
            .fail_commit_machine_lifecycle_after
            .is_some_and(|fail_after| call_index >= fail_after)
        {
            return Err(RuntimeStoreError::WriteFailed(
                "synthetic commit_machine_lifecycle failure".to_string(),
            ));
        }
        if self
            .delay_commit_machine_lifecycle_after
            .is_some_and(|delay_after| call_index >= delay_after)
            && !self.commit_machine_lifecycle_delay.is_zero()
        {
            tokio::time::sleep(self.commit_machine_lifecycle_delay).await;
        }
        Ok(())
    }
}

struct CountingArchiveHook {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl meerkat_runtime::MachineSessionArchivePostCommitHook for CountingArchiveHook {
    async fn after_runtime_retire_commit(
        &self,
    ) -> Result<(), meerkat_runtime::RuntimeControlPlaneError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct GatedArchiveHook {
    attempts: AtomicUsize,
    completions: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl meerkat_runtime::MachineSessionArchivePostCommitHook for GatedArchiveHook {
    async fn after_runtime_retire_commit(
        &self,
    ) -> Result<(), meerkat_runtime::RuntimeControlPlaneError> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            self.entered.notify_waiters();
            self.release.notified().await;
        }
        self.completions.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[async_trait::async_trait]
impl RuntimeStore for HarnessRuntimeStore {
    fn session_authority_ops(&self) -> &dyn meerkat_runtime::store::RuntimeSessionAuthorityOps {
        self.inner.session_authority_ops()
    }

    fn session_persistence_profile(
        &self,
    ) -> meerkat_runtime::store::RuntimeSessionPersistenceProfile {
        if self
            .fail_durable_tail_recovery_source
            .load(Ordering::SeqCst)
        {
            meerkat_runtime::store::RuntimeSessionPersistenceProfile::HeadCanonicalV1
        } else {
            RuntimeStore::session_persistence_profile(&self.inner)
        }
    }

    async fn load_durable_tail_recovery_source(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<meerkat_runtime::store::PreparedDurableTailRecoverySource>, RuntimeStoreError>
    {
        if self
            .fail_durable_tail_recovery_source
            .load(Ordering::SeqCst)
        {
            return Err(RuntimeStoreError::ReadFailed(
                "synthetic durable-tail recovery source failure".to_string(),
            ));
        }
        self.inner
            .load_durable_tail_recovery_source(runtime_id)
            .await
    }

    fn supports_compaction_projection_outbox(&self) -> bool {
        self.inner.supports_compaction_projection_outbox()
    }

    fn input_state_batch_cas_implementation_profile(
        &self,
    ) -> meerkat_runtime::store::InputStateBatchCasImplementationProfile {
        self.inner.input_state_batch_cas_implementation_profile()
    }

    async fn commit_prepared_session_boundary(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        request: meerkat_runtime::store::PreparedRuntimeSessionCommit,
    ) -> Result<meerkat_runtime::store::PreparedRuntimeSessionCommitResult, RuntimeStoreError> {
        use meerkat_runtime::store::PreparedRuntimeSessionCommitKind;

        if self.fail_atomic_apply.load(Ordering::SeqCst) {
            return Err(RuntimeStoreError::WriteFailed(
                "synthetic atomic service-turn commit failure".to_string(),
            ));
        }
        if request.kind() == PreparedRuntimeSessionCommitKind::ServiceTurnTerminal
            && self
                .fail_commit_machine_lifecycle_now
                .load(Ordering::SeqCst)
        {
            return Err(RuntimeStoreError::WriteFailed(
                "synthetic atomic service-turn commit failure".to_string(),
            ));
        }
        if matches!(
            request.kind(),
            PreparedRuntimeSessionCommitKind::MachineTerminal
                | PreparedRuntimeSessionCommitKind::Recovery
        ) {
            self.before_machine_lifecycle_commit().await?;
        }
        self.inner
            .commit_prepared_session_boundary(runtime_id, request)
            .await
    }

    async fn observe_machine_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
    ) -> Result<meerkat_runtime::store::MachineLifecycleObservation, RuntimeStoreError> {
        self.inner.observe_machine_lifecycle(runtime_id).await
    }

    async fn compare_and_swap_machine_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        expected: meerkat_runtime::store::MachineLifecycleExpectedVersion,
        replacement: meerkat_runtime::store::MachineLifecycleCommit,
    ) -> Result<meerkat_runtime::store::MachineLifecycleCasOutcome, RuntimeStoreError> {
        self.before_machine_lifecycle_commit().await?;
        self.inner
            .compare_and_swap_machine_lifecycle(runtime_id, expected, replacement)
            .await
    }

    async fn commit_session_snapshot(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        session_delta: SerializedSessionSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        self.inner
            .commit_session_snapshot(runtime_id, session_delta)
            .await
    }

    async fn commit_prepared_whole_blob_rewrite_boundary(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        boundary: meerkat_runtime::store::PreparedWholeBlobRewriteStoreParts,
    ) -> Result<meerkat_runtime::store::WholeBlobStoreAuthority, RuntimeStoreError> {
        self.inner
            .commit_prepared_whole_blob_rewrite_boundary(runtime_id, boundary)
            .await
    }

    async fn atomic_apply(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        session_delta: Option<SerializedSessionSnapshot>,
        receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: Option<meerkat_core::types::SessionId>,
    ) -> Result<(), RuntimeStoreError> {
        if self.fail_atomic_apply.load(Ordering::SeqCst) {
            return Err(RuntimeStoreError::WriteFailed(
                "synthetic atomic_apply failure".to_string(),
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

    async fn atomic_apply_with_machine_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        session_delta: SerializedSessionSnapshot,
        receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
        machine_lifecycle: meerkat_runtime::store::MachineLifecycleCommit,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Result<(), RuntimeStoreError> {
        if self.fail_atomic_apply.load(Ordering::SeqCst)
            || self
                .fail_commit_machine_lifecycle_now
                .load(Ordering::SeqCst)
        {
            return Err(RuntimeStoreError::WriteFailed(
                "synthetic atomic service-turn commit failure".to_string(),
            ));
        }
        self.inner
            .atomic_apply_with_machine_lifecycle(
                runtime_id,
                session_delta,
                receipt,
                machine_lifecycle,
                input_updates,
                session_store_key,
            )
            .await
    }

    async fn load_input_states(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
    ) -> Result<Vec<meerkat_runtime::InputStateRow>, RuntimeStoreError> {
        if !self.load_input_states_delay.is_zero() {
            tokio::time::sleep(self.load_input_states_delay).await;
        }
        self.inner.load_input_states(runtime_id).await
    }

    async fn load_input_states_with_versions(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
    ) -> Result<meerkat_runtime::store::PreparedRecoveryInputSnapshot, RuntimeStoreError> {
        if self.fail_recovery_read.load(Ordering::SeqCst) {
            return Err(RuntimeStoreError::ReadFailed(
                "synthetic cold recovery read failure".to_string(),
            ));
        }
        self.inner.load_input_states_with_versions(runtime_id).await
    }

    async fn load_boundary_receipt(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<Option<meerkat_core::lifecycle::RunBoundaryReceipt>, RuntimeStoreError> {
        self.inner
            .load_boundary_receipt(runtime_id, run_id, sequence)
            .await
    }

    async fn load_session_snapshot(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
    ) -> Result<Option<std::sync::Arc<Vec<u8>>>, RuntimeStoreError> {
        self.inner.load_session_snapshot(runtime_id).await
    }

    async fn load_pending_compaction_projections(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
    ) -> Result<Vec<meerkat_core::CompactionProjectionIntent>, RuntimeStoreError> {
        self.inner
            .load_pending_compaction_projections(runtime_id)
            .await
    }

    async fn mark_compaction_projection_finalized(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        projection: &meerkat_core::CompactionProjectionId,
    ) -> Result<(), RuntimeStoreError> {
        self.inner
            .mark_compaction_projection_finalized(runtime_id, projection)
            .await
    }

    async fn clear_session_snapshot(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        self.inner.clear_session_snapshot(runtime_id).await
    }

    async fn replace_session_snapshot_if_current(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        expected_current: &[u8],
        replacement: Vec<u8>,
    ) -> Result<bool, RuntimeStoreError> {
        self.inner
            .replace_session_snapshot_if_current(runtime_id, expected_current, replacement)
            .await
    }

    async fn clear_session_snapshot_if_current(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        expected_current: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        self.inner
            .clear_session_snapshot_if_current(runtime_id, expected_current)
            .await
    }

    async fn persist_input_state(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        state: &InputStatePersistenceRecord,
    ) -> Result<(), RuntimeStoreError> {
        let call_index = self
            .persist_input_state_calls
            .fetch_add(1, Ordering::SeqCst);
        if self
            .fail_persist_input_state_after
            .is_some_and(|fail_after| call_index >= fail_after)
        {
            return Err(RuntimeStoreError::WriteFailed(
                "synthetic persist_input_state failure".to_string(),
            ));
        }
        self.inner.persist_input_state(runtime_id, state).await
    }

    async fn persist_input_states_atomically(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        states: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError> {
        self.inner
            .persist_input_states_atomically(runtime_id, states)
            .await
    }

    async fn compare_and_swap_input_states_atomically(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        expected: &[StoredInputState],
        replacements: &[InputStatePersistenceRecord],
    ) -> Result<meerkat_runtime::store::InputStateBatchCasOutcome, RuntimeStoreError> {
        self.inner
            .compare_and_swap_input_states_atomically(runtime_id, expected, replacements)
            .await
    }

    async fn compare_and_swap_input_states_atomically_with_fence(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        expected: &[StoredInputState],
        replacements: &[InputStatePersistenceRecord],
        write_fence: Arc<dyn meerkat_runtime::store::RuntimeStoreWriteFence>,
    ) -> Result<meerkat_runtime::store::FencedInputStateBatchCasOutcome, RuntimeStoreError> {
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
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        expected_revision: meerkat_runtime::store::RecoveryInputSetRevision,
        mutations: &[meerkat_runtime::store::RecoveryInputStateMutation],
    ) -> Result<meerkat_runtime::store::InputStateBatchCasOutcome, RuntimeStoreError> {
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
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        expected_revision: meerkat_runtime::store::RecoveryInputSetRevision,
        mutations: &[meerkat_runtime::store::RecoveryInputStateMutation],
        write_fence: Arc<dyn meerkat_runtime::store::RuntimeStoreWriteFence>,
    ) -> Result<meerkat_runtime::store::FencedInputStateBatchCasOutcome, RuntimeStoreError> {
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
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        key: &meerkat_runtime::identifiers::IdempotencyKey,
    ) -> Result<Option<meerkat_runtime::store::ExactInputStateObservation>, RuntimeStoreError> {
        self.inner
            .load_input_state_by_idempotency_key(runtime_id, key)
            .await
    }

    async fn load_input_states_by_ids(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        input_ids: &[InputId],
    ) -> Result<Vec<Option<StoredInputState>>, RuntimeStoreError> {
        self.inner
            .load_input_states_by_ids(runtime_id, input_ids)
            .await
    }

    async fn load_pending_terminal_owner_ids_page(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        after: Option<&InputId>,
        limit: usize,
    ) -> Result<Vec<InputId>, RuntimeStoreError> {
        self.inner
            .load_pending_terminal_owner_ids_page(runtime_id, after, limit)
            .await
    }

    async fn load_input_state(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, RuntimeStoreError> {
        self.inner.load_input_state(runtime_id, input_id).await
    }

    async fn load_machine_lifecycle_record(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        if self.fail_lifecycle_load_for.as_ref() == Some(runtime_id) {
            return Err(RuntimeStoreError::ReadFailed(
                "synthetic legacy lifecycle load failure".to_string(),
            ));
        }
        self.inner.load_machine_lifecycle_record(runtime_id).await
    }

    async fn commit_machine_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        commit: meerkat_runtime::store::MachineLifecycleCommit,
        input_states: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError> {
        self.before_machine_lifecycle_commit().await?;
        self.inner
            .commit_machine_lifecycle(runtime_id, commit, input_states)
            .await
    }

    async fn commit_unregister_finalization(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        finalization: meerkat_runtime::store::UnregisterFinalizationCommit,
    ) -> Result<(), RuntimeStoreError> {
        self.before_machine_lifecycle_commit().await?;
        self.inner
            .commit_unregister_finalization(runtime_id, finalization)
            .await
    }

    async fn persist_ops_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        snapshot: &meerkat_runtime::PersistedOpsSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        self.inner.persist_ops_lifecycle(runtime_id, snapshot).await
    }

    async fn initialize_ops_lifecycle_if_absent(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        candidate: &meerkat_runtime::PersistedOpsSnapshot,
    ) -> Result<meerkat_runtime::PersistedOpsSnapshot, RuntimeStoreError> {
        self.inner
            .initialize_ops_lifecycle_if_absent(runtime_id, candidate)
            .await
    }

    async fn load_ops_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
    ) -> Result<Option<meerkat_runtime::PersistedOpsSnapshot>, RuntimeStoreError> {
        self.inner.load_ops_lifecycle(runtime_id).await
    }
}

#[tokio::test]
async fn ephemeral_adapter_accept_and_query() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");

    let input = make_prompt("hello");
    let outcome = adapter.accept_input(&sid, input).await.unwrap();
    assert!(outcome.is_accepted());

    let state = adapter.runtime_state(&sid).await.unwrap();
    assert_eq!(state, RuntimeState::Idle);

    let active = adapter.list_active_inputs(&sid).await.unwrap();
    assert_eq!(active.len(), 1);
}

#[tokio::test]
async fn accept_input_without_wake_keeps_idle_runtime_idle() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");

    let input = make_prompt("queued-only");
    let input_id = input.header().id.clone();
    let outcome = adapter
        .accept_input_without_wake(&sid, input)
        .await
        .unwrap();
    assert!(outcome.is_accepted());

    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Idle,
        "queue-only admission must not wake an idle runtime"
    );
    assert_eq!(
        adapter.list_active_inputs(&sid).await.unwrap(),
        vec![input_id],
        "queue-only admission should still stage the input for later processing"
    );
}

#[tokio::test]
async fn persistent_adapter_accept() {
    let store = Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent(store, memory_blob_store()));
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");

    let input = make_prompt("hello");
    let outcome = adapter.accept_input(&sid, input).await.unwrap();
    assert!(outcome.is_accepted());
}

#[tokio::test]
async fn lifecycle_commit_failure_restores_pre_retire_authority() {
    let store = Arc::new(HarnessRuntimeStore::failing_lifecycle_commit());
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");
    let runtime_id = LogicalRuntimeId::for_session(&sid);

    let input = make_prompt("retire rollback");
    let input_id = input.id().clone();
    adapter
        .accept_input_without_wake(&sid, input)
        .await
        .expect("input admission should succeed before lifecycle failure");

    let err = meerkat_runtime::traits::RuntimeControlPlane::retire(&*adapter, &runtime_id)
        .await
        .expect_err("retire should surface lifecycle commit failure");
    assert!(
        err.to_string()
            .contains("synthetic commit_machine_lifecycle failure"),
        "unexpected error: {err}",
    );
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Idle,
        "failed retire persistence must restore the pre-retire generated authority",
    );
    assert_eq!(
        adapter.list_active_inputs(&sid).await.unwrap(),
        vec![input_id],
        "failed retire must not abandon active input projection without generated input lifecycle authority",
    );
    assert_ne!(
        load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .unwrap(),
        Some(RuntimeState::Retired),
        "failed retire must not claim durable retired truth when persistence rejected it",
    );
}

#[tokio::test]
async fn archive_post_commit_hook_does_not_run_before_failed_retire_commit() {
    let store = Arc::new(HarnessRuntimeStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register hook failure fixture");
    let hook = Arc::new(CountingArchiveHook {
        calls: AtomicUsize::new(0),
    });
    store.set_fail_commit_machine_lifecycle_now(true);
    let failed_lease = adapter
        .prepare_session_archive_lease(&sid)
        .await
        .expect("prepare failed retire lease")
        .expect("registered runtime has archive lease");
    let error = adapter
        .retire_session_with_archive_lease_and_post_commit_hook_before(
            failed_lease,
            hook.clone(),
            meerkat_core::time_compat::Instant::now() + Duration::from_secs(2),
        )
        .await
        .expect_err("durable retire commit failure must surface");
    assert!(
        error
            .to_string()
            .contains("synthetic commit_machine_lifecycle failure")
    );
    assert_eq!(hook.calls.load(Ordering::SeqCst), 0);

    store.set_fail_commit_machine_lifecycle_now(false);
    let retry_adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    retry_adapter
        .register_session(sid.clone())
        .await
        .expect("cold retry reconstructs failed retire registration");
    let retry_lease = retry_adapter
        .prepare_session_archive_lease(&sid)
        .await
        .expect("prepare retry lease")
        .expect("failed commit retains registration");
    retry_adapter
        .retire_session_with_archive_lease_and_post_commit_hook_before(
            retry_lease,
            hook.clone(),
            meerkat_core::time_compat::Instant::now() + Duration::from_secs(2),
        )
        .await
        .expect("retry commits then invokes exact hook");
    assert_eq!(hook.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancelled_archive_post_commit_hook_retries_after_durable_retired() {
    let store = Arc::new(HarnessRuntimeStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    let runtime_id = LogicalRuntimeId::for_session(&sid);
    adapter
        .register_session(sid.clone())
        .await
        .expect("register cancellation fixture");
    let hook = Arc::new(GatedArchiveHook {
        attempts: AtomicUsize::new(0),
        completions: AtomicUsize::new(0),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let lease = adapter
        .prepare_session_archive_lease(&sid)
        .await
        .expect("prepare gated lease")
        .expect("registered runtime has archive lease");
    let task_adapter = Arc::clone(&adapter);
    let task_hook = Arc::clone(&hook);
    let task = tokio::spawn(async move {
        task_adapter
            .retire_session_with_archive_lease_and_post_commit_hook_before(
                lease,
                task_hook,
                meerkat_core::time_compat::Instant::now() + Duration::from_secs(2),
            )
            .await
    });
    hook.entered.notified().await;
    assert_eq!(
        load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .unwrap(),
        Some(RuntimeState::Retired),
        "hook must begin only after durable Retired"
    );
    task.abort();
    let _ = task.await;
    assert_eq!(hook.completions.load(Ordering::SeqCst), 0);
    let retry_lease = adapter
        .prepare_session_archive_lease(&sid)
        .await
        .expect("prepare retry after cancellation")
        .expect("cancelled hook retains registration");
    adapter
        .retire_session_with_archive_lease_and_post_commit_hook_before(
            retry_lease,
            hook.clone(),
            meerkat_core::time_compat::Instant::now() + Duration::from_secs(2),
        )
        .await
        .expect("Retired retry invokes outstanding hook");
    assert_eq!(hook.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(hook.completions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn destroy_lifecycle_commit_failure_leaves_live_projection_repair_blocked() {
    let store = Arc::new(HarnessRuntimeStore::failing_lifecycle_commit());
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");
    let runtime_id = LogicalRuntimeId::for_session(&sid);

    let input = make_prompt("destroy rollback");
    let input_id = input.id().clone();
    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, input)
        .await
        .expect("input admission should succeed before lifecycle failure");
    assert!(outcome.is_accepted());
    let handle = handle.expect("accepted input should produce a completion handle");

    let err = meerkat_runtime::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect_err("destroy should surface lifecycle commit failure");
    assert!(
        err.to_string()
            .contains("synthetic commit_machine_lifecycle failure"),
        "unexpected error: {err}",
    );
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Destroyed,
        "an uncertain destroy commit must leave the staged terminal projection fail-closed",
    );
    assert!(
        adapter.list_active_inputs(&sid).await.unwrap().is_empty(),
        "repair-blocked destroy must not republish pre-destroy input work as live authority",
    );
    assert_ne!(
        load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .unwrap(),
        Some(RuntimeState::Destroyed),
        "failed destroy must not persist destroyed runtime truth",
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), handle.wait())
            .await
            .is_err(),
        "an uncertain destroy must not claim a durable completion outcome",
    );
    let terminal_input = adapter
        .input_state(&sid, &input_id)
        .await
        .unwrap()
        .expect("repair-blocked projection should retain the staged terminal row");
    assert_eq!(
        terminal_input.seed.phase,
        meerkat_runtime::InputLifecycleState::Abandoned,
        "repair-blocked destroy must expose the input only as its staged terminal projection"
    );
}

#[tokio::test]
async fn service_turn_terminal_atomic_commit_failure_rolls_back_lifecycle_publication() {
    let store = Arc::new(HarnessRuntimeStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    let runtime_id = LogicalRuntimeId::for_session(&sid);
    let bindings = adapter
        .prepare_bindings(sid.clone())
        .await
        .expect("prepare runtime bindings");
    let turn_identity = adapter
        .capture_service_turn_identity(&sid)
        .await
        .expect("capture exact service-turn runtime identity");
    let run_id = RunId::new();
    bindings
        .turn_state()
        .start_immediate_append(run_id.clone())
        .expect("start service turn through runtime handle");
    bindings
        .turn_state()
        .primitive_applied(run_id.clone())
        .expect("service turn must reach a generated terminal before commit");
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Running,
        "service turn should put the machine in a running lifecycle before terminal commit"
    );
    let snapshot_before = store.load_session_snapshot(&runtime_id).await.unwrap();

    store.set_fail_commit_machine_lifecycle_now(true);
    let session_commit =
        BoundSessionCommit::sealed(Arc::new(meerkat_core::Session::with_id(sid.clone()))).unwrap();
    let mut commit_lease = adapter
        .prepare_service_turn_commit_lease(&turn_identity)
        .await
        .expect("acquire exact service-turn commit lease");
    let err = adapter
        .commit_service_turn_terminal_receipt_with_lease(&mut commit_lease, session_commit)
        .await
        .expect_err("atomic terminal commit failure should surface");
    assert!(
        err.to_string()
            .contains("synthetic atomic service-turn commit failure"),
        "unexpected error: {err}",
    );

    let snapshot = bindings.turn_state().snapshot();
    assert_eq!(
        snapshot.active_run_id, None,
        "terminal turn snapshots hide the live binding behind terminal_run_id"
    );
    assert_eq!(
        snapshot.turn_phase,
        meerkat_core::turn_execution_authority::TurnPhase::Completed,
        "failed durable commit must preserve the generated terminal that predated it"
    );
    assert_eq!(
        snapshot.terminal_outcome,
        Some(meerkat_core::TurnTerminalOutcome::Completed)
    );
    assert_eq!(snapshot.terminal_run_id.as_ref(), Some(&run_id));
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Attached,
        "the generated terminal predates persistence and returns the live executor to Attached"
    );
    assert!(
        store
            .load_boundary_receipt(&runtime_id, &run_id, 1)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.load_session_snapshot(&runtime_id).await.unwrap(),
        snapshot_before
    );
}

#[tokio::test]
async fn service_turn_commit_rejects_nonterminal_generated_state_before_publication() {
    let store = Arc::new(HarnessRuntimeStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    let runtime_id = LogicalRuntimeId::for_session(&sid);
    let bindings = adapter
        .prepare_bindings(sid.clone())
        .await
        .expect("prepare runtime bindings");
    let turn_identity = adapter
        .capture_service_turn_identity(&sid)
        .await
        .expect("capture exact service-turn runtime identity");
    let run_id = RunId::new();
    bindings
        .turn_state()
        .start_immediate_append(run_id.clone())
        .expect("start nonterminal direct service turn");
    let snapshot_before = store.load_session_snapshot(&runtime_id).await.unwrap();

    let session_commit =
        BoundSessionCommit::sealed(Arc::new(meerkat_core::Session::with_id(sid.clone()))).unwrap();
    let mut commit_lease = adapter
        .prepare_service_turn_commit_lease(&turn_identity)
        .await
        .expect("acquire exact service-turn commit lease");
    let error = adapter
        .commit_service_turn_terminal_receipt_with_lease(&mut commit_lease, session_commit)
        .await
        .expect_err("service commit must not manufacture completion");
    assert!(
        error.to_string().contains("generated terminal"),
        "unexpected nonterminal rejection: {error}"
    );
    let turn = bindings.turn_state().snapshot();
    assert_eq!(turn.active_run_id.as_ref(), Some(&run_id));
    assert_eq!(turn.terminal_run_id, None);
    assert_eq!(
        turn.turn_phase,
        meerkat_core::turn_execution_authority::TurnPhase::ApplyingPrimitive
    );
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Running
    );
    assert!(
        store
            .load_boundary_receipt(&runtime_id, &run_id, 1)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.load_session_snapshot(&runtime_id).await.unwrap(),
        snapshot_before
    );
}

#[tokio::test]
async fn failed_service_turn_atomically_commits_snapshot_receipt_and_lifecycle() {
    let store = Arc::new(HarnessRuntimeStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    let runtime_id = LogicalRuntimeId::for_session(&sid);
    let bindings = adapter
        .prepare_bindings(sid.clone())
        .await
        .expect("prepare runtime bindings");
    let turn_identity = adapter
        .capture_service_turn_identity(&sid)
        .await
        .expect("capture exact service-turn runtime identity");
    let run_id = RunId::new();
    bindings
        .turn_state()
        .start_immediate_append(run_id.clone())
        .expect("start direct failed service turn");
    bindings
        .turn_state()
        .fatal_failure(
            run_id.clone(),
            meerkat_core::turn_execution_authority::TurnFailureSource::new(
                meerkat_core::turn_execution_authority::TurnFailureSourceKind::ToolError,
                "direct service tool failure",
            ),
        )
        .expect("terminalize direct service turn through generated authority");

    let mut session = meerkat_core::Session::with_id(sid.clone());
    session.push(meerkat_core::types::Message::User(
        meerkat_core::types::UserMessage::text("failed service transcript"),
    ));
    let session = Arc::new(session);
    let session_commit = BoundSessionCommit::sealed(Arc::clone(&session)).unwrap();
    let session_snapshot = session_commit
        .whole_blob_artifact()
        .expect("materialize one exact whole-blob artifact")
        .bytes_arc();
    let mut commit_lease = adapter
        .prepare_service_turn_commit_lease(&turn_identity)
        .await
        .expect("acquire exact service-turn commit lease");
    adapter
        .commit_service_turn_terminal_receipt_with_lease(&mut commit_lease, session_commit)
        .await
        .expect("failed service turn should commit through one atomic runtime transaction");

    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Attached,
        "failed service turn must return to its generated pre-run lifecycle"
    );
    let terminal = bindings.turn_state().snapshot();
    assert_eq!(
        terminal.turn_phase,
        meerkat_core::turn_execution_authority::TurnPhase::Failed
    );
    assert_eq!(terminal.active_run_id, None);
    assert_eq!(terminal.terminal_run_id, Some(run_id.clone()));
    assert_eq!(
        store.load_session_snapshot(&runtime_id).await.unwrap(),
        Some(session_snapshot)
    );
    let receipt = store
        .load_boundary_receipt(&runtime_id, &run_id, 1)
        .await
        .unwrap()
        .expect("failed service turn receipt must be durable");
    assert_eq!(receipt.boundary, RunApplyBoundary::Immediate);
    assert!(receipt.contributing_input_ids.is_empty());
    assert_eq!(receipt.message_count, session.messages().len());
    assert_eq!(
        receipt.conversation_digest,
        Some(
            session
                .transcript_content_digest()
                .expect("digest session messages")
        ),
        "receipt must carry the canonical accumulator digest of the committed session"
    );
    assert!(
        store
            .load_input_states_strict(&runtime_id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .unwrap(),
        Some(RuntimeState::Idle),
        "live Attached is machine-classified as durable Idle"
    );
}

#[tokio::test]
async fn destroy_does_not_publish_destroyed_while_lifecycle_commit_is_in_flight() {
    let store = Arc::new(HarnessRuntimeStore::delayed_terminal_lifecycle_commit(
        Duration::from_millis(250),
    ));
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");
    let runtime_id = LogicalRuntimeId::for_session(&sid);

    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, make_prompt("destroy in flight"))
        .await
        .expect("input admission should succeed before delayed destroy");
    assert!(outcome.is_accepted());
    let handle = handle.expect("accepted input should produce a completion handle");

    let baseline_commits = store.commit_machine_lifecycle_calls();
    let destroy_adapter = Arc::clone(&adapter);
    let destroy_runtime_id = runtime_id.clone();
    let destroy_task = tokio::spawn(async move {
        meerkat_runtime::traits::RuntimeControlPlane::destroy(
            &*destroy_adapter,
            &destroy_runtime_id,
        )
        .await
    });

    wait_for_atomic_usize_at_least(
        &store.commit_machine_lifecycle_calls,
        baseline_commits + 1,
        "destroy lifecycle commit should enter delayed store call",
    )
    .await;

    assert_ne!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Destroyed,
        "in-flight durable destroy commit must not publish visible Destroyed state"
    );
    assert_ne!(
        load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .unwrap(),
        Some(RuntimeState::Destroyed),
        "in-flight durable destroy commit must not publish durable Destroyed state"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), handle.wait())
            .await
            .is_err(),
        "destroy must not terminate completion waiters before durable commit"
    );

    destroy_task
        .await
        .expect("destroy task should not panic")
        .expect("delayed destroy should eventually commit");
    wait_for_runtime_state(
        &adapter,
        &sid,
        RuntimeState::Destroyed,
        "destroy should publish after durable commit",
    )
    .await;
}

#[tokio::test]
async fn async_stop_lifecycle_commit_failure_does_not_publish_stopped() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::RunPrimitive;

    struct StopRecordingExecutor {
        stop_called: Arc<AtomicBool>,
        cleanup_called: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for StopRecordingExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Err(CoreExecutorError::apply_failed_runtime_turn(
                "unexpected apply during stop regression",
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            self.stop_called.store(true, Ordering::SeqCst);
            Ok(())
        }

        async fn cleanup_after_runtime_stop_terminalized(
            &mut self,
        ) -> Result<(), CoreExecutorError> {
            self.cleanup_called.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    let store = Arc::new(HarnessRuntimeStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    let stop_called = Arc::new(AtomicBool::new(false));
    let cleanup_called = Arc::new(AtomicBool::new(false));
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(StopRecordingExecutor {
                stop_called: Arc::clone(&stop_called),
                cleanup_called: Arc::clone(&cleanup_called),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");
    store.set_fail_commit_machine_lifecycle_now(true);

    let stop_error = adapter
        .stop_runtime_executor(&sid, "async stop lifecycle failure")
        .await
        .expect_err("durable stop failure must reach the exact stop acknowledgement");
    assert!(
        stop_error
            .to_string()
            .contains("synthetic commit_machine_lifecycle failure"),
        "stop acknowledgement must retain the durable commit failure: {stop_error}"
    );
    wait_for_atomic_bool(&stop_called, "stop effect should reach executor").await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(
        !cleanup_called.load(Ordering::SeqCst),
        "post-stop cleanup must not run when durable stop terminalization fails"
    );
    assert!(
        adapter.contains_session(&sid).await,
        "failed durable stop terminalization must not unregister the session"
    );
    assert_ne!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Stopped,
        "failed durable stop commit must not publish visible Stopped state"
    );
    assert_ne!(
        load_runtime_state(store.as_ref(), &LogicalRuntimeId::for_session(&sid))
            .await
            .unwrap(),
        Some(RuntimeState::Stopped),
        "failed durable stop commit must not publish durable Stopped state"
    );
}

#[tokio::test]
async fn async_stop_does_not_publish_stopped_while_lifecycle_commit_is_in_flight() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::RunPrimitive;

    struct StopRecordingExecutor {
        stop_called: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for StopRecordingExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Err(CoreExecutorError::apply_failed_runtime_turn(
                "unexpected apply during stop publish regression",
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            self.stop_called.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    let store = Arc::new(HarnessRuntimeStore::delayed_terminal_lifecycle_commit(
        Duration::from_millis(250),
    ));
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    let stop_called = Arc::new(AtomicBool::new(false));
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(StopRecordingExecutor {
                stop_called: Arc::clone(&stop_called),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    let baseline_commits = store.commit_machine_lifecycle_calls();
    let stop_adapter = Arc::clone(&adapter);
    let stop_sid = sid.clone();
    let stop_task = tokio::spawn(async move {
        stop_adapter
            .stop_runtime_executor(&stop_sid, "delayed async stop")
            .await
    });

    wait_for_atomic_bool(&stop_called, "stop effect should reach executor").await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while store.commit_machine_lifecycle_calls() <= baseline_commits {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("stop lifecycle commit should enter the delayed store call");

    assert_ne!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Stopped,
        "in-flight durable stop commit must not publish visible Stopped state"
    );

    stop_task
        .await
        .expect("stop task should not panic")
        .expect("delayed stop should eventually commit");
    wait_for_runtime_state(
        &adapter,
        &sid,
        RuntimeState::Stopped,
        "stop should publish after durable commit",
    )
    .await;
}

#[tokio::test]
async fn cold_reregister_replaces_destroyed_process_projection_with_idle_shell() {
    let store = Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new());
    let sid = SessionId::new();

    let adapter = Arc::new(MeerkatMachine::persistent(
        Arc::clone(&store) as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");
    let runtime_id = LogicalRuntimeId::for_session(&sid);
    meerkat_runtime::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should succeed before adapter restart");
    drop(adapter);

    let restarted = Arc::new(MeerkatMachine::persistent(
        Arc::clone(&store) as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    restarted
        .register_session(sid.clone())
        .await
        .expect("destroyed process phase is observation, not cold authority");
    assert_eq!(
        restarted.runtime_state(&sid).await.unwrap(),
        RuntimeState::Idle
    );
    assert_eq!(
        load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .unwrap(),
        Some(RuntimeState::Idle),
        "cold re-registration must publish a fresh unbound Idle shell",
    );
}

#[tokio::test]
async fn cold_reregister_ignores_legacy_session_uuid_runtime_state_alias() {
    let sid = SessionId::new();
    let legacy_runtime_alias = LogicalRuntimeId::legacy_session_uuid_alias(&sid);
    let store = Arc::new(HarnessRuntimeStore::fail_lifecycle_load_for(
        legacy_runtime_alias,
    ));

    let adapter = Arc::new(MeerkatMachine::persistent(
        Arc::clone(&store) as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Idle,
        "cold re-registration must not let the legacy runtime-state alias drive lifecycle",
    );
}

#[tokio::test]
async fn cold_reregister_prefers_canonical_runtime_state_over_stale_legacy_alias() {
    let sid = SessionId::new();
    let legacy_runtime_alias = LogicalRuntimeId::legacy_session_uuid_alias(&sid);
    let store = Arc::new(HarnessRuntimeStore::fail_lifecycle_load_for(
        legacy_runtime_alias,
    ));

    let adapter = Arc::new(MeerkatMachine::persistent(
        Arc::clone(&store) as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Idle,
        "cold re-registration must not let stale legacy runtime state override canonical state when both aliases exist",
    );
}

#[tokio::test]
async fn control_plane_receipt_lookup_ignores_legacy_storage_alias() {
    let store = Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new());
    let sid = SessionId::new();
    let canonical_runtime_id = LogicalRuntimeId::for_session(&sid);
    let legacy_runtime_alias = LogicalRuntimeId::legacy_session_uuid_alias(&sid);
    let run_id = RunId::new();
    let receipt = RunBoundaryReceipt {
        run_id: run_id.clone(),
        boundary: RunApplyBoundary::RunStart,
        contributing_input_ids: Vec::new(),
        conversation_digest: None,
        message_count: 0,
        sequence: 0,
    };
    store
        .atomic_apply(
            &legacy_runtime_alias,
            None,
            receipt.clone(),
            Vec::new(),
            None,
        )
        .await
        .expect("seed legacy boundary receipt alias");

    let adapter = Arc::new(MeerkatMachine::persistent(
        Arc::clone(&store) as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");

    let loaded = meerkat_runtime::traits::RuntimeControlPlane::load_boundary_receipt(
        adapter.as_ref(),
        &canonical_runtime_id,
        &run_id,
        0,
    )
    .await
    .expect("canonical runtime id should be accepted");
    assert!(
        loaded.is_none(),
        "canonical control-plane lookup must not read legacy receipt storage"
    );

    let raw_alias_err = meerkat_runtime::traits::RuntimeControlPlane::load_boundary_receipt(
        adapter.as_ref(),
        &legacy_runtime_alias,
        &run_id,
        0,
    )
    .await
    .expect_err("raw session UUID alias must not resolve as a runtime control-plane id");
    assert!(matches!(
        raw_alias_err,
        meerkat_runtime::traits::RuntimeControlPlaneError::NotFound(_)
    ));
}

#[tokio::test]
async fn unregistered_session_errors() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    let result = adapter.accept_input(&sid, make_prompt("hi")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn unregister_removes_driver() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");
    adapter
        .unregister_session(&sid)
        .await
        .expect("session should unregister cleanly");

    let result = adapter.runtime_state(&sid).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn recycle_preserves_ephemeral_queued_work() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");

    let first = make_prompt("first");
    let first_id = first.id().clone();
    let second = make_prompt("second");
    let second_id = second.id().clone();
    adapter.accept_input(&sid, first).await.unwrap();
    adapter.accept_input(&sid, second).await.unwrap();

    let runtime_id = LogicalRuntimeId::for_session(&sid);
    let report = meerkat_runtime::RuntimeControlPlane::recycle(&*adapter, &runtime_id)
        .await
        .unwrap();
    assert_eq!(report.inputs_transferred, 2);

    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Idle
    );
    let active_ids = adapter.list_active_inputs(&sid).await.unwrap();
    assert_eq!(active_ids, vec![first_id.clone(), second_id.clone()]);

    let first_state = adapter.input_state(&sid, &first_id).await.unwrap().unwrap();
    let second_state = adapter
        .input_state(&sid, &second_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        first_state.seed.phase,
        meerkat_runtime::InputLifecycleState::Queued
    );
    assert_eq!(
        second_state.seed.phase,
        meerkat_runtime::InputLifecycleState::Queued
    );
}

#[tokio::test]
async fn recycle_preserves_persistent_queued_work() {
    let store = Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent(store, memory_blob_store()));
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");

    let first = make_prompt("first");
    let first_id = first.id().clone();
    let second = make_prompt("second");
    let second_id = second.id().clone();
    adapter.accept_input(&sid, first).await.unwrap();
    adapter.accept_input(&sid, second).await.unwrap();

    let runtime_id = LogicalRuntimeId::for_session(&sid);
    let report = meerkat_runtime::RuntimeControlPlane::recycle(&*adapter, &runtime_id)
        .await
        .unwrap();
    assert_eq!(report.inputs_transferred, 2);

    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Idle
    );
    let active_ids = adapter.list_active_inputs(&sid).await.unwrap();
    assert_eq!(active_ids, vec![first_id.clone(), second_id.clone()]);

    let first_state = adapter.input_state(&sid, &first_id).await.unwrap().unwrap();
    let second_state = adapter
        .input_state(&sid, &second_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        first_state.seed.phase,
        meerkat_runtime::InputLifecycleState::Queued
    );
    assert_eq!(
        second_state.seed.phase,
        meerkat_runtime::InputLifecycleState::Queued
    );
}

#[tokio::test]
async fn recycle_lifecycle_commit_failure_preserves_generated_recycle_authority() {
    let store = Arc::new(HarnessRuntimeStore::failing_lifecycle_commit_after(2));
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");
    let runtime_id = LogicalRuntimeId::for_session(&sid);

    meerkat_runtime::traits::RuntimeControlPlane::retire(&*adapter, &runtime_id)
        .await
        .expect("retire should commit before recycle failure");
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Retired
    );
    assert_eq!(
        load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .unwrap(),
        Some(RuntimeState::Retired)
    );

    let err = meerkat_runtime::traits::RuntimeControlPlane::recycle(&*adapter, &runtime_id)
        .await
        .expect_err("recycle should surface lifecycle commit failure");
    assert!(
        err.to_string()
            .contains("synthetic commit_machine_lifecycle failure"),
        "unexpected error: {err}",
    );
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Idle,
        "failed recycle must preserve the generated Recycle transition",
    );
    assert_eq!(
        load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .unwrap(),
        Some(RuntimeState::Retired),
        "failed recycle must not persist idle runtime truth",
    );
}

#[tokio::test]
async fn recycle_keeps_waiters_for_preserved_pending_input() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};

    struct NoResultExecutor;
    #[async_trait::async_trait]
    impl CoreExecutor for NoResultExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }
        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");

    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, make_prompt("survive recycle"))
        .await
        .unwrap();
    assert!(outcome.is_accepted());
    let handle = handle.expect("accepted input should produce a completion handle");

    let runtime_id = LogicalRuntimeId::for_session(&sid);
    let report = meerkat_runtime::RuntimeControlPlane::recycle(&*adapter, &runtime_id)
        .await
        .unwrap();
    assert_eq!(report.inputs_transferred, 1);

    adapter
        .register_session_with_executor(sid.clone(), Box::new(NoResultExecutor))
        .await
        .expect("runtime executor registration should succeed");

    let result = tokio::time::timeout(Duration::from_secs(1), handle.wait())
        .await
        .expect("completion should resolve after recycle + executor attach")
        .expect("completion waiter should resolve");
    assert!(
        matches!(
            result,
            meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult
        ),
        "recycle should preserve pending waiter linkage for active input, got {result:?}"
    );
}

#[tokio::test]
async fn recycle_attached_runtime_wakes_preserved_queued_work() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_runtime::{PeerConvention, PeerInput, ResponseProgressPhase};

    struct CountingExecutor {
        apply_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for CountingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    fn make_progress_input(label: &str) -> Input {
        Input::Peer(PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            system_prompts: Vec::new(),
            injected_context: Vec::new(),
            sender_taint: None,
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "peer-1".into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Ephemeral,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::ResponseProgress {
                request_id: format!("req-{label}"),
                phase: ResponseProgressPhase::InProgress,
            }),
            content: format!("progress-{label}").into(),
            payload: Some(serde_json::json!({ "label": label })),
            handling_mode: None,
        })
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(CountingExecutor {
                apply_calls: Arc::clone(&apply_calls),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, make_progress_input("recycle-attached"))
        .await
        .unwrap();
    assert!(outcome.is_accepted());
    let handle = handle.expect("queued progress input should expose a completion handle");

    let runtime_id = LogicalRuntimeId::for_session(&sid);
    let report = meerkat_runtime::RuntimeControlPlane::recycle(&*adapter, &runtime_id)
        .await
        .unwrap();
    assert_eq!(report.inputs_transferred, 1);

    let result = tokio::time::timeout(Duration::from_secs(1), handle.wait())
        .await
        .expect("attached runtime should wake and drain recycled queued work")
        .expect("completion waiter should resolve");
    assert!(
        matches!(
            result,
            meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult
        ),
        "attached recycle should preserve and drain queued work, got {result:?}"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "recycle should wake the existing loop exactly once for preserved queued work"
    );
    let runtime_state = wait_for_runtime_state(
        &adapter,
        &sid,
        RuntimeState::Attached,
        "attached recycle should return the existing loop to Attached",
    )
    .await;
    assert_eq!(runtime_state, RuntimeState::Attached);
}

#[tokio::test]
async fn unregister_session_terminates_pending_completion_waiters() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");

    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, make_prompt("pending on unregister"))
        .await
        .unwrap();
    assert!(outcome.is_accepted());
    let handle = handle.expect("accepted input should produce a completion handle");

    adapter
        .unregister_session(&sid)
        .await
        .expect("session should unregister cleanly");

    let result = handle
        .wait()
        .await
        .expect("completion waiter should resolve");
    assert!(
        matches!(
            result,
            meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated { ref reason, .. }
            if reason == "runtime session unregistered"
        ),
        "unregister should explicitly terminate pending waiters, got {result:?}"
    );
}

/// Test that accept_input with a RuntimeLoop triggers input processing.
#[tokio::test]
async fn accept_with_executor_triggers_loop() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use std::sync::atomic::{AtomicBool, Ordering};

    // Track whether apply was called
    let apply_called = Arc::new(AtomicBool::new(false));
    let apply_called_clone = apply_called.clone();

    struct TestExecutor {
        called: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for TestExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.called.store(true, Ordering::SeqCst);
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    let executor = Box::new(TestExecutor {
        called: apply_called_clone,
    });
    adapter
        .register_session_with_executor(sid.clone(), executor)
        .await
        .expect("runtime executor registration should succeed");

    // Accept input — should trigger the loop
    let input = make_prompt("hello from executor test");
    let outcome = adapter.accept_input(&sid, input).await.unwrap();
    assert!(outcome.is_accepted());

    wait_for_atomic_bool(
        &apply_called,
        "CoreExecutor::apply() should have been called by the RuntimeLoop",
    )
    .await;

    // After processing, the input should be consumed and the runtime back to Attached
    // (executor is still connected, so Attached not Idle).
    let state = wait_for_runtime_state(
        &adapter,
        &sid,
        RuntimeState::Attached,
        "RuntimeLoop should settle back to Attached after committing the executor output",
    )
    .await;
    assert_eq!(state, RuntimeState::Attached);

    // The input should be consumed (terminal)
    let active = adapter.list_active_inputs(&sid).await.unwrap();
    assert!(active.is_empty(), "All inputs should be consumed");
}

#[tokio::test]
async fn runtime_comms_terminal_response_wake_drains_requester_queue() {
    use meerkat_comms::runtime::comms_runtime::CommsRuntime as InprocCommsRuntime;
    use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
    use meerkat_core::comms::{
        CommsCommand, PeerAddress, PeerName, PeerRoute, PeerTransport, TrustedPeerDescriptor,
    };
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_core::{HandlingMode, InteractionId, PeerCorrelationId, ResponseStatus};
    use meerkat_runtime::PeerConvention;
    use meerkat_runtime::meerkat_machine::dsl as mm_dsl;
    use tokio::sync::Notify;
    use uuid::Uuid;

    // Wall-clock ceiling only: the assertions below poll explicit recorder
    // and ledger state, so loaded-runner scheduling is not mistaken for a
    // two-second product liveness contract.
    const LOADED_CI_CONVERGENCE_TIMEOUT: Duration = Duration::from_secs(15);

    struct BlockingRecordingExecutor {
        calls: Arc<AtomicUsize>,
        first_apply_started: Arc<Notify>,
        release_first_apply: Arc<Notify>,
        terminal_notice_request_ids: Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingRecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == 1 {
                self.first_apply_started.notify_one();
                self.release_first_apply.notified().await;
            }

            let boundary = match &primitive {
                RunPrimitive::StagedInput(staged) => {
                    for append in &staged.appends {
                        if let meerkat_core::lifecycle::run_primitive::CoreRenderable::SystemNotice {
                            blocks,
                            ..
                        } = &append.content
                        {
                            for block in blocks {
                                if let meerkat_core::types::SystemNoticeBlock::Comms {
                                    kind:
                                        meerkat_core::types::CommsNoticeKind::ResponseTerminal,
                                    request_id: Some(request_id),
                                    ..
                                } = block
                                {
                                    self.terminal_notice_request_ids
                                        .lock()
                                        .expect("terminal_notice_request_ids mutex")
                                        .push(request_id.clone());
                                }
                            }
                        }
                    }
                    staged.boundary
                }
                RunPrimitive::ImmediateAppend(_) => RunApplyBoundary::Immediate,
                _ => RunApplyBoundary::RunStart,
            };

            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    fn descriptor_for_runtime(
        runtime: &InprocCommsRuntime,
    ) -> Result<TrustedPeerDescriptor, String> {
        Ok(TrustedPeerDescriptor {
            peer_id: runtime.public_key().to_peer_id(),
            name: PeerName::new(runtime.participant_name().to_string())?,
            address: PeerAddress::new(PeerTransport::Inproc, runtime.participant_name()),
            pubkey: *runtime.public_key().as_bytes(),
        })
    }

    let suffix = Uuid::new_v4().simple().to_string();
    let name_a = format!("runtime-requester-{suffix}");
    let name_b = format!("runtime-responder-{suffix}");
    let requester_comms =
        Arc::new(InprocCommsRuntime::inproc_only(&name_a).expect("requester comms runtime"));
    let responder_comms =
        Arc::new(InprocCommsRuntime::inproc_only(&name_b).expect("responder comms runtime"));
    let requester_descriptor =
        descriptor_for_runtime(requester_comms.as_ref()).expect("requester descriptor");
    let responder_descriptor =
        descriptor_for_runtime(responder_comms.as_ref()).expect("responder descriptor");

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    let bindings = adapter
        .prepare_bindings(sid.clone())
        .await
        .expect("prepare runtime bindings");
    bindings
        .install_peer_comms_on(requester_comms.as_ref())
        .expect("install requester peer-comms handle");
    requester_comms.install_peer_request_response_authority(
        meerkat_comms::PeerRequestResponseAuthority::new(
            Arc::clone(bindings.peer_interaction()),
            Arc::clone(bindings.interaction_stream()),
        ),
    );
    let responder_adapter = Arc::new(MeerkatMachine::ephemeral());
    let responder_sid = SessionId::new();
    let responder_bindings = responder_adapter
        .prepare_bindings(responder_sid.clone())
        .await
        .expect("prepare responder runtime bindings");
    responder_bindings
        .install_peer_comms_on(responder_comms.as_ref())
        .expect("install responder peer-comms handle");
    responder_comms.install_peer_request_response_authority(
        meerkat_comms::PeerRequestResponseAuthority::new(
            Arc::clone(responder_bindings.peer_interaction()),
            Arc::clone(responder_bindings.interaction_stream()),
        ),
    );

    let requester_for_trust: Arc<dyn CoreCommsRuntime> = requester_comms.clone();
    adapter
        .stage_add_direct_peer_endpoint(
            &sid,
            mm_dsl::PeerEndpoint::from(&responder_descriptor),
            requester_for_trust,
        )
        .await
        .expect("requester generated trust should apply");
    let responder_for_trust: Arc<dyn CoreCommsRuntime> = responder_comms.clone();
    responder_adapter
        .stage_add_direct_peer_endpoint(
            &responder_sid,
            mm_dsl::PeerEndpoint::from(&requester_descriptor),
            responder_for_trust,
        )
        .await
        .expect("responder generated trust should apply");

    let calls = Arc::new(AtomicUsize::new(0));
    let first_apply_started = Arc::new(Notify::new());
    let release_first_apply = Arc::new(Notify::new());
    let terminal_notice_request_ids = Arc::new(std::sync::Mutex::new(Vec::new()));
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(BlockingRecordingExecutor {
                calls: Arc::clone(&calls),
                first_apply_started: Arc::clone(&first_apply_started),
                release_first_apply: Arc::clone(&release_first_apply),
                terminal_notice_request_ids: Arc::clone(&terminal_notice_request_ids),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    let requester_for_drain: Arc<dyn CoreCommsRuntime> = requester_comms.clone();
    assert!(
        adapter
            .update_peer_ingress_context(&sid, true, Some(requester_for_drain))
            .await
            .expect("update peer ingress context"),
        "host-mode requester should spawn comms drain"
    );

    // This test exercises response-terminal wake behavior, not request
    // transport. Seed the exact generated correlation authority directly;
    // PeerRequest is DurableRuntime ingress and cannot be consumed by the
    // volatile response handoff used below.
    let request_id = Uuid::new_v4();
    bindings
        .peer_interaction()
        .request_sent(PeerCorrelationId::from_uuid(request_id))
        .expect("seed requester outbound request state");
    responder_bindings
        .peer_interaction()
        .request_received(
            PeerCorrelationId::from_uuid(request_id),
            meerkat_core::types::HandlingMode::Queue,
        )
        .expect("seed responder inbound request state");

    // Hold the requester only after the outbound request lifecycle is durable.
    // The contract under test is that the terminal response wakes this busy
    // requester, not that request admission can bypass its active apply.
    adapter
        .accept_input(&sid, make_prompt("keep requester busy"))
        .await
        .expect("accept blocking prompt");
    tokio::time::timeout(Duration::from_secs(2), first_apply_started.notified())
        .await
        .expect("first apply should start");

    CoreCommsRuntime::send(
        responder_comms.as_ref(),
        CommsCommand::PeerResponse {
            objective_id: None,
            content_taint: None,
            to: PeerRoute::with_display_name(
                requester_comms.public_key().to_peer_id(),
                PeerName::new(name_a.clone()).expect("requester peer name"),
            ),
            in_reply_to: InteractionId(request_id),
            status: ResponseStatus::Completed,
            result: serde_json::json!({"probe_reply": true}),
            blocks: None,
            handling_mode: Some(HandlingMode::Steer),
        },
    )
    .await
    .expect("send terminal response");

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let active_ids = adapter
                .list_active_inputs(&sid)
                .await
                .expect("active inputs");
            for input_id in active_ids {
                if let Some(state) = adapter
                    .input_state(&sid, &input_id)
                    .await
                    .expect("input state")
                    && matches!(
                        state.state.persisted_input.as_ref(),
                        Some(Input::Peer(meerkat_runtime::PeerInput {
                            convention: Some(PeerConvention::ResponseTerminal { .. }),
                            ..
                        }))
                    )
                {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("terminal response should queue while requester is running");

    // Point-to-point test handshake: `notify_one` stores a permit if the
    // executor was preempted between announcing start and awaiting release.
    release_first_apply.notify_one();
    let terminal_notice_request_ids_for_wait = Arc::clone(&terminal_notice_request_ids);
    tokio::time::timeout(LOADED_CI_CONVERGENCE_TIMEOUT, async move {
        loop {
            let terminal_was_applied = !terminal_notice_request_ids_for_wait
                .lock()
                .expect("terminal_notice_request_ids mutex")
                .is_empty();
            if terminal_was_applied {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("WakeLoop should drain queued peer_response_terminal");

    let request_ids = terminal_notice_request_ids
        .lock()
        .expect("terminal_notice_request_ids mutex")
        .clone();
    assert_eq!(
        request_ids,
        vec![request_id.to_string()],
        "terminal response should render through one typed durable SystemNotice append"
    );
    // `terminal_notice_request_ids` is recorded from inside `CoreExecutor::apply`,
    // before the runtime loop commits the returned receipt and consumes the
    // staged input. Observe the authoritative input ledger instead of racing
    // that post-apply commit under a loaded test runner.
    tokio::time::timeout(LOADED_CI_CONVERGENCE_TIMEOUT, async {
        loop {
            let active_ids = adapter
                .list_active_inputs(&sid)
                .await
                .expect("active inputs after drain");
            let mut terminal_still_active = false;
            for input_id in active_ids {
                if let Some(state) = adapter
                    .input_state(&sid, &input_id)
                    .await
                    .expect("input state")
                    && matches!(
                        state.state.persisted_input.as_ref(),
                        Some(Input::Peer(meerkat_runtime::PeerInput {
                            convention: Some(PeerConvention::ResponseTerminal { .. }),
                            ..
                        }))
                    )
                {
                    terminal_still_active = true;
                    break;
                }
            }
            if !terminal_still_active {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("queued peer_response_terminal must leave the active ledger after WakeLoop");
    assert!(
        calls.load(Ordering::SeqCst) >= 2,
        "requester executor should run once for prompt and once for terminal response"
    );
}

/// Test that a failed executor never strands the input in APC.
#[tokio::test]
async fn failed_executor_does_not_strand_input_in_apc() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::RunPrimitive;
    use meerkat_runtime::input_state::InputLifecycleState;
    struct FailingExecutor {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for FailingExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(CoreExecutorError::apply_failed_runtime_turn("LLM error"))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    let calls = Arc::new(AtomicUsize::new(0));
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(FailingExecutor {
                calls: Arc::clone(&calls),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    let input = make_prompt("hello failing");
    let input_id = input.id().clone();
    adapter.accept_input(&sid, input).await.unwrap();

    let is = wait_for_input_state(
        &adapter,
        &sid,
        &input_id,
        "failed executor should leave the input in a non-in-flight state",
        |state| {
            !matches!(
                state.seed.phase,
                InputLifecycleState::Staged | InputLifecycleState::AppliedPendingConsumption
            )
        },
    )
    .await;

    wait_for_atomic_usize_at_least(
        &calls,
        1,
        "failing executor should receive at least one apply attempt",
    )
    .await;
    // Observe the post-failure state, rather than the initial queued state or
    // a subsequent retry's transient Running phase.
    let state = wait_for_runtime_state(
        &adapter,
        &sid,
        RuntimeState::Attached,
        "failed executor should return to Attached",
    )
    .await;
    assert_eq!(state, RuntimeState::Attached);

    // Input should roll back or abandon after retry exhaustion, but never
    // remain stuck in an in-flight lifecycle state.
    assert!(
        matches!(
            is.seed.phase,
            InputLifecycleState::Queued | InputLifecycleState::Abandoned
        ),
        "Failed execution should roll input back or abandon it after retry budget exhaustion, not strand it in AppliedPendingConsumption"
    );
}

#[tokio::test]
async fn failed_executor_stops_retrying_after_stage_budget_exhausted() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::RunPrimitive;
    use meerkat_runtime::input_state::InputLifecycleState;

    struct CountingFailExecutor {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for CountingFailExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(CoreExecutorError::apply_failed_runtime_turn("always fails"))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(CountingFailExecutor {
                calls: Arc::clone(&calls),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    let input = make_prompt("hello failing forever");
    let input_id = input.id().clone();
    adapter.accept_input(&sid, input).await.unwrap();

    wait_for_atomic_usize_at_least(
        &calls,
        1,
        "failing executor should be called before retry-budget assertions",
    )
    .await;
    // The budget is exhausted only by attempts that actually accrue. Waiting
    // for "any non-in-flight phase" would also accept a rolled-back input that
    // parked in Queued after a single attempt, which is the disarmed-member
    // defect wearing a passing assertion.
    let state = wait_for_input_state(
        &adapter,
        &sid,
        &input_id,
        "failed input should terminalize typed once its stage budget is spent",
        |state| state.seed.phase == InputLifecycleState::Abandoned,
    )
    .await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "retry budget should be spent exactly once per stage attempt"
    );
    assert_eq!(state.seed.attempt_count, 3);
    assert_eq!(
        state.seed.terminal_outcome,
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::MaxAttemptsExhausted { attempts: 3 },
        }),
        "the generated retry policy owns the terminal outcome"
    );

    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "failed inputs must not keep spinning through fresh run ids"
    );
}

/// Field regression (household fleet, 2026-08-12, domain:home): a turn failed,
/// its staged input was rolled back to Queued, and nothing re-armed the runtime
/// loop. The input sat queued for 21 minutes with zero further attempts and
/// zero refusals; only an app restart - which wakes on
/// `!active_input_ids().is_empty()` at attachment commit - re-staged it. A
/// sibling input on the same member moved 288 seconds after its own rollback,
/// but only because an unrelated input arrived and woke the loop: the wake was
/// incidental, never owned by the rollback.
///
/// The particular provider error that failed that turn is irrelevant. This
/// drives a deliberately generic turn failure and then supplies NO further
/// stimulus: no second input, no wake, no completion traffic, no restart. The
/// machine's own stage-attempt policy must carry the input to a typed terminal.
#[tokio::test]
async fn generic_turn_failure_re_arms_its_own_rolled_back_input_without_external_stimulus() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::RunPrimitive;
    use meerkat_runtime::input_state::InputLifecycleState;

    struct GenericTurnFailureExecutor {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for GenericTurnFailureExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(CoreExecutorError::apply_failed_runtime_turn(
                "generic turn failure",
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(GenericTurnFailureExecutor {
                calls: Arc::clone(&calls),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    let input = make_prompt("generic failure with nothing else happening");
    let input_id = input.id().clone();
    adapter.accept_input(&sid, input).await.unwrap();

    // Nothing else touches this session from here on.
    let state = wait_for_input_state(
        &adapter,
        &sid,
        &input_id,
        "a failed turn must own the re-arm for the input it rolled back; \
         without it the input parks in Queued until unrelated activity wakes the loop",
        |state| state.seed.phase == InputLifecycleState::Abandoned,
    )
    .await;

    assert_eq!(
        state.seed.attempt_count, 3,
        "every re-arm must accrue a machine-owned stage attempt"
    );
    assert_eq!(
        state.seed.terminal_outcome,
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::MaxAttemptsExhausted { attempts: 3 },
        }),
        "the generated retry policy owns the terminal, not the shell"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "the re-arm must be bounded by the machine's stage-attempt budget"
    );

    // The budget is the bound: once the lane is empty the loop parks for real.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "a terminalized input must not keep spinning through fresh run ids"
    );
    assert!(
        adapter.list_active_inputs(&sid).await.unwrap().is_empty(),
        "a terminalized input must not remain active work"
    );
}

/// Sibling of the re-arm regression: the caller's completion waiter must still
/// resolve on the attempt that failed, AND the durable input must still reach a
/// typed terminal. Converting "caller hangs, input queued forever" into
/// "caller hangs, input abandoned" would be a worse lie than the defect.
#[tokio::test]
async fn generic_turn_failure_resolves_its_completion_waiter_and_terminalizes_the_input() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::RunPrimitive;
    use meerkat_runtime::input_state::InputLifecycleState;

    struct GenericTurnFailureExecutor;

    #[async_trait::async_trait]
    impl CoreExecutor for GenericTurnFailureExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Err(CoreExecutorError::apply_failed_runtime_turn(
                "generic turn failure",
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session_with_executor(sid.clone(), Box::new(GenericTurnFailureExecutor))
        .await
        .expect("runtime executor registration should succeed");

    let input = make_prompt("generic failure with a waiting caller");
    let input_id = input.id().clone();
    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, input)
        .await
        .unwrap();
    assert!(outcome.is_accepted());

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        handle
            .expect("accepted input should carry a completion handle")
            .wait(),
    )
    .await
    .expect("completion waiter must resolve after a failed turn")
    .expect("completion waiter should resolve");
    assert!(
        matches!(
            result,
            meerkat_runtime::completion::CompletionOutcome::AbandonedWithError { .. }
                | meerkat_runtime::completion::CompletionOutcome::Abandoned { .. }
        ),
        "a failed turn must report typed failure metadata to its caller, got {result:?}"
    );

    let state = wait_for_input_state(
        &adapter,
        &sid,
        &input_id,
        "a resolved caller waiter must not leave the durable input parked in Queued",
        |state| state.seed.phase == InputLifecycleState::Abandoned,
    )
    .await;
    assert_eq!(
        state.seed.terminal_outcome,
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::MaxAttemptsExhausted { attempts: 3 },
        }),
    );
}

/// Wake coalescing contract. `wake_tx` is a bounded `mpsc(16)` written with
/// `try_send`, so wakes are silently dropped when the channel is full and
/// merged whenever several arrive while the loop is inside `process_queue`.
/// That is only sound because ONE wake drains the queue to empty. Prompts never
/// batch with each other (the generated queue batcher breaks after a prompt),
/// so these three inputs are three separate runs from exactly one wake.
#[tokio::test]
async fn one_wake_drains_every_queued_input_so_a_merged_wake_cannot_park_work() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_runtime::input_state::InputLifecycleState;

    struct CountingExecutor {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for CountingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(CountingExecutor {
                calls: Arc::clone(&calls),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    let mut input_ids = Vec::new();
    for index in 0..3 {
        let input = make_prompt(&format!("queued without a wake {index}"));
        input_ids.push(input.id().clone());
        adapter
            .accept_input_without_wake(&sid, input)
            .await
            .expect("queued-only admission should succeed");
    }

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "queued-only admission must not process work before a wake"
    );

    assert!(
        adapter
            .wake_runtime_if_active_inputs(&sid)
            .await
            .expect("wake should reach the attached loop"),
        "a single wake must be delivered while queued work exists"
    );

    for input_id in &input_ids {
        let state = wait_for_input_state(
            &adapter,
            &sid,
            input_id,
            "one wake must drain every queued input; a dropped or merged wake \
             must not leave queued work parked",
            |state| state.seed.phase == InputLifecycleState::Consumed,
        )
        .await;
        assert_eq!(state.seed.phase, InputLifecycleState::Consumed);
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "each prompt is its own run, all reached from one wake"
    );
}

/// FIFO head contract for the re-arm: work queued behind a failing head must
/// still make progress, and the head itself must still terminalize typed. The
/// backlog input is the last external stimulus in this test.
#[tokio::test]
async fn queued_backlog_progresses_while_the_failing_head_terminalizes() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_runtime::input_state::InputLifecycleState;

    struct HeadFailsExecutor {
        head_id: InputId,
        head_calls: Arc<AtomicUsize>,
        backlog_calls: Arc<AtomicUsize>,
        first_apply_started: Arc<tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for HeadFailsExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            if primitive.contributing_input_ids().contains(&self.head_id) {
                if self.head_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    self.first_apply_started.notify_one();
                    // Hold the head's first run open long enough for the
                    // backlog input to be admitted behind it.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                return Err(CoreExecutorError::apply_failed_runtime_turn(
                    "generic turn failure",
                ));
            }
            self.backlog_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let head = make_prompt("failing head");
    let head_id = head.id().clone();
    let head_calls = Arc::new(AtomicUsize::new(0));
    let backlog_calls = Arc::new(AtomicUsize::new(0));
    let first_apply_started = Arc::new(tokio::sync::Notify::new());

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(HeadFailsExecutor {
                head_id: head_id.clone(),
                head_calls: Arc::clone(&head_calls),
                backlog_calls: Arc::clone(&backlog_calls),
                first_apply_started: Arc::clone(&first_apply_started),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    adapter.accept_input(&sid, head).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), first_apply_started.notified())
        .await
        .expect("the head run should start before the backlog input is queued");

    let backlog = make_prompt("backlog behind the failing head");
    let backlog_id = backlog.id().clone();
    adapter.accept_input(&sid, backlog).await.unwrap();

    let backlog_state = wait_for_input_state(
        &adapter,
        &sid,
        &backlog_id,
        "work queued behind a failing head must still drain",
        |state| state.seed.phase == InputLifecycleState::Consumed,
    )
    .await;
    assert_eq!(backlog_state.seed.phase, InputLifecycleState::Consumed);

    let head_state = wait_for_input_state(
        &adapter,
        &sid,
        &head_id,
        "the failing head must terminalize typed instead of parking in Queued \
         once the backlog behind it is empty",
        |state| state.seed.phase == InputLifecycleState::Abandoned,
    )
    .await;
    assert_eq!(
        head_state.seed.terminal_outcome,
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::MaxAttemptsExhausted { attempts: 3 },
        }),
    );
    assert_eq!(
        head_calls.load(Ordering::SeqCst),
        3,
        "the head must consume exactly its machine-owned stage budget"
    );
    assert_eq!(
        backlog_calls.load(Ordering::SeqCst),
        1,
        "the backlog input must run exactly once"
    );
}

#[tokio::test]
async fn failed_executor_continues_processing_backlog() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_runtime::input_state::InputLifecycleState;

    struct FailThenSucceedExecutor {
        calls: Arc<AtomicUsize>,
        first_apply_started: Arc<tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for FailThenSucceedExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                self.first_apply_started.notify_one();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            if call == 0 {
                return Err(CoreExecutorError::apply_failed_runtime_turn(
                    "first run fails",
                ));
            }
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let first_apply_started = Arc::new(tokio::sync::Notify::new());
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(FailThenSucceedExecutor {
                calls: Arc::clone(&calls),
                first_apply_started: Arc::clone(&first_apply_started),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    let first = make_prompt("first");
    let first_id = first.id().clone();
    let second = make_prompt("second");
    let second_id = second.id().clone();
    adapter.accept_input(&sid, first).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), first_apply_started.notified())
        .await
        .expect("first apply should start before the backlog input is queued");
    adapter.accept_input(&sid, second).await.unwrap();

    let second_state = wait_for_input_state(
        &adapter,
        &sid,
        &second_id,
        "runtime loop should keep draining queued backlog after a failed run",
        |state| state.seed.phase == InputLifecycleState::Consumed,
    )
    .await;
    assert_eq!(second_state.seed.phase, InputLifecycleState::Consumed);
    let runtime_state = wait_for_runtime_state(
        &adapter,
        &sid,
        RuntimeState::Attached,
        "runtime should return to Attached after draining queued backlog",
    )
    .await;
    assert_eq!(runtime_state, RuntimeState::Attached);
    assert!(
        calls.load(Ordering::SeqCst) >= 2,
        "the runtime loop should keep draining queued backlog after a failed run"
    );
    let first_state = adapter.input_state(&sid, &first_id).await.unwrap().unwrap();
    assert!(
        matches!(
            first_state.seed.phase,
            InputLifecycleState::Queued | InputLifecycleState::Consumed
        ),
        "the initially failed input should have been safely rolled back or retried after the backlog drained"
    );
}

#[tokio::test]
async fn ensure_session_with_executor_upgrades_registered_session() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_runtime::input_state::InputLifecycleState;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct SuccessExecutor {
        called: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for SuccessExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.called.store(true, Ordering::SeqCst);
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let apply_called = Arc::new(AtomicBool::new(false));
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");

    let input = make_prompt("upgrade me");
    let input_id = input.id().clone();
    let outcome = adapter.accept_input(&sid, input).await.unwrap();
    assert!(outcome.is_accepted());

    adapter
        .ensure_session_with_executor(
            sid.clone(),
            Box::new(SuccessExecutor {
                called: Arc::clone(&apply_called),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    wait_for_atomic_bool(
        &apply_called,
        "upgrading an already-registered session should attach a live loop",
    )
    .await;

    let is = wait_for_input_state(
        &adapter,
        &sid,
        &input_id,
        "the pre-upgrade queued input should be processed once the loop is attached",
        |state| state.seed.phase == InputLifecycleState::Consumed,
    )
    .await;
    assert_eq!(
        is.seed.phase,
        InputLifecycleState::Consumed,
        "the pre-upgrade queued input should be processed once the loop is attached"
    );

    let state = adapter.runtime_state(&sid).await.unwrap();
    assert_eq!(state, RuntimeState::Attached);

    let active = adapter.list_active_inputs(&sid).await.unwrap();
    assert!(active.is_empty(), "queued work should drain after upgrade");
}

#[tokio::test]
async fn ensure_session_with_executor_upgrades_racy_registration() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_runtime::input_state::InputLifecycleState;

    struct SuccessExecutor {
        called: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for SuccessExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.called.store(true, Ordering::SeqCst);
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let store = Arc::new(HarnessRuntimeStore::delayed_recover(Duration::from_millis(
        75,
    )));
    let adapter = Arc::new(MeerkatMachine::persistent(store, memory_blob_store()));
    let sid = SessionId::new();
    let apply_called = Arc::new(AtomicBool::new(false));

    let ensure_task = {
        let adapter = Arc::clone(&adapter);
        let sid = sid.clone();
        let apply_called = Arc::clone(&apply_called);
        tokio::spawn(async move {
            adapter
                .ensure_session_with_executor(
                    sid,
                    Box::new(SuccessExecutor {
                        called: apply_called,
                    }),
                )
                .await
                .expect("runtime executor registration should succeed");
        })
    };

    tokio::time::sleep(Duration::from_millis(10)).await;
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");
    ensure_task.await.unwrap();

    let input = make_prompt("race upgrade");
    let input_id = input.id().clone();
    adapter.accept_input(&sid, input).await.unwrap();

    wait_for_atomic_bool(
        &apply_called,
        "the racy registration path should still attach a live runtime loop",
    )
    .await;
    let state = wait_for_input_state(
        &adapter,
        &sid,
        &input_id,
        "racy registration path should process accepted input",
        |state| state.seed.phase == InputLifecycleState::Consumed,
    )
    .await;
    assert_eq!(state.seed.phase, InputLifecycleState::Consumed);
}

#[tokio::test]
async fn ensure_session_with_executor_repairs_stale_attached_driver() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_runtime::input_state::InputLifecycleState;

    struct PanicOnceOnStopExecutor {
        stop_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for PanicOnceOnStopExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            assert!(
                self.stop_calls.fetch_add(1, Ordering::SeqCst) != 0,
                "synthetic stop panic to kill the loop and leave driver attached"
            );
            Ok(())
        }
    }

    struct RecordingExecutor {
        called: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for RecordingExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.called.store(true, Ordering::SeqCst);
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    let stop_calls = Arc::new(AtomicUsize::new(0));
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(PanicOnceOnStopExecutor {
                stop_calls: Arc::clone(&stop_calls),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Attached
    );

    let stop_error = adapter
        .stop_runtime_executor(&sid, "stale attachment repair test")
        .await
        .expect_err("a panicked stop hook must fail closed without a cleanup acknowledgement");
    assert!(
        stop_error
            .to_string()
            .contains("runtime loop exited without acknowledging required stop cleanup"),
        "unexpected stop error: {stop_error}"
    );

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match adapter
                .hard_cancel_current_run(&sid, "stale attachment repair probe")
                .await
            {
                Err(RuntimeDriverError::NotReady { .. }) => break,
                _ => tokio::time::sleep(Duration::from_millis(5)).await,
            }
        }
    })
    .await
    .expect("runtime loop should die and leave a stale attached driver state behind");
    assert!(
        !adapter
            .session_has_executor(&sid)
            .await
            .expect("dead-attachment viability query"),
        "generated Active plus a closed attachment is not a viable executor"
    );
    adapter
        .stop_runtime_executor(&sid, "explicitly acknowledge repaired stale stop")
        .await
        .expect("explicit stop retry should consume the retained coordinator failure");

    let apply_called = Arc::new(AtomicBool::new(false));
    adapter
        .ensure_session_with_executor(
            sid.clone(),
            Box::new(RecordingExecutor {
                called: Arc::clone(&apply_called),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    let input = make_prompt("repair stale attachment");
    let input_id = input.id().clone();
    adapter.accept_input(&sid, input).await.unwrap();

    // Wait for the OUTCOME, not for the proxy that precedes it.
    //
    // `RecordingExecutor::apply` sets `apply_called` on ENTRY and only then
    // returns the output the runtime still has to process before the input
    // reaches `Consumed` and the runtime reaches `Attached`. Waiting on the
    // flag and asserting the phase immediately afterwards therefore races the
    // repair it is trying to observe: the flag says "apply was entered", and
    // the assertion reads "the repair committed". Those are different facts
    // with a window between them, and on a loaded runner the read lands inside
    // it - observed on a shard where this same commit had already passed the
    // identical lane minutes earlier.
    //
    // The bound is not the bug and raising it would not fix this: the old
    // 1s applied to reaching the proxy, which is fast and was never what
    // timed out. Poll the durable post-condition instead, so the test passes
    // as soon as the repair is real and fails only if it never becomes real.
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let state = adapter.input_state(&sid, &input_id).await.unwrap();
            let runtime_state = adapter.runtime_state(&sid).await.unwrap();
            if state.is_some_and(|state| state.seed.phase == InputLifecycleState::Consumed)
                && runtime_state == RuntimeState::Attached
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("ensuring with executor should repair the stale attached driver");

    assert!(
        apply_called.load(Ordering::SeqCst),
        "the repair must have run the executor, not merely reached the end state"
    );
    let state = adapter.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(state.seed.phase, InputLifecycleState::Consumed);
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Attached
    );
}

#[tokio::test]
async fn stop_runtime_executor_keeps_attachment_live_until_stop_completes() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use tokio::sync::Notify;

    struct BlockingStopExecutor {
        stop_entered: Arc<Notify>,
        release_stop: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingStopExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            self.stop_entered.notify_one();
            self.release_stop.notified().await;
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    let stop_entered = Arc::new(Notify::new());
    let release_stop = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(BlockingStopExecutor {
                stop_entered: Arc::clone(&stop_entered),
                release_stop: Arc::clone(&release_stop),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    let stop_adapter = Arc::clone(&adapter);
    let stop_sid = sid.clone();
    let stop_task = tokio::spawn(async move {
        stop_adapter
            .stop_runtime_executor(&stop_sid, "ownership-stop")
            .await
            .expect("stop command should send successfully");
    });

    stop_entered.notified().await;
    assert!(
        !stop_task.is_finished(),
        "stop must remain pending until executor stop and required cleanup acknowledge"
    );

    assert!(
        adapter.session_has_live_executor_attachment(&sid).await,
        "attachment should remain published while stop is still in progress"
    );

    release_stop.notify_one();
    tokio::time::timeout(Duration::from_secs(1), stop_task)
        .await
        .expect("stop should acknowledge after executor release")
        .expect("stop task should not panic");

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if adapter.runtime_state(&sid).await.unwrap() == RuntimeState::Stopped {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("runtime should reach Stopped after the blocking stop control is released");
    assert!(
        !adapter.session_has_live_executor_attachment(&sid).await,
        "attachment should be removed after stop completes"
    );

    let err = adapter
        .hard_cancel_current_run(&sid, "stopped runtime interrupt probe")
        .await
        .expect_err("stopped runtime should no longer expose a live attachment");
    assert!(matches!(
        err,
        RuntimeDriverError::NotReady {
            state: RuntimeState::Stopped
        }
    ));
}

#[tokio::test]
async fn stop_terminalization_converges_after_destroy_commits_while_stop_hook_is_in_flight() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use tokio::sync::Notify;

    struct DestroyRacingStopExecutor {
        stop_entered: Arc<Notify>,
        release_stop: Arc<Notify>,
        cleanup_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for DestroyRacingStopExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            self.stop_entered.notify_one();
            self.release_stop.notified().await;
            Ok(())
        }

        async fn cleanup_after_runtime_stop_terminalized(
            &mut self,
        ) -> Result<(), CoreExecutorError> {
            self.cleanup_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let store = Arc::new(HarnessRuntimeStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    let runtime_id = LogicalRuntimeId::for_session(&sid);
    let stop_entered = Arc::new(Notify::new());
    let release_stop = Arc::new(Notify::new());
    let cleanup_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(DestroyRacingStopExecutor {
                stop_entered: Arc::clone(&stop_entered),
                release_stop: Arc::clone(&release_stop),
                cleanup_calls: Arc::clone(&cleanup_calls),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    let stop_adapter = Arc::clone(&adapter);
    let stop_sid = sid.clone();
    let stop_task = tokio::spawn(async move {
        stop_adapter
            .stop_runtime_executor(&stop_sid, "destroy race")
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), stop_entered.notified())
        .await
        .expect("executor should accept the stop hook");

    tokio::time::timeout(
        Duration::from_secs(1),
        meerkat_runtime::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id),
    )
    .await
    .expect("destroy must commit while the accepted stop hook remains in flight")
    .expect("destroy should commit");
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Destroyed
    );
    assert_eq!(
        load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .unwrap(),
        Some(RuntimeState::Destroyed),
        "destroy must be durable before stop terminalization resumes"
    );
    assert!(
        !stop_task.is_finished(),
        "stop acknowledgement still requires terminalization and executor cleanup"
    );

    release_stop.notify_one();
    tokio::time::timeout(Duration::from_secs(1), stop_task)
        .await
        .expect("stop should converge after the executor hook is released")
        .expect("stop task should not panic")
        .expect("destroyed terminalization should absorb the late executor exit");
    wait_for_atomic_usize_at_least(
        &cleanup_calls,
        1,
        "destroyed stop terminalization must run required executor cleanup",
    )
    .await;
    assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);
    assert!(
        !adapter
            .session_has_executor(&sid)
            .await
            .expect("executor attachment query"),
        "cleanup convergence must retire the destroyed executor attachment"
    );
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Destroyed
    );
    assert_eq!(
        load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .unwrap(),
        Some(RuntimeState::Destroyed),
        "late stop terminalization must not overwrite absorbing destroy authority"
    );
}

#[tokio::test]
async fn completed_boundary_commit_failure_stops_executor_without_false_durable_terminal() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_runtime::input_state::InputLifecycleState;

    struct SuccessExecutor {
        stop_called: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for SuccessExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            self.stop_called.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    let store = Arc::new(HarnessRuntimeStore::failing_atomic_apply());
    let runtime_store: Arc<dyn RuntimeStore> = store.clone();
    let adapter = Arc::new(MeerkatMachine::persistent(
        runtime_store,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    let stop_called = Arc::new(AtomicBool::new(false));
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(SuccessExecutor {
                stop_called: Arc::clone(&stop_called),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    let degraded_registration = adapter
        .current_session_registration_witness(&sid)
        .await
        .expect("registered runtime should expose an exact registration witness");

    let input = make_prompt("loop boundary failure");
    let input_id = input.id().clone();
    adapter.accept_input(&sid, input).await.unwrap();

    wait_for_atomic_bool(
        &stop_called,
        "boundary commit failures should stop the dead executor path",
    )
    .await;
    assert_ne!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Stopped,
        "an uncertain boundary commit must not publish a false live Stopped terminal"
    );
    let state = adapter.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_ne!(
        state.seed.phase,
        InputLifecycleState::Abandoned,
        "an uncertain boundary commit must not publish a false input terminal"
    );

    // The original boundary fault has ended, but the first supported cold
    // recovery proof is independently unavailable. Disposal must preserve the
    // degraded process shell until that exact recovery read succeeds.
    store.set_fail_atomic_apply(false);
    store.set_fail_recovery_read(true);
    let repair_blocked = tokio::time::timeout(
        Duration::from_secs(2),
        adapter.recover_or_discard_reload_required_registration_if_current(&degraded_registration),
    )
    .await
    .expect("failed durable recovery proof should return without a timing-dependent retry")
    .expect_err("degraded disposal must not remove a shell before cold recovery succeeds");
    assert!(
        adapter.contains_session(&sid).await,
        "failed durable recovery proof must retain the exact degraded registration"
    );
    assert!(
        repair_blocked
            .to_string()
            .contains("synthetic cold recovery read failure"),
        "the supported cold recovery failure must remain observable: {repair_blocked}"
    );

    store.set_fail_recovery_read(false);
    store.set_fail_durable_tail_recovery_source(true);
    let tail_repair_blocked = tokio::time::timeout(
        Duration::from_secs(2),
        adapter.recover_or_discard_reload_required_registration_if_current(&degraded_registration),
    )
    .await
    .expect("durable-tail recovery failure should return without timing-dependent retry")
    .expect_err("cold successor publication must wait for exact durable-tail reconciliation");
    assert!(
        tail_repair_blocked
            .to_string()
            .contains("synthetic durable-tail recovery source failure"),
        "durable-tail failure must remain observable: {tail_repair_blocked}"
    );
    assert!(
        adapter.contains_session(&sid).await,
        "failed durable-tail recovery must retain the degraded registration"
    );

    store.set_fail_durable_tail_recovery_source(false);
    let (successor_published, release_discard_owner) =
        adapter.arm_reload_required_discard_after_successor_publication_test_hook(sid.clone());
    let cancelled_adapter = Arc::clone(&adapter);
    let cancelled_witness = degraded_registration.clone();
    let cancelled_waiter = tokio::spawn(async move {
        cancelled_adapter
            .recover_or_discard_reload_required_registration_if_current(&cancelled_witness)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), successor_published)
        .await
        .expect("owned recovery worker should publish its cold successor")
        .expect("successor-publication test hook should remain live");
    cancelled_waiter.abort();
    let _ = cancelled_waiter.await;
    release_discard_owner
        .send(())
        .expect("release owned recovery worker after waiter cancellation");

    let disposition = tokio::time::timeout(
        Duration::from_secs(2),
        adapter.recover_or_discard_reload_required_registration_if_current(&degraded_registration),
    )
    .await
    .expect("retry should observe the owned worker's completed replacement")
    .expect("cancelled-waiter retry should preserve durable authority");
    assert_eq!(
        disposition,
        meerkat_runtime::ReloadRequiredRegistrationDisposition::NotCurrent,
        "the detached owner must finish replacement after its first waiter is cancelled"
    );
    assert!(
        adapter.contains_session(&sid).await,
        "exact disposal must atomically publish the recovered cold successor"
    );
    let cold_successor = adapter
        .current_session_registration_witness(&sid)
        .await
        .expect("exact disposal publishes a cold successor witness");
    assert_ne!(
        cold_successor, degraded_registration,
        "the degraded witness must not identify the recovered successor"
    );
    assert!(
        adapter
            .registration_is_current_without_runtime_owner(&cold_successor)
            .await,
        "the recovered successor must retain no process-local runtime owner"
    );

    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(SuccessExecutor {
                stop_called: Arc::new(AtomicBool::new(false)),
            }),
        )
        .await
        .expect("the recovered cold successor should accept a new executor");
    assert_eq!(
        adapter
            .recover_or_discard_reload_required_registration_if_current(&degraded_registration,)
            .await
            .expect("stale degraded witness should be an idempotent observation"),
        meerkat_runtime::ReloadRequiredRegistrationDisposition::NotCurrent,
        "an old witness must not remove a same-session successor"
    );
    assert!(
        adapter.contains_session(&sid).await,
        "stale degraded cleanup must leave the successor registered"
    );
}

/// The non-mob half of the same durability contract, driven by the same real
/// store fault and with no explicit disposal call anywhere in the test.
///
/// The test above proves disposal works when a caller explicitly asks for it.
/// Nothing proved that an ordinary session ever gets that ask. A mob member
/// does, through the retire ladder in
/// meerkat-mob/src/runtime/provisioner.rs:2207. A CLI or RPC session has no
/// such ladder, so the only caller it will ever have is registration itself -
/// and registration used to answer a durability skew by handing the caller
/// back a demand for a cold reload that only registration could mint. One
/// durable fact, two consequences, decided by who owned the caller.
///
/// Built on the real fault deliberately. The reroute is guarded on the runtime
/// loop's teardown state, and a hand-aborted loop is exactly how one would
/// accidentally pin a state production never reaches.
#[tokio::test]
async fn degraded_registration_cold_reloads_through_the_ordinary_registration_path() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::RunPrimitive;

    struct SuccessExecutor {
        stop_called: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for SuccessExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            self.stop_called.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    let store = Arc::new(HarnessRuntimeStore::failing_atomic_apply());
    let runtime_store: Arc<dyn RuntimeStore> = store.clone();
    let adapter = Arc::new(MeerkatMachine::persistent(
        runtime_store,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    let stop_called = Arc::new(AtomicBool::new(false));
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(SuccessExecutor {
                stop_called: Arc::clone(&stop_called),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");
    let degraded_registration = adapter
        .current_session_registration_witness(&sid)
        .await
        .expect("registered runtime should expose an exact registration witness");

    adapter
        .accept_input(&sid, make_prompt("loop boundary failure"))
        .await
        .unwrap();
    wait_for_atomic_bool(
        &stop_called,
        "boundary commit failures should stop the dead executor path",
    )
    .await;

    // Settle on the wedge itself rather than on the stop hook that precedes
    // it, so this test pins a state instead of a moment. Both readings are the
    // machine's own health vocabulary: the session owes a cold reload it
    // cannot mint, and it is still registered pointing at a runtime loop that
    // teardown refused to clear. Without this the test could race ahead of the
    // loop's exit and report a scheduling accident as a verdict.
    tokio::time::timeout(Duration::from_secs(10), async {
        while !(adapter.reload_required_session_count() == Some(1)
            && adapter.dead_runtime_loop_session_count() == Some(1))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect(
        "a failed boundary commit must leave exactly the wedge this test exists for: one \
         session owing a cold reload and still holding a dead runtime loop",
    );

    // The transient store fault is over. Durable truth is readable again, so a
    // cold reload can succeed - which is precisely why leaving the session
    // unusable would be a bookkeeping verdict rather than a real terminal.
    store.set_fail_atomic_apply(false);

    // The act under test, and the whole test: the ordinary registration entry
    // point, called exactly as a CLI resume or an RPC re-attach calls it.
    let recovered_stop_called = Arc::new(AtomicBool::new(false));
    tokio::time::timeout(
        Duration::from_secs(10),
        adapter.register_session_with_executor(
            sid.clone(),
            Box::new(SuccessExecutor {
                stop_called: Arc::clone(&recovered_stop_called),
            }),
        ),
    )
    .await
    .expect("registration must not park on a degraded shell's disposal protocol")
    .expect(
        "registration owns the cold reload a durability-degraded session demands; refusing it \
         here strands every non-mob session on a fault the machine can already recover from",
    );

    let cold_successor = adapter
        .current_session_registration_witness(&sid)
        .await
        .expect("the recovered registration must expose an exact witness");
    assert_ne!(
        cold_successor, degraded_registration,
        "recovery must publish a replacement registration cold-loaded from durable truth, not \
         republish the unreadable shell as healthy"
    );
    // `reload_required_session_count` is a HEALTH accessor: it reads the session
    // map with `try_read` and returns None when it cannot get a reading, so that
    // a health probe can never block the runtime it is measuring. None therefore
    // means "no measurement was taken", NOT "zero sessions owe a reload" - the
    // exact distinction this release exists to enforce - and a single sample
    // cannot prove anything about the successor. Sampling once made this test
    // fail roughly one run in three, always with `left: None`, which reads as
    // "the successor cannot execute" when it actually means "nobody looked".
    //
    // Await real readings from both nonblocking accessors, then assert on them.
    // A genuine nonzero still fails.
    let mut reload_required = None;
    let mut dead_runtime_loops = None;
    for _ in 0..200 {
        reload_required = adapter.reload_required_session_count();
        dead_runtime_loops = adapter.dead_runtime_loop_session_count();
        if reload_required.is_some() && dead_runtime_loops.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        reload_required,
        Some(0),
        "the successor must be able to execute against durable state"
    );
    assert_eq!(
        dead_runtime_loops,
        Some(0),
        "and it must carry a live runtime loop rather than inherit the corpse"
    );

    // The successor is a working session, not merely a registered one.
    adapter
        .accept_input(&sid, make_prompt("post-recovery turn"))
        .await
        .expect("the recovered session must accept ordinary work again");
}

#[tokio::test]
async fn persistent_machine_rejects_missing_durability_reload_cleanup_capability_before_publish() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError, CoreExecutorPostStopCleanupHandle,
    };
    use meerkat_core::lifecycle::run_primitive::RunPrimitive;

    struct UnsupportedCleanupHandle;

    #[async_trait::async_trait]
    impl CoreExecutorPostStopCleanupHandle for UnsupportedCleanupHandle {
        async fn cleanup_after_runtime_stop_terminalized(&self) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    struct UnsupportedPersistentExecutor;

    #[async_trait::async_trait]
    impl CoreExecutor for UnsupportedPersistentExecutor {
        fn machine_managed_post_stop_unregister(&self) -> bool {
            true
        }

        fn post_stop_cleanup_handle(&self) -> Option<Arc<dyn CoreExecutorPostStopCleanupHandle>> {
            Some(Arc::new(UnsupportedCleanupHandle))
        }

        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            unreachable!("unsupported persistent executor must not publish")
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            unreachable!("unsupported persistent executor must not publish")
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            unreachable!("unsupported persistent executor must not publish")
        }
    }

    let store = Arc::new(HarnessRuntimeStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent(
        store as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    let error = adapter
        .register_session_with_executor(sid.clone(), Box::new(UnsupportedPersistentExecutor))
        .await
        .expect_err("persistent attachment without degraded cleanup capability must fail");
    assert!(matches!(
        error,
        RuntimeDriverError::ValidationFailed { ref reason }
            if reason.contains("explicit non-terminal durability-reload cleanup capability")
    ));
    assert!(
        !adapter
            .session_has_executor(&sid)
            .await
            .expect("rejected registration has no executor"),
        "capability rejection must happen before executor publication"
    );
}

#[tokio::test]
async fn completed_boundary_commit_failure_marks_completion_authority_unavailable() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_runtime::completion::CompletionWaitError;

    struct SuccessExecutor;

    #[async_trait::async_trait]
    impl CoreExecutor for SuccessExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let store = Arc::new(HarnessRuntimeStore::failing_atomic_apply());
    let adapter = Arc::new(MeerkatMachine::persistent(store, memory_blob_store()));
    let sid = SessionId::new();
    adapter
        .register_session_with_executor(sid.clone(), Box::new(SuccessExecutor))
        .await
        .expect("runtime executor registration should succeed");

    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, make_prompt("loop boundary waiter failure"))
        .await
        .expect("accept should succeed");
    assert!(outcome.is_accepted());
    let handle = handle.expect("accepted input should expose a completion handle");

    let error = tokio::time::timeout(Duration::from_secs(1), handle.wait())
        .await
        .expect("completion waiter should resolve when the runtime loop exits")
        .expect_err("an uncertain durable boundary cannot mint a completion outcome");
    assert!(
        matches!(
            error,
            CompletionWaitError::AuthorityUnavailable(ref reason)
                if reason.contains("synthetic atomic service-turn commit failure")
        ),
        "boundary commit failure should fail closed with unavailable completion authority, got {error:?}"
    );
}

#[tokio::test]
async fn completed_run_runtime_loop_skips_terminal_lifecycle_snapshot_writer() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};

    struct SuccessExecutor {
        adapter: Arc<MeerkatMachine>,
        session_id: SessionId,
        stop_called: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for SuccessExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            self.stop_called.store(true, Ordering::SeqCst);
            self.adapter
                .unregister_session(&self.session_id)
                .await
                .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))?;
            Ok(())
        }
    }

    let store = Arc::new(HarnessRuntimeStore::failing_terminal_snapshot());
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    let stop_called = Arc::new(AtomicBool::new(false));
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(SuccessExecutor {
                adapter: Arc::clone(&adapter),
                session_id: sid.clone(),
                stop_called: Arc::clone(&stop_called),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, make_prompt("loop skips terminal lifecycle snapshot"))
        .await
        .unwrap();
    assert!(outcome.is_accepted());
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        handle
            .expect("accepted input should expose a completion handle")
            .wait(),
    )
    .await
    .expect("completion waiter should resolve")
    .expect("completion waiter should resolve");
    assert!(
        matches!(
            result,
            meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult
        ),
        "completed runtime loop should not trip the old terminal lifecycle writer, got {result:?}"
    );
    assert_eq!(
        store.commit_machine_lifecycle_calls(),
        1,
        "completed runtime loop must not use the old post-receipt lifecycle snapshot writer"
    );
    assert!(
        !stop_called.load(Ordering::SeqCst),
        "old terminal snapshot failure path should not stop the executor"
    );
}

// ─── Phase A gate tests ───

/// Gate A2: Dedup on terminal input returns (Deduplicated, None) — no hang.
#[tokio::test]
async fn dedup_terminal_input_returns_none_handle() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_core::types::{RunResult, Usage};
    use meerkat_runtime::identifiers::IdempotencyKey;

    struct ResultExecutor;
    #[async_trait::async_trait]
    impl CoreExecutor for ResultExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            let run_result = RunResult {
                text: "done".into(),
                session_id: SessionId::new(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            };
            Ok(CoreApplyOutput::with_run_result(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                run_result,
            ))
        }
        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session_with_executor(sid.clone(), Box::new(ResultExecutor))
        .await
        .expect("runtime executor registration should succeed");

    // Accept first input with idempotency key
    let key = IdempotencyKey::new("gate-a2");
    let mut input1 = make_prompt("first");
    if let Input::Prompt(ref mut p) = input1 {
        p.header.idempotency_key = Some(key.clone());
    }
    let (outcome1, handle1) = adapter
        .accept_input_with_completion(&sid, input1)
        .await
        .unwrap();
    assert!(outcome1.is_accepted());
    assert!(handle1.is_some(), "accepted input should have a handle");

    // Wait for it to complete
    let result = handle1
        .unwrap()
        .wait()
        .await
        .expect("completion waiter should resolve");
    assert!(
        matches!(
            result,
            meerkat_runtime::completion::CompletionOutcome::Completed(_)
        ),
        "first input should complete successfully"
    );

    // Now send duplicate — input is already terminal (Consumed)
    let mut input2 = make_prompt("duplicate");
    if let Input::Prompt(ref mut p) = input2 {
        p.header.idempotency_key = Some(key);
    }
    let (outcome2, handle2) = adapter
        .accept_input_with_completion(&sid, input2)
        .await
        .unwrap();
    assert!(
        outcome2.is_deduplicated(),
        "second input with same key should be deduplicated"
    );
    assert!(
        handle2.is_none(),
        "dedup on terminal input should return None handle"
    );
}

/// Gate A3: Dedup on in-flight input returns (Deduplicated, Some(handle))
/// that resolves when the original completes.
#[tokio::test]
async fn dedup_inflight_input_returns_handle_that_resolves() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_core::types::{RunResult, Usage};
    use meerkat_runtime::identifiers::IdempotencyKey;

    struct SlowExecutor;
    #[async_trait::async_trait]
    impl CoreExecutor for SlowExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            // Simulate slow execution so duplicate arrives while in-flight
            tokio::time::sleep(Duration::from_millis(200)).await;
            let run_result = RunResult {
                text: "slow done".into(),
                session_id: SessionId::new(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            };
            Ok(CoreApplyOutput::with_run_result(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                run_result,
            ))
        }
        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session_with_executor(sid.clone(), Box::new(SlowExecutor))
        .await
        .expect("runtime executor registration should succeed");

    // Accept first input with idempotency key
    let key = IdempotencyKey::new("gate-a3");
    let mut input1 = make_prompt("original");
    if let Input::Prompt(ref mut p) = input1 {
        p.header.idempotency_key = Some(key.clone());
    }
    let (outcome1, handle1) = adapter
        .accept_input_with_completion(&sid, input1)
        .await
        .unwrap();
    assert!(outcome1.is_accepted());

    // Wait briefly so the input is in-flight (Staged/Running), not yet terminal
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send duplicate while original is still running
    let mut input2 = make_prompt("duplicate");
    if let Input::Prompt(ref mut p) = input2 {
        p.header.idempotency_key = Some(key);
    }
    let (outcome2, handle2) = adapter
        .accept_input_with_completion(&sid, input2)
        .await
        .unwrap();
    assert!(
        outcome2.is_deduplicated(),
        "second input should be deduplicated"
    );
    assert!(
        handle2.is_some(),
        "dedup on in-flight input should return Some(handle)"
    );

    // Both handles should resolve when the original completes
    let result1 = handle1
        .unwrap()
        .wait()
        .await
        .expect("original completion waiter should resolve");
    let result2 = handle2
        .unwrap()
        .wait()
        .await
        .expect("deduplicated completion waiter should resolve");
    assert!(
        matches!(result1, meerkat_runtime::completion::CompletionOutcome::Completed(ref r) if r.text == "slow done"),
        "original handle should complete with result"
    );
    assert!(
        matches!(result2, meerkat_runtime::completion::CompletionOutcome::Completed(ref r) if r.text == "slow done"),
        "duplicate handle should also complete with same result"
    );
}

/// Gate A4 (part 1): resolve_without_result sends CompletedWithoutResult
/// when executor returns no terminal result.
#[tokio::test]
async fn completion_handle_resolves_without_result() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};

    struct NoResultExecutor;
    #[async_trait::async_trait]
    impl CoreExecutor for NoResultExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput::with_untyped_snapshot(
                // No RunResult
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }
        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session_with_executor(sid.clone(), Box::new(NoResultExecutor))
        .await
        .expect("runtime executor registration should succeed");

    let input = make_prompt("context append");
    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, input)
        .await
        .unwrap();
    assert!(outcome.is_accepted());

    let result = handle
        .unwrap()
        .wait()
        .await
        .expect("completion waiter should resolve");
    assert!(
        matches!(
            result,
            meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult
        ),
        "executor returning no terminal result should resolve as CompletedWithoutResult, got {result:?}"
    );
}

#[tokio::test]
async fn completion_handle_resolves_cancelled_executor_separately() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::RunPrimitive;

    struct CancelledExecutor;
    #[async_trait::async_trait]
    impl CoreExecutor for CancelledExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Err(CoreExecutorError::Cancelled)
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session_with_executor(sid.clone(), Box::new(CancelledExecutor))
        .await
        .expect("runtime executor registration should succeed");

    let input = make_prompt("cancelled");
    let input_id = input.id().clone();
    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, input)
        .await
        .unwrap();
    assert!(outcome.is_accepted());

    let result = handle
        .unwrap()
        .wait()
        .await
        .expect("completion waiter should resolve");
    assert!(
        matches!(
            result,
            meerkat_runtime::completion::CompletionOutcome::Cancelled
        ),
        "executor cancellation should resolve as Cancelled, got {result:?}"
    );

    let state = adapter
        .input_state(&sid, &input_id)
        .await
        .unwrap()
        .expect("cancelled input state should remain observable");
    assert_eq!(
        state.seed.phase,
        meerkat_runtime::InputLifecycleState::Abandoned,
        "cancelled run must not requeue staged contributors"
    );
    assert_eq!(
        state.seed.terminal_outcome,
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Cancelled,
        }),
        "cancelled run must preserve a cancellation-specific input terminal"
    );
    assert!(
        adapter.list_active_inputs(&sid).await.unwrap().is_empty(),
        "cancelled run must not leave active/requeued work"
    );
    let snapshot = adapter
        .meerkat_machine_spine_snapshot(&sid)
        .await
        .expect("snapshot should exist for cancelled runtime");
    assert_eq!(snapshot.control.phase, RuntimeState::Attached);
    let admitted = snapshot
        .inputs
        .admission_order
        .iter()
        .find(|entry| entry.input_id == input_id)
        .expect("cancelled input should remain in admission diagnostics");
    assert_eq!(
        admitted.lifecycle,
        Some(meerkat_runtime::InputLifecycleState::Abandoned)
    );
    assert_eq!(
        admitted.terminal_outcome.clone(),
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Cancelled,
        })
    );
}

#[tokio::test]
async fn persistent_cancelled_executor_persists_cancelled_terminal_not_failed_requeued() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::RunPrimitive;

    struct CancelledExecutor;
    #[async_trait::async_trait]
    impl CoreExecutor for CancelledExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Err(CoreExecutorError::Cancelled)
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let store = Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    let runtime_id = LogicalRuntimeId::for_session(&sid);
    adapter
        .register_session_with_executor(sid.clone(), Box::new(CancelledExecutor))
        .await
        .expect("runtime executor registration should succeed");

    let input = make_prompt("persistent cancelled");
    let input_id = input.id().clone();
    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, input)
        .await
        .unwrap();
    assert!(outcome.is_accepted());

    let result = handle
        .unwrap()
        .wait()
        .await
        .expect("completion waiter should resolve");
    assert!(matches!(
        result,
        meerkat_runtime::completion::CompletionOutcome::Cancelled
    ));

    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Attached,
        "cancelled persistent run should publish pre-run phase after durable commit"
    );
    assert_eq!(
        load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .unwrap(),
        Some(RuntimeState::Idle),
        "persistent storage maps live Attached back to Idle for recovery"
    );
    let stored = store
        .load_input_state(&runtime_id, &input_id)
        .await
        .unwrap()
        .expect("cancelled input state should be durable");
    assert_eq!(
        stored.seed.phase,
        meerkat_runtime::InputLifecycleState::Abandoned
    );
    assert_eq!(
        stored.seed.terminal_outcome,
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Cancelled,
        }),
        "durable input terminal must be cancelled, not failed/requeued"
    );
    assert!(
        adapter.list_active_inputs(&sid).await.unwrap().is_empty(),
        "persistent cancelled run must not requeue staged contributors"
    );
}

/// Gate A5: reset_runtime resolves all pending waiters.
#[tokio::test]
async fn reset_runtime_resolves_pending_waiters() {
    // Register without executor so inputs queue but don't process
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");

    let input = make_prompt("pending");
    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, input)
        .await
        .unwrap();
    assert!(outcome.is_accepted());
    assert!(handle.is_some());

    // Reset the runtime
    adapter.reset_runtime(&sid).await.unwrap();

    // Handle should resolve as terminated
    let result = handle
        .unwrap()
        .wait()
        .await
        .expect("completion waiter should resolve");
    assert!(
        matches!(
            result,
            meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated { .. }
        ),
        "reset should resolve pending waiters as terminated, got {result:?}"
    );
}

/// Gate A6: retire_runtime without loop resolves waiters.
#[tokio::test]
async fn retire_without_loop_resolves_waiters() {
    // Register without executor (no RuntimeLoop)
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");

    let input = make_prompt("will be retired");
    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, input)
        .await
        .unwrap();
    assert!(outcome.is_accepted());
    assert!(handle.is_some());

    // Retire without loop attached
    adapter.retire_runtime(&sid).await.unwrap();

    // Handle should resolve as terminated since no loop will drain
    let result = handle
        .unwrap()
        .wait()
        .await
        .expect("completion waiter should resolve");
    assert!(
        matches!(
            result,
            meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated { .. }
        ),
        "retire without loop should resolve pending waiters as terminated, got {result:?}"
    );
}

#[tokio::test]
async fn unregister_session_aborts_spawned_drain_and_clears_suppression() {
    use meerkat_core::agent::CommsRuntime;
    use tokio::sync::Notify;

    struct IdleDrainRuntime {
        notify: Arc<Notify>,
    }

    impl IdleDrainRuntime {
        fn new() -> Self {
            Self {
                notify: Arc::new(Notify::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl CommsRuntime for IdleDrainRuntime {
        fn inbox_notify(&self) -> Arc<Notify> {
            Arc::clone(&self.notify)
        }

        async fn claim_classified_inbox_interaction(
            &self,
        ) -> Result<
            Option<meerkat_core::interaction::PeerIngressQueueClaim>,
            meerkat_core::agent::CommsCapabilityError,
        > {
            Ok(None)
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");

    let comms: Arc<dyn CommsRuntime> = Arc::new(IdleDrainRuntime::new());
    let spawned = adapter
        .update_peer_ingress_context(&sid, true, Some(comms))
        .await
        .expect("update peer ingress context");
    assert!(spawned, "registered host-mode session should spawn a drain");

    // Give the drain task time to start before unregistering.
    tokio::time::sleep(Duration::from_millis(50)).await;

    adapter
        .unregister_session(&sid)
        .await
        .expect("session should unregister cleanly");
    // The session was just unregistered; the wait verdict may be a typed
    // NotFound rejection from the machine rather than success — both prove
    // the drain is no longer running, which is what this test pins below.
    let _ = adapter.wait_comms_drain(&sid).await;
    // Ephemeral unregister fully removes the session: there is no durable store
    // to retain a Destroyed marker (unlike the persistent cold-reregister and
    // recovery-contract paths, which DO preserve canonical Destroyed truth via
    // the store). A subsequent runtime_state lookup therefore resolves to
    // NotFound — the session is gone — rather than a retained NotReady/Destroyed.
    assert!(matches!(
        adapter.runtime_state(&sid).await,
        Err(RuntimeDriverError::NotFound { .. })
    ));
}

#[tokio::test]
async fn idle_non_host_sessions_do_not_spawn_background_comms_drains() {
    use meerkat_core::agent::CommsRuntime;
    use tokio::sync::Notify;

    struct IdleDrainRuntime {
        notify: Arc<Notify>,
    }

    impl IdleDrainRuntime {
        fn new() -> Self {
            Self {
                notify: Arc::new(Notify::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl CommsRuntime for IdleDrainRuntime {
        fn inbox_notify(&self) -> Arc<Notify> {
            Arc::clone(&self.notify)
        }

        async fn claim_classified_inbox_interaction(
            &self,
        ) -> Result<
            Option<meerkat_core::interaction::PeerIngressQueueClaim>,
            meerkat_core::agent::CommsCapabilityError,
        > {
            Ok(None)
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");

    let comms: Arc<dyn CommsRuntime> = Arc::new(IdleDrainRuntime::new());
    let spawned = adapter
        .update_peer_ingress_context(&sid, false, Some(comms))
        .await
        .expect("update peer ingress context");

    assert!(
        !spawned,
        "idle non-host sessions must not leave a background comms drain alive"
    );
}

#[tokio::test]
async fn attached_sessions_do_not_spawn_comms_drains_without_keep_alive() {
    use meerkat_core::agent::CommsRuntime;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use tokio::sync::Notify;

    struct IdleDrainRuntime {
        notify: Arc<Notify>,
    }

    impl IdleDrainRuntime {
        fn new() -> Self {
            Self {
                notify: Arc::new(Notify::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl CommsRuntime for IdleDrainRuntime {
        fn inbox_notify(&self) -> Arc<Notify> {
            Arc::clone(&self.notify)
        }

        async fn claim_classified_inbox_interaction(
            &self,
        ) -> Result<
            Option<meerkat_core::interaction::PeerIngressQueueClaim>,
            meerkat_core::agent::CommsCapabilityError,
        > {
            Ok(None)
        }
    }

    struct NoopExecutor;

    #[async_trait::async_trait]
    impl CoreExecutor for NoopExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session_with_executor(sid.clone(), Box::new(NoopExecutor))
        .await
        .expect("runtime executor registration should succeed");

    let comms: Arc<dyn CommsRuntime> = Arc::new(IdleDrainRuntime::new());
    let spawned = adapter
        .update_peer_ingress_context(&sid, false, Some(comms))
        .await
        .expect("update peer ingress context");

    assert!(
        !spawned,
        "attached sessions should not spawn a comms drain when keep_alive is disabled"
    );

    adapter
        .unregister_session(&sid)
        .await
        .expect("session should unregister cleanly");
}

/// Test that BoundaryApplied fires with correct receipt on success.
#[tokio::test]
async fn successful_execution_fires_boundary_applied() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_runtime::input_state::InputLifecycleState;

    struct SuccessExecutor;

    #[async_trait::async_trait]
    impl CoreExecutor for SuccessExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter
        .register_session_with_executor(sid.clone(), Box::new(SuccessExecutor))
        .await
        .expect("runtime executor registration should succeed");

    let input = make_prompt("hello success");
    let input_id = input.id().clone();
    adapter.accept_input(&sid, input).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Input should have gone through full lifecycle: Queued → Staged → Applied → APC → Consumed
    let is = adapter.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(
        is.seed.phase,
        InputLifecycleState::Consumed,
        "Successful execution should consume the input"
    );

    // Runtime should be back to Attached (executor still connected)
    let state = adapter.runtime_state(&sid).await.unwrap();
    assert_eq!(state, RuntimeState::Attached);
}

// --- session_has_executor tests ---

#[tokio::test]
async fn registered_session_is_not_executor_ready() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    let _bindings = adapter.prepare_bindings(sid.clone()).await.unwrap();

    assert!(
        adapter.contains_session(&sid).await,
        "prepare_bindings should register the session"
    );
    assert!(
        !adapter
            .session_has_executor(&sid)
            .await
            .expect("session_has_executor should resolve"),
        "prepare_bindings alone should not attach an executor"
    );
}

#[tokio::test]
async fn executor_attached_session_is_executor_ready() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};

    struct NoopExecutor;
    #[async_trait::async_trait]
    impl CoreExecutor for NoopExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput::with_untyped_snapshot(
                RunBoundaryReceiptDraft {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ))
        }
        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    let _bindings = adapter.prepare_bindings(sid.clone()).await.unwrap();

    assert!(
        !adapter
            .session_has_executor(&sid)
            .await
            .expect("session_has_executor should resolve"),
        "before executor attachment"
    );

    adapter
        .ensure_session_with_executor(sid.clone(), Box::new(NoopExecutor))
        .await
        .expect("runtime executor registration should succeed");

    assert!(
        adapter
            .session_has_executor(&sid)
            .await
            .expect("session_has_executor should resolve"),
        "after executor attachment"
    );
}

#[tokio::test]
async fn session_has_executor_false_for_unknown() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let unknown = SessionId::new();
    assert!(
        !adapter
            .session_has_executor(&unknown)
            .await
            .expect("session_has_executor should resolve"),
        "unknown session should not have an executor"
    );
}

/// Executor that drives a real turn to its completed terminal and only then
/// fails its runtime completion.
///
/// Nothing about the machine state is hand-built. The executor applies the same
/// generated turn inputs an agent applies (`PrimitiveApplied` ->
/// `LlmReturnedToolCalls(0)` -> `BoundaryComplete`) through the session's own
/// `TurnStateHandle` from `prepare_bindings`, so `BoundaryCompleteCompleted`
/// fires on the exact shared authority the runtime loop reads. The failure it
/// then returns is what an ordinary run-boundary persistence failure looks like
/// from the loop's side: `AgentEvent::TurnCompleted` has already been published
/// to the host, the model call and the tools have already run, and only the
/// save after them failed.
struct CompletedTerminalThenFailingExecutor {
    turn_state: Arc<dyn meerkat_core::handles::TurnStateHandle>,
    calls: Arc<AtomicUsize>,
}

const COMPLETED_TERMINAL_FAILURE_DETAIL: &str =
    "run-boundary session save failed after the turn completed";

#[async_trait::async_trait]
impl meerkat_core::lifecycle::CoreExecutor for CompletedTerminalThenFailingExecutor {
    async fn apply(
        &mut self,
        run_id: RunId,
        _primitive: meerkat_core::lifecycle::run_primitive::RunPrimitive,
    ) -> Result<
        meerkat_core::lifecycle::core_executor::CoreApplyOutput,
        meerkat_core::lifecycle::core_executor::CoreExecutorError,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.turn_state
            .primitive_applied(run_id.clone())
            .expect("the runtime loop signals turn start before apply, so the primitive applies");
        self.turn_state
            .llm_returned_tool_calls(run_id.clone(), 0)
            .expect("a zero-tool-call answer must reach the draining boundary");
        self.turn_state
            .apply_turn_input(
                meerkat_core::turn_execution_authority::TurnExecutionInput::BoundaryComplete {
                    run_id,
                },
            )
            .expect("the completed boundary must reach the machine");
        Err(
            meerkat_core::lifecycle::core_executor::CoreExecutorError::apply_failed_runtime_turn(
                COMPLETED_TERMINAL_FAILURE_DETAIL,
            ),
        )
    }

    async fn cancel_after_boundary(
        &mut self,
        _reason: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(
        &mut self,
        _reason: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        Ok(())
    }
}

async fn completed_terminal_then_failing_session(
    calls: Arc<AtomicUsize>,
) -> (Arc<MeerkatMachine>, SessionId) {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    // The production binding route: the executor drives the same handle a
    // factory-built agent would be handed for this session.
    let bindings = adapter
        .prepare_bindings(sid.clone())
        .await
        .expect("session runtime bindings should prepare");
    let turn_state = Arc::clone(bindings.turn_state());
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(CompletedTerminalThenFailingExecutor { turn_state, calls }),
        )
        .await
        .expect("runtime executor registration should succeed");
    (adapter, sid)
}

/// Field regression (household fleet, 0.8.23). A run whose turn reached a
/// `Completed` terminal and whose runtime-level completion then failed was
/// refused as an incoherent terminal pair. That refusal corrupted the recovery
/// carrier, which ended the runtime loop task, and the session could not run
/// another turn for the remaining life of the process. The host had already
/// been told `AgentEvent::TurnCompleted`, and the completion waiter was never
/// resolved, so a watchdog keyed on turn completion was actively defeated
/// rather than merely uninformed.
#[tokio::test]
async fn completed_terminal_then_failed_runtime_completion_resolves_a_typed_terminal() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (adapter, sid) = completed_terminal_then_failing_session(Arc::clone(&calls)).await;

    let (outcome, handle) = adapter
        .accept_input_with_completion(
            &sid,
            make_prompt("the turn completes and only its persistence fails"),
        )
        .await
        .expect("input should be accepted");
    assert!(outcome.is_accepted());

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        handle
            .expect("accepted input should carry a completion handle")
            .wait(),
    )
    .await
    .expect(
        "a run that completed its turn and then failed its runtime completion must still resolve \
         its caller; hanging here is the field defect",
    )
    .expect("completion waiter should resolve");

    match result {
        meerkat_runtime::completion::CompletionOutcome::AbandonedWithError { reason, error } => {
            assert_eq!(
                error.kind,
                meerkat_core::TurnTerminalCauseKind::RuntimeApplyFailure,
                "the host must receive the typed runtime-apply failure, not silence behind an \
                 AgentEvent::TurnCompleted that lied"
            );
            assert!(
                reason.contains(COMPLETED_TERMINAL_FAILURE_DETAIL),
                "the typed terminal must carry the executor's own failure cause, got {reason}"
            );
        }
        other => panic!("expected a typed runtime-apply-failure terminal, got {other:?}"),
    }
}

/// The carrier fix removes the accidental guard that used to suppress replay:
/// before it, the loop died at the corrupt carrier and never reached the
/// failed-batch backlog. `failed_run_contributor_disposition` classifies from
/// the typed error alone, so an ordinary post-turn persistence failure is
/// `Replayed` and its contributor returns to the work lane. Restaging it
/// re-appends the same content and re-runs the whole turn - a second model call
/// with tools free to fire again, for work whose effects already happened.
///
/// The run must therefore be handed off rather than retried.
///
/// The replay window is closed by a positive signal, never by a sleep. A sleep
/// does not bound something that has not happened yet; it only bounds how long
/// the test was willing to wait for it. The version of this test that slept
/// 400ms did detect the regression, but not because of the sleep: on a build
/// without the teardown routing the restages all land before the completion
/// waiter resolves, so the count was already wrong when the sleep started.
/// Nothing in the test asserted that ordering, and it is not a contract - move
/// the caller's resolution earlier and the same assertion becomes a pure race
/// against a wall clock.
///
/// The signal used instead is the machine's own retirement of the session: once
/// the `MeerkatMachine` no longer holds a registration for this session there
/// is no runtime loop, no executor and no work lane left that could stage the
/// contributor, so the apply count is final rather than merely not-yet-moved.
/// `ListActiveInputs` reports that absence as
/// `RuntimeDriverError::NotReady { state: RuntimeState::Destroyed }`.
///
/// That signal cannot be satisfied early on a regressed build, and this was
/// measured rather than assumed. With the teardown routing removed, the session
/// stays registered and `Attached` with its executor still claimed for as long
/// as the test polls, so the wait cannot pass; the build without the fix can
/// only go red. Load moves how long the red takes to arrive, never whether it
/// arrives.
#[tokio::test]
async fn completed_terminal_then_failed_runtime_completion_refuses_to_replay_the_turn() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (adapter, sid) = completed_terminal_then_failing_session(Arc::clone(&calls)).await;

    let (outcome, handle) = adapter
        .accept_input_with_completion(
            &sid,
            make_prompt("one household instruction, executed exactly once"),
        )
        .await
        .expect("input should be accepted");
    assert!(outcome.is_accepted());

    let _ = tokio::time::timeout(
        Duration::from_secs(5),
        handle
            .expect("accepted input should carry a completion handle")
            .wait(),
    )
    .await
    .expect("the caller must be resolved before the replay window is measured");

    // Generous on purpose: a longer bound can only delay a pass, and the only
    // way to exceed it is for the retirement never to happen, which is the
    // regression itself.
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            // Fail on the replay itself rather than on the wait expiring, so a
            // regressed build reports the count that proves the defect instead
            // of a bare timeout.
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "the turn was executed again while the shell was still being retired; a run \
                 whose effects already ran must never re-enter the work lane"
            );
            match adapter.list_active_inputs(&sid).await {
                // The session registration is gone: nothing is left that could
                // stage the contributor, so the apply count below is final.
                Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                }) => break,
                // Still registered, teardown in flight, or mid-lifecycle. Keep
                // waiting for the retirement rather than sampling a clock.
                Ok(_) | Err(RuntimeDriverError::NotReady { .. }) => {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                Err(other) => {
                    panic!("unexpected error while waiting for the shell to be retired: {other}")
                }
            }
        }
    })
    .await
    .expect(
        "the post-mutation failure must retire the session instead of releasing its contributor \
         to the retry lane; a session still registered here means the turn is queued to run a \
         second time",
    );

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a turn whose effects already ran must not be executed a second time"
    );
}

/// What a host experiences on the far side of the fail-closed handoff.
///
/// The post-mutation failure hands the runtime loop off to the unregister saga
/// instead of retrying the turn, so the measurable question is whether the
/// shell is actually retired and whether an ordinary host - one with no mob
/// provisioner to call a recovery/discard route for it - can rebuild the
/// session afterwards. Wedging here would be the same class of defect as the
/// one being fixed, one branch later.
///
/// This also measures, rather than assumes, what the still-queued contributor
/// does on the rebuilt session: that number is the deferred replay this fix
/// does not close.
#[tokio::test]
async fn completed_terminal_then_failed_runtime_completion_retires_the_shell_and_rebuilds() {
    let store = Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let adapter = Arc::new(MeerkatMachine::persistent(
        Arc::clone(&store) as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    let bindings = adapter
        .prepare_bindings(sid.clone())
        .await
        .expect("session runtime bindings should prepare");
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(CompletedTerminalThenFailingExecutor {
                turn_state: Arc::clone(bindings.turn_state()),
                calls: Arc::clone(&calls),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, make_prompt("persisted household instruction"))
        .await
        .expect("input should be accepted");
    assert!(outcome.is_accepted());
    // Kept so the contributor can be read back by identity below. Its
    // machine-owned stage-attempt count is what makes the "no replay"
    // assertions final instead of merely early.
    let contributor_input_id = match &outcome {
        meerkat_runtime::accept::AcceptOutcome::Accepted { input_id, .. } => input_id.clone(),
        other => panic!("an accepted input is the premise of this test, got {other:?}"),
    };
    let _ = tokio::time::timeout(
        Duration::from_secs(5),
        handle
            .expect("accepted input should carry a completion handle")
            .wait(),
    )
    .await
    .expect("the caller must be resolved before teardown is observed");

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if !adapter
                .session_has_executor(&sid)
                .await
                .expect("session_has_executor should resolve")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect(
        "the fail-closed handoff must retire the exact executor; holding it is the wedge this \
         change exists to remove",
    );

    // An ordinary host rebuilds by asking for bindings again. There is no mob
    // provisioner in this shape, so this is the whole recovery affordance.
    let rebuilt_calls = Arc::new(AtomicUsize::new(0));
    let rebuilt = Arc::clone(&adapter);
    let mut last_rebind_error = None;
    let rebuilt_bindings = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match rebuilt.prepare_bindings(sid.clone()).await {
                Ok(bindings) => break bindings,
                Err(error) => {
                    last_rebind_error = Some(error.to_string());
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "an ordinary host must be able to rebind the torn-down session; last error: {last_rebind_error:?}"
        )
    });
    rebuilt
        .register_session_with_executor(
            sid.clone(),
            Box::new(CompletedTerminalThenFailingExecutor {
                turn_state: Arc::clone(rebuilt_bindings.turn_state()),
                calls: Arc::clone(&rebuilt_calls),
            }),
        )
        .await
        .expect("a cold host must be able to re-attach an executor");

    // Positive signal, not a sleep. The contributor's own terminal is the fact
    // that closes the replay window: an input in a terminal lifecycle phase
    // cannot be staged again, so no later apply can be pending behind this
    // read. `attempt_count` is the machine's own count of stage attempts, so a
    // replay cannot hide inside the wait either - restaging would have to move
    // the input back through `Staged` and leave the count above 1. Waiting on a
    // clock instead would assert that nothing happened yet, which is not the
    // same claim and is satisfied by a machine that is merely busy.
    let contributor = wait_for_input_state_within(
        &rebuilt,
        &sid,
        &contributor_input_id,
        Duration::from_secs(20),
        "the already-executed contributor must reach a machine-owned terminal; while it is \
         non-terminal it is still stageable and no count read here is final",
        |state| state.seed.phase == meerkat_runtime::InputLifecycleState::Abandoned,
    )
    .await;
    assert_eq!(
        contributor.seed.attempt_count, 1,
        "the contributor was staged more than once; the machine's own attempt count is the \
         witness that the turn whose effects already ran was handed off rather than replayed"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the retired shell must not have retried the turn whose effects already ran"
    );
    assert!(
        rebuilt
            .list_active_inputs(&sid)
            .await
            .expect("the rebuilt session should report its active work")
            .is_empty(),
        "the teardown must not leave the already-executed contributor as active work for the \
         rebuilt session to pick up"
    );
    assert_eq!(
        rebuilt_calls.load(Ordering::SeqCst),
        0,
        "the rebuilt session must not re-execute the turn the torn-down shell already ran"
    );
}

/// Baseline for the cold-rebuild question the sibling test raises. A session
/// unregistered through the ordinary public route, with no runtime-loop failure
/// anywhere near it, behaves the same way under a second `MeerkatMachine` over
/// the same store. Whatever that behaviour is, it is a property of cold
/// re-registration and not of the post-mutation teardown routing.
#[tokio::test]
async fn cold_rebind_after_plain_unregister_is_the_pre_existing_baseline() {
    let store = Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent(
        Arc::clone(&store) as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    adapter
        .prepare_bindings(sid.clone())
        .await
        .expect("session runtime bindings should prepare");
    adapter
        .unregister_session(&sid)
        .await
        .expect("ordinary unregister should succeed");

    let cold = Arc::new(MeerkatMachine::persistent(
        Arc::clone(&store) as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    assert!(
        cold.prepare_bindings(sid.clone()).await.is_ok(),
        "a second host over the same store must be able to bind an unregistered session"
    );
    assert!(
        adapter.prepare_bindings(sid.clone()).await.is_ok(),
        "the same host must be able to rebind a session it unregistered"
    );
}

/// The terminal publication capability a directed input requires. A session
/// with active directed work refuses to abandon it without one, so a directed
/// shape cannot be built without recording what is published.
#[derive(Default)]
struct RecordingTerminalPublisher {
    events: std::sync::Mutex<Vec<meerkat_core::event::AgentEvent>>,
}

impl RecordingTerminalPublisher {
    fn events(&self) -> Vec<meerkat_core::event::AgentEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl meerkat_core::lifecycle::CoreExecutorPublicationHandle for RecordingTerminalPublisher {
    async fn publish_interaction_terminals(
        &self,
        events: &[meerkat_core::event::AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        meerkat_core::lifecycle::core_executor::CoreExecutorError,
    > {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(events);
        events
            .iter()
            .enumerate()
            .map(|(index, event)| {
                meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt::try_new(
                    event,
                    index as u64 + 1,
                )
            })
            .collect()
    }
}

/// [`CompletedTerminalThenFailingExecutor`] with the publication capability a
/// directed input needs. Same turn, same failure, same detail.
struct DirectedCompletedTerminalThenFailingExecutor {
    turn_state: Arc<dyn meerkat_core::handles::TurnStateHandle>,
    calls: Arc<AtomicUsize>,
    publisher: Arc<RecordingTerminalPublisher>,
}

#[async_trait::async_trait]
impl meerkat_core::lifecycle::CoreExecutor for DirectedCompletedTerminalThenFailingExecutor {
    fn publication_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>> {
        Some(Arc::clone(&self.publisher) as Arc<_>)
    }

    async fn publish_interaction_terminals(
        &mut self,
        events: &[meerkat_core::event::AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        meerkat_core::lifecycle::core_executor::CoreExecutorError,
    > {
        meerkat_core::lifecycle::CoreExecutorPublicationHandle::publish_interaction_terminals(
            self.publisher.as_ref(),
            events,
        )
        .await
    }

    async fn apply(
        &mut self,
        run_id: RunId,
        _primitive: meerkat_core::lifecycle::run_primitive::RunPrimitive,
    ) -> Result<
        meerkat_core::lifecycle::core_executor::CoreApplyOutput,
        meerkat_core::lifecycle::core_executor::CoreExecutorError,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.turn_state
            .primitive_applied(run_id.clone())
            .expect("the runtime loop signals turn start before apply, so the primitive applies");
        self.turn_state
            .llm_returned_tool_calls(run_id.clone(), 0)
            .expect("a zero-tool-call answer must reach the draining boundary");
        self.turn_state
            .apply_turn_input(
                meerkat_core::turn_execution_authority::TurnExecutionInput::BoundaryComplete {
                    run_id,
                },
            )
            .expect("the completed boundary must reach the machine");
        Err(
            meerkat_core::lifecycle::core_executor::CoreExecutorError::apply_failed_runtime_turn(
                COMPLETED_TERMINAL_FAILURE_DETAIL,
            ),
        )
    }

    async fn cancel_after_boundary(
        &mut self,
        _reason: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(
        &mut self,
        _reason: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        Ok(())
    }
}

/// What a DIRECTED caller observes on the post-completion failure class, held
/// to what the code does rather than to what would be nice.
///
/// The two recipient classes do not observe the same thing, and the difference
/// is not cosmetic. A non-directed waiter is resolved by the failed run itself
/// and receives `AbandonedWithError` carrying
/// `TurnTerminalCauseKind::RuntimeApplyFailure` and the executor's own
/// apply-failure detail (the sibling test above asserts exactly that).
///
/// A directed caller receives neither. `failed_run_contributor_disposition`
/// classifies an ordinary post-turn persistence failure as `Replayed`, so the
/// contributor is requeued rather than terminalized; the failed-run realization
/// therefore computes an empty terminal recipient set and stages no interaction
/// terminal outbox for the run at all. What the directed caller eventually sees
/// is produced by the teardown that follows, not by the run: the unregister
/// path stages a runless runtime-termination batch, so the interaction terminal
/// is `InteractionFailed { reason: Abandoned { detail: "runtime session
/// unregistered" } }` and the completion waiter resolves `RuntimeTerminated`
/// with `TurnTerminalCauseKind::FatalFailure`.
///
/// The negative assertions are the point. A directed caller keying off the
/// typed cause cannot distinguish this from any other teardown, and the
/// operator-facing detail that names the actual failure never reaches it. That
/// gap is real and is recorded here rather than described as delivered.
#[tokio::test]
async fn directed_caller_on_post_completion_failure_observes_teardown_not_the_run_terminal() {
    let store = Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let publisher = Arc::new(RecordingTerminalPublisher::default());
    let adapter = Arc::new(MeerkatMachine::persistent(
        Arc::clone(&store) as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    let bindings = adapter
        .prepare_bindings(sid.clone())
        .await
        .expect("session runtime bindings should prepare");
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(DirectedCompletedTerminalThenFailingExecutor {
                turn_state: Arc::clone(bindings.turn_state()),
                calls: Arc::clone(&calls),
                publisher: Arc::clone(&publisher),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    let interaction_uuid = meerkat_core::time_compat::new_uuid_v7();
    let input = meerkat_runtime::mob_adapter::create_tracked_flow_step_input(
        "directed-step",
        meerkat_core::types::ContentInput::Text("directed household instruction".to_string()),
        "directed-flow",
        None,
        &interaction_uuid.to_string(),
    )
    .expect("a tracked flow step is the ordinary directed shape");
    let input_id = input.id().clone();
    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, input)
        .await
        .expect("input should be accepted");
    assert!(outcome.is_accepted());

    // Generous on purpose: a longer bound can only delay a pass, and the only
    // way to exceed it is for the directed caller never to be resolved, which
    // would itself be the wedge.
    let resolved = tokio::time::timeout(
        Duration::from_secs(20),
        handle
            .expect("accepted directed input should carry a completion handle")
            .wait(),
    )
    .await
    .expect("a directed caller must be resolved by the teardown; hanging here is a wedge")
    .expect("completion waiter should resolve");

    match resolved {
        meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated { reason, error } => {
            assert_eq!(
                reason, "runtime session unregistered",
                "the directed caller is resolved by the teardown, not by the failed run"
            );
            assert_eq!(
                error.kind,
                meerkat_core::TurnTerminalCauseKind::FatalFailure,
                "the directed caller does not receive the run's RuntimeApplyFailure cause"
            );
            assert!(
                !reason.contains(COMPLETED_TERMINAL_FAILURE_DETAIL),
                "the executor's own failure detail does not travel to the directed caller"
            );
        }
        other => panic!(
            "directed callers observe runtime termination on this class, not a run terminal; got \
             {other:?}"
        ),
    }

    let events = publisher.events();
    assert_eq!(
        events.len(),
        1,
        "exactly one interaction terminal reaches the directed caller, and it comes from the \
         teardown; got {events:?}"
    );
    match &events[0] {
        meerkat_core::event::AgentEvent::InteractionFailed {
            reason: meerkat_core::event::InteractionFailureReason::Abandoned { detail },
            ..
        } => {
            assert_eq!(
                detail, "runtime session unregistered",
                "the abandonment names the teardown, not the run-boundary persistence failure"
            );
            assert!(
                !detail.contains(COMPLETED_TERMINAL_FAILURE_DETAIL),
                "no interaction terminal outbox is staged for the failed run, so its detail \
                 cannot be what the directed caller reads"
            );
        }
        other => panic!("unexpected directed interaction terminal: {other:?}"),
    }

    let stored = adapter
        .input_state(&sid, &input_id)
        .await
        .expect("the directed input state should load")
        .expect("the directed input should still be recorded");
    assert_eq!(
        stored.seed.attempt_count, 1,
        "the directed contributor must not have been staged a second time either"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a turn whose effects already ran must not be executed a second time"
    );
}
