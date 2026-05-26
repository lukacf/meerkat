//! `workgraph/*` method handlers.

use std::sync::Arc;

use meerkat::{
    AttentionBindingRequest, AttentionListRequest, AttentionPauseRequest, AttentionResumeRequest,
    GoalConfirmRequest, GoalStatusRequest, PublicGoalCreateRequest, PublicGoalRequestCloseRequest,
    ReadyWorkFilter, WorkGraphError, WorkGraphEventFilter, WorkGraphSnapshotFilter, WorkItemFilter,
    WorkItemId, WorkNamespace,
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

fn trusted_workgraph_goal_mutation_required(id: Option<RpcId>) -> RpcResponse {
    RpcResponse::error(
        id,
        error::INVALID_PARAMS,
        "WorkGraph goal and attention mutations require trusted in-process host/session authority; the public JSON-RPC surface is fail-closed for these operations",
    )
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
    _runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let _request: PublicGoalCreateRequest = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    trusted_workgraph_goal_mutation_required(id)
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
    _runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let _request: GoalConfirmRequest = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    trusted_workgraph_goal_mutation_required(id)
}

pub async fn handle_goal_request_close(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    _runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let _request: PublicGoalRequestCloseRequest = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    trusted_workgraph_goal_mutation_required(id)
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
    _runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let _request: AttentionPauseRequest = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    trusted_workgraph_goal_mutation_required(id)
}

pub async fn handle_attention_resume(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    _runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let _request: AttentionResumeRequest = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    trusted_workgraph_goal_mutation_required(id)
}

pub async fn handle_attention_continue(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    _runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let _request: AttentionBindingRequest = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    trusted_workgraph_goal_mutation_required(id)
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
