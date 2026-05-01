use std::sync::Arc;

use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::error::ToolError;
use meerkat_core::types::{ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind};
use serde_json::Value;

use crate::{ScheduleService, handle_schedule_tools_call, schedule_tools_list};

/// Reusable tool dispatcher that exposes the built-in schedule tool surface.
pub struct ScheduleToolSurface {
    service: ScheduleService,
    tool_defs: Arc<[Arc<ToolDef>]>,
}

impl ScheduleToolSurface {
    pub fn new(service: ScheduleService) -> Self {
        Self {
            service,
            tool_defs: build_tool_defs(),
        }
    }

    pub fn service(&self) -> &ScheduleService {
        &self.service
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for ScheduleToolSurface {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tool_defs)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        let is_schedule_tool = self.tool_defs.iter().any(|tool| tool.name == call.name);
        if !is_schedule_tool {
            return Err(ToolError::NotFound {
                name: call.name.into(),
            });
        }

        let args: Value = serde_json::from_str(call.args.get())
            .unwrap_or_else(|_| Value::String(call.args.get().to_string()));
        let result = handle_schedule_tools_call(&self.service, call.name, &args)
            .await
            .map_err(|error| ToolError::ExecutionFailed {
                message: format!("{} (code {})", error.message, error.code),
            })?;
        Ok(ToolResult::new(call.id.to_string(), result.to_string(), false).into())
    }
}

fn build_tool_defs() -> Arc<[Arc<ToolDef>]> {
    schedule_tools_list()
        .into_iter()
        .map(|tool| {
            Arc::new(ToolDef {
                name: tool["name"].as_str().unwrap_or_default().into(),
                description: tool["description"].as_str().unwrap_or_default().to_string(),
                input_schema: tool["inputSchema"].clone(),
                provenance: Some(ToolProvenance {
                    kind: ToolSourceKind::Schedule,
                    source_id: "schedule".into(),
                }),
            })
        })
        .collect::<Vec<_>>()
        .into()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    use chrono::{Duration, Utc};
    use serde_json::json;

    use crate::{
        CreateScheduleRequest, IntervalTriggerSpec, MemoryScheduleStore, MisfirePolicy,
        MissingTargetPolicy, OverlapPolicy, ScheduledSessionAction, SessionTargetBinding,
        TargetBinding, TriggerSpec,
    };

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
                session_id: meerkat_core::SessionId::new(),
                action: ScheduledSessionAction::Prompt {
                    prompt: meerkat_core::ContentInput::from("tool surface"),
                    system_prompt: None,
                    turn_metadata: None,
                },
            }),
            misfire_policy: MisfirePolicy::Skip,
            overlap_policy: OverlapPolicy::SkipIfRunning,
            missing_target_policy: MissingTargetPolicy::MarkMisfired,
            labels: std::collections::BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(2),
        }
    }

    #[tokio::test]
    async fn schedule_tool_surface_dispatches_builtin_schedule_tools() {
        let surface = ScheduleToolSurface::new(ScheduleService::new(Arc::new(
            MemoryScheduleStore::default(),
        )));

        let created = dispatch(
            &surface,
            "meerkat_schedule_create",
            serde_json::to_string(&schedule_request()).unwrap(),
        )
        .await;
        let schedule_id = created["schedule_id"]
            .as_str()
            .expect("create should return schedule_id");

        let listed = dispatch(&surface, "meerkat_schedule_list", "{}".into()).await;
        assert_eq!(
            listed["schedules"][0]["schedule_id"].as_str(),
            Some(schedule_id)
        );
    }

    #[tokio::test]
    async fn schedule_tool_surface_rejects_unknown_tool_name() {
        let surface = ScheduleToolSurface::new(ScheduleService::new(Arc::new(
            MemoryScheduleStore::default(),
        )));
        let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
        let error = surface
            .dispatch(ToolCallView {
                id: "call-1",
                name: "not_a_schedule_tool",
                args: &args,
            })
            .await
            .expect_err("unknown tools should be rejected");
        assert!(matches!(error, ToolError::NotFound { .. }));
    }

    async fn dispatch(
        surface: &ScheduleToolSurface,
        name: &str,
        args: String,
    ) -> serde_json::Value {
        let args = serde_json::value::RawValue::from_string(args).unwrap();
        let outcome = surface
            .dispatch(ToolCallView {
                id: "call-1",
                name,
                args: &args,
            })
            .await
            .expect("schedule dispatch should succeed");
        let content = outcome.result.text_content();
        serde_json::from_str(&content).unwrap_or_else(|_| json!({ "raw": content }))
    }
}
