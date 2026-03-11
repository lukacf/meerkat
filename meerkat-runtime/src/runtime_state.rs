//! §22 RuntimeState — the runtime's own state machine with validated transitions.
//!
//! 7 states with strict transition rules from the spec.

use serde::{Deserialize, Serialize};

/// The state of a runtime instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RuntimeState {
    /// Initializing (first state after creation).
    Initializing,
    /// Idle — no run in progress, ready to accept input.
    Idle,
    /// A run is in progress.
    Running,
    /// Recovering from a crash or error.
    Recovering,
    /// Retired — no longer accepting new input, draining existing.
    Retired,
    /// Permanently stopped (terminal).
    Stopped,
    /// Destroyed (terminal).
    Destroyed,
}

impl RuntimeState {
    /// Check if this is a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Stopped | Self::Destroyed)
    }

    /// Check if the runtime can accept new input in this state.
    pub fn can_accept_input(&self) -> bool {
        matches!(self, Self::Idle | Self::Running)
    }

    /// Validate a transition from this state to another (§22 table).
    pub fn can_transition_to(&self, next: &RuntimeState) -> bool {
        use RuntimeState::{Destroyed, Idle, Initializing, Recovering, Retired, Running, Stopped};
        matches!(
            (self, next),
            // Initializing → Idle, Stopped, Destroyed
            (Initializing, Idle | Stopped | Destroyed)
            // Idle → Running, Retired, Recovering, Stopped, Destroyed
            | (Idle, Running | Retired | Recovering | Stopped | Destroyed)
            // Running → Idle, Recovering, Stopped, Destroyed
            | (Running, Idle | Recovering | Stopped | Destroyed)
            // Recovering → Idle, Running, Stopped, Destroyed
            | (Recovering, Idle | Running | Stopped | Destroyed)
            // Retired → Stopped, Destroyed
            | (Retired, Stopped | Destroyed)
        )
    }

    /// Attempt to transition, returning an error if invalid.
    pub fn transition(&mut self, next: RuntimeState) -> Result<(), RuntimeStateTransitionError> {
        if self.can_transition_to(&next) {
            *self = next;
            Ok(())
        } else {
            Err(RuntimeStateTransitionError {
                from: *self,
                to: next,
            })
        }
    }
}

impl std::fmt::Display for RuntimeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Initializing => write!(f, "initializing"),
            Self::Idle => write!(f, "idle"),
            Self::Running => write!(f, "running"),
            Self::Recovering => write!(f, "recovering"),
            Self::Retired => write!(f, "retired"),
            Self::Stopped => write!(f, "stopped"),
            Self::Destroyed => write!(f, "destroyed"),
        }
    }
}

/// Error when an invalid runtime state transition is attempted.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Invalid runtime state transition: {from} -> {to}")]
pub struct RuntimeStateTransitionError {
    pub from: RuntimeState,
    pub to: RuntimeState,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn terminal_states() {
        assert!(RuntimeState::Stopped.is_terminal());
        assert!(RuntimeState::Destroyed.is_terminal());

        assert!(!RuntimeState::Initializing.is_terminal());
        assert!(!RuntimeState::Idle.is_terminal());
        assert!(!RuntimeState::Running.is_terminal());
        assert!(!RuntimeState::Recovering.is_terminal());
        assert!(!RuntimeState::Retired.is_terminal());
    }

    #[test]
    fn can_accept_input() {
        assert!(RuntimeState::Idle.can_accept_input());
        assert!(RuntimeState::Running.can_accept_input());

        assert!(!RuntimeState::Initializing.can_accept_input());
        assert!(!RuntimeState::Recovering.can_accept_input());
        assert!(!RuntimeState::Retired.can_accept_input());
        assert!(!RuntimeState::Stopped.can_accept_input());
        assert!(!RuntimeState::Destroyed.can_accept_input());
    }

    // §22 transition table — exhaustive valid transitions
    #[test]
    fn initializing_valid_transitions() {
        let s = RuntimeState::Initializing;
        assert!(s.can_transition_to(&RuntimeState::Idle));
        assert!(s.can_transition_to(&RuntimeState::Stopped));
        assert!(s.can_transition_to(&RuntimeState::Destroyed));

        // Invalid
        assert!(!s.can_transition_to(&RuntimeState::Running));
        assert!(!s.can_transition_to(&RuntimeState::Recovering));
        assert!(!s.can_transition_to(&RuntimeState::Retired));
        assert!(!s.can_transition_to(&RuntimeState::Initializing));
    }

    #[test]
    fn idle_valid_transitions() {
        let s = RuntimeState::Idle;
        assert!(s.can_transition_to(&RuntimeState::Running));
        assert!(s.can_transition_to(&RuntimeState::Retired));
        assert!(s.can_transition_to(&RuntimeState::Recovering));
        assert!(s.can_transition_to(&RuntimeState::Stopped));
        assert!(s.can_transition_to(&RuntimeState::Destroyed));

        // Invalid
        assert!(!s.can_transition_to(&RuntimeState::Initializing));
        assert!(!s.can_transition_to(&RuntimeState::Idle));
    }

    #[test]
    fn running_valid_transitions() {
        let s = RuntimeState::Running;
        assert!(s.can_transition_to(&RuntimeState::Idle));
        assert!(s.can_transition_to(&RuntimeState::Recovering));
        assert!(s.can_transition_to(&RuntimeState::Stopped));
        assert!(s.can_transition_to(&RuntimeState::Destroyed));

        // Invalid
        assert!(!s.can_transition_to(&RuntimeState::Initializing));
        assert!(!s.can_transition_to(&RuntimeState::Running));
        assert!(!s.can_transition_to(&RuntimeState::Retired));
    }

    #[test]
    fn recovering_valid_transitions() {
        let s = RuntimeState::Recovering;
        assert!(s.can_transition_to(&RuntimeState::Idle));
        assert!(s.can_transition_to(&RuntimeState::Running));
        assert!(s.can_transition_to(&RuntimeState::Stopped));
        assert!(s.can_transition_to(&RuntimeState::Destroyed));

        // Invalid
        assert!(!s.can_transition_to(&RuntimeState::Initializing));
        assert!(!s.can_transition_to(&RuntimeState::Recovering));
        assert!(!s.can_transition_to(&RuntimeState::Retired));
    }

    #[test]
    fn retired_valid_transitions() {
        let s = RuntimeState::Retired;
        assert!(s.can_transition_to(&RuntimeState::Stopped));
        assert!(s.can_transition_to(&RuntimeState::Destroyed));

        // Invalid
        assert!(!s.can_transition_to(&RuntimeState::Initializing));
        assert!(!s.can_transition_to(&RuntimeState::Idle));
        assert!(!s.can_transition_to(&RuntimeState::Running));
        assert!(!s.can_transition_to(&RuntimeState::Recovering));
        assert!(!s.can_transition_to(&RuntimeState::Retired));
    }

    #[test]
    fn stopped_is_terminal() {
        let s = RuntimeState::Stopped;
        assert!(!s.can_transition_to(&RuntimeState::Initializing));
        assert!(!s.can_transition_to(&RuntimeState::Idle));
        assert!(!s.can_transition_to(&RuntimeState::Running));
        assert!(!s.can_transition_to(&RuntimeState::Recovering));
        assert!(!s.can_transition_to(&RuntimeState::Retired));
        assert!(!s.can_transition_to(&RuntimeState::Stopped));
        assert!(!s.can_transition_to(&RuntimeState::Destroyed));
    }

    #[test]
    fn destroyed_is_terminal() {
        let s = RuntimeState::Destroyed;
        assert!(!s.can_transition_to(&RuntimeState::Initializing));
        assert!(!s.can_transition_to(&RuntimeState::Idle));
        assert!(!s.can_transition_to(&RuntimeState::Running));
        assert!(!s.can_transition_to(&RuntimeState::Recovering));
        assert!(!s.can_transition_to(&RuntimeState::Retired));
        assert!(!s.can_transition_to(&RuntimeState::Stopped));
        assert!(!s.can_transition_to(&RuntimeState::Destroyed));
    }

    #[test]
    fn transition_success() {
        let mut state = RuntimeState::Initializing;
        assert!(state.transition(RuntimeState::Idle).is_ok());
        assert_eq!(state, RuntimeState::Idle);

        assert!(state.transition(RuntimeState::Running).is_ok());
        assert_eq!(state, RuntimeState::Running);

        assert!(state.transition(RuntimeState::Idle).is_ok());
        assert_eq!(state, RuntimeState::Idle);
    }

    #[test]
    fn transition_failure() {
        let mut state = RuntimeState::Stopped;
        let result = state.transition(RuntimeState::Idle);
        assert!(result.is_err());
        assert_eq!(state, RuntimeState::Stopped); // unchanged
    }

    #[test]
    fn serde_roundtrip_all_states() {
        for state in [
            RuntimeState::Initializing,
            RuntimeState::Idle,
            RuntimeState::Running,
            RuntimeState::Recovering,
            RuntimeState::Retired,
            RuntimeState::Stopped,
            RuntimeState::Destroyed,
        ] {
            let json = serde_json::to_value(state).unwrap();
            let parsed: RuntimeState = serde_json::from_value(json).unwrap();
            assert_eq!(state, parsed);
        }
    }

    #[test]
    fn display() {
        assert_eq!(RuntimeState::Idle.to_string(), "idle");
        assert_eq!(RuntimeState::Running.to_string(), "running");
        assert_eq!(RuntimeState::Destroyed.to_string(), "destroyed");
    }
}
