// @generated — protocol helpers for `flow_loop_until_evaluation`
// Composition: flow_frame_loop, Producer: loop_iteration, Effect: EvaluateUntilCondition
// Closure policy: AckRequired
// Liveness: eventual feedback under task-scheduling fairness

use crate::error::MobError;
use crate::generated::loop_iteration;
use crate::ids::{FlowNodeId, FrameId, LoopId, LoopInstanceId};

pub(crate) type LoopIterationTransition = loop_iteration::Outcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoopUntilEvaluationRequested {
    pub loop_instance_id: LoopInstanceId,
    pub iteration: u32,
    pub parent_frame_id: FrameId,
    pub parent_node_id: FlowNodeId,
    pub loop_id: LoopId,
}

impl LoopUntilEvaluationRequested {
    pub(crate) fn from_effect(effect: &loop_iteration::Effect) -> Result<Self, MobError> {
        match effect {
            loop_iteration::Effect::EvaluateUntilCondition(payload) => Ok(Self {
                loop_instance_id: payload.loop_instance_id.clone(),
                iteration: payload.iteration,
                parent_frame_id: payload.parent_frame_id.clone(),
                parent_node_id: payload.parent_node_id.clone(),
                loop_id: payload.loop_id.clone(),
            }),
            other => Err(MobError::Internal(format!(
                "expected EvaluateUntilCondition effect, got '{other:?}'"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FlowLoopUntilEvaluationObligation {
    pub loop_instance_id: LoopInstanceId,
    pub iteration: u32,
    pub parent_frame_id: FrameId,
    pub parent_node_id: FlowNodeId,
    pub loop_id: LoopId,
}

pub(crate) fn accept_evaluate_until_condition(
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

pub(crate) fn submit_until_condition_met(
    state: &loop_iteration::State,
    obligation: FlowLoopUntilEvaluationObligation,
) -> Result<LoopIterationTransition, MobError> {
    loop_iteration::transition(
        state,
        loop_iteration::Input::UntilConditionMet(loop_iteration::inputs::UntilConditionMet {
            loop_instance_id: obligation.loop_instance_id.clone(),
            iteration: obligation.iteration,
        }),
        &loop_iteration::EmptyContext,
    )
    .map_err(|error| MobError::Internal(format!("loop_iteration transition refused: {error:?}")))
}

pub(crate) fn submit_until_condition_failed(
    state: &loop_iteration::State,
    obligation: FlowLoopUntilEvaluationObligation,
) -> Result<LoopIterationTransition, MobError> {
    loop_iteration::transition(
        state,
        loop_iteration::Input::UntilConditionFailed(loop_iteration::inputs::UntilConditionFailed {
            loop_instance_id: obligation.loop_instance_id.clone(),
            iteration: obligation.iteration,
        }),
        &loop_iteration::EmptyContext,
    )
    .map_err(|error| MobError::Internal(format!("loop_iteration transition refused: {error:?}")))
}
