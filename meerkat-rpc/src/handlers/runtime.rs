//! v9 runtime RPC handlers — runtime/state, runtime/accept, runtime/retire, runtime/reset,
//! input/state, input/list.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use meerkat_core::{InputId, SessionId};
use meerkat_runtime::RuntimeState;
use meerkat_runtime::service_ext::SessionServiceRuntimeExt;

use super::{RpcResponseExt, parse_params};
use crate::protocol::{RpcId, RpcResponse};

// ---- Param types ----

#[derive(Debug, Deserialize)]
struct RuntimeStateParams {
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct RuntimeAcceptParams {
    session_id: String,
    input: meerkat_runtime::Input,
}

#[derive(Debug, Deserialize)]
struct RuntimeRetireParams {
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct RuntimeResetParams {
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct InputStateParams {
    session_id: String,
    input_id: String,
}

#[derive(Debug, Deserialize)]
struct InputListParams {
    session_id: String,
}

// ---- Result types ----

#[derive(Debug, Serialize)]
struct RuntimeStateResult {
    state: RuntimeState,
}

#[derive(Debug, Serialize)]
struct InputListResult {
    input_ids: Vec<InputId>,
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
        Ok(state) => RpcResponse::success(id, RuntimeStateResult { state }),
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

    match adapter.accept_input(&session_id, params.input).await {
        Ok(outcome) => RpcResponse::success(id, outcome),
        Err(meerkat_runtime::RuntimeDriverError::NotReady { state }) => RpcResponse::success(
            id,
            meerkat_runtime::AcceptOutcome::Rejected {
                reason: format!("runtime not accepting input while in state: {state}"),
            },
        ),
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
        Ok(report) => RpcResponse::success(id, report),
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
        Ok(report) => RpcResponse::success(id, report),
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
        Ok(state) => RpcResponse::success(id, state),
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
        Ok(input_ids) => RpcResponse::success(id, InputListResult { input_ids }),
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
