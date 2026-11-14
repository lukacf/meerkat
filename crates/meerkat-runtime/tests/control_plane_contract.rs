#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Phase 0 external-boundary contract tests for runtime control-plane seams.
//!
//! These exercise the live `MeerkatMachine` surface and its out-of-band
//! effect channel rather than only in-crate helpers.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::{fs, path::Path};

use chrono::Utc;
use meerkat_core::lifecycle::core_executor::{
    CoreApplyOutput, CoreApplyTerminal, CoreExecutor, CoreExecutorError,
};
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::{RunResult, SessionId, Usage};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PeerConvention, PeerInput,
    ResponseProgressPhase,
};
use meerkat_runtime::store::load_runtime_state;
use meerkat_runtime::{
    InMemoryRuntimeStore, InputAbandonReason, InputLifecycleState, InputTerminalOutcome,
    LogicalRuntimeId, MeerkatMachine, RuntimeState, RuntimeStore, SessionServiceRuntimeExt,
};

fn make_progress_input(label: &str) -> Input {
    Input::Peer(PeerInput {
        directed_interaction_id: None,
        objective_id: None,
        system_prompts: Vec::new(),
        injected_context: Vec::new(),
        sender_taint: None,
        header: InputHeader {
            ingress_context: None,
            retained_resume: None,
            authority_association: None,
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
            requester: None,
        },
        convention: Some(PeerConvention::ResponseProgress {
            request_id: format!("req-{label}"),
            phase: ResponseProgressPhase::InProgress,
        }),
        content: format!("progress-{label}").into(),
        payload: Some(serde_json::json!({ "label": label })),
        handling_mode: None,
        in_reply_to: None,
    })
}

fn make_run_result(text: &str) -> RunResult {
    RunResult {
        reasoning_effort: None,
        text: text.into(),
        session_id: SessionId::new(),
        usage: Usage::default(),
        turns: 1,
        tool_calls: 0,
        terminal_cause_kind: None,
        structured_output: None,
        extraction_error: None,
        schema_warnings: None,
        skill_diagnostics: None,
        run_usage: None,
        request_usage: Vec::new(),
    }
}

#[test]
fn control_plane_contract_only_runtime_loop_handoff_can_terminalize_stop() {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(crate_root.join("src/control_plane.rs"))
        .expect("control_plane.rs should be readable");
    let direct_start = source
        .find("pub(crate) async fn apply_executor_effect")
        .expect("apply_executor_effect should exist");
    let direct_end = source[direct_start..]
        .find("/// Typed handoff produced")
        .expect("runtime-loop effect handoff should follow apply_executor_effect");
    let loop_start = source
        .find("pub(crate) async fn apply_runtime_loop_executor_effect")
        .expect("apply_runtime_loop_executor_effect should exist");
    let loop_end = source[loop_start..]
        .find("/// Drain any ready executor effects")
        .expect("drain_ready_executor_effects should follow runtime-loop effect application");

    let split_terminalizers = [
        "notify_runtime_executor_exited",
        "finalize_runtime_stopped",
        "resolve_all_terminated",
    ];
    let direct_body = &source[direct_start..direct_start + direct_end];
    assert!(
        direct_body.contains("requires the runtime-loop terminal handoff path"),
        "direct executor effect helper must reject stop effects"
    );
    assert_eq!(
        direct_body.matches("terminalize_async_stop").count(),
        0,
        "direct executor effect helper must have no stop terminalizer"
    );

    let path = "runtime-loop executor effect";
    let body = &source[loop_start..loop_start + loop_end];
    let offenders = split_terminalizers
        .iter()
        .copied()
        .filter(|needle| body.contains(needle))
        .collect::<Vec<_>>();
    assert!(
        offenders.is_empty(),
        "{path} must use canonical async stop terminalization; found split terminalizers: \
         {offenders:?}"
    );
    assert_eq!(
        body.matches("terminalize_async_stop_announcing(").count(),
        1,
        "{path} should call terminalize_async_stop_announcing exactly once"
    );
    assert_eq!(
        body.matches("terminalize_async_stop(").count(),
        0,
        "{path} should tell the host about the inputs its stop terminalizes"
    );
}

struct RecordingExecutor {
    apply_calls: Arc<AtomicUsize>,
    stop_calls: Arc<AtomicUsize>,
    terminal: Option<CoreApplyTerminal>,
}

#[async_trait::async_trait]
impl CoreExecutor for RecordingExecutor {
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
            self.terminal.clone(),
        ))
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        self.stop_calls.fetch_add(1, Ordering::SeqCst);
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
                terminal: Some(CoreApplyTerminal::RunResult(Box::new(make_run_result(
                    "should not run",
                )))),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

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

    let result = handle
        .unwrap()
        .wait()
        .await
        .expect("completion waiter should resolve");
    assert!(
        matches!(result, CompletionOutcome::RuntimeTerminated { .. }),
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
        stored.seed.terminal_outcome,
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
                terminal: Some(CoreApplyTerminal::RunResult(Box::new(make_run_result(
                    "should not run",
                )))),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

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
        .stop_runtime_executor(&sid, "shutdown contract")
        .await
        .unwrap();

    let result = handle
        .unwrap()
        .wait()
        .await
        .expect("completion waiter should resolve");
    assert!(
        matches!(result, CompletionOutcome::RuntimeTerminated { .. }),
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
        stored.seed.terminal_outcome,
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Stopped,
        })
    ));
}

/// Counts the runless-terminal tells of `machine`.
fn count_runless_terminal_tells(machine: &MeerkatMachine) -> Arc<AtomicUsize> {
    let tells = Arc::new(AtomicUsize::new(0));
    machine.set_runless_input_terminal_hint(Arc::new({
        let tells = Arc::clone(&tells);
        move || {
            tells.fetch_add(1, Ordering::SeqCst);
        }
    }));
    tells
}

#[tokio::test]
async fn control_plane_contract_a_stop_that_terminalizes_queued_work_tells_the_host_once() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let runtime: &dyn SessionServiceRuntimeExt = &*adapter;
    let sid = SessionId::new();
    let tells = count_runless_terminal_tells(&adapter);
    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(RecordingExecutor {
                apply_calls: Arc::new(AtomicUsize::new(0)),
                stop_calls: Arc::new(AtomicUsize::new(0)),
                terminal: Some(CoreApplyTerminal::RunResult(Box::new(make_run_result(
                    "should not run",
                )))),
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

    let (outcome, handle) = runtime
        .accept_input_with_completion(&sid, make_progress_input("stop-tells"))
        .await
        .unwrap();
    assert!(outcome.is_accepted());
    assert_eq!(tells.load(Ordering::SeqCst), 0, "admission tells nothing");

    adapter
        .stop_runtime_executor(&sid, "stop tells the host")
        .await
        .unwrap();
    let result = handle
        .unwrap()
        .wait()
        .await
        .expect("completion waiter should resolve");
    assert!(matches!(
        result,
        CompletionOutcome::RuntimeTerminated { .. }
    ));
    assert_eq!(
        tells.load(Ordering::SeqCst),
        1,
        "the stop that terminalizes the queued input tells the host once"
    );
}

#[tokio::test]
async fn control_plane_contract_a_retirement_without_runtime_loop_tells_the_host_once() {
    let adapter = Arc::new(MeerkatMachine::ephemeral());
    let runtime: &dyn SessionServiceRuntimeExt = &*adapter;
    let sid = SessionId::new();
    let tells = count_runless_terminal_tells(&adapter);
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");

    let (outcome, _handle) = runtime
        .accept_input_with_completion(&sid, make_progress_input("retire-tells"))
        .await
        .unwrap();
    assert!(outcome.is_accepted());

    let report = runtime.retire_runtime(&sid).await.unwrap();
    assert_eq!(report.inputs_abandoned, 1);
    assert_eq!(
        tells.load(Ordering::SeqCst),
        1,
        "the retirement that abandons the queued input tells the host once"
    );
}

fn persistent_contract_machine() -> Arc<MeerkatMachine> {
    Arc::new(
        MeerkatMachine::persistent_without_blobs(
            Arc::new(InMemoryRuntimeStore::new()) as Arc<dyn RuntimeStore>
        )
        .expect("persistent machine"),
    )
}

async fn reset_tells_its_newly_terminal_inputs_once(adapter: Arc<MeerkatMachine>) {
    let runtime: &dyn SessionServiceRuntimeExt = &*adapter;
    let sid = SessionId::new();
    let tells = count_runless_terminal_tells(&adapter);
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");
    runtime
        .reset_runtime(&sid)
        .await
        .expect("reset with nothing queued");
    assert_eq!(
        tells.load(Ordering::SeqCst),
        0,
        "a reset that abandons nothing tells nothing"
    );

    let (outcome, _handle) = runtime
        .accept_input_with_completion(&sid, make_progress_input("reset-tells"))
        .await
        .unwrap();
    assert!(outcome.is_accepted());
    runtime
        .reset_runtime(&sid)
        .await
        .expect("reset the queued input");
    assert_eq!(
        tells.load(Ordering::SeqCst),
        1,
        "the reset that abandons the queued input tells the host once"
    );
}

#[tokio::test]
async fn control_plane_contract_a_reset_that_terminalizes_queued_work_tells_the_host_once() {
    reset_tells_its_newly_terminal_inputs_once(Arc::new(MeerkatMachine::ephemeral())).await;
    reset_tells_its_newly_terminal_inputs_once(persistent_contract_machine()).await;
}

async fn destroy_tells_its_newly_terminal_inputs_once(adapter: Arc<MeerkatMachine>) {
    let runtime: &dyn SessionServiceRuntimeExt = &*adapter;
    let tells = count_runless_terminal_tells(&adapter);
    let idle = SessionId::new();
    adapter
        .register_session(idle.clone())
        .await
        .expect("register the idle session");
    meerkat_runtime::traits::RuntimeControlPlane::destroy(
        &*adapter,
        &LogicalRuntimeId::for_session(&idle),
    )
    .await
    .expect("destroy with nothing queued");
    assert_eq!(
        tells.load(Ordering::SeqCst),
        0,
        "a destroy that abandons nothing tells nothing"
    );

    let sid = SessionId::new();
    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");
    let (outcome, _handle) = runtime
        .accept_input_with_completion(&sid, make_progress_input("destroy-tells"))
        .await
        .unwrap();
    assert!(outcome.is_accepted());
    let report = meerkat_runtime::traits::RuntimeControlPlane::destroy(
        &*adapter,
        &LogicalRuntimeId::for_session(&sid),
    )
    .await
    .expect("destroy the queued input");
    assert_eq!(report.inputs_abandoned, 1);
    assert_eq!(
        tells.load(Ordering::SeqCst),
        1,
        "the destroy that abandons the queued input tells the host once"
    );
}

#[tokio::test]
async fn control_plane_contract_a_destroy_that_terminalizes_queued_work_tells_the_host_once() {
    destroy_tells_its_newly_terminal_inputs_once(Arc::new(MeerkatMachine::ephemeral())).await;
    destroy_tells_its_newly_terminal_inputs_once(persistent_contract_machine()).await;
}

#[tokio::test]
async fn control_plane_contract_stop_runtime_executor_persists_stopped_state_without_loop() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let adapter = Arc::new(
        MeerkatMachine::persistent_without_blobs(Arc::clone(&store) as Arc<dyn RuntimeStore>)
            .expect("persistent machine"),
    );
    let runtime: &dyn SessionServiceRuntimeExt = &*adapter;
    let sid = SessionId::new();

    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");
    let _bindings = adapter
        .prepare_bindings(sid.clone())
        .await
        .expect("prepare generated runtime binding before accepting stopped waiter");

    let input = make_progress_input("persistent-stop");
    let input_id = input.id().clone();
    let (outcome, handle) = runtime
        .accept_input_with_completion(&sid, input)
        .await
        .unwrap();
    assert!(outcome.is_accepted());
    let handle = handle.expect("accepted input should expose a wait handle");

    adapter
        .stop_runtime_executor(&sid, "persistent stopped state")
        .await
        .unwrap();

    match handle
        .wait()
        .await
        .expect("completion waiter should resolve")
    {
        CompletionOutcome::RuntimeTerminated { reason, .. } => {
            assert_eq!(reason, "runtime stopped");
        }
        other => panic!("expected runtime stopped termination, got {other:?}"),
    }
    assert_eq!(
        runtime.runtime_state(&sid).await.unwrap(),
        RuntimeState::Stopped
    );

    let runtime_id = LogicalRuntimeId::for_session(&sid);
    assert_eq!(
        load_runtime_state(store.as_ref(), &runtime_id)
            .await
            .unwrap(),
        Some(RuntimeState::Stopped),
        "persistent stop terminalization should commit Stopped as durable runtime truth"
    );

    let stored = runtime.input_state(&sid, &input_id).await.unwrap().unwrap();
    assert_eq!(stored.seed.phase, InputLifecycleState::Abandoned);
    assert!(matches!(
        stored.seed.terminal_outcome,
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
                terminal: None,
            }),
        )
        .await
        .expect("runtime executor registration should succeed");

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

    let result = handle
        .unwrap()
        .wait()
        .await
        .expect("completion waiter should resolve");
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

    adapter
        .register_session(sid.clone())
        .await
        .expect("register session");

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

    let result = handle
        .unwrap()
        .wait()
        .await
        .expect("completion waiter should resolve");
    assert!(
        matches!(result, CompletionOutcome::RuntimeTerminated { .. }),
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
        stored.seed.terminal_outcome,
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Retired,
        })
    ));
}
