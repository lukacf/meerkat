use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;

use meerkat_core::agent::{AgentToolDispatcher};
use meerkat_core::error::ToolError;
use meerkat_core::types::{ToolCallView, ToolResult};

use crate::ids::MeerkatId;
use crate::runtime::MobHandle;
use crate::tools::empty_object_schema;

pub const TOOL_SPAWN_MEERKAT: &str = "spawn_meerkat";
pub const TOOL_RETIRE_MEERKAT: &str = "retire_meerkat";
pub const TOOL_WIRE_PEERS: &str = "wire_peers";
pub const TOOL_UNWIRE_PEERS: &str = "unwire_peers";
pub const TOOL_LIST_MEERKATS: &str = "list_meerkats";

#[derive(Default)]
pub struct MobToolDispatcher {
    handle: Option<Arc<MobHandle>>,
    tool_defs: Arc<[Arc<meerkat_core::types::ToolDef>]>,
}

impl MobToolDispatcher {
    pub fn new(handle: Arc<MobHandle>) -> Self {
        Self {
            handle: Some(handle),
            tool_defs: vec![
                Arc::new(meerkat_core::types::ToolDef {
                    name: TOOL_SPAWN_MEERKAT.to_string(),
                    description: "Spawn a new meerkat by profile".to_string(),
                    input_schema: empty_object_schema(),
                }),
                Arc::new(meerkat_core::types::ToolDef {
                    name: TOOL_RETIRE_MEERKAT.to_string(),
                    description: "Retire an existing meerkat".to_string(),
                    input_schema: empty_object_schema(),
                }),
                Arc::new(meerkat_core::types::ToolDef {
                    name: TOOL_WIRE_PEERS.to_string(),
                    description: "Wire two meerkats together".to_string(),
                    input_schema: empty_object_schema(),
                }),
                Arc::new(meerkat_core::types::ToolDef {
                    name: TOOL_UNWIRE_PEERS.to_string(),
                    description: "Unwire two meerkats".to_string(),
                    input_schema: empty_object_schema(),
                }),
                Arc::new(meerkat_core::types::ToolDef {
                    name: TOOL_LIST_MEERKATS.to_string(),
                    description: "List active meerkats".to_string(),
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
                name: "mob_tools".to_string(),
                reason: "tool dispatcher is not bound to a mob handle".to_string(),
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SpawnArgs {
    profile: String,
    key: String,
    initial_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerArgs {
    a: String,
    b: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ListArgs {
    role: Option<String>,
}

#[derive(Serialize)]
struct PeerInfo {
    meerkat_id: String,
    profile: String,
}

#[async_trait::async_trait]
impl AgentToolDispatcher for MobToolDispatcher {
    fn tools(&self) -> std::sync::Arc<[std::sync::Arc<meerkat_core::types::ToolDef>]> {
        self.tool_defs.clone()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        let handle = self.handle()?;
        match call.name {
            TOOL_SPAWN_MEERKAT => {
                let args: SpawnArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::InvalidArguments { name: call.name.to_string(), reason: e.to_string() })?;
                let id = handle
                    .spawn(
                        &args.profile.into(),
                        MeerkatId::from(args.key),
                        args.initial_message,
                    )
                    .await
                    .map_err(|e| ToolError::ExecutionFailed { message: e.to_string() })?;
                Ok(ToolResult::from_tool_call(
                    &meerkat_core::types::ToolCall {
                        id: call.id.to_string(),
                        name: call.name.to_string(),
                        args: serde_json::Value::Null,
                    },
                    serde_json::to_string(&json!({"meerkat_id": id.as_str()})).unwrap_or_else(|_| "{}".to_string()),
                    false,
                ))
            }
            TOOL_RETIRE_MEERKAT => {
                let args: PeerArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::InvalidArguments { name: call.name.to_string(), reason: e.to_string() })?;
                handle
                    .retire(&MeerkatId::from(args.a))
                    .await
                    .map_err(|e| ToolError::ExecutionFailed { message: e.to_string() })?;
                Ok(ToolResult::from_tool_call(
                    &meerkat_core::types::ToolCall {
                        id: call.id.to_string(),
                        name: call.name.to_string(),
                        args: serde_json::Value::Null,
                    },
                    serde_json::to_string(&json!({"ok": true})).unwrap_or_else(|_| "{}".to_string()),
                    false,
                ))
            }
            TOOL_WIRE_PEERS => {
                let args: PeerArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::InvalidArguments { name: call.name.to_string(), reason: e.to_string() })?;
                handle
                    .wire(&MeerkatId::from(args.a), &MeerkatId::from(args.b))
                    .await
                    .map_err(|e| ToolError::ExecutionFailed { message: e.to_string() })?;
                Ok(ToolResult::from_tool_call(
                    &meerkat_core::types::ToolCall {
                        id: call.id.to_string(),
                        name: call.name.to_string(),
                        args: serde_json::Value::Null,
                    },
                    serde_json::to_string(&json!({"ok": true})).unwrap_or_else(|_| "{}".to_string()),
                    false,
                ))
            }
            TOOL_UNWIRE_PEERS => {
                let args: PeerArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::InvalidArguments { name: call.name.to_string(), reason: e.to_string() })?;
                handle
                    .unwire(&MeerkatId::from(args.a), &MeerkatId::from(args.b))
                    .await
                    .map_err(|e| ToolError::ExecutionFailed { message: e.to_string() })?;
                Ok(ToolResult::from_tool_call(
                    &meerkat_core::types::ToolCall {
                        id: call.id.to_string(),
                        name: call.name.to_string(),
                        args: serde_json::Value::Null,
                    },
                    serde_json::to_string(&json!({"ok": true})).unwrap_or_else(|_| "{}".to_string()),
                    false,
                ))
            }
            TOOL_LIST_MEERKATS => {
                let args: ListArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::InvalidArguments { name: call.name.to_string(), reason: e.to_string() })?;
                let role = args.role.as_deref().map(crate::ids::ProfileName::from);
                let peers = handle
                    .list_meerkats(role.as_ref())
                    .into_iter()
                    .map(|entry| PeerInfo {
                        meerkat_id: entry.meerkat_id.as_str().to_string(),
                        profile: entry.profile.as_str().to_string(),
                    })
                    .collect::<Vec<_>>();
                Ok(ToolResult::from_tool_call(
                    &meerkat_core::types::ToolCall {
                        id: call.id.to_string(),
                        name: call.name.to_string(),
                        args: serde_json::Value::Null,
                    },
                    serde_json::to_string(&peers).unwrap_or_else(|_| "[]".to_string()),
                    false,
                ))
            }
            _ => Err(ToolError::NotFound {
                name: call.name.to_string(),
            }),
        }
    }
}
