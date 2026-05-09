#![allow(
    clippy::expect_used,
    clippy::large_futures,
    clippy::panic,
    clippy::unwrap_used
)]

use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use chrono::Utc;
use meerkat_core::BlobStore;
use meerkat_core::lifecycle::{
    InputId, RunBoundaryReceipt, RunId, run_primitive::RunApplyBoundary,
};
use meerkat_core::types::SessionId;
use meerkat_runtime::input_state::{InputAbandonReason, InputTerminalOutcome, StoredInputState};
use meerkat_runtime::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, LogicalRuntimeId,
    MeerkatMachine, PromptInput, RuntimeDriverError, RuntimeState, RuntimeStore, RuntimeStoreError,
    SessionDelta, SessionServiceRuntimeExt,
};
use meerkat_store::MemoryBlobStore;

fn memory_blob_store() -> Arc<dyn BlobStore> {
    Arc::new(MemoryBlobStore::new())
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
    tokio::time::timeout(Duration::from_secs(2), async {
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
    fail_atomic_apply: bool,
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
    runtime_state_overrides: Mutex<HashMap<LogicalRuntimeId, RuntimeState>>,
}

impl HarnessRuntimeStore {
    fn new() -> Self {
        Self {
            inner: meerkat_runtime::store::InMemoryRuntimeStore::new(),
            fail_atomic_apply: false,
            fail_commit_machine_lifecycle_now: AtomicBool::new(false),
            fail_commit_machine_lifecycle_after: None,
            delay_commit_machine_lifecycle_after: None,
            commit_machine_lifecycle_delay: Duration::ZERO,
            commit_machine_lifecycle_calls: AtomicUsize::new(0),
            load_input_states_delay: Duration::ZERO,
            fail_persist_input_state_after: None,
            persist_input_state_calls: AtomicUsize::new(0),
            runtime_state_overrides: Mutex::new(HashMap::new()),
        }
    }

    fn failing_atomic_apply() -> Self {
        Self {
            fail_atomic_apply: true,
            ..Self::new()
        }
    }

    fn delayed_recover(delay: Duration) -> Self {
        Self {
            load_input_states_delay: delay,
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

#[async_trait::async_trait]
impl RuntimeStore for HarnessRuntimeStore {
    async fn commit_session_snapshot(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        session_delta: SessionDelta,
    ) -> Result<(), RuntimeStoreError> {
        self.inner
            .commit_session_snapshot(runtime_id, session_delta)
            .await
    }

    async fn atomic_apply(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        session_delta: Option<SessionDelta>,
        receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
        input_updates: Vec<StoredInputState>,
        session_store_key: Option<meerkat_core::types::SessionId>,
    ) -> Result<(), RuntimeStoreError> {
        if self.fail_atomic_apply {
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

    async fn load_input_states(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
    ) -> Result<Vec<StoredInputState>, RuntimeStoreError> {
        if !self.load_input_states_delay.is_zero() {
            tokio::time::sleep(self.load_input_states_delay).await;
        }
        self.inner.load_input_states(runtime_id).await
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
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        self.inner.load_session_snapshot(runtime_id).await
    }

    async fn persist_input_state(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        state: &StoredInputState,
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

    async fn load_input_state(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, RuntimeStoreError> {
        self.inner.load_input_state(runtime_id, input_id).await
    }

    async fn load_runtime_state(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
    ) -> Result<Option<RuntimeState>, RuntimeStoreError> {
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
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        commit: meerkat_runtime::store::MachineLifecycleCommit,
        input_states: &[StoredInputState],
    ) -> Result<(), RuntimeStoreError> {
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
        self.inner
            .commit_machine_lifecycle(runtime_id, commit, input_states)
            .await
    }

    async fn persist_ops_lifecycle(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        snapshot: &meerkat_runtime::PersistedOpsSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        self.inner.persist_ops_lifecycle(runtime_id, snapshot).await
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
    adapter.register_session(sid.clone()).await;

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
    adapter.register_session(sid.clone()).await;

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
    adapter.register_session(sid.clone()).await;

    let input = make_prompt("hello");
    let outcome = adapter.accept_input(&sid, input).await.unwrap();
    assert!(outcome.is_accepted());
}

#[tokio::test]
async fn lifecycle_commit_failure_restores_staged_session_dsl_state() {
    let store = Arc::new(HarnessRuntimeStore::failing_lifecycle_commit());
    let adapter = Arc::new(MeerkatMachine::persistent(store, memory_blob_store()));
    let sid = SessionId::new();
    adapter.register_session(sid.clone()).await;
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
        "failed retire must restore the session DSL phase",
    );
    assert_eq!(
        adapter.list_active_inputs(&sid).await.unwrap(),
        vec![input_id],
        "failed retire must restore active input projection",
    );
}

#[tokio::test]
async fn destroy_lifecycle_commit_failure_restores_staged_session_dsl_state() {
    let store = Arc::new(HarnessRuntimeStore::failing_lifecycle_commit());
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    adapter.register_session(sid.clone()).await;
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
        RuntimeState::Idle,
        "failed destroy must restore the session DSL phase",
    );
    assert_eq!(
        adapter.list_active_inputs(&sid).await.unwrap(),
        vec![input_id],
        "failed destroy must restore active input projection",
    );
    assert_ne!(
        store.load_runtime_state(&runtime_id).await.unwrap(),
        Some(RuntimeState::Destroyed),
        "failed destroy must not persist destroyed runtime truth",
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), handle.wait())
            .await
            .is_err(),
        "failed destroy must not terminate completion waiters",
    );
}

#[tokio::test]
async fn service_turn_terminal_lifecycle_commit_failure_rolls_back_turn_completion() {
    let store = Arc::new(HarnessRuntimeStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    let bindings = adapter
        .prepare_bindings(sid.clone())
        .await
        .expect("prepare runtime bindings");
    let run_id = RunId::new();
    bindings
        .turn_state()
        .start_immediate_append(run_id.clone())
        .expect("start service turn through runtime handle");
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Running,
        "service turn should put the machine in a running lifecycle before terminal commit"
    );

    store.set_fail_commit_machine_lifecycle_now(true);
    let err = adapter
        .commit_service_turn_terminal_receipt(&sid)
        .await
        .expect_err("terminal lifecycle commit failure should surface");
    assert!(
        err.to_string()
            .contains("synthetic commit_machine_lifecycle failure"),
        "unexpected error: {err}",
    );

    let snapshot = bindings.turn_state().snapshot();
    assert_eq!(snapshot.active_run_id, Some(run_id));
    assert_eq!(
        snapshot.turn_phase,
        meerkat_core::turn_execution_authority::TurnPhase::ApplyingPrimitive,
        "failed durable service-turn receipt must roll back visible terminal turn state"
    );
    assert_eq!(
        snapshot.terminal_outcome, None,
        "failed durable service-turn receipt must not leave a terminal outcome projection"
    );
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Running,
        "failed durable service-turn receipt must preserve the running lifecycle"
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
    adapter.register_session(sid.clone()).await;
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
        store.load_runtime_state(&runtime_id).await.unwrap(),
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

    let store = Arc::new(HarnessRuntimeStore::failing_lifecycle_commit());
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
        .await;

    adapter
        .stop_runtime_executor(&sid, "async stop lifecycle failure")
        .await
        .expect("async stop command should enqueue the machine-approved effect");
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
        store
            .load_runtime_state(&LogicalRuntimeId::for_session(&sid))
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
        .await;

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
async fn cold_reregister_preserves_canonical_destroyed_runtime_state() {
    let store = Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new());
    let sid = SessionId::new();

    let adapter = Arc::new(MeerkatMachine::persistent(
        Arc::clone(&store) as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    adapter.register_session(sid.clone()).await;
    let runtime_id = LogicalRuntimeId::for_session(&sid);
    meerkat_runtime::traits::RuntimeControlPlane::destroy(&*adapter, &runtime_id)
        .await
        .expect("destroy should succeed before adapter restart");
    drop(adapter);

    let restarted = Arc::new(MeerkatMachine::persistent(
        Arc::clone(&store) as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    restarted.register_session(sid.clone()).await;
    assert_eq!(
        restarted.runtime_state(&sid).await.unwrap(),
        RuntimeState::Destroyed,
        "cold re-registration must preserve canonical durable destroyed runtime truth",
    );
    assert_eq!(
        store.load_runtime_state(&runtime_id).await.unwrap(),
        Some(RuntimeState::Destroyed),
        "cold re-registration must not rewrite canonical durable destroyed runtime truth",
    );
}

#[tokio::test]
async fn cold_reregister_ignores_legacy_session_uuid_runtime_state_alias() {
    let store = Arc::new(HarnessRuntimeStore::delayed_recover(Duration::ZERO));
    let sid = SessionId::new();
    let legacy_runtime_alias = LogicalRuntimeId::legacy_session_uuid_alias(&sid);
    store.seed_runtime_state_projection(legacy_runtime_alias, RuntimeState::Destroyed);

    let adapter = Arc::new(MeerkatMachine::persistent(
        Arc::clone(&store) as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    adapter.register_session(sid.clone()).await;
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Idle,
        "cold re-registration must not let the legacy runtime-state alias drive lifecycle",
    );
}

#[tokio::test]
async fn cold_reregister_prefers_canonical_runtime_state_over_stale_legacy_alias() {
    let store = Arc::new(HarnessRuntimeStore::delayed_recover(Duration::ZERO));
    let sid = SessionId::new();
    let canonical_runtime_id = LogicalRuntimeId::for_session(&sid);
    let legacy_runtime_alias = LogicalRuntimeId::legacy_session_uuid_alias(&sid);
    store.seed_runtime_state_projection(canonical_runtime_id, RuntimeState::Idle);
    store.seed_runtime_state_projection(legacy_runtime_alias, RuntimeState::Retired);

    let adapter = Arc::new(MeerkatMachine::persistent(
        Arc::clone(&store) as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    adapter.register_session(sid.clone()).await;
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
    adapter.register_session(sid.clone()).await;

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
    adapter.register_session(sid.clone()).await;
    adapter.unregister_session(&sid).await;

    let result = adapter.runtime_state(&sid).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn recycle_preserves_ephemeral_queued_work() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter.register_session(sid.clone()).await;

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
    adapter.register_session(sid.clone()).await;

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
async fn recycle_lifecycle_commit_failure_restores_retired_projection() {
    let store = Arc::new(HarnessRuntimeStore::failing_lifecycle_commit_after(2));
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    adapter.register_session(sid.clone()).await;
    let runtime_id = LogicalRuntimeId::for_session(&sid);

    meerkat_runtime::traits::RuntimeControlPlane::retire(&*adapter, &runtime_id)
        .await
        .expect("retire should commit before recycle failure");
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Retired
    );
    assert_eq!(
        store.load_runtime_state(&runtime_id).await.unwrap(),
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
        RuntimeState::Retired,
        "failed recycle must restore visible retired projection",
    );
    assert_eq!(
        store.load_runtime_state(&runtime_id).await.unwrap(),
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
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;

    struct NoResultExecutor;
    #[async_trait::async_trait]
    impl CoreExecutor for NoResultExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
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
    adapter.register_session(sid.clone()).await;

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
        .await;

    let result = tokio::time::timeout(Duration::from_secs(1), handle.wait())
        .await
        .expect("completion should resolve after recycle + executor attach");
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
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
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
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
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
            body: format!("progress-{label}"),
            payload: Some(serde_json::json!({ "label": label })),
            blocks: None,
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
        .await;

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
        .expect("attached runtime should wake and drain recycled queued work");
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
    adapter.register_session(sid.clone()).await;

    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, make_prompt("pending on unregister"))
        .await
        .unwrap();
    assert!(outcome.is_accepted());
    let handle = handle.expect("accepted input should produce a completion handle");

    adapter.unregister_session(&sid).await;

    let result = handle.wait().await;
    assert!(
        matches!(
            result,
            meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated(ref reason)
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
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
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
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
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
        .await;

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
    let state = adapter.runtime_state(&sid).await.unwrap();
    assert_eq!(state, RuntimeState::Attached);

    // The input should be consumed (terminal)
    let active = adapter.list_active_inputs(&sid).await.unwrap();
    assert!(active.is_empty(), "All inputs should be consumed");
}

#[tokio::test]
async fn runtime_comms_terminal_response_wake_drains_requester_queue() {
    use meerkat_comms::runtime::comms_runtime::CommsRuntime as InprocCommsRuntime;
    use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
    use meerkat_core::comms::{CommsCommand, InputStreamMode, PeerName, PeerRoute, SendReceipt};
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
    use meerkat_core::{
        HandlingMode, InteractionContent, InteractionId, PeerCorrelationId, ResponseStatus,
    };
    use meerkat_runtime::PeerConvention;
    use tokio::sync::Notify;
    use uuid::Uuid;

    struct BlockingRecordingExecutor {
        calls: Arc<AtomicUsize>,
        first_apply_started: Arc<Notify>,
        release_first_apply: Arc<Notify>,
        terminal_applied: Arc<Notify>,
        terminal_context_keys: Arc<std::sync::Mutex<Vec<String>>>,
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
                self.first_apply_started.notify_waiters();
                self.release_first_apply.notified().await;
            }

            let boundary = match &primitive {
                RunPrimitive::StagedInput(staged) => {
                    for append in &staged.context_appends {
                        if append.key.starts_with("peer_response_terminal:") {
                            self.terminal_context_keys
                                .lock()
                                .expect("terminal_context_keys mutex")
                                .push(append.key.clone());
                            self.terminal_applied.notify_waiters();
                        }
                    }
                    staged.boundary
                }
                RunPrimitive::ImmediateAppend(_) => RunApplyBoundary::Immediate,
                _ => RunApplyBoundary::RunStart,
            };

            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: call as u64,
                },
                session_snapshot: None,
                terminal: None,
            })
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

    async fn drain_until_nonempty(
        runtime: &InprocCommsRuntime,
    ) -> Vec<meerkat_core::ClassifiedInboxInteraction> {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let batch = CoreCommsRuntime::drain_classified_inbox_interactions(runtime)
                    .await
                    .unwrap_or_default();
                if !batch.is_empty() {
                    return batch;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("classified inbox should receive request")
    }

    let suffix = Uuid::new_v4().simple().to_string();
    let name_a = format!("runtime-requester-{suffix}");
    let name_b = format!("runtime-responder-{suffix}");
    let (requester_comms, responder_comms) =
        InprocCommsRuntime::inproc_pair_with_mutual_trust(&name_a, &name_b)
            .await
            .expect("inproc comms pair");

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    let bindings = adapter
        .prepare_bindings(sid.clone())
        .await
        .expect("prepare runtime bindings");
    requester_comms.install_peer_comms_handle(Arc::clone(bindings.peer_comms()));
    requester_comms.install_peer_request_response_authority(
        meerkat_comms::PeerRequestResponseAuthority::new(
            Arc::clone(bindings.peer_interaction()),
            Arc::clone(bindings.interaction_stream()),
        ),
    );
    let responder_adapter = Arc::new(MeerkatMachine::ephemeral());
    let responder_sid = SessionId::new();
    let responder_bindings = responder_adapter
        .prepare_bindings(responder_sid)
        .await
        .expect("prepare responder runtime bindings");
    responder_comms.install_peer_request_response_authority(
        meerkat_comms::PeerRequestResponseAuthority::new(
            Arc::clone(responder_bindings.peer_interaction()),
            Arc::clone(responder_bindings.interaction_stream()),
        ),
    );

    let calls = Arc::new(AtomicUsize::new(0));
    let first_apply_started = Arc::new(Notify::new());
    let release_first_apply = Arc::new(Notify::new());
    let terminal_applied = Arc::new(Notify::new());
    let terminal_context_keys = Arc::new(std::sync::Mutex::new(Vec::new()));
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(BlockingRecordingExecutor {
                calls: Arc::clone(&calls),
                first_apply_started: Arc::clone(&first_apply_started),
                release_first_apply: Arc::clone(&release_first_apply),
                terminal_applied: Arc::clone(&terminal_applied),
                terminal_context_keys: Arc::clone(&terminal_context_keys),
            }),
        )
        .await;

    let requester_for_drain: Arc<dyn CoreCommsRuntime> = requester_comms.clone();
    assert!(
        adapter
            .update_peer_ingress_context(&sid, true, Some(requester_for_drain))
            .await,
        "host-mode requester should spawn comms drain"
    );

    adapter
        .accept_input(&sid, make_prompt("keep requester busy"))
        .await
        .expect("accept blocking prompt");
    tokio::time::timeout(Duration::from_secs(2), first_apply_started.notified())
        .await
        .expect("first apply should start");

    let receipt = CoreCommsRuntime::send(
        requester_comms.as_ref(),
        CommsCommand::PeerRequest {
            to: PeerRoute::with_display_name(
                responder_comms.public_key().to_peer_id(),
                PeerName::new(name_b.clone()).expect("responder peer name"),
            ),
            intent: "terminal-wake-probe".to_string(),
            params: serde_json::json!({"probe": true}),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            stream: InputStreamMode::ReserveInteraction,
        },
    )
    .await
    .expect("send request");
    let request_id = match receipt {
        SendReceipt::PeerRequestSent { envelope_id, .. } => envelope_id,
        other => panic!("expected PeerRequestSent, got {other:?}"),
    };

    let request_at_responder = drain_until_nonempty(responder_comms.as_ref()).await;
    assert!(matches!(
        request_at_responder[0].interaction.content,
        InteractionContent::Request { .. }
    ));
    responder_bindings
        .peer_interaction()
        .request_received(PeerCorrelationId::from_uuid(request_id))
        .expect("seed responder inbound request state");

    CoreCommsRuntime::send(
        responder_comms.as_ref(),
        CommsCommand::PeerResponse {
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

    release_first_apply.notify_waiters();
    tokio::time::timeout(Duration::from_secs(2), terminal_applied.notified())
        .await
        .expect("WakeLoop should drain queued peer_response_terminal");

    let keys = terminal_context_keys
        .lock()
        .expect("terminal_context_keys mutex")
        .clone();
    let responder_route_id = responder_comms.public_key().to_peer_id();
    assert_eq!(
        keys,
        vec![format!(
            "peer_response_terminal:{responder_route_id}:{request_id}"
        )],
        "terminal response should render through typed context append"
    );
    let active_ids = adapter
        .list_active_inputs(&sid)
        .await
        .expect("active inputs after drain");
    for input_id in active_ids {
        let state = adapter
            .input_state(&sid, &input_id)
            .await
            .expect("input state")
            .expect("active input state");
        assert!(
            !matches!(
                state.state.persisted_input.as_ref(),
                Some(Input::Peer(meerkat_runtime::PeerInput {
                    convention: Some(PeerConvention::ResponseTerminal { .. }),
                    ..
                }))
            ),
            "queued peer_response_terminal must not remain stuck after WakeLoop: {state:?}"
        );
    }
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
    struct FailingExecutor;

    #[async_trait::async_trait]
    impl CoreExecutor for FailingExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
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
    adapter
        .register_session_with_executor(sid.clone(), Box::new(FailingExecutor))
        .await;

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

    // Runtime should be back to Attached (executor still connected, not stuck in Running)
    let state = adapter.runtime_state(&sid).await.unwrap();
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
        .await;

    let input = make_prompt("hello failing forever");
    let input_id = input.id().clone();
    adapter.accept_input(&sid, input).await.unwrap();

    wait_for_atomic_usize_at_least(
        &calls,
        1,
        "failing executor should be called before retry-budget assertions",
    )
    .await;
    let state = wait_for_input_state(
        &adapter,
        &sid,
        &input_id,
        "failed input should leave in-flight lifecycle state after retry exhaustion",
        |state| {
            !matches!(
                state.seed.phase,
                InputLifecycleState::Staged | InputLifecycleState::AppliedPendingConsumption
            )
        },
    )
    .await;
    let call_count = calls.load(Ordering::SeqCst);
    assert!(
        (1..=3).contains(&call_count),
        "retry budget should remain bounded; expected 1-3 attempts, saw {call_count}"
    );
    assert!(
        !matches!(
            state.seed.phase,
            InputLifecycleState::Staged | InputLifecycleState::AppliedPendingConsumption
        ),
        "failed inputs must not remain stuck in an in-flight lifecycle state"
    );

    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        call_count,
        "failed inputs must not keep spinning through fresh run ids"
    );
}

#[tokio::test]
async fn failed_executor_continues_processing_backlog() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
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
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
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
        .await;

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
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
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
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
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
    adapter.register_session(sid.clone()).await;

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
        .await;

    wait_for_atomic_bool(
        &apply_called,
        "upgrading an already-registered session should attach a live loop",
    )
    .await;

    let state = adapter.runtime_state(&sid).await.unwrap();
    assert_eq!(state, RuntimeState::Attached);

    let active = adapter.list_active_inputs(&sid).await.unwrap();
    assert!(active.is_empty(), "queued work should drain after upgrade");

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
}

#[tokio::test]
async fn ensure_session_with_executor_upgrades_racy_registration() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
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
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
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
                .await;
        })
    };

    tokio::time::sleep(Duration::from_millis(10)).await;
    adapter.register_session(sid.clone()).await;
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
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
    use meerkat_runtime::input_state::InputLifecycleState;

    struct PanicOnStopExecutor;

    #[async_trait::async_trait]
    impl CoreExecutor for PanicOnStopExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
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
            panic!("synthetic stop panic to kill the loop and leave driver attached");
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
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
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
        .register_session_with_executor(sid.clone(), Box::new(PanicOnStopExecutor))
        .await;
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Attached
    );

    adapter
        .stop_runtime_executor(&sid, "stale attachment repair test")
        .await
        .expect("attached runtime should accept the stop effect");

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match adapter
                .hard_cancel_current_run(&sid, "stale attachment repair probe")
                .await
            {
                Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Attached,
                }) => break,
                _ => tokio::time::sleep(Duration::from_millis(5)).await,
            }
        }
    })
    .await
    .expect("runtime loop should die and leave a stale attached driver state behind");

    let apply_called = Arc::new(AtomicBool::new(false));
    adapter
        .ensure_session_with_executor(
            sid.clone(),
            Box::new(RecordingExecutor {
                called: Arc::clone(&apply_called),
            }),
        )
        .await;

    let input = make_prompt("repair stale attachment");
    let input_id = input.id().clone();
    adapter.accept_input(&sid, input).await.unwrap();

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if apply_called.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("ensuring with executor should repair the stale attached driver");

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
        CoreApplyOutput, CoreExecutor, CoreExecutorError, CoreExecutorInterruptHandle,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
    use tokio::sync::Notify;

    struct BlockingStopExecutor {
        stop_entered: Arc<Notify>,
        release_stop: Arc<Notify>,
        interrupt_calls: Arc<AtomicUsize>,
    }

    struct BlockingStopInterruptHandle {
        interrupt_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutorInterruptHandle for BlockingStopInterruptHandle {
        async fn hard_cancel_current_run(&self, _reason: String) -> Result<(), CoreExecutorError> {
            self.interrupt_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingStopExecutor {
        fn interrupt_handle(&self) -> Option<Arc<dyn CoreExecutorInterruptHandle>> {
            Some(Arc::new(BlockingStopInterruptHandle {
                interrupt_calls: Arc::clone(&self.interrupt_calls),
            }))
        }

        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
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
    let interrupt_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(BlockingStopExecutor {
                stop_entered: Arc::clone(&stop_entered),
                release_stop: Arc::clone(&release_stop),
                interrupt_calls: Arc::clone(&interrupt_calls),
            }),
        )
        .await;

    let stop_adapter = Arc::clone(&adapter);
    let stop_sid = sid.clone();
    let stop_task = tokio::spawn(async move {
        stop_adapter
            .stop_runtime_executor(&stop_sid, "ownership-stop")
            .await
            .expect("stop command should send successfully");
    });

    stop_entered.notified().await;
    stop_task.await.unwrap();

    adapter
        .hard_cancel_current_run(&sid, "blocking stop attachment probe")
        .await
        .expect("attachment should remain published while stop is still in progress");
    assert_eq!(interrupt_calls.load(Ordering::SeqCst), 1);

    release_stop.notify_one();

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
async fn completed_boundary_commit_failure_preserves_sync_pre_terminal_state() {
    use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
    use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
    use meerkat_runtime::input_state::InputLifecycleState;

    let store = Arc::new(HarnessRuntimeStore::failing_atomic_apply());
    let adapter = Arc::new(MeerkatMachine::persistent(store, memory_blob_store()));
    let sid = SessionId::new();
    adapter.register_session(sid.clone()).await;

    let input = make_prompt("sync boundary failure");
    let input_id = input.id().clone();
    let result = adapter
        .accept_input_and_run(&sid, input, move |run_id, primitive| async move {
            Ok((
                (),
                CoreApplyOutput {
                    receipt: RunBoundaryReceipt {
                        run_id,
                        boundary: RunApplyBoundary::RunStart,
                        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                        conversation_digest: None,
                        message_count: 0,
                        sequence: 0,
                    },
                    session_snapshot: None,
                    terminal: None,
                },
            ))
        })
        .await;
    assert!(
        result.is_err(),
        "completed boundary commit failure should surface"
    );
    let Err(err) = result else {
        unreachable!("asserted runtime completed boundary commit failure above");
    };
    let err = err.to_string();
    assert!(
        err.contains("failed to persist runtime completed-boundary snapshot")
            && err.contains("synthetic atomic_apply failure"),
        "unexpected error: {err}"
    );
    assert!(
        adapter.contains_session(&sid).await,
        "completed boundary commit rollback should preserve the registered session"
    );
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Running
    );
    let state = adapter.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(state.seed.phase, InputLifecycleState::Staged);
}

#[tokio::test]
async fn completed_boundary_commit_failure_unwinds_runtime_loop_state() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
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
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
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
    let adapter = Arc::new(MeerkatMachine::persistent(store, memory_blob_store()));
    let sid = SessionId::new();
    let stop_called = Arc::new(AtomicBool::new(false));
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(SuccessExecutor {
                stop_called: Arc::clone(&stop_called),
            }),
        )
        .await;

    let input = make_prompt("loop boundary failure");
    let input_id = input.id().clone();
    adapter.accept_input(&sid, input).await.unwrap();

    wait_for_atomic_bool(
        &stop_called,
        "boundary commit failures should stop the dead executor path",
    )
    .await;
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Stopped
    );
    let state = adapter.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(state.seed.phase, InputLifecycleState::Abandoned);
}

#[tokio::test]
async fn completed_boundary_commit_failure_terminates_runtime_loop_completion_waiter() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
    use meerkat_core::turn_execution_authority::{TurnTerminalCauseKind, TurnTerminalOutcome};
    use meerkat_runtime::completion::CompletionOutcome;

    struct SuccessExecutor;

    #[async_trait::async_trait]
    impl CoreExecutor for SuccessExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
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
        .await;

    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, make_prompt("loop boundary waiter failure"))
        .await
        .expect("accept should succeed");
    assert!(outcome.is_accepted());
    let handle = handle.expect("accepted input should expose a completion handle");

    let result = tokio::time::timeout(Duration::from_secs(1), handle.wait())
        .await
        .expect("completion waiter should resolve when the runtime loop exits");
    assert!(
        matches!(
            result,
            CompletionOutcome::AbandonedWithError { ref error, .. }
                if error.kind == TurnTerminalCauseKind::RuntimeApplyFailure
                    && error.terminal
                    && error.outcome == Some(TurnTerminalOutcome::Failed)
        ),
        "boundary commit failure should abandon the waiter with typed runtime apply failure, got {result:?}"
    );
}

#[tokio::test]
async fn completed_run_runtime_loop_skips_terminal_lifecycle_snapshot_writer() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;

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
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
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
            self.adapter.unregister_session(&self.session_id).await;
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
        .await;

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

#[tokio::test]
async fn completed_run_sync_path_skips_terminal_lifecycle_snapshot_writer() {
    use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
    use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;

    let store = Arc::new(HarnessRuntimeStore::failing_terminal_snapshot());
    let adapter = Arc::new(MeerkatMachine::persistent(
        store.clone() as Arc<dyn RuntimeStore>,
        memory_blob_store(),
    ));
    let sid = SessionId::new();
    adapter.register_session(sid.clone()).await;

    let result = adapter
        .accept_input_and_run(
            &sid,
            make_prompt("sync skips terminal lifecycle snapshot"),
            move |run_id, primitive| async move {
                Ok((
                    (),
                    CoreApplyOutput {
                        receipt: RunBoundaryReceipt {
                            run_id,
                            boundary: RunApplyBoundary::RunStart,
                            contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                            conversation_digest: None,
                            message_count: 0,
                            sequence: 0,
                        },
                        session_snapshot: None,
                        terminal: None,
                    },
                ))
            },
        )
        .await;
    assert!(
        result.is_ok(),
        "completed sync path should not use the old terminal lifecycle writer: {result:?}"
    );
    assert_eq!(
        store.commit_machine_lifecycle_calls(),
        1,
        "completed sync path must not use the old post-receipt lifecycle snapshot writer"
    );
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Idle
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
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
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
                RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
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
        .await;

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
    let result = handle1.unwrap().wait().await;
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
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
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
                RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
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
        .await;

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
    let result1 = handle1.unwrap().wait().await;
    let result2 = handle2.unwrap().wait().await;
    assert!(
        matches!(result1, meerkat_runtime::completion::CompletionOutcome::Completed(ref r) if r.text == "slow done"),
        "original handle should complete with result"
    );
    assert!(
        matches!(result2, meerkat_runtime::completion::CompletionOutcome::Completed(ref r) if r.text == "slow done"),
        "duplicate handle should also complete with same result"
    );
}

#[tokio::test]
async fn accept_input_and_run_rejects_deduplicated_admission() {
    use meerkat_runtime::identifiers::IdempotencyKey;

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter.register_session(sid.clone()).await;

    let key = IdempotencyKey::new("sync-dedup");
    let mut first = make_prompt("first");
    if let Input::Prompt(ref mut prompt) = first {
        prompt.header.idempotency_key = Some(key.clone());
    }
    let first_id = first.id().clone();
    let outcome = adapter.accept_input(&sid, first).await.unwrap();
    assert!(outcome.is_accepted());

    let mut duplicate = make_prompt("duplicate");
    if let Input::Prompt(ref mut prompt) = duplicate {
        prompt.header.idempotency_key = Some(key);
    }

    let err = adapter
        .accept_input_and_run::<(), _, _>(&sid, duplicate, |_run_id, _primitive| async move {
            unreachable!("deduplicated admission must be rejected before sync execution starts")
        })
        .await
        .expect_err("deduplicated sync admission should be rejected");
    assert!(matches!(
        err,
        RuntimeDriverError::ValidationFailed { ref reason }
        if reason.contains("does not support deduplicated admission")
    ));

    let state = adapter.input_state(&sid, &first_id).await.unwrap().unwrap();
    assert_eq!(
        state.seed.phase,
        meerkat_runtime::InputLifecycleState::Queued
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
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;

    struct NoResultExecutor;
    #[async_trait::async_trait]
    impl CoreExecutor for NoResultExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None, // No RunResult
            })
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
        .await;

    let input = make_prompt("context append");
    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, input)
        .await
        .unwrap();
    assert!(outcome.is_accepted());

    let result = handle.unwrap().wait().await;
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
        .await;

    let input = make_prompt("cancelled");
    let input_id = input.id().clone();
    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, input)
        .await
        .unwrap();
    assert!(outcome.is_accepted());

    let result = handle.unwrap().wait().await;
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
        .await;

    let input = make_prompt("persistent cancelled");
    let input_id = input.id().clone();
    let (outcome, handle) = adapter
        .accept_input_with_completion(&sid, input)
        .await
        .unwrap();
    assert!(outcome.is_accepted());

    let result = handle.unwrap().wait().await;
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
        store.load_runtime_state(&runtime_id).await.unwrap(),
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
    adapter.register_session(sid.clone()).await;

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
    let result = handle.unwrap().wait().await;
    assert!(
        matches!(
            result,
            meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated(_)
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
    adapter.register_session(sid.clone()).await;

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
    let result = handle.unwrap().wait().await;
    assert!(
        matches!(
            result,
            meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated(_)
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
        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<Notify> {
            Arc::clone(&self.notify)
        }

        fn dismiss_received(&self) -> bool {
            false
        }

        async fn drain_peer_input_candidates(
            &self,
        ) -> Vec<meerkat_core::interaction::PeerInputCandidate> {
            Vec::new()
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter.register_session(sid.clone()).await;

    let comms: Arc<dyn CommsRuntime> = Arc::new(IdleDrainRuntime::new());
    let spawned = adapter
        .update_peer_ingress_context(&sid, true, Some(comms))
        .await;
    assert!(spawned, "registered host-mode session should spawn a drain");

    // Give the drain task time to start before unregistering.
    tokio::time::sleep(Duration::from_millis(50)).await;

    adapter.unregister_session(&sid).await;
    adapter.wait_comms_drain(&sid).await;
    assert!(matches!(
        adapter.runtime_state(&sid).await,
        Err(RuntimeDriverError::NotReady {
            state: RuntimeState::Destroyed
        })
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
        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<Notify> {
            Arc::clone(&self.notify)
        }

        fn dismiss_received(&self) -> bool {
            false
        }

        async fn drain_peer_input_candidates(
            &self,
        ) -> Vec<meerkat_core::interaction::PeerInputCandidate> {
            Vec::new()
        }
    }

    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let sid = SessionId::new();
    adapter.register_session(sid.clone()).await;

    let comms: Arc<dyn CommsRuntime> = Arc::new(IdleDrainRuntime::new());
    let spawned = adapter
        .update_peer_ingress_context(&sid, false, Some(comms))
        .await;

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
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
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
        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<Notify> {
            Arc::clone(&self.notify)
        }

        fn dismiss_received(&self) -> bool {
            false
        }

        async fn drain_peer_input_candidates(
            &self,
        ) -> Vec<meerkat_core::interaction::PeerInputCandidate> {
            Vec::new()
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
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
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
        .await;

    let comms: Arc<dyn CommsRuntime> = Arc::new(IdleDrainRuntime::new());
    let spawned = adapter
        .update_peer_ingress_context(&sid, false, Some(comms))
        .await;

    assert!(
        !spawned,
        "attached sessions should not spawn a comms drain when keep_alive is disabled"
    );

    adapter.unregister_session(&sid).await;
}

/// Test that BoundaryApplied fires with correct receipt on success.
#[tokio::test]
async fn successful_execution_fires_boundary_applied() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
    use meerkat_runtime::input_state::InputLifecycleState;

    struct SuccessExecutor;

    #[async_trait::async_trait]
    impl CoreExecutor for SuccessExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
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
        .await;

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
        !adapter.session_has_executor(&sid).await,
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
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;

    struct NoopExecutor;
    #[async_trait::async_trait]
    impl CoreExecutor for NoopExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            Ok(CoreApplyOutput {
                receipt: RunBoundaryReceipt {
                    run_id,
                    boundary: RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
            })
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
        !adapter.session_has_executor(&sid).await,
        "before executor attachment"
    );

    adapter
        .ensure_session_with_executor(sid.clone(), Box::new(NoopExecutor))
        .await;

    assert!(
        adapter.session_has_executor(&sid).await,
        "after executor attachment"
    );
}

#[tokio::test]
async fn session_has_executor_false_for_unknown() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let unknown = SessionId::new();
    assert!(
        !adapter.session_has_executor(&unknown).await,
        "unknown session should not have an executor"
    );
}
