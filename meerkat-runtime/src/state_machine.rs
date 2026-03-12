//! Concrete RuntimeStateMachine — wraps RuntimeState with RunId tracking.
//!
//! Enforces §22 transitions and tracks which run is active.

use meerkat_core::lifecycle::RunId;

use crate::runtime_state::{RuntimeState, RuntimeStateTransitionError};

/// Concrete runtime state machine with run tracking.
#[derive(Debug, Clone)]
pub struct RuntimeStateMachine {
    state: RuntimeState,
    current_run_id: Option<RunId>,
}

impl RuntimeStateMachine {
    /// Create a new state machine in the Initializing state.
    pub fn new() -> Self {
        Self {
            state: RuntimeState::Initializing,
            current_run_id: None,
        }
    }

    /// Create from an existing state (for recovery).
    pub fn from_state(state: RuntimeState) -> Self {
        Self {
            state,
            current_run_id: None,
        }
    }

    /// Get the current state.
    pub fn state(&self) -> RuntimeState {
        self.state
    }

    /// Get the current run ID (if running).
    pub fn current_run_id(&self) -> Option<&RunId> {
        self.current_run_id.as_ref()
    }

    /// Check if the runtime is idle.
    pub fn is_idle(&self) -> bool {
        self.state == RuntimeState::Idle
    }

    /// Check if the runtime is running.
    pub fn is_running(&self) -> bool {
        self.state == RuntimeState::Running
    }

    /// Transition to a new state.
    pub fn transition(
        &mut self,
        next: RuntimeState,
    ) -> Result<RuntimeState, RuntimeStateTransitionError> {
        let from = self.state;
        self.state.transition(next)?;

        // Clear run ID when leaving Running
        if from == RuntimeState::Running && next != RuntimeState::Running {
            self.current_run_id = None;
        }

        Ok(from)
    }

    /// Transition to Running with a specific run ID.
    pub fn start_run(&mut self, run_id: RunId) -> Result<(), RuntimeStateTransitionError> {
        self.state.transition(RuntimeState::Running)?;
        self.current_run_id = Some(run_id);
        Ok(())
    }

    /// Transition from Running to Idle (run completed).
    pub fn complete_run(&mut self) -> Result<RunId, RuntimeStateTransitionError> {
        self.state.transition(RuntimeState::Idle)?;
        self.current_run_id
            .take()
            .ok_or(RuntimeStateTransitionError {
                from: RuntimeState::Running,
                to: RuntimeState::Idle,
            })
    }

    /// Mark as initialized (Initializing → Idle).
    pub fn initialize(&mut self) -> Result<(), RuntimeStateTransitionError> {
        self.state.transition(RuntimeState::Idle)
    }

    /// Reset the runtime back to Idle after lifecycle cleanup.
    ///
    /// `reset` is allowed to revive a retired runtime so callers can keep
    /// using the same logical runtime instance after abandoning queued work.
    pub fn reset_to_idle(&mut self) -> Result<Option<RuntimeState>, RuntimeStateTransitionError> {
        let from = self.state;
        match from {
            RuntimeState::Idle => Ok(None),
            RuntimeState::Retired => {
                self.state = RuntimeState::Idle;
                self.current_run_id = None;
                Ok(Some(from))
            }
            _ => {
                self.state.transition(RuntimeState::Idle)?;
                if from == RuntimeState::Running {
                    self.current_run_id = None;
                }
                Ok(Some(from))
            }
        }
    }
}

impl Default for RuntimeStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn new_starts_initializing() {
        let sm = RuntimeStateMachine::new();
        assert_eq!(sm.state(), RuntimeState::Initializing);
        assert!(sm.current_run_id().is_none());
    }

    #[test]
    fn initialize_transitions_to_idle() {
        let mut sm = RuntimeStateMachine::new();
        sm.initialize().unwrap();
        assert!(sm.is_idle());
    }

    #[test]
    fn start_run_transitions_to_running() {
        let mut sm = RuntimeStateMachine::new();
        sm.initialize().unwrap();
        let run_id = RunId::new();
        sm.start_run(run_id.clone()).unwrap();
        assert!(sm.is_running());
        assert_eq!(sm.current_run_id(), Some(&run_id));
    }

    #[test]
    fn complete_run_returns_to_idle() {
        let mut sm = RuntimeStateMachine::new();
        sm.initialize().unwrap();
        let run_id = RunId::new();
        sm.start_run(run_id.clone()).unwrap();
        let completed_id = sm.complete_run().unwrap();
        assert_eq!(completed_id, run_id);
        assert!(sm.is_idle());
        assert!(sm.current_run_id().is_none());
    }

    #[test]
    fn transition_clears_run_id() {
        let mut sm = RuntimeStateMachine::new();
        sm.initialize().unwrap();
        sm.start_run(RunId::new()).unwrap();
        sm.transition(RuntimeState::Recovering).unwrap();
        assert!(sm.current_run_id().is_none());
    }

    #[test]
    fn from_state_recovery() {
        let sm = RuntimeStateMachine::from_state(RuntimeState::Recovering);
        assert_eq!(sm.state(), RuntimeState::Recovering);
    }

    #[test]
    fn idle_running_idle_cycle() {
        let mut sm = RuntimeStateMachine::new();
        sm.initialize().unwrap();

        for _ in 0..3 {
            sm.start_run(RunId::new()).unwrap();
            assert!(sm.is_running());
            sm.complete_run().unwrap();
            assert!(sm.is_idle());
        }
    }

    #[test]
    fn invalid_transition_rejected() {
        let mut sm = RuntimeStateMachine::new();
        // Can't go straight to Running from Initializing
        assert!(sm.transition(RuntimeState::Running).is_err());
    }

    #[test]
    fn retire_from_idle() {
        let mut sm = RuntimeStateMachine::new();
        sm.initialize().unwrap();
        sm.transition(RuntimeState::Retired).unwrap();
        assert_eq!(sm.state(), RuntimeState::Retired);
    }

    #[test]
    fn stop_from_retired() {
        let mut sm = RuntimeStateMachine::new();
        sm.initialize().unwrap();
        sm.transition(RuntimeState::Retired).unwrap();
        sm.transition(RuntimeState::Stopped).unwrap();
        assert!(sm.state().is_terminal());
    }

    #[test]
    fn reset_from_retired_returns_to_idle() {
        let mut sm = RuntimeStateMachine::new();
        sm.initialize().unwrap();
        sm.transition(RuntimeState::Retired).unwrap();
        let from = sm.reset_to_idle().unwrap();
        assert_eq!(from, Some(RuntimeState::Retired));
        assert_eq!(sm.state(), RuntimeState::Idle);
        assert!(sm.current_run_id().is_none());
    }

    #[test]
    fn destroy_from_idle() {
        let mut sm = RuntimeStateMachine::new();
        sm.initialize().unwrap();
        sm.transition(RuntimeState::Destroyed).unwrap();
        assert!(sm.state().is_terminal());
    }

    #[test]
    fn recovering_to_running() {
        let mut sm = RuntimeStateMachine::from_state(RuntimeState::Recovering);
        sm.start_run(RunId::new()).unwrap();
        assert!(sm.is_running());
    }
}
