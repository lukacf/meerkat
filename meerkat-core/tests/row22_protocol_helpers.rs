#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::PathBuf;

use meerkat_core::generated::protocol_ops_barrier_satisfaction::{
    accept_wait_all_satisfied, submit_ops_barrier_satisfied,
};
use meerkat_core::lifecycle::RunId;
use meerkat_core::lifecycle::identifiers::WaitRequestId;
use meerkat_core::ops::OperationId;
use meerkat_core::ops_lifecycle::WaitAllSatisfied;
use meerkat_core::turn_execution_authority::TurnExecutionInput;

fn generated_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/generated")
}

#[test]
fn ops_barrier_satisfaction_generated_helpers_are_typed() {
    let path = generated_dir().join("protocol_ops_barrier_satisfaction.rs");
    let source = fs::read_to_string(&path).expect("protocol_ops_barrier_satisfaction");
    for forbidden in ["KernelState", "KernelInput", "KernelEffect", "KernelValue"] {
        assert!(
            !source.contains(forbidden),
            "{} should not contain `{forbidden}`",
            path.display()
        );
    }
    assert!(
        source.contains("OpsBarrierSatisfactionObligation"),
        "{} should expose a typed obligation",
        path.display()
    );
}

#[test]
fn ops_barrier_satisfaction_round_trips_typed_values() {
    let wait_request_id = WaitRequestId::new();
    let operation_ids = vec![OperationId::new(), OperationId::new()];
    let source = WaitAllSatisfied {
        wait_request_id,
        operation_ids: operation_ids.clone(),
    };

    let obligation = accept_wait_all_satisfied(source);
    let run_id = RunId::new();
    let input = submit_ops_barrier_satisfied(obligation, run_id.clone());

    match input {
        TurnExecutionInput::OpsBarrierSatisfied {
            run_id: actual_run_id,
            operation_ids: actual_operation_ids,
        } => {
            assert_eq!(actual_run_id, run_id);
            assert_eq!(actual_operation_ids, operation_ids);
        }
        other => panic!("expected OpsBarrierSatisfied input, got {other:?}"),
    }
}
