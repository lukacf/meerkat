#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

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
}
