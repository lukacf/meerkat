use crate::{
    CreateScheduleRequest, Occurrence, ScheduleDomainError, ScheduleId, ScheduleService,
    ScheduleStoreError, UpdateScheduleRequest,
};
use async_trait::async_trait;
use meerkat_core::error::ToolError;
use meerkat_core::types::{ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind};
use meerkat_core::{AgentToolDispatcher, ToolDispatchOutcome};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

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
            "Create a realm-scoped schedule. Use an explicit TriggerSpec and TargetBinding; the input schema includes the exact JSON shapes for once, interval, calendar, session, and mob schedules. For follow-ups in an existing long-lived session such as TUX, use target_kind=session with type=exact_session or resumable_session and a real session_id. If you do not have a session_id, use materialize_on_demand_session instead.",
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
            "Update a persisted schedule by schedule_id. Any provided trigger or target must use the same typed shapes as meerkat_schedule_create.",
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

pub struct ScheduleToolDispatcher {
    service: ScheduleService,
    tool_defs: Arc<[Arc<ToolDef>]>,
}

impl ScheduleToolDispatcher {
    pub fn new(service: ScheduleService) -> Self {
        let tool_defs: Arc<[Arc<ToolDef>]> = schedule_tools_list()
            .into_iter()
            .map(|tool| {
                Arc::new(ToolDef {
                    name: tool["name"].as_str().unwrap_or_default().to_string(),
                    description: tool["description"].as_str().unwrap_or_default().to_string(),
                    input_schema: tool["inputSchema"].clone(),
                    provenance: Some(ToolProvenance {
                        kind: ToolSourceKind::Schedule,
                        source_id: "schedule".into(),
                    }),
                })
            })
            .collect::<Vec<_>>()
            .into();
        Self { service, tool_defs }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for ScheduleToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tool_defs)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if !self.tool_defs.iter().any(|tool| tool.name == call.name) {
            return Err(ToolError::not_found(call.name));
        }

        let arguments: Value = serde_json::from_str(call.args.get())
            .unwrap_or_else(|_| Value::String(call.args.get().to_string()));
        let result = handle_schedule_tools_call(&self.service, call.name, &arguments)
            .await
            .map_err(|error| map_schedule_tool_dispatch_error(call.name, error))?;

        Ok(ToolResult::new(call.id.to_string(), result.to_string(), false).into())
    }
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

fn map_schedule_tool_dispatch_error(name: &str, error: ScheduleToolError) -> ToolError {
    if error.code == INVALID_ARGUMENTS {
        return ToolError::invalid_arguments(name, error.message);
    }
    ToolError::ExecutionFailed {
        message: format!("{name}: {}", error.message),
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
            "trigger": trigger_spec_schema(),
            "target": target_binding_schema(),
            "misfire_policy": misfire_policy_schema(),
            "overlap_policy": overlap_policy_schema(),
            "missing_target_policy": missing_target_policy_schema(),
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
            "trigger": trigger_spec_schema(),
            "target": target_binding_schema(),
            "misfire_policy": misfire_policy_schema(),
            "overlap_policy": overlap_policy_schema(),
            "missing_target_policy": missing_target_policy_schema(),
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

fn date_time_schema(description: &'static str) -> Value {
    json!({
        "type": "string",
        "format": "date-time",
        "description": description,
    })
}

fn trigger_spec_schema() -> Value {
    json!({
        "description": "TriggerSpec uses internally tagged JSON with a type field. Example once trigger: {\"type\":\"once\",\"due_at_utc\":\"2026-04-09T12:00:00Z\"}. Example interval trigger: {\"type\":\"interval\",\"start_at_utc\":\"2026-04-09T12:00:00Z\",\"every_seconds\":60}.",
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "type": { "const": "once" },
                    "due_at_utc": date_time_schema("Deliver once at this UTC timestamp.")
                },
                "required": ["type", "due_at_utc"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "interval" },
                    "start_at_utc": date_time_schema("First due time in UTC."),
                    "every_seconds": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Repeat cadence in seconds."
                    },
                    "end_at_utc": date_time_schema("Optional final due time in UTC.")
                },
                "required": ["type", "start_at_utc", "every_seconds"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "calendar" },
                    "timezone": {
                        "type": "string",
                        "description": "IANA timezone such as Europe/Stockholm or UTC."
                    },
                    "minute": calendar_field_schema("Minute values."),
                    "hour": calendar_field_schema("Hour values, 0-23."),
                    "day_of_month": calendar_field_schema("Day-of-month values, 1-31."),
                    "month": calendar_field_schema("Month values, 1-12."),
                    "day_of_week": calendar_field_schema("Weekday values using the scheduler's cron-style domain: 0-6 and weekday names such as MON or FRI."),
                    "year": calendar_field_schema("Optional year filter.")
                },
                "required": ["type", "timezone"],
                "additionalProperties": false
            }
        ]
    })
}

fn calendar_field_schema(description: &'static str) -> Value {
    json!({
        "description": description,
        "oneOf": [
            {
                "type": "object",
                "properties": { "kind": { "const": "any" } },
                "required": ["kind"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "kind": { "const": "values" },
                    "values": {
                        "type": "array",
                        "items": { "type": "integer", "minimum": 0 }
                    }
                },
                "required": ["kind", "values"],
                "additionalProperties": false
            }
        ]
    })
}

fn target_binding_schema() -> Value {
    json!({
        "description": "TargetBinding selects a session or mob recipient. For a follow-up in the current TUX session, use target_kind=session with type=exact_session or resumable_session and a real session_id. If you need a fresh scheduled session, use materialize_on_demand_session.",
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "target_kind": { "const": "session" },
                    "type": {
                        "enum": ["exact_session", "resumable_session", "materialize_on_demand_session"]
                    },
                    "session_id": {
                        "type": "string",
                        "description": "Required for exact_session and resumable_session."
                    },
                    "action": scheduled_session_action_schema(),
                    "create": session_materialization_spec_schema(),
                    "bound_session_id": {
                        "type": "string",
                        "description": "Only used by persisted materialize_on_demand_session schedules after the first materialization."
                    }
                },
                "required": ["target_kind", "type", "action"],
                "allOf": [
                    {
                        "if": { "properties": { "type": { "const": "materialize_on_demand_session" } } },
                        "then": { "required": ["create"] }
                    },
                    {
                        "if": {
                            "properties": {
                                "type": { "enum": ["exact_session", "resumable_session"] }
                            }
                        },
                        "then": { "required": ["session_id"] }
                    }
                ],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "target_kind": { "const": "mob" },
                    "type": { "enum": ["member", "flow", "spawn_helper", "fork_helper"] },
                    "mob_id": { "type": "string" },
                    "member_id": { "type": "string" },
                    "source_member_id": { "type": "string" },
                    "flow_id": { "type": "string" },
                    "params": {
                        "description": "Raw JSON value for mob flow parameters."
                    },
                    "prompt": { "type": "string" },
                    "action": scheduled_mob_action_schema(),
                    "fork_context": fork_context_schema(),
                    "options": helper_options_schema()
                },
                "required": ["target_kind", "type", "mob_id"],
                "additionalProperties": false
            }
        ]
    })
}

fn scheduled_session_action_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "type": { "const": "prompt" },
                    "prompt": content_input_schema(),
                    "system_prompt": {
                        "type": "string",
                        "description": "Only supported when materializing a new session."
                    },
                    "render_metadata": { "type": "object" },
                    "skill_references": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Canonical skill references in source_uuid/skill-name form."
                    },
                    "additional_instructions": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                },
                "required": ["type", "prompt"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "event" },
                    "event_type": { "type": "string" },
                    "payload": {},
                    "render_metadata": { "type": "object" }
                },
                "required": ["type", "event_type", "payload"],
                "additionalProperties": false
            }
        ]
    })
}

fn session_materialization_spec_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "model": { "type": "string" },
            "system_prompt": { "type": "string" },
            "max_tokens": { "type": "integer", "minimum": 1 },
            "provider": { "enum": ["anthropic", "openai", "gemini", "other"] },
            "output_schema": { "type": "object" },
            "structured_output_retries": { "type": "integer", "minimum": 0 },
            "provider_params": { "type": "object" },
            "comms_name": { "type": "string" },
            "peer_meta": { "type": "object" },
            "labels": {
                "type": "object",
                "additionalProperties": { "type": "string" }
            },
            "preload_skills": {
                "type": "array",
                "items": { "type": "string" }
            },
            "additional_instructions": {
                "type": "array",
                "items": { "type": "string" }
            },
            "realm_id": { "type": "string" },
            "instance_id": { "type": "string" },
            "backend": { "type": "string" },
            "config_generation": { "type": "integer", "minimum": 0 },
            "keep_alive": { "type": "boolean" },
            "app_context": { "type": "object" }
        },
        "required": ["model"],
        "additionalProperties": false
    })
}

fn scheduled_mob_action_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "type": { "const": "send" },
            "content": content_input_schema(),
            "render_metadata": { "type": "object" }
        },
        "required": ["type", "content"],
        "additionalProperties": false
    })
}

fn helper_options_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "role_name": { "type": "string" },
            "runtime_mode": { "enum": ["autonomous_host", "turn_driven"] },
            "backend": { "enum": ["session", "external"] },
            "tool_access_policy": { "type": "object" }
        },
        "additionalProperties": false
    })
}

fn fork_context_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "properties": { "type": { "const": "full_history" } },
                "required": ["type"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "last_messages" },
                    "count": { "type": "integer", "minimum": 1 }
                },
                "required": ["type", "count"],
                "additionalProperties": false
            }
        ]
    })
}

fn content_input_schema() -> Value {
    json!({
        "description": "Either a plain text string or a list of multimodal content blocks.",
        "oneOf": [
            { "type": "string" },
            {
                "type": "array",
                "items": {
                    "oneOf": [
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "text" },
                                "text": { "type": "string" }
                            },
                            "required": ["type", "text"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "image" },
                                "media_type": { "type": "string" },
                                "source": { "enum": ["inline", "blob"] },
                                "data": { "type": "string" },
                                "blob_id": { "type": "string" }
                            },
                            "required": ["type", "media_type", "source"],
                            "additionalProperties": true
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "video" },
                                "media_type": { "type": "string" },
                                "duration_ms": { "type": "integer", "minimum": 0 },
                                "source": { "const": "inline" },
                                "data": { "type": "string" }
                            },
                            "required": ["type", "media_type", "duration_ms", "source", "data"],
                            "additionalProperties": false
                        }
                    ]
                }
            }
        ]
    })
}

fn misfire_policy_schema() -> Value {
    json!({
        "description": "Misfire policy object. Most schedules should use {\"type\":\"skip\"}.",
        "oneOf": [
            {
                "type": "object",
                "properties": { "type": { "const": "skip" } },
                "required": ["type"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "catch_up_within" },
                    "window_seconds": { "type": "integer", "minimum": 1 }
                },
                "required": ["type", "window_seconds"],
                "additionalProperties": false
            }
        ]
    })
}

fn overlap_policy_schema() -> Value {
    json!({
        "type": "string",
        "enum": ["allow_concurrent", "skip_if_running"],
        "description": "How to handle overlapping occurrences for the same schedule."
    })
}

fn missing_target_policy_schema() -> Value {
    json!({
        "type": "string",
        "enum": ["skip", "mark_misfired"],
        "description": "What to do if the target session or mob is missing when the occurrence becomes due."
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
    use meerkat_core::{AgentToolDispatcher, ToolError};
    use meerkat_core::{ContentInput, SessionId};
    use serde_json::value::RawValue;
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

    fn tool_call<'a>(
        id: &'a str,
        name: &'a str,
        args: &'a RawValue,
    ) -> meerkat_core::ToolCallView<'a> {
        meerkat_core::ToolCallView { id, name, args }
    }

    #[tokio::test]
    async fn schedule_tool_dispatcher_tools_match_tool_list() {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let dispatcher = ScheduleToolDispatcher::new(service);

        let actual: Vec<String> = dispatcher
            .tools()
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        let expected: Vec<String> = schedule_tools_list()
            .into_iter()
            .map(|value| value["name"].as_str().expect("tool name").to_string())
            .collect();

        assert_eq!(actual, expected);
    }

    #[test]
    fn schedule_trigger_schema_matches_internal_tagged_deserializer_shape() {
        let tools = schedule_tools_list();
        let create_schema = &tools
            .iter()
            .find(|tool| tool["name"] == "meerkat_schedule_create")
            .expect("create tool schema must exist")["inputSchema"];
        let trigger_schema = &create_schema["properties"]["trigger"];
        let variants = trigger_schema["oneOf"]
            .as_array()
            .expect("trigger schema variants must be an array");

        assert_eq!(
            trigger_schema["description"].as_str(),
            Some(
                "TriggerSpec uses internally tagged JSON with a type field. Example once trigger: {\"type\":\"once\",\"due_at_utc\":\"2026-04-09T12:00:00Z\"}. Example interval trigger: {\"type\":\"interval\",\"start_at_utc\":\"2026-04-09T12:00:00Z\",\"every_seconds\":60}."
            )
        );
        assert_eq!(variants[0]["properties"]["type"]["const"], json!("once"));
        assert_eq!(
            variants[1]["properties"]["type"]["const"],
            json!("interval")
        );
        assert_eq!(
            variants[2]["properties"]["type"]["const"],
            json!("calendar")
        );
    }

    #[tokio::test]
    async fn schedule_tool_dispatcher_delegates_to_schedule_handler() -> Result<(), String> {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let dispatcher = ScheduleToolDispatcher::new(service.clone());
        let args = serde_json::to_string(&schedule_request()).map_err(|error| error.to_string())?;
        let raw = RawValue::from_string(args).map_err(|error| error.to_string())?;
        let call = tool_call("sched-1", "meerkat_schedule_create", raw.as_ref());

        let outcome = dispatcher
            .dispatch(call)
            .await
            .map_err(|error| format!("{error:?}"))?;
        let created_value: Value = serde_json::from_str(&outcome.result.text_content())
            .map_err(|error| error.to_string())?;
        assert_eq!(created_value["name"].as_str(), Some("heartbeat"));
        assert!(created_value["schedule_id"].as_str().is_some());

        let listed = handle_schedule_tools_call(&service, "meerkat_schedule_list", &json!({}))
            .await
            .map_err(|error| format!("{error:?}"))?;
        assert_eq!(listed["schedules"].as_array().map(Vec::len), Some(1));
        Ok(())
    }

    #[tokio::test]
    async fn schedule_tool_dispatcher_unknown_tool_is_not_found() {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let dispatcher = ScheduleToolDispatcher::new(service);
        let raw = RawValue::from_string("{}".to_string()).expect("raw args");
        let err = dispatcher
            .dispatch(tool_call("sched-2", "unknown_schedule_tool", raw.as_ref()))
            .await
            .expect_err("unknown tool should fail");

        assert!(matches!(err, ToolError::NotFound { .. }));
    }

    #[tokio::test]
    async fn schedule_tool_dispatcher_maps_unsupported_backend_to_execution_failed() {
        let service = ScheduleService::new(Arc::new(crate::DisabledScheduleStore));
        let dispatcher = ScheduleToolDispatcher::new(service);
        let raw = RawValue::from_string(
            serde_json::to_string(&schedule_request()).expect("schedule request json"),
        )
        .expect("raw args");
        let err = dispatcher
            .dispatch(tool_call(
                "sched-3",
                "meerkat_schedule_create",
                raw.as_ref(),
            ))
            .await
            .expect_err("unsupported backend should fail");

        assert!(matches!(err, ToolError::ExecutionFailed { .. }));
    }

    #[test]
    #[allow(clippy::panic)]
    fn schedule_tools_have_schedule_provenance() {
        let service = ScheduleService::new(Arc::new(crate::DisabledScheduleStore));
        let dispatcher = ScheduleToolDispatcher::new(service);
        let tools = dispatcher.tools();
        assert!(
            !tools.is_empty(),
            "schedule should expose at least one tool"
        );
        for tool in tools.iter() {
            let prov = tool
                .provenance
                .as_ref()
                .unwrap_or_else(|| panic!("schedule tool '{}' is missing provenance", tool.name));
            assert_eq!(
                prov.kind,
                meerkat_core::types::ToolSourceKind::Schedule,
                "schedule tool '{}' should have Schedule provenance",
                tool.name
            );
            assert_eq!(prov.source_id, "schedule");
        }
    }
}
