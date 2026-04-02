#![allow(clippy::expect_used, clippy::unwrap_used)]

use meerkat_machine_kernels::generated::loop_iteration;
use meerkat_machine_kernels::{KernelInput, KernelValue};
use std::collections::BTreeMap;

fn str_val(s: &str) -> KernelValue {
    KernelValue::String(s.into())
}
fn u64_val(n: u64) -> KernelValue {
    KernelValue::U64(n)
}
fn frame_id(s: &str) -> KernelValue {
    str_val(s)
}
fn loop_inst(s: &str) -> KernelValue {
    str_val(s)
}

fn start_loop_input(loop_instance_id: &str, max_iterations: u64) -> KernelInput {
    KernelInput {
        variant: "StartLoop".into(),
        fields: BTreeMap::from([
            ("loop_instance_id".into(), loop_inst(loop_instance_id)),
            ("max_iterations".into(), u64_val(max_iterations)),
            ("parent_frame_id".into(), frame_id("parent-frame")),
            ("parent_node_id".into(), str_val("parent-node")),
            ("loop_id".into(), str_val("loop-spec")),
            ("depth".into(), u64_val(1)),
        ]),
    }
}

fn body_frame_started_input(loop_instance_id: &str, fid: &str, iteration: u64) -> KernelInput {
    KernelInput {
        variant: "BodyFrameStarted".into(),
        fields: BTreeMap::from([
            ("loop_instance_id".into(), loop_inst(loop_instance_id)),
            ("frame_id".into(), frame_id(fid)),
            ("iteration".into(), u64_val(iteration)),
        ]),
    }
}

fn body_frame_completed_input(loop_instance_id: &str, iteration: u64) -> KernelInput {
    KernelInput {
        variant: "BodyFrameCompleted".into(),
        fields: BTreeMap::from([
            ("loop_instance_id".into(), loop_inst(loop_instance_id)),
            ("iteration".into(), u64_val(iteration)),
        ]),
    }
}

/// Advance through one complete iteration: BodyFrameStarted then BodyFrameCompleted
fn advance_one_iteration(
    loop_instance_id: &str,
    iteration: u64,
    state: meerkat_machine_kernels::KernelState,
) -> meerkat_machine_kernels::KernelState {
    let started = body_frame_started_input(loop_instance_id, "frame-x", iteration);
    let state = loop_iteration::transition(&state, &started)
        .expect("BodyFrameStarted")
        .next_state;
    let completed = body_frame_completed_input(loop_instance_id, iteration);
    loop_iteration::transition(&state, &completed)
        .expect("BodyFrameCompleted")
        .next_state
}

/// REQ-04 (loop side): Loop machine requests body frame start after StartLoop
#[test]
fn test_start_loop_emits_request_body_frame_start() {
    let state = loop_iteration::initial_state().expect("init");
    let start = start_loop_input("loop-a", 3);
    let outcome = loop_iteration::transition(&state, &start).expect("StartLoop");

    assert!(
        outcome
            .effects
            .iter()
            .any(|e| e.variant == "RequestBodyFrameStart"),
        "StartLoop should emit RequestBodyFrameStart, got: {:?}",
        outcome
            .effects
            .iter()
            .map(|e| &e.variant)
            .collect::<Vec<_>>()
    );
    let req = outcome
        .effects
        .iter()
        .find(|e| e.variant == "RequestBodyFrameStart")
        .unwrap();
    assert_eq!(
        req.fields.get("loop_instance_id"),
        Some(&loop_inst("loop-a")),
        "RequestBodyFrameStart should carry loop_instance_id"
    );
}

/// REQ-04 (loop side): BodyFrameCompleted advances current_iteration and emits EvaluateUntilCondition
#[test]
fn test_loop_iteration_advances_current_iteration() {
    let state = loop_iteration::initial_state().expect("init");

    // Start loop
    let outcome =
        loop_iteration::transition(&state, &start_loop_input("loop-a", 5)).expect("StartLoop");
    let state = outcome.next_state;

    // BodyFrameStarted
    let outcome =
        loop_iteration::transition(&state, &body_frame_started_input("loop-a", "frame-1", 0))
            .expect("BodyFrameStarted");
    let state = outcome.next_state;

    // BodyFrameCompleted → should emit EvaluateUntilCondition and increment iteration
    let outcome = loop_iteration::transition(&state, &body_frame_completed_input("loop-a", 0))
        .expect("BodyFrameCompleted");
    let state = outcome.next_state;

    assert!(
        outcome
            .effects
            .iter()
            .any(|e| e.variant == "EvaluateUntilCondition"),
        "BodyFrameCompleted should emit EvaluateUntilCondition, got: {:?}",
        outcome
            .effects
            .iter()
            .map(|e| &e.variant)
            .collect::<Vec<_>>()
    );

    let iteration = state.fields.get("current_iteration").cloned();
    assert_eq!(
        iteration,
        Some(u64_val(1)),
        "current_iteration should be 1 after first body frame"
    );
}

/// REQ-04: UntilConditionMet → Completed + LoopCompleted
#[test]
fn test_until_condition_met_completes_loop() {
    let state = loop_iteration::initial_state().expect("init");
    let outcome =
        loop_iteration::transition(&state, &start_loop_input("loop-a", 5)).expect("StartLoop");
    let state = advance_one_iteration("loop-a", 0, outcome.next_state);

    let met = KernelInput {
        variant: "UntilConditionMet".into(),
        fields: BTreeMap::from([
            ("loop_instance_id".into(), loop_inst("loop-a")),
            ("iteration".into(), u64_val(0)),
        ]),
    };
    let outcome = loop_iteration::transition(&state, &met).expect("UntilConditionMet");

    assert_eq!(outcome.next_state.phase, "Completed");
    assert!(
        outcome.effects.iter().any(|e| e.variant == "LoopCompleted"),
        "UntilConditionMet should emit LoopCompleted, got: {:?}",
        outcome
            .effects
            .iter()
            .map(|e| &e.variant)
            .collect::<Vec<_>>()
    );
}

/// REQ-04 + CHOKE-02: UntilConditionFailed → re-requests body frame for next iteration
#[test]
fn test_until_condition_failed_requests_next_body_frame() {
    let state = loop_iteration::initial_state().expect("init");
    let outcome =
        loop_iteration::transition(&state, &start_loop_input("loop-a", 5)).expect("StartLoop");
    let state = advance_one_iteration("loop-a", 0, outcome.next_state);

    let failed = KernelInput {
        variant: "UntilConditionFailed".into(),
        fields: BTreeMap::from([
            ("loop_instance_id".into(), loop_inst("loop-a")),
            ("iteration".into(), u64_val(0)),
        ]),
    };
    let outcome = loop_iteration::transition(&state, &failed).expect("UntilConditionFailed");

    assert_eq!(
        outcome.next_state.phase, "Running",
        "Should still be Running after condition failed"
    );
    assert!(
        outcome
            .effects
            .iter()
            .any(|e| e.variant == "RequestBodyFrameStart"),
        "UntilConditionFailed should emit RequestBodyFrameStart for next iteration, got: {:?}",
        outcome
            .effects
            .iter()
            .map(|e| &e.variant)
            .collect::<Vec<_>>()
    );
}

/// REQ-04 + REQ-07: Loop exhausted when max_iterations reached
#[test]
fn test_loop_exhausted_when_max_iterations_reached() {
    let state = loop_iteration::initial_state().expect("init");
    // max_iterations = 2
    let outcome =
        loop_iteration::transition(&state, &start_loop_input("loop-a", 2)).expect("StartLoop");

    // Run first iteration, condition fails
    let state = advance_one_iteration("loop-a", 0, outcome.next_state);
    let failed = KernelInput {
        variant: "UntilConditionFailed".into(),
        fields: BTreeMap::from([
            ("loop_instance_id".into(), loop_inst("loop-a")),
            ("iteration".into(), u64_val(0)),
        ]),
    };
    // After 1 iteration, current_iteration=1, max=2, so NOT exhausted → re-requests
    let outcome =
        loop_iteration::transition(&state, &failed).expect("UntilConditionFailed after iter 1");
    assert_eq!(
        outcome.next_state.phase, "Running",
        "Should still be Running after first condition failed"
    );

    // Run second iteration, condition fails again
    let state = advance_one_iteration("loop-a", 1, outcome.next_state);
    // current_iteration=2, max=2 → exhausted
    let failed2 = KernelInput {
        variant: "UntilConditionFailed".into(),
        fields: BTreeMap::from([
            ("loop_instance_id".into(), loop_inst("loop-a")),
            ("iteration".into(), u64_val(1)),
        ]),
    };
    let outcome =
        loop_iteration::transition(&state, &failed2).expect("UntilConditionFailed after iter 2");

    assert_eq!(
        outcome.next_state.phase, "Exhausted",
        "Loop should be Exhausted when current_iteration >= max_iterations"
    );
    assert!(
        outcome.effects.iter().any(|e| e.variant == "LoopExhausted"),
        "Should emit LoopExhausted when iterations exhausted, got: {:?}",
        outcome
            .effects
            .iter()
            .map(|e| &e.variant)
            .collect::<Vec<_>>()
    );
}

/// BodyFrameFailed → Failed + LoopFailed
#[test]
fn test_body_frame_failed_transitions_to_failed() {
    let state = loop_iteration::initial_state().expect("init");
    let outcome =
        loop_iteration::transition(&state, &start_loop_input("loop-b", 3)).expect("StartLoop");
    let state = loop_iteration::transition(
        &outcome.next_state,
        &body_frame_started_input("loop-b", "frame-y", 0),
    )
    .expect("BodyFrameStarted")
    .next_state;

    let failed = KernelInput {
        variant: "BodyFrameFailed".into(),
        fields: BTreeMap::from([
            ("loop_instance_id".into(), loop_inst("loop-b")),
            ("iteration".into(), u64_val(0)),
        ]),
    };
    let outcome = loop_iteration::transition(&state, &failed).expect("BodyFrameFailed");

    assert_eq!(outcome.next_state.phase, "Failed");
    assert!(
        outcome.effects.iter().any(|e| e.variant == "LoopFailed"),
        "BodyFrameFailed should emit LoopFailed, got: {:?}",
        outcome
            .effects
            .iter()
            .map(|e| &e.variant)
            .collect::<Vec<_>>()
    );
}

/// BodyFrameCanceled → Canceled + LoopCanceled
#[test]
fn test_body_frame_canceled_transitions_to_canceled() {
    let state = loop_iteration::initial_state().expect("init");
    let outcome =
        loop_iteration::transition(&state, &start_loop_input("loop-c", 3)).expect("StartLoop");
    let state = loop_iteration::transition(
        &outcome.next_state,
        &body_frame_started_input("loop-c", "frame-z", 0),
    )
    .expect("BodyFrameStarted")
    .next_state;

    let canceled = KernelInput {
        variant: "BodyFrameCanceled".into(),
        fields: BTreeMap::from([
            ("loop_instance_id".into(), loop_inst("loop-c")),
            ("iteration".into(), u64_val(0)),
        ]),
    };
    let outcome = loop_iteration::transition(&state, &canceled).expect("BodyFrameCanceled");

    assert_eq!(outcome.next_state.phase, "Canceled");
    assert!(
        outcome.effects.iter().any(|e| e.variant == "LoopCanceled"),
        "BodyFrameCanceled should emit LoopCanceled, got: {:?}",
        outcome
            .effects
            .iter()
            .map(|e| &e.variant)
            .collect::<Vec<_>>()
    );
}

/// CancelLoop → Canceled + LoopCanceled
#[test]
fn test_cancel_loop_transitions_to_canceled() {
    let state = loop_iteration::initial_state().expect("init");
    let outcome =
        loop_iteration::transition(&state, &start_loop_input("loop-d", 3)).expect("StartLoop");
    let state = outcome.next_state;

    let cancel = KernelInput {
        variant: "CancelLoop".into(),
        fields: BTreeMap::from([("loop_instance_id".into(), loop_inst("loop-d"))]),
    };
    let outcome = loop_iteration::transition(&state, &cancel).expect("CancelLoop");

    assert_eq!(outcome.next_state.phase, "Canceled");
    assert!(
        outcome.effects.iter().any(|e| e.variant == "LoopCanceled"),
        "CancelLoop should emit LoopCanceled, got: {:?}",
        outcome
            .effects
            .iter()
            .map(|e| &e.variant)
            .collect::<Vec<_>>()
    );
}
