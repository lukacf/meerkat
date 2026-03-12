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
