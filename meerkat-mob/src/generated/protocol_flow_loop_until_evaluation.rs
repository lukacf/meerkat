// @generated — protocol helpers for `flow_loop_until_evaluation`
// Composition: flow_frame_loop, Producer: loop_iteration, Effect: EvaluateUntilCondition
// Closure policy: AckRequired
// Liveness: eventual feedback under task-scheduling fairness

use crate::error::MobError;
use crate::ids::{FlowNodeId, FrameId, LoopId, LoopInstanceId};
use crate::runtime::loop_iteration_authority::{
    LoopIterationAuthority, LoopIterationInput, LoopIterationMutator, LoopIterationTransition,
    LoopUntilEvaluationRequested, inputs,
};

#[derive(Debug, Clone)]
pub struct FlowLoopUntilEvaluationObligation {
    pub loop_instance_id: LoopInstanceId,
    pub iteration: u32,
    pub parent_frame_id: FrameId,
    pub parent_node_id: FlowNodeId,
    pub loop_id: LoopId,
}

pub fn accept_evaluate_until_condition(
    source: LoopUntilEvaluationRequested,
) -> FlowLoopUntilEvaluationObligation {
    FlowLoopUntilEvaluationObligation {
        loop_instance_id: source.loop_instance_id,
        iteration: source.iteration,
        parent_frame_id: source.parent_frame_id,
        parent_node_id: source.parent_node_id,
        loop_id: source.loop_id,
    }
}

pub fn submit_until_condition_met(
    authority: &mut LoopIterationAuthority,
    obligation: FlowLoopUntilEvaluationObligation,
) -> Result<LoopIterationTransition, MobError> {
    let transition = authority.apply(LoopIterationInput::UntilConditionMet(
        inputs::UntilConditionMet {
            loop_instance_id: obligation.loop_instance_id,
            iteration: obligation.iteration,
        },
    ))?;
    Ok(transition)
}

pub fn submit_until_condition_failed(
    authority: &mut LoopIterationAuthority,
    obligation: FlowLoopUntilEvaluationObligation,
) -> Result<LoopIterationTransition, MobError> {
    let transition = authority.apply(LoopIterationInput::UntilConditionFailed(
        inputs::UntilConditionFailed {
            loop_instance_id: obligation.loop_instance_id,
            iteration: obligation.iteration,
        },
    ))?;
    Ok(transition)
}
