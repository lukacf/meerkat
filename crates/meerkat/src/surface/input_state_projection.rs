//! The one wire projection of a stored runtime input state, shared by every
//! surface that answers an input-status read.

use meerkat_contracts::{
    WireInputLifecycleState, WireInputState, WireInputStateHistoryEntry,
    wire::runtime::WireInputTerminalOutcome,
};

fn to_wire_input_lifecycle_state(
    state: meerkat_runtime::meerkat_machine::dsl::InputPublicLifecycleState,
) -> WireInputLifecycleState {
    // Transport-only mirror of the generated public lifecycle class.
    match state {
        meerkat_runtime::meerkat_machine::dsl::InputPublicLifecycleState::Accepted => {
            WireInputLifecycleState::Accepted
        }
        meerkat_runtime::meerkat_machine::dsl::InputPublicLifecycleState::Queued => {
            WireInputLifecycleState::Queued
        }
        meerkat_runtime::meerkat_machine::dsl::InputPublicLifecycleState::Staged => {
            WireInputLifecycleState::Staged
        }
        meerkat_runtime::meerkat_machine::dsl::InputPublicLifecycleState::Applied => {
            WireInputLifecycleState::Applied
        }
        meerkat_runtime::meerkat_machine::dsl::InputPublicLifecycleState::AppliedPendingConsumption => {
            WireInputLifecycleState::AppliedPendingConsumption
        }
        meerkat_runtime::meerkat_machine::dsl::InputPublicLifecycleState::Consumed => {
            WireInputLifecycleState::Consumed
        }
        meerkat_runtime::meerkat_machine::dsl::InputPublicLifecycleState::Superseded => {
            WireInputLifecycleState::Superseded
        }
        meerkat_runtime::meerkat_machine::dsl::InputPublicLifecycleState::Coalesced => {
            WireInputLifecycleState::Coalesced
        }
        meerkat_runtime::meerkat_machine::dsl::InputPublicLifecycleState::Abandoned => {
            WireInputLifecycleState::Abandoned
        }
    }
}

fn to_wire_input_terminal_outcome(
    outcome: meerkat_runtime::meerkat_machine::dsl::InputPublicTerminalOutcome,
) -> WireInputTerminalOutcome {
    // Transport-only mirror of the generated public terminal result class.
    match outcome {
        meerkat_runtime::meerkat_machine::dsl::InputPublicTerminalOutcome::Completed => {
            WireInputTerminalOutcome::Completed
        }
        meerkat_runtime::meerkat_machine::dsl::InputPublicTerminalOutcome::Abandoned => {
            WireInputTerminalOutcome::Abandoned
        }
        meerkat_runtime::meerkat_machine::dsl::InputPublicTerminalOutcome::Superseded => {
            WireInputTerminalOutcome::Superseded
        }
        meerkat_runtime::meerkat_machine::dsl::InputPublicTerminalOutcome::Coalesced => {
            WireInputTerminalOutcome::Coalesced
        }
        meerkat_runtime::meerkat_machine::dsl::InputPublicTerminalOutcome::Cancelled => {
            WireInputTerminalOutcome::Cancelled
        }
    }
}

/// Project a stored input state onto [`WireInputState`].
///
/// # Errors
/// The generated machine authority refused to classify the stored state.
pub fn wire_input_state(
    bundle: meerkat_runtime::input_state::StoredInputState,
) -> Result<WireInputState, String> {
    // The public lifecycle and terminal classes are resolved by generated
    // MeerkatMachine authority from the machine-derived seed. The wire only
    // mirrors the generated class onto the transport enum.
    let meerkat_runtime::input_state::StoredInputState { state, seed } = bundle;
    let public_projection =
        meerkat_runtime::meerkat_machine::resolve_input_public_state_projection(
            &state.input_id,
            &seed,
        )?;
    let history = state
        .history()
        .iter()
        .map(|entry| {
            let from = meerkat_runtime::meerkat_machine::resolve_input_public_lifecycle_projection(
                &state.input_id,
                entry.from,
            )?;
            let to = meerkat_runtime::meerkat_machine::resolve_input_public_lifecycle_projection(
                &state.input_id,
                entry.to,
            )?;
            Ok(WireInputStateHistoryEntry {
                timestamp: entry.timestamp.to_rfc3339(),
                from: to_wire_input_lifecycle_state(from),
                to: to_wire_input_lifecycle_state(to),
                reason: entry.reason.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(WireInputState {
        input_id: state.input_id.to_string(),
        current_state: to_wire_input_lifecycle_state(public_projection.lifecycle_state),
        policy: None,
        terminal_outcome: public_projection
            .terminal_outcome
            .map(to_wire_input_terminal_outcome),
        durability: None,
        idempotency_key: None,
        attempt_count: seed.attempt_count,
        recovery_count: state.recovery_count,
        history,
        reconstruction_source: None,
        persisted_input: None,
        last_run_id: seed.last_run_id.map(|id| id.to_string()),
        last_boundary_sequence: seed.last_boundary_sequence,
        created_at: state.created_at.to_rfc3339(),
        updated_at: state.updated_at().to_rfc3339(),
    })
}
