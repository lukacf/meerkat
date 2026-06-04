// @generated — protocol helpers for `ops_barrier_satisfaction`
// Composition: meerkat_mob_seam, Producer: meerkat, Effect: WaitAllSatisfied
// Closure policy: AckRequired
// Liveness: eventual feedback under task-scheduling fairness

use crate::handles::{DslTransitionError, TurnStateHandle};
use crate::{OperationId, RunId};

#[derive(Debug, Clone)]
pub struct OpsBarrierSatisfactionObligation {
    pub run_id: RunId,
    pub operation_ids: std::collections::BTreeSet<OperationId>,
}

pub fn submit_ops_barrier_satisfied(
    handle: &(impl TurnStateHandle + ?Sized),
    obligation: OpsBarrierSatisfactionObligation,
) -> Result<(), DslTransitionError> {
    handle.ops_barrier_satisfied(
        obligation.run_id,
        obligation.operation_ids.into_iter().collect(),
    )
}
