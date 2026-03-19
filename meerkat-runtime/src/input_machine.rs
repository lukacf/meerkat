//! S15 InputStateMachine -- DEPRECATED in favor of InputLifecycleAuthority.
//!
//! This module is retained for backward compatibility of the error type.
//! All new code should use `InputLifecycleAuthority` and `InputLifecycleInput`
//! from `input_lifecycle_authority` instead.

use crate::input_state::InputLifecycleState;
use meerkat_core::lifecycle::InputId;

/// Errors from the input state machine.
///
/// Retained for backward compatibility. New code should use
/// `InputLifecycleError` from `input_lifecycle_authority`.
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

/// DEPRECATED: Use `InputLifecycleAuthority` instead.
pub struct InputStateMachine;
