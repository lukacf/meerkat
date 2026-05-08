//! Runtime-backed session support handlers.

use serde_json::value::RawValue;

use meerkat_contracts::{
    RuntimeAcceptOutcomeType, RuntimeAcceptResult, RuntimeRealtimeAttachmentStatusParams,
    RuntimeRealtimeAttachmentStatusResult, WireInputLifecycleState, WireInputState,
    WireInputStateHistoryEntry, WireRealtimeAttachmentStatus, wire::runtime::WireInputPolicy,
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
        terminal_outcome: None,
        durability: None,
        idempotency_key: None,
        attempt_count: state.attempt_count(),
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
        } => {
            // Re-bundle the returned shell with a queue-seeded seed for the
            // wire payload. accept_input transitions the DSL to Queued for
            // durable/queued inputs; callers that need exact phase/run_id
            // should use input_state lookups post-accept.
            let bundle = meerkat_runtime::input_state::StoredInputState {
                state,
                seed: meerkat_runtime::input_state::InputStateSeed {
                    phase: meerkat_runtime::InputLifecycleState::Queued,
                    last_run_id: None,
                    last_boundary_sequence: None,
                    terminal_outcome: None,
                    attempt_count: 0,
                },
            };
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
