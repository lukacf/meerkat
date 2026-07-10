//! Shared surface infrastructure helpers.
//!
//! Cross-cutting helpers used by all protocol surfaces (RPC, REST, MCP Server).

mod embedded;
mod request_execution;
#[cfg(feature = "session-store")]
mod runtime_backed;
#[cfg(feature = "session-store")]
mod runtime_schedule_host;
mod schedule_host;
#[cfg(not(target_arch = "wasm32"))]
mod stdio_json;

pub use embedded::{
    build_embedded_service, build_embedded_service_from_builder, set_default_schedule_tools,
    set_default_workgraph_tools,
};
pub use meerkat_core::{
    BUILD_ONLY_RECOVERY_OVERRIDE_ERROR, RecoveredSessionBuild, SurfaceSessionRecoveryContext,
    SurfaceSessionRecoveryError, SurfaceSessionRecoveryOverrides, build_recovered_session,
    has_build_only_turn_overrides, has_materialization_overrides,
    session_allows_first_turn_build_overrides,
};
pub use request_execution::{
    CancelActionInstallOutcome, CancelOutcome, CompleteOutcome, PreparedSurfaceSession,
    PublishOutcome, RequestAdmissionError, RequestAsyncAction, RequestContext, RequestTerminal,
    RequestTerminalResolution, RequestTransitionError, SurfaceRequestExecution,
    SurfaceRequestExecutor, SurfaceRequestPhase, SurfaceRequestSemantics,
    SurfaceRequestTerminalPolicy, noop_request_action, prepare_surface_session, request_action,
};
#[cfg(all(feature = "session-store", feature = "comms"))]
pub use runtime_backed::configure_peer_ingress;
#[cfg(feature = "session-store")]
pub use runtime_backed::{
    PersistentRuntimeExecutor, RuntimeBackedInitialTurn, SurfaceRuntimeMaterializeError,
    build_runtime_backed_service, build_runtime_backed_service_with_capacities,
    default_persistent_executor, default_persistent_executor_with_workgraph_service,
    install_prepared_runtime_interrupt_handle, materialize_session,
    materialize_session_with_reserved_admission, run_runtime_backed_initial_turn_with_machine,
    split_runtime_backed_eager_create_request,
};
#[cfg(feature = "session-store")]
pub use runtime_schedule_host::{
    spawn_runtime_backed_schedule_host, spawn_runtime_backed_schedule_host_with_mobs,
};
pub use schedule_host::{
    AcceptedScheduledInput, AcceptedScheduledInputCompletion,
    MobMemberCurrentSessionScheduleResolver, NoopScheduleMobHost, ScheduleHostHandle,
    ScheduledPromptDispatch, SharedScheduleTargetAdapter, SurfaceScheduleMobHost,
    SurfaceScheduleSessionHost, async_completion_dispatch, build_dispatch_from_accepted,
    immediate_completed_dispatch, immediate_delivery_failure, mob_member_schedule_identity,
    parse_mob_member_schedule_identity, recover_mob_member_identity_from_session_target,
    schedule_attempt_idempotency_key, schedule_host_supported, spawn_schedule_host,
};
#[cfg(not(target_arch = "wasm32"))]
pub use stdio_json::{StdioJsonWriter, spawn_stdio_json_writer};

use meerkat_contracts::{
    CapabilitiesResponse, CapabilityEntry, ContractVersion, RuntimeHostCapabilities,
    RuntimeHostEndpointProjection, RuntimeHostFeatureFlags, RuntimeHostHealth,
    RuntimeHostHealthStatus, RuntimeHostIdScope, RuntimeHostInfo, RuntimeHostRealmProjection,
};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::ConfigStoreMetadata;
use meerkat_core::{AgentEvent, Config, PeerMeta};

use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::mpsc;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::mpsc;

#[cfg(feature = "skills")]
use meerkat_core::skills::{
    SkillDocument, SkillError, SkillFilter, SkillIntrospectionEntry, SkillKey, SkillRuntime,
};
#[cfg(feature = "mcp")]
use std::collections::HashMap;
#[cfg(feature = "mcp")]
use std::collections::VecDeque;
#[cfg(feature = "skills")]
use std::sync::Arc;
#[cfg(feature = "mcp")]
use std::sync::{Mutex, OnceLock};

#[derive(Debug, thiserror::Error)]
pub enum WorkGraphAttentionTurnOverlayError {
    #[error("failed to read session {session_id} for WorkGraph attention overlay: {source}")]
    SessionRead {
        session_id: meerkat_core::SessionId,
        source: meerkat_core::SessionError,
    },
    #[error("{0}")]
    WorkGraph(#[from] crate::WorkGraphError),
    #[error("conflicting turn tool overlay dispatch context for {key}")]
    ConflictingDispatchContext { key: String },
}

impl From<meerkat_core::service::TurnToolOverlayComposeError>
    for WorkGraphAttentionTurnOverlayError
{
    fn from(error: meerkat_core::service::TurnToolOverlayComposeError) -> Self {
        match error {
            meerkat_core::service::TurnToolOverlayComposeError::ConflictingDispatchContext {
                key,
            } => Self::ConflictingDispatchContext { key },
        }
    }
}

pub async fn inject_workgraph_attention_turn_overlay(
    session_service: &dyn meerkat_core::SessionService,
    workgraph_service: Option<&crate::WorkGraphService>,
    session_id: &meerkat_core::SessionId,
    request: &mut meerkat_core::StartTurnRequest,
) -> Result<(), WorkGraphAttentionTurnOverlayError> {
    let Some(workgraph_service) = workgraph_service else {
        return Ok(());
    };
    let session = session_service.read(session_id).await.map_err(|source| {
        WorkGraphAttentionTurnOverlayError::SessionRead {
            session_id: session_id.clone(),
            source,
        }
    })?;
    inject_workgraph_attention_turn_overlay_from_labels(
        workgraph_service,
        session_service,
        session_id,
        &session.state.labels,
        request,
    )
    .await
}

pub async fn inject_workgraph_attention_turn_overlay_from_labels(
    workgraph_service: &crate::WorkGraphService,
    session_service: &dyn meerkat_core::SessionService,
    session_id: &meerkat_core::SessionId,
    labels: &BTreeMap<String, String>,
    request: &mut meerkat_core::StartTurnRequest,
) -> Result<(), WorkGraphAttentionTurnOverlayError> {
    let existing_overlay = request
        .runtime
        .turn_metadata
        .as_ref()
        .and_then(|metadata| metadata.turn_tool_overlay.clone())
        .or_else(|| request.runtime.turn_tool_overlay.clone());
    let Some(overlay) = compose_workgraph_attention_turn_overlay_from_labels(
        workgraph_service,
        Some(session_service),
        session_id,
        labels,
        existing_overlay,
    )
    .await?
    else {
        return Ok(());
    };
    if let Some(metadata) = request.runtime.turn_metadata.as_mut() {
        metadata.turn_tool_overlay = Some(overlay.clone());
    }
    request.runtime.turn_tool_overlay = Some(overlay);
    Ok(())
}

async fn compose_workgraph_attention_turn_overlay_from_labels(
    workgraph_service: &crate::WorkGraphService,
    session_service: Option<&dyn meerkat_core::SessionService>,
    session_id: &meerkat_core::SessionId,
    labels: &BTreeMap<String, String>,
    existing_overlay: Option<meerkat_core::service::TurnToolOverlay>,
) -> Result<Option<meerkat_core::service::TurnToolOverlay>, WorkGraphAttentionTurnOverlayError> {
    if workgraph_service.store().kind() == crate::WorkGraphStoreKind::Disabled {
        return Ok(existing_overlay);
    }
    let Some(projection) = resolve_workgraph_attention_projection_for_session(
        workgraph_service,
        session_service,
        session_id,
        labels,
    )
    .await?
    else {
        return Ok(existing_overlay);
    };
    let attention_overlay =
        crate::WorkGraphToolSurface::turn_overlay_for_attention_projection(&projection)?;
    let overlay = compose_turn_tool_overlay(existing_overlay, attention_overlay)?;
    Ok(Some(overlay))
}

async fn resolve_workgraph_attention_projection_for_session(
    workgraph_service: &crate::WorkGraphService,
    session_service: Option<&dyn meerkat_core::SessionService>,
    session_id: &meerkat_core::SessionId,
    labels: &BTreeMap<String, String>,
) -> Result<Option<crate::AttentionContextProjection>, WorkGraphAttentionTurnOverlayError> {
    let active = workgraph_service
        .list_attention(crate::AttentionListRequest {
            realm_id: None,
            namespace: None,
            target: None,
            status: Some(crate::WorkAttentionStatus::Active),
        })
        .await?;
    let mut matching = active
        .attention
        .into_iter()
        .filter(|binding| attention_target_matches_session(&binding.target, session_id, labels))
        .collect::<Vec<_>>();
    // Multiple active bindings matching one session is a store-invariant
    // violation (the active-binding-per-target occupancy guard rejects new
    // duplicates transactionally), reachable only through legacy rows or
    // mixed-version writers. It must not brick the member's every turn:
    // arbitrate deterministically — newest binding wins, binding id as the
    // tie-break — with a loud diagnostic, mirroring newest-session-wins.
    if matching.len() > 1 {
        matching.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.binding_id.as_str().cmp(left.binding_id.as_str()))
        });
        tracing::error!(
            session_id = %session_id,
            bindings = ?matching
                .iter()
                .map(|binding| binding.binding_id.as_str().to_string())
                .collect::<Vec<_>>(),
            winner = %matching[0].binding_id,
            "multiple active WorkGraph attention bindings resolve to one session; \
             arbitrating newest-binding-wins — prune or reassign the losers"
        );
        matching.truncate(1);
    }
    let Some(binding) = matching.first() else {
        return Ok(None);
    };
    if !attention_binding_resolves_to_session(binding, session_service, session_id, labels).await? {
        return Ok(None);
    }
    workgraph_service
        .attention_projection(crate::AttentionProjectionRequest {
            binding_id: binding.binding_id.clone(),
            realm_id: Some(binding.work_ref.realm_id.clone()),
            namespace: Some(binding.work_ref.namespace.clone()),
        })
        .await
        .map(|result| Some(result.projection))
        .map_err(Into::into)
}

async fn attention_binding_resolves_to_session(
    binding: &crate::WorkAttentionBinding,
    session_service: Option<&dyn meerkat_core::SessionService>,
    session_id: &meerkat_core::SessionId,
    labels: &BTreeMap<String, String>,
) -> Result<bool, WorkGraphAttentionTurnOverlayError> {
    let crate::WorkAttentionTarget::LoweredOwner { owner_key } = &binding.target else {
        return Ok(true);
    };
    let Some(session_service) = session_service else {
        return Ok(true);
    };
    let Some((mob_id, agent_identity)) = mob_agent_owner_key_parts(&owner_key.id) else {
        return Ok(true);
    };
    let filter = BTreeMap::from([
        ("mob_id".to_string(), mob_id.to_string()),
        ("agent_identity".to_string(), agent_identity.to_string()),
    ]);
    if labels.get("mob_id").is_none_or(|value| value != mob_id)
        || labels
            .get("agent_identity")
            .is_none_or(|value| value != agent_identity)
    {
        return Ok(false);
    }
    let mut summaries = session_service
        .list(meerkat_core::service::SessionQuery {
            limit: None,
            offset: None,
            labels: Some(filter),
        })
        .await
        .map_err(|source| WorkGraphAttentionTurnOverlayError::SessionRead {
            session_id: session_id.clone(),
            source,
        })?;
    summaries.sort_by(|left, right| {
        right.created_at.cmp(&left.created_at).then_with(|| {
            right
                .session_id
                .to_string()
                .cmp(&left.session_id.to_string())
        })
    });
    // A session not yet visible in the listing is mid-creation: it is
    // definitionally the newest session carrying these labels, so the
    // attention overlay may bind to it. Otherwise the newest listed live
    // session wins, so overlapping respawn generations resolve
    // deterministically instead of both receiving the attention overlay.
    if !summaries
        .iter()
        .any(|summary| summary.session_id == *session_id)
    {
        return Ok(true);
    }
    Ok(summaries
        .first()
        .is_none_or(|summary| summary.session_id == *session_id))
}

fn attention_target_matches_session(
    target: &crate::WorkAttentionTarget,
    session_id: &meerkat_core::SessionId,
    labels: &BTreeMap<String, String>,
) -> bool {
    match target {
        crate::WorkAttentionTarget::Session {
            session_id: target_session_id,
        } => target_session_id == session_id,
        crate::WorkAttentionTarget::LoweredOwner { owner_key } => {
            if owner_key.kind == crate::WorkOwnerKind::Session
                && owner_key.id == session_id.to_string()
            {
                return true;
            }
            if owner_key.kind != crate::WorkOwnerKind::Agent {
                return false;
            }
            let Some((mob_id, agent_identity)) = mob_agent_owner_key_parts(&owner_key.id) else {
                return false;
            };
            labels.get("mob_id").is_some_and(|label| label == mob_id)
                && labels
                    .get("agent_identity")
                    .is_some_and(|label| label == agent_identity)
        }
    }
}

fn mob_agent_owner_key_parts(owner_id: &str) -> Option<(&str, &str)> {
    let rest = owner_id.strip_prefix("mob/")?;
    let (mob_id, agent_identity) = rest.split_once("/agent/")?;
    if mob_id.is_empty()
        || agent_identity.is_empty()
        || mob_id.contains('/')
        || agent_identity.contains('/')
    {
        return None;
    }
    Some((mob_id, agent_identity))
}

fn compose_turn_tool_overlay(
    existing: Option<meerkat_core::service::TurnToolOverlay>,
    attention: meerkat_core::service::TurnToolOverlay,
) -> Result<meerkat_core::service::TurnToolOverlay, WorkGraphAttentionTurnOverlayError> {
    // Single canonical composition path: core owns overlay composition
    // semantics (`TurnToolOverlay::compose`); this surface only maps the typed
    // conflict into its own error vocabulary.
    Ok(meerkat_core::service::TurnToolOverlay::compose(
        existing, attention,
    )?)
}

/// Surface-specific facts used to build a read-only runtime host projection.
#[derive(Debug, Clone)]
pub struct RuntimeHostSurfaceOptions {
    pub process_name: String,
    pub process_version: String,
    pub runtime_backed_sessions: bool,
    pub mobs: bool,
    pub mcp_live: bool,
    pub comms: bool,
    pub blobs: bool,
    pub artifacts: bool,
    pub session_events: bool,
    pub event_replay: bool,
    pub session_streams: bool,
    pub schedules: bool,
    pub skills: bool,
    pub approvals: bool,
    pub rpc_transport: Option<String>,
    pub rest_base_url: Option<String>,
    pub rpc_methods: Vec<String>,
    pub rest_paths: Vec<String>,
}

/// Config-store facts needed for runtime host projection.
///
/// This keeps the public surface helper independent of native-only config-store
/// types so browser/WASM builds can still construct host projections.
#[derive(Debug, Clone, Default)]
pub struct RuntimeHostMetadataProjection {
    pub realm_id: Option<String>,
    pub instance_id: Option<String>,
    pub backend: Option<String>,
    pub state_root: Option<String>,
}

#[cfg(not(target_arch = "wasm32"))]
impl From<&ConfigStoreMetadata> for RuntimeHostMetadataProjection {
    fn from(metadata: &ConfigStoreMetadata) -> Self {
        Self {
            realm_id: metadata.realm_id.clone(),
            instance_id: metadata.instance_id.clone(),
            backend: metadata.backend.clone(),
            state_root: metadata
                .resolved_paths
                .as_ref()
                .map(|paths| paths.root.clone()),
        }
    }
}

impl RuntimeHostSurfaceOptions {
    /// Build a process-local default for callers that do not own a richer
    /// transport context.
    pub fn process(process_name: impl Into<String>, process_version: impl Into<String>) -> Self {
        Self {
            process_name: process_name.into(),
            process_version: process_version.into(),
            runtime_backed_sessions: false,
            mobs: false,
            mcp_live: false,
            comms: false,
            blobs: false,
            artifacts: false,
            session_events: false,
            event_replay: false,
            session_streams: false,
            schedules: false,
            skills: false,
            approvals: false,
            rpc_transport: None,
            rest_base_url: None,
            rpc_methods: Vec::new(),
            rest_paths: Vec::new(),
        }
    }
}

fn runtime_host_id(
    metadata: Option<&RuntimeHostMetadataProjection>,
) -> (String, RuntimeHostIdScope) {
    if let Some(metadata) = metadata
        && let (Some(realm_id), Some(instance_id)) =
            (metadata.realm_id.as_ref(), metadata.instance_id.as_ref())
    {
        return (
            format!("realm-instance:{realm_id}:{instance_id}"),
            RuntimeHostIdScope::RealmInstance,
        );
    }

    (
        format!("process:{}", runtime_host_process_id()),
        RuntimeHostIdScope::Process,
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn runtime_host_process_id() -> u32 {
    std::process::id()
}

#[cfg(target_arch = "wasm32")]
fn runtime_host_process_id() -> u32 {
    0
}

fn runtime_host_realm_projection(
    metadata: Option<&RuntimeHostMetadataProjection>,
    context_root: Option<std::path::PathBuf>,
) -> RuntimeHostRealmProjection {
    let mut projection = RuntimeHostRealmProjection::default();
    if let Some(metadata) = metadata {
        projection.realm_id = metadata.realm_id.clone();
        projection.instance_id = metadata.instance_id.clone();
        projection.backend = metadata.backend.clone();
        projection.state_root = metadata.state_root.clone();
    }
    projection.context_root = context_root.map(|path| path.display().to_string());
    projection
}

/// Build read-only capability flags for the runtime host surface.
pub fn build_runtime_host_capabilities(
    options: &RuntimeHostSurfaceOptions,
) -> RuntimeHostCapabilities {
    RuntimeHostCapabilities {
        contract_version: ContractVersion::CURRENT,
        features: RuntimeHostFeatureFlags {
            runtime_backed_sessions: options.runtime_backed_sessions,
            mobs: options.mobs,
            mcp_live: options.mcp_live,
            comms: options.comms,
            blobs: options.blobs,
            session_events: options.session_events,
            session_streams: options.session_streams,
            schedules: options.schedules,
            skills: options.skills,
            event_replay: options.event_replay,
            artifacts: options.artifacts,
            // First slice: these future surfaces are deliberately reported as
            // unsupported until their owning lanes add durable contracts.
            approvals: options.approvals,
            external_members: false,
            secure_remote_rpc: false,
        },
    }
}

/// Build a read-only runtime health projection.
pub fn build_runtime_host_health() -> RuntimeHostHealth {
    RuntimeHostHealth {
        contract_version: ContractVersion::CURRENT,
        status: RuntimeHostHealthStatus::Ok,
        checks: BTreeMap::new(),
    }
}

/// Build host information from existing surface/runtime facts.
///
/// This projection intentionally does not expose topology, lease, project, or
/// host-registry authority. Callers can rebuild it from the config-store
/// metadata and surface options.
pub fn build_runtime_host_info(
    options: &RuntimeHostSurfaceOptions,
    metadata: Option<&RuntimeHostMetadataProjection>,
    context_root: Option<std::path::PathBuf>,
) -> RuntimeHostInfo {
    let (host_id, host_id_scope) = runtime_host_id(metadata);
    RuntimeHostInfo {
        contract_version: ContractVersion::CURRENT,
        host_id,
        host_id_scope,
        process_name: options.process_name.clone(),
        process_version: options.process_version.clone(),
        capabilities: build_runtime_host_capabilities(options),
        health: build_runtime_host_health(),
        realm: runtime_host_realm_projection(metadata, context_root),
        endpoints: RuntimeHostEndpointProjection {
            rpc_transport: options.rpc_transport.clone(),
            rest_base_url: options.rest_base_url.clone(),
            rpc_methods: options.rpc_methods.clone(),
            rest_paths: options.rest_paths.clone(),
        },
        placement_labels: BTreeMap::new(),
        policy_profile_summary: None,
    }
}

/// Build a [`CapabilitiesResponse`] with status resolved against config.
///
/// For each registered capability, calls its `status_resolver` (if provided)
/// to determine runtime status. Capabilities without a resolver are reported
/// as `Available`. This keeps policy knowledge in the owning crate.
pub fn build_capabilities_response(config: &Config) -> CapabilitiesResponse {
    let capabilities = meerkat_contracts::resolve_capabilities(config)
        .into_iter()
        .map(|(reg, status)| CapabilityEntry {
            id: reg.id,
            description: reg.description.to_string(),
            status,
        })
        .collect();

    CapabilitiesResponse {
        contract_version: ContractVersion::CURRENT,
        capabilities,
    }
}

/// Build a [`ModelsCatalogResponse`] from the resolved model registry.
///
/// This is **not** a pure compiled-in snapshot: it reads the config-backed
/// `ModelRegistry`, which combines the compiled-in
/// `meerkat_core::model_profile` catalog with any config-declared self-hosted
/// aliases, so two responses for the same binary can differ by config.
pub fn build_models_catalog_response(
    config: &meerkat_core::Config,
) -> Result<meerkat_contracts::ModelsCatalogResponse, meerkat_core::ConfigError> {
    let registry = config.model_registry(meerkat_models::canonical())?;
    let providers = registry
        .provider_defaults()
        .map(|(provider, default_model_id)| {
            let models = registry
                .entries_for_provider(provider)
                .map(|entry| {
                    let model_profile = registry
                        .profile_for_provider(provider, &entry.id)
                        .ok_or_else(|| {
                            meerkat_core::ConfigError::InternalError(format!(
                                "missing provider-aware profile for {}:{}",
                                provider.as_str(),
                                entry.id
                            ))
                        })?;
                    let profile = Some(meerkat_contracts::WireModelProfile {
                        model_family: model_profile.model_family.clone(),
                        supports_temperature: model_profile.supports_temperature,
                        supports_thinking: model_profile.supports_thinking,
                        supports_reasoning: model_profile.supports_reasoning,
                        supports_web_search: model_profile.supports_web_search,
                        vision: model_profile.vision,
                        image_input: model_profile.image_input,
                        image_tool_results: model_profile.image_tool_results,
                        inline_video: model_profile.inline_video,
                        realtime: model_profile.realtime,
                        image_generation: model_profile.image_generation,
                        params_schema: model_profile.params_schema.clone(),
                        beta_headers: model_profile
                            .beta_headers
                            .iter()
                            .map(|header| meerkat_contracts::WireModelBetaHeader {
                                feature: header.feature.clone(),
                                header_name: header.header_name.clone(),
                                header_value: header.header_value.clone(),
                            })
                            .collect(),
                    });
                    Ok(meerkat_contracts::CatalogModelEntry {
                        id: entry.id.clone(),
                        display_name: entry.display_name.clone(),
                        tier: match entry.tier {
                            meerkat_core::model_profile::catalog::ModelTier::Recommended => {
                                meerkat_contracts::WireModelTier::Recommended
                            }
                            meerkat_core::model_profile::catalog::ModelTier::Supported => {
                                meerkat_contracts::WireModelTier::Supported
                            }
                        },
                        context_window: entry.context_window,
                        max_output_tokens: entry.max_output_tokens,
                        server_id: entry
                            .self_hosted
                            .as_ref()
                            .map(|server| server.server_id.clone()),
                        profile,
                    })
                })
                .collect::<Result<Vec<_>, meerkat_core::ConfigError>>()?;
            Ok(meerkat_contracts::ProviderCatalog {
                provider: provider.as_str().to_string(),
                default_model_id: default_model_id.to_string(),
                models,
            })
        })
        .collect::<Result<Vec<_>, meerkat_core::ConfigError>>()?;

    Ok(meerkat_contracts::ModelsCatalogResponse {
        contract_version: ContractVersion::CURRENT,
        providers,
    })
}

/// Validate whether keep-alive mode can be enabled in the current build.
///
/// Delegates to `meerkat_comms::validate_keep_alive()` when the comms feature
/// is compiled in; returns an error if requested but comms is not available.
///
/// This is the canonical entry point — all surfaces should call this.
pub fn resolve_keep_alive(requested: bool) -> Result<bool, String> {
    #[cfg(feature = "comms")]
    {
        meerkat_comms::validate_keep_alive(requested)
    }
    #[cfg(not(feature = "comms"))]
    {
        if requested {
            return Err(
                "keep_alive requires comms support (build with --features comms)".to_string(),
            );
        }
        Ok(false)
    }
}

/// Reject public `PeerMeta` labels that are reserved for mob-managed sessions.
///
/// Mob runtime code stamps these labels internally when provisioning members.
/// Public protocol surfaces must reject caller-supplied values so ordinary
/// sessions cannot spoof durable mob ownership markers.
pub fn validate_public_peer_meta(peer_meta: Option<&PeerMeta>) -> Result<(), String> {
    let Some(peer_meta) = peer_meta else {
        return Ok(());
    };

    validate_raw_labels(Some(&peer_meta.labels))
}

/// Reject raw labels that are reserved for mob-managed sessions.
///
/// Similar to \[`validate_public_peer_meta`\], but works on a raw labels map.
/// Public surfaces should call this for any caller-supplied labels, including
/// top-level `CreateSessionRequest::labels`.
pub fn validate_raw_labels(
    labels: Option<&std::collections::BTreeMap<String, String>>,
) -> Result<(), String> {
    meerkat_core::validate_public_labels(labels).map_err(|err| err.to_string())
}

/// Reject caller-owned surface metadata that attempts to set Meerkat-owned
/// labels or top-level app-context keys.
pub fn validate_public_surface_metadata(
    labels: Option<&std::collections::BTreeMap<String, String>>,
    app_context: Option<&serde_json::Value>,
) -> Result<(), String> {
    let metadata =
        meerkat_core::SurfaceMetadata::from_optional_parts(labels.cloned(), app_context.cloned());
    metadata.validate_public().map_err(|err| err.to_string())
}

/// List all skills with provenance and shadow information.
#[cfg(feature = "skills")]
///
/// Returns `None` if the skill runtime is not available.
pub async fn list_skills_introspection(
    skill_runtime: &Option<Arc<SkillRuntime>>,
    filter: &SkillFilter,
) -> Option<Result<Vec<SkillIntrospectionEntry>, SkillError>> {
    let runtime = skill_runtime.as_ref()?;
    Some(runtime.list_all_with_provenance(filter).await)
}

/// Load and inspect a skill by ID, optionally from a specific source.
#[cfg(feature = "skills")]
///
/// Returns `None` if the skill runtime is not available.
pub async fn inspect_skill(
    skill_runtime: &Option<Arc<SkillRuntime>>,
    key: &SkillKey,
    source_name: Option<&str>,
) -> Option<Result<SkillDocument, SkillError>> {
    let runtime = skill_runtime.as_ref()?;
    Some(runtime.load_from_source(key, source_name).await)
}

/// Spawn a task that forwards agent events from a channel to a callback.
///
/// Returns the sender half of the channel. The spawned task runs until
/// the sender is dropped.
pub fn spawn_event_forwarder<F>(callback: F) -> mpsc::Sender<AgentEvent>
where
    F: Fn(AgentEvent) + Send + 'static,
{
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

    #[cfg(not(target_arch = "wasm32"))]
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            callback(event);
        }
    });
    #[cfg(target_arch = "wasm32")]
    tokio_with_wasm::alias::task::spawn(async move {
        while let Some(event) = rx.recv().await {
            callback(event);
        }
    });

    tx
}

// ---------------------------------------------------------------------------
// Live MCP surface helpers (shared across RPC, REST)
// ---------------------------------------------------------------------------

/// Build a canonical [`McpLiveOpResponse`] for a staged operation.
///
/// All surfaces use this to ensure identical response shaping — operation,
/// server name, persisted handling, and `applied_at_turn` semantics.
#[cfg(feature = "mcp")]
pub fn mcp_live_response(
    session_id: String,
    operation: meerkat_contracts::McpLiveOperation,
    server_name: Option<String>,
    persisted: bool,
) -> meerkat_contracts::McpLiveOpResponse {
    meerkat_contracts::McpLiveOpResponse {
        session_id,
        operation,
        server_name,
        status: meerkat_contracts::McpLiveOpStatus::Staged,
        persisted,
        applied_at_turn: None,
    }
}

/// Build the MCP config mutation authority for public live-MCP surfaces.
#[cfg(feature = "mcp")]
pub fn mcp_config_mutation_authority(
    context_root: Option<std::path::PathBuf>,
    user_config_root: Option<std::path::PathBuf>,
) -> meerkat_core::mcp_config::McpConfigMutationAuthority {
    meerkat_core::mcp_config::McpConfigMutationAuthority::project(context_root, user_config_root)
}

/// Persist a live MCP add when requested by the caller.
#[cfg(all(feature = "mcp", not(target_arch = "wasm32")))]
pub async fn persist_mcp_add_if_requested(
    persisted: bool,
    authority: &meerkat_core::mcp_config::McpConfigMutationAuthority,
    server: meerkat_core::mcp_config::McpServerConfig,
) -> Result<Option<meerkat_core::mcp_config::McpConfigRollback>, String> {
    if !persisted {
        return Ok(None);
    }
    meerkat_core::mcp_config::McpConfig::persist_add_with_rollback(authority, server)
        .await
        .map(Some)
        .map_err(|err| err.to_string())
}

/// Persist a live MCP remove when requested by the caller.
#[cfg(all(feature = "mcp", not(target_arch = "wasm32")))]
pub async fn persist_mcp_remove_if_requested(
    persisted: bool,
    authority: &meerkat_core::mcp_config::McpConfigMutationAuthority,
    server_name: &str,
) -> Result<Option<meerkat_core::mcp_config::McpConfigRollback>, String> {
    if !persisted {
        return Ok(None);
    }
    match meerkat_core::mcp_config::McpConfig::persist_remove_with_rollback(authority, server_name)
        .await
    {
        Ok(rollback) => Ok(Some(rollback)),
        Err(meerkat_core::mcp_config::McpConfigError::ServerNotFound(_)) => Ok(None),
        Err(err) => Err(err.to_string()),
    }
}

/// Roll back a persisted MCP config mutation after a coupled live-stage failure.
#[cfg(all(feature = "mcp", not(target_arch = "wasm32")))]
pub async fn rollback_mcp_persisted_mutation(
    rollback: Option<meerkat_core::mcp_config::McpConfigRollback>,
) -> Result<(), String> {
    let Some(rollback) = rollback else {
        return Ok(());
    };
    rollback.rollback().await.map_err(|err| err.to_string())
}

/// Validate that a reload target exists in the adapter's active server set.
///
/// Returns `Ok(())` if the server is active, or an error message suitable
/// for returning to the caller.
#[cfg(feature = "mcp")]
pub async fn validate_reload_target(
    adapter: &meerkat_mcp::McpRouterAdapter,
    server_name: &str,
) -> Result<(), String> {
    let active = adapter.active_server_names().await;
    if active.iter().any(|n| n == server_name) {
        Ok(())
    } else {
        Err(format!(
            "MCP server '{server_name}' is not registered on this session"
        ))
    }
}

/// Emit [`AgentEvent::ToolConfigChanged`] events for a batch of lifecycle actions.
///
/// Also appends `[system-notice]` lines to `prompt` for forced removals.
/// Both RPC and REST call this at turn boundaries.
#[cfg(feature = "mcp")]
pub async fn emit_mcp_lifecycle_events(
    event_tx: &mpsc::Sender<meerkat_core::EventEnvelope<AgentEvent>>,
    source: &meerkat_core::EventSourceIdentity,
    prompt: &mut String,
    turn_number: u32,
    actions: Vec<meerkat_mcp::McpLifecycleAction>,
) {
    use meerkat_mcp::McpLifecyclePhase;

    const MCP_SEQ_SOURCE_CAP: usize = 8192;

    #[derive(Default)]
    struct McpSeqState {
        seq_by_source: HashMap<meerkat_core::EventSourceIdentity, u64>,
        source_order: VecDeque<meerkat_core::EventSourceIdentity>,
    }

    static MCP_EVENT_SEQ_BY_SOURCE: OnceLock<Mutex<McpSeqState>> = OnceLock::new();

    for action in actions {
        let source_id = source.clone();
        let mut payload = action.to_tool_config_changed_payload();
        payload.applied_at_turn = Some(turn_number);
        let target = payload.target.clone();
        let seq = {
            let map = MCP_EVENT_SEQ_BY_SOURCE.get_or_init(|| Mutex::new(McpSeqState::default()));
            let mut guard = match map.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if !guard.seq_by_source.contains_key(&source_id) {
                let source_key = source_id.clone();
                guard.source_order.push_back(source_key.clone());
                guard.seq_by_source.insert(source_key, 0);

                while guard.seq_by_source.len() > MCP_SEQ_SOURCE_CAP {
                    if let Some(evicted) = guard.source_order.pop_front() {
                        guard.seq_by_source.remove(&evicted);
                    } else {
                        break;
                    }
                }
            }

            let entry = guard.seq_by_source.entry(source_id).or_insert(0);
            *entry += 1;
            *entry
        };
        let _ = event_tx
            .send(meerkat_core::EventEnvelope::new_with_source(
                source.clone(),
                seq,
                None,
                AgentEvent::ToolConfigChanged {
                    payload: payload.clone(),
                },
            ))
            .await;
        if action.phase == McpLifecyclePhase::Forced {
            if !prompt.is_empty() {
                prompt.push('\n');
            }
            prompt.push_str(&format!(
                "[system-notice] MCP server '{target}' removal forced after drain timeout."
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::Config;

    #[test]
    fn mob_owner_key_matches_session_by_mob_labels() {
        let session_id = meerkat_core::SessionId::new();
        let target = crate::WorkAttentionTarget::LoweredOwner {
            owner_key: crate::WorkOwnerKey::agent("mob/alpha/agent/reviewer").expect("owner key"),
        };
        let labels = BTreeMap::from([
            ("mob_id".to_string(), "alpha".to_string()),
            ("agent_identity".to_string(), "reviewer".to_string()),
        ]);

        assert!(attention_target_matches_session(
            &target,
            &session_id,
            &labels
        ));
    }

    #[test]
    fn mob_owner_key_rejects_ambiguous_separator_round_trips() {
        assert_eq!(
            mob_agent_owner_key_parts("mob/alpha/agent/reviewer"),
            Some(("alpha", "reviewer"))
        );
        assert_eq!(
            mob_agent_owner_key_parts("mob/alpha/agent/reviewer/2"),
            None
        );
        assert_eq!(
            mob_agent_owner_key_parts("mob/alpha/extra/agent/reviewer"),
            None
        );

        let session_id = meerkat_core::SessionId::new();
        let target = crate::WorkAttentionTarget::LoweredOwner {
            owner_key: crate::WorkOwnerKey::agent("mob/alpha/agent/reviewer/2").expect("owner key"),
        };
        let labels = BTreeMap::from([
            ("mob_id".to_string(), "alpha".to_string()),
            ("agent_identity".to_string(), "reviewer/2".to_string()),
        ]);
        assert!(!attention_target_matches_session(
            &target,
            &session_id,
            &labels
        ));
    }

    #[test]
    fn attention_overlay_intersects_existing_turn_overlay() {
        let existing = meerkat_core::service::TurnToolOverlay {
            allowed_tools: Some(vec![
                meerkat_core::types::ToolName::from("workgraph_get"),
                meerkat_core::types::ToolName::from("non_workgraph"),
            ]),
            blocked_tools: Some(vec![meerkat_core::types::ToolName::from("blocked_by_flow")]),
            dispatch_context: BTreeMap::from([("turn".to_string(), serde_json::json!(true))]),
        };
        let attention = meerkat_core::service::TurnToolOverlay {
            allowed_tools: Some(vec![
                meerkat_core::types::ToolName::from("workgraph_get"),
                meerkat_core::types::ToolName::from("workgraph_update"),
            ]),
            blocked_tools: Some(vec![meerkat_core::types::ToolName::from(
                "workgraph_update",
            )]),
            dispatch_context: BTreeMap::from([(
                "workgraph.attention_projection".to_string(),
                serde_json::json!({"binding_id": "b"}),
            )]),
        };

        let overlay = compose_turn_tool_overlay(Some(existing), attention).expect("compose");

        assert_eq!(
            overlay.allowed_tools,
            Some(vec![meerkat_core::types::ToolName::from("workgraph_get")])
        );
        assert_eq!(
            overlay.blocked_tools,
            Some(vec![
                meerkat_core::types::ToolName::from("blocked_by_flow"),
                meerkat_core::types::ToolName::from("workgraph_update")
            ])
        );
        assert_eq!(overlay.dispatch_context["turn"], serde_json::json!(true));
        assert_eq!(
            overlay.dispatch_context["workgraph.attention_projection"],
            serde_json::json!({"binding_id": "b"})
        );
    }

    /// Minimal SessionService fake for owner-key attention arbitration: `read`
    /// serves the configured labels for any known session and `list` applies
    /// the label filter over the configured summaries, mirroring the real
    /// live-session listing the arbitration guard consults.
    struct RespawnOverlapSessionService {
        summaries: Vec<meerkat_core::service::SessionSummary>,
        labels: BTreeMap<String, String>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::SessionService for RespawnOverlapSessionService {
        async fn create_session(
            &self,
            _req: meerkat_core::service::CreateSessionRequest,
        ) -> Result<meerkat_core::RunResult, meerkat_core::service::SessionError> {
            Err(meerkat_core::service::SessionError::Agent(
                meerkat_core::AgentError::InternalError(
                    "not used by arbitration tests".to_string(),
                ),
            ))
        }

        async fn start_turn(
            &self,
            id: &meerkat_core::SessionId,
            _req: meerkat_core::StartTurnRequest,
        ) -> Result<meerkat_core::RunResult, meerkat_core::service::SessionError> {
            Err(meerkat_core::service::SessionError::NotFound { id: id.clone() })
        }

        async fn interrupt(
            &self,
            _id: &meerkat_core::SessionId,
        ) -> Result<(), meerkat_core::service::SessionError> {
            Ok(())
        }

        async fn read(
            &self,
            id: &meerkat_core::SessionId,
        ) -> Result<meerkat_core::service::SessionView, meerkat_core::service::SessionError>
        {
            Ok(meerkat_core::service::SessionView {
                state: meerkat_core::service::SessionInfo {
                    session_id: id.clone(),
                    created_at: std::time::SystemTime::UNIX_EPOCH,
                    updated_at: std::time::SystemTime::UNIX_EPOCH,
                    message_count: 0,
                    is_active: false,
                    model: "claude-sonnet-4-5".to_string(),
                    provider: meerkat_core::Provider::Anthropic,
                    last_assistant_text: None,
                    labels: self.labels.clone(),
                },
                billing: meerkat_core::service::SessionUsage {
                    total_tokens: 0,
                    usage: meerkat_core::Usage::default(),
                },
            })
        }

        async fn list(
            &self,
            query: meerkat_core::service::SessionQuery,
        ) -> Result<Vec<meerkat_core::service::SessionSummary>, meerkat_core::service::SessionError>
        {
            Ok(self
                .summaries
                .iter()
                .filter(|summary| {
                    query.labels.as_ref().is_none_or(|filter| {
                        filter.iter().all(|(key, value)| {
                            summary.labels.get(key).is_some_and(|label| label == value)
                        })
                    })
                })
                .cloned()
                .collect())
        }

        async fn archive(
            &self,
            _id: &meerkat_core::SessionId,
        ) -> Result<(), meerkat_core::service::SessionError> {
            Ok(())
        }
    }

    fn respawn_summary(
        session_id: meerkat_core::SessionId,
        created_at: std::time::SystemTime,
        labels: &BTreeMap<String, String>,
    ) -> meerkat_core::service::SessionSummary {
        meerkat_core::service::SessionSummary {
            session_id,
            created_at,
            updated_at: created_at,
            message_count: 0,
            total_tokens: 0,
            is_active: true,
            labels: labels.clone(),
        }
    }

    fn mob_owner_labels() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("mob_id".to_string(), "alpha".to_string()),
            ("agent_identity".to_string(), "reviewer".to_string()),
        ])
    }

    async fn owner_scoped_workgraph_service() -> crate::WorkGraphService {
        let service =
            crate::WorkGraphService::new(std::sync::Arc::new(crate::MemoryWorkGraphStore::new()));
        service
            .create_goal(crate::GoalCreateRequest {
                realm_id: None,
                namespace: None,
                title: "arbitrated goal".to_string(),
                description: None,
                target: crate::GoalAttentionTarget::Owner {
                    owner_key: crate::WorkOwnerKey::agent("mob/alpha/agent/reviewer")
                        .expect("owner key"),
                },
                mode: crate::WorkAttentionMode::Coordinate,
                completion_policy: crate::WorkCompletionPolicy::SelfAttest,
                delegated_authority: crate::AttentionDelegatedAuthority::AddEvidence,
                projection_policy: crate::AttentionProjectionPolicy::default(),
            })
            .await
            .expect("create owner-scoped goal");
        service
    }

    fn empty_start_turn_request() -> meerkat_core::StartTurnRequest {
        meerkat_core::StartTurnRequest {
            injected_context: Vec::new(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            system_prompt: None,
            event_tx: None,
            runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                meerkat_core::types::HandlingMode::Queue,
                None,
                Vec::new(),
                None,
            ),
        }
    }

    /// Respawn-overlap arbitration on the canonical apply-time injection
    /// path: when two live sessions carry the same mob owner labels, only
    /// the newest receives the attention overlay.
    #[tokio::test]
    async fn attention_overlay_binds_only_to_newest_labeled_session() {
        let labels = mob_owner_labels();
        let older_id = meerkat_core::SessionId::new();
        let newer_id = meerkat_core::SessionId::new();
        let base = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        let session_service = RespawnOverlapSessionService {
            summaries: vec![
                respawn_summary(older_id.clone(), base, &labels),
                respawn_summary(
                    newer_id.clone(),
                    base + std::time::Duration::from_secs(60),
                    &labels,
                ),
            ],
            labels: labels.clone(),
        };
        let workgraph_service = owner_scoped_workgraph_service().await;

        let mut older_request = empty_start_turn_request();
        inject_workgraph_attention_turn_overlay(
            &session_service,
            Some(&workgraph_service),
            &older_id,
            &mut older_request,
        )
        .await
        .expect("inject for older session");
        assert!(
            older_request.runtime.turn_tool_overlay.is_none(),
            "the older overlapping session must not receive the attention overlay"
        );

        let mut newer_request = empty_start_turn_request();
        inject_workgraph_attention_turn_overlay(
            &session_service,
            Some(&workgraph_service),
            &newer_id,
            &mut newer_request,
        )
        .await
        .expect("inject for newest session");
        let overlay = newer_request
            .runtime
            .turn_tool_overlay
            .expect("newest session must receive the attention overlay");
        assert!(
            overlay
                .dispatch_context
                .contains_key("workgraph.attention_projection"),
            "attention overlay must carry the projection dispatch context"
        );
    }

    /// Ask 25 degrade pin: multiple ACTIVE bindings resolving to one session
    /// (reachable through legacy rows, mixed-version writers, or cross-kind
    /// targets like Session{id} + Owner(session-key id) that share a session
    /// but not a target key) must NOT brick the member's every turn. The
    /// overlay arbitrates deterministically — newest binding wins — with a
    /// loud diagnostic instead of the former hard MultipleActiveBindings
    /// failure.
    #[tokio::test]
    async fn attention_overlay_arbitrates_multiple_active_bindings_newest_wins() {
        let session_id = meerkat_core::SessionId::new();
        let workgraph_service =
            crate::WorkGraphService::new(std::sync::Arc::new(crate::MemoryWorkGraphStore::new()));
        workgraph_service
            .create_goal(crate::GoalCreateRequest {
                realm_id: None,
                namespace: None,
                title: "session-target goal".to_string(),
                description: None,
                target: crate::GoalAttentionTarget::Session {
                    session_id: session_id.clone(),
                },
                mode: crate::WorkAttentionMode::Coordinate,
                completion_policy: crate::WorkCompletionPolicy::SelfAttest,
                delegated_authority: crate::AttentionDelegatedAuthority::AddEvidence,
                projection_policy: crate::AttentionProjectionPolicy::default(),
            })
            .await
            .expect("session-target goal");
        workgraph_service
            .create_goal(crate::GoalCreateRequest {
                realm_id: None,
                namespace: None,
                title: "owner-target goal on the same session".to_string(),
                description: None,
                target: crate::GoalAttentionTarget::Owner {
                    owner_key: crate::WorkOwnerKey::session(session_id.to_string())
                        .expect("owner key"),
                },
                mode: crate::WorkAttentionMode::Coordinate,
                completion_policy: crate::WorkCompletionPolicy::SelfAttest,
                delegated_authority: crate::AttentionDelegatedAuthority::AddEvidence,
                projection_policy: crate::AttentionProjectionPolicy::default(),
            })
            .await
            .expect("owner-target goal (distinct target key, same session)");

        let session_service = RespawnOverlapSessionService {
            summaries: Vec::new(),
            labels: BTreeMap::new(),
        };
        let mut request = empty_start_turn_request();
        inject_workgraph_attention_turn_overlay(
            &session_service,
            Some(&workgraph_service),
            &session_id,
            &mut request,
        )
        .await
        .expect("multi-binding resolution must arbitrate, not hard-fail the turn");
        assert!(
            request.runtime.turn_tool_overlay.is_some(),
            "the arbitrated winner's overlay must be injected"
        );
    }

    /// A session that is not yet visible in the session listing is
    /// mid-creation and definitionally the newest carrier of its labels, so
    /// creation-time injection (RPC/REST create-and-start paths) still binds.
    #[tokio::test]
    async fn attention_overlay_allows_unlisted_mid_creation_session() {
        let labels = mob_owner_labels();
        let listed_id = meerkat_core::SessionId::new();
        let mid_creation_id = meerkat_core::SessionId::new();
        let base = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        let session_service = RespawnOverlapSessionService {
            summaries: vec![respawn_summary(listed_id, base, &labels)],
            labels: labels.clone(),
        };
        let workgraph_service = owner_scoped_workgraph_service().await;

        let mut request = empty_start_turn_request();
        inject_workgraph_attention_turn_overlay_from_labels(
            &workgraph_service,
            &session_service,
            &mid_creation_id,
            &labels,
            &mut request,
        )
        .await
        .expect("inject for mid-creation session");
        assert!(
            request.runtime.turn_tool_overlay.is_some(),
            "a mid-creation session absent from the listing is the newest by construction"
        );
    }

    #[test]
    fn validate_public_peer_meta_rejects_reserved_labels() {
        let peer_meta = PeerMeta::default().with_label("mob_id", "team");
        let result = validate_public_peer_meta(Some(&peer_meta));
        assert!(
            result.is_err(),
            "reserved mob labels must be rejected on public surfaces"
        );
        let Err(err) = result else {
            unreachable!("asserted reserved labels are rejected above");
        };
        assert!(err.contains("reserved"));
        assert!(err.contains("mob_id"));
    }

    #[test]
    fn validate_public_peer_meta_allows_unreserved_labels() {
        let peer_meta = PeerMeta::default().with_label("team", "infra");
        let result = validate_public_peer_meta(Some(&peer_meta));
        assert!(
            result.is_ok(),
            "ordinary peer metadata should stay available"
        );
    }

    #[test]
    fn validate_public_surface_metadata_rejects_reserved_app_context_keys() {
        let app_context = serde_json::json!({
            "meerkat.runtime_id": "spoof",
            "client_ref": "ok"
        });
        let result = validate_public_surface_metadata(None, Some(&app_context));

        assert!(result.is_err(), "reserved app_context keys must fail");
        assert!(result.unwrap_err().contains("meerkat.runtime_id"));
    }

    #[test]
    fn build_models_catalog_response_returns_error_for_invalid_self_hosted_config() {
        let config: Result<Config, _> = toml::from_str(
            r#"
[self_hosted.models.gemma-4-e2b]
server = "missing"
remote_model = "gemma4:e2b"
display_name = "Gemma 4 E2B"
family = "gemma-4"
"#,
        );
        assert!(config.is_ok(), "config parse failed");
        let Ok(config) = config else {
            unreachable!("asserted config parse succeeded above");
        };

        let result = build_models_catalog_response(&config);
        assert!(result.is_err(), "invalid catalog config should fail");
        let Err(err) = result else {
            unreachable!("asserted invalid catalog config fails above");
        };
        assert!(err.to_string().contains("unknown server"));
    }

    #[test]
    fn build_models_catalog_response_exposes_stable_capability_bits() {
        let catalog = build_models_catalog_response(&Config::default()).expect("catalog response");
        let find_profile = |provider: &str, model: &str| {
            catalog
                .providers
                .iter()
                .find(|p| p.provider == provider)
                .and_then(|p| p.models.iter().find(|m| m.id == model))
                .and_then(|m| m.profile.as_ref())
                .unwrap_or_else(|| panic!("missing profile for {provider}:{model}"))
        };

        let claude = find_profile("anthropic", "claude-sonnet-4-5");
        assert!(claude.vision);
        assert!(claude.image_input);
        assert!(claude.image_tool_results);
        assert!(!claude.inline_video);
        assert!(!claude.realtime);
        assert!(claude.supports_web_search);
        assert!(!claude.image_generation);

        let gpt = find_profile("openai", "gpt-5.4");
        assert!(gpt.vision);
        assert!(gpt.image_input);
        assert!(gpt.image_tool_results);
        assert!(!gpt.inline_video);
        assert!(!gpt.realtime);
        assert!(gpt.supports_web_search);
        assert!(gpt.image_generation);

        for model_id in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            let model = catalog
                .providers
                .iter()
                .find(|provider| provider.provider == "openai")
                .and_then(|provider| provider.models.iter().find(|model| model.id == model_id))
                .unwrap_or_else(|| panic!("missing public catalog model openai:{model_id}"));
            assert_eq!(model.context_window, Some(1_050_000));
            assert_eq!(model.max_output_tokens, Some(128_000));
            let profile = model
                .profile
                .as_ref()
                .unwrap_or_else(|| panic!("missing public profile openai:{model_id}"));
            assert!(profile.vision);
            assert!(profile.image_input);
            assert!(profile.image_tool_results);
            assert!(profile.image_generation);
            assert!(profile.supports_reasoning);
            assert!(profile.supports_web_search);
            assert!(!profile.supports_temperature);

            let properties = profile.params_schema["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("missing params schema properties for {model_id}"));
            let effort = properties["reasoning_effort"]["enum"]
                .as_array()
                .expect("reasoning effort enum");
            assert!(effort.iter().any(|value| value == "max"));
            assert!(!effort.iter().any(|value| value == "ultra"));
            assert_eq!(
                properties["reasoning_mode"]["enum"],
                serde_json::json!(["standard", "pro"])
            );
            assert_eq!(
                properties["reasoning_context"]["enum"],
                serde_json::json!(["auto", "current_turn", "all_turns"])
            );
            assert_eq!(
                properties["text_verbosity"]["enum"],
                serde_json::json!(["low", "medium", "high"])
            );
            assert_eq!(
                properties["prompt_cache_options"]["properties"]["mode"]["enum"],
                serde_json::json!(["implicit", "explicit"])
            );
            assert_eq!(
                properties["prompt_cache_options"]["properties"]["ttl"]["enum"],
                serde_json::json!(["30m"])
            );
        }

        let realtime = find_profile("openai", "gpt-realtime-2");
        // gpt-realtime-2 accepts text + audio + image input per
        // OpenAI's model docs (`developers.openai.com/api/docs/models/gpt-realtime-2`),
        // so `image_input` is `true`. The previous expectation
        // (`!image_input`) reflected the older `gpt-realtime` /
        // `gpt-4o-realtime-preview` capability shape that the
        // catalog cleanup retired.
        assert!(realtime.image_input);
        assert!(realtime.realtime);
    }
}
