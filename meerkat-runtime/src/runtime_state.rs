//! §22 RuntimeState — the runtime's public state projection.
//!
//! Canonical live transition legality lives in the checked-in `MeerkatMachine`
//! plus the runtime driver that realizes its coarse control transitions.

use serde::{Deserialize, Serialize};

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
    /// Retired — no longer accepting new input, draining existing.
    Retired,
    /// Permanently stopped (terminal).
    Stopped,
    /// Destroyed (terminal).
    Destroyed,
}

impl RuntimeState {
    /// Check if this is a terminal state.
    ///
    /// Only `Destroyed` is terminal. `Stopped` allows transitions like
    /// `RegisterSession`, `UnregisterSession`, `PrepareBindings`, and `Destroy`.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Destroyed)
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
        assert!(!RuntimeState::Stopped.is_terminal());
        assert!(RuntimeState::Destroyed.is_terminal());
        assert!(!RuntimeState::Initializing.is_terminal());
        assert!(!RuntimeState::Idle.is_terminal());
        assert!(!RuntimeState::Attached.is_terminal());
        assert!(!RuntimeState::Running.is_terminal());
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
    fn serde_roundtrip_all_states() {
        for state in [
            RuntimeState::Initializing,
            RuntimeState::Idle,
            RuntimeState::Attached,
            RuntimeState::Running,
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
