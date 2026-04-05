//! `schedule/*` method handlers.

use std::sync::Arc;

use meerkat::{
    CreateScheduleRequest, ScheduleDomainError, ScheduleId, ScheduleService, ScheduleStoreError,
    handle_schedule_tools_call, schedule_tools_list,
};
use meerkat_contracts::{
    ListSchedulesParams, ScheduleIdParams, ScheduleListResult, ScheduleOccurrencesResult,
    UpdateScheduleParams,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

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

fn parse_schedule_id(id: Option<RpcId>, raw: &str) -> Result<ScheduleId, Box<RpcResponse>> {
    ScheduleId::parse(raw).map_err(|error| {
        Box::new(RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!("invalid schedule_id '{raw}': {error}"),
        ))
    })
}

fn map_schedule_error(id: Option<RpcId>, error: ScheduleDomainError) -> RpcResponse {
    match error {
        ScheduleDomainError::Store(ScheduleStoreError::ScheduleNotFound { .. }) => {
            RpcResponse::error(id, error::SESSION_NOT_FOUND, "schedule not found")
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

    if let Err(error) = runtime.ensure_schedule_host_started().await {
        return map_schedule_error(id, error);
    }

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
        Err(response) => return *response,
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
        Err(response) => return *response,
    };

    if let Err(error) = runtime.ensure_schedule_host_started().await {
        return map_schedule_error(id, error);
    }

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
        Err(response) => return *response,
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
        Err(response) => return *response,
    };

    if let Err(error) = runtime.ensure_schedule_host_started().await {
        return map_schedule_error(id, error);
    }

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
        Err(response) => return *response,
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
        Err(response) => return *response,
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

    if let Err(error) = runtime.ensure_schedule_host_started().await {
        return map_schedule_error(id, error);
    }

    match handle_schedule_tools_call(&schedule_service(&runtime), &params.name, &params.arguments)
        .await
    {
        Ok(value) => RpcResponse::success(id, value),
        Err(error) => RpcResponse::error(id, error.code, error.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::{Duration, Utc};
    use meerkat::{
        AgentFactory, MemoryScheduleStore, MemoryStore, OccurrenceFailureClass, OccurrencePhase,
        PersistenceBundle, SessionStore,
    };
    use meerkat_core::{Config, SessionId};
    use serde_json::json;
    use tempfile::TempDir;

    fn memory_blob_store() -> Arc<dyn meerkat_core::BlobStore> {
        Arc::new(meerkat_store::MemoryBlobStore::new())
    }

    fn missing_target_schedule_tool_args() -> Value {
        json!({
            "name": "missing-target",
            "description": "create a due schedule through the tool surface",
            "trigger": {
                "type": "once",
                "due_at_utc": (Utc::now() - Duration::seconds(1)).to_rfc3339(),
            },
            "target": {
                "target_kind": "session",
                "type": "exact_session",
                "session_id": SessionId::new(),
                "action": {
                    "type": "prompt",
                    "prompt": "scheduled hello"
                }
            },
            "missing_target_policy": "mark_misfired",
            "planning_horizon_days": 1,
            "planning_horizon_occurrences": 1
        })
    }

    fn test_runtime(temp: &TempDir) -> Arc<SessionRuntime> {
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        Arc::new(SessionRuntime::new(
            factory,
            Config::default(),
            10,
            PersistenceBundle::new_with_schedule_store(
                store,
                None,
                memory_blob_store(),
                Arc::new(MemoryScheduleStore::new()),
            ),
            crate::router::NotificationSink::noop(),
        ))
    }

    async fn wait_for_missing_target_misfire(
        runtime: &Arc<SessionRuntime>,
        schedule_id: &ScheduleId,
    ) -> Option<meerkat::Occurrence> {
        for _ in 0..40 {
            let occurrences_result = runtime
                .schedule_service()
                .list_occurrences(schedule_id)
                .await;
            assert!(
                occurrences_result.is_ok(),
                "list occurrences: {occurrences_result:?}"
            );
            let Ok(occurrences) = occurrences_result else {
                return None;
            };
            if let Some(occurrence) = occurrences.into_iter().find(|occurrence| {
                occurrence.phase == OccurrencePhase::Misfired
                    && occurrence.failure_class == Some(OccurrenceFailureClass::TargetMissing)
            }) {
                return Some(occurrence);
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        None
    }

    #[tokio::test]
    async fn schedule_call_starts_host_and_services_due_schedule() {
        let temp_result = TempDir::new();
        assert!(temp_result.is_ok(), "temp dir: {temp_result:?}");
        let Ok(temp) = temp_result else {
            return;
        };
        let runtime = test_runtime(&temp);
        let serialized_result = serde_json::to_string(&json!({
            "name": "meerkat_schedule_create",
            "arguments": missing_target_schedule_tool_args(),
        }));
        assert!(
            serialized_result.is_ok(),
            "serialize params: {serialized_result:?}"
        );
        let Ok(serialized) = serialized_result else {
            return;
        };
        let params_result = serde_json::value::RawValue::from_string(serialized);
        assert!(params_result.is_ok(), "raw value: {params_result:?}");
        let Ok(params) = params_result else {
            return;
        };

        let response =
            handle_call(Some(RpcId::Num(1)), Some(params.as_ref()), runtime.clone()).await;
        assert!(
            response.error.is_none(),
            "schedule/call should succeed: {:?}",
            response.error
        );

        assert!(
            response.result.is_some(),
            "schedule/call result should exist"
        );
        let Some(result) = response.result.as_ref() else {
            return;
        };
        let created_result: Result<Value, _> = serde_json::from_str(result.get());
        assert!(
            created_result.is_ok(),
            "valid JSON result: {created_result:?}"
        );
        let Ok(created) = created_result else {
            return;
        };
        let schedule_id_str = created["schedule_id"].as_str();
        assert!(
            schedule_id_str.is_some(),
            "schedule_id should be returned: {created:?}"
        );
        let Some(schedule_id_str) = schedule_id_str else {
            return;
        };
        let schedule_id_result = ScheduleId::parse(schedule_id_str);
        assert!(
            schedule_id_result.is_ok(),
            "valid schedule id: {schedule_id_result:?}"
        );
        let Ok(schedule_id) = schedule_id_result else {
            return;
        };

        let occurrence = wait_for_missing_target_misfire(&runtime, &schedule_id).await;
        assert!(
            occurrence.is_some(),
            "schedule/call should start the host and service due work"
        );
        let Some(occurrence) = occurrence else {
            return;
        };
        assert_eq!(occurrence.phase, OccurrencePhase::Misfired);
    }
}
