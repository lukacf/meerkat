//! §22 RuntimeState — the runtime's public state projection.
//!
//! Canonical transition legality lives in `RuntimeControlAuthority`.

use serde::{Deserialize, Serialize};

#[allow(dead_code)]
fn can_transition(from: &RuntimeState, next: &RuntimeState) -> bool {
    use RuntimeState::{
        Attached, Destroyed, Idle, Initializing, Recovering, Retired, Running, Stopped,
    };

    matches!(
        (from, next),
        (Initializing, Idle | Stopped | Destroyed)
            | (
                Idle,
                Attached | Running | Retired | Recovering | Stopped | Destroyed
            )
            | (
                Attached,
                Running | Idle | Retired | Recovering | Stopped | Destroyed
            )
            | (
                Running,
                Idle | Attached | Recovering | Retired | Stopped | Destroyed
            )
            | (Recovering, Idle | Attached | Running | Stopped | Destroyed)
            | (Retired, Running | Stopped | Destroyed)
    )
}

/// The state of a runtime instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RuntimeState {
    /// Initializing (first state after creation).
    Initializing,
    /// Idle — no executor attached, no run in progress, ready to accept input.
    Idle,
    /// Attached — executor attached, runtime loop alive, waiting for input.
    Attached,
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
        matches!(self, Self::Idle | Self::Attached | Self::Running)
    }

    /// Check if the runtime can process queued inputs in this state.
    pub fn can_process_queue(&self) -> bool {
        matches!(self, Self::Idle | Self::Attached | Self::Retired)
    }

    /// Check if the runtime is in the Attached state.
    pub fn is_attached(&self) -> bool {
        matches!(self, Self::Attached)
    }

    /// Check if the runtime is Idle or Attached.
    pub fn is_idle_or_attached(&self) -> bool {
        matches!(self, Self::Idle | Self::Attached)
    }
}

impl std::fmt::Display for RuntimeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Initializing => write!(f, "initializing"),
            Self::Idle => write!(f, "idle"),
            Self::Attached => write!(f, "attached"),
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
        assert!(!RuntimeState::Attached.is_terminal());
        assert!(!RuntimeState::Running.is_terminal());
        assert!(!RuntimeState::Recovering.is_terminal());
        assert!(!RuntimeState::Retired.is_terminal());
    }

    #[test]
    fn input_and_queue_capabilities() {
        assert!(RuntimeState::Idle.can_accept_input());
        assert!(RuntimeState::Attached.can_accept_input());
        assert!(RuntimeState::Running.can_accept_input());
        assert!(!RuntimeState::Retired.can_accept_input());

        assert!(RuntimeState::Idle.can_process_queue());
        assert!(RuntimeState::Attached.can_process_queue());
        assert!(RuntimeState::Retired.can_process_queue());
        assert!(!RuntimeState::Running.can_process_queue());
    }

    #[test]
    fn attachment_predicates() {
        assert!(RuntimeState::Attached.is_attached());
        assert!(RuntimeState::Idle.is_idle_or_attached());
        assert!(RuntimeState::Attached.is_idle_or_attached());
        assert!(!RuntimeState::Running.is_idle_or_attached());
    }

    #[test]
    fn transition_table_matches_spec_examples() {
        assert!(can_transition(
            &RuntimeState::Initializing,
            &RuntimeState::Idle
        ));
        assert!(can_transition(&RuntimeState::Idle, &RuntimeState::Attached));
        assert!(can_transition(
            &RuntimeState::Attached,
            &RuntimeState::Running
        ));
        assert!(can_transition(
            &RuntimeState::Running,
            &RuntimeState::Retired
        ));
        assert!(can_transition(
            &RuntimeState::Retired,
            &RuntimeState::Stopped
        ));

        assert!(!can_transition(&RuntimeState::Stopped, &RuntimeState::Idle));
        assert!(!can_transition(
            &RuntimeState::Destroyed,
            &RuntimeState::Running
        ));
        assert!(!can_transition(&RuntimeState::Retired, &RuntimeState::Idle));
    }

    #[test]
    fn transition_failure_shape_matches_runtime_error() {
        let result = if can_transition(&RuntimeState::Stopped, &RuntimeState::Idle) {
            Ok(())
        } else {
            Err(RuntimeStateTransitionError {
                from: RuntimeState::Stopped,
                to: RuntimeState::Idle,
            })
        };

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RuntimeStateTransitionError {
                from: RuntimeState::Stopped,
                to: RuntimeState::Idle
            }
        ));
    }

    #[test]
    fn serde_roundtrip_all_states() {
        for state in [
            RuntimeState::Initializing,
            RuntimeState::Idle,
            RuntimeState::Attached,
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
        assert_eq!(RuntimeState::Attached.to_string(), "attached");
        assert_eq!(RuntimeState::Running.to_string(), "running");
        assert_eq!(RuntimeState::Destroyed.to_string(), "destroyed");
    }
}
