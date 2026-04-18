//! `schedule/*` method handlers.

use std::sync::Arc;

use meerkat::{
    CreateScheduleRequest, ScheduleDomainError, ScheduleId, ScheduleService, ScheduleStoreError,
    handle_schedule_tools_call, schedule_tools_list,
};
use meerkat_contracts::{
    ListSchedulesParams, ScheduleIdParams, ScheduleListResult, ScheduleOccurrencesParams,
    ScheduleOccurrencesResult, UpdateScheduleParams,
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
    let params: ListSchedulesParams = match params {
        Some(raw) => match serde_json::from_str(raw.get()) {
            Ok(params) => params,
            Err(error) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("invalid params: {error}"),
                );
            }
        },
        None => ListSchedulesParams::default(),
    };

    match schedule_service(&runtime).list().await {
        Ok(mut schedules) => {
            if let Some(labels) = params.labels {
                schedules.retain(|schedule| {
                    labels
                        .iter()
                        .all(|(key, value)| schedule.config.labels.get(key) == Some(value))
                });
            }

            let offset = params.offset.unwrap_or(0);
            if offset > 0 {
                schedules = schedules.into_iter().skip(offset).collect();
            }
            if let Some(limit) = params.limit {
                schedules.truncate(limit);
            }

            RpcResponse::success(id, ScheduleListResult { schedules })
        }
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

    if let Err(error) = runtime.ensure_schedule_host_started().await {
        return map_schedule_error(id, error);
    }

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
    let params: ScheduleOccurrencesParams = match parse_params(params) {
        Ok(params) => params,
        Err(response) => return response.with_id(id),
    };
    let schedule_id = match parse_schedule_id(id.clone(), &params.schedule_id) {
        Ok(schedule_id) => schedule_id,
        Err(response) => return *response,
    };

    match schedule_service(&runtime)
        .list_occurrences_filtered(&schedule_id, params.include_terminal.unwrap_or(true))
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
#[allow(clippy::unwrap_used)]
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

    fn missing_target_schedule_request() -> Result<CreateScheduleRequest, serde_json::Error> {
        serde_json::from_value(missing_target_schedule_tool_args())
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

    fn schedule_id_raw(
        schedule_id: &ScheduleId,
    ) -> Result<Box<serde_json::value::RawValue>, serde_json::Error> {
        serde_json::value::RawValue::from_string(
            json!({ "schedule_id": schedule_id.to_string() }).to_string(),
        )
    }

    fn schedule_list_raw(
        value: Value,
    ) -> Result<Box<serde_json::value::RawValue>, serde_json::Error> {
        serde_json::value::RawValue::from_string(value.to_string())
    }

    fn schedule_occurrences_raw(
        schedule_id: &ScheduleId,
        include_terminal: bool,
    ) -> Result<Box<serde_json::value::RawValue>, serde_json::Error> {
        serde_json::value::RawValue::from_string(
            json!({
                "schedule_id": schedule_id.to_string(),
                "include_terminal": include_terminal
            })
            .to_string(),
        )
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

    #[tokio::test]
    async fn schedule_get_unknown_id_returns_schedule_not_found_code() {
        let temp_result = TempDir::new();
        assert!(temp_result.is_ok(), "temp dir: {temp_result:?}");
        let Ok(temp) = temp_result else {
            return;
        };
        let runtime = test_runtime(&temp);
        let raw_result = schedule_id_raw(&ScheduleId::new());
        assert!(raw_result.is_ok(), "raw value: {raw_result:?}");
        let Ok(raw) = raw_result else {
            return;
        };

        let response = handle_get(Some(RpcId::Num(1)), Some(raw.as_ref()), runtime.clone()).await;
        assert!(response.error.is_some(), "unknown schedule should error");
        let Some(error) = response.error else {
            return;
        };
        assert_eq!(error.code, crate::error::SCHEDULE_NOT_FOUND);
    }

    #[tokio::test]
    async fn schedule_pause_and_resume_round_trip() {
        let temp_result = TempDir::new();
        assert!(temp_result.is_ok(), "temp dir: {temp_result:?}");
        let Ok(temp) = temp_result else {
            return;
        };
        let runtime = test_runtime(&temp);
        let request_result = missing_target_schedule_request();
        assert!(
            request_result.is_ok(),
            "valid schedule request: {request_result:?}"
        );
        let Ok(request) = request_result else {
            return;
        };
        let schedule_result = runtime.schedule_service().create(request).await;
        assert!(
            schedule_result.is_ok(),
            "create schedule: {schedule_result:?}"
        );
        let Ok(schedule) = schedule_result else {
            return;
        };
        let raw_result = schedule_id_raw(&schedule.schedule_id);
        assert!(raw_result.is_ok(), "raw value: {raw_result:?}");
        let Ok(raw) = raw_result else {
            return;
        };

        let paused_response =
            handle_pause(Some(RpcId::Num(1)), Some(raw.as_ref()), runtime.clone()).await;
        assert!(
            paused_response.error.is_none(),
            "pause should succeed: {:?}",
            paused_response.error
        );
        let paused_result = runtime.schedule_service().get(&schedule.schedule_id).await;
        assert!(paused_result.is_ok(), "paused schedule: {paused_result:?}");
        let Ok(paused) = paused_result else {
            return;
        };
        assert_eq!(paused.phase, meerkat::SchedulePhase::Paused);

        let raw_result = schedule_id_raw(&schedule.schedule_id);
        assert!(raw_result.is_ok(), "raw value: {raw_result:?}");
        let Ok(raw) = raw_result else {
            return;
        };
        let resumed_response =
            handle_resume(Some(RpcId::Num(1)), Some(raw.as_ref()), runtime.clone()).await;
        assert!(
            resumed_response.error.is_none(),
            "resume should succeed: {:?}",
            resumed_response.error
        );
        let resumed_result = runtime.schedule_service().get(&schedule.schedule_id).await;
        assert!(
            resumed_result.is_ok(),
            "resumed schedule: {resumed_result:?}"
        );
        let Ok(resumed) = resumed_result else {
            return;
        };
        assert_eq!(resumed.phase, meerkat::SchedulePhase::Active);
    }

    #[tokio::test]
    async fn schedule_list_supports_label_filter_and_pagination() {
        let temp_result = TempDir::new();
        assert!(temp_result.is_ok(), "temp dir: {temp_result:?}");
        let Ok(temp) = temp_result else {
            return;
        };
        let runtime = test_runtime(&temp);

        let prod_result = missing_target_schedule_request();
        assert!(prod_result.is_ok(), "valid schedule: {prod_result:?}");
        let Ok(mut prod) = prod_result else {
            return;
        };
        prod.name = Some("prod".to_string());
        prod.labels.insert("env".to_string(), "prod".to_string());
        let create_prod_result = runtime.schedule_service().create(prod).await;
        assert!(
            create_prod_result.is_ok(),
            "create prod schedule: {create_prod_result:?}"
        );

        let dev_result = missing_target_schedule_request();
        assert!(dev_result.is_ok(), "valid schedule: {dev_result:?}");
        let Ok(mut dev) = dev_result else {
            return;
        };
        dev.name = Some("dev".to_string());
        dev.labels.insert("env".to_string(), "dev".to_string());
        let create_dev_result = runtime.schedule_service().create(dev).await;
        assert!(
            create_dev_result.is_ok(),
            "create dev schedule: {create_dev_result:?}"
        );

        let raw_result = schedule_list_raw(json!({
            "labels": { "env": "prod" },
            "limit": 1,
            "offset": 0
        }));
        assert!(raw_result.is_ok(), "raw list params: {raw_result:?}");
        let Ok(raw) = raw_result else {
            return;
        };

        let response = handle_list(Some(RpcId::Num(1)), Some(raw.as_ref()), runtime.clone()).await;
        assert!(response.error.is_none(), "schedule/list should succeed");
        assert!(
            response.result.is_some(),
            "schedule/list result should exist"
        );
        let Some(result) = response.result else {
            return;
        };
        let value_result: Result<Value, _> = serde_json::from_str(result.get());
        assert!(value_result.is_ok(), "valid json: {value_result:?}");
        let Ok(value) = value_result else {
            return;
        };
        let schedules_opt = value["schedules"].as_array();
        assert!(schedules_opt.is_some(), "schedules array should exist");
        let Some(schedules) = schedules_opt else {
            return;
        };
        assert_eq!(schedules.len(), 1);
        assert_eq!(schedules[0]["name"], "prod");
    }

    #[tokio::test]
    async fn schedule_occurrences_honors_include_terminal_flag() {
        let temp_result = TempDir::new();
        assert!(temp_result.is_ok(), "temp dir: {temp_result:?}");
        let Ok(temp) = temp_result else {
            return;
        };
        let runtime = test_runtime(&temp);
        let request_result = missing_target_schedule_request();
        assert!(
            request_result.is_ok(),
            "valid schedule request: {request_result:?}"
        );
        let Ok(request) = request_result else {
            return;
        };
        let schedule_result = runtime.schedule_service().create(request).await;
        assert!(
            schedule_result.is_ok(),
            "create schedule: {schedule_result:?}"
        );
        let Ok(schedule) = schedule_result else {
            return;
        };
        let start_host_result = runtime.ensure_schedule_host_started().await;
        assert!(
            start_host_result.is_ok(),
            "start schedule host: {start_host_result:?}"
        );

        let occurrence = wait_for_missing_target_misfire(&runtime, &schedule.schedule_id).await;
        assert!(
            occurrence.is_some(),
            "expected terminal occurrence to be created"
        );

        let exclude_raw_result = schedule_occurrences_raw(&schedule.schedule_id, false);
        assert!(
            exclude_raw_result.is_ok(),
            "occurrence params: {exclude_raw_result:?}"
        );
        let Ok(exclude_raw) = exclude_raw_result else {
            return;
        };
        let exclude_response = handle_occurrences(
            Some(RpcId::Num(1)),
            Some(exclude_raw.as_ref()),
            runtime.clone(),
        )
        .await;
        assert!(
            exclude_response.error.is_none(),
            "schedule/occurrences exclude-terminal should succeed"
        );
        assert!(
            exclude_response.result.is_some(),
            "exclude result should exist"
        );
        let Some(exclude_result) = exclude_response.result else {
            return;
        };
        let exclude_value_result: Result<Value, _> = serde_json::from_str(exclude_result.get());
        assert!(
            exclude_value_result.is_ok(),
            "valid exclude json: {exclude_value_result:?}"
        );
        let Ok(exclude_value) = exclude_value_result else {
            return;
        };
        let exclude_occurrences = exclude_value["occurrences"].as_array();
        assert!(
            exclude_occurrences.is_some(),
            "exclude occurrences array should exist"
        );
        let Some(exclude_occurrences) = exclude_occurrences else {
            return;
        };
        assert_eq!(exclude_occurrences.len(), 0);

        let include_raw_result = schedule_occurrences_raw(&schedule.schedule_id, true);
        assert!(
            include_raw_result.is_ok(),
            "occurrence params: {include_raw_result:?}"
        );
        let Ok(include_raw) = include_raw_result else {
            return;
        };
        let include_response =
            handle_occurrences(Some(RpcId::Num(1)), Some(include_raw.as_ref()), runtime).await;
        assert!(
            include_response.error.is_none(),
            "schedule/occurrences include-terminal should succeed"
        );
        assert!(
            include_response.result.is_some(),
            "include result should exist"
        );
        let Some(include_result) = include_response.result else {
            return;
        };
        let include_value_result: Result<Value, _> = serde_json::from_str(include_result.get());
        assert!(
            include_value_result.is_ok(),
            "valid include json: {include_value_result:?}"
        );
        let Ok(include_value) = include_value_result else {
            return;
        };
        let include_occurrences = include_value["occurrences"].as_array();
        assert!(
            include_occurrences.is_some(),
            "include occurrences array should exist"
        );
        let Some(include_occurrences) = include_occurrences else {
            return;
        };
        assert_eq!(include_occurrences.len(), 1);
    }

    #[tokio::test]
    async fn schedule_delete_marks_schedule_deleted() {
        let temp_result = TempDir::new();
        assert!(temp_result.is_ok(), "temp dir: {temp_result:?}");
        let Ok(temp) = temp_result else {
            return;
        };
        let runtime = test_runtime(&temp);
        let request_result = missing_target_schedule_request();
        assert!(
            request_result.is_ok(),
            "valid schedule request: {request_result:?}"
        );
        let Ok(request) = request_result else {
            return;
        };
        let schedule_result = runtime.schedule_service().create(request).await;
        assert!(
            schedule_result.is_ok(),
            "create schedule: {schedule_result:?}"
        );
        let Ok(schedule) = schedule_result else {
            return;
        };
        let raw_result = schedule_id_raw(&schedule.schedule_id);
        assert!(raw_result.is_ok(), "raw value: {raw_result:?}");
        let Ok(raw) = raw_result else {
            return;
        };

        let response =
            handle_delete(Some(RpcId::Num(1)), Some(raw.as_ref()), runtime.clone()).await;
        assert!(
            response.error.is_none(),
            "delete should succeed: {:?}",
            response.error
        );

        let deleted_result = runtime.schedule_service().get(&schedule.schedule_id).await;
        assert!(
            deleted_result.is_ok(),
            "deleted schedule: {deleted_result:?}"
        );
        let Ok(deleted) = deleted_result else {
            return;
        };
        assert_eq!(deleted.phase, meerkat::SchedulePhase::Deleted);
        assert!(
            deleted.revision > schedule.revision,
            "delete should advance revision"
        );
    }
}
