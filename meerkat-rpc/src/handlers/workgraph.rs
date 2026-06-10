//! `workgraph/*` method handlers.

use std::sync::Arc;

use meerkat::{
    AttentionListRequest, GoalStatusRequest, ReadyWorkFilter, WorkGraphError, WorkGraphEventFilter,
    WorkGraphIdParams, WorkGraphPublicErrorClass, WorkGraphSnapshotFilter, WorkItemFilter,
};
use serde_json::value::RawValue;

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

fn map_workgraph_error(id: Option<RpcId>, error: WorkGraphError) -> RpcResponse {
    let message = error.to_string();
    match meerkat::WorkGraphMachine::public_error_class(&error) {
        Ok(public_class) => map_workgraph_public_error_class(id, public_class, message),
        Err(classification_error) => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!(
                "generated WorkGraph error classification failed: {classification_error}; original error: {message}"
            ),
        ),
    }
}

fn map_workgraph_public_error_class(
    id: Option<RpcId>,
    public_class: WorkGraphPublicErrorClass,
    message: String,
) -> RpcResponse {
    match public_class {
        WorkGraphPublicErrorClass::NotFound
        | WorkGraphPublicErrorClass::Conflict
        | WorkGraphPublicErrorClass::InvalidTransition
        | WorkGraphPublicErrorClass::InvalidArguments => {
            RpcResponse::error(id, error::INVALID_PARAMS, message)
        }
        WorkGraphPublicErrorClass::CapabilityUnavailable => RpcResponse::error(
            id,
            meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code(),
            message,
        ),
        WorkGraphPublicErrorClass::StoreError => {
            RpcResponse::error(id, error::INTERNAL_ERROR, message)
        }
    }
}

/// Resolve the realm-scoped WorkGraph service or surface the typed
/// no-realm-identity fault (never an invented "default" scope).
#[allow(clippy::result_large_err)]
fn workgraph_service_or_error(
    id: &Option<RpcId>,
    runtime: &SessionRuntime,
) -> Result<meerkat::WorkGraphService, RpcResponse> {
    runtime
        .workgraph_service()
        .map_err(|err| RpcResponse::error(id.clone(), err.code, err.message))
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
    let service = match workgraph_service_or_error(&id, &runtime) {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .get(params.realm_id, params.namespace, params.id)
        .await
    {
        Ok(item) => RpcResponse::success(id, item),
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
    let service = match workgraph_service_or_error(&id, &runtime) {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.goal_status(request).await {
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
    let service = match workgraph_service_or_error(&id, &runtime) {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.list_attention(request).await {
        Ok(result) => RpcResponse::success(id, result),
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
    let service = match workgraph_service_or_error(&id, &runtime) {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.list(filter).await {
        Ok(items) => work_items_response(id, &items),
        Err(error) => map_workgraph_error(id, error),
    }
}

/// Serialize a typed work-item list into the contracts `WorkItemsResult`
/// wire payload, failing closed on serialization (K17).
fn work_items_response<T: serde::Serialize>(id: Option<RpcId>, items: &[T]) -> RpcResponse {
    let items: Result<Vec<serde_json::Value>, serde_json::Error> =
        items.iter().map(serde_json::to_value).collect();
    match items {
        Ok(items) => RpcResponse::success(id, meerkat_contracts::WorkItemsResult { items }),
        Err(err) => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("failed to serialize workgraph items: {err}"),
        ),
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
    let service = match workgraph_service_or_error(&id, &runtime) {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.ready(filter).await {
        Ok(items) => work_items_response(id, &items),
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
    let service = match workgraph_service_or_error(&id, &runtime) {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.snapshot(filter).await {
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
    let service = match workgraph_service_or_error(&id, &runtime) {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.events(filter).await {
        Ok(events) => {
            let events: Result<Vec<serde_json::Value>, serde_json::Error> =
                events.iter().map(serde_json::to_value).collect();
            match events {
                Ok(events) => {
                    RpcResponse::success(id, meerkat_contracts::WorkEventsResult { events })
                }
                Err(err) => RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("failed to serialize workgraph events: {err}"),
                ),
            }
        }
        Err(error) => map_workgraph_error(id, error),
    }
}
