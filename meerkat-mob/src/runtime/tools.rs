use super::*;

// ---------------------------------------------------------------------------
// Mob tool dispatcher
// ---------------------------------------------------------------------------

pub(super) fn compose_external_tools_for_profile(
    profile: &crate::profile::Profile,
    tool_bundles: &BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    mob_handle: MobHandle,
) -> Result<Option<Arc<dyn AgentToolDispatcher>>, MobError> {
    let mut dispatchers: Vec<Arc<dyn AgentToolDispatcher>> = Vec::new();

    if profile.tools.mob || profile.tools.mob_tasks {
        dispatchers.push(Arc::new(MobToolDispatcher::new(
            mob_handle,
            profile.tools.mob,
            profile.tools.mob_tasks,
        )));
    }

    for name in &profile.tools.rust_bundles {
        let dispatcher = tool_bundles.get(name).cloned().ok_or_else(|| {
            MobError::Internal(format!(
                "tool bundle '{name}' is not registered on this mob builder"
            ))
        })?;
        dispatchers.push(dispatcher);
    }

    if dispatchers.is_empty() {
        return Ok(None);
    }
    if dispatchers.len() == 1 {
        return Ok(dispatchers.pop());
    }

    let mut gateway_builder = ToolGatewayBuilder::new();
    for dispatcher in dispatchers {
        gateway_builder = gateway_builder.add_dispatcher(dispatcher);
    }
    let gateway = gateway_builder
        .build()
        .map_err(|e| MobError::Internal(format!("failed to compose tool bundles: {e}")))?;
    Ok(Some(Arc::new(gateway)))
}

struct MobToolDispatcher {
    handle: MobHandle,
    tools: Arc<[Arc<ToolDef>]>,
}

impl MobToolDispatcher {
    fn new(handle: MobHandle, enable_mob: bool, enable_mob_tasks: bool) -> Self {
        let mut defs: Vec<Arc<ToolDef>> = Vec::new();
        if enable_mob {
            defs.push(tool_def(
                TOOL_SPAWN_MEERKAT,
                "Spawn a meerkat from a profile",
                json!({
                    "type": "object",
                    "properties": {
                        "profile": {"type": "string"},
                        "meerkat_id": {"type": "string"},
                        "initial_message": {"type": "string"},
                        "backend": {"type": "string", "enum": ["subagent", "external"]},
                        "runtime_mode": {"type": "string", "enum": ["autonomous_host", "turn_driven"]}
                    },
                    "required": ["profile", "meerkat_id"]
                }),
            ));
            defs.push(tool_def(
                TOOL_RETIRE_MEERKAT,
                "Retire a meerkat and archive its session",
                json!({
                    "type": "object",
                    "properties": {"meerkat_id": {"type": "string"}},
                    "required": ["meerkat_id"]
                }),
            ));
            defs.push(tool_def(
                TOOL_WIRE_PEERS,
                "Wire two meerkats with bidirectional trust",
                json!({
                    "type": "object",
                    "properties": {"a": {"type": "string"}, "b": {"type": "string"}},
                    "required": ["a", "b"]
                }),
            ));
            defs.push(tool_def(
                TOOL_UNWIRE_PEERS,
                "Unwire two meerkats and revoke bidirectional trust",
                json!({
                    "type": "object",
                    "properties": {"a": {"type": "string"}, "b": {"type": "string"}},
                    "required": ["a", "b"]
                }),
            ));
            defs.push(tool_def(
                TOOL_LIST_MEERKATS,
                "List all active meerkats. Response includes meerkat_id, profile, member_ref, session_id, wired_to.",
                json!({
                    "type": "object",
                    "properties": {}
                }),
            ));
            defs.push(tool_def(
                TOOL_MOB_LIST_FLOWS,
                "List all configured flow IDs for this mob.",
                json!({
                    "type": "object",
                    "properties": {}
                }),
            ));
            defs.push(tool_def(
                TOOL_MOB_RUN_FLOW,
                "Run a configured flow by ID with optional activation params. Returns run_id.",
                json!({
                    "type": "object",
                    "properties": {
                        "flow_id": {"type": "string"},
                        "params": {"type": "object"}
                    },
                    "required": ["flow_id"]
                }),
            ));
            defs.push(tool_def(
                TOOL_MOB_FLOW_STATUS,
                "Get persisted status and ledgers for a flow run.",
                json!({
                    "type": "object",
                    "properties": {
                        "run_id": {"type": "string"}
                    },
                    "required": ["run_id"]
                }),
            ));
            defs.push(tool_def(
                TOOL_MOB_CANCEL_FLOW,
                "Cancel an in-flight flow run by run_id.",
                json!({
                    "type": "object",
                    "properties": {
                        "run_id": {"type": "string"}
                    },
                    "required": ["run_id"]
                }),
            ));
        }
        if enable_mob_tasks {
            defs.push(tool_def(
                TOOL_MOB_TASK_CREATE,
                "Create a shared mob task. Response includes generated task_id.",
                json!({
                    "type": "object",
                    "properties": {
                        "subject": {"type": "string"},
                        "description": {"type": "string"},
                        "blocked_by": {"type": "array", "items": {"type": "string"}}
                    },
                    "required": ["subject", "description"]
                }),
            ));
            defs.push(tool_def(
                TOOL_MOB_TASK_LIST,
                "List shared mob tasks",
                json!({
                    "type": "object",
                    "properties": {}
                }),
            ));
            defs.push(tool_def(
                TOOL_MOB_TASK_UPDATE,
                "Update task status and owner. If owner is set, status must be in_progress and blocked_by dependencies must be completed.",
                json!({
                    "type": "object",
                    "properties": {
                        "task_id": {"type": "string"},
                        "status": {
                            "type": "string",
                            "enum": ["open", "in_progress", "completed", "cancelled"]
                        },
                        "owner": {"type": "string", "description": "Optional. Only valid when status is in_progress."}
                    },
                    "required": ["task_id", "status"]
                }),
            ));
            defs.push(tool_def(
                TOOL_MOB_TASK_GET,
                "Get a shared task by ID",
                json!({
                    "type": "object",
                    "properties": {"task_id": {"type": "string"}},
                    "required": ["task_id"]
                }),
            ));
        }

        Self {
            handle,
            tools: defs.into(),
        }
    }

    fn map_mob_error(call: ToolCallView<'_>, error: MobError) -> ToolError {
        ToolError::execution_failed(format!("tool '{}' failed: {error}", call.name))
    }

    fn encode_result(
        call: ToolCallView<'_>,
        value: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let content = serde_json::to_string(&value)
            .map_err(|error| ToolError::execution_failed(format!("encode tool result: {error}")))?;
        Ok(ToolResult {
            tool_use_id: call.id.to_string(),
            content,
            is_error: false,
        })
    }
}

fn tool_def(name: &str, description: &str, input_schema: serde_json::Value) -> Arc<ToolDef> {
    Arc::new(ToolDef {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
    })
}

#[derive(Deserialize)]
struct SpawnMeerkatArgs {
    profile: String,
    meerkat_id: String,
    #[serde(default)]
    initial_message: Option<String>,
    #[serde(default)]
    backend: Option<MobBackendKind>,
    #[serde(default)]
    runtime_mode: Option<crate::MobRuntimeMode>,
}

#[derive(Deserialize)]
struct RetireMeerkatArgs {
    meerkat_id: String,
}

#[derive(Deserialize)]
struct WirePeersArgs {
    a: String,
    b: String,
}

#[derive(Deserialize)]
struct TaskCreateArgs {
    subject: String,
    description: String,
    #[serde(default)]
    blocked_by: Vec<TaskId>,
}

#[derive(Deserialize)]
struct TaskUpdateArgs {
    task_id: TaskId,
    status: TaskStatus,
    owner: Option<String>,
}

#[derive(Deserialize)]
struct TaskGetArgs {
    task_id: TaskId,
}

#[derive(Deserialize)]
struct RunFlowArgs {
    flow_id: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Deserialize)]
struct FlowStatusArgs {
    run_id: String,
}

#[async_trait::async_trait]
impl AgentToolDispatcher for MobToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        match call.name {
            TOOL_SPAWN_MEERKAT => {
                let args: SpawnMeerkatArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                let member_ref = self
                    .handle
                    .spawn_member_ref_with_runtime_mode_and_backend(
                        ProfileName::from(args.profile),
                        MeerkatId::from(args.meerkat_id),
                        args.initial_message,
                        args.runtime_mode,
                        args.backend,
                    )
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(
                    call,
                    json!({
                        "member_ref": member_ref,
                        "session_id": member_ref.session_id(),
                    }),
                )
            }
            TOOL_RETIRE_MEERKAT => {
                let args: RetireMeerkatArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                self.handle
                    .retire(MeerkatId::from(args.meerkat_id))
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({"ok": true}))
            }
            TOOL_WIRE_PEERS => {
                let args: WirePeersArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                self.handle
                    .wire(MeerkatId::from(args.a), MeerkatId::from(args.b))
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({"ok": true}))
            }
            TOOL_UNWIRE_PEERS => {
                let args: WirePeersArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                self.handle
                    .unwire(MeerkatId::from(args.a), MeerkatId::from(args.b))
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({"ok": true}))
            }
            TOOL_LIST_MEERKATS => {
                let meerkats = self.handle.list_meerkats().await;
                let meerkats = meerkats
                    .into_iter()
                    .map(|entry| {
                        json!({
                            "meerkat_id": entry.meerkat_id,
                            "profile": entry.profile,
                            "runtime_mode": entry.runtime_mode,
                            "member_ref": entry.member_ref,
                            "session_id": entry.session_id(),
                            "wired_to": entry.wired_to,
                        })
                    })
                    .collect::<Vec<_>>();
                Self::encode_result(call, json!({ "meerkats": meerkats }))
            }
            TOOL_MOB_LIST_FLOWS => {
                let flows = self.handle.list_flows();
                Self::encode_result(call, json!({ "flows": flows }))
            }
            TOOL_MOB_RUN_FLOW => {
                let args: RunFlowArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                let run_id = self
                    .handle
                    .run_flow(FlowId::from(args.flow_id), args.params)
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({ "run_id": run_id }))
            }
            TOOL_MOB_FLOW_STATUS => {
                let args: FlowStatusArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                let run_id = args.run_id.parse::<RunId>().map_err(|error| {
                    ToolError::invalid_arguments(call.name, format!("invalid run_id: {error}"))
                })?;
                let run = self
                    .handle
                    .flow_status(run_id)
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({ "run": run }))
            }
            TOOL_MOB_CANCEL_FLOW => {
                let args: FlowStatusArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                let run_id = args.run_id.parse::<RunId>().map_err(|error| {
                    ToolError::invalid_arguments(call.name, format!("invalid run_id: {error}"))
                })?;
                self.handle
                    .cancel_flow(run_id)
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({"ok": true}))
            }
            TOOL_MOB_TASK_CREATE => {
                let args: TaskCreateArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                let task_id = self
                    .handle
                    .task_create(args.subject, args.description, args.blocked_by)
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({"ok": true, "task_id": task_id}))
            }
            TOOL_MOB_TASK_LIST => {
                let tasks = self
                    .handle
                    .task_list()
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({ "tasks": tasks }))
            }
            TOOL_MOB_TASK_UPDATE => {
                let args: TaskUpdateArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                self.handle
                    .task_update(args.task_id, args.status, args.owner.map(MeerkatId::from))
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({"ok": true}))
            }
            TOOL_MOB_TASK_GET => {
                let args: TaskGetArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                let task = self
                    .handle
                    .task_get(&args.task_id)
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({ "task": task }))
            }
            _ => Err(ToolError::not_found(call.name)),
        }
    }
}

const TOOL_SPAWN_MEERKAT: &str = "spawn_meerkat";
const TOOL_RETIRE_MEERKAT: &str = "retire_meerkat";
const TOOL_WIRE_PEERS: &str = "wire_peers";
const TOOL_UNWIRE_PEERS: &str = "unwire_peers";
const TOOL_LIST_MEERKATS: &str = "list_meerkats";
const TOOL_MOB_LIST_FLOWS: &str = "mob_list_flows";
const TOOL_MOB_RUN_FLOW: &str = "mob_run_flow";
const TOOL_MOB_FLOW_STATUS: &str = "mob_flow_status";
const TOOL_MOB_CANCEL_FLOW: &str = "mob_cancel_flow";
const TOOL_MOB_TASK_CREATE: &str = "mob_task_create";
const TOOL_MOB_TASK_LIST: &str = "mob_task_list";
const TOOL_MOB_TASK_UPDATE: &str = "mob_task_update";
const TOOL_MOB_TASK_GET: &str = "mob_task_get";
