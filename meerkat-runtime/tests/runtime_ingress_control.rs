#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Phase 1 red-ok chokepoint tests for runtime ingress and control wiring.

use std::sync::Arc;

use chrono::Utc;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::ops::{OpEvent, OperationId};
use meerkat_core::types::{RunResult, SessionId, Usage};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::input::{
    ContinuationInput, Input, InputDurability, InputHeader, InputOrigin, InputVisibility,
    OperationInput, PromptInput,
};
use meerkat_runtime::{
    ApplyMode, DrainPolicy, InputAbandonReason, InputLifecycleState, InputTerminalOutcome,
    PolicyDecision, QueueMode, RoutingDisposition, RuntimeSessionAdapter, RuntimeState,
    SessionServiceRuntimeExt, WakeMode,
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
            run_result: Some(make_run_result("runtime ingress ok")),
        })
    }

    async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

#[derive(Default)]
struct RecordingBatchExecutor {
    seen_contributors: Arc<tokio::sync::Mutex<Vec<Vec<InputId>>>>,
}

#[async_trait::async_trait]
impl CoreExecutor for RecordingBatchExecutor {
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        self.seen_contributors
            .lock()
            .await
            .push(primitive.contributing_input_ids().to_vec());
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
            run_result: Some(make_run_result("batched runtime ingress ok")),
        })
    }

    async fn control(&mut self, _cmd: RunControlCommand) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

#[tokio::test]
#[ignore = "Phase 1 red-ok runtime ingress/control suite"]
async fn runtime_ingress_control_red_ok_accepts_prompt_and_resolves_completion_handle() {
    let adapter = RuntimeSessionAdapter::ephemeral();
    let runtime: &dyn SessionServiceRuntimeExt = &adapter;
    let sid = SessionId::new();
    adapter
        .register_session_with_executor(sid.clone(), Box::new(ResultExecutor))
        .await;

    let input = make_prompt("phase 1 runtime ingress");
    let input_id = input.id().clone();
    let (outcome, handle) = runtime
        .accept_input_with_completion(&sid, input)
        .await
        .expect("accept input");
    assert!(outcome.is_accepted(), "prompt should be admitted");

    let completion = handle.expect("accepted input should expose a wait handle");
    let result = completion.wait().await;
    assert!(
        matches!(result, CompletionOutcome::Completed(ref run) if run.text == "runtime ingress ok"),
        "runtime-backed ingress should resolve through the completion handle"
    );

    let state = runtime
        .input_state(&sid, &input_id)
        .await
        .expect("input state")
        .expect("input record");
    assert_eq!(state.current_state(), InputLifecycleState::Consumed);
    assert_eq!(
        runtime.runtime_state(&sid).await.expect("runtime state"),
        RuntimeState::Idle
    );
    assert!(
        runtime
            .list_active_inputs(&sid)
            .await
            .expect("active inputs")
            .is_empty(),
        "completed ingress should leave no active inputs behind"
    );
}

#[tokio::test]
#[ignore = "Phase 1 red-ok runtime ingress/control suite"]
async fn runtime_ingress_control_red_ok_reset_preempts_queued_input_once() {
    let adapter = RuntimeSessionAdapter::ephemeral();
    let runtime: &dyn SessionServiceRuntimeExt = &adapter;
    let sid = SessionId::new();
    adapter.register_session(sid.clone()).await;

    let input = make_prompt("queued before reset");
    let input_id = input.id().clone();
    let (outcome, handle) = runtime
        .accept_input_with_completion(&sid, input)
        .await
        .expect("accept queued input");
    assert!(outcome.is_accepted());

    runtime.reset_runtime(&sid).await.expect("reset runtime");

    let result = handle
        .expect("queued input should expose a handle")
        .wait()
        .await;
    assert!(
        matches!(result, CompletionOutcome::RuntimeTerminated(_)),
        "queued ingress should resolve as terminated when control-plane reset wins"
    );

    let state = runtime
        .input_state(&sid, &input_id)
        .await
        .expect("input state")
        .expect("input record");
    assert_eq!(state.current_state(), InputLifecycleState::Abandoned);
    assert!(matches!(
        state.terminal_outcome(),
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Reset,
        })
    ));
}

#[tokio::test]
#[ignore = "Phase 3 runtime input taxonomy closure"]
async fn runtime_ingress_control_closed_taxonomy_uses_explicit_continuation_and_operation_inputs() {
    let continuation = Input::Continuation(ContinuationInput::terminal_peer_response(
        "terminal peer response injected into session state",
    ));
    let continuation_policy = meerkat_runtime::DefaultPolicyTable::resolve(&continuation, true);
    assert_eq!(continuation.kind_id().0, "continuation");
    assert_eq!(continuation_policy.apply_mode, ApplyMode::StageRunBoundary);
    assert_eq!(continuation_policy.wake_mode, WakeMode::WakeIfIdle);
    assert_eq!(continuation_policy.drain_policy, DrainPolicy::SteerBatch);
    assert_eq!(
        continuation_policy.routing_disposition,
        RoutingDisposition::Steer
    );

    let operation = Input::Operation(OperationInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::System,
            durability: InputDurability::Derived,
            visibility: InputVisibility {
                transcript_eligible: false,
                operator_eligible: false,
            },
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        operation_id: OperationId::new(),
        event: OpEvent::Cancelled {
            id: OperationId::new(),
        },
    });
    let operation_policy = meerkat_runtime::DefaultPolicyTable::resolve(&operation, true);
    assert_eq!(operation.kind_id().0, "operation");
    assert_eq!(operation_policy.apply_mode, ApplyMode::Ignore);
    assert_eq!(operation_policy.queue_mode, QueueMode::Priority);
    assert_eq!(
        operation_policy,
        PolicyDecision {
            apply_mode: ApplyMode::Ignore,
            wake_mode: WakeMode::None,
            queue_mode: QueueMode::Priority,
            consume_point: meerkat_runtime::ConsumePoint::OnAccept,
            interrupt_policy: meerkat_runtime::InterruptPolicy::None,
            drain_policy: DrainPolicy::Ignore,
            routing_disposition: RoutingDisposition::Drop,
            record_transcript: false,
            emit_operator_content: false,
            policy_version: meerkat_runtime::DEFAULT_POLICY_VERSION,
        }
    );
}

#[tokio::test]
#[ignore = "Phase 3 multi-contributor staged-run contract"]
async fn runtime_ingress_control_batches_same_boundary_contributors_in_runtime_order() {
    let adapter = RuntimeSessionAdapter::ephemeral();
    let runtime: &dyn SessionServiceRuntimeExt = &adapter;
    let sid = SessionId::new();
    let seen = Arc::new(tokio::sync::Mutex::new(Vec::<Vec<InputId>>::new()));

    adapter
        .register_session_with_executor(
            sid.clone(),
            Box::new(RecordingBatchExecutor {
                seen_contributors: Arc::clone(&seen),
            }),
        )
        .await;

    let first = make_prompt("batched one");
    let first_id = first.id().clone();
    let second = make_prompt("batched two");
    let second_id = second.id().clone();

    let (_, first_handle) = runtime
        .accept_input_with_completion(&sid, first)
        .await
        .expect("accept first input");
    let (_, second_handle) = runtime
        .accept_input_with_completion(&sid, second)
        .await
        .expect("accept second input");

    let first_result = first_handle.expect("first handle").wait().await;
    let second_result = second_handle.expect("second handle").wait().await;
    assert!(matches!(first_result, CompletionOutcome::Completed(_)));
    assert!(matches!(second_result, CompletionOutcome::Completed(_)));

    let batches = seen.lock().await;
    assert_eq!(
        batches.len(),
        1,
        "expected exactly one staged runtime batch"
    );
    assert_eq!(batches[0], vec![first_id, second_id]);
}
