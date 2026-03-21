// @generated — protocol helpers for `comms_drain_abort`
// Composition: comms_drain_lifecycle, Producer: comms_drain, Effect: AbortDrainTask
// Closure policy: TerminalClosure

use crate::comms_drain_lifecycle_authority::{
    CommsDrainLifecycleAuthority, CommsDrainLifecycleEffect, CommsDrainLifecycleError,
    CommsDrainLifecycleInput, CommsDrainLifecycleMutator, CommsDrainPhase,
};

#[derive(Debug, Clone)]
pub struct CommsDrainAbortObligation {
    _private: (),
}

#[derive(Debug)]
pub struct CommsDrainAbortExecutionResult {
    pub effects: Vec<CommsDrainLifecycleEffect>,
    pub obligation: Option<CommsDrainAbortObligation>,
}

pub fn execute_stop_requested(
    authority: &mut CommsDrainLifecycleAuthority,
) -> Result<CommsDrainAbortExecutionResult, CommsDrainLifecycleError> {
    let transition = authority.apply(CommsDrainLifecycleInput::StopRequested)?;
    let obligation = transition.effects.iter().find_map(|effect| match effect {
        CommsDrainLifecycleEffect::AbortDrainTask => {
            Some(CommsDrainAbortObligation { _private: () })
        }
        _ => None,
    });
    Ok(CommsDrainAbortExecutionResult {
        effects: transition.effects,
        obligation,
    })
}

pub fn submit_abort_observed(
    authority: &mut CommsDrainLifecycleAuthority,
    obligation: CommsDrainAbortObligation,
) -> Result<Vec<CommsDrainLifecycleEffect>, CommsDrainLifecycleError> {
    let transition = authority.apply(CommsDrainLifecycleInput::AbortObserved)?;
    Ok(transition.effects)
}

pub fn notify_abort_observed(
    authority: &mut CommsDrainLifecycleAuthority,
) -> Result<Vec<CommsDrainLifecycleEffect>, CommsDrainLifecycleError> {
    let transition = authority.apply(CommsDrainLifecycleInput::AbortObserved)?;
    Ok(transition.effects)
}
