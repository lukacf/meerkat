#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;

use meerkat_machine_kernels::generated::turn_execution;
use meerkat_machine_kernels::{KernelInput, KernelValue};

fn string(value: &str) -> KernelValue {
    KernelValue::String(value.to_string())
}

fn input(variant: &str, fields: Vec<(&str, KernelValue)>) -> KernelInput {
    KernelInput {
        variant: variant.to_string(),
        fields: fields
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn field<'a>(
    state: &'a meerkat_machine_kernels::KernelState,
    name: &str,
) -> Option<&'a KernelValue> {
    state.fields.get(name)
}

#[test]
fn turn_execution_kernel_tool_loop_yields_back_to_llm_after_boundary() {
    let state = turn_execution::initial_state().expect("initial state");
    let started = turn_execution::transition(
        &state,
        &input(
            "StartConversationRun",
            vec![("run_id", string("run-conversation"))],
        ),
    )
    .expect("start conversation");
    assert_eq!(started.transition, "StartConversationRun");
    assert_eq!(started.next_state.phase, "ApplyingPrimitive");
    assert_eq!(started.effects[0].variant, "RunStarted");

    let applied = turn_execution::transition(
        &started.next_state,
        &input(
            "PrimitiveApplied",
            vec![
                ("run_id", string("run-conversation")),
                ("admitted_content_shape", string("text")),
                ("vision_enabled", KernelValue::Bool(false)),
                ("image_tool_results_enabled", KernelValue::Bool(false)),
            ],
        ),
    )
    .expect("apply primitive");
    assert_eq!(applied.next_state.phase, "CallingLlm");

    let waiting = turn_execution::transition(
        &applied.next_state,
        &input(
            "LlmReturnedToolCalls",
            vec![
                ("run_id", string("run-conversation")),
                ("tool_count", KernelValue::U64(2)),
            ],
        ),
    )
    .expect("llm returns tool calls");
    assert_eq!(waiting.next_state.phase, "WaitingForOps");
    assert_eq!(
        field(&waiting.next_state, "tool_calls_pending"),
        Some(&KernelValue::U64(2))
    );

    let draining = turn_execution::transition(
        &waiting.next_state,
        &input(
            "ToolCallsResolved",
            vec![("run_id", string("run-conversation"))],
        ),
    )
    .expect("tool calls resolved");
    assert_eq!(draining.transition, "ToolCallsResolved");
    assert_eq!(draining.next_state.phase, "DrainingBoundary");
    assert_eq!(draining.effects[0].variant, "BoundaryApplied");
    assert_eq!(
        draining.effects[0].fields.get("boundary_sequence"),
        Some(&KernelValue::U64(1))
    );

    let continued = turn_execution::transition(
        &draining.next_state,
        &input(
            "BoundaryContinue",
            vec![("run_id", string("run-conversation"))],
        ),
    )
    .expect("boundary continue");
    assert_eq!(continued.transition, "BoundaryContinue");
    assert_eq!(continued.next_state.phase, "CallingLlm");
}

#[test]
fn turn_execution_kernel_immediate_context_completes_without_llm_loop() {
    let state = turn_execution::initial_state().expect("initial state");
    let started = turn_execution::transition(
        &state,
        &input(
            "StartImmediateContext",
            vec![("run_id", string("run-context"))],
        ),
    )
    .expect("start immediate context");
    assert_eq!(started.next_state.phase, "ApplyingPrimitive");

    let completed = turn_execution::transition(
        &started.next_state,
        &input(
            "PrimitiveApplied",
            vec![
                ("run_id", string("run-context")),
                ("admitted_content_shape", string("image")),
                ("vision_enabled", KernelValue::Bool(true)),
                ("image_tool_results_enabled", KernelValue::Bool(true)),
            ],
        ),
    )
    .expect("apply immediate context primitive");

    assert_eq!(completed.transition, "PrimitiveAppliedImmediateContext");
    assert_eq!(completed.next_state.phase, "Completed");
    assert_eq!(
        field(&completed.next_state, "terminal_outcome"),
        Some(&string("Completed"))
    );
    assert_eq!(
        field(&completed.next_state, "boundary_count"),
        Some(&KernelValue::U64(1))
    );
    assert_eq!(completed.effects[0].variant, "BoundaryApplied");
    assert_eq!(completed.effects[1].variant, "RunCompleted");
}

#[test]
fn turn_execution_kernel_cancel_and_failure_paths_emit_terminal_effects() {
    let state = turn_execution::initial_state().expect("initial state");
    let started = turn_execution::transition(
        &state,
        &input(
            "StartConversationRun",
            vec![("run_id", string("run-cancel"))],
        ),
    )
    .expect("start conversation");
    let applied = turn_execution::transition(
        &started.next_state,
        &input(
            "PrimitiveApplied",
            vec![
                ("run_id", string("run-cancel")),
                ("admitted_content_shape", string("text")),
                ("vision_enabled", KernelValue::Bool(false)),
                ("image_tool_results_enabled", KernelValue::Bool(false)),
            ],
        ),
    )
    .expect("apply primitive");
    let boundary_cancel = turn_execution::transition(
        &applied.next_state,
        &input(
            "CancelAfterBoundary",
            vec![("run_id", string("run-cancel"))],
        ),
    )
    .expect("request boundary cancellation");
    assert_eq!(
        field(&boundary_cancel.next_state, "cancel_after_boundary"),
        Some(&KernelValue::Bool(true))
    );

    let draining = turn_execution::transition(
        &boundary_cancel.next_state,
        &input(
            "LlmReturnedTerminal",
            vec![("run_id", string("run-cancel"))],
        ),
    )
    .expect("llm terminal");
    let cancelled = turn_execution::transition(
        &draining.next_state,
        &input("BoundaryComplete", vec![("run_id", string("run-cancel"))]),
    )
    .expect("boundary completes cancellation");
    assert_eq!(cancelled.transition, "BoundaryCompleteCancelsAfterBoundary");
    assert_eq!(cancelled.next_state.phase, "Cancelled");
    assert_eq!(cancelled.effects[0].variant, "RunCancelled");

    let state = turn_execution::initial_state().expect("initial state");
    let started = turn_execution::transition(
        &state,
        &input("StartImmediateAppend", vec![("run_id", string("run-fail"))]),
    )
    .expect("start immediate append");
    let failed = turn_execution::transition(
        &started.next_state,
        &input("FatalFailure", vec![("run_id", string("run-fail"))]),
    )
    .expect("fatal failure");
    assert_eq!(failed.transition, "FatalFailureFromApplyingPrimitive");
    assert_eq!(failed.next_state.phase, "Failed");
    assert_eq!(failed.effects[0].variant, "RunFailed");
}
