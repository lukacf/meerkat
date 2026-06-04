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
    /// Stopped by runtime control; generated lifecycle authority classifies
    /// this as non-terminal while the runtime can still be recovered or
    /// destroyed.
    Stopped,
    /// Destroyed (terminal).
    Destroyed,
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
    use crate::meerkat_machine::{self, dsl};

    #[test]
    fn lifecycle_facts_come_from_machine_authority() {
        let stopped = meerkat_machine::classify_runtime_lifecycle_state(RuntimeState::Stopped)
            .expect("stopped classification");
        assert_eq!(
            stopped.terminality,
            dsl::RuntimeLifecycleTerminality::NonTerminal
        );
        assert_eq!(
            stopped.ingress_admission,
            dsl::RuntimeIngressAdmission::NotReady
        );

        let destroyed = meerkat_machine::classify_runtime_lifecycle_state(RuntimeState::Destroyed)
            .expect("destroyed classification");
        assert_eq!(
            destroyed.terminality,
            dsl::RuntimeLifecycleTerminality::Terminal
        );
        assert_eq!(
            destroyed.ingress_admission,
            dsl::RuntimeIngressAdmission::Destroyed
        );

        let idle = meerkat_machine::classify_runtime_lifecycle_state(RuntimeState::Idle)
            .expect("idle classification");
        assert!(idle.can_accept_input());
        assert!(idle.can_process_queue());
        assert!(idle.can_prepare_run());

        let running = meerkat_machine::classify_runtime_lifecycle_state(RuntimeState::Running)
            .expect("running classification");
        assert!(running.can_accept_input());
        assert!(!running.can_process_queue());
        assert!(!running.can_prepare_run());

        let running_without_binding =
            meerkat_machine::classify_runtime_loop_queue_admission(RuntimeState::Running, false)
                .expect("running without binding queue admission");
        assert!(!running_without_binding.can_process_queue());
        assert_eq!(
            running_without_binding.run_binding,
            dsl::RuntimeLoopRunBinding::Blocked
        );

        let running_with_binding =
            meerkat_machine::classify_runtime_loop_queue_admission(RuntimeState::Running, true)
                .expect("running with binding queue admission");
        assert!(running_with_binding.can_process_queue());
        assert_eq!(
            running_with_binding.run_binding,
            dsl::RuntimeLoopRunBinding::UsePrebound
        );

        let idle_queue =
            meerkat_machine::classify_runtime_loop_queue_admission(RuntimeState::Idle, false)
                .expect("idle queue admission");
        assert!(idle_queue.can_process_queue());
        assert_eq!(
            idle_queue.run_binding,
            dsl::RuntimeLoopRunBinding::AllocateNew
        );

        assert_eq!(
            meerkat_machine::classify_runtime_lifecycle_durable_state(RuntimeState::Attached)
                .expect("attached durability classification"),
            RuntimeState::Idle,
            "generated durability classification owns the process-local Attached recovery projection"
        );
        assert_eq!(
            meerkat_machine::classify_runtime_lifecycle_durable_state(RuntimeState::Running)
                .expect("running durability classification"),
            RuntimeState::Running
        );
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
