//! §25 Lifecycle operations — retire, recycle, reset, destroy.
//!
//! These operate on InputState entries, transitioning all non-terminal
//! inputs to Abandoned with the appropriate reason. Terminal states
//! are left unchanged.

use meerkat_core::lifecycle::InputId;

use crate::input_lifecycle_authority::InputLifecycleInput;
use crate::input_state::{InputAbandonReason, InputState};

/// Abandon all non-terminal inputs with the given reason.
/// Returns the number of inputs abandoned.
pub fn abandon_non_terminal(states: &mut [&mut InputState], reason: InputAbandonReason) -> usize {
    let mut count = 0;
    for state in states {
        if !state.is_terminal()
            && state
                .apply(InputLifecycleInput::Abandon {
                    reason: reason.clone(),
                })
                .is_ok()
        {
            count += 1;
        }
    }
    count
}

/// Check which inputs would be abandoned by a lifecycle operation.
pub fn would_abandon(states: &[&InputState]) -> Vec<InputId> {
    states
        .iter()
        .filter(|s| !s.is_terminal())
        .map(|s| s.input_id.clone())
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input_lifecycle_authority::InputLifecycleInput;
    use crate::input_state::InputLifecycleState;
    use meerkat_core::lifecycle::RunId;

    #[test]
    fn abandon_non_terminal_inputs() {
        let mut s1 = InputState::new_accepted(InputId::new());
        let mut s2 = InputState::new_accepted(InputId::new());
        s2.apply(InputLifecycleInput::QueueAccepted).unwrap();

        // s3 is terminal (already consumed)
        let mut s3 = InputState::new_accepted(InputId::new());
        s3.apply(InputLifecycleInput::ConsumeOnAccept).unwrap();

        let mut refs: Vec<&mut InputState> = vec![&mut s1, &mut s2, &mut s3];
        let count = abandon_non_terminal(&mut refs, InputAbandonReason::Retired);
        assert_eq!(count, 2); // s1 and s2 abandoned, s3 unchanged

        assert!(s1.is_terminal());
        assert!(s2.is_terminal());
        assert!(s3.is_terminal()); // Was already terminal
    }

    #[test]
    fn terminal_unchanged() {
        let mut s = InputState::new_accepted(InputId::new());
        s.apply(InputLifecycleInput::QueueAccepted).unwrap();
        s.apply(InputLifecycleInput::Supersede).unwrap();

        let mut refs: Vec<&mut InputState> = vec![&mut s];
        let count = abandon_non_terminal(&mut refs, InputAbandonReason::Reset);
        assert_eq!(count, 0);
    }

    #[test]
    fn would_abandon_predicts_correctly() {
        let s1 = InputState::new_accepted(InputId::new());
        let mut s2 = InputState::new_accepted(InputId::new());
        s2.apply(InputLifecycleInput::ConsumeOnAccept).unwrap();

        let refs: Vec<&InputState> = vec![&s1, &s2];
        let ids = would_abandon(&refs);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], s1.input_id);
    }

    #[test]
    fn abandon_from_all_non_terminal_states() {
        for initial_state in [
            InputLifecycleState::Accepted,
            InputLifecycleState::Queued,
            InputLifecycleState::Staged,
            InputLifecycleState::Applied,
            InputLifecycleState::AppliedPendingConsumption,
        ] {
            let mut state = InputState::new_accepted(InputId::new());
            let run_id = RunId::new();
            match initial_state {
                InputLifecycleState::Accepted => {}
                InputLifecycleState::Queued => {
                    state.apply(InputLifecycleInput::QueueAccepted).unwrap();
                }
                InputLifecycleState::Staged => {
                    state.apply(InputLifecycleInput::QueueAccepted).unwrap();
                    state
                        .apply(InputLifecycleInput::StageForRun {
                            run_id: run_id.clone(),
                        })
                        .unwrap();
                }
                InputLifecycleState::Applied => {
                    state.apply(InputLifecycleInput::QueueAccepted).unwrap();
                    state
                        .apply(InputLifecycleInput::StageForRun {
                            run_id: run_id.clone(),
                        })
                        .unwrap();
                    state
                        .apply(InputLifecycleInput::MarkApplied {
                            run_id: run_id.clone(),
                        })
                        .unwrap();
                }
                InputLifecycleState::AppliedPendingConsumption => {
                    state.apply(InputLifecycleInput::QueueAccepted).unwrap();
                    state
                        .apply(InputLifecycleInput::StageForRun {
                            run_id: run_id.clone(),
                        })
                        .unwrap();
                    state
                        .apply(InputLifecycleInput::MarkApplied {
                            run_id: run_id.clone(),
                        })
                        .unwrap();
                    state
                        .apply(InputLifecycleInput::MarkAppliedPendingConsumption {
                            boundary_sequence: 1,
                        })
                        .unwrap();
                }
                _ => unreachable!(),
            }

            let mut refs: Vec<&mut InputState> = vec![&mut state];
            let count = abandon_non_terminal(&mut refs, InputAbandonReason::Destroyed);
            assert_eq!(count, 1, "Should abandon from {initial_state:?}");
            assert!(state.is_terminal());
        }
    }
}
