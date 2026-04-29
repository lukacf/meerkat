#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Phase 0 external-boundary contract tests for runtime control-plane seams.
//!
//! These exercise the live `MeerkatMachine` surface and its out-of-band
//! control channel rather than only in-crate helpers.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::{fs, path::Path};

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
    InMemoryRuntimeStore, InputAbandonReason, InputLifecycleState, InputTerminalOutcome,
    LogicalRuntimeId, MeerkatMachine, RuntimeState, RuntimeStore, SessionServiceRuntimeExt,
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
        payload: Some(serde_json::json!({ "label": label })),
        blocks: None,
        handling_mode: None,
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

#[test]
fn control_plane_contract_async_stop_terminalization_has_one_call_site() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(crate_root.join("src/control_plane.rs"))
        .expect("control_plane.rs should be readable");
    let start = source
        .find("pub(crate) async fn apply_executor_control")
        .expect("apply_executor_control should exist");
    let end = source[start..]
        .find("/// Drain any ready executor control commands")
        .expect("drain_ready_executor_controls should follow apply_executor_control");
    let apply_body = &source[start..start + end];

    let split_terminalizers = [
        "notify_runtime_executor_exited",
        "finalize_runtime_stopped",
        "resolve_all_terminated",
    ];
    let offenders = split_terminalizers
        .iter()
        .copied()
        .filter(|needle| apply_body.contains(needle))
        .collect::<Vec<_>>();
    assert!(
        offenders.is_empty(),
        "async stop terminalization must be a single canonical call from \
         apply_executor_control; found split terminalizers: {offenders:?}"
    );
    assert_eq!(
        apply_body.matches("terminalize_async_stop(").count(),
        1,
        "apply_executor_control should call terminalize_async_stop exactly once"
    );
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
            terminal: None,
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
async fn control_plane_contract_reset_terminates_waited_progress_work_without_running_it() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let runtime: &dyn SessionServiceRuntimeExt = &*adapter;
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

    let stored = runtime.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(stored.seed.phase, InputLifecycleState::Abandoned);
    assert!(matches!(
        stored.state.terminal_outcome(),
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Reset,
        })
    ));
}

#[tokio::test]
async fn control_plane_contract_stop_runtime_executor_preempts_queued_progress_work() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let runtime: &dyn SessionServiceRuntimeExt = &*adapter;
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

    let stored = runtime.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(stored.seed.phase, InputLifecycleState::Abandoned);
    assert!(matches!(
        stored.state.terminal_outcome(),
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Stopped,
        })
    ));
}

#[tokio::test]
async fn control_plane_contract_stop_runtime_executor_persists_stopped_state_without_loop() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent_without_blobs(
        Arc::clone(&store) as Arc<dyn RuntimeStore>
    ));
    let runtime: &dyn SessionServiceRuntimeExt = &*adapter;
    let sid = SessionId::new();

    adapter.register_session(sid.clone()).await;

    let input = make_progress_input("persistent-stop");
    let input_id = input.id().clone();
    let (outcome, handle) = runtime
        .accept_input_with_completion(&sid, input)
        .await
        .unwrap();
    assert!(outcome.is_accepted());
    let handle = handle.expect("accepted input should expose a wait handle");

    adapter
        .stop_runtime_executor(
            &sid,
            RunControlCommand::StopRuntimeExecutor {
                reason: "persistent stopped state".into(),
            },
        )
        .await
        .unwrap();

    match handle.wait().await {
        CompletionOutcome::RuntimeTerminated(reason) => {
            assert_eq!(reason, "runtime stopped");
        }
        other => panic!("expected runtime stopped termination, got {other:?}"),
    }
    assert_eq!(
        runtime.runtime_state(&sid).await.unwrap(),
        RuntimeState::Stopped
    );

    let runtime_id = LogicalRuntimeId::new(sid.to_string());
    assert_eq!(
        store.load_runtime_state(&runtime_id).await.unwrap(),
        Some(RuntimeState::Stopped),
        "persistent stop terminalization should commit Stopped as durable runtime truth"
    );

    let stored = runtime.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(stored.seed.phase, InputLifecycleState::Abandoned);
    assert!(matches!(
        stored.state.terminal_outcome(),
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Stopped,
        })
    ));
}

#[tokio::test]
async fn control_plane_contract_retire_drains_waited_progress_work_to_completion() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let runtime: &dyn SessionServiceRuntimeExt = &*adapter;
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

    let stored = runtime.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(stored.seed.phase, InputLifecycleState::Consumed);
}

#[tokio::test]
async fn control_plane_contract_retire_without_runtime_loop_abandons_waited_work() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let runtime: &dyn SessionServiceRuntimeExt = &*adapter;
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

    let stored = runtime.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(stored.seed.phase, InputLifecycleState::Abandoned);
    assert!(matches!(
        stored.state.terminal_outcome(),
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Retired,
        })
    ));
}
