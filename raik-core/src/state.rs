//! State machine for the agent loop
//!
//! Defines valid states and transitions for the core loop.

use crate::error::AgentError;
use serde::{Deserialize, Serialize};

/// States of the core agent loop
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopState {
    /// Waiting for LLM response
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

    /// Validate a transition from this state to another
    pub fn can_transition_to(&self, next: &LoopState) -> bool {
        use LoopState::*;

        match (self, next) {
            // From CallingLlm
            (CallingLlm, WaitingForOps) => true,      // ops pending after tool dispatch
            (CallingLlm, DrainingEvents) => true,     // tool_use stop reason
            (CallingLlm, Completed) => true,          // end_turn and no ops
            (CallingLlm, ErrorRecovery) => true,      // LLM error
            (CallingLlm, Cancelling) => true,         // cancel signal

            // From WaitingForOps
            (WaitingForOps, DrainingEvents) => true,  // when ops complete
            (WaitingForOps, Cancelling) => true,      // cancel signal

            // From DrainingEvents
            (DrainingEvents, CallingLlm) => true,     // more work needed
            (DrainingEvents, Completed) => true,      // done
            (DrainingEvents, Cancelling) => true,     // cancel signal

            // From Cancelling
            (Cancelling, Completed) => true,          // after drain

            // From ErrorRecovery
            (ErrorRecovery, CallingLlm) => true,      // after recovery
            (ErrorRecovery, Completed) => true,       // if unrecoverable
            (ErrorRecovery, Cancelling) => true,      // cancel during recovery

            // No other transitions allowed
            _ => false,
        }
    }

    /// Transition to a new state, returning error if invalid
    pub fn transition(&mut self, next: LoopState) -> Result<(), AgentError> {
        if self.can_transition_to(&next) {
            *self = next;
            Ok(())
        } else {
            Err(AgentError::InvalidStateTransition {
                from: format!("{:?}", self),
                to: format!("{:?}", next),
            })
        }
    }

    /// Force a transition (use with caution)
    pub fn force_transition(&mut self, next: LoopState) {
        *self = next;
    }
}

impl Default for LoopState {
    fn default() -> Self {
        Self::CallingLlm
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
mod tests {
    use super::*;

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
        assert!(state.can_transition_to(&LoopState::WaitingForOps));
        assert!(state.can_transition_to(&LoopState::DrainingEvents));
        assert!(state.can_transition_to(&LoopState::Completed));
        assert!(state.can_transition_to(&LoopState::ErrorRecovery));
        assert!(state.can_transition_to(&LoopState::Cancelling));

        // Invalid
        assert!(!state.can_transition_to(&LoopState::CallingLlm));
    }

    #[test]
    fn test_valid_transitions_from_waiting_for_ops() {
        let state = LoopState::WaitingForOps;
        assert!(state.can_transition_to(&LoopState::DrainingEvents));
        assert!(state.can_transition_to(&LoopState::Cancelling));

        // Invalid
        assert!(!state.can_transition_to(&LoopState::CallingLlm));
        assert!(!state.can_transition_to(&LoopState::Completed));
    }

    #[test]
    fn test_valid_transitions_from_draining_events() {
        let state = LoopState::DrainingEvents;
        assert!(state.can_transition_to(&LoopState::CallingLlm));
        assert!(state.can_transition_to(&LoopState::Completed));
        assert!(state.can_transition_to(&LoopState::Cancelling));

        // Invalid
        assert!(!state.can_transition_to(&LoopState::WaitingForOps));
        assert!(!state.can_transition_to(&LoopState::ErrorRecovery));
    }

    #[test]
    fn test_valid_transitions_from_cancelling() {
        let state = LoopState::Cancelling;
        assert!(state.can_transition_to(&LoopState::Completed));

        // Invalid
        assert!(!state.can_transition_to(&LoopState::CallingLlm));
        assert!(!state.can_transition_to(&LoopState::WaitingForOps));
    }

    #[test]
    fn test_valid_transitions_from_error_recovery() {
        let state = LoopState::ErrorRecovery;
        assert!(state.can_transition_to(&LoopState::CallingLlm));
        assert!(state.can_transition_to(&LoopState::Completed));
        assert!(state.can_transition_to(&LoopState::Cancelling));

        // Invalid
        assert!(!state.can_transition_to(&LoopState::WaitingForOps));
        assert!(!state.can_transition_to(&LoopState::DrainingEvents));
    }

    #[test]
    fn test_completed_is_terminal() {
        let state = LoopState::Completed;

        // No transitions from completed state
        assert!(!state.can_transition_to(&LoopState::CallingLlm));
        assert!(!state.can_transition_to(&LoopState::WaitingForOps));
        assert!(!state.can_transition_to(&LoopState::DrainingEvents));
        assert!(!state.can_transition_to(&LoopState::Cancelling));
        assert!(!state.can_transition_to(&LoopState::ErrorRecovery));
        assert!(!state.can_transition_to(&LoopState::Completed));
    }

    #[test]
    fn test_state_transition_success() {
        let mut state = LoopState::CallingLlm;
        assert!(state.transition(LoopState::DrainingEvents).is_ok());
        assert_eq!(state, LoopState::DrainingEvents);

        assert!(state.transition(LoopState::CallingLlm).is_ok());
        assert_eq!(state, LoopState::CallingLlm);
    }

    #[test]
    fn test_state_transition_failure() {
        let mut state = LoopState::Completed;
        let result = state.transition(LoopState::CallingLlm);
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

        // LLM returns tool_use
        assert!(state.transition(LoopState::DrainingEvents).is_ok());

        // Process events, need more work
        assert!(state.transition(LoopState::CallingLlm).is_ok());

        // LLM returns end_turn
        assert!(state.transition(LoopState::Completed).is_ok());

        assert!(state.is_terminal());
    }

    #[test]
    fn test_cancellation_path() {
        let mut state = LoopState::CallingLlm;

        // Cancel signal received
        assert!(state.transition(LoopState::Cancelling).is_ok());

        // Cleanup complete
        assert!(state.transition(LoopState::Completed).is_ok());

        assert!(state.is_terminal());
    }

    #[test]
    fn test_error_recovery_path() {
        let mut state = LoopState::CallingLlm;

        // LLM error
        assert!(state.transition(LoopState::ErrorRecovery).is_ok());

        // Successful retry
        assert!(state.transition(LoopState::CallingLlm).is_ok());

        // Complete normally
        assert!(state.transition(LoopState::Completed).is_ok());
    }
}
