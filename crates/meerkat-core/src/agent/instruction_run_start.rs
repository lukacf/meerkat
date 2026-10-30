//! The operator-instruction run-start seam of the agent.
//!
//! The session task calls [`Agent::apply_run_start_operator_instructions`]
//! once per turn, after it resolves the turn's start disposition and before
//! it appends any input of the run. The session-document authority decides
//! whether the session's pending operator instructions apply now; when they
//! do, the session appends the one entry that carries all of them, naming the
//! run the agent then runs. When the turn ends, the session task reports
//! whether that run started ([`Agent::conclude_run_start_operator_instructions`]):
//! the entry of a run that never started is taken back, so its changes stay
//! pending for the next fresh run start.

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
    ///
    /// An entry names the run this agent runs, as a compaction fold at the
    /// run's first request does: the run its turn authority already started
    /// (the run a runtime dispatched), else a run id reserved here, the
    /// dispatched one or a new one, which its run loop starts the run under.
    pub fn apply_run_start_operator_instructions(
        &mut self,
        run_start: InstructionRunStart,
        run_id: Option<RunId>,
    ) -> Result<RunStartInstructionOutcome, AgentError> {
        // A run start replaces whatever an earlier one left unconcluded.
        self.unconcluded_run_start_instructions = None;
        self.run_start_reserved_run_id = None;
        let lowering = self.operator_instruction_lowering();
        // Only a session that records the instruction state can have
        // anything to apply; any other run start reads no turn state here.
        let names_a_run = run_start == InstructionRunStart::FreshRun
            && self.session.carries_operator_instruction_facts();
        let (run_id, reserve) = if names_a_run {
            match self
                .turn_state_handle
                .as_deref()
                .and_then(|handle| handle.snapshot().active_run_id)
            {
                Some(started) => (Some(started), false),
                None => (Some(run_id.unwrap_or_else(RunId::new)), true),
            }
        } else {
            (run_id, false)
        };
        let (outcome, application) = self
            .session
            .apply_run_start_instructions_reversibly(run_start, lowering, run_id.clone())
            .map_err(|error| {
                AgentError::InternalError(format!(
                    "operator instructions could not be resolved at the run start: {error}"
                ))
            })?;
        if matches!(outcome, RunStartInstructionOutcome::Applied { .. }) {
            self.tool_application_observations.invalidate();
            self.unconcluded_run_start_instructions = application.map(Box::new);
            if reserve {
                self.run_start_reserved_run_id = run_id;
            }
        }
        Ok(outcome)
    }

    /// Conclude the run start that applied operator instructions, once its
    /// turn ended: `run_started` is whether the run started. A run that
    /// never started (a refusal or a veto before it, an interrupt before it
    /// began) takes its entry back and restores the recorded target, so no
    /// entry names a run that never ran and the changes stay pending for the
    /// next fresh run start. Returns whether an entry was taken back.
    pub fn conclude_run_start_operator_instructions(
        &mut self,
        run_started: bool,
    ) -> Result<bool, AgentError> {
        let application = self.unconcluded_run_start_instructions.take();
        if run_started {
            return Ok(false);
        }
        self.run_start_reserved_run_id = None;
        let Some(application) = application else {
            return Ok(false);
        };
        self.session
            .revert_run_start_instructions(*application)
            .map_err(|error| {
                AgentError::InternalError(format!(
                    "the operator-instruction entry of a run that never started could not be \
                     taken back: {error}"
                ))
            })?;
        self.tool_application_observations.invalidate();
        Ok(true)
    }
}
