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
use meerkat_core::error::ToolError;
use meerkat_core::ops::AsyncOpRef;
use meerkat_core::service::{
    CreateSessionRequest, SessionError, SessionQuery, SessionService, SessionSummary, SessionView,
    StartTurnRequest,
};
use meerkat_core::types::{ContentInput, RunResult, SessionId, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{AgentToolDispatcher, EventStream, StreamError};
use meerkat_mob::{
    MeerkatId, MobBackendKind, MobDefinition, MobError, MobId, MobRuntimeMode, ProfileName,
    SpawnMemberSpec,
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
    /// Pre-seeded on resume; otherwise set by first delegate via get_or_create_implicit_mob.
    /// Read-only cache — MobMcpState is the canonical owner.
    cached_implicit_mob_id: RwLock<Option<MobId>>,
    /// Mobs owned by this session (implicit + explicitly created). Session-scoped access control.
    owned_mob_ids: RwLock<std::collections::HashSet<MobId>>,
    tools: Arc<[Arc<ToolDef>]>,
    owner_session_id: SessionId,
    /// Model name inherited by implicit mob helpers.
    model: String,
    /// Parent agent's comms name (for building TrustedPeerSpec when wiring helpers).
    comms_name: Option<String>,
    /// Parent agent's comms peer ID (ed25519 public key).
    comms_peer_id: Option<String>,
}

impl AgentMobToolSurface {
    /// Create a new agent mob tool surface.
    ///
    /// # Arguments
    /// * `state` - Shared MobMcpState for mob lifecycle operations
    /// * `implicit_mob_id` - Pre-seeded implicit mob ID (resume case)
    /// * `model` - Model name inherited by spawned helpers
    /// * `owner_session_id` - Session ID of the owning agent
    pub fn new(
        state: Arc<MobMcpState>,
        implicit_mob_id: Option<MobId>,
        model: String,
        owner_session_id: SessionId,
        comms_name: Option<String>,
        comms_peer_id: Option<String>,
    ) -> Self {
        let mut owned = std::collections::HashSet::new();
        if let Some(ref id) = implicit_mob_id {
            owned.insert(id.clone());
        }
        Self::new_with_owned(
            state,
            implicit_mob_id,
            owned.into_iter().collect(),
            model,
            owner_session_id,
            comms_name,
            comms_peer_id,
        )
    }

    /// Create with a pre-populated set of owned mob IDs (for resume).
    pub fn new_with_owned(
        state: Arc<MobMcpState>,
        implicit_mob_id: Option<MobId>,
        owned_mob_ids: Vec<MobId>,
        model: String,
        owner_session_id: SessionId,
        comms_name: Option<String>,
        comms_peer_id: Option<String>,
    ) -> Self {
        let tools = build_tool_defs();
        Self {
            state,
            cached_implicit_mob_id: RwLock::new(implicit_mob_id),
            owned_mob_ids: RwLock::new(owned_mob_ids.into_iter().collect()),
            tools,
            owner_session_id,
            model,
            comms_name,
            comms_peer_id,
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

    /// Check if this session owns the given mob.
    async fn owns_mob(&self, mob_id: &MobId) -> bool {
        self.owned_mob_ids.read().await.contains(mob_id)
    }

    /// Register a mob as owned by this session.
    async fn register_owned_mob(&self, mob_id: MobId) {
        self.owned_mob_ids.write().await.insert(mob_id);
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
            .get_or_create_implicit_mob(&self.owner_session_id.to_string(), &self.model)
            .await?;

        // Check if we were the creator by seeing if cache was empty
        let first_delegate = {
            let mut cache = self.cached_implicit_mob_id.write().await;
            let was_empty = cache.is_none();
            *cache = Some(mob_id.clone());
            was_empty
        };

        // Register as owned by this session
        self.register_owned_mob(mob_id.clone()).await;

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
        // Implicit mob always uses the "delegate" profile.
        let mut spec = SpawnMemberSpec::new(ProfileName::from("delegate"), meerkat_id.clone());
        spec.initial_message = Some(ContentInput::Text(args.task));
        spec.runtime_mode = Some(MobRuntimeMode::AutonomousHost);
        // Don't use auto_wire_parent — it requires an orchestrator member in the roster.
        // We wire explicitly below using PeerTarget::External.
        spec.auto_wire_parent = false;
        if let Some(instructions) = args.additional_instructions {
            spec.additional_instructions = Some(vec![instructions]);
        }

        // Spawn via MobMcpState
        let member_ref = self
            .state
            .mob_spawn_spec(&mob_id, spec)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        // Wire the helper to the parent agent's comms identity.
        // The parent is external to the mob (not a roster member), so we use
        // PeerTarget::External with a TrustedPeerSpec built from the parent's comms info.
        if let (Some(name), Some(peer_id)) = (&self.comms_name, &self.comms_peer_id)
            && let Ok(parent_spec) = meerkat_core::comms::TrustedPeerSpec::new(
                name.as_str(),
                peer_id.as_str(),
                format!("inproc://{name}"),
            )
        {
            let _ = self
                .state
                .mob_wire(
                    &mob_id,
                    meerkat_id.clone(),
                    meerkat_mob::PeerTarget::External(parent_spec),
                )
                .await;
        }

        let mut result = json!({
            "mob_id": mob_id,
            "meerkat_id": meerkat_id,
            "member_ref": member_ref,
            "session_id": member_ref.session_id(),
        });

        if first_delegate {
            result["system_notice"] = json!(
                "Implicit delegation mob created. Helpers are wired to you via comms. \
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

        // Tag with owner session so this mob is session-scoped and discoverable on resume.
        let mut definition = args.definition;
        definition.owner_session_id = Some(self.owner_session_id.to_string());

        let mob_id = self
            .state
            .mob_create_definition(definition)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        self.register_owned_mob(mob_id.clone()).await;

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

        // Session-scoped: only allow destroying mobs created by this session
        if !self.owns_mob(&mob_id).await {
            return Err(ToolError::Other(format!(
                "Mob '{mob_id}' is not owned by this session"
            )));
        }

        self.state
            .mob_destroy(&mob_id)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        self.owned_mob_ids.write().await.remove(&mob_id);

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

        if !self.owns_mob(&mob_id).await {
            return Err(ToolError::Other(format!(
                "Mob '{mob_id}' is not owned by this session"
            )));
        }

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

        let mob_id = MobId::from(args.mob_id);
        if !self.owns_mob(&mob_id).await {
            return Err(ToolError::Other(format!(
                "Mob '{mob_id}' is not owned by this session"
            )));
        }

        self.state
            .mob_retire(&mob_id, MeerkatId::from(args.member_id))
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

        let mob_id = MobId::from(args.mob_id);
        if !self.owns_mob(&mob_id).await {
            return Err(ToolError::Other(format!(
                "Mob '{mob_id}' is not owned by this session"
            )));
        }

        let snapshot = self
            .state
            .mob_member_status(&mob_id, &MeerkatId::from(args.member_id))
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

        let mob_id = MobId::from(args.mob_id);
        if !self.owns_mob(&mob_id).await {
            return Err(ToolError::Other(format!(
                "Mob '{mob_id}' is not owned by this session"
            )));
        }

        let members = self
            .state
            .mob_list_members(&mob_id)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        Self::encode_result(call, json!({"members": members}))
    }

    async fn dispatch_mob_list(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let owned = self.owned_mob_ids.read().await;
        let mobs = self.state.mob_list().await;
        let mob_list: Vec<serde_json::Value> = mobs
            .into_iter()
            .filter(|(id, _)| owned.contains(id))
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
        let mut implicit_mob_id = self.state.find_implicit_mob(&session_id_str).await;

        // If the implicit mob exists but its delegate profile has a stale model
        // (e.g. session resumed with --model override), destroy and recreate it
        // so new helpers inherit the current model.
        if let Some(ref mob_id) = implicit_mob_id
            && let Ok(handle) = self.state.handle_for(mob_id).await
        {
            let profile_model = handle
                .definition()
                .profiles
                .get(&ProfileName::from("delegate"))
                .map(|p| p.model.as_str());
            if profile_model != Some(&args.model) {
                let _ = self.state.mob_destroy(mob_id).await;
                implicit_mob_id = None;
            }
        }

        // Find all mobs owned by this session (implicit + explicit) for resume.
        let owned_mob_ids = self.state.find_mobs_for_session(&session_id_str).await;
        // Extract parent comms identity for wiring helpers.
        let comms_peer_id = args.comms_runtime.as_ref().and_then(|r| r.public_key());
        let surface = AgentMobToolSurface::new_with_owned(
            Arc::clone(&self.state),
            implicit_mob_id,
            owned_mob_ids,
            args.model,
            args.session_id,
            args.comms_name,
            comms_peer_id,
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

// ─── MobCleanupSessionService ───────────────────────────────────────────

/// Session service decorator that intercepts `archive()` to destroy
/// all mobs owned by the archived session (both implicit and explicit).
///
/// Wraps a plain `dyn SessionService` — no extension traits needed.
/// Wire this around the main session service at surface construction
/// time so every archive path triggers cleanup automatically.
pub struct MobCleanupSessionService {
    inner: Arc<dyn SessionService>,
    mob_state: Arc<MobMcpState>,
}

impl MobCleanupSessionService {
    /// Wrap a session service with mob cleanup on archive.
    pub fn new(inner: Arc<dyn SessionService>, mob_state: Arc<MobMcpState>) -> Self {
        Self { inner, mob_state }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionService for MobCleanupSessionService {
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
        // Archive the session FIRST — only clean up mob on success.
        self.inner.archive(id).await?;
        // Best-effort cleanup — session is already archived, mobs are now orphaned.
        let _ = self.mob_state.destroy_implicit_mob(&id.to_string()).await;
        Ok(())
    }

    async fn subscribe_session_events(&self, id: &SessionId) -> Result<EventStream, StreamError> {
        self.inner.subscribe_session_events(id).await
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
        let surface = AgentMobToolSurface::new(
            state,
            None,
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
            None,
        );

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
        let surface = AgentMobToolSurface::new(
            state,
            None,
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
            None,
        );

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
