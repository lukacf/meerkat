#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::PathBuf;

use meerkat_machine_kernels::compat_generated::loop_iteration;
use meerkat_machine_schema::compat::types as kernel_types;
use meerkat_mob::protocol_flow_loop_until_evaluation::{
    FlowLoopUntilEvaluationContext, accept_evaluate_until_condition, submit_until_condition_failed,
    submit_until_condition_met,
};

struct EmptyContext;

impl FlowLoopUntilEvaluationContext for EmptyContext {}

fn generated_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/generated")
}

#[test]
fn flow_loop_until_evaluation_generated_helpers_are_typed() {
    let path = generated_dir().join("protocol_flow_loop_until_evaluation.rs");
    let source = fs::read_to_string(&path).expect("protocol_flow_loop_until_evaluation");
    for forbidden in ["KernelState", "KernelInput", "KernelEffect", "KernelValue"] {
        assert!(
            !source.contains(forbidden),
            "{} should not contain `{forbidden}`",
            path.display()
        );
    }
    assert!(
        source.contains("FlowLoopUntilEvaluationObligation"),
        "{} should expose a typed obligation",
        path.display()
    );
    assert!(
        source.contains("CompositionRefusal") && source.contains("FlowLoopUntilEvaluationContext"),
        "{} should expose typed refusal and context contracts",
        path.display()
    );
}

#[test]
fn flow_loop_until_evaluation_round_trips_typed_values() {
    let effect = loop_iteration::effects::EvaluateUntilCondition {
        loop_instance_id: kernel_types::LoopInstanceId::from("loop-1"),
        iteration: 2,
        parent_frame_id: kernel_types::FrameId::from("frame-1"),
        parent_node_id: kernel_types::FlowNodeId::from("node-1"),
        loop_id: kernel_types::LoopId::from("loop-spec"),
    };

    let obligation = accept_evaluate_until_condition(effect).expect("accept");
    let met = submit_until_condition_met(obligation.clone(), &EmptyContext).expect("submit met");
    let failed = submit_until_condition_failed(obligation, &EmptyContext).expect("submit failed");

    match met {
        loop_iteration::Input::UntilConditionMet(payload) => {
            assert_eq!(
                payload.loop_instance_id,
                kernel_types::LoopInstanceId::from("loop-1")
            );
            assert_eq!(payload.iteration, 2);
        }
        other => panic!("expected UntilConditionMet input, got {other:?}"),
    }

    match failed {
        loop_iteration::Input::UntilConditionFailed(payload) => {
            assert_eq!(
                payload.loop_instance_id,
                kernel_types::LoopInstanceId::from("loop-1")
            );
            assert_eq!(payload.iteration, 2);
        }
        other => panic!("expected UntilConditionFailed input, got {other:?}"),
    }
}
