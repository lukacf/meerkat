//! `workgraph/*` method handlers.

use std::sync::Arc;

use meerkat::{
    AttentionListRequest, GoalStatusRequest, ReadyWorkFilter, WorkGraphError, WorkGraphEventFilter,
    WorkGraphIdParams, WorkGraphPublicErrorClass, WorkGraphSnapshotFilter, WorkItemFilter,
};
use serde_json::value::RawValue;

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse, bounded_collection_limit};
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
        .map_err(|err| RpcResponse::from_error(id.clone(), err))
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
    let mut filter: WorkItemFilter = match params {
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
    filter.limit = match bounded_collection_limit(filter.limit) {
        Ok(limit) => Some(limit),
        Err(message) => return RpcResponse::error(id, error::INVALID_PARAMS, message),
    };
    let service = match workgraph_service_or_error(&id, &runtime) {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.list_page(filter).await {
        Ok(page) => work_items_response(id, &page),
        Err(error) => map_workgraph_error(id, error),
    }
}

/// Serialize a typed work-item page into the contracts `WorkItemsResult`
/// wire payload, failing closed on serialization (K17).
fn work_items_response(id: Option<RpcId>, page: &meerkat::WorkGraphItemsResponse) -> RpcResponse {
    #[derive(serde::Serialize)]
    struct WorkItemsResultRef<'a> {
        items: &'a [meerkat::WorkItem],
        #[serde(skip_serializing_if = "Option::is_none")]
        total_items: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        truncated: Option<bool>,
    }

    RpcResponse::success(
        id,
        WorkItemsResultRef {
            items: &page.items,
            total_items: page.total_items,
            truncated: page.truncated,
        },
    )
}

pub async fn handle_ready(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let mut filter: ReadyWorkFilter = match params {
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
    filter.limit = match bounded_collection_limit(filter.limit) {
        Ok(limit) => Some(limit),
        Err(message) => return RpcResponse::error(id, error::INVALID_PARAMS, message),
    };
    let service = match workgraph_service_or_error(&id, &runtime) {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.ready_page(filter).await {
        Ok(page) => work_items_response(id, &page),
        Err(error) => map_workgraph_error(id, error),
    }
}

pub async fn handle_snapshot(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let mut filter: WorkGraphSnapshotFilter = match params {
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
    filter.limit = match bounded_collection_limit(filter.limit) {
        Ok(limit) => Some(limit),
        Err(message) => return RpcResponse::error(id, error::INVALID_PARAMS, message),
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
    let mut filter: WorkGraphEventFilter = match params {
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
    filter.limit = match bounded_collection_limit(filter.limit) {
        Ok(limit) => Some(limit),
        Err(message) => return RpcResponse::error(id, error::INVALID_PARAMS, message),
    };
    let service = match workgraph_service_or_error(&id, &runtime) {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.events(filter).await {
        Ok(events) => {
            #[derive(serde::Serialize)]
            struct WorkEventsResultRef<'a, T> {
                events: &'a [T],
            }
            RpcResponse::success(id, WorkEventsResultRef { events: &events })
        }
        Err(error) => map_workgraph_error(id, error),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::sync::Arc;

    use serde_json::{Value, json};

    use super::work_items_response;
    use crate::protocol::{RpcId, RpcResponse};

    fn result_json(response: &RpcResponse) -> Value {
        let raw = response.result.as_ref().expect("result");
        serde_json::from_str(raw.get()).expect("result json")
    }

    /// `workgraph/list` and `workgraph/ready` answer through
    /// `work_items_response`: the rows, then the page's `total_items` and
    /// `truncated`, each left out when the page carries none.
    #[tokio::test]
    async fn work_items_response_carries_the_page_fields() {
        let service =
            meerkat::WorkGraphService::new(Arc::new(meerkat::MemoryWorkGraphStore::new()));
        for title in ["first", "second"] {
            service
                .create(meerkat::CreateWorkItemRequest {
                    title: title.to_string(),
                    ..Default::default()
                })
                .await
                .expect("create");
        }
        let page = service
            .list_page(meerkat::WorkItemFilter {
                limit: Some(1),
                ..Default::default()
            })
            .await
            .expect("list page");

        let result = result_json(&work_items_response(Some(RpcId::Num(1)), &page));
        assert_eq!(
            result["items"].as_array().map(Vec::len),
            Some(1),
            "{result}"
        );
        assert_eq!(result["total_items"], json!(2), "{result}");
        assert_eq!(result["truncated"], json!(true), "{result}");

        let bare = meerkat::WorkGraphItemsResponse {
            items: page.items,
            total_items: None,
            truncated: None,
        };
        let result = result_json(&work_items_response(Some(RpcId::Num(2)), &bare));
        let keys = result.as_object().expect("result object");
        assert!(keys.contains_key("items"), "{result}");
        assert!(!keys.contains_key("total_items"), "{result}");
        assert!(!keys.contains_key("truncated"), "{result}");
    }
}
