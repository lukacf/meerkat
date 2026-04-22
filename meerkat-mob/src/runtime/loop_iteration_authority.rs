//! Thin authority wrapper around the generated `loop_iteration` kernel.
//!
//! The compat `LoopIterationMachine` is emitted as a pure-function
//! kernel (`loop_iteration::transition(&State, Input, &Ctx) -> Outcome`).
//! The codegen-emitted protocol helpers for the `flow_loop_until_evaluation`
//! handoff, however, expect a conventional authority type with a
//! mutable `apply(input)` method so the generator can stay uniform
//! across all handoff protocols in the workspace.
//!
//! This wrapper owns a `State` and delegates each transition to the
//! kernel, mapping the kernel's typed `TransitionError` into a
//! `MobError` (which is the canonical error surface every mob-crate
//! protocol helper already speaks).
//!
//! The authority also carries the `LoopUntilEvaluationRequested`
//! bridge-source struct that the protocol's `accept_*` helper converts
//! into an obligation token. Both types are re-exported so the generated
//! protocol file can `use` them without reaching into the kernel module.

use crate::error::MobError;
use crate::generated::loop_iteration;
use crate::ids::{FlowNodeId, FrameId, LoopId, LoopInstanceId};

pub use loop_iteration::Effect as LoopIterationEffect;
pub use loop_iteration::Input as LoopIterationInput;
pub use loop_iteration::Outcome as LoopIterationTransition;
pub use loop_iteration::State as LoopIterationState;
pub use loop_iteration::inputs;

/// Sealed mutator trait — `authority.apply(input)` is the only legal
/// path that mutates `LoopIterationAuthority` state.
pub trait LoopIterationMutator: sealed::Sealed {
    fn apply(&mut self, input: LoopIterationInput) -> Result<LoopIterationTransition, MobError>;
}

mod sealed {
    pub trait Sealed {}
}

/// Mutable authority wrapper over `loop_iteration::State`.
#[derive(Debug, Clone)]
pub struct LoopIterationAuthority {
    state: LoopIterationState,
}

impl LoopIterationAuthority {
    pub fn new() -> Self {
        Self {
            state: loop_iteration::initial_state(),
        }
    }

    /// Restore an authority from a persisted kernel state snapshot.
    /// Flow-frame engine callers hold `loop_iteration::State` in
    /// `LoopSnapshot`; this is the canonical hydration path.
    pub fn from_state(state: LoopIterationState) -> Self {
        Self { state }
    }

    pub fn state(&self) -> &LoopIterationState {
        &self.state
    }

    /// Consume the authority and return the owned state.
    pub fn into_state(self) -> LoopIterationState {
        self.state
    }
}

impl Default for LoopIterationAuthority {
    fn default() -> Self {
        Self::new()
    }
}

impl sealed::Sealed for LoopIterationAuthority {}

impl LoopIterationMutator for LoopIterationAuthority {
    fn apply(&mut self, input: LoopIterationInput) -> Result<LoopIterationTransition, MobError> {
        let outcome = loop_iteration::transition(&self.state, input, &loop_iteration::EmptyContext)
            .map_err(|error| {
                MobError::Internal(format!("loop_iteration transition refused: {error:?}"))
            })?;
        self.state = outcome.next_state.clone();
        Ok(outcome)
    }
}

/// Bridge-source struct converted into the protocol obligation by the
/// generated `accept_evaluate_until_condition` helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopUntilEvaluationRequested {
    pub loop_instance_id: LoopInstanceId,
    pub iteration: u32,
    pub parent_frame_id: FrameId,
    pub parent_node_id: FlowNodeId,
    pub loop_id: LoopId,
}

impl LoopUntilEvaluationRequested {
    /// Extract the request from a kernel effect, matching the
    /// `EvaluateUntilCondition` variant exactly. Returns
    /// `MobError::Internal` on any other variant.
    pub fn from_effect(effect: &LoopIterationEffect) -> Result<Self, MobError> {
        match effect {
            LoopIterationEffect::EvaluateUntilCondition(payload) => Ok(Self {
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
