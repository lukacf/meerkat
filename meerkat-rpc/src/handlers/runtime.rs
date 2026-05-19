//! Runtime-backed session support handlers.

use serde_json::value::RawValue;

use meerkat_contracts::{
    RuntimeAcceptOutcomeType, RuntimeAcceptResult, WireInputLifecycleState, WireInputState,
    WireInputStateHistoryEntry,
    wire::runtime::{WireInputPolicy, WireInputTerminalOutcome},
};
use meerkat_core::SessionId;
use meerkat_runtime::service_ext::SessionServiceRuntimeExt;

use super::{RpcResponseExt, parse_params};
use crate::protocol::{RpcId, RpcResponse};

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

pub(crate) fn to_wire_input_state(
    bundle: meerkat_runtime::input_state::StoredInputState,
) -> Result<WireInputState, String> {
    // The public lifecycle and terminal classes are resolved by generated
    // MeerkatMachine authority from the machine-derived seed. RPC only mirrors
    // the generated class onto the transport enum.
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

pub(crate) fn to_wire_accept_result(
    outcome: meerkat_runtime::AcceptOutcome,
) -> Result<RuntimeAcceptResult, String> {
    Ok(match outcome {
        meerkat_runtime::AcceptOutcome::Accepted {
            input_id,
            policy,
            state,
            seed,
        } => {
            let bundle = meerkat_runtime::input_state::StoredInputState { state, seed };
            // Wave B: policy is a typed `WireInputPolicy`. The runtime
            // `policy` value is not yet richly projectable without wave-c
            // plumbing, so callers get `None` until the typed projection
            // lands; the structural shape is typed regardless.
            let _ = policy;
            RuntimeAcceptResult {
                outcome_type: RuntimeAcceptOutcomeType::Accepted,
                input_id: Some(input_id.to_string()),
                existing_id: None,
                reason: None,
                policy: Option::<WireInputPolicy>::None,
                state: Some(to_wire_input_state(bundle)?),
            }
        }
        meerkat_runtime::AcceptOutcome::Deduplicated {
            input_id,
            existing_id,
        } => RuntimeAcceptResult {
            outcome_type: RuntimeAcceptOutcomeType::Deduplicated,
            input_id: Some(input_id.to_string()),
            existing_id: Some(existing_id.to_string()),
            reason: None,
            policy: None,
            state: None,
        },
        meerkat_runtime::AcceptOutcome::Rejected { reason } => RuntimeAcceptResult {
            outcome_type: RuntimeAcceptOutcomeType::Rejected,
            input_id: None,
            existing_id: None,
            reason: Some(reason.to_string()),
            policy: None,
            state: None,
        },
        _ => return Err("unsupported accept outcome variant".to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::lifecycle::InputId;
    use meerkat_runtime::input_state::{
        InputAbandonReason, InputState, InputStateSeed, InputTerminalOutcome,
    };
    use meerkat_runtime::{
        ApplyMode, ConsumePoint, DrainPolicy, PolicyDecision, QueueMode, RoutingDisposition,
        WakeMode,
    };

    #[test]
    fn accept_result_projects_machine_seed_state() {
        let input_id = InputId::new();
        let outcome = meerkat_runtime::AcceptOutcome::Accepted {
            input_id: input_id.clone(),
            policy: PolicyDecision {
                apply_mode: ApplyMode::Ignore,
                wake_mode: WakeMode::None,
                queue_mode: QueueMode::Priority,
                consume_point: ConsumePoint::OnAccept,
                drain_policy: DrainPolicy::Ignore,
                routing_disposition: RoutingDisposition::Drop,
                record_transcript: false,
                emit_operator_content: false,
                policy_version: meerkat_runtime::identifiers::PolicyVersion(1),
            },
            state: InputState::new_accepted(input_id),
            seed: InputStateSeed {
                phase: meerkat_runtime::InputLifecycleState::Consumed,
                last_run_id: None,
                last_boundary_sequence: None,
                admission_sequence: Some(7),
                terminal_outcome: Some(InputTerminalOutcome::Consumed),
                attempt_count: 3,
                recovery_lane: None,
            },
        };

        let wire = to_wire_accept_result(outcome).expect("accepted result should project");
        let state = wire.state.expect("accepted result should include state");
        assert_eq!(state.current_state, WireInputLifecycleState::Consumed);
        assert_eq!(
            state.terminal_outcome,
            Some(WireInputTerminalOutcome::Completed)
        );
        assert_eq!(state.attempt_count, 3);
    }

    #[test]
    fn cancelled_abandon_projects_generated_cancelled_public_outcome() {
        let input_id = InputId::new();
        let state = to_wire_input_state(meerkat_runtime::input_state::StoredInputState {
            state: InputState::new_accepted(input_id),
            seed: InputStateSeed {
                phase: meerkat_runtime::InputLifecycleState::Abandoned,
                last_run_id: None,
                last_boundary_sequence: None,
                admission_sequence: Some(9),
                terminal_outcome: Some(InputTerminalOutcome::Abandoned {
                    reason: InputAbandonReason::Cancelled,
                }),
                attempt_count: 1,
                recovery_lane: None,
            },
        })
        .expect("cancelled terminal projection should be generated");

        assert_eq!(state.current_state, WireInputLifecycleState::Abandoned);
        assert_eq!(
            state.terminal_outcome,
            Some(WireInputTerminalOutcome::Cancelled)
        );
    }
}
