//! v9 runtime RPC handlers — runtime/state, runtime/accept, runtime/retire, runtime/reset,
//! input/state, input/list.

use serde_json::value::RawValue;

use meerkat_contracts::{
    InputListParams, InputListResult, InputStateParams, RuntimeAcceptOutcomeType,
    RuntimeAcceptParams, RuntimeAcceptResult, RuntimeResetParams, RuntimeResetResult,
    RuntimeRetireParams, RuntimeRetireResult, RuntimeStateParams, RuntimeStateResult,
    WireInputLifecycleState, WireInputState, WireInputStateHistoryEntry, WireRuntimeState,
};
use meerkat_core::{InputId, SessionId};
use meerkat_runtime::RuntimeState;
use meerkat_runtime::service_ext::SessionServiceRuntimeExt;

use super::{RpcResponseExt, parse_params};
use crate::protocol::{RpcId, RpcResponse};

fn to_wire_runtime_state(state: RuntimeState) -> Result<WireRuntimeState, String> {
    Ok(match state {
        RuntimeState::Initializing => WireRuntimeState::Initializing,
        RuntimeState::Idle => WireRuntimeState::Idle,
        RuntimeState::Attached => WireRuntimeState::Attached,
        RuntimeState::Running => WireRuntimeState::Running,
        RuntimeState::Recovering => WireRuntimeState::Recovering,
        RuntimeState::Retired => WireRuntimeState::Retired,
        RuntimeState::Stopped => WireRuntimeState::Stopped,
        RuntimeState::Destroyed => WireRuntimeState::Destroyed,
        _ => return Err("unsupported runtime state variant".to_string()),
    })
}

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

fn to_wire_input_state(state: meerkat_runtime::InputState) -> Result<WireInputState, String> {
    let json = serde_json::to_value(&state).map_err(|err| err.to_string())?;
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
        current_state: to_wire_input_lifecycle_state(state.current_state())?,
        policy: json.get("policy").cloned().filter(|value| !value.is_null()),
        terminal_outcome: json
            .get("terminal_outcome")
            .cloned()
            .filter(|value| !value.is_null()),
        durability: json
            .get("durability")
            .cloned()
            .filter(|value| !value.is_null()),
        idempotency_key: json
            .get("idempotency_key")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        attempt_count: state.attempt_count,
        recovery_count: state.recovery_count,
        history,
        reconstruction_source: json
            .get("reconstruction_source")
            .cloned()
            .filter(|value| !value.is_null()),
        persisted_input: json
            .get("persisted_input")
            .cloned()
            .filter(|value| !value.is_null()),
        last_run_id: state.last_run_id().map(ToString::to_string),
        last_boundary_sequence: state.last_boundary_sequence(),
        created_at: state.created_at.to_rfc3339(),
        updated_at: state.updated_at().to_rfc3339(),
    })
}

fn to_wire_accept_result(
    outcome: meerkat_runtime::AcceptOutcome,
) -> Result<RuntimeAcceptResult, String> {
    Ok(match outcome {
        meerkat_runtime::AcceptOutcome::Accepted {
            input_id,
            policy,
            state,
        } => RuntimeAcceptResult {
            outcome_type: RuntimeAcceptOutcomeType::Accepted,
            input_id: Some(input_id.to_string()),
            existing_id: None,
            reason: None,
            policy: Some(serde_json::to_value(policy).map_err(|err| err.to_string())?),
            state: Some(to_wire_input_state(state)?),
        },
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
            reason: Some(reason),
            policy: None,
            state: None,
        },
        _ => return Err("unsupported accept outcome variant".to_string()),
    })
}

// ---- Handlers ----

/// Handle `runtime/state` — get the runtime state for a session.
pub async fn handle_runtime_state(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
) -> RpcResponse {
    let params: RuntimeStateParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    let session_id = match SessionId::parse(&params.session_id) {
        Ok(id) => id,
        Err(_) => {
            return RpcResponse::error(id, crate::error::INVALID_PARAMS, "Invalid session ID");
        }
    };

    match adapter.runtime_state(&session_id).await {
        Ok(state) => match to_wire_runtime_state(state) {
            Ok(state) => RpcResponse::success(id, RuntimeStateResult { state }),
            Err(err) => RpcResponse::error(id, crate::error::INTERNAL_ERROR, err),
        },
        Err(e) => RpcResponse::error(id, crate::error::INVALID_PARAMS, e.to_string()),
    }
}

/// Handle `runtime/accept` — accept an input for a session.
pub async fn handle_runtime_accept(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
) -> RpcResponse {
    let params: RuntimeAcceptParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    let session_id = match SessionId::parse(&params.session_id) {
        Ok(id) => id,
        Err(_) => {
            return RpcResponse::error(id, crate::error::INVALID_PARAMS, "Invalid session ID");
        }
    };

    let input = match serde_json::from_value::<meerkat_runtime::Input>(params.input) {
        Ok(input) => input,
        Err(err) => {
            return RpcResponse::error(id, crate::error::INVALID_PARAMS, err.to_string());
        }
    };

    match adapter.accept_input(&session_id, input).await {
        Ok(outcome) => match to_wire_accept_result(outcome) {
            Ok(result) => RpcResponse::success(id, result),
            Err(err) => RpcResponse::error(id, crate::error::INTERNAL_ERROR, err),
        },
        Err(meerkat_runtime::RuntimeDriverError::NotReady { state }) => {
            match to_wire_accept_result(meerkat_runtime::AcceptOutcome::Rejected {
                reason: format!("runtime not accepting input while in state: {state}"),
            }) {
                Ok(result) => RpcResponse::success(id, result),
                Err(err) => RpcResponse::error(id, crate::error::INTERNAL_ERROR, err),
            }
        }
        Err(e) => RpcResponse::error(id, crate::error::INVALID_PARAMS, e.to_string()),
    }
}

/// Handle `runtime/retire` — retire a session's runtime.
pub async fn handle_runtime_retire(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
) -> RpcResponse {
    let params: RuntimeRetireParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    let session_id = match SessionId::parse(&params.session_id) {
        Ok(id) => id,
        Err(_) => {
            return RpcResponse::error(id, crate::error::INVALID_PARAMS, "Invalid session ID");
        }
    };

    match adapter.retire_runtime(&session_id).await {
        Ok(report) => RpcResponse::success(
            id,
            RuntimeRetireResult {
                inputs_abandoned: report.inputs_abandoned,
                inputs_pending_drain: report.inputs_pending_drain,
            },
        ),
        Err(e) => RpcResponse::error(id, crate::error::INVALID_PARAMS, e.to_string()),
    }
}

/// Handle `runtime/reset` — reset a session's runtime.
pub async fn handle_runtime_reset(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
) -> RpcResponse {
    let params: RuntimeResetParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    let session_id = match SessionId::parse(&params.session_id) {
        Ok(id) => id,
        Err(_) => {
            return RpcResponse::error(id, crate::error::INVALID_PARAMS, "Invalid session ID");
        }
    };

    match adapter.reset_runtime(&session_id).await {
        Ok(report) => RpcResponse::success(
            id,
            RuntimeResetResult {
                inputs_abandoned: report.inputs_abandoned,
            },
        ),
        Err(e) => RpcResponse::error(id, crate::error::INVALID_PARAMS, e.to_string()),
    }
}

/// Handle `input/state` — get the state of a specific input.
pub async fn handle_input_state(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
) -> RpcResponse {
    let params: InputStateParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    let session_id = match SessionId::parse(&params.session_id) {
        Ok(id) => id,
        Err(_) => {
            return RpcResponse::error(id, crate::error::INVALID_PARAMS, "Invalid session ID");
        }
    };

    let input_id = match uuid::Uuid::parse_str(&params.input_id) {
        Ok(uuid) => InputId::from_uuid(uuid),
        Err(_) => return RpcResponse::error(id, crate::error::INVALID_PARAMS, "Invalid input ID"),
    };

    match adapter.input_state(&session_id, &input_id).await {
        Ok(Some(state)) => match to_wire_input_state(state) {
            Ok(result) => RpcResponse::success(id, result),
            Err(err) => RpcResponse::error(id, crate::error::INTERNAL_ERROR, err),
        },
        Ok(None) => RpcResponse::success(id, Option::<WireInputState>::None),
        Err(e) => RpcResponse::error(id, crate::error::INVALID_PARAMS, e.to_string()),
    }
}

/// Handle `input/list` — list active inputs for a session.
pub async fn handle_input_list(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    adapter: &dyn SessionServiceRuntimeExt,
) -> RpcResponse {
    let params: InputListParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    let session_id = match SessionId::parse(&params.session_id) {
        Ok(id) => id,
        Err(_) => {
            return RpcResponse::error(id, crate::error::INVALID_PARAMS, "Invalid session ID");
        }
    };

    match adapter.list_active_inputs(&session_id).await {
        Ok(input_ids) => RpcResponse::success(
            id,
            InputListResult {
                input_ids: input_ids
                    .into_iter()
                    .map(|input_id| input_id.to_string())
                    .collect(),
            },
        ),
        Err(e) => RpcResponse::error(id, crate::error::INVALID_PARAMS, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_runtime::{
        AcceptOutcome, Input, InputState, PromptInput, ResetReport, RetireReport,
        RuntimeDriverError, RuntimeMode, RuntimeState,
    };
    use serde_json::{json, value::to_raw_value};

    struct RetiredRejectingAdapter;

    #[async_trait]
    impl SessionServiceRuntimeExt for RetiredRejectingAdapter {
        fn runtime_mode(&self) -> RuntimeMode {
            RuntimeMode::V9Compliant
        }

        async fn accept_input(
            &self,
            _session_id: &SessionId,
            _input: Input,
        ) -> Result<AcceptOutcome, RuntimeDriverError> {
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Retired,
            })
        }

        async fn accept_input_with_completion(
            &self,
            _session_id: &SessionId,
            _input: Input,
        ) -> Result<(AcceptOutcome, Option<meerkat_runtime::CompletionHandle>), RuntimeDriverError>
        {
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Retired,
            })
        }

        async fn runtime_state(
            &self,
            _session_id: &SessionId,
        ) -> Result<RuntimeState, RuntimeDriverError> {
            Ok(RuntimeState::Retired)
        }

        async fn retire_runtime(
            &self,
            _session_id: &SessionId,
        ) -> Result<RetireReport, RuntimeDriverError> {
            Ok(RetireReport {
                inputs_abandoned: 0,
                inputs_pending_drain: 0,
            })
        }

        async fn reset_runtime(
            &self,
            _session_id: &SessionId,
        ) -> Result<ResetReport, RuntimeDriverError> {
            Ok(ResetReport {
                inputs_abandoned: 0,
            })
        }

        async fn input_state(
            &self,
            _session_id: &SessionId,
            _input_id: &InputId,
        ) -> Result<Option<InputState>, RuntimeDriverError> {
            Ok(None)
        }

        async fn list_active_inputs(
            &self,
            _session_id: &SessionId,
        ) -> Result<Vec<InputId>, RuntimeDriverError> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn runtime_accept_returns_rejected_result_when_runtime_is_retired() {
        let params_result = to_raw_value(&json!({
            "session_id": uuid::Uuid::new_v4().to_string(),
            "input": Input::Prompt(PromptInput::new("hello", None)),
        }));
        assert!(params_result.is_ok(), "serialize params should succeed");
        let Some(params) = params_result.ok() else {
            return;
        };

        let response = handle_runtime_accept(
            Some(RpcId::Num(1)),
            Some(params.as_ref()),
            &RetiredRejectingAdapter,
        )
        .await;

        assert!(
            response.error.is_none(),
            "retired accept should not surface as RPC error"
        );
        assert!(
            response.result.is_some(),
            "result payload should be present"
        );
        let Some(result) = response.result else {
            return;
        };
        let value_result: Result<serde_json::Value, _> = serde_json::from_str(result.get());
        assert!(value_result.is_ok(), "json result parse should succeed");
        let Some(value) = value_result.ok() else {
            return;
        };
        assert_eq!(value["outcome_type"], "rejected");
        assert!(
            value["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("retired"),
            "rejection reason should mention retired state"
        );
    }
}
