//! Agent-facing mob tool surface for delegation and orchestration.
//!
//! `AgentMobToolSurface` provides the 8 agent-internal mob tools (delegate,
//! mob_create, mob_destroy, mob_spawn_member, mob_retire_member,
//! mob_check_member, mob_list_members, mob_list) composed into the tool
//! gateway by `build_agent()`.
//!
//! `archive_session_with_mob_cleanup()` is a helper that archives a session
//! and destroys its owned mobs in a single call.

use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::error::ToolError;
use meerkat_core::service::{MobToolAuthorityContext, SessionError, SessionService};
use meerkat_core::types::{ContentInput, SessionId, ToolCallView, ToolDef, ToolResult};
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
    /// Typed runtime-injected operator authority. This is the only source of
    /// authorization for agent-facing mob tools.
    authority_context: RwLock<MobToolAuthorityContext>,
    tools: Arc<[Arc<ToolDef>]>,
    owner_session_id: SessionId,
    /// Model name inherited by implicit mob helpers.
    model: String,
    /// Parent agent's comms name (for building TrustedPeerSpec when wiring helpers).
    comms_name: Option<String>,
    /// Parent agent's comms peer ID (ed25519 public key).
    comms_peer_id: Option<String>,
    /// Parent agent's comms runtime for bidirectional wiring.
    comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
}

impl AgentMobToolSurface {
    /// Create a new agent mob tool surface.
    ///
    /// # Arguments
    /// * `state` - Shared MobMcpState for mob lifecycle operations
    /// * `implicit_mob_id` - Pre-seeded implicit mob ID (resume case)
    /// * `model` - Model name inherited by spawned helpers
    /// * `owner_session_id` - Session ID of the owning agent
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state: Arc<MobMcpState>,
        implicit_mob_id: Option<MobId>,
        authority_context: MobToolAuthorityContext,
        model: String,
        owner_session_id: SessionId,
        comms_name: Option<String>,
        comms_peer_id: Option<String>,
        comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
    ) -> Self {
        Self::new_with_authority(
            state,
            implicit_mob_id,
            authority_context,
            model,
            owner_session_id,
            comms_name,
            comms_peer_id,
            comms_runtime,
        )
    }

    /// Create with a pre-populated set of owned mob IDs (for resume).
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_authority(
        state: Arc<MobMcpState>,
        implicit_mob_id: Option<MobId>,
        authority_context: MobToolAuthorityContext,
        model: String,
        owner_session_id: SessionId,
        comms_name: Option<String>,
        comms_peer_id: Option<String>,
        comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
    ) -> Self {
        let tools = build_tool_defs();
        Self {
            state,
            cached_implicit_mob_id: RwLock::new(implicit_mob_id),
            authority_context: RwLock::new(authority_context),
            tools,
            owner_session_id,
            model,
            comms_name,
            comms_peer_id,
            comms_runtime,
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
        let content = serde_json::to_string(&value)
            .map_err(|e| ToolError::execution_failed(format!("encode tool result: {e}")))?;
        Ok(meerkat_core::ToolDispatchOutcome::sync_result(
            ToolResult::new(call.id.to_string(), content, false),
        ))
    }

    fn map_mob_error(call: ToolCallView<'_>, error: MobError) -> ToolError {
        ToolError::execution_failed(format!("tool '{}' failed: {error}", call.name))
    }

    async fn authority_context_snapshot(&self) -> MobToolAuthorityContext {
        self.authority_context.read().await.clone()
    }

    async fn ensure_create_authority(&self, tool_name: &str) -> Result<(), ToolError> {
        if self.authority_context.read().await.can_create_mobs() {
            return Ok(());
        }
        Err(ToolError::access_denied(tool_name))
    }

    async fn ensure_mob_scope_authority(
        &self,
        tool_name: &str,
        mob_id: &MobId,
    ) -> Result<(), ToolError> {
        if self
            .authority_context
            .read()
            .await
            .can_manage_mob(mob_id.as_str())
        {
            return Ok(());
        }
        Err(ToolError::access_denied(tool_name))
    }

    async fn record_successful_operator_action(
        &self,
        handle: &meerkat_mob::MobHandle,
        tool_name: &str,
    ) {
        let authority_context = self.authority_context_snapshot().await;
        if let Err(error) = handle
            .record_operator_action_provenance(tool_name, &authority_context)
            .await
        {
            tracing::warn!(
                tool_name,
                mob_id = %handle.definition().id,
                error = %error,
                "agent mob operator provenance projection append failed"
            );
        }
    }

    async fn grant_exact_mob_scope_after_create(
        &self,
        tool_name: &str,
        mob_id: &MobId,
    ) -> Result<(), ToolError> {
        let authority_context = self
            .authority_context_snapshot()
            .await
            .grant_manage_mob(mob_id.to_string());

        match self
            .state
            .session_service()
            .update_session_mob_authority_context(
                &self.owner_session_id,
                Some(authority_context.clone()),
            )
            .await
        {
            Ok(()) => {}
            Err(SessionError::Unsupported(_)) => {
                tracing::debug!(
                    session_id = %self.owner_session_id,
                    mob_id = %mob_id,
                    "session service does not persist mob authority updates; keeping in-memory scope only"
                );
            }
            Err(error) => {
                return Err(ToolError::execution_failed(format!(
                    "tool '{tool_name}' failed: unable to persist exact mob scope for {mob_id}: {error}"
                )));
            }
        }

        *self.authority_context.write().await = authority_context;
        Ok(())
    }

    /// Get or create the implicit mob for this agent's session.
    ///
    /// Returns (mob_id, first_delegate) where first_delegate is true if the
    /// mob was just created.
    async fn ensure_implicit_mob(&self) -> Result<(MobId, bool), MobError> {
        let cached_mob_id = self.cached_implicit_mob_id.read().await.clone();
        let (mob_id, first_delegate) = self
            .state
            .ensure_implicit_mob_for_model(
                &self.owner_session_id.to_string(),
                &self.model,
                cached_mob_id.as_ref(),
            )
            .await?;

        let mut cache = self.cached_implicit_mob_id.write().await;
        *cache = Some(mob_id.clone());

        Ok((mob_id, first_delegate))
    }

    async fn dispatch_delegate(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        self.ensure_create_authority(call.name).await?;
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

        // Bidirectional comms wiring:
        // 1. Wire helper → parent: helper trusts parent as external peer
        // 2. Wire parent → helper: parent trusts helper so it can receive messages
        let wired = if let (Some(name), Some(peer_id)) = (&self.comms_name, &self.comms_peer_id)
            && let Ok(parent_spec) = meerkat_core::comms::TrustedPeerSpec::new(
                name.as_str(),
                peer_id.as_str(),
                format!("inproc://{name}"),
            ) {
            // Direction 1: helper trusts parent
            let helper_trusts_parent = self
                .state
                .mob_wire(
                    &mob_id,
                    meerkat_id.clone(),
                    meerkat_mob::PeerTarget::External(parent_spec),
                )
                .await
                .is_ok();

            // Direction 2: parent trusts helper
            // Get helper's comms identity from the mob roster.
            let parent_trusts_helper = if let Some(ref comms_rt) = self.comms_runtime {
                let handle = self.state.handle_for(&mob_id).await;
                if let Ok(handle) = handle {
                    let roster = handle.roster().await;
                    if let Some(entry) = roster.get(&meerkat_id) {
                        if let Some(ref helper_peer_id) = entry.peer_id {
                            let helper_comms_name =
                                format!("{}/{}/{}", mob_id, entry.profile, meerkat_id);
                            if let Ok(helper_spec) = meerkat_core::comms::TrustedPeerSpec::new(
                                &helper_comms_name,
                                helper_peer_id.as_str(),
                                format!("inproc://{helper_comms_name}"),
                            ) {
                                comms_rt.add_trusted_peer(helper_spec).await.is_ok()
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };

            helper_trusts_parent && parent_trusts_helper
        } else {
            false
        };

        let mut result = json!({
            "mob_id": mob_id,
            "meerkat_id": meerkat_id,
            "member_ref": member_ref,
            "session_id": member_ref.session_id(),
            "wired": wired,
        });

        if first_delegate {
            let notice = if wired {
                "Implicit delegation mob created. Helpers are wired to you via comms. \
                 Use `send` to communicate. The mob persists across turns."
            } else {
                "Implicit delegation mob created. The mob persists across turns. \
                 Use `mob_check_member` to poll helper status."
            };
            result["system_notice"] = json!(notice);
        }

        if let Ok(handle) = self.state.handle_for(&mob_id).await {
            self.record_successful_operator_action(&handle, call.name)
                .await;
        }

        Self::encode_result(call, result)
    }

    async fn dispatch_mob_create(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        self.ensure_create_authority(call.name).await?;
        let args: MobCreateArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;

        // Explicit mob creation owns its lifecycle semantics here. Caller-
        // supplied bookkeeping/lifecycle fields must not mint faux implicit
        // mobs, spoof cross-session ownership, or bypass session-scoped
        // cleanup policy.
        let mut definition = args.definition;
        definition.clear_internal_lifecycle_flags();
        definition.mark_session_scoped(&self.owner_session_id.to_string());

        let mob_id = self
            .state
            .mob_create_definition(definition)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        if let Err(error) = self
            .grant_exact_mob_scope_after_create(call.name, &mob_id)
            .await
        {
            if let Err(cleanup_error) = self.state.mob_destroy(&mob_id).await {
                tracing::warn!(
                    mob_id = %mob_id,
                    error = %cleanup_error,
                    "failed to roll back explicit mob after authority persistence error"
                );
            }
            return Err(error);
        }

        if let Ok(handle) = self.state.handle_for(&mob_id).await {
            self.record_successful_operator_action(&handle, call.name)
                .await;
        }

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

        self.ensure_mob_scope_authority(call.name, &mob_id).await?;
        let audit_handle = self.state.handle_for(&mob_id).await.ok();

        self.state
            .mob_destroy(&mob_id)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        if let Some(handle) = audit_handle.as_ref() {
            self.record_successful_operator_action(handle, call.name)
                .await;
        }

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

        self.ensure_mob_scope_authority(call.name, &mob_id).await?;
        let audit_handle = self
            .state
            .handle_for(&mob_id)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

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

        self.record_successful_operator_action(&audit_handle, call.name)
            .await;

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
        self.ensure_mob_scope_authority(call.name, &mob_id).await?;
        let audit_handle = self
            .state
            .handle_for(&mob_id)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        self.state
            .mob_retire(&mob_id, MeerkatId::from(args.member_id))
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        self.record_successful_operator_action(&audit_handle, call.name)
            .await;

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
        self.ensure_mob_scope_authority(call.name, &mob_id).await?;

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
        self.ensure_mob_scope_authority(call.name, &mob_id).await?;

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
        let authority_context = self.authority_context_snapshot().await;
        let mobs = self.state.mob_list().await;
        let mob_list: Vec<serde_json::Value> = mobs
            .into_iter()
            .filter(|(id, _)| authority_context.can_manage_mob(id.as_str()))
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
        let Some(authority_context) = args.authority_context else {
            return Ok(Arc::new(EmptyAgentToolSurface));
        };
        let session_id_str = args.session_id.to_string();
        let implicit_mob_id = self.state.find_implicit_mob(&session_id_str).await;

        // Extract parent comms identity for wiring helpers.
        let comms_peer_id = args.comms_runtime.as_ref().and_then(|r| r.public_key());
        let surface = AgentMobToolSurface::new_with_authority(
            Arc::clone(&self.state),
            implicit_mob_id,
            authority_context,
            args.model,
            args.session_id,
            args.comms_name,
            comms_peer_id,
            args.comms_runtime,
        );
        Ok(Arc::new(surface))
    }
}

struct EmptyAgentToolSurface;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for EmptyAgentToolSurface {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Vec::<Arc<ToolDef>>::new().into()
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        Err(ToolError::not_found(call.name))
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

// ─── Mob cleanup helper ─────────────────────────────────────────────────

/// Archive a session and clean up any mobs it owns (best-effort).
///
/// Single-function cleanup path used by CLI delete_session. Other surfaces
/// (REST, MCP, RPC) call `destroy_session_mobs` inline after their own
/// archive calls because their session service types are concrete and
/// can't be wrapped with a decorator.
pub async fn archive_session_with_mob_cleanup(
    service: &dyn SessionService,
    mob_state: &MobMcpState,
    session_id: &SessionId,
) -> Result<(), SessionError> {
    service.archive(session_id).await?;
    let _ = mob_state
        .destroy_session_mobs(&session_id.to_string())
        .await;
    Ok(())
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::service::{MobToolAuthorityContext, MobToolsFactory, OpaquePrincipalToken};

    fn create_only_authority() -> MobToolAuthorityContext {
        MobToolAuthorityContext::new(OpaquePrincipalToken::new("create-only"), true)
    }

    fn scope_only_authority(mob_id: &str) -> MobToolAuthorityContext {
        MobToolAuthorityContext::new(OpaquePrincipalToken::new("scope-only"), false)
            .with_managed_mob_scope([mob_id])
    }

    fn create_only_authority_with_provenance() -> MobToolAuthorityContext {
        create_only_authority()
            .with_caller_provenance(
                meerkat_core::service::MobToolCallerProvenance::new()
                    .with_session_id(SessionId::new())
                    .with_member_id("lead-1"),
            )
            .with_audit_invocation_id("audit-create")
    }

    fn sample_definition(mob_id: &str) -> MobDefinition {
        let mut profiles = std::collections::BTreeMap::new();
        profiles.insert(
            ProfileName::from("delegate"),
            meerkat_mob::Profile {
                model: "claude-sonnet-4-5".to_string(),
                skills: Vec::new(),
                tools: meerkat_mob::ToolConfig {
                    comms: true,
                    ..Default::default()
                },
                peer_description: "delegate helper".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            },
        );
        profiles.insert(
            ProfileName::from("worker"),
            meerkat_mob::Profile {
                model: "claude-sonnet-4-5".to_string(),
                skills: Vec::new(),
                tools: meerkat_mob::ToolConfig {
                    comms: true,
                    ..Default::default()
                },
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            },
        );

        MobDefinition {
            id: MobId::from(mob_id),
            orchestrator: None,
            profiles,
            mcp_servers: std::collections::BTreeMap::new(),
            wiring: Default::default(),
            skills: std::collections::BTreeMap::new(),
            backend: Default::default(),
            flows: std::collections::BTreeMap::new(),
            topology: None,
            supervisor: None,
            limits: None,
            spawn_policy: None,
            event_router: None,
            owner_session_id: None,
            session_cleanup_policy: meerkat_mob::definition::SessionCleanupPolicy::Manual,
            is_implicit: false,
        }
    }

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
            create_only_authority(),
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
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
    async fn test_build_mob_tools_returns_empty_surface_without_operator_capabilities() {
        let state = MobMcpState::new_in_memory();
        let factory = AgentMobToolSurfaceFactory::new(state);
        let dispatcher = factory
            .build_mob_tools(meerkat_core::service::MobToolsBuildArgs {
                session_id: SessionId::new(),
                model: "claude-sonnet-4-5".to_string(),
                authority_context: None,
                comms_name: None,
                comms_runtime: None,
            })
            .await
            .expect("build_mob_tools");

        assert!(
            dispatcher.tools().is_empty(),
            "ambient mob enablement must not surface operator tools without runtime-injected capabilities"
        );
    }

    #[tokio::test]
    async fn test_build_mob_tools_does_not_widen_scope_from_session_owned_mobs() {
        let state = MobMcpState::new_in_memory();
        let factory = AgentMobToolSurfaceFactory::new(Arc::clone(&state));
        let session_id = SessionId::new();
        let mut definition = sample_definition("owned-without-scope");
        definition.owner_session_id = Some(session_id.to_string());
        let mob_id = state
            .mob_create_definition(definition)
            .await
            .expect("create explicit mob");

        let dispatcher = factory
            .build_mob_tools(meerkat_core::service::MobToolsBuildArgs {
                session_id,
                model: "claude-sonnet-4-5".to_string(),
                authority_context: Some(create_only_authority()),
                comms_name: None,
                comms_runtime: None,
            })
            .await
            .expect("build_mob_tools");

        let list_args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let list_result = dispatcher
            .dispatch(ToolCallView {
                id: "list-owned",
                name: "mob_list",
                args: &list_args,
            })
            .await
            .expect("mob_list should still succeed");
        let listed: serde_json::Value =
            serde_json::from_str(&list_result.result.text_content()).unwrap();
        assert_eq!(
            listed["mobs"],
            json!([]),
            "session-owned mobs must not be widened into scope during rebuild"
        );

        let members_args =
            serde_json::value::RawValue::from_string(json!({ "mob_id": mob_id }).to_string())
                .unwrap();
        let members_error = dispatcher
            .dispatch(ToolCallView {
                id: "members-owned",
                name: "mob_list_members",
                args: &members_args,
            })
            .await
            .expect_err("owned mobs still require reinjected exact scope");
        assert!(matches!(members_error, ToolError::AccessDenied { .. }));
    }

    #[tokio::test]
    async fn test_build_mob_tools_does_not_mutate_implicit_mob_and_surface_reconciles_on_demand() {
        let state = MobMcpState::new_in_memory();
        let factory = AgentMobToolSurfaceFactory::new(Arc::clone(&state));
        let session_id = SessionId::new();
        let session_key = session_id.to_string();
        let stale_mob_id = state
            .get_or_create_implicit_mob(&session_key, "claude-sonnet-4-5")
            .await
            .expect("create stale implicit mob");

        let _dispatcher = factory
            .build_mob_tools(meerkat_core::service::MobToolsBuildArgs {
                session_id,
                model: "gpt-5.4".to_string(),
                authority_context: Some(create_only_authority()),
                comms_name: None,
                comms_runtime: None,
            })
            .await
            .expect("build_mob_tools");

        assert_eq!(
            state.find_implicit_mob(&session_key).await,
            Some(stale_mob_id.clone()),
            "surface building must not own implicit-mob reconciliation"
        );
        let stale_handle = state
            .handle_for(&stale_mob_id)
            .await
            .expect("stale implicit mob should still exist after build");
        assert_eq!(
            stale_handle
                .definition()
                .profiles
                .get(&ProfileName::from("delegate"))
                .expect("delegate profile")
                .model,
            "claude-sonnet-4-5"
        );

        let surface = AgentMobToolSurface::new(
            Arc::clone(&state),
            Some(stale_mob_id.clone()),
            create_only_authority(),
            "gpt-5.4".to_string(),
            SessionId::parse(&session_key).expect("session_id"),
            None,
            None,
            None,
        );
        let (reconciled_mob_id, created) = surface
            .ensure_implicit_mob()
            .await
            .expect("surface should reconcile the implicit mob on demand");

        assert!(
            created,
            "on-demand surface reconciliation should report a fresh implicit mob when the model changes"
        );
        assert_eq!(
            reconciled_mob_id, stale_mob_id,
            "implicit mob ids stay stable while the runtime refreshes their definition"
        );
        let reconciled_handle = state
            .handle_for(&reconciled_mob_id)
            .await
            .expect("reconciled implicit mob must exist");
        assert_eq!(
            reconciled_handle
                .definition()
                .profiles
                .get(&ProfileName::from("delegate"))
                .expect("delegate profile")
                .model,
            "gpt-5.4"
        );
    }

    #[tokio::test]
    async fn test_mob_list_empty() {
        let state = MobMcpState::new_in_memory();
        let surface = AgentMobToolSurface::new(
            state,
            None,
            create_only_authority(),
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
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

    #[tokio::test]
    async fn test_create_only_authority_grants_exact_scope_for_new_explicit_mob() {
        let state = MobMcpState::new_in_memory();
        let session_id = SessionId::new();
        let expected_session_id = session_id.to_string();
        let surface = AgentMobToolSurface::new(
            Arc::clone(&state),
            None,
            create_only_authority(),
            "claude-sonnet-4-5".to_string(),
            session_id,
            None,
            None,
            None,
        );

        let create_args = serde_json::value::RawValue::from_string(
            json!({
                "definition": {
                    "id": "created-by-create-only",
                    "profiles": {
                        "worker": {
                            "model": "claude-sonnet-4-5",
                            "tools": { "comms": true },
                            "peer_description": "worker",
                            "runtime_mode": "turn_driven"
                        }
                    },
                    "owner_session_id": "spoofed-session",
                    "is_implicit": true,
                    "session_cleanup_policy": "manual"
                }
            })
            .to_string(),
        )
        .unwrap();
        let create_result = surface
            .dispatch(ToolCallView {
                id: "create-1",
                name: "mob_create",
                args: &create_args,
            })
            .await
            .expect("create-only authority should allow mob_create");
        let created: serde_json::Value =
            serde_json::from_str(&create_result.result.text_content()).unwrap();
        let mob_id = created["mob_id"].as_str().expect("mob_id").to_string();

        assert!(
            state
                .handle_for(&MobId::from(mob_id.as_str()))
                .await
                .is_ok(),
            "mob_create should still create the mob"
        );
        let created_handle = state
            .handle_for(&MobId::from(mob_id.as_str()))
            .await
            .expect("created mob handle");
        let created_definition = created_handle.definition();
        assert_eq!(
            created_definition.owner_session_id.as_deref(),
            Some(expected_session_id.as_str()),
            "mob_create must rebind session indexing to the current owner session"
        );
        assert_eq!(
            created_definition.session_cleanup_policy,
            meerkat_mob::definition::SessionCleanupPolicy::DestroyOnOwnerArchive,
            "mob_create must set explicit session-scoped cleanup truth"
        );
        assert!(
            !created_definition.is_implicit,
            "mob_create must not allow callers to mint faux implicit mobs"
        );

        let list_args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let list_result = surface
            .dispatch(ToolCallView {
                id: "list-1",
                name: "mob_list",
                args: &list_args,
            })
            .await
            .expect("mob_list should still be callable");
        let listed: serde_json::Value =
            serde_json::from_str(&list_result.result.text_content()).unwrap();
        assert_eq!(
            listed["mobs"].as_array().map(Vec::len),
            Some(1),
            "mob_create should grant exact scope for the new explicit mob"
        );
        assert_eq!(listed["mobs"][0]["mob_id"], json!(mob_id));

        let destroy_args =
            serde_json::value::RawValue::from_string(json!({ "mob_id": mob_id }).to_string())
                .unwrap();
        let destroy_result = surface
            .dispatch(ToolCallView {
                id: "destroy-1",
                name: "mob_destroy",
                args: &destroy_args,
            })
            .await
            .expect("newly-created explicit mob should be immediately manageable");
        let destroyed: serde_json::Value =
            serde_json::from_str(&destroy_result.result.text_content()).unwrap();
        assert_eq!(destroyed, json!({ "ok": true }));
    }

    #[tokio::test]
    async fn test_scope_only_authority_denies_delegate_but_allows_in_scope_operator_reads() {
        let state = MobMcpState::new_in_memory();
        let mob_id = state
            .mob_create_definition(sample_definition("scope-only-mob"))
            .await
            .expect("create scope-only mob");
        let surface = AgentMobToolSurface::new(
            Arc::clone(&state),
            None,
            scope_only_authority(mob_id.as_str()),
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
            None,
            None,
        );

        let delegate_args =
            serde_json::value::RawValue::from_string(json!({ "task": "say hi" }).to_string())
                .unwrap();
        let delegate_error = surface
            .dispatch(ToolCallView {
                id: "delegate-1",
                name: "delegate",
                args: &delegate_args,
            })
            .await
            .expect_err("scope-only authority must deny delegate");
        assert!(matches!(delegate_error, ToolError::AccessDenied { .. }));

        let list_members_args =
            serde_json::value::RawValue::from_string(json!({ "mob_id": mob_id }).to_string())
                .unwrap();
        let list_members_result = surface
            .dispatch(ToolCallView {
                id: "members-1",
                name: "mob_list_members",
                args: &list_members_args,
            })
            .await
            .expect("in-scope operator read should succeed");
        let listed: serde_json::Value =
            serde_json::from_str(&list_members_result.result.text_content()).unwrap();
        assert_eq!(listed["members"], json!([]));
    }

    #[tokio::test]
    async fn test_successful_create_persists_operator_provenance_projection() {
        let state = MobMcpState::new_in_memory();
        let surface = AgentMobToolSurface::new(
            Arc::clone(&state),
            None,
            create_only_authority_with_provenance(),
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
            None,
            None,
        );

        let create_args = serde_json::value::RawValue::from_string(
            json!({
                "definition": sample_definition("provenance-create")
            })
            .to_string(),
        )
        .unwrap();
        let result = surface
            .dispatch(ToolCallView {
                id: "create-provenance",
                name: "mob_create",
                args: &create_args,
            })
            .await
            .expect("mob_create should succeed");
        let payload: serde_json::Value =
            serde_json::from_str(&result.result.text_content()).unwrap();
        let mob_id = MobId::from(payload["mob_id"].as_str().expect("mob_id"));

        let handle = state.handle_for(&mob_id).await.expect("mob handle");
        let events = handle.events().replay_all().await.expect("replay events");
        let audit_event = events
            .into_iter()
            .find_map(|event| match event.kind {
                meerkat_mob::MobEventKind::OperatorActionRecorded {
                    tool_name,
                    principal_token,
                    caller_provenance,
                    audit_invocation_id,
                } => Some((
                    tool_name,
                    principal_token,
                    caller_provenance,
                    audit_invocation_id,
                )),
                _ => None,
            })
            .expect("operator action event");

        assert_eq!(audit_event.0, "mob_create");
        assert_eq!(audit_event.1.as_str(), "create-only");
        assert_eq!(audit_event.3.as_deref(), Some("audit-create"));
        assert_eq!(
            audit_event
                .2
                .as_ref()
                .and_then(|provenance| provenance.caller_member_id()),
            Some("lead-1")
        );
    }

    #[tokio::test]
    async fn test_successful_in_scope_mutation_persists_provenance_and_denied_calls_do_not() {
        let state = MobMcpState::new_in_memory();
        let mob_id = state
            .mob_create_definition(sample_definition("provenance-scope"))
            .await
            .expect("create mob");
        let authority =
            MobToolAuthorityContext::new(OpaquePrincipalToken::new("scope-principal"), false)
                .with_managed_mob_scope([mob_id.to_string()])
                .with_audit_invocation_id("audit-scope");
        let surface = AgentMobToolSurface::new(
            Arc::clone(&state),
            None,
            authority,
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
            None,
            None,
        );

        let delegate_args = serde_json::value::RawValue::from_string(
            json!({ "task": "denied delegate" }).to_string(),
        )
        .unwrap();
        let delegate_error = surface
            .dispatch(ToolCallView {
                id: "delegate-denied",
                name: "delegate",
                args: &delegate_args,
            })
            .await
            .expect_err("delegate should be denied without create authority");
        assert!(matches!(delegate_error, ToolError::AccessDenied { .. }));

        let spawn_args = serde_json::value::RawValue::from_string(
            json!({
                "mob_id": mob_id,
                "profile": "worker",
                "member_id": "w-1"
            })
            .to_string(),
        )
        .unwrap();
        surface
            .dispatch(ToolCallView {
                id: "spawn-scope",
                name: "mob_spawn_member",
                args: &spawn_args,
            })
            .await
            .expect("in-scope spawn should succeed");

        let handle = state.handle_for(&mob_id).await.expect("handle");
        let audit_events = handle
            .events()
            .replay_all()
            .await
            .expect("replay events")
            .into_iter()
            .filter_map(|event| match event.kind {
                meerkat_mob::MobEventKind::OperatorActionRecorded {
                    tool_name,
                    principal_token,
                    audit_invocation_id,
                    ..
                } => Some((tool_name, principal_token, audit_invocation_id)),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            audit_events.len(),
            1,
            "denied calls must not persist provenance"
        );
        assert_eq!(audit_events[0].0, "mob_spawn_member");
        assert_eq!(audit_events[0].1.as_str(), "scope-principal");
        assert_eq!(audit_events[0].2.as_deref(), Some("audit-scope"));
    }
}
