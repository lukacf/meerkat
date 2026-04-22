// @generated — protocol helpers for `flow_loop_until_evaluation`
// Composition: flow_frame_loop, Producer: loop_iteration, Effect: EvaluateUntilCondition
// Closure policy: AckRequired
// Liveness: eventual feedback under task-scheduling fairness

use crate::flow_machine_types::{
    local_flow_node_id, local_frame_id, local_loop_id, local_loop_instance_id,
};
use crate::ids::{FlowNodeId, FrameId, LoopId, LoopInstanceId};
use meerkat_machine_kernels::compat_generated::loop_iteration;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompositionRefusal {
    #[error("context violation: {detail}")]
    ContextViolation { detail: String },
    #[error("codegen invariant: {detail}")]
    CodegenInvariant { detail: String },
}

pub trait FlowLoopUntilEvaluationContext {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowLoopUntilEvaluationObligation {
    pub loop_instance_id: LoopInstanceId,
    pub iteration: u32,
    pub parent_frame_id: FrameId,
    pub parent_node_id: FlowNodeId,
    pub loop_id: LoopId,
}

pub fn accept_evaluate_until_condition(
    source: loop_iteration::effects::EvaluateUntilCondition,
) -> Result<FlowLoopUntilEvaluationObligation, CompositionRefusal> {
    Ok(FlowLoopUntilEvaluationObligation {
        loop_instance_id: local_loop_instance_id(&source.loop_instance_id),
        iteration: source.iteration,
        parent_frame_id: local_frame_id(&source.parent_frame_id),
        parent_node_id: local_flow_node_id(&source.parent_node_id),
        loop_id: local_loop_id(&source.loop_id),
    })
}

pub fn submit_until_condition_met<C: FlowLoopUntilEvaluationContext>(
    obligation: FlowLoopUntilEvaluationObligation,
    _context: &C,
) -> Result<loop_iteration::Input, CompositionRefusal> {
    Ok(loop_iteration::Input::UntilConditionMet(
        loop_iteration::inputs::UntilConditionMet {
            loop_instance_id: crate::flow_machine_types::loop_instance_id(
                &obligation.loop_instance_id,
            ),
            iteration: obligation.iteration,
        },
    ))
}

pub fn submit_until_condition_failed<C: FlowLoopUntilEvaluationContext>(
    obligation: FlowLoopUntilEvaluationObligation,
    _context: &C,
) -> Result<loop_iteration::Input, CompositionRefusal> {
    Ok(loop_iteration::Input::UntilConditionFailed(
        loop_iteration::inputs::UntilConditionFailed {
            loop_instance_id: crate::flow_machine_types::loop_instance_id(
                &obligation.loop_instance_id,
            ),
            iteration: obligation.iteration,
        },
    ))
}
