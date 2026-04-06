//! Canonical authority surface for loop-iteration feedback.
//!
//! The frame runtime realizes `EvaluateUntilCondition` in shell code, but the
//! feedback that closes that handoff must still flow through a typed authority
//! boundary. This module owns that boundary and delegates transition legality to
//! the generated loop-iteration machine kernel.

use std::collections::BTreeMap;

use crate::error::MobError;
use crate::ids::{FlowNodeId, FrameId, LoopId, LoopInstanceId};
use meerkat_machine_kernels::generated::loop_iteration;
use meerkat_machine_kernels::{
    KernelEffect, KernelInput, KernelState, KernelValue, TransitionOutcome,
};

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

pub(crate) type LoopIterationTransition = TransitionOutcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoopUntilEvaluationRequested {
    pub loop_instance_id: LoopInstanceId,
    pub iteration: u32,
    pub parent_frame_id: FrameId,
    pub parent_node_id: FlowNodeId,
    pub loop_id: LoopId,
}

impl LoopUntilEvaluationRequested {
    pub(crate) fn from_effect(effect: &KernelEffect) -> Result<Self, MobError> {
        if effect.variant != "EvaluateUntilCondition" {
            return Err(MobError::Internal(format!(
                "expected EvaluateUntilCondition effect, got '{}'",
                effect.variant
            )));
        }
        Ok(Self {
            loop_instance_id: parse_loop_instance_id(effect, "loop_instance_id")?,
            iteration: parse_u32(effect, "iteration")?,
            parent_frame_id: parse_frame_id(effect, "parent_frame_id")?,
            parent_node_id: parse_flow_node_id(effect, "parent_node_id")?,
            loop_id: parse_loop_id(effect, "loop_id")?,
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
    state: KernelState,
}

impl sealed::Sealed for LoopIterationAuthority {}

impl LoopIterationAuthority {
    pub(crate) fn from_state(state: KernelState) -> Self {
        Self { state }
    }
}

impl LoopIterationMutator for LoopIterationAuthority {
    fn apply(&mut self, input: LoopIterationInput) -> Result<LoopIterationTransition, MobError> {
        let kernel_input = input.into_kernel_input();
        let variant = kernel_input.variant.clone();
        let transition =
            loop_iteration::transition(&self.state, &kernel_input).map_err(|error| {
                MobError::Internal(format!(
                    "loop_iteration {variant} transition refused: {error}"
                ))
            })?;
        self.state = transition.next_state.clone();
        Ok(transition)
    }
}

impl LoopIterationInput {
    fn into_kernel_input(self) -> KernelInput {
        match self {
            Self::UntilConditionMet {
                loop_instance_id,
                iteration,
            } => KernelInput {
                variant: "UntilConditionMet".into(),
                fields: BTreeMap::from([
                    (
                        "loop_instance_id".into(),
                        KernelValue::String(loop_instance_id.to_string()),
                    ),
                    ("iteration".into(), KernelValue::U64(u64::from(iteration))),
                ]),
            },
            Self::UntilConditionFailed {
                loop_instance_id,
                iteration,
            } => KernelInput {
                variant: "UntilConditionFailed".into(),
                fields: BTreeMap::from([
                    (
                        "loop_instance_id".into(),
                        KernelValue::String(loop_instance_id.to_string()),
                    ),
                    ("iteration".into(), KernelValue::U64(u64::from(iteration))),
                ]),
            },
        }
    }
}

fn parse_string(effect: &KernelEffect, field: &str) -> Result<String, MobError> {
    match effect.fields.get(field) {
        Some(KernelValue::String(value)) => Ok(value.clone()),
        other => Err(MobError::Internal(format!(
            "effect '{}' field '{}' expected string, got {other:?}",
            effect.variant, field
        ))),
    }
}

fn parse_u32(effect: &KernelEffect, field: &str) -> Result<u32, MobError> {
    match effect.fields.get(field) {
        Some(KernelValue::U64(value)) => u32::try_from(*value).map_err(|_| {
            MobError::Internal(format!(
                "effect '{}' field '{}' value {} does not fit in u32",
                effect.variant, field, value
            ))
        }),
        other => Err(MobError::Internal(format!(
            "effect '{}' field '{}' expected u64, got {other:?}",
            effect.variant, field
        ))),
    }
}

fn parse_loop_instance_id(effect: &KernelEffect, field: &str) -> Result<LoopInstanceId, MobError> {
    Ok(LoopInstanceId::from(parse_string(effect, field)?))
}

fn parse_frame_id(effect: &KernelEffect, field: &str) -> Result<FrameId, MobError> {
    Ok(FrameId::from(parse_string(effect, field)?))
}

fn parse_flow_node_id(effect: &KernelEffect, field: &str) -> Result<FlowNodeId, MobError> {
    Ok(FlowNodeId::from(parse_string(effect, field)?))
}

fn parse_loop_id(effect: &KernelEffect, field: &str) -> Result<LoopId, MobError> {
    Ok(LoopId::from(parse_string(effect, field)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn until_request_parses_from_kernel_effect() {
        let effect = KernelEffect {
            variant: "EvaluateUntilCondition".into(),
            fields: BTreeMap::from([
                (
                    "loop_instance_id".into(),
                    KernelValue::String("loop-1".into()),
                ),
                ("iteration".into(), KernelValue::U64(2)),
                (
                    "parent_frame_id".into(),
                    KernelValue::String("frame-root".into()),
                ),
                (
                    "parent_node_id".into(),
                    KernelValue::String("loop-node".into()),
                ),
                ("loop_id".into(), KernelValue::String("loop".into())),
            ]),
        };

        let request = LoopUntilEvaluationRequested::from_effect(&effect).unwrap();
        assert_eq!(request.loop_instance_id, LoopInstanceId::from("loop-1"));
        assert_eq!(request.iteration, 2);
        assert_eq!(request.parent_frame_id, FrameId::from("frame-root"));
        assert_eq!(request.parent_node_id, FlowNodeId::from("loop-node"));
        assert_eq!(request.loop_id, LoopId::from("loop"));
    }
}
