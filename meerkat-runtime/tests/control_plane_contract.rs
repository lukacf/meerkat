#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Phase 0 external-boundary contract tests for runtime control-plane seams.
//!
//! These stay out of the fast suite on purpose. They exercise the live
//! `RuntimeSessionAdapter` surface and its out-of-band control channel rather
//! than only in-crate helpers.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use chrono::Utc;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::{RunResult, SessionId, Usage};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PeerConvention, PeerInput,
    ResponseProgressPhase,
};
use meerkat_runtime::{
    InputAbandonReason, InputLifecycleState, InputTerminalOutcome, RuntimeSessionAdapter,
    RuntimeState, SessionServiceRuntimeExt,
};

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
    })
}

fn make_run_result(text: &str) -> RunResult {
    RunResult {
        text: text.into(),
        session_id: SessionId::new(),
        usage: Usage::default(),
        turns: 1,
        tool_calls: 0,
        structured_output: None,
        schema_warnings: None,
        skill_diagnostics: None,
    }
}

struct RecordingExecutor {
    apply_calls: Arc<AtomicUsize>,
    stop_calls: Arc<AtomicUsize>,
    run_result: Option<RunResult>,
}

#[async_trait::async_trait]
impl CoreExecutor for RecordingExecutor {
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
            run_result: self.run_result.clone(),
        })
    }

    async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
        if matches!(command, RunControlCommand::StopRuntimeExecutor { .. }) {
            self.stop_calls.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[tokio::test]
#[ignore = "Phase 0 external boundary contract"]
async fn control_plane_contract_reset_terminates_waited_progress_work_without_running_it() {
    let adapter = RuntimeSessionAdapter::ephemeral();
    let runtime: &dyn SessionServiceRuntimeExt = &adapter;
    let sid = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let stop_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                stop_calls: Arc::clone(&stop_calls),
                run_result: Some(make_run_result("should not run")),
            }),
        )
        .await;

    let input = make_progress_input("reset");
    let input_id = input.id().clone();
    let (outcome, handle) = runtime
        .accept_input_with_completion(&sid, input)
        .await
        .unwrap();
    assert!(outcome.is_accepted());
    assert!(
        handle.is_some(),
        "accepted input should expose a wait handle"
    );

    runtime.reset_runtime(&sid).await.unwrap();

    let result = handle.unwrap().wait().await;
    assert!(
        matches!(result, CompletionOutcome::RuntimeTerminated(_)),
        "reset should terminate queued waiters, got {result:?}"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "queued progress input should never reach apply when reset preempts it"
    );
    assert_eq!(
        stop_calls.load(Ordering::SeqCst),
        0,
        "reset should not need to send a stop-runtime-executor control"
    );
    assert_eq!(
        runtime.runtime_state(&sid).await.unwrap(),
        RuntimeState::Idle
    );
    assert!(
        runtime.list_active_inputs(&sid).await.unwrap().is_empty(),
        "reset should clear all active inputs"
    );

    let state = runtime.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(state.current_state(), InputLifecycleState::Abandoned);
    assert!(matches!(
        state.terminal_outcome(),
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Reset,
        })
    ));
}

#[tokio::test]
#[ignore = "Phase 0 external boundary contract"]
async fn control_plane_contract_stop_runtime_executor_preempts_queued_progress_work() {
    let adapter = RuntimeSessionAdapter::ephemeral();
    let runtime: &dyn SessionServiceRuntimeExt = &adapter;
    let sid = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let stop_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                stop_calls: Arc::clone(&stop_calls),
                run_result: Some(make_run_result("should not run")),
            }),
        )
        .await;

    let input = make_progress_input("stop");
    let input_id = input.id().clone();
    let (outcome, handle) = runtime
        .accept_input_with_completion(&sid, input)
        .await
        .unwrap();
    assert!(outcome.is_accepted());
    assert!(
        handle.is_some(),
        "accepted input should expose a wait handle"
    );

    adapter
        .stop_runtime_executor(
            &sid,
            RunControlCommand::StopRuntimeExecutor {
                reason: "shutdown contract".into(),
            },
        )
        .await
        .unwrap();

    let result = handle.unwrap().wait().await;
    assert!(
        matches!(result, CompletionOutcome::RuntimeTerminated(_)),
        "stop-runtime-executor should terminate queued waiters, got {result:?}"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        0,
        "out-of-band stop should beat queued ordinary work"
    );
    assert_eq!(
        stop_calls.load(Ordering::SeqCst),
        1,
        "the executor should observe exactly one stop-runtime-executor control"
    );
    assert_eq!(
        runtime.runtime_state(&sid).await.unwrap(),
        RuntimeState::Stopped
    );
    assert!(
        runtime.list_active_inputs(&sid).await.unwrap().is_empty(),
        "stopping the runtime should drain active inputs from the ledger"
    );

    let state = runtime.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(state.current_state(), InputLifecycleState::Abandoned);
    assert!(matches!(
        state.terminal_outcome(),
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Destroyed,
        })
    ));
}

#[tokio::test]
#[ignore = "Phase 0 external boundary contract"]
async fn control_plane_contract_retire_drains_waited_progress_work_to_completion() {
    let adapter = RuntimeSessionAdapter::ephemeral();
    let runtime: &dyn SessionServiceRuntimeExt = &adapter;
    let sid = SessionId::new();
    let apply_calls = Arc::new(AtomicUsize::new(0));
    let stop_calls = Arc::new(AtomicUsize::new(0));

    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::clone(&apply_calls),
                stop_calls: Arc::clone(&stop_calls),
                run_result: None,
            }),
        )
        .await;

    let input = make_progress_input("retire");
    let input_id = input.id().clone();
    let (outcome, handle) = runtime
        .accept_input_with_completion(&sid, input)
        .await
        .unwrap();
    assert!(outcome.is_accepted());
    assert!(
        handle.is_some(),
        "accepted input should expose a wait handle"
    );

    let report = runtime.retire_runtime(&sid).await.unwrap();
    assert_eq!(
        report.inputs_pending_drain, 1,
        "retire should preserve already-queued work for drain"
    );
    assert_eq!(report.inputs_abandoned, 0);

    let result = handle.unwrap().wait().await;
    assert!(
        matches!(result, CompletionOutcome::CompletedWithoutResult),
        "retire+drain should complete the queued waiter through the runtime, got {result:?}"
    );
    assert_eq!(
        apply_calls.load(Ordering::SeqCst),
        1,
        "retired runtime should still drain queued work exactly once"
    );
    assert_eq!(
        stop_calls.load(Ordering::SeqCst),
        0,
        "retire should not stop the executor when it can drain queued work"
    );
    assert_eq!(
        runtime.runtime_state(&sid).await.unwrap(),
        RuntimeState::Retired
    );
    assert!(
        runtime.list_active_inputs(&sid).await.unwrap().is_empty(),
        "retire+drain should leave no active inputs behind"
    );

    let state = runtime.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(state.current_state(), InputLifecycleState::Consumed);
}

#[tokio::test]
#[ignore = "Phase 0 external boundary contract"]
async fn control_plane_contract_retire_without_runtime_loop_abandons_waited_work() {
    let adapter = RuntimeSessionAdapter::ephemeral();
    let runtime: &dyn SessionServiceRuntimeExt = &adapter;
    let sid = SessionId::new();

    adapter.register_session(sid.clone()).await;

    let input = make_progress_input("retire-without-loop");
    let input_id = input.id().clone();
    let (outcome, handle) = runtime
        .accept_input_with_completion(&sid, input)
        .await
        .unwrap();
    assert!(outcome.is_accepted());

    let report = runtime.retire_runtime(&sid).await.unwrap();
    assert_eq!(
        report.inputs_pending_drain, 0,
        "retire without a live loop must not leave future drain work behind"
    );
    assert_eq!(report.inputs_abandoned, 1);

    let result = handle.unwrap().wait().await;
    assert!(
        matches!(result, CompletionOutcome::RuntimeTerminated(_)),
        "retire without a runtime loop should terminate the waiter, got {result:?}"
    );
    assert_eq!(
        runtime.runtime_state(&sid).await.unwrap(),
        RuntimeState::Retired
    );
    assert!(
        runtime.list_active_inputs(&sid).await.unwrap().is_empty(),
        "retire without a loop should not leave active inputs behind"
    );

    let state = runtime.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(state.current_state(), InputLifecycleState::Abandoned);
    assert!(matches!(
        state.terminal_outcome(),
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Retired,
        })
    ));
}
