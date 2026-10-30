//! The operator-instruction run-start seam of the agent.
//!
//! The session task calls [`Agent::apply_run_start_operator_instructions`]
//! once per turn, after it resolves the turn's start disposition and before
//! it appends any input of the run. The session-document authority decides
//! whether the session's pending operator instructions apply now; when they
//! do, the session appends the one entry that carries all of them.

use super::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};
use crate::error::AgentError;
use crate::lifecycle::RunId;
use crate::{
    InstructionLowering, InstructionRunStart, ModelProfileWitness, RunStartInstructionOutcome,
};

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized,
    T: AgentToolDispatcher + ?Sized,
    S: AgentSessionStore + ?Sized,
{
    /// How this agent's model carries an operator-instruction entry: the
    /// lowering its build resolved from the model as its backend serves it,
    /// else the one its active model profile states, as the binding that
    /// profile was judged through serves it (a model fallback activates a
    /// witness judged through the target's binding).
    #[must_use]
    pub fn operator_instruction_lowering(&self) -> InstructionLowering {
        self.operator_instruction_lowering.unwrap_or_else(|| {
            self.active_model_profile.as_ref().map_or_else(
                || InstructionLowering::for_resolved_profile(None),
                ModelProfileWitness::instruction_lowering,
            )
        })
    }

    /// Apply the session's pending operator instructions at a run start:
    /// `run_start` is how the turn's start disposition starts the run, and
    /// `run_id` the run a runtime dispatched, when there is one. Nothing
    /// applies on a resumed run or a turn without a pending boundary.
    pub fn apply_run_start_operator_instructions(
        &mut self,
        run_start: InstructionRunStart,
        run_id: Option<RunId>,
    ) -> Result<RunStartInstructionOutcome, AgentError> {
        let lowering = self.operator_instruction_lowering();
        let outcome = self
            .session
            .apply_run_start_instructions(run_start, lowering, run_id)
            .map_err(|error| {
                AgentError::InternalError(format!(
                    "operator instructions could not be resolved at the run start: {error}"
                ))
            })?;
        if matches!(outcome, RunStartInstructionOutcome::Applied { .. }) {
            self.tool_application_observations.invalidate();
        }
        Ok(outcome)
    }
}
