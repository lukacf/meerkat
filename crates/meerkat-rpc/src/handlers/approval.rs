//! `approval/*` JSON-RPC handlers.

use std::sync::Arc;

use meerkat_contracts::{
    ApprovalDecideParams, ApprovalGetParams, ApprovalListParams, ApprovalListResult,
    ApprovalRequestParams,
};
use meerkat_core::ApprovalError;

use crate::error;
use crate::handlers::{RpcResponseExt, parse_params};
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

fn approval_error_to_response(id: Option<RpcId>, error_value: ApprovalError) -> RpcResponse {
    let code = match error_value {
        ApprovalError::NotFound { .. } => error::INVALID_PARAMS,
        ApprovalError::Store(_) => error::INTERNAL_ERROR,
        ApprovalError::AlreadyDecided { .. }
        | ApprovalError::Expired { .. }
        | ApprovalError::InvalidDecision { .. }
        | ApprovalError::EmptyAllowedDecisions
        | ApprovalError::InvalidPrincipal
        | ApprovalError::InvalidMetadata(_) => error::INVALID_PARAMS,
    };
    RpcResponse::error(id, code, error_value.to_string())
}

pub async fn handle_request(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: ApprovalRequestParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    match runtime.approval_service().request(params.request) {
        Ok(record) => RpcResponse::success(id, record),
        Err(error_value) => approval_error_to_response(id, error_value),
    }
}

pub async fn handle_list(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: ApprovalListParams = match params {
        Some(_) => match parse_params(params) {
            Ok(params) => params,
            Err(response) => return response.with_id(id),
        },
        None => ApprovalListParams::default(),
    };
    match runtime.approval_service().list(params.filter) {
        Ok(approvals) => RpcResponse::success(id, ApprovalListResult { approvals }),
        Err(error_value) => approval_error_to_response(id, error_value),
    }
}

pub async fn handle_get(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: ApprovalGetParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    match runtime.approval_service().get(&params.approval_id) {
        Ok(record) => RpcResponse::success(id, record),
        Err(error_value) => approval_error_to_response(id, error_value),
    }
}

pub async fn handle_decide(
    id: Option<RpcId>,
    params: Option<&serde_json::value::RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: ApprovalDecideParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    match runtime.approval_service().decide(
        &params.approval_id,
        params.decision,
        params.actor,
        params.reason,
        params.provenance,
    ) {
        Ok(record) => RpcResponse::success(id, record),
        Err(error_value) => approval_error_to_response(id, error_value),
    }
}
