//! Runtime-backed session support handlers.

use serde_json::value::RawValue;

use meerkat_contracts::{
    RuntimeAcceptOutcomeType, RuntimeAcceptResult, wire::runtime::WireInputPolicy,
};
use meerkat_core::SessionId;
use meerkat_runtime::service_ext::SessionServiceRuntimeExt;

use super::{RpcResponseExt, parse_params};
use crate::protocol::{RpcId, RpcResponse};

/// The input-state projection; its one owner is the facade.
pub(crate) use meerkat::surface::wire_input_state as to_wire_input_state;

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
            ..
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
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_contracts::{WireInputLifecycleState, wire::runtime::WireInputTerminalOutcome};
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
