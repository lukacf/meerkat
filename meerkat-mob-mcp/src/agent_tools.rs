//! Agent-facing mob tool surface for delegation and orchestration.
//!
//! `AgentMobToolSurface` provides the 8 agent-internal mob tools (delegate,
//! mob_create, mob_destroy, mob_spawn_member, mob_retire_member,
//! mob_check_member, mob_list_members, mob_list) composed into the tool
//! gateway by `build_agent()`.
//!
//! `MobAwareSessionService` is a decorator that intercepts `archive()` to
//! clean up implicit mobs when their owning session is archived.

use async_trait::async_trait;
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::error::ToolError;
use meerkat_core::ops::AsyncOpRef;
use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextResult, CreateSessionRequest,
    SessionControlError, SessionError, SessionHistoryPage, SessionHistoryQuery, SessionQuery,
    SessionService, SessionServiceCommsExt, SessionServiceControlExt, SessionServiceHistoryExt,
    SessionSummary, SessionView, StartTurnRequest,
};
use meerkat_core::types::{ContentInput, RunResult, SessionId, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{AgentToolDispatcher, EventStream, StreamError};
use meerkat_mob::{
    MeerkatId, MobBackendKind, MobDefinition, MobError, MobId, MobRuntimeMode, MobSessionService,
    ProfileName, SpawnMemberSpec,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use ::tokio::sync::RwLock;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::RwLock;

use crate::MobMcpState;

// ─── Tool name constants ─────────────────────────────────────────────────

const TOOL_DELEGATE: &str = "delegate";
const TOOL_MOB_CREATE: &str = "mob_create";
const TOOL_MOB_DESTROY: &str = "mob_destroy";
const TOOL_MOB_SPAWN_MEMBER: &str = "mob_spawn_member";
const TOOL_MOB_RETIRE_MEMBER: &str = "mob_retire_member";
const TOOL_MOB_CHECK_MEMBER: &str = "mob_check_member";
const TOOL_MOB_LIST_MEMBERS: &str = "mob_list_members";
const TOOL_MOB_LIST: &str = "mob_list";

// ─── AgentMobToolSurface ─────────────────────────────────────────────────

/// Agent-internal tool surface for mob delegation and orchestration.
///
/// Composed by `build_agent()` into the tool gateway. Provides 8 tools
/// for implicit delegation (lazy mob creation) and explicit orchestration.
pub struct AgentMobToolSurface {
    state: Arc<MobMcpState>,
    /// Comms runtime for the owning agent — will be used for explicit peer
    /// wiring when the orchestrating agent needs to wire itself to mob members
    /// beyond the auto_wire_parent path.
    _comms_runtime: Option<Arc<dyn CoreCommsRuntime>>,
    /// Pre-seeded on resume; otherwise set by first delegate via get_or_create_implicit_mob.
    /// Read-only cache — MobMcpState is the canonical owner.
    cached_implicit_mob_id: RwLock<Option<MobId>>,
    tools: Arc<[Arc<ToolDef>]>,
    owner_session_id: SessionId,
    /// Ops lifecycle registry — will be used for registering spawned member
    /// operations as barriers on the owning agent's turn boundary.
    _ops_registry: Arc<dyn OpsLifecycleRegistry>,
}

impl AgentMobToolSurface {
    /// Create a new agent mob tool surface.
    ///
    /// # Arguments
    /// * `state` - Shared MobMcpState for mob lifecycle operations
    /// * `comms_runtime` - Optional comms runtime for auto-wiring spawned members
    /// * `implicit_mob_id` - Pre-seeded implicit mob ID (resume case)
    /// * `ops_registry` - Ops lifecycle registry for barrier operations
    /// * `owner_session_id` - Session ID of the owning agent
    pub fn new(
        state: Arc<MobMcpState>,
        comms_runtime: Option<Arc<dyn CoreCommsRuntime>>,
        implicit_mob_id: Option<MobId>,
        ops_registry: Arc<dyn OpsLifecycleRegistry>,
        owner_session_id: SessionId,
    ) -> Self {
        let tools = build_tool_defs();
        Self {
            state,
            _comms_runtime: comms_runtime,
            cached_implicit_mob_id: RwLock::new(implicit_mob_id),
            tools,
            owner_session_id,
            _ops_registry: ops_registry,
        }
    }

    /// Usage instructions for agent mob tools to be added to the system prompt.
    pub fn usage_instructions() -> &'static str {
        "# Agent Delegation & Orchestration\n\n\
         You can delegate work to helper agents and orchestrate multi-agent mobs:\n\n\
         - delegate: Quick helper spawn — creates an implicit mob on first use, spawns a member, auto-wires comms\n\
         - mob_create: Create an explicit mob with full control over profiles, wiring, and flows\n\
         - mob_destroy: Destroy an explicit mob (cannot destroy implicit delegation mob)\n\
         - mob_spawn_member: Spawn a member into any mob\n\
         - mob_retire_member: Archive a member and its session\n\
         - mob_check_member: Check a member's execution status and output\n\
         - mob_list_members: List all members of a mob\n\
         - mob_list: List all mobs you manage\n\n\
         Use `delegate` for simple one-off helpers. Use explicit mob tools for complex multi-agent workflows."
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
            .map_err(|e| ToolError::execution_failed(format!("encode tool result: {e}")))?;
        Ok(meerkat_core::ToolDispatchOutcome {
            result: ToolResult::new(call.id.to_string(), content, false),
            async_ops,
        })
    }

    fn map_mob_error(call: ToolCallView<'_>, error: MobError) -> ToolError {
        ToolError::execution_failed(format!("tool '{}' failed: {error}", call.name))
    }

    /// Get or create the implicit mob for this agent's session.
    ///
    /// Returns (mob_id, first_delegate) where first_delegate is true if the
    /// mob was just created.
    async fn ensure_implicit_mob(&self) -> Result<(MobId, bool), MobError> {
        // Fast path: check cache
        {
            let cache = self.cached_implicit_mob_id.read().await;
            if let Some(ref mob_id) = *cache {
                return Ok((mob_id.clone(), false));
            }
        }

        // Slow path: create via single-flight on MobMcpState
        let mob_id: MobId = self
            .state
            .get_or_create_implicit_mob(&self.owner_session_id.to_string())
            .await?;

        // Check if we were the creator by seeing if cache was empty
        let first_delegate = {
            let mut cache = self.cached_implicit_mob_id.write().await;
            let was_empty = cache.is_none();
            *cache = Some(mob_id.clone());
            was_empty
        };

        Ok((mob_id, first_delegate))
    }

    async fn dispatch_delegate(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: DelegateArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;

        let (mob_id, first_delegate) = self
            .ensure_implicit_mob()
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        // Build spawn spec
        let meerkat_id = MeerkatId::from(
            args.member_id
                .unwrap_or_else(|| format!("helper-{}", uuid::Uuid::new_v4())),
        );
        let profile = ProfileName::from(args.profile.unwrap_or_else(|| "delegate".to_string()));
        let mut spec = SpawnMemberSpec::new(profile, meerkat_id.clone());
        spec.initial_message = Some(ContentInput::Text(args.task));
        spec.runtime_mode = Some(MobRuntimeMode::AutonomousHost);
        spec.auto_wire_parent = true;
        if let Some(instructions) = args.additional_instructions {
            spec.additional_instructions = Some(vec![instructions]);
        }

        // Spawn via MobMcpState with ops lifecycle
        let member_ref = self
            .state
            .mob_spawn_spec(&mob_id, spec)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        let mut result = json!({
            "mob_id": mob_id,
            "meerkat_id": meerkat_id,
            "member_ref": member_ref,
            "session_id": member_ref.session_id(),
        });

        if first_delegate {
            result["system_notice"] = json!(
                "Implicit delegation mob created. Helpers are auto-wired to you via comms. \
                 Use `send` to communicate. The mob persists across turns."
            );
        }

        Self::encode_result(call, result)
    }

    async fn dispatch_mob_create(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: MobCreateArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;

        let mob_id = self
            .state
            .mob_create_definition(args.definition)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        Self::encode_result(call, json!({"mob_id": mob_id}))
    }

    async fn dispatch_mob_destroy(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: MobIdArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
        let mob_id = MobId::from(args.mob_id);

        // Canonical ownership check: query MobMcpState for the mob's definition
        if self.state.is_implicit_mob(&mob_id).await {
            return Err(ToolError::Other(
                "Cannot destroy implicit delegation mob. It is managed automatically.".to_string(),
            ));
        }

        self.state
            .mob_destroy(&mob_id)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        Self::encode_result(call, json!({"ok": true}))
    }

    async fn dispatch_mob_spawn_member(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: SpawnMemberArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
        let mob_id = MobId::from(args.mob_id);

        let mut spec = SpawnMemberSpec::new(
            ProfileName::from(args.profile),
            MeerkatId::from(args.member_id),
        );
        spec.initial_message = args.initial_message;
        spec.runtime_mode = args.runtime_mode;
        spec.backend = args.backend;
        if let Some(auto_wire) = args.auto_wire_parent {
            spec.auto_wire_parent = auto_wire;
        }

        let member_ref = self
            .state
            .mob_spawn_spec(&mob_id, spec)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        Self::encode_result(
            call,
            json!({
                "member_ref": member_ref,
                "session_id": member_ref.session_id(),
            }),
        )
    }

    async fn dispatch_mob_retire_member(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: MemberArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;

        self.state
            .mob_retire(&MobId::from(args.mob_id), MeerkatId::from(args.member_id))
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        Self::encode_result(call, json!({"ok": true}))
    }

    async fn dispatch_mob_check_member(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: MemberArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;

        let snapshot = self
            .state
            .mob_member_status(&MobId::from(args.mob_id), &MeerkatId::from(args.member_id))
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        Self::encode_result(call, json!(snapshot))
    }

    async fn dispatch_mob_list_members(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: MobIdArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;

        let members = self
            .state
            .mob_list_members(&MobId::from(args.mob_id))
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        Self::encode_result(call, json!({"members": members}))
    }

    async fn dispatch_mob_list(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let mobs = self.state.mob_list().await;
        let mob_list: Vec<serde_json::Value> = mobs
            .into_iter()
            .map(|(id, status)| {
                json!({
                    "mob_id": id,
                    "status": status.as_str(),
                })
            })
            .collect();

        Self::encode_result(call, json!({"mobs": mob_list}))
    }
}

// ─── MobToolsFactory implementation ─────────────────────────────────────

/// Factory that captures `MobMcpState` and produces `AgentMobToolSurface`
/// instances with session-scoped bindings.
///
/// Passed to `SessionBuildOptions.mob_tools` by surfaces that enable mob
/// tools. The factory is invoked inside `build_agent()` with session-specific
/// arguments (session ID, ops lifecycle, comms runtime).
pub struct AgentMobToolSurfaceFactory {
    state: Arc<MobMcpState>,
}

impl AgentMobToolSurfaceFactory {
    /// Create a new factory wrapping the given mob state.
    pub fn new(state: Arc<MobMcpState>) -> Self {
        Self { state }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl meerkat_core::service::MobToolsFactory for AgentMobToolSurfaceFactory {
    async fn build_mob_tools(
        &self,
        args: meerkat_core::service::MobToolsBuildArgs,
    ) -> Result<Arc<dyn AgentToolDispatcher>, Box<dyn std::error::Error + Send + Sync>> {
        let session_id_str = args.session_id.to_string();
        let implicit_mob_id = self.state.find_implicit_mob(&session_id_str).await;
        let surface = AgentMobToolSurface::new(
            Arc::clone(&self.state),
            args.comms_runtime,
            implicit_mob_id,
            args.ops_registry,
            args.session_id,
        );
        Ok(Arc::new(surface))
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for AgentMobToolSurface {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        match call.name {
            TOOL_DELEGATE => self.dispatch_delegate(call).await,
            TOOL_MOB_CREATE => self.dispatch_mob_create(call).await,
            TOOL_MOB_DESTROY => self.dispatch_mob_destroy(call).await,
            TOOL_MOB_SPAWN_MEMBER => self.dispatch_mob_spawn_member(call).await,
            TOOL_MOB_RETIRE_MEMBER => self.dispatch_mob_retire_member(call).await,
            TOOL_MOB_CHECK_MEMBER => self.dispatch_mob_check_member(call).await,
            TOOL_MOB_LIST_MEMBERS => self.dispatch_mob_list_members(call).await,
            TOOL_MOB_LIST => self.dispatch_mob_list(call).await,
            _ => Err(ToolError::not_found(call.name)),
        }
    }
}

// ─── Tool definitions ────────────────────────────────────────────────────

fn tool_def(name: &str, description: &str, input_schema: serde_json::Value) -> Arc<ToolDef> {
    Arc::new(ToolDef {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
    })
}

fn build_tool_defs() -> Arc<[Arc<ToolDef>]> {
    vec![
        tool_def(
            TOOL_DELEGATE,
            "Delegate a task to a helper agent. Creates an implicit mob on first use, \
             spawns a member with auto-wiring to you. Use for quick one-off delegation.",
            json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "The task description/prompt for the helper"
                    },
                    "profile": {
                        "type": "string",
                        "description": "Profile name for the helper (default: 'helper')"
                    },
                    "member_id": {
                        "type": "string",
                        "description": "Unique identifier for this helper (auto-generated if omitted)"
                    },
                    "additional_instructions": {
                        "type": "string",
                        "description": "Extra instructions appended to the helper's system prompt"
                    }
                },
                "required": ["task"]
            }),
        ),
        tool_def(
            TOOL_MOB_CREATE,
            "Create a new explicit mob with full control over profiles, wiring, and flows.",
            json!({
                "type": "object",
                "properties": {
                    "definition": {
                        "type": "object",
                        "description": "Full mob definition (id, profiles, wiring, flows, etc.)"
                    }
                },
                "required": ["definition"]
            }),
        ),
        tool_def(
            TOOL_MOB_DESTROY,
            "Destroy an explicit mob. Cannot destroy implicit delegation mobs.",
            json!({
                "type": "object",
                "properties": {
                    "mob_id": {"type": "string", "description": "Mob identifier to destroy"}
                },
                "required": ["mob_id"]
            }),
        ),
        tool_def(
            TOOL_MOB_SPAWN_MEMBER,
            "Spawn a new member into a mob from a profile.",
            json!({
                "type": "object",
                "properties": {
                    "mob_id": {"type": "string"},
                    "profile": {"type": "string", "description": "Profile name to spawn from"},
                    "member_id": {"type": "string", "description": "Unique member identifier"},
                    "initial_message": {
                        "oneOf": [
                            {"type": "string"},
                            {"type": "array", "items": {"type": "object"}}
                        ],
                        "description": "Initial message/task for the member"
                    },
                    "runtime_mode": {
                        "type": "string",
                        "enum": ["autonomous_host", "turn_driven"],
                        "description": "Runtime mode (default: autonomous_host)"
                    },
                    "backend": {
                        "type": "string",
                        "enum": ["session", "external"]
                    },
                    "auto_wire_parent": {
                        "type": "boolean",
                        "description": "Auto-wire to spawner after spawn"
                    }
                },
                "required": ["mob_id", "profile", "member_id"]
            }),
        ),
        tool_def(
            TOOL_MOB_RETIRE_MEMBER,
            "Retire a mob member and archive its session.",
            json!({
                "type": "object",
                "properties": {
                    "mob_id": {"type": "string"},
                    "member_id": {"type": "string"}
                },
                "required": ["mob_id", "member_id"]
            }),
        ),
        tool_def(
            TOOL_MOB_CHECK_MEMBER,
            "Check a member's execution status, output preview, and token usage.",
            json!({
                "type": "object",
                "properties": {
                    "mob_id": {"type": "string"},
                    "member_id": {"type": "string"}
                },
                "required": ["mob_id", "member_id"]
            }),
        ),
        tool_def(
            TOOL_MOB_LIST_MEMBERS,
            "List all members of a mob with their status and session info.",
            json!({
                "type": "object",
                "properties": {
                    "mob_id": {"type": "string"}
                },
                "required": ["mob_id"]
            }),
        ),
        tool_def(
            TOOL_MOB_LIST,
            "List all mobs managed by this agent.",
            json!({
                "type": "object",
                "properties": {}
            }),
        ),
    ]
    .into()
}

// ─── Argument types ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct DelegateArgs {
    task: String,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    member_id: Option<String>,
    #[serde(default)]
    additional_instructions: Option<String>,
}

#[derive(Deserialize)]
struct MobCreateArgs {
    definition: MobDefinition,
}

#[derive(Deserialize)]
struct MobIdArgs {
    mob_id: String,
}

#[derive(Deserialize)]
struct SpawnMemberArgs {
    mob_id: String,
    profile: String,
    member_id: String,
    #[serde(default)]
    initial_message: Option<ContentInput>,
    #[serde(default)]
    runtime_mode: Option<MobRuntimeMode>,
    #[serde(default)]
    backend: Option<MobBackendKind>,
    #[serde(default)]
    auto_wire_parent: Option<bool>,
}

#[derive(Deserialize)]
struct MemberArgs {
    mob_id: String,
    member_id: String,
}

// ─── MobAwareSessionService ─────────────────────────────────────────────

/// Session service decorator that intercepts `archive()` to clean up
/// implicit mobs when their owning session is archived.
///
/// All other methods delegate to the inner service unchanged.
pub struct MobAwareSessionService {
    inner: Arc<dyn MobSessionService>,
    mob_state: Arc<MobMcpState>,
}

impl MobAwareSessionService {
    /// Create a new mob-aware session service wrapper.
    pub fn new(inner: Arc<dyn MobSessionService>, mob_state: Arc<MobMcpState>) -> Self {
        Self { inner, mob_state }
    }
}

// ── SessionService impl (archive() overridden) ──────────────────────────

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionService for MobAwareSessionService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        self.inner.create_session(req).await
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        self.inner.start_turn(id, req).await
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        self.inner.interrupt(id).await
    }

    async fn set_session_client(
        &self,
        id: &SessionId,
        client: Arc<dyn meerkat_core::AgentLlmClient>,
    ) -> Result<(), SessionError> {
        self.inner.set_session_client(id, client).await
    }

    async fn hot_swap_session_llm_identity(
        &self,
        id: &SessionId,
        client: Arc<dyn meerkat_core::AgentLlmClient>,
        identity: meerkat_core::SessionLlmIdentity,
    ) -> Result<(), SessionError> {
        self.inner
            .hot_swap_session_llm_identity(id, client, identity)
            .await
    }

    async fn update_session_keep_alive(
        &self,
        id: &SessionId,
        keep_alive: bool,
    ) -> Result<(), SessionError> {
        self.inner.update_session_keep_alive(id, keep_alive).await
    }

    async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
        self.inner.has_live_session(id).await
    }

    async fn set_session_tool_filter(
        &self,
        id: &SessionId,
        filter: meerkat_core::ToolFilter,
    ) -> Result<(), SessionError> {
        self.inner.set_session_tool_filter(id, filter).await
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        self.inner.read(id).await
    }

    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        self.inner.list(query).await
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        // Archive the session FIRST — only clean up mob on success
        self.inner.archive(id).await?;
        // Best-effort cleanup — session is already archived, mob is now orphaned
        let _ = self.mob_state.destroy_implicit_mob(&id.to_string()).await;
        Ok(())
    }

    async fn subscribe_session_events(&self, id: &SessionId) -> Result<EventStream, StreamError> {
        SessionService::subscribe_session_events(self.inner.as_ref(), id).await
    }
}

// ── SessionServiceCommsExt impl ─────────────────────────────────────────

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionServiceCommsExt for MobAwareSessionService {
    async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
        self.inner.comms_runtime(session_id).await
    }

    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        self.inner.event_injector(session_id).await
    }

    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        self.inner.interaction_event_injector(session_id).await
    }
}

// ── SessionServiceControlExt impl ───────────────────────────────────────

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionServiceControlExt for MobAwareSessionService {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        self.inner.append_system_context(id, req).await
    }
}

// ── SessionServiceHistoryExt impl ───────────────────────────────────────

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionServiceHistoryExt for MobAwareSessionService {
    async fn read_history(
        &self,
        id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Result<SessionHistoryPage, SessionError> {
        self.inner.read_history(id, query).await
    }
}

// ── MobSessionService impl ─────────────────────────────────────────────

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobSessionService for MobAwareSessionService {
    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        MobSessionService::subscribe_session_events(self.inner.as_ref(), session_id).await
    }

    fn supports_persistent_sessions(&self) -> bool {
        self.inner.supports_persistent_sessions()
    }

    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::RuntimeSessionAdapter>> {
        self.inner.runtime_adapter()
    }

    async fn session_belongs_to_mob(&self, session_id: &SessionId, mob_id: &MobId) -> bool {
        self.inner.session_belongs_to_mob(session_id, mob_id).await
    }

    async fn load_persisted_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::Session>, SessionError> {
        self.inner.load_persisted_session(session_id).await
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::RunId,
        req: StartTurnRequest,
        boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::InputId>,
    ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, SessionError> {
        self.inner
            .apply_runtime_turn(session_id, run_id, req, boundary, contributing_input_ids)
            .await
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        self.inner.discard_live_session(session_id).await
    }

    async fn cancel_all_checkpointers(&self) {
        self.inner.cancel_all_checkpointers().await;
    }

    async fn rearm_all_checkpointers(&self) {
        self.inner.rearm_all_checkpointers().await;
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_all_tool_definitions_present() {
        let defs = build_tool_defs();
        assert_eq!(defs.len(), 8);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"delegate"));
        assert!(names.contains(&"mob_create"));
        assert!(names.contains(&"mob_destroy"));
        assert!(names.contains(&"mob_spawn_member"));
        assert!(names.contains(&"mob_retire_member"));
        assert!(names.contains(&"mob_check_member"));
        assert!(names.contains(&"mob_list_members"));
        assert!(names.contains(&"mob_list"));
    }

    #[test]
    fn test_tool_schemas_are_valid_json_objects() {
        let defs = build_tool_defs();
        for def in defs.iter() {
            assert!(
                def.input_schema.is_object(),
                "tool '{}' schema is not an object",
                def.name
            );
            let schema = def.input_schema.as_object().unwrap();
            assert_eq!(
                schema.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "tool '{}' schema type is not 'object'",
                def.name
            );
        }
    }

    #[test]
    fn test_delegate_requires_task() {
        let defs = build_tool_defs();
        let delegate = defs.iter().find(|d| d.name == "delegate").unwrap();
        let required = delegate.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("task")));
    }

    #[tokio::test]
    async fn test_dispatch_unknown_tool_returns_not_found() {
        let state = MobMcpState::new_in_memory();
        let ops_registry = meerkat_runtime::RuntimeOpsLifecycleRegistry::default();
        let surface =
            AgentMobToolSurface::new(state, None, None, Arc::new(ops_registry), SessionId::new());

        let args_raw = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "test-1",
            name: "unknown_tool",
            args: &args_raw,
        };
        let result = surface.dispatch(call).await;
        assert!(matches!(result, Err(ToolError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_mob_list_empty() {
        let state = MobMcpState::new_in_memory();
        let ops_registry = meerkat_runtime::RuntimeOpsLifecycleRegistry::default();
        let surface =
            AgentMobToolSurface::new(state, None, None, Arc::new(ops_registry), SessionId::new());

        let args_raw = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let call = ToolCallView {
            id: "test-1",
            name: "mob_list",
            args: &args_raw,
        };
        let result = surface.dispatch(call).await.unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&result.result.text_content()).unwrap();
        assert_eq!(parsed["mobs"], json!([]));
    }
}
