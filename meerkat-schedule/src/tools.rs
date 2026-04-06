use crate::{
    CreateScheduleRequest, Occurrence, ScheduleDomainError, ScheduleId, ScheduleService,
    ScheduleStoreError, UpdateScheduleRequest,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const INVALID_ARGUMENTS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32000;
pub const NOT_FOUND: i32 = -32004;
pub const CAPABILITY_UNAVAILABLE: i32 = -32001;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduleToolError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ScheduleToolError {
    fn invalid_arguments(message: impl Into<String>) -> Self {
        Self {
            code: INVALID_ARGUMENTS,
            message: message.into(),
            data: None,
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            code: INTERNAL_ERROR,
            message: message.into(),
            data: None,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct EmptyArgs {}

#[derive(Debug, Deserialize)]
struct ScheduleIdArgs {
    schedule_id: String,
}

#[derive(Debug, Deserialize)]
struct UpdateScheduleArgs {
    schedule_id: String,
    #[serde(flatten)]
    update: UpdateScheduleRequest,
}

pub fn schedule_tools_list() -> Vec<Value> {
    vec![
        tool_descriptor(
            "meerkat_schedule_create",
            "Create a realm-scoped schedule from typed trigger and target data.",
            create_schedule_schema(),
        ),
        tool_descriptor(
            "meerkat_schedule_get",
            "Fetch one persisted schedule by schedule_id.",
            schedule_id_schema("The schedule_id to fetch."),
        ),
        tool_descriptor(
            "meerkat_schedule_list",
            "List persisted schedules in the active realm.",
            empty_schema(),
        ),
        tool_descriptor(
            "meerkat_schedule_update",
            "Update a persisted schedule by schedule_id.",
            update_schedule_schema(),
        ),
        tool_descriptor(
            "meerkat_schedule_pause",
            "Pause a persisted schedule by schedule_id.",
            schedule_id_schema("The schedule_id to pause."),
        ),
        tool_descriptor(
            "meerkat_schedule_resume",
            "Resume a paused schedule by schedule_id.",
            schedule_id_schema("The schedule_id to resume."),
        ),
        tool_descriptor(
            "meerkat_schedule_delete",
            "Delete a schedule by schedule_id while preserving history.",
            schedule_id_schema("The schedule_id to delete."),
        ),
        tool_descriptor(
            "meerkat_schedule_occurrences",
            "List persisted occurrences for one schedule_id.",
            schedule_id_schema("The schedule_id whose occurrences should be listed."),
        ),
    ]
}

pub async fn handle_schedule_tools_call(
    service: &ScheduleService,
    name: &str,
    arguments: &Value,
) -> Result<Value, ScheduleToolError> {
    match name {
        "meerkat_schedule_create" => {
            let request: CreateScheduleRequest = parse_args(name, arguments)?;
            let schedule = service.create(request).await.map_err(map_schedule_error)?;
            encode(name, schedule)
        }
        "meerkat_schedule_get" => {
            let args: ScheduleIdArgs = parse_args(name, arguments)?;
            let schedule_id = parse_schedule_id(&args.schedule_id)?;
            let schedule = service
                .get(&schedule_id)
                .await
                .map_err(map_schedule_error)?;
            encode(name, schedule)
        }
        "meerkat_schedule_list" => {
            let _: EmptyArgs = parse_args(name, arguments)?;
            let schedules = service.list().await.map_err(map_schedule_error)?;
            encode(name, json!({ "schedules": schedules }))
        }
        "meerkat_schedule_update" => {
            let args: UpdateScheduleArgs = parse_args(name, arguments)?;
            let schedule_id = parse_schedule_id(&args.schedule_id)?;
            let schedule = service
                .update(&schedule_id, args.update)
                .await
                .map_err(map_schedule_error)?;
            encode(name, schedule)
        }
        "meerkat_schedule_pause" => {
            let args: ScheduleIdArgs = parse_args(name, arguments)?;
            let schedule_id = parse_schedule_id(&args.schedule_id)?;
            let schedule = service
                .pause(&schedule_id)
                .await
                .map_err(map_schedule_error)?;
            encode(name, schedule)
        }
        "meerkat_schedule_resume" => {
            let args: ScheduleIdArgs = parse_args(name, arguments)?;
            let schedule_id = parse_schedule_id(&args.schedule_id)?;
            let schedule = service
                .resume(&schedule_id)
                .await
                .map_err(map_schedule_error)?;
            encode(name, schedule)
        }
        "meerkat_schedule_delete" => {
            let args: ScheduleIdArgs = parse_args(name, arguments)?;
            let schedule_id = parse_schedule_id(&args.schedule_id)?;
            let schedule = service
                .delete(&schedule_id)
                .await
                .map_err(map_schedule_error)?;
            encode(name, schedule)
        }
        "meerkat_schedule_occurrences" => {
            let args: ScheduleIdArgs = parse_args(name, arguments)?;
            let schedule_id = parse_schedule_id(&args.schedule_id)?;
            let occurrences = service
                .list_occurrences(&schedule_id)
                .await
                .map_err(map_schedule_error)?;
            encode_occurrences(occurrences)
        }
        other => Err(ScheduleToolError::invalid_arguments(format!(
            "unknown schedule tool: {other}"
        ))),
    }
}

fn parse_args<T>(name: &str, arguments: &Value) -> Result<T, ScheduleToolError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(arguments.clone()).map_err(|error| {
        ScheduleToolError::invalid_arguments(format!("invalid arguments for {name}: {error}"))
    })
}

fn parse_schedule_id(raw: &str) -> Result<ScheduleId, ScheduleToolError> {
    ScheduleId::parse(raw).map_err(|error| {
        ScheduleToolError::invalid_arguments(format!("invalid schedule_id: {error}"))
    })
}

fn encode(name: &str, value: impl Serialize) -> Result<Value, ScheduleToolError> {
    serde_json::to_value(value).map_err(|error| {
        ScheduleToolError::internal(format!("failed to encode {name} result: {error}"))
    })
}

fn encode_occurrences(occurrences: Vec<Occurrence>) -> Result<Value, ScheduleToolError> {
    encode(
        "meerkat_schedule_occurrences",
        json!({ "occurrences": occurrences }),
    )
}

fn map_schedule_error(error: ScheduleDomainError) -> ScheduleToolError {
    match error {
        ScheduleDomainError::Store(ScheduleStoreError::ScheduleNotFound { .. }) => {
            ScheduleToolError {
                code: NOT_FOUND,
                message: "schedule not found".into(),
                data: None,
            }
        }
        ScheduleDomainError::Store(ScheduleStoreError::UnsupportedBackend { .. }) => {
            ScheduleToolError {
                code: CAPABILITY_UNAVAILABLE,
                message: error.to_string(),
                data: None,
            }
        }
        ScheduleDomainError::InvalidSchedule(_)
        | ScheduleDomainError::InvalidTrigger(_)
        | ScheduleDomainError::InvalidCron(_) => ScheduleToolError {
            code: INVALID_ARGUMENTS,
            message: error.to_string(),
            data: None,
        },
        other => ScheduleToolError {
            code: INTERNAL_ERROR,
            message: other.to_string(),
            data: None,
        },
    }
}

fn tool_descriptor(name: &'static str, description: &'static str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
    })
}

fn empty_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false,
    })
}

fn schedule_id_schema(description: &'static str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "schedule_id": {
                "type": "string",
                "description": description,
            }
        },
        "required": ["schedule_id"],
        "additionalProperties": false,
    })
}

fn create_schedule_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "description": { "type": "string" },
            "trigger": {
                "type": "object",
                "description": "Typed trigger specification; canonical truth is TriggerSpec, not a cron string."
            },
            "target": {
                "type": "object",
                "description": "Typed session or mob target binding."
            },
            "misfire_policy": {
                "description": "Misfire policy value; e.g. skip or catch_up_within."
            },
            "overlap_policy": {
                "description": "Overlap policy value; e.g. allow_concurrent or skip_if_running."
            },
            "missing_target_policy": {
                "description": "Missing target policy value; e.g. skip or mark_misfired."
            },
            "labels": {
                "type": "object",
                "additionalProperties": { "type": "string" }
            },
            "planning_horizon_days": {
                "type": "integer",
                "minimum": 1
            },
            "planning_horizon_occurrences": {
                "type": "integer",
                "minimum": 1
            }
        },
        "required": ["trigger", "target", "misfire_policy", "overlap_policy", "missing_target_policy"],
        "additionalProperties": false,
    })
}

fn update_schedule_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "schedule_id": {
                "type": "string",
                "description": "The persisted schedule_id to update."
            },
            "name": { "type": "string" },
            "description": { "type": "string" },
            "trigger": {
                "type": "object",
                "description": "Updated typed trigger specification."
            },
            "target": {
                "type": "object",
                "description": "Updated typed session or mob target binding."
            },
            "misfire_policy": { "description": "Updated misfire policy." },
            "overlap_policy": { "description": "Updated overlap policy." },
            "missing_target_policy": { "description": "Updated missing-target policy." },
            "labels": {
                "type": "object",
                "additionalProperties": { "type": "string" }
            },
            "planning_horizon_days": {
                "type": "integer",
                "minimum": 1
            },
            "planning_horizon_occurrences": {
                "type": "integer",
                "minimum": 1
            }
        },
        "required": ["schedule_id"],
        "additionalProperties": false,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{
        IntervalTriggerSpec, MemoryScheduleStore, MisfirePolicy, MissingTargetPolicy,
        OverlapPolicy, ScheduledSessionAction, SessionTargetBinding, TargetBinding, TriggerSpec,
    };
    use chrono::{Duration, Utc};
    use meerkat_core::{ContentInput, SessionId};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn schedule_request() -> CreateScheduleRequest {
        CreateScheduleRequest {
            name: Some("heartbeat".into()),
            description: Some("tool surface schedule".into()),
            trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                start_at_utc: Utc::now() + Duration::minutes(1),
                every_seconds: 60,
                end_at_utc: None,
            }),
            target: TargetBinding::session(SessionTargetBinding::ExactSession {
                session_id: SessionId::new(),
                action: ScheduledSessionAction::Prompt {
                    prompt: ContentInput::from("tool surface"),
                    system_prompt: None,
                    render_metadata: None,
                    skill_references: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            }),
            misfire_policy: MisfirePolicy::Skip,
            overlap_policy: OverlapPolicy::SkipIfRunning,
            missing_target_policy: MissingTargetPolicy::MarkMisfired,
            labels: BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(2),
        }
    }

    #[tokio::test]
    async fn schedule_tools_create_and_list_round_trip() -> Result<(), String> {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let request =
            serde_json::to_value(schedule_request()).map_err(|error| error.to_string())?;
        let created = handle_schedule_tools_call(&service, "meerkat_schedule_create", &request)
            .await
            .map_err(|error| format!("{error:?}"))?;
        let schedule_id = created["schedule_id"]
            .as_str()
            .ok_or_else(|| "create should return schedule_id".to_string())?;

        let listed = handle_schedule_tools_call(&service, "meerkat_schedule_list", &json!({}))
            .await
            .map_err(|error| format!("{error:?}"))?;
        assert_eq!(
            listed["schedules"][0]["schedule_id"].as_str(),
            Some(schedule_id)
        );

        let occurrences = handle_schedule_tools_call(
            &service,
            "meerkat_schedule_occurrences",
            &json!({ "schedule_id": schedule_id }),
        )
        .await
        .map_err(|error| format!("{error:?}"))?;
        assert!(
            occurrences["occurrences"]
                .as_array()
                .map(|rows| !rows.is_empty())
                .unwrap_or(false),
            "planning should persist occurrences"
        );
        Ok(())
    }
}
