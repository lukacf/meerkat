// @generated — protocol helpers for `ops_barrier_satisfaction`
// Composition: meerkat_mob_seam, Producer: meerkat, Effect: WaitAllSatisfied
// Closure policy: AckRequired
// Liveness: eventual feedback under task-scheduling fairness

use crate::handles::{DslTransitionError, TurnStateHandle};
use crate::lifecycle::identifiers::WaitRequestId;
use crate::ops::OperationId;
use crate::ops_lifecycle::WaitAllSatisfied;

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
    handle: &(impl TurnStateHandle + ?Sized),
    obligation: OpsBarrierSatisfactionObligation,
) -> Result<(), DslTransitionError> {
    handle.ops_barrier_satisfied(
        obligation
            .operation_ids
            .iter()
            .map(ToString::to_string)
            .collect(),
    )
}
