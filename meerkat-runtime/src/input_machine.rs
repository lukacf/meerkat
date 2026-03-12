//! §15 InputStateMachine — enforces all mandatory lifecycle transitions.
//!
//! Hard rules:
//! - AppliedPendingConsumption → Queued is REJECTED
//! - Terminal states reject ALL transitions
//! - Every transition is recorded in history

use chrono::Utc;

use crate::input_state::{
    InputAbandonReason, InputLifecycleState, InputState, InputStateHistoryEntry,
    InputTerminalOutcome,
};
use meerkat_core::lifecycle::InputId;

/// Errors from the input state machine.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum InputStateMachineError {
    /// The transition is not valid from the current state.
    #[error("Invalid transition: {from:?} -> {to:?}")]
    InvalidTransition {
        from: InputLifecycleState,
        to: InputLifecycleState,
    },
    /// The input is already in a terminal state.
    #[error("Input {input_id} is in terminal state {state:?}")]
    TerminalState {
        input_id: InputId,
        state: InputLifecycleState,
    },
}

/// Validates and applies lifecycle state transitions on InputState.
pub struct InputStateMachine;

impl InputStateMachine {
    /// Check if a transition from `from` to `to` is valid per §15.
    pub fn is_valid_transition(from: InputLifecycleState, to: InputLifecycleState) -> bool {
        use InputLifecycleState::{
            Abandoned, Accepted, Applied, AppliedPendingConsumption, Coalesced, Consumed, Queued,
            Staged, Superseded,
        };

        // Terminal states reject all transitions
        if from.is_terminal() {
            return false;
        }

        matches!(
            (from, to),
            // Accepted → Queued, Consumed (Ignore+OnAccept), Superseded, Coalesced, Abandoned
            (Accepted, Queued | Consumed | Superseded | Coalesced | Abandoned)
            // Queued → Staged, Superseded, Coalesced, Abandoned
            | (Queued, Staged | Superseded | Coalesced | Abandoned)
            // Staged → Queued (rollback on run failure), Applied, Superseded, Abandoned
            | (Staged, Queued | Applied | Superseded | Abandoned)
            // Applied → AppliedPendingConsumption, Abandoned
            | (Applied, AppliedPendingConsumption | Abandoned)
            // AppliedPendingConsumption → Consumed, Abandoned
            // NOTE: AppliedPendingConsumption → Queued is REJECTED (§15 hard rule)
            | (AppliedPendingConsumption, Consumed | Abandoned)
        )
    }

    /// Transition the input state, recording history.
    ///
    /// Returns an error if the transition is invalid or the state is terminal.
    pub fn transition(
        state: &mut InputState,
        to: InputLifecycleState,
        reason: Option<String>,
    ) -> Result<(), InputStateMachineError> {
        let from = state.current_state;

        // Check terminal first for better error message
        if from.is_terminal() {
            return Err(InputStateMachineError::TerminalState {
                input_id: state.input_id.clone(),
                state: from,
            });
        }

        if !Self::is_valid_transition(from, to) {
            return Err(InputStateMachineError::InvalidTransition { from, to });
        }

        let now = Utc::now();

        // Record history
        state.history.push(InputStateHistoryEntry {
            timestamp: now,
            from,
            to,
            reason,
        });

        // Update state
        state.current_state = to;
        state.updated_at = now;

        // Set terminal outcome if transitioning to terminal
        if to.is_terminal() && state.terminal_outcome.is_none() {
            state.terminal_outcome = Some(match to {
                InputLifecycleState::Consumed => InputTerminalOutcome::Consumed,
                InputLifecycleState::Abandoned => InputTerminalOutcome::Abandoned {
                    reason: InputAbandonReason::Cancelled,
                },
                // Superseded and Coalesced terminal outcomes are set by the caller
                // via set_terminal_outcome() since they need additional data
                _ => return Ok(()),
            });
        }

        Ok(())
    }

    /// Set the terminal outcome explicitly (for Superseded/Coalesced which need extra data).
    pub fn set_terminal_outcome(state: &mut InputState, outcome: InputTerminalOutcome) {
        state.terminal_outcome = Some(outcome);
    }

    /// Transition to Abandoned with a specific reason.
    pub fn abandon(
        state: &mut InputState,
        reason: InputAbandonReason,
    ) -> Result<(), InputStateMachineError> {
        Self::transition(
            state,
            InputLifecycleState::Abandoned,
            Some(format!("abandoned: {reason:?}")),
        )?;
        state.terminal_outcome = Some(InputTerminalOutcome::Abandoned { reason });
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn new_state() -> InputState {
        InputState::new_accepted(InputId::new())
    }

    // ---- Happy path transitions ----

    #[test]
    fn accepted_to_queued() {
        let mut state = new_state();
        assert!(
            InputStateMachine::transition(
                &mut state,
                InputLifecycleState::Queued,
                Some("policy resolved".into()),
            )
            .is_ok()
        );
        assert_eq!(state.current_state, InputLifecycleState::Queued);
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.history[0].from, InputLifecycleState::Accepted);
        assert_eq!(state.history[0].to, InputLifecycleState::Queued);
    }

    #[test]
    fn queued_to_staged() {
        let mut state = new_state();
        InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None).unwrap();
        assert!(
            InputStateMachine::transition(&mut state, InputLifecycleState::Staged, None,).is_ok()
        );
        assert_eq!(state.current_state, InputLifecycleState::Staged);
    }

    #[test]
    fn staged_to_applied() {
        let mut state = new_state();
        InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None).unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Staged, None).unwrap();
        assert!(
            InputStateMachine::transition(&mut state, InputLifecycleState::Applied, None,).is_ok()
        );
        assert_eq!(state.current_state, InputLifecycleState::Applied);
    }

    #[test]
    fn applied_to_applied_pending_consumption() {
        let mut state = new_state();
        InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None).unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Staged, None).unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Applied, None).unwrap();
        assert!(
            InputStateMachine::transition(
                &mut state,
                InputLifecycleState::AppliedPendingConsumption,
                None,
            )
            .is_ok()
        );
        assert_eq!(
            state.current_state,
            InputLifecycleState::AppliedPendingConsumption
        );
    }

    #[test]
    fn applied_pending_to_consumed() {
        let mut state = new_state();
        InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None).unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Staged, None).unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Applied, None).unwrap();
        InputStateMachine::transition(
            &mut state,
            InputLifecycleState::AppliedPendingConsumption,
            None,
        )
        .unwrap();
        assert!(
            InputStateMachine::transition(&mut state, InputLifecycleState::Consumed, None,).is_ok()
        );
        assert!(state.is_terminal());
        assert!(matches!(
            state.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        ));
    }

    #[test]
    fn full_happy_path_history() {
        let mut state = new_state();
        InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None).unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Staged, None).unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Applied, None).unwrap();
        InputStateMachine::transition(
            &mut state,
            InputLifecycleState::AppliedPendingConsumption,
            None,
        )
        .unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Consumed, None).unwrap();
        assert_eq!(state.history.len(), 5);
    }

    // ---- Staged rollback on run failure ----

    #[test]
    fn staged_to_queued_rollback() {
        let mut state = new_state();
        InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None).unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Staged, None).unwrap();
        assert!(
            InputStateMachine::transition(
                &mut state,
                InputLifecycleState::Queued,
                Some("run failed, rollback".into()),
            )
            .is_ok()
        );
        assert_eq!(state.current_state, InputLifecycleState::Queued);
    }

    // ---- §15 HARD RULE: AppliedPendingConsumption → Queued rejected ----

    #[test]
    fn applied_pending_to_queued_rejected() {
        let mut state = new_state();
        InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None).unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Staged, None).unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Applied, None).unwrap();
        InputStateMachine::transition(
            &mut state,
            InputLifecycleState::AppliedPendingConsumption,
            None,
        )
        .unwrap();

        let result = InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            InputStateMachineError::InvalidTransition { .. }
        ));
        // State unchanged
        assert_eq!(
            state.current_state,
            InputLifecycleState::AppliedPendingConsumption
        );
    }

    // ---- Terminal states reject all transitions ----

    #[test]
    fn consumed_rejects_all() {
        let mut state = new_state();
        InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None).unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Staged, None).unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Applied, None).unwrap();
        InputStateMachine::transition(
            &mut state,
            InputLifecycleState::AppliedPendingConsumption,
            None,
        )
        .unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Consumed, None).unwrap();

        for target in [
            InputLifecycleState::Accepted,
            InputLifecycleState::Queued,
            InputLifecycleState::Staged,
            InputLifecycleState::Applied,
            InputLifecycleState::Consumed,
        ] {
            let result = InputStateMachine::transition(&mut state, target, None);
            assert!(result.is_err());
            assert!(matches!(
                result.unwrap_err(),
                InputStateMachineError::TerminalState { .. }
            ));
        }
    }

    #[test]
    fn superseded_rejects_all() {
        let mut state = new_state();
        InputStateMachine::transition(&mut state, InputLifecycleState::Superseded, None).unwrap();
        assert!(state.is_terminal());

        let result = InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None);
        assert!(result.is_err());
    }

    #[test]
    fn coalesced_rejects_all() {
        let mut state = new_state();
        InputStateMachine::transition(&mut state, InputLifecycleState::Coalesced, None).unwrap();
        assert!(state.is_terminal());

        let result = InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None);
        assert!(result.is_err());
    }

    #[test]
    fn abandoned_rejects_all() {
        let mut state = new_state();
        InputStateMachine::abandon(&mut state, InputAbandonReason::Retired).unwrap();
        assert!(state.is_terminal());

        let result = InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None);
        assert!(result.is_err());
    }

    // ---- Abandon from various states ----

    #[test]
    fn abandon_from_accepted() {
        let mut state = new_state();
        assert!(InputStateMachine::abandon(&mut state, InputAbandonReason::Retired).is_ok());
        assert!(matches!(
            state.terminal_outcome,
            Some(InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::Retired,
            })
        ));
    }

    #[test]
    fn abandon_from_queued() {
        let mut state = new_state();
        InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None).unwrap();
        assert!(InputStateMachine::abandon(&mut state, InputAbandonReason::Reset).is_ok());
    }

    #[test]
    fn abandon_from_staged() {
        let mut state = new_state();
        InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None).unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Staged, None).unwrap();
        assert!(InputStateMachine::abandon(&mut state, InputAbandonReason::Destroyed).is_ok());
    }

    #[test]
    fn abandon_from_applied() {
        let mut state = new_state();
        InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None).unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Staged, None).unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Applied, None).unwrap();
        assert!(InputStateMachine::abandon(&mut state, InputAbandonReason::Cancelled).is_ok());
    }

    #[test]
    fn abandon_from_applied_pending() {
        let mut state = new_state();
        InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None).unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Staged, None).unwrap();
        InputStateMachine::transition(&mut state, InputLifecycleState::Applied, None).unwrap();
        InputStateMachine::transition(
            &mut state,
            InputLifecycleState::AppliedPendingConsumption,
            None,
        )
        .unwrap();
        assert!(InputStateMachine::abandon(&mut state, InputAbandonReason::Retired).is_ok());
    }

    // ---- Accepted direct to Consumed (Ignore + OnAccept) ----

    #[test]
    fn accepted_to_consumed_ignore_on_accept() {
        let mut state = new_state();
        assert!(
            InputStateMachine::transition(
                &mut state,
                InputLifecycleState::Consumed,
                Some("Ignore + OnAccept".into()),
            )
            .is_ok()
        );
        assert!(state.is_terminal());
    }

    // ---- Invalid transitions ----

    #[test]
    fn accepted_to_staged_invalid() {
        let mut state = new_state();
        let result = InputStateMachine::transition(&mut state, InputLifecycleState::Staged, None);
        assert!(result.is_err());
    }

    #[test]
    fn accepted_to_applied_invalid() {
        let mut state = new_state();
        let result = InputStateMachine::transition(&mut state, InputLifecycleState::Applied, None);
        assert!(result.is_err());
    }

    #[test]
    fn queued_to_applied_invalid() {
        let mut state = new_state();
        InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None).unwrap();
        let result = InputStateMachine::transition(&mut state, InputLifecycleState::Applied, None);
        assert!(result.is_err());
    }

    #[test]
    fn queued_to_consumed_invalid() {
        let mut state = new_state();
        InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None).unwrap();
        let result = InputStateMachine::transition(&mut state, InputLifecycleState::Consumed, None);
        assert!(result.is_err());
    }

    // ---- Set terminal outcome ----

    #[test]
    fn set_terminal_outcome_superseded() {
        let mut state = new_state();
        let superseder = InputId::new();
        InputStateMachine::transition(&mut state, InputLifecycleState::Superseded, None).unwrap();
        InputStateMachine::set_terminal_outcome(
            &mut state,
            InputTerminalOutcome::Superseded {
                superseded_by: superseder,
            },
        );
        assert!(matches!(
            state.terminal_outcome,
            Some(InputTerminalOutcome::Superseded { .. })
        ));
    }

    // ---- History recording ----

    #[test]
    fn history_records_reason() {
        let mut state = new_state();
        InputStateMachine::transition(
            &mut state,
            InputLifecycleState::Queued,
            Some("test reason".into()),
        )
        .unwrap();
        assert_eq!(state.history[0].reason.as_deref(), Some("test reason"));
    }

    #[test]
    fn history_records_timestamps() {
        let mut state = new_state();
        let before = Utc::now();
        InputStateMachine::transition(&mut state, InputLifecycleState::Queued, None).unwrap();
        let after = Utc::now();
        assert!(state.history[0].timestamp >= before);
        assert!(state.history[0].timestamp <= after);
    }
}
