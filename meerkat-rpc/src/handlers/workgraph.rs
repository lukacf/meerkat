//! `workgraph/*` method handlers.

use std::sync::Arc;

use meerkat::{
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
