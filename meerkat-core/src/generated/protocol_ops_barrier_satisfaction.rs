// @generated — protocol helpers for `ops_barrier_satisfaction`
// Composition: mob_bundle, Producer: ops_lifecycle, Effect: WaitAllSatisfied
// Closure policy: AckRequired
// Liveness: eventual feedback under task-scheduling fairness

use crate::lifecycle::RunId;
use crate::lifecycle::identifiers::WaitRequestId;
use crate::ops::OperationId;
use crate::ops_lifecycle::WaitAllSatisfied;
use crate::turn_execution_authority::TurnExecutionInput;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompositionRefusal {
    #[error("context violation: {detail}")]
    ContextViolation { detail: String },
    #[error("codegen invariant: {detail}")]
    CodegenInvariant { detail: String },
}

pub trait OpsBarrierSatisfactionContext {
    fn run_id(&self) -> RunId;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpsBarrierSatisfactionObligation {
    pub wait_request_id: WaitRequestId,
    pub operation_ids: Vec<OperationId>,
}

pub fn accept_wait_all_satisfied(
    source: WaitAllSatisfied,
) -> Result<OpsBarrierSatisfactionObligation, CompositionRefusal> {
    Ok(OpsBarrierSatisfactionObligation {
        wait_request_id: source.wait_request_id,
        operation_ids: source.operation_ids,
    })
}

pub fn submit_ops_barrier_satisfied<C: OpsBarrierSatisfactionContext>(
    obligation: OpsBarrierSatisfactionObligation,
    context: &C,
) -> Result<TurnExecutionInput, CompositionRefusal> {
    Ok(TurnExecutionInput::OpsBarrierSatisfied {
        run_id: context.run_id(),
        operation_ids: obligation.operation_ids,
    })
}
