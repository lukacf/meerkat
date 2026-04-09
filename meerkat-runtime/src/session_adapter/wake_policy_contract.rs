#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use super::*;
use crate::{
    CompletionHandle, CompletionOutcome, Input, InputDurability, InputHeader, InputLifecycleState,
    InputOrigin, InputTerminalOutcome, InputVisibility, LogicalRuntimeId, PeerConvention,
    PeerInput, PromptInput, ResponseProgressPhase, RuntimeControlPlane, RuntimeState,
    SessionServiceRuntimeExt,
};
use chrono::Utc;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::SessionId;
use tokio::sync::Notify;

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

fn make_progress_input(label: &str) -> Input {
    Input::Peer(PeerInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Peer {
                peer_id: "peer-1".into(),
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
        blocks: None,
        handling_mode: None,
    })
}

fn receipt(run_id: RunId, primitive: &RunPrimitive) -> RunBoundaryReceipt {
    RunBoundaryReceipt {
        run_id,
        boundary: RunApplyBoundary::RunStart,
        contributing_input_ids: primitive.contributing_input_ids().to_vec(),
        conversation_digest: None,
        message_count: 0,
        sequence: 0,
    }
}

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
            receipt: receipt(run_id, &primitive),
            session_snapshot: None,
            terminal: None,
            run_result: None,
        })
    }

    async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

struct BlockingExecutor {
    apply_calls: Arc<AtomicUsize>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait::async_trait]
impl CoreExecutor for BlockingExecutor {
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let call = self.apply_calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            self.entered.notify_waiters();
            self.release.notified().await;
        }
        Ok(CoreApplyOutput {
            receipt: receipt(run_id, &primitive),
            session_snapshot: None,
            terminal: None,
            run_result: None,
        })
    }

    async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

async fn wait_for_apply_count(apply_calls: &Arc<AtomicUsize>, expected: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if apply_calls.load(Ordering::SeqCst) >= expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("executor apply count should advance");
}

async fn assert_completed_without_result(handle: Option<CompletionHandle>, context: &str) {
    let result = tokio::time::timeout(Duration::from_secs(2), handle.expect(context).wait())
        .await
        .expect("completion should resolve");
    assert!(
        matches!(result, CompletionOutcome::CompletedWithoutResult),
        "expected CompletedWithoutResult, got {result:?}"
    );
}

async fn assert_input_not_abandoned(
    adapter: &RuntimeSessionAdapter,
    session_id: &SessionId,
    input_id: &InputId,
    context: &str,
) {
    let state = adapter
        .input_state(session_id, input_id)
        .await
        .expect("input_state should succeed")
        .expect("input state should exist");
    assert!(
        state.current_state() != InputLifecycleState::Abandoned,
        "{context}: queued input should not be abandoned while a live loop still exists: {state:?}"
    );
    assert!(
        !matches!(
            state.terminal_outcome(),
            Some(InputTerminalOutcome::Abandoned { .. })
        ),
        "{context}: queued input should not expose an abandoned terminal outcome: {state:?}"
    );
}

#[tokio::test]
async fn wake_policy_contract_publish_attach_wakes_queued_input_after_registration() {
    let adapter = RuntimeSessionAdapter::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));

    adapter.register_session(session_id.clone()).await;

    let (outcome, handle) = adapter
        .accept_input_with_completion(&session_id, make_prompt("publish attach"))
        .await
        .expect("input should queue before executor attachment");
    assert!(outcome.is_accepted());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(CountingExecutor {
                apply_calls: Arc::clone(&apply_calls),
            }),
        )
        .await;

    assert_completed_without_result(handle, "queued input should expose a waiter").await;
    wait_for_apply_count(&apply_calls, 1).await;
    assert_eq!(
        SessionServiceRuntimeExt::runtime_state(&adapter, &session_id)
            .await
            .unwrap(),
        RuntimeState::Attached
    );
}

#[tokio::test]
async fn wake_policy_contract_accept_input_wakes_attached_runtime() {
    let adapter = RuntimeSessionAdapter::ephemeral();
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(CountingExecutor {
                apply_calls: Arc::clone(&apply_calls),
            }),
        )
        .await;

    let (outcome, handle) = adapter
        .accept_input_with_completion(&session_id, make_prompt("attached accept"))
        .await
        .expect("attached runtime should accept input");
    assert!(outcome.is_accepted());

    assert_completed_without_result(handle, "accepted input should expose a waiter").await;
    wait_for_apply_count(&apply_calls, 1).await;
}

#[tokio::test]
async fn wake_policy_contract_control_plane_ingest_wakes_attached_runtime() {
    let adapter = RuntimeSessionAdapter::ephemeral();
    let session_id = SessionId::new();
    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let apply_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(CountingExecutor {
                apply_calls: Arc::clone(&apply_calls),
            }),
        )
        .await;

    let outcome = RuntimeControlPlane::ingest(&adapter, &runtime_id, make_prompt("ingest wake"))
        .await
        .expect("control-plane ingest should succeed");
    assert!(outcome.is_accepted());

    wait_for_apply_count(&apply_calls, 1).await;
    assert!(
        adapter
            .list_active_inputs(&session_id)
            .await
            .unwrap()
            .is_empty(),
        "ingest wake should drain the accepted input"
    );
}

#[tokio::test]
async fn wake_policy_contract_recycle_wakes_preserved_work() {
    let adapter = RuntimeSessionAdapter::ephemeral();
    let session_id = SessionId::new();
    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let apply_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(CountingExecutor {
                apply_calls: Arc::clone(&apply_calls),
            }),
        )
        .await;

    let (outcome, handle) = adapter
        .accept_input_with_completion(&session_id, make_progress_input("recycle"))
        .await
        .expect("progress input should be accepted");
    assert!(outcome.is_accepted());

    let report = RuntimeControlPlane::recycle(&adapter, &runtime_id)
        .await
        .expect("recycle should preserve queued work");
    assert_eq!(report.inputs_transferred, 1);

    assert_completed_without_result(handle, "recycled input should keep its waiter").await;
    wait_for_apply_count(&apply_calls, 1).await;
    assert_eq!(
        SessionServiceRuntimeExt::runtime_state(&adapter, &session_id)
            .await
            .unwrap(),
        RuntimeState::Attached
    );
}

#[tokio::test]
async fn wake_policy_contract_best_effort_drops_extra_wakes_when_channel_is_full() {
    let adapter = RuntimeSessionAdapter::ephemeral_with_test_settings(1, false);
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            }),
        )
        .await;

    let (_, first) = adapter
        .accept_input_with_completion(&session_id, make_prompt("first"))
        .await
        .expect("first input should be accepted");
    entered.notified().await;

    assert!(
        adapter
            .signal_runtime_wake_for_test(&session_id)
            .await
            .expect("first test wake should resolve"),
        "the first synthetic wake should occupy the single-slot channel"
    );
    assert!(
        !adapter
            .signal_runtime_wake_for_test(&session_id)
            .await
            .expect("second test wake should resolve"),
        "best-effort wake should drop an extra signal when the channel is already full"
    );

    let (_, second) = adapter
        .accept_input_with_completion(&session_id, make_prompt("second"))
        .await
        .expect("second input should be accepted");
    let (_, third) = adapter
        .accept_input_with_completion(&session_id, make_prompt("third"))
        .await
        .expect("third input should still return promptly under best-effort wake");

    release.notify_waiters();

    assert_completed_without_result(first, "first input should complete").await;
    assert_completed_without_result(second, "second input should complete").await;
    assert_completed_without_result(third, "third input should complete").await;
    wait_for_apply_count(&apply_calls, 3).await;
}

#[tokio::test]
async fn wake_policy_contract_guaranteed_mode_waits_for_capacity_before_returning() {
    let adapter = Arc::new(RuntimeSessionAdapter::ephemeral_with_test_settings(1, true));
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            }),
        )
        .await;

    let (_, first) = adapter
        .accept_input_with_completion(&session_id, make_prompt("first"))
        .await
        .expect("first input should be accepted");
    entered.notified().await;

    assert!(
        adapter
            .signal_runtime_wake_for_test(&session_id)
            .await
            .expect("first guaranteed wake should resolve"),
        "the first guaranteed wake should occupy the single-slot channel"
    );

    let third_adapter = Arc::clone(&adapter);
    let third_session = session_id.clone();
    let third_task = tokio::spawn(async move {
        third_adapter
            .signal_runtime_wake_for_test(&third_session)
            .await
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !third_task.is_finished(),
        "guaranteed wake should wait for channel capacity instead of dropping the signal"
    );

    release.notify_waiters();

    let third_result = tokio::time::timeout(Duration::from_secs(2), third_task)
        .await
        .expect("guaranteed test wake should complete after capacity frees")
        .expect("guaranteed wake task should not panic")
        .expect("guaranteed wake should resolve");
    assert!(
        third_result,
        "guaranteed wake should eventually deliver once the receiver drains"
    );

    assert_completed_without_result(first, "first input should complete").await;
    wait_for_apply_count(&apply_calls, 1).await;
}

#[tokio::test]
async fn wake_policy_contract_service_retire_keeps_queued_work_when_best_effort_channel_is_only_full()
 {
    let adapter = RuntimeSessionAdapter::ephemeral_with_test_settings(1, false);
    let session_id = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            }),
        )
        .await;

    let (_, first) = adapter
        .accept_input_with_completion(&session_id, make_prompt("first"))
        .await
        .expect("first input should be accepted");
    entered.notified().await;

    assert!(
        adapter
            .signal_runtime_wake_for_test(&session_id)
            .await
            .expect("synthetic wake should resolve"),
        "synthetic wake should occupy the single-slot channel"
    );

    let second_input = make_prompt("second");
    let second_id = second_input.id().clone();
    let (_, second) = adapter
        .accept_input_with_completion(&session_id, second_input)
        .await
        .expect("second input should be accepted while the wake channel is full");

    let report = adapter
        .retire_runtime(&session_id)
        .await
        .expect("retire should succeed");
    assert_eq!(
        report.inputs_abandoned, 0,
        "a full best-effort wake channel must not be treated as a dead runtime loop"
    );
    assert_input_not_abandoned(&adapter, &session_id, &second_id, "service retire").await;

    release.notify_waiters();

    assert_completed_without_result(first, "first input should complete").await;
    assert_completed_without_result(second, "second input should still drain").await;
    wait_for_apply_count(&apply_calls, 2).await;
}

#[tokio::test]
async fn wake_policy_contract_control_plane_retire_keeps_queued_work_when_best_effort_channel_is_only_full()
 {
    let adapter = RuntimeSessionAdapter::ephemeral_with_test_settings(1, false);
    let session_id = SessionId::new();
    let runtime_id = LogicalRuntimeId::new(session_id.to_string());
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());

    adapter
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            }),
        )
        .await;

    let (_, first) = adapter
        .accept_input_with_completion(&session_id, make_prompt("first"))
        .await
        .expect("first input should be accepted");
    entered.notified().await;

    assert!(
        adapter
            .signal_runtime_wake_for_test(&session_id)
            .await
            .expect("synthetic wake should resolve"),
        "synthetic wake should occupy the single-slot channel"
    );

    let second_input = make_prompt("second");
    let second_id = second_input.id().clone();
    let (_, second) = adapter
        .accept_input_with_completion(&session_id, second_input)
        .await
        .expect("second input should be accepted while the wake channel is full");

    let report = RuntimeControlPlane::retire(&adapter, &runtime_id)
        .await
        .expect("control-plane retire should succeed");
    assert_eq!(
        report.inputs_abandoned, 0,
        "control-plane retire must not abandon queued work when the loop is still alive"
    );
    assert_input_not_abandoned(&adapter, &session_id, &second_id, "control-plane retire").await;

    release.notify_waiters();

    assert_completed_without_result(first, "first input should complete").await;
    assert_completed_without_result(second, "second input should still drain").await;
    wait_for_apply_count(&apply_calls, 2).await;
}
