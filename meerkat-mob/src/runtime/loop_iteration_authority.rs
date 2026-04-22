//! Canonical authority surface for loop-iteration feedback.
//!
//! The frame runtime realizes `EvaluateUntilCondition` in shell code, but the
//! feedback that closes that handoff must still flow through a typed authority
//! boundary. This module owns that boundary and delegates transition legality to
//! the generated loop-iteration machine kernel.

use crate::error::MobError;
use crate::flow_machine_types::{
    local_flow_node_id, local_frame_id, local_loop_id, local_loop_instance_id,
};
use crate::ids::{FlowNodeId, FrameId, LoopId, LoopInstanceId};
use meerkat_machine_kernels::compat_generated::loop_iteration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoopIterationInput {
    UntilConditionMet {
        loop_instance_id: LoopInstanceId,
        iteration: u32,
    },
    UntilConditionFailed {
        loop_instance_id: LoopInstanceId,
        iteration: u32,
    },
}

pub(crate) type LoopIterationTransition = loop_iteration::Outcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoopUntilEvaluationRequested {
    pub loop_instance_id: LoopInstanceId,
    pub iteration: u32,
    pub parent_frame_id: FrameId,
    pub parent_node_id: FlowNodeId,
    pub loop_id: LoopId,
}

#[allow(dead_code)]
impl LoopUntilEvaluationRequested {
    pub(crate) fn from_effect(effect: &loop_iteration::Effect) -> Result<Self, MobError> {
        let loop_iteration::Effect::EvaluateUntilCondition(payload) = effect else {
            return Err(MobError::Internal(format!(
                "expected EvaluateUntilCondition effect, got {:?}",
                effect.kind()
            )));
        };
        Ok(Self {
            loop_instance_id: local_loop_instance_id(&payload.loop_instance_id),
            iteration: payload.iteration,
            parent_frame_id: local_frame_id(&payload.parent_frame_id),
            parent_node_id: local_flow_node_id(&payload.parent_node_id),
            loop_id: local_loop_id(&payload.loop_id),
        })
    }
}

mod sealed {
    pub trait Sealed {}
}

pub(crate) trait LoopIterationMutator: sealed::Sealed {
    fn apply(&mut self, input: LoopIterationInput) -> Result<LoopIterationTransition, MobError>;
}

#[derive(Debug, Clone)]
pub(crate) struct LoopIterationAuthority {
    state: loop_iteration::State,
}

impl sealed::Sealed for LoopIterationAuthority {}

impl LoopIterationAuthority {
    pub(crate) fn from_state(state: loop_iteration::State) -> Self {
        Self { state }
    }
}

impl LoopIterationMutator for LoopIterationAuthority {
    fn apply(&mut self, input: LoopIterationInput) -> Result<LoopIterationTransition, MobError> {
        let machine_input = input.into_machine_input();
        let transition =
            loop_iteration::transition(&self.state, machine_input, &loop_iteration::EmptyContext)
                .map_err(|error| {
                MobError::Internal(format!("loop_iteration transition refused: {error:?}"))
            })?;
        self.state = transition.next_state.clone();
        Ok(transition)
    }
}

impl LoopIterationInput {
    fn into_machine_input(self) -> loop_iteration::Input {
        match self {
            Self::UntilConditionMet {
                loop_instance_id,
                iteration,
            } => loop_iteration::Input::UntilConditionMet(
                loop_iteration::inputs::UntilConditionMet {
                    loop_instance_id: crate::flow_machine_types::loop_instance_id(
                        &loop_instance_id,
                    ),
                    iteration,
                },
            ),
            Self::UntilConditionFailed {
                loop_instance_id,
                iteration,
            } => loop_iteration::Input::UntilConditionFailed(
                loop_iteration::inputs::UntilConditionFailed {
                    loop_instance_id: crate::flow_machine_types::loop_instance_id(
                        &loop_instance_id,
                    ),
                    iteration,
                },
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn until_request_parses_from_kernel_effect() {
        let effect = loop_iteration::Effect::EvaluateUntilCondition(
            loop_iteration::effects::EvaluateUntilCondition {
                loop_instance_id: crate::flow_machine_types::loop_instance_id(
                    &LoopInstanceId::from("loop-1"),
                ),
                iteration: 2,
                parent_frame_id: crate::flow_machine_types::frame_id(&FrameId::from("frame-root")),
                parent_node_id: crate::flow_machine_types::flow_node_id(&FlowNodeId::from(
                    "loop-node",
                )),
                loop_id: crate::flow_machine_types::loop_id(&LoopId::from("loop")),
            },
        );

        let request = LoopUntilEvaluationRequested::from_effect(&effect).unwrap();
        assert_eq!(request.loop_instance_id, LoopInstanceId::from("loop-1"));
        assert_eq!(request.iteration, 2);
        assert_eq!(request.parent_frame_id, FrameId::from("frame-root"));
        assert_eq!(request.parent_node_id, FlowNodeId::from("loop-node"));
        assert_eq!(request.loop_id, LoopId::from("loop"));
    }
}
