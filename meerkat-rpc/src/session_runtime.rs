//! SessionRuntime - keeps agents alive between turns.
//!
//! Delegates to [`PersistentSessionService`] for session lifecycle management.
//! `FactoryAgentBuilder` bridges `AgentFactory::build_agent()` into the
//! `SessionAgentBuilder` / `SessionAgent` traits used by the service.
//!
//! The runtime preserves the two-step create-then-run API required by the
//! JSON-RPC handlers: `create_session()` stages a build config and returns a
//! `SessionId`; the first `start_turn()` call for that ID materializes the
//! session inside the service (which runs the first turn).

#[path = "session_runtime/schedule_host.rs"]
mod schedule_host;

use std::collections::BTreeMap;
use std::path::PathBuf;
#[cfg(feature = "mcp")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};
#[cfg(feature = "mcp")]
use std::time::Duration;

use meerkat::{
    AgentBuildConfig, AgentFactory, FactoryAgentBuilder, PersistenceBundle,
    PersistentSessionService, ScheduleService, ScheduleToolDispatcher, StagedPhase,
    StagedSessionRegistry, StagedSlot, encode_llm_client_override_for_service,
};
use meerkat_client::{LlmClient, realtime_session::RealtimeSessionOpenConfig};
use meerkat_core::EventEnvelope;
use meerkat_core::RunId;
#[cfg(all(test, feature = "mcp"))]
use meerkat_core::ToolConfigChangedPayload;
use meerkat_core::event::AgentEvent;
use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
use meerkat_core::lifecycle::run_primitive::{
    ConversationContextAppend, CoreRenderable, RunApplyBoundary, RunPrimitive,
};
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextResult, CreateSessionRequest,
    DeferredPromptPolicy, SessionControlError, SessionError, SessionHistoryQuery, SessionQuery,
    SessionService, SessionServiceControlExt, SessionServiceHistoryExt, StartTurnRequest,
};
use meerkat_core::skills::{SkillError, SourceIdentityRegistry};
use meerkat_core::types::{Message, RunResult, SessionId};
#[cfg(feature = "mcp")]
use meerkat_core::{AgentToolDispatcher, ToolGateway};
use meerkat_core::{
    Config, ConfigStore, ContentInput, PendingSystemContextAppend, Session, SessionLlmIdentity,
    SessionSystemContextState, SurfaceSessionRecoveryContext, SurfaceSessionRecoveryError,
    SurfaceSessionRecoveryOverrides, SystemMessage, build_recovered_session,
};
use meerkat_runtime::RuntimeDriverError;
use meerkat_runtime::{
    HydratedSessionLlmState, MeerkatMachine, ResolvedSessionLlmReconfigure,
    SessionLlmCapabilitySurface, SessionLlmCapabilitySurfaceStatus, SessionLlmReconfigureHost,
    SessionLlmReconfigureRequest, SessionServiceRuntimeExt,
};
use tokio::sync::{Mutex, RwLock, mpsc};

use crate::error;
use crate::protocol::RpcError;
#[cfg(feature = "mcp")]
use meerkat::{
    McpLifecycleAction, McpLifecyclePhase, McpReloadTarget, McpRouter, McpRouterAdapter,
    McpServerConfig,
};
#[cfg(feature = "mcp")]
use meerkat_core::ToolConfigChangeOperation;

fn render_context_append_text(content: &CoreRenderable) -> String {
    match content {
        CoreRenderable::Text { text } => text.clone(),
        CoreRenderable::Blocks { blocks } => meerkat_core::types::text_content(blocks),
        CoreRenderable::Json { value } => {
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
        }
        CoreRenderable::Reference { uri, label } => match label {
            Some(label) if !label.trim().is_empty() => format!("[Reference] {label} ({uri})"),
            _ => format!("[Reference] {uri}"),
        },
        _ => String::new(),
    }
}

fn pending_system_context_appends(
    appends: &[ConversationContextAppend],
) -> Vec<PendingSystemContextAppend> {
    let accepted_at = meerkat_core::time_compat::SystemTime::now();
    appends
        .iter()
        .map(|append| PendingSystemContextAppend {
            text: render_context_append_text(&append.content),
            source: Some(append.key.clone()),
            idempotency_key: Some(append.key.clone()),
            accepted_at,
        })
        .collect()
}

fn render_runtime_system_context_message(append: &PendingSystemContextAppend) -> Message {
    let mut content = String::from("[Runtime System Context]");
    if let Some(source) = &append.source {
        content.push_str("\nsource: ");
        content.push_str(source);
    }
    content.push_str("\n\n");
    content.push_str(&append.text);
    Message::System(SystemMessage { content })
}

fn realtime_projection_root_system_message(session: &Session) -> Option<Message> {
    let build_state = session.build_state().unwrap_or_default();
    let mut content = build_state
        .system_prompt
        .or_else(|| {
            session
                .messages()
                .first()
                .and_then(|message| match message {
                    Message::System(system) => Some(system.content.clone()),
                    Message::SystemNotice(notice) => Some(notice.rendered_text()),
                    _ => None,
                })
        })
        .unwrap_or_default();

    if let Some(additional_instructions) = build_state.additional_instructions
        && !additional_instructions.is_empty()
    {
        if !content.trim().is_empty() {
            content.push_str("\n\n");
        }
        content.push_str("[Session Build Instructions]");
        for instruction in additional_instructions {
            let instruction = instruction.trim();
            if instruction.is_empty() {
                continue;
            }
            content.push_str("\n- ");
            content.push_str(instruction);
        }
    }

    if content.trim().is_empty() {
        None
    } else {
        Some(Message::System(SystemMessage { content }))
    }
}

fn realtime_projection_messages(session: &Session) -> Vec<Message> {
    let mut projected = session.messages().to_vec();
    if let Some(root_system) = realtime_projection_root_system_message(session) {
        match projected.first() {
            Some(Message::System(_) | Message::SystemNotice(_)) => projected[0] = root_system,
            _ => projected.insert(0, root_system),
        }
    }
    let pending = session.system_context_state().unwrap_or_default().pending;
    if !pending.is_empty() {
        // Projection-only and rebuildable:
        // runtime-owned system-context appends are canonical in live session
        // metadata, not in transcript messages. For provider reconstruction we
        // preserve those appends as explicit synthetic system history items
        // instead of burying them inside the root instruction string. That
        // keeps async runtime facts closer to the way the local runtime applies
        // them: authoritative, replayable, and distinct from the base prompt.
        projected.extend(pending.iter().map(render_runtime_system_context_message));
    }
    projected
}

#[cfg(test)]
fn exported_tool_visibility_state(session: &Session) -> meerkat_core::SessionToolVisibilityState {
    session.tool_visibility_state().unwrap_or_default()
}

fn session_error_to_runtime_driver(err: SessionError) -> RuntimeDriverError {
    match err {
        SessionError::NotFound { .. } => RuntimeDriverError::NotReady {
            state: meerkat_runtime::RuntimeState::Destroyed,
        },
        other => RuntimeDriverError::Internal(other.to_string()),
    }
}

fn runtime_driver_error_to_rpc(err: RuntimeDriverError) -> RpcError {
    match err {
        RuntimeDriverError::ValidationFailed { reason } => RpcError {
            code: error::INVALID_PARAMS,
            message: reason,
            data: None,
        },
        RuntimeDriverError::NotReady { state } => RpcError {
            code: error::INVALID_REQUEST,
            message: format!("runtime not ready for llm reconfiguration: {state}"),
            data: None,
        },
        RuntimeDriverError::Destroyed => RpcError {
            code: error::SESSION_NOT_FOUND,
            message: "runtime destroyed".to_string(),
            data: None,
        },
        RuntimeDriverError::Internal(message) => RpcError {
            code: error::INTERNAL_ERROR,
            message,
            data: None,
        },
        other => RpcError {
            code: error::INTERNAL_ERROR,
            message: other.to_string(),
            data: None,
        },
    }
}

fn profile_to_capability_surface(
    profile: &meerkat_models::profile::ModelProfile,
) -> SessionLlmCapabilitySurface {
    SessionLlmCapabilitySurface {
        supports_temperature: profile.supports_temperature,
        supports_thinking: profile.supports_thinking,
        supports_reasoning: profile.supports_reasoning,
        inline_video: profile.inline_video,
        vision: profile.vision,
        image_tool_results: profile.image_tool_results,
        supports_web_search: profile.supports_web_search,
        realtime: profile.realtime,
        call_timeout_secs: profile.call_timeout_secs,
    }
}

#[derive(Clone)]
struct SessionRuntimeLlmReconfigureHost {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    factory: AgentFactory,
    default_llm_client: Arc<StdRwLock<Option<Arc<dyn LlmClient>>>>,
    config_runtime: Arc<StdRwLock<Option<Arc<meerkat_core::ConfigRuntime>>>>,
}

impl SessionRuntimeLlmReconfigureHost {
    async fn model_registry(&self) -> Result<meerkat_core::ModelRegistry, RuntimeDriverError> {
        let config_runtime = self
            .config_runtime
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let config = if let Some(runtime) = config_runtime {
            runtime
                .get()
                .await
                .map(|snapshot| snapshot.config)
                .map_err(|e| RuntimeDriverError::Internal(format!("Failed to load config: {e}")))?
        } else {
            meerkat_core::Config::default()
        };

        config.model_registry().map_err(|e| {
            RuntimeDriverError::Internal(format!("Failed to resolve model registry: {e}"))
        })
    }

    async fn build_adapter_for_llm_identity(
        &self,
        identity: &SessionLlmIdentity,
    ) -> Result<Arc<dyn meerkat_core::AgentLlmClient>, RuntimeDriverError> {
        let default_llm_client = self
            .default_llm_client
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let raw_client = if let Some(default) = default_llm_client {
            default
        } else {
            let config_runtime = self
                .config_runtime
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let config = if let Some(runtime) = config_runtime {
                runtime
                    .get()
                    .await
                    .map(|snapshot| snapshot.config)
                    .map_err(|e| {
                        RuntimeDriverError::Internal(format!(
                            "Failed to load config for hot-swap: {e}"
                        ))
                    })?
            } else {
                meerkat_core::Config::default()
            };
            self.factory
                .build_llm_client_for_identity(&config, identity)
                .await
                .map_err(|e| {
                    RuntimeDriverError::Internal(format!(
                        "Failed to build LLM client for session identity hot-swap: {e}"
                    ))
                })?
        };

        let adapter = self
            .factory
            .build_llm_adapter(raw_client, identity.model.clone())
            .await;
        // Post-wave-a: `SessionLlmIdentity.provider_params` stays stringly
        // typed (`serde_json::Value`) while the adapter seam expects a typed
        // `ProviderTag`. The hot-swap path no longer carries provider_params
        // overlay through this seam until the typed projector is plumbed.
        let _ = identity.provider_params.clone();
        Ok(Arc::new(adapter))
    }

    async fn resolve_target_llm_identity(
        &self,
        current: &SessionLlmIdentity,
        request: &SessionLlmReconfigureRequest,
    ) -> Result<SessionLlmIdentity, RuntimeDriverError> {
        if request.provider.is_some() && request.model.is_none() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "provider override requires model on an existing session".to_string(),
            });
        }

        let registry = self.model_registry().await?;
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| current.model.clone());
        let provider = if let Some(provider_name) = request.provider.as_ref() {
            meerkat_core::Provider::from_name(provider_name)
        } else if request.model.is_some() {
            registry
                .entry(&model)
                .map(|entry| entry.provider)
                .or_else(|| meerkat_core::Provider::infer_from_model(&model))
                .unwrap_or(current.provider)
        } else {
            current.provider
        };
        let provider_params = request
            .provider_params
            .clone()
            .or_else(|| current.provider_params.clone());
        let self_hosted_server_id = if provider == meerkat_core::Provider::SelfHosted {
            if request.model.is_none() {
                current.self_hosted_server_id.clone().or_else(|| {
                    registry
                        .entry(&model)
                        .and_then(|entry| entry.self_hosted.as_ref())
                        .map(|server| server.server_id.clone())
                })
            } else {
                match registry.entry(&model) {
                    Some(entry) => entry
                        .self_hosted
                        .as_ref()
                        .map(|server| server.server_id.clone()),
                    None => {
                        return Err(RuntimeDriverError::ValidationFailed {
                            reason: format!(
                                "self-hosted provider requires a registered model alias; '{model}' is not configured"
                            ),
                        });
                    }
                }
            }
        } else {
            None
        };

        // Dogma §10 inherit/set: `None` on the request preserves the
        // session's existing binding, `Some(...)` sets a new one
        // explicitly for this hot-swap. Prevents cross-realm credential
        // bleed in multi-tenant swaps.
        let connection_ref = request
            .connection_ref
            .clone()
            .or_else(|| current.connection_ref.clone());

        Ok(SessionLlmIdentity {
            model,
            provider,
            self_hosted_server_id,
            provider_params,
            connection_ref,
        })
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SessionLlmReconfigureHost for SessionRuntimeLlmReconfigureHost {
    async fn hydrate_session_llm_state(
        &self,
        session_id: &SessionId,
    ) -> Result<HydratedSessionLlmState, RuntimeDriverError> {
        let current_identity = self
            .service
            .live_session_llm_identity(session_id)
            .await
            .map_err(session_error_to_runtime_driver)?;
        let session = self
            .service
            .export_live_session(session_id)
            .await
            .map_err(session_error_to_runtime_driver)?;
        let current_visibility_state = session.tool_visibility_state().unwrap_or_default();
        let base_tool_names = self
            .service
            .tool_scope_snapshot(session_id)
            .await
            .map_err(session_error_to_runtime_driver)?
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "session {session_id} missing live tool scope snapshot during llm reconfiguration"
                ))
            })?
            .known_base_names
            .into_iter()
            .collect();

        let registry = self.model_registry().await?;
        let (current_capability_surface, capability_surface_status) =
            match registry.profile_for(&current_identity.model) {
                Some(profile) => (
                    Some(profile_to_capability_surface(&profile)),
                    SessionLlmCapabilitySurfaceStatus::Resolved,
                ),
                None => (None, SessionLlmCapabilitySurfaceStatus::Unresolved),
            };

        Ok(HydratedSessionLlmState {
            current_identity,
            current_visibility_state,
            current_capability_surface,
            capability_surface_status,
            base_tool_names,
        })
    }

    async fn resolve_target_session_llm_identity(
        &self,
        request: &SessionLlmReconfigureRequest,
        current_identity: &SessionLlmIdentity,
    ) -> Result<ResolvedSessionLlmReconfigure, RuntimeDriverError> {
        let target_identity = self
            .resolve_target_llm_identity(current_identity, request)
            .await?;
        let registry = self.model_registry().await?;
        let profile = registry
            .profile_for(&target_identity.model)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "no capability profile is registered for model '{}'",
                    target_identity.model
                ),
            })?;

        Ok(ResolvedSessionLlmReconfigure {
            target_identity,
            target_capability_surface: profile_to_capability_surface(&profile),
        })
    }

    async fn apply_live_session_llm_identity(
        &self,
        session_id: &SessionId,
        identity: &SessionLlmIdentity,
    ) -> Result<(), RuntimeDriverError> {
        let adapter = self.build_adapter_for_llm_identity(identity).await?;
        self.service
            .apply_runtime_session_llm_identity(session_id, adapter, identity.clone())
            .await
            .map_err(session_error_to_runtime_driver)
    }

    async fn apply_live_session_tool_visibility_state(
        &self,
        session_id: &SessionId,
        visibility_state: Option<meerkat_core::SessionToolVisibilityState>,
    ) -> Result<(), RuntimeDriverError> {
        self.service
            .set_session_tool_visibility_state(session_id, visibility_state)
            .await
            .map_err(session_error_to_runtime_driver)
    }

    async fn persist_live_session(&self, session_id: &SessionId) -> Result<(), RuntimeDriverError> {
        self.service
            .persist_live_session_now(session_id)
            .await
            .map(|_| ())
            .map_err(session_error_to_runtime_driver)
    }

    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), RuntimeDriverError> {
        self.service
            .discard_live_session(session_id)
            .await
            .map_err(session_error_to_runtime_driver)
    }
}

#[derive(Clone)]
struct SkillIdentityRegistryState {
    generation: u64,
    registry: SourceIdentityRegistry,
}

#[cfg(feature = "mcp")]
struct SessionMcpState {
    adapter: Arc<McpRouterAdapter>,
    turn_counter: u32,
    lifecycle_tx: mpsc::UnboundedSender<McpLifecycleAction>,
    lifecycle_rx: mpsc::UnboundedReceiver<McpLifecycleAction>,
    drain_task_running: Arc<AtomicBool>,
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Observable state of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// The session is idle and ready to accept a new turn.
    Idle,
    /// A turn is currently running.
    Running,
    /// The session is shutting down.
    ShuttingDown,
}

impl SessionState {
    /// Return a stable string representation matching the serde `rename_all` convention.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::ShuttingDown => "shutting_down",
        }
    }
}

/// Summary information about a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub session_id: SessionId,
    pub state: SessionState,
    pub labels: BTreeMap<String, String>,
}

fn now_unix_secs() -> u64 {
    meerkat_core::time_compat::SystemTime::now()
        .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Merge a deferred prompt with a turn prompt, preserving multimodal content.
/// If both are plain text, concatenates with `\n\n`. Otherwise, converts both
/// to content blocks and concatenates (deferred blocks first, then turn blocks).
fn merge_content_inputs(deferred: ContentInput, turn: ContentInput) -> ContentInput {
    match (&deferred, &turn) {
        (ContentInput::Text(d), ContentInput::Text(t)) => ContentInput::Text(format!("{d}\n\n{t}")),
        _ => {
            let mut blocks = deferred.into_blocks();
            blocks.extend(turn.into_blocks());
            ContentInput::Blocks(blocks)
        }
    }
}

// FactoryAgent and FactoryAgentBuilder are imported from meerkat::service_factory.
// Staged-session authority lives in meerkat::StagedSessionRegistry.

// ---------------------------------------------------------------------------
// SessionRuntime
// ---------------------------------------------------------------------------

/// Core runtime that manages agent sessions.
///
/// Wraps [`PersistentSessionService`] for session lifecycle management while
/// preserving the two-step create-then-run API required by JSON-RPC handlers.
pub struct SessionRuntime {
    factory: AgentFactory,
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    schedule_service: ScheduleService,
    schedule_host: Mutex<Option<meerkat::surface::ScheduleHostHandle>>,
    /// Canonical staged-session authority (facade-owned). Holds sessions
    /// that have been created (ID returned to caller) but not yet materialized
    /// in the service. The first `start_turn` call promotes them through
    /// `staged_sessions.begin_promotion()`.
    staged_sessions: Arc<StagedSessionRegistry>,
    max_sessions: usize,
    /// Override LLM client for all sessions (primarily for testing).
    default_llm_client: Arc<StdRwLock<Option<Arc<dyn LlmClient>>>>,
    realm_id: Option<meerkat_core::connection::RealmId>,
    instance_id: Option<String>,
    backend: Option<String>,
    config_runtime: Arc<StdRwLock<Option<Arc<meerkat_core::ConfigRuntime>>>>,
    runtime_adapter: Arc<MeerkatMachine>,
    /// Notification sink for event forwarding to the RPC transport.
    /// Wrapped in `RwLock` so it can be updated when a new TCP client
    /// connects (each connection has its own transport sink).
    notification_sink: StdRwLock<crate::router::NotificationSink>,
    skill_identity_registry: Arc<StdRwLock<SkillIdentityRegistryState>>,
    skill_identity_context_root: Arc<StdRwLock<Option<PathBuf>>>,
    skill_identity_user_root: Arc<StdRwLock<Option<PathBuf>>>,
    #[cfg(feature = "mob")]
    mob_state: StdRwLock<Option<Arc<meerkat_mob_mcp::MobMcpState>>>,
    #[cfg(feature = "mcp")]
    mcp_sessions: RwLock<std::collections::HashMap<SessionId, SessionMcpState>>,
    /// Channel for sending callback tool requests to the RPC server loop.
    /// Wrapped in `RwLock` so it can be set after Arc wrapping (server construction).
    #[allow(clippy::type_complexity)]
    callback_request_tx: StdRwLock<
        Option<
            mpsc::Sender<(
                crate::protocol::RpcRequest,
                tokio::sync::oneshot::Sender<crate::protocol::RpcResponse>,
            )>,
        >,
    >,
    /// Counter for generating unique server-originated callback request IDs.
    callback_id_counter_slot: StdRwLock<Arc<std::sync::atomic::AtomicU64>>,
    /// Globally registered callback tool definitions (via `tools/register`).
    registered_tools_slot: StdRwLock<Arc<StdRwLock<Vec<meerkat_core::ToolDef>>>>,
    /// Handle to the builder's mob tools slot inside the session service.
    /// Captured before the builder is consumed so `set_mob_tools` can write
    /// through to the actual builder that creates agents.
    pub builder_mob_tools_slot:
        Arc<StdRwLock<Option<Arc<dyn meerkat_core::service::MobToolsFactory>>>>,
    /// Captured before the builder is consumed so runtime construction can
    /// inject scheduler tools into resumed/runtime-backed agent builds.
    pub builder_schedule_tools_slot:
        Arc<StdRwLock<Option<Arc<dyn meerkat_core::AgentToolDispatcher>>>>,
}

fn session_metadata_marks_archived(session: &Session) -> bool {
    session
        .metadata()
        .get("session_archived")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

impl SessionRuntime {
    fn turn_keep_alive_policy(
        requested: Option<bool>,
    ) -> Option<meerkat_core::lifecycle::run_primitive::KeepAlivePolicy> {
        requested.and_then(|keep_alive| {
            keep_alive.then(|| meerkat_core::lifecycle::run_primitive::KeepAlivePolicy {
                ttl: std::time::Duration::from_secs(30),
                policy: meerkat_core::lifecycle::run_primitive::KeepAliveMode::Pinned,
            })
        })
    }

    fn turn_additional_instructions(
        requested: Option<Vec<String>>,
    ) -> Option<Vec<meerkat_core::lifecycle::run_primitive::TurnInstruction>> {
        requested.map(|instructions| {
            instructions
                .into_iter()
                .map(
                    |body| meerkat_core::lifecycle::run_primitive::TurnInstruction {
                        kind: meerkat_core::lifecycle::run_primitive::TurnInstructionKind::User,
                        body,
                    },
                )
                .collect()
        })
    }

    fn turn_metadata_from_overrides(
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
        additional_instructions: Option<Vec<String>>,
        overrides: Option<&crate::handlers::turn::TurnOverrides>,
    ) -> Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata> {
        let metadata = meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
            handling_mode: None,
            keep_alive: overrides.and_then(|ov| Self::turn_keep_alive_policy(ov.keep_alive)),
            skill_references,
            flow_tool_overlay,
            additional_instructions: Self::turn_additional_instructions(additional_instructions),
            model: overrides
                .and_then(|ov| ov.model.clone())
                .map(meerkat_core::lifecycle::run_primitive::ModelId::new),
            provider: overrides
                .and_then(|ov| ov.provider.as_ref())
                .map(|provider| meerkat_core::Provider::from_name(provider)),
            provider_params: None,
            render_metadata: None,
            execution_kind: None,
            connection_ref: overrides.and_then(|ov| ov.connection_ref.clone()),
        };
        (!metadata.is_empty()).then_some(metadata)
    }

    pub(crate) fn turn_overrides_from_metadata(
        metadata: Option<&meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata>,
    ) -> Option<crate::handlers::turn::TurnOverrides> {
        let metadata = metadata?;
        let overrides = crate::handlers::turn::TurnOverrides {
            keep_alive: metadata.keep_alive.as_ref().map(|_| true),
            model: metadata.model.as_ref().map(ToString::to_string),
            provider: metadata
                .provider
                .map(|provider| provider.as_str().to_string()),
            provider_params: None,
            connection_ref: metadata.connection_ref.clone(),
            ..Default::default()
        };
        (!overrides.is_empty()).then_some(overrides)
    }

    #[cfg(feature = "comms")]
    async fn preserve_existing_peer_ingress(
        &self,
        session_id: &SessionId,
        requested_keep_alive: bool,
    ) -> bool {
        requested_keep_alive || self.runtime_adapter.session_has_comms(session_id).await
    }

    async fn live_session_is_stale(&self, session_id: &SessionId) -> Result<bool, RpcError> {
        let live = match self.service.export_live_session(session_id).await {
            Ok(session) => session,
            Err(SessionError::NotFound { .. }) => return Ok(false),
            Err(err) => return Err(session_error_to_rpc(err)),
        };
        let Some(stored) = self.load_persisted_session(session_id).await? else {
            return Ok(false);
        };
        Ok(stored.updated_at() > live.updated_at()
            || (stored.updated_at() == live.updated_at()
                && stored.messages().len() > live.messages().len())
            || stored_has_unapplied_system_context(&stored, &live))
    }

    /// Create a new session runtime.
    pub fn new(
        factory: AgentFactory,
        config: Config,
        max_sessions: usize,
        persistence: PersistenceBundle,
        notification_sink: crate::router::NotificationSink,
    ) -> Self {
        let schedule_service = ScheduleService::new(persistence.schedule_store());
        let factory_clone = factory.clone();
        let builder = FactoryAgentBuilder::new(factory, config);
        let builder_mob_tools_slot = Arc::clone(&builder.default_mob_tools);
        let builder_schedule_tools_slot = Arc::clone(&builder.default_schedule_tools);
        let default_llm_client = Arc::new(StdRwLock::new(None));
        let config_runtime = Arc::new(StdRwLock::new(None));
        meerkat::surface::set_default_schedule_tools(
            &builder,
            Some(Arc::new(ScheduleToolDispatcher::new(
                schedule_service.clone(),
            ))),
        );
        let (service, runtime_adapter) =
            meerkat::surface::build_runtime_backed_service(builder, max_sessions, persistence);
        let service = Arc::new(service);
        runtime_adapter.set_session_llm_reconfigure_host(Arc::new(
            SessionRuntimeLlmReconfigureHost {
                service: Arc::clone(&service),
                factory: factory_clone.clone(),
                default_llm_client: Arc::clone(&default_llm_client),
                config_runtime: Arc::clone(&config_runtime),
            },
        ));

        Self {
            factory: factory_clone,
            service,
            schedule_service,
            schedule_host: Mutex::new(None),
            staged_sessions: Arc::new(StagedSessionRegistry::new()),
            max_sessions,
            default_llm_client,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_runtime,
            runtime_adapter,
            notification_sink: StdRwLock::new(notification_sink),
            skill_identity_registry: Arc::new(StdRwLock::new(SkillIdentityRegistryState {
                generation: 0,
                registry: SourceIdentityRegistry::default(),
            })),
            skill_identity_context_root: Arc::new(StdRwLock::new(None)),
            skill_identity_user_root: Arc::new(StdRwLock::new(None)),
            #[cfg(feature = "mob")]
            mob_state: StdRwLock::new(None),
            #[cfg(feature = "mcp")]
            mcp_sessions: RwLock::new(std::collections::HashMap::new()),
            callback_request_tx: StdRwLock::new(None),
            callback_id_counter_slot: StdRwLock::new(Arc::new(std::sync::atomic::AtomicU64::new(
                0,
            ))),
            registered_tools_slot: StdRwLock::new(Arc::new(StdRwLock::new(Vec::new()))),
            builder_mob_tools_slot,
            builder_schedule_tools_slot,
        }
    }

    /// Create a runtime that resolves config from a shared config store.
    pub fn new_with_config_store(
        factory: AgentFactory,
        initial_config: Config,
        config_store: Arc<dyn ConfigStore>,
        max_sessions: usize,
        persistence: PersistenceBundle,
        notification_sink: crate::router::NotificationSink,
    ) -> Self {
        let schedule_service = ScheduleService::new(persistence.schedule_store());
        let factory_clone = factory.clone();
        let builder =
            FactoryAgentBuilder::new_with_config_store(factory, initial_config, config_store);
        let builder_mob_tools_slot = Arc::clone(&builder.default_mob_tools);
        let builder_schedule_tools_slot = Arc::clone(&builder.default_schedule_tools);
        let default_llm_client = Arc::new(StdRwLock::new(None));
        let config_runtime = Arc::new(StdRwLock::new(None));
        meerkat::surface::set_default_schedule_tools(
            &builder,
            Some(Arc::new(ScheduleToolDispatcher::new(
                schedule_service.clone(),
            ))),
        );
        let (service, runtime_adapter) =
            meerkat::surface::build_runtime_backed_service(builder, max_sessions, persistence);
        let service = Arc::new(service);
        runtime_adapter.set_session_llm_reconfigure_host(Arc::new(
            SessionRuntimeLlmReconfigureHost {
                service: Arc::clone(&service),
                factory: factory_clone.clone(),
                default_llm_client: Arc::clone(&default_llm_client),
                config_runtime: Arc::clone(&config_runtime),
            },
        ));

        Self {
            factory: factory_clone,
            service,
            schedule_service,
            schedule_host: Mutex::new(None),
            staged_sessions: Arc::new(StagedSessionRegistry::new()),
            max_sessions,
            default_llm_client,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_runtime,
            runtime_adapter,
            notification_sink: StdRwLock::new(notification_sink),
            skill_identity_registry: Arc::new(StdRwLock::new(SkillIdentityRegistryState {
                generation: 0,
                registry: SourceIdentityRegistry::default(),
            })),
            skill_identity_context_root: Arc::new(StdRwLock::new(None)),
            skill_identity_user_root: Arc::new(StdRwLock::new(None)),
            #[cfg(feature = "mob")]
            mob_state: StdRwLock::new(None),
            #[cfg(feature = "mcp")]
            mcp_sessions: RwLock::new(std::collections::HashMap::new()),
            callback_request_tx: StdRwLock::new(None),
            callback_id_counter_slot: StdRwLock::new(Arc::new(std::sync::atomic::AtomicU64::new(
                0,
            ))),
            registered_tools_slot: StdRwLock::new(Arc::new(StdRwLock::new(Vec::new()))),
            builder_mob_tools_slot,
            builder_schedule_tools_slot,
        }
    }

    /// Attach realm context defaults used for session metadata.
    pub fn set_realm_context(
        &mut self,
        realm_id: Option<meerkat_core::connection::RealmId>,
        instance_id: Option<String>,
        backend: Option<String>,
    ) {
        self.realm_id = realm_id;
        self.instance_id = instance_id;
        self.backend = backend;
    }

    pub fn set_skill_identity_roots(
        &mut self,
        context_root: Option<PathBuf>,
        user_root: Option<PathBuf>,
    ) {
        if let Ok(mut slot) = self.skill_identity_context_root.write() {
            *slot = context_root;
        }
        if let Ok(mut slot) = self.skill_identity_user_root.write() {
            *slot = user_root;
        }
    }

    pub fn skill_identity_roots(&self) -> (Option<PathBuf>, Option<PathBuf>) {
        let context_root = self
            .skill_identity_context_root
            .read()
            .map(|slot| slot.clone())
            .unwrap_or(None);
        let user_root = self
            .skill_identity_user_root
            .read()
            .map(|slot| slot.clone())
            .unwrap_or(None);
        (context_root, user_root)
    }

    /// Set the default mob tools factory for all agents built by this runtime.
    ///
    /// Writes through the shared slot to the `FactoryAgentBuilder` inside the
    /// session service — the builder that actually creates agents. The runtime's
    /// own `factory` clone is NOT used for session creation.
    pub fn set_mob_tools(&mut self, factory: Arc<dyn meerkat_core::service::MobToolsFactory>) {
        *self
            .builder_mob_tools_slot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(factory);
    }

    /// Active realm id for this runtime, if configured.
    pub fn realm_id(&self) -> Option<&meerkat_core::connection::RealmId> {
        self.realm_id.as_ref()
    }

    /// Attach config runtime for generation stamping.
    pub fn set_config_runtime(&mut self, runtime: Arc<meerkat_core::ConfigRuntime>) {
        *self
            .config_runtime
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(runtime);
    }

    /// Shared config runtime used by config handlers.
    pub fn config_runtime(&self) -> Option<Arc<meerkat_core::ConfigRuntime>> {
        self.config_runtime
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Persistent TokenStore used by OAuth-backed bindings (shared with
    /// the AgentFactory so login writes + resolve reads see the same
    /// credentials).
    pub fn token_store(&self) -> Option<Arc<dyn meerkat_providers::auth_store::TokenStore>> {
        self.factory.token_store.clone()
    }

    /// Override the shared default LLM client used by this runtime.
    pub fn set_default_llm_client(&mut self, client: Option<Arc<dyn LlmClient>>) {
        *self
            .default_llm_client
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = client;
    }

    /// Build the runtime adapter appropriate for this runtime's persistence mode.
    pub fn runtime_adapter(&self) -> Arc<MeerkatMachine> {
        self.runtime_adapter.clone()
    }

    /// Project the owning live session into the provider-backed realtime open seam.
    pub async fn realtime_session_open_config(
        &self,
        session_id: &SessionId,
        turning_mode: meerkat_contracts::RealtimeTurningMode,
    ) -> Result<RealtimeSessionOpenConfig, SessionError> {
        let session = self.service.export_live_session(session_id).await?;
        let llm_identity = self.service.live_session_llm_identity(session_id).await?;
        let visible_tools = self.service.live_visible_tool_defs(session_id).await?;
        Ok(RealtimeSessionOpenConfig::new(
            turning_mode,
            llm_identity,
            visible_tools,
            realtime_projection_messages(&session),
        ))
    }

    fn recovery_overrides_from_turn(
        &self,
        overrides: Option<&crate::handlers::turn::TurnOverrides>,
        keep_alive: bool,
    ) -> Result<SurfaceSessionRecoveryOverrides, RpcError> {
        let output_schema = match overrides.and_then(|ov| ov.output_schema.clone()) {
            Some(value) => Some(meerkat_core::OutputSchema::from_json_value(value).map_err(
                |e| RpcError {
                    code: error::INVALID_PARAMS,
                    message: format!("Invalid output_schema override: {e}"),
                    data: None,
                },
            )?),
            None => None,
        };

        Ok(SurfaceSessionRecoveryOverrides {
            model: overrides.and_then(|ov| ov.model.clone()),
            provider: overrides.and_then(|ov| {
                ov.provider
                    .as_ref()
                    .map(|provider| meerkat_core::Provider::from_name(provider))
            }),
            provider_params: overrides.and_then(|ov| ov.provider_params.clone()),
            max_tokens: overrides.and_then(|ov| ov.max_tokens),
            system_prompt: overrides.and_then(|ov| ov.system_prompt.clone()),
            output_schema,
            structured_output_retries: overrides.and_then(|ov| ov.structured_output_retries),
            keep_alive: Some(keep_alive),
            ..Default::default()
        })
    }

    fn recovery_external_tools(&self) -> Option<Arc<dyn meerkat_core::AgentToolDispatcher>> {
        let tx = self.callback_request_tx()?;
        Some(
            Arc::new(crate::callback_dispatcher::CallbackToolDispatcher::new(
                self.registered_tools(),
                tx,
                self.callback_id_counter(),
                vec![],
            )) as Arc<dyn meerkat_core::AgentToolDispatcher>,
        )
    }

    fn recovery_error_to_rpc(error: SurfaceSessionRecoveryError) -> RpcError {
        match error {
            SurfaceSessionRecoveryError::InvalidOverride(message) => RpcError {
                code: error::INVALID_PARAMS,
                message,
                data: None,
            },
            other => RpcError {
                code: error::INTERNAL_ERROR,
                message: other.to_string(),
                data: None,
            },
        }
    }

    async fn recovered_create_request(
        &self,
        session_id: &SessionId,
        session: Session,
        overrides: SurfaceSessionRecoveryOverrides,
    ) -> Result<CreateSessionRequest, RpcError> {
        let current_generation = match self.config_runtime() {
            Some(runtime) => runtime.get().await.ok().map(|snapshot| snapshot.generation),
            None => None,
        };
        let bindings = self
            .runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|e| RpcError {
                code: error::INTERNAL_ERROR,
                message: format!(
                    "failed to prepare runtime bindings for session {session_id}: {e}"
                ),
                data: None,
            })?;
        let recovered = build_recovered_session(
            session,
            &overrides,
            SurfaceSessionRecoveryContext {
                llm_client_override: self
                    .default_llm_client()
                    .map(encode_llm_client_override_for_service),
                external_tools: self.recovery_external_tools(),
                checkpointer: None,
                runtime_build_mode: Some(meerkat_core::RuntimeBuildMode::SessionOwned(bindings)),
                require_runtime_build_mode: true,
                realm_id: self.realm_id.as_ref().map(ToString::to_string),
                instance_id: self.instance_id.clone(),
                backend: self.backend.clone(),
                config_generation: current_generation,
            },
        )
        .map_err(Self::recovery_error_to_rpc)?;
        Ok(recovered.into_deferred_create_request())
    }

    pub fn schedule_service(&self) -> ScheduleService {
        self.schedule_service.clone()
    }

    pub fn blob_store(&self) -> Arc<dyn meerkat_core::BlobStore> {
        self.service.blob_store()
    }

    pub fn default_llm_client(&self) -> Option<Arc<dyn LlmClient>> {
        self.default_llm_client
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn model_registry(&self) -> Result<meerkat_core::ModelRegistry, RpcError> {
        let config = if let Some(runtime) = self.config_runtime() {
            runtime
                .get()
                .await
                .map(|snapshot| snapshot.config)
                .map_err(|e| RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!("Failed to load config: {e}"),
                    data: None,
                })?
        } else {
            meerkat_core::Config::default()
        };

        config.model_registry().map_err(|e| RpcError {
            code: error::INTERNAL_ERROR,
            message: format!("Failed to resolve model registry: {e}"),
            data: None,
        })
    }

    async fn llm_identity_from_pending_build(
        &self,
        build_config: &AgentBuildConfig,
    ) -> Result<SessionLlmIdentity, RpcError> {
        let registry = self.model_registry().await?;
        let model = build_config.model.clone();
        let provider = if let Some(provider) = build_config.provider {
            provider
        } else {
            registry
                .entry(&model)
                .map(|entry| entry.provider)
                .or_else(|| meerkat_core::Provider::infer_from_model(&model))
                .unwrap_or(meerkat_core::Provider::Other)
        };
        let self_hosted_server_id = if provider == meerkat_core::Provider::SelfHosted {
            if let Some(server_id) = build_config.self_hosted_server_id.clone() {
                Some(server_id)
            } else if let Some(metadata) = build_config
                .resume_session
                .as_ref()
                .and_then(Session::session_metadata)
            {
                metadata.self_hosted_server_id
            } else if let Some(entry) = registry.entry(&model) {
                entry
                    .self_hosted
                    .as_ref()
                    .map(|server| server.server_id.clone())
            } else {
                return Err(RpcError {
                    code: error::INVALID_PARAMS,
                    message: format!(
                        "self-hosted provider requires a registered model alias; '{model}' is not configured"
                    ),
                    data: None,
                });
            }
        } else {
            None
        };
        Ok(SessionLlmIdentity {
            model,
            provider,
            self_hosted_server_id,
            provider_params: build_config.provider_params.clone(),
            connection_ref: build_config.connection_ref.clone(),
        })
    }

    async fn provider_supports_inline_video(&self, identity: &SessionLlmIdentity) -> bool {
        self.model_registry()
            .await
            .ok()
            .and_then(|registry| registry.profile_for(&identity.model))
            .map(|profile| profile.inline_video)
            .unwrap_or(false)
    }

    async fn validate_prompt_video_input(
        &self,
        prompt: &ContentInput,
        identity: &SessionLlmIdentity,
    ) -> Result<(), RpcError> {
        let blocks = match prompt {
            ContentInput::Text(_) => return Ok(()),
            ContentInput::Blocks(blocks) => blocks,
        };

        meerkat_core::validate_inline_video_blocks(blocks).map_err(|message| RpcError {
            code: error::INVALID_PARAMS,
            message,
            data: None,
        })?;

        if meerkat_core::has_video(blocks) && !self.provider_supports_inline_video(identity).await {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: format!(
                    "inline video input is not supported by model '{}' on provider '{}'",
                    identity.model,
                    identity.provider.as_str()
                ),
                data: None,
            });
        }

        Ok(())
    }

    async fn resolve_target_llm_identity(
        &self,
        current: &SessionLlmIdentity,
        ov: &crate::handlers::turn::TurnOverrides,
    ) -> Result<SessionLlmIdentity, RpcError> {
        if ov.provider.is_some() && ov.model.is_none() {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "provider override requires model on an existing session".to_string(),
                data: None,
            });
        }

        let registry = self.model_registry().await?;
        let model = ov.model.clone().unwrap_or_else(|| current.model.clone());
        let provider = if let Some(provider_name) = ov.provider.as_ref() {
            meerkat_core::Provider::from_name(provider_name)
        } else if ov.model.is_some() {
            registry
                .entry(&model)
                .map(|entry| entry.provider)
                .or_else(|| meerkat_core::Provider::infer_from_model(&model))
                .unwrap_or(current.provider)
        } else {
            current.provider
        };
        let provider_params = ov
            .provider_params
            .clone()
            .or_else(|| current.provider_params.clone());
        let self_hosted_server_id = if provider == meerkat_core::Provider::SelfHosted {
            if ov.model.is_none() {
                current.self_hosted_server_id.clone().or_else(|| {
                    registry
                        .entry(&model)
                        .and_then(|entry| entry.self_hosted.as_ref())
                        .map(|server| server.server_id.clone())
                })
            } else {
                match registry.entry(&model) {
                    Some(entry) => entry
                        .self_hosted
                        .as_ref()
                        .map(|server| server.server_id.clone()),
                    None => {
                        return Err(RpcError {
                            code: error::INVALID_PARAMS,
                            message: format!(
                                "self-hosted provider requires a registered model alias; '{model}' is not configured"
                            ),
                            data: None,
                        });
                    }
                }
            }
        } else {
            None
        };

        // Dogma §10 inherit/set: `None` on the override preserves the
        // session's existing binding, `Some(...)` sets a new one
        // explicitly for this turn. Prevents cross-realm credential
        // bleed when the override is the only changed field.
        let connection_ref = ov
            .connection_ref
            .clone()
            .or_else(|| current.connection_ref.clone());

        Ok(SessionLlmIdentity {
            model,
            provider,
            self_hosted_server_id,
            provider_params,
            connection_ref,
        })
    }

    fn apply_llm_identity_to_build_config(
        build_config: &mut AgentBuildConfig,
        identity: &SessionLlmIdentity,
    ) {
        build_config.model = identity.model.clone();
        build_config.provider = Some(identity.provider);
        build_config.self_hosted_server_id = identity.self_hosted_server_id.clone();
        build_config.provider_params = identity.provider_params.clone();
        build_config.connection_ref = identity.connection_ref.clone();
    }

    async fn current_materialized_llm_identity(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionLlmIdentity, RpcError> {
        match self.service.live_session_llm_identity(session_id).await {
            Ok(identity) => Ok(identity),
            Err(SessionError::NotFound { .. }) => {
                let session = self
                    .load_persisted_session(session_id)
                    .await?
                    .ok_or_else(|| RpcError {
                        code: error::SESSION_NOT_FOUND,
                        message: format!("session not found: {session_id}"),
                        data: None,
                    })?;
                let metadata = session.session_metadata().ok_or_else(|| RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!(
                        "session {session_id} is missing durable llm identity metadata"
                    ),
                    data: None,
                })?;
                Ok(metadata.llm_identity())
            }
            Err(err) => Err(session_error_to_rpc(err)),
        }
    }

    async fn effective_llm_identity_for_turn(
        &self,
        session_id: &SessionId,
        overrides: Option<&crate::handlers::turn::TurnOverrides>,
    ) -> Result<SessionLlmIdentity, RpcError> {
        let pending_identity = self
            .staged_sessions
            .effective_llm_identity(session_id)
            .await;
        if let Some(pending_identity) = pending_identity {
            return match overrides {
                Some(ov) => {
                    self.resolve_target_llm_identity(&pending_identity, ov)
                        .await
                }
                None => Ok(pending_identity),
            };
        }

        let current = self.current_materialized_llm_identity(session_id).await?;
        match overrides {
            Some(ov) => self.resolve_target_llm_identity(&current, ov).await,
            None => Ok(current),
        }
    }

    fn llm_reconfigure_host(&self) -> SessionRuntimeLlmReconfigureHost {
        SessionRuntimeLlmReconfigureHost {
            service: Arc::clone(&self.service),
            factory: self.factory.clone(),
            default_llm_client: Arc::clone(&self.default_llm_client),
            config_runtime: Arc::clone(&self.config_runtime),
        }
    }

    fn committed_visibility_allows(
        base_tool_names: &std::collections::BTreeSet<String>,
        visibility_state: &meerkat_core::SessionToolVisibilityState,
        tool_name: &str,
    ) -> bool {
        if !base_tool_names.contains(tool_name) {
            return false;
        }

        meerkat_core::ToolScope::compose(&[
            visibility_state.capability_base_filter.clone(),
            visibility_state.inherited_base_filter.clone(),
            visibility_state.active_filter.clone(),
        ])
        .allows(tool_name)
    }

    fn derive_reconfigured_visibility_state(
        current: &meerkat_core::SessionToolVisibilityState,
        target_capability_surface: &SessionLlmCapabilitySurface,
        base_tool_names: &std::collections::BTreeSet<String>,
    ) -> meerkat_core::SessionToolVisibilityState {
        let current_view_image_visible = Self::committed_visibility_allows(
            base_tool_names,
            current,
            meerkat_core::VIEW_IMAGE_TOOL_NAME,
        );

        let mut next = current.clone();
        next.capability_base_filter = meerkat_core::capability_base_filter_for_image_tool_results(
            target_capability_surface.image_tool_results,
        );

        let next_view_image_visible = Self::committed_visibility_allows(
            base_tool_names,
            &next,
            meerkat_core::VIEW_IMAGE_TOOL_NAME,
        );
        if current_view_image_visible != next_view_image_visible {
            next.active_revision = current.active_revision.max(current.staged_revision) + 1;
        }

        next
    }

    async fn rollback_idle_hot_swap_failure(
        &self,
        host: &SessionRuntimeLlmReconfigureHost,
        session_id: &SessionId,
        previous_identity: &SessionLlmIdentity,
        previous_visibility_state: &meerkat_core::SessionToolVisibilityState,
        original_error: RuntimeDriverError,
    ) -> Result<(), RuntimeDriverError> {
        let rollback_result = async {
            host.apply_live_session_llm_identity(session_id, previous_identity)
                .await?;
            host.apply_live_session_tool_visibility_state(
                session_id,
                Some(previous_visibility_state.clone()),
            )
            .await?;
            Ok::<(), RuntimeDriverError>(())
        }
        .await;

        match rollback_result {
            Ok(()) => Err(original_error),
            Err(rollback_error) => {
                let _ = host.discard_live_session(session_id).await;
                Err(RuntimeDriverError::Internal(format!(
                    "failed to rollback idle live llm reconfiguration after error ({original_error}): {rollback_error}"
                )))
            }
        }
    }

    async fn hot_swap_llm_client_on_idle_session(
        &self,
        session_id: &SessionId,
        request: &SessionLlmReconfigureRequest,
    ) -> Result<(), RuntimeDriverError> {
        let host = self.llm_reconfigure_host();
        let hydrated = host.hydrate_session_llm_state(session_id).await?;
        let resolved = host
            .resolve_target_session_llm_identity(request, &hydrated.current_identity)
            .await?;
        let next_visibility_state = Self::derive_reconfigured_visibility_state(
            &hydrated.current_visibility_state,
            &resolved.target_capability_surface,
            &hydrated.base_tool_names,
        );

        host.apply_live_session_llm_identity(session_id, &resolved.target_identity)
            .await?;
        if let Err(error) = host
            .apply_live_session_tool_visibility_state(session_id, Some(next_visibility_state))
            .await
        {
            return self
                .rollback_idle_hot_swap_failure(
                    &host,
                    session_id,
                    &hydrated.current_identity,
                    &hydrated.current_visibility_state,
                    error,
                )
                .await;
        }
        if let Err(error) = host.persist_live_session(session_id).await {
            return self
                .rollback_idle_hot_swap_failure(
                    &host,
                    session_id,
                    &hydrated.current_identity,
                    &hydrated.current_visibility_state,
                    error,
                )
                .await;
        }

        Ok(())
    }

    /// Hot-swap the LLM client on a materialized session.
    ///
    /// The machine owns the semantic transition; RPC only adapts the request.
    async fn hot_swap_llm_client(
        &self,
        session_id: &SessionId,
        ov: &crate::handlers::turn::TurnOverrides,
    ) -> Result<(), RpcError> {
        let request = SessionLlmReconfigureRequest {
            model: ov.model.clone(),
            provider: ov.provider.clone(),
            provider_params: ov.provider_params.clone(),
            connection_ref: ov.connection_ref.clone(),
        };

        if !self.runtime_adapter.contains_session(session_id).await
            && self.service.read(session_id).await.is_ok()
        {
            self.runtime_adapter
                .prepare_bindings(session_id.clone())
                .await
                .map_err(|e| RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!(
                        "failed to prepare runtime bindings for live session {session_id}: {e}"
                    ),
                    data: None,
                })?;
        }

        match self
            .runtime_adapter
            .reconfigure_session_llm_identity(session_id, request.clone())
            .await
        {
            Ok(_) => Ok(()),
            Err(RuntimeDriverError::NotReady {
                state: meerkat_runtime::RuntimeState::Idle,
            }) => self
                .hot_swap_llm_client_on_idle_session(session_id, &request)
                .await
                .map_err(runtime_driver_error_to_rpc),
            Err(err) => Err(runtime_driver_error_to_rpc(err)),
        }
    }

    pub async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        self.service.discard_live_session(session_id).await
    }

    pub async fn dispatch_external_tool_call(
        &self,
        session_id: &SessionId,
        call: meerkat_core::ToolCall,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, SessionError> {
        self.service
            .dispatch_external_tool_call(session_id, call)
            .await
    }

    pub async fn append_external_user_content(
        &self,
        session_id: &SessionId,
        content: meerkat_core::types::ContentInput,
    ) -> Result<(), SessionError> {
        self.service
            .append_external_user_content(session_id, content)
            .await
    }

    pub async fn append_external_assistant_output(
        &self,
        session_id: &SessionId,
        blocks: Vec<meerkat_core::types::AssistantBlock>,
        stop_reason: meerkat_core::types::StopReason,
        usage: meerkat_core::types::Usage,
    ) -> Result<(), SessionError> {
        self.service
            .append_external_assistant_output(session_id, blocks, stop_reason, usage)
            .await
    }

    #[cfg(feature = "mob")]
    pub fn session_service(&self) -> Arc<dyn meerkat_mob::MobSessionService> {
        self.service.clone()
    }

    /// Apply runtime-owned system context appends to the canonical session
    /// transcript through the persistent session service. Used by the
    /// CoreExecutor context-only-immediate short-circuit so peer terminal
    /// responses (and other context-only primitives) land as system-context
    /// blocks rather than triggering a turn.
    pub(crate) async fn apply_runtime_context_appends_via_service(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::lifecycle::RunId,
        appends: Vec<meerkat_core::PendingSystemContextAppend>,
        contributing_input_ids: Vec<meerkat_core::lifecycle::InputId>,
    ) -> Result<
        meerkat_core::lifecycle::core_executor::CoreApplyOutput,
        meerkat_core::service::SessionError,
    > {
        self.service
            .apply_runtime_context_appends(session_id, run_id, appends, contributing_input_ids)
            .await
    }

    /// Pre-initialize the callback channel and return the receiver half.
    ///
    /// Call this before any code that reads `callback_request_tx()` (e.g.
    /// mob resume that invokes an `ExternalToolsProvider`). The server
    /// constructor that accepts a pre-created rx will reuse this channel
    /// instead of creating a new one.
    pub fn init_callback_channel(
        &self,
    ) -> mpsc::Receiver<(
        crate::protocol::RpcRequest,
        tokio::sync::oneshot::Sender<crate::protocol::RpcResponse>,
    )> {
        let (tx, rx) = mpsc::channel(crate::NOTIFICATION_CHANNEL_CAPACITY);
        let id_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let registered_tools = Arc::new(StdRwLock::new(Vec::new()));
        self.set_callback_channel(tx, id_counter, registered_tools);
        rx
    }

    /// Set the callback request channel for tool callbacks.
    ///
    /// Takes `&self` so it can be called after the runtime is wrapped in `Arc`
    /// (e.g. during `RpcServer` construction).
    pub fn set_callback_channel(
        &self,
        tx: mpsc::Sender<(
            crate::protocol::RpcRequest,
            tokio::sync::oneshot::Sender<crate::protocol::RpcResponse>,
        )>,
        id_counter: Arc<std::sync::atomic::AtomicU64>,
        registered_tools: Arc<StdRwLock<Vec<meerkat_core::ToolDef>>>,
    ) {
        if let Ok(mut slot) = self.callback_request_tx.write() {
            *slot = Some(tx);
        }
        // id_counter and registered_tools are already Arc-shared — the server
        // passes its own Arcs into the runtime at construction time. When using
        // the &self path (after Arc wrapping), we store them atomically.
        if let Ok(mut c) = self.callback_id_counter_slot.write() {
            *c = id_counter;
        }
        if let Ok(mut t) = self.registered_tools_slot.write() {
            *t = registered_tools;
        }
    }

    /// Get a clone of the callback request sender, if configured.
    pub fn callback_request_tx(
        &self,
    ) -> Option<
        mpsc::Sender<(
            crate::protocol::RpcRequest,
            tokio::sync::oneshot::Sender<crate::protocol::RpcResponse>,
        )>,
    > {
        self.callback_request_tx
            .read()
            .ok()
            .and_then(|guard| guard.clone())
    }

    /// Get the callback ID counter.
    pub fn callback_id_counter(&self) -> Arc<std::sync::atomic::AtomicU64> {
        self.callback_id_counter_slot
            .read()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// Get the globally registered callback tool definitions.
    pub fn registered_tools(&self) -> Arc<StdRwLock<Vec<meerkat_core::ToolDef>>> {
        self.registered_tools_slot
            .read()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_else(|| Arc::new(StdRwLock::new(Vec::new())))
    }

    #[cfg(feature = "mob")]
    pub fn set_mob_state(&self, mob_state: Arc<meerkat_mob_mcp::MobMcpState>) {
        if let Ok(mut slot) = self.mob_state.write() {
            *slot = Some(mob_state);
        }
    }

    #[cfg(feature = "mob")]
    pub fn mob_state(&self) -> Option<Arc<meerkat_mob_mcp::MobMcpState>> {
        self.mob_state.read().ok().and_then(|slot| slot.clone())
    }

    /// Replace the notification sink used by lazily-created session executors.
    ///
    /// Called by `serve_tcp_connection` when a new TCP client connects — each
    /// connection has its own transport writer so the sink must be updated to
    /// route events to the currently-connected client.
    /// Read the current notification sink (for use by executors at apply time).
    pub fn current_notification_sink(&self) -> crate::router::NotificationSink {
        self.notification_sink
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_notification_sink(&self, sink: crate::router::NotificationSink) {
        if let Ok(mut slot) = self.notification_sink.write() {
            *slot = sink;
        }
    }

    /// Wire an external comms runtime as the peer-ingress source for a session.
    ///
    /// This enables the session to receive incoming comms messages (peer
    /// requests/responses) between turns. The session must already be
    /// registered via `create_session` (which calls `prepare_bindings`).
    ///
    /// The comms drain starts once an executor is attached (i.e., on the first
    /// `turn/start`). Calling this before the first turn is safe — the drain
    /// context is stored and will be reconciled when the executor materializes.
    ///
    /// Used by the kennel hive agent where the comms runtime is shared at the
    /// factory level (not per-session via `comms_name`).
    #[cfg(feature = "comms")]
    pub async fn enable_comms_drain(
        self: &Arc<Self>,
        session_id: &meerkat_core::types::SessionId,
        comms_runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) {
        // Only store the comms context — don't create an executor yet.
        // The executor is created lazily on first turn/start, and it needs
        // the per-connection notification sink (not the startup noop sink).
        self.runtime_adapter
            .update_peer_ingress_context(session_id, true, Some(comms_runtime))
            .await;
    }

    pub fn set_skill_identity_registry(&self, registry: SourceIdentityRegistry) {
        if let Ok(mut slot) = self.skill_identity_registry.write() {
            slot.registry = registry;
        }
    }

    pub fn set_skill_identity_registry_for_generation(
        &self,
        generation: u64,
        registry: SourceIdentityRegistry,
    ) {
        if let Ok(mut slot) = self.skill_identity_registry.write()
            && generation >= slot.generation
        {
            slot.generation = generation;
            slot.registry = registry;
        }
    }

    async fn ensure_runtime_executor(
        self: &Arc<Self>,
        session_id: &SessionId,
    ) -> Result<(), RpcError> {
        // Check for a live executor (not just registration). Sessions
        // registered via `prepare_bindings()` exist in the adapter map but
        // have no RuntimeLoop — inputs would queue without being processed.
        if !self.runtime_adapter.session_has_executor(session_id).await {
            let session_exists = self.runtime_adapter.contains_session(session_id).await
                || self.staged_sessions.contains(session_id).await
                || self.service.read(session_id).await.is_ok()
                || self
                    .load_persisted_session(session_id)
                    .await
                    .ok()
                    .flatten()
                    .is_some();
            if !session_exists {
                return Err(RpcError {
                    code: error::SESSION_NOT_FOUND,
                    message: format!("session not found: {session_id}"),
                    data: None,
                });
            }
            #[cfg(feature = "mob")]
            let executor: Box<dyn meerkat_core::lifecycle::CoreExecutor> =
                match self.mob_state().as_ref() {
                    Some(mob_state)
                        if mob_state.owns_live_bridge_session(session_id).await
                            || mob_state.owns_persisted_bridge_session(session_id).await =>
                    {
                        let sink = self
                            .notification_sink
                            .read()
                            .ok()
                            .map(|slot| slot.clone())
                            .unwrap_or_else(crate::router::NotificationSink::noop);
                        Box::new(crate::session_executor::MobRpcRuntimeExecutor::new(
                            mob_state.session_service(),
                            session_id.clone(),
                            sink,
                        ))
                    }
                    _ => Box::new(crate::session_executor::SessionRuntimeExecutor::new(
                        Arc::clone(self),
                        session_id.clone(),
                    )),
                };
            #[cfg(not(feature = "mob"))]
            let executor: Box<dyn meerkat_core::lifecycle::CoreExecutor> =
                Box::new(crate::session_executor::SessionRuntimeExecutor::new(
                    Arc::clone(self),
                    session_id.clone(),
                ));
            self.runtime_adapter
                .ensure_session_with_executor(session_id.clone(), executor)
                .await;
        }
        Ok(())
    }

    /// W3-H: install the correct runtime executor for the session, picking
    /// between `SessionRuntimeExecutor` (standalone sessions) and
    /// `MobRpcRuntimeExecutor` (mob-owned bridge sessions). Mirrors the
    /// `MethodRouter::ensure_runtime_session_registered` logic — used by the
    /// realtime WS observer after a MobMember channel's binding rotates to a
    /// new bridge session id (no RPC path crosses, so the pre-dispatch
    /// registration gate doesn't fire; we need to register the replacement
    /// session here so the next status poll does not see NotReady/Destroyed).
    ///
    /// Returns `Ok(())` if the session is already registered, newly registered,
    /// or not found (the observer closes the channel separately on terminal
    /// retire, so "not found after rotation" surfaces as a downstream status
    /// error on the next poll tick).
    #[cfg(feature = "mob")]
    pub async fn ensure_runtime_session_for_rotation(
        self: &Arc<Self>,
        session_id: &SessionId,
    ) -> Result<(), meerkat_core::service::SessionError> {
        if self.runtime_adapter.session_has_executor(session_id).await {
            return Ok(());
        }
        let mob_state = self.mob_state();
        let executor: Box<dyn meerkat_core::lifecycle::CoreExecutor> = match mob_state.as_ref() {
            Some(mob_state)
                if mob_state.owns_live_bridge_session(session_id).await
                    || mob_state.owns_persisted_bridge_session(session_id).await =>
            {
                let sink = self
                    .notification_sink
                    .read()
                    .ok()
                    .map(|slot| slot.clone())
                    .unwrap_or_else(crate::router::NotificationSink::noop);
                Box::new(crate::session_executor::MobRpcRuntimeExecutor::new(
                    mob_state.session_service(),
                    session_id.clone(),
                    sink,
                ))
            }
            _ => Box::new(crate::session_executor::SessionRuntimeExecutor::new(
                Arc::clone(self),
                session_id.clone(),
            )),
        };
        self.runtime_adapter
            .ensure_session_with_executor(session_id.clone(), executor)
            .await;
        Ok(())
    }

    /// Start a turn by routing through the runtime input/completion waiter path.
    ///
    /// Instead of calling `SessionService::start_turn()` directly, this method:
    /// 1. Creates an `Input::Prompt` from the request parameters
    /// 2. Accepts it via `MeerkatMachine::accept_input_with_completion()`
    /// 3. Awaits the completion handle
    /// 4. Returns the `RunResult` produced by the executor
    ///
    /// This ensures all session-driving work flows through the single runtime
    /// authority (the RuntimeLoop → CoreExecutor pipeline).
    #[allow(clippy::too_many_arguments, unused_variables)]
    pub async fn start_turn_via_runtime(
        self: &Arc<Self>,
        session_id: &SessionId,
        prompt: ContentInput,
        mcp_event_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
        additional_instructions: Option<Vec<String>>,
        overrides: Option<crate::handlers::turn::TurnOverrides>,
    ) -> Result<RunResult, RpcError> {
        use meerkat_runtime::accept::AcceptOutcome;
        use meerkat_runtime::input::{Input, PromptInput};
        #[allow(unused_mut)]
        let mut prompt = prompt;
        let effective_identity = self
            .effective_llm_identity_for_turn(session_id, overrides.as_ref())
            .await?;
        self.validate_prompt_video_input(&prompt, &effective_identity)
            .await?;

        if self.live_session_is_stale(session_id).await? {
            let _ = self.service.discard_live_session(session_id).await;
            self.runtime_adapter.unregister_session(session_id).await;
        }

        // Apply MCP boundary before building the input — staged MCP ops
        // (from mcp/add, mcp/remove, mcp/reload) must be connected and made
        // visible to the agent before the turn starts.
        #[cfg(feature = "mcp")]
        {
            let mut mcp_text = String::new();
            self.apply_mcp_boundary(session_id, &mcp_event_tx, &mut mcp_text)
                .await?;
            if !mcp_text.is_empty() {
                let mut blocks = prompt.into_blocks();
                blocks.insert(
                    0,
                    meerkat_core::types::ContentBlock::Text { text: mcp_text },
                );
                prompt = ContentInput::Blocks(blocks);
            }
        }

        // Reject build-only overrides that cannot be applied via runtime turn
        // metadata. These fields only apply during pending→materialized
        // promotion (handled by the legacy start_turn path). Silently
        // dropping them would violate surface contract.
        if let Some(ref ov) = overrides {
            let rejected = [
                ov.max_tokens.map(|_| "max_tokens"),
                ov.system_prompt.as_ref().map(|_| "system_prompt"),
                ov.output_schema.as_ref().map(|_| "output_schema"),
                ov.structured_output_retries
                    .map(|_| "structured_output_retries"),
            ];
            let rejected: Vec<&str> = rejected.into_iter().flatten().collect();
            if !rejected.is_empty() {
                return Err(RpcError {
                    code: error::INVALID_PARAMS,
                    message: format!(
                        "Cannot override {} on a runtime-routed turn; \
                         set these at session/create time or use a deferred session",
                        rejected.join(", ")
                    ),
                    data: None,
                });
            }
        }

        let turn_metadata = Self::turn_metadata_from_overrides(
            skill_references,
            flow_tool_overlay,
            additional_instructions,
            overrides.as_ref(),
        );

        let input = Input::Prompt(PromptInput::from_content_input(prompt, turn_metadata));

        self.ensure_runtime_executor(session_id).await?;

        // Manage comms drain lifecycle based on keep_alive override.
        #[cfg(feature = "comms")]
        {
            let keep_alive_override = overrides.as_ref().and_then(|ov| ov.keep_alive);
            let keep_alive = match keep_alive_override {
                Some(val) => val,
                None => self
                    .load_persisted_session(session_id)
                    .await?
                    .and_then(|s| s.session_metadata().map(|m| m.keep_alive))
                    .unwrap_or(false),
            };
            let comms_rt = self.service.comms_runtime(session_id).await;
            if keep_alive && comms_rt.is_none() {
                // Check if the runtime adapter already has comms configured
                // for this session (e.g., via enable_comms_drain). If so,
                // the session-service comms check is not authoritative.
                let adapter_has_comms = self.runtime_adapter.session_has_comms(session_id).await;
                if !adapter_has_comms {
                    return Err(RpcError {
                        code: error::INVALID_PARAMS,
                        message: "keep_alive requires a session created with comms_name"
                            .to_string(),
                        data: None,
                    });
                }
            }
            // Persist explicit override so subsequent inheriting calls observe it.
            if keep_alive_override.is_some() {
                self.service
                    .apply_runtime_session_keep_alive(session_id, keep_alive)
                    .await
                    .map_err(session_error_to_rpc)?;
            }
            // W2-G: never reconfigure a mob-owned drain from the session-runtime
            // turn-start path. The mob provisioner owns peer-ingress for its
            // members; the session runtime has no visibility into the mob's
            // comms context and must not substitute its own (this is the
            // s71 silent-downgrade class). The DSL's
            // `AttachSessionIngress` guard would already reject the swap,
            // but we short-circuit here so the turn-start path doesn't emit
            // a spurious WARN for every turn on every mob-owned member.
            let owner = self.runtime_adapter.peer_ingress_owner(session_id).await;
            if owner.is_mob_owned() {
                tracing::debug!(
                    %session_id,
                    ?owner,
                    "start_turn_via_runtime: mob-owned peer ingress — skipping drain reconfigure"
                );
            } else {
                let peer_ingress_enabled = self
                    .preserve_existing_peer_ingress(session_id, keep_alive)
                    .await;
                // Preserve an already-active peer ingress channel for
                // externally-enabled sessions even when the persisted
                // keep_alive bit is false. Without this, ordinary turn
                // routing can accidentally tear down the persistent comms
                // drain that autonomous peers rely on for between-turn
                // responses.
                if comms_rt.is_some() || peer_ingress_enabled {
                    self.runtime_adapter
                        .update_peer_ingress_context(session_id, peer_ingress_enabled, comms_rt)
                        .await;
                }
            }
        }

        let (outcome, handle) = self
            .runtime_adapter
            .accept_input_with_completion(session_id, input)
            .await
            .map_err(|e| RpcError {
                code: error::INTERNAL_ERROR,
                message: format!("runtime accept failed: {e}"),
                data: None,
            })?;
        // Forward events while waiting for completion
        // (Events are forwarded by the executor's forwarder task,
        // which is spawned inside SessionRuntimeExecutor::apply())

        let Some(handle) = handle else {
            // Input already terminal (dedup of completed input)
            let existing_id = match outcome {
                AcceptOutcome::Deduplicated { existing_id, .. } => existing_id.to_string(),
                _ => "unknown".to_string(),
            };
            return Err(RpcError {
                code: error::DUPLICATE_INPUT,
                message: "input already processed".to_string(),
                data: Some(serde_json::json!({ "existing_id": existing_id })),
            });
        };

        completion_outcome_to_rpc_result(handle.wait().await, session_id)
    }

    /// Admit a canonical external event through the runtime-backed path.
    pub async fn accept_external_event_via_runtime(
        self: &Arc<Self>,
        session_id: &SessionId,
        event_type: String,
        payload: serde_json::Value,
        blocks: Option<Vec<meerkat_contracts::WireContentBlock>>,
    ) -> Result<meerkat_runtime::AcceptOutcome, RpcError> {
        use meerkat_runtime::input::{
            ExternalEventInput, Input, InputDurability, InputHeader, InputOrigin, InputVisibility,
        };

        if self.live_session_is_stale(session_id).await? {
            let _ = self.service.discard_live_session(session_id).await;
            self.runtime_adapter.unregister_session(session_id).await;
        }

        self.ensure_runtime_executor(session_id).await?;

        if event_type.trim().is_empty() {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "event_type cannot be empty".to_string(),
                data: None,
            });
        }

        let blocks = decode_wire_content_blocks(blocks)?;

        let input = Input::ExternalEvent(ExternalEventInput {
            header: InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: InputOrigin::External {
                    source_name: event_type.clone(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type,
            payload,
            blocks,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        });

        self.runtime_adapter
            .accept_input_without_wake(session_id, input)
            .await
            .map_err(|e| RpcError {
                code: error::INTERNAL_ERROR,
                message: format!("runtime accept failed: {e}"),
                data: None,
            })
    }

    /// Admit a typed correlated terminal peer response through the
    /// runtime-backed path.
    pub async fn accept_peer_response_terminal_via_runtime(
        self: &Arc<Self>,
        session_id: &SessionId,
        peer_name: meerkat_core::comms::PeerName,
        request_id: String,
        status: meerkat_contracts::PeerResponseTerminalStatusWire,
        result: serde_json::Value,
    ) -> Result<meerkat_runtime::AcceptOutcome, RpcError> {
        use meerkat_runtime::input::{
            Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PeerConvention,
            PeerInput, ResponseTerminalStatus,
        };

        if self.live_session_is_stale(session_id).await? {
            let _ = self.service.discard_live_session(session_id).await;
            self.runtime_adapter.unregister_session(session_id).await;
        }

        self.ensure_runtime_executor(session_id).await?;

        let peer_name = meerkat_core::comms::PeerName::new(peer_name.as_str().to_string())
            .map_err(|message| RpcError {
                code: error::INVALID_PARAMS,
                message,
                data: None,
            })?;

        if request_id.trim().is_empty() {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "request_id cannot be empty".to_string(),
                data: None,
            });
        }

        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: peer_name.as_string(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::ResponseTerminal {
                request_id,
                status: match status {
                    meerkat_contracts::PeerResponseTerminalStatusWire::Completed => {
                        ResponseTerminalStatus::Completed
                    }
                    meerkat_contracts::PeerResponseTerminalStatusWire::Failed => {
                        ResponseTerminalStatus::Failed
                    }
                    meerkat_contracts::PeerResponseTerminalStatusWire::Cancelled => {
                        ResponseTerminalStatus::Cancelled
                    }
                },
            }),
            body: String::new(),
            payload: Some(result),
            blocks: None,
            handling_mode: None,
        });

        self.runtime_adapter
            .accept_input(session_id, input)
            .await
            .map_err(|e| RpcError {
                code: error::INTERNAL_ERROR,
                message: format!("runtime accept failed: {e}"),
                data: None,
            })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: RunId,
        primitive: &RunPrimitive,
        prompt: ContentInput,
        event_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
        _additional_instructions: Option<Vec<String>>,
        overrides: Option<crate::handlers::turn::TurnOverrides>,
    ) -> Result<CoreApplyOutput, RpcError> {
        // Context-only staged primitive (e.g. peer_response_terminal) must
        // land as runtime system-context appends rather than trigger a turn.
        // Runtime boundary is Steer-derived (RunCheckpoint), so the stricter
        // `is_context_only_immediate` gate misses; use the
        // appends-empty + context-appends-nonempty criterion directly.
        if let RunPrimitive::StagedInput(staged) = primitive
            && staged.appends.is_empty()
            && !staged.context_appends.is_empty()
        {
            return self
                .service
                .apply_runtime_context_appends(
                    session_id,
                    run_id,
                    pending_system_context_appends(&staged.context_appends),
                    staged.contributing_input_ids.clone(),
                )
                .await
                .map_err(session_error_to_rpc);
        }

        let effective_identity = self
            .effective_llm_identity_for_turn(session_id, overrides.as_ref())
            .await?;
        self.validate_prompt_video_input(&prompt, &effective_identity)
            .await?;

        let pending_session = match self.staged_sessions.begin_promotion(session_id).await {
            Ok(slot) => slot.map(|s| {
                (
                    *s.build_config,
                    s.labels,
                    s.deferred_prompt,
                    s.created_at_secs,
                    s.updated_at_secs,
                )
            }),
            Err(meerkat::StagedLifecycleError::AlreadyPromoting(_)) => {
                return Err(RpcError {
                    code: error::SESSION_BUSY,
                    message: format!("session {session_id} is already being materialized"),
                    data: None,
                });
            }
            Err(e) => {
                return Err(RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!("staged session lifecycle error: {e}"),
                    data: None,
                });
            }
        };

        let persisted_keep_alive = if pending_session.is_none() {
            self.load_persisted_session(session_id)
                .await?
                .and_then(|session| session.session_metadata().map(|meta| meta.keep_alive))
        } else {
            None
        };
        let keep_alive = overrides
            .as_ref()
            .and_then(|ov| ov.keep_alive)
            .or_else(|| {
                pending_session
                    .as_ref()
                    .map(|(build_config, _, _, _, _)| build_config.keep_alive)
            })
            .or(persisted_keep_alive)
            .unwrap_or(false);

        if pending_session.is_none() && !self.live_session_is_stale(session_id).await? {
            // Hot-swap LLM client if model/provider overrides are present.
            if let Some(ref ov) = overrides
                && (ov.model.is_some()
                    || ov.provider.is_some()
                    || ov.provider_params.is_some()
                    || ov.connection_ref.is_some())
            {
                self.hot_swap_llm_client(session_id, ov).await?;
            }

            let req = StartTurnRequest {
                prompt: prompt.clone(),
                system_prompt: None,
                render_metadata: None,
                handling_mode: meerkat_core::types::HandlingMode::Queue,
                event_tx: Some(event_tx.clone()),

                skill_references: skill_references.clone(),
                flow_tool_overlay: flow_tool_overlay.clone(),
                turn_metadata: primitive.turn_metadata().cloned(),
                execution_kind: primitive
                    .turn_metadata()
                    .and_then(|meta| meta.execution_kind),
            };

            match self
                .service
                .apply_runtime_turn(
                    session_id,
                    run_id.clone(),
                    req,
                    match primitive {
                        RunPrimitive::StagedInput(staged) => staged.boundary,
                        _ => RunApplyBoundary::Immediate,
                    },
                    primitive.contributing_input_ids().to_vec(),
                )
                .await
            {
                Ok(output) => return Ok(output),
                Err(SessionError::NotFound { .. }) => {}
                Err(err) => return Err(session_error_to_rpc(err)),
            }
        }

        if let Some((mut build_config, labels, deferred_prompt, created_at_secs, updated_at_secs)) =
            pending_session
        {
            let mut runtime_prompt: ContentInput = prompt.clone();
            let saved_deferred_prompt = deferred_prompt.clone();
            if let Some(deferred) = deferred_prompt {
                runtime_prompt = merge_content_inputs(deferred, runtime_prompt);
            }

            if let Some(ref ov) = overrides {
                if ov.provider.is_some() && ov.model.is_none() {
                    self.restore_pending_from_promoting(
                        session_id,
                        build_config,
                        labels,
                        saved_deferred_prompt,
                        created_at_secs,
                        updated_at_secs,
                    )
                    .await;
                    return Err(RpcError {
                        code: error::INVALID_PARAMS,
                        message: "provider override requires model on a session turn".to_string(),
                        data: None,
                    });
                }
                let resolved_identity = self
                    .resolve_target_llm_identity(
                        &self.llm_identity_from_pending_build(&build_config).await?,
                        ov,
                    )
                    .await?;
                Self::apply_llm_identity_to_build_config(&mut build_config, &resolved_identity);
                if let Some(max_tokens) = ov.max_tokens {
                    build_config.max_tokens = Some(max_tokens);
                }
                if let Some(ref system_prompt) = ov.system_prompt {
                    build_config.system_prompt = Some(system_prompt.clone());
                }
                if let Some(ref output_schema) = ov.output_schema {
                    match meerkat_core::OutputSchema::from_json_value(output_schema.clone()) {
                        Ok(os) => build_config.output_schema = Some(os),
                        Err(e) => {
                            self.restore_pending_from_promoting(
                                session_id,
                                build_config,
                                labels,
                                saved_deferred_prompt,
                                created_at_secs,
                                updated_at_secs,
                            )
                            .await;
                            return Err(RpcError {
                                code: error::INVALID_PARAMS,
                                message: format!("Invalid output_schema override: {e}"),
                                data: None,
                            });
                        }
                    }
                }
                if let Some(retries) = ov.structured_output_retries {
                    build_config.structured_output_retries = retries;
                }
                if let Some(keep_alive) = ov.keep_alive {
                    build_config.keep_alive = keep_alive;
                }
            }

            if build_config.llm_client_override.is_none()
                && let Some(client) = self.default_llm_client()
            {
                build_config.llm_client_override = Some(client);
            }
            let runtime_generation = if build_config.config_generation.is_none() {
                if let Some(runtime) = self.config_runtime() {
                    runtime.get().await.ok().map(|snapshot| snapshot.generation)
                } else {
                    None
                }
            } else {
                None
            };

            let mut build = build_config.to_session_build_options();
            build.realm_id = build
                .realm_id
                .or_else(|| self.realm_id.as_ref().map(ToString::to_string));
            build.instance_id = build.instance_id.or_else(|| self.instance_id.clone());
            build.backend = build.backend.or_else(|| self.backend.clone());
            build.config_generation = build.config_generation.or(runtime_generation);

            match self
                .service
                .create_session(CreateSessionRequest {
                    model: build_config.model.clone(),
                    prompt: runtime_prompt.clone(),
                    render_metadata: None,
                    system_prompt: build_config.system_prompt.clone(),
                    max_tokens: build_config.max_tokens,
                    event_tx: None,

                    skill_references: skill_references.clone(),
                    initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                    deferred_prompt_policy: DeferredPromptPolicy::Discard,
                    build: Some(build),
                    labels: labels.clone(),
                })
                .await
            {
                Ok(_) => {
                    if let Some((starting_system_context_state, current_system_context_state)) =
                        self.take_promoting_system_context_state(session_id).await
                        && let Err(err) = self
                            .replay_promoted_system_context(
                                session_id,
                                &starting_system_context_state,
                                &current_system_context_state,
                            )
                            .await
                    {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %err.message,
                            "failed to replay promoted system-context state after runtime materialization"
                        );
                    }
                    #[cfg(feature = "comms")]
                    {
                        // W2-G: never reconfigure a mob-owned drain during
                        // runtime materialization. See `start_turn_via_runtime`.
                        let owner = self.runtime_adapter.peer_ingress_owner(session_id).await;
                        if !owner.is_mob_owned() {
                            let comms_rt = self.service.comms_runtime(session_id).await;
                            let peer_ingress_enabled = self
                                .preserve_existing_peer_ingress(session_id, build_config.keep_alive)
                                .await;
                            self.runtime_adapter
                                .update_peer_ingress_context(
                                    session_id,
                                    peer_ingress_enabled,
                                    comms_rt,
                                )
                                .await;
                        }
                    }
                }
                Err(err) => {
                    if let Some((_starting_system_context_state, current_system_context_state)) =
                        self.take_promoting_system_context_state(session_id).await
                    {
                        let session = build_config
                            .resume_session
                            .get_or_insert_with(|| Session::with_id(session_id.clone()));
                        session
                            .set_system_context_state(current_system_context_state)
                            .map_err(|serialize_err| RpcError {
                                code: error::INTERNAL_ERROR,
                                message: format!(
                                    "failed to serialize system-context state: {serialize_err}"
                                ),
                                data: None,
                            })?;
                    }
                    self.restore_pending_from_promoting(
                        session_id,
                        build_config,
                        labels,
                        saved_deferred_prompt,
                        created_at_secs,
                        updated_at_secs,
                    )
                    .await;
                    return Err(session_error_to_rpc(err));
                }
            }

            let (_, output) = self
                .service
                .apply_runtime_turn_with_result(
                    session_id,
                    run_id,
                    StartTurnRequest {
                        prompt: runtime_prompt,
                        system_prompt: None,
                        render_metadata: None,
                        handling_mode: meerkat_core::types::HandlingMode::Queue,
                        event_tx: Some(event_tx),

                        skill_references,
                        flow_tool_overlay,
                        turn_metadata: primitive.turn_metadata().cloned(),
                        execution_kind: primitive
                            .turn_metadata()
                            .and_then(|meta| meta.execution_kind),
                    },
                    match primitive {
                        RunPrimitive::StagedInput(staged) => staged.boundary,
                        _ => RunApplyBoundary::Immediate,
                    },
                    primitive.contributing_input_ids().to_vec(),
                )
                .await
                .map_err(session_error_to_rpc)?;
            return Ok(output);
        }

        let stored_session = self
            .load_persisted_session(session_id)
            .await?
            .ok_or_else(|| RpcError {
                code: error::SESSION_NOT_FOUND,
                message: format!("session not found: {session_id}"),
                data: None,
            })?;
        let recovery_overrides =
            self.recovery_overrides_from_turn(overrides.as_ref(), keep_alive)?;
        let create_request = self
            .recovered_create_request(session_id, stored_session, recovery_overrides)
            .await?;
        self.service
            .create_session(create_request)
            .await
            .map_err(session_error_to_rpc)?;
        #[cfg(feature = "comms")]
        {
            // W2-G: never reconfigure a mob-owned drain during recovery
            // materialization. See `start_turn_via_runtime`.
            let owner = self.runtime_adapter.peer_ingress_owner(session_id).await;
            if !owner.is_mob_owned() {
                let comms_rt = self.service.comms_runtime(session_id).await;
                let peer_ingress_enabled = self
                    .preserve_existing_peer_ingress(session_id, keep_alive)
                    .await;
                self.runtime_adapter
                    .update_peer_ingress_context(session_id, peer_ingress_enabled, comms_rt)
                    .await;
            }
        }
        let (_, output) = self
            .service
            .apply_runtime_turn_with_result(
                session_id,
                run_id,
                StartTurnRequest {
                    prompt,
                    system_prompt: None,
                    render_metadata: None,
                    handling_mode: meerkat_core::types::HandlingMode::Queue,
                    event_tx: Some(event_tx),

                    skill_references,
                    flow_tool_overlay,
                    turn_metadata: primitive.turn_metadata().cloned(),
                    execution_kind: primitive
                        .turn_metadata()
                        .and_then(|meta| meta.execution_kind),
                },
                match primitive {
                    RunPrimitive::StagedInput(staged) => staged.boundary,
                    _ => RunApplyBoundary::Immediate,
                },
                primitive.contributing_input_ids().to_vec(),
            )
            .await
            .map_err(session_error_to_rpc)?;
        Ok(output)
    }

    pub fn skill_identity_registry(&self) -> SourceIdentityRegistry {
        self.skill_identity_registry
            .read()
            .map(|state| state.registry.clone())
            .unwrap_or_default()
    }

    /// Build a source identity registry from runtime config.
    pub fn build_skill_identity_registry(
        config: &Config,
        context_root: Option<&std::path::Path>,
        user_root: Option<&std::path::Path>,
    ) -> Result<SourceIdentityRegistry, SkillError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (context_root, user_root);
            config.skills.build_source_identity_registry()
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (context_root, user_root);
            config.skills.build_source_identity_registry()
        }
    }

    /// Create a new session with the given build configuration.
    ///
    /// Returns the session ID on success. The session is staged as "pending"
    /// and will be materialized inside the service on the first `start_turn`.
    pub async fn create_session(
        &self,
        mut build_config: AgentBuildConfig,
        labels: Option<BTreeMap<String, String>>,
        deferred_prompt: Option<ContentInput>,
    ) -> Result<SessionId, RpcError> {
        let effective_llm_identity = self.llm_identity_from_pending_build(&build_config).await?;
        build_config.self_hosted_server_id = effective_llm_identity.self_hosted_server_id.clone();
        if let Some(prompt) = deferred_prompt.as_ref() {
            self.validate_prompt_video_input(prompt, &effective_llm_identity)
                .await?;
        }

        // Check combined capacity (pending + active).
        {
            let staged = self.staged_sessions.len().await;
            let active = self
                .service
                .list(Default::default())
                .await
                .map_err(session_error_to_rpc)?
                .len();
            let total = staged + active;
            if total >= self.max_sessions {
                return Err(RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!("Max sessions reached ({}/{})", total, self.max_sessions),
                    data: None,
                });
            }
        }

        // Pre-create a session to claim a stable SessionId.
        let session = Session::new();
        let session_id = session.id().clone();

        // Inject the pre-created session so the agent builder will reuse this ID.
        let bindings = self
            .runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
            .map_err(|e| RpcError {
                code: error::INTERNAL_ERROR,
                message: format!(
                    "failed to prepare runtime bindings for session {session_id}: {e}"
                ),
                data: None,
            })?;

        let build_config = AgentBuildConfig {
            resume_session: Some(session),
            runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
            ..build_config
        };

        #[cfg(feature = "mcp")]
        let build_config = self
            .attach_mcp_adapter_for_pending_session(session_id.clone(), build_config)
            .await?;

        let now = now_unix_secs();
        self.staged_sessions
            .stage(
                session_id.clone(),
                StagedSlot {
                    effective_llm_identity,
                    phase: StagedPhase::Staged {
                        build_config: Box::new(build_config),
                    },
                    labels,
                    deferred_prompt,
                    created_at_secs: now,
                    updated_at_secs: now,
                },
            )
            .await
            .map_err(|e| RpcError {
                code: error::INTERNAL_ERROR,
                message: format!("failed to stage session {session_id}: {e}"),
                data: None,
            })?;

        Ok(session_id)
    }

    /// Start a turn on the given session.
    ///
    /// Events are forwarded to `event_tx` during the turn. Returns the
    /// `RunResult` when the turn completes.
    ///
    /// `overrides` may contain per-turn overrides. For pending (deferred)
    /// sessions, all overrides are applied to the staged `AgentBuildConfig`.
    /// For materialized sessions, only `keep_alive` is allowed; all other
    /// overrides are rejected with an error.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_turn(
        &self,
        session_id: &SessionId,
        prompt: ContentInput,
        event_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
        additional_instructions: Option<Vec<String>>,
        overrides: Option<crate::handlers::turn::TurnOverrides>,
    ) -> Result<RunResult, RpcError> {
        #[allow(unused_mut)]
        let mut turn_prompt = prompt;
        #[cfg(feature = "mcp")]
        {
            let mut mcp_text = String::new();
            self.apply_mcp_boundary(session_id, &event_tx, &mut mcp_text)
                .await?;
            if !mcp_text.is_empty() {
                let mut blocks = turn_prompt.into_blocks();
                blocks.insert(
                    0,
                    meerkat_core::types::ContentBlock::Text { text: mcp_text },
                );
                turn_prompt = ContentInput::Blocks(blocks);
            }
        }

        // Check if this is a pending (not-yet-materialized) session.
        let pending_session = match self.staged_sessions.begin_promotion(session_id).await {
            Ok(slot) => slot.map(|s| {
                (
                    *s.build_config,
                    s.labels,
                    s.deferred_prompt,
                    s.created_at_secs,
                    s.updated_at_secs,
                )
            }),
            Err(meerkat::StagedLifecycleError::AlreadyPromoting(_)) => {
                return Err(RpcError {
                    code: error::SESSION_BUSY,
                    message: format!("session {session_id} is already being materialized"),
                    data: None,
                });
            }
            Err(e) => {
                return Err(RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!("staged session lifecycle error: {e}"),
                    data: None,
                });
            }
        };

        if let Some((mut build_config, labels, deferred_prompt, created_at_secs, updated_at_secs)) =
            pending_session
        {
            // Prepend the deferred create-time prompt to the turn prompt.
            // Keep copies for rollback paths that re-stage the pending session.
            let saved_deferred_prompt = deferred_prompt.clone();
            let saved_created_at_secs = created_at_secs;
            let saved_updated_at_secs = updated_at_secs;
            if let Some(deferred) = deferred_prompt {
                turn_prompt = merge_content_inputs(deferred, turn_prompt);
            }
            // Apply per-turn overrides to the pending build config.
            if let Some(ref ov) = overrides {
                if ov.provider.is_some() && ov.model.is_none() {
                    self.restore_pending_from_promoting(
                        session_id,
                        build_config,
                        labels,
                        saved_deferred_prompt,
                        saved_created_at_secs,
                        saved_updated_at_secs,
                    )
                    .await;
                    return Err(RpcError {
                        code: error::INVALID_PARAMS,
                        message: "provider override requires model on a session turn".to_string(),
                        data: None,
                    });
                }
                let resolved_identity = self
                    .resolve_target_llm_identity(
                        &self.llm_identity_from_pending_build(&build_config).await?,
                        ov,
                    )
                    .await?;
                Self::apply_llm_identity_to_build_config(&mut build_config, &resolved_identity);
                if let Some(max_tokens) = ov.max_tokens {
                    build_config.max_tokens = Some(max_tokens);
                }
                if let Some(ref system_prompt) = ov.system_prompt {
                    build_config.system_prompt = Some(system_prompt.clone());
                }
                if let Some(ref output_schema) = ov.output_schema {
                    match meerkat_core::OutputSchema::from_json_value(output_schema.clone()) {
                        Ok(os) => build_config.output_schema = Some(os),
                        Err(e) => {
                            // Restore pending state before returning error.
                            self.restore_pending_from_promoting(
                                session_id,
                                build_config,
                                labels,
                                saved_deferred_prompt,
                                saved_created_at_secs,
                                saved_updated_at_secs,
                            )
                            .await;
                            return Err(RpcError {
                                code: error::INVALID_PARAMS,
                                message: format!("Invalid output_schema override: {e}"),
                                data: None,
                            });
                        }
                    }
                }
                if let Some(retries) = ov.structured_output_retries {
                    build_config.structured_output_retries = retries;
                }
                if let Some(keep_alive) = ov.keep_alive {
                    build_config.keep_alive = keep_alive;
                }
            }
            // Inject default LLM client if the caller didn't provide one.
            if build_config.llm_client_override.is_none()
                && let Some(client) = self.default_llm_client()
            {
                build_config.llm_client_override = Some(client);
            }
            let runtime_generation = if build_config.config_generation.is_none() {
                if let Some(runtime) = self.config_runtime() {
                    runtime.get().await.ok().map(|snapshot| snapshot.generation)
                } else {
                    None
                }
            } else {
                None
            };

            let mut build = build_config.to_session_build_options();
            build.realm_id = build
                .realm_id
                .or_else(|| self.realm_id.as_ref().map(ToString::to_string));
            build.instance_id = build.instance_id.or_else(|| self.instance_id.clone());
            build.backend = build.backend.or_else(|| self.backend.clone());
            build.config_generation = build.config_generation.or(runtime_generation);

            let req = CreateSessionRequest {
                model: build_config.model.clone(),
                prompt: turn_prompt,
                render_metadata: None,
                system_prompt: build_config.system_prompt.clone(),
                max_tokens: build_config.max_tokens,
                event_tx: Some(event_tx),

                skill_references: skill_references.clone(),
                initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(build),
                labels: labels.clone(),
            };

            match self.service.create_session(req).await {
                Ok(result) => {
                    if let Some((starting_system_context_state, current_system_context_state)) =
                        self.take_promoting_system_context_state(session_id).await
                        && let Err(err) = self
                            .replay_promoted_system_context(
                                session_id,
                                &starting_system_context_state,
                                &current_system_context_state,
                            )
                            .await
                    {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %err.message,
                            "failed to replay promoted system-context state after create_session; preserving completed turn result"
                        );
                    }
                    return Ok(result);
                }
                Err(err) => {
                    if let Some((_starting_system_context_state, current_system_context_state)) =
                        self.take_promoting_system_context_state(session_id).await
                    {
                        let session = build_config
                            .resume_session
                            .get_or_insert_with(|| Session::with_id(session_id.clone()));
                        session
                            .set_system_context_state(current_system_context_state)
                            .map_err(|serialize_err| RpcError {
                                code: error::INTERNAL_ERROR,
                                message: format!(
                                    "failed to serialize system-context state: {serialize_err}"
                                ),
                                data: None,
                            })?;
                        let effective_llm_identity =
                            self.llm_identity_from_pending_build(&build_config).await?;
                        self.staged_sessions
                            .abandon_promotion(
                                session_id.clone(),
                                build_config,
                                effective_llm_identity,
                                labels,
                                saved_deferred_prompt.clone(),
                                saved_created_at_secs,
                                saved_updated_at_secs,
                            )
                            .await;
                    }
                    return Err(session_error_to_rpc(err));
                }
            }
        }

        // Normal turn on an existing (materialized) session.
        // Reject overrides that cannot be applied mid-session.
        if let Some(ref ov) = overrides {
            let rejected = [
                ov.max_tokens.map(|_| "max_tokens"),
                ov.system_prompt.as_ref().map(|_| "system_prompt"),
                ov.output_schema.as_ref().map(|_| "output_schema"),
                ov.structured_output_retries
                    .map(|_| "structured_output_retries"),
            ];
            let rejected: Vec<&str> = rejected.into_iter().flatten().collect();
            if !rejected.is_empty() {
                return Err(RpcError {
                    code: error::INVALID_PARAMS,
                    message: format!(
                        "Cannot override {} on a materialized session; use deferred session/create",
                        rejected.join(", ")
                    ),
                    data: None,
                });
            }
        }

        let keep_alive = match overrides.as_ref().and_then(|ov| ov.keep_alive) {
            Some(keep_alive) => keep_alive,
            None => self
                .load_persisted_session(session_id)
                .await?
                .and_then(|session| session.session_metadata().map(|meta| meta.keep_alive))
                .unwrap_or(false),
        };

        let req = StartTurnRequest {
            prompt: turn_prompt.clone(),
            system_prompt: None,
            render_metadata: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            event_tx: Some(event_tx.clone()),
            skill_references: skill_references.clone(),
            flow_tool_overlay: flow_tool_overlay.clone(),
            turn_metadata: None,
            execution_kind: None,
        };

        if self.live_session_is_stale(session_id).await? {
            return Box::pin(self.try_recover_persisted_session(
                session_id,
                turn_prompt,
                event_tx,
                keep_alive,
                skill_references,
                flow_tool_overlay,
                additional_instructions,
            ))
            .await;
        }

        // Persist explicit keep_alive override so subsequent inheriting calls
        // (REST/MCP resume with None) observe the updated intent.
        // This is not fire-and-forget: if the update fails, the turn must not
        // proceed with divergent runtime vs persisted state.
        if overrides.as_ref().and_then(|ov| ov.keep_alive).is_some() {
            #[cfg(feature = "comms")]
            let comms_rt = self.service.comms_runtime(session_id).await;
            #[cfg(feature = "comms")]
            if keep_alive && comms_rt.is_none() {
                return Err(RpcError {
                    code: error::INVALID_PARAMS,
                    message: "keep_alive requires a session created with comms_name".to_string(),
                    data: None,
                });
            }
            self.service
                .apply_runtime_session_keep_alive(session_id, keep_alive)
                .await
                .map_err(session_error_to_rpc)?;
            // W2-G: never reconfigure a mob-owned drain from the
            // keep_alive-override path. See `start_turn_via_runtime`.
            #[cfg(feature = "comms")]
            {
                let owner = self.runtime_adapter.peer_ingress_owner(session_id).await;
                if !owner.is_mob_owned() {
                    self.runtime_adapter
                        .update_peer_ingress_context(session_id, keep_alive, comms_rt)
                        .await;
                }
            }
        }

        // Hot-swap LLM client if model/provider/provider_params changed.
        if let Some(ref ov) = overrides
            && (ov.model.is_some() || ov.provider.is_some() || ov.provider_params.is_some())
        {
            self.hot_swap_llm_client(session_id, ov).await?;
        }

        match self.service.start_turn(session_id, req).await {
            Ok(result) => Ok(result),
            Err(SessionError::NotFound { .. }) => {
                // Attempt persisted session recovery: the session may exist in the
                // store from a previous runtime lifetime.
                Box::pin(self.try_recover_persisted_session(
                    session_id,
                    turn_prompt,
                    event_tx,
                    keep_alive,
                    skill_references,
                    flow_tool_overlay,
                    additional_instructions,
                ))
                .await
            }
            Err(err) => Err(session_error_to_rpc(err)),
        }
    }

    /// Restore a pending session from `Promoting` state back to `Staged` after
    /// an override validation failure. Prevents the session from being stuck in
    /// the promoting state when the turn is aborted before `create_session`.
    async fn restore_pending_from_promoting(
        &self,
        session_id: &SessionId,
        mut build_config: AgentBuildConfig,
        labels: Option<BTreeMap<String, String>>,
        deferred_prompt: Option<ContentInput>,
        created_at_secs: u64,
        updated_at_secs: u64,
    ) {
        let Ok(effective_llm_identity) = self.llm_identity_from_pending_build(&build_config).await
        else {
            return;
        };
        build_config.self_hosted_server_id = effective_llm_identity.self_hosted_server_id.clone();
        self.staged_sessions
            .abandon_promotion(
                session_id.clone(),
                build_config,
                effective_llm_identity,
                labels,
                deferred_prompt,
                created_at_secs,
                updated_at_secs,
            )
            .await;
    }

    /// Attempt to recover a persisted session that is no longer live.
    ///
    /// Mirrors the REST recovery path: load the session from the store, extract
    /// stored metadata, build an `AgentBuildConfig`, stage as pending, and
    /// fall through to the normal materialization path.
    ///
    /// Returns `SESSION_NOT_FOUND` if the session cannot be recovered.
    #[allow(clippy::too_many_arguments)]
    async fn try_recover_persisted_session(
        &self,
        session_id: &SessionId,
        prompt: ContentInput,
        event_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
        keep_alive: bool,
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
        flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
        additional_instructions: Option<Vec<String>>,
    ) -> Result<RunResult, RpcError> {
        let loaded_session = self.load_persisted_session(session_id).await?;

        let Some(session) = loaded_session else {
            return Err(RpcError {
                code: error::SESSION_NOT_FOUND,
                message: format!("Session not found: {session_id}"),
                data: None,
            });
        };
        if session_metadata_marks_archived(&session) {
            return Err(RpcError {
                code: error::SESSION_NOT_FOUND,
                message: format!("Session not found: {session_id}"),
                data: None,
            });
        }

        let recovery_overrides = self.recovery_overrides_from_turn(None, keep_alive)?;
        let create_request = self
            .recovered_create_request(session_id, session, recovery_overrides)
            .await?;
        let build = create_request.build.clone().ok_or_else(|| RpcError {
            code: error::INTERNAL_ERROR,
            message: format!(
                "recovered create request for session {session_id} is missing build options"
            ),
            data: None,
        })?;
        let model = create_request.model.clone();
        let mut build_config = AgentBuildConfig::new(model);
        build_config.apply_session_build_options(&build);
        build_config.system_prompt = create_request.system_prompt.clone();
        build_config.max_tokens = create_request.max_tokens;

        // Stage as pending and re-enter the materialization path.
        // Labels are managed by the session service — pass None so
        // CreateSessionRequest.labels is empty and the service preserves
        // whatever labels were stored with the original session.
        let labels: Option<BTreeMap<String, String>> = None;

        {
            let effective_llm_identity =
                self.llm_identity_from_pending_build(&build_config).await?;
            let now = now_unix_secs();
            self.staged_sessions
                .stage(
                    session_id.clone(),
                    StagedSlot {
                        effective_llm_identity,
                        phase: StagedPhase::Staged {
                            build_config: Box::new(build_config),
                        },
                        labels,
                        deferred_prompt: None,
                        created_at_secs: now,
                        updated_at_secs: now,
                    },
                )
                .await
                .map_err(|e| RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!("failed to stage recovered session {session_id}: {e}"),
                    data: None,
                })?;
        }

        // Recursively call start_turn which will now find the pending session.
        // Use Box::pin to avoid infinite recursion concerns in async.
        Box::pin(self.start_turn(
            session_id,
            prompt,
            event_tx,
            skill_references,
            flow_tool_overlay,
            additional_instructions,
            None,
        ))
        .await
    }

    async fn take_promoting_system_context_state(
        &self,
        session_id: &SessionId,
    ) -> Option<(SessionSystemContextState, SessionSystemContextState)> {
        self.staged_sessions
            .take_promoting_system_context_state(session_id)
            .await
    }

    async fn replay_promoted_system_context(
        &self,
        session_id: &SessionId,
        starting_state: &SessionSystemContextState,
        current_state: &SessionSystemContextState,
    ) -> Result<(), RpcError> {
        for pending in &current_state.pending {
            if starting_state.pending.contains(pending) {
                continue;
            }
            self.service
                .append_system_context(
                    session_id,
                    AppendSystemContextRequest {
                        text: pending.text.clone(),
                        source: pending.source.clone(),
                        idempotency_key: pending.idempotency_key.clone(),
                    },
                )
                .await
                .map_err(system_context_error_to_rpc)?;
        }
        Ok(())
    }

    /// Interrupt a running turn on the given session.
    ///
    /// If the session is idle, this is a no-op.
    pub async fn interrupt(&self, session_id: &SessionId) -> Result<(), RpcError> {
        // Pending sessions have no running turn.
        if self.staged_sessions.contains(session_id).await {
            return Ok(());
        }

        match self.service.interrupt(session_id).await {
            Ok(()) => Ok(()),
            // The service returns NotRunning when no turn is active — map to no-op.
            Err(SessionError::NotRunning { .. }) => Ok(()),
            Err(e) => Err(session_error_to_rpc(e)),
        }
    }

    /// Append runtime system context to a pending, live, or persisted session.
    pub async fn append_system_context(
        &self,
        session_id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, RpcError> {
        if let Some(result) = self
            .staged_sessions
            .append_system_context(
                session_id,
                &req,
                meerkat_core::time_compat::SystemTime::now(),
                now_unix_secs(),
            )
            .await
        {
            let status = result
                .map_err(|err| system_context_error_to_rpc(err.into_control_error(session_id)))?;
            return Ok(AppendSystemContextResult { status });
        }

        self.service
            .append_system_context(session_id, req)
            .await
            .map_err(system_context_error_to_rpc)
    }

    /// Get the current state of a session, or `None` if the session does not exist.
    pub async fn session_state(&self, session_id: &SessionId) -> Option<SessionInfo> {
        // Check pending sessions first.
        {
            if let Some(info) = self.staged_sessions.info(session_id).await {
                return Some(SessionInfo {
                    session_id: session_id.clone(),
                    state: if info.is_promoting {
                        SessionState::Running
                    } else {
                        SessionState::Idle
                    },
                    labels: info.labels,
                });
            }
        }

        // Use `list()` instead of `read()` to avoid blocking on a
        // `ReadSnapshot` command while a turn is in progress. `list()`
        // reads state from non-blocking watch receivers.
        if let Ok(summaries) = self.service.list(Default::default()).await {
            for summary in summaries {
                if summary.session_id == *session_id {
                    return Some(SessionInfo {
                        session_id: summary.session_id,
                        state: if summary.is_active {
                            SessionState::Running
                        } else {
                            SessionState::Idle
                        },
                        labels: summary.labels,
                    });
                }
            }
        }

        None
    }

    pub async fn pending_session_exists(&self, session_id: &SessionId) -> bool {
        self.staged_sessions.contains(session_id).await
    }

    /// Read the authoritative session view from the owning session service.
    pub async fn read_session(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_core::service::SessionView, RpcError> {
        self.service
            .read(session_id)
            .await
            .map_err(session_error_to_rpc)
    }

    /// Load the persisted session snapshot for authoritative post-turn inspection.
    pub async fn load_persisted_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, RpcError> {
        self.service
            .load_authoritative_session(session_id)
            .await
            .map_err(session_error_to_rpc)
    }

    /// Archive (remove) a session.
    pub async fn archive_session(&self, session_id: &SessionId) -> Result<(), RpcError> {
        // Check pending sessions first.
        if self.staged_sessions.abandon(session_id).await {
            #[cfg(feature = "mcp")]
            if let Some(state) = self.mcp_sessions.write().await.remove(session_id) {
                state.adapter.shutdown().await;
            }
            return Ok(());
        }

        let result = self
            .service
            .archive(session_id)
            .await
            .map_err(session_error_to_rpc);

        #[cfg(feature = "mcp")]
        if let Some(state) = self.mcp_sessions.write().await.remove(session_id) {
            state.adapter.shutdown().await;
        }

        #[cfg(feature = "comms")]
        self.runtime_adapter.abort_comms_drain(session_id).await;

        result
    }

    /// List all active sessions, optionally filtered by query parameters.
    pub async fn list_sessions(&self, query: SessionQuery) -> Vec<SessionInfo> {
        let mut result = Vec::new();

        // Include pending sessions as Idle, filtered by labels if requested.
        for (session_id, info) in self.staged_sessions.list(query.labels.as_ref()).await {
            result.push(SessionInfo {
                session_id,
                state: if info.is_promoting {
                    SessionState::Running
                } else {
                    SessionState::Idle
                },
                labels: info.labels,
            });
        }

        // Include active sessions from the service. The service's `list()`
        // already handles label filtering on materialized sessions.
        if let Ok(summaries) = self.service.list(query).await {
            for summary in summaries {
                let state = if summary.is_active {
                    SessionState::Running
                } else {
                    SessionState::Idle
                };
                result.push(SessionInfo {
                    session_id: summary.session_id,
                    state,
                    labels: summary.labels,
                });
            }
        }

        result
    }

    /// List all active sessions as canonical wire summaries, including pending sessions.
    pub async fn list_sessions_rich(
        &self,
        query: SessionQuery,
    ) -> Vec<meerkat_contracts::WireSessionSummary> {
        let mut result = Vec::new();

        // Include pending sessions as synthetic entries.
        for (session_id, info) in self.staged_sessions.list(query.labels.as_ref()).await {
            result.push(meerkat_contracts::WireSessionSummary {
                session_id,
                session_ref: None,
                created_at: info.created_at_secs,
                updated_at: info.updated_at_secs,
                message_count: 0,
                total_tokens: 0,
                is_active: info.is_promoting,
                labels: info.labels,
            });
        }

        // Include materialized sessions from the service.
        if let Ok(summaries) = self.service.list(query).await {
            for summary in summaries {
                result.push(summary.into());
            }
        }

        result
    }

    /// Read a session as a canonical wire info object, checking pending and materialized.
    pub async fn read_session_rich(
        &self,
        session_id: &SessionId,
    ) -> Option<meerkat_contracts::WireSessionInfo> {
        // Check pending sessions first.
        if let Some(info) = self.staged_sessions.info(session_id).await {
            return Some(meerkat_contracts::WireSessionInfo {
                session_id: session_id.clone(),
                session_ref: None,
                created_at: info.created_at_secs,
                updated_at: info.updated_at_secs,
                message_count: 0,
                is_active: info.is_promoting,
                model: info.effective_llm_identity.model.clone(),
                provider: info.effective_llm_identity.provider.as_str().to_string(),
                last_assistant_text: None,
                labels: info.labels,
            });
        }

        // Try list() first — non-blocking (uses watch receivers).
        if let Ok(summaries) = self.service.list(Default::default()).await {
            for summary in summaries {
                if summary.session_id == *session_id {
                    if !summary.is_active {
                        // Idle session: safe to call read() without blocking,
                        // which provides last_assistant_text.
                        if let Ok(view) = self.service.read(session_id).await {
                            return Some(view.state.into());
                        }
                    }
                    // Active session: return summary without last_assistant_text
                    // to avoid blocking the caller during a running turn.
                    let wire: meerkat_contracts::WireSessionSummary = summary.into();
                    let llm_identity = self
                        .service
                        .live_session_llm_identity(session_id)
                        .await
                        .ok()?;
                    return Some(meerkat_contracts::WireSessionInfo {
                        session_id: wire.session_id,
                        session_ref: None,
                        created_at: wire.created_at,
                        updated_at: wire.updated_at,
                        message_count: wire.message_count,
                        is_active: wire.is_active,
                        model: llm_identity.model,
                        provider: llm_identity.provider.as_str().to_string(),
                        last_assistant_text: None,
                        labels: wire.labels,
                    });
                }
            }
        }

        // Fallback: try read() for archived/persisted sessions not in the live list.
        if let Ok(view) = self.service.read(session_id).await {
            return Some(view.state.into());
        }

        None
    }

    /// Read a session transcript as canonical wire history, including pending sessions.
    pub async fn read_session_history_rich(
        &self,
        session_id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Option<meerkat_contracts::WireSessionHistory> {
        if self.staged_sessions.contains(session_id).await {
            return Some(meerkat_contracts::WireSessionHistory {
                session_id: session_id.clone(),
                session_ref: None,
                message_count: 0,
                offset: query.offset,
                limit: query.limit,
                has_more: false,
                messages: Vec::new(),
            });
        }

        self.service
            .read_history(session_id, query)
            .await
            .ok()
            .map(Into::into)
    }

    /// Get the event injector for a session, if available.
    pub async fn event_injector(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::EventInjector>> {
        self.service.event_injector(session_id).await
    }

    /// Subscribe to session-wide events regardless of triggering interaction.
    pub async fn subscribe_session_events(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<meerkat_core::EventStream, meerkat_core::StreamError> {
        self.service.subscribe_session_events(session_id).await
    }

    /// Wait until a live session's authoritative summary timestamp advances.
    pub async fn wait_for_session_mutation_after(
        &self,
        session_id: &meerkat_core::SessionId,
        after: std::time::SystemTime,
    ) -> Result<std::time::SystemTime, meerkat_core::StreamError> {
        self.service
            .wait_for_session_mutation_after(session_id, after)
            .await
    }

    /// Get the comms runtime for a session, if available.
    pub async fn comms_runtime(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.service.comms_runtime(session_id).await
    }

    /// Shut down the runtime, closing all sessions.
    pub async fn shutdown(&self) {
        // Clear pending sessions.
        self.staged_sessions.clear().await;

        self.shutdown_schedule_host().await;

        // Shut down the service.
        self.service.shutdown().await;

        #[cfg(feature = "mcp")]
        {
            let mut map = self.mcp_sessions.write().await;
            let adapters = map
                .drain()
                .map(|(_, state)| state.adapter)
                .collect::<Vec<_>>();
            drop(map);
            for adapter in adapters {
                adapter.shutdown().await;
            }
        }

        // Abort all comms drain tasks.
        #[cfg(feature = "comms")]
        self.runtime_adapter.abort_comms_drains().await;
    }

    #[cfg(feature = "mcp")]
    pub async fn mcp_stage_add(
        &self,
        session_id: &SessionId,
        server_name: String,
        server_config: serde_json::Value,
    ) -> Result<(), RpcError> {
        self.mcp_stage_add_with_persistence(session_id, server_name, server_config, false)
            .await
            .map(|_| ())
    }

    #[cfg(feature = "mcp")]
    pub async fn mcp_stage_add_with_persistence(
        &self,
        session_id: &SessionId,
        server_name: String,
        mut server_config: serde_json::Value,
        persisted: bool,
    ) -> Result<bool, RpcError> {
        self.ensure_session_exists(session_id).await?;
        if server_name.trim().is_empty() {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "server_name cannot be empty".to_string(),
                data: None,
            });
        }

        if let Some(obj) = server_config.as_object_mut() {
            obj.insert(
                "name".to_string(),
                serde_json::Value::String(server_name.clone()),
            );
        }

        let config: McpServerConfig =
            serde_json::from_value(server_config).map_err(|e| RpcError {
                code: error::INVALID_PARAMS,
                message: format!("invalid server_config: {e}"),
                data: None,
            })?;

        let adapter = self.mcp_adapter_for_session(session_id).await?;
        let rollback = if persisted {
            let (context_root, user_root) = self.skill_identity_roots();
            let authority =
                meerkat::surface::mcp_config_mutation_authority(context_root, user_root);
            match meerkat::surface::persist_mcp_add_if_requested(true, &authority, config.clone())
                .await
            {
                Ok(rollback) => rollback,
                Err(message) => {
                    return Err(RpcError {
                        code: error::INTERNAL_ERROR,
                        message,
                        data: None,
                    });
                }
            }
        } else {
            None
        };
        if let Err(e) = adapter.stage_add(config).await {
            let rollback_message =
                match meerkat::surface::rollback_mcp_persisted_mutation(rollback).await {
                    Ok(()) => String::new(),
                    Err(rollback_err) => format!("; persisted rollback failed: {rollback_err}"),
                };
            return Err(RpcError {
                code: error::INTERNAL_ERROR,
                message: format!("{e}{rollback_message}"),
                data: None,
            });
        }
        Ok(rollback.is_some())
    }

    #[cfg(feature = "mcp")]
    pub async fn mcp_stage_remove(
        &self,
        session_id: &SessionId,
        server_name: String,
    ) -> Result<(), RpcError> {
        self.mcp_stage_remove_with_persistence(session_id, server_name, false)
            .await
            .map(|_| ())
    }

    #[cfg(feature = "mcp")]
    pub async fn mcp_stage_remove_with_persistence(
        &self,
        session_id: &SessionId,
        server_name: String,
        persisted: bool,
    ) -> Result<bool, RpcError> {
        self.ensure_session_exists(session_id).await?;
        if server_name.trim().is_empty() {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "server_name cannot be empty".to_string(),
                data: None,
            });
        }
        let adapter = self.mcp_adapter_for_session(session_id).await?;
        let rollback = if persisted {
            let (context_root, user_root) = self.skill_identity_roots();
            let authority =
                meerkat::surface::mcp_config_mutation_authority(context_root, user_root);
            match meerkat::surface::persist_mcp_remove_if_requested(true, &authority, &server_name)
                .await
            {
                Ok(rollback) => rollback,
                Err(message) => {
                    return Err(RpcError {
                        code: error::INTERNAL_ERROR,
                        message,
                        data: None,
                    });
                }
            }
        } else {
            None
        };
        if let Err(e) = adapter.stage_remove(server_name).await {
            let rollback_message =
                match meerkat::surface::rollback_mcp_persisted_mutation(rollback).await {
                    Ok(()) => String::new(),
                    Err(rollback_err) => format!("; persisted rollback failed: {rollback_err}"),
                };
            return Err(RpcError {
                code: error::INTERNAL_ERROR,
                message: format!("{e}{rollback_message}"),
                data: None,
            });
        }
        Ok(rollback.is_some())
    }

    #[cfg(feature = "mcp")]
    pub async fn mcp_stage_reload(
        &self,
        session_id: &SessionId,
        server_name: Option<String>,
    ) -> Result<(), RpcError> {
        self.ensure_session_exists(session_id).await?;
        if let Some(name) = server_name.as_ref()
            && name.trim().is_empty()
        {
            return Err(RpcError {
                code: error::INVALID_PARAMS,
                message: "server_name cannot be empty".to_string(),
                data: None,
            });
        }
        let adapter = self.mcp_adapter_for_session(session_id).await?;
        match server_name {
            Some(name) => {
                // Validate server exists before staging to avoid deferred failures
                // at the next turn boundary.
                let active = adapter.active_server_names().await;
                if !active.iter().any(|n| n == &name) {
                    return Err(RpcError {
                        code: error::INVALID_PARAMS,
                        message: format!("MCP server '{name}' is not registered on this session"),
                        data: None,
                    });
                }
                adapter
                    .stage_reload(McpReloadTarget::ServerName(name))
                    .await
                    .map_err(|e| RpcError {
                        code: error::INTERNAL_ERROR,
                        message: e,
                        data: None,
                    })?;
            }
            None => {
                // Reload all active servers.
                let names = adapter.active_server_names().await;
                for name in names {
                    adapter
                        .stage_reload(McpReloadTarget::ServerName(name))
                        .await
                        .map_err(|e| RpcError {
                            code: error::INTERNAL_ERROR,
                            message: e,
                            data: None,
                        })?;
                }
            }
        }
        Ok(())
    }

    #[cfg(feature = "mcp")]
    async fn attach_mcp_adapter_for_pending_session(
        &self,
        session_id: SessionId,
        mut build_config: AgentBuildConfig,
    ) -> Result<AgentBuildConfig, RpcError> {
        let adapter = Arc::new(McpRouterAdapter::new(McpRouter::new()));
        let adapter_dispatcher: Arc<dyn AgentToolDispatcher> = adapter.clone();
        let combined = match build_config.external_tools.clone() {
            Some(existing) => Arc::new(
                ToolGateway::new(existing, Some(adapter_dispatcher)).map_err(|e| RpcError {
                    code: error::INVALID_PARAMS,
                    message: format!("failed to compose external tools with MCP adapter: {e}"),
                    data: None,
                })?,
            ),
            None => adapter_dispatcher,
        };
        build_config.external_tools = Some(combined);

        let (lifecycle_tx, lifecycle_rx) = mpsc::unbounded_channel();
        let state = SessionMcpState {
            adapter,
            turn_counter: 0,
            lifecycle_tx,
            lifecycle_rx,
            drain_task_running: Arc::new(AtomicBool::new(false)),
        };
        self.mcp_sessions.write().await.insert(session_id, state);
        Ok(build_config)
    }

    #[cfg(feature = "mcp")]
    async fn ensure_session_exists(&self, session_id: &SessionId) -> Result<(), RpcError> {
        if self.staged_sessions.contains(session_id).await {
            return Ok(());
        }
        if self.session_state(session_id).await.is_none() {
            return Err(RpcError {
                code: error::SESSION_NOT_FOUND,
                message: format!("Session not found: {session_id}"),
                data: None,
            });
        }
        Ok(())
    }

    #[cfg(feature = "mcp")]
    async fn mcp_adapter_for_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Arc<McpRouterAdapter>, RpcError> {
        let map = self.mcp_sessions.read().await;
        map.get(session_id)
            .map(|state| state.adapter.clone())
            .ok_or_else(|| RpcError {
                code: error::INVALID_PARAMS,
                message: "session does not support live MCP operations".to_string(),
                data: None,
            })
    }

    #[cfg(feature = "mcp")]
    async fn apply_mcp_boundary(
        &self,
        session_id: &SessionId,
        event_tx: &mpsc::Sender<EventEnvelope<AgentEvent>>,
        prompt: &mut String,
    ) -> Result<(), RpcError> {
        let (adapter, turn_number, drain_task_running, lifecycle_tx, mut queued_actions) = {
            let mut map = self.mcp_sessions.write().await;
            let state = match map.get_mut(session_id) {
                Some(state) => state,
                None => return Ok(()),
            };
            state.turn_counter = state.turn_counter.saturating_add(1);
            let mut queued = Vec::new();
            while let Ok(action) = state.lifecycle_rx.try_recv() {
                queued.push(action);
            }
            (
                state.adapter.clone(),
                state.turn_counter,
                state.drain_task_running.clone(),
                state.lifecycle_tx.clone(),
                queued,
            )
        };

        if !queued_actions.is_empty() {
            let drained = std::mem::take(&mut queued_actions);
            self.emit_mcp_lifecycle_actions(session_id, event_tx, prompt, turn_number, drained)
                .await;
        }

        let result = adapter.apply_staged().await.map_err(|e| RpcError {
            code: error::INTERNAL_ERROR,
            message: format!("failed to apply staged MCP operations: {e}"),
            data: None,
        })?;

        if result.delta.lifecycle_actions.iter().any(|action| {
            action.operation == ToolConfigChangeOperation::Remove
                && action.phase == McpLifecyclePhase::Draining
        }) {
            Self::spawn_mcp_drain_task_if_needed(adapter.clone(), drain_task_running, lifecycle_tx);
        }

        queued_actions.extend(result.delta.lifecycle_actions);
        if queued_actions.is_empty() {
            return Ok(());
        }

        self.emit_mcp_lifecycle_actions(session_id, event_tx, prompt, turn_number, queued_actions)
            .await;
        Ok(())
    }

    #[cfg(feature = "mcp")]
    fn spawn_mcp_drain_task_if_needed(
        adapter: Arc<McpRouterAdapter>,
        task_running: Arc<AtomicBool>,
        lifecycle_tx: mpsc::UnboundedSender<McpLifecycleAction>,
    ) {
        if task_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let delta = match adapter.progress_removals().await {
                    Ok(delta) => delta,
                    Err(err) => {
                        tracing::warn!("background MCP drain apply failed: {err}");
                        break;
                    }
                };

                for action in delta.lifecycle_actions {
                    let _ = lifecycle_tx.send(action);
                }

                match adapter.has_removing_servers().await {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(err) => {
                        tracing::warn!("background MCP drain state check failed: {err}");
                        break;
                    }
                }
            }
            task_running.store(false, Ordering::Release);
        });
    }

    #[cfg(feature = "mcp")]
    async fn emit_mcp_lifecycle_actions(
        &self,
        session_id: &SessionId,
        event_tx: &mpsc::Sender<EventEnvelope<AgentEvent>>,
        prompt: &mut String,
        turn_number: u32,
        actions: Vec<McpLifecycleAction>,
    ) {
        let source_id = format!("session:{session_id}");
        meerkat::surface::emit_mcp_lifecycle_events(
            event_tx,
            &source_id,
            prompt,
            turn_number,
            actions,
        )
        .await;
    }
}

fn decode_wire_content_blocks(
    blocks: Option<Vec<meerkat_contracts::WireContentBlock>>,
) -> Result<Option<Vec<meerkat_core::types::ContentBlock>>, RpcError> {
    let Some(blocks) = blocks else {
        return Ok(None);
    };

    blocks
        .into_iter()
        .map(meerkat_core::types::ContentBlock::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
        .map_err(|message| RpcError {
            code: error::INVALID_PARAMS,
            message: message.to_string(),
            data: None,
        })
}

fn stored_has_unapplied_system_context(stored: &Session, live: &Session) -> bool {
    let stored_state = stored.system_context_state().unwrap_or_default();
    let live_state = live.system_context_state().unwrap_or_default();
    stored_state
        .pending
        .iter()
        .any(|pending| !live_state.pending.contains(pending))
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

fn session_error_to_rpc(err: SessionError) -> RpcError {
    let code = match &err {
        SessionError::NotFound { .. } => error::SESSION_NOT_FOUND,
        SessionError::Busy { .. } => error::SESSION_BUSY,
        SessionError::NotRunning { .. } => error::INTERNAL_ERROR,
        SessionError::Agent(agent_err) => match agent_err {
            meerkat_core::AgentError::TokenBudgetExceeded { .. }
            | meerkat_core::AgentError::TimeBudgetExceeded { .. }
            | meerkat_core::AgentError::ToolCallBudgetExceeded { .. } => error::BUDGET_EXHAUSTED,
            meerkat_core::AgentError::ConfigError(_) => error::INVALID_PARAMS,
            meerkat_core::AgentError::HookDenied { .. } => error::HOOK_DENIED,
            meerkat_core::AgentError::Llm { .. } => error::PROVIDER_ERROR,
            meerkat_core::AgentError::SessionNotFound(_) => error::SESSION_NOT_FOUND,
            meerkat_core::AgentError::BuildError(_) => error::PROVIDER_ERROR,
            meerkat_core::AgentError::InternalError(_) => error::INTERNAL_ERROR,
            _ => error::INTERNAL_ERROR,
        },
        _ => error::INTERNAL_ERROR,
    };
    RpcError {
        code,
        message: err.to_string(),
        data: None,
    }
}

fn completion_outcome_to_rpc_result(
    outcome: meerkat_runtime::completion::CompletionOutcome,
    session_id: &SessionId,
) -> Result<RunResult, RpcError> {
    use meerkat_runtime::completion::CompletionOutcome;

    match outcome {
        CompletionOutcome::Completed(result) => Ok(result),
        CompletionOutcome::CompletedWithoutResult => Err(RpcError {
            code: error::INTERNAL_ERROR,
            message: "turn completed without result".to_string(),
            data: None,
        }),
        CompletionOutcome::CallbackPending { tool_name, args } => Err(RpcError {
            code: error::INTERNAL_ERROR,
            message: format!("callback pending for tool '{tool_name}'"),
            data: Some(serde_json::json!({
                "session_id": session_id.to_string(),
                "resumable": true,
                "tool_name": tool_name,
                "args": args,
            })),
        }),
        CompletionOutcome::Abandoned(reason) => Err(RpcError {
            code: error::INTERNAL_ERROR,
            message: format!("turn abandoned: {reason}"),
            data: None,
        }),
        CompletionOutcome::RuntimeTerminated(reason) => Err(RpcError {
            code: error::INTERNAL_ERROR,
            message: format!("runtime terminated: {reason}"),
            data: None,
        }),
    }
}

fn system_context_error_to_rpc(err: SessionControlError) -> RpcError {
    match err {
        SessionControlError::Session(session_err) => session_error_to_rpc(session_err),
        SessionControlError::InvalidRequest { message } => RpcError {
            code: error::INVALID_PARAMS,
            message,
            data: None,
        },
        SessionControlError::Conflict { key, .. } => RpcError {
            code: error::INVALID_PARAMS,
            message: format!("system-context idempotency conflict for key '{key}'"),
            data: None,
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    // Phase 1a added ~6 DSL fields to MeerkatMachineState pushing several
    // test futures past the default 16384-byte stack budget. Tests only.
    clippy::large_futures
)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use futures::stream;
    use meerkat::AgentBuildConfig;
    use meerkat_client::{LlmClient, LlmError};
    use meerkat_core::StopReason;
    use meerkat_core::skills::{
        SkillKey, SkillKeyRemap, SkillName, SourceIdentityLineage, SourceIdentityLineageEvent,
        SourceUuid,
    };
    use meerkat_core::skills_config::{
        SkillRepoTransport, SkillRepositoryConfig, SkillsConfig, SkillsIdentityConfig,
    };
    #[cfg(feature = "mcp")]
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::Arc;

    // -----------------------------------------------------------------------
    // Mock LLM client
    // -----------------------------------------------------------------------

    struct MockLlmClient;

    #[async_trait]
    impl LlmClient for MockLlmClient {
        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>,
        > {
            Box::pin(stream::iter(vec![
                Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "Hello from mock".to_string(),
                    meta: None,
                }),
                Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: StopReason::EndTurn,
                    },
                }),
            ]))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    /// A mock LLM client that introduces a delay before responding.
    /// Used to test concurrent access (SESSION_BUSY).
    struct SlowMockLlmClient {
        delay_ms: u64,
    }

    impl SlowMockLlmClient {
        fn new(delay_ms: u64) -> Self {
            Self { delay_ms }
        }
    }

    #[async_trait]
    impl LlmClient for SlowMockLlmClient {
        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>,
        > {
            let delay_ms = self.delay_ms;
            Box::pin(async_stream::stream! {
                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                yield Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "Slow response".to_string(),
                    meta: None,
                });
                yield Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: StopReason::EndTurn,
                    },
                });
            })
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn temp_factory(temp: &tempfile::TempDir) -> AgentFactory {
        AgentFactory::new(temp.path().join("sessions"))
    }

    fn mock_build_config() -> AgentBuildConfig {
        AgentBuildConfig {
            llm_client_override: Some(Arc::new(MockLlmClient)),
            ..AgentBuildConfig::new("claude-sonnet-4-5")
        }
    }

    fn slow_build_config(delay_ms: u64) -> AgentBuildConfig {
        AgentBuildConfig {
            llm_client_override: Some(Arc::new(SlowMockLlmClient::new(delay_ms))),
            ..AgentBuildConfig::new("claude-sonnet-4-5")
        }
    }

    #[tokio::test]
    async fn realtime_open_config_projects_pending_system_context_into_seed_messages() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");
        runtime
            .append_system_context(
                &session_id,
                AppendSystemContextRequest {
                    text: "Authoritative peer token is birch seventeen.".to_string(),
                    source: Some("peer_response_terminal:analyst:req-123".to_string()),
                    idempotency_key: Some("peer_response_terminal:analyst:req-123".to_string()),
                },
            )
            .await
            .expect("append_system_context");

        let open_config = runtime
            .realtime_session_open_config(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("realtime_session_open_config");

        let system_messages = open_config
            .seed_messages
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            system_messages.iter().any(|text| {
                text.contains("[Runtime System Context]")
                    && text.contains("Authoritative peer token is birch seventeen.")
            }),
            "projected realtime seed messages should include pending runtime system context: {system_messages:?}"
        );
    }

    #[tokio::test]
    async fn realtime_open_config_projects_build_state_additional_instructions_into_root_system_message()
     {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let mut build = mock_build_config();
        build.keep_alive = true;
        build.comms_name = Some("realtime-open-config-build-state".to_string());
        build.system_prompt = Some("You are the realtime operator.".to_string());
        build.additional_instructions = Some(vec![
            "Remember user-provided codewords verbatim.".to_string(),
            "Use the most recent authoritative terminal peer response.".to_string(),
        ]);

        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");

        let open_config = runtime
            .realtime_session_open_config(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("realtime_session_open_config");

        let root_system = open_config
            .seed_messages
            .first()
            .and_then(|message| match message {
                Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .expect("expected root system projection");

        assert!(
            root_system.contains("You are the realtime operator."),
            "expected root system prompt to preserve canonical build-state system prompt: {root_system}"
        );
        assert!(
            root_system.contains("[Session Build Instructions]"),
            "expected root system prompt to render durable build-state instructions: {root_system}"
        );
        assert!(
            root_system.contains("Remember user-provided codewords verbatim.")
                && root_system
                    .contains("Use the most recent authoritative terminal peer response."),
            "expected root system prompt to include all durable additional instructions: {root_system}"
        );
    }

    #[tokio::test]
    async fn recovery_restores_build_state_additional_instructions_into_realtime_projection() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let mut build = mock_build_config();
        build.system_prompt = Some("You are the recovered realtime operator.".to_string());
        build.additional_instructions = Some(vec![
            "Remember user-provided codewords verbatim.".to_string(),
            "Prefer the latest authoritative peer response over stale memory.".to_string(),
        ]);

        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");

        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard_live_session");
        runtime
            .runtime_adapter()
            .unregister_session(&session_id)
            .await;

        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .try_recover_persisted_session(
                &session_id,
                "Recover".into(),
                event_tx,
                false,
                None,
                None,
                None,
            )
            .await
            .expect("try_recover_persisted_session");

        let open_config = runtime
            .realtime_session_open_config(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("realtime_session_open_config");

        let root_system = open_config
            .seed_messages
            .first()
            .and_then(|message| match message {
                Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .expect("expected root system projection after recovery");

        assert!(
            root_system.contains("You are the recovered realtime operator."),
            "expected recovered realtime projection to preserve canonical system prompt: {root_system}"
        );
        assert!(
            root_system.contains("[Session Build Instructions]"),
            "expected recovered realtime projection to render durable build-state instructions: {root_system}"
        );
        assert!(
            root_system.contains("Remember user-provided codewords verbatim.")
                && root_system
                    .contains("Prefer the latest authoritative peer response over stale memory."),
            "expected recovered realtime projection to include all durable additional instructions: {root_system}"
        );
    }

    #[tokio::test]
    async fn realtime_open_config_includes_runtime_owned_terminal_peer_response_projection() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut base_runtime = make_runtime(temp_factory(&temp), 10);
        base_runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(base_runtime);

        let mut build = mock_build_config();
        build.keep_alive = true;
        build.comms_name = Some("realtime-open-config-peer-response".to_string());
        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");
        runtime
            .ensure_runtime_executor(&session_id)
            .await
            .expect("ensure_runtime_executor");

        runtime
            .runtime_adapter()
            .accept_input(
                &session_id,
                meerkat_runtime::Input::Peer(meerkat_runtime::PeerInput {
                    header: meerkat_runtime::InputHeader {
                        id: meerkat_core::lifecycle::InputId::new(),
                        timestamp: chrono::Utc::now(),
                        source: meerkat_runtime::InputOrigin::Peer {
                            peer_id: "analyst-rt".to_string(),
                            runtime_id: None,
                        },
                        durability: meerkat_runtime::InputDurability::Durable,
                        visibility: meerkat_runtime::InputVisibility::default(),
                        idempotency_key: None,
                        supersession_key: None,
                        correlation_id: None,
                    },
                    convention: Some(meerkat_runtime::PeerConvention::ResponseTerminal {
                        request_id: "req-123".to_string(),
                        status: meerkat_runtime::ResponseTerminalStatus::Completed,
                    }),
                    body: "done".to_string(),
                    payload: Some(serde_json::json!({
                        "request_intent": "checksum_token",
                        "token": "birch seventeen",
                    })),
                    blocks: None,
                    handling_mode: None,
                }),
            )
            .await
            .expect("accept terminal peer response");

        let open_config = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let open_config = runtime
                    .realtime_session_open_config(
                        &session_id,
                        meerkat_contracts::RealtimeTurningMode::ProviderManaged,
                    )
                    .await
                    .expect("realtime_session_open_config");
                let projected = open_config
                    .seed_messages
                    .iter()
                    .filter_map(|message| match message {
                        Message::System(system) => Some(system.content.to_lowercase()),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if projected.iter().any(|text| {
                    text.contains("peer_response_terminal:analyst-rt:req-123")
                        && text.contains("birch seventeen")
                }) {
                    break open_config;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("terminal peer response should reach realtime projection");

        let projected = open_config
            .seed_messages
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system.content.to_lowercase()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            projected.iter().any(|text| {
                text.contains("peer_response_terminal:analyst-rt:req-123")
                    && text.contains("birch seventeen")
            }),
            "expected realtime projection to include runtime-owned terminal peer response: {projected:?}"
        );
    }

    #[tokio::test]
    async fn recovery_restores_runtime_owned_terminal_peer_response_from_authoritative_snapshot() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut base_runtime = make_runtime_with_runtime_store(temp_factory(&temp), 10);
        base_runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(base_runtime);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");
        runtime
            .ensure_runtime_executor(&session_id)
            .await
            .expect("ensure_runtime_executor");

        runtime
            .runtime_adapter()
            .accept_input(
                &session_id,
                meerkat_runtime::Input::Peer(meerkat_runtime::PeerInput {
                    header: meerkat_runtime::InputHeader {
                        id: meerkat_core::lifecycle::InputId::new(),
                        timestamp: chrono::Utc::now(),
                        source: meerkat_runtime::InputOrigin::Peer {
                            peer_id: "analyst-rt".to_string(),
                            runtime_id: None,
                        },
                        durability: meerkat_runtime::InputDurability::Durable,
                        visibility: meerkat_runtime::InputVisibility::default(),
                        idempotency_key: None,
                        supersession_key: None,
                        correlation_id: None,
                    },
                    convention: Some(meerkat_runtime::PeerConvention::ResponseTerminal {
                        request_id: "req-123".to_string(),
                        status: meerkat_runtime::ResponseTerminalStatus::Completed,
                    }),
                    body: "done".to_string(),
                    payload: Some(serde_json::json!({
                        "request_intent": "checksum_token",
                        "token": "birch seventeen",
                    })),
                    blocks: None,
                    handling_mode: None,
                }),
            )
            .await
            .expect("accept terminal peer response");

        let authoritative = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let Some(session) = runtime
                    .load_persisted_session(&session_id)
                    .await
                    .expect("load authoritative session")
                else {
                    panic!("authoritative session missing");
                };
                let projected = session
                    .messages()
                    .iter()
                    .filter_map(|message| match message {
                        Message::System(system) => Some(system.content.to_lowercase()),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if projected.iter().any(|text| {
                    text.contains("peer_response_terminal:analyst-rt:req-123")
                        && text.contains("birch seventeen")
                }) {
                    break session;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("authoritative snapshot should include terminal peer response");

        let authoritative_projected = authoritative
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system.content.to_lowercase()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            authoritative_projected.iter().any(|text| {
                text.contains("peer_response_terminal:analyst-rt:req-123")
                    && text.contains("birch seventeen")
            }),
            "expected authoritative snapshot to include runtime-owned terminal peer response: {authoritative_projected:?}"
        );

        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard_live_session");

        let (event_tx, _event_rx) = mpsc::channel(100);
        runtime
            .try_recover_persisted_session(
                &session_id,
                "Recover".into(),
                event_tx,
                false,
                None,
                None,
                None,
            )
            .await
            .expect("try_recover_persisted_session");

        let open_config = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let open_config = runtime
                    .realtime_session_open_config(
                        &session_id,
                        meerkat_contracts::RealtimeTurningMode::ProviderManaged,
                    )
                    .await
                    .expect("realtime_session_open_config");
                let projected = open_config
                    .seed_messages
                    .iter()
                    .filter_map(|message| match message {
                        Message::System(system) => Some(system.content.to_lowercase()),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if projected.iter().any(|text| {
                    text.contains("peer_response_terminal:analyst-rt:req-123")
                        && text.contains("birch seventeen")
                }) {
                    break open_config;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("recovered realtime projection should include terminal peer response");

        let projected = open_config
            .seed_messages
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system.content.to_lowercase()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            projected.iter().any(|text| {
                text.contains("peer_response_terminal:analyst-rt:req-123")
                    && text.contains("birch seventeen")
            }),
            "expected recovered realtime projection to include authoritative terminal peer response: {projected:?}"
        );
    }

    #[tokio::test]
    async fn accept_input_with_completion_persists_runtime_owned_terminal_peer_response() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime_with_runtime_store(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(runtime);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");
        runtime
            .ensure_runtime_executor(&session_id)
            .await
            .expect("ensure_runtime_executor");

        let (_outcome, handle) = runtime
            .runtime_adapter()
            .accept_input_with_completion(
                &session_id,
                meerkat_runtime::Input::Peer(meerkat_runtime::PeerInput {
                    header: meerkat_runtime::InputHeader {
                        id: meerkat_core::lifecycle::InputId::new(),
                        timestamp: chrono::Utc::now(),
                        source: meerkat_runtime::InputOrigin::Peer {
                            peer_id: "analyst-rt".to_string(),
                            runtime_id: None,
                        },
                        durability: meerkat_runtime::InputDurability::Durable,
                        visibility: meerkat_runtime::InputVisibility::default(),
                        idempotency_key: None,
                        supersession_key: None,
                        correlation_id: None,
                    },
                    convention: Some(meerkat_runtime::PeerConvention::ResponseTerminal {
                        request_id: "req-123".to_string(),
                        status: meerkat_runtime::ResponseTerminalStatus::Completed,
                    }),
                    body: "done".to_string(),
                    payload: Some(serde_json::json!({
                        "request_intent": "checksum_token",
                        "token": "birch seventeen",
                    })),
                    blocks: None,
                    handling_mode: None,
                }),
            )
            .await
            .expect("accept terminal peer response with completion");
        if let Some(handle) = handle {
            let _ = handle.wait().await;
        }

        let authoritative = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let Some(session) = runtime
                    .load_persisted_session(&session_id)
                    .await
                    .expect("load authoritative session")
                else {
                    panic!("authoritative session missing");
                };
                let projected = session
                    .messages()
                    .iter()
                    .filter_map(|message| match message {
                        Message::System(system) => Some(system.content.to_lowercase()),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if projected.iter().any(|text| {
                    text.contains("peer_response_terminal:analyst-rt:req-123")
                        && text.contains("birch seventeen")
                }) {
                    break session;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("authoritative snapshot should include terminal peer response");

        let projected = authoritative
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system.content.to_lowercase()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            projected.iter().any(|text| {
                text.contains("peer_response_terminal:analyst-rt:req-123")
                    && text.contains("birch seventeen")
            }),
            "expected accept_input_with_completion to persist runtime-owned terminal peer response: {projected:?}"
        );
    }

    #[tokio::test]
    async fn accept_input_with_completion_emits_run_completed_for_runtime_owned_terminal_peer_response()
     {
        use futures::StreamExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime_with_runtime_store(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(runtime);

        let mut build = mock_build_config();
        build.keep_alive = true;
        build.comms_name = Some("runtime-owned-terminal-peer-response-events".to_string());
        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");
        runtime
            .ensure_runtime_executor(&session_id)
            .await
            .expect("ensure_runtime_executor");

        let mut events = runtime
            .service
            .subscribe_session_events(&session_id)
            .await
            .expect("subscribe_session_events");

        let (_outcome, handle) = runtime
            .runtime_adapter()
            .accept_input_with_completion(
                &session_id,
                meerkat_runtime::Input::Peer(meerkat_runtime::PeerInput {
                    header: meerkat_runtime::InputHeader {
                        id: meerkat_core::lifecycle::InputId::new(),
                        timestamp: chrono::Utc::now(),
                        source: meerkat_runtime::InputOrigin::Peer {
                            peer_id: "analyst-rt".to_string(),
                            runtime_id: None,
                        },
                        durability: meerkat_runtime::InputDurability::Durable,
                        visibility: meerkat_runtime::InputVisibility::default(),
                        idempotency_key: None,
                        supersession_key: None,
                        correlation_id: None,
                    },
                    convention: Some(meerkat_runtime::PeerConvention::ResponseTerminal {
                        request_id: "req-123".to_string(),
                        status: meerkat_runtime::ResponseTerminalStatus::Completed,
                    }),
                    body: "done".to_string(),
                    payload: Some(serde_json::json!({
                        "request_intent": "checksum_token",
                        "token": "birch seventeen",
                    })),
                    blocks: None,
                    handling_mode: None,
                }),
            )
            .await
            .expect("accept terminal peer response with completion");
        if let Some(handle) = handle {
            let _ = handle.wait().await;
        }

        let started = tokio::time::timeout(std::time::Duration::from_secs(3), events.next())
            .await
            .expect("run_started timeout")
            .expect("run_started event should exist");
        match started.payload {
            AgentEvent::RunStarted { prompt, .. } => {
                let normalized = prompt.text_content().to_lowercase();
                assert!(
                    normalized.contains("peer_response_terminal:analyst-rt:req-123"),
                    "run_started prompt should expose runtime system-context source: {normalized}"
                );
                assert!(
                    normalized.contains("birch seventeen"),
                    "run_started prompt should expose authoritative terminal peer payload: {normalized}"
                );
            }
            other => panic!("expected run_started, got {other:?}"),
        }

        let completed = tokio::time::timeout(std::time::Duration::from_secs(3), events.next())
            .await
            .expect("run_completed timeout")
            .expect("run_completed event should exist");
        match completed.payload {
            AgentEvent::RunCompleted { result, usage, .. } => {
                assert!(
                    result.is_empty(),
                    "context-only runtime apply should not synthesize assistant output: {result:?}"
                );
                assert_eq!(
                    usage,
                    meerkat_core::types::Usage::default(),
                    "context-only runtime apply should not report model usage"
                );
            }
            other => panic!("expected run_completed, got {other:?}"),
        }
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn keep_alive_comms_drain_emits_run_completed_for_terminal_peer_response() {
        use futures::StreamExt;
        use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
        use meerkat_core::comms::{CommsCommand, PeerName, PeerRoute, TrustedPeerDescriptor};
        use meerkat_core::interaction::InteractionId;

        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime_with_runtime_store(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        let runtime = Arc::new(runtime);

        let operator_name = "operator-drain-test";
        let mut build = mock_build_config();
        build.keep_alive = true;
        build.comms_name = Some(operator_name.to_string());
        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");
        runtime
            .ensure_runtime_executor(&session_id)
            .await
            .expect("ensure_runtime_executor");

        let operator_comms = runtime
            .comms_runtime(&session_id)
            .await
            .expect("session comms runtime");
        runtime
            .enable_comms_drain(&session_id, operator_comms.clone())
            .await;

        let sender = Arc::new(
            meerkat::CommsRuntime::inproc_only("analyst-drain-test").expect("sender comms runtime"),
        );
        let sender_public_key = sender.public_key();
        let sender_peer_id = sender_public_key.to_peer_id().to_string();
        let sender_addr = sender.advertised_address();
        let operator_peer_id = operator_comms.public_key().expect("operator peer id");
        let operator_addr = operator_comms
            .advertised_address()
            .expect("operator advertised address");
        let operator_pubkey =
            meerkat_comms::PubKey::from_pubkey_string(&operator_peer_id).expect("operator pubkey");

        CoreCommsRuntime::add_trusted_peer(
            &*sender,
            TrustedPeerDescriptor::unsigned_with_pubkey(
                operator_name,
                operator_pubkey.to_peer_id().to_string(),
                *operator_pubkey.as_bytes(),
                operator_addr,
            )
            .expect("operator trusted peer spec"),
        )
        .await
        .expect("sender trusts operator");
        CoreCommsRuntime::add_trusted_peer(
            operator_comms.as_ref(),
            TrustedPeerDescriptor::unsigned_with_pubkey(
                "analyst-drain-test",
                sender_peer_id,
                *sender_public_key.as_bytes(),
                sender_addr,
            )
            .expect("sender trusted peer spec"),
        )
        .await
        .expect("operator trusts sender");

        let mut events = runtime
            .service
            .subscribe_session_events(&session_id)
            .await
            .expect("subscribe_session_events");

        CoreCommsRuntime::send(
            &*sender,
            CommsCommand::PeerResponse {
                to: PeerRoute::with_display_name(
                    operator_pubkey.to_peer_id(),
                    PeerName::new(operator_name.to_string()).expect("valid operator peer name"),
                ),
                in_reply_to: InteractionId(uuid::Uuid::new_v4()),
                status: meerkat_core::ResponseStatus::Completed,
                result: serde_json::json!({
                    "request_intent": "checksum_token",
                    "token": "birch seventeen",
                }),
                handling_mode: None,
            },
        )
        .await
        .expect("send terminal peer response");

        let started = tokio::time::timeout(std::time::Duration::from_secs(5), events.next())
            .await
            .expect("run_started timeout")
            .expect("run_started event should exist");
        assert!(matches!(started.payload, AgentEvent::RunStarted { .. }));

        let completed = tokio::time::timeout(std::time::Duration::from_secs(5), events.next())
            .await
            .expect("run_completed timeout")
            .expect("run_completed event should exist");
        match completed.payload {
            AgentEvent::RunCompleted { result, usage, .. } => {
                assert!(
                    result.is_empty(),
                    "context-only drain apply should not synthesize assistant output"
                );
                assert_eq!(usage, meerkat_core::types::Usage::default());
            }
            other => panic!("expected run_completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn realtime_open_config_includes_runtime_context_applied_via_session_service() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let mut build = mock_build_config();
        build.keep_alive = true;
        build.comms_name = Some("realtime-open-config-runtime-append".to_string());
        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");

        runtime
            .service
            .apply_runtime_context_appends(
                &session_id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] Correlated peer response from analyst-rt. Request ID: req-123. Status: completed. Result: {\"request_intent\":\"checksum_token\",\"token\":\"birch seventeen\"}. For checksum_token requests, the exact token answer is `birch seventeen`.".to_string(),
                    source: Some("peer_response_terminal:analyst-rt:req-123".to_string()),
                    idempotency_key: Some("peer_response_terminal:analyst-rt:req-123".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![],
            )
            .await
            .expect("apply_runtime_context_appends");

        let open_config = runtime
            .realtime_session_open_config(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("realtime_session_open_config");

        let projected = open_config
            .seed_messages
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system.content.to_lowercase()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            projected.iter().any(|text| {
                text.contains("peer_response_terminal:analyst-rt:req-123")
                    && text.contains("birch seventeen")
            }),
            "expected realtime projection to include runtime context applied via session service: {projected:?}"
        );
    }

    #[tokio::test]
    async fn realtime_open_config_preserves_committed_external_dialogue_for_reconstruction() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let mut build = mock_build_config();
        build.keep_alive = true;
        build.comms_name = Some("realtime-open-config-dialogue-recap".to_string());
        build.additional_instructions = Some(vec![
            "Remember user-provided codewords verbatim.".to_string(),
            "Use authoritative terminal peer responses for token recall.".to_string(),
        ]);

        let session_id = runtime
            .create_session(build, None, None)
            .await
            .expect("create_session");
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_turn");

        runtime
            .append_external_user_content(
                &session_id,
                meerkat_core::types::ContentInput::Text(
                    "Remember the codeword amber lantern.".to_string(),
                ),
            )
            .await
            .expect("append remember user turn");
        runtime
            .append_external_assistant_output(
                &session_id,
                vec![meerkat_core::types::AssistantBlock::Text {
                    text: "Remembering amber lantern.".to_string(),
                    meta: None,
                }],
                meerkat_core::types::StopReason::EndTurn,
                meerkat_core::types::Usage::default(),
            )
            .await
            .expect("append remember assistant turn");
        runtime
            .append_external_user_content(
                &session_id,
                meerkat_core::types::ContentInput::Text("Ask analyst for the token.".to_string()),
            )
            .await
            .expect("append checksum request user turn");
        runtime
            .append_external_assistant_output(
                &session_id,
                vec![meerkat_core::types::AssistantBlock::Text {
                    text: "Waiting for analyst token.".to_string(),
                    meta: None,
                }],
                meerkat_core::types::StopReason::EndTurn,
                meerkat_core::types::Usage::default(),
            )
            .await
            .expect("append checksum request assistant turn");
        runtime
            .service
            .apply_runtime_context_appends(
                &session_id,
                RunId::new(),
                vec![PendingSystemContextAppend {
                    text: "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] Correlated peer response from analyst-rt. Request ID: req-123. Status: completed. Result: {\"request_intent\":\"checksum_token\",\"token\":\"birch seventeen\"}. For checksum_token requests, the exact token answer is `birch seventeen`.".to_string(),
                    source: Some("peer_response_terminal:analyst-rt:req-123".to_string()),
                    idempotency_key: Some("peer_response_terminal:analyst-rt:req-123".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                vec![],
            )
            .await
            .expect("apply runtime context");

        let open_config = runtime
            .realtime_session_open_config(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("realtime_session_open_config");

        let user_messages = open_config
            .seed_messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let assistant_messages = open_config
            .seed_messages
            .iter()
            .filter_map(|message| match message {
                Message::Assistant(assistant) => Some(assistant.content.clone()),
                Message::BlockAssistant(assistant) => {
                    Some(assistant.text_blocks().collect::<Vec<_>>().join("\n"))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let system_messages = open_config
            .seed_messages
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system.content.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert!(
            user_messages
                .iter()
                .any(|text| text.contains("Remember the codeword amber lantern.")),
            "expected realtime projection to preserve remembered user turn: {user_messages:?}"
        );
        assert!(
            assistant_messages
                .iter()
                .any(|text| text.contains("Remembering amber lantern.")),
            "expected realtime projection to preserve remembered assistant turn: {assistant_messages:?}"
        );
        assert!(
            system_messages.iter().any(|text| {
                text.contains("peer_response_terminal:analyst-rt:req-123")
                    && text.contains("birch seventeen")
            }),
            "expected realtime projection to preserve authoritative token system context: {system_messages:?}"
        );
    }

    fn make_runtime(factory: AgentFactory, max_sessions: usize) -> SessionRuntime {
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        SessionRuntime::new(
            factory,
            Config::default(),
            max_sessions,
            meerkat::PersistenceBundle::new(store, None, blob_store),
            crate::router::NotificationSink::noop(),
        )
    }

    fn make_runtime_with_runtime_store(
        factory: AgentFactory,
        max_sessions: usize,
    ) -> SessionRuntime {
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        SessionRuntime::new(
            factory,
            Config::default(),
            max_sessions,
            meerkat::PersistenceBundle::new(store, Some(runtime_store), blob_store),
            crate::router::NotificationSink::noop(),
        )
    }

    fn self_hosted_test_config(server: &str, inline_video: bool) -> Config {
        let mut config = Config::default();
        config.self_hosted.servers.insert(
            server.to_string(),
            meerkat_core::SelfHostedServerConfig {
                transport: meerkat_core::SelfHostedTransport::OpenAiCompatible,
                base_url: format!(
                    "http://127.0.0.1:{}",
                    if server == "local" { 11434 } else { 22434 }
                ),
                api_style: meerkat_core::SelfHostedApiStyle::ChatCompletions,
                bearer_token: None,
                bearer_token_env: None,
            },
        );
        config.self_hosted.models.insert(
            "gemma-4-e2b".to_string(),
            meerkat_core::SelfHostedModelConfig {
                server: server.to_string(),
                remote_model: "gemma4:e2b".to_string(),
                display_name: "Gemma 4 E2B".to_string(),
                family: "gemma-4".to_string(),
                tier: meerkat_models::ModelTier::Supported,
                context_window: Some(128_000),
                max_output_tokens: Some(8_192),
                vision: true,
                image_tool_results: true,
                inline_video,
                supports_temperature: true,
                supports_thinking: true,
                supports_reasoning: true,
                supports_web_search: false,
                call_timeout_secs: Some(600),
            },
        );
        config
    }

    fn inline_video_prompt() -> ContentInput {
        ContentInput::Blocks(vec![meerkat_core::ContentBlock::Video {
            media_type: "video/mp4".to_string(),
            duration_ms: 1000,
            data: meerkat_core::VideoData::Inline {
                data: "AAAA".to_string(),
            },
        }])
    }

    #[cfg(feature = "mcp")]
    fn mcp_test_server_path() -> PathBuf {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        let workspace_root = PathBuf::from(manifest_dir)
            .parent()
            .expect("workspace root")
            .to_path_buf();
        workspace_root
            .join("target")
            .join("debug")
            .join("mcp-test-server")
    }

    #[cfg(feature = "mcp")]
    fn maybe_mcp_server_config(server_name: &str) -> Option<serde_json::Value> {
        let path = mcp_test_server_path();
        if !path.exists() {
            eprintln!(
                "Skipping MCP runtime boundary test: mcp-test-server not built. Run `cargo build -p mcp-test-server` first."
            );
            return None;
        }
        Some(serde_json::json!({
            "command": path.to_string_lossy().to_string(),
            "args": [],
            "env": {},
            "name": server_name,
        }))
    }

    #[tokio::test]
    async fn create_session_accepts_video_for_self_hosted_alias_from_runtime_config() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        let store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(
            meerkat_core::MemoryConfigStore::new(self_hosted_test_config("local", true)),
        );
        runtime.set_config_runtime(Arc::new(meerkat_core::ConfigRuntime::new(
            store,
            temp.path().join("config_state.json"),
        )));

        let session_id = runtime
            .create_session(
                AgentBuildConfig {
                    llm_client_override: Some(Arc::new(MockLlmClient)),
                    ..AgentBuildConfig::new("gemma-4-e2b")
                },
                None,
                Some(inline_video_prompt()),
            )
            .await
            .expect("self-hosted inline-video alias should validate against runtime registry");

        assert!(runtime.pending_session_exists(&session_id).await);
    }

    #[tokio::test]
    async fn create_session_pins_self_hosted_server_id_before_materialization() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        let store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(
            meerkat_core::MemoryConfigStore::new(self_hosted_test_config("local", false)),
        );
        let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
            store,
            temp.path().join("config_state.json"),
        ));
        runtime.set_config_runtime(config_runtime.clone());

        let session_id = runtime
            .create_session(
                AgentBuildConfig {
                    llm_client_override: Some(Arc::new(MockLlmClient)),
                    ..AgentBuildConfig::new("gemma-4-e2b")
                },
                None,
                None,
            )
            .await
            .expect("pending self-hosted session should stage");

        config_runtime
            .set(self_hosted_test_config("other", false), None)
            .await
            .expect("config patch");

        let staged = runtime
            .staged_sessions
            .effective_llm_identity(&session_id)
            .await
            .expect("pending session");
        assert_eq!(
            staged.self_hosted_server_id.as_deref(),
            Some("local"),
            "pending sessions must remain pinned to the server selected at create time"
        );
    }

    #[tokio::test]
    async fn provider_param_override_keeps_pinned_self_hosted_server_binding() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        let store: Arc<dyn meerkat_core::ConfigStore> = Arc::new(
            meerkat_core::MemoryConfigStore::new(self_hosted_test_config("local", false)),
        );
        let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
            store,
            temp.path().join("config_state.json"),
        ));
        runtime.set_config_runtime(config_runtime.clone());

        config_runtime
            .set(self_hosted_test_config("other", false), None)
            .await
            .expect("config patch");

        let current = SessionLlmIdentity {
            model: "gemma-4-e2b".to_string(),
            provider: meerkat_core::Provider::SelfHosted,
            self_hosted_server_id: Some("local".to_string()),
            provider_params: None,
            connection_ref: None,
        };
        let overrides = crate::handlers::turn::TurnOverrides {
            provider_params: Some(serde_json::json!({ "temperature": 0.2 })),
            ..Default::default()
        };

        let resolved = runtime
            .resolve_target_llm_identity(&current, &overrides)
            .await
            .expect("provider-param override should resolve");

        assert_eq!(
            resolved.self_hosted_server_id.as_deref(),
            Some("local"),
            "provider-param overrides must preserve the pinned self-hosted server binding"
        );
    }

    #[tokio::test]
    async fn turn_override_connection_ref_propagates_to_resolved_identity() {
        // Dogma §10 (inherit/set) + Wave 3 row 15: an explicit
        // `connection_ref` override on a turn must flow through
        // `resolve_target_llm_identity` onto the resolved identity —
        // NOT be silently dropped to None. Guards against the earlier
        // bug where hot-swap inherited stale binding even when the
        // caller asked for a different realm.
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let current = SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            connection_ref: Some(meerkat_core::ConnectionRef {
                realm: meerkat_core::RealmId::parse("tenant_a").expect("valid realm"),
                binding: meerkat_core::BindingId::parse("anthropic_default")
                    .expect("valid binding"),
                profile: None,
            }),
        };
        let overrides = crate::handlers::turn::TurnOverrides {
            connection_ref: Some(meerkat_core::ConnectionRef {
                realm: meerkat_core::RealmId::parse("tenant_b").expect("valid realm"),
                binding: meerkat_core::BindingId::parse("anthropic_vip").expect("valid binding"),
                profile: None,
            }),
            ..Default::default()
        };

        let resolved = runtime
            .resolve_target_llm_identity(&current, &overrides)
            .await
            .expect("connection_ref override should resolve");

        assert_eq!(
            resolved.connection_ref.as_ref().map(|c| c.realm.as_str()),
            Some("tenant_b"),
            "explicit connection_ref override must win over the session's current binding"
        );
        assert_eq!(
            resolved.connection_ref.as_ref().map(|c| c.binding.as_str()),
            Some("anthropic_vip"),
        );
    }

    #[tokio::test]
    async fn turn_without_connection_ref_override_preserves_current_binding() {
        // Dogma §10 inherit semantics: `None` on the override does
        // NOT mean "clear the binding" — it means "keep the current
        // one". The earlier bug returned None either way, breaking
        // multi-tenant sessions whose next turn happened to set
        // another override (model/provider_params) alongside no
        // explicit connection_ref.
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let current = SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            connection_ref: Some(meerkat_core::ConnectionRef {
                realm: meerkat_core::RealmId::parse("tenant_a").expect("valid realm"),
                binding: meerkat_core::BindingId::parse("anthropic_default")
                    .expect("valid binding"),
                profile: None,
            }),
        };
        let overrides = crate::handlers::turn::TurnOverrides {
            provider_params: Some(serde_json::json!({ "temperature": 0.1 })),
            ..Default::default()
        };

        let resolved = runtime
            .resolve_target_llm_identity(&current, &overrides)
            .await
            .expect("resolve must succeed");

        assert_eq!(
            resolved.connection_ref.as_ref().map(|c| c.realm.as_str()),
            Some("tenant_a"),
            "absent connection_ref override must inherit the session's current binding, not drop it"
        );
    }

    #[cfg(feature = "mcp")]
    fn collect_tool_config_events(
        event_rx: &mut mpsc::Receiver<EventEnvelope<AgentEvent>>,
    ) -> Vec<ToolConfigChangedPayload> {
        let mut out = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            if let AgentEvent::ToolConfigChanged { payload } = event.payload {
                out.push(payload);
            }
        }
        out
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    /// 1. Creating a session returns a SessionId and the state is Idle.
    #[tokio::test]
    async fn create_session_returns_id_and_idle_state() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        let info = runtime.session_state(&session_id).await;
        assert_eq!(info.map(|i| i.state), Some(SessionState::Idle));
    }

    /// 2. Starting a turn returns a RunResult with expected text.
    #[tokio::test]
    async fn start_turn_returns_run_result() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);

        let result = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        assert!(
            result.text.contains("Hello from mock"),
            "Expected mock response text, got: {}",
            result.text
        );
    }

    #[tokio::test]
    async fn append_system_context_survives_pending_session_promotion() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        let session_id = runtime
            .create_session(slow_build_config(200), None, None)
            .await
            .unwrap();

        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_clone = Arc::clone(&runtime);
        let sid_clone = session_id.clone();
        let turn_handle = tokio::spawn(async move {
            runtime_clone
                .start_turn(&sid_clone, "Hello".into(), event_tx, None, None, None, None)
                .await
        });

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            let is_promoting = runtime
                .staged_sessions
                .info(&session_id)
                .await
                .map(|info| info.is_promoting)
                .unwrap_or(false);
            if is_promoting {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "session did not enter the promoting state before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        let append_req = AppendSystemContextRequest {
            text: "Coordinate with the orchestrator.".to_string(),
            source: Some("mob".to_string()),
            idempotency_key: Some("ctx-promotion".to_string()),
        };
        let append_result = runtime
            .append_system_context(&session_id, append_req.clone())
            .await
            .expect("append during promotion should succeed");
        assert_eq!(
            append_result.status,
            meerkat_core::AppendSystemContextStatus::Staged
        );

        let result = turn_handle.await.unwrap().unwrap();
        assert!(result.text.contains("Slow response"));

        let duplicate = runtime
            .append_system_context(&session_id, append_req)
            .await
            .expect("replayed append should be visible after materialization");
        assert_eq!(
            duplicate.status,
            meerkat_core::AppendSystemContextStatus::Duplicate
        );
    }

    /// 3. Verify state transitions: Idle -> Running -> Idle during a turn.
    #[tokio::test]
    async fn start_turn_transitions_idle_running_idle() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        // Use a slow mock to give us time to observe Running state
        let session_id = runtime
            .create_session(slow_build_config(100), None, None)
            .await
            .unwrap();

        // Verify initial state
        assert_eq!(
            runtime.session_state(&session_id).await.map(|i| i.state),
            Some(SessionState::Idle)
        );

        // Start the turn in a background task so we can observe state mid-run
        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_clone = runtime.clone();
        let sid_clone = session_id.clone();

        let turn_handle = tokio::spawn(async move {
            runtime_clone
                .start_turn(&sid_clone, "Hello".into(), event_tx, None, None, None, None)
                .await
        });

        // Wait until the session transitions to Running.
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            if runtime.session_state(&session_id).await.map(|i| i.state)
                == Some(SessionState::Running)
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "session did not enter running state before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Wait for the turn to complete
        let result = turn_handle.await.unwrap().unwrap();
        assert!(result.text.contains("Slow response"));

        // After completion, state should be Idle again
        assert_eq!(
            runtime.session_state(&session_id).await.map(|i| i.state),
            Some(SessionState::Idle),
            "Session should be Idle after turn completes"
        );
    }

    /// 4. Starting a second turn while one is running fails with SESSION_BUSY.
    #[tokio::test]
    async fn start_turn_on_busy_session_fails() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        let session_id = runtime
            .create_session(slow_build_config(200), None, None)
            .await
            .unwrap();

        // Start first turn in background
        let (event_tx1, _rx1) = mpsc::channel(100);
        let runtime_clone = runtime.clone();
        let sid_clone = session_id.clone();
        let _turn_handle = tokio::spawn(async move {
            runtime_clone
                .start_turn(
                    &sid_clone,
                    "First".into(),
                    event_tx1,
                    None,
                    None,
                    None,
                    None,
                )
                .await
        });

        // Wait until the first turn is definitely running.
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            if runtime.session_state(&session_id).await.map(|i| i.state)
                == Some(SessionState::Running)
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "session did not enter running state before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Try to start a second turn
        let (event_tx2, _rx2) = mpsc::channel(100);
        let result = runtime
            .start_turn(
                &session_id,
                "Second".into(),
                event_tx2,
                None,
                None,
                None,
                None,
            )
            .await;

        assert!(result.is_err(), "Second turn should fail");
        let err = result.unwrap_err();
        assert_eq!(
            err.code,
            error::SESSION_BUSY,
            "Error code should be SESSION_BUSY, got: {}",
            err.code
        );
    }

    /// 5. Agent events are forwarded through the per-turn event channel.
    #[tokio::test]
    async fn start_turn_emits_events() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, mut event_rx) = mpsc::channel(100);

        let _result = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Collect received events
        let mut events = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            events.push(event);
        }

        // We should have received at least some events (RunStarted, TextDelta, etc.)
        assert!(
            !events.is_empty(),
            "Should have received at least one event"
        );

        // Check that we got a RunStarted event
        let has_run_started = events
            .iter()
            .any(|e| matches!(e.payload, AgentEvent::RunStarted { .. }));
        assert!(has_run_started, "Should have received a RunStarted event");
    }

    /// 6. Interrupting an idle session is a no-op (no error).
    #[tokio::test]
    async fn interrupt_on_idle_is_noop() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Interrupt while idle should succeed without error
        let result = runtime.interrupt(&session_id).await;
        assert!(result.is_ok(), "Interrupt on idle should not fail");

        // State should still be idle
        assert_eq!(
            runtime.session_state(&session_id).await.map(|i| i.state),
            Some(SessionState::Idle)
        );
    }

    /// 7. Archiving a session removes it from the runtime.
    #[tokio::test]
    async fn archive_session_removes_handle() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Verify it exists
        assert!(runtime.session_state(&session_id).await.is_some());

        // Archive it
        runtime.archive_session(&session_id).await.unwrap();

        // Verify it's gone
        assert!(
            runtime.session_state(&session_id).await.is_none(),
            "Archived session should no longer exist"
        );

        // Archiving again should fail
        let result = runtime.archive_session(&session_id).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, error::SESSION_NOT_FOUND);
    }

    /// set_mob_tools writes through to the builder, so sessions get mob tools.
    #[tokio::test]
    async fn set_mob_tools_delivers_tools_to_created_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"))
            .builtins(true)
            .mob(true);
        let mut runtime = make_runtime(factory, 10);

        // Create a MobMcpState and set it via set_mob_tools.
        let mob_svc = runtime.session_service();
        let mob_state = Arc::new(meerkat_mob_mcp::MobMcpState::new(mob_svc));
        runtime.set_mob_tools(Arc::new(meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(
            mob_state,
        )));

        // Create a session — the agent should have mob tools.
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Read the session and check tool names. The session's tool list
        // should include mob tools (delegate, mob_create, etc.).
        let info = runtime.session_state(&session_id).await.unwrap();
        assert_eq!(info.state, SessionState::Idle);

        // Start a turn to materialize the agent, then check tool list
        // via a build that captures tool names.
        // (We can't directly inspect the agent's tools, but we can verify
        // the mob tools factory was injected by checking the builder slot.)
        let slot = runtime.builder_mob_tools_slot.read().unwrap();
        assert!(
            slot.is_some(),
            "builder_mob_tools_slot should be set after set_mob_tools"
        );
    }

    /// 8. Max sessions limit is enforced.
    #[tokio::test]
    async fn max_sessions_enforced() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 2);

        // Create two sessions (the max)
        let _s1 = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let _s2 = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Third should fail
        let result = runtime
            .create_session(mock_build_config(), None, None)
            .await;
        assert!(result.is_err(), "Third session should fail");
        let err = result.unwrap_err();
        assert!(
            err.message.contains("Max sessions"),
            "Error message should mention max sessions, got: {}",
            err.message
        );
    }

    /// 9. session_state returns None for an unknown session ID.
    #[tokio::test]
    async fn session_state_none_for_unknown() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let unknown_id = SessionId::new();
        assert_eq!(runtime.session_state(&unknown_id).await, None);
    }

    /// 10. Shutdown closes all sessions.
    #[tokio::test]
    async fn shutdown_closes_all_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let s1 = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let s2 = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Verify both exist
        assert!(runtime.session_state(&s1).await.is_some());
        assert!(runtime.session_state(&s2).await.is_some());

        // Shutdown
        runtime.shutdown().await;

        // Both should be gone from the sessions map
        let sessions = runtime.list_sessions(Default::default()).await;
        assert!(
            sessions.is_empty(),
            "All sessions should be removed after shutdown"
        );
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn start_turn_applies_staged_mcp_ops_at_turn_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        runtime
            .mcp_stage_add(
                &session_id,
                "broken-server".to_string(),
                serde_json::json!({
                    "command": "echo",
                    "args": [],
                    "env": {}
                }),
            )
            .await
            .expect("staging should succeed");

        // Non-blocking: apply_staged spawns background task and succeeds
        // (the broken server fails asynchronously, not at staging time).
        let (event_tx, mut event_rx) = mpsc::channel(32);
        let first = runtime
            .start_turn(
                &session_id,
                "hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            first.is_ok(),
            "non-blocking apply_staged should not fail synchronously"
        );
        let first_events = collect_tool_config_events(&mut event_rx);
        assert!(
            first_events.iter().any(|payload| {
                payload.operation == ToolConfigChangeOperation::Add
                    && payload.target == "broken-server"
                    && payload.status == "pending"
            }),
            "should emit pending event for staged add"
        );

        let (event_tx, _event_rx) = mpsc::channel(32);
        let second = runtime
            .start_turn(
                &session_id,
                "hello again".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(
            second.is_ok(),
            "failed staged operation should not poison subsequent turns"
        );
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn start_turn_applies_staged_mcp_remove_and_reload_at_turn_boundary() {
        let Some(server_config) = maybe_mcp_server_config("test-server") else {
            return;
        };
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        runtime
            .mcp_stage_add(&session_id, "test-server".to_string(), server_config)
            .await
            .expect("stage add");

        // Non-blocking: add is now async — boundary emits "pending" not "applied".
        let (event_tx, mut event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn add".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("turn add should apply staged add");
        let add_events = collect_tool_config_events(&mut event_rx);
        assert!(add_events.iter().any(|payload| {
            payload.operation == ToolConfigChangeOperation::Add
                && payload.target == "test-server"
                && payload.status == "pending"
        }));

        runtime
            .mcp_stage_remove(&session_id, "test-server".to_string())
            .await
            .expect("stage remove");
        let (event_tx, mut event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn remove".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("turn remove should apply staged remove");
        let remove_events = collect_tool_config_events(&mut event_rx);
        assert!(remove_events.iter().any(|payload| {
            payload.operation == ToolConfigChangeOperation::Remove
                && payload.target == "test-server"
                && (payload.status == "applied" || payload.status == "draining")
        }));
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn async_mcp_removal_timeout_is_emitted_on_next_boundary() {
        let Some(server_config) = maybe_mcp_server_config("timeout-server") else {
            return;
        };
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        runtime
            .mcp_stage_add(&session_id, "timeout-server".to_string(), server_config)
            .await
            .expect("stage add");

        let (event_tx, _event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn add".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("add boundary");

        let adapter = runtime
            .mcp_adapter_for_session(&session_id)
            .await
            .expect("mcp adapter");

        // Wait for the background MCP connect to complete before setting
        // inflight calls — otherwise `drain_pending` on the next boundary
        // replaces the entry and resets active_calls to 0.
        adapter.wait_until_ready(Duration::from_secs(5)).await;

        adapter
            .set_removal_timeout_for_testing(Duration::from_millis(20))
            .await
            .expect("set timeout");
        adapter
            .set_inflight_calls_for_testing("timeout-server", 1)
            .await
            .expect("set inflight");

        runtime
            .mcp_stage_remove(&session_id, "timeout-server".to_string())
            .await
            .expect("stage remove");

        let (event_tx, mut event_rx) = mpsc::channel(128);
        runtime
            .start_turn(
                &session_id,
                "turn remove".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("remove boundary");
        let first_turn_events = collect_tool_config_events(&mut event_rx);
        assert!(first_turn_events.iter().any(|payload| {
            payload.operation == ToolConfigChangeOperation::Remove
                && payload.target == "timeout-server"
                && payload.status == "draining"
        }));

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !adapter
                    .has_removing_servers()
                    .await
                    .expect("check removing state")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("timed out waiting for forced removal to finalize");

        let (event_tx, mut event_rx) = mpsc::channel(128);
        runtime
            .start_turn(
                &session_id,
                "turn after timeout".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("follow-up boundary");
        let second_turn_events = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let events = collect_tool_config_events(&mut event_rx);
                if !events.is_empty() {
                    break events;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_default();
        assert!(
            second_turn_events.iter().any(|payload| {
                payload.operation == ToolConfigChangeOperation::Remove
                    && payload.target == "timeout-server"
                    && (payload.status == "forced" || payload.status == "applied")
            }),
            "expected forced removal event on a follow-up boundary, got: {second_turn_events:?}"
        );
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    #[ignore = "integration-real: requires mcp-test-server binary and real process spawning"]
    async fn staged_ops_remain_boundary_gated_while_background_drain_runs() {
        let Some(server1_config) = maybe_mcp_server_config("server-draining") else {
            return;
        };
        let Some(server2_config) = maybe_mcp_server_config("server-staged") else {
            return;
        };

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        runtime
            .mcp_stage_add(&session_id, "server-draining".to_string(), server1_config)
            .await
            .expect("stage add draining server");
        let (event_tx, _event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn add first".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("add first server at boundary");

        let adapter = runtime
            .mcp_adapter_for_session(&session_id)
            .await
            .expect("mcp adapter");
        adapter
            .set_removal_timeout_for_testing(Duration::from_secs(3))
            .await
            .expect("set long timeout");
        adapter
            .set_inflight_calls_for_testing("server-draining", 1)
            .await
            .expect("keep server draining");

        runtime
            .mcp_stage_remove(&session_id, "server-draining".to_string())
            .await
            .expect("stage remove");
        let (event_tx, mut event_rx) = mpsc::channel(128);
        runtime
            .start_turn(
                &session_id,
                "turn remove first".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("remove starts draining");
        let first_turn_events = collect_tool_config_events(&mut event_rx);
        assert!(first_turn_events.iter().any(|payload| {
            payload.operation == ToolConfigChangeOperation::Remove
                && payload.target == "server-draining"
                && payload.status == "draining"
        }));
        assert!(
            adapter.tools().is_empty(),
            "draining server should be hidden immediately"
        );

        runtime
            .mcp_stage_add(&session_id, "server-staged".to_string(), server2_config)
            .await
            .expect("stage second add");

        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            adapter.tools().is_empty(),
            "background drain must not apply newly staged add outside boundary"
        );

        // Turn 3: apply the staged add. Since MCP adds are non-blocking
        // (spawn_pending), this turn will emit PendingConnect ("pending"),
        // not "applied". The actual connection completes asynchronously.
        let (event_tx, mut event_rx) = mpsc::channel(128);
        runtime
            .start_turn(
                &session_id,
                "turn apply staged add".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("next boundary should apply staged add");

        let next_turn_events = collect_tool_config_events(&mut event_rx);
        assert!(
            next_turn_events.iter().any(|payload| {
                payload.operation == ToolConfigChangeOperation::Add
                    && payload.target == "server-staged"
                    && payload.status == "pending"
            }),
            "expected Add+pending for server-staged at boundary, got: {next_turn_events:?}"
        );

        // Turn 4: after the background connection resolves, drain_pending
        // picks it up and the server becomes visible.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let (event_tx, mut event_rx) = mpsc::channel(128);
        runtime
            .start_turn(
                &session_id,
                "turn drain pending add".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("drain should resolve pending add");

        let drain_events = collect_tool_config_events(&mut event_rx);
        // The server becomes active via drain_pending → process_pending_result
        // → Activated lifecycle action. The event status may be "applied" (from
        // emit_mcp_lifecycle_events) or "activated" (from session service tool
        // scope machinery). Either confirms the server was successfully connected.
        let has_add = drain_events.iter().any(|payload| {
            payload.operation == ToolConfigChangeOperation::Add && payload.target == "server-staged"
        });
        assert!(
            has_add,
            "expected Add event for server-staged after drain, got: {drain_events:?}"
        );
        assert!(
            !adapter.tools().is_empty(),
            "second server should become visible after drain"
        );
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    #[ignore = "integration-real: requires mcp-test-server binary and real process spawning"]
    async fn queued_lifecycle_actions_survive_boundary_apply_failure() {
        let Some(server_config) = maybe_mcp_server_config("lossless-server") else {
            return;
        };

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        runtime
            .mcp_stage_add(&session_id, "lossless-server".to_string(), server_config)
            .await
            .expect("stage add");
        let (event_tx, _event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn add".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("add boundary");

        let adapter = runtime
            .mcp_adapter_for_session(&session_id)
            .await
            .expect("mcp adapter");
        adapter
            .set_removal_timeout_for_testing(Duration::from_millis(20))
            .await
            .expect("set timeout");
        adapter
            .set_inflight_calls_for_testing("lossless-server", 1)
            .await
            .expect("set inflight");

        runtime
            .mcp_stage_remove(&session_id, "lossless-server".to_string())
            .await
            .expect("stage remove");
        let (event_tx, _event_rx) = mpsc::channel(64);
        runtime
            .start_turn(
                &session_id,
                "turn remove".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("remove boundary");

        // Wait for the background drain task to process the forced removal.
        // Drain task polls every 100ms, timeout is 20ms, so after 500ms
        // the forced removal should be queued in lifecycle_rx.
        tokio::time::sleep(Duration::from_millis(500)).await;

        runtime
            .mcp_stage_add(
                &session_id,
                "broken-server".to_string(),
                serde_json::json!({
                    "command": "echo",
                    "args": [],
                    "env": {}
                }),
            )
            .await
            .expect("stage broken add");

        let (event_tx, mut event_rx) = mpsc::channel(128);
        // The broken add (echo) spawns a background connection that will fail
        // asynchronously. apply_staged() may or may not return an error since
        // MCP adds are non-blocking. The important assertion is that the
        // lossless-server's forced removal event was emitted via the lifecycle
        // channel despite the broken add also being staged.
        let _result = runtime
            .start_turn(
                &session_id,
                "turn with broken add and queued removal".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;

        let fail_turn_events = collect_tool_config_events(&mut event_rx);
        assert!(
            fail_turn_events.iter().any(|payload| {
                payload.operation == ToolConfigChangeOperation::Remove
                    && payload.target == "lossless-server"
                    && (payload.status == "forced" || payload.status == "applied")
            }),
            "removal of lossless-server should survive alongside broken add, got: {fail_turn_events:?}"
        );
    }

    /// 11. Startup registry build rejects invalid lineage/remap config.
    #[test]
    fn build_skill_identity_registry_rejects_invalid_config() {
        let cfg = Config {
            skills: SkillsConfig {
                repositories: vec![
                    SkillRepositoryConfig {
                        name: "old".to_string(),
                        source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                            .expect("uuid"),
                        transport: SkillRepoTransport::Filesystem {
                            path: ".rkat/skills-old".to_string(),
                        },
                    },
                    SkillRepositoryConfig {
                        name: "new-a".to_string(),
                        source_uuid: SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7")
                            .expect("uuid"),
                        transport: SkillRepoTransport::Filesystem {
                            path: ".rkat/skills-a".to_string(),
                        },
                    },
                    SkillRepositoryConfig {
                        name: "new-b".to_string(),
                        source_uuid: SourceUuid::parse("e8df561d-d38f-4242-af55-3a6efb34c950")
                            .expect("uuid"),
                        transport: SkillRepoTransport::Filesystem {
                            path: ".rkat/skills-b".to_string(),
                        },
                    },
                ],
                identity: SkillsIdentityConfig {
                    lineage: vec![SourceIdentityLineage {
                        event_id: "split-1".to_string(),
                        recorded_at_unix_secs: 1,
                        required_from_skills: vec![
                            SkillName::parse("email-extractor").expect("skill"),
                            SkillName::parse("pdf-processing").expect("skill"),
                        ],
                        event: SourceIdentityLineageEvent::Split {
                            from: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                                .expect("uuid"),
                            into: vec![
                                SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7")
                                    .expect("uuid"),
                                SourceUuid::parse("e8df561d-d38f-4242-af55-3a6efb34c950")
                                    .expect("uuid"),
                            ],
                        },
                    }],
                    remaps: vec![SkillKeyRemap {
                        from: SkillKey {
                            source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                                .expect("uuid"),
                            skill_name: SkillName::parse("email-extractor").expect("skill"),
                        },
                        to: SkillKey {
                            source_uuid: SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7")
                                .expect("uuid"),
                            skill_name: SkillName::parse("mail-extractor").expect("skill"),
                        },
                        reason: None,
                    }],
                    aliases: vec![],
                },
                ..SkillsConfig::default()
            },
            ..Config::default()
        };

        let result = SessionRuntime::build_skill_identity_registry(&cfg, None, None);
        assert!(matches!(
            result,
            Err(meerkat_core::skills::SkillError::MissingSkillRemaps { .. })
        ));
    }

    // -----------------------------------------------------------------------
    // Phase 1 tests: Deferred create, keep_alive override, per-turn overrides
    // -----------------------------------------------------------------------

    /// Deferred session/create returns a session in Idle state (no turn executed).
    #[tokio::test]
    async fn deferred_create_returns_idle_session() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Session exists and is idle (no turn started).
        let info = runtime.session_state(&session_id).await.unwrap();
        assert_eq!(info.state, SessionState::Idle);
    }

    /// Deferred create followed by turn/start works correctly.
    #[tokio::test]
    async fn deferred_create_then_turn_start_works() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        let (event_tx, _event_rx) = mpsc::channel(100);
        let result = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        assert!(result.text.contains("Hello from mock"));
    }

    /// turn/start with keep_alive override on a materialized session is not rejected.
    #[tokio::test]
    async fn turn_start_with_keep_alive_override_accepted_on_materialized() {
        use crate::handlers::turn::TurnOverrides;

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Materialize the session with a first turn.
        let (event_tx, _rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "First".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Second turn with only keep_alive override (no model/provider etc.)
        // should not be rejected — keep_alive is allowed on materialized sessions.
        // Use keep_alive: false to avoid needing comms runtime.
        let (event_tx, _rx) = mpsc::channel(100);
        let overrides = TurnOverrides {
            keep_alive: Some(false),
            ..Default::default()
        };
        let result = runtime
            .start_turn(
                &session_id,
                "Second".into(),
                event_tx,
                None,
                None,
                None,
                Some(overrides),
            )
            .await;
        assert!(
            result.is_ok(),
            "keep_alive override should succeed on materialized session"
        );
    }

    /// turn/start on a pending session with model override applies it.
    #[tokio::test]
    async fn turn_start_on_pending_session_with_model_override() {
        use crate::handlers::turn::TurnOverrides;

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Start turn with model override on pending session.
        let (event_tx, _rx) = mpsc::channel(100);
        let overrides = TurnOverrides {
            model: Some("claude-opus-4-6".to_string()),
            ..Default::default()
        };
        let result = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                Some(overrides),
            )
            .await;
        assert!(
            result.is_ok(),
            "model override should succeed on pending session"
        );
    }

    #[tokio::test]
    async fn pending_session_read_reports_effective_llm_identity() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        let info = runtime
            .read_session_rich(&session_id)
            .await
            .expect("pending session should be readable");
        assert_eq!(info.model, "claude-sonnet-4-5");
        assert_eq!(info.provider, "anthropic");
    }

    #[tokio::test]
    async fn turn_start_on_pending_session_rejects_provider_only_override() {
        use crate::handlers::turn::TurnOverrides;

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        let (event_tx, _rx) = mpsc::channel(100);
        let overrides = TurnOverrides {
            provider: Some("openai".to_string()),
            ..Default::default()
        };
        let err = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .expect_err("provider-only override should be rejected on pending sessions too");
        assert_eq!(err.code, error::INVALID_PARAMS);
        assert!(
            err.message.contains("provider override requires model"),
            "unexpected error message: {}",
            err.message
        );
    }

    /// turn/start on a materialized session allows model override (hot-swap).
    #[tokio::test]
    async fn turn_start_on_materialized_session_allows_model_override() {
        use crate::handlers::turn::TurnOverrides;

        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Materialize the session.
        let (event_tx, _rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "First".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Second turn with model override should NOT be rejected with INVALID_PARAMS.
        // In CI (no API key), the client build may fail with INTERNAL_ERROR, which
        // is fine — the point is that the override is accepted, not rejected at the
        // parameter validation layer.
        let (event_tx, _rx) = mpsc::channel(100);
        let overrides = TurnOverrides {
            model: Some("claude-opus-4-6".to_string()),
            ..Default::default()
        };
        let result = runtime
            .start_turn(
                &session_id,
                "Second".into(),
                event_tx,
                None,
                None,
                None,
                Some(overrides),
            )
            .await;
        if let Err(ref err) = result {
            assert_ne!(
                err.code,
                error::INVALID_PARAMS,
                "model override should not be rejected on materialized session: {err:?}"
            );
        }
    }

    /// StartTurnParams deserializes with all fields.
    #[test]
    fn turn_start_params_deserialize_with_all_fields() {
        use crate::handlers::turn::StartTurnParams;

        let json = serde_json::json!({
            "session_id": "test-id",
            "prompt": "hello",
            "keep_alive": true,
            "model": "claude-opus-4-6",
            "provider": "anthropic",
            "max_tokens": 4096,
            "system_prompt": "You are helpful",
            "output_schema": {"type": "object"},
            "structured_output_retries": 3,
            "provider_params": {"thinking": true}
        });
        let params: StartTurnParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.session_id, "test-id");
        assert_eq!(params.prompt, ContentInput::Text("hello".to_string()));
        assert_eq!(params.keep_alive, Some(true));
        assert_eq!(params.model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(params.provider.as_deref(), Some("anthropic"));
        assert_eq!(params.max_tokens, Some(4096));
        assert_eq!(params.system_prompt.as_deref(), Some("You are helpful"));
        assert!(params.output_schema.is_some());
        assert_eq!(params.structured_output_retries, Some(3));
        assert!(params.provider_params.is_some());
    }

    // -----------------------------------------------------------------------
    // Phase 4: Persisted session recovery
    // -----------------------------------------------------------------------

    /// turn/start on a completely unknown session returns SESSION_NOT_FOUND.
    #[tokio::test]
    async fn turn_start_unknown_session_returns_not_found() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let unknown_id = SessionId::new();
        let (event_tx, _rx) = mpsc::channel(100);
        let result = runtime
            .start_turn(
                &unknown_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, error::SESSION_NOT_FOUND);
    }

    /// Pending session timestamps are stable across list calls.
    #[tokio::test]
    async fn pending_session_timestamps_are_stable() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        let list1 = runtime.list_sessions_rich(Default::default()).await;
        let ts1 = list1.iter().find(|s| s.session_id == session_id).unwrap();
        let created1 = ts1.created_at;
        let updated1 = ts1.updated_at;
        assert!(created1 > 0, "created_at should be non-zero");
        assert_eq!(created1, updated1, "initial created_at == updated_at");

        // Wait a moment and list again — timestamps should be identical.
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let list2 = runtime.list_sessions_rich(Default::default()).await;
        let ts2 = list2.iter().find(|s| s.session_id == session_id).unwrap();
        assert_eq!(ts2.created_at, created1, "created_at must not drift");
        assert_eq!(
            ts2.updated_at, updated1,
            "updated_at must not drift without mutation"
        );
    }

    /// append_system_context on a pending session bumps updated_at.
    #[tokio::test]
    async fn pending_session_updated_at_advances_on_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        let before = runtime.read_session_rich(&session_id).await.unwrap();
        let created = before.created_at;

        // Wait so updated_at can differ.
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let req = AppendSystemContextRequest {
            text: "new context".to_string(),
            source: None,
            idempotency_key: Some("key-1".to_string()),
        };
        runtime
            .append_system_context(&session_id, req)
            .await
            .unwrap();

        let after = runtime.read_session_rich(&session_id).await.unwrap();
        assert_eq!(after.created_at, created, "created_at must not change");
        assert!(
            after.updated_at >= created,
            "updated_at ({}) should be >= created_at ({}) after mutation",
            after.updated_at,
            created,
        );
    }

    /// read_session_rich returns last_assistant_text for idle materialized sessions.
    #[tokio::test]
    async fn read_session_rich_returns_last_assistant_text_for_idle() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        // Materialize the session with a turn.
        let (event_tx, _rx) = mpsc::channel(100);
        runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Session is now idle — read_session_rich should provide last_assistant_text.
        let info = runtime.read_session_rich(&session_id).await.unwrap();
        assert!(
            info.last_assistant_text.is_some(),
            "idle session should have last_assistant_text, got None"
        );
    }

    fn temp_factory_with_builtins(temp: &tempfile::TempDir) -> AgentFactory {
        AgentFactory::new(temp.path().join("sessions")).builtins(true)
    }

    /// After hot-swapping from an Anthropic model to an OpenAI model, the
    /// machine-owned capability filter should hide `view_image`.
    #[tokio::test]
    async fn hot_swap_to_openai_hides_view_image() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory_with_builtins(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        // Create session with an Anthropic model (supports image_tool_results).
        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);

        // Materialize the session with the first turn.
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Hot-swap to an OpenAI model (does NOT support image_tool_results).
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("gpt-5.4".to_string()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        // Export the session and check that the capability-owned visibility
        // state blocks view_image without mutating the external overlay.
        let session = runtime
            .service
            .export_live_session(&session_id)
            .await
            .unwrap();
        let visibility_state = exported_tool_visibility_state(&session);
        assert_eq!(
            visibility_state.capability_base_filter,
            meerkat_core::capability_base_filter_for_image_tool_results(false),
            "hot-swap to OpenAI should deny view_image via the capability base filter"
        );
        assert_eq!(
            visibility_state.staged_filter,
            meerkat_core::ToolFilter::All,
            "hot-swap should not widen or replace the user-owned external filter"
        );
    }

    /// After hot-swapping from OpenAI back to Anthropic, the tool filter should
    /// be cleared (set to All).
    #[tokio::test]
    async fn hot_swap_to_anthropic_clears_view_image_deny() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory_with_builtins(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);

        // Materialize with first turn.
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Hot-swap to OpenAI (deny view_image).
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("gpt-5.4".to_string()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        // Hot-swap back to Anthropic (should clear the deny).
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("claude-sonnet-4-5".to_string()),
            ..Default::default()
        };
        let (event_tx3, _event_rx3) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Third turn".into(),
                event_tx3,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        // Export session and verify the capability-owned deny is cleared.
        let session = runtime
            .service
            .export_live_session(&session_id)
            .await
            .unwrap();
        let visibility_state = exported_tool_visibility_state(&session);
        assert_eq!(
            visibility_state.capability_base_filter,
            meerkat_core::ToolFilter::All,
            "hot-swap back to Anthropic should clear the capability-owned view_image deny"
        );
        assert_eq!(
            visibility_state.staged_filter,
            meerkat_core::ToolFilter::All,
            "hot-swap back to Anthropic should leave the external filter untouched"
        );
    }

    /// Hot-swapping preserves pre-existing session-local denies while adding
    /// capability-owned model gating on top.
    #[tokio::test]
    async fn hot_swap_preserves_preexisting_deny_filter() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory_with_builtins(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);

        // Materialize with first turn.
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Stage a pre-existing deny filter for "datetime".
        runtime
            .service
            .set_session_tool_filter(
                &session_id,
                meerkat_core::ToolFilter::Deny(["datetime".to_string()].into_iter().collect()),
            )
            .await
            .expect("staging deny filter for datetime should succeed");

        // Hot-swap to OpenAI — should add view_image to the deny set, not replace it.
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("gpt-5.4".to_string()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        let session = runtime
            .service
            .export_live_session(&session_id)
            .await
            .unwrap();
        let visibility_state = exported_tool_visibility_state(&session);
        assert_eq!(
            visibility_state.capability_base_filter,
            meerkat_core::capability_base_filter_for_image_tool_results(false),
            "hot-swap should deny view_image via the capability base filter"
        );
        assert_eq!(
            visibility_state.staged_filter,
            meerkat_core::ToolFilter::Deny(["datetime".to_string()].into_iter().collect()),
            "hot-swap should preserve the existing user-owned deny filter"
        );
    }

    /// Hot-swapping back to a capable model clears only the capability-owned
    /// `view_image` denial, keeping user-owned denies.
    #[tokio::test]
    async fn hot_swap_back_preserves_other_denied_tools() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory_with_builtins(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);

        // Materialize with first turn.
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        // Simulate a prior OpenAI swap by combining a capability-owned
        // `view_image` deny with a user-owned external deny for `datetime`.
        let mut visibility_state = exported_tool_visibility_state(
            &runtime
                .service
                .export_live_session(&session_id)
                .await
                .unwrap(),
        );
        visibility_state.capability_base_filter =
            meerkat_core::capability_base_filter_for_image_tool_results(false);
        visibility_state.active_filter =
            meerkat_core::ToolFilter::Deny(["datetime".to_string()].into_iter().collect());
        visibility_state.staged_filter = visibility_state.active_filter.clone();
        runtime
            .service
            .set_session_tool_visibility_state(&session_id, Some(visibility_state))
            .await
            .expect("staging mixed capability and external denies should succeed");

        // Hot-swap to Anthropic — should clear only the capability-owned deny.
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("claude-sonnet-4-5".to_string()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        let session = runtime
            .service
            .export_live_session(&session_id)
            .await
            .unwrap();
        let visibility_state = exported_tool_visibility_state(&session);

        assert_eq!(
            visibility_state.capability_base_filter,
            meerkat_core::ToolFilter::All,
            "hot-swap to Anthropic should clear the capability-owned view_image deny"
        );
        assert_eq!(
            visibility_state.staged_filter,
            meerkat_core::ToolFilter::Deny(["datetime".to_string()].into_iter().collect()),
            "hot-swap to Anthropic should keep the user-owned deny filter"
        );
    }

    #[tokio::test]
    async fn materialized_provider_only_override_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        let overrides = crate::handlers::turn::TurnOverrides {
            provider: Some("openai".to_string()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let err = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .expect_err("provider-only override should be rejected");
        assert_eq!(err.code, error::INVALID_PARAMS);
        assert!(
            err.message.contains("provider override requires model"),
            "unexpected error message: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn materialized_provider_params_hot_swap_persists_identity() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        let provider_params = serde_json::json!({
            "thinking": { "budget_tokens": 10_000 }
        });
        let overrides = crate::handlers::turn::TurnOverrides {
            provider_params: Some(provider_params.clone()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        let live = runtime
            .service
            .export_live_session(&session_id)
            .await
            .expect("live session after provider_params hot-swap");
        let live_meta = live
            .session_metadata()
            .expect("live session metadata after provider_params hot-swap");
        assert_eq!(live_meta.model, "claude-sonnet-4-5");
        assert_eq!(live_meta.provider, meerkat_core::Provider::Anthropic);
        assert_eq!(live_meta.provider_params, Some(provider_params.clone()));

        let stored = runtime
            .load_persisted_session(&session_id)
            .await
            .expect("load persisted session after provider_params hot-swap")
            .expect("stored session should exist");
        let stored_meta = stored
            .session_metadata()
            .expect("stored session metadata after provider_params hot-swap");
        assert_eq!(stored_meta.model, "claude-sonnet-4-5");
        assert_eq!(stored_meta.provider, meerkat_core::Provider::Anthropic);
        assert_eq!(stored_meta.provider_params, Some(provider_params));
    }

    #[tokio::test]
    async fn materialized_hot_swap_updates_durable_identity_for_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let mut runtime = make_runtime(temp_factory(&temp), 10);
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Hello".into(),
                event_tx,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        let provider_params = serde_json::json!({
            "reasoning": { "effort": "medium" }
        });
        let overrides = crate::handlers::turn::TurnOverrides {
            model: Some("gpt-5.4".to_string()),
            provider_params: Some(provider_params.clone()),
            ..Default::default()
        };
        let (event_tx2, _event_rx2) = mpsc::channel(100);
        let _ = runtime
            .start_turn(
                &session_id,
                "Second turn".into(),
                event_tx2,
                None,
                None,
                None,
                Some(overrides),
            )
            .await
            .unwrap();

        let info = runtime
            .read_session_rich(&session_id)
            .await
            .expect("read_session_rich after hot-swap");
        assert_eq!(info.model, "gpt-5.4");
        assert_eq!(info.provider, "openai");

        runtime
            .service
            .discard_live_session(&session_id)
            .await
            .expect("discard live session after hot-swap");

        let (event_tx, _rx) = mpsc::channel(100);
        let recovered = runtime
            .try_recover_persisted_session(
                &session_id,
                "recover".into(),
                event_tx,
                false,
                None,
                None,
                None,
            )
            .await;
        assert!(
            recovered.is_ok(),
            "expected persisted hot-swapped session to be recoverable: {:?}",
            recovered.err()
        );

        // After recovery the session has been promoted out of pending into a
        // live session.  Verify the recovered run completed with the preserved
        // hot-swapped identity by reading back the live session metadata.
        let info = runtime
            .read_session_rich(&session_id)
            .await
            .expect("read recovered session after re-materialization");
        assert_eq!(info.model, "gpt-5.4", "recovered model must match hot-swap");
        assert_eq!(
            info.provider, "openai",
            "recovered provider must match hot-swap"
        );
    }

    /// Regression: start_turn_via_runtime must reject build-only overrides
    /// (max_tokens, system_prompt, output_schema, structured_output_retries)
    /// that cannot be applied via RuntimeTurnMetadata. Before the fix these
    /// were silently dropped.
    #[tokio::test]
    async fn runtime_turn_rejects_build_only_overrides() {
        use crate::handlers::turn::TurnOverrides;

        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();

        for (field, overrides) in [
            (
                "max_tokens",
                TurnOverrides {
                    max_tokens: Some(1024),
                    ..Default::default()
                },
            ),
            (
                "system_prompt",
                TurnOverrides {
                    system_prompt: Some("override".into()),
                    ..Default::default()
                },
            ),
            (
                "output_schema",
                TurnOverrides {
                    output_schema: Some(serde_json::json!({"type": "object"})),
                    ..Default::default()
                },
            ),
            (
                "structured_output_retries",
                TurnOverrides {
                    structured_output_retries: Some(5),
                    ..Default::default()
                },
            ),
        ] {
            let (tx, _rx) = mpsc::channel(100);
            let result = runtime
                .start_turn_via_runtime(
                    &session_id,
                    "test".into(),
                    tx,
                    None,
                    None,
                    None,
                    Some(overrides),
                )
                .await;
            assert!(
                result.is_err(),
                "build-only override '{field}' must be rejected on runtime-routed turn"
            );
            let err = result.unwrap_err();
            assert_eq!(
                err.code,
                error::INVALID_PARAMS,
                "'{field}' rejection must use INVALID_PARAMS"
            );
            assert!(
                err.message.contains(field),
                "error message must mention '{field}': {}",
                err.message
            );
        }
    }

    /// W2-G regression: once a mob has claimed peer-ingress ownership on a
    /// session, calling the session-runtime drain-reconfigure helpers with a
    /// different comms runtime must NOT swap the mob's comms runtime out
    /// from under it. This is the s71 silent-downgrade class referenced in
    /// issue #264: session-runtime code used to rewire a session's comms
    /// every turn, without checking whether a mob had already taken
    /// ownership.
    ///
    /// The W2-G fix enforces this structurally at two layers:
    ///   (a) DSL guard: `AttachSessionIngress` requires owner `== Unattached`,
    ///       so an attempted swap from `MobOwned` is rejected at the DSL
    ///       authority. This is the core structural guarantee.
    ///   (b) Shell short-circuit: `start_turn_via_runtime` (and the
    ///       materialization / recovery paths that also call
    ///       `update_peer_ingress_context`) consult
    ///       `peer_ingress_owner()` and skip the reconfigure branch when
    ///       the owner is `MobOwned`. This avoids spurious warnings and
    ///       the attendant rollback churn.
    ///
    /// This test exercises the runtime adapter directly (layer (a) + the
    /// shell short-circuit inside the adapter's
    /// `update_peer_ingress_context`). End-to-end `start_turn_via_runtime`
    /// coverage for mob-owned drains requires the mob harness to keep the
    /// session alive across the turn boundary, which is verified in the
    /// mob integration lane.
    #[tokio::test]
    async fn update_peer_ingress_context_preserves_mob_owned_identity() {
        use meerkat_core::agent::CommsRuntime as CommsRuntimeTrait;

        struct StubCommsRuntime;

        #[async_trait::async_trait]
        impl CommsRuntimeTrait for StubCommsRuntime {
            async fn drain_messages(&self) -> Vec<String> {
                Vec::new()
            }
            fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
                Arc::new(tokio::sync::Notify::new())
            }
            fn dismiss_received(&self) -> bool {
                true
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        let session_id = runtime
            .create_session(mock_build_config(), None, None)
            .await
            .unwrap();
        runtime
            .ensure_runtime_executor(&session_id)
            .await
            .expect("ensure_runtime_executor");

        let adapter = runtime.runtime_adapter();

        // Mob claims peer-ingress ownership on behalf of this session.
        let mob_comms: Arc<dyn CommsRuntimeTrait> = Arc::new(StubCommsRuntime);
        let expected_id =
            meerkat_runtime::meerkat_machine::dsl::CommsRuntimeId::from_runtime(&mob_comms);
        let mob_id = meerkat_runtime::meerkat_machine::dsl::MobId::from("mob-w2g-integration");
        adapter
            .maybe_spawn_mob_comms_drain(&session_id, Arc::clone(&mob_comms), mob_id.clone())
            .await;

        // Simulate the s71 regression path: session-runtime code tries to
        // reconfigure peer-ingress with a different comms runtime. Before
        // W2-G this would silently swap the mob-owned drain. With W2-G the
        // DSL's `AttachSessionIngress` guard rejects the transition.
        let session_comms: Arc<dyn CommsRuntimeTrait> = Arc::new(StubCommsRuntime);
        adapter
            .update_peer_ingress_context(&session_id, true, Some(Arc::clone(&session_comms)))
            .await;

        // Assert the mob-owned drain's CommsRuntimeId is unchanged.
        let owner_after = adapter.peer_ingress_owner(&session_id).await;
        match owner_after {
            meerkat_runtime::PeerIngressOwner::MobOwned {
                comms_runtime_id,
                mob_id: actual_mob_id,
            } => {
                assert_eq!(
                    comms_runtime_id, expected_id,
                    "update_peer_ingress_context must not swap the mob-owned comms runtime id"
                );
                assert_eq!(actual_mob_id, mob_id);
            }
            other => {
                panic!("expected MobOwned to survive session-runtime swap attempt, got {other:?}")
            }
        }
    }

    #[test]
    fn completion_outcome_to_rpc_result_surfaces_callback_pending_payload() {
        let session_id = SessionId::new();
        let err = completion_outcome_to_rpc_result(
            meerkat_runtime::completion::CompletionOutcome::CallbackPending {
                tool_name: "external_mock".to_string(),
                args: serde_json::json!({ "value": "browser" }),
            },
            &session_id,
        )
        .expect_err("callback pending should map to an RPC error");

        assert_eq!(err.code, error::INTERNAL_ERROR);
        assert_eq!(err.message, "callback pending for tool 'external_mock'");
        let data = err.data.expect("callback pending error data");
        assert_eq!(data["session_id"], session_id.to_string());
        assert_eq!(data["resumable"], true);
        assert_eq!(data["tool_name"], "external_mock");
        assert_eq!(data["args"], serde_json::json!({ "value": "browser" }));
    }

    // -- P2-6: Typed BuildError → PROVIDER_ERROR classification --

    #[test]
    fn test_build_error_missing_api_key_classifies_as_provider_error() {
        let session_err = SessionError::Agent(meerkat_core::AgentError::BuildError(
            "Missing API key for provider 'anthropic'".to_string(),
        ));
        let rpc_err = session_error_to_rpc(session_err);
        assert_eq!(
            rpc_err.code,
            error::PROVIDER_ERROR,
            "BuildError with missing API key must map to PROVIDER_ERROR, got code: {}",
            rpc_err.code,
        );
    }

    #[test]
    fn test_build_error_unknown_provider_classifies_as_provider_error() {
        let session_err = SessionError::Agent(meerkat_core::AgentError::BuildError(
            "Unknown provider for model 'llama-3'".to_string(),
        ));
        let rpc_err = session_error_to_rpc(session_err);
        assert_eq!(
            rpc_err.code,
            error::PROVIDER_ERROR,
            "BuildError with unknown provider must map to PROVIDER_ERROR, got code: {}",
            rpc_err.code,
        );
    }

    #[test]
    fn test_build_error_generic_classifies_as_provider_error() {
        // ALL BuildErrors should be PROVIDER_ERROR regardless of message content.
        // This is the point — we match on the variant, not the string.
        let session_err = SessionError::Agent(meerkat_core::AgentError::BuildError(
            "something completely unrelated to API keys or providers".to_string(),
        ));
        let rpc_err = session_error_to_rpc(session_err);
        assert_eq!(
            rpc_err.code,
            error::PROVIDER_ERROR,
            "ALL BuildErrors must map to PROVIDER_ERROR regardless of message content, got code: {}",
            rpc_err.code,
        );
    }
}
