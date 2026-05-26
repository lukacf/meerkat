//! `workgraph/*` method handlers.

use std::sync::Arc;

use meerkat::{
    AttentionBindingRequest, AttentionListRequest, AttentionPauseRequest, GoalConfirmRequest,
    GoalCreateRequest, GoalRequestCloseRequest, GoalStatusRequest, ReadyWorkFilter, WorkGraphError,
    WorkGraphEventFilter, WorkGraphSnapshotFilter, WorkItemFilter, WorkItemId, WorkNamespace,
};
use serde::Deserialize;
use serde_json::value::RawValue;

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

#[derive(Debug, Deserialize)]
pub struct WorkGraphIdParams {
    pub id: WorkItemId,
    #[serde(default)]
    pub realm_id: Option<String>,
    #[serde(default)]
    pub namespace: Option<WorkNamespace>,
}

fn map_workgraph_error(id: Option<RpcId>, error: WorkGraphError) -> RpcResponse {
    match error {
        WorkGraphError::NotFound { .. } => {
            RpcResponse::error(id, error::INVALID_PARAMS, error.to_string())
        }
        WorkGraphError::StaleRevision { .. }
        | WorkGraphError::Conflict(_)
        | WorkGraphError::InvalidTransition(_) => {
            RpcResponse::error(id, error::INVALID_PARAMS, error.to_string())
        }
        WorkGraphError::InvalidInput(_) => {
            RpcResponse::error(id, error::INVALID_PARAMS, error.to_string())
        }
        WorkGraphError::UnsupportedBackend(_) => RpcResponse::error(
            id,
            meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code(),
            error.to_string(),
        ),
        WorkGraphError::Store(_) => {
            RpcResponse::error(id, error::INTERNAL_ERROR, error.to_string())
        }
    }
}

pub async fn handle_get(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: WorkGraphIdParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    match runtime
        .workgraph_service()
        .get(params.realm_id, params.namespace, params.id)
        .await
    {
        Ok(item) => RpcResponse::success(id, item),
        Err(error) => map_workgraph_error(id, error),
    }
}

pub async fn handle_goal_create(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let request: GoalCreateRequest = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    if request.completion_policy.requires_trusted_principal() {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            "principal-gated WorkGraph goal policies require trusted in-process principal authority and are not available on the public RPC workgraph/goal/create surface",
        );
    }
    match runtime.workgraph_service().create_goal(request).await {
        Ok(result) => RpcResponse::success(id, result),
        Err(error) => map_workgraph_error(id, error),
    }
}

pub async fn handle_goal_status(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let request: GoalStatusRequest = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    match runtime.workgraph_service().goal_status(request).await {
        Ok(result) => RpcResponse::success(id, result),
        Err(error) => map_workgraph_error(id, error),
    }
}

pub async fn handle_goal_confirm(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let request: GoalConfirmRequest = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    match runtime.workgraph_service().goal_confirm(request).await {
        Ok(result) => RpcResponse::success(id, result),
        Err(error) => map_workgraph_error(id, error),
    }
}

pub async fn handle_goal_request_close(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let request: GoalRequestCloseRequest = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    match runtime
        .workgraph_service()
        .goal_request_close(request)
        .await
    {
        Ok(result) => RpcResponse::success(id, result),
        Err(error) => map_workgraph_error(id, error),
    }
}

pub async fn handle_attention_list(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let request: AttentionListRequest = match params {
        Some(raw) => match serde_json::from_str(raw.get()) {
            Ok(filter) => filter,
            Err(error) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("invalid params: {error}"),
                );
            }
        },
        None => AttentionListRequest::default(),
    };
    match runtime.workgraph_service().list_attention(request).await {
        Ok(result) => RpcResponse::success(id, result),
        Err(error) => map_workgraph_error(id, error),
    }
}

pub async fn handle_attention_pause(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let request: AttentionPauseRequest = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    match runtime.workgraph_service().pause_attention(request).await {
        Ok(result) => RpcResponse::success(id, result),
        Err(error) => map_workgraph_error(id, error),
    }
}

pub async fn handle_attention_resume(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let request: AttentionBindingRequest = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    match runtime.workgraph_service().resume_attention(request).await {
        Ok(result) => RpcResponse::success(id, result),
        Err(error) => map_workgraph_error(id, error),
    }
}

pub async fn handle_attention_continue(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let request: AttentionBindingRequest = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    match runtime
        .enqueue_workgraph_attention_binding_continuation(request)
        .await
        .map(attention_continue_result)
    {
        Ok(result) => RpcResponse::success(id, result),
        Err(error) => RpcResponse::error(id, error.code, error.message),
    }
}

fn attention_continue_result(
    outcome: meerkat_runtime::AcceptOutcome,
) -> meerkat::AttentionContinueResult {
    match outcome {
        meerkat_runtime::AcceptOutcome::Accepted { input_id, .. } => {
            meerkat::AttentionContinueResult {
                outcome: meerkat::AttentionContinueOutcome::Accepted,
                input_id: Some(input_id.to_string()),
                existing_id: None,
                reason: None,
            }
        }
        meerkat_runtime::AcceptOutcome::Deduplicated {
            input_id,
            existing_id,
        } => meerkat::AttentionContinueResult {
            outcome: meerkat::AttentionContinueOutcome::Deduplicated,
            input_id: Some(input_id.to_string()),
            existing_id: Some(existing_id.to_string()),
            reason: None,
        },
        meerkat_runtime::AcceptOutcome::Rejected { reason } => meerkat::AttentionContinueResult {
            outcome: meerkat::AttentionContinueOutcome::Rejected,
            input_id: None,
            existing_id: None,
            reason: Some(reason.to_string()),
        },
        _ => meerkat::AttentionContinueResult {
            outcome: meerkat::AttentionContinueOutcome::Rejected,
            input_id: None,
            existing_id: None,
            reason: Some("unsupported accept outcome variant".to_string()),
        },
    }
}

pub async fn handle_list(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let filter: WorkItemFilter = match params {
        Some(raw) => match serde_json::from_str(raw.get()) {
            Ok(filter) => filter,
            Err(error) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("invalid params: {error}"),
                );
            }
        },
        None => WorkItemFilter::default(),
    };
    match runtime.workgraph_service().list(filter).await {
        Ok(items) => RpcResponse::success(id, serde_json::json!({ "items": items })),
        Err(error) => map_workgraph_error(id, error),
    }
}

pub async fn handle_ready(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let filter: ReadyWorkFilter = match params {
        Some(raw) => match serde_json::from_str(raw.get()) {
            Ok(filter) => filter,
            Err(error) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("invalid params: {error}"),
                );
            }
        },
        None => ReadyWorkFilter::default(),
    };
    match runtime.workgraph_service().ready(filter).await {
        Ok(items) => RpcResponse::success(id, serde_json::json!({ "items": items })),
        Err(error) => map_workgraph_error(id, error),
    }
}

pub async fn handle_snapshot(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let filter: WorkGraphSnapshotFilter = match params {
        Some(raw) => match serde_json::from_str(raw.get()) {
            Ok(filter) => filter,
            Err(error) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("invalid params: {error}"),
                );
            }
        },
        None => WorkGraphSnapshotFilter::default(),
    };
    match runtime.workgraph_service().snapshot(filter).await {
        Ok(snapshot) => RpcResponse::success(id, snapshot),
        Err(error) => map_workgraph_error(id, error),
    }
}

pub async fn handle_events(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let filter: WorkGraphEventFilter = match params {
        Some(raw) => match serde_json::from_str(raw.get()) {
            Ok(filter) => filter,
            Err(error) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("invalid params: {error}"),
                );
            }
        },
        None => WorkGraphEventFilter::default(),
    };
    match runtime.workgraph_service().events(filter).await {
        Ok(events) => RpcResponse::success(id, serde_json::json!({ "events": events })),
        Err(error) => map_workgraph_error(id, error),
    }
}
