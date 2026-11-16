//! Projection of the queued-input recall onto its wire contract. It lives in
//! the runtime so the RPC and browser surfaces report the same classes.

use crate::meerkat_machine::dsl::{InputPublicLifecycleState, InputPublicTerminalOutcome};
use crate::queued_input_recall::QueuedInputRecall;
use meerkat_contracts::{
    WireInputLifecycleState, WireQueuedInputRecall, wire::runtime::WireInputTerminalOutcome,
};
use meerkat_core::lifecycle::InputId;

/// Parse a wire input id. A malformed id is a caller error, never an unknown
/// input.
pub fn parse_wire_input_id(raw: &str) -> Result<InputId, String> {
    uuid::Uuid::parse_str(raw)
        .map(InputId::from_uuid)
        .map_err(|error| format!("invalid input_id '{raw}': {error}"))
}

/// Project a runtime [`QueuedInputRecall`] onto [`WireQueuedInputRecall`].
/// The lifecycle and terminal classes are resolved by generated MeerkatMachine
/// authority; the wire only mirrors them.
pub fn wire_queued_input_recall(
    recall: &QueuedInputRecall,
) -> Result<WireQueuedInputRecall, String> {
    Ok(match recall {
        QueuedInputRecall::Abandoned {
            input_id,
            attempt_count,
        } => WireQueuedInputRecall::Abandoned {
            input_id: input_id.to_string(),
            attempt_count: *attempt_count,
        },
        QueuedInputRecall::NotQueued {
            input_id,
            phase,
            run_id,
        } => WireQueuedInputRecall::NotQueued {
            input_id: input_id.to_string(),
            phase: wire_input_lifecycle_state(
                crate::meerkat_machine::resolve_input_public_lifecycle_projection(
                    input_id, *phase,
                )?,
            ),
            run_id: run_id.as_ref().map(ToString::to_string),
        },
        QueuedInputRecall::AlreadyTerminal { input_id, terminal } => {
            let terminal =
                crate::meerkat_machine::resolve_input_public_terminal_outcome_projection(
                    input_id, terminal,
                )?
                .ok_or_else(|| {
                    format!("the terminal of input {input_id} has no public terminal class")
                })?;
            WireQueuedInputRecall::AlreadyTerminal {
                input_id: input_id.to_string(),
                terminal: wire_input_terminal_outcome(terminal),
            }
        }
    })
}

fn wire_input_lifecycle_state(state: InputPublicLifecycleState) -> WireInputLifecycleState {
    // Transport-only mirror of the generated public lifecycle class.
    match state {
        InputPublicLifecycleState::Accepted => WireInputLifecycleState::Accepted,
        InputPublicLifecycleState::Queued => WireInputLifecycleState::Queued,
        InputPublicLifecycleState::Staged => WireInputLifecycleState::Staged,
        InputPublicLifecycleState::Applied => WireInputLifecycleState::Applied,
        InputPublicLifecycleState::AppliedPendingConsumption => {
            WireInputLifecycleState::AppliedPendingConsumption
        }
        InputPublicLifecycleState::Consumed => WireInputLifecycleState::Consumed,
        InputPublicLifecycleState::Superseded => WireInputLifecycleState::Superseded,
        InputPublicLifecycleState::Coalesced => WireInputLifecycleState::Coalesced,
        InputPublicLifecycleState::Abandoned => WireInputLifecycleState::Abandoned,
    }
}

fn wire_input_terminal_outcome(outcome: InputPublicTerminalOutcome) -> WireInputTerminalOutcome {
    // Transport-only mirror of the generated public terminal result class.
    match outcome {
        InputPublicTerminalOutcome::Completed => WireInputTerminalOutcome::Completed,
        InputPublicTerminalOutcome::Abandoned => WireInputTerminalOutcome::Abandoned,
        InputPublicTerminalOutcome::Superseded => WireInputTerminalOutcome::Superseded,
        InputPublicTerminalOutcome::Coalesced => WireInputTerminalOutcome::Coalesced,
        InputPublicTerminalOutcome::Cancelled => WireInputTerminalOutcome::Cancelled,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input_state::{InputAbandonReason, InputLifecycleState, InputTerminalOutcome};
    use meerkat_core::lifecycle::RunId;

    #[test]
    fn every_answer_projects_its_generated_class() {
        let input_id = InputId::new();
        assert_eq!(
            wire_queued_input_recall(&QueuedInputRecall::Abandoned {
                input_id: input_id.clone(),
                attempt_count: 2,
            })
            .unwrap(),
            WireQueuedInputRecall::Abandoned {
                input_id: input_id.to_string(),
                attempt_count: 2,
            }
        );
        let run_id = RunId::new();
        assert_eq!(
            wire_queued_input_recall(&QueuedInputRecall::NotQueued {
                input_id: input_id.clone(),
                phase: InputLifecycleState::Staged,
                run_id: Some(run_id.clone()),
            })
            .unwrap(),
            WireQueuedInputRecall::NotQueued {
                input_id: input_id.to_string(),
                phase: WireInputLifecycleState::Staged,
                run_id: Some(run_id.to_string()),
            }
        );
        // The cancelled-versus-abandoned distinction is the machine's: a
        // recalled input reads `cancelled`, a retired one `abandoned`.
        for (reason, class) in [
            (
                InputAbandonReason::Cancelled,
                WireInputTerminalOutcome::Cancelled,
            ),
            (
                InputAbandonReason::Retired,
                WireInputTerminalOutcome::Abandoned,
            ),
        ] {
            assert_eq!(
                wire_queued_input_recall(&QueuedInputRecall::AlreadyTerminal {
                    input_id: input_id.clone(),
                    terminal: InputTerminalOutcome::Abandoned { reason },
                })
                .unwrap(),
                WireQueuedInputRecall::AlreadyTerminal {
                    input_id: input_id.to_string(),
                    terminal: class,
                }
            );
        }
        assert_eq!(
            wire_queued_input_recall(&QueuedInputRecall::AlreadyTerminal {
                input_id: input_id.clone(),
                terminal: InputTerminalOutcome::Consumed,
            })
            .unwrap(),
            WireQueuedInputRecall::AlreadyTerminal {
                input_id: input_id.to_string(),
                terminal: WireInputTerminalOutcome::Completed,
            }
        );
    }

    #[test]
    fn malformed_input_ids_are_caller_errors() {
        assert!(parse_wire_input_id("not-a-uuid").is_err());
        let input_id = InputId::new();
        assert_eq!(
            parse_wire_input_id(&input_id.to_string()).unwrap(),
            input_id
        );
    }
}
