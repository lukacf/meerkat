//! State projection for the agent loop.
//!
//! Canonical transition legality lives in `TurnExecutionAuthority`; this enum
//! is the persisted/user-facing loop shape.

use serde::{Deserialize, Serialize};

/// States of the core agent loop
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopState {
    /// Waiting for LLM response
    #[default]
    CallingLlm,
    /// No LLM work, waiting for operation completions
    WaitingForOps,
    /// Processing buffered operation events
    DrainingEvents,
    /// Cleanup on interrupt or budget exhaustion
    Cancelling,
    /// Retry logic for transient LLM failures
    ErrorRecovery,
    /// Terminal state
    Completed,
}

impl LoopState {
    /// Check if this is a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed)
    }

    /// Check if we're actively waiting for external input
    pub fn is_waiting(&self) -> bool {
        matches!(self, Self::CallingLlm | Self::WaitingForOps)
    }
}

impl std::fmt::Display for LoopState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CallingLlm => write!(f, "calling_llm"),
            Self::WaitingForOps => write!(f, "waiting_for_ops"),
            Self::DrainingEvents => write!(f, "draining_events"),
            Self::Cancelling => write!(f, "cancelling"),
            Self::ErrorRecovery => write!(f, "error_recovery"),
            Self::Completed => write!(f, "completed"),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::error::AgentError;

    fn can_transition(from: &LoopState, next: &LoopState) -> bool {
        use LoopState::{
            CallingLlm, Cancelling, Completed, DrainingEvents, ErrorRecovery, WaitingForOps,
        };

        matches!(
            (from, next),
            (
                CallingLlm,
                WaitingForOps | DrainingEvents | Completed | ErrorRecovery | Cancelling
            ) | (WaitingForOps, DrainingEvents | Cancelling)
                | (
                    DrainingEvents | ErrorRecovery,
                    CallingLlm | Completed | Cancelling
                )
                | (Cancelling, Completed)
        )
    }

    fn transition(state: &mut LoopState, next: LoopState) -> Result<(), AgentError> {
        if can_transition(state, &next) {
            *state = next;
            Ok(())
        } else {
            Err(AgentError::InvalidStateTransition {
                from: format!("{state:?}"),
                to: format!("{next:?}"),
            })
        }
    }

    #[test]
    fn test_state_is_terminal() {
        assert!(LoopState::Completed.is_terminal());
        assert!(!LoopState::CallingLlm.is_terminal());
        assert!(!LoopState::WaitingForOps.is_terminal());
        assert!(!LoopState::DrainingEvents.is_terminal());
        assert!(!LoopState::Cancelling.is_terminal());
        assert!(!LoopState::ErrorRecovery.is_terminal());
    }

    #[test]
    fn test_state_is_waiting() {
        assert!(LoopState::CallingLlm.is_waiting());
        assert!(LoopState::WaitingForOps.is_waiting());
        assert!(!LoopState::DrainingEvents.is_waiting());
        assert!(!LoopState::Completed.is_waiting());
    }

    #[test]
    fn test_valid_transitions_from_calling_llm() {
        let state = LoopState::CallingLlm;
        assert!(can_transition(&state, &LoopState::WaitingForOps));
        assert!(can_transition(&state, &LoopState::DrainingEvents));
        assert!(can_transition(&state, &LoopState::Completed));
        assert!(can_transition(&state, &LoopState::ErrorRecovery));
        assert!(can_transition(&state, &LoopState::Cancelling));

        assert!(!can_transition(&state, &LoopState::CallingLlm));
    }

    #[test]
    fn test_valid_transitions_from_waiting_for_ops() {
        let state = LoopState::WaitingForOps;
        assert!(can_transition(&state, &LoopState::DrainingEvents));
        assert!(can_transition(&state, &LoopState::Cancelling));

        assert!(!can_transition(&state, &LoopState::CallingLlm));
        assert!(!can_transition(&state, &LoopState::Completed));
    }

    #[test]
    fn test_valid_transitions_from_draining_events() {
        let state = LoopState::DrainingEvents;
        assert!(can_transition(&state, &LoopState::CallingLlm));
        assert!(can_transition(&state, &LoopState::Completed));
        assert!(can_transition(&state, &LoopState::Cancelling));

        assert!(!can_transition(&state, &LoopState::WaitingForOps));
        assert!(!can_transition(&state, &LoopState::ErrorRecovery));
    }

    #[test]
    fn test_valid_transitions_from_cancelling() {
        let state = LoopState::Cancelling;
        assert!(can_transition(&state, &LoopState::Completed));

        assert!(!can_transition(&state, &LoopState::CallingLlm));
        assert!(!can_transition(&state, &LoopState::WaitingForOps));
    }

    #[test]
    fn test_valid_transitions_from_error_recovery() {
        let state = LoopState::ErrorRecovery;
        assert!(can_transition(&state, &LoopState::CallingLlm));
        assert!(can_transition(&state, &LoopState::Completed));
        assert!(can_transition(&state, &LoopState::Cancelling));

        assert!(!can_transition(&state, &LoopState::WaitingForOps));
        assert!(!can_transition(&state, &LoopState::DrainingEvents));
    }

    #[test]
    fn test_completed_is_terminal() {
        let state = LoopState::Completed;

        assert!(!can_transition(&state, &LoopState::CallingLlm));
        assert!(!can_transition(&state, &LoopState::WaitingForOps));
        assert!(!can_transition(&state, &LoopState::DrainingEvents));
        assert!(!can_transition(&state, &LoopState::Cancelling));
        assert!(!can_transition(&state, &LoopState::ErrorRecovery));
        assert!(!can_transition(&state, &LoopState::Completed));
    }

    #[test]
    fn test_state_transition_success() {
        let mut state = LoopState::CallingLlm;
        assert!(transition(&mut state, LoopState::DrainingEvents).is_ok());
        assert_eq!(state, LoopState::DrainingEvents);

        assert!(transition(&mut state, LoopState::CallingLlm).is_ok());
        assert_eq!(state, LoopState::CallingLlm);
    }

    #[test]
    fn test_state_transition_failure() {
        let mut state = LoopState::Completed;
        let result = transition(&mut state, LoopState::CallingLlm);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AgentError::InvalidStateTransition { .. }
        ));
    }

    #[test]
    fn test_state_serialization() {
        let states = vec![
            LoopState::CallingLlm,
            LoopState::WaitingForOps,
            LoopState::DrainingEvents,
            LoopState::Cancelling,
            LoopState::ErrorRecovery,
            LoopState::Completed,
        ];

        for state in states {
            let json = serde_json::to_value(&state).unwrap();
            let parsed: LoopState = serde_json::from_value(json).unwrap();
            assert_eq!(state, parsed);
        }
    }

    #[test]
    fn test_full_happy_path() {
        let mut state = LoopState::CallingLlm;
        assert!(transition(&mut state, LoopState::DrainingEvents).is_ok());
        assert!(transition(&mut state, LoopState::CallingLlm).is_ok());
        assert!(transition(&mut state, LoopState::Completed).is_ok());
        assert!(state.is_terminal());
    }

    #[test]
    fn test_cancellation_path() {
        let mut state = LoopState::CallingLlm;
        assert!(transition(&mut state, LoopState::Cancelling).is_ok());
        assert!(transition(&mut state, LoopState::Completed).is_ok());
        assert!(state.is_terminal());
    }

    #[test]
    fn test_error_recovery_path() {
        let mut state = LoopState::CallingLlm;
        assert!(transition(&mut state, LoopState::ErrorRecovery).is_ok());
        assert!(transition(&mut state, LoopState::CallingLlm).is_ok());
        assert!(transition(&mut state, LoopState::Completed).is_ok());
    }
}
