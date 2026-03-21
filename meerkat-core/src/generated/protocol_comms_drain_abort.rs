// @generated — protocol helpers for `comms_drain_abort`
// Producer: CommsDrainLifecycleMachine, Effect: AbortDrainTask
// Closure policy: TerminalClosure
//
// This module enforces the abort handoff protocol. When `StopRequested` emits
// `AbortDrainTask`, the owner must eventually submit `AbortObserved` to close
// the obligation. Under terminal closure policy, the obligation is implicitly
// satisfied when the machine reaches a terminal phase.

use crate::comms_drain_lifecycle_authority::{
    CommsDrainLifecycleAuthority, CommsDrainLifecycleEffect, CommsDrainLifecycleError,
    CommsDrainLifecycleInput, CommsDrainLifecycleMutator, CommsDrainPhase,
};

/// Obligation token for the `comms_drain_abort` protocol.
///
/// Created when `AbortDrainTask` is emitted; consumed when the owner submits
/// `AbortObserved` feedback or the machine reaches terminal phase (Stopped).
#[derive(Debug)]
pub struct CommsDrainAbortObligation {
    _private: (),
}

/// Outcome of executing the stop protocol through the authority.
#[derive(Debug)]
pub struct StopExecutionResult {
    /// All effects emitted by the authority transition.
    pub effects: Vec<CommsDrainLifecycleEffect>,
    /// The abort obligation, if `AbortDrainTask` was emitted. `None` if the
    /// transition did not require aborting a running task (e.g., stopping from
    /// ExitedRespawnable).
    pub abort_obligation: Option<CommsDrainAbortObligation>,
}

/// Execute `StopRequested` through the authority, returning effects and
/// optionally an abort obligation.
pub fn execute_stop_requested(
    authority: &mut CommsDrainLifecycleAuthority,
) -> Result<StopExecutionResult, CommsDrainLifecycleError> {
    let transition = authority.apply(CommsDrainLifecycleInput::StopRequested)?;

    let has_abort = transition
        .effects
        .iter()
        .any(|e| matches!(e, CommsDrainLifecycleEffect::AbortDrainTask));

    Ok(StopExecutionResult {
        effects: transition.effects,
        abort_obligation: if has_abort {
            Some(CommsDrainAbortObligation { _private: () })
        } else {
            None
        },
    })
}

/// Submit `AbortObserved` feedback, consuming the abort obligation token.
///
/// Called when the drain task has acknowledged the abort signal.
pub fn submit_abort_observed(
    authority: &mut CommsDrainLifecycleAuthority,
    _obligation: CommsDrainAbortObligation,
) -> Result<Vec<CommsDrainLifecycleEffect>, CommsDrainLifecycleError> {
    let transition = authority.apply(CommsDrainLifecycleInput::AbortObserved)?;
    Ok(transition.effects)
}

/// Surface result classification for comms drain lifecycle terminal outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommsDrainSurfaceResultClass {
    /// Normal shutdown — the drain task stopped cleanly.
    Success,
}

/// Classify the terminal outcome for `CommsDrainLifecycleMachine`.
///
/// The only terminal phase is `Stopped`, which is always a normal shutdown.
/// Returns `None` when the machine has not yet reached a terminal phase.
pub fn classify_terminal(phase: &CommsDrainPhase) -> Option<CommsDrainSurfaceResultClass> {
    match phase {
        CommsDrainPhase::Stopped => Some(CommsDrainSurfaceResultClass::Success),
        CommsDrainPhase::Inactive
        | CommsDrainPhase::Starting
        | CommsDrainPhase::Running
        | CommsDrainPhase::ExitedRespawnable => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comms_drain_lifecycle_authority::{CommsDrainMode, CommsDrainPhase};
    use crate::generated::protocol_comms_drain_spawn;

    fn authority_at_running() -> CommsDrainLifecycleAuthority {
        let mut auth = CommsDrainLifecycleAuthority::new();
        let result = protocol_comms_drain_spawn::execute_ensure_running(
            &mut auth,
            CommsDrainMode::PersistentHost,
        )
        .expect("ensure running");
        protocol_comms_drain_spawn::submit_task_spawned(&mut auth, result.obligation)
            .expect("task spawned");
        assert_eq!(auth.phase(), CommsDrainPhase::Running);
        auth
    }

    #[test]
    fn stop_from_running_emits_abort_obligation() {
        let mut auth = authority_at_running();

        let result = execute_stop_requested(&mut auth).expect("stop requested");
        assert_eq!(auth.phase(), CommsDrainPhase::Stopped);
        assert!(
            result
                .effects
                .iter()
                .any(|e| matches!(e, CommsDrainLifecycleEffect::AbortDrainTask))
        );
        assert!(result.abort_obligation.is_some());
        assert!(!auth.suppresses_turn_boundary_drain());
    }

    #[test]
    fn stop_from_exited_respawnable_has_no_abort_obligation() {
        let mut auth = CommsDrainLifecycleAuthority::new();

        // Spawn + failure → ExitedRespawnable
        let result = protocol_comms_drain_spawn::execute_ensure_running(
            &mut auth,
            CommsDrainMode::PersistentHost,
        )
        .expect("ensure running");
        protocol_comms_drain_spawn::submit_task_exited(
            &mut auth,
            result.obligation,
            crate::comms_drain_lifecycle_authority::DrainExitReason::Failed,
        )
        .expect("task exited");
        assert_eq!(auth.phase(), CommsDrainPhase::ExitedRespawnable);

        let result = execute_stop_requested(&mut auth).expect("stop requested");
        assert_eq!(auth.phase(), CommsDrainPhase::Stopped);
        // No abort needed — task already exited
        assert!(result.abort_obligation.is_none());
        assert!(
            !result
                .effects
                .iter()
                .any(|e| matches!(e, CommsDrainLifecycleEffect::AbortDrainTask))
        );
    }

    #[test]
    fn stop_from_starting_emits_abort_obligation() {
        let mut auth = CommsDrainLifecycleAuthority::new();
        let result = protocol_comms_drain_spawn::execute_ensure_running(
            &mut auth,
            CommsDrainMode::PersistentHost,
        )
        .expect("ensure running");
        assert_eq!(auth.phase(), CommsDrainPhase::Starting);
        // Drop the spawn obligation — we're testing stop, not spawn feedback
        drop(result.obligation);

        let stop_result = execute_stop_requested(&mut auth).expect("stop requested");
        assert_eq!(auth.phase(), CommsDrainPhase::Stopped);
        assert!(stop_result.abort_obligation.is_some());
        assert!(
            stop_result
                .effects
                .iter()
                .any(|e| matches!(e, CommsDrainLifecycleEffect::AbortDrainTask))
        );
    }

    // -----------------------------------------------------------------------
    // Terminal classification tests
    // -----------------------------------------------------------------------

    #[test]
    fn stopped_classifies_as_success() {
        assert_eq!(
            classify_terminal(&CommsDrainPhase::Stopped),
            Some(CommsDrainSurfaceResultClass::Success)
        );
    }

    #[test]
    fn non_terminal_phases_classify_as_none() {
        for phase in &[
            CommsDrainPhase::Inactive,
            CommsDrainPhase::Starting,
            CommsDrainPhase::Running,
            CommsDrainPhase::ExitedRespawnable,
        ] {
            assert_eq!(
                classify_terminal(phase),
                None,
                "classify_terminal returned Some for non-terminal phase {phase:?}"
            );
        }
    }

    /// Witness: drive authority to Stopped via stop and verify classify_terminal
    /// agrees with the authority phase.
    #[test]
    fn authority_stopped_classifies_success() {
        let mut auth = authority_at_running();
        let result = execute_stop_requested(&mut auth).expect("stop requested");
        assert_eq!(auth.phase(), CommsDrainPhase::Stopped);
        drop(result);
        assert_eq!(
            classify_terminal(&auth.phase()),
            Some(CommsDrainSurfaceResultClass::Success)
        );
    }
}
