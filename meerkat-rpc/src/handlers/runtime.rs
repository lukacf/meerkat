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
    state: meerkat_runtime::InputLifecycleState,
) -> Result<WireInputLifecycleState, String> {
    Ok(match state {
        meerkat_runtime::InputLifecycleState::Accepted => WireInputLifecycleState::Accepted,
        meerkat_runtime::InputLifecycleState::Queued => WireInputLifecycleState::Queued,
        meerkat_runtime::InputLifecycleState::Staged => WireInputLifecycleState::Staged,
        meerkat_runtime::InputLifecycleState::Applied => WireInputLifecycleState::Applied,
        meerkat_runtime::InputLifecycleState::AppliedPendingConsumption => {
            WireInputLifecycleState::AppliedPendingConsumption
        }
        meerkat_runtime::InputLifecycleState::Consumed => WireInputLifecycleState::Consumed,
        meerkat_runtime::InputLifecycleState::Superseded => WireInputLifecycleState::Superseded,
        meerkat_runtime::InputLifecycleState::Coalesced => WireInputLifecycleState::Coalesced,
        meerkat_runtime::InputLifecycleState::Abandoned => WireInputLifecycleState::Abandoned,
        _ => return Err("unsupported input lifecycle state variant".to_string()),
    })
}

fn to_wire_input_terminal_outcome(
    outcome: meerkat_runtime::input_state::InputTerminalOutcome,
) -> Result<WireInputTerminalOutcome, String> {
    Ok(match outcome {
        meerkat_runtime::input_state::InputTerminalOutcome::Consumed => {
            WireInputTerminalOutcome::Completed
        }
        meerkat_runtime::input_state::InputTerminalOutcome::Abandoned { .. } => {
            WireInputTerminalOutcome::Abandoned
        }
        meerkat_runtime::input_state::InputTerminalOutcome::Superseded { .. } => {
            WireInputTerminalOutcome::Superseded
        }
        meerkat_runtime::input_state::InputTerminalOutcome::Coalesced { .. } => {
            WireInputTerminalOutcome::Coalesced
        }
        _ => return Err("unsupported input terminal outcome variant".to_string()),
    })
}

pub(crate) fn to_wire_input_state(
    bundle: meerkat_runtime::input_state::StoredInputState,
) -> Result<WireInputState, String> {
    // Wave B: fields that were raw `serde_json::Value` pre-B-9 are now
    // typed enums on the wire. The runtime-side `StoredInputState` still
    // carries the richer internal shapes, so projection happens here —
    // wave-c will plumb the typed projections through `meerkat-runtime`
    // so this fn reduces to field-wise clones. For wave-b the policy /
    // terminal_outcome / durability / reconstruction_source /
    // persisted_input fields are left as `None` until wave-c lands the
    // typed conversions on the runtime side. The structural shape is
    // already typed on the wire — callers consuming this projection see
    // typed enums or nothing.
    let meerkat_runtime::input_state::StoredInputState { state, seed } = bundle;
    let history = state
        .history()
        .iter()
        .map(|entry| {
            Ok(WireInputStateHistoryEntry {
                timestamp: entry.timestamp.to_rfc3339(),
                from: to_wire_input_lifecycle_state(entry.from)?,
                to: to_wire_input_lifecycle_state(entry.to)?,
                reason: entry.reason.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(WireInputState {
        input_id: state.input_id.to_string(),
        current_state: to_wire_input_lifecycle_state(seed.phase)?,
        policy: None,
        terminal_outcome: seed
            .terminal_outcome
            .map(to_wire_input_terminal_outcome)
            .transpose()?,
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
    use meerkat_runtime::input_state::{InputState, InputStateSeed, InputTerminalOutcome};
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
}
