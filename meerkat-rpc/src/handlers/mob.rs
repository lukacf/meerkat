//! `mob/*` method handlers.

use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;
use std::convert::TryFrom;

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;
use meerkat::surface::RequestContext;
use meerkat_contracts::wire::WireMobProfile;
use meerkat_contracts::{
    ErrorCode, MobAppendSystemContextResult, MobCancelAllWorkResult, MobCancelWorkResult,
    MobCreateParams, MobCreateResult, MobDestroyResult, MobEventsResult, MobFlowCancelResult,
    MobFlowRunResult, MobFlowsResult, MobForceCancelResult, MobHelperResult, MobLifecycleResult,
    MobListResult, MobMemberListEntryWire, MobMembersResult, MobProfileDeleteResult,
    MobRespawnReceipt, MobRespawnResult, MobRetireResult, MobRotateSupervisorResult, MobRunParams,
    MobRunResult, MobRunResultParams, MobSnapshotResult, MobSpawnManyResult,
    MobSpawnManyResultEntry, MobSpawnResult, MobStatusResult, MobUnwireResult,
    MobWaitMembersResult, MobWireMembersBatchEdge, MobWireMembersBatchParams,
    MobWireMembersBatchResult, MobWireResult, SupervisorRotationIncompleteDataWire,
    SupervisorRotationIncompleteDetailsWire, SupervisorRotationReportWire,
    SupervisorRotationRetryAuthority, SupervisorRotationRetryScope, WireMobBackendKind,
    WireMobMemberStatus, WireMobRespawnOutcome, WireMobRuntimeMode,
};
use meerkat_core::lifecycle::run_primitive::TurnMetadataOverride;
use meerkat_core::service::{AppendSystemContextRequest, TurnToolOverlay};
use meerkat_core::types::ContentInput;
use meerkat_mob::{
    AgentIdentity, FlowId, MemberRespawnReceipt, MobBackendKind, MobError, MobId, MobMemberStatus,
    MobRespawnError, MobRuntimeMode, Profile, RunId, SpawnMemberSpec, SpawnResult, ToolConfig,
};
use meerkat_mob_mcp::{MobMcpDestroyError, MobMcpState};
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

fn invalid_params(id: Option<RpcId>, message: impl Into<String>) -> RpcResponse {
    RpcResponse::error(id, error::INVALID_PARAMS, message.into())
}

fn mob_rotate_supervisor_error(id: Option<RpcId>, err: &MobError) -> RpcResponse {
    match err {
        MobError::SupervisorRotationIncomplete {
            previous_epoch,
            attempted_epoch,
            attempted_public_peer_id,
            rotated_peer_count,
            rollback_succeeded,
            pending_authority_recorded,
            rollback_error,
            ..
        } => {
            let code = ErrorCode::SupervisorRotationIncomplete;
            let message = err.to_string();
            let details = SupervisorRotationIncompleteDetailsWire {
                kind: meerkat_contracts::SupervisorRotationIncompleteKind::SupervisorRotationIncomplete,
                previous_epoch: *previous_epoch,
                attempted_epoch: *attempted_epoch,
                attempted_public_peer_id: attempted_public_peer_id.clone(),
                rotated_peer_count: *rotated_peer_count,
                rollback_succeeded: *rollback_succeeded,
                pending_authority_recorded: *pending_authority_recorded,
                rollback_error: rollback_error.clone(),
                retry_authority: if *pending_authority_recorded {
                    SupervisorRotationRetryAuthority::PendingRotation
                } else {
                    SupervisorRotationRetryAuthority::PreRotation
                },
                retry_scope: if *pending_authority_recorded {
                    SupervisorRotationRetryScope::Durable
                } else {
                    SupervisorRotationRetryScope::PreRotation
                },
            };
            let data = SupervisorRotationIncompleteDataWire {
                code: code.to_string(),
                message: message.clone(),
                details,
            };
            match serde_json::to_value(&data) {
                Ok(data) => RpcResponse::error_with_data(id, code.jsonrpc_code(), message, data),
                Err(serialize_error) => RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!(
                        "failed to serialize supervisor rotation incomplete error data: \
                         {serialize_error}"
                    ),
                ),
            }
        }
        _ => invalid_params(id, err.to_string()),
    }
}

fn destroy_incomplete_response(
    id: Option<RpcId>,
    report: &meerkat_mob::MobDestroyReport,
) -> RpcResponse {
    RpcResponse::error_with_data(
        id,
        error::INTERNAL_ERROR,
        MobMcpDestroyError::incomplete_message(report),
        MobMcpDestroyError::incomplete_error_data(report),
    )
}

fn mob_runtime_mode_from_wire(mode: WireMobRuntimeMode) -> MobRuntimeMode {
    match mode {
        WireMobRuntimeMode::AutonomousHost => MobRuntimeMode::AutonomousHost,
        WireMobRuntimeMode::TurnDriven => MobRuntimeMode::TurnDriven,
    }
}

fn mob_backend_kind_from_wire(kind: WireMobBackendKind) -> MobBackendKind {
    match kind {
        WireMobBackendKind::Session => MobBackendKind::Session,
        WireMobBackendKind::External => MobBackendKind::External,
    }
}

fn resume_override_field_from_wire(
    field: meerkat_contracts::WireMobResumeOverrideField,
) -> meerkat_mob::ResumeOverrideField {
    match field {
        meerkat_contracts::WireMobResumeOverrideField::Model => {
            meerkat_mob::ResumeOverrideField::Model
        }
        meerkat_contracts::WireMobResumeOverrideField::Provider => {
            meerkat_mob::ResumeOverrideField::Provider
        }
        meerkat_contracts::WireMobResumeOverrideField::ProviderParams => {
            meerkat_mob::ResumeOverrideField::ProviderParams
        }
    }
}

fn profile_from_wire(profile: WireMobProfile) -> Result<Profile, meerkat_core::SchemaError> {
    let tools = profile.tools;
    // Wire-decode validation: the profile output schema is validated into the
    // typed `MeerkatSchema` owner here, at the boundary, instead of ferrying a
    // raw `Value` into the mob domain.
    let output_schema = profile
        .output_schema
        .map(meerkat_core::MeerkatSchema::new)
        .transpose()?;
    Ok(Profile {
        model: profile.model,
        provider: profile.provider,
        self_hosted_server_id: profile.self_hosted_server_id,
        image_generation_provider: profile.image_generation_provider,
        auto_compact_threshold: profile.auto_compact_threshold,
        resume_overrides: profile
            .resume_overrides
            .into_iter()
            .map(resume_override_field_from_wire)
            .collect(),
        skills: profile.skills,
        tools: ToolConfig {
            builtins: tools.builtins,
            shell: tools.shell,
            comms: tools.comms,
            memory: tools.memory,
            workgraph: tools.workgraph,
            mob: tools.mob,
            schedule: tools.schedule,
            image_generation: tools.image_generation,
            mcp: tools.mcp,
            rust_bundles: Vec::new(),
        },
        peer_description: profile.peer_description,
        external_addressable: profile.external_addressable,
        backend: profile.backend.map(mob_backend_kind_from_wire),
        runtime_mode: mob_runtime_mode_from_wire(profile.runtime_mode),
        max_inline_peer_notifications: profile.max_inline_peer_notifications,
        output_schema,
        provider_params: profile.provider_params.map(Into::into),
    })
}

#[allow(clippy::result_large_err)]
fn parse_mob_id(id: Option<RpcId>, raw: &str) -> Result<MobId, RpcResponse> {
    if raw.trim().is_empty() {
        return Err(invalid_params(id, "mob_id must not be empty"));
    }
    Ok(MobId::from(raw))
}

fn spawn_result_payload(mob_id: &MobId, result: &SpawnResult) -> MobSpawnResult {
    let identity_str = result.agent_identity.to_string();
    MobSpawnResult {
        mob_id: mob_id.to_string(),
        agent_identity: identity_str.clone(),
        member_ref: meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), &identity_str),
    }
}

/// Project a domain `MobMemberStatus` into its wire twin.
///
/// `MobMemberStatus` is `#[non_exhaustive]`, so a plain exhaustive match is not
/// possible without a wildcard. We map every known variant explicitly and fail
/// the wire projection on the wildcard arm: an unmapped upstream variant is a
/// genuine projection-boundary fault, and reporting it as `Unknown` (a valid
/// status the client trusts) would launder an unrepresentable domain state into
/// a fabricated valid one. Adding the variant to this match is the expected
/// response to the returned error.
fn project_member_status(status: MobMemberStatus) -> Result<WireMobMemberStatus, String> {
    match status {
        MobMemberStatus::Active => Ok(WireMobMemberStatus::Active),
        MobMemberStatus::Retiring => Ok(WireMobMemberStatus::Retiring),
        MobMemberStatus::Broken => Ok(WireMobMemberStatus::Broken),
        MobMemberStatus::Completed => Ok(WireMobMemberStatus::Completed),
        MobMemberStatus::Unknown => Ok(WireMobMemberStatus::Unknown),
        other => Err(format!(
            "MobMemberStatus variant {other:?} has no WireMobMemberStatus mapping; \
             add an explicit arm in `project_member_status`"
        )),
    }
}

fn runtime_binding_from_wire(
    binding: meerkat_contracts::WireRuntimeBinding,
) -> Result<meerkat_mob::RuntimeBinding, String> {
    match binding {
        meerkat_contracts::WireRuntimeBinding::Session => Ok(meerkat_mob::RuntimeBinding::Session),
        meerkat_contracts::WireRuntimeBinding::External {
            address,
            bootstrap_token,
            identity,
        } => {
            let resolved = identity.resolve().map_err(|err| err.to_string())?;
            Ok(meerkat_mob::RuntimeBinding::External {
                peer_id: resolved.peer_id.to_string(),
                address,
                bootstrap_token,
                pubkey: resolved.pubkey,
            })
        }
    }
}

/// Convert a mob roster entry into the public typed wire shape. Used for
/// `mob/ensure_member`'s `Existed` outcome and for typed member-list
/// responses.
fn member_list_entry_wire(
    mob_id: &MobId,
    entry: &meerkat_mob::runtime::MobMemberListEntry,
) -> Result<MobMemberListEntryWire, String> {
    let identity_str = entry.agent_identity.to_string();
    Ok(MobMemberListEntryWire {
        agent_identity: identity_str.clone(),
        member_ref: meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), &identity_str),
        role: entry.role.to_string(),
        runtime_mode: match entry.runtime_mode {
            MobRuntimeMode::AutonomousHost => WireMobRuntimeMode::AutonomousHost,
            MobRuntimeMode::TurnDriven => WireMobRuntimeMode::TurnDriven,
        },
        wired_to: entry
            .wired_to
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
        labels: entry.labels.clone(),
        status: project_member_status(entry.status)?,
        error: entry.error.clone(),
        is_final: entry.is_final,
    })
}

pub async fn handle_create(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobCreateParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };

    let definition = match meerkat_mob_mcp::decode_public_mob_definition(params.definition) {
        Ok(definition) => definition,
        Err(error) => return invalid_params(id, format!("invalid mob definition: {error}")),
    };

    let result = state.mob_create_definition(definition).await;

    match result {
        Ok(mob_id) => RpcResponse::success(
            id,
            MobCreateResult {
                mob_id: mob_id.to_string(),
            },
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_list(id: Option<RpcId>, state: &Arc<MobMcpState>) -> RpcResponse {
    let mobs = match state.mob_list().await {
        Ok(mobs) => mobs,
        Err(err) => return RpcResponse::error(id, error::INTERNAL_ERROR, err.to_string()),
    };
    let mobs = mobs
        .into_iter()
        .map(|(mob_id, status)| MobStatusResult {
            mob_id: mob_id.to_string(),
            status: meerkat_mob_mcp::wire_mob_lifecycle_status(status),
        })
        .collect::<Vec<_>>();
    RpcResponse::success(id, MobListResult { mobs })
}

#[derive(Debug, Deserialize)]
pub struct MobIdParams {
    pub mob_id: String,
}

pub async fn handle_status(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobIdParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state.mob_status(&mob_id).await {
        Ok(status) => RpcResponse::success(
            id,
            MobStatusResult {
                mob_id: mob_id.to_string(),
                status: meerkat_mob_mcp::wire_mob_lifecycle_status(status),
            },
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobMembersParams {
    pub mob_id: String,
}

pub async fn handle_members(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobMembersParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state.mob_list_members(&mob_id).await {
        Ok(members) => {
            let typed: Result<Vec<_>, String> = members
                .iter()
                .map(|entry| member_list_entry_wire(&mob_id, entry))
                .collect();
            match typed {
                Ok(typed) => RpcResponse::success(
                    id,
                    MobMembersResult {
                        mob_id: mob_id.to_string(),
                        members: typed,
                    },
                ),
                Err(projection_error) => {
                    RpcResponse::error(id, error::INTERNAL_ERROR, projection_error)
                }
            }
        }
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobSpawnParams {
    pub mob_id: String,
    pub profile: String,
    pub agent_identity: String,
    #[serde(default)]
    pub initial_message: Option<ContentInput>,
    #[serde(default)]
    pub runtime_mode: Option<MobRuntimeMode>,
    #[serde(default)]
    pub backend: Option<MobBackendKind>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub context: Option<Value>,
    #[serde(default)]
    pub additional_instructions: Option<Vec<String>>,
    // DELETE_ME A10: `SpawnMemberSpec` has 15 public fields; the RPC
    // mirrors every non-internal one. The first pass added `binding`,
    // `shell_env`, and `auto_wire_parent`; A3 + C1 unblocked
    // `launch_mode`; and this pass adds `tool_access_policy`,
    // `budget_split_policy`, `inherited_tool_filter`, and
    // `override_profile` to reach full parity with Rust-in-process spawn.
    // Internal-only profile fields stay behind the RPC boundary: override
    // profiles parse through a public wire-owned profile shape and are then
    // converted into the runtime profile type.
    /// Optional runtime binding for external-peer members (maps to
    /// SpawnMemberSpec::binding).
    #[serde(default)]
    pub binding: Option<meerkat_contracts::WireRuntimeBinding>,
    /// Per-agent environment variables injected into shell tool subprocesses
    /// (maps to SpawnMemberSpec::shell_env).
    #[serde(default)]
    pub shell_env: Option<std::collections::HashMap<String, String>>,
    /// Whether the spawned member should be auto-wired to its spawner
    /// (maps to SpawnMemberSpec::auto_wire_parent).
    #[serde(default)]
    pub auto_wire_parent: Option<bool>,
    /// How this member should be launched (fresh / resume / fork)
    /// (maps to SpawnMemberSpec::launch_mode). Defaults to `Fresh`.
    #[serde(default)]
    pub launch_mode: Option<meerkat_mob::MemberLaunchMode>,
    /// Tool access policy for the spawned member
    /// (maps to SpawnMemberSpec::tool_access_policy).
    #[serde(default)]
    pub tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
    /// Budget split policy from the orchestrator to this member
    /// (maps to SpawnMemberSpec::budget_split_policy).
    #[serde(default)]
    pub budget_split_policy: Option<meerkat_mob::BudgetSplitPolicy>,
    /// Legacy name-only inherited tool filter. Runtime-backed spawn rejects
    /// this field because inherited visibility now requires witnesses from
    /// agent-owned spawn tooling.
    #[serde(default)]
    pub inherited_tool_filter: Option<meerkat_core::tool_scope::ToolFilter>,
    /// Public profile override for `mob/spawn`. The handler converts this
    /// wire-owned profile into the internal profile type with runtime-owned
    /// Rust tool bundles intentionally empty.
    #[serde(default)]
    pub override_profile: Option<WireMobProfile>,
    /// Explicit provider binding for this member's session build.
    ///
    /// The mob runtime refuses ambient credential selection; callers
    /// that spawn live model-backed members must name the realm binding
    /// that owns auth resolution.
    #[serde(default)]
    pub auth_binding: Option<meerkat_core::AuthBindingRef>,
}

pub async fn handle_spawn(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobSpawnParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    if let Err(err) = meerkat::surface::validate_public_surface_metadata(
        params.labels.as_ref(),
        params.context.as_ref(),
    ) {
        return invalid_params(id, err);
    }
    let mut spec = SpawnMemberSpec::new(params.profile.as_str(), params.agent_identity.as_str());
    spec.initial_message = params.initial_message;
    spec.runtime_mode = params.runtime_mode;
    spec.backend = params.backend;
    spec.context = params.context;
    spec.labels = params.labels;
    spec.additional_instructions = params.additional_instructions;
    if let Some(binding) = params.binding {
        match runtime_binding_from_wire(binding) {
            Ok(binding) => spec.binding = Some(binding),
            Err(err) => return invalid_params(id, err),
        }
    }
    if let Some(shell_env) = params.shell_env {
        spec.shell_env = Some(shell_env);
    }
    if let Some(auto_wire_parent) = params.auto_wire_parent {
        spec.auto_wire_parent = auto_wire_parent;
    }
    if let Some(launch_mode) = params.launch_mode {
        spec.launch_mode = launch_mode;
    }
    if let Some(tool_access_policy) = params.tool_access_policy {
        spec.tool_access_policy = Some(tool_access_policy);
    }
    if let Some(budget_split_policy) = params.budget_split_policy {
        spec.budget_split_policy = Some(budget_split_policy);
    }
    if params.inherited_tool_filter.is_some() {
        return invalid_params(
            id,
            "inherited_tool_filter is name-only and no longer accepted for runtime-backed mob spawn; use agent-owned spawn tooling so tool witnesses are captured",
        );
    }
    if let Some(override_profile) = params.override_profile {
        match profile_from_wire(override_profile) {
            Ok(profile) => spec.override_profile = Some(profile),
            Err(err) => {
                return invalid_params(id, format!("override_profile.output_schema: {err}"));
            }
        }
    }
    if let Some(auth_binding) = params.auth_binding {
        spec.auth_binding = Some(auth_binding);
    }
    match state.mob_spawn_spec(&mob_id, spec).await {
        Ok(spawn_result) => RpcResponse::success(id, spawn_result_payload(&mob_id, &spawn_result)),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/spawn_many
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobSpawnManyParams {
    pub mob_id: String,
    pub specs: Vec<MobSpawnSpecParams>,
}

/// Per-member spec within a `mob/spawn_many` batch.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobSpawnSpecParams {
    pub profile: String,
    pub agent_identity: String,
    #[serde(default)]
    pub initial_message: Option<ContentInput>,
    #[serde(default)]
    pub runtime_mode: Option<MobRuntimeMode>,
    #[serde(default)]
    pub backend: Option<MobBackendKind>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub context: Option<Value>,
    #[serde(default)]
    pub additional_instructions: Option<Vec<String>>,
    #[serde(default)]
    pub auth_binding: Option<meerkat_core::AuthBindingRef>,
}

/// Handle `mob/spawn_many` — batch spawn multiple members.
pub async fn handle_spawn_many(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobSpawnManyParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };

    let mut specs = Vec::with_capacity(params.specs.len());
    for s in &params.specs {
        if let Err(err) = meerkat::surface::validate_public_surface_metadata(
            s.labels.as_ref(),
            s.context.as_ref(),
        ) {
            return invalid_params(id, err);
        }
        let mut spec = SpawnMemberSpec::new(s.profile.as_str(), s.agent_identity.as_str());
        spec.initial_message = s.initial_message.clone();
        spec.runtime_mode = s.runtime_mode;
        spec.backend = s.backend;
        spec.context = s.context.clone();
        spec.labels = s.labels.clone();
        spec.additional_instructions = s.additional_instructions.clone();
        spec.auth_binding = s.auth_binding.clone();
        specs.push(spec);
    }

    match state.mob_spawn_many(&mob_id, specs).await {
        Ok(results) => {
            let entries: Vec<MobSpawnManyResultEntry> = results
                .into_iter()
                .map(|r| match r {
                    Ok(spawn_result) => {
                        let identity_str = spawn_result.agent_identity.to_string();
                        MobSpawnManyResultEntry::spawned(
                            identity_str.clone(),
                            WireMemberRef::encode(mob_id.as_str(), &identity_str),
                        )
                    }
                    Err(err) => MobSpawnManyResultEntry::failed(err.cause(), err.to_string()),
                })
                .collect();
            RpcResponse::success(id, MobSpawnManyResult { results: entries })
        }
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobMemberParams {
    pub mob_id: String,
    pub agent_identity: String,
}

pub async fn handle_retire(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobMemberParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state
        .mob_retire(&mob_id, AgentIdentity::from(params.agent_identity.as_str()))
        .await
    {
        Ok(()) => RpcResponse::success(id, MobRetireResult { retired: true }),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobRespawnParams {
    pub mob_id: String,
    pub agent_identity: String,
    #[serde(default)]
    pub initial_message: Option<ContentInput>,
}

pub async fn handle_respawn(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobRespawnParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state
        .mob_respawn(
            &mob_id,
            AgentIdentity::from(params.agent_identity.as_str()),
            params.initial_message,
        )
        .await
    {
        Ok(receipt) => respawn_result_response(id, &mob_id, Ok(receipt)),
        Err(err) => respawn_result_response(id, &mob_id, Err(err)),
    }
}

fn respawn_receipt_wire(mob_id: &MobId, receipt: &MemberRespawnReceipt) -> MobRespawnReceipt {
    let identity_str = receipt.identity.to_string();
    // App-facing respawn receipt exposes only identity-native fields and the
    // server-resolved `member_ref`. Binding-era fence tokens are retired per
    // dogma #10 — callers never reason about incarnation counters. The
    // typed `status` outcome on the parent response communicates success;
    // clients that need "before vs after" observability should track their
    // own state around the call.
    MobRespawnReceipt {
        identity: identity_str.clone(),
        member_ref: meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), &identity_str),
    }
}

fn respawn_result_response(
    id: Option<RpcId>,
    mob_id: &MobId,
    result: Result<MemberRespawnReceipt, MobRespawnError>,
) -> RpcResponse {
    match result {
        Ok(receipt) => RpcResponse::success(
            id,
            MobRespawnResult {
                status: WireMobRespawnOutcome::Completed,
                receipt: respawn_receipt_wire(mob_id, &receipt),
                failed_peer_ids: Vec::new(),
            },
        ),
        Err(MobRespawnError::TopologyRestoreFailed {
            receipt,
            failed_peer_ids,
        }) => RpcResponse::success(
            id,
            MobRespawnResult {
                status: WireMobRespawnOutcome::TopologyRestoreFailed,
                receipt: respawn_receipt_wire(mob_id, &receipt),
                failed_peer_ids: failed_peer_ids
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect(),
            },
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobWireParams {
    pub mob_id: String,
    pub member: String,
    pub peer: meerkat_mob::PeerTarget,
}

pub async fn handle_wire(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobWireParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state
        .mob_wire(
            &mob_id,
            AgentIdentity::from(params.member.as_str()),
            params.peer,
        )
        .await
    {
        Ok(()) => RpcResponse::success(id, MobWireResult { wired: true }),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

fn member_wire_edge_wire(edge: meerkat_mob::event::MemberWireEdge) -> MobWireMembersBatchEdge {
    MobWireMembersBatchEdge {
        a: edge.a.to_string(),
        b: edge.b.to_string(),
    }
}

pub async fn handle_wire_members_batch(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobWireMembersBatchParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let edges = params
        .edges
        .into_iter()
        .map(|edge| {
            (
                AgentIdentity::from(edge.a.as_str()),
                AgentIdentity::from(edge.b.as_str()),
            )
        })
        .collect::<Vec<_>>();
    let handle = match state.handle_for(&mob_id).await {
        Ok(handle) => handle,
        Err(err) => return invalid_params(id, err.to_string()),
    };
    match handle.wire_members_batch(edges).await {
        Ok(report) => RpcResponse::success(
            id,
            MobWireMembersBatchResult {
                requested: report.requested,
                wired: report
                    .wired
                    .into_iter()
                    .map(member_wire_edge_wire)
                    .collect(),
                already_wired: report
                    .already_wired
                    .into_iter()
                    .map(member_wire_edge_wire)
                    .collect(),
            },
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_unwire(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobWireParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state
        .mob_unwire(
            &mob_id,
            AgentIdentity::from(params.member.as_str()),
            params.peer,
        )
        .await
    {
        Ok(()) => RpcResponse::success(id, MobUnwireResult { unwired: true }),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub use meerkat_contracts::{MobLifecycleParams, WireMobLifecycleAction};

pub async fn handle_lifecycle(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobLifecycleParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    // `destroy` returns a structured `MobDestroyReport` only on complete
    // cleanup. Incomplete cleanup is a typed JSON-RPC error carrying that
    // report, which keeps `ok: true` reserved for fully completed destroy.
    let destroy_report = match params.action {
        WireMobLifecycleAction::Stop => match state.mob_stop(&mob_id).await {
            Ok(()) => None,
            Err(err) => return invalid_params(id, err.to_string()),
        },
        WireMobLifecycleAction::Resume => match state.mob_resume(&mob_id).await {
            Ok(()) => None,
            Err(err) => return invalid_params(id, err.to_string()),
        },
        WireMobLifecycleAction::Complete => match state.mob_complete(&mob_id).await {
            Ok(()) => None,
            Err(err) => return invalid_params(id, err.to_string()),
        },
        WireMobLifecycleAction::Reset => match state.mob_reset(&mob_id).await {
            Ok(()) => None,
            Err(err) => return invalid_params(id, err.to_string()),
        },
        WireMobLifecycleAction::Destroy => match state.mob_destroy(&mob_id).await {
            Ok(report) => Some(report),
            Err(MobMcpDestroyError::Incomplete { report }) => {
                return destroy_incomplete_response(id, &report);
            }
            Err(MobMcpDestroyError::Mob(err)) => return invalid_params(id, err.to_string()),
        },
    };
    let destroy_report = match destroy_report {
        Some(report) => match serde_json::to_value(&report) {
            Ok(value) => Some(value),
            Err(err) => {
                return invalid_params(id, format!("destroy report serialize: {err}"));
            }
        },
        None => None,
    };
    RpcResponse::success(
        id,
        MobLifecycleResult {
            mob_id: mob_id.to_string(),
            action: params.action,
            ok: true,
            destroy_report,
        },
    )
}

#[derive(Debug, Deserialize)]
pub struct MobAppendSystemContextParams {
    pub mob_id: String,
    pub agent_identity: String,
    pub text: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

pub type MobMemberSendParams = meerkat_contracts::MobMemberSendParams;
pub type MobMemberSendResult = meerkat_contracts::MobMemberSendResult;
pub type MobIngressInteractionParams = meerkat_contracts::MobIngressInteractionParams;
pub type MobIngressInteractionResult = meerkat_contracts::MobIngressInteractionResult;

pub async fn handle_member_send(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobMemberSendParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let agent_identity = AgentIdentity::from(params.agent_identity.as_str());
    let content = match ContentInput::try_from(params.content) {
        Ok(content) => content,
        Err(error) => return invalid_params(id, error),
    };
    match state
        .mob_member_send(
            &mob_id,
            agent_identity.clone(),
            content,
            params.handling_mode.into(),
            params.render_metadata.map(Into::into),
        )
        .await
    {
        Ok(receipt) => {
            let identity_str = receipt.identity.to_string();
            RpcResponse::success(
                id,
                MobMemberSendResult {
                    mob_id: mob_id.to_string(),
                    agent_identity: identity_str.clone(),
                    member_ref: meerkat_contracts::WireMemberRef::encode(
                        mob_id.as_str(),
                        &identity_str,
                    ),
                    handling_mode: receipt.handling_mode.into(),
                },
            )
        }
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_ingress_interaction(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobIngressInteractionParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let spec = match spawn_spec_from_wire(&params.spec) {
        Ok(spec) => spec,
        Err(err) => return invalid_params(id, err),
    };
    let identity = spec.identity.clone();
    let content = match ContentInput::try_from(params.content) {
        Ok(content) => content,
        Err(error) => return invalid_params(id, error),
    };
    let handle = match state.handle_for(&mob_id).await {
        Ok(handle) => handle,
        Err(err) => return invalid_params(id, err.to_string()),
    };
    let events_after_cursor = match handle.events().latest_cursor().await {
        Ok(cursor) => cursor,
        Err(err) => return invalid_params(id, err.to_string()),
    };
    let ensure_outcome = match handle.ensure_member(spec).await {
        Ok(meerkat_mob::runtime::EnsureMemberOutcome::Spawned(spawn)) => {
            meerkat_contracts::MobEnsureMemberOutcomeWire::Spawned(spawn_receipt_wire(
                &mob_id, &spawn,
            ))
        }
        Ok(meerkat_mob::runtime::EnsureMemberOutcome::Existed(entry)) => {
            match member_list_entry_wire(&mob_id, &entry) {
                Ok(wire) => meerkat_contracts::MobEnsureMemberOutcomeWire::Existed(wire),
                Err(projection_error) => {
                    return RpcResponse::error(id, error::INTERNAL_ERROR, projection_error);
                }
            }
        }
        Err(err) => return invalid_params(id, err.to_string()),
    };
    let member = match handle.member(&identity).await {
        Ok(member) => member,
        Err(err) => return invalid_params(id, err.to_string()),
    };
    let receipt = match member
        .send_with_render_metadata(
            content,
            params.handling_mode.into(),
            params.render_metadata.map(Into::into),
        )
        .await
    {
        Ok(receipt) => receipt,
        Err(err) => return invalid_params(id, err.to_string()),
    };
    let latest_event_cursor = match handle.events().latest_cursor().await {
        Ok(cursor) => cursor,
        Err(err) => return invalid_params(id, err.to_string()),
    };
    let identity_str = receipt.identity.to_string();
    let delivery = MobMemberSendResult {
        mob_id: mob_id.to_string(),
        agent_identity: identity_str.clone(),
        member_ref: meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), &identity_str),
        handling_mode: receipt.handling_mode.into(),
    };
    RpcResponse::success(
        id,
        MobIngressInteractionResult {
            mob_id: mob_id.to_string(),
            agent_identity: identity_str.clone(),
            member_ref: meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), &identity_str),
            ensure_outcome,
            delivery,
            events_after_cursor,
            latest_event_cursor,
        },
    )
}

pub async fn handle_append_system_context(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
    _runtime: &SessionRuntime,
) -> RpcResponse {
    let params: MobAppendSystemContextParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let agent_identity = AgentIdentity::from(params.agent_identity.as_str());
    match state
        .mob_append_system_context(
            &mob_id,
            &agent_identity,
            AppendSystemContextRequest {
                content: meerkat_core::lifecycle::run_primitive::CoreRenderable::text(params.text),
                source: params.source,
                idempotency_key: params.idempotency_key,
                source_kind: meerkat_core::session::SystemContextSource::Normal,
                peer_response_terminal: None,
            },
        )
        .await
    {
        Ok((_bridge_session_id, result)) => RpcResponse::success(
            id,
            MobAppendSystemContextResult {
                mob_id: mob_id.to_string(),
                agent_identity: agent_identity.to_string(),
                status: result.status.into(),
            },
        ),
        Err(err) => RpcResponse::error(id, error::INVALID_PARAMS, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobFlowsParams {
    pub mob_id: String,
}

#[derive(Debug, Deserialize)]
pub struct MobEventsParams {
    pub mob_id: String,
    #[serde(default)]
    pub after_cursor: u64,
    #[serde(default = "default_events_limit")]
    pub limit: usize,
    #[serde(default)]
    pub strict: bool,
}

const fn default_events_limit() -> usize {
    100
}

pub async fn handle_events(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobEventsParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = meerkat_mob::MobId::from(params.mob_id.as_str());
    let result = if params.strict {
        state
            .mob_events_strict(&mob_id, params.after_cursor, params.limit)
            .await
    } else {
        state
            .mob_events(&mob_id, params.after_cursor, params.limit)
            .await
    };
    match result {
        Ok(events) => {
            let events: Result<Vec<Value>, serde_json::Error> =
                events.iter().map(serde_json::to_value).collect();
            match events {
                Ok(events) => RpcResponse::success(id, MobEventsResult { events }),
                Err(err) => RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("failed to serialize mob events: {err}"),
                ),
            }
        }
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_flows(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobFlowsParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state.mob_list_flows(&mob_id).await {
        Ok(flows) => RpcResponse::success(
            id,
            MobFlowsResult {
                mob_id: mob_id.to_string(),
                flows,
            },
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobFlowRunParams {
    pub mob_id: String,
    pub flow_id: String,
    #[serde(default)]
    pub params: Value,
}

pub async fn handle_flow_run(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobFlowRunParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state
        .mob_run_flow(
            &mob_id,
            FlowId::from(params.flow_id.as_str()),
            params.params,
        )
        .await
    {
        Ok(run_id) => RpcResponse::success(
            id,
            MobFlowRunResult {
                run_id: run_id.to_string(),
            },
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_run(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobRunParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let activation_params = match bind_prompt_param(params.params, params.prompt) {
        Ok(params) => params,
        Err(message) => return invalid_params(id, message),
    };
    let flow_id = FlowId::from(params.flow_id.as_deref().unwrap_or("main"));
    match state
        .mob_run_flow(&mob_id, flow_id, activation_params)
        .await
    {
        Ok(run_id) => RpcResponse::success(
            id,
            MobFlowRunResult {
                run_id: run_id.to_string(),
            },
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

fn bind_prompt_param(params: Value, prompt: Option<String>) -> Result<Value, String> {
    let mut params = match params {
        Value::Null => serde_json::Map::new(),
        Value::Object(map) => map,
        _ => return Err("mob/run params must be an object".to_string()),
    };
    if let Some(prompt) = prompt
        && !params.contains_key("prompt")
    {
        params.insert("prompt".to_string(), Value::String(prompt));
    }
    Ok(Value::Object(params))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobFlowStatusParams {
    pub mob_id: String,
    pub run_id: String,
}

pub async fn handle_flow_status(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobFlowStatusParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let run_id = match RunId::from_str(&params.run_id) {
        Ok(run_id) => run_id,
        Err(err) => return invalid_params(id, format!("Invalid run_id: {err}")),
    };
    match state.mob_flow_status(&mob_id, run_id).await {
        Ok(run) => match meerkat_mob::MobRun::public_flow_status_run_value(run.as_ref()) {
            Ok(run) => RpcResponse::success(id, meerkat_contracts::MobFlowStatusResult { run }),
            Err(err) => invalid_params(id, err.to_string()),
        },
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_run_result(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobRunResultParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let run_id = match RunId::from_str(&params.run_id) {
        Ok(run_id) => run_id,
        Err(err) => return invalid_params(id, format!("Invalid run_id: {err}")),
    };
    match state.mob_flow_status(&mob_id, run_id).await {
        Ok(run) => match meerkat_mob::MobRun::public_run_result_value(run.as_ref()) {
            Ok(run) => RpcResponse::success(id, MobRunResult { run }),
            Err(err) => invalid_params(id, err.to_string()),
        },
        Err(err) => invalid_params(id, err.to_string()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobFlowCancelParams {
    pub mob_id: String,
    pub run_id: String,
}

pub async fn handle_flow_cancel(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobFlowCancelParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let run_id = match RunId::from_str(&params.run_id) {
        Ok(run_id) => run_id,
        Err(err) => return invalid_params(id, format!("Invalid run_id: {err}")),
    };
    match state.mob_cancel_flow(&mob_id, run_id).await {
        Ok(()) => RpcResponse::success(id, MobFlowCancelResult { canceled: true }),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/spawn_helper
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct MobSpawnHelperParams {
    pub mob_id: String,
    pub prompt: String,
    #[serde(default)]
    pub agent_identity: Option<String>,
    #[serde(default)]
    pub role_name: Option<String>,
    #[serde(default)]
    pub runtime_mode: Option<MobRuntimeMode>,
    #[serde(default)]
    pub backend: Option<MobBackendKind>,
}

pub async fn handle_spawn_helper(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobSpawnHelperParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    // #115: the surface must not mint mob-member identity. A missing
    // `agent_identity` fails closed with a typed INVALID_PARAMS rather than
    // fabricating a synthetic `helper-{uuid}` on the runtime identity path —
    // identity allocation is the mob substrate's responsibility.
    let Some(agent_identity_str) = params.agent_identity else {
        return invalid_params(
            id,
            "mob/spawn_helper requires agent_identity; the surface does not allocate member identity",
        );
    };
    let agent_identity = AgentIdentity::from(agent_identity_str);
    let mut options = meerkat_mob::HelperOptions::default();
    if let Some(role) = params.role_name {
        options.role_name = Some(meerkat_mob::ProfileName::from(role));
    }
    options.runtime_mode = params.runtime_mode;
    options.backend = params.backend;
    match state
        .mob_spawn_helper(&mob_id, agent_identity, params.prompt, options)
        .await
    {
        Ok(result) => {
            let identity_str = result.agent_identity.to_string();
            RpcResponse::success(
                id,
                MobHelperResult {
                    output: result.output,
                    tokens_used: result.tokens_used,
                    agent_identity: identity_str.clone(),
                    member_ref: meerkat_contracts::WireMemberRef::encode(
                        mob_id.as_str(),
                        &identity_str,
                    ),
                },
            )
        }
        Err(err) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/fork_helper
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct MobForkHelperParams {
    pub mob_id: String,
    pub source_member_id: String,
    pub prompt: String,
    #[serde(default)]
    pub agent_identity: Option<String>,
    #[serde(default)]
    pub role_name: Option<String>,
    #[serde(default)]
    pub fork_context: Option<meerkat_mob::ForkContext>,
    #[serde(default)]
    pub runtime_mode: Option<MobRuntimeMode>,
    #[serde(default)]
    pub backend: Option<MobBackendKind>,
}

pub async fn handle_fork_helper(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobForkHelperParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let source_identity = AgentIdentity::from(params.source_member_id.as_str());
    // #115: the surface must not mint mob-member identity. A missing
    // `agent_identity` fails closed with a typed INVALID_PARAMS rather than
    // fabricating a synthetic `fork-{uuid}` on the runtime identity path.
    let Some(agent_identity_str) = params.agent_identity else {
        return invalid_params(
            id,
            "mob/fork_helper requires agent_identity; the surface does not allocate member identity",
        );
    };
    let agent_identity = AgentIdentity::from(agent_identity_str);
    let fork_context = params
        .fork_context
        .unwrap_or(meerkat_mob::ForkContext::FullHistory);
    let mut options = meerkat_mob::HelperOptions::default();
    if let Some(role) = params.role_name {
        options.role_name = Some(meerkat_mob::ProfileName::from(role));
    }
    options.runtime_mode = params.runtime_mode;
    options.backend = params.backend;
    match state
        .mob_fork_helper(
            &mob_id,
            &source_identity,
            agent_identity,
            params.prompt,
            fork_context,
            options,
        )
        .await
    {
        Ok(result) => {
            let identity_str = result.agent_identity.to_string();
            RpcResponse::success(
                id,
                MobHelperResult {
                    output: result.output,
                    tokens_used: result.tokens_used,
                    agent_identity: identity_str.clone(),
                    member_ref: meerkat_contracts::WireMemberRef::encode(
                        mob_id.as_str(),
                        &identity_str,
                    ),
                },
            )
        }
        Err(err) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/force_cancel
// ---------------------------------------------------------------------------

pub async fn handle_force_cancel(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobMemberParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state
        .mob_force_cancel(&mob_id, AgentIdentity::from(params.agent_identity.as_str()))
        .await
    {
        Ok(()) => RpcResponse::success(id, MobForceCancelResult { cancelled: true }),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/destroy (Finding C3)
// ---------------------------------------------------------------------------

/// Handle `mob/destroy` — dedicated destroy endpoint that returns the
/// structured `MobDestroyReport` on complete cleanup and a typed incomplete
/// error with the report when cleanup remains retryable.
pub async fn handle_destroy(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobIdParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state.mob_destroy(&mob_id).await {
        Ok(report) => {
            let report_value = match serde_json::to_value(&report) {
                Ok(v) => v,
                Err(err) => {
                    return invalid_params(
                        id,
                        format!("failed to serialize MobDestroyReport: {err}"),
                    );
                }
            };
            RpcResponse::success(
                id,
                MobDestroyResult {
                    mob_id: mob_id.to_string(),
                    ok: true,
                    destroy_report: report_value,
                },
            )
        }
        Err(MobMcpDestroyError::Incomplete { report }) => destroy_incomplete_response(id, &report),
        Err(MobMcpDestroyError::Mob(err)) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/snapshot (Finding C2)
// ---------------------------------------------------------------------------

/// Handle `mob/snapshot` — return a point-in-time aggregate of mob status +
/// member list in a single atomic call. Replaces the "subscribe to events
/// and run your own projection" pattern that mobkit and other consumers had
/// to fall back to when they just wanted to render current state.
pub async fn handle_snapshot(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobIdParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let status = match state.mob_status(&mob_id).await {
        Ok(status) => meerkat_mob_mcp::wire_mob_lifecycle_status(status),
        Err(err) => return invalid_params(id, err.to_string()),
    };
    let members = match state.mob_list_members(&mob_id).await {
        Ok(members) => members,
        Err(err) => return invalid_params(id, err.to_string()),
    };
    let members: Result<Vec<MobMemberListEntryWire>, String> = members
        .iter()
        .map(|entry| member_list_entry_wire(&mob_id, entry))
        .collect();
    match members {
        Ok(members) => RpcResponse::success(
            id,
            MobSnapshotResult {
                mob_id: mob_id.to_string(),
                status,
                members,
            },
        ),
        Err(projection_error) => RpcResponse::error(id, error::INTERNAL_ERROR, projection_error),
    }
}

// ---------------------------------------------------------------------------
// mob/rotate_supervisor (Finding C10)
// ---------------------------------------------------------------------------

/// Handle `mob/rotate_supervisor` — rotate the supervisor bridge for all
/// members of a mob, surfacing the structured rotation report so operators
/// can inspect per-member outcomes instead of getting a bare ok:true.
pub async fn handle_rotate_supervisor(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobIdParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    match state.mob_rotate_supervisor(&mob_id).await {
        Ok(report) => RpcResponse::success(
            id,
            MobRotateSupervisorResult {
                mob_id: mob_id.to_string(),
                ok: true,
                report: SupervisorRotationReportWire {
                    previous_epoch: report.previous_epoch,
                    current_epoch: report.current_epoch,
                    public_peer_id: report.public_peer_id,
                },
            },
        ),
        Err(err) => mob_rotate_supervisor_error(id, &err),
    }
}

// ---------------------------------------------------------------------------
// mob/submit_work, mob/cancel_work, mob/cancel_all_work (Finding C4)
// ---------------------------------------------------------------------------
//
// Exposes the work lane (previously Rust-only) through JSON-RPC so mobkit
// and other non-Rust consumers can submit / cancel work.

pub use meerkat_contracts::{
    MobCancelAllWorkParams, MobCancelWorkParams, MobSubmitWorkParams, MobSubmitWorkResult,
    WireMemberRef, WireWorkOrigin,
};

/// Resolve the `(mob_id, agent_identity)` pair carried by a `WireMemberRef`
/// against the live mob roster, returning the current `AgentRuntimeId` and
/// fence token. Replaces binding-era `{generation, fence_token}` arguments
/// from app callers — clients pass the opaque token, the server looks up the
/// current incarnation.
async fn resolve_member_ref(
    member_ref: &WireMemberRef,
    state: &Arc<MobMcpState>,
) -> Result<
    (
        MobId,
        AgentIdentity,
        meerkat_mob::AgentRuntimeId,
        meerkat_mob::FenceToken,
    ),
    String,
> {
    let (mob_id_str, identity_str) = member_ref
        .decode()
        .map_err(|err| format!("invalid member_ref: {err}"))?;
    let mob_id = MobId::from(mob_id_str);
    let identity = AgentIdentity::from(identity_str);
    let entry = state
        .mob_list_members(&mob_id)
        .await
        .map_err(|err| err.to_string())?
        .into_iter()
        .find(|entry| entry.agent_identity == identity)
        .ok_or_else(|| format!("member {identity} not found in mob {mob_id}"))?;
    let (agent_runtime_id, fence_token) = entry
        .binding_atoms()
        .ok_or_else(|| format!("member {identity} has no MobMachine runtime binding"))?;
    Ok((mob_id, identity, agent_runtime_id, fence_token))
}

pub async fn handle_submit_work(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobSubmitWorkParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let (mob_id, _identity, runtime_id, fence_token) =
        match resolve_member_ref(&params.member_ref, state).await {
            Ok(resolved) => resolved,
            Err(err) => return invalid_params(id, err),
        };
    let work_ref = match params.work_ref {
        Some(ref s) if !s.is_empty() => match meerkat_mob::WorkRef::from_str(s) {
            Ok(wr) => wr,
            Err(err) => {
                return invalid_params(id, format!("work_ref must be a valid UUID: {err}"));
            }
        },
        _ => meerkat_mob::WorkRef::new(),
    };
    let origin = match params.origin {
        WireWorkOrigin::External => meerkat_mob::WorkOrigin::External,
        WireWorkOrigin::Internal => meerkat_mob::WorkOrigin::Internal,
    };
    let content = match ContentInput::try_from(params.content) {
        Ok(c) => c,
        Err(err) => return invalid_params(id, format!("invalid content: {err}")),
    };
    let spec = meerkat_mob::WorkSpec::new(content, origin);
    match state
        .mob_submit_work(&mob_id, runtime_id.clone(), fence_token, work_ref, spec)
        .await
    {
        Ok(receipt) => {
            let identity_str = receipt.runtime_id().identity.to_string();
            let body = MobSubmitWorkResult {
                mob_id: mob_id.to_string(),
                work_ref: receipt.work_ref.to_string(),
                member_ref: WireMemberRef::encode(mob_id.as_str(), &identity_str),
            };
            RpcResponse::success(id, body)
        }
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_cancel_work(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobCancelWorkParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let work_ref = match meerkat_mob::WorkRef::from_str(&params.work_ref) {
        Ok(wr) => wr,
        Err(err) => {
            return invalid_params(id, format!("work_ref must be a valid UUID: {err}"));
        }
    };
    match state.mob_cancel_work(&mob_id, work_ref).await {
        Ok(()) => RpcResponse::success(
            id,
            MobCancelWorkResult {
                mob_id: mob_id.to_string(),
                ok: true,
            },
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_cancel_all_work(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobCancelAllWorkParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let (mob_id, _identity, runtime_id, fence_token) =
        match resolve_member_ref(&params.member_ref, state).await {
            Ok(resolved) => resolved,
            Err(err) => return invalid_params(id, err),
        };
    match state
        .mob_cancel_all_work(&mob_id, runtime_id, fence_token)
        .await
    {
        Ok(()) => RpcResponse::success(
            id,
            MobCancelAllWorkResult {
                mob_id: mob_id.to_string(),
                ok: true,
            },
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/member_status
// ---------------------------------------------------------------------------

pub async fn handle_member_status(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobMemberParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let agent_identity = AgentIdentity::from(params.agent_identity.as_str());
    match state.mob_member_status(&mob_id, &agent_identity).await {
        Ok(snapshot) => {
            let member_ref =
                meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), agent_identity.as_str());
            match snapshot.to_member_status_result(member_ref) {
                Ok(result) => RpcResponse::success(id, result),
                Err(projection_error) => RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("failed to project mob member status: {projection_error}"),
                ),
            }
        }
        Err(err) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/wait_kickoff
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct MobWaitKickoffParams {
    pub mob_id: String,
    #[serde(default)]
    pub member_ids: Option<Vec<String>>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

pub async fn handle_wait_kickoff(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobWaitKickoffParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };

    let member_ids = params.member_ids.map(|ids| {
        ids.into_iter()
            .map(|member_id| AgentIdentity::from(member_id.as_str()))
            .collect::<Vec<_>>()
    });

    match state
        .mob_wait_kickoff(&mob_id, member_ids, params.timeout_ms)
        .await
    {
        Ok(members) => wait_members_response(id, &members),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

/// Serialize a typed kickoff/ready snapshot list into the contracts
/// `MobWaitMembersResult` wire payload, failing closed on serialization.
fn wait_members_response<T: serde::Serialize>(id: Option<RpcId>, members: &[T]) -> RpcResponse {
    let members: Result<Vec<Value>, serde_json::Error> =
        members.iter().map(serde_json::to_value).collect();
    match members {
        Ok(members) => RpcResponse::success(id, MobWaitMembersResult { members }),
        Err(err) => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("failed to serialize mob wait members result: {err}"),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct MobWaitReadyParams {
    pub mob_id: String,
    #[serde(default)]
    pub member_ids: Option<Vec<String>>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

pub async fn handle_wait_ready(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: MobWaitReadyParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };

    let member_ids = params.member_ids.map(|ids| {
        ids.into_iter()
            .map(|member_id| AgentIdentity::from(member_id.as_str()))
            .collect::<Vec<_>>()
    });

    match state
        .mob_wait_ready(&mob_id, member_ids, params.timeout_ms)
        .await
    {
        Ok(members) => wait_members_response(id, &members),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/profile/* — realm profile CRUD
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ProfileCreateParams {
    name: String,
    profile: meerkat_mob::Profile,
}

#[derive(Debug, Deserialize)]
struct ProfileNameParams {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ProfileUpdateParams {
    name: String,
    profile: meerkat_mob::Profile,
    expected_revision: u64,
}

#[derive(Debug, Deserialize)]
struct ProfileDeleteParams {
    name: String,
    expected_revision: u64,
}

pub async fn handle_profile_create(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: ProfileCreateParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    match state
        .realm_profile_create(&params.name, &params.profile)
        .await
    {
        Ok(stored) => RpcResponse::success(id, meerkat_mob::stored_realm_profile_to_wire(&stored)),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_profile_get(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: ProfileNameParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    match state.realm_profile_get(&params.name).await {
        Ok(Some(stored)) => {
            RpcResponse::success(id, meerkat_mob::stored_realm_profile_to_wire(&stored))
        }
        Ok(None) => RpcResponse::success(
            id,
            meerkat_contracts::MobProfileLookupResult {
                not_found: true,
                name: params.name,
                profile: None,
                revision: None,
                created_at: None,
                updated_at: None,
            },
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_profile_list(id: Option<RpcId>, state: &Arc<MobMcpState>) -> RpcResponse {
    match state.realm_profile_list().await {
        Ok(profiles) => {
            let profiles = profiles
                .iter()
                .map(meerkat_mob::stored_realm_profile_to_wire)
                .collect();
            RpcResponse::success(id, meerkat_contracts::MobProfileListResult { profiles })
        }
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_profile_update(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: ProfileUpdateParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    match state
        .realm_profile_update(&params.name, &params.profile, params.expected_revision)
        .await
    {
        Ok(stored) => RpcResponse::success(id, meerkat_mob::stored_realm_profile_to_wire(&stored)),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_profile_delete(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: ProfileDeleteParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    match state
        .realm_profile_delete(&params.name, params.expected_revision)
        .await
    {
        Ok(deleted) => RpcResponse::success(
            id,
            MobProfileDeleteResult {
                name: deleted.name,
                deleted_revision: deleted.revision,
            },
        ),
        Err(err) => invalid_params(id, err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// mob/turn_start — identity-native turn routing
// ---------------------------------------------------------------------------

/// Lower a wire turn-metadata override into the canonical domain tri-state.
///
/// `Set(v)` lowers to `TurnMetadataOverride::Set(v.into())` and `Clear` to
/// `TurnMetadataOverride::Clear`; the absent case stays `None` (Inherit). This
/// is the only conversion needed because the canonical contract
/// `meerkat_contracts::MobTurnStartParams` already enforces the
/// Inherit/Set/Clear shape at the serde boundary (folding the legacy split
/// `clear_*` form and rejecting `set + clear`), so the handler no longer
/// maintains a divergent shadow twin.
fn lower_turn_metadata_override<W, D>(
    wire: Option<meerkat_contracts::wire::runtime::WireTurnMetadataOverride<W>>,
) -> Option<TurnMetadataOverride<D>>
where
    D: From<W>,
{
    wire.map(|wire| match wire {
        meerkat_contracts::wire::runtime::WireTurnMetadataOverride::Set(value) => {
            TurnMetadataOverride::Set(value.into())
        }
        meerkat_contracts::wire::runtime::WireTurnMetadataOverride::Clear => {
            TurnMetadataOverride::Clear
        }
    })
}

/// Handle `mob/turn_start` — resolve identity to session and delegate to turn/start.
pub async fn handle_mob_turn_start(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
    runtime: Arc<SessionRuntime>,
    notification_sink: &crate::router::NotificationSink,
    runtime_adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    request_context: Option<RequestContext>,
) -> RpcResponse {
    let mob_params: meerkat_contracts::MobTurnStartParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &mob_params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let identity = AgentIdentity::from(mob_params.agent_identity.as_str());

    // Resolve identity → bridge session ID.
    let handle = match state.handle_for(&mob_id).await {
        Ok(h) => h,
        Err(err) => return invalid_params(id, err.to_string()),
    };
    let runtime_mode = handle
        .list_members()
        .await
        .into_iter()
        .find(|entry| entry.agent_identity == identity)
        .map(|entry| entry.runtime_mode);
    if matches!(runtime_mode, Some(MobRuntimeMode::AutonomousHost)) {
        return invalid_params(
            id,
            format!(
                "mob/turn_start is only valid for turn_driven members; \
                 autonomous member '{identity}' is driven by mob kickoff and mob/member_send"
            ),
        );
    }
    let session_id = match handle.resolve_bridge_session_id(&identity).await {
        Some(sid) => sid,
        None => {
            return invalid_params(
                id,
                format!("member '{identity}' has no active bridge session in mob '{mob_id}'"),
            );
        }
    };

    // Construct the typed `turn/start` params directly with the resolved
    // session_id and delegate to the shared typed entrypoint. The canonical
    // contract `MobTurnStartParams` already carries the Inherit/Set/Clear
    // tri-state; we lower each override into the domain `TurnMetadataOverride`
    // so any conversion stays typed rather than round-tripping through JSON.
    let prompt = match ContentInput::try_from(mob_params.prompt) {
        Ok(prompt) => prompt,
        Err(err) => return invalid_params(id, format!("invalid prompt: {err}")),
    };
    let turn_params = super::turn::StartTurnParams {
        session_id: session_id.to_string(),
        prompt,
        skill_refs: mob_params.skill_refs,
        // The canonical contract retired the legacy string `skill_references`
        // field; older clients that still send it are rejected at the contract
        // serde boundary (`deny_unknown_fields`). Nothing to thread here.
        skill_references: None,
        flow_tool_overlay: mob_params.flow_tool_overlay.map(TurnToolOverlay::from),
        additional_instructions: mob_params.additional_instructions,
        keep_alive: mob_params.keep_alive,
        model: mob_params.model,
        provider: mob_params.provider,
        max_tokens: mob_params.max_tokens,
        system_prompt: mob_params.system_prompt,
        output_schema: mob_params.output_schema,
        structured_output_retries: mob_params.structured_output_retries,
        provider_params: lower_turn_metadata_override(mob_params.provider_params),
        auth_binding: lower_turn_metadata_override(mob_params.auth_binding),
    };

    super::turn::start_turn_with_params(
        id,
        turn_params,
        runtime,
        notification_sink,
        runtime_adapter,
        request_context,
    )
    .await
}

// ---------------------------------------------------------------------------
// Declarative roster API: mob/ensure_member, mob/reconcile,
// mob/list_members_matching.
// ---------------------------------------------------------------------------

fn spawn_spec_from_wire(
    spec_wire: &meerkat_contracts::MobMemberSpecWire,
) -> Result<SpawnMemberSpec, String> {
    spec_wire
        .validate_public_surface_metadata()
        .map_err(|err| err.to_string())?;
    let mut spec = SpawnMemberSpec::new(
        spec_wire.profile.as_str(),
        spec_wire.agent_identity.as_str(),
    );
    spec.initial_message = match spec_wire.initial_message.clone() {
        Some(wire) => Some(
            ContentInput::try_from(wire)
                .map_err(|err| format!("invalid initial_message: {err}"))?,
        ),
        None => None,
    };
    spec.runtime_mode = spec_wire.runtime_mode.map(mob_runtime_mode_from_wire);
    spec.backend = spec_wire.backend.map(mob_backend_kind_from_wire);
    spec.context = spec_wire.context.clone();
    spec.labels = spec_wire.labels.clone();
    spec.additional_instructions = spec_wire.additional_instructions.clone();
    if let Some(binding) = spec_wire.binding.clone() {
        spec.binding = Some(runtime_binding_from_wire(binding)?);
    }
    if let Some(auto_wire_parent) = spec_wire.auto_wire_parent {
        spec.auto_wire_parent = auto_wire_parent;
    }
    Ok(spec)
}

fn spawn_receipt_wire(
    mob_id: &MobId,
    result: &meerkat_mob::SpawnResult,
) -> meerkat_contracts::MobSpawnReceiptWire {
    let identity_str = result.agent_identity.to_string();
    meerkat_contracts::MobSpawnReceiptWire {
        agent_identity: identity_str.clone(),
        member_ref: meerkat_contracts::WireMemberRef::encode(mob_id.as_str(), &identity_str),
    }
}

pub async fn handle_ensure_member(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: meerkat_contracts::MobEnsureMemberParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let handle = match state.handle_for(&mob_id).await {
        Ok(h) => h,
        Err(err) => return invalid_params(id, err.to_string()),
    };
    let spec = match spawn_spec_from_wire(&params.spec) {
        Ok(s) => s,
        Err(err) => return invalid_params(id, err),
    };
    match handle.ensure_member(spec).await {
        Ok(meerkat_mob::runtime::EnsureMemberOutcome::Spawned(spawn)) => {
            let outcome = meerkat_contracts::MobEnsureMemberOutcomeWire::Spawned(
                spawn_receipt_wire(&mob_id, &spawn),
            );
            RpcResponse::success(id, meerkat_contracts::MobEnsureMemberResult { outcome })
        }
        Ok(meerkat_mob::runtime::EnsureMemberOutcome::Existed(entry)) => {
            match member_list_entry_wire(&mob_id, &entry) {
                Ok(wire) => {
                    let outcome = meerkat_contracts::MobEnsureMemberOutcomeWire::Existed(wire);
                    RpcResponse::success(id, meerkat_contracts::MobEnsureMemberResult { outcome })
                }
                Err(projection_error) => {
                    RpcResponse::error(id, error::INTERNAL_ERROR, projection_error)
                }
            }
        }
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_reconcile(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: meerkat_contracts::MobReconcileParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let handle = match state.handle_for(&mob_id).await {
        Ok(h) => h,
        Err(err) => return invalid_params(id, err.to_string()),
    };
    let desired: Vec<SpawnMemberSpec> = match params
        .desired
        .iter()
        .map(spawn_spec_from_wire)
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(v) => v,
        Err(err) => return invalid_params(id, err),
    };
    let options = meerkat_mob::runtime::ReconcileOptions {
        retire_stale: params.options.retire_stale,
    };
    match handle.reconcile(desired, options).await {
        Ok(report) => {
            let wire_report = meerkat_contracts::MobReconcileReportWire {
                desired: report
                    .desired
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect(),
                retained: report
                    .retained
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect(),
                spawned: report
                    .spawned
                    .iter()
                    .map(|spawn| spawn_receipt_wire(&mob_id, spawn))
                    .collect(),
                retired: report
                    .retired
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect(),
                failures: report
                    .failures
                    .iter()
                    .map(|f| meerkat_contracts::MobReconcileFailureWire {
                        agent_identity: f.agent_identity.to_string(),
                        stage: match f.stage {
                            meerkat_mob::runtime::ReconcileStage::Spawn => {
                                meerkat_contracts::WireMobReconcileStage::Spawn
                            }
                            meerkat_mob::runtime::ReconcileStage::Retire => {
                                meerkat_contracts::WireMobReconcileStage::Retire
                            }
                        },
                        error: meerkat_contracts::WireMobError {
                            code: meerkat_mob::mob_error_wire_code(&f.error),
                            message: f.error.to_string(),
                        },
                    })
                    .collect(),
            };
            RpcResponse::success(
                id,
                meerkat_contracts::MobReconcileResult {
                    report: wire_report,
                },
            )
        }
        Err(err) => invalid_params(id, err.to_string()),
    }
}

pub async fn handle_list_members_matching(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    state: &Arc<MobMcpState>,
) -> RpcResponse {
    let params: meerkat_contracts::MobListMembersMatchingParams = match parse_params(params) {
        Ok(p) => p,
        Err(resp) => return resp.with_id(id),
    };
    let mob_id = match parse_mob_id(id.clone(), &params.mob_id) {
        Ok(m) => m,
        Err(resp) => return resp,
    };
    let handle = match state.handle_for(&mob_id).await {
        Ok(h) => h,
        Err(err) => return invalid_params(id, err.to_string()),
    };
    let filter = meerkat_mob::runtime::MemberFilter {
        labels: params.filter.labels,
        role: params
            .filter
            .role
            .map(|r| meerkat_mob::ProfileName::from(r.as_str())),
        status: params.filter.status.map(|s| match s {
            meerkat_contracts::WireMobMemberStatus::Active => MobMemberStatus::Active,
            meerkat_contracts::WireMobMemberStatus::Retiring => MobMemberStatus::Retiring,
            meerkat_contracts::WireMobMemberStatus::Broken => MobMemberStatus::Broken,
            meerkat_contracts::WireMobMemberStatus::Completed => MobMemberStatus::Completed,
            meerkat_contracts::WireMobMemberStatus::Unknown => MobMemberStatus::Unknown,
        }),
    };
    let entries = match handle.list_members_matching(filter).await {
        Ok(entries) => entries,
        Err(err) => return invalid_params(id, err.to_string()),
    };
    let members: Result<Vec<Value>, String> = entries
        .iter()
        .map(|entry| {
            let wire = member_list_entry_wire(&mob_id, entry)?;
            serde_json::to_value(wire)
                .map_err(|error| format!("failed to serialize mob member list entry: {error}"))
        })
        .collect();
    match members {
        Ok(members) => RpcResponse::success(
            id,
            meerkat_contracts::MobListMembersMatchingResult { members },
        ),
        Err(projection_error) => RpcResponse::error(id, error::INTERNAL_ERROR, projection_error),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_mob::store::InMemoryMobEventStore;
    use meerkat_mob::{
        AgentIdentity, AgentRuntimeId, FenceToken, MobBuilder, MobDefinition, MobStorage,
    };
    use std::sync::Arc;

    fn rpc_destroy_test_definition(mob_id: &MobId) -> MobDefinition {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(Box::new(Profile {
                model: "claude-sonnet-4-5".to_string(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
                skills: Vec::new(),
                tools: ToolConfig::default(),
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            })),
        );
        let mut definition = MobDefinition::explicit(mob_id.clone());
        definition.profiles = profiles;
        definition
    }

    async fn state_with_incomplete_destroy(mob_id: &MobId) -> Arc<MobMcpState> {
        let state = MobMcpState::new_in_memory();
        let events = Arc::new(InMemoryMobEventStore::new());
        events.fail_clear_until_allowed();
        let storage = MobStorage::with_events(events);
        let handle = MobBuilder::new(rpc_destroy_test_definition(mob_id), storage)
            .with_session_service(state.session_service())
            .allow_ephemeral_sessions(true)
            .create()
            .await
            .expect("create mob with failing clear store");
        state.mob_insert_handle(mob_id.clone(), handle).await;
        state
    }

    fn assert_destroy_incomplete_rpc_error(response: RpcResponse) {
        assert!(response.result.is_none());
        let error = response.error.expect("incomplete destroy should be error");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(
            error.message.contains("mob destroy incomplete"),
            "message should identify incomplete destroy: {}",
            error.message
        );
        let data = error.data.expect("typed incomplete destroy data");
        assert_eq!(
            data.get("code").and_then(serde_json::Value::as_str),
            Some("mob_destroy_incomplete")
        );
        assert_eq!(
            data.get("retryable").and_then(serde_json::Value::as_bool),
            Some(true)
        );
        let first_error = data
            .get("destroy_report")
            .and_then(|value| value.get("errors"))
            .and_then(serde_json::Value::as_array)
            .and_then(|errors| errors.first())
            .and_then(serde_json::Value::as_str)
            .expect("destroy report first error");
        assert!(
            first_error.contains("forced mob event store clear failure"),
            "unexpected destroy report error: {first_error}"
        );
    }

    #[test]
    fn destroy_incomplete_response_is_typed_error_with_report()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut report = meerkat_mob::MobDestroyReport::default();
        report.errors.push("worker-1: archive failed".to_string());

        let response = destroy_incomplete_response(Some(RpcId::Num(7)), &report);

        assert_destroy_incomplete_rpc_error_with_message(response, "worker-1: archive failed");
        Ok(())
    }

    /// #115: the surface never mints mob-member identity. A `mob/spawn_helper`
    /// request without `agent_identity` fails closed with INVALID_PARAMS
    /// instead of fabricating a synthetic `helper-{uuid}`.
    #[tokio::test]
    async fn spawn_helper_without_agent_identity_fails_closed() {
        let state = MobMcpState::new_in_memory();
        let mob_id = MobId::from("rpc-helper-identity-test");
        let params = serde_json::json!({
            "mob_id": mob_id.as_str(),
            "prompt": "do the thing",
        });
        let raw = serde_json::value::RawValue::from_string(params.to_string())
            .expect("serialize spawn_helper params");

        let response = handle_spawn_helper(Some(RpcId::Num(11)), Some(&raw), &state).await;

        assert!(response.result.is_none());
        let error = response.error.expect("missing identity must be an error");
        assert_eq!(error.code, crate::error::INVALID_PARAMS);
        assert!(
            error.message.contains("requires agent_identity"),
            "message should name the missing identity: {}",
            error.message
        );
    }

    /// #115: the fork path likewise fails closed rather than minting
    /// `fork-{uuid}`.
    #[tokio::test]
    async fn fork_helper_without_agent_identity_fails_closed() {
        let state = MobMcpState::new_in_memory();
        let mob_id = MobId::from("rpc-helper-identity-test");
        let params = serde_json::json!({
            "mob_id": mob_id.as_str(),
            "source_member_id": "worker",
            "prompt": "do the thing",
        });
        let raw = serde_json::value::RawValue::from_string(params.to_string())
            .expect("serialize fork_helper params");

        let response = handle_fork_helper(Some(RpcId::Num(12)), Some(&raw), &state).await;

        assert!(response.result.is_none());
        let error = response.error.expect("missing identity must be an error");
        assert_eq!(error.code, crate::error::INVALID_PARAMS);
        assert!(
            error.message.contains("requires agent_identity"),
            "message should name the missing identity: {}",
            error.message
        );
    }

    fn assert_destroy_incomplete_rpc_error_with_message(
        response: RpcResponse,
        expected_error: &str,
    ) {
        assert!(response.result.is_none());
        let error = response.error.expect("incomplete destroy should be error");
        assert_eq!(error.code, crate::error::INTERNAL_ERROR);
        assert!(
            error.message.contains("mob destroy incomplete"),
            "message should identify incomplete destroy: {}",
            error.message
        );
        let data = error.data.expect("typed incomplete destroy data");
        assert_eq!(
            data.get("code").and_then(serde_json::Value::as_str),
            Some("mob_destroy_incomplete")
        );
        assert_eq!(
            data.get("retryable").and_then(serde_json::Value::as_bool),
            Some(true)
        );
        let first_error = data
            .get("destroy_report")
            .and_then(|value| value.get("errors"))
            .and_then(serde_json::Value::as_array)
            .and_then(|errors| errors.first())
            .and_then(serde_json::Value::as_str)
            .expect("destroy report first error");
        assert!(
            first_error.contains(expected_error),
            "unexpected destroy report error: {first_error}"
        );
    }

    #[tokio::test]
    async fn mob_destroy_handler_surfaces_incomplete_destroy_as_typed_error() {
        let mob_id = MobId::from("rpc-partial-destroy");
        let state = state_with_incomplete_destroy(&mob_id).await;
        let params = serde_json::value::to_raw_value(&serde_json::json!({
            "mob_id": mob_id.to_string(),
        }))
        .expect("params");

        let response = handle_destroy(Some(RpcId::Num(8)), Some(params.as_ref()), &state).await;

        assert_destroy_incomplete_rpc_error(response);
    }

    #[tokio::test]
    async fn mob_lifecycle_destroy_handler_surfaces_incomplete_destroy_as_typed_error() {
        let mob_id = MobId::from("rpc-lifecycle-partial-destroy");
        let state = state_with_incomplete_destroy(&mob_id).await;
        let params = serde_json::value::to_raw_value(&serde_json::json!({
            "mob_id": mob_id.to_string(),
            "action": "destroy",
        }))
        .expect("params");

        let response = handle_lifecycle(Some(RpcId::Num(9)), Some(params.as_ref()), &state).await;

        assert_destroy_incomplete_rpc_error(response);
    }

    #[test]
    fn respawn_result_preserves_receipt_on_topology_restore_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let receipt = MemberRespawnReceipt::new(
            AgentIdentity::from("worker"),
            AgentRuntimeId::initial(AgentIdentity::from("worker")),
            FenceToken::new(3),
            FenceToken::new(4),
        );
        let mob_id = MobId::from("mob-1");
        let response = respawn_result_response(
            Some(RpcId::Num(42)),
            &mob_id,
            Err(MobRespawnError::TopologyRestoreFailed {
                receipt: receipt.clone(),
                failed_peer_ids: vec![
                    meerkat_mob::RespawnTopologyPeerId::from("peer-a"),
                    meerkat_mob::RespawnTopologyPeerId::from("peer-b"),
                ],
            }),
        );

        assert!(
            response.error.is_none(),
            "partial failure should stay in result envelope"
        );
        let Some(raw) = response.result.as_ref() else {
            panic!("result payload should exist");
        };
        let value: serde_json::Value = serde_json::from_str(raw.get())?;
        assert_eq!(value["status"], "topology_restore_failed");
        assert_eq!(value["receipt"]["identity"], receipt.identity.to_string());
        assert_eq!(
            value["failed_peer_ids"],
            serde_json::json!(["peer-a", "peer-b"])
        );
        Ok(())
    }

    #[test]
    fn rotate_supervisor_incomplete_uses_typed_retryable_error_envelope()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = mob_rotate_supervisor_error(
            Some(RpcId::Num(7)),
            &MobError::SupervisorRotationIncomplete {
                previous_epoch: 1,
                attempted_epoch: 2,
                attempted_public_peer_id: "peer-next".to_string(),
                rotated_peer_count: 1,
                rollback_succeeded: false,
                pending_authority_recorded: true,
                rollback_error: Some("rollback failed".to_string()),
                reason: "remote peer rejected rotation".to_string(),
            },
        );

        let error = response.error.expect("typed error response");
        assert_eq!(
            error.code,
            ErrorCode::SupervisorRotationIncomplete.jsonrpc_code()
        );
        let data = error.data.expect("typed error data");
        assert_eq!(data["code"], "SUPERVISOR_ROTATION_INCOMPLETE");
        assert_eq!(data["details"]["kind"], "supervisor_rotation_incomplete");
        assert_eq!(data["details"]["retry_authority"], "pending_rotation");
        assert_eq!(data["details"]["retry_scope"], "durable");
        Ok(())
    }

    #[test]
    fn mob_wire_params_accept_canonical_member_and_peer_fields()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = serde_json::json!({
            "mob_id": "mob-1",
            "member": "worker-a",
            "peer": { "local": "worker-b" }
        });
        let params: MobWireParams = serde_json::from_value(value)?;
        assert_eq!(
            AgentIdentity::from(params.member.as_str()),
            AgentIdentity::from("worker-a")
        );
        assert_eq!(
            params.peer,
            meerkat_mob::PeerTarget::Local(AgentIdentity::from("worker-b"))
        );
        Ok(())
    }

    #[test]
    fn mob_wire_members_batch_params_accept_only_local_edges()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = serde_json::json!({
            "mob_id": "mob-1",
            "edges": [
                { "a": "worker-a", "b": "worker-b" },
                { "a": "worker-c", "b": "worker-a" }
            ]
        });
        let params: MobWireMembersBatchParams = serde_json::from_value(value)?;
        assert_eq!(params.mob_id, "mob-1");
        assert_eq!(params.edges.len(), 2);
        assert_eq!(params.edges[0].a, "worker-a");
        assert_eq!(params.edges[0].b, "worker-b");

        let external = serde_json::json!({
            "mob_id": "mob-1",
            "edges": [
                { "a": "worker-a", "b": { "external": { "name": "peer" } } }
            ]
        });
        let err = serde_json::from_value::<MobWireMembersBatchParams>(external)
            .expect_err("batch wiring is local-member only");
        assert!(
            err.to_string().contains("invalid type"),
            "unexpected error: {err}"
        );
        Ok(())
    }

    #[test]
    fn mob_create_params_reject_reserved_owner_session_id() {
        let value = serde_json::json!({
            "definition": {
                "id": "mob-1",
                "owner_session_id": "session-123",
                "profiles": {
                    "worker": { "model": "claude-sonnet-4-6" }
                }
            }
        });
        let err = serde_json::from_value::<MobCreateParams>(value)
            .expect_err("reserved owner_session_id must be rejected");
        assert!(err.to_string().contains("unknown field `owner_session_id`"));
    }

    /// DELETE_ME A10 regression: `MobSpawnParams` now carries all 15
    /// `SpawnMemberSpec` public fields so RPC spawn reaches parity with
    /// Rust-in-process spawn. The first A10 partial covered binding /
    /// shell_env / auto_wire_parent; A3+C1 unblocked launch_mode; this
    /// final pass adds tool_access_policy, budget_split_policy,
    /// inherited_tool_filter, and override_profile. This test pins each
    /// new field to round-trip through serde while override_profile stays
    /// on the public wire-owned profile shape.
    #[test]
    fn mob_spawn_params_carry_full_member_spec_surface() {
        use meerkat_core::ops::ToolAccessPolicy;
        use meerkat_core::tool_scope::ToolFilter;

        let value = serde_json::json!({
            "mob_id": "m1",
            "profile": "worker",
            "agent_identity": "w1",
            "launch_mode": { "mode": "fresh" },
            "tool_access_policy": {
                "type": "allow_list",
                "value": ["grep", "read"]
            },
            "budget_split_policy": { "type": "remaining" },
            "inherited_tool_filter": { "Allow": ["grep", "read"] },
            "override_profile": {
                "model": "claude-sonnet-4-6",
                "skills": [],
                "tools": {},
                "peer_description": "",
                "external_addressable": false
            },
            "auth_binding": {
                "realm": "dev",
                "binding": "default_anthropic"
            },
        });
        let params: MobSpawnParams =
            serde_json::from_value(value).expect("spawn params with full surface deserialize");

        // launch_mode is Fresh (default shape via serde tag).
        assert!(matches!(
            params.launch_mode,
            Some(meerkat_mob::MemberLaunchMode::Fresh)
        ));

        // tool_access_policy is the AllowList variant with the two tools.
        match params.tool_access_policy {
            Some(ToolAccessPolicy::AllowList(ref tools)) => {
                assert!(tools.contains("grep"));
                assert!(tools.contains("read"));
                assert_eq!(tools.len(), 2);
            }
            other => panic!("expected AllowList, got {other:?}"),
        }

        // budget_split_policy is Remaining.
        assert!(matches!(
            params.budget_split_policy,
            Some(meerkat_mob::BudgetSplitPolicy::Remaining)
        ));

        // inherited_tool_filter round-trips through serde.
        assert!(matches!(
            params.inherited_tool_filter,
            Some(ToolFilter::Allow(_))
        ));
        if let Some(ToolFilter::Allow(ref allowlist)) = params.inherited_tool_filter {
            assert!(allowlist.contains("grep"));
            assert!(allowlist.contains("read"));
        }

        // override_profile model survives the round-trip.
        let override_profile = params
            .override_profile
            .as_ref()
            .expect("override_profile round-trips through serde");
        assert_eq!(override_profile.model, "claude-sonnet-4-6");
        let auth_binding = params
            .auth_binding
            .as_ref()
            .expect("auth_binding round-trips through serde");
        assert_eq!(auth_binding.realm.as_str(), "dev");
        assert_eq!(auth_binding.binding.as_str(), "default_anthropic");

        // And all older fields that aren't set stay None so the additive
        // wire extension doesn't break prior callers.
        let minimal = serde_json::json!({
            "mob_id": "m1",
            "profile": "worker",
            "agent_identity": "w1",
        });
        let minimal_params: MobSpawnParams = serde_json::from_value(minimal)
            .expect("spawn params without optional fields deserialize");
        assert!(minimal_params.launch_mode.is_none());
        assert!(minimal_params.tool_access_policy.is_none());
        assert!(minimal_params.budget_split_policy.is_none());
        assert!(minimal_params.inherited_tool_filter.is_none());
        assert!(minimal_params.override_profile.is_none());
        assert!(minimal_params.auth_binding.is_none());
    }

    /// Generated-Artifact Theater (Dogma Rule 9) drift gate. The handler-local
    /// `MobSpawnParams` and the canonical contracts wire `MobSpawnParams`
    /// (`meerkat_contracts::MobSpawnParams`, which feeds `params.json` + SDK
    /// codegen) are serde-equivalent by construction: identical field names and
    /// identical enum reprs (domain `MobRuntimeMode`/`BudgetSplitPolicy`/... and
    /// their `Wire*` twins share `#[serde]` tagging). Both carry
    /// `#[serde(deny_unknown_fields)]`, so a field added to one but not the
    /// other makes one of these two deserializations fail. This pins the two
    /// representations against silent fork drift (the catalog/SDK describe the
    /// wire shape; the handler must accept the same bytes).
    #[test]
    fn mob_spawn_params_stay_serde_equivalent_to_contracts_wire() {
        let value = serde_json::json!({
            "mob_id": "m1",
            "profile": "worker",
            "agent_identity": "w1",
            "runtime_mode": "turn_driven",
            "backend": "session",
            "labels": {"team": "blue"},
            "context": {"k": "v"},
            "additional_instructions": ["be brief"],
            "shell_env": {"FOO": "bar"},
            "auto_wire_parent": true,
            "launch_mode": { "mode": "fresh" },
            "tool_access_policy": { "type": "allow_list", "value": ["grep", "read"] },
            "budget_split_policy": { "type": "remaining" },
            "inherited_tool_filter": { "Allow": ["grep", "read"] },
            "override_profile": {
                "model": "claude-sonnet-4-6",
                "skills": [],
                "tools": {},
                "peer_description": "",
                "external_addressable": false
            },
            "auth_binding": { "realm": "dev", "binding": "default_anthropic" },
        });

        let handler: MobSpawnParams = serde_json::from_value(value.clone())
            .expect("handler-local MobSpawnParams must accept the full wire payload");
        let wire: meerkat_contracts::MobSpawnParams = serde_json::from_value(value)
            .expect("contracts wire MobSpawnParams must accept the identical payload");

        // Shared scalar fields agree across the two representations.
        assert_eq!(handler.mob_id, wire.mob_id);
        assert_eq!(handler.profile, wire.profile);
        assert_eq!(handler.agent_identity, wire.agent_identity);
        assert_eq!(handler.auto_wire_parent, wire.auto_wire_parent);
    }

    #[test]
    fn mob_run_prompt_sugar_binds_params_prompt_without_overwrite() {
        let bound = bind_prompt_param(
            serde_json::json!({
                "severity": "high"
            }),
            Some("triage this".to_string()),
        )
        .expect("object params should bind prompt");
        assert_eq!(bound["severity"], "high");
        assert_eq!(bound["prompt"], "triage this");

        let explicit = bind_prompt_param(
            serde_json::json!({
                "prompt": "caller explicit",
            }),
            Some("sugar should not overwrite".to_string()),
        )
        .expect("explicit prompt should survive");
        assert_eq!(explicit["prompt"], "caller explicit");

        let from_null =
            bind_prompt_param(Value::Null, Some("from null".to_string())).expect("null params");
        assert_eq!(from_null["prompt"], "from null");

        let err = bind_prompt_param(Value::String("bad".into()), None)
            .expect_err("non-object params should fail closed");
        assert!(err.contains("must be an object"));
    }

    #[test]
    fn mob_spawn_params_reject_internal_override_profile_tool_bundles() {
        let value = serde_json::json!({
            "mob_id": "m1",
            "profile": "worker",
            "agent_identity": "w1",
            "override_profile": {
                "model": "claude-sonnet-4-6",
                "tools": {
                    "rust_bundles": ["internal-only"]
                }
            }
        });
        let err = serde_json::from_value::<MobSpawnParams>(value)
            .expect_err("internal rust bundle fields must be rejected");
        assert!(
            err.to_string().contains("unknown field `rust_bundles`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn mob_spawn_many_params_reject_unknown_fields() {
        let top_level = serde_json::json!({
            "mob_id": "m1",
            "specs": [],
            "unexpected": true
        });
        let err = serde_json::from_value::<MobSpawnManyParams>(top_level)
            .expect_err("spawn_many top-level contract must fail closed");
        assert!(
            err.to_string().contains("unknown field `unexpected`"),
            "unexpected error: {err}"
        );

        let nested = serde_json::json!({
            "mob_id": "m1",
            "specs": [{
                "profile": "worker",
                "agent_identity": "w1",
                "launch_mode": { "mode": "fresh" }
            }]
        });
        let err = serde_json::from_value::<MobSpawnManyParams>(nested)
            .expect_err("spawn_many nested specs must fail closed");
        assert!(
            err.to_string().contains("unknown field `launch_mode`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn mob_turn_start_params_accept_only_known_turn_overrides() {
        let value = serde_json::json!({
            "mob_id": "m1",
            "agent_identity": "w1",
            "prompt": [{"type": "text", "text": "continue"}],
            "flow_tool_overlay": {
                "allowed_tools": ["read"],
                "blocked_tools": []
            },
            "additional_instructions": ["stay concise"],
            "keep_alive": true,
            "model": "gpt-test",
            "provider": "openai",
            "max_tokens": 128,
            "system_prompt": "system",
            "output_schema": { "type": "object" },
            "structured_output_retries": 2,
            "provider_params": { "temperature": 0.2 },
            "auth_binding": {
                "realm": "dev",
                "binding": "default_openai"
            }
        });
        let params: meerkat_contracts::MobTurnStartParams =
            serde_json::from_value(value).expect("known turn overrides deserialize");

        assert!(matches!(
            params.prompt,
            meerkat_contracts::WireContentInput::Blocks(_)
        ));
        assert_eq!(
            params
                .flow_tool_overlay
                .as_ref()
                .and_then(|overlay| overlay.allowed_tools.as_ref())
                .expect("allowed tools"),
            &vec!["read".to_string()]
        );
        assert_eq!(params.model.as_deref(), Some("gpt-test"));
        assert_eq!(params.max_tokens, Some(128));
        assert_eq!(params.structured_output_retries, Some(2));
        let Some(meerkat_contracts::wire::runtime::WireTurnMetadataOverride::Set(provider_params)) =
            params.provider_params.as_ref()
        else {
            panic!("provider_params should be a Set override");
        };
        assert_eq!(provider_params.temperature, Some(0.2));
        let Some(meerkat_contracts::wire::runtime::WireTurnMetadataOverride::Set(auth_binding)) =
            params.auth_binding.as_ref()
        else {
            panic!("auth_binding should be a Set override");
        };
        assert_eq!(auth_binding.binding.as_str(), "default_openai");

        let unknown = serde_json::json!({
            "mob_id": "m1",
            "agent_identity": "w1",
            "prompt": "continue",
            "unexpected_override": true
        });
        let err = serde_json::from_value::<meerkat_contracts::MobTurnStartParams>(unknown)
            .expect_err("unknown turn override should fail closed");
        assert!(err.to_string().contains("unknown field"));

        // The canonical contract retired the legacy string `skill_references`
        // field; older clients that still send it fail closed at the serde
        // boundary via `deny_unknown_fields`.
        let retired = serde_json::json!({
            "mob_id": "m1",
            "agent_identity": "w1",
            "prompt": "continue",
            "skill_references": ["legacy/ref"]
        });
        let err = serde_json::from_value::<meerkat_contracts::MobTurnStartParams>(retired)
            .expect_err("retired skill references should fail closed");
        assert!(
            err.to_string().contains("unknown field"),
            "unexpected error: {err}"
        );
    }

    /// `mob/turn_start` must hand `turn/start` typed [`StartTurnParams`] built
    /// directly from [`MobTurnStartParams`] + the resolved session_id — no
    /// `serde_json::Map` round-trip, no `to_value(..).unwrap_or(Null)` /
    /// `to_string().unwrap_or_default()` laundering. This exercises the same
    /// field move the handler performs and asserts the tri-state overrides and
    /// resolved session identity survive intact.
    #[test]
    fn mob_turn_start_builds_typed_start_turn_params_without_laundering() {
        let value = serde_json::json!({
            "mob_id": "m1",
            "agent_identity": "w1",
            "prompt": [{"type": "text", "text": "continue"}],
            "model": "gpt-test",
            "output_schema": { "type": "object" },
            "provider_params": { "temperature": 0.2 },
            "auth_binding": {
                "realm": "dev",
                "binding": "default_openai"
            }
        });
        let mob_params: meerkat_contracts::MobTurnStartParams =
            serde_json::from_value(value).expect("known turn overrides deserialize");

        // Mirror the handler's typed field-move with a resolved session_id,
        // lowering the canonical wire tri-state overrides into the domain
        // `TurnMetadataOverride` exactly as the handler does.
        let prompt = ContentInput::try_from(mob_params.prompt).expect("prompt lowers");
        let turn_params = super::super::turn::StartTurnParams {
            session_id: "resolved-session-123".to_string(),
            prompt,
            skill_refs: mob_params.skill_refs,
            skill_references: None,
            flow_tool_overlay: mob_params.flow_tool_overlay.map(TurnToolOverlay::from),
            additional_instructions: mob_params.additional_instructions,
            keep_alive: mob_params.keep_alive,
            model: mob_params.model,
            provider: mob_params.provider,
            max_tokens: mob_params.max_tokens,
            system_prompt: mob_params.system_prompt,
            output_schema: mob_params.output_schema,
            structured_output_retries: mob_params.structured_output_retries,
            provider_params: lower_turn_metadata_override(mob_params.provider_params),
            auth_binding: lower_turn_metadata_override(mob_params.auth_binding),
        };

        assert_eq!(turn_params.session_id, "resolved-session-123");
        assert!(matches!(turn_params.prompt, ContentInput::Blocks(_)));
        assert_eq!(turn_params.model.as_deref(), Some("gpt-test"));
        // output_schema carried verbatim, not collapsed to Null.
        assert_eq!(
            turn_params.output_schema,
            Some(serde_json::json!({ "type": "object" }))
        );
        // Tri-state Set override preserved (not laundered through a JSON Map).
        let Some(TurnMetadataOverride::Set(provider_params)) = turn_params.provider_params.as_ref()
        else {
            panic!("provider_params should be a Set override");
        };
        assert_eq!(provider_params.temperature, Some(0.2));
        let Some(TurnMetadataOverride::Set(auth_binding)) = turn_params.auth_binding.as_ref()
        else {
            panic!("auth_binding should be a Set override");
        };
        assert_eq!(auth_binding.binding.as_str(), "default_openai");
    }

    #[test]
    fn mob_spawn_surface_metadata_rejects_reserved_labels() {
        let value = serde_json::json!({
            "mob_id": "m1",
            "profile": "worker",
            "agent_identity": "w1",
            "labels": {
                "meerkat.runtime_id": "spoof"
            }
        });
        let params: MobSpawnParams = serde_json::from_value(value).unwrap();
        let result = meerkat::surface::validate_public_surface_metadata(
            params.labels.as_ref(),
            params.context.as_ref(),
        );

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("meerkat.runtime_id"));
    }

    #[test]
    fn mob_member_wire_surface_metadata_rejects_reserved_app_context() {
        let spec = meerkat_contracts::MobMemberSpecWire {
            profile: "worker".into(),
            agent_identity: "w1".into(),
            initial_message: None,
            runtime_mode: None,
            backend: None,
            binding: None,
            context: Some(serde_json::json!({"meerkat.runtime_id": "spoof"})),
            labels: None,
            additional_instructions: None,
            auto_wire_parent: None,
        };

        let result = spawn_spec_from_wire(&spec);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("meerkat.runtime_id"));
    }

    #[test]
    fn mob_create_params_reject_reserved_internal_lifecycle_flags() {
        let value = serde_json::json!({
            "definition": {
                "id": "mob-1",
                "is_implicit": true,
                "session_cleanup_policy": "destroy_on_owner_archive",
                "profiles": {
                    "worker": { "model": "claude-sonnet-4-6" }
                }
            }
        });
        let err = serde_json::from_value::<MobCreateParams>(value)
            .expect_err("reserved lifecycle fields must be rejected");
        let message = err.to_string();
        assert!(
            message.contains("unknown field `is_implicit`")
                || message.contains("unknown field `session_cleanup_policy`"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn mob_create_params_reject_internal_profile_tool_bundles() {
        let value = serde_json::json!({
            "definition": {
                "id": "mob-1",
                "profiles": {
                    "worker": {
                        "model": "claude-sonnet-4-6",
                        "tools": {
                            "rust_bundles": ["internal-only"]
                        }
                    }
                }
            }
        });
        let err = serde_json::from_value::<MobCreateParams>(value)
            .expect_err("internal rust bundle fields must be rejected");
        assert!(
            err.to_string().contains("did not match any variant")
                || err.to_string().contains("unknown field `rust_bundles`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn mob_wire_params_reject_compatibility_shapes() {
        let value = serde_json::json!({
            "mob_id": "mob-1",
            "local": "worker-a",
            "target": { "local": "worker-b" }
        });
        let err = serde_json::from_value::<MobWireParams>(value)
            .expect_err("compatibility shape must be rejected");
        assert!(err.to_string().contains("unknown field `local`"));
    }

    #[test]
    fn mob_wire_params_reject_legacy_a_b_shape() {
        let value = serde_json::json!({
            "mob_id": "mob-1",
            "a": "worker-a",
            "b": "worker-b"
        });
        let err = serde_json::from_value::<MobWireParams>(value)
            .expect_err("legacy a/b shape must be rejected");
        assert!(err.to_string().contains("unknown field `a`"));
    }

    #[test]
    fn mob_wait_kickoff_params_accept_optional_member_ids_and_timeout()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = serde_json::json!({
            "mob_id": "mob-1",
            "member_ids": ["a", "b"],
            "timeout_ms": 2500
        });
        let params: MobWaitKickoffParams = serde_json::from_value(value)?;
        assert_eq!(params.mob_id, "mob-1");
        assert_eq!(params.member_ids.unwrap_or_default(), vec!["a", "b"]);
        assert_eq!(params.timeout_ms, Some(2500));
        Ok(())
    }
}
