//! Agent-facing mob tool surface for delegation and orchestration.
//!
//! `AgentMobToolSurface` provides the agent-internal mob tools (delegate,
//! mob_create, mob_destroy, mob_spawn_member, mob_retire_member,
//! mob_check_member, mob_list_members, mob_list) composed into the tool
//! gateway by `build_agent()`.
//!
//! `archive_session_with_mob_cleanup()` is a helper that archives a session
//! and destroys its owned mobs in a single call.

use async_trait::async_trait;
use meerkat_contracts::wire::WireHostRef;
use meerkat_core::error::ToolError;
use meerkat_core::service::{MobToolAuthorityContext, SessionError};
use meerkat_core::types::{
    ContentInput, SessionId, ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind,
};
use meerkat_core::{AgentToolDispatcher, ToolUnavailableReason};
use meerkat_mob::machines::mob_machine::HostId;
use meerkat_mob::{
    AgentIdentity, BoundedResultSpec, DelegationExecutionError, DelegationExecutionRequest,
    DelegationExecutionService, DelegationMemberOptions, DelegationParentContext, MobBackendKind,
    MobDefinition, MobError, MobHandle, MobId, MobRuntimeMode, ProfileBinding, ProfileName,
    SpawnMemberSpec, SpawnResult, runtime::MobSessionService,
};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc};

#[cfg(not(target_arch = "wasm32"))]
use ::tokio::{
    self,
    sync::{RwLock, oneshot},
};
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::{
    self as tokio,
    sync::{RwLock, oneshot},
};

use crate::MobMcpState;
use crate::temporary_council::{
    MergeBackPolicy, TemporaryCouncilBounds, TemporaryCouncilDeadline, TemporaryCouncilError,
    TemporaryCouncilParticipantSpec, TemporaryCouncilRequest, TemporaryCouncilStructuredContract,
};
use meerkat_core::comms::{
    CommsCommand, CommsTrustMutation, CommsTrustMutationResult, PeerId, PeerName, PeerRoute,
    SendError, TrustedPeerDescriptor,
};

// ─── Tool name constants ─────────────────────────────────────────────────

const TOOL_DELEGATE: &str = "delegate";
const TOOL_CONCLUDE_OBJECTIVE: &str = "conclude_objective";
const TOOL_MOB_CREATE: &str = "mob_create";
const TOOL_MOB_DESTROY: &str = "mob_destroy";
const TOOL_MOB_SPAWN_MEMBER: &str = "mob_spawn_member";
const TOOL_FORK_OFF: &str = "fork_off";
const TOOL_COUNCIL: &str = "council";
const TOOL_MOB_RETIRE_MEMBER: &str = "mob_retire_member";
const TOOL_MOB_CHECK_MEMBER: &str = "mob_check_member";
const TOOL_MOB_LIST_MEMBERS: &str = "mob_list_members";
const TOOL_MOB_LIST: &str = "mob_list";
const TOOL_MOB_WIRE: &str = "mob_wire";
const TOOL_MOB_UNWIRE: &str = "mob_unwire";
const TOOL_MOB_PROFILE_CREATE: &str = "mob_profile_create";
const TOOL_MOB_PROFILE_GET: &str = "mob_profile_get";
const TOOL_MOB_PROFILE_LIST: &str = "mob_profile_list";
const TOOL_MOB_PROFILE_UPDATE: &str = "mob_profile_update";
const TOOL_MOB_PROFILE_DELETE: &str = "mob_profile_delete";
const TOOL_MOB_PROFILE_LIST_SOURCES: &str = "mob_profile_list_sources";

#[cfg(not(target_arch = "wasm32"))]
type AgentDispatchFuture<'a> =
    Pin<Box<dyn Future<Output = Result<meerkat_core::ToolDispatchOutcome, ToolError>> + Send + 'a>>;
#[cfg(target_arch = "wasm32")]
type AgentDispatchFuture<'a> =
    Pin<Box<dyn Future<Output = Result<meerkat_core::ToolDispatchOutcome, ToolError>> + 'a>>;

#[cfg(not(target_arch = "wasm32"))]
type AgentOperationFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
#[cfg(target_arch = "wasm32")]
type AgentOperationFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

#[cfg(not(target_arch = "wasm32"))]
type MobCreateFuture = Pin<Box<dyn Future<Output = Result<MobId, MobError>> + Send + 'static>>;
#[cfg(target_arch = "wasm32")]
type MobCreateFuture = Pin<Box<dyn Future<Output = Result<MobId, MobError>> + 'static>>;

// Construct each large concrete async state machine behind an inline-resistant
// heap boundary. `Box::pin(self.dispatch_...)` inside the dispatch match still
// materializes the concrete future as a stack temporary in debug builds.
macro_rules! boxed_agent_dispatch {
    ($boxed:ident, $inner:ident $(, $arg:ident : $arg_ty:ty)*) => {
        #[inline(never)]
        fn $boxed<'a>(
            &'a self,
            call: ToolCallView<'a>,
            $($arg: $arg_ty),*
        ) -> AgentDispatchFuture<'a> {
            Box::pin(self.$inner(call, $($arg),*))
        }
    };
}

#[inline(never)]
fn mob_create_with_owner_bridge_boxed(
    state: Arc<MobMcpState>,
    definition: MobDefinition,
    owner_bridge_session_id: SessionId,
) -> MobCreateFuture {
    // Mob creation (definition validation, store bootstrap, actor start)
    // is reached from inside the calling agent's tool-dispatch poll; its
    // opt-level=0 poll frames are large, so run it on its own task instead
    // of stacking those frames onto the caller's run-loop chain (2 MiB
    // production worker-stack budget).
    Box::pin(meerkat_runtime::stack_relief::relieve_caller_stack(
        move || async move {
            state
                .mob_create_definition_with_owner_bridge_session(
                    definition,
                    owner_bridge_session_id,
                    true,
                    false,
                )
                .await
        },
    ))
}

// ─── ResolvedSpawnTooling ────────────────────────────────────────────────

/// Result of resolving `SpawnTooling` into concrete values for spawning.
#[derive(Debug, Clone)]
pub struct ResolvedSpawnTooling {
    /// Parent/composition-authorized inherited tool filter for the child session.
    pub inherited_tool_filter: Option<meerkat_core::InheritedToolVisibilityAuthority>,
    /// Override profile resolved from `SpawnTooling::Profile` source.
    /// When set, the spawn path uses this profile instead of the definition's.
    pub override_profile: Option<meerkat_mob::Profile>,
}

/// Resolve a child's effective call-level tool access policy at the spawn seam.
///
/// - An explicit `AllowList`/`DenyList` is admitted as-is (presence is already
///   the MobMachine-privileged admission fact; this function does not change
///   admission).
/// - `Inherit` or absent resolves to the PARENT's persisted effective policy
///   (transitive containment: a restricted parent cannot mint an unrestricted
///   child by spawning). The parent's effective policy is itself never
///   `Inherit` — the factory fails the build closed before persisting one —
///   so the result of this function is always resolved.
/// - No parent policy (`parent_effective = None`) means unrestricted.
pub(crate) fn effective_child_tool_access_policy(
    requested: Option<meerkat_core::ops::ToolAccessPolicy>,
    parent_effective: Option<meerkat_core::ops::ToolAccessPolicy>,
) -> Option<meerkat_core::ops::ToolAccessPolicy> {
    match requested {
        Some(meerkat_core::ops::ToolAccessPolicy::Inherit) | None => parent_effective,
        explicit => explicit,
    }
}

fn lower_wire_placement(placement: Option<WireHostRef>) -> Option<HostId> {
    placement.map(|host| HostId::from(host.0))
}

// ─── AgentMobToolSurface ─────────────────────────────────────────────────

#[derive(Clone)]
enum ParentToolAccessPolicySource {
    /// AgentFactory supplied the already-resolved effective parent policy
    /// through its opaque parent-composition authority.
    Resolved(Option<meerkat_core::ops::ToolAccessPolicy>),
    /// Compatibility path for public constructors that predate the opaque
    /// authority handoff. Resolve from durable session metadata at dispatch.
    LegacySessionMetadata,
}

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
    parent_tool_access_policy_source: ParentToolAccessPolicySource,
    tools: Arc<[Arc<ToolDef>]>,
    owner_bridge_session_id: SessionId,
    /// Model name inherited by implicit mob helpers.
    model: String,
    /// Parent agent's comms name (for building TrustedPeerDescriptor when wiring helpers).
    comms_name: Option<String>,
    /// Parent agent's canonical comms peer ID.
    comms_peer_id: Option<PeerId>,
    /// Parent agent's comms runtime for bidirectional wiring.
    comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
    /// Context for capturing a parent agent's tool scope snapshot.
    snapshot_context: meerkat_core::service::MobToolSnapshotContext,
    /// Chokepoint-(b) console principal for this surface (SD-7 / DEC-P5E-8):
    /// injected at build, never ambient. v1: a locally built session belongs
    /// to the building surface's principal (`Owner`); v2 derives it from the
    /// session's builder (owner-session equality against the machine owner
    /// fact — `MobControlPrincipal::from_owner_bridge_session`).
    control_principal: meerkat_mob::MobControlPrincipal,
}

impl AgentMobToolSurface {
    /// Acquire a mob handle bound to THIS surface's console principal
    /// (chokepoint-(b) principal minting; SD-7: this surface is gated
    /// exactly like the RPC/REST/MCP handlers). The agent-lane `ensure_*`
    /// gates on the machine-composed authority context stay, additive.
    async fn bound_handle(&self, mob_id: &MobId) -> Result<MobHandle, MobError> {
        // Handle resolution (store restore + runtime rebind in
        // `MobMcpState::ensure_restored`) is reached from inside the calling
        // agent's tool-dispatch poll; run it on its own task so its
        // opt-level=0 poll frames do not stack onto the caller's run-loop
        // chain (2 MiB production worker-stack budget).
        let state = Arc::clone(&self.state);
        let mob_id = mob_id.clone();
        let handle = meerkat_runtime::stack_relief::relieve_caller_stack(move || async move {
            state.handle_for(&mob_id).await
        })
        .await?;
        Ok(
            handle.with_command_authority(meerkat_mob::CommandAuthority::principal(
                self.control_principal.clone(),
            )),
        )
    }

    /// Create a new agent mob tool surface.
    ///
    /// # Arguments
    /// * `state` - Shared MobMcpState for mob lifecycle operations
    /// * `implicit_mob_id` - Pre-seeded implicit mob ID (resume case)
    /// * `model` - Model name inherited by spawned helpers
    /// * `owner_bridge_session_id` - Bridge session ID of the owning agent
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state: Arc<MobMcpState>,
        implicit_mob_id: Option<MobId>,
        authority_context: MobToolAuthorityContext,
        model: String,
        owner_bridge_session_id: SessionId,
        comms_name: Option<String>,
        comms_peer_id: Option<PeerId>,
        comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
    ) -> Self {
        Self::new_with_effective_authority(
            state,
            implicit_mob_id,
            Arc::new(std::sync::RwLock::new(authority_context)),
            model,
            owner_bridge_session_id,
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
        owner_bridge_session_id: SessionId,
        comms_name: Option<String>,
        comms_peer_id: Option<PeerId>,
        comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
        snapshot_context: meerkat_core::service::MobToolSnapshotContext,
    ) -> Self {
        Self::new_with_effective_authority_and_policy_source(
            state,
            implicit_mob_id,
            effective_authority,
            ParentToolAccessPolicySource::LegacySessionMetadata,
            model,
            owner_bridge_session_id,
            comms_name,
            comms_peer_id,
            comms_runtime,
            snapshot_context,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_effective_authority_and_policy_source(
        state: Arc<MobMcpState>,
        implicit_mob_id: Option<MobId>,
        effective_authority: Arc<std::sync::RwLock<MobToolAuthorityContext>>,
        parent_tool_access_policy_source: ParentToolAccessPolicySource,
        model: String,
        owner_bridge_session_id: SessionId,
        comms_name: Option<String>,
        comms_peer_id: Option<PeerId>,
        comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
        snapshot_context: meerkat_core::service::MobToolSnapshotContext,
    ) -> Self {
        let has_profile_store = state.realm_profile_store().is_some();
        let has_snapshot_provider = matches!(
            &snapshot_context,
            meerkat_core::service::MobToolSnapshotContext::ParentOwned(_)
        );
        let authority_snapshot = effective_authority
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let has_generated_authority = authority_snapshot.is_generated_authority_context();
        let tools = if has_generated_authority {
            build_tool_defs_with_profile_support(
                has_profile_store,
                has_snapshot_provider,
                authority_snapshot.can_run_adaptive_packs(),
            )
        } else {
            Arc::<[Arc<ToolDef>]>::from([])
        };
        Self {
            state,
            cached_implicit_mob_id: RwLock::new(implicit_mob_id),
            effective_authority,
            parent_tool_access_policy_source,
            tools,
            owner_bridge_session_id,
            model,
            comms_name,
            comms_peer_id,
            comms_runtime,
            snapshot_context,
            // v1: every locally built session surface is the owner console
            // (A16); byte-identical single-user behavior. The field is the
            // v2 injection seam, not an ambient default at call sites.
            control_principal: meerkat_mob::MobControlPrincipal::Owner,
        }
    }

    /// Usage instructions for agent mob tools to be added to the system prompt.
    pub fn usage_instructions() -> &'static str {
        "# Agent Delegation & Orchestration\n\n\
         You can delegate work to helper agents and orchestrate multi-agent mobs:\n\n\
         - delegate: Run one exact bounded helper task in an implicit mob, then retire the helper\n\
         - conclude_objective: Publish the final answer for the current pre-addressed kickoff objective\n\
         - mob_create: Create an explicit mob with full control over profiles, wiring, and flows\n\
         - mob_destroy: Destroy an explicit mob (cannot destroy implicit delegation mob)\n\
         - mob_spawn_member: Spawn a member into any mob\n\
         - fork_off: Fork a durable member and run one exact child turn while retaining it\n\
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
        // K1 structured egress: structured mob-tool output stays a typed
        // `ContentBlock::Structured` payload instead of being collapsed into
        // serialized text. Serialization faults propagate as typed errors.
        let block = meerkat_core::types::ContentBlock::structured(&value).map_err(|e| {
            ToolError::execution_failed(format!(
                "failed to serialize JSON tool output for '{}': {e}",
                call.name
            ))
        })?;
        Ok(meerkat_core::ToolDispatchOutcome::new(
            ToolResult::with_blocks(call.id.to_string(), vec![block], false),
            vec![],
            session_effects,
        ))
    }

    fn spawn_result_payload(mob_id: &MobId, result: &SpawnResult) -> serde_json::Value {
        let identity_str = result.agent_identity.to_string();
        json!({
            "agent_identity": result.agent_identity,
            "member_ref": meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), &identity_str),
        })
    }

    fn map_mob_error(call: ToolCallView<'_>, error: MobError) -> ToolError {
        let message = format!("tool '{}' failed: {error}", call.name);
        match error.structured_data() {
            Some(data) => ToolError::execution_failed_with_data(message, data),
            None => ToolError::execution_failed(message),
        }
    }

    fn map_destroy_error(call: ToolCallView<'_>, error: crate::MobMcpDestroyError) -> ToolError {
        match error {
            crate::MobMcpDestroyError::Incomplete { report } => {
                ToolError::execution_failed_with_data(
                    format!(
                        "tool '{}' failed: mob destroy incomplete: {}",
                        call.name,
                        crate::destroy_report_summary(&report)
                    ),
                    crate::MobMcpDestroyError::incomplete_error_data(&report),
                )
            }
            crate::MobMcpDestroyError::Mob(error) => Self::map_mob_error(call, error),
        }
    }

    fn map_bounded_member_run_error(
        call: ToolCallView<'_>,
        error: meerkat_mob::BoundedMemberRunError,
    ) -> ToolError {
        match error {
            meerkat_mob::BoundedMemberRunError::Admission(error) => {
                Self::map_mob_error(call, error)
            }
            other => ToolError::execution_failed(format!(
                "tool '{}' exact bounded child turn failed: {other}",
                call.name
            )),
        }
    }

    fn map_council_error(call: ToolCallView<'_>, error: TemporaryCouncilError) -> ToolError {
        ToolError::execution_failed(format!("tool '{}' failed: {error}", call.name))
    }

    fn authority_context_snapshot(&self) -> MobToolAuthorityContext {
        self.effective_authority
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn ensure_create_authority(&self, tool_name: &str) -> Result<(), ToolError> {
        // Pure observation extracted from the machine-minted operator-authority
        // projection (the create-mobs capability bit). MobMachine — not this
        // surface — decides the Allow/Deny verdict; we mirror it (Denied ->
        // access_denied). This operator-capability admission is not scoped to a
        // live mob, so it is resolved by a stateless MobMachine classification.
        // Fails closed.
        let can_create_mobs = self.authority_context_snapshot().can_create_mobs();
        let admission =
            meerkat_mob::mob_machine_create_mob_admission(can_create_mobs).map_err(|error| {
                ToolError::execution_failed(format!(
                    "tool '{tool_name}' create-mob admission failed: {error}"
                ))
            })?;
        match admission {
            meerkat_mob::CreateMobAdmission::Allowed => Ok(()),
            meerkat_mob::CreateMobAdmission::Denied => Err(ToolError::access_denied(tool_name)),
        }
    }

    async fn ensure_profile_mutation_authority(&self, tool_name: &str) -> Result<(), ToolError> {
        // Pure observation extracted from the machine-minted operator-authority
        // projection (the mutate-profiles capability bit). MobMachine — not this
        // surface — decides the Allow/Deny verdict; we mirror it (Denied ->
        // access_denied). This operator-capability admission is not scoped to a
        // live mob, so it is resolved by a stateless MobMachine classification.
        // Fails closed.
        let can_mutate_profiles = self.authority_context_snapshot().can_mutate_profiles();
        let admission = meerkat_mob::mob_machine_profile_mutation_admission(can_mutate_profiles)
            .map_err(|error| {
                ToolError::execution_failed(format!(
                    "tool '{tool_name}' profile-mutation admission failed: {error}"
                ))
            })?;
        match admission {
            meerkat_mob::ProfileMutationAdmission::Allowed => Ok(()),
            meerkat_mob::ProfileMutationAdmission::Denied => {
                Err(ToolError::access_denied(tool_name))
            }
        }
    }

    async fn ensure_mob_scope_authority(
        &self,
        tool_name: &str,
        mob_id: &MobId,
    ) -> Result<(), ToolError> {
        // Pure observation extracted from the machine-owned operator-scope
        // projection. MobMachine — not this surface — decides the Allow/Deny
        // verdict; we mirror it (Denied -> access_denied). Fails closed.
        let can_manage_mob = self
            .authority_context_snapshot()
            .can_manage_mob(mob_id.as_str());
        let handle = self.bound_handle(mob_id).await.map_err(|error| {
            ToolError::execution_failed(format!(
                "tool '{tool_name}' current-mob admission failed: {error}"
            ))
        })?;
        let admission = handle
            .resolve_current_mob_admission(can_manage_mob)
            .await
            .map_err(|error| {
                ToolError::execution_failed(format!(
                    "tool '{tool_name}' current-mob admission failed: {error}"
                ))
            })?;
        match admission {
            meerkat_mob::CurrentMobAdmission::Allowed => Ok(()),
            meerkat_mob::CurrentMobAdmission::Denied => Err(ToolError::access_denied(tool_name)),
        }
    }

    async fn ensure_spawn_member_scope(
        &self,
        tool_name: &str,
        mob_id: &MobId,
        args: &SpawnMemberArgs,
    ) -> Result<(), ToolError> {
        // RAW, atomic observations extracted from the machine-owned
        // operator-scope projection and the typed spawn args, fed WITHOUT
        // pre-composing them. MobMachine — not this surface — owns the
        // privileged-argument SET membership policy (which args are privileged)
        // and the `manage_scope || profile_scope_contains` disjunction, and
        // composes the Allow/Deny admission verdict; we mirror it. We extract
        // each arg's pure `.is_some()` presence and the raw per-profile scope
        // set membership; args this surface's spawn tool does not accept stay
        // `false`.
        let authority = self.authority_context_snapshot();
        let observations = meerkat_mob::SpawnMemberAdmissionObservations {
            manage_scope_present: authority.can_manage_mob(mob_id.as_str()),
            profile_scope_contains: authority
                .spawn_profile_scope_contains(mob_id.as_str(), &args.profile),
            runtime_mode_present: args.runtime_mode.is_some(),
            backend_present: args.backend.is_some(),
            tooling_present: args.tooling.is_some(),
            auth_binding_present: args.auth_binding.is_some(),
            ..meerkat_mob::SpawnMemberAdmissionObservations::default()
        };
        let handle = self.bound_handle(mob_id).await.map_err(|error| {
            ToolError::execution_failed(format!(
                "tool '{tool_name}' spawn-member admission failed: {error}"
            ))
        })?;
        let admission = handle
            .resolve_spawn_member_admission(observations)
            .await
            .map_err(|error| {
                ToolError::execution_failed(format!(
                    "tool '{tool_name}' spawn-member admission failed: {error}"
                ))
            })?;
        match admission {
            meerkat_mob::SpawnMemberAdmission::Allowed => Ok(()),
            meerkat_mob::SpawnMemberAdmission::Denied => Err(ToolError::access_denied(tool_name)),
        }
    }

    /// Resolve spawn tooling into inherited tool filter and optional override profile.
    ///
    /// - `InheritParent`: snapshot parent's visible tools, apply overlays
    /// - `Minimal`: only comms tools (send, send_message, reply_to_peer,
    ///   send_request, send_response, peers)
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
                let filter = provider
                    .authorize_inherited_tool_visibility_with_overlays(
                        allow_set.as_ref(),
                        deny_set.as_ref(),
                    )
                    .map_err(|err| {
                        ToolError::execution_failed(format!(
                            "parent tool visibility inheritance requires tool provenance witnesses: {err}"
                        ))
                    })?;
                Ok(ResolvedSpawnTooling {
                    inherited_tool_filter: Some(filter),
                    override_profile: None,
                })
            }
            meerkat_mob::SpawnTooling::Minimal => {
                let provider = match &self.snapshot_context {
                    meerkat_core::service::MobToolSnapshotContext::ParentOwned(p) => p,
                    meerkat_core::service::MobToolSnapshotContext::Standalone => {
                        return Err(ToolError::execution_failed(
                            "Minimal tooling requires a parent tool scope (ParentOwned context), \
                             but this agent is running in Standalone mode",
                        ));
                    }
                };
                let comms_tools: std::collections::HashSet<String> = [
                    "send",
                    "send_message",
                    "reply_to_peer",
                    "send_request",
                    "send_response",
                    "peers",
                ]
                .into_iter()
                .map(String::from)
                .collect();
                let filter = provider
                    .authorize_inherited_tool_visibility_with_overlays(Some(&comms_tools), None)
                    .map_err(|err| {
                        ToolError::execution_failed(format!(
                            "minimal tool visibility inheritance requires tool provenance witnesses: {err}"
                        ))
                    })?;
                Ok(ResolvedSpawnTooling {
                    inherited_tool_filter: Some(filter),
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
                        Box::new(
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
                                .profile,
                        )
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
                    Some(
                        provider
                            .authorize_inherited_tool_visibility_with_overlays(
                                allow_set.as_ref(),
                                deny_set.as_ref(),
                            )
                            .map_err(|err| {
                                ToolError::execution_failed(format!(
                                    "profile tool visibility inheritance requires tool provenance witnesses: {err}"
                                ))
                            })?,
                    )
                };

                Ok(ResolvedSpawnTooling {
                    inherited_tool_filter,
                    override_profile: Some(*resolved_profile),
                })
            }
        }
    }

    /// Resolve the spawned child's effective tool access policy.
    ///
    /// Factory-built surfaces resolve from their immutable composition
    /// authority. Legacy public constructors retain their metadata-read
    /// behavior, but that branch stays erased so it cannot inflate the
    /// factory spawn future or re-enter a factory-owned live parent.
    #[inline(never)]
    fn resolve_child_tool_access_policy_boxed<'a>(
        &'a self,
        tool_name: &'a str,
        requested: Option<meerkat_core::ops::ToolAccessPolicy>,
    ) -> AgentOperationFuture<'a, Result<Option<meerkat_core::ops::ToolAccessPolicy>, ToolError>>
    {
        // Preserve the historical explicit-policy fast path: an admitted
        // AllowList/DenyList never needs parent metadata.
        if matches!(
            requested.as_ref(),
            Some(
                meerkat_core::ops::ToolAccessPolicy::AllowList(_)
                    | meerkat_core::ops::ToolAccessPolicy::DenyList(_)
            )
        ) {
            return Box::pin(std::future::ready(Ok(requested)));
        }

        match &self.parent_tool_access_policy_source {
            ParentToolAccessPolicySource::Resolved(parent_effective) => {
                let result = if matches!(
                    parent_effective,
                    Some(meerkat_core::ops::ToolAccessPolicy::Inherit)
                ) {
                    Err(ToolError::execution_failed(format!(
                        "tool '{tool_name}' parent tool access policy was not resolved at build"
                    )))
                } else {
                    Ok(effective_child_tool_access_policy(
                        requested,
                        parent_effective.clone(),
                    ))
                };
                Box::pin(std::future::ready(result))
            }
            ParentToolAccessPolicySource::LegacySessionMetadata => {
                self.resolve_child_tool_access_policy_from_metadata_boxed(tool_name, requested)
            }
        }
    }

    #[inline(never)]
    fn resolve_child_tool_access_policy_from_metadata_boxed<'a>(
        &'a self,
        tool_name: &'a str,
        requested: Option<meerkat_core::ops::ToolAccessPolicy>,
    ) -> AgentOperationFuture<'a, Result<Option<meerkat_core::ops::ToolAccessPolicy>, ToolError>>
    {
        Box::pin(async move {
            let parent_view = self
                .state
                .session_service()
                .load_persisted_session_metadata(&self.owner_bridge_session_id)
                .await
                .map_err(|error| {
                    ToolError::execution_failed(format!(
                        "tool '{tool_name}' parent tool access policy read failed; \
                         refusing to resolve the child tool access policy: {error}"
                    ))
                })?;
            let parent_effective = parent_view
                .and_then(|view| view.session_metadata)
                .and_then(|metadata| metadata.tooling.tool_access_policy);
            Ok(effective_child_tool_access_policy(
                requested,
                parent_effective,
            ))
        })
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

    // grant_exact_mob_scope_after_create removed — generated mob authority
    // returns a replacement context as a typed session effect. The turn owner
    // (agent loop) commits that projection to session build_state.
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
                &self.owner_bridge_session_id.to_string(),
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
        objective_id: Option<meerkat_core::interaction::ObjectiveId>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        self.ensure_create_authority(call.name).await?;
        let args: DelegateArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
        let result_spec = BoundedResultSpec::new(&args.result_label, args.max_text_bytes)
            .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;

        // Build spawn identity before any implicit-mob mutation. The tool
        // surface must not create durable mob state for a request that has not
        // supplied its substrate-owned member identity.
        let Some(member_id) = args.member_id else {
            return Err(ToolError::invalid_arguments(
                call.name,
                "delegate requires member_id; the tool surface does not allocate member identity",
            ));
        };
        let identity = AgentIdentity::from(member_id);

        let (mob_id, first_delegate) = self
            .ensure_implicit_mob()
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        if let Some(objective_id) = objective_id {
            let owner_identity = self
                .state
                .objective_principal_for_mob_owner_session(&mob_id, &self.owner_bridge_session_id)
                .await
                .map_err(|error| Self::map_mob_error(call, error))?;
            self.state
                .mob_bind_objective_owner(&mob_id, owner_identity, objective_id)
                .await
                .map_err(|error| Self::map_mob_error(call, error))?;
        }

        // Authority grant is returned as a typed effect for the turn owner
        // to merge and commit — no re-entrant session service call.
        // Emit the grant whenever the mob isn't already in scope, not just
        // on first_delegate — a prior failed delegate may have created the
        // implicit mob without the grant effect being applied.
        let mut session_effects = Vec::new();
        let authority_context = self.authority_context_snapshot();
        if !authority_context.can_manage_mob(mob_id.as_str()) {
            let authority_context = meerkat_runtime::mob_operator_authority::grant_manage_mob(
                &authority_context,
                mob_id.as_str(),
            )
            .map_err(|error| {
                ToolError::execution_failed(format!(
                    "{}: generated mob operator authority rejected implicit mob grant: {error}",
                    call.name
                ))
            })?;
            session_effects.push(
                meerkat_core::SessionEffect::ReplaceMobToolAuthorityContext { authority_context },
            );
        }

        // Resolve spawn tooling: default to InheritParent for delegates
        let tooling = args
            .tooling
            .unwrap_or(meerkat_mob::SpawnTooling::InheritParent {
                allow_overlay: None,
                deny_overlay: None,
            });
        let resolved = self.resolve_spawn_tooling(&tooling).await?;

        // Transitive containment: the delegate surface carries no explicit
        // policy, so the helper inherits the parent's resolved policy from
        // the factory authority or the legacy metadata seam.
        let tool_access_policy = self
            .resolve_child_tool_access_policy_boxed(call.name, None)
            .await?;

        let handle = self
            .bound_handle(&mob_id)
            .await
            .map_err(|error| Self::map_mob_error(call, error))?;
        let parent = match (
            self.comms_name.as_ref(),
            self.comms_peer_id,
            self.comms_runtime.as_ref(),
        ) {
            (Some(name), Some(peer_id), Some(runtime)) => Some(DelegationParentContext::new(
                name.clone(),
                peer_id,
                Arc::clone(runtime),
            )),
            _ => None,
        };
        let mut request = DelegationExecutionRequest::new(identity.clone(), args.task, result_spec);
        let mut member = DelegationMemberOptions::default();
        member.placement = lower_wire_placement(args.placement);
        member.additional_instructions = args.additional_instructions.map(|value| vec![value]);
        member.inherited_tool_filter = resolved.inherited_tool_filter;
        member.override_profile = resolved.override_profile;
        member.tool_access_policy = tool_access_policy;
        member.objective_id = objective_id.clone();
        request.member = member;
        request.parent = parent;

        let execution = match DelegationExecutionService::new(handle.clone())
            .execute(request)
            .await
        {
            Ok(execution) => execution,
            Err(DelegationExecutionError::Spawn(error)) => {
                return Err(Self::map_mob_error(call, error));
            }
            Err(DelegationExecutionError::WorkAdmission {
                error,
                retirement_error,
            }) => {
                return Err(ToolError::execution_failed(format!(
                    "tool '{}' failed to admit delegated helper turn: {error}; retirement_error={retirement_error:?}",
                    call.name
                )));
            }
            Err(DelegationExecutionError::Turn {
                error,
                retirement_error,
            }) => {
                return Err(ToolError::execution_failed(format!(
                    "tool '{}' delegated turn failed: {error}; retirement_error={retirement_error:?}",
                    call.name
                )));
            }
            Err(error) => {
                return Err(ToolError::execution_failed(format!(
                    "tool '{}' delegated helper failed: {error}",
                    call.name
                )));
            }
        };
        let turn = execution.turn();

        let mut result = Self::spawn_result_payload(&mob_id, execution.spawn());
        result["mob_id"] = json!(mob_id);
        result["agent_identity"] = json!(identity);
        result["wired"] = json!(execution.wired());
        result["output"] = json!(turn.result().result().text());
        result["tokens_used"] = json!(turn.result().usage().total_tokens());
        result["bounded_result"] = json!(turn.result().result());
        result["session_id"] = json!(turn.result().session_id());
        result["usage"] = json!(turn.result().usage());
        result["turns"] = json!(turn.result().turns());
        result["tool_calls"] = json!(turn.result().tool_calls());
        result["retirement_error"] = json!(execution.retirement_error());

        if first_delegate {
            let notice = "Implicit delegation mob created. The exact helper result was captured \
                          before retirement; the implicit mob persists across turns.";
            result["system_notice"] = json!(notice);
        }

        self.record_successful_operator_action(&handle, call.name)
            .await;

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

        // Compute the operator grant from the *intended* mob id (the definition
        // carries the id) BEFORE the durable create mutation lands, so the
        // create outcome and the grant effect are produced together (row #211).
        // If the generated authority rejects the intended scope, we fail before
        // any mutation lands — there is no "mob created but grant absent" window.
        let intended_mob_id = args.definition.id.clone();
        let authority_context = meerkat_runtime::mob_operator_authority::grant_manage_mob(
            &self.authority_context_snapshot(),
            intended_mob_id.as_str(),
        )
        .map_err(|error| {
            ToolError::execution_failed(format!(
                "{}: generated mob operator authority rejected created mob grant: {error}",
                call.name
            ))
        })?;
        let session_effects =
            vec![meerkat_core::SessionEffect::ReplaceMobToolAuthorityContext { authority_context }];

        let mob_id = mob_create_with_owner_bridge_boxed(
            Arc::clone(&self.state),
            args.definition,
            self.owner_bridge_session_id.clone(),
        )
        .await
        .map_err(|e| Self::map_mob_error(call, e))?;
        if let Ok(handle) = self.bound_handle(&mob_id).await {
            self.record_successful_operator_action_boxed(&handle, call.name)
                .await;
        }
        // The outcome and the grant computed from the same intended mob id are
        // returned as a single atomic effect bundle for the turn owner to commit.
        Self::encode_result_with_effects(call, json!({"mob_id": mob_id}), session_effects)
    }

    #[inline(never)]
    fn record_successful_operator_action_boxed<'a>(
        &'a self,
        handle: &'a MobHandle,
        tool_name: &'a str,
    ) -> AgentOperationFuture<'a, ()> {
        Box::pin(self.record_successful_operator_action(handle, tool_name))
    }

    async fn dispatch_mob_destroy(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: MobIdArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
        let mob_id = MobId::from(args.mob_id.clone());

        self.ensure_mob_scope_authority(call.name, &mob_id).await?;
        let audit_handle = self.bound_handle(&mob_id).await.ok();

        let report = self
            .state
            .mob_destroy(&mob_id)
            .await
            .map_err(|e| Self::map_destroy_error(call, e))?;

        if let Some(handle) = audit_handle.as_ref() {
            self.record_successful_operator_action(handle, call.name)
                .await;
        }

        // Surface the structured destroy report so agents can observe
        // force-destroyed members, orphaned remotes, deadline overruns,
        // and partial cleanup errors rather than getting a bare `ok: true`.
        let report_value = serde_json::to_value(&report).map_err(|e| {
            ToolError::execution_failed(format!(
                "{}: failed to serialize destroy report: {e}",
                call.name,
            ))
        })?;
        let mut body = json!({"ok": true});
        if let Some(obj) = body.as_object_mut() {
            obj.insert("destroy_report".to_string(), report_value);
        }
        Self::encode_result(call, body)
    }

    async fn dispatch_mob_spawn_member(
        &self,
        call: ToolCallView<'_>,
        objective_id: Option<meerkat_core::interaction::ObjectiveId>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: SpawnMemberArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
        let mob_id = MobId::from(args.mob_id.clone());

        self.ensure_spawn_member_scope_boxed(call.name, &mob_id, &args)
            .await?;
        let audit_handle = self
            .bound_handle(&mob_id)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;
        if let Some(objective_id) = objective_id {
            self.bind_spawn_objective_owner_boxed(&mob_id, objective_id)
                .await
                .map_err(|error| Self::map_mob_error(call, error))?;
        }

        let mut spec = SpawnMemberSpec::new(
            ProfileName::from(args.profile),
            AgentIdentity::from(args.member_id),
        );
        spec.initial_message = args.initial_message;
        spec.objective_id = objective_id;
        spec.runtime_mode = args.runtime_mode;
        spec.backend = args.backend;
        spec.placement = lower_wire_placement(args.placement);
        if let Some(auto_wire) = args.auto_wire_parent {
            spec.auto_wire_parent = auto_wire;
        }
        if let Some(tooling) = args.tooling {
            let resolved = self.resolve_spawn_tooling_boxed(&tooling).await?;
            spec.inherited_tool_filter = resolved.inherited_tool_filter;
            spec.override_profile = resolved.override_profile;
        }
        // Transitive containment: this spawn surface accepts no explicit
        // policy argument, so `Inherit`/absent resolves to the parent's
        // effective policy through the selected compatibility source.
        spec.tool_access_policy = self
            .resolve_child_tool_access_policy_boxed(call.name, spec.tool_access_policy.take())
            .await?;
        if let Some(cref) = args.auth_binding {
            // Reconstruct origin: Configured server-side (client cannot forge it).
            spec.auth_binding = Some(cref.into());
        }

        let spawn_result = self
            .spawn_spec_with_generated_owner_context_boxed(&audit_handle, spec)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;
        self.record_successful_operator_action_boxed(&audit_handle, call.name)
            .await;
        Self::encode_result(call, Self::spawn_result_payload(&mob_id, &spawn_result))
    }

    async fn dispatch_fork_off(
        &self,
        call: ToolCallView<'_>,
        objective_id: Option<meerkat_core::interaction::ObjectiveId>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: ForkOffArgs = call
            .parse_args()
            .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
        let (mob_id, source_identity) = self
            .state
            .member_for_bridge_session(&self.owner_bridge_session_id)
            .await
            .map_err(|error| Self::map_mob_error(call, error))?
            .ok_or_else(|| {
                ToolError::execution_failed(
                    "fork_off requires the current session to be a durable mob member",
                )
            })?;
        let audit_handle = self
            .bound_handle(&mob_id)
            .await
            .map_err(|error| Self::map_mob_error(call, error))?;
        let source_entry = audit_handle
            .roster()
            .await
            .get_by_identity(&source_identity)
            .cloned()
            .ok_or_else(|| {
                ToolError::execution_failed(
                    "fork_off resolved the current session binding but its durable roster member is absent",
                )
            })?;
        let authority = self.authority_context_snapshot();
        let observations = meerkat_mob::SpawnMemberAdmissionObservations {
            manage_scope_present: authority.can_manage_mob(mob_id.as_str()),
            profile_scope_contains: authority
                .spawn_profile_scope_contains(mob_id.as_str(), source_entry.role.as_str()),
            ..meerkat_mob::SpawnMemberAdmissionObservations::default()
        };
        let admission = audit_handle
            .resolve_spawn_member_admission(observations)
            .await
            .map_err(|error| Self::map_mob_error(call, error))?;
        if matches!(admission, meerkat_mob::SpawnMemberAdmission::Denied) {
            return Err(ToolError::access_denied(call.name));
        }

        let child_input = fork_off_child_input(&args.task, args.expected_output.as_deref());
        let mut member = SpawnMemberSpec::new(source_entry.role, args.member_id);
        member.initial_message = Some(ContentInput::Text(child_input));
        member.override_profile = source_entry.effective_profile_override;
        member.model_override = source_entry.effective_model_override;
        member.objective_id = objective_id;
        member.tool_access_policy = self
            .resolve_child_tool_access_policy_boxed(call.name, member.tool_access_policy.take())
            .await?;
        let handle = audit_handle.clone();
        let operation_source = source_identity.clone();
        let outcome = meerkat_runtime::stack_relief::relieve_caller_stack(move || async move {
            handle
                .fork_member_then_run_bounded(
                    &operation_source,
                    member,
                    args.message_count,
                    args.result_label,
                    args.max_text_bytes,
                )
                .await
        })
        .await
        .map_err(|error| Self::map_bounded_member_run_error(call, error))?;
        self.record_successful_operator_action_boxed(&audit_handle, call.name)
            .await;
        let identity = outcome.fork.agent_identity.to_string();
        let bounded_result = outcome.turn.result().result().to_wire();
        let result = ForkOffResult {
            mob_id: mob_id.to_string(),
            source_member_id: source_identity.to_string(),
            agent_identity: identity.clone(),
            member_ref: meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), &identity),
            fork_session_id: outcome.fork.session_id.to_string(),
            turn_session_id: outcome.turn.result().session_id().to_string(),
            cache_inheritance: outcome.fork.cache_inheritance,
            bounded_result,
            usage: outcome.turn.result().usage().clone(),
            turns: outcome.turn.result().turns(),
            tool_calls: outcome.turn.result().tool_calls(),
        };
        let value = serde_json::to_value(result).map_err(|error| {
            ToolError::execution_failed(format!(
                "tool '{}' failed to encode durable fork result: {error}",
                call.name
            ))
        })?;
        Self::encode_result(call, value)
    }

    async fn dispatch_council(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        self.ensure_create_authority(call.name).await?;
        let args: CouncilArgs = call
            .parse_args()
            .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
        if args.participants.is_empty() {
            return Err(ToolError::invalid_arguments(
                call.name,
                "a council needs at least one participant",
            ));
        }

        let council_id = match args.council_id {
            Some(council_id) => meerkat_mob::temporary_council::TemporaryCouncilId::new(council_id)
                .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?,
            None => agent_council_id(&self.owner_bridge_session_id, call.id, call.name)?,
        };
        let mut definition = MobDefinition::explicit(MobId::from("agent-council-template"));
        let mut participants = Vec::with_capacity(args.participants.len());
        let mut target_identities = Vec::with_capacity(args.participants.len());

        for (index, participant) in args.participants.into_iter().enumerate() {
            let source_mob_id = MobId::from(participant.mob_id);
            self.ensure_mob_scope_authority(call.name, &source_mob_id)
                .await?;
            let source = self
                .bound_handle(&source_mob_id)
                .await
                .map_err(|error| Self::map_mob_error(call, error))?;
            let source_identity = AgentIdentity::from(participant.member_id);
            let source_profile = source
                .effective_member_profile(&source_identity)
                .await
                .map_err(|error| Self::map_mob_error(call, error))?;
            let source_definition = source.definition();
            merge_council_definition_dependencies(call, &mut definition, source_definition)?;

            let order = u32::try_from(index).map_err(|_| {
                ToolError::invalid_arguments(call.name, "too many council participants")
            })?;
            let target_profile = ProfileName::from(format!("council_p{order}"));
            definition.profiles.insert(
                target_profile.clone(),
                ProfileBinding::Inline(Box::new(source_profile)),
            );
            let target_identity = AgentIdentity::from(format!("council-p{order}"));
            let mut spec = TemporaryCouncilParticipantSpec::new(
                order,
                participant.role,
                source_mob_id,
                source_identity,
                target_identity.clone(),
                target_profile,
            );
            if let Some(prefix_message_count) = participant.prefix_message_count {
                spec = spec.with_prefix_message_count(prefix_message_count);
            }
            target_identities.push(target_identity);
            participants.push(spec);
        }

        let merge_back =
            lower_agent_council_merge(call, args.merge, &target_identities, args.max_result_bytes)?;
        let max_exchanges = args.max_exchanges.unwrap_or_else(|| {
            u32::try_from(participants.len())
                .unwrap_or(u32::MAX)
                .saturating_mul(args.max_rounds)
        });
        let bounds = TemporaryCouncilBounds {
            deadline: TemporaryCouncilDeadline::Relative {
                after: std::time::Duration::from_secs(args.timeout_seconds),
            },
            max_rounds: args.max_rounds,
            max_exchanges,
            max_result_bytes: args.max_result_bytes,
        };
        let durability = match self.state.temporary_council_store().durability() {
            meerkat_mob::temporary_council::TemporaryCouncilStoreDurability::Durable => {
                meerkat_mob::temporary_council::TemporaryCouncilDurability::Durable
            }
            meerkat_mob::temporary_council::TemporaryCouncilStoreDurability::ProcessBound => {
                meerkat_mob::temporary_council::TemporaryCouncilDurability::ProcessBound
            }
            _ => {
                return Err(ToolError::execution_failed(
                    "council custody reported an unsupported durability class",
                ));
            }
        };
        let mut request = TemporaryCouncilRequest::new(
            council_id,
            definition,
            participants,
            args.topic,
            bounds,
            merge_back,
        );
        request.durability = durability;
        let outcome = self
            .state
            .temporary_council()
            .run(request)
            .await
            .map_err(|error| Self::map_council_error(call, error))?;
        Self::encode_result(
            call,
            json!({
                "result": outcome.result,
                "cleanup": outcome.cleanup,
                "replayed": outcome.replayed,
            }),
        )
    }

    #[inline(never)]
    fn ensure_spawn_member_scope_boxed<'a>(
        &'a self,
        tool_name: &'a str,
        mob_id: &'a MobId,
        args: &'a SpawnMemberArgs,
    ) -> AgentOperationFuture<'a, Result<(), ToolError>> {
        Box::pin(self.ensure_spawn_member_scope(tool_name, mob_id, args))
    }

    #[inline(never)]
    fn bind_spawn_objective_owner_boxed<'a>(
        &'a self,
        mob_id: &'a MobId,
        objective_id: meerkat_core::interaction::ObjectiveId,
    ) -> AgentOperationFuture<'a, Result<(), MobError>> {
        Box::pin(async move {
            let owner_identity = self
                .state
                .objective_principal_for_mob_owner_session(mob_id, &self.owner_bridge_session_id)
                .await?;
            self.state
                .mob_bind_objective_owner(mob_id, owner_identity, objective_id)
                .await
        })
    }

    #[inline(never)]
    fn resolve_spawn_tooling_boxed<'a>(
        &'a self,
        tooling: &'a meerkat_mob::SpawnTooling,
    ) -> AgentOperationFuture<'a, Result<ResolvedSpawnTooling, ToolError>> {
        Box::pin(self.resolve_spawn_tooling(tooling))
    }

    #[inline(never)]
    fn spawn_spec_with_generated_owner_context_boxed<'a>(
        &'a self,
        handle: &'a MobHandle,
        spec: SpawnMemberSpec,
    ) -> AgentOperationFuture<'a, Result<meerkat_mob::SpawnResult, MobError>> {
        // Member spawn (owner-binding preparation + the machine spawn
        // command) is reached from inside the calling agent's tool-dispatch
        // poll; run it on its own task so its opt-level=0 poll frames do not
        // stack onto the caller's run-loop chain (2 MiB production
        // worker-stack budget).
        let handle = handle.clone();
        let owner_bridge_session_id = self.owner_bridge_session_id.clone();
        Box::pin(meerkat_runtime::stack_relief::relieve_caller_stack(
            move || async move {
                handle
                    .spawn_spec_with_generated_owner_context(spec, owner_bridge_session_id)
                    .await
            },
        ))
    }

    async fn dispatch_conclude_objective(
        &self,
        call: ToolCallView<'_>,
        objective_id: Option<meerkat_core::interaction::ObjectiveId>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: ConcludeObjectiveArgs = call
            .parse_args()
            .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
        if args.outcome.trim().is_empty() {
            return Err(ToolError::invalid_arguments(
                call.name,
                "outcome must not be empty",
            ));
        }
        let objective_id = objective_id.ok_or_else(|| {
            ToolError::execution_failed(
                "conclude_objective is only available inside an objective-correlated turn",
            )
        })?;
        let (mob_id, identity) = self
            .state
            .objective_principal_for_bridge_session(&self.owner_bridge_session_id)
            .await
            .map_err(|error| Self::map_mob_error(call, error))?
            .ok_or_else(|| {
                ToolError::execution_failed(
                    "conclude_objective could not resolve this session to its objective lead principal",
                )
            })?;
        self.state
            .mob_conclude_objective(&mob_id, &identity, objective_id, args.outcome)
            .await
            .map_err(|error| Self::map_mob_error(call, error))?;
        Self::encode_result(
            call,
            json!({"ok": true, "objective_id": objective_id.to_string()}),
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
            .bound_handle(&mob_id)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        self.state
            .mob_retire(&mob_id, AgentIdentity::from(args.member_id))
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

        let identity = AgentIdentity::from(args.member_id);
        let snapshot = self
            .state
            .mob_member_status(&mob_id, &identity)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        let member_ref =
            meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), identity.as_str());
        let result = snapshot.to_member_status_result(member_ref).map_err(|e| {
            ToolError::invalid_arguments(
                call.name,
                format!("failed to project mob member status: {e}"),
            )
        })?;
        Self::encode_result(call, json!(result))
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
        let mobs = self
            .state
            .mob_list()
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;
        // Lower each per-mob visibility decision through MobMachine, exactly
        // as the mutating-dispatch siblings do (see `ensure_mob_scope_authority`).
        // MobMachine — not this surface — owns the Allow/Deny verdict; we extract
        // only the raw `can_manage_mob` observation and mirror the machine's
        // ruling. Fails closed: a mob whose handle cannot resolve, or whose
        // admission errors or is Denied, is omitted.
        let mut mob_list: Vec<serde_json::Value> = Vec::new();
        for (id, status) in mobs {
            let can_manage_mob = authority_context.can_manage_mob(id.as_str());
            let Ok(handle) = self.bound_handle(&id).await else {
                continue;
            };
            match handle.resolve_current_mob_admission(can_manage_mob).await {
                Ok(meerkat_mob::CurrentMobAdmission::Allowed) => {
                    mob_list.push(json!({
                        "mob_id": id,
                        "status": status.as_str(),
                    }));
                }
                Ok(meerkat_mob::CurrentMobAdmission::Denied) | Err(_) => {}
            }
        }

        Self::encode_result(call, json!({"mobs": mob_list}))
    }
    // ─── Profile CRUD dispatch ────────────────────────────────────────
    // ─── Wire / Unwire ────────────────────────────────────────────────

    async fn dispatch_mob_wire(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: WireArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
        let mob_id = meerkat_mob::MobId::from(args.mob_id.as_str());
        self.ensure_mob_scope_authority(call.name, &mob_id).await?;
        let audit_handle = self
            .bound_handle(&mob_id)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        let local = AgentIdentity::from(args.member_id.as_str());
        let target = wire_peer_target_from_args(args.peer);
        self.state
            .mob_wire(&mob_id, local, target)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;
        self.record_successful_operator_action(&audit_handle, call.name)
            .await;
        Self::encode_result(call, json!({ "wired": true }))
    }

    async fn dispatch_mob_unwire(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let args: UnwireArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
        let mob_id = meerkat_mob::MobId::from(args.mob_id.as_str());
        self.ensure_mob_scope_authority(call.name, &mob_id).await?;
        let audit_handle = self
            .bound_handle(&mob_id)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;

        let local = AgentIdentity::from(args.member_id.as_str());
        let target = unwire_peer_target_from_args(args.peer)?;
        self.state
            .mob_unwire(&mob_id, local, target)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;
        self.record_successful_operator_action(&audit_handle, call.name)
            .await;
        Self::encode_result(call, json!({ "unwired": true }))
    }

    async fn dispatch_mob_profile_create(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        self.ensure_profile_mutation_authority(call.name).await?;
        let args: ProfileCreateArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
        let stored = self
            .state
            .realm_profile_create(&args.name, &args.profile)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;
        Self::encode_result(
            call,
            json!(meerkat_mob::stored_realm_profile_to_wire(&stored)),
        )
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
            Some(profile) => Self::encode_result(
                call,
                json!(meerkat_mob::stored_realm_profile_to_wire(&profile)),
            ),
            None => Self::encode_result(
                call,
                json!(meerkat_contracts::MobProfileLookupResult {
                    not_found: true,
                    name: args.name,
                    profile: None,
                    revision: None,
                    created_at: None,
                    updated_at: None,
                }),
            ),
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
        let profiles = profiles
            .iter()
            .map(meerkat_mob::stored_realm_profile_to_wire)
            .collect();
        Self::encode_result(
            call,
            json!(meerkat_contracts::MobProfileListResult { profiles }),
        )
    }

    async fn dispatch_mob_profile_update(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        self.ensure_profile_mutation_authority(call.name).await?;
        let args: ProfileUpdateArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
        let stored = self
            .state
            .realm_profile_update(&args.name, &args.profile, args.expected_revision)
            .await
            .map_err(|e| Self::map_mob_error(call, e))?;
        Self::encode_result(
            call,
            json!(meerkat_mob::stored_realm_profile_to_wire(&stored)),
        )
    }

    async fn dispatch_mob_profile_delete(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        self.ensure_profile_mutation_authority(call.name).await?;
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
                    (kind_str, p.source_id.to_string())
                }
                None => ("unknown".to_string(), "unknown".to_string()),
            };
            groups
                .entry((kind, source_id))
                .or_default()
                .push(tool.name.to_string());
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

    boxed_agent_dispatch!(
        dispatch_delegate_boxed,
        dispatch_delegate,
        objective_id: Option<meerkat_core::interaction::ObjectiveId>
    );
    boxed_agent_dispatch!(
        dispatch_conclude_objective_boxed,
        dispatch_conclude_objective,
        objective_id: Option<meerkat_core::interaction::ObjectiveId>
    );
    boxed_agent_dispatch!(dispatch_mob_create_boxed, dispatch_mob_create);
    boxed_agent_dispatch!(dispatch_mob_destroy_boxed, dispatch_mob_destroy);
    boxed_agent_dispatch!(
        dispatch_mob_spawn_member_boxed,
        dispatch_mob_spawn_member,
        objective_id: Option<meerkat_core::interaction::ObjectiveId>
    );
    boxed_agent_dispatch!(
        dispatch_fork_off_boxed,
        dispatch_fork_off,
        objective_id: Option<meerkat_core::interaction::ObjectiveId>
    );
    boxed_agent_dispatch!(dispatch_council_boxed, dispatch_council);
    boxed_agent_dispatch!(dispatch_mob_retire_member_boxed, dispatch_mob_retire_member);
    boxed_agent_dispatch!(dispatch_mob_check_member_boxed, dispatch_mob_check_member);
    boxed_agent_dispatch!(dispatch_mob_list_members_boxed, dispatch_mob_list_members);
    boxed_agent_dispatch!(dispatch_mob_list_boxed, dispatch_mob_list);
    boxed_agent_dispatch!(dispatch_mob_wire_boxed, dispatch_mob_wire);
    boxed_agent_dispatch!(dispatch_mob_unwire_boxed, dispatch_mob_unwire);
    boxed_agent_dispatch!(
        dispatch_mob_profile_create_boxed,
        dispatch_mob_profile_create
    );
    boxed_agent_dispatch!(dispatch_mob_profile_get_boxed, dispatch_mob_profile_get);
    boxed_agent_dispatch!(dispatch_mob_profile_list_boxed, dispatch_mob_profile_list);
    boxed_agent_dispatch!(
        dispatch_mob_profile_update_boxed,
        dispatch_mob_profile_update
    );
    boxed_agent_dispatch!(
        dispatch_mob_profile_delete_boxed,
        dispatch_mob_profile_delete
    );
    boxed_agent_dispatch!(
        dispatch_mob_profile_list_sources_boxed,
        dispatch_mob_profile_list_sources
    );

    async fn dispatch_with_context_stack_bounded(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        let objective_id = context
            .turn_metadata(meerkat_core::agent::TOOL_DISPATCH_OBJECTIVE_ID_KEY)
            .and_then(serde_json::Value::as_str)
            .map(uuid::Uuid::parse_str)
            .transpose()
            .map_err(|error| {
                ToolError::execution_failed(format!(
                    "{}: invalid objective correlation in dispatch context: {error}",
                    call.name
                ))
            })?
            .map(meerkat_core::interaction::ObjectiveId);
        match call.name {
            TOOL_DELEGATE => self.dispatch_delegate_boxed(call, objective_id).await,
            TOOL_CONCLUDE_OBJECTIVE => {
                self.dispatch_conclude_objective_boxed(call, objective_id)
                    .await
            }
            TOOL_MOB_CREATE => self.dispatch_mob_create_boxed(call).await,
            TOOL_MOB_DESTROY => self.dispatch_mob_destroy_boxed(call).await,
            TOOL_MOB_SPAWN_MEMBER => {
                self.dispatch_mob_spawn_member_boxed(call, objective_id)
                    .await
            }
            TOOL_FORK_OFF => self.dispatch_fork_off_boxed(call, objective_id).await,
            TOOL_COUNCIL => self.dispatch_council_boxed(call).await,
            TOOL_MOB_RETIRE_MEMBER => self.dispatch_mob_retire_member_boxed(call).await,
            TOOL_MOB_CHECK_MEMBER => self.dispatch_mob_check_member_boxed(call).await,
            TOOL_MOB_LIST_MEMBERS => self.dispatch_mob_list_members_boxed(call).await,
            TOOL_MOB_LIST => self.dispatch_mob_list_boxed(call).await,
            TOOL_MOB_WIRE => self.dispatch_mob_wire_boxed(call).await,
            TOOL_MOB_UNWIRE => self.dispatch_mob_unwire_boxed(call).await,
            TOOL_MOB_PROFILE_CREATE => self.dispatch_mob_profile_create_boxed(call).await,
            TOOL_MOB_PROFILE_GET => self.dispatch_mob_profile_get_boxed(call).await,
            TOOL_MOB_PROFILE_LIST => self.dispatch_mob_profile_list_boxed(call).await,
            TOOL_MOB_PROFILE_UPDATE => self.dispatch_mob_profile_update_boxed(call).await,
            TOOL_MOB_PROFILE_DELETE => self.dispatch_mob_profile_delete_boxed(call).await,
            TOOL_MOB_PROFILE_LIST_SOURCES => {
                self.dispatch_mob_profile_list_sources_boxed(call).await
            }
            _ => Err(ToolError::not_found(call.name)),
        }
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
        if !authority_context.is_generated_authority_context() {
            return Ok(Arc::new(EmptyAgentToolSurface));
        }
        let session_id_str = args.session_id.to_string();
        let implicit_mob_id = self
            .state
            .find_implicit_mob_for_bridge_session(&session_id_str)
            .await;

        // Extract parent canonical comms identity for wiring helpers.
        let comms_peer_id = args.comms_runtime.as_ref().and_then(|r| r.peer_id());
        // Use the shared effective-authority handle if provided (runtime-backed
        // sessions). The agent/turn owner updates this handle via
        // apply_session_effects; mob tools read from it for authorization.
        // Falls back to a local handle for non-runtime paths.
        let effective_authority_handle = args
            .effective_authority
            .unwrap_or_else(|| Arc::new(std::sync::RwLock::new(authority_context)));
        let parent_tool_access_policy_source = match &args.snapshot_context {
            meerkat_core::service::MobToolSnapshotContext::ParentOwned(authority) => {
                ParentToolAccessPolicySource::Resolved(authority.resolved_tool_access_policy())
            }
            meerkat_core::service::MobToolSnapshotContext::Standalone => {
                ParentToolAccessPolicySource::LegacySessionMetadata
            }
        };
        if matches!(
            &parent_tool_access_policy_source,
            ParentToolAccessPolicySource::Resolved(Some(
                meerkat_core::ops::ToolAccessPolicy::Inherit
            ))
        ) {
            return Err(std::io::Error::other(
                "mob tool creator policy must be resolved before surface construction",
            )
            .into());
        }
        let surface = AgentMobToolSurface::new_with_effective_authority_and_policy_source(
            Arc::clone(&self.state),
            implicit_mob_id,
            effective_authority_handle,
            parent_tool_access_policy_source,
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
        self.dispatch_with_context(call, &meerkat_core::ToolDispatchContext::default())
            .await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
        self.dispatch_with_context_stack_bounded(call, context)
            .await
    }

    fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
        meerkat_core::agent::DispatcherCapabilities {
            ops_lifecycle: true,
        }
    }

    fn bind_ops_lifecycle(
        self: Arc<Self>,
        _registry: Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
        owner_bridge_session_id: SessionId,
    ) -> Result<meerkat_core::agent::BindOutcome, meerkat_core::agent::OpsLifecycleBindError> {
        if Arc::strong_count(&self) != 1 {
            return Err(meerkat_core::agent::OpsLifecycleBindError::SharedOwnership);
        }
        let this = Arc::try_unwrap(self)
            .map_err(|_| meerkat_core::agent::OpsLifecycleBindError::SharedOwnership)?;
        Ok(meerkat_core::agent::BindOutcome::Bound(Arc::new(Self {
            state: this.state,
            cached_implicit_mob_id: this.cached_implicit_mob_id,
            effective_authority: this.effective_authority,
            parent_tool_access_policy_source: this.parent_tool_access_policy_source,
            control_principal: this.control_principal,
            tools: this.tools,
            owner_bridge_session_id,
            model: this.model,
            comms_name: this.comms_name,
            comms_peer_id: this.comms_peer_id,
            comms_runtime: this.comms_runtime,
            snapshot_context: this.snapshot_context,
        })))
    }
}

// ─── Tool definitions ────────────────────────────────────────────────────

fn tool_def(name: &str, description: &str, input_schema: serde_json::Value) -> Arc<ToolDef> {
    Arc::new(ToolDef {
        name: name.into(),
        description: description.to_string(),
        input_schema,
        provenance: Some(ToolProvenance {
            kind: ToolSourceKind::Mob,
            source_id: "mob".into(),
        }),
    })
}

/// Single schema source for every agent-facing mob tool: derive the
/// `input_schema` from the same typed argument struct that
/// [`AgentMobToolSurface`] deserializes from (`schema_for!`). This mirrors the
/// public MCP surface (`public_mcp::typed_schema`) and removes the
/// hand-authored `json!` schema literals that could drift from the deserialize
/// path (remediation rows #32/#157).
fn typed_schema<T: JsonSchema>() -> Value {
    // K1: ONE infallible schema generator (the core schema module owns the
    // tool-input schema contract) — no fail-open `{"type": "object"}` null
    // schema fallback.
    meerkat_core::schema::tool_input_schema_for::<T>()
}

#[cfg(test)]
fn build_tool_defs() -> Arc<[Arc<ToolDef>]> {
    build_tool_defs_with_profile_support(false, false, false)
}

fn build_tool_defs_with_profile_support(
    has_profile_store: bool,
    has_snapshot_provider: bool,
    _can_run_adaptive_packs: bool,
) -> Arc<[Arc<ToolDef>]> {
    let mut defs = vec![
        tool_def(
            TOOL_DELEGATE,
            "Delegate a task to a helper agent.\n\n\
             WHAT IT DOES:\n\
             Creates a disposable helper agent that runs your task autonomously. On first call, \
             an implicit mob is created behind the scenes to manage helpers. Each subsequent call \
             spawns a new helper into that same mob. Helpers are wired back to you for comms \
             (messaging) when your session has a comms identity; the response's wired field \
             tells you whether that wiring succeeded. Helpers run independently -- they do not \
             share your session or memory.\n\n\
             HELPERS ARE DISPOSABLE:\n\
             Each helper gets its own session that exists only for the delegated task. Helpers \
             do not persist between your turns. Once a helper completes or is retired, its session \
             is archived. Use delegate for work you want done and reported back, not for \
             long-lived collaborators (use mob_create + mob_spawn_member for those).\n\n\
             WHEN TO USE DELEGATE vs MOB_*:\n\
             - delegate: Quick one-off tasks with an exact bounded result returned by the call.\n\
             - mob_create + mob_spawn_member: Multi-member teams, custom wiring/flows, reusable \
               profiles, long-lived collaborators, or when you need fine-grained control over \
               backend, runtime_mode, and topology.\n\n\
             TOOLING OPTIONS:\n\
             By default, the helper inherits your current tool set (inherit_parent mode). \
             Override via the tooling parameter:\n\
             - {\"mode\":\"inherit_parent\"} -- default. Helper gets your tools. Add \
               allow_overlay/deny_overlay arrays to narrow the set.\n\
             - {\"mode\":\"minimal\"} -- comms tools only (send, send_message, \
               reply_to_peer, send_request, send_response, peers). Lightweight helper.\n\
             - {\"mode\":\"profile\",\"source\":{\"type\":\"realm_profile\",\"name\":\"my-profile\"}} \
               -- use a saved realm profile for model + tool config.\n\
             - {\"mode\":\"profile\",\"source\":{\"type\":\"inline\",\"model\":\"claude-sonnet-4-6\",\
               \"tools\":{\"builtins\":true,\"shell\":true,\"comms\":true}}} -- inline profile.\n\n\
             RESULT CONTRACT:\n\
             The call returns only after the exact admitted helper turn reaches its committed \
             terminal boundary. result_label names the returned bounded result and \
             max_text_bytes sets its UTF-8 byte ceiling. The response includes explicit \
             truncation state, exact session attribution, usage, and any retirement cleanup debt.\n\n\
             EXAMPLES:\n\
             1. Quick one-off: {\"task\": \"Summarize the README.md file and send me the result\"}\n\
             2. Longer task with polling: {\"task\": \"Run the full test suite and report failures\", \
                \"member_id\": \"test-runner\", \"tooling\": {\"mode\": \"inherit_parent\", \
                \"deny_overlay\": [\"delegate\"]}} -- then later call mob_check_member to see \
                the result.",
            typed_schema::<DelegateArgs>(),
        ),
        tool_def(
            TOOL_CONCLUDE_OBJECTIVE,
            "Conclude the current kickoff objective with its final answer. The objective and member are pre-addressed from this turn; supply only the final outcome.",
            typed_schema::<ConcludeObjectiveArgs>(),
        ),
        tool_def(
            TOOL_MOB_CREATE,
            "Create a new explicit mob with full control over profiles, wiring, and flows.\n\n\
             WHAT IS A MOB:\n\
             A mob is a managed group of agent members that can communicate, share tasks, and \
             coordinate via wiring rules. Unlike delegate (which creates a temporary implicit mob), \
             mob_create gives you full control over the mob's lifecycle, member profiles, \
             communication topology, and execution flows.\n\n\
             WHEN TO USE mob_create vs delegate:\n\
             - Use delegate for quick one-off helpers that run a single task and are discarded.\n\
             - Use mob_create when you need: multiple coordinated members, custom communication \
               wiring, flow-based execution (repeat_until loops), named profiles for role-based \
               spawning, or long-lived teams that persist across interactions.\n\n\
             KEY DEFINITION FIELDS:\n\
             - id: Unique mob identifier (string).\n\
             - profiles: Named role templates (inline or realm profile references) that members \
               are spawned from. Each profile specifies model, tools, skills, and runtime_mode.\n\
             - wiring: Rules for automatic peer connections between members (e.g., hub-spoke, \
               mesh, or custom patterns).\n\
             - flows: Named flow definitions for structured execution (e.g., repeat_until loops).\n\
             - backend: Default backend for members. \"session\" (default) runs within the session \
               runtime. \"external\" delegates to an external process.\n\
             - topology: Optional role dispatch policy.\n\n\
             TYPICAL WORKFLOW:\n\
             1. mob_create with profiles and wiring rules.\n\
             2. mob_spawn_member for each role (e.g., \"researcher\", \"writer\").\n\
             3. mob_check_member or mob_list_members to monitor progress.\n\
             4. mob_retire_member when a member's work is done.\n\
             5. mob_destroy to clean up the mob and all remaining members.",
            typed_schema::<MobCreateArgs>(),
        ),
        tool_def(
            TOOL_MOB_DESTROY,
            "Destroy an explicit mob and archive all its members' sessions.\n\n\
             Only works on mobs created via mob_create. Cannot destroy implicit mobs created \
             by delegate (those are cleaned up automatically when your session ends). \
             Retire individual members first if you need their final output before destroying.",
            typed_schema::<MobIdArgs>(),
        ),
        tool_def(
            TOOL_MOB_SPAWN_MEMBER,
            "Spawn a new member into an explicit mob from a named profile.\n\n\
             The profile parameter references a role name defined in the mob's definition.profiles \
             map (set during mob_create). The member inherits the profile's model, tools, skills, \
             and runtime_mode unless overridden here.\n\n\
             RUNTIME_MODE:\n\
             - \"autonomous_host\" (default): The member runs autonomously in a long-lived host \
               loop. It processes its initial_message and any subsequent comms messages without \
               further prompting from you. Best for workers that run to completion on their own.\n\
             - \"turn_driven\": The member only runs when explicitly given a turn. Use for \
               members you want to control step-by-step.\n\n\
             BACKEND:\n\
             - \"session\" (default): Member runs within the session runtime. Supports full \
               session persistence, compaction, and event streaming.\n\
             - \"external\": Member delegates execution to an external process.\n\n\
             AUTO_WIRE_PARENT:\n\
             When true, the spawned member is automatically wired as a trusted peer of the \
             mob's orchestrator, enabling bidirectional comms immediately after spawn. When \
             false (default), you must wire peers manually or rely on the mob's wiring rules.\n\n\
             TOOLING OVERRIDE:\n\
             If provided, overrides the profile's model/tool config for this specific member. \
             Same options as delegate's tooling parameter: inherit_parent, minimal, or profile \
             (realm_profile or inline). Useful for spawning the same role with different models \
             or restricted tool sets.\n\n\
             You can spawn multiple members from the same profile with different member_ids \
             to create parallel workers (e.g., spawn 3 \"researcher\" members with different tasks).",
            typed_schema::<SpawnMemberArgs>(),
        ),
        tool_def(
            TOOL_FORK_OFF,
            "Delegate one task through a real durable transcript fork.\n\n\
             Unlike delegate, the child starts from an exact committed prefix of an existing mob member's transcript. Unlike prompt-context fork_helper, this persists a real child session and provisions it through the ordinary resume path. The tool visibly commits the task and expected-output guidance as the child input, captures ordinary final assistant text under the caller's byte bound, and retains the child in the normal mob roster.\n\n\
             The parent remains responsible for replying to the user. The child does not autonomously deliver across sessions. The returned bounded result is ordinary final text with explicit status and truncation, not a validated summary or report.",
            typed_schema::<ForkOffArgs>(),
        ),
        tool_def(
            TOOL_COUNCIL,
            "Convene a bounded council of existing mob members for decision support.\n\n\
             Each participant is forked at its source execution owner, preserving that member's \
             transcript prefix, tools, auth, realm, and filesystem boundaries. The forks are seated \
             in a real short-lived mob, wired together, run for bounded sequential rounds, and \
             cleaned up automatically. Use this when a decision benefits from several existing \
             specialists debating from their native contexts; use delegate for independent one-off \
             work and fork_off for one durable continuation of the current member.\n\n\
             Supply source participants as mob_id/member_id plus the role they should play in the \
             discussion. The tool resolves and copies their existing profiles; you do not construct \
             a temporary mob definition. The default merge asks the last participant for a bounded \
             summary. council_id is optional and should be supplied only when you need an explicit \
             idempotency key across retries.",
            typed_schema::<CouncilArgs>(),
        ),
        tool_def(
            TOOL_MOB_RETIRE_MEMBER,
            "Retire a mob member and archive its session.\n\n\
             Retirement is graceful: the member's session is archived (preserving its history) \
             and it is removed from the mob roster. The member can no longer receive messages \
             or run turns after retirement. Use mob_check_member first if you need the member's \
             final output before retiring it.\n\n\
             Retired members cannot be re-spawned. To replace a retired member, spawn a new \
             one with a different member_id using the same profile.",
            typed_schema::<MemberArgs>(),
        ),
        tool_def(
            TOOL_MOB_CHECK_MEMBER,
            "Check a member's execution status, output preview, and token usage.\n\n\
             Returns the member's current state: running, completed, or failed, along with \
             a preview of its latest output and cumulative token usage.\n\n\
             POLLING GUIDANCE:\n\
             Members in autonomous_host mode run asynchronously. Rather than polling in a \
             tight loop, check at reasonable intervals (e.g., after completing your own work \
             steps). For multi-member mobs, use mob_list_members to get all statuses at once \
             instead of checking each member individually.\n\n\
             PUSH vs PULL:\n\
             Members can proactively send you results via comms (send_message/send_request). \
             If you gave the member instructions to report back when done, wait for its comms \
             message rather than polling. Use mob_check_member as a fallback or when you need \
             token usage information that comms messages do not include.\n\n\
             COST/PERFORMANCE:\n\
             This call is lightweight (reads from local state, no LLM calls). Safe to call \
             frequently, but unnecessary polling wastes your own turns.",
            typed_schema::<MemberArgs>(),
        ),
        tool_def(
            TOOL_MOB_LIST_MEMBERS,
            "List all members of a mob with their status and session info.\n\n\
             Returns each member's id, profile, status (running/completed/failed), runtime_mode, \
             and session metadata. More efficient than calling mob_check_member on each member \
             individually when you need a status overview of the whole mob.",
            typed_schema::<MobIdArgs>(),
        ),
        tool_def(
            TOOL_MOB_LIST,
            "List all mobs managed by this agent.\n\n\
             Returns both explicit mobs (created via mob_create) and implicit mobs (created \
             automatically by delegate). Each entry includes the mob_id, member count, and \
             creation metadata. Use this to discover mob_ids for mob_list_members or mob_destroy.",
            typed_schema::<NoParamsArgs>(),
        ),
        tool_def(
            TOOL_MOB_WIRE,
            "Wire a mob member to another local member or an external binding.\n\n\
             Creates a comms trust relationship so the wired members can exchange messages. \
             For local members (both in the same mob roster), wiring is bidirectional. \
             For external peers (outside the roster), trust is added on the local member's side.\n\n\
             PEER TARGET TYPES:\n\
             - {\"local\": \"member-id\"} — another member in the same mob roster.\n\
             - {\"external_binding\": {\"name\": \"peer-name\", \"address\": \"tcp://host:port\", \
               \"identity\": {\"kind\": \"ed25519_public_key\", \"public_key\": \"ed25519:...\"}}} \
               — a typed external binding request resolved by the mob authority.",
            typed_schema::<WireArgs>(),
        ),
        tool_def(
            TOOL_MOB_UNWIRE,
            "Remove a wiring relationship between a mob member and a peer.\n\n\
             Removes the comms trust relationship established by mob_wire. \
             Use {\"local\": \"member-id\"} for roster peers or \
             {\"external\": {\"name\": \"peer-name\"}} for an external binding handle.",
            typed_schema::<UnwireArgs>(),
        ),
    ];

    if has_profile_store {
        defs.push(tool_def(
            TOOL_MOB_PROFILE_CREATE,
            "Create a new realm profile -- a reusable template for spawning mob members.\n\n\
             WHAT IS A PROFILE:\n\
             A realm profile defines the model, tool surface, skills, peer description, backend, \
             and runtime mode for a mob member. Once created, it can be referenced by name when \
             spawning members via mob_spawn_member or delegate (tooling.source.type = \
             \"realm_profile\"). This avoids repeating the same configuration across multiple spawns.\n\n\
             WHEN TO USE PROFILES vs DELEGATE:\n\
             - delegate with no tooling: Quick one-off, inherits your tools. No profile needed.\n\
             - delegate with inline tooling: One-off with custom model/tools. No profile needed.\n\
             - Profiles: When you spawn multiple members with the same config (e.g., 5 workers \
               all using the same model + tools), or when you want to version and update the \
               config independently of spawn calls.\n\n\
             PROFILE FIELDS:\n\
             - model (required): LLM model name, e.g. \"claude-sonnet-4-5\".\n\
             - tools: {builtins: bool, shell: bool, comms: bool, memory: bool, mob: bool, \
               schedule: bool, image_generation: bool, read_only: bool}. Each defaults to false. \
               read_only enforces at the execution gate that the member may only call tools \
               declared read-only (shell and MCP tools are refused; a spawn cannot widen it).\n\
             - skills: Array of skill names to load.\n\
             - peer_description: Human-readable role description visible to other members.\n\
             - runtime_mode: \"autonomous_host\" (default) or \"turn_driven\".\n\
             - backend: \"session\" (default) or \"external\".\n\
             - external_addressable: Whether the member can receive turns from external callers.\n\n\
             EXAMPLE PROFILE:\n\
             {\"model\": \"claude-sonnet-4-5\", \"tools\": {\"builtins\": true, \"shell\": true, \
             \"comms\": true}, \"skills\": [\"code-review\"], \"peer_description\": \"Code reviewer \
             that analyzes PRs\", \"runtime_mode\": \"autonomous_host\"}\n\n\
             LIFECYCLE:\n\
             1. mob_profile_create -- creates the profile (returns revision 0).\n\
             2. mob_profile_get -- read back the profile and its current revision.\n\
             3. mob_profile_update -- modify the profile (requires expected_revision for safety).\n\
             4. mob_profile_delete -- remove the profile when no longer needed.\n\n\
             REUSE ACROSS SPAWNS:\n\
             After creating a profile named \"researcher\", spawn multiple members from it:\n\
             - delegate with tooling: {\"mode\":\"profile\",\"source\":{\"type\":\"realm_profile\",\
               \"name\":\"researcher\"}}\n\
             - mob_spawn_member referencing the profile in the mob definition, or with tooling \
               override pointing to the realm profile.",
            typed_schema::<ProfileCreateArgs>(),
        ));
        defs.push(tool_def(
            TOOL_MOB_PROFILE_GET,
            "Get a realm profile by name.\n\n\
             Returns the full profile definition and its current revision number. The revision \
             is needed for mob_profile_update and mob_profile_delete (they require \
             expected_revision to prevent concurrent modification).\n\n\
             INHERITANCE AND OVERRIDES:\n\
             The returned profile shows the stored configuration. When spawning a member, you \
             can further narrow the tool set using allow_overlay/deny_overlay in the tooling \
             parameter without modifying the stored profile. For example, a profile with \
             {\"tools\": {\"builtins\": true, \"shell\": true, \"comms\": true}} can be spawned \
             with deny_overlay: [\"shell_exec\"] to create a member that has builtins and comms \
             but not shell access.",
            typed_schema::<ProfileNameArgs>(),
        ));
        defs.push(tool_def(
            TOOL_MOB_PROFILE_LIST,
            "List all realm profiles.\n\n\
             Returns the name and revision of each stored profile. Use mob_profile_get to \
             retrieve the full definition of a specific profile. Useful for discovering \
             available profiles before spawning members or before creating a new profile \
             to check for name conflicts.",
            typed_schema::<NoParamsArgs>(),
        ));
        defs.push(tool_def(
            TOOL_MOB_PROFILE_UPDATE,
            "Update a realm profile. Requires expected_revision for safe concurrent updates.\n\n\
             WHAT IS expected_revision:\n\
             Every profile has a revision number that increments on each update. You must pass \
             the current revision (from mob_profile_get or the last create/update response) as \
             expected_revision. If the stored revision does not match, the update is rejected \
             with a conflict error -- this prevents you from accidentally overwriting changes \
             made by another agent or process. On conflict, re-read the profile with \
             mob_profile_get, merge your changes with the current state, and retry with the \
             new revision.\n\n\
             The profile field is a full replacement, not a merge. Include all fields you want \
             to keep, not just the ones you are changing.\n\n\
             ALREADY-SPAWNED MEMBERS:\n\
             Updating a profile does not affect members already spawned from it. Only future \
             spawns will use the updated configuration.",
            typed_schema::<ProfileUpdateArgs>(),
        ));
        defs.push(tool_def(
            TOOL_MOB_PROFILE_DELETE,
            "Delete a realm profile.\n\n\
             Requires expected_revision (same as mob_profile_update) to prevent deleting a \
             profile that was modified since you last read it. Get the current revision via \
             mob_profile_get before deleting.\n\n\
             Deleting a profile does not affect members already spawned from it -- they continue \
             running with the configuration they were spawned with. However, future spawn \
             attempts referencing this profile name will fail until a new profile with the \
             same name is created.",
            typed_schema::<ProfileDeleteArgs>(),
        ));
    }

    if has_profile_store && has_snapshot_provider {
        defs.push(tool_def(
            TOOL_MOB_PROFILE_LIST_SOURCES,
            "List visible tool sources grouped by provenance (kind and source).\n\n\
             Returns all tool sources available to you, organized by where they come from: \
             built-in tools, MCP servers, mob tools, comms tools, etc. Each group shows the \
             source kind, source identifier, and the tool names it provides.\n\n\
             Use this to discover what tools are available before creating profiles. When \
             building a profile's tools config or setting up allow_overlay/deny_overlay \
             filters, this tells you the exact tool names you can reference.",
            typed_schema::<NoParamsArgs>(),
        ));
    }

    defs.into()
}

// ─── Argument types ──────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
struct DelegateArgs {
    /// The task description/prompt for the helper.
    task: String,
    /// Label attached to the exact bounded result.
    result_label: String,
    /// Maximum UTF-8 bytes returned in the exact result, including any
    /// explicit truncation marker.
    max_text_bytes: usize,
    /// Unique identifier for this helper (required; the tool surface does
    /// not allocate member identity).
    #[serde(default)]
    member_id: Option<String>,
    /// Bound host peer ID for placed execution; omit for the controlling host.
    #[serde(default)]
    placement: Option<WireHostRef>,
    /// Extra instructions appended to the helper's system prompt.
    #[serde(default)]
    additional_instructions: Option<String>,
    /// Controls the helper tool surface. Use {"mode":"inherit_parent"} to
    /// inherit current tools, optionally with allow_overlay/deny_overlay. Use
    /// mode=profile with an inline source to request explicit model/tools.
    #[serde(default)]
    #[schemars(with = "serde_json::Value")]
    tooling: Option<meerkat_mob::SpawnTooling>,
}

#[derive(Deserialize, JsonSchema)]
struct ForkOffArgs {
    /// Stable identity for the child fork. The surface never allocates one.
    member_id: String,
    /// Caller-authored work to perform on the durable fork.
    task: String,
    /// Optional visible guidance for how the child should present its final
    /// answer. This is prompt text, not a validated output schema.
    #[serde(default)]
    expected_output: Option<String>,
    /// Optional committed transcript prefix length. Omit to fork the source's
    /// exact current committed end.
    #[serde(default)]
    message_count: Option<usize>,
    /// Label attached to the exact bounded final assistant text.
    #[serde(default = "fork_off_default_result_label")]
    result_label: String,
    /// Maximum UTF-8 bytes returned, including any truncation marker.
    #[serde(default = "fork_off_default_max_text_bytes")]
    max_text_bytes: usize,
}

#[derive(Deserialize, JsonSchema)]
struct CouncilArgs {
    /// Optional explicit idempotency key. Omit to derive one from this tool call.
    #[serde(default)]
    council_id: Option<String>,
    /// Decision, question, or proposal the council should examine.
    topic: String,
    /// Existing mob members whose native contexts should participate.
    participants: Vec<CouncilParticipantArgs>,
    /// Sequential discussion rounds.
    #[serde(default = "council_default_rounds")]
    max_rounds: u32,
    /// Total participant turns. Defaults to participants × rounds.
    #[serde(default)]
    max_exchanges: Option<u32>,
    /// Per-exchange UTF-8 result ceiling.
    #[serde(default = "council_default_result_bytes")]
    max_result_bytes: usize,
    /// Absolute execution budget for discussion, merge, and cleanup.
    #[serde(default = "council_default_timeout_seconds")]
    timeout_seconds: u64,
    /// Explicit result merge policy. Defaults to a bounded summary by the last participant.
    #[serde(default)]
    merge: CouncilMergeArgs,
}

#[derive(Deserialize, JsonSchema)]
struct CouncilParticipantArgs {
    /// Mob containing the source member.
    mob_id: String,
    /// Existing source member identity.
    member_id: String,
    /// Role this participant should play in the discussion.
    role: String,
    /// Optional exact committed transcript prefix length; omit for the current head.
    #[serde(default)]
    prefix_message_count: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "policy", rename_all = "snake_case")]
enum CouncilMergeArgs {
    Summary {
        /// Participant index asked to synthesize the decision. Defaults to the last participant.
        #[serde(default)]
        finalizer: Option<u32>,
        /// UTF-8 byte ceiling for the summary.
        #[serde(default)]
        max_bytes: Option<usize>,
    },
    Structured {
        /// Participant index asked to produce the structured decision.
        finalizer: u32,
        schema_id: String,
        schema_version: u32,
        #[schemars(with = "serde_json::Value")]
        json_schema: Value,
        /// UTF-8 byte ceiling for the structured output.
        #[serde(default)]
        max_bytes: Option<usize>,
    },
    SelectedExchanges {
        /// Participant index whose council exchanges are selected.
        participant: u32,
        exchange_sequences: Vec<u32>,
        /// Aggregate UTF-8 byte ceiling.
        #[serde(default)]
        max_bytes: Option<usize>,
    },
    ArtifactReference {
        /// Participant index asked to return a durable artifact claim.
        participant: u32,
        /// UTF-8 byte ceiling for the claim.
        #[serde(default)]
        max_bytes: Option<usize>,
    },
    NoMerge,
}

impl Default for CouncilMergeArgs {
    fn default() -> Self {
        Self::Summary {
            finalizer: None,
            max_bytes: None,
        }
    }
}

const fn council_default_rounds() -> u32 {
    2
}

const fn council_default_result_bytes() -> usize {
    8 * 1024
}

const fn council_default_timeout_seconds() -> u64 {
    5 * 60
}

const AGENT_COUNCIL_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x312d_5af0_861a_4ba8_a35d_d6b4_734a_25b9);

fn agent_council_id(
    owner_bridge_session_id: &SessionId,
    tool_call_id: &str,
    tool_name: &str,
) -> Result<meerkat_mob::temporary_council::TemporaryCouncilId, ToolError> {
    let seed = format!("{owner_bridge_session_id}:{tool_call_id}");
    let id = uuid::Uuid::new_v5(&AGENT_COUNCIL_NAMESPACE, seed.as_bytes());
    meerkat_mob::temporary_council::TemporaryCouncilId::new(format!("agent:{id}")).map_err(
        |error| {
            ToolError::execution_failed(format!(
                "tool '{tool_name}' could not derive its canonical council id: {error}"
            ))
        },
    )
}

fn council_target_identity(
    call: ToolCallView<'_>,
    targets: &[AgentIdentity],
    index: u32,
) -> Result<AgentIdentity, ToolError> {
    targets
        .get(usize::try_from(index).unwrap_or(usize::MAX))
        .cloned()
        .ok_or_else(|| {
            ToolError::invalid_arguments(
                call.name,
                format!(
                    "participant index {index} is out of range for {} participants",
                    targets.len()
                ),
            )
        })
}

fn lower_agent_council_merge(
    call: ToolCallView<'_>,
    merge: CouncilMergeArgs,
    targets: &[AgentIdentity],
    default_max_bytes: usize,
) -> Result<MergeBackPolicy, ToolError> {
    match merge {
        CouncilMergeArgs::Summary {
            finalizer,
            max_bytes,
        } => {
            let index = finalizer.unwrap_or_else(|| {
                u32::try_from(targets.len().saturating_sub(1)).unwrap_or(u32::MAX)
            });
            Ok(MergeBackPolicy::BoundedTextSummary {
                finalizer: council_target_identity(call, targets, index)?,
                max_bytes: max_bytes.unwrap_or(default_max_bytes),
            })
        }
        CouncilMergeArgs::Structured {
            finalizer,
            schema_id,
            schema_version,
            json_schema,
            max_bytes,
        } => Ok(MergeBackPolicy::StructuredResult {
            finalizer: council_target_identity(call, targets, finalizer)?,
            contract: TemporaryCouncilStructuredContract::new(
                schema_id,
                schema_version,
                json_schema,
            ),
            max_bytes: max_bytes.unwrap_or(default_max_bytes),
        }),
        CouncilMergeArgs::SelectedExchanges {
            participant,
            exchange_sequences,
            max_bytes,
        } => Ok(MergeBackPolicy::SelectedTranscript {
            participant: council_target_identity(call, targets, participant)?,
            exchange_sequences,
            max_bytes: max_bytes.unwrap_or(default_max_bytes),
        }),
        CouncilMergeArgs::ArtifactReference {
            participant,
            max_bytes,
        } => Ok(MergeBackPolicy::DurableArtifactReference {
            participant: council_target_identity(call, targets, participant)?,
            max_bytes: max_bytes.unwrap_or(default_max_bytes),
        }),
        CouncilMergeArgs::NoMerge => Ok(MergeBackPolicy::NoMerge),
    }
}

fn merge_council_definition_dependencies(
    call: ToolCallView<'_>,
    target: &mut MobDefinition,
    source: &MobDefinition,
) -> Result<(), ToolError> {
    for (name, model) in &source.models {
        match target.models.get(name) {
            Some(existing) if existing != model => {
                return Err(ToolError::execution_failed(format!(
                    "tool '{}' found conflicting custom model definitions named '{name}'",
                    call.name
                )));
            }
            Some(_) => {}
            None => {
                target.models.insert(name.clone(), model.clone());
            }
        }
    }
    for (name, skill) in &source.skills {
        match target.skills.get(name) {
            Some(existing) if existing != skill => {
                return Err(ToolError::execution_failed(format!(
                    "tool '{}' found conflicting skill sources named '{name}'",
                    call.name
                )));
            }
            Some(_) => {}
            None => {
                target.skills.insert(name.clone(), skill.clone());
            }
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct ForkOffResult {
    /// Authoritative mob resolved from the current durable session binding.
    mob_id: String,
    /// Authoritative source member resolved from the current session binding.
    source_member_id: String,
    agent_identity: String,
    member_ref: meerkat_contracts::WireMemberRef,
    fork_session_id: String,
    turn_session_id: String,
    cache_inheritance: meerkat_core::ForkCacheInheritance,
    bounded_result: meerkat_contracts::MobBoundedHelperResult,
    usage: meerkat_core::Usage,
    turns: u32,
    tool_calls: u32,
}

fn fork_off_default_result_label() -> String {
    "fork_off_result".to_string()
}

const fn fork_off_default_max_text_bytes() -> usize {
    16 * 1024
}

fn fork_off_child_input(task: &str, expected_output: Option<&str>) -> String {
    let expected_output = expected_output.unwrap_or(
        "Report the work performed and the final result clearly in your final assistant text.",
    );
    format!(
        "DELEGATED DURABLE FORK\n\nTask:\n{task}\n\nExpected output guidance:\n{expected_output}\n\nInstructions:\n- This is a delegated durable transcript fork.\n- Perform the task, then report the work performed and the final result in your final assistant text.\n- The parent remains responsible for replying to the user.\n- Do not attempt autonomous cross-session delivery."
    )
}

#[derive(Deserialize, JsonSchema)]
struct ConcludeObjectiveArgs {
    /// Final answer or terminal outcome for the current objective.
    outcome: String,
}

#[derive(Deserialize, JsonSchema)]
struct MobCreateArgs {
    /// Explicit mob definition. Minimal useful shape:
    /// {"id":"mob-id","profiles":{"role":{"model":"gpt-5.5","tools":{"builtins":true,"shell":true,"comms":true}}}}.
    #[schemars(with = "serde_json::Value")]
    definition: MobDefinition,
}

#[derive(Deserialize, JsonSchema)]
struct MobIdArgs {
    /// Mob identifier.
    mob_id: String,
}

#[derive(Deserialize, JsonSchema)]
struct SpawnMemberArgs {
    /// Mob identifier (from mob_create response).
    mob_id: String,
    /// Role name (profile key) from the mob definition's profiles map.
    profile: String,
    /// Unique member identifier within this mob.
    member_id: String,
    /// Initial message/task for the member. Required for autonomous_host members.
    #[serde(default)]
    #[schemars(with = "serde_json::Value")]
    initial_message: Option<ContentInput>,
    /// autonomous_host (default): runs autonomously. turn_driven: waits for explicit turns.
    #[serde(default)]
    #[schemars(with = "serde_json::Value")]
    runtime_mode: Option<MobRuntimeMode>,
    /// session (default): runs in session runtime. external: delegates to external process.
    #[serde(default)]
    #[schemars(with = "serde_json::Value")]
    backend: Option<MobBackendKind>,
    /// Bound host peer ID for placed execution; omit for the controlling host.
    #[serde(default)]
    placement: Option<WireHostRef>,
    /// If true, auto-wire bidirectional comms with the orchestrator after spawn.
    #[serde(default)]
    auto_wire_parent: Option<bool>,
    /// Optional tool-surface override for this member (same shape as delegate's tooling).
    #[serde(default)]
    #[schemars(with = "serde_json::Value")]
    tooling: Option<meerkat_mob::SpawnTooling>,
    /// Per-member auth binding (deferral §1). Accepts the struct
    /// `{"realm": "...", "binding": "..."}` (wire-contract shape).
    /// When set, this member resolves credentials via the named
    /// realm + binding; otherwise the member uses env-default /
    /// config-realm fallback.
    #[serde(default)]
    #[schemars(with = "serde_json::Value")]
    auth_binding: Option<meerkat_contracts::wire::WireAuthBindingRef>,
}

#[derive(Deserialize, JsonSchema)]
struct MemberArgs {
    /// Mob identifier.
    mob_id: String,
    /// Member identifier within the mob.
    member_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WireArgs {
    /// Mob identifier.
    mob_id: String,
    /// Local member to wire from.
    member_id: String,
    /// Target peer: local member or typed external binding.
    peer: WirePeerArg,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct UnwireArgs {
    /// Mob identifier.
    mob_id: String,
    /// Local member to unwire from.
    member_id: String,
    /// Target peer to unwire.
    peer: UnwirePeerArg,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
enum WirePeerArg {
    Local(LocalPeerArg),
    ExternalBinding(WireExternalPeerBindingArg),
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
enum UnwirePeerArg {
    Local(LocalPeerArg),
    External(UnwireExternalPeerHandleArg),
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LocalPeerArg {
    /// Another member in this mob.
    local: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WireExternalPeerBindingArg {
    external_binding: ExternalPeerBindingArg,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UnwireExternalPeerHandleArg {
    external: ExternalPeerHandleArg,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExternalPeerBindingArg {
    name: String,
    address: String,
    #[schemars(with = "serde_json::Value")]
    identity: meerkat_contracts::WireTrustedPeerIdentity,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExternalPeerHandleArg {
    name: String,
}

fn wire_peer_target_from_args(peer: WirePeerArg) -> meerkat_mob::PeerTarget {
    match peer {
        WirePeerArg::Local(LocalPeerArg { local }) => meerkat_mob::PeerTarget::Local(local.into()),
        WirePeerArg::ExternalBinding(WireExternalPeerBindingArg { external_binding }) => {
            meerkat_mob::PeerTarget::ExternalBinding(meerkat_mob::ExternalPeerBindingSpec::new(
                external_binding.name,
                external_binding.address,
                external_binding.identity,
            ))
        }
    }
}

fn unwire_peer_target_from_args(
    peer: UnwirePeerArg,
) -> Result<meerkat_mob::PeerTarget, meerkat_core::error::ToolError> {
    match peer {
        UnwirePeerArg::Local(LocalPeerArg { local }) => {
            Ok(meerkat_mob::PeerTarget::Local(local.into()))
        }
        UnwirePeerArg::External(UnwireExternalPeerHandleArg { external }) => {
            let peer_name = PeerName::new(external.name)
                .map_err(|e| meerkat_core::error::ToolError::invalid_arguments("mob_unwire", e))?;
            Ok(meerkat_mob::PeerTarget::ExternalName(peer_name))
        }
    }
}

#[derive(Deserialize, JsonSchema)]
struct ProfileCreateArgs {
    /// Unique profile name. Use descriptive role names like 'researcher', 'code-reviewer'.
    name: String,
    /// Profile definition. Required field: model (string). Optional: tools,
    /// skills, peer_description, runtime_mode, backend, external_addressable.
    #[schemars(with = "serde_json::Value")]
    profile: meerkat_mob::Profile,
}

#[derive(Deserialize, JsonSchema)]
struct ProfileNameArgs {
    /// Profile name to retrieve.
    name: String,
}

#[derive(Deserialize, JsonSchema)]
struct ProfileUpdateArgs {
    /// Profile name to update.
    name: String,
    /// Complete updated profile definition (full replacement, not merge).
    #[schemars(with = "serde_json::Value")]
    profile: meerkat_mob::Profile,
    /// Current revision from mob_profile_get. Prevents accidental overwrites.
    expected_revision: u64,
}

#[derive(Deserialize, JsonSchema)]
struct ProfileDeleteArgs {
    /// Profile name to delete.
    name: String,
    /// Current revision from mob_profile_get. Prevents accidental deletion.
    expected_revision: u64,
}

/// Schema source for agent mob tools that accept no parameters.
#[derive(Debug, Deserialize, JsonSchema)]
struct NoParamsArgs {}

// ─── Mob cleanup helper ─────────────────────────────────────────────────

/// Archive a session and clean up any mobs it owns.
///
/// Single-function cleanup path used by CLI delete_session.
pub async fn archive_session_with_mob_cleanup<S>(
    service: Arc<S>,
    mob_state: Arc<MobMcpState>,
    session_id: &SessionId,
) -> Result<(), SessionError>
where
    S: MobSessionService + ?Sized + 'static,
{
    let session_id = session_id.clone();
    let result_session_id = session_id.clone();
    let (result_tx, result_rx) = oneshot::channel();
    tokio::spawn(async move {
        let session_key = session_id.to_string();
        let result = match mob_state
            .archive_mob_owned_bridge_session_with_cleanup(
                &session_id,
                "mob cleanup during archive incomplete",
            )
            .await
        {
            Ok(true) => Ok(()),
            Ok(false) => {
                let had_cleanup_anchor =
                    mob_state.has_bridge_session_scoped_mobs(&session_key).await;
                match service
                    .archive_with_mob_lifecycle_authority(&session_id)
                    .await
                {
                    Ok(()) => mob_state
                        .destroy_bridge_session_mobs(&session_key)
                        .await
                        .map_err(|error| {
                            error.into_session_error("mob cleanup during archive incomplete")
                        }),
                    Err(SessionError::NotFound { .. }) if had_cleanup_anchor => mob_state
                        .destroy_bridge_session_mobs(&session_key)
                        .await
                        .map_err(|error| {
                            error.into_session_error("mob cleanup during archive incomplete")
                        }),
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        };
        let _ = result_tx.send(result);
    });
    result_rx.await.map_err(|_| {
        SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
            "mob archive task ended before reporting a result for {result_session_id}"
        )))
    })?
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
    use meerkat_core::comms::{
        CommsCommand, PeerCapabilitySet, PeerDirectoryEntry, PeerDirectorySource, PeerSendability,
        SendError, SendReceipt, TrustedPeerDescriptor,
    };
    use meerkat_core::event::AgentEvent;
    use meerkat_core::event_injector::{InteractionSubscription, SubscribableInjector};
    use meerkat_core::interaction::{
        InboxInteraction, InteractionContent, InteractionId, PeerInputCandidate, PeerInputClass,
    };
    use meerkat_core::service::{
        AppendSystemContextRequest, AppendSystemContextResult, MobToolAuthorityContext,
        MobToolSnapshotContext, MobToolsFactory, SessionControlError, SessionHistoryPage,
        SessionHistoryQuery, SessionInfo, SessionQuery, SessionService, SessionServiceCommsExt,
        SessionServiceControlExt, SessionServiceHistoryExt, SessionSummary, SessionUsage,
        SessionView, StartTurnRequest,
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

    const ED25519_PUBLIC_KEY_7: &str = "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";

    /// T-B12 (DEC-P7B-18, ADJ-P7-6): the LLM-visible delegation roster is
    /// snapshot-pinned — phase 7 adds NOTHING here. No live, no host, no
    /// grant, no member-history tools; drift into the agent-facing surface
    /// must be a reviewed roster change, never an accident.
    #[test]
    fn agent_tool_roster_unchanged() {
        let names = |defs: Arc<[Arc<ToolDef>]>| -> Vec<String> {
            defs.iter().map(|def| def.name.to_string()).collect()
        };

        // Full composition (profile store + snapshot provider present).
        assert_eq!(
            names(build_tool_defs_with_profile_support(true, true, true)),
            vec![
                "delegate",
                "conclude_objective",
                "mob_create",
                "mob_destroy",
                "mob_spawn_member",
                "fork_off",
                "council",
                "mob_retire_member",
                "mob_check_member",
                "mob_list_members",
                "mob_list",
                "mob_wire",
                "mob_unwire",
                "mob_profile_create",
                "mob_profile_get",
                "mob_profile_list",
                "mob_profile_update",
                "mob_profile_delete",
                "mob_profile_list_sources",
            ],
            "agent-facing roster (full composition) must stay exactly this set"
        );

        // Minimal composition.
        assert_eq!(
            names(build_tool_defs()),
            vec![
                "delegate",
                "conclude_objective",
                "mob_create",
                "mob_destroy",
                "mob_spawn_member",
                "fork_off",
                "council",
                "mob_retire_member",
                "mob_check_member",
                "mob_list_members",
                "mob_list",
                "mob_wire",
                "mob_unwire",
            ],
            "agent-facing roster (minimal composition) must stay exactly this set"
        );

        // Explicit negatives (SD-4/SD-7/§16.9): none of the phase-7
        // console verbs leak into the LLM-visible tool surface.
        let full = names(build_tool_defs_with_profile_support(true, true, true));
        for forbidden in [
            "mob_member_history",
            "mob_hosts",
            "mob_route_installs",
            "mob_bind_host",
            "mob_revoke_host",
            "mob_grant_scopes",
            "mob_revoke_scopes",
            "mob_grants",
            "mob_member_live_open",
            "mob_member_live_close",
            "mob_member_live_status",
            "mob_member_live_control",
            "mob_hard_cancel_member",
        ] {
            assert!(
                !full.iter().any(|name| name == forbidden),
                "{forbidden} must never appear in the agent-facing roster"
            );
        }
    }

    #[test]
    fn fork_off_commits_caller_task_and_guidance_visibly() {
        let input = fork_off_child_input("Inspect the ledger", Some("Return a concise finding"));

        assert!(input.contains("DELEGATED DURABLE FORK"));
        assert!(input.contains("Task:\nInspect the ledger"));
        assert!(input.contains("Expected output guidance:\nReturn a concise finding"));
        assert!(input.contains("parent remains responsible for replying to the user"));
        assert!(input.contains("Do not attempt autonomous cross-session delivery"));
    }

    #[test]
    fn fork_off_schema_keeps_source_authority_out_of_llm_arguments() {
        let definitions = build_tool_defs();
        let definition = definitions
            .iter()
            .find(|definition| definition.name == "fork_off")
            .expect("fork_off tool definition");
        let required = definition.input_schema["required"]
            .as_array()
            .expect("fork_off required fields");
        for field in ["member_id", "task"] {
            assert!(
                required.iter().any(|entry| entry == field),
                "fork_off schema must require {field}"
            );
        }
        let properties = definition.input_schema["properties"]
            .as_object()
            .expect("fork_off properties");
        for forbidden in ["mob_id", "source_member_id", "profile"] {
            assert!(
                !properties.contains_key(forbidden),
                "fork_off must infer {forbidden} from the current durable session binding"
            );
        }
    }

    #[test]
    fn fork_off_common_case_uses_bounded_result_defaults() {
        let args: ForkOffArgs = serde_json::from_value(serde_json::json!({
            "member_id": "analysis-fork",
            "task": "Inspect the ledger"
        }))
        .expect("minimal fork_off args");

        assert_eq!(args.result_label, "fork_off_result");
        assert_eq!(args.max_text_bytes, 16 * 1024);
        assert_eq!(args.message_count, None);
        assert_eq!(args.expected_output, None);
    }

    #[test]
    fn test_agent_surface_destroy_error_preserves_incomplete_error_data() {
        let raw = serde_json::value::RawValue::from_string("{}".to_string()).expect("raw args");
        let call = ToolCallView {
            id: "test-1",
            name: "mob_destroy",
            args: &raw,
        };
        let mut report = meerkat_mob::MobDestroyReport::default();
        report.errors.push("worker: archive failed".to_string());

        let error = AgentMobToolSurface::map_destroy_error(
            call,
            crate::MobMcpDestroyError::Incomplete { report },
        );
        let data = error
            .structured_data()
            .expect("incomplete destroy should include structured data");

        assert_eq!(
            data.get("code").and_then(serde_json::Value::as_str),
            Some("mob_destroy_incomplete")
        );
        assert_eq!(
            data.get("retryable").and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert!(data.get("destroy_report").is_some());
    }

    #[test]
    fn test_agent_surface_mob_error_preserves_provider_auth_data() {
        let raw = serde_json::value::RawValue::from_string("{}".to_string()).expect("raw args");
        let call = ToolCallView {
            id: "test-1",
            name: "mob_spawn_member",
            args: &raw,
        };
        let failure = meerkat_core::service::SessionProviderAuthFailure {
            kind: meerkat_core::AuthErrorKind::InteractiveLoginRequired,
            provider: meerkat_core::Provider::OpenAI,
            realm_id: Some(meerkat_core::RealmId::parse("project").expect("realm")),
            binding_id: Some(meerkat_core::BindingId::parse("openai").expect("binding")),
        };

        let error = AgentMobToolSurface::map_mob_error(
            call,
            MobError::SessionError(meerkat_core::SessionError::provider_auth_failure(failure)),
        );
        let data = error
            .structured_data()
            .expect("provider-auth failures should retain typed data");

        assert_eq!(data["cause"], "provider_auth");
        assert_eq!(data["kind"], "interactive_login_required");
        assert_eq!(data["provider"], "openai");
        assert_eq!(data["realm_id"], "project");
        assert_eq!(data["binding_id"], "openai");
    }

    fn canonical_agent_wire_peer_arg() -> WirePeerArg {
        serde_json::from_value(serde_json::json!({
            "external_binding": {
                "name": "external-worker",
                "address": "inproc://external-worker",
                "identity": {
                    "kind": "ed25519_public_key",
                    "public_key": ED25519_PUBLIC_KEY_7
                }
            }
        }))
        .expect("canonical agent external peer target should deserialize")
    }

    #[test]
    fn agent_mcp_wire_accepts_typed_external_binding_request() {
        let target = wire_peer_target_from_args(canonical_agent_wire_peer_arg());

        let meerkat_mob::PeerTarget::ExternalBinding(binding) = target else {
            panic!("canonical agent external peer should remain a mob-resolved external binding");
        };
        assert_eq!(binding.name, "external-worker");
        assert_eq!(binding.address, "inproc://external-worker");
    }

    #[test]
    fn agent_mcp_wire_rejects_external_peer_raw_peer_id_shape() {
        let err = serde_json::from_value::<WirePeerArg>(serde_json::json!({
            "external": {
                "name": "external-worker",
                "peer_id": meerkat_core::comms::PeerId::from_ed25519_pubkey(&[7u8; 32]).to_string(),
                "address": "inproc://external-worker",
                "pubkey": vec![7u8; 32]
            }
        }))
        .expect_err("agent MCP raw peer_id/pubkey external peer shape must be rejected");

        let msg = err.to_string();
        assert!(
            msg.contains("external")
                || msg.contains("external_binding")
                || msg.contains("did not match"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn agent_mcp_wire_rejects_external_peer_missing_pubkey_material() {
        let err = serde_json::from_value::<WirePeerArg>(serde_json::json!({
            "external_binding": {
                "name": "external-worker",
                "address": "inproc://external-worker",
                "identity": {
                    "kind": "ed25519_public_key"
                }
            }
        }))
        .expect_err("agent MCP external peer identity must not default missing pubkey material");

        let msg = err.to_string();
        assert!(
            msg.contains("public_key") || msg.contains("identity") || msg.contains("did not match"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn agent_mcp_wire_rejects_ambiguous_peer_target_shape() {
        let err = serde_json::from_value::<WirePeerArg>(serde_json::json!({
            "local": "worker-a",
            "external_binding": {
                "name": "external-worker",
                "address": "inproc://external-worker",
                "identity": {
                    "kind": "ed25519_public_key",
                    "public_key": ED25519_PUBLIC_KEY_7
                }
            }
        }))
        .expect_err("agent MCP wire peer target must not accept multiple target shapes");

        let msg = err.to_string();
        assert!(
            msg.contains("did not match")
                || msg.contains("unknown field")
                || msg.contains("external_binding"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn agent_mcp_unwire_accepts_external_peer_name_handle() {
        let target = unwire_peer_target_from_args(
            serde_json::from_value::<UnwirePeerArg>(serde_json::json!({
                "external": { "name": "external-worker" }
            }))
            .expect("external handle should deserialize"),
        )
        .expect("external handle should convert");

        let meerkat_mob::PeerTarget::ExternalName(peer_name) = target else {
            panic!("unwire external should use the external peer handle");
        };
        assert_eq!(peer_name.as_str(), "external-worker");
    }

    #[test]
    fn agent_mcp_unwire_rejects_ambiguous_peer_target_shape() {
        let err = serde_json::from_value::<UnwirePeerArg>(serde_json::json!({
            "local": "worker-a",
            "external": { "name": "external-worker" }
        }))
        .expect_err("agent MCP unwire peer target must not accept multiple target shapes");

        let msg = err.to_string();
        assert!(
            msg.contains("did not match") || msg.contains("unknown field"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn agent_mcp_wire_schema_uses_external_binding_without_raw_peer_atoms() {
        let tools = build_tool_defs();
        let schema = tools
            .iter()
            .find(|tool| tool.name == TOOL_MOB_WIRE)
            .map(|tool| &tool.input_schema)
            .expect("wire tool schema present");
        let schema_text = serde_json::to_string(schema).expect("schema should encode");

        assert!(schema_text.contains("external_binding"));
        assert!(
            !schema_text.contains("\"peer_id\"") && !schema_text.contains("\"pubkey\""),
            "wire schema must not expose raw comms identity atoms: {schema_text}"
        );
    }

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
        fn inject_with_delivery_identity(
            &self,
            _input_identity: meerkat_core::service::StartTurnInputIdentity,
            _objective_id: Option<meerkat_core::interaction::ObjectiveId>,
            _content: ContentInput,
            _source: PlainEventSource,
            _handling_mode: HandlingMode,
            _render_metadata: Option<RenderMetadata>,
        ) -> Result<(), EventInjectorError> {
            Err(EventInjectorError::Closed)
        }

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
                        structured_output: None,
                    })
                    .await;
            });
            Ok(InteractionSubscription {
                id: interaction_id,
                events: rx,
            })
        }

        fn inject_with_interaction_id(
            &self,
            _interaction_id: InteractionId,
            body: ContentInput,
            source: PlainEventSource,
            handling_mode: HandlingMode,
            render_metadata: Option<RenderMetadata>,
        ) -> Result<(), EventInjectorError> {
            self.inject(body, source, handling_mode, render_metadata)
        }
    }

    impl TestCommsRegistry {
        async fn insert(&self, runtime: Arc<TestCommsRuntime>) {
            self.runtimes
                .write()
                .await
                .insert(runtime.peer_id.as_str(), runtime);
        }

        async fn get(&self, peer_id: &str) -> Option<Arc<TestCommsRuntime>> {
            self.runtimes.read().await.get(peer_id).cloned()
        }
    }

    struct TestCommsRuntime {
        name: String,
        peer_id: PeerId,
        public_key_bytes: [u8; 32],
        trusted: tokio::sync::RwLock<HashMap<String, TrustedPeerDescriptor>>,
        mob_machine_trust_owner: std::sync::RwLock<Option<Arc<dyn std::any::Any + Send + Sync>>>,
        inbox: tokio::sync::RwLock<Vec<InboxInteraction>>,
        notify: Arc<tokio::sync::Notify>,
        registry: Arc<TestCommsRegistry>,
    }

    impl TestCommsRuntime {
        async fn new(name: &str, registry: Arc<TestCommsRegistry>) -> Arc<Self> {
            let mut public_key_bytes = [0u8; 32];
            for (index, byte) in name.bytes().enumerate() {
                let slot = index % public_key_bytes.len();
                public_key_bytes[slot] = public_key_bytes[slot]
                    .wrapping_add(byte)
                    .wrapping_add(index as u8);
            }
            if public_key_bytes == [0u8; 32] {
                public_key_bytes[0] = 1;
            }
            let peer_id = PeerId::from_ed25519_pubkey(&public_key_bytes);
            let runtime = Arc::new(Self {
                name: name.into(),
                peer_id,
                public_key_bytes,
                trusted: tokio::sync::RwLock::new(HashMap::new()),
                mob_machine_trust_owner: std::sync::RwLock::new(None),
                inbox: tokio::sync::RwLock::new(Vec::new()),
                notify: Arc::new(tokio::sync::Notify::new()),
                registry,
            });
            runtime.registry.insert(runtime.clone()).await;
            runtime
        }

        fn validate_mob_trust_authority_owner(
            &self,
            authority: &meerkat_core::comms::CommsTrustMutationAuthority,
        ) -> Result<(), SendError> {
            if !authority.is_mob_machine_source() {
                return Ok(());
            }
            let expected = self
                .mob_machine_trust_owner
                .read()
                .expect("poisoned mob_machine_trust_owner lock in test comms runtime");
            authority
                .validate_raw_source_owner_token(expected.as_ref())
                .map_err(SendError::Validation)
        }
    }

    #[async_trait]
    impl CoreCommsRuntime for TestCommsRuntime {
        fn peer_id(&self) -> Option<PeerId> {
            Some(self.peer_id)
        }

        fn public_key(&self) -> Option<String> {
            use base64::Engine as _;

            Some(format!(
                "ed25519:{}",
                base64::engine::general_purpose::STANDARD.encode(self.public_key_bytes)
            ))
        }

        fn public_key_bytes(&self) -> Option<[u8; 32]> {
            Some(self.public_key_bytes)
        }

        fn comms_name(&self) -> Option<String> {
            Some(self.name.clone())
        }

        fn advertised_address(&self) -> Option<String> {
            Some(format!("inproc://{}", self.name))
        }

        async fn apply_trust_mutation(
            &self,
            mutation: CommsTrustMutation,
        ) -> Result<CommsTrustMutationResult, SendError> {
            match mutation {
                CommsTrustMutation::AddTrustedPeer { peer, authority } => {
                    self.validate_mob_trust_authority_owner(&authority)?;
                    authority
                        .validate_public_add(self.peer_id(), &peer)
                        .map_err(SendError::Validation)?;
                    TrustedPeerDescriptor::validate_pubkey_for_peer_id(peer.peer_id, &peer.pubkey)
                        .map_err(SendError::Validation)?;
                    let created = self
                        .trusted
                        .write()
                        .await
                        .insert(peer.peer_id.as_str().to_string(), peer)
                        .is_none();
                    Ok(CommsTrustMutationResult::Added { created })
                }
                CommsTrustMutation::RemoveTrustedPeer { peer_id, authority } => {
                    self.validate_mob_trust_authority_owner(&authority)?;
                    let parsed_peer_id = PeerId::parse(&peer_id)
                        .map_err(|err| SendError::Validation(err.to_string()))?;
                    authority
                        .validate_public_remove(self.peer_id(), parsed_peer_id)
                        .map_err(SendError::Validation)?;
                    let removed = self.trusted.write().await.remove(&peer_id).is_some();
                    Ok(CommsTrustMutationResult::Removed { removed })
                }
                CommsTrustMutation::AddPrivateTrustedPeer { peer, authority } => {
                    self.validate_mob_trust_authority_owner(&authority)?;
                    authority
                        .validate_private_add(self.peer_id(), &peer)
                        .map_err(SendError::Validation)?;
                    TrustedPeerDescriptor::validate_pubkey_for_peer_id(peer.peer_id, &peer.pubkey)
                        .map_err(SendError::Validation)?;
                    Ok(CommsTrustMutationResult::Added { created: true })
                }
                CommsTrustMutation::RemovePrivateTrustedPeer { peer_id, authority } => {
                    self.validate_mob_trust_authority_owner(&authority)?;
                    let parsed_peer_id = PeerId::parse(&peer_id)
                        .map_err(|err| SendError::Validation(err.to_string()))?;
                    authority
                        .validate_private_remove(self.peer_id(), parsed_peer_id)
                        .map_err(SendError::Validation)?;
                    let removed = self.trusted.write().await.remove(&peer_id).is_some();
                    Ok(CommsTrustMutationResult::Removed { removed })
                }
            }
        }

        async fn install_generated_mob_trust_owner(
            &self,
            owner: Arc<dyn std::any::Any + Send + Sync>,
        ) -> Result<(), SendError> {
            let mut expected = self
                .mob_machine_trust_owner
                .write()
                .expect("poisoned mob_machine_trust_owner lock in test comms runtime");
            if let Some(existing) = expected.as_ref() {
                if Arc::ptr_eq(existing, &owner) {
                    return Ok(());
                }
                return Err(SendError::Validation(
                    "target runtime is already bound to a different generated MobMachine trust owner"
                        .to_string(),
                ));
            }
            *expected = Some(owner);
            Ok(())
        }

        async fn validate_recovered_generated_mob_trust_owner(
            &self,
            owner: Arc<dyn std::any::Any + Send + Sync>,
        ) -> Result<(), SendError> {
            let expected = self
                .mob_machine_trust_owner
                .read()
                .expect("poisoned mob_machine_trust_owner lock in test comms runtime");
            if let Some(existing) = expected.as_ref()
                && !Arc::ptr_eq(existing, &owner)
            {
                return Err(SendError::Validation(
                    "target runtime is already bound to a different generated MobMachine trust owner"
                        .to_string(),
                ));
            }
            Ok(())
        }

        async fn install_recovered_generated_mob_trust_owner(
            &self,
            owner: Arc<dyn std::any::Any + Send + Sync>,
        ) -> Result<(), SendError> {
            let mut expected = self
                .mob_machine_trust_owner
                .write()
                .expect("poisoned mob_machine_trust_owner lock in test comms runtime");
            if let Some(existing) = expected.as_ref() {
                if Arc::ptr_eq(existing, &owner) {
                    return Ok(());
                }
                return Err(SendError::Validation(
                    "target runtime is already bound to a different generated MobMachine trust owner"
                        .to_string(),
                ));
            }
            *expected = Some(owner);
            Ok(())
        }

        async fn add_private_trusted_peer(
            &self,
            peer: TrustedPeerDescriptor,
        ) -> Result<(), SendError> {
            TrustedPeerDescriptor::validate_pubkey_for_peer_id(peer.peer_id, &peer.pubkey)
                .map_err(SendError::Validation)?;
            Ok(())
        }

        async fn send(&self, cmd: CommsCommand) -> Result<SendReceipt, SendError> {
            match cmd {
                CommsCommand::PeerRequest {
                    objective_id: None,
                    to,
                    intent,
                    params,
                    blocks,
                    content_taint: _,
                    handling_mode: _,
                    stream: _,
                } => {
                    let trusted = self.trusted.read().await;
                    let peer_id = to.peer_id.as_str();
                    if !trusted.contains_key(&peer_id) {
                        return Err(SendError::PeerNotFound(to.label()));
                    }
                    drop(trusted);
                    let recipient = self
                        .registry
                        .get(&peer_id)
                        .await
                        .ok_or_else(|| SendError::PeerNotFound(to.label()))?;
                    recipient.inbox.write().await.push(InboxInteraction {
                        objective_id: None,
                        sender_taint: None,
                        id: InteractionId(uuid::Uuid::new_v4()),
                        from_route: Some(self.peer_id),
                        from: self.name.clone(),
                        content: InteractionContent::Request {
                            intent,
                            params,
                            blocks,
                        },
                        rendered_text: String::new(),
                        handling_mode: HandlingMode::Queue,
                        render_metadata: None,
                    });
                    recipient.notify.notify_waiters();
                    Ok(SendReceipt::PeerRequestSent {
                        envelope_id: uuid::Uuid::new_v4(),
                        interaction_id: InteractionId(uuid::Uuid::new_v4()),
                        stream_reserved: false,
                        delivery: meerkat_core::comms::PeerDeliveryOutcome::Queued,
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
                        name: meerkat_core::comms::PeerName::new(peer.name.as_str()).ok()?,
                        peer_id: peer.peer_id.clone(),
                        address: peer.address.clone(),
                        source: PeerDirectorySource::Trusted,
                        sendable_kinds: vec![PeerSendability::PeerRequest],
                        capabilities: PeerCapabilitySet::default(),
                        meta: Default::default(),
                    })
                })
                .collect()
        }

        fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
            self.notify.clone()
        }

        async fn handoff_volatile_peer_input_candidates(
            &self,
        ) -> Result<Vec<PeerInputCandidate>, meerkat_core::CommsCapabilityError> {
            let peer_id_by_name: HashMap<String, PeerId> = self
                .trusted
                .read()
                .await
                .values()
                .map(|peer| (peer.name.as_string(), peer.peer_id))
                .collect();
            let interactions = {
                let mut inbox = self.inbox.write().await;
                std::mem::take(&mut *inbox)
            };
            Ok(interactions
                .into_iter()
                .map(|interaction| {
                    let canonical_peer_id = peer_id_by_name
                        .get(interaction.from.as_str())
                        .copied()
                        .unwrap_or_else(PeerId::new);
                    meerkat_runtime::test_peer_input_candidate_from_interaction(
                        interaction,
                        canonical_peer_id,
                    )
                })
                .collect())
        }
    }

    struct RealCommsSessionActor {
        comms: Arc<TestCommsRuntime>,
        witness: meerkat_session::LiveSessionActorWitness,
    }

    struct RealCommsSessionSvc {
        sessions: tokio::sync::RwLock<HashMap<SessionId, RealCommsSessionActor>>,
        persisted_sessions: tokio::sync::RwLock<HashMap<SessionId, meerkat_core::Session>>,
        persisted_metadata_loads: AtomicU64,
        actor_registry: meerkat_session::LiveSessionActorRegistry,
        counter: AtomicU64,
        runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
        registry: Arc<TestCommsRegistry>,
        injector: Arc<TestInjector>,
    }

    impl RealCommsSessionSvc {
        fn new() -> Self {
            Self {
                sessions: tokio::sync::RwLock::new(HashMap::new()),
                persisted_sessions: tokio::sync::RwLock::new(HashMap::new()),
                persisted_metadata_loads: AtomicU64::new(0),
                actor_registry: meerkat_session::LiveSessionActorRegistry::default(),
                counter: AtomicU64::new(0),
                runtime_adapter: Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
                registry: Arc::new(TestCommsRegistry::default()),
                injector: Arc::new(TestInjector),
            }
        }

        async fn real_comms(&self, session_id: &SessionId) -> Option<Arc<TestCommsRuntime>> {
            self.sessions
                .read()
                .await
                .get(session_id)
                .map(|actor| actor.comms.clone())
        }

        async fn register_external_comms(&self, name: &str) -> Arc<TestCommsRuntime> {
            TestCommsRuntime::new(name, Arc::clone(&self.registry)).await
        }

        async fn seed_persisted_session(&self, session: meerkat_core::Session) {
            self.persisted_sessions
                .write()
                .await
                .insert(session.id().clone(), session);
        }

        async fn create_session_with_actor_slot(
            &self,
            req: meerkat_core::service::CreateSessionRequest,
            actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
        ) -> Result<RunResult, SessionError> {
            let sid = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.as_ref())
                .map(|session| session.id().clone())
                .unwrap_or_default();
            let n = self.counter.fetch_add(1, Ordering::Relaxed);
            let name = req
                .build
                .as_ref()
                .and_then(|build| build.comms_name.clone())
                .unwrap_or_else(|| format!("real-comms-session-{n}"));
            let actor_materialization_permit =
                crate::begin_live_actor_materialization(req.build.as_ref().and_then(|build| {
                    match &build.runtime_build_mode {
                        meerkat_core::RuntimeBuildMode::SessionOwned(bindings) => Some(bindings),
                        meerkat_core::RuntimeBuildMode::StandaloneEphemeral => None,
                    }
                }))?;
            let comms = TestCommsRuntime::new(&name, Arc::clone(&self.registry)).await;
            let actor_witness = {
                let mut sessions = self.sessions.write().await;
                if sessions.contains_key(&sid) {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "live session actor is already registered: {sid}"
                        )),
                    ));
                }
                let witness = crate::register_live_actor(
                    &self.actor_registry,
                    actor_witness_slot,
                    sid.clone(),
                )?;
                sessions.insert(
                    sid.clone(),
                    RealCommsSessionActor {
                        comms,
                        witness: witness.clone(),
                    },
                );
                witness
            };
            crate::commit_live_actor_materialization_or_discard(
                self,
                actor_materialization_permit,
                &actor_witness,
            )
            .await?;
            Ok(RunResult {
                text: "ok".to_string(),
                session_id: sid,
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: None,
            })
        }
    }

    #[async_trait]
    impl SessionService for RealCommsSessionSvc {
        async fn create_session(
            &self,
            req: meerkat_core::service::CreateSessionRequest,
        ) -> Result<RunResult, SessionError> {
            let actor_witness_slot = meerkat_session::LiveSessionActorWitnessSlot::default();
            self.create_session_with_actor_slot(req, &actor_witness_slot)
                .await
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
                terminal_cause_kind: None,
                structured_output: None,
                extraction_error: None,
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
            let removed = {
                let mut sessions = self.sessions.write().await;
                let had_live_actor = sessions.contains_key(id);
                let removed_registry_actor = self.actor_registry.remove_current(id);
                if had_live_actor && !removed_registry_actor {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "live actor registry omitted current session {id} during archive"
                        )),
                    ));
                }
                sessions.remove(id).is_some()
            };
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
                .map(|actor| actor.comms.clone() as Arc<dyn CoreCommsRuntime>)
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
                status: AppendSystemContextStatus::Applied,
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
        async fn materialize_session_resume_verdict(
            &self,
            session_id: &SessionId,
        ) -> Result<meerkat_mob::SessionResumeVerdict, SessionError> {
            meerkat_mob::materialize_nonpersistent_session_resume_verdict(self, session_id).await
        }

        async fn observe_session_resume_authority(
            &self,
            _session_id: &SessionId,
        ) -> Result<meerkat_mob::SessionResumeAuthority, SessionError> {
            // This process-local test map has no RuntimeStore authority.
            Ok(meerkat_mob::SessionResumeAuthority::default())
        }

        async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
            &self,
            _session_id: &SessionId,
            _authority: &meerkat_core::CommittedSessionBoundaryAuthority,
        ) -> Result<(), SessionError> {
            Err(SessionError::Unsupported(
                "real-comms test service has no store-owned boundary authority".to_string(),
            ))
        }

        async fn enqueue_committed_parent_session_boundary_after_runtime_turn(
            &self,
            _session_id: &SessionId,
            _runtime_adapter: &meerkat_runtime::MeerkatMachine,
        ) -> Result<usize, SessionError> {
            Ok(0)
        }

        /// Test double: the two-read composition is the exact truth here.
        async fn load_session_for_resume(
            &self,
            session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::runtime::ResumeSessionLoad, meerkat_core::service::SessionError>
        {
            use meerkat_mob::runtime::ResumeSessionLoad;
            if let Some(session) = self.load_persisted_session(session_id).await? {
                return Ok(ResumeSessionLoad::Active(Box::new(session)));
            }
            if let Some(session) = self.load_revivable_retired_session(session_id).await? {
                return Ok(ResumeSessionLoad::Revivable(Box::new(session)));
            }
            Ok(ResumeSessionLoad::Absent)
        }

        async fn create_session_under_runtime_turn_boundary(
            &self,
            req: meerkat_core::service::CreateSessionRequest,
        ) -> Result<RunResult, SessionError> {
            <Self as SessionService>::create_session(self, req).await
        }

        async fn create_session_with_actor_witness_under_runtime_turn_boundary(
            &self,
            req: meerkat_core::service::CreateSessionRequest,
            _resume_preparation: Option<meerkat_mob::SessionResumePreparationReceipt>,
            actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
        ) -> Result<RunResult, SessionError> {
            self.create_session_with_actor_slot(req, actor_witness_slot)
                .await
        }

        async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            self.archive_with_mob_lifecycle_authority(session_id).await
        }

        async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
            &self,
            session_id: &SessionId,
            _deadline: meerkat_core::time_compat::Instant,
        ) -> Result<(), SessionError> {
            self.archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(session_id)
                .await
        }

        async fn discard_live_session_under_runtime_turn_boundary(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            self.discard_live_session(session_id).await
        }

        async fn discard_live_session_actor_under_runtime_turn_boundary(
            &self,
            witness: &meerkat_session::LiveSessionActorWitness,
        ) -> Result<bool, SessionError> {
            let removed = {
                let mut sessions = self.sessions.write().await;
                let Some(actor) = sessions.get(witness.session_id()) else {
                    return Ok(false);
                };
                if !actor.witness.eq(witness) {
                    return Ok(false);
                }
                if !self.actor_registry.compare_remove(witness) {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "live actor registry rejected current witness for {}",
                            witness.session_id()
                        )),
                    ));
                }
                sessions.remove(witness.session_id()).is_some()
            };
            Ok(removed)
        }

        async fn subscribe_session_events(
            &self,
            session_id: &SessionId,
        ) -> Result<EventStream, StreamError> {
            Err(StreamError::NotFound(format!("session {session_id}")))
        }

        async fn live_session_actor_registered(
            &self,
            session_id: &SessionId,
        ) -> Result<bool, SessionError> {
            Ok(self.actor_registry.contains(session_id))
        }

        fn supports_runtime_turn_apply(&self) -> bool {
            true
        }

        fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
            Some(self.runtime_adapter.clone())
        }

        async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
            {
                let mut sessions = self.sessions.write().await;
                let had_live_actor = sessions.contains_key(session_id);
                let removed_registry_actor = self.actor_registry.remove_current(session_id);
                if had_live_actor && !removed_registry_actor {
                    return Err(SessionError::Agent(
                        meerkat_core::error::AgentError::InternalError(format!(
                            "live actor registry omitted current session {session_id} during discard"
                        )),
                    ));
                }
                sessions.remove(session_id);
            }
            Ok(())
        }

        async fn session_belongs_to_mob(&self, session_id: &SessionId, mob_id: &MobId) -> bool {
            self.sessions.read().await.contains_key(session_id) && !mob_id.as_str().is_empty()
        }

        async fn load_persisted_session(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<meerkat_core::Session>, SessionError> {
            Ok(self
                .persisted_sessions
                .read()
                .await
                .get(session_id)
                .cloned())
        }

        async fn load_persisted_session_metadata(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<meerkat_core::PersistedSessionMetadataView>, SessionError> {
            self.persisted_metadata_loads
                .fetch_add(1, Ordering::Relaxed);
            let Some(session) = self
                .persisted_sessions
                .read()
                .await
                .get(session_id)
                .cloned()
            else {
                return Ok(None);
            };
            meerkat_core::PersistedSessionMetadataView::try_from_session(&session)
                .map(Some)
                .map_err(|error| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "session {session_id} metadata projection failed in test service: {error}"
                    )))
                })
        }

        async fn apply_runtime_turn(
            &self,
            session_id: &SessionId,
            run_id: meerkat_core::RunId,
            req: StartTurnRequest,
            boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
            contributing_input_ids: Vec<meerkat_core::InputId>,
        ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, SessionError> {
            let run_result = <Self as SessionService>::start_turn(self, session_id, req).await?;
            Ok(
                meerkat_core::lifecycle::core_executor::CoreApplyOutput::with_run_result(
                    meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft {
                        run_id,
                        boundary,
                        contributing_input_ids,
                        conversation_digest: None,
                        message_count: 0,
                    },
                    None,
                    run_result,
                ),
            )
        }
    }

    fn create_only_authority() -> MobToolAuthorityContext {
        meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
            .expect("generated create-only mob authority should be accepted")
    }

    fn scope_only_authority(mob_id: &str) -> MobToolAuthorityContext {
        let authority = meerkat_runtime::mob_operator_authority::set_create_authority(
            &create_only_authority(),
            false,
        )
        .expect("generated mob authority should disable create scope");
        meerkat_runtime::mob_operator_authority::grant_manage_mob(&authority, mob_id)
            .expect("generated mob authority should grant managed mob scope")
    }

    fn spawn_profile_authority(mob_id: &str, profile: &str) -> MobToolAuthorityContext {
        let authority = meerkat_runtime::mob_operator_authority::set_create_authority(
            &create_only_authority(),
            false,
        )
        .expect("generated mob authority should disable create scope");
        meerkat_runtime::mob_operator_authority::grant_spawn_profile_in_mob(
            &authority, mob_id, profile,
        )
        .expect("generated mob authority should grant spawn profile scope")
    }

    fn create_only_authority_with_provenance() -> MobToolAuthorityContext {
        let authority = meerkat_runtime::mob_operator_authority::with_caller_provenance(
            &create_only_authority(),
            meerkat_core::service::MobToolCallerProvenance::new()
                .with_session_id(SessionId::new())
                .with_member_id("lead-1"),
        )
        .expect("generated mob authority should attach caller provenance");
        meerkat_runtime::mob_operator_authority::with_audit_invocation_id(
            &authority,
            "audit-create",
        )
        .expect("generated mob authority should attach audit invocation id")
    }

    struct TestVisibilityToolDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
    }

    #[async_trait]
    impl AgentToolDispatcher for TestVisibilityToolDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolDispatchOutcome, ToolError> {
            Err(ToolError::not_found(call.name))
        }
    }

    struct CaptureSnapshotMobFactory {
        captured: Arc<std::sync::Mutex<Option<MobToolSnapshotContext>>>,
    }

    #[async_trait]
    impl MobToolsFactory for CaptureSnapshotMobFactory {
        async fn build_mob_tools(
            &self,
            args: meerkat_core::service::MobToolsBuildArgs,
        ) -> Result<Arc<dyn AgentToolDispatcher>, Box<dyn std::error::Error + Send + Sync>>
        {
            *self.captured.lock().expect("snapshot capture lock") =
                Some(args.snapshot_context.clone());
            Ok(Arc::new(TestVisibilityToolDispatcher {
                tools: Arc::from(Vec::<Arc<ToolDef>>::new()),
            }))
        }
    }

    async fn parent_snapshot_context_for_tools_with_filter(
        tools: Vec<Arc<ToolDef>>,
        initial_filter: Option<meerkat_core::ToolFilter>,
    ) -> MobToolSnapshotContext {
        let captured = Arc::new(std::sync::Mutex::new(None));
        let mob_factory = Arc::new(CaptureSnapshotMobFactory {
            captured: Arc::clone(&captured),
        });
        let temp = tempfile::tempdir().expect("temp agent factory dir");
        let factory = meerkat::AgentFactory::new(temp.path().join("sessions"))
            .builtins(false)
            .mob(true)
            .mob_tools_factory(mob_factory.clone());
        let mut build = meerkat::AgentBuildConfig::new("claude-sonnet-4-5");
        build.provider = Some(meerkat_core::Provider::Anthropic);
        build.llm_client_override = Some(Arc::new(meerkat_client::TestClient::default()));
        build.override_mob = meerkat_core::ToolCategoryOverride::Enable;
        build.mob_tool_authority_context = Some(create_only_authority());
        build.mob_tools = Some(mob_factory);
        build.tool_dispatcher_override = Some(Arc::new(TestVisibilityToolDispatcher {
            tools: Arc::from(tools),
        }));
        if let Some(filter) = initial_filter {
            build.set_initial_tool_filter(filter);
        }

        let agent = factory
            .build_agent(build, &meerkat_core::Config::default())
            .await
            .expect("agent build should mint parent tool composition authority");
        let context = captured
            .lock()
            .expect("snapshot capture lock")
            .take()
            .expect("mob factory should capture snapshot context");
        let _agent = Box::leak(Box::new(agent));
        context
    }

    async fn parent_snapshot_context_for_tools(tools: Vec<Arc<ToolDef>>) -> MobToolSnapshotContext {
        parent_snapshot_context_for_tools_with_filter(tools, None).await
    }

    fn sample_definition(mob_id: &str) -> MobDefinition {
        let mut profiles = std::collections::BTreeMap::new();
        profiles.insert(
            ProfileName::from("delegate"),
            meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::Profile {
                model: "claude-sonnet-4-5".to_string(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
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
            })),
        );
        profiles.insert(
            ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::Profile {
                model: "claude-sonnet-4-5".to_string(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
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
            })),
        );

        let mut definition = MobDefinition::explicit(MobId::from(mob_id));
        definition.profiles = profiles;
        definition
    }

    #[test]
    fn test_all_tool_definitions_present() {
        let defs = build_tool_defs();
        assert_eq!(defs.len(), 13);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"delegate"));
        assert!(names.contains(&"fork_off"));
        assert!(names.contains(&"council"));
        assert!(names.contains(&"conclude_objective"));
        assert!(names.contains(&"mob_create"));
        assert!(names.contains(&"mob_destroy"));
        assert!(names.contains(&"mob_spawn_member"));
        assert!(names.contains(&"mob_retire_member"));
        assert!(names.contains(&"mob_check_member"));
        assert!(names.contains(&"mob_list_members"));
        assert!(names.contains(&"mob_list"));
        assert!(names.contains(&"mob_wire"));
        assert!(names.contains(&"mob_unwire"));
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

    /// Gate for remediation row #32: every agent-facing mob tool's
    /// `input_schema` must be produced by `schema_for!` via [`typed_schema`],
    /// never a hand-authored `json!({...})` literal. `schema_for!` always stamps
    /// the root with a `$schema` meta-schema URL; the deleted hand-written object
    /// literals in this surface never carried one. The full profile-store
    /// surface (15 tools) is exercised so every tool_def call is covered.
    #[test]
    fn agent_mob_tool_schemas_are_typed_not_handwritten() {
        let defs = build_tool_defs_with_profile_support(true, true, false);
        for def in defs.iter() {
            assert!(
                def.input_schema
                    .get("$schema")
                    .and_then(serde_json::Value::as_str)
                    .is_some(),
                "agent mob tool '{}' has a hand-authored json! schema (missing \
                 $schema marker); it must derive from schema_for! via typed_schema::<T>()",
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
        assert!(required.iter().any(|v| v.as_str() == Some("result_label")));
        assert!(
            required
                .iter()
                .any(|v| v.as_str() == Some("max_text_bytes"))
        );
    }

    #[test]
    fn delegate_schema_exposes_spawn_tooling_controls() {
        // The delegate input schema is derived from `DelegateArgs` via
        // `schema_for!` (rows #32/#157), not a hand-authored `json!` literal.
        // It must therefore carry the schemars `$schema` marker and advertise
        // the typed `tooling` field as an optional property.
        let defs = build_tool_defs();
        let delegate = defs.iter().find(|d| d.name == "delegate").unwrap();
        assert!(
            delegate.input_schema.get("$schema").is_some(),
            "delegate schema must be derived via schema_for! (carrying $schema)"
        );
        assert!(
            delegate.input_schema["properties"].get("tooling").is_some(),
            "delegate schema must expose the typed tooling control field"
        );
        assert!(
            delegate.input_schema["properties"]
                .get("placement")
                .is_some(),
            "delegate schema must expose the typed placement field"
        );

        let spawn_member = defs
            .iter()
            .find(|definition| definition.name == "mob_spawn_member")
            .expect("mob_spawn_member tool definition");
        assert!(
            spawn_member.input_schema["properties"]
                .get("placement")
                .is_some(),
            "mob_spawn_member schema must expose the typed placement field"
        );
    }

    #[test]
    fn agent_spawn_args_parse_and_lower_wire_placement() {
        let delegate: DelegateArgs = serde_json::from_value(json!({
            "task": "review",
            "member_id": "reviewer",
            "result_label": "review_result",
            "max_text_bytes": 4096,
            "placement": "host-b-peer"
        }))
        .expect("delegate placement parses");
        assert_eq!(
            lower_wire_placement(delegate.placement)
                .as_ref()
                .map(HostId::as_str),
            Some("host-b-peer")
        );

        let spawn: SpawnMemberArgs = serde_json::from_value(json!({
            "mob_id": "mob-1",
            "profile": "worker",
            "member_id": "worker-1",
            "placement": "host-c-peer"
        }))
        .expect("mob spawn placement parses");
        assert_eq!(
            lower_wire_placement(spawn.placement)
                .as_ref()
                .map(HostId::as_str),
            Some("host-c-peer")
        );
    }

    #[test]
    fn mob_create_schema_exposes_definition_profile_shape() {
        // `MobCreateArgs::definition` is the typed deserialize owner; the schema
        // is generated from it (rows #32/#157) rather than a `json!` literal.
        let defs = build_tool_defs();
        let mob_create = defs.iter().find(|d| d.name == "mob_create").unwrap();
        assert!(
            mob_create.input_schema.get("$schema").is_some(),
            "mob_create schema must be derived via schema_for! (carrying $schema)"
        );
        let required = mob_create.input_schema["required"].as_array().unwrap();
        assert!(
            required.iter().any(|v| v.as_str() == Some("definition")),
            "mob_create schema must require the typed definition field"
        );
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

        // Guard the concrete dispatcher future itself. Large-stack CI lanes
        // can otherwise mask a compiler-generated aggregate branch frame.
        let context = meerkat_core::ToolDispatchContext::default();
        let dispatch_future = surface.dispatch_with_context_stack_bounded(call, &context);
        let dispatch_future_size = std::mem::size_of_val(&dispatch_future);
        assert!(
            dispatch_future_size <= 1024,
            "agent mob dispatcher future grew beyond its stack budget: {dispatch_future_size} bytes"
        );
        drop(dispatch_future);

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
    async fn test_build_mob_tools_rejects_deserialized_operator_authority_projection() {
        let state = MobMcpState::new_in_memory();
        let factory = AgentMobToolSurfaceFactory::new(state);
        let authority_projection: MobToolAuthorityContext =
            serde_json::from_value(serde_json::to_value(create_only_authority()).unwrap()).unwrap();
        assert!(!authority_projection.is_generated_authority_context());

        let dispatcher = factory
            .build_mob_tools(meerkat_core::service::MobToolsBuildArgs {
                session_id: SessionId::new(),
                model: "claude-sonnet-4-5".to_string(),
                authority_context: Some(authority_projection),
                effective_authority: None,
                comms_name: None,
                comms_runtime: None,
                snapshot_context: meerkat_core::service::MobToolSnapshotContext::Standalone,
            })
            .await
            .expect("build_mob_tools");

        assert!(
            dispatcher.tools().is_empty(),
            "deserialized mob authority projection must not surface operator tools"
        );
    }

    #[tokio::test]
    async fn test_build_mob_tools_does_not_widen_scope_from_bridge_session_owned_mobs() {
        let state = MobMcpState::new_in_memory();
        let factory = AgentMobToolSurfaceFactory::new(Arc::clone(&state));
        let session_id = SessionId::new();
        let definition = sample_definition("owned-without-scope");
        let mob_id = state
            .mob_create_definition_with_owner_bridge_session(
                definition,
                session_id.clone(),
                true,
                false,
            )
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
            "bridge-session-owned mobs must not be widened into scope during rebuild"
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
            .get_or_create_implicit_mob_for_bridge_session(&session_key, "claude-sonnet-4-5")
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
            state
                .find_implicit_mob_for_bridge_session(&session_key)
                .await,
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
        let owner_authority = created_handle
            .owner_bridge_session_lifecycle_authority()
            .expect("created mob owner bridge authority");
        assert_eq!(
            owner_authority.bridge_session_id.to_string(),
            expected_session_id,
            "mob_create must rebind bridge-session indexing to the current owner bridge session"
        );
        assert!(
            owner_authority.destroy_on_owner_archive,
            "mob_create must set explicit bridge-session-scoped cleanup truth"
        );
        assert!(
            !owner_authority.implicit_delegation_mob,
            "mob_create must not allow callers to mint faux implicit mobs"
        );

        // mob_create should return a generated replacement context for the
        // turn owner to merge into canonical session authority.
        assert_eq!(
            create_result.session_effects.len(),
            1,
            "mob_create should emit exactly one session effect"
        );
        match &create_result.session_effects[0] {
            meerkat_core::SessionEffect::ReplaceMobToolAuthorityContext { authority_context } => {
                assert!(
                    authority_context.can_manage_mob(&mob_id),
                    "mob_create replacement authority should manage the created mob_id"
                );
            }
            other => panic!("unexpected mob_create session effect: {other:?}"),
        }
    }

    /// Gate for remediation row #211: `mob_create` must produce the durable
    /// create outcome and the operator grant as a single atomic effect bundle,
    /// with the grant keyed on the *intended* definition id. There must be no
    /// split where the mob exists in storage but no grant effect is present, or
    /// where the grant manages a different id than the one created.
    #[tokio::test]
    async fn mob_create_grant_is_atomic_with_create_outcome() {
        let state = MobMcpState::new_in_memory();
        let session_id = SessionId::new();
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

        let intended_id = "atomic-grant-mob";
        let create_args = serde_json::value::RawValue::from_string(
            json!({
                "definition": {
                    "id": intended_id,
                    "profiles": {
                        "worker": {
                            "model": "claude-sonnet-4-5",
                            "tools": { "comms": true },
                            "peer_description": "worker",
                            "runtime_mode": "turn_driven"
                        }
                    },
                    "is_implicit": false,
                    "session_cleanup_policy": "manual"
                }
            })
            .to_string(),
        )
        .unwrap();

        let create_result = surface
            .dispatch(ToolCallView {
                id: "atomic-1",
                name: "mob_create",
                args: &create_args,
            })
            .await
            .expect("mob_create should succeed");

        let created: serde_json::Value =
            serde_json::from_str(&create_result.result.text_content()).unwrap();
        let created_mob_id = created["mob_id"].as_str().expect("mob_id");
        assert_eq!(
            created_mob_id, intended_id,
            "created mob id must equal the intended definition id"
        );

        // The mob exists in storage; the grant effect MUST be present in the
        // same outcome bundle (no mutation-lands-then-grant-lands window).
        assert!(
            state.handle_for(&MobId::from(intended_id)).await.is_ok(),
            "mob_create must create the mob in storage"
        );
        assert_eq!(
            create_result.session_effects.len(),
            1,
            "mob_create must emit exactly one grant effect alongside the create outcome"
        );
        match &create_result.session_effects[0] {
            meerkat_core::SessionEffect::ReplaceMobToolAuthorityContext { authority_context } => {
                assert!(
                    authority_context.can_manage_mob(intended_id),
                    "the atomic grant must manage the same id that was created"
                );
            }
            other => panic!("unexpected mob_create session effect: {other:?}"),
        }
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

        let delegate_args = serde_json::value::RawValue::from_string(
            json!({
                "task": "say hi",
                "member_id": "helper-bootstrap-test",
                "result_label": "delegate_result",
                "max_text_bytes": 4096
            })
            .to_string(),
        )
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
            .find_implicit_mob_for_bridge_session(&session_key)
            .await
            .expect("delegate should still create an implicit mob");

        // No session effect is returned when delegate errors — the effect
        // is part of the ToolDispatchOutcome which is only produced on success.
        // This is correct: the turn owner should not widen authority for a
        // failed tool call.
    }

    #[tokio::test]
    async fn test_delegate_missing_member_id_does_not_create_implicit_mob() {
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

        let delegate_args = serde_json::value::RawValue::from_string(
            json!({
                "task": "say hi",
                "result_label": "delegate_result",
                "max_text_bytes": 4096
            })
            .to_string(),
        )
        .unwrap();
        let delegate_error = surface
            .dispatch(ToolCallView {
                id: "delegate-missing-member",
                name: "delegate",
                args: &delegate_args,
            })
            .await
            .expect_err("delegate without member_id must fail before mob creation");
        assert!(matches!(
            delegate_error,
            ToolError::InvalidArguments { reason, .. }
                if reason == "delegate requires member_id; the tool surface does not allocate member identity"
        ));

        assert!(
            state
                .find_implicit_mob_for_bridge_session(&session_key)
                .await
                .is_none(),
            "invalid delegate args must not create an implicit mob"
        );
    }

    #[tokio::test]
    async fn test_delegate_invalid_result_bound_does_not_create_implicit_mob() {
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
        let delegate_args = serde_json::value::RawValue::from_string(
            json!({
                "member_id": "must-not-spawn",
                "task": "say hi",
                "result_label": "",
                "max_text_bytes": 4096
            })
            .to_string(),
        )
        .unwrap();

        let error = surface
            .dispatch(ToolCallView {
                id: "delegate-invalid-bound",
                name: "delegate",
                args: &delegate_args,
            })
            .await
            .expect_err("invalid result bounds must fail before mob creation");

        assert!(matches!(error, ToolError::InvalidArguments { .. }));
        assert!(
            state
                .find_implicit_mob_for_bridge_session(&session_key)
                .await
                .is_none(),
            "invalid result bounds must have zero implicit-mob effects"
        );
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
        assert!(!audit_event.1.as_str().is_empty());
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
        let authority = meerkat_runtime::mob_operator_authority::with_audit_invocation_id(
            &scope_only_authority(mob_id.as_str()),
            "audit-scope",
        )
        .expect("generated mob authority should attach scope audit id");
        let expected_principal = authority.principal_token().clone();
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
        let spawn_result = surface
            .dispatch(ToolCallView {
                id: "spawn-scope",
                name: "mob_spawn_member",
                args: &spawn_args,
            })
            .await
            .expect("in-scope spawn should succeed");
        let spawn_payload: serde_json::Value =
            serde_json::from_str(&spawn_result.result.text_content()).unwrap();
        assert!(
            spawn_payload["agent_identity"].is_string(),
            "Mob-MCP operator results should surface the canonical agent identity"
        );
        assert!(
            spawn_payload["member_ref"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "Mob-MCP operator results should surface the server-resolved member_ref"
        );
        assert!(
            spawn_payload.get("agent_runtime_id").is_none(),
            "Mob-MCP operator results must not leak the binding-era agent_runtime_id"
        );
        assert!(
            spawn_payload.get("fence_token").is_none(),
            "Mob-MCP operator results must not leak the binding-era fence_token"
        );

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
        assert_eq!(audit_events[0].1, expected_principal);
        assert_eq!(audit_events[0].2.as_deref(), Some("audit-scope"));
    }

    #[tokio::test]
    async fn test_spawn_profile_scope_allows_spawn_but_not_privileged_overrides() {
        let state = MobMcpState::new_in_memory();
        let mob_id = state
            .mob_create_definition(sample_definition("spawn-profile-scope"))
            .await
            .expect("create mob");
        let surface = AgentMobToolSurface::new(
            Arc::clone(&state),
            None,
            spawn_profile_authority(mob_id.as_str(), "worker"),
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
            None,
            None,
        );

        let spawn_args = serde_json::value::RawValue::from_string(
            json!({
                "mob_id": mob_id,
                "profile": "worker",
                "member_id": "w-1",
                "auto_wire_parent": true,
                "initial_message": "hello"
            })
            .to_string(),
        )
        .unwrap();
        surface
            .dispatch(ToolCallView {
                id: "spawn-profile-ok",
                name: "mob_spawn_member",
                args: &spawn_args,
            })
            .await
            .expect("profile-scoped authority should spawn allowed definition profiles");

        let runtime_override_args = serde_json::value::RawValue::from_string(
            json!({
                "mob_id": mob_id,
                "profile": "worker",
                "member_id": "w-2",
                "runtime_mode": "autonomous_host"
            })
            .to_string(),
        )
        .unwrap();
        let runtime_override_error = surface
            .dispatch(ToolCallView {
                id: "spawn-profile-runtime-override",
                name: "mob_spawn_member",
                args: &runtime_override_args,
            })
            .await
            .expect_err("profile-scoped authority must not override runtime binding policy");
        assert!(matches!(
            runtime_override_error,
            ToolError::AccessDenied { .. }
        ));

        let unknown_profile_args = serde_json::value::RawValue::from_string(
            json!({
                "mob_id": mob_id,
                "profile": "unknown",
                "member_id": "w-3"
            })
            .to_string(),
        )
        .unwrap();
        let unknown_profile_error = surface
            .dispatch(ToolCallView {
                id: "spawn-profile-unknown",
                name: "mob_spawn_member",
                args: &unknown_profile_args,
            })
            .await
            .expect_err("profile-scoped authority must not spawn ungranted profiles");
        assert!(matches!(
            unknown_profile_error,
            ToolError::AccessDenied { .. }
        ));
    }

    #[tokio::test]
    async fn test_mob_spawn_member_auto_wire_parent_uses_bound_owner_session() {
        let state = MobMcpState::new_in_memory();
        let mob_id = state
            .mob_create_definition(sample_definition("spawn-auto-wire-parent"))
            .await
            .expect("create mob");
        let handle = state.handle_for(&mob_id).await.expect("handle");
        let parent_identity = AgentIdentity::from("parent");
        state
            .mob_spawn_spec(
                &mob_id,
                SpawnMemberSpec::new(ProfileName::from("worker"), parent_identity.clone()),
            )
            .await
            .expect("spawn parent");
        let parent_bridge_session_id = handle
            .resolve_bridge_session_id(&parent_identity)
            .await
            .expect("parent bridge session");
        let surface: Arc<dyn AgentToolDispatcher> = Arc::new(AgentMobToolSurface::new(
            Arc::clone(&state),
            None,
            spawn_profile_authority(mob_id.as_str(), "worker"),
            "claude-sonnet-4-5".to_string(),
            parent_bridge_session_id.clone(),
            None,
            None,
            None,
        ));
        let surface = surface
            .bind_ops_lifecycle(
                Arc::new(meerkat_runtime::ops_lifecycle::RuntimeOpsLifecycleRegistry::new()),
                parent_bridge_session_id,
            )
            .expect("bind ops lifecycle")
            .into_dispatcher();

        let spawn_args = serde_json::value::RawValue::from_string(
            json!({
                "mob_id": mob_id,
                "profile": "worker",
                "member_id": "child",
                "auto_wire_parent": true
            })
            .to_string(),
        )
        .unwrap();
        surface
            .dispatch(ToolCallView {
                id: "spawn-auto-wire-child",
                name: "mob_spawn_member",
                args: &spawn_args,
            })
            .await
            .expect("owner-bound spawn should succeed");

        let roster = handle.roster().await;
        let child = roster
            .get_by_identity(&AgentIdentity::from("child"))
            .expect("child roster entry");
        assert!(
            child.wired_to.contains(&parent_identity),
            "auto_wire_parent must wire the spawned member to the bound spawning member"
        );
    }

    #[tokio::test]
    async fn test_delegate_dispatch_auto_wires_parent_and_helper_peers() {
        let service = Arc::new(RealCommsSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            service.clone(),
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let parent_name = "parent/lead/l-1".to_string();
        let parent_comms = service.register_external_comms(&parent_name).await;
        let parent_peer_id = parent_comms.peer_id().expect("parent peer id");
        let snapshot_context = parent_snapshot_context_for_tools(vec![Arc::new(ToolDef {
            name: "read_file".into(),
            description: "read_file tool".to_string(),
            input_schema: json!({"type": "object"}),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Builtin,
                source_id: "test-read-file".into(),
            }),
        })])
        .await;
        let session_id = SessionId::new();
        let surface = AgentMobToolSurface::new_with_effective_authority(
            Arc::clone(&state),
            None,
            Arc::new(std::sync::RwLock::new(create_only_authority())),
            "claude-sonnet-4-5".to_string(),
            session_id,
            Some(parent_name.clone()),
            Some(parent_peer_id),
            Some(parent_comms.clone() as Arc<dyn CoreCommsRuntime>),
            snapshot_context,
        );

        let delegate_args = serde_json::value::RawValue::from_string(
            json!({
                "member_id": "helper-dispatch-1",
                "task": "report back",
                "result_label": "delegate_result",
                "max_text_bytes": 4096
            })
            .to_string(),
        )
        .unwrap();
        let result = surface
            .dispatch(ToolCallView {
                id: "delegate-wired",
                name: "delegate",
                args: &delegate_args,
            })
            .await
            .expect("delegate should spawn and wire helper");
        let parsed: serde_json::Value =
            serde_json::from_str(&result.result.text_content()).expect("delegate JSON");
        assert_eq!(
            parsed["wired"].as_bool(),
            Some(true),
            "delegate() must report wired=true when parent comms are available"
        );
        assert_eq!(
            parsed["output"].as_str(),
            Some("ok"),
            "delegate() must return the canonical result from its exact admitted turn"
        );
        assert_eq!(
            parsed["bounded_result"]["text"].as_str(),
            Some("ok"),
            "delegate() must project the same exact turn through the bounded carrier"
        );
        assert!(
            parsed["retirement_error"].as_str().is_some(),
            "this harness cannot archive the helper, so cleanup debt must stay explicit"
        );

        let mob_id = parsed["mob_id"].as_str().expect("mob id");
        let helper_name = format!("{mob_id}/delegate/helper-dispatch-1");
        let parent_peers = CoreCommsRuntime::peers(&*parent_comms).await;
        assert!(
            parent_peers
                .iter()
                .any(|entry| entry.name.as_str() == helper_name),
            "delegate() should add the helper to the parent peer directory"
        );
    }

    #[tokio::test]
    async fn test_delegate_wiring_links_parent_and_helper_peers_and_emits_peer_added_lifecycle() {
        let service = Arc::new(RealCommsSessionSvc::new());
        let state = Arc::new(MobMcpState::new(
            service.clone(),
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let parent_name = "parent/lead/l-1".to_string();
        let parent_comms = service.register_external_comms(&parent_name).await;
        let parent_peer_id = parent_comms.peer_id().expect("parent peer id");
        let parent_public_key = parent_comms
            .public_key_bytes()
            .expect("parent public key bytes");
        assert_ne!(parent_public_key, [0u8; 32]);
        assert_eq!(
            parent_peer_id,
            PeerId::from_ed25519_pubkey(&parent_public_key),
            "regression fixture must derive peer id from typed public-key bytes"
        );
        let session_id = SessionId::new();
        let probe_surface = AgentMobToolSurface::new(
            Arc::clone(&state),
            None,
            create_only_authority(),
            "claude-sonnet-4-5".to_string(),
            session_id,
            None,
            None,
            None,
        );
        let (mob_id, _created) = probe_surface
            .ensure_implicit_mob()
            .await
            .expect("create implicit mob");
        let helper_id = AgentIdentity::from("helper-1");
        let handle = state.handle_for(&mob_id).await.expect("mob handle");
        let result_spec =
            BoundedResultSpec::new("delegate_result", 4096).expect("valid delegate result spec");
        let mut request =
            DelegationExecutionRequest::new(helper_id.clone(), "report back", result_spec);
        request.parent = Some(DelegationParentContext::new(
            parent_name.clone(),
            parent_peer_id,
            parent_comms.clone() as Arc<dyn CoreCommsRuntime>,
        ));
        let delegation_service = DelegationExecutionService::new(handle.clone());
        let execution = delegation_service
            .start(request)
            .await
            .expect("start delegated helper for wiring test")
            .await_terminal()
            .await;
        assert!(
            execution.wired(),
            "delegate wiring should succeed when creator comms are present"
        );

        let helper_bridge_session_id = handle
            .resolve_bridge_session_id(&execution.spawn().agent_identity)
            .await
            .expect("helper bridge session id");
        let helper_comms = service
            .real_comms(&helper_bridge_session_id)
            .await
            .expect("helper comms");
        let helper_name =
            meerkat_core::MemberCommsName::new(mob_id.as_str(), "delegate", helper_id.as_str())
                .expect("delegate helper comms name")
                .to_string();

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
        assert!(
            parent_comms
                .trusted
                .read()
                .await
                .values()
                .any(|spec| spec.name.as_str() == helper_name && !spec.has_zero_pubkey()),
            "parent must trust helper with a non-zero pubkey"
        );
        assert!(
            helper_comms
                .trusted
                .read()
                .await
                .values()
                .any(|spec| spec.name.as_str() == parent_name && !spec.has_zero_pubkey()),
            "helper must trust parent with a non-zero pubkey"
        );

        let parent_inbox = CoreCommsRuntime::handoff_volatile_peer_input_candidates(&*parent_comms)
            .await
            .expect("exact parent volatile handoff")
            .into_iter()
            .map(|candidate| candidate.interaction)
            .collect::<Vec<_>>();
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

        let helper_inbox = CoreCommsRuntime::handoff_volatile_peer_input_candidates(&*helper_comms)
            .await
            .expect("exact helper volatile handoff")
            .into_iter()
            .map(|candidate| candidate.interaction)
            .collect::<Vec<_>>();
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
        assert!(
            delegation_service
                .retire_terminalized(&execution)
                .await
                .is_err(),
            "this fixture retains explicit cleanup debt after the wiring evidence is inspected"
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

    fn surface_with_profiles_and_authority(
        state: Arc<MobMcpState>,
        authority: MobToolAuthorityContext,
    ) -> AgentMobToolSurface {
        AgentMobToolSurface::new(
            state,
            None,
            authority,
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
            None,
            None,
        )
    }

    #[test]
    fn test_profile_tools_present_when_store_available() {
        let defs = build_tool_defs_with_profile_support(true, false, false);
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
        let defs = build_tool_defs_with_profile_support(false, false, false);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(!names.contains(&"mob_profile_create"));
        assert!(!names.contains(&"mob_profile_list_sources"));
    }

    #[test]
    fn test_list_sources_tool_present_when_both_store_and_provider() {
        let defs = build_tool_defs_with_profile_support(true, true, false);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"mob_profile_list_sources"));
    }

    #[test]
    fn test_adaptive_authority_does_not_expose_adaptive_named_agent_tools() {
        let without = build_tool_defs_with_profile_support(false, false, false);
        let without_names: Vec<&str> = without.iter().map(|d| d.name.as_str()).collect();
        assert!(!without_names.contains(&"adaptive_flow_start"));

        let with = build_tool_defs_with_profile_support(false, false, true);
        assert!(
            with.iter()
                .all(|tool| !tool.name.as_str().contains("adaptive")),
            "agent tools must expose mob run capabilities without adaptive implementation names"
        );
        assert!(
            with.iter().all(|tool| {
                tool.provenance
                    .as_ref()
                    .is_none_or(|provenance| provenance.source_id.as_str() != "mob-adaptive")
            }),
            "agent tool provenance must not expose the adaptive implementation crate"
        );
    }

    #[tokio::test]
    async fn test_profile_crud_roundtrip() {
        let state = MobMcpState::new_in_memory();
        let surface = surface_with_profiles(Arc::clone(&state));

        // Create
        let create_args = serde_json::value::RawValue::from_string(
            json!({
                "name": "worker",
                "profile": sample_profile_json("claude-opus-4-8")
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
        assert_eq!(got["profile"]["model"], "claude-opus-4-8");

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
    async fn test_profile_mutation_requires_profile_authority() {
        let state = MobMcpState::new_in_memory();
        let surface = surface_with_profiles_and_authority(
            Arc::clone(&state),
            meerkat_runtime::mob_operator_authority::set_profile_mutation(
                &create_only_authority(),
                false,
            )
            .expect("generated mob authority should disable profile mutation"),
        );
        let create_args = serde_json::value::RawValue::from_string(
            json!({
                "name": "worker",
                "profile": sample_profile_json("claude-opus-4-8")
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
            .await;
        assert!(
            create_result.is_err(),
            "profile create must require explicit profile mutation authority"
        );
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
                "profile": sample_profile_json("claude-opus-4-8")
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
        let state = MobMcpState::new_in_memory();
        let snapshot_context = parent_snapshot_context_for_tools(vec![
            Arc::new(ToolDef {
                name: "tool_a".into(),
                description: "Tool A".to_string(),
                input_schema: json!({"type": "object"}),
                provenance: Some(ToolProvenance {
                    kind: ToolSourceKind::Builtin,
                    source_id: "core".into(),
                }),
            }),
            Arc::new(ToolDef {
                name: "tool_b".into(),
                description: "Tool B".to_string(),
                input_schema: json!({"type": "object"}),
                provenance: Some(ToolProvenance {
                    kind: ToolSourceKind::Mob,
                    source_id: "mob".into(),
                }),
            }),
        ])
        .await;
        let surface = AgentMobToolSurface::new_with_effective_authority(
            Arc::clone(&state),
            None,
            Arc::new(std::sync::RwLock::new(create_only_authority())),
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
            None,
            None,
            snapshot_context,
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

    // ─── Tool access policy spawn resolution tests ──────────────────────

    fn allow_policy(names: &[&str]) -> meerkat_core::ops::ToolAccessPolicy {
        meerkat_core::ops::ToolAccessPolicy::AllowList(names.iter().copied().collect())
    }

    fn deny_policy(names: &[&str]) -> meerkat_core::ops::ToolAccessPolicy {
        meerkat_core::ops::ToolAccessPolicy::DenyList(names.iter().copied().collect())
    }

    /// Public constructors predate the opaque AgentFactory composition
    /// authority. They must continue to inherit a restricted parent's durable
    /// policy, while explicit child policy remains a metadata-free fast path.
    #[tokio::test]
    async fn public_constructor_preserves_restricted_parent_policy_containment() {
        let service = Arc::new(RealCommsSessionSvc::new());
        let parent_session_id = SessionId::new();
        let parent_policy = allow_policy(&["read_file"]);
        let mut parent_session = meerkat_core::Session::with_id(parent_session_id.clone());
        parent_session
            .set_session_metadata(meerkat_core::SessionMetadata {
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: "claude-sonnet-4-5".to_string(),
                max_tokens: 4096,
                structured_output_retries: meerkat_core::config::default_structured_output_retries(
                ),
                provider: Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: meerkat_core::SessionTooling {
                    tool_access_policy: Some(parent_policy.clone()),
                    ..meerkat_core::SessionTooling::default()
                },
                keep_alive: false,
                comms_name: None,
                peer_meta: None,
                realm_id: None,
                instance_id: None,
                backend: None,
                config_generation: None,
                auth_binding: None,
                mob_member_binding: None,
            })
            .expect("restricted parent metadata must serialize");
        service.seed_persisted_session(parent_session).await;
        let session_service: Arc<dyn meerkat_mob::MobSessionService> = service.clone();
        let state = Arc::new(MobMcpState::new(
            session_service,
            meerkat_mob::MobControlPrincipal::Owner,
        ));
        let surface = AgentMobToolSurface::new(
            state,
            None,
            create_only_authority(),
            "claude-sonnet-4-5".to_string(),
            parent_session_id,
            None,
            None,
            None,
        );

        let inherited = surface
            .resolve_child_tool_access_policy_boxed(TOOL_MOB_SPAWN_MEMBER, None)
            .await
            .expect("legacy constructor must resolve persisted parent policy");
        assert_eq!(inherited, Some(parent_policy));
        assert_eq!(
            service.persisted_metadata_loads.load(Ordering::Relaxed),
            1,
            "inherited policy must use the legacy metadata seam exactly once"
        );

        let explicit = deny_policy(&["bash"]);
        let explicit_result = surface
            .resolve_child_tool_access_policy_boxed(TOOL_MOB_SPAWN_MEMBER, Some(explicit.clone()))
            .await
            .expect("explicit child policy remains admitted");
        assert_eq!(explicit_result, Some(explicit));
        assert_eq!(
            service.persisted_metadata_loads.load(Ordering::Relaxed),
            1,
            "explicit child policy must bypass the parent metadata read"
        );
    }

    /// Escape regression: a restricted parent + `Inherit` (or absent) child
    /// request must stay restricted — spawning is not a policy escape hatch.
    #[test]
    fn inherit_child_policy_stays_restricted_under_restricted_parent() {
        let parent = allow_policy(&["read_file"]);
        assert_eq!(
            effective_child_tool_access_policy(
                Some(meerkat_core::ops::ToolAccessPolicy::Inherit),
                Some(parent.clone()),
            ),
            Some(parent.clone()),
        );
        assert_eq!(
            effective_child_tool_access_policy(None, Some(parent.clone())),
            Some(parent),
        );
    }

    /// Explicit `AllowList`/`DenyList` is admitted as-is — presence is
    /// already the MobMachine-privileged admission fact, and resolution must
    /// not re-shape it.
    #[test]
    fn explicit_child_policy_is_admitted_as_is() {
        let parent = allow_policy(&["read_file"]);
        let explicit_deny = deny_policy(&["bash"]);
        assert_eq!(
            effective_child_tool_access_policy(Some(explicit_deny.clone()), Some(parent)),
            Some(explicit_deny),
        );
        let explicit_allow = allow_policy(&["send_message"]);
        assert_eq!(
            effective_child_tool_access_policy(Some(explicit_allow.clone()), None),
            Some(explicit_allow),
        );
    }

    /// No parent policy (host/operator launch, ephemeral parent, or an
    /// unrestricted parent) resolves `Inherit`/absent to unrestricted.
    #[test]
    fn inherit_without_parent_policy_resolves_unrestricted() {
        assert_eq!(
            effective_child_tool_access_policy(
                Some(meerkat_core::ops::ToolAccessPolicy::Inherit),
                None,
            ),
            None,
        );
        assert_eq!(effective_child_tool_access_policy(None, None), None);
    }

    // ─── SpawnTooling resolution tests (T2.3) ───────────────────────────

    /// Comms + non-comms tools for overlay testing.
    fn tooling_test_tools() -> Vec<Arc<ToolDef>> {
        [
            "send",
            "send_message",
            "reply_to_peer",
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
                name: (*name).into(),
                description: format!("{name} tool"),
                input_schema: json!({"type": "object"}),
                provenance: Some(ToolProvenance {
                    kind: ToolSourceKind::Callback,
                    source_id: format!("parent-{name}").into(),
                }),
            })
        })
        .collect()
    }

    async fn surface_with_parent_tools() -> AgentMobToolSurface {
        let snapshot_context = parent_snapshot_context_for_tools(tooling_test_tools()).await;
        AgentMobToolSurface::new_with_effective_authority(
            MobMcpState::new_in_memory(),
            None,
            Arc::new(std::sync::RwLock::new(create_only_authority())),
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
            None,
            None,
            snapshot_context,
        )
    }

    async fn surface_with_filtered_parent_tools() -> AgentMobToolSurface {
        let filter = meerkat_core::ToolFilter::Deny(["bash".to_string()].into_iter().collect());
        let snapshot_context =
            parent_snapshot_context_for_tools_with_filter(tooling_test_tools(), Some(filter)).await;
        AgentMobToolSurface::new_with_effective_authority(
            MobMcpState::new_in_memory(),
            None,
            Arc::new(std::sync::RwLock::new(create_only_authority())),
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
            None,
            None,
            snapshot_context,
        )
    }

    async fn surface_with_unprovenanced_parent_tool() -> AgentMobToolSurface {
        let snapshot_context = parent_snapshot_context_for_tools(vec![Arc::new(ToolDef {
            name: "external_ob3_tool".into(),
            description: "external Ob3 tool".to_string(),
            input_schema: json!({"type": "object"}),
            provenance: None,
        })])
        .await;
        AgentMobToolSurface::new_with_effective_authority(
            MobMcpState::new_in_memory(),
            None,
            Arc::new(std::sync::RwLock::new(create_only_authority())),
            "claude-sonnet-4-5".to_string(),
            SessionId::new(),
            None,
            None,
            None,
            snapshot_context,
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

    fn inherited_allow_names(resolved: ResolvedSpawnTooling) -> meerkat_core::types::ToolNameSet {
        let authority = resolved
            .inherited_tool_filter
            .expect("expected inherited tool filter authority");
        let names = match authority.filter().clone() {
            meerkat_core::tool_scope::ToolFilter::Allow(names) => names,
            other => panic!("expected Allow, got {other:?}"),
        };
        for name in &names {
            assert!(
                authority
                    .witnesses()
                    .get(name.as_str())
                    .is_some_and(meerkat_core::ToolVisibilityWitness::has_identity_witness),
                "inherited filter should carry witness for {name}"
            );
        }
        names
    }

    #[tokio::test]
    async fn test_resolve_spawn_tooling_inherit_parent_captures_all_visible() {
        let surface = surface_with_parent_tools().await;
        let tooling = meerkat_mob::SpawnTooling::InheritParent {
            allow_overlay: None,
            deny_overlay: None,
        };
        let resolved = surface.resolve_spawn_tooling(&tooling).await.unwrap();
        let names = inherited_allow_names(resolved);
        assert_eq!(names.len(), 9, "should inherit all 9 parent tools");
        assert!(names.contains("send"));
        assert!(names.contains("read_file"));
        assert!(names.contains("bash"));
    }

    #[tokio::test]
    async fn test_resolve_spawn_tooling_inherit_parent_uses_tool_scope_visibility() {
        let surface = surface_with_filtered_parent_tools().await;
        let tooling = meerkat_mob::SpawnTooling::InheritParent {
            allow_overlay: None,
            deny_overlay: None,
        };
        let resolved = surface.resolve_spawn_tooling(&tooling).await.unwrap();
        let names = inherited_allow_names(resolved);
        assert_eq!(names.len(), 8, "hidden parent tools must not be inherited");
        assert!(names.contains("read_file"));
        assert!(!names.contains("bash"));
    }

    #[tokio::test]
    async fn test_resolve_spawn_tooling_inherit_parent_rejects_unprovenanced_parent_tool() {
        let surface = surface_with_unprovenanced_parent_tool().await;
        let tooling = meerkat_mob::SpawnTooling::InheritParent {
            allow_overlay: None,
            deny_overlay: None,
        };
        let err = surface.resolve_spawn_tooling(&tooling).await.unwrap_err();

        match err {
            ToolError::ExecutionFailed { message } => {
                assert!(message.contains("requires tool provenance witnesses"));
                assert!(message.contains("external_ob3_tool"));
            }
            other => {
                panic!("expected ExecutionFailed for unprovenanced parent tool, got {other:?}")
            }
        }
    }

    #[tokio::test]
    async fn test_resolve_spawn_tooling_inherit_parent_with_deny_overlay() {
        let surface = surface_with_parent_tools().await;
        let tooling = meerkat_mob::SpawnTooling::InheritParent {
            allow_overlay: None,
            deny_overlay: Some(vec!["bash".to_string(), "write_file".to_string()]),
        };
        let resolved = surface.resolve_spawn_tooling(&tooling).await.unwrap();
        let names = inherited_allow_names(resolved);
        assert_eq!(names.len(), 7);
        assert!(!names.contains("bash"));
        assert!(!names.contains("write_file"));
        assert!(names.contains("read_file"));
        assert!(names.contains("send"));
    }

    #[tokio::test]
    async fn test_resolve_spawn_tooling_inherit_parent_with_allow_overlay() {
        let surface = surface_with_parent_tools().await;
        let tooling = meerkat_mob::SpawnTooling::InheritParent {
            allow_overlay: Some(vec!["send".to_string(), "read_file".to_string()]),
            deny_overlay: None,
        };
        let resolved = surface.resolve_spawn_tooling(&tooling).await.unwrap();
        let names = inherited_allow_names(resolved);
        assert_eq!(names.len(), 2);
        assert!(names.contains("send"));
        assert!(names.contains("read_file"));
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
        let surface = surface_with_parent_tools().await;
        let tooling = meerkat_mob::SpawnTooling::Minimal;
        let resolved = surface.resolve_spawn_tooling(&tooling).await.unwrap();
        let names = inherited_allow_names(resolved);
        assert_eq!(names.len(), 6);
        assert!(names.contains("send"));
        assert!(names.contains("send_message"));
        assert!(names.contains("reply_to_peer"));
        assert!(names.contains("send_request"));
        assert!(names.contains("send_response"));
        assert!(names.contains("peers"));
        assert!(!names.contains("bash"));
        assert!(!names.contains("read_file"));
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
        let surface = surface_with_parent_tools().await;
        let tooling = meerkat_mob::SpawnTooling::Profile {
            source: Box::new(meerkat_mob::ProfileSource::Inline(Box::new(
                meerkat_mob::Profile {
                    model: "claude-sonnet-4-5".to_string(),
                    provider: None,
                    self_hosted_server_id: None,
                    image_generation_provider: None,
                    auto_compact_threshold: None,
                    resume_overrides: Vec::new(),
                    skills: Vec::new(),
                    tools: meerkat_mob::ToolConfig::default(),
                    peer_description: "test".to_string(),
                    external_addressable: false,
                    backend: None,
                    runtime_mode: MobRuntimeMode::TurnDriven,
                    max_inline_peer_notifications: None,
                    output_schema: None,
                    provider_params: None,
                },
            ))),
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
        let surface = surface_with_parent_tools().await;
        let tooling = meerkat_mob::SpawnTooling::Profile {
            source: Box::new(meerkat_mob::ProfileSource::Inline(Box::new(
                meerkat_mob::Profile {
                    model: "claude-sonnet-4-5".to_string(),
                    provider: None,
                    self_hosted_server_id: None,
                    image_generation_provider: None,
                    auto_compact_threshold: None,
                    resume_overrides: Vec::new(),
                    skills: Vec::new(),
                    tools: meerkat_mob::ToolConfig::default(),
                    peer_description: "test".to_string(),
                    external_addressable: false,
                    backend: None,
                    runtime_mode: MobRuntimeMode::TurnDriven,
                    max_inline_peer_notifications: None,
                    output_schema: None,
                    provider_params: None,
                },
            ))),
            allow_overlay: None,
            deny_overlay: Some(vec!["bash".to_string()]),
        };
        let resolved = surface.resolve_spawn_tooling(&tooling).await.unwrap();
        let names = inherited_allow_names(resolved);
        assert!(!names.contains("bash"));
        assert!(names.contains("read_file"));
    }

    #[tokio::test]
    async fn test_resolve_spawn_tooling_profile_with_overlays_standalone_errors() {
        let surface = surface_standalone();
        let tooling = meerkat_mob::SpawnTooling::Profile {
            source: Box::new(meerkat_mob::ProfileSource::Inline(Box::new(
                meerkat_mob::Profile {
                    model: "claude-sonnet-4-5".to_string(),
                    provider: None,
                    self_hosted_server_id: None,
                    image_generation_provider: None,
                    auto_compact_threshold: None,
                    resume_overrides: Vec::new(),
                    skills: Vec::new(),
                    tools: meerkat_mob::ToolConfig::default(),
                    peer_description: "test".to_string(),
                    external_addressable: false,
                    backend: None,
                    runtime_mode: MobRuntimeMode::TurnDriven,
                    max_inline_peer_notifications: None,
                    output_schema: None,
                    provider_params: None,
                },
            ))),
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
        let surface = surface_with_parent_tools().await;
        let expected_model = "claude-opus-4-8".to_string();
        let tooling = meerkat_mob::SpawnTooling::Profile {
            source: Box::new(meerkat_mob::ProfileSource::Inline(Box::new(
                meerkat_mob::Profile {
                    model: expected_model.clone(),
                    provider: None,
                    self_hosted_server_id: None,
                    image_generation_provider: None,
                    auto_compact_threshold: None,
                    resume_overrides: Vec::new(),
                    skills: Vec::new(),
                    tools: meerkat_mob::ToolConfig::default(),
                    peer_description: "override test".to_string(),
                    external_addressable: false,
                    backend: None,
                    runtime_mode: MobRuntimeMode::TurnDriven,
                    max_inline_peer_notifications: None,
                    output_schema: None,
                    provider_params: None,
                },
            ))),
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
