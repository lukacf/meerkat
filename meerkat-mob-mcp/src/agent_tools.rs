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
use meerkat_core::types::{
    ContentInput, SessionId, ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind,
};
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
use meerkat_core::comms::{CommsCommand, PeerName};

// ─── Tool name constants ─────────────────────────────────────────────────

const TOOL_DELEGATE: &str = "delegate";
const TOOL_MOB_CREATE: &str = "mob_create";
const TOOL_MOB_DESTROY: &str = "mob_destroy";
const TOOL_MOB_SPAWN_MEMBER: &str = "mob_spawn_member";
const TOOL_MOB_RETIRE_MEMBER: &str = "mob_retire_member";
const TOOL_MOB_CHECK_MEMBER: &str = "mob_check_member";
const TOOL_MOB_LIST_MEMBERS: &str = "mob_list_members";
const TOOL_MOB_LIST: &str = "mob_list";
const TOOL_MOB_PROFILE_CREATE: &str = "mob_profile_create";
const TOOL_MOB_PROFILE_GET: &str = "mob_profile_get";
const TOOL_MOB_PROFILE_LIST: &str = "mob_profile_list";
const TOOL_MOB_PROFILE_UPDATE: &str = "mob_profile_update";
const TOOL_MOB_PROFILE_DELETE: &str = "mob_profile_delete";
const TOOL_MOB_PROFILE_LIST_SOURCES: &str = "mob_profile_list_sources";

// ─── ResolvedSpawnTooling ────────────────────────────────────────────────

/// Result of resolving `SpawnTooling` into concrete values for spawning.
#[derive(Debug, Clone)]
pub struct ResolvedSpawnTooling {
    /// Inherited tool filter for the child session (from overlays or inherit mode).
    pub inherited_tool_filter: Option<meerkat_core::tool_scope::ToolFilter>,
    /// Override profile resolved from `SpawnTooling::Profile` source.
    /// When set, the spawn path uses this profile instead of the definition's.
    pub override_profile: Option<meerkat_mob::Profile>,
}

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
    /// Effective mob authority — shared handle owned by the agent/turn executor.
    /// Mob tools read from this for authorization. The agent is the sole writer
    /// (via apply_session_effects). Falls back to a local RwLock when no shared
    /// handle is provided (non-runtime test paths).
    effective_authority: Arc<std::sync::RwLock<MobToolAuthorityContext>>,
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
    /// Context for capturing a parent agent's tool scope snapshot.
    snapshot_context: meerkat_core::service::MobToolSnapshotContext,
}

impl AgentMobToolSurface {
    fn synthetic_parent_peer_added_fields(parent_name: &str) -> (String, String, String) {
        let mut parts = parent_name.split('/');
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(_mob_id), Some(role), Some(meerkat_id), None) => (
                meerkat_id.to_string(),
                role.to_string(),
                format!("peer {role}"),
            ),
            _ => (
                parent_name.to_string(),
                "external".to_string(),
                "external peer".to_string(),
            ),
        }
    }

    async fn notify_peer_added(
        sender: &Arc<dyn meerkat_core::agent::CommsRuntime>,
        recipient_comms_name: &str,
        peer: &str,
        role: &str,
        description: &str,
    ) -> bool {
        let Ok(to) = PeerName::new(recipient_comms_name) else {
            return false;
        };
        sender
            .send(CommsCommand::PeerRequest {
                to,
                intent: "mob.peer_added".to_string(),
                params: serde_json::json!({
                    "peer": peer,
                    "role": role,
                    "description": description,
                }),
                handling_mode: meerkat_core::types::HandlingMode::Queue,
            })
            .await
            .is_ok()
    }

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
        Self::new_with_effective_authority(
            state,
            implicit_mob_id,
            Arc::new(std::sync::RwLock::new(authority_context)),
            model,
            owner_session_id,
            comms_name,
            comms_peer_id,
            comms_runtime,
            meerkat_core::service::MobToolSnapshotContext::Standalone,
        )
    }

    /// Create with a shared effective authority handle.
    ///
    /// The handle is owned by the agent and updated via `apply_session_effects`.
    /// Mob tools read from it for authorization checks.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_effective_authority(
        state: Arc<MobMcpState>,
        implicit_mob_id: Option<MobId>,
        effective_authority: Arc<std::sync::RwLock<MobToolAuthorityContext>>,
        model: String,
        owner_session_id: SessionId,
        comms_name: Option<String>,
        comms_peer_id: Option<String>,
        comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
        snapshot_context: meerkat_core::service::MobToolSnapshotContext,
    ) -> Self {
        let has_profile_store = state.realm_profile_store().is_some();
        let has_snapshot_provider = matches!(
            &snapshot_context,
            meerkat_core::service::MobToolSnapshotContext::ParentOwned(_)
        );
        let tools = build_tool_defs_with_profile_support(has_profile_store, has_snapshot_provider);
        Self {
            state,
            cached_implicit_mob_id: RwLock::new(implicit_mob_id),
            effective_authority,
            tools,
            owner_session_id,
            model,
            comms_name,
            comms_peer_id,
            comms_runtime,
            snapshot_context,
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
        Self::encode_result_with_effects(call, value, vec![])
    }

    fn encode_result_with_effects(
        call: ToolCallView<'_>,
        value: serde_json::Value,
        session_effects: Vec<meerkat_core::SessionEffect>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let content = serde_json::to_string(&value)
            .map_err(|e| ToolError::execution_failed(format!("encode tool result: {e}")))?;
        Ok(meerkat_core::ToolDispatchOutcome {
            result: ToolResult::new(call.id.to_string(), content, false),
            async_ops: vec![],
            session_effects,
        })
    }

    fn map_mob_error(call: ToolCallView<'_>, error: MobError) -> ToolError {
        ToolError::execution_failed(format!("tool '{}' failed: {error}", call.name))
    }

    fn authority_context_snapshot(&self) -> MobToolAuthorityContext {
        self.effective_authority
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn ensure_create_authority(&self, tool_name: &str) -> Result<(), ToolError> {
        if self.authority_context_snapshot().can_create_mobs() {
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
            .authority_context_snapshot()
            .can_manage_mob(mob_id.as_str())
        {
            return Ok(());
        }
        Err(ToolError::access_denied(tool_name))
    }

    /// Resolve spawn tooling into inherited tool filter and optional override profile.
    ///
    /// - `InheritParent`: snapshot parent's visible tools, apply overlays
    /// - `Minimal`: only comms tools (send, send_message, send_request, send_response, peers)
    /// - `Profile`: resolve the profile from inline/realm source and apply overlays
    async fn resolve_spawn_tooling(
        &self,
        tooling: &meerkat_mob::SpawnTooling,
    ) -> Result<ResolvedSpawnTooling, ToolError> {
        match tooling {
            meerkat_mob::SpawnTooling::InheritParent {
                allow_overlay,
                deny_overlay,
            } => {
                let provider = match &self.snapshot_context {
                    meerkat_core::service::MobToolSnapshotContext::ParentOwned(p) => p,
                    meerkat_core::service::MobToolSnapshotContext::Standalone => {
                        return Err(ToolError::execution_failed(
                            "InheritParent tooling requires a parent tool scope (ParentOwned context), \
                             but this agent is running in Standalone mode",
                        ));
                    }
                };
                let tools = provider.snapshot_visible_tools();
                let snapshot = meerkat_mob::snapshot::ParentToolScopeSnapshot::from_tools(&tools);
                let allow_set = allow_overlay.as_ref().map(|v| {
                    v.iter()
                        .cloned()
                        .collect::<std::collections::HashSet<String>>()
                });
                let deny_set = deny_overlay.as_ref().map(|v| {
                    v.iter()
                        .cloned()
                        .collect::<std::collections::HashSet<String>>()
                });
                let filter = snapshot.with_overlays(allow_set.as_ref(), deny_set.as_ref());
                Ok(ResolvedSpawnTooling {
                    inherited_tool_filter: Some(filter),
                    override_profile: None,
                })
            }
            meerkat_mob::SpawnTooling::Minimal => {
                match &self.snapshot_context {
                    meerkat_core::service::MobToolSnapshotContext::ParentOwned(_) => {}
                    meerkat_core::service::MobToolSnapshotContext::Standalone => {
                        return Err(ToolError::execution_failed(
                            "Minimal tooling requires a parent tool scope (ParentOwned context), \
                             but this agent is running in Standalone mode",
                        ));
                    }
                }
                let comms_tools: std::collections::HashSet<String> = [
                    "send",
                    "send_message",
                    "send_request",
                    "send_response",
                    "peers",
                ]
                .iter()
                .map(ToString::to_string)
                .collect();
                Ok(ResolvedSpawnTooling {
                    inherited_tool_filter: Some(meerkat_core::tool_scope::ToolFilter::Allow(
                        comms_tools,
                    )),
                    override_profile: None,
                })
            }
            meerkat_mob::SpawnTooling::Profile {
                source,
                allow_overlay,
                deny_overlay,
            } => {
                // Profile mode: resolve the profile from inline or realm source.
                let resolved_profile = match source.as_ref() {
                    meerkat_mob::ProfileSource::Inline(profile) => profile.clone(),
                    meerkat_mob::ProfileSource::RealmProfile { name } => {
                        let store = self
                            .state
                            .realm_profile_store()
                            .ok_or_else(|| {
                                ToolError::execution_failed(
                                    "Profile tooling with RealmProfile source requires a realm profile store",
                                )
                            })?;
                        store
                            .get(name)
                            .await
                            .map_err(|e| {
                                ToolError::execution_failed(format!(
                                    "failed to resolve realm profile '{name}': {e}"
                                ))
                            })?
                            .ok_or_else(|| {
                                ToolError::execution_failed(format!(
                                    "realm profile '{name}' not found"
                                ))
                            })?
                            .profile
                    }
                };

                // The profile's ToolConfig controls categories (builtins,
                // shell, etc.) through build_agent_config(). Overlays become the
                // inherited filter on session metadata.
                let inherited_tool_filter = if allow_overlay.is_none() && deny_overlay.is_none() {
                    None
                } else {
                    // When overlays are present but we need a base set from the parent
                    // to apply them against, require ParentOwned.
                    let provider = match &self.snapshot_context {
                        meerkat_core::service::MobToolSnapshotContext::ParentOwned(p) => p,
                        meerkat_core::service::MobToolSnapshotContext::Standalone => {
                            return Err(ToolError::execution_failed(
                                "Profile tooling with overlays requires a parent tool scope",
                            ));
                        }
                    };
                    let tools = provider.snapshot_visible_tools();
                    let snapshot =
                        meerkat_mob::snapshot::ParentToolScopeSnapshot::from_tools(&tools);
                    let allow_set = allow_overlay.as_ref().map(|v| {
                        v.iter()
                            .cloned()
                            .collect::<std::collections::HashSet<String>>()
                    });
                    let deny_set = deny_overlay.as_ref().map(|v| {
                        v.iter()
                            .cloned()
                            .collect::<std::collections::HashSet<String>>()
                    });
                    Some(snapshot.with_overlays(allow_set.as_ref(), deny_set.as_ref()))
                };

                Ok(ResolvedSpawnTooling {
                    inherited_tool_filter,
                    override_profile: Some(resolved_profile),
                })
            }
        }
    }

    async fn record_successful_operator_action(
        &self,
        handle: &meerkat_mob::MobHandle,
        tool_name: &str,
    ) {
        let authority_context = self.authority_context_snapshot();
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

    // grant_exact_mob_scope_after_create removed — mob authority grants are
    // now returned as typed SessionEffect::GrantManageMob effects. The turn
    // owner (agent loop) merges and commits them to session build_state.
    // No re-entrant session service call from inside tool dispatch.

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

    async fn wire_delegate_helper_to_creator(
        &self,
        mob_id: &MobId,
        meerkat_id: &MeerkatId,
    ) -> bool {
        let Some(name) = self.comms_name.as_ref() else {
            return false;
        };
        let Some(peer_id) = self.comms_peer_id.as_ref() else {
            return false;
        };
        let Some(comms_rt) = self.comms_runtime.as_ref() else {
            return false;
        };

        let Ok(parent_spec) = meerkat_core::comms::TrustedPeerSpec::new(
            name.as_str(),
            peer_id.as_str(),
            format!("inproc://{name}"),
        ) else {
            return false;
        };

        let helper_trusts_parent = self
            .state
            .mob_wire(
                mob_id,
                meerkat_id.clone(),
                meerkat_mob::PeerTarget::External(parent_spec),
            )
            .await
            .is_ok();
        if !helper_trusts_parent {
            return false;
        }

        let Ok(handle) = self.state.handle_for(mob_id).await else {
            return false;
        };
        let roster = handle.roster().await;
        let Some(entry) = roster.get(meerkat_id) else {
            return false;
        };
        let Some(helper_peer_id) = entry.peer_id.as_ref() else {
            return false;
        };
        let helper_comms_name = format!("{}/{}/{}", mob_id, entry.profile, meerkat_id);
        if helper_comms_name == *name {
            return false;
        }
        let Ok(helper_spec) = meerkat_core::comms::TrustedPeerSpec::new(
            &helper_comms_name,
            helper_peer_id.as_str(),
            format!("inproc://{helper_comms_name}"),
        ) else {
            return false;
        };

        if comms_rt.add_trusted_peer(helper_spec).await.is_err() {
            return false;
        }

        let peer_description = handle
            .definition()
            .resolve_inline_profile(&entry.profile)
            .map(|profile| profile.peer_description.as_str())
            .unwrap_or("delegate helper");
        let Some(helper_session_id) = entry.member_ref.session_id() else {
            return false;
        };
        let helper_runtime = meerkat_core::service::SessionServiceCommsExt::comms_runtime(
            self.state.session_service().as_ref(),
            helper_session_id,
        )
        .await;
        let Some(helper_runtime) = helper_runtime else {
            return false;
        };

        let notify_parent = Self::notify_peer_added(
            &helper_runtime,
            name,
            meerkat_id.as_str(),
            entry.profile.as_str(),
            peer_description,
        )
        .await;
        let (parent_peer, parent_role, parent_description) =
            Self::synthetic_parent_peer_added_fields(name);
        let notify_helper = Self::notify_peer_added(
            comms_rt,
            &helper_comms_name,
            &parent_peer,
            &parent_role,
            &parent_description,
        )
        .await;

        notify_parent && notify_helper
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

        // Authority grant is returned as a typed effect for the turn owner
        // to merge and commit — no re-entrant session service call.
        // Emit the grant whenever the mob isn't already in scope, not just
        // on first_delegate — a prior failed delegate may have created the
        // implicit mob without the grant effect being applied.
        let mut session_effects = Vec::new();
        if !self
            .authority_context_snapshot()
            .can_manage_mob(mob_id.as_str())
        {
            session_effects.push(meerkat_core::SessionEffect::GrantManageMob {
                mob_id: mob_id.to_string(),
            });
        }

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

        // Resolve spawn tooling: default to InheritParent for delegates
        let tooling = args
            .tooling
            .unwrap_or(meerkat_mob::SpawnTooling::InheritParent {
                allow_overlay: None,
                deny_overlay: None,
            });
        let resolved = self.resolve_spawn_tooling(&tooling).await?;
        spec.inherited_tool_filter = resolved.inherited_tool_filter;
        spec.override_profile = resolved.override_profile;

        // Spawn via MobMcpState
        let member_ref = self
            .state
            .mob_spawn_spec(&mob_id, spec)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        // Bidirectional comms wiring:
        // 1. Wire helper → parent: helper trusts parent as external peer
        // 2. Wire parent → helper: parent trusts helper so it can receive messages
        let wired = self
            .wire_delegate_helper_to_creator(&mob_id, &meerkat_id)
            .await;

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

        Self::encode_result_with_effects(call, result, session_effects)
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

        // Authority grant as typed effect — no re-entrant session service call.
        let session_effects = vec![meerkat_core::SessionEffect::GrantManageMob {
            mob_id: mob_id.to_string(),
        }];

        if let Ok(handle) = self.state.handle_for(&mob_id).await {
            self.record_successful_operator_action(&handle, call.name)
                .await;
        }

        Self::encode_result_with_effects(call, json!({"mob_id": mob_id}), session_effects)
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
        if let Some(tooling) = args.tooling {
            let resolved = self.resolve_spawn_tooling(&tooling).await?;
            spec.inherited_tool_filter = resolved.inherited_tool_filter;
            spec.override_profile = resolved.override_profile;
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
        let authority_context = self.authority_context_snapshot();
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
    // ─── Profile CRUD dispatch ────────────────────────────────────────

    async fn dispatch_mob_profile_create(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: ProfileCreateArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
        let stored = self
            .state
            .realm_profile_create(&args.name, &args.profile)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;
        Self::encode_result(call, json!(stored))
    }

    async fn dispatch_mob_profile_get(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: ProfileNameArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
        let stored = self
            .state
            .realm_profile_get(&args.name)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;
        match stored {
            Some(profile) => Self::encode_result(call, json!(profile)),
            None => Self::encode_result(call, json!({"not_found": true, "name": args.name})),
        }
    }

    async fn dispatch_mob_profile_list(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let profiles = self
            .state
            .realm_profile_list()
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;
        Self::encode_result(call, json!({"profiles": profiles}))
    }

    async fn dispatch_mob_profile_update(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: ProfileUpdateArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
        let stored = self
            .state
            .realm_profile_update(&args.name, &args.profile, args.expected_revision)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;
        Self::encode_result(call, json!(stored))
    }

    async fn dispatch_mob_profile_delete(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: ProfileDeleteArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
        let deleted = self
            .state
            .realm_profile_delete(&args.name, args.expected_revision)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;
        Self::encode_result(
            call,
            json!({"name": deleted.name, "deleted_revision": deleted.revision}),
        )
    }

    async fn dispatch_mob_profile_list_sources(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let provider = match &self.snapshot_context {
            meerkat_core::service::MobToolSnapshotContext::ParentOwned(p) => p,
            meerkat_core::service::MobToolSnapshotContext::Standalone => {
                return Err(ToolError::not_found(call.name));
            }
        };
        let tools = provider.snapshot_visible_tools();
        let mut groups: std::collections::BTreeMap<(String, String), Vec<String>> =
            std::collections::BTreeMap::new();
        for tool in &tools {
            let (kind, source_id) = match &tool.provenance {
                Some(p) => {
                    let kind_str = serde_json::to_value(&p.kind)
                        .ok()
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_else(|| format!("{:?}", p.kind));
                    (kind_str, p.source_id.clone())
                }
                None => ("unknown".to_string(), "unknown".to_string()),
            };
            groups
                .entry((kind, source_id))
                .or_default()
                .push(tool.name.clone());
        }
        let sources: Vec<serde_json::Value> = groups
            .into_iter()
            .map(|((kind, source_id), tool_names)| {
                json!({
                    "kind": kind,
                    "source_id": source_id,
                    "tool_names": tool_names,
                })
            })
            .collect();
        Self::encode_result(call, json!({"sources": sources}))
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
        // Use the shared effective-authority handle if provided (runtime-backed
        // sessions). The agent/turn owner updates this handle via
        // apply_session_effects; mob tools read from it for authorization.
        // Falls back to a local handle for non-runtime paths.
        let effective_authority_handle = args
            .effective_authority
            .unwrap_or_else(|| Arc::new(std::sync::RwLock::new(authority_context)));
        let surface = AgentMobToolSurface::new_with_effective_authority(
            Arc::clone(&self.state),
            implicit_mob_id,
            effective_authority_handle,
            args.model,
            args.session_id,
            args.comms_name,
            comms_peer_id,
            args.comms_runtime,
            args.snapshot_context,
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
            TOOL_MOB_PROFILE_CREATE => self.dispatch_mob_profile_create(call).await,
            TOOL_MOB_PROFILE_GET => self.dispatch_mob_profile_get(call).await,
            TOOL_MOB_PROFILE_LIST => self.dispatch_mob_profile_list(call).await,
            TOOL_MOB_PROFILE_UPDATE => self.dispatch_mob_profile_update(call).await,
            TOOL_MOB_PROFILE_DELETE => self.dispatch_mob_profile_delete(call).await,
            TOOL_MOB_PROFILE_LIST_SOURCES => self.dispatch_mob_profile_list_sources(call).await,
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
        provenance: Some(ToolProvenance {
            kind: ToolSourceKind::Mob,
            source_id: "mob".into(),
        }),
    })
}

#[cfg(test)]
fn build_tool_defs() -> Arc<[Arc<ToolDef>]> {
    build_tool_defs_with_profile_support(false, false)
}

fn build_tool_defs_with_profile_support(
    has_profile_store: bool,
    has_snapshot_provider: bool,
) -> Arc<[Arc<ToolDef>]> {
    let mut defs = vec![
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
    ];

    if has_profile_store {
        defs.push(tool_def(
            TOOL_MOB_PROFILE_CREATE,
            "Create a new realm profile for spawning mob members.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Unique profile name"},
                    "profile": {"type": "object", "description": "Profile definition (model, skills, tools, etc.)"}
                },
                "required": ["name", "profile"]
            }),
        ));
        defs.push(tool_def(
            TOOL_MOB_PROFILE_GET,
            "Get a realm profile by name.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Profile name to retrieve"}
                },
                "required": ["name"]
            }),
        ));
        defs.push(tool_def(
            TOOL_MOB_PROFILE_LIST,
            "List all realm profiles.",
            json!({
                "type": "object",
                "properties": {}
            }),
        ));
        defs.push(tool_def(
            TOOL_MOB_PROFILE_UPDATE,
            "Update a realm profile with CAS revision check.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Profile name to update"},
                    "profile": {"type": "object", "description": "Updated profile definition"},
                    "expected_revision": {"type": "integer", "description": "Expected current revision for CAS"}
                },
                "required": ["name", "profile", "expected_revision"]
            }),
        ));
        defs.push(tool_def(
            TOOL_MOB_PROFILE_DELETE,
            "Delete a realm profile.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Profile name to delete"},
                    "expected_revision": {"type": "integer", "description": "Expected current revision for CAS"}
                },
                "required": ["name", "expected_revision"]
            }),
        ));
    }

    if has_profile_store && has_snapshot_provider {
        defs.push(tool_def(
            TOOL_MOB_PROFILE_LIST_SOURCES,
            "List visible tool sources grouped by provenance (kind and source).",
            json!({
                "type": "object",
                "properties": {}
            }),
        ));
    }

    defs.into()
}

// ─── Argument types ──────────────────────────────────────────────────────

#[derive(Deserialize)]
struct DelegateArgs {
    task: String,
    #[serde(default)]
    member_id: Option<String>,
    #[serde(default)]
    additional_instructions: Option<String>,
    #[serde(default)]
    tooling: Option<meerkat_mob::SpawnTooling>,
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
    #[serde(default)]
    tooling: Option<meerkat_mob::SpawnTooling>,
}

#[derive(Deserialize)]
struct MemberArgs {
    mob_id: String,
    member_id: String,
}

#[derive(Deserialize)]
struct ProfileCreateArgs {
    name: String,
    profile: meerkat_mob::Profile,
}

#[derive(Deserialize)]
struct ProfileNameArgs {
    name: String,
}

#[derive(Deserialize)]
struct ProfileUpdateArgs {
    name: String,
    profile: meerkat_mob::Profile,
    expected_revision: u64,
}

#[derive(Deserialize)]
struct ProfileDeleteArgs {
    name: String,
    expected_revision: u64,
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
    use async_trait::async_trait;
    use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
    use meerkat_core::comms::{
        CommsCommand, PeerDirectoryEntry, PeerDirectorySource, PeerReachability, SendError,
        SendReceipt, TrustedPeerSpec,
    };
    use meerkat_core::event::AgentEvent;
    use meerkat_core::event_injector::{InteractionSubscription, SubscribableInjector};
    use meerkat_core::interaction::{InboxInteraction, InteractionContent, InteractionId};
    use meerkat_core::service::{
        AppendSystemContextRequest, AppendSystemContextResult, MobToolAuthorityContext,
        MobToolSnapshotContext, MobToolsFactory, OpaquePrincipalToken, SessionControlError,
        SessionHistoryPage, SessionHistoryQuery, SessionInfo, SessionQuery, SessionServiceCommsExt,
        SessionServiceControlExt, SessionServiceHistoryExt, SessionSummary, SessionUsage,
        SessionView, StartTurnRequest, VisibleToolSnapshotProvider,
    };
    use meerkat_core::time_compat::SystemTime;
    use meerkat_core::types::{ContentInput, HandlingMode, RenderMetadata, RunResult, Usage};
    use meerkat_core::{
        AppendSystemContextStatus, EventInjector, EventStream, Provider, StreamError,
    };
    use meerkat_core::{EventInjectorError, PlainEventSource};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Default)]
    struct TestCommsRegistry {
        runtimes: tokio::sync::RwLock<HashMap<String, Arc<TestCommsRuntime>>>,
    }

    struct TestInjector;

    impl meerkat_core::EventInjector for TestInjector {
        fn inject(
            &self,
            _body: ContentInput,
            _source: PlainEventSource,
            _handling_mode: HandlingMode,
            _render_metadata: Option<RenderMetadata>,
        ) -> Result<(), EventInjectorError> {
            Ok(())
        }
    }

    impl SubscribableInjector for TestInjector {
        fn inject_with_subscription(
            &self,
            body: ContentInput,
            source: PlainEventSource,
            handling_mode: HandlingMode,
            render_metadata: Option<RenderMetadata>,
        ) -> Result<InteractionSubscription, EventInjectorError> {
            self.inject(body, source, handling_mode, render_metadata)?;
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            let interaction_id = InteractionId(uuid::Uuid::new_v4());
            let interaction_id_for_task = interaction_id;
            tokio::spawn(async move {
                let _ = tx
                    .send(AgentEvent::InteractionComplete {
                        interaction_id: interaction_id_for_task,
                        result: "ok".to_string(),
                    })
                    .await;
            });
            Ok(InteractionSubscription {
                id: interaction_id,
                events: rx,
            })
        }
    }

    impl TestCommsRegistry {
        async fn insert(&self, runtime: Arc<TestCommsRuntime>) {
            self.runtimes
                .write()
                .await
                .insert(runtime.name.clone(), runtime);
        }

        async fn get(&self, name: &str) -> Option<Arc<TestCommsRuntime>> {
            self.runtimes.read().await.get(name).cloned()
        }
    }

    struct TestCommsRuntime {
        name: String,
        key: String,
        trusted: tokio::sync::RwLock<HashMap<String, TrustedPeerSpec>>,
        inbox: tokio::sync::RwLock<Vec<InboxInteraction>>,
        notify: Arc<tokio::sync::Notify>,
        registry: Arc<TestCommsRegistry>,
    }

    impl TestCommsRuntime {
        async fn new(name: &str, registry: Arc<TestCommsRegistry>) -> Arc<Self> {
            let runtime = Arc::new(Self {
                name: name.to_string(),
                key: format!("ed25519:{name}"),
                trusted: tokio::sync::RwLock::new(HashMap::new()),
                inbox: tokio::sync::RwLock::new(Vec::new()),
                notify: Arc::new(tokio::sync::Notify::new()),
                registry,
            });
            runtime.registry.insert(runtime.clone()).await;
            runtime
        }
    }

    #[async_trait]
    impl CoreCommsRuntime for TestCommsRuntime {
        fn public_key(&self) -> Option<String> {
            Some(self.key.clone())
        }

        async fn add_trusted_peer(&self, peer: TrustedPeerSpec) -> Result<(), SendError> {
            self.trusted
                .write()
                .await
                .insert(peer.peer_id.clone(), peer);
            Ok(())
        }

        async fn remove_trusted_peer(&self, peer_id: &str) -> Result<bool, SendError> {
            Ok(self.trusted.write().await.remove(peer_id).is_some())
        }

        async fn send(&self, cmd: CommsCommand) -> Result<SendReceipt, SendError> {
            match cmd {
                CommsCommand::PeerRequest {
                    to,
                    intent,
                    params,
                    handling_mode: _,
                } => {
                    let trusted = self.trusted.read().await;
                    if !trusted.values().any(|peer| peer.name == to.as_str()) {
                        return Err(SendError::PeerNotFound(to.as_string()));
                    }
                    drop(trusted);
                    let recipient = self
                        .registry
                        .get(to.as_str())
                        .await
                        .ok_or_else(|| SendError::PeerNotFound(to.as_string()))?;
                    recipient.inbox.write().await.push(InboxInteraction {
                        id: InteractionId(uuid::Uuid::new_v4()),
                        from: self.name.clone(),
                        content: InteractionContent::Request { intent, params },
                        rendered_text: String::new(),
                        handling_mode: HandlingMode::Queue,
                        render_metadata: None,
                    });
                    recipient.notify.notify_waiters();
                    Ok(SendReceipt::PeerRequestSent {
                        request_id: InteractionId(uuid::Uuid::new_v4()),
                        envelope_id: uuid::Uuid::new_v4(),
                    })
                }
                unsupported => Err(SendError::Unsupported(format!(
                    "unsupported test comms command: {unsupported:?}"
                ))),
            }
        }

        async fn peers(&self) -> Vec<PeerDirectoryEntry> {
            self.trusted
                .read()
                .await
                .values()
                .filter_map(|peer| {
                    Some(PeerDirectoryEntry {
                        name: meerkat_core::comms::PeerName::new(&peer.name).ok()?,
                        peer_id: peer.peer_id.clone(),
                        address: peer.address.clone(),
                        source: PeerDirectorySource::Trusted,
                        sendable_kinds: vec!["peer_request".to_string()],
                        capabilities: serde_json::json!({}),
                        reachability: PeerReachability::Reachable,
                        last_unreachable_reason: None,
                        meta: Default::default(),
                    })
                })
                .collect()
        }

        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
            self.notify.clone()
        }

        async fn drain_peer_input_candidates(&self) -> Vec<meerkat_core::PeerInputCandidate> {
            self.drain_inbox_interactions()
                .await
                .into_iter()
                .map(|interaction| meerkat_core::PeerInputCandidate {
                    interaction,
                    class: meerkat_core::PeerInputClass::ActionableRequest,
                    lifecycle_peer: None,
                })
                .collect()
        }

        async fn drain_inbox_interactions(&self) -> Vec<InboxInteraction> {
            let mut inbox = self.inbox.write().await;
            std::mem::take(&mut *inbox)
        }
    }

    struct RealCommsSessionSvc {
        sessions: tokio::sync::RwLock<HashMap<SessionId, Arc<TestCommsRuntime>>>,
        counter: AtomicU64,
        runtime_adapter: Arc<meerkat_runtime::RuntimeSessionAdapter>,
        registry: Arc<TestCommsRegistry>,
        injector: Arc<TestInjector>,
    }

    impl RealCommsSessionSvc {
        fn new() -> Self {
            Self {
                sessions: tokio::sync::RwLock::new(HashMap::new()),
                counter: AtomicU64::new(0),
                runtime_adapter: Arc::new(meerkat_runtime::RuntimeSessionAdapter::ephemeral()),
                registry: Arc::new(TestCommsRegistry::default()),
                injector: Arc::new(TestInjector),
            }
        }

        async fn real_comms(&self, session_id: &SessionId) -> Option<Arc<TestCommsRuntime>> {
            self.sessions.read().await.get(session_id).cloned()
        }

        async fn register_external_comms(&self, name: &str) -> Arc<TestCommsRuntime> {
            TestCommsRuntime::new(name, Arc::clone(&self.registry)).await
        }
    }

    #[async_trait]
    impl SessionService for RealCommsSessionSvc {
        async fn create_session(
            &self,
            req: meerkat_core::service::CreateSessionRequest,
        ) -> Result<RunResult, SessionError> {
            let sid = SessionId::new();
            let n = self.counter.fetch_add(1, Ordering::Relaxed);
            let name = req
                .build
                .as_ref()
                .and_then(|b| b.comms_name.clone())
                .unwrap_or_else(|| format!("real-comms-session-{n}"));
            let comms = TestCommsRuntime::new(&name, Arc::clone(&self.registry)).await;
            self.sessions.write().await.insert(sid.clone(), comms);
            Ok(RunResult {
                text: "ok".to_string(),
                session_id: sid,
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                structured_output: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        async fn start_turn(
            &self,
            id: &SessionId,
            _req: StartTurnRequest,
        ) -> Result<RunResult, SessionError> {
            if !self.sessions.read().await.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            Ok(RunResult {
                text: "ok".to_string(),
                session_id: id.clone(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                structured_output: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }

        async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
            if !self.sessions.read().await.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            Ok(())
        }

        async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
            if !self.sessions.read().await.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            Ok(SessionView {
                state: SessionInfo {
                    session_id: id.clone(),
                    created_at: SystemTime::now(),
                    updated_at: SystemTime::now(),
                    message_count: 0,
                    is_active: true,
                    model: "claude-sonnet-4-5".to_string(),
                    provider: Provider::Anthropic,
                    last_assistant_text: None,
                    labels: Default::default(),
                },
                billing: SessionUsage {
                    total_tokens: 0,
                    usage: Usage::default(),
                },
            })
        }

        async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
            Ok(Vec::new())
        }

        async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
            let removed = self.sessions.write().await.remove(id).is_some();
            if removed {
                Ok(())
            } else {
                Err(SessionError::NotFound { id: id.clone() })
            }
        }
    }

    #[async_trait]
    impl SessionServiceCommsExt for RealCommsSessionSvc {
        async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
            self.sessions
                .read()
                .await
                .get(session_id)
                .map(|runtime| runtime.clone() as Arc<dyn CoreCommsRuntime>)
        }

        async fn event_injector(
            &self,
            _session_id: &SessionId,
        ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
            Some(self.injector.clone() as Arc<dyn meerkat_core::EventInjector>)
        }

        async fn interaction_event_injector(
            &self,
            _session_id: &SessionId,
        ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
            Some(self.injector.clone() as Arc<dyn SubscribableInjector>)
        }
    }

    #[async_trait]
    impl SessionServiceControlExt for RealCommsSessionSvc {
        async fn append_system_context(
            &self,
            id: &SessionId,
            _req: AppendSystemContextRequest,
        ) -> Result<AppendSystemContextResult, SessionControlError> {
            if !self.sessions.read().await.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() }.into());
            }
            Ok(AppendSystemContextResult {
                status: AppendSystemContextStatus::Staged,
            })
        }
    }

    #[async_trait]
    impl SessionServiceHistoryExt for RealCommsSessionSvc {
        async fn read_history(
            &self,
            id: &SessionId,
            query: SessionHistoryQuery,
        ) -> Result<SessionHistoryPage, SessionError> {
            if !self.sessions.read().await.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            Ok(SessionHistoryPage::from_messages(id.clone(), &[], query))
        }
    }

    #[async_trait]
    impl meerkat_mob::MobSessionService for RealCommsSessionSvc {
        async fn subscribe_session_events(
            &self,
            session_id: &SessionId,
        ) -> Result<EventStream, StreamError> {
            Err(StreamError::NotFound(format!("session {session_id}")))
        }

        fn supports_persistent_sessions(&self) -> bool {
            true
        }

        fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::RuntimeSessionAdapter>> {
            Some(self.runtime_adapter.clone())
        }

        async fn session_belongs_to_mob(&self, session_id: &SessionId, mob_id: &MobId) -> bool {
            self.sessions.read().await.contains_key(session_id) && !mob_id.as_str().is_empty()
        }

        async fn load_persisted_session(
            &self,
            _session_id: &SessionId,
        ) -> Result<Option<meerkat_core::Session>, SessionError> {
            Ok(None)
        }

        async fn apply_runtime_turn(
            &self,
            session_id: &SessionId,
            run_id: meerkat_core::RunId,
            req: StartTurnRequest,
            boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
            contributing_input_ids: Vec<meerkat_core::InputId>,
        ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, SessionError> {
            <Self as SessionService>::start_turn(self, session_id, req).await?;
            Ok(meerkat_core::lifecycle::core_executor::CoreApplyOutput {
                receipt: meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt {
                    run_id,
                    boundary,
                    contributing_input_ids,
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                session_snapshot: None,
                terminal: None,
                run_result: None,
            })
        }
    }

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
            meerkat_mob::ProfileBinding::Inline(meerkat_mob::Profile {
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
            }),
        );
        profiles.insert(
            ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(meerkat_mob::Profile {
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
            }),
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
    fn agent_mob_tools_have_mob_provenance() {
        let defs = build_tool_defs();
        for def in defs.iter() {
            let prov = def
                .provenance
                .as_ref()
                .unwrap_or_else(|| panic!("agent mob tool '{}' is missing provenance", def.name));
            assert_eq!(
                prov.kind,
                meerkat_core::types::ToolSourceKind::Mob,
                "agent mob tool '{}' should have Mob provenance",
                def.name
            );
            assert_eq!(prov.source_id, "mob");
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
                effective_authority: None,
                comms_name: None,
                comms_runtime: None,
                snapshot_context: meerkat_core::service::MobToolSnapshotContext::Standalone,
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
                effective_authority: None,
                comms_name: None,
                comms_runtime: None,
                snapshot_context: meerkat_core::service::MobToolSnapshotContext::Standalone,
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
                effective_authority: None,
                comms_name: None,
                comms_runtime: None,
                snapshot_context: meerkat_core::service::MobToolSnapshotContext::Standalone,
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
                .as_inline()
                .unwrap()
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
                .as_inline()
                .unwrap()
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

        // mob_create should return a GrantManageMob effect for the turn owner
        // to merge into canonical session authority.
        assert_eq!(
            create_result.session_effects.len(),
            1,
            "mob_create should emit exactly one session effect"
        );
        assert_eq!(
            create_result.session_effects[0],
            meerkat_core::SessionEffect::GrantManageMob {
                mob_id: mob_id.clone()
            },
            "mob_create effect should carry the created mob_id"
        );
    }

    #[tokio::test]
    async fn test_create_only_authority_delegate_grants_exact_scope_for_new_implicit_mob() {
        let state = MobMcpState::new_in_memory();
        let session_id = SessionId::new();
        let session_key = session_id.to_string();
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
            .expect_err("in-memory harness cannot fully bootstrap autonomous delegate helper");
        assert!(
            matches!(delegate_error, ToolError::ExecutionFailed { .. }),
            "unexpected delegate error: {delegate_error:?}"
        );
        // The implicit mob should still be created even though spawn failed.
        let _mob_id = state
            .find_implicit_mob(&session_key)
            .await
            .expect("delegate should still create an implicit mob");

        // No session effect is returned when delegate errors — the effect
        // is part of the ToolDispatchOutcome which is only produced on success.
        // This is correct: the turn owner should not widen authority for a
        // failed tool call.
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

    #[tokio::test]
    async fn test_delegate_wiring_links_parent_and_helper_peers_and_emits_peer_added_lifecycle() {
        let service = Arc::new(RealCommsSessionSvc::new());
        let state = Arc::new(MobMcpState::new(service.clone()));
        let parent_name = "parent/lead/l-1".to_string();
        let parent_comms = service.register_external_comms(&parent_name).await;
        let parent_peer_id = parent_comms.public_key().expect("parent public key");
        let session_id = SessionId::new();
        let surface = AgentMobToolSurface::new(
            Arc::clone(&state),
            None,
            create_only_authority(),
            "claude-sonnet-4-5".to_string(),
            session_id.clone(),
            Some(parent_name.clone()),
            Some(parent_peer_id.clone()),
            Some(parent_comms.clone() as Arc<dyn CoreCommsRuntime>),
        );

        let (mob_id, _created) = surface
            .ensure_implicit_mob()
            .await
            .expect("create implicit mob");
        let helper_id = MeerkatId::from("helper-1");
        let mut spec = SpawnMemberSpec::new(ProfileName::from("delegate"), helper_id.clone());
        spec.runtime_mode = Some(MobRuntimeMode::TurnDriven);
        let member_ref = state
            .mob_spawn_spec(&mob_id, spec)
            .await
            .expect("spawn helper for delegate wiring test");
        let wired = surface
            .wire_delegate_helper_to_creator(&mob_id, &helper_id)
            .await;
        assert!(
            wired,
            "delegate wiring should succeed when creator comms are present"
        );

        let helper_session_id = member_ref.session_id().cloned().expect("helper session id");
        let helper_comms = service
            .real_comms(&helper_session_id)
            .await
            .expect("helper comms");
        let helper_name = format!("{}/{}/{}", mob_id, "delegate", helper_id);

        let parent_peers = CoreCommsRuntime::peers(&*parent_comms).await;
        assert!(
            parent_peers
                .iter()
                .any(|entry| entry.name.as_str() == helper_name),
            "delegate should expose helper in parent peers()"
        );

        let helper_peers = CoreCommsRuntime::peers(&*helper_comms).await;
        assert!(
            helper_peers
                .iter()
                .any(|entry| entry.name.as_str() == parent_name),
            "delegate should expose the creating meerkat in helper peers()"
        );

        let parent_inbox = CoreCommsRuntime::drain_inbox_interactions(&*parent_comms).await;
        assert!(
            parent_inbox.iter().any(|interaction| {
                matches!(
                    &interaction.content,
                    meerkat_core::InteractionContent::Request { intent, .. }
                        if intent == "mob.peer_added"
                )
            }),
            "delegate wiring must emit mob.peer_added to the creating meerkat"
        );

        let helper_inbox = CoreCommsRuntime::drain_inbox_interactions(&*helper_comms).await;
        assert!(
            helper_inbox.iter().any(|interaction| {
                matches!(
                    &interaction.content,
                    meerkat_core::InteractionContent::Request { intent, .. }
                        if intent == "mob.peer_added"
                )
            }),
            "delegate wiring must emit mob.peer_added to the helper"
        );
    }

    // ─── Profile CRUD tests ─────────────────────────────────────────

    fn sample_profile_json(model: &str) -> serde_json::Value {
        json!({
            "model": model,
            "peer_description": "test profile",
            "runtime_mode": "autonomous_host"
        })
    }

    fn surface_with_profiles(state: Arc<MobMcpState>) -> AgentMobToolSurface {
        AgentMobToolSurface::new(
            state,
            None,
            create_only_authority(),
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
            None,
            None,
        )
    }

    #[test]
    fn test_profile_tools_present_when_store_available() {
        let defs = build_tool_defs_with_profile_support(true, false);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"mob_profile_create"));
        assert!(names.contains(&"mob_profile_get"));
        assert!(names.contains(&"mob_profile_list"));
        assert!(names.contains(&"mob_profile_update"));
        assert!(names.contains(&"mob_profile_delete"));
        // list_sources requires snapshot provider
        assert!(!names.contains(&"mob_profile_list_sources"));
    }

    #[test]
    fn test_profile_tools_absent_without_store() {
        let defs = build_tool_defs_with_profile_support(false, false);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(!names.contains(&"mob_profile_create"));
        assert!(!names.contains(&"mob_profile_list_sources"));
    }

    #[test]
    fn test_list_sources_tool_present_when_both_store_and_provider() {
        let defs = build_tool_defs_with_profile_support(true, true);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"mob_profile_list_sources"));
    }

    #[tokio::test]
    async fn test_profile_crud_roundtrip() {
        let state = MobMcpState::new_in_memory();
        let surface = surface_with_profiles(Arc::clone(&state));

        // Create
        let create_args = serde_json::value::RawValue::from_string(
            json!({
                "name": "worker",
                "profile": sample_profile_json("claude-opus-4-6")
            })
            .to_string(),
        )
        .unwrap();
        let create_result = surface
            .dispatch(ToolCallView {
                id: "c1",
                name: "mob_profile_create",
                args: &create_args,
            })
            .await
            .expect("profile create should succeed");
        let created: serde_json::Value =
            serde_json::from_str(&create_result.result.text_content()).unwrap();
        assert_eq!(created["name"], "worker");
        assert_eq!(created["revision"], 1);

        // Get
        let get_args =
            serde_json::value::RawValue::from_string(json!({"name": "worker"}).to_string())
                .unwrap();
        let get_result = surface
            .dispatch(ToolCallView {
                id: "g1",
                name: "mob_profile_get",
                args: &get_args,
            })
            .await
            .expect("profile get should succeed");
        let got: serde_json::Value =
            serde_json::from_str(&get_result.result.text_content()).unwrap();
        assert_eq!(got["name"], "worker");
        assert_eq!(got["profile"]["model"], "claude-opus-4-6");

        // List
        let list_args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let list_result = surface
            .dispatch(ToolCallView {
                id: "l1",
                name: "mob_profile_list",
                args: &list_args,
            })
            .await
            .expect("profile list should succeed");
        let listed: serde_json::Value =
            serde_json::from_str(&list_result.result.text_content()).unwrap();
        assert_eq!(listed["profiles"].as_array().unwrap().len(), 1);

        // Update
        let update_args = serde_json::value::RawValue::from_string(
            json!({
                "name": "worker",
                "profile": sample_profile_json("claude-sonnet-4-6"),
                "expected_revision": 1
            })
            .to_string(),
        )
        .unwrap();
        let update_result = surface
            .dispatch(ToolCallView {
                id: "u1",
                name: "mob_profile_update",
                args: &update_args,
            })
            .await
            .expect("profile update should succeed");
        let updated: serde_json::Value =
            serde_json::from_str(&update_result.result.text_content()).unwrap();
        assert_eq!(updated["revision"], 2);

        // Delete
        let delete_args = serde_json::value::RawValue::from_string(
            json!({"name": "worker", "expected_revision": 2}).to_string(),
        )
        .unwrap();
        let delete_result = surface
            .dispatch(ToolCallView {
                id: "d1",
                name: "mob_profile_delete",
                args: &delete_args,
            })
            .await
            .expect("profile delete should succeed");
        let deleted: serde_json::Value =
            serde_json::from_str(&delete_result.result.text_content()).unwrap();
        assert_eq!(deleted["name"], "worker");
        assert_eq!(deleted["deleted_revision"], 2);

        // Confirm deleted
        let get_result2 = surface
            .dispatch(ToolCallView {
                id: "g2",
                name: "mob_profile_get",
                args: &get_args,
            })
            .await
            .expect("profile get after delete should succeed");
        let got2: serde_json::Value =
            serde_json::from_str(&get_result2.result.text_content()).unwrap();
        assert_eq!(got2["not_found"], true);
    }

    #[tokio::test]
    async fn test_profile_get_nonexistent_returns_not_found() {
        let state = MobMcpState::new_in_memory();
        let surface = surface_with_profiles(state);

        let args =
            serde_json::value::RawValue::from_string(json!({"name": "ghost"}).to_string()).unwrap();
        let result = surface
            .dispatch(ToolCallView {
                id: "g1",
                name: "mob_profile_get",
                args: &args,
            })
            .await
            .expect("get nonexistent should return result, not error");
        let got: serde_json::Value = serde_json::from_str(&result.result.text_content()).unwrap();
        assert_eq!(got["not_found"], true);
    }

    #[tokio::test]
    async fn test_profile_update_wrong_revision_fails() {
        let state = MobMcpState::new_in_memory();
        let surface = surface_with_profiles(Arc::clone(&state));

        // Create first
        let create_args = serde_json::value::RawValue::from_string(
            json!({
                "name": "stale",
                "profile": sample_profile_json("claude-opus-4-6")
            })
            .to_string(),
        )
        .unwrap();
        surface
            .dispatch(ToolCallView {
                id: "c1",
                name: "mob_profile_create",
                args: &create_args,
            })
            .await
            .expect("create");

        // Update with wrong revision
        let update_args = serde_json::value::RawValue::from_string(
            json!({
                "name": "stale",
                "profile": sample_profile_json("claude-sonnet-4-6"),
                "expected_revision": 99
            })
            .to_string(),
        )
        .unwrap();
        let update_result = surface
            .dispatch(ToolCallView {
                id: "u1",
                name: "mob_profile_update",
                args: &update_args,
            })
            .await;
        assert!(
            update_result.is_err(),
            "update with wrong revision should fail"
        );
    }

    #[tokio::test]
    async fn test_list_sources_standalone_returns_not_found() {
        let state = MobMcpState::new_in_memory();
        // Standalone context — list_sources should not be in tools()
        let surface = surface_with_profiles(state);
        let tools = surface.tools();
        let names: Vec<&str> = tools.iter().map(|d| d.name.as_str()).collect();
        assert!(
            !names.contains(&"mob_profile_list_sources"),
            "list_sources must not appear in Standalone context"
        );
    }

    #[tokio::test]
    async fn test_list_sources_with_parent_provider() {
        use meerkat_core::service::{MobToolSnapshotContext, VisibleToolSnapshotProvider};

        struct TestSnapshotProvider;
        impl VisibleToolSnapshotProvider for TestSnapshotProvider {
            fn snapshot_visible_tools(&self) -> Vec<Arc<ToolDef>> {
                vec![
                    Arc::new(ToolDef {
                        name: "tool_a".to_string(),
                        description: "Tool A".to_string(),
                        input_schema: json!({"type": "object"}),
                        provenance: Some(ToolProvenance {
                            kind: ToolSourceKind::Builtin,
                            source_id: "core".to_string(),
                        }),
                    }),
                    Arc::new(ToolDef {
                        name: "tool_b".to_string(),
                        description: "Tool B".to_string(),
                        input_schema: json!({"type": "object"}),
                        provenance: Some(ToolProvenance {
                            kind: ToolSourceKind::Mob,
                            source_id: "mob".to_string(),
                        }),
                    }),
                ]
            }
        }

        let state = MobMcpState::new_in_memory();
        let provider: Arc<dyn VisibleToolSnapshotProvider> = Arc::new(TestSnapshotProvider);
        let surface = AgentMobToolSurface::new_with_effective_authority(
            Arc::clone(&state),
            None,
            Arc::new(std::sync::RwLock::new(create_only_authority())),
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
            None,
            None,
            MobToolSnapshotContext::ParentOwned(provider),
        );

        // list_sources should be in tools
        let tools = surface.tools();
        let names: Vec<&str> = tools.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"mob_profile_list_sources"));

        let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
        let result = surface
            .dispatch(ToolCallView {
                id: "ls1",
                name: "mob_profile_list_sources",
                args: &args,
            })
            .await
            .expect("list_sources should succeed");
        let parsed: serde_json::Value =
            serde_json::from_str(&result.result.text_content()).unwrap();
        let sources = parsed["sources"].as_array().unwrap();
        assert_eq!(sources.len(), 2, "two provenance groups expected");
    }

    // ─── SpawnTooling resolution tests (T2.3) ───────────────────────────

    /// Snapshot provider with comms + non-comms tools for overlay testing.
    struct ToolingTestSnapshotProvider;
    impl VisibleToolSnapshotProvider for ToolingTestSnapshotProvider {
        fn snapshot_visible_tools(&self) -> Vec<Arc<ToolDef>> {
            [
                "send",
                "send_message",
                "send_request",
                "send_response",
                "peers",
                "read_file",
                "write_file",
                "bash",
            ]
            .iter()
            .map(|name| {
                Arc::new(ToolDef {
                    name: name.to_string(),
                    description: format!("{name} tool"),
                    input_schema: json!({"type": "object"}),
                    provenance: None,
                })
            })
            .collect()
        }
    }

    fn surface_with_parent_tools() -> AgentMobToolSurface {
        let provider: Arc<dyn VisibleToolSnapshotProvider> = Arc::new(ToolingTestSnapshotProvider);
        AgentMobToolSurface::new_with_effective_authority(
            MobMcpState::new_in_memory(),
            None,
            Arc::new(std::sync::RwLock::new(create_only_authority())),
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
            None,
            None,
            MobToolSnapshotContext::ParentOwned(provider),
        )
    }

    fn surface_standalone() -> AgentMobToolSurface {
        AgentMobToolSurface::new(
            MobMcpState::new_in_memory(),
            None,
            create_only_authority(),
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
            None,
            None,
        )
    }

    #[tokio::test]
    async fn test_resolve_spawn_tooling_inherit_parent_captures_all_visible() {
        let surface = surface_with_parent_tools();
        let tooling = meerkat_mob::SpawnTooling::InheritParent {
            allow_overlay: None,
            deny_overlay: None,
        };
        let resolved = surface.resolve_spawn_tooling(&tooling).await.unwrap();
        match resolved.inherited_tool_filter {
            Some(meerkat_core::tool_scope::ToolFilter::Allow(names)) => {
                assert_eq!(names.len(), 8, "should inherit all 8 parent tools");
                assert!(names.contains("send"));
                assert!(names.contains("read_file"));
                assert!(names.contains("bash"));
            }
            other => panic!("expected Some(Allow), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_resolve_spawn_tooling_inherit_parent_with_deny_overlay() {
        let surface = surface_with_parent_tools();
        let tooling = meerkat_mob::SpawnTooling::InheritParent {
            allow_overlay: None,
            deny_overlay: Some(vec!["bash".to_string(), "write_file".to_string()]),
        };
        let resolved = surface.resolve_spawn_tooling(&tooling).await.unwrap();
        match resolved.inherited_tool_filter {
            Some(meerkat_core::tool_scope::ToolFilter::Allow(names)) => {
                assert_eq!(names.len(), 6);
                assert!(!names.contains("bash"));
                assert!(!names.contains("write_file"));
                assert!(names.contains("read_file"));
                assert!(names.contains("send"));
            }
            other => panic!("expected Some(Allow), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_resolve_spawn_tooling_inherit_parent_with_allow_overlay() {
        let surface = surface_with_parent_tools();
        let tooling = meerkat_mob::SpawnTooling::InheritParent {
            allow_overlay: Some(vec!["send".to_string(), "read_file".to_string()]),
            deny_overlay: None,
        };
        let resolved = surface.resolve_spawn_tooling(&tooling).await.unwrap();
        match resolved.inherited_tool_filter {
            Some(meerkat_core::tool_scope::ToolFilter::Allow(names)) => {
                assert_eq!(names.len(), 2);
                assert!(names.contains("send"));
                assert!(names.contains("read_file"));
            }
            other => panic!("expected Some(Allow), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_resolve_spawn_tooling_inherit_parent_standalone_errors() {
        let surface = surface_standalone();
        let tooling = meerkat_mob::SpawnTooling::InheritParent {
            allow_overlay: None,
            deny_overlay: None,
        };
        let err = surface.resolve_spawn_tooling(&tooling).await.unwrap_err();
        assert!(
            matches!(err, ToolError::ExecutionFailed { .. }),
            "InheritParent in Standalone context should return ExecutionFailed, got {err:?}"
        );
    }

    #[tokio::test]
    async fn test_resolve_spawn_tooling_minimal_returns_comms_only() {
        let surface = surface_with_parent_tools();
        let tooling = meerkat_mob::SpawnTooling::Minimal;
        let resolved = surface.resolve_spawn_tooling(&tooling).await.unwrap();
        match resolved.inherited_tool_filter {
            Some(meerkat_core::tool_scope::ToolFilter::Allow(names)) => {
                assert_eq!(names.len(), 5);
                assert!(names.contains("send"));
                assert!(names.contains("send_message"));
                assert!(names.contains("send_request"));
                assert!(names.contains("send_response"));
                assert!(names.contains("peers"));
                assert!(!names.contains("bash"));
                assert!(!names.contains("read_file"));
            }
            other => panic!("expected Some(Allow), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_resolve_spawn_tooling_minimal_standalone_errors() {
        let surface = surface_standalone();
        let tooling = meerkat_mob::SpawnTooling::Minimal;
        let err = surface.resolve_spawn_tooling(&tooling).await.unwrap_err();
        assert!(matches!(err, ToolError::ExecutionFailed { .. }));
    }

    #[tokio::test]
    async fn test_resolve_spawn_tooling_profile_no_overlays_returns_none() {
        let surface = surface_with_parent_tools();
        let tooling = meerkat_mob::SpawnTooling::Profile {
            source: Box::new(meerkat_mob::ProfileSource::Inline(meerkat_mob::Profile {
                model: "claude-sonnet-4-5".to_string(),
                skills: Vec::new(),
                tools: meerkat_mob::ToolConfig::default(),
                peer_description: "test".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            })),
            allow_overlay: None,
            deny_overlay: None,
        };
        let resolved = surface.resolve_spawn_tooling(&tooling).await.unwrap();
        assert!(
            resolved.inherited_tool_filter.is_none(),
            "Profile without overlays should return None (no inherited filter)"
        );
    }

    #[tokio::test]
    async fn test_resolve_spawn_tooling_profile_with_deny_overlay() {
        let surface = surface_with_parent_tools();
        let tooling = meerkat_mob::SpawnTooling::Profile {
            source: Box::new(meerkat_mob::ProfileSource::Inline(meerkat_mob::Profile {
                model: "claude-sonnet-4-5".to_string(),
                skills: Vec::new(),
                tools: meerkat_mob::ToolConfig::default(),
                peer_description: "test".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            })),
            allow_overlay: None,
            deny_overlay: Some(vec!["bash".to_string()]),
        };
        let resolved = surface.resolve_spawn_tooling(&tooling).await.unwrap();
        match resolved.inherited_tool_filter {
            Some(meerkat_core::tool_scope::ToolFilter::Allow(names)) => {
                assert!(!names.contains("bash"));
                assert!(names.contains("read_file"));
            }
            other => panic!("expected Some(Allow), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_resolve_spawn_tooling_profile_with_overlays_standalone_errors() {
        let surface = surface_standalone();
        let tooling = meerkat_mob::SpawnTooling::Profile {
            source: Box::new(meerkat_mob::ProfileSource::Inline(meerkat_mob::Profile {
                model: "claude-sonnet-4-5".to_string(),
                skills: Vec::new(),
                tools: meerkat_mob::ToolConfig::default(),
                peer_description: "test".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            })),
            allow_overlay: Some(vec!["send".to_string()]),
            deny_overlay: None,
        };
        let err = surface.resolve_spawn_tooling(&tooling).await.unwrap_err();
        assert!(matches!(err, ToolError::ExecutionFailed { .. }));
    }

    /// Regression: SpawnTooling::Profile with an inline profile must populate
    /// `override_profile` so the spawn path uses it instead of the definition's default.
    #[tokio::test]
    async fn test_resolve_spawn_tooling_profile_source_populates_override_profile() {
        let surface = surface_with_parent_tools();
        let expected_model = "claude-opus-4-6".to_string();
        let tooling = meerkat_mob::SpawnTooling::Profile {
            source: Box::new(meerkat_mob::ProfileSource::Inline(meerkat_mob::Profile {
                model: expected_model.clone(),
                skills: Vec::new(),
                tools: meerkat_mob::ToolConfig::default(),
                peer_description: "override test".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            })),
            allow_overlay: None,
            deny_overlay: None,
        };
        let resolved = surface.resolve_spawn_tooling(&tooling).await.unwrap();
        let profile = resolved
            .override_profile
            .expect("Profile source should populate override_profile");
        assert_eq!(
            profile.model, expected_model,
            "override_profile.model must match the inline profile's model"
        );
    }
}
