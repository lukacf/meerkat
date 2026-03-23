use super::*;
use meerkat_core::SessionId;
use meerkat_core::agent::OpsLifecycleBindError;
use meerkat_core::ops::AsyncOpRef;
use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;

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
    owner_session_id: Option<SessionId>,
    ops_registry: Option<Arc<dyn OpsLifecycleRegistry>>,
}

impl MobToolDispatcher {
    fn new(handle: MobHandle, enable_mob: bool, enable_mob_tasks: bool) -> Self {
        let mut defs: Vec<Arc<ToolDef>> = Vec::new();
        if enable_mob {
            defs.push(tool_def(
                TOOL_SPAWN_MEERKAT,
                "Spawn a meerkat from a profile. Supports fresh, resume, or fork launch modes.",
                json!({
                    "type": "object",
                    "properties": {
                        "profile": {"type": "string"},
                        "meerkat_id": {"type": "string"},
                        "initial_message": content_input_schema(),
                        "resume_session_id": {"type": "string", "description": "Deprecated: use launch_mode.resume instead"},
                        "backend": {"type": "string", "enum": ["session", "external"]},
                        "runtime_mode": {"type": "string", "enum": ["autonomous_host", "turn_driven"]},
                        "launch_mode": {
                            "type": "object",
                            "description": "Launch mode: fresh (default), resume {session_id}, or fork {source_member_id, fork_context}",
                        },
                        "tool_access_policy": {
                            "type": "object",
                            "description": "Tool access policy: inherit (default), allow_list, or deny_list"
                        },
                        "budget_split_policy": {
                            "type": "object",
                            "description": "Budget split policy: equal, proportional, remaining, or fixed"
                        },
                        "auto_wire_parent": {"type": "boolean", "description": "Auto-wire to spawner after spawn"}
                    },
                    "required": ["profile", "meerkat_id"]
                }),
            ));
            defs.push(tool_def(
                TOOL_SPAWN_MANY_MEERKATS,
                "Spawn multiple meerkats in one call. Returns per-item results in input order.",
                json!({
                    "type": "object",
                    "properties": {
                        "specs": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "profile": {"type": "string"},
                                    "meerkat_id": {"type": "string"},
                                    "initial_message": content_input_schema(),
                                    "resume_session_id": {"type": "string"},
                                    "backend": {"type": "string", "enum": ["session", "external"]},
                                    "runtime_mode": {"type": "string", "enum": ["autonomous_host", "turn_driven"]}
                                },
                                "required": ["profile", "meerkat_id"]
                            }
                        }
                    },
                    "required": ["specs"]
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
                "List all active meerkats. Response includes meerkat_id, profile, member_ref, peer_id, session_id, wired_to, external_peer_specs.",
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
            defs.push(tool_def(
                TOOL_FORCE_CANCEL_MEERKAT,
                "Force-cancel a meerkat's in-flight turn. Does not retire the member.",
                json!({
                    "type": "object",
                    "properties": {
                        "meerkat_id": {"type": "string"}
                    },
                    "required": ["meerkat_id"]
                }),
            ));
            defs.push(tool_def(
                TOOL_MEERKAT_STATUS,
                "Get a meerkat's execution status snapshot including output preview and token usage.",
                json!({
                    "type": "object",
                    "properties": {
                        "meerkat_id": {"type": "string"}
                    },
                    "required": ["meerkat_id"]
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
            owner_session_id: None,
            ops_registry: None,
        }
    }

    fn map_mob_error(call: ToolCallView<'_>, error: MobError) -> ToolError {
        ToolError::execution_failed(format!("tool '{}' failed: {error}", call.name))
    }

    fn encode_result(
        call: ToolCallView<'_>,
        value: serde_json::Value,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        Self::encode_result_with_async_ops(call, value, Vec::new())
    }

    fn encode_result_with_async_ops(
        call: ToolCallView<'_>,
        value: serde_json::Value,
        async_ops: Vec<AsyncOpRef>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let content = serde_json::to_string(&value)
            .map_err(|error| ToolError::execution_failed(format!("encode tool result: {error}")))?;
        Ok(meerkat_core::ToolDispatchOutcome {
            result: ToolResult::new(call.id.to_string(), content, false),
            async_ops,
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

fn content_input_schema() -> serde_json::Value {
    json!({
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
                            "required": ["type", "text"]
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "image" },
                                "media_type": { "type": "string" },
                                "data": { "type": "string" }
                            },
                            "required": ["type", "media_type", "data"]
                        }
                    ]
                }
            }
        ]
    })
}

#[derive(Deserialize)]
struct SpawnMeerkatArgs {
    profile: String,
    meerkat_id: String,
    #[serde(default)]
    initial_message: Option<ContentInput>,
    #[serde(default)]
    resume_session_id: Option<meerkat_core::types::SessionId>,
    #[serde(default)]
    backend: Option<MobBackendKind>,
    #[serde(default)]
    runtime_mode: Option<crate::MobRuntimeMode>,
    #[serde(default)]
    launch_mode: Option<crate::launch::MemberLaunchMode>,
    #[serde(default)]
    tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
    #[serde(default)]
    budget_split_policy: Option<crate::launch::BudgetSplitPolicy>,
    #[serde(default)]
    auto_wire_parent: Option<bool>,
}

#[derive(Deserialize)]
struct ForceCancelArgs {
    meerkat_id: String,
}

#[derive(Deserialize)]
struct MeerkatStatusArgs {
    meerkat_id: String,
}

#[derive(Deserialize)]
struct SpawnManyMeerkatsArgs {
    specs: Vec<SpawnMeerkatArgs>,
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

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl AgentToolDispatcher for MobToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        match call.name {
            TOOL_SPAWN_MEERKAT => {
                let args: SpawnMeerkatArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                let mut spec = SpawnMemberSpec::from_wire(
                    args.profile,
                    args.meerkat_id,
                    args.initial_message,
                    args.runtime_mode,
                    args.backend,
                );
                // Resolve launch mode: explicit launch_mode takes precedence,
                // then legacy resume_session_id, then default (Fresh).
                if let Some(launch_mode) = args.launch_mode {
                    spec = spec.with_launch_mode(launch_mode);
                } else if let Some(session_id) = args.resume_session_id {
                    spec = spec.with_resume_session_id(session_id);
                }
                if let Some(policy) = args.tool_access_policy {
                    spec = spec.with_tool_access_policy(policy);
                }
                if let Some(policy) = args.budget_split_policy {
                    spec = spec.with_budget_split_policy(policy);
                }
                if let Some(auto_wire) = args.auto_wire_parent {
                    spec = spec.with_auto_wire_parent(auto_wire);
                }
                let (result, async_ops) = match (&self.owner_session_id, &self.ops_registry) {
                    (Some(owner_session_id), Some(ops_registry)) => {
                        let receipt = self
                            .handle
                            .spawn_spec_receipt_with_owner_context(
                                spec,
                                super::handle::CanonicalOpsOwnerContext {
                                    owner_session_id: owner_session_id.clone(),
                                    ops_registry: Arc::clone(ops_registry),
                                },
                            )
                            .await
                            .map_err(|error| Self::map_mob_error(call, error))?;
                        (
                            json!({
                                "member_ref": receipt.member_ref,
                                "session_id": receipt.member_ref.session_id(),
                            }),
                            vec![AsyncOpRef::detached(receipt.operation_id)],
                        )
                    }
                    _ => {
                        let member_ref = self
                            .handle
                            .spawn_spec(spec)
                            .await
                            .map_err(|error| Self::map_mob_error(call, error))?;
                        (
                            json!({
                                "member_ref": member_ref,
                                "session_id": member_ref.session_id(),
                            }),
                            Vec::new(),
                        )
                    }
                };
                Self::encode_result_with_async_ops(call, result, async_ops)
            }
            TOOL_SPAWN_MANY_MEERKATS => {
                let args: SpawnManyMeerkatsArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                let specs = args
                    .specs
                    .into_iter()
                    .map(|spec| {
                        let mut spawn_spec = SpawnMemberSpec::from_wire(
                            spec.profile,
                            spec.meerkat_id,
                            spec.initial_message,
                            spec.runtime_mode,
                            spec.backend,
                        );
                        if let Some(launch_mode) = spec.launch_mode {
                            spawn_spec = spawn_spec.with_launch_mode(launch_mode);
                        } else if let Some(session_id) = spec.resume_session_id {
                            spawn_spec = spawn_spec.with_resume_session_id(session_id);
                        }
                        if let Some(policy) = spec.tool_access_policy {
                            spawn_spec = spawn_spec.with_tool_access_policy(policy);
                        }
                        if let Some(policy) = spec.budget_split_policy {
                            spawn_spec = spawn_spec.with_budget_split_policy(policy);
                        }
                        if let Some(auto_wire) = spec.auto_wire_parent {
                            spawn_spec = spawn_spec.with_auto_wire_parent(auto_wire);
                        }
                        spawn_spec
                    })
                    .collect::<Vec<_>>();
                let (results, async_ops) = match (&self.owner_session_id, &self.ops_registry) {
                    (Some(owner_session_id), Some(ops_registry)) => {
                        let receipts = self
                            .handle
                            .spawn_many_receipts_with_owner_context(
                                specs,
                                super::handle::CanonicalOpsOwnerContext {
                                    owner_session_id: owner_session_id.clone(),
                                    ops_registry: Arc::clone(ops_registry),
                                },
                            )
                            .await;
                        let async_ops = receipts
                            .iter()
                            .filter_map(|result| result.as_ref().ok())
                            .map(|receipt| AsyncOpRef::detached(receipt.operation_id.clone()))
                            .collect::<Vec<_>>();
                        let results = receipts
                            .into_iter()
                            .map(|result| match result {
                                Ok(receipt) => json!({
                                    "ok": true,
                                    "member_ref": receipt.member_ref,
                                    "session_id": receipt.member_ref.session_id(),
                                }),
                                Err(error) => json!({
                                    "ok": false,
                                    "error": error.to_string(),
                                }),
                            })
                            .collect::<Vec<_>>();
                        (results, async_ops)
                    }
                    _ => {
                        let results = self
                            .handle
                            .spawn_many(specs)
                            .await
                            .into_iter()
                            .map(|result| match result {
                                Ok(member_ref) => json!({
                                    "ok": true,
                                    "member_ref": member_ref,
                                    "session_id": member_ref.session_id(),
                                }),
                                Err(error) => json!({
                                    "ok": false,
                                    "error": error.to_string(),
                                }),
                            })
                            .collect::<Vec<_>>();
                        (results, Vec::new())
                    }
                };
                Self::encode_result_with_async_ops(call, json!({ "results": results }), async_ops)
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
                let meerkats = self.handle.list_members().await;
                let meerkats = meerkats
                    .into_iter()
                    .map(|entry| {
                        json!({
                            "meerkat_id": entry.meerkat_id,
                            "profile": entry.profile,
                            "runtime_mode": entry.runtime_mode,
                            "member_ref": entry.member_ref,
                            "peer_id": entry.peer_id,
                            "session_id": entry.session_id(),
                            "wired_to": entry.wired_to,
                            "external_peer_specs": entry.external_peer_specs,
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
            TOOL_FORCE_CANCEL_MEERKAT => {
                let args: ForceCancelArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                self.handle
                    .force_cancel_member(MeerkatId::from(args.meerkat_id))
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({"ok": true}))
            }
            TOOL_MEERKAT_STATUS => {
                let args: MeerkatStatusArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                let snapshot = self
                    .handle
                    .member_status(&MeerkatId::from(args.meerkat_id))
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!(snapshot))
            }
            _ => Err(ToolError::not_found(call.name)),
        }
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn OpsLifecycleRegistry>,
        owner_session_id: SessionId,
    ) -> Result<Arc<dyn AgentToolDispatcher>, OpsLifecycleBindError> {
        if Arc::strong_count(&self) != 1 {
            return Err(OpsLifecycleBindError::SharedOwnership);
        }
        let this = Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
        Ok(Arc::new(Self {
            handle: this.handle,
            tools: this.tools,
            owner_session_id: Some(owner_session_id),
            ops_registry: Some(registry),
        }))
    }

    fn supports_ops_lifecycle_binding(&self) -> bool {
        true
    }
}

const TOOL_SPAWN_MEERKAT: &str = "spawn_meerkat";
const TOOL_SPAWN_MANY_MEERKATS: &str = "spawn_many_meerkats";
const TOOL_RETIRE_MEERKAT: &str = "retire_meerkat";
const TOOL_FORCE_CANCEL_MEERKAT: &str = "force_cancel_meerkat";
const TOOL_MEERKAT_STATUS: &str = "meerkat_status";
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
