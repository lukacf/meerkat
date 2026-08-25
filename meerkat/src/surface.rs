//! Shared surface infrastructure helpers.
//!
//! Cross-cutting helpers used by all protocol surfaces (RPC, REST, MCP Server).

mod embedded;
#[cfg(all(
    feature = "session-store",
    feature = "live",
    not(target_arch = "wasm32")
))]
mod live_host;
#[cfg(all(
    feature = "session-store",
    feature = "live",
    not(target_arch = "wasm32")
))]
mod live_projection;
mod request_execution;
#[cfg(feature = "session-store")]
mod runtime_backed;
#[cfg(feature = "session-store")]
mod runtime_schedule_host;
mod schedule_host;
#[cfg(not(target_arch = "wasm32"))]
mod stdio_json;

#[cfg(all(
    feature = "session-store",
    feature = "live",
    feature = "experimental-gpt-live",
    not(target_arch = "wasm32")
))]
pub use crate::session_runtime::live_orchestration::ExperimentalLivePendingChannel;
pub use embedded::{
    build_embedded_service, build_embedded_service_from_builder, set_default_schedule_tools,
    set_default_workgraph_namespace_grant, set_default_workgraph_tools,
};
#[cfg(all(
    feature = "session-store",
    feature = "live-webrtc",
    not(target_arch = "wasm32")
))]
pub use live_host::{
    CoordinatedLiveWebrtcAnswer, LiveWebrtcAnswerCoordinatorError, LiveWebrtcAnswerDeliveryCustody,
    LiveWebrtcBoundReadyBindFailure, LiveWebrtcBoundReadyBinder, LiveWebrtcBoundReadyCustody,
    coordinate_live_webrtc_answer,
};
#[cfg(all(
    feature = "session-store",
    feature = "live",
    feature = "live-webrtc",
    feature = "experimental-gpt-live",
    not(target_arch = "wasm32")
))]
pub use live_host::{
    ExperimentalGptLiveContextMirrorHost, ExperimentalLiveChannelCloseError,
    ExperimentalLiveChannelCustodyStatus, ExperimentalLiveChannelPhaseStatus,
    ExperimentalLiveContextRecoveryError, ExperimentalLivePlaybackOwnerReadiness,
    ExperimentalLiveReplacementRequired,
};
#[cfg(all(
    feature = "session-store",
    feature = "live",
    not(target_arch = "wasm32")
))]
pub use live_host::{
    MobOwnedOnlyIngress, ServiceLiveToolDispatcher, ServiceMemberLiveHost,
    ServiceMemberLiveHostConfig, member_live_error_from_open, member_live_error_from_verb,
};
#[cfg(all(
    feature = "session-store",
    feature = "live",
    not(target_arch = "wasm32")
))]
pub use live_projection::ServiceLiveProjection;
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
    SurfaceRequestTerminalPolicy, noop_request_action, prepare_surface_session,
    prepare_surface_session_with_id, request_action,
};
#[cfg(all(feature = "session-store", feature = "comms"))]
pub use runtime_backed::configure_peer_ingress;
#[cfg(feature = "session-store")]
pub(crate) use runtime_backed::materialize_session_actor_unattached_with_reserved_admission_and_actor_slot;
#[cfg(feature = "session-store")]
pub use runtime_backed::{
    AttachedActorPublicationRefreshFn, PersistentRuntimeExecutor, RuntimeBackedInitialTurn,
    SurfaceRuntimeMaterializeError, build_runtime_backed_service,
    build_runtime_backed_service_with_capacities, default_persistent_executor,
    default_persistent_executor_with_workgraph_service, install_prepared_runtime_interrupt_handle,
    install_prepared_runtime_interrupt_handle_for_actor_slot,
    materialize_attached_session_actor_only,
    materialize_attached_session_actor_only_with_reserved_admission, materialize_session,
    materialize_session_actor_unattached_with_reserved_admission,
    materialize_session_under_runtime_turn_boundary, materialize_session_with_reserved_admission,
    materialize_session_with_reserved_admission_and_actor_slot,
    materialize_session_with_reserved_admission_under_runtime_turn_boundary,
    materialize_session_with_reserved_admission_under_runtime_turn_boundary_and_publication_refresh,
    persistent_runtime_post_stop_cleanup_handle,
    persistent_runtime_post_stop_cleanup_handle_for_actor_slot,
    persistent_runtime_publication_handle, persistent_runtime_publication_handle_for_actor_slot,
    persistent_runtime_turn_finalization_boundary_handle,
    run_runtime_backed_initial_turn_with_machine, split_runtime_backed_eager_create_request,
};
#[cfg(feature = "session-store")]
pub use runtime_schedule_host::{
    spawn_runtime_backed_schedule_host, spawn_runtime_backed_schedule_host_with_mobs,
};
pub use schedule_host::{
    AcceptedScheduledInput, MobMemberCurrentSessionScheduleResolver, NoopScheduleMobHost,
    ScheduleAdmissionOutcome, ScheduleHostHandle, ScheduledEventDispatch, ScheduledPromptDispatch,
    SharedScheduleTargetAdapter, SurfaceScheduleMobHost, SurfaceScheduleSessionHost,
    accept_schedule_runtime_input_with_reconciliation, async_completion_dispatch,
    async_completion_dispatch_with_admission_outcome, build_dispatch_from_accepted,
    immediate_completed_dispatch, immediate_completed_dispatch_with_admission_outcome,
    immediate_delivery_failure, mob_member_schedule_identity, parse_mob_member_schedule_identity,
    recover_mob_member_identity_from_session_target, runtime_delivery_dispatch_from_admission,
    schedule_delivery_idempotency_key, schedule_host_supported, schedule_runtime_correlation_id,
    schedule_runtime_delivery_idempotency_key, spawn_schedule_host,
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
    pub multi_host_mobs: bool,
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
    pub durable_jobs: bool,
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
            multi_host_mobs: false,
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
            durable_jobs: false,
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
            // Multi-host mobs (SD-8) is an explicit producer fact rather
            // than an alias for generic mob availability. This keeps the
            // projection honest in compositions that ship local mobs only.
            multi_host_mobs: options.multi_host_mobs,
            durable_jobs: options.durable_jobs,
        },
    }
}

/// Prefix on a `checks` key that names a dimension **nobody probed**.
///
/// The value carried under such a key describes the *coverage*, not the
/// dimension: `Degraded` means "this was not looked at", never "this was
/// looked at and found broken".
///
/// This coverage class is deliberately kept **out** of the `status` rollup, and
/// its sibling [`RUNTIME_HEALTH_UNREADABLE_PREFIX`] is deliberately kept **in**.
/// The asymmetry is the whole design and is not an oversight to be tidied away:
/// an unmeasured dimension is a *permanent structural* fact - no probe exists,
/// and none will until someone writes one - so folding it into `status` pins
/// every healthy host at `Degraded` forever and disarms the alarm. An
/// unreadable dimension is a *transient runtime* fact - a probe ran and lost a
/// race - so folding it in costs at most one amber scrape, which any alert
/// window absorbs.
///
/// The spelling has one owner. [`meerkat_core::UNMEASURED_MARKER_PREFIX`] is
/// the same vocabulary used by degradation markers minted inside the agent
/// loop (for example `unmeasured:turn_usage_accounting`), so an operator who
/// learns to read one reads the other; this alias exists because the facade
/// depends on core and not the reverse.
pub const RUNTIME_HEALTH_UNMEASURED_PREFIX: &str = meerkat_core::UNMEASURED_MARKER_PREFIX;

/// Prefix on a `checks` key that names a dimension a probe **tried and failed
/// to observe**.
///
/// Like [`RUNTIME_HEALTH_UNMEASURED_PREFIX`] the value describes the
/// observation rather than the dimension, but it makes the opposite claim:
/// something ran, and it came back without a reading.
///
/// This class **does** enter the `status` rollup, at `Degraded`. A scrape that
/// could not look at a dimension has not established that the dimension is
/// healthy, and `status: ok` in that payload would be exactly the "one reading
/// stands in for the whole host" defect this projection exists to remove,
/// reached by a different route. Folding it in is affordable precisely because
/// it is transient: the condition clears on the next successful read, so a
/// one-off blip dies inside any `for:` window, and a dimension that stays
/// unreadable across scrapes is itself the incident (something is holding the
/// authority a probe must not block on).
///
/// The reason this is a separate key rather than a `Degraded` under the plain
/// dimension name: a plain `session_durability: degraded` is a *specific and
/// actionable* claim - some session needs a cold reload - and publishing it
/// from a failed read would be asserting a fault nobody observed. A diagnostic
/// that can report faults it did not see is back to being noise.
pub const RUNTIME_HEALTH_UNREADABLE_PREFIX: &str = "unreadable:";

/// Dimensions a runtime host health projection is expected to cover.
///
/// A caller reports one [`RuntimeHealthObservation`] per dimension it probed;
/// [`build_runtime_host_health_from_observations`] names every remaining one as
/// unmeasured rather than leaving it out, so an operator reading `checks` sees
/// the coverage instead of inferring it from absence.
///
/// Deleting a name from this list asserts the dimension is out of scope for
/// this projection. It does not assert the dimension is healthy, and it is not
/// a way to quiet a report: the way to move a dimension out of
/// `unmeasured:<name>` is to probe it and pass the observation in.
pub const RUNTIME_HEALTH_DIMENSIONS: [&str; 5] = [
    "jobs",
    "session_liveness",
    "session_durability",
    "session_runtime_loop",
    "session_run_start",
];

/// What one probe established about one health dimension.
///
/// Three facts are possible about a dimension and all three are different, so
/// all three get their own representation rather than sharing one:
///
/// - **nobody probed it** - the dimension is simply absent from the observation
///   set handed to [`build_runtime_host_health_from_observations`], and is
///   published as `unmeasured:<dimension>`;
/// - **a probe ran and observed it** - [`Self::Measured`], published under the
///   plain dimension name at the rung it saw;
/// - **a probe ran and could not observe it** - [`Self::Unreadable`], published
///   as `unreadable:<dimension>`.
///
/// The distinction between the first and the last is the one this type exists
/// to make impossible to lose. "Never measured" and "attempted and failed" are
/// not the same fact: the first is a gap in what this host was ever built to
/// see, the second is a gap in what this particular scrape saw. Collapsing them
/// into one key would force a single rollup rule onto two facts that need
/// opposite ones, and whichever rule won, one of the two would be reported as a
/// lie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeHealthObservation {
    /// The probe read the dimension. This is what it found.
    Measured(RuntimeHostHealthStatus),
    /// The probe ran and could not read the dimension at that instant.
    ///
    /// This is a fact about the observation, not about the dimension: it says
    /// nothing at all about whether the underlying dimension is healthy, which
    /// is exactly why it may not leave `status` at `Ok`.
    Unreadable,
}

/// Worse-wins fold over two health rungs.
///
/// [`RuntimeHostHealthStatus`] carries no ordering, so the severity ladder is
/// spelled out here rather than derived on the wire type.
fn worse_runtime_health(
    left: RuntimeHostHealthStatus,
    right: RuntimeHostHealthStatus,
) -> RuntimeHostHealthStatus {
    match (left, right) {
        (RuntimeHostHealthStatus::Unhealthy, _) | (_, RuntimeHostHealthStatus::Unhealthy) => {
            RuntimeHostHealthStatus::Unhealthy
        }
        (RuntimeHostHealthStatus::Degraded, _) | (_, RuntimeHostHealthStatus::Degraded) => {
            RuntimeHostHealthStatus::Degraded
        }
        (RuntimeHostHealthStatus::Ok, RuntimeHostHealthStatus::Ok) => RuntimeHostHealthStatus::Ok,
    }
}

/// Build a read-only runtime health projection from what a caller's probes
/// actually established.
///
/// The rule this function exists to enforce: **a diagnostic may only assert the
/// scope it observed.** Callers hand in one [`RuntimeHealthObservation`] per
/// dimension of [`RUNTIME_HEALTH_DIMENSIONS`] they probed. Every dimension with
/// no entry is named as `unmeasured:<dimension>` at
/// [`RuntimeHostHealthStatus::Degraded`] so the gap is legible in the payload
/// instead of being inferred from a missing key.
///
/// ## The three key shapes in `checks`
///
/// | shape | meaning | in the rollup? |
/// |---|---|---|
/// | `<dimension>` | a probe ran and saw this | yes, at the rung it saw |
/// | `unreadable:<dimension>` | a probe ran and could not see | yes, at `Degraded` |
/// | `unmeasured:<dimension>` | no probe exists | no |
///
/// ## What `status` means, and why only one coverage class sets it
///
/// `status` is the worst rung over the dimensions this caller **attempted**:
/// "everything I looked at is ok" (`Ok`), or "something I looked at is not, or
/// I could not look" (`Degraded` / `Unhealthy`). Dimensions nobody probed are
/// reported in `checks` and are deliberately excluded from that fold.
///
/// The alternative - folding unmeasured coverage in too - is more literal and
/// strictly worse in practice: as long as one declared dimension has no probe,
/// every healthy host publishes `Degraded` forever, so `status != ok` carries
/// no incident signal and the first thing an operator does is mute it. A
/// permanently amber light is not honesty; it is a disabled alarm with a good
/// excuse. That argument turns entirely on the condition being *permanent*, so
/// it does not extend to [`RuntimeHealthObservation::Unreadable`], which clears
/// itself on the next successful read.
///
/// This is emphatically not the defect this projection was built to fix. That
/// one published a single `ok` derived from a single reading, with no statement
/// anywhere of what had been looked at, so a caller could not distinguish full
/// coverage from a fifth of it. Here the coverage claim is in the same payload,
/// enumerated against a declared dimension list, and it is impossible for a
/// caller to read `status: ok` as "everything is fine" without also being
/// handed the list of things nobody checked.
///
/// Four properties keep that boundary from eroding:
///
/// - a caller that probed **nothing** still reports `Degraded`, so an empty
///   fold can never mint an unearned `Ok`;
/// - a measured `Unhealthy` still escalates;
/// - a probe that **ran and could not read** still reaches `status`, so a
///   scrape that failed to observe a dimension can never publish `ok` on the
///   strength of the dimensions it did read;
/// - every unprobed dimension is still named, so shrinking coverage is visible
///   in the payload rather than silent.
///
/// Keys outside [`RUNTIME_HEALTH_DIMENSIONS`] are accepted and rolled up like
/// any other observation; they simply do not clear an unmeasured dimension.
pub fn build_runtime_host_health_from_observations(
    observations: impl IntoIterator<Item = (String, RuntimeHealthObservation)>,
) -> RuntimeHostHealth {
    // Collected first so a duplicate dimension resolves last-wins into a single
    // entry, rather than publishing one dimension under two coverage keys.
    let attempted: BTreeMap<String, RuntimeHealthObservation> = observations.into_iter().collect();
    let mut checks: BTreeMap<String, RuntimeHostHealthStatus> = BTreeMap::new();
    // Folded over the attempted set only, so `status` speaks for what this
    // caller tried to observe. Nothing attempted is not health, so an empty
    // fold is `Degraded` rather than the `Ok` identity.
    let mut rollup: Option<RuntimeHostHealthStatus> = None;
    for (dimension, observation) in &attempted {
        let rung = match observation {
            RuntimeHealthObservation::Measured(status) => {
                checks.insert(dimension.clone(), *status);
                *status
            }
            RuntimeHealthObservation::Unreadable => {
                checks.insert(
                    format!("{RUNTIME_HEALTH_UNREADABLE_PREFIX}{dimension}"),
                    RuntimeHostHealthStatus::Degraded,
                );
                RuntimeHostHealthStatus::Degraded
            }
        };
        rollup = Some(match rollup {
            Some(previous) => worse_runtime_health(previous, rung),
            None => rung,
        });
    }
    let status = rollup.unwrap_or(RuntimeHostHealthStatus::Degraded);
    for dimension in RUNTIME_HEALTH_DIMENSIONS {
        if !attempted.contains_key(dimension) {
            checks.insert(
                format!("{RUNTIME_HEALTH_UNMEASURED_PREFIX}{dimension}"),
                RuntimeHostHealthStatus::Degraded,
            );
        }
    }
    RuntimeHostHealth {
        contract_version: ContractVersion::CURRENT,
        status,
        checks,
    }
}

/// Build a read-only runtime health projection that probed nothing.
///
/// Every dimension of [`RUNTIME_HEALTH_DIMENSIONS`] is reported unmeasured and
/// `status` is `Degraded`, because a caller that probed nothing has nothing to
/// roll up and may not mint an unearned `Ok`. This is the one case where
/// `Degraded` names coverage rather than an observed fault, and it exists so
/// the empty fold cannot be mistaken for a clean bill of health.
/// A surface that probes something must call
/// [`build_runtime_host_health_from_observations`] with what its probes
/// established.
pub fn build_runtime_host_health() -> RuntimeHostHealth {
    build_runtime_host_health_from_observations(std::iter::empty::<(
        String,
        RuntimeHealthObservation,
    )>())
}

/// Build host information from existing surface/runtime facts.
///
/// This projection intentionally does not expose topology, lease, project, or
/// host-registry authority. Callers can rebuild it from the config-store
/// metadata and surface options.
///
/// # `health` is a placeholder every probing caller must replace
///
/// `RuntimeHostInfo::health` is a required wire field and this function is a
/// pure projection: it is handed surface options and config metadata, holds no
/// runtime, and can therefore observe no health dimension. It cannot omit the
/// field and it may not invent a reading, so it publishes
/// [`build_runtime_host_health`] - the probed-nothing projection, `Degraded`
/// with every declared dimension named `unmeasured:*`.
///
/// That is the deliberate failure direction. A caller that forgets to replace
/// it now ships a payload that visibly names everything nobody looked at,
/// instead of the `status: ok, checks: {}` this used to publish, which was an
/// invisible clean bill of health from a function that had measured nothing.
/// **A surface that can probe owes `host_info` the health it actually
/// measured**, and every in-tree surface does exactly that:
/// `meerkat_rpc::handlers::runtime_host::handle_info` and
/// `meerkat_rest`'s `get_runtime_host_info` both overwrite `health` with their
/// own probe results, each pinned by a test asserting the embedded value equals
/// what that surface's own health route publishes.
///
/// Taking the health as a parameter would make that obligation a type rather
/// than a convention. It is deliberately not done here: it is a breaking change
/// to a published signature, and it would change zero published bytes, since
/// every caller that can probe already replaces the value. Worth doing on the
/// next release that is already taking breaks; not worth doing for its own
/// sake.
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

/// Build a [`meerkat_contracts::ModelsCatalogResponse`] from the resolved model registry.
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
                        release_stage: match model_profile.release_stage {
                            meerkat_core::ModelReleaseStage::Stable => {
                                meerkat_contracts::WireModelReleaseStage::Stable
                            }
                            meerkat_core::ModelReleaseStage::Experimental => {
                                meerkat_contracts::WireModelReleaseStage::Experimental
                            }
                            meerkat_core::ModelReleaseStage::OperatorDefined => {
                                meerkat_contracts::WireModelReleaseStage::OperatorDefined
                            }
                        },
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
                        release_stage: match entry.release_stage {
                            meerkat_core::ModelReleaseStage::Stable => {
                                meerkat_contracts::WireModelReleaseStage::Stable
                            }
                            meerkat_core::ModelReleaseStage::Experimental => {
                                meerkat_contracts::WireModelReleaseStage::Experimental
                            }
                            meerkat_core::ModelReleaseStage::OperatorDefined => {
                                meerkat_contracts::WireModelReleaseStage::OperatorDefined
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

/// Build a canonical [`meerkat_contracts::McpLiveOpResponse`] for a staged operation.
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
    fn runtime_host_capabilities_project_multi_host_mob_availability() {
        let mut options = RuntimeHostSurfaceOptions::process("test-host", "0");
        assert!(
            !build_runtime_host_capabilities(&options)
                .features
                .multi_host_mobs,
            "process defaults must not claim an unconfigured multi-host surface"
        );

        options.mobs = true;
        options.multi_host_mobs = true;
        let capabilities = build_runtime_host_capabilities(&options);
        assert!(capabilities.features.mobs);
        assert!(capabilities.features.multi_host_mobs);
    }

    /// The dimension names below are typed out rather than read from
    /// [`RUNTIME_HEALTH_DIMENSIONS`], so emptying that list fails these tests
    /// instead of trivially satisfying them.
    #[test]
    fn runtime_health_that_probed_nothing_does_not_report_ok() {
        let health = build_runtime_host_health();

        for dimension in [
            "unmeasured:jobs",
            "unmeasured:session_liveness",
            "unmeasured:session_durability",
            "unmeasured:session_runtime_loop",
            "unmeasured:session_run_start",
        ] {
            assert_eq!(
                health.checks.get(dimension),
                Some(&RuntimeHostHealthStatus::Degraded),
                "a projection that measured nothing must name `{dimension}`: {:?}",
                health.checks
            );
        }
        assert_eq!(
            health.status,
            RuntimeHostHealthStatus::Degraded,
            "nothing measured is not health: {health:?}"
        );
    }

    /// Unmeasured coverage is published, and is deliberately kept out of the
    /// rollup so `status != ok` stays an incident predicate rather than a
    /// standing amber light nobody can act on.
    #[test]
    fn runtime_health_names_unmeasured_dimensions_without_spending_the_rollup_on_them() {
        let health = build_runtime_host_health_from_observations([(
            "jobs".to_string(),
            RuntimeHealthObservation::Measured(RuntimeHostHealthStatus::Ok),
        )]);

        assert_eq!(
            health.checks.get("jobs"),
            Some(&RuntimeHostHealthStatus::Ok),
            "a measured dimension keeps its measured rung: {:?}",
            health.checks
        );
        assert_eq!(
            health.checks.get("unmeasured:jobs"),
            None,
            "a measured dimension must not also be reported unmeasured: {:?}",
            health.checks
        );
        assert_eq!(
            health.checks.get("unmeasured:session_liveness"),
            Some(&RuntimeHostHealthStatus::Degraded),
            "an unmeasured dimension must be named, not omitted: {:?}",
            health.checks
        );
        assert_eq!(
            health.status,
            RuntimeHostHealthStatus::Ok,
            "everything measured was ok, so the rollup says so and `checks` \
             carries the coverage gap: {health:?}"
        );
    }

    /// The rollup is over measurements, so a fault in one dimension is not
    /// laundered by healthy readings elsewhere or by unmeasured coverage.
    #[test]
    fn runtime_health_rollup_takes_the_worst_measured_rung() {
        let health = build_runtime_host_health_from_observations([
            (
                "jobs".to_string(),
                RuntimeHealthObservation::Measured(RuntimeHostHealthStatus::Ok),
            ),
            (
                "session_runtime_loop".to_string(),
                RuntimeHealthObservation::Measured(RuntimeHostHealthStatus::Degraded),
            ),
        ]);

        assert_eq!(
            health.status,
            RuntimeHostHealthStatus::Degraded,
            "one measured fault must survive beside a healthy sibling: {health:?}"
        );
        assert_eq!(
            health.checks.get("unmeasured:session_liveness"),
            Some(&RuntimeHostHealthStatus::Degraded),
            "the coverage gap is still published beside the fault: {:?}",
            health.checks
        );
    }

    #[test]
    fn runtime_health_publishes_a_measured_unhealthy_beside_unmeasured_coverage() {
        let health = build_runtime_host_health_from_observations([(
            "jobs".to_string(),
            RuntimeHealthObservation::Measured(RuntimeHostHealthStatus::Unhealthy),
        )]);

        assert_eq!(
            health.status,
            RuntimeHostHealthStatus::Unhealthy,
            "a measured failure reaches the rollup on its own, without needing \
             unmeasured coverage to carry it: {health:?}"
        );
        assert_eq!(
            health.checks.get("unmeasured:session_liveness"),
            Some(&RuntimeHostHealthStatus::Degraded),
            "and the coverage gap is still named beside it: {:?}",
            health.checks
        );
    }

    #[test]
    fn runtime_health_full_coverage_leaves_no_unmeasured_key_behind() {
        let health = build_runtime_host_health_from_observations([
            (
                "jobs".to_string(),
                RuntimeHealthObservation::Measured(RuntimeHostHealthStatus::Ok),
            ),
            (
                "session_liveness".to_string(),
                RuntimeHealthObservation::Measured(RuntimeHostHealthStatus::Ok),
            ),
            (
                "session_durability".to_string(),
                RuntimeHealthObservation::Measured(RuntimeHostHealthStatus::Ok),
            ),
            (
                "session_runtime_loop".to_string(),
                RuntimeHealthObservation::Measured(RuntimeHostHealthStatus::Ok),
            ),
            (
                "session_run_start".to_string(),
                RuntimeHealthObservation::Measured(RuntimeHostHealthStatus::Ok),
            ),
        ]);

        assert!(
            !health
                .checks
                .keys()
                .any(|key| key.starts_with(RUNTIME_HEALTH_UNMEASURED_PREFIX)),
            "full coverage leaves nothing unmeasured: {:?}",
            health.checks
        );
        assert_eq!(
            health.status,
            RuntimeHostHealthStatus::Ok,
            "every declared dimension measured and ok: {health:?}"
        );

        // Drop one dimension entirely - nobody probes it, as opposed to a probe
        // failing. The rollup is unchanged, because nothing that was observed
        // got worse. What changes is the published coverage, which is the whole
        // point of naming the gap rather than folding it in.
        let partial = build_runtime_host_health_from_observations([
            (
                "jobs".to_string(),
                RuntimeHealthObservation::Measured(RuntimeHostHealthStatus::Ok),
            ),
            (
                "session_liveness".to_string(),
                RuntimeHealthObservation::Measured(RuntimeHostHealthStatus::Ok),
            ),
            (
                "session_durability".to_string(),
                RuntimeHealthObservation::Measured(RuntimeHostHealthStatus::Ok),
            ),
        ]);
        assert_eq!(
            partial.checks.get("unmeasured:session_runtime_loop"),
            Some(&RuntimeHostHealthStatus::Degraded),
            "the dropped dimension is named: {:?}",
            partial.checks
        );
        assert_eq!(
            partial.checks.get("session_runtime_loop"),
            None,
            "a dropped dimension must not keep a measured key: {:?}",
            partial.checks
        );
        assert_eq!(
            partial.status,
            RuntimeHostHealthStatus::Ok,
            "narrower coverage is reported in `checks`, not smuggled into \
             `status` as a fault nobody can act on: {partial:?}"
        );
    }

    /// The exact shape of the original defect, reached by the other route: a
    /// scrape where the only dimension that answered was `jobs`, because both
    /// session probes ran and could not read.
    ///
    /// If an unreadable probe were treated like an unbuilt one, this payload
    /// would be `status: ok` on the strength of a single reading while two
    /// dimensions went unobserved. That is the defect this whole projection
    /// exists to remove.
    #[test]
    fn runtime_health_that_could_not_read_a_dimension_does_not_report_ok() {
        let health = build_runtime_host_health_from_observations([
            (
                "jobs".to_string(),
                RuntimeHealthObservation::Measured(RuntimeHostHealthStatus::Ok),
            ),
            (
                "session_durability".to_string(),
                RuntimeHealthObservation::Unreadable,
            ),
            (
                "session_runtime_loop".to_string(),
                RuntimeHealthObservation::Unreadable,
            ),
        ]);

        assert_eq!(
            health.status,
            RuntimeHostHealthStatus::Degraded,
            "a scrape that could not look at two dimensions has not established \
             that this host is healthy: {health:?}"
        );
        for dimension in ["session_durability", "session_runtime_loop"] {
            assert_eq!(
                health.checks.get(&format!("unreadable:{dimension}")),
                Some(&RuntimeHostHealthStatus::Degraded),
                "a failed read must be named as a failed read: {:?}",
                health.checks
            );
            assert_eq!(
                health.checks.get(dimension),
                None,
                "a failed read must not publish a rung under the plain \
                 dimension name, which would assert a fault nobody observed: \
                 {:?}",
                health.checks
            );
            // The two coverage vocabularies are exclusive. A dimension that was
            // attempted is not also unbuilt, and publishing both would let a
            // reader pick whichever claim suited them.
            assert_eq!(
                health.checks.get(&format!("unmeasured:{dimension}")),
                None,
                "an attempted dimension must not also be published as \
                 unmeasured: {:?}",
                health.checks
            );
        }
        assert_eq!(
            health.checks.get("unmeasured:session_liveness"),
            Some(&RuntimeHostHealthStatus::Degraded),
            "the genuinely unbuilt dimension keeps its own vocabulary: {:?}",
            health.checks
        );
    }

    /// The asymmetry stated as an executable claim, so a future simplification
    /// that unifies the two coverage classes fails here rather than in
    /// production.
    ///
    /// Both fixtures below are states a real surface can be in: the only
    /// difference between them is whether the two probed dimensions came back
    /// with a reading. `session_liveness` is absent from both, because a
    /// dimension with no probe cannot have had a probe fail and a fixture that
    /// claimed otherwise would be modelling a state nothing can produce.
    #[test]
    fn runtime_health_folds_a_failed_read_into_status_but_never_an_unbuilt_probe() {
        let unbuilt = build_runtime_host_health_from_observations([(
            "jobs".to_string(),
            RuntimeHealthObservation::Measured(RuntimeHostHealthStatus::Ok),
        )]);
        let failed_read = build_runtime_host_health_from_observations([
            (
                "jobs".to_string(),
                RuntimeHealthObservation::Measured(RuntimeHostHealthStatus::Ok),
            ),
            (
                "session_durability".to_string(),
                RuntimeHealthObservation::Unreadable,
            ),
            (
                "session_runtime_loop".to_string(),
                RuntimeHealthObservation::Unreadable,
            ),
        ]);

        assert_eq!(
            unbuilt.checks.get("unmeasured:session_liveness"),
            failed_read.checks.get("unmeasured:session_liveness"),
            "the unbuilt dimension is reported identically on both sides, so \
             the only thing that differs is the failed read: {:?} vs {:?}",
            unbuilt.checks,
            failed_read.checks
        );
        assert_eq!(
            unbuilt.status,
            RuntimeHostHealthStatus::Ok,
            "a host whose unbuilt dimensions are simply unbuilt must not stand \
             permanently amber; that is a muted alarm, not honesty: {unbuilt:?}"
        );
        assert_eq!(
            failed_read.status,
            RuntimeHostHealthStatus::Degraded,
            "a host whose probes ran and lost has observed nothing about those \
             dimensions this scrape, and one amber scrape is affordable because \
             the condition clears itself: {failed_read:?}"
        );
    }

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
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
                priority: Default::default(),
                labels: Default::default(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
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
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
                priority: Default::default(),
                labels: Default::default(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
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
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
                priority: Default::default(),
                labels: Default::default(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
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
