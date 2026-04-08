use std::path::Path;
use std::sync::Arc;

use anyhow::Context as _;
use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::error::ToolError;
use meerkat_core::types::{ToolCallView, ToolDef, ToolResult};
use meerkat_schedule::{ScheduleService, handle_schedule_tools_call, schedule_tools_list};
use meerkat_store::SqliteScheduleStore;
use serde_json::Value;

pub fn open_schedule_tool_dispatcher(
    root: &Path,
) -> anyhow::Result<Arc<dyn AgentToolDispatcher>> {
    let store = Arc::new(
        SqliteScheduleStore::open(root.join("schedules.sqlite3"))
            .with_context(|| format!("open sqlite schedule store in {}", root.display()))?,
    );
    Ok(Arc::new(ScheduleToolDispatcher::new(ScheduleService::new(
        store,
    ))))
}

struct ScheduleToolDispatcher {
    service: ScheduleService,
    tool_defs: Arc<[Arc<ToolDef>]>,
}

impl ScheduleToolDispatcher {
    fn new(service: ScheduleService) -> Self {
        let tool_defs: Arc<[Arc<ToolDef>]> = schedule_tools_list()
            .into_iter()
            .map(|tool| {
                Arc::new(ToolDef {
                    name: tool["name"].as_str().unwrap_or_default().to_string(),
                    description: tool["description"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    input_schema: tool["inputSchema"].clone(),
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

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        let is_schedule_tool = self.tool_defs.iter().any(|tool| tool.name == call.name);
        if !is_schedule_tool {
            return Err(ToolError::NotFound {
                name: call.name.to_string(),
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
