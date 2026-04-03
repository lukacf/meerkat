//! `schedule/*` method handlers.

use std::sync::Arc;

use meerkat::{
    CreateScheduleRequest, Occurrence, Schedule, ScheduleDomainError, ScheduleId, ScheduleService,
    ScheduleStoreError, UpdateScheduleRequest, handle_schedule_tools_call, schedule_tools_list,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

#[derive(Debug, Default, Deserialize)]
pub struct ListSchedulesParams {}

#[derive(Debug, Deserialize)]
pub struct ScheduleIdParams {
    pub schedule_id: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateScheduleParams {
    pub schedule_id: String,
    #[serde(flatten)]
    pub update: UpdateScheduleRequest,
}

#[derive(Debug, Serialize)]
pub struct ScheduleListResult {
    pub schedules: Vec<Schedule>,
}

#[derive(Debug, Serialize)]
pub struct ScheduleOccurrencesResult {
    pub occurrences: Vec<Occurrence>,
}

#[derive(Debug, Serialize)]
pub struct ScheduleToolsResult {
    pub tools: Vec<Value>,
}

#[derive(Debug, Deserialize)]
pub struct ScheduleToolCallParams {
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

fn parse_schedule_id(id: Option<RpcId>, raw: &str) -> Result<ScheduleId, RpcResponse> {
    ScheduleId::parse(raw).map_err(|error| {
        RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("invalid schedule_id '{raw}': {error}"),
        )
    })
}

fn map_schedule_error(id: Option<RpcId>, error: ScheduleDomainError) -> RpcResponse {
    match error {
        ScheduleDomainError::Store(ScheduleStoreError::ScheduleNotFound { .. }) => {
            RpcResponse::error(id, error::SCHEDULE_NOT_FOUND, "schedule not found")
        }
        ScheduleDomainError::Store(ScheduleStoreError::UnsupportedBackend { .. }) => {
            RpcResponse::error(
                id,
                meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code(),
                error.to_string(),
            )
        }
        ScheduleDomainError::InvalidSchedule(_)
        | ScheduleDomainError::InvalidTrigger(_)
        | ScheduleDomainError::InvalidCron(_) => {
            RpcResponse::error(id, error::INVALID_PARAMS, error.to_string())
        }
        other => RpcResponse::error(id, error::INTERNAL_ERROR, other.to_string()),
    }
}

fn schedule_service(runtime: &SessionRuntime) -> ScheduleService {
    runtime.schedule_service()
}

pub async fn handle_create(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let request: CreateScheduleRequest = match parse_params(params) {
        Ok(request) => request,
        Err(response) => return response.with_id(id),
    };

    match schedule_service(&runtime).create(request).await {
        Ok(schedule) => RpcResponse::success(id, schedule),
        Err(error) => map_schedule_error(id, error),
    }
}

pub async fn handle_get(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: ScheduleIdParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let schedule_id = match parse_schedule_id(id.clone(), &params.schedule_id) {
        Ok(schedule_id) => schedule_id,
        Err(response) => return response,
    };

    match schedule_service(&runtime).get(&schedule_id).await {
        Ok(schedule) => RpcResponse::success(id, schedule),
        Err(error) => map_schedule_error(id, error),
    }
}

pub async fn handle_list(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    if let Some(raw) = params {
        let _: ListSchedulesParams = match serde_json::from_str(raw.get()) {
            Ok(params) => params,
            Err(error) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("invalid params: {error}"),
                );
            }
        };
    }

    match schedule_service(&runtime).list().await {
        Ok(schedules) => RpcResponse::success(id, ScheduleListResult { schedules }),
        Err(error) => map_schedule_error(id, error),
    }
}

pub async fn handle_update(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: UpdateScheduleParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let schedule_id = match parse_schedule_id(id.clone(), &params.schedule_id) {
        Ok(schedule_id) => schedule_id,
        Err(response) => return response,
    };

    match schedule_service(&runtime)
        .update(&schedule_id, params.update)
        .await
    {
        Ok(schedule) => RpcResponse::success(id, schedule),
        Err(error) => map_schedule_error(id, error),
    }
}

pub async fn handle_pause(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: ScheduleIdParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let schedule_id = match parse_schedule_id(id.clone(), &params.schedule_id) {
        Ok(schedule_id) => schedule_id,
        Err(response) => return response,
    };

    match schedule_service(&runtime).pause(&schedule_id).await {
        Ok(schedule) => RpcResponse::success(id, schedule),
        Err(error) => map_schedule_error(id, error),
    }
}

pub async fn handle_resume(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: ScheduleIdParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let schedule_id = match parse_schedule_id(id.clone(), &params.schedule_id) {
        Ok(schedule_id) => schedule_id,
        Err(response) => return response,
    };

    match schedule_service(&runtime).resume(&schedule_id).await {
        Ok(schedule) => RpcResponse::success(id, schedule),
        Err(error) => map_schedule_error(id, error),
    }
}

pub async fn handle_delete(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: ScheduleIdParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let schedule_id = match parse_schedule_id(id.clone(), &params.schedule_id) {
        Ok(schedule_id) => schedule_id,
        Err(response) => return response,
    };

    match schedule_service(&runtime).delete(&schedule_id).await {
        Ok(schedule) => RpcResponse::success(id, schedule),
        Err(error) => map_schedule_error(id, error),
    }
}

pub async fn handle_occurrences(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: ScheduleIdParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let schedule_id = match parse_schedule_id(id.clone(), &params.schedule_id) {
        Ok(schedule_id) => schedule_id,
        Err(response) => return response,
    };

    match schedule_service(&runtime)
        .list_occurrences(&schedule_id)
        .await
    {
        Ok(occurrences) => RpcResponse::success(id, ScheduleOccurrencesResult { occurrences }),
        Err(error) => map_schedule_error(id, error),
    }
}

pub async fn handle_tools(id: Option<RpcId>) -> RpcResponse {
    RpcResponse::success(
        id,
        ScheduleToolsResult {
            tools: schedule_tools_list(),
        },
    )
}

pub async fn handle_call(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: Arc<SessionRuntime>,
) -> RpcResponse {
    let params: ScheduleToolCallParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };

    match handle_schedule_tools_call(&schedule_service(&runtime), &params.name, &params.arguments)
        .await
    {
        Ok(value) => RpcResponse::success(id, value),
        Err(error) => RpcResponse::error(id, error.code, error.message),
    }
}
