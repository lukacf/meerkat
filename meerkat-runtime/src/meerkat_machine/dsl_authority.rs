//! Authority adapter for the MeerkatMachine DSL.
//!
//! Bridges between the runtime's distributed state and the DSL's flat
//! state representation. The DSL validates transition legality; the
//! runtime shell executes the actual mutations.

use super::dsl as mm_dsl;
use crate::runtime_state::RuntimeState;

// ---------------------------------------------------------------------------
// Transition result returned to the runtime shell
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) struct MeerkatTransitionResult {
    pub new_phase: MeerkatPhaseResult,
    pub effects: Vec<mm_dsl::MeerkatMachineEffect>,
}

// ---------------------------------------------------------------------------
// Phase conversion
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MeerkatPhaseResult {
    Initializing,
    Idle,
    Attached,
    Running,
    Retired,
    Stopped,
    Destroyed,
}

impl MeerkatPhaseResult {
    pub fn to_runtime_state(self) -> RuntimeState {
        match self {
            Self::Initializing => RuntimeState::Initializing,
            Self::Idle => RuntimeState::Idle,
            Self::Attached => RuntimeState::Attached,
            Self::Running => RuntimeState::Running,
            Self::Retired => RuntimeState::Retired,
            Self::Stopped => RuntimeState::Stopped,
            Self::Destroyed => RuntimeState::Destroyed,
        }
    }

    pub fn to_dsl_phase(self) -> mm_dsl::MeerkatPhase {
        match self {
            Self::Initializing => mm_dsl::MeerkatPhase::Initializing,
            Self::Idle => mm_dsl::MeerkatPhase::Idle,
            Self::Attached => mm_dsl::MeerkatPhase::Attached,
            Self::Running => mm_dsl::MeerkatPhase::Running,
            Self::Retired => mm_dsl::MeerkatPhase::Retired,
            Self::Stopped => mm_dsl::MeerkatPhase::Stopped,
            Self::Destroyed => mm_dsl::MeerkatPhase::Destroyed,
        }
    }
}

pub(crate) fn dsl_phase_to_result(phase: mm_dsl::MeerkatPhase) -> MeerkatPhaseResult {
    match phase {
        mm_dsl::MeerkatPhase::Initializing => MeerkatPhaseResult::Initializing,
        mm_dsl::MeerkatPhase::Idle => MeerkatPhaseResult::Idle,
        mm_dsl::MeerkatPhase::Attached => MeerkatPhaseResult::Attached,
        mm_dsl::MeerkatPhase::Running => MeerkatPhaseResult::Running,
        mm_dsl::MeerkatPhase::Retired => MeerkatPhaseResult::Retired,
        mm_dsl::MeerkatPhase::Stopped => MeerkatPhaseResult::Stopped,
        mm_dsl::MeerkatPhase::Destroyed => MeerkatPhaseResult::Destroyed,
    }
}

/// Map DSL transition errors into plain strings with context.
pub(crate) fn map_error(err: mm_dsl::MeerkatMachineTransitionError, context: &str) -> String {
    match err {
        mm_dsl::MeerkatMachineTransitionError::NoMatchingTransition { phase, trigger } => {
            format!("DSL authority ({context}): no matching transition from {phase} for {trigger}")
        }
    }
}

pub(crate) fn write_back_phase(dsl_phase: mm_dsl::MeerkatPhase) -> RuntimeState {
    match dsl_phase {
        mm_dsl::MeerkatPhase::Initializing => RuntimeState::Initializing,
        mm_dsl::MeerkatPhase::Idle => RuntimeState::Idle,
        mm_dsl::MeerkatPhase::Attached => RuntimeState::Attached,
        mm_dsl::MeerkatPhase::Running => RuntimeState::Running,
        mm_dsl::MeerkatPhase::Retired => RuntimeState::Retired,
        mm_dsl::MeerkatPhase::Stopped => RuntimeState::Stopped,
        mm_dsl::MeerkatPhase::Destroyed => RuntimeState::Destroyed,
    }
}

pub(crate) fn project_phase(state: RuntimeState) -> mm_dsl::MeerkatPhase {
    match state {
        RuntimeState::Initializing => mm_dsl::MeerkatPhase::Initializing,
        RuntimeState::Idle => mm_dsl::MeerkatPhase::Idle,
        RuntimeState::Attached => mm_dsl::MeerkatPhase::Attached,
        RuntimeState::Running => mm_dsl::MeerkatPhase::Running,
        RuntimeState::Retired => mm_dsl::MeerkatPhase::Retired,
        RuntimeState::Stopped => mm_dsl::MeerkatPhase::Stopped,
        RuntimeState::Destroyed => mm_dsl::MeerkatPhase::Destroyed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_state() -> mm_dsl::MeerkatMachineState {
        mm_dsl::MeerkatMachineState::default()
    }

    #[test]
    fn project_and_write_back_round_trips() {
        for state in [
            RuntimeState::Initializing,
            RuntimeState::Idle,
            RuntimeState::Attached,
            RuntimeState::Running,
            RuntimeState::Retired,
            RuntimeState::Stopped,
            RuntimeState::Destroyed,
        ] {
            let dsl = project_phase(state);
            let back = write_back_phase(dsl);
            assert_eq!(back, state);
        }
    }

    #[test]
    fn map_error_includes_context() {
        let err = mm_dsl::MeerkatMachineTransitionError::NoMatchingTransition {
            phase: "Idle".into(),
            trigger: "Destroy".into(),
        };
        let msg = map_error(err, "test_context");
        assert!(msg.contains("test_context"));
        assert!(msg.contains("Idle"));
    }
}
