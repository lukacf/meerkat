//! Projection of the run-fenced Stop onto its wire contract. It lives in the
//! runtime so the RPC, REST, browser and supervisor-bridge surfaces all report
//! the same classes.

use crate::completion::CompletionOutcome;
use crate::meerkat_machine::dsl::InputPublicTerminalOutcome;
use crate::run_stop::{RunStopContributor, RunStopReceipt};
use crate::runtime_state::RuntimeState;
use meerkat_contracts::{
    WireRunStopCompletion, WireRunStopContributor, WireRunStopReceipt, WireRuntimeState,
    wire::runtime::WireInputTerminalOutcome,
};
use meerkat_core::lifecycle::RunId;

/// Parse a wire run id. A malformed id is a caller error, never a stale run.
pub fn parse_wire_run_id(raw: &str) -> Result<RunId, String> {
    uuid::Uuid::parse_str(raw)
        .map(RunId::from_uuid)
        .map_err(|error| format!("invalid run_id '{raw}': {error}"))
}

/// Project a runtime [`RunStopReceipt`] onto [`WireRunStopReceipt`]. Every
/// class is resolved by the runtime: the delivered completion, the generated
/// public terminal class, and the runtime state that refused the stop.
pub fn wire_run_stop_receipt(receipt: &RunStopReceipt) -> Result<WireRunStopReceipt, String> {
    Ok(match receipt {
        RunStopReceipt::Stopped {
            run_id,
            contributors,
        } => WireRunStopReceipt::Stopped {
            run_id: run_id.to_string(),
            contributors: contributors
                .iter()
                .map(wire_run_stop_contributor)
                .collect::<Result<_, _>>()?,
        },
        RunStopReceipt::NotCurrent {
            run_id,
            current_run_id,
        } => WireRunStopReceipt::NotCurrent {
            run_id: run_id.to_string(),
            current_run_id: current_run_id.as_ref().map(ToString::to_string),
        },
        RunStopReceipt::NotStoppable { run_id, state } => WireRunStopReceipt::NotStoppable {
            run_id: run_id.to_string(),
            state: wire_runtime_state(*state)?,
        },
    })
}

fn wire_run_stop_contributor(
    contributor: &RunStopContributor,
) -> Result<WireRunStopContributor, String> {
    let terminal = match contributor.terminal.as_ref() {
        Some(terminal) => crate::meerkat_machine::resolve_input_public_terminal_outcome_projection(
            &contributor.input_id,
            terminal,
        )?
        .map(wire_input_terminal_outcome),
        None => None,
    };
    Ok(WireRunStopContributor {
        input_id: contributor.input_id.to_string(),
        completion: wire_run_stop_completion(&contributor.outcome),
        terminal,
    })
}

fn wire_run_stop_completion(outcome: &CompletionOutcome) -> WireRunStopCompletion {
    match outcome {
        CompletionOutcome::Completed(_) => WireRunStopCompletion::Completed,
        CompletionOutcome::CompletedWithoutResult => WireRunStopCompletion::CompletedWithoutResult,
        CompletionOutcome::CallbackPending { .. }
        | CompletionOutcome::CallbackBatchPending { .. } => WireRunStopCompletion::CallbackPending,
        CompletionOutcome::Cancelled => WireRunStopCompletion::Cancelled,
        CompletionOutcome::Abandoned { .. } => WireRunStopCompletion::Abandoned,
        CompletionOutcome::AbandonedWithError { .. } => WireRunStopCompletion::AbandonedWithError,
        CompletionOutcome::CompletedWithFinalizationFailure { .. } => {
            WireRunStopCompletion::CompletedWithFinalizationFailure
        }
        CompletionOutcome::RuntimeTerminated { .. } => WireRunStopCompletion::RuntimeTerminated,
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

fn wire_runtime_state(state: RuntimeState) -> Result<WireRuntimeState, String> {
    // Transport-only mirror of the runtime phase.
    Ok(match state {
        RuntimeState::Initializing => WireRuntimeState::Initializing,
        RuntimeState::Idle => WireRuntimeState::Idle,
        RuntimeState::Attached => WireRuntimeState::Attached,
        RuntimeState::Running => WireRuntimeState::Running,
        RuntimeState::Retired => WireRuntimeState::Retired,
        RuntimeState::Stopped => WireRuntimeState::Stopped,
        RuntimeState::Destroyed => WireRuntimeState::Destroyed,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn not_current_and_not_stoppable_project_their_run_ids() {
        let run_id = RunId::new();
        let current = RunId::new();
        assert_eq!(
            wire_run_stop_receipt(&RunStopReceipt::NotCurrent {
                run_id: run_id.clone(),
                current_run_id: Some(current.clone()),
            })
            .unwrap(),
            WireRunStopReceipt::NotCurrent {
                run_id: run_id.to_string(),
                current_run_id: Some(current.to_string()),
            }
        );
        assert_eq!(
            wire_run_stop_receipt(&RunStopReceipt::NotStoppable {
                run_id: run_id.clone(),
                state: RuntimeState::Stopped,
            })
            .unwrap(),
            WireRunStopReceipt::NotStoppable {
                run_id: run_id.to_string(),
                state: WireRuntimeState::Stopped,
            }
        );
    }

    #[test]
    fn malformed_run_ids_are_caller_errors() {
        assert!(parse_wire_run_id("not-a-uuid").is_err());
        let run_id = RunId::new();
        assert_eq!(parse_wire_run_id(&run_id.to_string()).unwrap(), run_id);
    }
}
