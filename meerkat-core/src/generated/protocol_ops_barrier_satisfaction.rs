// @generated — protocol helpers for `ops_barrier_satisfaction`
// Composition: mob_bundle, Producer: ops_lifecycle, Effect: WaitAllSatisfied
// Closure policy: AckRequired
// Liveness: eventual feedback under task-scheduling fairness

use crate::error::AgentError;
use crate::lifecycle::identifiers::{RunId, WaitRequestId};
use crate::ops::OperationId;
use crate::ops_lifecycle::WaitAllSatisfied;
use crate::turn_execution_authority::{
    TurnExecutionAuthority, TurnExecutionInput, TurnExecutionMutator, TurnExecutionTransition,
};

#[derive(Debug, Clone)]
pub struct OpsBarrierSatisfactionObligation {
    pub wait_request_id: WaitRequestId,
    pub operation_ids: Vec<OperationId>,
}

pub fn accept_wait_all_satisfied(source: WaitAllSatisfied) -> OpsBarrierSatisfactionObligation {
    OpsBarrierSatisfactionObligation {
        wait_request_id: source.wait_request_id,
        operation_ids: source.operation_ids,
    }
}

pub fn submit_ops_barrier_satisfied(
    authority: &mut TurnExecutionAuthority,
    obligation: OpsBarrierSatisfactionObligation,
    run_id: RunId,
) -> Result<TurnExecutionTransition, AgentError> {
    let transition = authority.apply(TurnExecutionInput::OpsBarrierSatisfied {
        run_id: run_id,
        operation_ids: obligation.operation_ids,
    })?;
    Ok(transition)
}
