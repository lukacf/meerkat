#![allow(clippy::unwrap_used)]

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

use chrono::Utc;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::SessionId;
use meerkat_runtime::{
    Input, InputDurability, InputHeader, InputOrigin, InputState, InputVisibility, PromptInput,
    RuntimeDriverError, RuntimeSessionAdapter, RuntimeState, RuntimeStore, RuntimeStoreError,
    SessionDelta, SessionServiceRuntimeExt,
};

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

struct HarnessRuntimeStore {
    inner: meerkat_runtime::store::InMemoryRuntimeStore,
    fail_atomic_apply: bool,
    /// Fail atomic_lifecycle_commit after N successful calls (None = never fail).
    fail_atomic_lifecycle_commit_after: Option<usize>,
    atomic_lifecycle_commit_calls: AtomicUsize,
    load_input_states_delay: Duration,
    fail_persist_input_state_after: Option<usize>,
    persist_input_state_calls: AtomicUsize,
}

impl HarnessRuntimeStore {
    fn failing_atomic_apply() -> Self {
        Self {
            inner: meerkat_runtime::store::InMemoryRuntimeStore::new(),
            fail_atomic_apply: true,
            fail_atomic_lifecycle_commit_after: None,
            atomic_lifecycle_commit_calls: AtomicUsize::new(0),
            load_input_states_delay: Duration::ZERO,
            fail_persist_input_state_after: None,
            persist_input_state_calls: AtomicUsize::new(0),
        }
    }

    fn delayed_recover(delay: Duration) -> Self {
        Self {
            inner: meerkat_runtime::store::InMemoryRuntimeStore::new(),
            fail_atomic_apply: false,
            fail_atomic_lifecycle_commit_after: None,
            atomic_lifecycle_commit_calls: AtomicUsize::new(0),
            load_input_states_delay: delay,
            fail_persist_input_state_after: None,
            persist_input_state_calls: AtomicUsize::new(0),
        }
    }

    fn failing_terminal_snapshot() -> Self {
        Self {
            inner: meerkat_runtime::store::InMemoryRuntimeStore::new(),
            fail_atomic_apply: false,
            // Recovery calls atomic_lifecycle_commit once (call 0 succeeds),
            // the terminal event call (call 1) fails.
            fail_atomic_lifecycle_commit_after: Some(1),
            atomic_lifecycle_commit_calls: AtomicUsize::new(0),
            load_input_states_delay: Duration::ZERO,
            fail_persist_input_state_after: None,
            persist_input_state_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl RuntimeStore for HarnessRuntimeStore {
    async fn commit_session_boundary(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        session_delta: SessionDelta,
        run_id: RunId,
        boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
        input_updates: Vec<InputState>,
    ) -> Result<meerkat_core::lifecycle::RunBoundaryReceipt, RuntimeStoreError> {
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
        session_delta: Option<SessionDelta>,
        receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
        input_updates: Vec<InputState>,
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
    ) -> Result<Vec<InputState>, RuntimeStoreError> {
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
        state: &InputState,
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
    ) -> Result<Option<InputState>, RuntimeStoreError> {
        self.inner.load_input_state(runtime_id, input_id).await
    }

    async fn persist_runtime_state(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        state: RuntimeState,
    ) -> Result<(), RuntimeStoreError> {
        self.inner.persist_runtime_state(runtime_id, state).await
    }

    async fn load_runtime_state(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
    ) -> Result<Option<RuntimeState>, RuntimeStoreError> {
        self.inner.load_runtime_state(runtime_id).await
    }

    async fn atomic_lifecycle_commit(
        &self,
        runtime_id: &meerkat_runtime::identifiers::LogicalRuntimeId,
        runtime_state: RuntimeState,
        input_states: &[InputState],
    ) -> Result<(), RuntimeStoreError> {
        let call_index = self
            .atomic_lifecycle_commit_calls
            .fetch_add(1, Ordering::SeqCst);
        if self
            .fail_atomic_lifecycle_commit_after
            .is_some_and(|fail_after| call_index >= fail_after)
        {
            return Err(RuntimeStoreError::WriteFailed(
                "synthetic atomic_lifecycle_commit failure".to_string(),
            ));
        }
        self.inner
            .atomic_lifecycle_commit(runtime_id, runtime_state, input_states)
            .await
    }
}

#[tokio::test]
async fn ephemeral_adapter_accept_and_query() {
    let adapter = RuntimeSessionAdapter::ephemeral();
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
async fn persistent_adapter_accept() {
    let store = Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new());
    let adapter = RuntimeSessionAdapter::persistent(store);
    let sid = SessionId::new();
    adapter.register_session(sid.clone()).await;

    let input = make_prompt("hello");
    let outcome = adapter.accept_input(&sid, input).await.unwrap();
    assert!(outcome.is_accepted());
}

#[tokio::test]
async fn unregistered_session_errors() {
    let adapter = RuntimeSessionAdapter::ephemeral();
    let sid = SessionId::new();
    let result = adapter.accept_input(&sid, make_prompt("hi")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn unregister_removes_driver() {
    let adapter = RuntimeSessionAdapter::ephemeral();
    let sid = SessionId::new();
    adapter.register_session(sid.clone()).await;
    adapter.unregister_session(&sid).await;

    let result = adapter.runtime_state(&sid).await;
    assert!(result.is_err());
}

/// Test that accept_input with a RuntimeLoop triggers input processing.
#[tokio::test]
async fn accept_with_executor_triggers_loop() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_control::RunControlCommand;
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
                run_result: None,
            })
        }

        async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = RuntimeSessionAdapter::ephemeral();
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

    // Give the loop time to process
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert!(
        apply_called.load(Ordering::SeqCst),
        "CoreExecutor::apply() should have been called by the RuntimeLoop"
    );

    // After processing, the input should be consumed and the runtime back to Attached
    // (executor is still connected, so Attached not Idle).
    let state = adapter.runtime_state(&sid).await.unwrap();
    assert_eq!(state, RuntimeState::Attached);

    // The input should be consumed (terminal)
    let active = adapter.list_active_inputs(&sid).await.unwrap();
    assert!(active.is_empty(), "All inputs should be consumed");
}

/// Test that a failed executor re-queues the input (not stranded in APC).
#[tokio::test]
async fn failed_executor_requeues_input() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_control::RunControlCommand;
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
            Err(CoreExecutorError::ApplyFailed {
                reason: "LLM error".into(),
            })
        }

        async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = RuntimeSessionAdapter::ephemeral();
    let sid = SessionId::new();
    adapter
        .register_session_with_executor(sid.clone(), Box::new(FailingExecutor))
        .await;

    let input = make_prompt("hello failing");
    let input_id = input.id().clone();
    adapter.accept_input(&sid, input).await.unwrap();

    // Give the loop time to process and fail
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Runtime should be back to Attached (executor still connected, not stuck in Running)
    let state = adapter.runtime_state(&sid).await.unwrap();
    assert_eq!(state, RuntimeState::Attached);

    // Input should be rolled back to Queued (not stranded in APC)
    let is = adapter.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(
        is.current_state(),
        InputLifecycleState::Queued,
        "Failed execution should roll input back to Queued, not strand in AppliedPendingConsumption"
    );
}

#[tokio::test]
async fn failed_executor_continues_processing_backlog() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_control::RunControlCommand;
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
    use meerkat_runtime::input_state::InputLifecycleState;

    struct FailThenSucceedExecutor {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutor for FailThenSucceedExecutor {
        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
            if call == 0 {
                return Err(CoreExecutorError::ApplyFailed {
                    reason: "first run fails".into(),
                });
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
                run_result: None,
            })
        }

        async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = RuntimeSessionAdapter::ephemeral();
    let sid = SessionId::new();
    let calls = Arc::new(AtomicUsize::new(0));
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(FailThenSucceedExecutor {
                calls: Arc::clone(&calls),
            }),
        )
        .await;

    let first = make_prompt("first");
    let first_id = first.id().clone();
    let second = make_prompt("second");
    let second_id = second.id().clone();
    adapter.accept_input(&sid, first).await.unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    adapter.accept_input(&sid, second).await.unwrap();

    tokio::time::sleep(Duration::from_millis(220)).await;

    let second_state = adapter
        .input_state(&sid, &second_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second_state.current_state(), InputLifecycleState::Consumed);
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Attached
    );
    assert!(
        calls.load(Ordering::SeqCst) >= 2,
        "the runtime loop should keep draining queued backlog after a failed run"
    );
    let first_state = adapter.input_state(&sid, &first_id).await.unwrap().unwrap();
    assert!(
        matches!(
            first_state.current_state(),
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
    use meerkat_core::lifecycle::run_control::RunControlCommand;
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
                run_result: None,
            })
        }

        async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let apply_called = Arc::new(AtomicBool::new(false));
    let adapter = RuntimeSessionAdapter::ephemeral();
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

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert!(
        apply_called.load(Ordering::SeqCst),
        "upgrading an already-registered session should attach a live loop"
    );

    let state = adapter.runtime_state(&sid).await.unwrap();
    assert_eq!(state, RuntimeState::Attached);

    let active = adapter.list_active_inputs(&sid).await.unwrap();
    assert!(active.is_empty(), "queued work should drain after upgrade");

    let is = adapter.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(
        is.current_state(),
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
    use meerkat_core::lifecycle::run_control::RunControlCommand;
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
                run_result: None,
            })
        }

        async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let store = Arc::new(HarnessRuntimeStore::delayed_recover(Duration::from_millis(
        75,
    )));
    let adapter = Arc::new(RuntimeSessionAdapter::persistent(store));
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
    tokio::time::sleep(Duration::from_millis(120)).await;

    assert!(
        apply_called.load(Ordering::SeqCst),
        "the racy registration path should still attach a live runtime loop"
    );
    let state = adapter.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(state.current_state(), InputLifecycleState::Consumed);
}

#[tokio::test]
async fn boundary_commit_failure_unwinds_sync_runtime_state() {
    use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
    use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
    use meerkat_runtime::input_state::InputLifecycleState;

    let store = Arc::new(HarnessRuntimeStore::failing_atomic_apply());
    let adapter = RuntimeSessionAdapter::persistent(store);
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
                    run_result: None,
                },
            ))
        })
        .await;
    assert!(result.is_err(), "boundary commit failure should surface");
    let Err(err) = result else {
        unreachable!("asserted runtime boundary commit failure above");
    };
    assert!(
        err.to_string().contains("runtime boundary commit failed"),
        "unexpected error: {err}"
    );
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Idle
    );
    let state = adapter.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(state.current_state(), InputLifecycleState::Queued);
}

#[tokio::test]
async fn boundary_commit_failure_unwinds_runtime_loop_state() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_control::RunControlCommand;
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
                run_result: None,
            })
        }

        async fn control(&mut self, cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(cmd, RunControlCommand::StopRuntimeExecutor { .. }) {
                self.stop_called.store(true, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    let store = Arc::new(HarnessRuntimeStore::failing_atomic_apply());
    let adapter = RuntimeSessionAdapter::persistent(store);
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
    tokio::time::sleep(Duration::from_millis(120)).await;

    assert!(
        stop_called.load(Ordering::SeqCst),
        "boundary commit failures should stop the dead executor path"
    );
    assert_eq!(
        adapter.runtime_state(&sid).await.unwrap(),
        RuntimeState::Attached
    );
    let state = adapter.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(state.current_state(), InputLifecycleState::Queued);
}

#[tokio::test]
async fn terminal_snapshot_failure_unregisters_runtime_loop_session() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_control::RunControlCommand;
    use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;

    struct SuccessExecutor {
        adapter: Arc<RuntimeSessionAdapter>,
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
                run_result: None,
            })
        }

        async fn control(&mut self, cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(cmd, RunControlCommand::StopRuntimeExecutor { .. }) {
                self.stop_called.store(true, Ordering::SeqCst);
                self.adapter.unregister_session(&self.session_id).await;
            }
            Ok(())
        }
    }

    let store = Arc::new(HarnessRuntimeStore::failing_terminal_snapshot());
    let adapter = Arc::new(RuntimeSessionAdapter::persistent(store));
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

    adapter
        .accept_input(&sid, make_prompt("terminal snapshot failure"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(120)).await;

    assert!(
        stop_called.load(Ordering::SeqCst),
        "terminal snapshot persistence failures should stop the runtime loop"
    );
    let state_result = adapter.runtime_state(&sid).await;
    assert!(
        state_result.is_err(),
        "stopped runtime sessions should be unregistered"
    );
    let Err(err) = state_result else {
        unreachable!("asserted stopped runtime unregistration above");
    };
    assert!(matches!(
        err,
        RuntimeDriverError::NotReady {
            state: RuntimeState::Destroyed
        }
    ));
}

#[tokio::test]
async fn terminal_snapshot_failure_unregisters_sync_runtime_session() {
    use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
    use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
    use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;

    let store = Arc::new(HarnessRuntimeStore::failing_terminal_snapshot());
    let adapter = RuntimeSessionAdapter::persistent(store);
    let sid = SessionId::new();
    adapter.register_session(sid.clone()).await;

    let result = adapter
        .accept_input_and_run(
            &sid,
            make_prompt("sync terminal snapshot failure"),
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
                        run_result: None,
                    },
                ))
            },
        )
        .await;
    assert!(
        result.is_err(),
        "terminal snapshot persistence failure should surface"
    );
    let Err(err) = result else {
        unreachable!("asserted terminal snapshot failure above");
    };

    assert!(
        err.to_string().contains("terminal event persist failed")
            || err
                .to_string()
                .contains("failed to persist runtime completion snapshot"),
        "unexpected error: {err}"
    );
    let runtime_state = adapter.runtime_state(&sid).await;
    assert!(
        matches!(
            runtime_state,
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed
            })
        ),
        "sync path should unregister the broken runtime session"
    );
}

// ─── Phase A gate tests ───

/// Gate A2: Dedup on terminal input returns (Deduplicated, None) — no hang.
#[tokio::test]
async fn dedup_terminal_input_returns_none_handle() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_control::RunControlCommand;
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
                run_result: Some(RunResult {
                    text: "done".into(),
                    session_id: SessionId::new(),
                    usage: Usage::default(),
                    turns: 1,
                    tool_calls: 0,
                    structured_output: None,
                    schema_warnings: None,
                    skill_diagnostics: None,
                }),
            })
        }
        async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = RuntimeSessionAdapter::ephemeral();
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
    use meerkat_core::lifecycle::run_control::RunControlCommand;
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
                run_result: Some(RunResult {
                    text: "slow done".into(),
                    session_id: SessionId::new(),
                    usage: Usage::default(),
                    turns: 1,
                    tool_calls: 0,
                    structured_output: None,
                    schema_warnings: None,
                    skill_diagnostics: None,
                }),
            })
        }
        async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = RuntimeSessionAdapter::ephemeral();
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

/// Gate A4 (part 1): resolve_without_result sends CompletedWithoutResult
/// when executor returns run_result: None.
#[tokio::test]
async fn completion_handle_resolves_without_result() {
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_control::RunControlCommand;
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
                run_result: None, // No RunResult
            })
        }
        async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = RuntimeSessionAdapter::ephemeral();
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
        "executor returning run_result: None should resolve as CompletedWithoutResult, got {result:?}"
    );
}

/// Gate A5: reset_runtime resolves all pending waiters.
#[tokio::test]
async fn reset_runtime_resolves_pending_waiters() {
    // Register without executor so inputs queue but don't process
    let adapter = RuntimeSessionAdapter::ephemeral();
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
    let adapter = RuntimeSessionAdapter::ephemeral();
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

/// Test that BoundaryApplied fires with correct receipt on success.
#[tokio::test]
async fn successful_execution_fires_boundary_applied() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_control::RunControlCommand;
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
                run_result: None,
            })
        }

        async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let adapter = RuntimeSessionAdapter::ephemeral();
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
        is.current_state(),
        InputLifecycleState::Consumed,
        "Successful execution should consume the input"
    );

    // Runtime should be back to Attached (executor still connected)
    let state = adapter.runtime_state(&sid).await.unwrap();
    assert_eq!(state, RuntimeState::Attached);
}
