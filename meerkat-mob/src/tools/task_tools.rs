use std::sync::Arc;

use serde::{Deserialize, Serialize};

use meerkat_core::agent::{AgentToolDispatcher};
use meerkat_core::error::ToolError;
use meerkat_core::types::{ToolCallView, ToolResult};

use crate::ids::MeerkatId;
use crate::runtime::MobHandle;
use crate::tasks::TaskStatus;
use crate::tools::empty_object_schema;

pub const TOOL_TASK_CREATE: &str = "mob_task_create";
pub const TOOL_TASK_LIST: &str = "mob_task_list";
pub const TOOL_TASK_UPDATE: &str = "mob_task_update";
pub const TOOL_TASK_GET: &str = "mob_task_get";

#[derive(Default)]
pub struct MobTaskToolDispatcher {
    handle: Option<Arc<MobHandle>>,
    tool_defs: Arc<[Arc<meerkat_core::types::ToolDef>]>,
}

impl MobTaskToolDispatcher {
    pub fn new(handle: Arc<MobHandle>) -> Self {
        Self {
            handle: Some(handle),
            tool_defs: vec![
                Arc::new(meerkat_core::types::ToolDef {
                    name: TOOL_TASK_CREATE.to_string(),
                    description: "Create a task".to_string(),
                    input_schema: empty_object_schema(),
                }),
                Arc::new(meerkat_core::types::ToolDef {
                    name: TOOL_TASK_LIST.to_string(),
                    description: "List tasks".to_string(),
                    input_schema: empty_object_schema(),
                }),
                Arc::new(meerkat_core::types::ToolDef {
                    name: TOOL_TASK_UPDATE.to_string(),
                    description: "Update task status/owner".to_string(),
                    input_schema: empty_object_schema(),
                }),
                Arc::new(meerkat_core::types::ToolDef {
                    name: TOOL_TASK_GET.to_string(),
                    description: "Get a task".to_string(),
                    input_schema: empty_object_schema(),
                }),
            ]
            .into(),
        }
    }

    fn handle(&self) -> Result<Arc<MobHandle>, ToolError> {
        self.handle
            .as_ref()
            .cloned()
            .ok_or_else(|| ToolError::Unavailable {
                name: "mob_task_tools".to_string(),
                reason: "tool dispatcher is not bound to a mob handle".to_string(),
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskCreateArgs {
    subject: String,
    description: String,
    blocked_by: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskUpdateArgs {
    task_id: String,
    status: Option<String>,
    owner: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskGetArgs {
    task_id: String,
}

#[async_trait::async_trait]
impl AgentToolDispatcher for MobTaskToolDispatcher {
    fn tools(&self) -> std::sync::Arc<[std::sync::Arc<meerkat_core::types::ToolDef>]> {
        self.tool_defs.clone()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        let handle = self.handle()?;
        match call.name {
            TOOL_TASK_CREATE => {
                let args: TaskCreateArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::InvalidArguments {
                        name: call.name.to_string(),
                        reason: e.to_string(),
                    })?;
                let task_id = handle
                    .task_create(
                        args.subject,
                        args.description,
                        args.blocked_by.unwrap_or_default(),
                    )
                    .await
                    .map_err(|err| ToolError::ExecutionFailed {
                        message: err.to_string(),
                    })?;
                Ok(ToolResult::from_tool_call(
                    &meerkat_core::types::ToolCall {
                        id: call.id.to_string(),
                        name: call.name.to_string(),
                        args: serde_json::Value::Null,
                    },
                    serde_json::json!({ "task_id": task_id }).to_string(),
                    false,
                ))
            }
            TOOL_TASK_LIST => {
                let tasks = handle
                    .task_list()
                    .await
                    .map_err(|err| ToolError::ExecutionFailed {
                        message: err.to_string(),
                    })?;
                Ok(ToolResult::from_tool_call(
                    &meerkat_core::types::ToolCall {
                        id: call.id.to_string(),
                        name: call.name.to_string(),
                        args: serde_json::Value::Null,
                    },
                    serde_json::to_string(&tasks).unwrap_or_else(|_| "[]".to_string()),
                    false,
                ))
            }
            TOOL_TASK_UPDATE => {
                let args: TaskUpdateArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::InvalidArguments {
                        name: call.name.to_string(),
                        reason: e.to_string(),
                    })?;
                let status = args.status.and_then(|status| match status.as_str() {
                    "Planned" => Some(TaskStatus::Planned),
                    "Open" => Some(TaskStatus::Open),
                    "InProgress" => Some(TaskStatus::InProgress),
                    "Blocked" => Some(TaskStatus::Blocked),
                    "Done" => Some(TaskStatus::Done),
                    "Failed" => Some(TaskStatus::Failed),
                    "Cancelled" => Some(TaskStatus::Cancelled),
                    _ => None,
                });
                handle
                    .task_update(args.task_id, status, args.owner.map(MeerkatId::from))
                    .await
                    .map_err(|err| ToolError::ExecutionFailed {
                        message: err.to_string(),
                    })?;
                Ok(ToolResult::from_tool_call(
                    &meerkat_core::types::ToolCall {
                        id: call.id.to_string(),
                        name: call.name.to_string(),
                        args: serde_json::Value::Null,
                    },
                    serde_json::json!({ "ok": true }).to_string(),
                    false,
                ))
            }
            TOOL_TASK_GET => {
                let args: TaskGetArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::InvalidArguments {
                        name: call.name.to_string(),
                        reason: e.to_string(),
                    })?;
                let tasks = handle
                    .task_list()
                    .await
                    .map_err(|err| ToolError::ExecutionFailed {
                        message: err.to_string(),
                    })?;
                let task = tasks.into_iter().find(|task| task.id == args.task_id);
                Ok(ToolResult::from_tool_call(
                    &meerkat_core::types::ToolCall {
                        id: call.id.to_string(),
                        name: call.name.to_string(),
                        args: serde_json::Value::Null,
                    },
                    serde_json::to_string(&task).unwrap_or_else(|_| "null".to_string()),
                    false,
                ))
            }
            _ => Err(ToolError::NotFound {
                name: call.name.to_string(),
            }),
        }
    }
}
