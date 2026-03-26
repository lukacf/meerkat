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
    assert_eq!(
        field(&waiting.next_state, "pending_op_refs"),
        Some(&KernelValue::None)
    );

    let registered = turn_execution::transition(
        &waiting.next_state,
        &input(
            "RegisterPendingOps",
            vec![
                ("run_id", string("run-conversation")),
                (
                    "op_refs",
                    KernelValue::Seq(vec![string("op-a"), string("op-b")]),
                ),
                (
                    "barrier_operation_ids",
                    KernelValue::Seq(vec![string("op-a")]),
                ),
                ("has_barrier_ops", KernelValue::Bool(true)),
            ],
        ),
    )
    .expect("register pending ops");
    assert_eq!(registered.next_state.phase, "WaitingForOps");
    assert_eq!(
        field(&registered.next_state, "pending_op_refs"),
        Some(&KernelValue::Seq(vec![string("op-a"), string("op-b"),]))
    );
    assert_eq!(
        field(&registered.next_state, "has_barrier_ops"),
        Some(&KernelValue::Bool(true))
    );
    assert_eq!(
        field(&registered.next_state, "barrier_satisfied"),
        Some(&KernelValue::Bool(false))
    );

    // ToolCallsResolved must be rejected while barrier_satisfied == false
    let rejected = turn_execution::transition(
        &registered.next_state,
        &input(
            "ToolCallsResolved",
            vec![("run_id", string("run-conversation"))],
        ),
    );
    assert!(
        rejected.is_err(),
        "ToolCallsResolved must fail when barrier_satisfied is false"
    );

    let barrier_satisfied = turn_execution::transition(
        &registered.next_state,
        &input(
            "OpsBarrierSatisfied",
            vec![
                ("run_id", string("run-conversation")),
                ("operation_ids", KernelValue::Seq(vec![string("op-a")])),
            ],
        ),
    )
    .expect("ops barrier satisfied");
    assert_eq!(barrier_satisfied.transition, "OpsBarrierSatisfied");
    assert_eq!(barrier_satisfied.next_state.phase, "WaitingForOps");
    assert_eq!(
        field(&barrier_satisfied.next_state, "barrier_satisfied"),
        Some(&KernelValue::Bool(true))
    );

    let draining = turn_execution::transition(
        &barrier_satisfied.next_state,
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

/// Kernel witness: detached-only ops do not block ToolCallsResolved.
#[test]
fn turn_execution_kernel_detached_only_ops_do_not_block_resolution() {
    let state = turn_execution::initial_state().expect("initial state");
    let started = turn_execution::transition(
        &state,
        &input(
            "StartConversationRun",
            vec![("run_id", string("run-detached"))],
        ),
    )
    .expect("start");
    let applied = turn_execution::transition(
        &started.next_state,
        &input(
            "PrimitiveApplied",
            vec![
                ("run_id", string("run-detached")),
                ("admitted_content_shape", string("text")),
                ("vision_enabled", KernelValue::Bool(false)),
                ("image_tool_results_enabled", KernelValue::Bool(false)),
            ],
        ),
    )
    .expect("apply primitive");
    let waiting = turn_execution::transition(
        &applied.next_state,
        &input(
            "LlmReturnedToolCalls",
            vec![
                ("run_id", string("run-detached")),
                ("tool_count", KernelValue::U64(1)),
            ],
        ),
    )
    .expect("llm returns tool calls");

    // Register detached-only ops
    let registered = turn_execution::transition(
        &waiting.next_state,
        &input(
            "RegisterPendingOps",
            vec![
                ("run_id", string("run-detached")),
                (
                    "op_refs",
                    KernelValue::Seq(vec![string("op-d1"), string("op-d2")]),
                ),
                ("barrier_operation_ids", KernelValue::Seq(vec![])),
                ("has_barrier_ops", KernelValue::Bool(false)),
            ],
        ),
    )
    .expect("register detached ops");

    // barrier_satisfied stays true when no barrier ops
    assert_eq!(
        field(&registered.next_state, "barrier_satisfied"),
        Some(&KernelValue::Bool(true))
    );

    // ToolCallsResolved succeeds immediately — no OpsBarrierSatisfied needed
    let resolved = turn_execution::transition(
        &registered.next_state,
        &input(
            "ToolCallsResolved",
            vec![("run_id", string("run-detached"))],
        ),
    )
    .expect("tool calls resolved without barrier");
    assert_eq!(resolved.next_state.phase, "DrainingBoundary");
}

/// Kernel witness: mixed Barrier+Detached ops block until barrier is satisfied.
#[test]
fn turn_execution_kernel_mixed_barrier_detached_blocks_until_satisfied() {
    let state = turn_execution::initial_state().expect("initial state");
    let started = turn_execution::transition(
        &state,
        &input(
            "StartConversationRun",
            vec![("run_id", string("run-mixed"))],
        ),
    )
    .expect("start");
    let applied = turn_execution::transition(
        &started.next_state,
        &input(
            "PrimitiveApplied",
            vec![
                ("run_id", string("run-mixed")),
                ("admitted_content_shape", string("text")),
                ("vision_enabled", KernelValue::Bool(false)),
                ("image_tool_results_enabled", KernelValue::Bool(false)),
            ],
        ),
    )
    .expect("apply primitive");
    let waiting = turn_execution::transition(
        &applied.next_state,
        &input(
            "LlmReturnedToolCalls",
            vec![
                ("run_id", string("run-mixed")),
                ("tool_count", KernelValue::U64(2)),
            ],
        ),
    )
    .expect("llm returns tool calls");

    // Register mixed ops (one barrier + one detached)
    let registered = turn_execution::transition(
        &waiting.next_state,
        &input(
            "RegisterPendingOps",
            vec![
                ("run_id", string("run-mixed")),
                (
                    "op_refs",
                    KernelValue::Seq(vec![string("op-barrier"), string("op-detached")]),
                ),
                (
                    "barrier_operation_ids",
                    KernelValue::Seq(vec![string("op-barrier")]),
                ),
                ("has_barrier_ops", KernelValue::Bool(true)),
            ],
        ),
    )
    .expect("register mixed ops");

    assert_eq!(
        field(&registered.next_state, "barrier_satisfied"),
        Some(&KernelValue::Bool(false))
    );

    // ToolCallsResolved must be rejected
    let rejected = turn_execution::transition(
        &registered.next_state,
        &input("ToolCallsResolved", vec![("run_id", string("run-mixed"))]),
    );
    assert!(
        rejected.is_err(),
        "must block when barrier_satisfied is false"
    );

    // Satisfy barrier
    let satisfied = turn_execution::transition(
        &registered.next_state,
        &input(
            "OpsBarrierSatisfied",
            vec![
                ("run_id", string("run-mixed")),
                (
                    "operation_ids",
                    KernelValue::Seq(vec![string("op-barrier")]),
                ),
            ],
        ),
    )
    .expect("ops barrier satisfied");
    assert_eq!(
        field(&satisfied.next_state, "barrier_satisfied"),
        Some(&KernelValue::Bool(true))
    );

    // Now ToolCallsResolved succeeds
    let resolved = turn_execution::transition(
        &satisfied.next_state,
        &input("ToolCallsResolved", vec![("run_id", string("run-mixed"))]),
    )
    .expect("tool calls resolved");
    assert_eq!(resolved.next_state.phase, "DrainingBoundary");
}

/// Kernel witness: OpsBarrierSatisfied rejected when barrier is already satisfied.
#[test]
fn turn_execution_kernel_ops_barrier_satisfied_rejected_when_already_true() {
    let state = turn_execution::initial_state().expect("initial state");
    let started = turn_execution::transition(
        &state,
        &input("StartConversationRun", vec![("run_id", string("run-dup"))]),
    )
    .expect("start");
    let applied = turn_execution::transition(
        &started.next_state,
        &input(
            "PrimitiveApplied",
            vec![
                ("run_id", string("run-dup")),
                ("admitted_content_shape", string("text")),
                ("vision_enabled", KernelValue::Bool(false)),
                ("image_tool_results_enabled", KernelValue::Bool(false)),
            ],
        ),
    )
    .expect("apply primitive");
    let waiting = turn_execution::transition(
        &applied.next_state,
        &input(
            "LlmReturnedToolCalls",
            vec![
                ("run_id", string("run-dup")),
                ("tool_count", KernelValue::U64(1)),
            ],
        ),
    )
    .expect("llm returns tool calls");

    // Register with no barrier ops — barrier_satisfied stays true
    let registered = turn_execution::transition(
        &waiting.next_state,
        &input(
            "RegisterPendingOps",
            vec![
                ("run_id", string("run-dup")),
                ("op_refs", KernelValue::Seq(vec![])),
                ("barrier_operation_ids", KernelValue::Seq(vec![])),
                ("has_barrier_ops", KernelValue::Bool(false)),
            ],
        ),
    )
    .expect("register no-barrier ops");

    // OpsBarrierSatisfied should be rejected since barrier_satisfied is already true
    let rejected = turn_execution::transition(
        &registered.next_state,
        &input(
            "OpsBarrierSatisfied",
            vec![
                ("run_id", string("run-dup")),
                ("operation_ids", KernelValue::Seq(vec![])),
            ],
        ),
    );
    assert!(
        rejected.is_err(),
        "OpsBarrierSatisfied must be rejected when barrier is already satisfied"
    );
}
