use super::*;
use meerkat_core::SessionId;
use meerkat_core::agent::{BindOutcome, DispatcherCapabilities, OpsLifecycleBindError};
use meerkat_core::ops::AsyncOpRef;
use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;
use meerkat_core::service::MobToolAuthorityContext;
use meerkat_core::types::{ToolProvenance, ToolSourceKind};
use std::collections::HashSet;

// ---------------------------------------------------------------------------
// NameFilteredDispatcher
// ---------------------------------------------------------------------------

use meerkat_core::DynamicToolComposite;

// ---------------------------------------------------------------------------
// NameFilteredDispatcher
// ---------------------------------------------------------------------------

/// Wraps an inner dispatcher and hides tools whose names collide with
/// profile-declared tools. Profile tools always win on name collision.
pub(crate) struct NameFilteredDispatcher {
    inner: Arc<dyn AgentToolDispatcher>,
    excluded: HashSet<String>,
}

impl NameFilteredDispatcher {
    pub(crate) fn new(inner: Arc<dyn AgentToolDispatcher>, excluded: HashSet<String>) -> Self {
        Self { inner, excluded }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl AgentToolDispatcher for NameFilteredDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.inner
            .tools()
            .iter()
            .filter(|t| !self.excluded.contains(t.name.as_str()))
            .cloned()
            .collect::<Vec<_>>()
            .into()
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        if self.excluded.contains(call.name) {
            return Err(ToolError::not_found(call.name));
        }
        self.inner.dispatch(call).await
    }

    async fn poll_external_updates(&self) -> meerkat_core::ExternalToolUpdate {
        self.inner.poll_external_updates().await
    }

    fn capabilities(&self) -> DispatcherCapabilities {
        self.inner.capabilities()
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn OpsLifecycleRegistry>,
        owner_bridge_session_id: SessionId,
    ) -> Result<BindOutcome, OpsLifecycleBindError> {
        let owned = Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
        let outcome = owned
            .inner
            .bind_ops_lifecycle(registry, owner_bridge_session_id)?;
        let bound = outcome.was_bound();
        let inner = outcome.into_dispatcher();
        let wrapper = Arc::new(NameFilteredDispatcher {
            inner,
            excluded: owned.excluded,
        });
        Ok(if bound {
            BindOutcome::Bound(wrapper)
        } else {
            BindOutcome::Skipped(wrapper)
        })
    }
}

// ---------------------------------------------------------------------------
// Mob tool dispatcher
// ---------------------------------------------------------------------------

pub(super) fn compose_external_tools_for_profile(
    profile: &crate::profile::Profile,
    tool_bundles: &BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    mob_handle: MobHandle,
    default_external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    mob_tool_access_context: crate::build::MobToolAccessContext,
) -> Result<Option<Arc<dyn AgentToolDispatcher>>, MobError> {
    let mut dispatchers: Vec<Arc<dyn AgentToolDispatcher>> = Vec::new();

    if let crate::build::MobToolAccessContext::InjectedAuthority(authority_context) =
        mob_tool_access_context
        && (profile.tools.mob || profile.tools.mob_tasks)
    {
        dispatchers.push(Arc::new(MobOperatorToolDispatcher::new(
            mob_handle,
            profile.tools.mob,
            profile.tools.mob_tasks,
            authority_context,
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

    // Compose default external tools (e.g. callback tools from SDK) with
    // name-collision filtering: profile-declared tools always win.
    //
    // The dispatcher is always included even if it currently reports 0 tools,
    // because it may be dynamic (e.g. CallbackToolDispatcher backed by a shared
    // registry that gets populated later via tools/register). Dropping it here
    // would permanently disconnect the session from late-registered tools.
    if let Some(ext) = default_external_tools {
        let profile_names: HashSet<String> = dispatchers
            .iter()
            .flat_map(|d| {
                d.tools()
                    .iter()
                    .map(|t| t.name.to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
        let collisions: HashSet<String> = ext
            .tools()
            .iter()
            .filter(|t| profile_names.contains(t.name.as_str()))
            .map(|t| t.name.to_string())
            .collect();
        if collisions.is_empty() {
            dispatchers.push(ext);
        } else {
            dispatchers.push(Arc::new(NameFilteredDispatcher::new(ext, collisions)));
        }
    }

    if dispatchers.is_empty() {
        return Ok(None);
    }
    if dispatchers.len() == 1 {
        return Ok(dispatchers.pop());
    }

    // Use DynamicToolComposite instead of ToolGateway so that child
    // dispatchers that support dynamic tool lists (e.g. CallbackToolDispatcher
    // backed by a live registered_tools list) have their additions visible
    // at each tools()/dispatch() call. ToolGateway caches the tool list at
    // construction time which would freeze dynamic dispatchers.
    Ok(Some(Arc::new(DynamicToolComposite::new(dispatchers))))
}

struct MobOperatorToolDispatcher {
    handle: MobHandle,
    authority_context: MobToolAuthorityContext,
    tools: Arc<[Arc<ToolDef>]>,
    owner_bridge_session_id: Option<SessionId>,
    ops_registry: Option<Arc<dyn OpsLifecycleRegistry>>,
}

impl MobOperatorToolDispatcher {
    fn new(
        handle: MobHandle,
        enable_mob: bool,
        enable_mob_tasks: bool,
        authority_context: MobToolAuthorityContext,
    ) -> Self {
        let mut defs: Vec<Arc<ToolDef>> = Vec::new();
        if enable_mob {
            defs.push(tool_def(
                TOOL_SPAWN_MEMBER,
                "Spawn a mob member from a profile. Supports fresh, resume, or fork launch modes.",
                json!({
                    "type": "object",
                    "properties": {
                        "profile": {"type": "string"},
                        "member_id": {"type": "string"},
                        "initial_message": content_input_schema(),
                        "resume_bridge_session_id": {"type": "string", "description": "Preferred compatibility field for resume bridge bindings when launch_mode is omitted"},
                        "resume_session_id": {"type": "string", "description": "Deprecated: use resume_bridge_session_id or launch_mode.resume instead"},
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
                    "required": ["profile", "member_id"]
                }),
                ToolSourceKind::Mob,
            ));
            defs.push(tool_def(
                TOOL_SPAWN_MANY_MEMBERS,
                "Spawn multiple mob members in one call. Returns per-item results in input order.",
                json!({
                    "type": "object",
                    "properties": {
                        "specs": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "profile": {"type": "string"},
                                    "member_id": {"type": "string"},
                                    "initial_message": content_input_schema(),
                                    "resume_bridge_session_id": {"type": "string"},
                                    "resume_session_id": {"type": "string"},
                                    "backend": {"type": "string", "enum": ["session", "external"]},
                                    "runtime_mode": {"type": "string", "enum": ["autonomous_host", "turn_driven"]}
                                },
                                "required": ["profile", "member_id"]
                            }
                        }
                    },
                    "required": ["specs"]
                }),
                ToolSourceKind::Mob,
            ));
            defs.push(tool_def(
                TOOL_RETIRE_MEMBER,
                "Retire a member and archive its session.",
                json!({
                    "type": "object",
                    "properties": {"member_id": {"type": "string"}},
                    "required": ["member_id"]
                }),
                ToolSourceKind::Mob,
            ));
            defs.push(tool_def(
                TOOL_WIRE_MEMBERS,
                "Wire two mob members with bidirectional trust.",
                json!({
                    "type": "object",
                    "properties": {
                        "member_id": {"type": "string"},
                        "peer_member_id": {"type": "string"}
                    },
                    "required": ["member_id", "peer_member_id"]
                }),
                ToolSourceKind::Mob,
            ));
            defs.push(tool_def(
                TOOL_UNWIRE_MEMBERS,
                "Unwire two mob members and revoke bidirectional trust.",
                json!({
                    "type": "object",
                    "properties": {
                        "member_id": {"type": "string"},
                        "peer_member_id": {"type": "string"}
                    },
                    "required": ["member_id", "peer_member_id"]
                }),
                ToolSourceKind::Mob,
            ));
            defs.push(tool_def(
                TOOL_LIST_MEMBERS,
                "List all active mob members. Response includes identity-native lifecycle and runtime fields.",
                json!({
                    "type": "object",
                    "properties": {}
                }),
                ToolSourceKind::Mob,
            ));
            defs.push(tool_def(
                TOOL_MOB_LIST_FLOWS,
                "List all configured flow IDs for this mob.",
                json!({
                    "type": "object",
                    "properties": {}
                }),
                ToolSourceKind::Mob,
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
                ToolSourceKind::Mob,
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
                ToolSourceKind::Mob,
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
                ToolSourceKind::Mob,
            ));
            defs.push(tool_def(
                TOOL_FORCE_CANCEL_MEMBER,
                "Force-cancel a member's in-flight turn. Does not retire the member.",
                json!({
                    "type": "object",
                    "properties": {
                        "member_id": {"type": "string"}
                    },
                    "required": ["member_id"]
                }),
                ToolSourceKind::Mob,
            ));
            defs.push(tool_def(
                TOOL_MEMBER_STATUS,
                "Get a member's execution status snapshot including output preview and token usage.",
                json!({
                    "type": "object",
                    "properties": {
                        "member_id": {"type": "string"}
                    },
                    "required": ["member_id"]
                }),
                ToolSourceKind::Mob,
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
                ToolSourceKind::MobTasks,
            ));
            defs.push(tool_def(
                TOOL_MOB_TASK_LIST,
                "List shared mob tasks",
                json!({
                    "type": "object",
                    "properties": {}
                }),
                ToolSourceKind::MobTasks,
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
                ToolSourceKind::MobTasks,
            ));
            defs.push(tool_def(
                TOOL_MOB_TASK_GET,
                "Get a shared task by ID",
                json!({
                    "type": "object",
                    "properties": {"task_id": {"type": "string"}},
                    "required": ["task_id"]
                }),
                ToolSourceKind::MobTasks,
            ));
        }

        Self {
            handle,
            authority_context,
            tools: defs.into(),
            owner_bridge_session_id: None,
            ops_registry: None,
        }
    }

    fn ensure_current_mob_scope(&self, tool_name: &str) -> Result<(), ToolError> {
        let mob_id = self.handle.definition().id.as_str();
        if self.authority_context.can_manage_mob(mob_id) {
            return Ok(());
        }
        Err(ToolError::access_denied(tool_name))
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
            session_effects: vec![],
        })
    }

    fn member_ref_payload(
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> meerkat_contracts::WireMemberRef {
        meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), identity.as_str())
    }

    fn spawn_result_payload(mob_id: &MobId, result: &super::SpawnResult) -> serde_json::Value {
        json!({
            "agent_identity": result.agent_identity,
            "member_ref": Self::member_ref_payload(mob_id, &result.agent_identity),
        })
    }

    fn member_list_entry_result_payload(
        mob_id: &MobId,
        entry: &MobMemberListEntry,
    ) -> Result<serde_json::Value, ToolError> {
        let mut value = serde_json::to_value(entry).map_err(|error| {
            ToolError::execution_failed(format!("encode member list entry: {error}"))
        })?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "member_ref".to_string(),
                serde_json::to_value(Self::member_ref_payload(mob_id, &entry.agent_identity))
                    .map_err(|error| {
                        ToolError::execution_failed(format!("encode member_ref: {error}"))
                    })?,
            );
        }
        Ok(value)
    }

    async fn spawn_result_payload_for_identity(
        &self,
        identity: &AgentIdentity,
    ) -> Result<serde_json::Value, MobError> {
        let entry = self.handle.get_member(identity).await.ok_or_else(|| {
            MobError::Internal(format!(
                "spawn succeeded but roster entry missing for '{identity}'"
            ))
        })?;
        let agent_identity = entry.agent_identity;
        let member_ref = Self::member_ref_payload(&self.handle.definition().id, &agent_identity);
        Ok(json!({
            "agent_identity": agent_identity,
            "member_ref": member_ref,
        }))
    }

    async fn spawn_many_result_entry_for_identity(
        &self,
        identity: &AgentIdentity,
    ) -> Result<meerkat_contracts::MobSpawnManyResultEntry, MobError> {
        let entry = self.handle.get_member(identity).await.ok_or_else(|| {
            MobError::Internal(format!(
                "spawn succeeded but roster entry missing for '{identity}'"
            ))
        })?;
        let agent_identity = entry.agent_identity;
        let member_ref = Self::member_ref_payload(&self.handle.definition().id, &agent_identity);
        Ok(meerkat_contracts::MobSpawnManyResultEntry::spawned(
            agent_identity.to_string(),
            member_ref,
        ))
    }

    async fn record_successful_operator_action(&self, tool_name: &str) {
        if let Err(error) = self
            .handle
            .record_operator_action_provenance(tool_name, &self.authority_context)
            .await
        {
            tracing::warn!(
                tool_name,
                mob_id = %self.handle.definition().id,
                error = %error,
                "operator provenance projection append failed"
            );
        }
    }
}

fn tool_def(
    name: &str,
    description: &str,
    input_schema: serde_json::Value,
    kind: ToolSourceKind,
) -> Arc<ToolDef> {
    Arc::new(ToolDef {
        name: name.into(),
        description: description.to_string(),
        input_schema,
        provenance: Some(ToolProvenance {
            kind,
            source_id: "mob".into(),
        }),
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
struct SpawnMemberArgs {
    profile: String,
    member_id: String,
    #[serde(default)]
    initial_message: Option<ContentInput>,
    #[serde(default)]
    resume_bridge_session_id: Option<meerkat_core::types::SessionId>,
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
    member_id: String,
}

#[derive(Deserialize)]
struct MemberStatusArgs {
    member_id: String,
}

#[derive(Deserialize)]
struct SpawnManyMembersArgs {
    specs: Vec<SpawnMemberArgs>,
}

#[derive(Deserialize)]
struct RetireMemberArgs {
    member_id: String,
}

#[derive(Deserialize)]
struct WireMembersArgs {
    member_id: String,
    peer_member_id: String,
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
impl AgentToolDispatcher for MobOperatorToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        if self.tools.iter().any(|tool| tool.name == call.name) {
            self.ensure_current_mob_scope(call.name)?;
        }
        match call.name {
            TOOL_SPAWN_MEMBER => {
                let args: SpawnMemberArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                let agent_identity = AgentIdentity::from(args.member_id.as_str());
                let mut spec = SpawnMemberSpec::from_wire(
                    args.profile,
                    args.member_id,
                    args.initial_message,
                    args.runtime_mode,
                    args.backend,
                );
                // Resolve launch mode: explicit launch_mode takes precedence,
                // then legacy resume_session_id, then default (Fresh).
                if let Some(launch_mode) = args.launch_mode {
                    spec = spec.with_launch_mode(launch_mode);
                } else if let Some(session_id) =
                    args.resume_bridge_session_id.or(args.resume_session_id)
                {
                    spec = spec.with_resume_bridge_session_id(session_id);
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
                let (result, async_ops) = match (&self.owner_bridge_session_id, &self.ops_registry)
                {
                    (Some(owner_bridge_session_id), Some(ops_registry)) => {
                        let receipt = self
                            .handle
                            .spawn_spec_receipt_with_owner_context(
                                spec,
                                super::handle::CanonicalOpsOwnerContext {
                                    owner_bridge_session_id: owner_bridge_session_id.clone(),
                                    ops_registry: Arc::clone(ops_registry),
                                },
                            )
                            .await
                            .map_err(|error| Self::map_mob_error(call, error))?;
                        let result = self
                            .spawn_result_payload_for_identity(&agent_identity)
                            .await
                            .map_err(|error| Self::map_mob_error(call, error))?;
                        (result, vec![AsyncOpRef::detached(receipt.operation_id)])
                    }
                    _ => {
                        let spawn_result = self
                            .handle
                            .spawn_spec(spec)
                            .await
                            .map_err(|error| Self::map_mob_error(call, error))?;
                        let result =
                            Self::spawn_result_payload(&self.handle.definition().id, &spawn_result);
                        (result, Vec::new())
                    }
                };
                self.record_successful_operator_action(call.name).await;
                Self::encode_result_with_async_ops(call, result, async_ops)
            }
            TOOL_SPAWN_MANY_MEMBERS => {
                let args: SpawnManyMembersArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                let identities = args
                    .specs
                    .iter()
                    .map(|spec| AgentIdentity::from(spec.member_id.as_str()))
                    .collect::<Vec<_>>();
                let specs = args
                    .specs
                    .into_iter()
                    .map(|spec| {
                        let mut spawn_spec = SpawnMemberSpec::from_wire(
                            spec.profile,
                            spec.member_id,
                            spec.initial_message,
                            spec.runtime_mode,
                            spec.backend,
                        );
                        if let Some(launch_mode) = spec.launch_mode {
                            spawn_spec = spawn_spec.with_launch_mode(launch_mode);
                        } else if let Some(session_id) =
                            spec.resume_bridge_session_id.or(spec.resume_session_id)
                        {
                            spawn_spec = spawn_spec.with_resume_bridge_session_id(session_id);
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
                let (results, async_ops) = match (&self.owner_bridge_session_id, &self.ops_registry)
                {
                    (Some(owner_bridge_session_id), Some(ops_registry)) => {
                        let receipts = self
                            .handle
                            .spawn_many_receipts_with_owner_context(
                                specs,
                                super::handle::CanonicalOpsOwnerContext {
                                    owner_bridge_session_id: owner_bridge_session_id.clone(),
                                    ops_registry: Arc::clone(ops_registry),
                                },
                            )
                            .await;
                        let async_ops = receipts
                            .iter()
                            .filter_map(|result| result.as_ref().ok())
                            .map(|receipt| AsyncOpRef::detached(receipt.operation_id.clone()))
                            .collect::<Vec<_>>();
                        let mut results = Vec::with_capacity(receipts.len());
                        for (result, identity) in receipts.into_iter().zip(identities.into_iter()) {
                            match result {
                                Ok(_receipt) => {
                                    let entry = self
                                        .spawn_many_result_entry_for_identity(&identity)
                                        .await
                                        .map_err(|error| Self::map_mob_error(call, error))?;
                                    results.push(json!(entry));
                                }
                                Err(error) => {
                                    results.push(json!(
                                        meerkat_contracts::MobSpawnManyResultEntry::failed(
                                            error.to_string()
                                        )
                                    ));
                                }
                            }
                        }
                        (results, async_ops)
                    }
                    _ => {
                        let results = self
                            .handle
                            .spawn_many(specs)
                            .await
                            .into_iter()
                            .map(|result| match result {
                                Ok(spawn_result) => {
                                    let identity = spawn_result.agent_identity.to_string();
                                    json!(meerkat_contracts::MobSpawnManyResultEntry::spawned(
                                        identity.clone(),
                                        Self::member_ref_payload(
                                            &self.handle.definition().id,
                                            &spawn_result.agent_identity,
                                        ),
                                    ))
                                }
                                Err(error) => {
                                    json!(meerkat_contracts::MobSpawnManyResultEntry::failed(
                                        error.to_string()
                                    ))
                                }
                            })
                            .collect::<Vec<_>>();
                        (results, Vec::new())
                    }
                };
                self.record_successful_operator_action(call.name).await;
                Self::encode_result_with_async_ops(call, json!({ "results": results }), async_ops)
            }
            TOOL_RETIRE_MEMBER => {
                let args: RetireMemberArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                self.handle
                    .retire(AgentIdentity::from(args.member_id))
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                self.record_successful_operator_action(call.name).await;
                Self::encode_result(call, json!({"ok": true}))
            }
            TOOL_WIRE_MEMBERS => {
                let args: WireMembersArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                self.handle
                    .wire(
                        AgentIdentity::from(args.member_id),
                        AgentIdentity::from(args.peer_member_id),
                    )
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                self.record_successful_operator_action(call.name).await;
                Self::encode_result(call, json!({"ok": true}))
            }
            TOOL_UNWIRE_MEMBERS => {
                let args: WireMembersArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                self.handle
                    .unwire(
                        AgentIdentity::from(args.member_id),
                        AgentIdentity::from(args.peer_member_id),
                    )
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                self.record_successful_operator_action(call.name).await;
                Self::encode_result(call, json!({"ok": true}))
            }
            TOOL_LIST_MEMBERS => {
                let members = self.handle.list_members().await;
                let mob_id = self.handle.definition().id.clone();
                let members = members
                    .into_iter()
                    .map(|entry| Self::member_list_entry_result_payload(&mob_id, &entry))
                    .collect::<Result<Vec<_>, _>>()?;
                Self::encode_result(call, json!({ "members": members }))
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
                self.record_successful_operator_action(call.name).await;
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
                self.record_successful_operator_action(call.name).await;
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
                self.record_successful_operator_action(call.name).await;
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
                    .task_update(
                        args.task_id,
                        args.status,
                        args.owner.map(AgentIdentity::from),
                    )
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                self.record_successful_operator_action(call.name).await;
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
            TOOL_FORCE_CANCEL_MEMBER => {
                let args: ForceCancelArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                self.handle
                    .force_cancel_member(AgentIdentity::from(args.member_id))
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                self.record_successful_operator_action(call.name).await;
                Self::encode_result(call, json!({"ok": true}))
            }
            TOOL_MEMBER_STATUS => {
                let args: MemberStatusArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                let snapshot = self
                    .handle
                    .member_status(&AgentIdentity::from(args.member_id))
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!(snapshot))
            }
            _ => Err(ToolError::not_found(call.name)),
        }
    }

    fn capabilities(&self) -> DispatcherCapabilities {
        DispatcherCapabilities {
            ops_lifecycle: true,
        }
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn OpsLifecycleRegistry>,
        owner_bridge_session_id: SessionId,
    ) -> Result<BindOutcome, OpsLifecycleBindError> {
        if Arc::strong_count(&self) != 1 {
            return Err(OpsLifecycleBindError::SharedOwnership);
        }
        let this = Arc::try_unwrap(self).map_err(|_| OpsLifecycleBindError::SharedOwnership)?;
        Ok(BindOutcome::Bound(Arc::new(Self {
            handle: this.handle,
            authority_context: this.authority_context,
            tools: this.tools,
            owner_bridge_session_id: Some(owner_bridge_session_id),
            ops_registry: Some(registry),
        })))
    }
}

const TOOL_SPAWN_MEMBER: &str = "spawn_member";
const TOOL_SPAWN_MANY_MEMBERS: &str = "spawn_many_members";
const TOOL_RETIRE_MEMBER: &str = "retire_member";
const TOOL_FORCE_CANCEL_MEMBER: &str = "force_cancel_member";
const TOOL_MEMBER_STATUS: &str = "member_status";
const TOOL_WIRE_MEMBERS: &str = "wire_members";
const TOOL_UNWIRE_MEMBERS: &str = "unwire_members";
const TOOL_LIST_MEMBERS: &str = "list_members";
const TOOL_MOB_LIST_FLOWS: &str = "mob_list_flows";
const TOOL_MOB_RUN_FLOW: &str = "mob_run_flow";
const TOOL_MOB_FLOW_STATUS: &str = "mob_flow_status";
const TOOL_MOB_CANCEL_FLOW: &str = "mob_cancel_flow";
const TOOL_MOB_TASK_CREATE: &str = "mob_task_create";
const TOOL_MOB_TASK_LIST: &str = "mob_task_list";
const TOOL_MOB_TASK_UPDATE: &str = "mob_task_update";
const TOOL_MOB_TASK_GET: &str = "mob_task_get";
