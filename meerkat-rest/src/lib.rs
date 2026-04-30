//! meerkat-rest - REST API server for Meerkat
//!
//! Provides HTTP endpoints for running and managing Meerkat agents:
//! - POST /sessions - Create and run a new agent
//! - POST /sessions/:id/messages - Continue an existing session
//! - POST /comms/send - Send a canonical comms command
//! - GET /comms/peers - List peers visible to a session
//! - GET /mob/prefabs - List built-in mob prefab templates
//! - POST /sessions/:id/external-events - Push an external event to a session
//! - GET /sessions/:id - Get session details
//! - GET /sessions/:id/events - SSE stream for agent events
//!
//! # Built-in Tools
//! Built-in tools are configured via the REST config store.
//! When enabled, the REST instance uses its instance-scoped data directory
//! as the project root for task storage and shell working directory.

#![allow(
    dead_code,
    unused_imports,
    clippy::boxed_local,
    clippy::expect_used,
    clippy::implicit_clone,
    clippy::large_futures,
    clippy::redundant_clone,
    clippy::unnested_or_patterns
)]

pub mod auth_endpoints;
mod schedule_host;
pub mod webhook;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use futures::stream::Stream;
use meerkat::surface::{
    RequestAlreadyExists, RequestContext, RequestTerminal, RequestTerminalResolution,
    SurfaceRequestExecutor, SurfaceSessionRecoveryContext, SurfaceSessionRecoveryOverrides,
    build_recovered_session, noop_request_action, request_action,
};
use meerkat::{
    AgentEvent, AgentFactory, FactoryAgentBuilder, LlmClient, OutputSchema,
    PersistentSessionService, ScheduleService, ScheduleToolDispatcher, Session, SessionId,
    SessionService, SessionServiceControlExt, SessionServiceHistoryExt,
    encode_llm_client_override_for_service, handle_schedule_tools_call, open_realm_persistence_in,
    schedule_tools_list,
};
use meerkat_contracts::{
    ErrorCode, RealtimeCapabilitiesParams, RealtimeCapabilitiesResult, RealtimeOpenInfo,
    RealtimeOpenRequest, RealtimeStatusParams, RealtimeStatusResult,
    RuntimeRealtimeAttachmentStatusResult, RuntimeStateResult, SessionLocator, SkillsParams,
    WireError, format_session_ref,
};
use meerkat_core::EventEnvelope;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{
    ConversationContextAppend, CoreRenderable, RunApplyBoundary, RunPrimitive,
};
use meerkat_core::service::{
    AppendSystemContextRequest as SvcAppendSystemContextRequest,
    CreateSessionRequest as SvcCreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy,
    ResumeOverrideMask, SessionBuildOptions, SessionControlError, SessionError,
    StartTurnRequest as SvcStartTurnRequest,
};
use meerkat_core::{
    Config, ConfigDelta, ConfigEnvelope, ConfigEnvelopePolicy, ConfigStore, ContentInput,
    FileConfigStore, HookRunOverrides, PendingSystemContextAppend, Provider, RealmSelection,
    RuntimeBootstrap, SessionLlmIdentity, ToolCategoryOverride, agent_event_type,
    format_verbose_event,
};
#[cfg(feature = "mob")]
use meerkat_mob::MobSessionService as _;
use meerkat_runtime::SessionServiceRuntimeExt as _;
use meerkat_store::{RealmBackend, RealmOrigin};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc};

#[cfg(feature = "mcp")]
use meerkat::{
    AgentToolDispatcher, McpLifecycleAction, McpLifecyclePhase, McpReloadTarget, McpRouter,
    McpRouterAdapter,
};
#[cfg(feature = "mcp")]
use meerkat_core::ToolConfigChangeOperation;
#[cfg(feature = "mcp")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "mcp")]
use std::time::Duration;
#[cfg(feature = "mcp")]
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Live MCP per-session state
// ---------------------------------------------------------------------------

#[cfg(feature = "mcp")]
pub struct SessionMcpState {
    pub(crate) adapter: Arc<McpRouterAdapter>,
    pub(crate) turn_counter: u32,
    pub(crate) lifecycle_tx: mpsc::UnboundedSender<McpLifecycleAction>,
    pub(crate) lifecycle_rx: mpsc::UnboundedReceiver<McpLifecycleAction>,
    pub(crate) drain_task_running: Arc<AtomicBool>,
}

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub store_path: PathBuf,
    pub default_model: Cow<'static, str>,
    pub max_tokens: u32,
    pub rest_host: Cow<'static, str>,
    pub rest_port: u16,
    /// Whether to enable built-in tools (task management, shell)
    pub enable_builtins: bool,
    /// Whether to enable shell tools (requires enable_builtins=true)
    pub enable_shell: bool,
    /// Project root for file-based task store and shell working directory
    pub project_root: Option<PathBuf>,
    /// Optional context root used by convention-based default skill sources.
    pub context_root: Option<PathBuf>,
    /// Optional user config root used by convention-based default skill sources.
    pub user_config_root: Option<PathBuf>,
    /// Override the resolved LLM client (primarily for tests and embedding).
    pub llm_client_override: Option<Arc<dyn LlmClient>>,
    pub config_store: Arc<dyn ConfigStore>,
    pub event_tx: broadcast::Sender<SessionEvent>,
    /// Session service for managing agent lifecycle.
    pub session_service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    pub schedule_service: ScheduleService,
    /// Webhook authentication, resolved once at startup from RKAT_WEBHOOK_SECRET.
    pub webhook_auth: webhook::WebhookAuth,
    pub realm: meerkat_core::RealmId,
    pub instance_id: Option<String>,
    pub backend: String,
    pub resolved_paths: meerkat_core::ConfigResolvedPaths,
    pub expose_paths: bool,
    pub realtime_rpc_tcp_addr: Option<String>,
    pub config_runtime: Arc<meerkat_core::ConfigRuntime>,
    pub realm_lease: Arc<tokio::sync::Mutex<Option<meerkat_store::RealmLeaseGuard>>>,
    pub skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    /// Optional v9 runtime adapter for runtime/input endpoints.
    pub runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    pub schedule_host: Arc<schedule_host::ScheduleHostState>,
    /// Shared in-process mob lifecycle state for protocol mob operations.
    #[cfg(feature = "mob")]
    pub mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
    /// Per-session MCP adapter state (live MCP mutation).
    #[cfg(feature = "mcp")]
    pub mcp_sessions: Arc<RwLock<std::collections::HashMap<SessionId, SessionMcpState>>>,
    /// Request-level cancellation executor.
    pub request_executor: Arc<SurfaceRequestExecutor>,
    /// Persistent TokenStore for OAuth-backed bindings. Shared with the
    /// AgentFactory so both read and write paths (login, resolve,
    /// logout) see the same credentials.
    pub token_store: Arc<dyn meerkat_providers::auth_store::TokenStore>,
    /// Process-local AuthMachine lifecycle registry for auth endpoints.
    pub auth_lease: Arc<dyn meerkat_core::handles::AuthLeaseHandle>,
    /// Provider-runtime registry shared with the AgentFactory's auth
    /// resolution path.
    pub provider_registry: Arc<meerkat_providers::ProviderRuntimeRegistry>,
}

#[derive(Debug, Clone)]
pub struct SessionEvent {
    session_id: SessionId,
    event: EventEnvelope<AgentEvent>,
}

#[derive(Clone)]
struct RestRuntimeExecutorContext {
    llm_client_override: Option<Arc<dyn LlmClient>>,
    event_tx: broadcast::Sender<SessionEvent>,
    session_service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    realm: meerkat_core::RealmId,
    instance_id: Option<String>,
    backend: String,
    config_runtime: Arc<meerkat_core::ConfigRuntime>,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
}

struct RestSessionRuntimeExecutor {
    context: RestRuntimeExecutorContext,
    session_id: SessionId,
}

impl AppState {
    pub async fn load() -> Result<Self, Box<dyn std::error::Error>> {
        Self::load_with_bootstrap_and_options(RuntimeBootstrap::default(), false).await
    }

    #[cfg(test)]
    async fn load_from(instance_root: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let mut bootstrap = RuntimeBootstrap::default();
        bootstrap.realm.state_root = Some(instance_root.join("realms"));
        bootstrap.context.context_root = Some(instance_root.clone());
        Self::load_from_with_bootstrap(instance_root, bootstrap, false).await
    }

    pub async fn load_with_bootstrap(
        bootstrap: RuntimeBootstrap,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::load_with_bootstrap_and_options(bootstrap, false).await
    }

    pub async fn load_with_bootstrap_and_options(
        bootstrap: RuntimeBootstrap,
        expose_paths: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::load_from_with_bootstrap(rest_instance_root(), bootstrap, expose_paths).await
    }

    async fn load_from_with_bootstrap(
        instance_root: PathBuf,
        bootstrap: RuntimeBootstrap,
        expose_paths: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (event_tx, _) = broadcast::channel(256);
        let locator = bootstrap.realm.resolve_locator()?;
        let realm = locator.realm;
        let instance_id = bootstrap.realm.instance_id;
        let backend_hint = bootstrap
            .realm
            .backend_hint
            .as_deref()
            .and_then(parse_backend_hint);
        let origin_hint = Some(realm_origin_from_selection(&bootstrap.realm.selection));
        let realms_root = locator.state_root;
        let (manifest, persistence) =
            open_realm_persistence_in(&realms_root, realm.as_str(), backend_hint, origin_hint)
                .await?;
        let session_store = persistence.session_store();
        let schedule_service = ScheduleService::new(persistence.schedule_store());
        let realm_paths = meerkat_store::realm_paths_in(&realms_root, realm.as_str());
        let resolved_paths = meerkat_core::ConfigResolvedPaths {
            root: realm_paths.root.display().to_string(),
            manifest_path: realm_paths.manifest_path.display().to_string(),
            config_path: realm_paths.config_path.display().to_string(),
            sessions_sqlite_path: Some(realm_paths.sessions_sqlite_path.display().to_string()),
            sessions_jsonl_dir: realm_paths.sessions_jsonl_dir.display().to_string(),
        };
        let base_config_store: Arc<dyn ConfigStore> =
            Arc::new(FileConfigStore::new(realm_paths.config_path.clone()));
        let config_store: Arc<dyn ConfigStore> = Arc::new(meerkat_core::TaggedConfigStore::new(
            base_config_store,
            meerkat_core::ConfigStoreMetadata {
                realm_id: Some(realm.to_string()),
                instance_id: instance_id.clone(),
                backend: Some(manifest.backend.as_str().to_string()),
                resolved_paths: Some(resolved_paths.clone()),
            },
        ));
        let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
            Arc::clone(&config_store),
            realm_paths.root.join("config_state.json"),
        ));
        let lease = meerkat_store::start_realm_lease_in(
            &realms_root,
            realm.as_str(),
            instance_id.as_deref(),
            "rkat-rest",
        )
        .await?;

        let mut config = config_store
            .get()
            .await
            .unwrap_or_else(|_| Config::default());
        if let Err(err) = config.apply_env_overrides() {
            tracing::warn!("Failed to apply env overrides: {}", err);
        }

        let store_path = persistence
            .store_path()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| match manifest.backend {
                meerkat_store::RealmBackend::Jsonl => realm_paths.sessions_jsonl_dir.clone(),
                meerkat_store::RealmBackend::Sqlite => realm_paths.root.clone(),
            });

        let enable_builtins = config.tools.builtins_enabled;
        let enable_shell = config.tools.shell_enabled;

        let default_model = Cow::Owned(config.agent.model.clone());
        let max_tokens = config.agent.max_tokens_per_turn;
        let rest_host = Cow::Owned(config.rest.host.clone());
        let rest_port = config.rest.port;

        // Shared TokenStore: attach to the factory AND to AppState so the
        // OAuth write-path handlers (auth/login/complete, auth/profile/
        // create, auth/logout) can read/write the same persisted
        // credentials as the factory's resolve_binding path.
        let token_store: Arc<dyn meerkat_providers::auth_store::TokenStore> =
            match meerkat_providers::auth_store::TokenStoreBackend::default_auto()
                .and_then(meerkat_providers::auth_store::TokenStoreBackend::open)
            {
                Ok(store) => store,
                Err(_) => Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new()),
            };
        let auth_lease: Arc<dyn meerkat_core::handles::AuthLeaseHandle> =
            Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
        let mut factory = AgentFactory::new(store_path.clone())
            .with_token_store(Arc::clone(&token_store))
            .session_store(session_store.clone())
            .runtime_root(realm_paths.root.clone())
            .builtins(enable_builtins)
            .shell(enable_shell)
            .schedule(true);
        let conventions_context_root = bootstrap.context.context_root.clone();
        let conventions_user_root = bootstrap.context.user_config_root.clone();
        let task_project_root = conventions_context_root
            .clone()
            .unwrap_or_else(|| instance_root.clone());
        factory = factory.project_root(task_project_root.clone());
        if let Some(context_root) = conventions_context_root {
            factory = factory.context_root(context_root);
        }
        if let Some(user_root) = bootstrap.context.user_config_root.clone() {
            factory = factory.user_config_root(user_root);
        }

        let skill_runtime = factory.build_skill_runtime(&config).await?;
        let provider_registry = factory.provider_runtime_registry();

        let builder =
            FactoryAgentBuilder::new_with_config_store(factory, config, Arc::clone(&config_store));
        // Capture the mob tools slot before the builder is consumed into the session service.
        // We set the actual factory after mob_state is constructed (circular dep break).
        #[cfg(feature = "mob")]
        let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
        meerkat::surface::set_default_schedule_tools(
            &builder,
            Some(Arc::new(ScheduleToolDispatcher::new(
                schedule_service.clone(),
            ))),
        );
        let (session_service, runtime_adapter) =
            meerkat::surface::build_runtime_backed_service(builder, 100, persistence);
        runtime_adapter.set_auth_lease_handle(Arc::clone(&auth_lease));
        let session_service = Arc::new(session_service);
        #[cfg(feature = "mob")]
        let mob_session_service = session_service.clone();

        Ok(Self {
            store_path,
            default_model,
            max_tokens,
            rest_host,
            rest_port,
            enable_builtins,
            enable_shell,
            project_root: Some(task_project_root),
            context_root: bootstrap.context.context_root.clone(),
            user_config_root: conventions_user_root,
            llm_client_override: None,
            config_store,
            event_tx,
            session_service,
            schedule_service,
            webhook_auth: webhook::WebhookAuth::from_env(),
            realm,
            instance_id,
            backend: manifest.backend.as_str().to_string(),
            resolved_paths,
            expose_paths,
            realtime_rpc_tcp_addr: None,
            config_runtime,
            realm_lease: Arc::new(tokio::sync::Mutex::new(Some(lease))),
            skill_runtime,
            runtime_adapter,
            schedule_host: Arc::new(schedule_host::ScheduleHostState::default()),
            #[cfg(feature = "mob")]
            mob_state: {
                let state = Arc::new(
                    meerkat_mob_mcp::MobMcpState::new(mob_session_service)
                        .with_persistent_storage_root(Some(realm_paths.root.clone())),
                );
                *mob_tools_slot
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
                    meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(Arc::clone(&state)),
                ));
                state
            },
            #[cfg(feature = "mcp")]
            mcp_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
            request_executor: Arc::new(SurfaceRequestExecutor::new(
                std::time::Duration::from_secs(5),
            )),
            token_store,
            auth_lease,
            provider_registry,
        })
    }

    fn runtime_executor_context(&self) -> RestRuntimeExecutorContext {
        RestRuntimeExecutorContext {
            llm_client_override: self.llm_client_override.clone(),
            event_tx: self.event_tx.clone(),
            session_service: self.session_service.clone(),
            realm: self.realm.clone(),
            instance_id: self.instance_id.clone(),
            backend: self.backend.clone(),
            config_runtime: self.config_runtime.clone(),
            runtime_adapter: self.runtime_adapter.clone(),
        }
    }
}

impl RestSessionRuntimeExecutor {
    fn new(context: RestRuntimeExecutorContext, session_id: SessionId) -> Self {
        Self {
            context,
            session_id,
        }
    }
}

async fn ensure_rest_session_runtime_executor(state: &AppState, session_id: &SessionId) {
    let executor = Box::new(RestSessionRuntimeExecutor::new(
        state.runtime_executor_context(),
        session_id.clone(),
    ));
    state
        .runtime_adapter
        .ensure_session_with_executor(session_id.clone(), executor)
        .await;
}

async fn require_rest_session_exists_for_read(
    state: &AppState,
    session_id: &SessionId,
) -> Result<(), Response> {
    state
        .session_service
        .read(session_id)
        .await
        .map(|_| ())
        .map_err(|err| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": err.to_string()})),
            )
                .into_response()
        })
}

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

async fn resolve_validation_identity(
    config_runtime: &meerkat_core::ConfigRuntime,
    model: &str,
    provider: Option<meerkat_core::Provider>,
) -> Result<SessionLlmIdentity, String> {
    let snapshot = config_runtime.get().await.ok().map(|state| state.config);
    let registry = snapshot
        .as_ref()
        .and_then(|config| config.model_registry().ok());
    let entry = registry.as_ref().and_then(|registry| registry.entry(model));
    if let (Some(registry), Some(provider)) = (registry.as_ref(), provider)
        && let Some(reason) = registry.provider_override_mismatch_reason(provider, model)
    {
        return Err(reason);
    }
    let provider = provider
        .or_else(|| entry.map(|entry| entry.provider))
        .or_else(|| meerkat_core::Provider::infer_from_model(model))
        .unwrap_or(meerkat_core::Provider::Other);
    let self_hosted_server_id = if provider == meerkat_core::Provider::SelfHosted {
        entry
            .and_then(|entry| entry.self_hosted.as_ref())
            .map(|server| server.server_id.clone())
    } else {
        None
    };

    Ok(SessionLlmIdentity {
        model: model.to_string(),
        provider,
        self_hosted_server_id,
        provider_params: None,
        connection_ref: None,
    })
}

async fn validate_prompt_video_input(
    config_runtime: &meerkat_core::ConfigRuntime,
    prompt: &ContentInput,
    identity: &SessionLlmIdentity,
) -> Result<(), String> {
    let blocks = match prompt {
        ContentInput::Text(_) => return Ok(()),
        ContentInput::Blocks(blocks) => blocks,
    };

    meerkat_core::validate_inline_video_blocks(blocks)?;

    let supports_inline_video = config_runtime
        .get()
        .await
        .ok()
        .and_then(|state| state.config.model_registry().ok())
        .and_then(|registry| {
            registry
                .profile_for(&identity.model)
                .map(|profile| profile.inline_video)
        })
        .unwrap_or(false);

    if meerkat_core::has_video(blocks) && !supports_inline_video {
        return Err(format!(
            "inline video input is not supported by model '{}' on provider '{}'",
            identity.model,
            identity.provider.as_str()
        ));
    }

    Ok(())
}

async fn apply_runtime_turn(
    context: &RestRuntimeExecutorContext,
    session_id: &SessionId,
    run_id: meerkat_core::lifecycle::RunId,
    primitive: &RunPrimitive,
    prompt: ContentInput,
) -> Result<CoreApplyOutput, SessionError> {
    if let Some(reason) = primitive.peer_response_terminal_apply_intent_violation() {
        return Err(SessionError::Agent(
            meerkat_core::error::AgentError::InternalError(reason.to_string()),
        ));
    }

    // Context-only staged primitives may land directly as runtime
    // system-context appends, but terminal peer responses carry a typed apply
    // intent that requires a requester reaction turn.
    if primitive.is_context_only_apply_without_turn() {
        let RunPrimitive::StagedInput(staged) = primitive else {
            unreachable!("context-only apply without turn only matches staged primitives");
        };
        return context
            .session_service
            .apply_runtime_context_appends(
                session_id,
                run_id,
                pending_system_context_appends(&staged.context_appends),
                staged.contributing_input_ids.clone(),
            )
            .await;
    }

    if primitive.is_peer_response_terminal_context_and_run() {
        let RunPrimitive::StagedInput(staged) = primitive else {
            unreachable!("terminal peer-response apply intent only matches staged primitives");
        };
        context
            .session_service
            .apply_runtime_system_context_for_turn(
                session_id,
                pending_system_context_appends(&staged.context_appends),
            )
            .await?;
    }

    let (event_tx, event_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(100);
    let forwarder = spawn_event_forwarder(
        event_rx,
        context.event_tx.clone(),
        session_id.clone(),
        false,
    );
    // The turn-metadata keep_alive carrier is typed (`KeepAlivePolicy`); the
    // session recovery override and stored session metadata still track a
    // boolean. Collapse the typed per-turn policy into the boolean used by
    // the recovery path: presence of a policy is interpreted as "keep the
    // materialized resources alive across this turn".
    let keep_alive = match primitive
        .turn_metadata()
        .and_then(|metadata| metadata.keep_alive.as_ref())
    {
        Some(_policy) => true,
        None => context
            .session_service
            .load_authoritative_session(session_id)
            .await
            .ok()
            .flatten()
            .and_then(|session| {
                session
                    .session_metadata()
                    .map(|metadata| metadata.keep_alive)
            })
            .unwrap_or(false),
    };

    let svc_req = SvcStartTurnRequest {
        prompt: prompt.clone(),
        system_prompt: None,
        render_metadata: None,
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        event_tx: Some(event_tx.clone()),

        skill_references: primitive
            .turn_metadata()
            .and_then(|meta| meta.skill_references.clone()),
        flow_tool_overlay: primitive
            .turn_metadata()
            .and_then(|meta| meta.flow_tool_overlay.clone()),
        turn_metadata: primitive.turn_metadata().cloned(),
    };

    let session_identity = context
        .session_service
        .load_authoritative_session(session_id)
        .await
        .ok()
        .flatten()
        .and_then(|session| {
            session
                .session_metadata()
                .map(|metadata| metadata.llm_identity())
        });
    if let Some(identity) = session_identity
        && let Err(message) =
            validate_prompt_video_input(&context.config_runtime, &prompt, &identity).await
    {
        return Err(SessionError::Agent(meerkat_core::AgentError::ConfigError(
            message,
        )));
    }

    let boundary = match primitive {
        RunPrimitive::StagedInput(staged) => staged.boundary,
        _ => RunApplyBoundary::Immediate,
    };
    let contributing_input_ids = primitive.contributing_input_ids().to_vec();

    let result = match context
        .session_service
        .apply_runtime_turn(
            session_id,
            run_id.clone(),
            svc_req,
            boundary,
            contributing_input_ids.clone(),
        )
        .await
    {
        Ok(output) => Ok(output),
        Err(SessionError::NotFound { .. }) => {
            let session = context
                .session_service
                .load_authoritative_session(session_id)
                .await?
                .ok_or(SessionError::NotFound {
                    id: session_id.clone(),
                })?;
            let current_generation = context
                .config_runtime
                .get()
                .await
                .ok()
                .map(|s| s.generation);
            let bindings = context
                .runtime_adapter
                .prepare_bindings(session_id.clone())
                .await
                .map_err(|e| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "failed to prepare runtime bindings for session {session_id}: {e}"
                    )))
                })?;
            let recovered = build_recovered_session(
                session,
                &SurfaceSessionRecoveryOverrides {
                    keep_alive: Some(keep_alive),
                    ..Default::default()
                },
                SurfaceSessionRecoveryContext {
                    llm_client_override: context
                        .llm_client_override
                        .clone()
                        .map(encode_llm_client_override_for_service),
                    external_tools: None,
                    checkpointer: None,
                    runtime_build_mode: Some(meerkat_core::RuntimeBuildMode::SessionOwned(
                        bindings,
                    )),
                    require_runtime_build_mode: true,
                    realm_id: Some(context.realm.to_string()),
                    instance_id: context.instance_id.clone(),
                    backend: Some(context.backend.clone()),
                    config_generation: current_generation,
                },
            )
            .map_err(|error| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    error.to_string(),
                ))
            })?;
            context
                .session_service
                .create_session(recovered.into_deferred_create_request())
                .await?;
            let output = context
                .session_service
                .apply_runtime_turn_outcome(
                    session_id,
                    run_id,
                    SvcStartTurnRequest {
                        prompt,
                        system_prompt: None,
                        render_metadata: None,
                        handling_mode: meerkat_core::types::HandlingMode::Queue,
                        event_tx: Some(event_tx.clone()),

                        skill_references: primitive
                            .turn_metadata()
                            .and_then(|meta| meta.skill_references.clone()),
                        flow_tool_overlay: primitive
                            .turn_metadata()
                            .and_then(|meta| meta.flow_tool_overlay.clone()),
                        turn_metadata: primitive.turn_metadata().cloned(),
                    },
                    boundary,
                    contributing_input_ids,
                )
                .await?;
            Ok(output)
        }
        Err(err) => Err(err),
    };

    drop(event_tx);
    drain_event_forwarder(session_id, forwarder).await;
    result
}

#[async_trait::async_trait]
impl CoreExecutor for RestSessionRuntimeExecutor {
    async fn apply(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let prompt = primitive.extract_content_input();

        apply_runtime_turn(&self.context, &self.session_id, run_id, &primitive, prompt)
            .await
            .map_err(|err| CoreExecutorError::ApplyFailed {
                reason: err.to_string(),
            })
    }

    async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
        match command {
            RunControlCommand::CancelCurrentRun { .. } => self
                .context
                .session_service
                .interrupt(&self.session_id)
                .await
                .map_err(|err| CoreExecutorError::ControlFailed {
                    reason: err.to_string(),
                }),
            RunControlCommand::StopRuntimeExecutor { .. } => {
                let discard_result = self
                    .context
                    .session_service
                    .discard_live_session(&self.session_id)
                    .await;
                self.context
                    .runtime_adapter
                    .unregister_session(&self.session_id)
                    .await;
                match discard_result {
                    Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
                    Err(err) => Err(CoreExecutorError::ControlFailed {
                        reason: err.to_string(),
                    }),
                }
            }
            _ => Ok(()),
        }
    }
}

fn rest_instance_root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("meerkat")
        .join("rest")
}

fn parse_backend_hint(raw: &str) -> Option<RealmBackend> {
    match raw {
        "jsonl" => Some(RealmBackend::Jsonl),
        "sqlite" => Some(RealmBackend::Sqlite),
        _ => None,
    }
}

fn realm_origin_from_selection(selection: &RealmSelection) -> RealmOrigin {
    match selection {
        RealmSelection::Explicit { .. } => RealmOrigin::Explicit,
        RealmSelection::Isolated => RealmOrigin::Generated,
        RealmSelection::WorkspaceDerived { .. } => RealmOrigin::Workspace,
    }
}

/// Resolve an explicit keep_alive override. Returns None when input is None (inherit).
fn resolve_keep_alive(requested: Option<bool>) -> Result<Option<bool>, ApiError> {
    match requested {
        Some(true) => meerkat::surface::resolve_keep_alive(true)
            .map(Some)
            .map_err(ApiError::BadRequest),
        other => Ok(other), // None (inherit) or Some(false) (disable) pass through
    }
}

/// Default keep-alive TTL applied to per-turn metadata when the REST wire
/// carries the boolean `keep_alive: true` override. Mirrors the canonical
/// default used elsewhere in the runtime layer.
const REST_TURN_KEEP_ALIVE_TTL_SECS: u64 = 30;

/// Translate the REST wire `Option<bool>` keep-alive override into the typed
/// `Option<KeepAlivePolicy>` carried on `RuntimeTurnMetadata`.
///
/// * `Some(true)` -> `Some(Pinned, ttl = REST_TURN_KEEP_ALIVE_TTL_SECS)` —
///   opts in to a caller-owned keep-alive lifetime for this turn.
/// * `Some(false)` and `None` -> `None`. Per-turn metadata cannot "disable"
///   keep-alive; the session-level `keep_alive` flag on `SessionBuildOptions`
///   is the authoritative switch. A false override is interpreted as
///   "inherit session default" on the turn metadata seam.
fn resolve_turn_keep_alive_policy(
    requested: Option<bool>,
) -> Option<meerkat_core::lifecycle::run_primitive::KeepAlivePolicy> {
    match requested {
        Some(true) => Some(meerkat_core::lifecycle::run_primitive::KeepAlivePolicy {
            ttl: std::time::Duration::from_secs(REST_TURN_KEEP_ALIVE_TTL_SECS),
            policy: meerkat_core::lifecycle::run_primitive::KeepAliveMode::Pinned,
        }),
        Some(false) | None => None,
    }
}

/// Translate the REST wire `Option<Vec<String>>` additional-instructions list
/// into the typed `Option<Vec<TurnInstruction>>` carried on
/// `RuntimeTurnMetadata`. The REST envelope does not carry a role per entry;
/// strings are interpreted as `TurnInstructionKind::User` to match the
/// end-user prompt lineage they originate from.
fn resolve_turn_additional_instructions(
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

fn validate_public_peer_meta(peer_meta: Option<&meerkat_core::PeerMeta>) -> Result<(), ApiError> {
    meerkat::surface::validate_public_peer_meta(peer_meta).map_err(ApiError::BadRequest)
}

fn validate_public_surface_metadata(
    labels: Option<&BTreeMap<String, String>>,
    app_context: Option<&Value>,
) -> Result<(), ApiError> {
    meerkat::surface::validate_public_surface_metadata(labels, app_context)
        .map_err(ApiError::BadRequest)
}

/// Create session request
#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub prompt: ContentInput,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub model: Option<Cow<'static, str>>,
    #[serde(default)]
    pub provider: Option<Provider>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// JSON schema for structured output extraction (wrapper or raw schema).
    #[serde(default)]
    pub output_schema: Option<OutputSchema>,
    /// Max retries for structured output validation.
    /// Omit to use the product default.
    #[serde(default)]
    pub structured_output_retries: Option<u32>,
    /// Enable verbose event logging (server-side).
    #[serde(default)]
    pub verbose: bool,
    /// Keep session alive after turn completes, listening for comms messages.
    /// None = inherit persisted session intent, Some(true) = enable, Some(false) = disable.
    /// Requires comms_name when enabled.
    #[serde(default)]
    pub keep_alive: Option<bool>,
    /// Agent name for inter-agent communication. Required for keep_alive.
    #[serde(default)]
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery.
    #[serde(default)]
    pub peer_meta: Option<meerkat_core::PeerMeta>,
    /// Optional run-scoped hook overrides.
    #[serde(default)]
    pub hooks_override: Option<HookRunOverrides>,
    /// Enable built-in tools. Omit to use factory defaults.
    #[serde(default)]
    pub enable_builtins: Option<bool>,
    /// Enable shell tool. Omit to use factory defaults.
    #[serde(default)]
    pub enable_shell: Option<bool>,
    /// Enable semantic memory. Omit to use factory defaults.
    #[serde(default)]
    pub enable_memory: Option<bool>,
    /// Enable mob tools. Omit to use factory defaults.
    #[serde(default)]
    pub enable_mob: Option<bool>,
    /// Explicit budget limits for this run.
    #[serde(default)]
    pub budget_limits: Option<meerkat_core::BudgetLimits>,
    /// Provider-specific parameters (for example reasoning config).
    #[serde(default)]
    pub provider_params: Option<Value>,
    /// Skills to preload into the system prompt.
    #[serde(default)]
    pub preload_skills: Option<Vec<meerkat_core::skills::SkillKey>>,
    /// Structured refs for per-turn skill injection.
    #[serde(default)]
    pub skill_refs: Option<Vec<meerkat_core::skills::SkillRef>>,
    /// Optional key-value labels attached at session creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
    /// Additional instruction sections appended to the system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
    /// Opaque application context passed through to custom builders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_context: Option<Value>,
    /// Per-agent environment variables injected into shell tool subprocesses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_env: Option<std::collections::HashMap<String, String>>,
}

fn default_structured_output_retries() -> u32 {
    2
}

fn rest_continue_requires_rebuild(req: &ContinueSessionRequest) -> bool {
    req.model.is_some()
        || req.provider.is_some()
        || req.max_tokens.is_some()
        || req.system_prompt.is_some()
        || req.output_schema.is_some()
        || req.structured_output_retries.is_some()
        || req.hooks_override.is_some()
        || req.comms_name.is_some()
        || req.peer_meta.is_some()
}

async fn canonical_skill_keys_for_state(
    state: &AppState,
    skill_refs: Option<Vec<meerkat_core::skills::SkillRef>>,
) -> Result<Option<Vec<meerkat_core::skills::SkillKey>>, ApiError> {
    let params = SkillsParams {
        preload_skills: None,
        skill_refs,
    };

    // Validate the registry builds: invalid source-identity config is
    // surfaced as a typed `BadRequest` even though the helper itself does
    // not consult the registry to canonicalize keys (the runtime skill
    // engine applies remaps at resolution time).
    let snapshot = state
        .config_runtime
        .get()
        .await
        .map_err(config_runtime_err_to_api)?;
    snapshot
        .config
        .skills
        .build_source_identity_registry()
        .map_err(|e| ApiError::BadRequest(format!("Invalid skills config: {e}")))?;

    Ok(params.canonical_skill_keys())
}

/// Continue session request
#[derive(Debug, Deserialize, Clone)]
pub struct ContinueSessionRequest {
    pub session_id: String,
    pub prompt: ContentInput,
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// JSON schema for structured output extraction (wrapper or raw schema).
    #[serde(default)]
    pub output_schema: Option<OutputSchema>,
    /// Max retries for structured output validation.
    /// Omit to inherit the current/persisted session value.
    #[serde(default)]
    pub structured_output_retries: Option<u32>,
    /// Keep session alive after turn completes, listening for comms messages.
    /// None = inherit persisted session intent, Some(true) = enable, Some(false) = disable.
    #[serde(default)]
    pub keep_alive: Option<bool>,
    /// Agent name for inter-agent communication. Required for keep_alive.
    #[serde(default)]
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery.
    #[serde(default)]
    pub peer_meta: Option<meerkat_core::PeerMeta>,
    /// Enable verbose event logging (server-side).
    #[serde(default)]
    pub verbose: bool,
    #[serde(default)]
    pub model: Option<Cow<'static, str>>,
    #[serde(default)]
    pub provider: Option<Provider>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Optional run-scoped hook overrides.
    #[serde(default)]
    pub hooks_override: Option<HookRunOverrides>,
    /// Structured refs for per-turn skill injection.
    #[serde(default)]
    pub skill_refs: Option<Vec<meerkat_core::skills::SkillRef>>,
    /// Optional per-turn flow tool overlay.
    #[serde(default)]
    pub flow_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
    /// Additional instruction sections prepended as system notices to the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
}

/// Append runtime system context to a session.
#[derive(Debug, Deserialize)]
pub struct AppendSystemContextRequest {
    pub text: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListSessionsQuery {
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
    /// Repeatable label filter: `?label=env%3Dprod&label=team%3Dinfra`.
    /// Each value is parsed as `key=value` via `split_once('=')`.
    #[serde(default)]
    pub label: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct SessionHistoryQuery {
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Session response — canonical wire type from contracts.
pub type SessionResponse = meerkat_contracts::WireRunResult;

/// Usage response — re-export from contracts.
pub type UsageResponse = meerkat_contracts::WireUsage;

/// Session details response
#[derive(Debug, Serialize)]
pub struct SessionDetailsResponse {
    pub session_id: String,
    pub session_ref: String,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: usize,
    pub total_tokens: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct ScheduleListResponse {
    pub schedules: Vec<meerkat::Schedule>,
}

#[derive(Debug, Serialize)]
pub struct ScheduleOccurrencesResponse {
    pub occurrences: Vec<meerkat::Occurrence>,
}

/// API error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// Build the REST API router
pub fn router(state: AppState) -> Router {
    let schedule_state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = schedule_state.ensure_schedule_host_started().await {
            tracing::warn!("failed to start REST schedule host: {error}");
        }
    });

    let r = Router::new()
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/sessions/{id}", get(get_session).delete(archive_session))
        .route("/sessions/{id}/history", get(get_session_history))
        .route("/sessions/{id}/interrupt", post(interrupt_session))
        .route("/sessions/{id}/status", get(get_runtime_status))
        .route(
            "/sessions/{id}/realtime-attachment-status",
            get(get_realtime_attachment_status),
        )
        .route("/sessions/{id}/system_context", post(append_system_context))
        .route("/sessions/{id}/messages", post(continue_session))
        .route("/sessions/{id}/external-events", post(post_external_event))
        .route(
            "/sessions/{id}/peer-response-terminal",
            post(post_peer_response_terminal),
        )
        .route("/sessions/{id}/events", get(session_events))
        .route("/schedule/tools", get(schedule_tools))
        .route("/schedule/call", post(schedule_call))
        .route("/schedules", get(list_schedules).post(create_schedule))
        .route(
            "/schedules/{id}",
            get(get_schedule)
                .patch(update_schedule)
                .delete(delete_schedule),
        )
        .route("/schedules/{id}/pause", post(pause_schedule))
        .route("/schedules/{id}/resume", post(resume_schedule))
        .route(
            "/schedules/{id}/occurrences",
            get(list_schedule_occurrences),
        )
        .route("/requests/{request_id}/cancel", post(cancel_request))
        .route("/comms/send", post(comms_send))
        .route("/comms/peers", get(comms_peers))
        .route(
            "/config",
            get(get_config).put(set_config).patch(patch_config),
        )
        .route("/health", get(health_check))
        .route("/skills", get(list_skills))
        .route("/capabilities", get(get_capabilities))
        .route("/runtime/host_info", get(get_runtime_host_info))
        .route("/runtime/capabilities", get(get_runtime_capabilities))
        .route("/runtime/health", get(get_runtime_health))
        .route("/models/catalog", get(get_models_catalog))
        .route("/realtime/status", post(realtime_status))
        .route("/realtime/capabilities", post(realtime_capabilities))
        .route("/realtime/open_info", post(realtime_open_info))
        // Phase 4d — auth + realm endpoints.
        .route(
            "/auth/profiles",
            get(crate::auth_endpoints::list_auth_profiles)
                .post(crate::auth_endpoints::create_auth_profile),
        )
        .route(
            "/auth/bindings/{binding_id}",
            get(crate::auth_endpoints::get_auth_profile)
                .delete(crate::auth_endpoints::delete_auth_profile),
        )
        .route(
            "/auth/bindings/{binding_id}/test",
            post(crate::auth_endpoints::test_auth_binding),
        )
        .route(
            "/auth/login/start",
            post(crate::auth_endpoints::start_login),
        )
        .route(
            "/auth/login/complete",
            post(crate::auth_endpoints::complete_login),
        )
        .route(
            "/auth/login/device/start",
            post(crate::auth_endpoints::start_device_login),
        )
        .route(
            "/auth/login/device/complete",
            post(crate::auth_endpoints::complete_device_login),
        )
        .route(
            "/auth/bindings/{binding_id}/status",
            get(crate::auth_endpoints::get_auth_status),
        )
        .route(
            "/auth/bindings/{binding_id}/logout",
            post(crate::auth_endpoints::logout),
        )
        .route("/realms", get(crate::auth_endpoints::list_realms))
        .route("/realms/{id}", get(crate::auth_endpoints::get_realm));

    #[cfg(feature = "mob")]
    let r = r
        .route("/mob/{id}/events", get(mob_event_stream))
        .route("/mob/{id}/spawn-helper", post(mob_spawn_helper))
        .route("/mob/{id}/fork-helper", post(mob_fork_helper))
        .route("/mob/{id}/wait-kickoff", post(mob_wait_kickoff))
        .route(
            "/mob/{id}/members/{agent_identity}/status",
            get(mob_member_status),
        )
        .route(
            "/mob/{id}/members/{agent_identity}/cancel",
            post(mob_force_cancel),
        )
        .route(
            "/mob/{id}/members/{agent_identity}/respawn",
            post(mob_member_respawn),
        );

    #[cfg(feature = "mcp")]
    let r = r
        .route("/sessions/{id}/mcp/add", post(mcp_add))
        .route("/sessions/{id}/mcp/remove", post(mcp_remove))
        .route("/sessions/{id}/mcp/reload", post(mcp_reload));

    r.with_state(state)
}

// ---------------------------------------------------------------------------
// v9 Runtime / Input endpoints
// ---------------------------------------------------------------------------

/// Helper: get the runtime adapter reference.
fn get_runtime_adapter(state: &AppState) -> &Arc<meerkat_runtime::MeerkatMachine> {
    &state.runtime_adapter
}

fn session_metadata_marks_archived(session: &Session) -> bool {
    session
        .metadata()
        .get("session_archived")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn runtime_state_to_wire(
    state: meerkat_runtime::RuntimeState,
) -> meerkat_contracts::WireRuntimeState {
    match state {
        meerkat_runtime::RuntimeState::Initializing => {
            meerkat_contracts::WireRuntimeState::Initializing
        }
        meerkat_runtime::RuntimeState::Idle => meerkat_contracts::WireRuntimeState::Idle,
        meerkat_runtime::RuntimeState::Attached => meerkat_contracts::WireRuntimeState::Attached,
        meerkat_runtime::RuntimeState::Running => meerkat_contracts::WireRuntimeState::Running,
        meerkat_runtime::RuntimeState::Retired => meerkat_contracts::WireRuntimeState::Retired,
        meerkat_runtime::RuntimeState::Stopped => meerkat_contracts::WireRuntimeState::Stopped,
        meerkat_runtime::RuntimeState::Destroyed => meerkat_contracts::WireRuntimeState::Destroyed,
        _ => meerkat_contracts::WireRuntimeState::Destroyed,
    }
}

async fn get_runtime_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RuntimeStateResult>, Response> {
    let session_id =
        resolve_session_id_for_state(&id, &state).map_err(IntoResponse::into_response)?;
    let adapter = get_runtime_adapter(&state);
    let runtime_state = adapter.runtime_state(&session_id).await.map_err(|err| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": err.to_string()})),
        )
            .into_response()
    })?;
    Ok(Json(RuntimeStateResult {
        state: runtime_state_to_wire(runtime_state),
    }))
}

async fn get_realtime_attachment_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RuntimeRealtimeAttachmentStatusResult>, Response> {
    let session_id =
        resolve_session_id_for_state(&id, &state).map_err(IntoResponse::into_response)?;
    let adapter = get_runtime_adapter(&state);
    let status = adapter
        .realtime_attachment_status(&session_id)
        .await
        .map_err(|err| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": err.to_string()})),
            )
                .into_response()
        })?;
    let wire = match status {
        meerkat_runtime::RealtimeAttachmentStatus::Unattached => {
            meerkat_contracts::WireRealtimeAttachmentStatus::Unattached
        }
        meerkat_runtime::RealtimeAttachmentStatus::IntentPresentUnbound => {
            meerkat_contracts::WireRealtimeAttachmentStatus::IntentPresentUnbound
        }
        meerkat_runtime::RealtimeAttachmentStatus::BindingNotReady => {
            meerkat_contracts::WireRealtimeAttachmentStatus::BindingNotReady
        }
        meerkat_runtime::RealtimeAttachmentStatus::BindingReady => {
            meerkat_contracts::WireRealtimeAttachmentStatus::BindingReady
        }
        meerkat_runtime::RealtimeAttachmentStatus::ReplacementPending => {
            meerkat_contracts::WireRealtimeAttachmentStatus::ReplacementPending
        }
        meerkat_runtime::RealtimeAttachmentStatus::ReattachRequired => {
            meerkat_contracts::WireRealtimeAttachmentStatus::ReattachRequired
        }
    };
    Ok(Json(RuntimeRealtimeAttachmentStatusResult { status: wire }))
}

// `realtime_status_from_mob_serialized` and
// `ensure_realtime_mob_member_target_available` were removed in Phase
// 5G/T5i alongside the `RealtimeChannelTarget::MobMemberTarget` variant.

async fn realtime_status(
    State(state): State<AppState>,
    Json(body): Json<RealtimeStatusParams>,
) -> Result<Json<RealtimeStatusResult>, Response> {
    call_realtime_rpc(&state, RestRealtimeRpcMethod::Status, &body)
        .await
        .map(Json)
}

#[derive(Debug, Clone, Copy)]
enum RestRealtimeRpcMethod {
    Status,
    Capabilities,
    OpenInfo,
}

impl RestRealtimeRpcMethod {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Status => "realtime/status",
            Self::Capabilities => "realtime/capabilities",
            Self::OpenInfo => "realtime/open_info",
        }
    }

    const fn unavailable_message(self) -> &'static str {
        match self {
            Self::Status => {
                "realtime/status is unavailable until the realtime RPC host is configured"
            }
            Self::Capabilities => {
                "realtime/capabilities is unavailable until the realtime RPC host is configured"
            }
            Self::OpenInfo => {
                "realtime/open_info is unavailable until the realtime websocket host ships"
            }
        }
    }
}

fn rest_wire_error(
    code: ErrorCode,
    message: impl Into<std::borrow::Cow<'static, str>>,
) -> Response {
    let status =
        StatusCode::from_u16(code.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(WireError::new(code, message))).into_response()
}

fn rest_wire_error_with_details(
    code: ErrorCode,
    message: impl Into<std::borrow::Cow<'static, str>>,
    details: Option<Value>,
) -> Response {
    let mut error = WireError::new(code, message);
    if let Some(details) = details {
        error = error.with_details(details);
    }
    let status =
        StatusCode::from_u16(code.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(error)).into_response()
}

fn realtime_rpc_error_response(error: meerkat_rpc::protocol::RpcError) -> Response {
    let code = ErrorCode::from_jsonrpc_code(error.code).unwrap_or(ErrorCode::InternalError);
    rest_wire_error_with_details(code, error.message, error.data)
}

async fn call_realtime_rpc<TParams, TResult>(
    state: &AppState,
    method: RestRealtimeRpcMethod,
    params: &TParams,
) -> Result<TResult, Response>
where
    TParams: Serialize,
    TResult: DeserializeOwned,
{
    let Some(addr) = state.realtime_rpc_tcp_addr.as_deref() else {
        return Err(rest_wire_error(
            ErrorCode::CapabilityUnavailable,
            method.unavailable_message(),
        ));
    };

    let params = serde_json::value::to_raw_value(params).map_err(|err| {
        rest_wire_error(
            ErrorCode::InvalidParams,
            format!("realtime RPC params could not be serialized: {err}"),
        )
    })?;
    let request = meerkat_rpc::protocol::RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(meerkat_rpc::protocol::RpcId::Num(1)),
        method: method.as_str().to_string(),
        params: Some(params),
    };
    let line = serde_json::to_string(&request).map_err(|err| {
        rest_wire_error(
            ErrorCode::InternalError,
            format!("realtime RPC request could not be encoded: {err}"),
        )
    })?;

    let mut stream = TcpStream::connect(addr).await.map_err(|err| {
        rest_wire_error(
            ErrorCode::ProviderError,
            format!("realtime RPC connection failed: {err}"),
        )
    })?;
    stream
        .write_all(format!("{line}\n").as_bytes())
        .await
        .map_err(|err| {
            rest_wire_error(
                ErrorCode::ProviderError,
                format!("realtime RPC write failed: {err}"),
            )
        })?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await.map_err(|err| {
        rest_wire_error(
            ErrorCode::ProviderError,
            format!("realtime RPC read failed: {err}"),
        )
    })?;
    let response: meerkat_rpc::protocol::RpcResponse =
        serde_json::from_str(&line).map_err(|err| {
            rest_wire_error(
                ErrorCode::ProviderError,
                format!("realtime RPC response was invalid: {err}"),
            )
        })?;
    if let Some(error) = response.error {
        return Err(realtime_rpc_error_response(error));
    }
    let Some(result) = response.result else {
        return Err(rest_wire_error(
            ErrorCode::InternalError,
            "realtime RPC response did not contain a result",
        ));
    };

    serde_json::from_str(result.get()).map_err(|err| {
        rest_wire_error(
            ErrorCode::ProviderError,
            format!("realtime RPC result had unexpected shape: {err}"),
        )
    })
}

async fn realtime_capabilities(
    State(state): State<AppState>,
    Json(body): Json<RealtimeCapabilitiesParams>,
) -> Result<Json<RealtimeCapabilitiesResult>, Response> {
    call_realtime_rpc(&state, RestRealtimeRpcMethod::Capabilities, &body)
        .await
        .map(Json)
}

async fn realtime_open_info(
    State(state): State<AppState>,
    Json(body): Json<RealtimeOpenRequest>,
) -> Result<Json<RealtimeOpenInfo>, Response> {
    call_realtime_rpc(&state, RestRealtimeRpcMethod::OpenInfo, &body)
        .await
        .map(Json)
}

// ---------------------------------------------------------------------------

/// Health check endpoint
async fn health_check() -> &'static str {
    "ok"
}

/// Query parameters for `GET /mob/{id}/events`.
#[derive(Debug, Deserialize)]
#[cfg(feature = "mob")]
struct MobEventStreamQuery {
    /// Subscribe to a single member's agent events. If absent, subscribes
    /// to the mob-wide event bus covering all members.
    #[serde(default)]
    member: Option<String>,
}

/// GET /mob/{id}/events — SSE stream of mob or per-member agent events.
///
/// If `?member=<agent_identity>` is provided, streams that member's session
/// events. Otherwise streams the mob-wide event bus which merges all
/// member events tagged with [`AttributedEvent`] attribution.
#[cfg(feature = "mob")]
async fn mob_event_stream(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<MobEventStreamQuery>,
) -> Result<Sse<std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>>, ApiError>
{
    let mob_id = meerkat_mob::MobId::from(id.as_str());

    let stream: std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        if let Some(member) = query.member {
            // Per-member agent event stream.
            let identity = meerkat_mob::AgentIdentity::from(member.as_str());
            let mut event_stream = state
                .mob_state
                .subscribe_agent_events(&mob_id, &identity)
                .await
                .map_err(|e| ApiError::NotFound(e.to_string()))?;

            Box::pin(async_stream::stream! {
                yield Ok(Event::default().event("stream_opened").data(
                    serde_json::to_string(&json!({
                        "mob_id": id,
                        "member": member,
                    })).unwrap_or_default(),
                ));

                while let Some(envelope) = futures::StreamExt::next(&mut event_stream).await {
                    let event_type = agent_event_type(&envelope.payload);
                    let data = serde_json::to_string(&envelope).unwrap_or_default();
                    yield Ok(Event::default().event(event_type).data(data));
                }

                yield Ok(Event::default().event("done").data("{}"));
            })
        } else {
            // Mob-wide event stream (all members, continuously updated).
            let mut handle = state
                .mob_state
                .subscribe_mob_events(&mob_id)
                .await
                .map_err(|e| ApiError::NotFound(e.to_string()))?;

            Box::pin(async_stream::stream! {
                yield Ok(Event::default().event("stream_opened").data(
                    serde_json::to_string(&json!({
                        "mob_id": id,
                    })).unwrap_or_default(),
                ));

                while let Some(attributed) = handle.event_rx.recv().await {
                    let event_type = agent_event_type(&attributed.envelope.payload);
                    let data = serde_json::to_string(&attributed).unwrap_or_default();
                    yield Ok(Event::default().event(event_type).data(data));
                }

                yield Ok(Event::default().event("done").data("{}"));
            })
        };

    Ok(Sse::new(stream))
}

// ---------------------------------------------------------------------------
// Mob parity endpoints
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[cfg(feature = "mob")]
struct SpawnHelperRequest {
    prompt: String,
    #[serde(default)]
    agent_identity: Option<String>,
    #[serde(default)]
    role_name: Option<String>,
    #[serde(default)]
    runtime_mode: Option<meerkat_mob::MobRuntimeMode>,
    #[serde(default)]
    backend: Option<meerkat_mob::MobBackendKind>,
}

/// POST /mob/{id}/spawn-helper — spawn a short-lived helper, wait, return result.
#[cfg(feature = "mob")]
async fn mob_spawn_helper(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SpawnHelperRequest>,
) -> Result<Json<Value>, ApiError> {
    let mob_id = meerkat_mob::MobId::from(id.as_str());
    let identity = meerkat_mob::AgentIdentity::from(
        req.agent_identity
            .unwrap_or_else(|| format!("helper-{}", uuid::Uuid::new_v4())),
    );
    let mut options = meerkat_mob::HelperOptions::default();
    if let Some(role) = req.role_name {
        options.role_name = Some(meerkat_mob::ProfileName::from(role));
    }
    options.runtime_mode = req.runtime_mode;
    options.backend = req.backend;
    let result = state
        .mob_state
        .mob_spawn_helper(&mob_id, identity, req.prompt, options)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let payload = serde_json::to_value(result)
        .map_err(|e| ApiError::Internal(format!("serialize helper result: {e}")))?;
    Ok(Json(payload))
}

#[derive(Debug, Deserialize)]
#[cfg(feature = "mob")]
struct ForkHelperRequest {
    source_member_id: String,
    prompt: String,
    #[serde(default)]
    agent_identity: Option<String>,
    #[serde(default)]
    role_name: Option<String>,
    #[serde(default)]
    fork_context: Option<meerkat_mob::ForkContext>,
    #[serde(default)]
    runtime_mode: Option<meerkat_mob::MobRuntimeMode>,
    #[serde(default)]
    backend: Option<meerkat_mob::MobBackendKind>,
}

#[derive(Debug, Deserialize, Default)]
#[cfg(feature = "mob")]
struct WaitKickoffRequest {
    #[serde(default)]
    member_ids: Option<Vec<String>>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// POST /mob/{id}/wait-kickoff — wait for autonomous kickoff completion barrier.
#[cfg(feature = "mob")]
async fn mob_wait_kickoff(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<WaitKickoffRequest>>,
) -> Result<Json<Value>, ApiError> {
    let mob_id = meerkat_mob::MobId::from(id.as_str());
    let request = body.map(|Json(value)| value).unwrap_or_default();
    let member_ids = request.member_ids.map(|member_ids| {
        member_ids
            .into_iter()
            .map(|member_id| meerkat_mob::AgentIdentity::from(member_id.as_str()))
            .collect::<Vec<_>>()
    });
    let members = state
        .mob_state
        .mob_wait_kickoff(&mob_id, member_ids, request.timeout_ms)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    Ok(Json(json!({ "members": members })))
}

/// POST /mob/{id}/fork-helper — fork from a member's context, wait, return result.
#[cfg(feature = "mob")]
async fn mob_fork_helper(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ForkHelperRequest>,
) -> Result<Json<Value>, ApiError> {
    let mob_id = meerkat_mob::MobId::from(id.as_str());
    let source_identity = meerkat_mob::AgentIdentity::from(req.source_member_id.as_str());
    let identity = meerkat_mob::AgentIdentity::from(
        req.agent_identity
            .unwrap_or_else(|| format!("fork-{}", uuid::Uuid::new_v4())),
    );
    let fork_context = req
        .fork_context
        .unwrap_or(meerkat_mob::ForkContext::FullHistory);
    let mut options = meerkat_mob::HelperOptions::default();
    if let Some(role) = req.role_name {
        options.role_name = Some(meerkat_mob::ProfileName::from(role));
    }
    options.runtime_mode = req.runtime_mode;
    options.backend = req.backend;
    let result = state
        .mob_state
        .mob_fork_helper(
            &mob_id,
            &source_identity,
            identity,
            req.prompt,
            fork_context,
            options,
        )
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    let payload = serde_json::to_value(result)
        .map_err(|e| ApiError::Internal(format!("serialize helper result: {e}")))?;
    Ok(Json(payload))
}

/// GET /mob/{id}/members/{agent_identity}/status — member execution snapshot.
#[cfg(feature = "mob")]
async fn mob_member_status(
    State(state): State<AppState>,
    Path((id, agent_identity)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let mob_id = meerkat_mob::MobId::from(id.as_str());
    let identity = meerkat_mob::AgentIdentity::from(agent_identity.as_str());
    let snapshot = state
        .mob_state
        .mob_member_status(&mob_id, &identity)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(json!(snapshot)))
}

/// POST /mob/{id}/members/{agent_identity}/cancel — force-cancel in-flight turn.
#[cfg(feature = "mob")]
async fn mob_force_cancel(
    State(state): State<AppState>,
    Path((id, agent_identity)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let mob_id = meerkat_mob::MobId::from(id.as_str());
    let identity = meerkat_mob::AgentIdentity::from(agent_identity.as_str());
    state
        .mob_state
        .mob_force_cancel(&mob_id, identity)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(json!({"cancelled": true})))
}

/// POST /mob/{id}/members/{agent_identity}/respawn — retire + respawn member.
#[cfg(feature = "mob")]
async fn mob_member_respawn(
    State(state): State<AppState>,
    Path((id, agent_identity)): Path<(String, String)>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ApiError> {
    let mob_id = meerkat_mob::MobId::from(id.as_str());
    let identity = meerkat_mob::AgentIdentity::from(agent_identity.as_str());
    let initial_message = body
        .and_then(|Json(v)| v.get("initial_message").cloned())
        .map(serde_json::from_value::<ContentInput>)
        .transpose()
        .map_err(|e| ApiError::BadRequest(format!("invalid initial_message: {e}")))?;
    match state
        .mob_state
        .mob_respawn(&mob_id, identity, initial_message)
        .await
    {
        Ok(receipt) => Ok(Json(json!({
            "status": "completed",
            "receipt": receipt,
        }))),
        Err(meerkat_mob::MobRespawnError::TopologyRestoreFailed {
            receipt,
            failed_peer_ids,
        }) => Ok(Json(json!({
            "status": "topology_restore_failed",
            "receipt": receipt,
            "failed_peer_ids": failed_peer_ids.iter().map(std::string::ToString::to_string).collect::<Vec<_>>(),
        }))),
        Err(e) => Err(ApiError::BadRequest(e.to_string())),
    }
}

/// Canonical comms send request body.
///
/// `command` carries the typed [`meerkat_core::comms::CommsCommandRequest`]
/// enum (serde-tagged on `kind`). Wire shape is flat:
/// `{"session_id": "...", "kind": "...", ...}`.
#[derive(Debug, Deserialize)]
pub struct CommsSendRequest {
    pub session_id: String,
    #[serde(flatten)]
    pub command: meerkat_core::comms::CommsCommandRequest,
}

impl CommsSendRequest {
    /// Recipient peer id for error normalization, if the command targets one.
    ///
    fn peer_label(&self) -> Option<String> {
        use meerkat_core::comms::CommsCommandRequest;
        match &self.command {
            CommsCommandRequest::Input { .. } => None,
            CommsCommandRequest::PeerMessage { to, .. }
            | CommsCommandRequest::PeerLifecycle { to, .. }
            | CommsCommandRequest::PeerRequest { to, .. }
            | CommsCommandRequest::PeerResponse { to, .. } => Some(to.to_string()),
        }
    }
}

/// Canonical comms peers request body.
#[derive(Debug, Deserialize)]
pub struct CommsPeersRequest {
    pub session_id: String,
}

fn is_transport_internal(message: &str) -> bool {
    message.starts_with("Transport error:") || message.starts_with("IO error:")
}

/// POST /comms/send — dispatch a canonical comms command.
async fn comms_send(
    State(state): State<AppState>,
    Json(req): Json<CommsSendRequest>,
) -> Result<Json<Value>, ApiError> {
    let session_id = resolve_session_id_for_state(&req.session_id, &state)?;

    let comms = state
        .session_service
        .comms_runtime(&session_id)
        .await
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "Session not found or comms not enabled: {session_id}"
            ))
        })?;

    let peer_name = req.peer_label();
    let cmd = req
        .command
        .into_command(&session_id)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;

    match comms.send(cmd).await {
        Ok(receipt) => Ok(Json(comms_send_receipt_json(receipt))),
        Err(e) => Err(normalize_rest_comms_send_error(peer_name.as_deref(), &e)),
    }
}

fn comms_send_receipt_json(receipt: meerkat_core::comms::SendReceipt) -> Value {
    use meerkat_core::comms::SendReceipt;

    match receipt {
        SendReceipt::InputAccepted {
            interaction_id,
            stream_reserved,
        } => json!({
            "kind": "input_accepted",
            "interaction_id": interaction_id.0.to_string(),
            "stream_reserved": stream_reserved,
        }),
        SendReceipt::PeerMessageSent { envelope_id, acked } => json!({
            "kind": "peer_message_sent",
            "envelope_id": envelope_id.to_string(),
            "acked": acked,
        }),
        SendReceipt::PeerLifecycleSent { envelope_id } => json!({
            "kind": "peer_lifecycle_sent",
            "envelope_id": envelope_id.to_string(),
        }),
        SendReceipt::PeerRequestSent {
            envelope_id,
            interaction_id,
            stream_reserved,
        } => json!({
            "kind": "peer_request_sent",
            "envelope_id": envelope_id.to_string(),
            "interaction_id": interaction_id.0.to_string(),
            "request_id": envelope_id.to_string(),
            "stream_reserved": stream_reserved,
        }),
        SendReceipt::PeerResponseSent {
            envelope_id,
            in_reply_to,
        } => json!({
            "kind": "peer_response_sent",
            "envelope_id": envelope_id.to_string(),
            "in_reply_to": in_reply_to.0.to_string(),
        }),
    }
}

fn normalize_rest_comms_send_error(
    peer_name: Option<&str>,
    error: &meerkat_core::comms::SendError,
) -> ApiError {
    match error {
        meerkat_core::comms::SendError::PeerNotFound(peer) => ApiError::InternalWithData {
            message: format!(
                "peer_not_found_or_not_trusted: peer '{peer}' is not found or not trusted"
            ),
            code: "peer_not_found_or_not_trusted".to_string(),
            details: json!({
                "code": "peer_not_found_or_not_trusted",
                "peer": peer,
                "message": format!("peer '{peer}' is not found or not trusted"),
            }),
        },
        meerkat_core::comms::SendError::PeerOffline => {
            let peer = peer_name.unwrap_or("<unknown>");
            ApiError::InternalWithData {
                message: format!(
                    "peer_unreachable: peer '{peer}' is unreachable: offline_or_no_ack"
                ),
                code: "peer_unreachable".to_string(),
                details: json!({
                    "code": "peer_unreachable",
                    "peer": peer,
                    "reason": "offline_or_no_ack",
                    "message": format!("peer '{peer}' is unreachable: offline_or_no_ack"),
                }),
            }
        }
        meerkat_core::comms::SendError::Internal(details) if is_transport_internal(details) => {
            let peer = peer_name.unwrap_or("<unknown>");
            ApiError::InternalWithData {
                message: format!(
                    "peer_unreachable: peer '{peer}' is unreachable: transport_error ({details})"
                ),
                code: "peer_unreachable".to_string(),
                details: json!({
                    "code": "peer_unreachable",
                    "peer": peer,
                    "reason": "transport_error",
                    "message": format!("peer '{peer}' is unreachable: transport_error"),
                    "details": details,
                }),
            }
        }
        other => ApiError::InternalWithData {
            message: format!("send_failed: {other}"),
            code: "send_failed".to_string(),
            details: json!({
                "code": "send_failed",
                "message": other.to_string(),
            }),
        },
    }
}

/// GET /comms/peers — list peers visible to a session's comms runtime.
async fn comms_peers(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<CommsPeersRequest>,
) -> Result<Json<Value>, ApiError> {
    let session_id = resolve_session_id_for_state(&params.session_id, &state)?;

    let comms = state
        .session_service
        .comms_runtime(&session_id)
        .await
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "Session not found or comms not enabled: {session_id}"
            ))
        })?;

    Ok(Json(comms_peers_payload(comms.peers().await)))
}

fn comms_peers_payload(peers: Vec<meerkat_core::comms::PeerDirectoryEntry>) -> Value {
    json!(meerkat_contracts::CommsPeersResult::from_entries(&peers))
}

fn make_runtime_external_event_input(
    event_type: &str,
    payload: Value,
    blocks: Option<Vec<meerkat_contracts::WireContentBlock>>,
) -> Result<meerkat_runtime::Input, ApiError> {
    if event_type.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "event_type cannot be empty".to_string(),
        ));
    }

    let blocks = blocks
        .map(|blocks| {
            blocks
                .into_iter()
                .map(meerkat_core::types::ContentBlock::try_from)
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
        .map_err(|message| ApiError::BadRequest(message.to_string()))?;

    Ok(meerkat_runtime::Input::ExternalEvent(
        meerkat_runtime::ExternalEventInput {
            header: meerkat_runtime::InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: meerkat_runtime::InputOrigin::External {
                    source_name: event_type.to_string(),
                },
                durability: meerkat_runtime::InputDurability::Durable,
                visibility: meerkat_runtime::InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: event_type.to_string(),
            payload,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
            blocks,
        },
    ))
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RestSessionExternalEventEnvelope {
    GenericJson {
        event_type: String,
        payload: Value,
        #[serde(default)]
        blocks: Option<Vec<meerkat_contracts::WireContentBlock>>,
    },
    PeerResponseTerminal {},
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestPeerResponseTerminalBody {
    peer_id: meerkat_core::comms::PeerId,
    #[serde(default)]
    display_name: Option<meerkat_core::comms::PeerName>,
    request_id: meerkat_core::PeerCorrelationId,
    status: meerkat_contracts::PeerResponseTerminalStatusWire,
    result: Value,
}

/// Queue an external event into the runtime without waking an idle session.
///
/// Authentication is controlled by `RKAT_WEBHOOK_SECRET` env var:
/// - If not set: no authentication (suitable for localhost/dev)
/// - If set: requires `X-Webhook-Secret` header with matching value
async fn post_external_event(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(event): Json<RestSessionExternalEventEnvelope>,
) -> Result<(StatusCode, Json<Value>), Response> {
    // Webhook auth resolved once at startup, stored in AppState.
    webhook::verify_webhook(&headers, &state.webhook_auth)
        .map_err(|msg| ApiError::Unauthorized(msg.to_string()).into_response())?;

    let session_id =
        resolve_session_id_for_state(&id, &state).map_err(IntoResponse::into_response)?;

    let input = match event {
        RestSessionExternalEventEnvelope::GenericJson {
            event_type,
            payload,
            blocks,
        } => make_runtime_external_event_input(&event_type, payload, blocks),
        RestSessionExternalEventEnvelope::PeerResponseTerminal {} => Err(ApiError::BadRequest(
            "peer_response_terminal is reserved on /external-events; use /peer-response-terminal"
                .to_string(),
        )),
    }
    .map_err(IntoResponse::into_response)?;

    admit_runtime_input_via_webhook(
        &state,
        &session_id,
        input,
        WebhookAdmissionMode::WithoutWake,
    )
    .await
}

/// Admit a correlated terminal peer response through the typed runtime ingress.
async fn post_peer_response_terminal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<RestPeerResponseTerminalBody>,
) -> Result<(StatusCode, Json<Value>), Response> {
    webhook::verify_webhook(&headers, &state.webhook_auth)
        .map_err(|msg| ApiError::Unauthorized(msg.to_string()).into_response())?;

    let session_id =
        resolve_session_id_for_state(&id, &state).map_err(IntoResponse::into_response)?;

    let RestPeerResponseTerminalBody {
        peer_id,
        display_name,
        request_id,
        status,
        result,
    } = body;

    let input = meerkat_runtime::peer_response_terminal_input(
        peer_id,
        display_name,
        request_id,
        status,
        result,
    );

    admit_runtime_input_via_webhook(&state, &session_id, input, WebhookAdmissionMode::Wakeful).await
}

#[derive(Debug, Clone, Copy)]
enum WebhookAdmissionMode {
    /// Stage the input without waking an idle runtime. Used by generic
    /// external events that are explicitly "next turn boundary" work.
    WithoutWake,
    /// Admit normally, allowing the runtime policy to wake or steer. Used by
    /// terminal peer responses that may unblock live work.
    Wakeful,
}

/// Admit a webhook-originated runtime input and map the typed accept outcome
/// into the HTTP response shape shared by `/external-events` and
/// `/peer-response-terminal`.
async fn admit_runtime_input_via_webhook(
    state: &AppState,
    session_id: &SessionId,
    input: meerkat_runtime::Input,
    mode: WebhookAdmissionMode,
) -> Result<(StatusCode, Json<Value>), Response> {
    let outcome = match mode {
        WebhookAdmissionMode::WithoutWake => {
            state
                .runtime_adapter
                .accept_input_without_wake(session_id, input)
                .await
        }
        WebhookAdmissionMode::Wakeful => {
            state.runtime_adapter.accept_input(session_id, input).await
        }
    };

    match outcome {
        Ok(meerkat_runtime::AcceptOutcome::Accepted { .. })
        | Ok(meerkat_runtime::AcceptOutcome::Deduplicated { .. }) => {
            Ok((StatusCode::ACCEPTED, Json(json!({"queued": true}))))
        }
        Ok(meerkat_runtime::AcceptOutcome::Rejected { reason }) => {
            Err((StatusCode::CONFLICT, Json(json!({"error": reason}))).into_response())
        }
        Ok(outcome) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": format!("unexpected runtime accept outcome: {outcome:?}"),
            })),
        )
            .into_response()),
        Err(meerkat_runtime::RuntimeDriverError::NotReady { state }) => Err((
            StatusCode::CONFLICT,
            Json(json!({
                "error": format!("runtime not accepting input while in state: {state}"),
            })),
        )
            .into_response()),
        Err(err) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": err.to_string()})),
        )
            .into_response()),
    }
}

/// List skills with provenance information.
async fn list_skills(
    State(state): State<AppState>,
) -> Result<Json<meerkat_contracts::SkillListResponse>, ApiError> {
    let runtime = state
        .skill_runtime
        .as_ref()
        .ok_or_else(|| ApiError::NotFound("skills not enabled".into()))?;

    let entries = runtime
        .list_all_with_provenance(&meerkat_core::skills::SkillFilter::default())
        .await
        .map_err(|e| ApiError::Internal(format!("skill list failed: {e}")))?;

    let wire: Vec<meerkat_contracts::SkillEntry> =
        entries.iter().map(skill_entry).collect::<Result<_, _>>()?;
    Ok(Json(meerkat_contracts::SkillListResponse { skills: wire }))
}

fn skill_source_provenance(
    identity: meerkat_core::skills::SourceIdentityRecord,
) -> meerkat_contracts::SkillSourceProvenance {
    meerkat_contracts::SkillSourceProvenance { identity }
}

fn skill_entry(
    e: &meerkat_core::skills::SkillIntrospectionEntry,
) -> Result<meerkat_contracts::SkillEntry, ApiError> {
    let source_identity = e.source_identity.clone().ok_or_else(|| {
        ApiError::Internal(format!(
            "skill {} missing typed source identity",
            e.descriptor.key
        ))
    })?;
    Ok(meerkat_contracts::SkillEntry {
        key: e.descriptor.key.clone(),
        name: e.descriptor.name.clone(),
        description: e.descriptor.description.clone(),
        scope: e.descriptor.scope.to_string(),
        source: skill_source_provenance(source_identity),
        is_active: e.is_active,
        shadowed_by: e.shadowed_by_identity.clone().map(skill_source_provenance),
    })
}

/// Get runtime capabilities with status resolved against config.
async fn get_capabilities(
    State(state): State<AppState>,
) -> Json<meerkat_contracts::CapabilitiesResponse> {
    let config = state.config_store.get().await.unwrap_or_default();
    Json(meerkat::surface::build_capabilities_response(&config))
}

fn rest_runtime_host_surface_options(
    state: &AppState,
) -> meerkat::surface::RuntimeHostSurfaceOptions {
    let mut options = meerkat::surface::RuntimeHostSurfaceOptions::process(
        "meerkat-rest",
        env!("CARGO_PKG_VERSION"),
    );
    options.runtime_backed_sessions = true;
    options.mobs = cfg!(feature = "mob");
    options.mcp_live = cfg!(feature = "mcp");
    options.comms = cfg!(feature = "comms");
    options.blobs = true;
    // Artifact records currently have a JSON-RPC surface and may reference REST
    // blob transport, but REST does not yet expose artifact/* routes.
    options.artifacts = false;
    options.session_events = true;
    options.session_streams = true;
    options.schedules = cfg!(feature = "schedule");
    options.skills = true;
    options.approvals = false;
    options.rest_base_url = Some(format!("http://{}:{}", state.rest_host, state.rest_port));
    options.rest_paths = meerkat_contracts::rest_path_catalog()
        .into_iter()
        .map(|path| path.path.to_string())
        .collect();
    options
}

async fn get_runtime_host_info(
    State(state): State<AppState>,
) -> Json<meerkat_contracts::RuntimeHostInfo> {
    let options = rest_runtime_host_surface_options(&state);
    let metadata = state.config_store.metadata();
    let metadata = metadata
        .as_ref()
        .map(meerkat::surface::RuntimeHostMetadataProjection::from);
    Json(meerkat::surface::build_runtime_host_info(
        &options,
        metadata.as_ref(),
        state.context_root.clone(),
    ))
}

async fn get_runtime_capabilities(
    State(state): State<AppState>,
) -> Json<meerkat_contracts::RuntimeHostCapabilities> {
    let options = rest_runtime_host_surface_options(&state);
    Json(meerkat::surface::build_runtime_host_capabilities(&options))
}

async fn get_runtime_health() -> Json<meerkat_contracts::RuntimeHostHealth> {
    Json(meerkat::surface::build_runtime_host_health())
}

/// Get the effective model catalog for the current config.
async fn get_models_catalog(
    State(state): State<AppState>,
) -> Result<Json<meerkat_contracts::ModelsCatalogResponse>, ApiError> {
    let config = state.config_store.get().await.unwrap_or_default();
    let response = meerkat::surface::build_models_catalog_response(&config)
        .map_err(|e| ApiError::Configuration(e.to_string()))?;
    Ok(Json(response))
}

/// Get the current config
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SetConfigRequest {
    Wrapped {
        config: Config,
        #[serde(default)]
        expected_generation: Option<u64>,
    },
    Direct(Config),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PatchConfigRequest {
    Wrapped {
        patch: Value,
        #[serde(default)]
        expected_generation: Option<u64>,
    },
    Direct(Value),
}

async fn get_config(State(state): State<AppState>) -> Result<Json<ConfigEnvelope>, ApiError> {
    let snapshot = state
        .config_runtime
        .get()
        .await
        .map_err(config_runtime_err_to_api)?;
    Ok(Json(ConfigEnvelope::from_snapshot(
        snapshot,
        if state.expose_paths {
            ConfigEnvelopePolicy::Diagnostic
        } else {
            ConfigEnvelopePolicy::Public
        },
    )))
}

/// Replace the current config
async fn set_config(
    State(state): State<AppState>,
    Json(req): Json<SetConfigRequest>,
) -> Result<Json<ConfigEnvelope>, ApiError> {
    let (config, expected_generation) = match req {
        SetConfigRequest::Wrapped {
            config,
            expected_generation,
        } => (config, expected_generation),
        SetConfigRequest::Direct(config) => (config, None),
    };
    validate_config_for_commit_with_roots(
        &config,
        state.context_root.as_deref(),
        state.user_config_root.as_deref(),
    )?;
    let snapshot = state
        .config_runtime
        .set(config, expected_generation)
        .await
        .map_err(config_runtime_err_to_api)?;
    Ok(Json(ConfigEnvelope::from_snapshot(
        snapshot,
        if state.expose_paths {
            ConfigEnvelopePolicy::Diagnostic
        } else {
            ConfigEnvelopePolicy::Public
        },
    )))
}

/// Patch the current config using a JSON merge patch
async fn patch_config(
    State(state): State<AppState>,
    Json(req): Json<PatchConfigRequest>,
) -> Result<Json<ConfigEnvelope>, ApiError> {
    let (delta, expected_generation) = match req {
        PatchConfigRequest::Wrapped {
            patch,
            expected_generation,
        } => (patch, expected_generation),
        PatchConfigRequest::Direct(patch) => (patch, None),
    };
    let current = state
        .config_runtime
        .get()
        .await
        .map_err(config_runtime_err_to_api)?;
    let preview = apply_patch_preview(&current.config, delta.clone())?;
    validate_config_for_commit_with_roots(
        &preview,
        state.context_root.as_deref(),
        state.user_config_root.as_deref(),
    )?;
    let snapshot = state
        .config_runtime
        .patch(ConfigDelta(delta), expected_generation)
        .await
        .map_err(config_runtime_err_to_api)?;
    Ok(Json(ConfigEnvelope::from_snapshot(
        snapshot,
        if state.expose_paths {
            ConfigEnvelopePolicy::Diagnostic
        } else {
            ConfigEnvelopePolicy::Public
        },
    )))
}

fn config_runtime_err_to_api(err: meerkat_core::ConfigRuntimeError) -> ApiError {
    match err {
        meerkat_core::ConfigRuntimeError::GenerationConflict { expected, current } => {
            ApiError::BadRequest(format!(
                "Generation conflict: expected {expected}, current {current}"
            ))
        }
        other => ApiError::Configuration(other.to_string()),
    }
}

fn validate_config_for_commit_with_roots(
    config: &Config,
    _context_root: Option<&std::path::Path>,
    _user_root: Option<&std::path::Path>,
) -> Result<(), ApiError> {
    config
        .validate()
        .map_err(|e| ApiError::BadRequest(format!("Invalid config: {e}")))?;
    config
        .skills
        .build_source_identity_registry()
        .map_err(|e| ApiError::BadRequest(format!("Invalid skills source-identity config: {e}")))?;
    Ok(())
}

fn merge_patch(base: &mut Value, patch: Value) {
    match (base, patch) {
        (Value::Object(base_map), Value::Object(patch_map)) => {
            for (k, v) in patch_map {
                if v.is_null() {
                    base_map.remove(&k);
                } else {
                    merge_patch(base_map.entry(k).or_insert(Value::Null), v);
                }
            }
        }
        (base_val, patch_val) => {
            *base_val = patch_val;
        }
    }
}

fn apply_patch_preview(config: &Config, patch: Value) -> Result<Config, ApiError> {
    let mut value = serde_json::to_value(config)
        .map_err(|e| ApiError::Internal(format!("Failed to serialize config: {e}")))?;
    merge_patch(&mut value, patch);
    serde_json::from_value(value).map_err(|e| ApiError::BadRequest(format!("Invalid patch: {e}")))
}

/// Spawn a task that forwards events from an mpsc receiver to the broadcast channel,
/// optionally logging verbose events. Returns the join handle.
fn spawn_event_forwarder(
    mut event_rx: mpsc::Receiver<EventEnvelope<AgentEvent>>,
    broadcast_tx: broadcast::Sender<SessionEvent>,
    session_id: SessionId,
    verbose: bool,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            if verbose && let Some(line) = format_verbose_event(&event.payload) {
                tracing::info!("{}", line);
            }
            let _ = broadcast_tx.send(SessionEvent {
                session_id: session_id.clone(),
                event,
            });
        }
    })
}

async fn drain_event_forwarder(session_id: &SessionId, forwarder: tokio::task::JoinHandle<()>) {
    if tokio::time::timeout(std::time::Duration::from_millis(500), forwarder)
        .await
        .is_err()
    {
        tracing::debug!(
            session_id = %session_id,
            "event forwarder still draining after timeout; detaching task"
        );
    }
}

/// Convert a `RunResult` into a `SessionResponse` (via contracts `From` impl).
fn run_result_to_response(
    result: meerkat_core::types::RunResult,
    realm: &meerkat_core::RealmId,
) -> SessionResponse {
    let mut response: SessionResponse = result.into();
    response.session_ref = Some(format_session_ref(realm, &response.session_id));
    response
}

fn callback_pending_api_error(
    session_id: &SessionId,
    realm: &meerkat_core::RealmId,
    tool_name: String,
    args: Value,
    session_created: bool,
) -> ApiError {
    ApiError::InternalWithData {
        message: format!("callback pending for tool '{tool_name}'"),
        code: "CALLBACK_PENDING".to_string(),
        details: json!({
            "session_id": session_id.to_string(),
            "session_ref": format_session_ref(realm, session_id),
            "session_created": session_created,
            "resumable": true,
            "tool_name": tool_name,
            "args": args,
        }),
    }
}

fn completion_outcome_to_api_result(
    outcome: meerkat_runtime::completion::CompletionOutcome,
    session_id: &SessionId,
    realm: &meerkat_core::RealmId,
    session_created: bool,
) -> Result<meerkat_core::types::RunResult, ApiError> {
    match outcome {
        meerkat_runtime::completion::CompletionOutcome::Completed(run_result) => Ok(run_result),
        meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult => Err(
            ApiError::Internal("turn completed without result".to_string()),
        ),
        meerkat_runtime::completion::CompletionOutcome::CallbackPending { tool_name, args } => Err(
            callback_pending_api_error(session_id, realm, tool_name, args, session_created),
        ),
        meerkat_runtime::completion::CompletionOutcome::Abandoned(reason) => {
            Err(ApiError::Internal(format!("turn abandoned: {reason}")))
        }
        meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated(reason) => {
            Err(ApiError::Internal(format!("runtime terminated: {reason}")))
        }
    }
}

fn resolve_session_id_for_state(input: &str, state: &AppState) -> Result<SessionId, ApiError> {
    let locator = SessionLocator::parse(input)
        .map_err(|e| ApiError::BadRequest(format!("Invalid session locator '{input}': {e}")))?;
    if let Some(locator_realm) = locator.realm_id.as_ref()
        && locator_realm != &state.realm
    {
        return Err(ApiError::BadRequest(format!(
            "Session locator realm '{}' does not match active realm '{}'",
            locator_realm, state.realm
        )));
    }
    Ok(locator.session_id)
}

fn resolve_schedule_id(input: &str) -> Result<meerkat::ScheduleId, ApiError> {
    meerkat::ScheduleId::parse(input)
        .map_err(|e| ApiError::BadRequest(format!("Invalid schedule id '{input}': {e}")))
}

fn schedule_error_to_api(error: meerkat::ScheduleDomainError) -> ApiError {
    match error {
        meerkat::ScheduleDomainError::Store(meerkat::ScheduleStoreError::ScheduleNotFound {
            schedule_id,
        }) => ApiError::NotFound(format!("Schedule not found: {schedule_id}")),
        meerkat::ScheduleDomainError::Store(meerkat::ScheduleStoreError::UnsupportedBackend {
            ..
        }) => ApiError::ServiceUnavailable(error.to_string()),
        meerkat::ScheduleDomainError::InvalidSchedule(_)
        | meerkat::ScheduleDomainError::InvalidTrigger(_)
        | meerkat::ScheduleDomainError::InvalidCron(_) => ApiError::BadRequest(error.to_string()),
        other => ApiError::Internal(other.to_string()),
    }
}

fn schedule_tool_error_to_api(error: meerkat::ScheduleToolError) -> ApiError {
    match error.code {
        meerkat::SCHEDULE_TOOL_INVALID_ARGUMENTS => ApiError::BadRequest(error.message),
        meerkat::SCHEDULE_TOOL_NOT_FOUND => ApiError::NotFound(error.message),
        meerkat::SCHEDULE_TOOL_CAPABILITY_UNAVAILABLE => {
            ApiError::ServiceUnavailable(error.message)
        }
        _ => ApiError::Internal(error.message),
    }
}

/// Extract an optional `RequestContext` from the `X-Meerkat-Request-Id` header
/// and register it with the canonical surface request executor.
///
/// Requests without the header are not tracked (returns `Ok(None)`). Empty or
/// malformed headers surface as `BadRequest`. Duplicate keys are rejected via
/// the executor's typed `RequestAlreadyExists` outcome.
fn extract_request_context(
    headers: &axum::http::HeaderMap,
    executor: &SurfaceRequestExecutor,
) -> Result<Option<RequestContext>, ApiError> {
    let Some(header_value) = headers.get("x-meerkat-request-id") else {
        return Ok(None);
    };
    let request_id = header_value
        .to_str()
        .map_err(|_| ApiError::BadRequest("X-Meerkat-Request-Id must be valid UTF-8".into()))?
        .trim();
    if request_id.is_empty() {
        return Err(ApiError::BadRequest(
            "X-Meerkat-Request-Id must not be empty".into(),
        ));
    }
    match executor.try_begin_request(request_id, noop_request_action()) {
        Ok(ctx) => Ok(Some(ctx)),
        Err(RequestAlreadyExists) => Err(ApiError::DuplicateRequestId {
            request_id: request_id.to_string(),
        }),
    }
}

/// Drive a `RequestTerminal` outcome through the canonical surface request
/// executor: `Publish(val)` must claim committed publication before the
/// response is returned; if cancel arrived first, the cancellation response
/// wins. `RespondWithoutPublish(val)` routes through `finish_unpublished` so
/// any late cancel that arrived first still supersedes the response.
async fn with_request_lifecycle(
    executor: &SurfaceRequestExecutor,
    ctx: Option<RequestContext>,
    outcome: RequestTerminal<Result<Json<SessionResponse>, ApiError>>,
) -> Result<Json<SessionResponse>, ApiError> {
    let request_key = ctx.as_ref().map(|ctx| ctx.key().to_string());
    match executor
        .resolve_terminal(request_key.as_deref(), outcome)
        .await
    {
        RequestTerminalResolution::Emit(val) => val,
        RequestTerminalResolution::Cancelled => Err(ApiError::RequestCancelled { details: None }),
        RequestTerminalResolution::LifecycleError(err) => Err(ApiError::Internal(format!(
            "request lifecycle rejected publish response: {err}"
        ))),
    }
}

/// Create and run a new session. Extracts the optional request-lifecycle
/// context from `X-Meerkat-Request-Id`, runs the typed inner body, and routes
/// the `RequestTerminal` outcome through the canonical surface request
/// executor seam.
async fn create_session(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<SessionResponse>, ApiError> {
    let req_ctx = extract_request_context(&headers, &state.request_executor)?;
    let executor = state.request_executor.clone();
    let outcome = Box::pin(create_session_inner(&state, req, req_ctx.clone())).await;
    with_request_lifecycle(&executor, req_ctx, outcome).await
}

/// Create and run a new session (typed-terminal inner body).
async fn create_session_inner(
    state: &AppState,
    req: CreateSessionRequest,
    req_ctx: Option<RequestContext>,
) -> RequestTerminal<Result<Json<SessionResponse>, ApiError>> {
    // --- Validation (pre-stateful work) ---
    if let Err(e) = validate_public_peer_meta(req.peer_meta.as_ref()) {
        return RequestTerminal::RespondWithoutPublish(Err(e));
    }
    if let Err(e) = validate_public_surface_metadata(req.labels.as_ref(), req.app_context.as_ref())
    {
        return RequestTerminal::RespondWithoutPublish(Err(e));
    }
    let keep_alive_override = match resolve_keep_alive(req.keep_alive) {
        Ok(v) => v,
        Err(e) => return RequestTerminal::RespondWithoutPublish(Err(e)),
    };
    // Create: no persisted session to inherit from, so None → false.
    let keep_alive = keep_alive_override.unwrap_or(false);
    let model = req.model.unwrap_or_else(|| state.default_model.clone());
    let max_tokens = req.max_tokens.unwrap_or(state.max_tokens);
    let skill_references = match canonical_skill_keys_for_state(state, req.skill_refs.clone()).await
    {
        Ok(v) => v,
        Err(e) => return RequestTerminal::RespondWithoutPublish(Err(e)),
    };

    // Early cancel check — before any stateful work.
    if let Some(ctx) = req_ctx.as_ref()
        && ctx.cancel_already_requested()
    {
        return RequestTerminal::RespondWithoutPublish(Err(ApiError::RequestCancelled {
            details: None,
        }));
    }

    // --- Preclaim: session, runtime registration, MCP adapter ---

    // Pre-create a session to claim the session_id (needed for CallbackPending handling
    // and event forwarding before the service call returns).
    let pre_session = Session::new();
    let session_id = pre_session.id().clone();
    let bindings = match state
        .runtime_adapter
        .prepare_bindings(session_id.clone())
        .await
    {
        Ok(v) => v,
        Err(e) => {
            let message = format!("failed to prepare runtime bindings: {e}");
            return RequestTerminal::RespondWithoutPublish(Err(ApiError::Internal(message)));
        }
    };

    // Set up event forwarding: caller channel -> broadcast
    let (caller_event_tx, caller_event_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(100);
    let forward_task = spawn_event_forwarder(
        caller_event_rx,
        state.event_tx.clone(),
        session_id.clone(),
        req.verbose,
    );

    // Create MCP adapter and compose with external tools.
    #[cfg(feature = "mcp")]
    let mcp_external_tools = {
        let adapter = Arc::new(McpRouterAdapter::new(McpRouter::new()));
        let adapter_dispatcher: Arc<dyn AgentToolDispatcher> = adapter.clone();
        let (lifecycle_tx, lifecycle_rx) = mpsc::unbounded_channel();
        let mcp_state = SessionMcpState {
            adapter,
            turn_counter: 0,
            lifecycle_tx,
            lifecycle_rx,
            drain_task_running: Arc::new(AtomicBool::new(false)),
        };
        state
            .mcp_sessions
            .write()
            .await
            .insert(session_id.clone(), mcp_state);
        Some(adapter_dispatcher)
    };
    #[cfg(not(feature = "mcp"))]
    let mcp_external_tools: Option<Arc<dyn meerkat_core::AgentToolDispatcher>> = None;

    // Install unpublished cleanup: archive + mcp cleanup + comms drain abort + unregister.
    if let Some(ctx) = req_ctx.as_ref() {
        let cleanup_state = state.clone();
        let cleanup_session_id = session_id.clone();
        ctx.set_unpublished_cleanup(request_action(move || {
            let state = cleanup_state.clone();
            let sid = cleanup_session_id.clone();
            async move {
                let _ = state.session_service.archive(&sid).await;
                #[cfg(feature = "mcp")]
                cleanup_mcp_session(&state, &sid).await;
                #[cfg(feature = "comms")]
                state.runtime_adapter.abort_comms_drain(&sid).await;
                state.runtime_adapter.unregister_session(&sid).await;
            }
        }));

        // Install cancel action: interrupt the session. If a cancel already
        // landed before this install, install_cancel_action fires the newly
        // installed action immediately and reports the observed phase.
        let cancel_svc = state.session_service.clone();
        let cancel_sid = session_id.clone();
        let phase = ctx
            .install_cancel_action_or_cancelled(request_action(move || {
                let svc = cancel_svc.clone();
                let sid = cancel_sid.clone();
                async move {
                    let _ = svc.interrupt(&sid).await;
                }
            }))
            .await;

        if phase == meerkat::surface::CancelActionInstallOutcome::AlreadyCancelled {
            drop(caller_event_tx);
            drain_event_forwarder(&session_id, forward_task).await;
            return RequestTerminal::RespondWithoutPublish(Err(ApiError::RequestCancelled {
                details: None,
            }));
        }
    }

    let current_generation = state.config_runtime.get().await.ok().map(|s| s.generation);
    let initial_identity =
        match resolve_validation_identity(&state.config_runtime, &model, req.provider).await {
            Ok(identity) => identity,
            Err(err) => {
                drop(caller_event_tx);
                drain_event_forwarder(&session_id, forward_task).await;
                return RequestTerminal::RespondWithoutPublish(Err(ApiError::BadRequest(err)));
            }
        };
    let mut build = SessionBuildOptions {
        provider: req.provider,
        self_hosted_server_id: initial_identity.self_hosted_server_id.clone(),
        output_schema: req.output_schema,
        structured_output_retries: req
            .structured_output_retries
            .unwrap_or(default_structured_output_retries()),
        hooks_override: req.hooks_override.unwrap_or_default(),
        comms_name: req.comms_name.clone(),
        peer_meta: req.peer_meta.clone(),
        resume_session: Some(pre_session),
        budget_limits: req.budget_limits,
        provider_params: req.provider_params.clone(),
        external_tools: mcp_external_tools,
        recoverable_tool_defs: None,
        llm_client_override: state
            .llm_client_override
            .clone()
            .map(encode_llm_client_override_for_service),
        override_builtins: ToolCategoryOverride::from_override(req.enable_builtins),
        override_shell: ToolCategoryOverride::from_override(req.enable_shell),
        override_schedule: ToolCategoryOverride::Inherit,
        override_memory: ToolCategoryOverride::from_override(req.enable_memory),
        override_mob: ToolCategoryOverride::Inherit,
        schedule_tools: None,
        mob_tool_authority_context: None,
        preload_skills: req.preload_skills.clone(),
        realm_id: Some(state.realm.to_string()),
        instance_id: state.instance_id.clone(),
        backend: Some(state.backend.clone()),
        config_generation: current_generation,
        connection_ref: None,
        keep_alive,
        checkpointer: None,
        silent_comms_intents: Vec::new(),
        max_inline_peer_notifications: None,
        app_context: req.app_context,
        additional_instructions: req.additional_instructions,
        shell_env: req.shell_env,
        resume_override_mask: ResumeOverrideMask {
            provider: req.provider.is_some(),
            max_tokens: req.max_tokens.is_some(),
            structured_output_retries: req.structured_output_retries.is_some(),
            provider_params: req.provider_params.is_some(),
            preload_skills: req.preload_skills.is_some(),
            keep_alive: keep_alive_override.is_some(),
            comms_name: req.comms_name.is_some(),
            peer_meta: req.peer_meta.is_some(),
            ..Default::default()
        },
        call_timeout_override: Default::default(),
        blob_store_override: None,
        mob_tools: None,
        runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
    };
    build.apply_generated_create_only_mob_operator_access(ToolCategoryOverride::from_override(
        req.enable_mob,
    ));
    let create_provider = build.provider;

    let svc_req = SvcCreateSessionRequest {
        model: model.to_string(),
        prompt: req.prompt.clone(),
        render_metadata: None,
        system_prompt: req.system_prompt,
        max_tokens: Some(max_tokens),
        event_tx: Some(caller_event_tx.clone()),

        skill_references: skill_references.clone(),
        initial_turn: InitialTurnPolicy::Defer,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
        build: Some(build),
        labels: req.labels,
    };

    let validation_identity =
        match resolve_validation_identity(&state.config_runtime, &svc_req.model, create_provider)
            .await
        {
            Ok(identity) => identity,
            Err(err) => {
                return RequestTerminal::RespondWithoutPublish(Err(ApiError::BadRequest(err)));
            }
        };
    if let Err(err) =
        validate_prompt_video_input(&state.config_runtime, &svc_req.prompt, &validation_identity)
            .await
    {
        return RequestTerminal::RespondWithoutPublish(Err(ApiError::BadRequest(err)));
    }

    let adapter = state.runtime_adapter.clone();

    // Create session with Defer, then route through runtime
    let create_result = match state.session_service.create_session(svc_req).await {
        Ok(result) => result,
        Err(err) => {
            #[cfg(feature = "mcp")]
            cleanup_mcp_session(state, &session_id).await;
            state.runtime_adapter.unregister_session(&session_id).await;
            drop(caller_event_tx);
            drain_event_forwarder(&session_id, forward_task).await;
            let api_err = match err {
                SessionError::NotFound { .. } => ApiError::NotFound(err.to_string()),
                SessionError::Busy { .. } => ApiError::BadRequest(err.to_string()),
                SessionError::Agent(meerkat_core::error::AgentError::ConfigError(_)) => {
                    ApiError::BadRequest(err.to_string())
                }
                _ => ApiError::Agent(err.to_string()),
            };
            return RequestTerminal::RespondWithoutPublish(Err(api_err));
        }
    };

    ensure_rest_session_runtime_executor(state, &create_result.session_id).await;

    // Update peer-ingress context so live sessions always get attached ingress
    // and idle keep_alive sessions retain a persistent host drain.
    #[cfg(feature = "comms")]
    {
        let comms_rt = state.session_service.comms_runtime(&session_id).await;
        adapter
            .update_peer_ingress_context(&session_id, keep_alive, comms_rt)
            .await;
    }

    // Create input and route through runtime
    let input = meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::from_content_input(
        req.prompt,
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                keep_alive: resolve_turn_keep_alive_policy(keep_alive_override),
                skill_references,
                flow_tool_overlay: None,
                additional_instructions: None,
                ..Default::default()
            },
        ),
    ));

    // Final cancel recheck before submitting input — interrupt() is a no-op
    // when no turn is running, so cancel between the last recheck and here
    // would be lost without this gate.
    if let Some(ctx) = req_ctx.as_ref()
        && ctx.cancel_already_requested()
    {
        drop(caller_event_tx);
        drain_event_forwarder(&session_id, forward_task).await;
        return RequestTerminal::RespondWithoutPublish(Err(ApiError::RequestCancelled {
            details: None,
        }));
    }

    let (outcome, handle) = match adapter
        .accept_input_with_completion(&create_result.session_id, input)
        .await
    {
        Ok(pair) => pair,
        Err(err) => {
            drop(caller_event_tx);
            drain_event_forwarder(&session_id, forward_task).await;
            return RequestTerminal::Publish(Err(ApiError::InternalWithData {
                message: err.to_string(),
                code: "SESSION_CREATED_WITH_TURN_FAILURE".to_string(),
                details: json!({
                    "session_id": session_id.to_string(),
                    "session_ref": format_session_ref(&state.realm, &session_id),
                    "session_created": true,
                    "resumable": true,
                }),
            }));
        }
    };

    let result = match handle {
        Some(handle) => {
            completion_outcome_to_api_result(handle.wait().await, &session_id, &state.realm, true)
        }
        None => {
            let existing_id = match &outcome {
                meerkat_runtime::AcceptOutcome::Deduplicated { existing_id, .. } => {
                    existing_id.to_string()
                }
                _ => String::new(),
            };
            Err(ApiError::DuplicateInput { existing_id })
        }
    };

    // Drop the sender so the forwarder sees channel closure and can drain.
    drop(caller_event_tx);
    drain_event_forwarder(&session_id, forward_task).await;

    match result {
        Ok(run_result) => {
            RequestTerminal::Publish(Ok(Json(run_result_to_response(run_result, &state.realm))))
        }
        Err(err) => {
            // SESSION_CREATED_WITH_TURN_FAILURE: session exists and is preserved
            // for resumption. Do NOT tear down MCP or comms sidecars — they belong
            // to the live session.
            let message = match err {
                ApiError::Internal(msg) => msg,
                other => return RequestTerminal::Publish(Err(other)),
            };
            RequestTerminal::Publish(Err(ApiError::InternalWithData {
                message,
                code: "SESSION_CREATED_WITH_TURN_FAILURE".to_string(),
                details: json!({
                    "session_id": session_id.to_string(),
                    "session_ref": format_session_ref(&state.realm, &session_id),
                    "session_created": true,
                    "resumable": true,
                }),
            }))
        }
    }
}

/// Parse repeatable `label` query params into a `BTreeMap`.
///
/// Each param is expected as `key=value`. Returns an error for params without `=`.
fn parse_label_filters(
    raw: Option<Vec<String>>,
) -> Result<Option<BTreeMap<String, String>>, String> {
    let labels = match raw {
        Some(l) if !l.is_empty() => l,
        _ => return Ok(None),
    };
    let mut map = BTreeMap::new();
    for s in labels {
        let (k, v) = s
            .split_once('=')
            .ok_or_else(|| format!("malformed label filter, expected key=value: {s}"))?;
        map.insert(k.to_string(), v.to_string());
    }
    Ok(if map.is_empty() { None } else { Some(map) })
}

async fn create_schedule(
    State(state): State<AppState>,
    Json(request): Json<meerkat::CreateScheduleRequest>,
) -> Result<Json<meerkat::Schedule>, ApiError> {
    state
        .ensure_schedule_host_started()
        .await
        .map_err(schedule_error_to_api)?;
    state
        .schedule_service
        .create(request)
        .await
        .map(Json)
        .map_err(schedule_error_to_api)
}

async fn schedule_tools() -> Json<Value> {
    Json(json!({ "tools": schedule_tools_list() }))
}

#[derive(Debug, Deserialize)]
struct ScheduleToolCallRequest {
    name: String,
    #[serde(default)]
    arguments: Value,
}

async fn schedule_call(
    State(state): State<AppState>,
    Json(req): Json<ScheduleToolCallRequest>,
) -> Result<Json<Value>, ApiError> {
    state
        .ensure_schedule_host_started()
        .await
        .map_err(schedule_error_to_api)?;
    handle_schedule_tools_call(&state.schedule_service, &req.name, &req.arguments)
        .await
        .map(Json)
        .map_err(schedule_tool_error_to_api)
}

async fn list_schedules(
    State(state): State<AppState>,
) -> Result<Json<ScheduleListResponse>, ApiError> {
    state
        .schedule_service
        .list()
        .await
        .map(|schedules| Json(ScheduleListResponse { schedules }))
        .map_err(schedule_error_to_api)
}

async fn get_schedule(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<meerkat::Schedule>, ApiError> {
    let schedule_id = resolve_schedule_id(&id)?;
    state
        .schedule_service
        .get(&schedule_id)
        .await
        .map(Json)
        .map_err(schedule_error_to_api)
}

async fn update_schedule(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<meerkat::UpdateScheduleRequest>,
) -> Result<Json<meerkat::Schedule>, ApiError> {
    let schedule_id = resolve_schedule_id(&id)?;
    state
        .ensure_schedule_host_started()
        .await
        .map_err(schedule_error_to_api)?;
    state
        .schedule_service
        .update(&schedule_id, request)
        .await
        .map(Json)
        .map_err(schedule_error_to_api)
}

async fn pause_schedule(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<meerkat::Schedule>, ApiError> {
    let schedule_id = resolve_schedule_id(&id)?;
    state
        .ensure_schedule_host_started()
        .await
        .map_err(schedule_error_to_api)?;
    state
        .schedule_service
        .pause(&schedule_id)
        .await
        .map(Json)
        .map_err(schedule_error_to_api)
}

async fn resume_schedule(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<meerkat::Schedule>, ApiError> {
    let schedule_id = resolve_schedule_id(&id)?;
    state
        .ensure_schedule_host_started()
        .await
        .map_err(schedule_error_to_api)?;
    state
        .schedule_service
        .resume(&schedule_id)
        .await
        .map(Json)
        .map_err(schedule_error_to_api)
}

async fn delete_schedule(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<meerkat::Schedule>, ApiError> {
    let schedule_id = resolve_schedule_id(&id)?;
    state
        .schedule_service
        .delete(&schedule_id)
        .await
        .map(Json)
        .map_err(schedule_error_to_api)
}

async fn list_schedule_occurrences(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ScheduleOccurrencesResponse>, ApiError> {
    let schedule_id = resolve_schedule_id(&id)?;
    state
        .schedule_service
        .list_occurrences(&schedule_id)
        .await
        .map(|occurrences| Json(ScheduleOccurrencesResponse { occurrences }))
        .map_err(schedule_error_to_api)
}

/// List sessions in the active realm.
async fn list_sessions(
    State(state): State<AppState>,
    Query(query): Query<ListSessionsQuery>,
) -> Result<Json<Value>, ApiError> {
    let label_filters = parse_label_filters(query.label).map_err(ApiError::BadRequest)?;

    let sessions = state
        .session_service
        .list(meerkat_core::service::SessionQuery {
            limit: query.limit,
            offset: query.offset,
            labels: label_filters,
        })
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to list sessions: {e}")))?;

    let wire_sessions: Vec<meerkat_contracts::WireSessionSummary> = sessions
        .into_iter()
        .map(|s| {
            let session_ref = format_session_ref(&state.realm, &s.session_id);
            let mut wire = meerkat_contracts::WireSessionSummary::from(s);
            wire.session_ref = Some(session_ref);
            wire
        })
        .collect();

    Ok(Json(json!({ "sessions": wire_sessions })))
}

/// Get session details
async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionDetailsResponse>, ApiError> {
    let session_id = resolve_session_id_for_state(&id, &state)?;

    let view = state
        .session_service
        .read(&session_id)
        .await
        .map_err(|e| match e {
            SessionError::NotFound { .. } => ApiError::NotFound(format!("Session not found: {id}")),
            _ => ApiError::Internal(format!("{e}")),
        })?;

    let created_at: DateTime<Utc> = view.state.created_at.into();
    let updated_at: DateTime<Utc> = view.state.updated_at.into();

    Ok(Json(SessionDetailsResponse {
        session_id: view.state.session_id.to_string(),
        session_ref: format_session_ref(&state.realm, &view.state.session_id),
        created_at: created_at.to_rfc3339(),
        updated_at: updated_at.to_rfc3339(),
        message_count: view.state.message_count,
        total_tokens: view.billing.total_tokens,
        labels: view.state.labels,
    }))
}

/// Get full session history.
async fn get_session_history(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<SessionHistoryQuery>,
) -> Result<Json<meerkat_contracts::WireSessionHistory>, ApiError> {
    let session_id = resolve_session_id_for_state(&id, &state)?;
    let history = state
        .session_service
        .read_history(
            &session_id,
            meerkat_core::service::SessionHistoryQuery {
                offset: query.offset.unwrap_or(0),
                limit: query.limit,
            },
        )
        .await
        .map_err(|e| match e {
            SessionError::NotFound { .. } => ApiError::NotFound(format!("Session not found: {id}")),
            SessionError::PersistenceDisabled => {
                ApiError::BadRequest(format!("Session history is unavailable for session: {id}"))
            }
            _ => ApiError::Internal(format!("{e}")),
        })?;

    let mut wire: meerkat_contracts::WireSessionHistory = history.into();
    wire.session_ref = Some(format_session_ref(&state.realm, &session_id));
    Ok(Json(wire))
}

/// Interrupt an in-flight turn on a session.
async fn interrupt_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let session_id = resolve_session_id_for_state(&id, &state)?;
    match state.session_service.interrupt(&session_id).await {
        Ok(()) | Err(SessionError::NotRunning { .. }) => Ok(Json(json!({
            "session_id": session_id.to_string(),
            "interrupted": true
        }))),
        Err(SessionError::NotFound { .. }) => {
            Err(ApiError::NotFound(format!("Session not found: {id}")))
        }
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to interrupt session: {e}"
        ))),
    }
}

/// Archive (remove) a session.
async fn archive_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let session_id = resolve_session_id_for_state(&id, &state)?;
    match state.session_service.archive(&session_id).await {
        Ok(()) => {
            #[cfg(feature = "mob")]
            let _ = state
                .mob_state
                .destroy_bridge_session_mobs(&session_id.to_string())
                .await;
            #[cfg(feature = "mcp")]
            cleanup_mcp_session(&state, &session_id).await;
            #[cfg(feature = "comms")]
            state.runtime_adapter.abort_comms_drain(&session_id).await;
            state.runtime_adapter.unregister_session(&session_id).await;
            Ok(Json(json!({
                "session_id": session_id.to_string(),
                "archived": true
            })))
        }
        Err(SessionError::NotFound { .. }) => {
            Err(ApiError::NotFound(format!("Session not found: {id}")))
        }
        Err(e) => Err(ApiError::Internal(format!(
            "Failed to archive session: {e}"
        ))),
    }
}

/// Cancel a tracked in-flight request.
async fn cancel_request(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    use meerkat::surface::CancelOutcome;
    match state.request_executor.cancel_request(&request_id).await {
        CancelOutcome::Cancelled | CancelOutcome::AlreadyCancelled => Ok(Json(json!({
            "request_id": request_id,
            "cancelled": true
        }))),
        CancelOutcome::AlreadyPublished | CancelOutcome::AlreadyCompleted => {
            // Committed work cannot be revoked; caller already observed (or
            // will observe) the terminal response on the original request.
            Ok(Json(json!({
                "request_id": request_id,
                "cancelled": false,
                "reason": "already_terminal"
            })))
        }
        CancelOutcome::NotFound => Err(ApiError::NotFound(format!(
            "request not found: {request_id}"
        ))),
    }
}

fn system_context_error_to_api(err: SessionControlError) -> ApiError {
    match err {
        SessionControlError::Session(SessionError::NotFound { .. }) => {
            ApiError::NotFound("Session not found".to_string())
        }
        SessionControlError::Session(other) => ApiError::Internal(other.to_string()),
        SessionControlError::InvalidRequest { message } => ApiError::BadRequest(message),
        SessionControlError::Conflict { key, .. } => ApiError::Conflict(format!(
            "system-context idempotency conflict for key '{key}'"
        )),
    }
}

/// Append runtime system context to a session.
async fn append_system_context(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<AppendSystemContextRequest>,
) -> Result<Json<Value>, ApiError> {
    let session_id = resolve_session_id_for_state(&id, &state)?;
    let svc_req = SvcAppendSystemContextRequest {
        text: req.text,
        source: req.source,
        idempotency_key: req.idempotency_key,
    };
    let result = state
        .session_service
        .append_system_context(&session_id, svc_req)
        .await
        .map_err(system_context_error_to_api)?;
    Ok(Json(json!({
        "session_id": session_id.to_string(),
        "status": result.status,
    })))
}

/// Continue an existing session. Extracts the optional request-lifecycle
/// context from `X-Meerkat-Request-Id`, runs the typed inner body, and routes
/// the `RequestTerminal` outcome through the canonical surface request
/// executor seam.
async fn continue_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ContinueSessionRequest>,
) -> Result<Json<SessionResponse>, ApiError> {
    let req_ctx = extract_request_context(&headers, &state.request_executor)?;
    let executor = state.request_executor.clone();
    let outcome = Box::pin(continue_session_inner(&state, &id, req, req_ctx.clone())).await;
    with_request_lifecycle(&executor, req_ctx, outcome).await
}

/// Continue an existing session (typed-terminal inner body).
async fn continue_session_inner(
    state: &AppState,
    id: &str,
    req: ContinueSessionRequest,
    req_ctx: Option<RequestContext>,
) -> RequestTerminal<Result<Json<SessionResponse>, ApiError>> {
    if let Err(e) = validate_public_peer_meta(req.peer_meta.as_ref()) {
        return RequestTerminal::RespondWithoutPublish(Err(e));
    }
    let path_session_id = match resolve_session_id_for_state(id, state) {
        Ok(v) => v,
        Err(e) => return RequestTerminal::RespondWithoutPublish(Err(e)),
    };
    let body_session_id = match resolve_session_id_for_state(&req.session_id, state) {
        Ok(v) => v,
        Err(e) => return RequestTerminal::RespondWithoutPublish(Err(e)),
    };
    if body_session_id != path_session_id {
        return RequestTerminal::RespondWithoutPublish(Err(ApiError::BadRequest(format!(
            "Session ID mismatch: path={} body={}",
            id, req.session_id
        ))));
    }
    let session_id = body_session_id;

    let keep_alive_override = match resolve_keep_alive(req.keep_alive) {
        Ok(v) => v,
        Err(e) => return RequestTerminal::RespondWithoutPublish(Err(e)),
    };
    let skill_references = match canonical_skill_keys_for_state(state, req.skill_refs.clone()).await
    {
        Ok(v) => v,
        Err(e) => return RequestTerminal::RespondWithoutPublish(Err(e)),
    };

    // Early cancel check — before any stateful work.
    if let Some(ctx) = req_ctx.as_ref()
        && ctx.cancel_already_requested()
    {
        return RequestTerminal::RespondWithoutPublish(Err(ApiError::RequestCancelled {
            details: None,
        }));
    }

    // Set up event forwarding: caller channel -> broadcast
    let (caller_event_tx, caller_event_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(100);
    let forward_task = spawn_event_forwarder(
        caller_event_rx,
        state.event_tx.clone(),
        session_id.clone(),
        req.verbose,
    );

    let loaded_session = match state
        .session_service
        .load_authoritative_session(&session_id)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            drop(caller_event_tx);
            drain_event_forwarder(&session_id, forward_task).await;
            return RequestTerminal::RespondWithoutPublish(Err(ApiError::Internal(format!(
                "Failed to load session: {e}"
            ))));
        }
    };
    let stored_metadata = loaded_session
        .as_ref()
        .and_then(meerkat::Session::session_metadata);
    // Continue: None → inherit from persisted session metadata.
    let keep_alive = match keep_alive_override {
        Some(val) => val,
        None => stored_metadata.as_ref().is_some_and(|m| m.keep_alive),
    };
    let effective_comms_name = req.comms_name.clone().or_else(|| {
        stored_metadata
            .as_ref()
            .and_then(|meta| meta.comms_name.clone())
    });
    if keep_alive
        && effective_comms_name
            .as_ref()
            .is_none_or(|name| name.trim().is_empty())
    {
        drop(caller_event_tx);
        drain_event_forwarder(&session_id, forward_task).await;
        return RequestTerminal::RespondWithoutPublish(Err(ApiError::BadRequest(
            "keep_alive requires comms_name".to_string(),
        )));
    }

    // Apply staged MCP operations at the turn boundary.
    // MCP boundary appends text-only system notices; extract a String for it,
    // then fold any additions back into the final ContentInput.
    #[allow(unused_mut)]
    let mut turn_prompt = req.prompt.clone();
    #[cfg(feature = "mcp")]
    {
        let mut mcp_text = String::new();
        match apply_mcp_boundary(state, &session_id, &caller_event_tx, &mut mcp_text).await {
            Ok(()) => {}
            Err(e) => {
                drop(caller_event_tx);
                drain_event_forwarder(&session_id, forward_task).await;
                return RequestTerminal::RespondWithoutPublish(Err(e));
            }
        }
        // If the MCP boundary appended notices, prepend as a text block
        // to preserve any multimodal content in the original prompt.
        if !mcp_text.is_empty() {
            let mut blocks = turn_prompt.into_blocks();
            blocks.insert(
                0,
                meerkat_core::types::ContentBlock::Text { text: mcp_text },
            );
            turn_prompt = ContentInput::Blocks(blocks);
        }
    }

    let adapter = state.runtime_adapter.clone();
    let final_result = if rest_continue_requires_rebuild(&req) {
        let session = match loaded_session {
            Some(s) => s,
            None => {
                drop(caller_event_tx);
                drain_event_forwarder(&session_id, forward_task).await;
                return RequestTerminal::RespondWithoutPublish(Err(ApiError::NotFound(format!(
                    "Session not found: {session_id}"
                ))));
            }
        };
        let bindings = match state
            .runtime_adapter
            .prepare_bindings(session_id.clone())
            .await
        {
            Ok(v) => v,
            Err(e) => {
                let message = format!("failed to prepare runtime bindings: {e}");
                drop(caller_event_tx);
                drain_event_forwarder(&session_id, forward_task).await;
                return RequestTerminal::RespondWithoutPublish(Err(ApiError::Internal(message)));
            }
        };
        let llm_binding = meerkat_core::session_recovery::resolve_resume_llm_binding(
            session
                .session_metadata()
                .map(|meta| meta.provider)
                .unwrap_or(Provider::Other),
            session
                .session_metadata()
                .and_then(|meta| meta.self_hosted_server_id),
            req.model.as_deref(),
            req.provider,
        );
        let mut build = SessionBuildOptions {
            provider: llm_binding.provider,
            self_hosted_server_id: llm_binding.self_hosted_server_id,
            output_schema: req.output_schema,
            structured_output_retries: req
                .structured_output_retries
                .unwrap_or(default_structured_output_retries()),
            hooks_override: req.hooks_override.clone().unwrap_or_default(),
            comms_name: req.comms_name.clone(),
            peer_meta: req.peer_meta.clone(),
            resume_session: Some(session),
            budget_limits: None,
            provider_params: None,
            external_tools: None,
            recoverable_tool_defs: None,
            llm_client_override: state
                .llm_client_override
                .clone()
                .map(encode_llm_client_override_for_service),
            override_builtins: ToolCategoryOverride::Inherit,
            override_shell: ToolCategoryOverride::Inherit,
            override_memory: ToolCategoryOverride::Inherit,
            override_schedule: ToolCategoryOverride::Inherit,
            override_mob: ToolCategoryOverride::Inherit,
            schedule_tools: None,
            mob_tool_authority_context: None,
            preload_skills: None,
            realm_id: Some(state.realm.to_string()),
            instance_id: state.instance_id.clone(),
            backend: Some(state.backend.clone()),
            config_generation: state.config_runtime.get().await.ok().map(|s| s.generation),
            connection_ref: None,
            keep_alive,
            checkpointer: None,
            silent_comms_intents: Vec::new(),
            max_inline_peer_notifications: None,
            app_context: None,
            additional_instructions: None,
            shell_env: None,
            resume_override_mask: ResumeOverrideMask {
                model: req.model.is_some(),
                provider: llm_binding.provider_overridden,
                max_tokens: req.max_tokens.is_some(),
                structured_output_retries: req.structured_output_retries.is_some(),
                keep_alive: keep_alive_override.is_some(),
                comms_name: req.comms_name.is_some(),
                peer_meta: req.peer_meta.is_some(),
                ..Default::default()
            },
            call_timeout_override: Default::default(),
            blob_store_override: None,
            mob_tools: None,
            runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
        };
        build.apply_generated_create_only_mob_operator_access(ToolCategoryOverride::Inherit);
        let create_req = SvcCreateSessionRequest {
            model: req
                .model
                .clone()
                .unwrap_or_else(|| state.default_model.clone())
                .to_string(),
            prompt: turn_prompt.clone(),
            render_metadata: None,
            system_prompt: req.system_prompt.clone(),
            max_tokens: req.max_tokens.or(Some(state.max_tokens)),
            event_tx: Some(caller_event_tx.clone()),
            skill_references: skill_references.clone(),
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: None,
        };
        let create_provider = create_req.build.as_ref().and_then(|build| build.provider);
        let validation_identity = match resolve_validation_identity(
            &state.config_runtime,
            &create_req.model,
            create_provider,
        )
        .await
        {
            Ok(identity) => identity,
            Err(err) => {
                drop(caller_event_tx);
                drain_event_forwarder(&session_id, forward_task).await;
                return RequestTerminal::RespondWithoutPublish(Err(ApiError::BadRequest(err)));
            }
        };
        if let Err(err) = validate_prompt_video_input(
            &state.config_runtime,
            &create_req.prompt,
            &validation_identity,
        )
        .await
        {
            drop(caller_event_tx);
            drain_event_forwarder(&session_id, forward_task).await;
            return RequestTerminal::RespondWithoutPublish(Err(ApiError::BadRequest(err)));
        }
        let create_result = match state.session_service.create_session(create_req).await {
            Ok(v) => v,
            Err(e) => {
                drop(caller_event_tx);
                drain_event_forwarder(&session_id, forward_task).await;
                return RequestTerminal::RespondWithoutPublish(Err(ApiError::Internal(format!(
                    "Failed to rebuild session: {e}"
                ))));
            }
        };

        // Rebuilt session now exists — install cleanup to archive it if cancel
        // fires before the turn starts, and interrupt as the running cancel action.
        if let Some(ctx) = req_ctx.as_ref() {
            let cleanup_svc = state.session_service.clone();
            let cleanup_adapter = state.runtime_adapter.clone();
            let cleanup_sid = session_id.clone();
            ctx.set_unpublished_cleanup(request_action(move || {
                let svc = cleanup_svc.clone();
                let adapter = cleanup_adapter.clone();
                let sid = cleanup_sid.clone();
                async move {
                    let _ = svc.archive(&sid).await;
                    adapter.unregister_session(&sid).await;
                }
            }));

            let cancel_svc = state.session_service.clone();
            let cancel_sid = session_id.clone();
            let phase = ctx
                .install_cancel_action_or_cancelled(request_action(move || {
                    let svc = cancel_svc.clone();
                    let sid = cancel_sid.clone();
                    async move {
                        let _ = svc.interrupt(&sid).await;
                    }
                }))
                .await;

            // If cancel raced with install, install_cancel_action already ran
            // the newly installed action; bail out with a cancelled terminal.
            if phase == meerkat::surface::CancelActionInstallOutcome::AlreadyCancelled {
                drop(caller_event_tx);
                drain_event_forwarder(&session_id, forward_task).await;
                return RequestTerminal::RespondWithoutPublish(Err(ApiError::RequestCancelled {
                    details: None,
                }));
            }
        }

        #[cfg(feature = "comms")]
        {
            let comms_rt = state.session_service.comms_runtime(&session_id).await;
            adapter
                .update_peer_ingress_context(&session_id, keep_alive, comms_rt)
                .await;
        }
        ensure_rest_session_runtime_executor(state, &create_result.session_id).await;
        let input =
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::from_content_input(
                turn_prompt.clone(),
                Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        keep_alive: resolve_turn_keep_alive_policy(keep_alive_override),
                        skill_references: skill_references.clone(),
                        flow_tool_overlay: req.flow_tool_overlay.clone(),
                        additional_instructions: resolve_turn_additional_instructions(
                            req.additional_instructions.clone(),
                        ),
                        ..Default::default()
                    },
                ),
            ));
        // Final cancel recheck before submitting input.
        if let Some(ctx) = req_ctx.as_ref()
            && ctx.cancel_already_requested()
        {
            drop(caller_event_tx);
            drain_event_forwarder(&session_id, forward_task).await;
            return RequestTerminal::RespondWithoutPublish(Err(ApiError::RequestCancelled {
                details: None,
            }));
        }
        let (_outcome, handle) = match adapter
            .accept_input_with_completion(&create_result.session_id, input)
            .await
        {
            Ok(pair) => pair,
            Err(err) => {
                drop(caller_event_tx);
                drain_event_forwarder(&session_id, forward_task).await;
                return RequestTerminal::RespondWithoutPublish(Err(ApiError::Internal(
                    err.to_string(),
                )));
            }
        };
        match handle {
            Some(handle) => completion_outcome_to_api_result(
                handle.wait().await,
                &session_id,
                &state.realm,
                false,
            ),
            None => Err(ApiError::DuplicateInput {
                existing_id: String::new(),
            }),
        }
    } else {
        ensure_rest_session_runtime_executor(state, &session_id).await;
        #[cfg(feature = "comms")]
        {
            let comms_rt = state.session_service.comms_runtime(&session_id).await;
            if keep_alive && comms_rt.is_none() {
                drop(caller_event_tx);
                drain_event_forwarder(&session_id, forward_task).await;
                return RequestTerminal::RespondWithoutPublish(Err(ApiError::BadRequest(
                    "keep_alive requires a session created with comms_name".to_string(),
                )));
            }
            if keep_alive_override.is_some()
                && let Err(e) = state
                    .session_service
                    .update_session_keep_alive(&session_id, keep_alive)
                    .await
            {
                drop(caller_event_tx);
                drain_event_forwarder(&session_id, forward_task).await;
                return RequestTerminal::RespondWithoutPublish(Err(ApiError::Internal(format!(
                    "failed to persist keep_alive: {e}"
                ))));
            }
            adapter
                .update_peer_ingress_context(&session_id, keep_alive, comms_rt)
                .await;
        }

        let fallback_identity =
            match resolve_validation_identity(&state.config_runtime, &state.default_model, None)
                .await
            {
                Ok(identity) => identity,
                Err(err) => {
                    drop(caller_event_tx);
                    drain_event_forwarder(&session_id, forward_task).await;
                    return RequestTerminal::RespondWithoutPublish(Err(ApiError::BadRequest(err)));
                }
            };
        let current_identity = stored_metadata
            .as_ref()
            .map(meerkat::SessionMetadata::llm_identity)
            .unwrap_or(fallback_identity);
        if let Err(err) =
            validate_prompt_video_input(&state.config_runtime, &turn_prompt, &current_identity)
                .await
        {
            drop(caller_event_tx);
            drain_event_forwarder(&session_id, forward_task).await;
            return RequestTerminal::RespondWithoutPublish(Err(ApiError::BadRequest(err)));
        }

        // Install cancel action: interrupt the session.
        if let Some(ctx) = req_ctx.as_ref() {
            let cancel_svc = state.session_service.clone();
            let cancel_sid = session_id.clone();
            let phase = ctx
                .install_cancel_action_or_cancelled(request_action(move || {
                    let svc = cancel_svc.clone();
                    let sid = cancel_sid.clone();
                    async move {
                        let _ = svc.interrupt(&sid).await;
                    }
                }))
                .await;

            // If cancel raced with install, install_cancel_action already ran
            // the newly installed action; bail out with a cancelled terminal.
            if phase == meerkat::surface::CancelActionInstallOutcome::AlreadyCancelled {
                drop(caller_event_tx);
                drain_event_forwarder(&session_id, forward_task).await;
                return RequestTerminal::RespondWithoutPublish(Err(ApiError::RequestCancelled {
                    details: None,
                }));
            }
        }

        let input =
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::from_content_input(
                turn_prompt.clone(),
                Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        keep_alive: resolve_turn_keep_alive_policy(keep_alive_override),
                        skill_references: skill_references.clone(),
                        flow_tool_overlay: req.flow_tool_overlay.clone(),
                        additional_instructions: resolve_turn_additional_instructions(
                            req.additional_instructions.clone(),
                        ),
                        ..Default::default()
                    },
                ),
            ));
        // Final cancel recheck before submitting input.
        if let Some(ctx) = req_ctx.as_ref()
            && ctx.cancel_already_requested()
        {
            drop(caller_event_tx);
            drain_event_forwarder(&session_id, forward_task).await;
            return RequestTerminal::RespondWithoutPublish(Err(ApiError::RequestCancelled {
                details: None,
            }));
        }
        let (outcome, handle) = match adapter
            .accept_input_with_completion(&session_id, input)
            .await
        {
            Ok(pair) => pair,
            Err(err) => {
                drop(caller_event_tx);
                drain_event_forwarder(&session_id, forward_task).await;
                return RequestTerminal::RespondWithoutPublish(Err(ApiError::Internal(
                    err.to_string(),
                )));
            }
        };

        match handle {
            Some(handle) => completion_outcome_to_api_result(
                handle.wait().await,
                &session_id,
                &state.realm,
                false,
            ),
            None => {
                let existing_id = match &outcome {
                    meerkat_runtime::AcceptOutcome::Deduplicated { existing_id, .. } => {
                        existing_id.to_string()
                    }
                    _ => String::new(),
                };
                Err(ApiError::DuplicateInput { existing_id })
            }
        }
    };

    // Drop the sender so the forwarder sees channel closure and can drain.
    drop(caller_event_tx);
    drain_event_forwarder(&session_id, forward_task).await;

    match final_result {
        Ok(run_result) => {
            RequestTerminal::Publish(Ok(Json(run_result_to_response(run_result, &state.realm))))
        }
        Err(err) => {
            if let ApiError::Internal(message) = &err
                && message.contains("runtime boundary commit failed")
            {
                let _ = state
                    .session_service
                    .discard_live_session(&session_id)
                    .await;
                state.runtime_adapter.unregister_session(&session_id).await;
            }
            // Session exists for continue — this is a Published error.
            RequestTerminal::Publish(Err(err))
        }
    }
}

/// SSE endpoint for streaming session events
async fn session_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let session_id = resolve_session_id_for_state(&id, &state)?;

    // load_persisted() only checks the redb store. A session on its first
    // in-progress turn (not yet persisted) will return 404. Future improvement:
    // try read() first for message_count (works for live sessions), fall back
    // to load_persisted() for inactive sessions.
    let session = state
        .session_service
        .load_authoritative_session(&session_id)
        .await
        .map_err(|e| ApiError::Internal(format!("{e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Session not found: {id}")))?;

    let mut rx = state.event_tx.subscribe();

    // Create a stream that sends agent events as SSE events
    let stream = async_stream::stream! {
        // Emit a session_loaded event for compatibility
        let event = Event::default()
            .event("session_loaded")
            .data(serde_json::to_string(&json!({
                "session_id": session_id.to_string(),
                "message_count": session.messages().len(),
            })).unwrap_or_default());
        yield Ok(event);

        loop {
            match rx.recv().await {
                Ok(payload) => {
                    if payload.session_id != session_id {
                        continue;
                    }

                    let event_type = agent_event_type(&payload.event.payload);
                    let json = serde_json::to_value(&payload.event).unwrap_or_else(|_| {
                        json!({
                            "payload": {"type": "unknown"}
                        })
                    });

                    let event = Event::default()
                        .event(event_type)
                        .data(serde_json::to_string(&json).unwrap_or_default());
                    yield Ok(event);
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }

        let event = Event::default().event("done").data("{}");
        yield Ok(event);
    };

    Ok(Sse::new(stream))
}

// ---------------------------------------------------------------------------
// Live MCP handlers
// ---------------------------------------------------------------------------

/// Apply staged MCP operations at the turn boundary.
///
/// Mirrors RPC's `apply_mcp_boundary()`: drain queued lifecycle actions,
/// call `apply_staged()`, spawn drain task if removals started, emit events.
/// Returns an error if staged operations fail to apply — the caller should
/// fail the turn to match RPC semantics.
#[cfg(feature = "mcp")]
async fn apply_mcp_boundary(
    state: &AppState,
    session_id: &SessionId,
    event_tx: &mpsc::Sender<EventEnvelope<AgentEvent>>,
    prompt: &mut String,
) -> Result<(), ApiError> {
    let (adapter, turn_number, drain_task_running, lifecycle_tx, mut queued_actions) = {
        let mut map = state.mcp_sessions.write().await;
        let mcp_state = match map.get_mut(session_id) {
            Some(s) => s,
            None => return Ok(()),
        };
        mcp_state.turn_counter = mcp_state.turn_counter.saturating_add(1);
        let mut queued = Vec::new();
        while let Ok(action) = mcp_state.lifecycle_rx.try_recv() {
            queued.push(action);
        }
        (
            mcp_state.adapter.clone(),
            mcp_state.turn_counter,
            mcp_state.drain_task_running.clone(),
            mcp_state.lifecycle_tx.clone(),
            queued,
        )
    };

    // Emit events for queued actions from the background drain task.
    if !queued_actions.is_empty() {
        let drained = std::mem::take(&mut queued_actions);
        let source_id = format!("session:{session_id}");
        meerkat::surface::emit_mcp_lifecycle_events(
            event_tx,
            &source_id,
            prompt,
            turn_number,
            drained,
        )
        .await;
    }

    // Apply staged operations — fail the turn on error.
    let result = adapter
        .apply_staged()
        .await
        .map_err(|e| ApiError::Internal(format!("failed to apply staged MCP operations: {e}")))?;

    // Spawn drain task if any removals started.
    if result.delta.lifecycle_actions.iter().any(|action| {
        action.operation == ToolConfigChangeOperation::Remove
            && action.phase == McpLifecyclePhase::Draining
    }) {
        spawn_mcp_drain_task(adapter, drain_task_running, lifecycle_tx);
    }

    queued_actions.extend(result.delta.lifecycle_actions);
    if !queued_actions.is_empty() {
        let source_id = format!("session:{session_id}");
        meerkat::surface::emit_mcp_lifecycle_events(
            event_tx,
            &source_id,
            prompt,
            turn_number,
            queued_actions,
        )
        .await;
    }

    Ok(())
}

/// Spawn a background task that monitors removing MCP servers.
#[cfg(feature = "mcp")]
fn spawn_mcp_drain_task(
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
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("background MCP drain apply failed: {e}");
                    break;
                }
            };
            for action in delta.lifecycle_actions {
                let _ = lifecycle_tx.send(action);
            }
            match adapter.has_removing_servers().await {
                Ok(true) => continue,
                Ok(false) => break,
                Err(e) => {
                    tracing::warn!("background MCP drain state check failed: {e}");
                    break;
                }
            }
        }
        task_running.store(false, Ordering::Release);
    });
}

/// Validate session existence and retrieve its MCP adapter.
///
/// Two-step validation:
/// 1. Check session exists in the service (authoritative).
/// 2. Check session has a live MCP adapter (runtime capability).
#[cfg(feature = "mcp")]
async fn resolve_mcp_adapter(
    state: &AppState,
    session_id: &SessionId,
) -> Result<Arc<McpRouterAdapter>, ApiError> {
    // Step 1: session must exist.
    if state.session_service.read(session_id).await.is_err() {
        return Err(ApiError::NotFound(format!(
            "Session not found: {session_id}"
        )));
    }
    // Step 2: session must have a live MCP adapter.
    let map = state.mcp_sessions.read().await;
    map.get(session_id)
        .map(|s| s.adapter.clone())
        .ok_or_else(|| {
            ApiError::Conflict(
                "Live MCP unavailable for this session. Recovery: create a new session \
                 (live MCP adapters are attached at session creation time; sessions from \
                 before the server started do not have them)."
                    .to_string(),
            )
        })
}

/// Validate that path and body session IDs resolve to the same session.
#[cfg(feature = "mcp")]
fn validate_session_id_consistency(
    path_id: &str,
    body_id: &str,
    state: &AppState,
) -> Result<SessionId, ApiError> {
    let path_sid = resolve_session_id_for_state(path_id, state)?;
    let body_sid = resolve_session_id_for_state(body_id, state)?;
    if path_sid != body_sid {
        return Err(ApiError::BadRequest(format!(
            "Session ID mismatch: path={path_id} body={body_id}"
        )));
    }
    Ok(path_sid)
}

/// `POST /sessions/{id}/mcp/add` — stage a live MCP server addition.
#[cfg(feature = "mcp")]
async fn mcp_add(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<meerkat_contracts::McpAddParams>,
) -> Result<Json<meerkat_contracts::McpLiveOpResponse>, ApiError> {
    let session_id = validate_session_id_consistency(&id, &req.session_id, &state)?;
    if req.server_name.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "server_name cannot be empty".to_string(),
        ));
    }

    let adapter = resolve_mcp_adapter(&state, &session_id).await?;

    // Inject the server name into the config object.
    let mut server_config = req.server_config;
    if let Some(obj) = server_config.as_object_mut() {
        obj.insert(
            "name".to_string(),
            serde_json::Value::String(req.server_name.clone()),
        );
    }

    let config: meerkat_core::McpServerConfig = serde_json::from_value(server_config)
        .map_err(|e| ApiError::BadRequest(format!("invalid server_config: {e}")))?;

    let rollback = if req.persisted {
        let authority = meerkat::surface::mcp_config_mutation_authority(
            state.context_root.clone(),
            state.user_config_root.clone(),
        );
        meerkat::surface::persist_mcp_add_if_requested(true, &authority, config.clone())
            .await
            .map_err(ApiError::Internal)?
    } else {
        None
    };

    if let Err(err) = adapter.stage_add(config).await {
        let rollback_message =
            match meerkat::surface::rollback_mcp_persisted_mutation(rollback).await {
                Ok(()) => String::new(),
                Err(rollback_err) => format!("; persisted rollback failed: {rollback_err}"),
            };
        return Err(ApiError::Internal(format!("{err}{rollback_message}")));
    }

    Ok(Json(meerkat::surface::mcp_live_response(
        req.session_id,
        meerkat_contracts::McpLiveOperation::Add,
        Some(req.server_name),
        rollback.is_some(),
    )))
}

/// `POST /sessions/{id}/mcp/remove` — stage a live MCP server removal.
#[cfg(feature = "mcp")]
async fn mcp_remove(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<meerkat_contracts::McpRemoveParams>,
) -> Result<Json<meerkat_contracts::McpLiveOpResponse>, ApiError> {
    let session_id = validate_session_id_consistency(&id, &req.session_id, &state)?;
    if req.server_name.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "server_name cannot be empty".to_string(),
        ));
    }

    let adapter = resolve_mcp_adapter(&state, &session_id).await?;
    let rollback = if req.persisted {
        let authority = meerkat::surface::mcp_config_mutation_authority(
            state.context_root.clone(),
            state.user_config_root.clone(),
        );
        meerkat::surface::persist_mcp_remove_if_requested(true, &authority, &req.server_name)
            .await
            .map_err(ApiError::Internal)?
    } else {
        None
    };

    if let Err(err) = adapter.stage_remove(req.server_name.clone()).await {
        let rollback_message =
            match meerkat::surface::rollback_mcp_persisted_mutation(rollback).await {
                Ok(()) => String::new(),
                Err(rollback_err) => format!("; persisted rollback failed: {rollback_err}"),
            };
        return Err(ApiError::Internal(format!("{err}{rollback_message}")));
    }

    Ok(Json(meerkat::surface::mcp_live_response(
        req.session_id,
        meerkat_contracts::McpLiveOperation::Remove,
        Some(req.server_name),
        rollback.is_some(),
    )))
}

/// `POST /sessions/{id}/mcp/reload` — stage a live MCP server reload.
#[cfg(feature = "mcp")]
async fn mcp_reload(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<meerkat_contracts::McpReloadParams>,
) -> Result<Json<meerkat_contracts::McpLiveOpResponse>, ApiError> {
    let session_id = validate_session_id_consistency(&id, &req.session_id, &state)?;
    if let Some(name) = req.server_name.as_ref()
        && name.trim().is_empty()
    {
        return Err(ApiError::BadRequest(
            "server_name cannot be empty".to_string(),
        ));
    }

    let adapter = resolve_mcp_adapter(&state, &session_id).await?;
    match req.server_name.as_ref() {
        Some(name) => {
            meerkat::surface::validate_reload_target(&adapter, name)
                .await
                .map_err(ApiError::BadRequest)?;
            adapter
                .stage_reload(McpReloadTarget::ServerName(name.clone()))
                .await
                .map_err(ApiError::Internal)?;
        }
        None => {
            let names = adapter.active_server_names().await;
            for name in names {
                adapter
                    .stage_reload(McpReloadTarget::ServerName(name))
                    .await
                    .map_err(ApiError::Internal)?;
            }
        }
    }

    Ok(Json(meerkat::surface::mcp_live_response(
        req.session_id,
        meerkat_contracts::McpLiveOperation::Reload,
        req.server_name,
        false,
    )))
}

/// Clean up MCP state for a session (failure cleanup, archive, shutdown).
#[cfg(feature = "mcp")]
async fn cleanup_mcp_session(state: &AppState, session_id: &SessionId) {
    if let Some(mcp_state) = state.mcp_sessions.write().await.remove(session_id) {
        mcp_state.adapter.shutdown().await;
    }
}

/// Shut down all MCP adapters (called on server shutdown).
#[cfg(feature = "mcp")]
pub async fn shutdown_all_mcp_sessions(state: &AppState) {
    let sessions: Vec<_> = state.mcp_sessions.write().await.drain().collect();
    for (_, mcp_state) in sessions {
        mcp_state.adapter.shutdown().await;
    }
}

/// API error types
#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    Unauthorized(String),
    NotFound(String),
    Conflict(String),
    DuplicateInput {
        existing_id: String,
    },
    Configuration(String),
    Agent(String),
    Internal(String),
    InternalWithData {
        message: String,
        code: String,
        details: Value,
    },
    RequestCancelled {
        details: Option<Value>,
    },
    DuplicateRequestId {
        request_id: String,
    },
    ServiceUnavailable(String),
    Gone(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message, details) = match self {
            ApiError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                "BAD_REQUEST".to_string(),
                msg,
                None,
            ),
            ApiError::Unauthorized(msg) => (
                StatusCode::UNAUTHORIZED,
                "UNAUTHORIZED".to_string(),
                msg,
                None,
            ),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, "NOT_FOUND".to_string(), msg, None),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, "CONFLICT".to_string(), msg, None),
            ApiError::DuplicateInput { existing_id } => {
                let body = Json(serde_json::json!({
                    "error": "duplicate_input",
                    "code": "DUPLICATE_INPUT",
                    "existing_id": existing_id,
                }));
                return (StatusCode::CONFLICT, body).into_response();
            }
            ApiError::Configuration(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "CONFIGURATION_ERROR".to_string(),
                msg,
                None,
            ),
            ApiError::Agent(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "AGENT_ERROR".to_string(),
                msg,
                None,
            ),
            ApiError::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR".to_string(),
                msg,
                None,
            ),
            ApiError::InternalWithData {
                message,
                code,
                details,
            } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                code,
                message,
                Some(details),
            ),
            ApiError::RequestCancelled { details } => (
                StatusCode::from_u16(499).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                "REQUEST_CANCELLED".to_string(),
                "request cancelled".to_string(),
                details,
            ),
            ApiError::DuplicateRequestId { request_id } => (
                StatusCode::CONFLICT,
                "DUPLICATE_REQUEST_ID".to_string(),
                format!("request ID already in flight: {request_id}"),
                None,
            ),
            ApiError::ServiceUnavailable(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "SERVICE_UNAVAILABLE".to_string(),
                msg,
                None,
            ),
            ApiError::Gone(msg) => (StatusCode::GONE, "GONE".to_string(), msg, None),
        };

        let body = Json(ErrorResponse {
            error: message,
            code,
            details,
        });

        (status, body).into_response()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::{Duration, Utc};
    use futures::stream;
    use meerkat::{OccurrenceFailureClass, OccurrencePhase, ScheduleId};
    use meerkat_client::{LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::{
        MemoryConfigStore, SelfHostedApiStyle, SelfHostedServerConfig, SelfHostedTransport,
        SessionId,
    };
    use serde_json::json;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[test]
    fn rest_peer_response_terminal_body_deserializes_typed_identity_and_correlation() {
        let body: RestPeerResponseTerminalBody = serde_json::from_value(json!({
            "peer_id": "00000000-0000-4000-8000-000000000161",
            "display_name": "analyst",
            "request_id": "00000000-0000-4000-8000-000000000162",
            "status": "completed",
            "result": {"ok": true},
        }))
        .unwrap();

        assert_eq!(
            body.peer_id.to_string(),
            "00000000-0000-4000-8000-000000000161"
        );
        assert_eq!(body.display_name.unwrap().as_str(), "analyst");
        assert_eq!(
            body.request_id.to_string(),
            "00000000-0000-4000-8000-000000000162"
        );
        assert_eq!(
            body.status,
            meerkat_contracts::PeerResponseTerminalStatusWire::Completed
        );
        assert_eq!(body.result["ok"], true);
    }

    #[test]
    fn rest_peer_response_terminal_body_rejects_name_only_origin() {
        let err = serde_json::from_value::<RestPeerResponseTerminalBody>(json!({
            "peer_name": "analyst",
            "request_id": "00000000-0000-4000-8000-000000000162",
            "status": "completed",
            "result": null,
        }))
        .unwrap_err();

        assert!(
            err.to_string().contains("peer_id"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rest_peer_response_terminal_body_rejects_mixed_peer_name_origin() {
        let err = serde_json::from_value::<RestPeerResponseTerminalBody>(json!({
            "peer_id": "00000000-0000-4000-8000-000000000161",
            "peer_name": "analyst",
            "request_id": "00000000-0000-4000-8000-000000000162",
            "status": "completed",
            "result": null,
        }))
        .unwrap_err();

        assert!(
            err.to_string().contains("peer_name"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rest_peer_response_terminal_body_rejects_stringly_request_id() {
        let err = serde_json::from_value::<RestPeerResponseTerminalBody>(json!({
            "peer_id": "00000000-0000-4000-8000-000000000161",
            "request_id": "req-1",
            "status": "completed",
            "result": null,
        }))
        .unwrap_err();

        assert!(
            err.to_string().contains("UUID") || err.to_string().contains("uuid"),
            "unexpected error: {err}"
        );
    }

    async fn spawn_realtime_rpc_stub(
        expected_method: &'static str,
        rpc_result: Result<Value, Value>,
    ) -> (
        String,
        tokio::sync::oneshot::Receiver<Value>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind realtime rpc stub");
        let addr = listener
            .local_addr()
            .expect("realtime rpc stub local addr")
            .to_string();
        let (captured_tx, captured_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept realtime rpc stub");
            let (read_half, mut write_half) = stream.into_split();
            let mut reader = tokio::io::BufReader::new(read_half).lines();
            let mut captured_tx = Some(captured_tx);
            while let Some(line) = reader
                .next_line()
                .await
                .expect("read realtime rpc stub line")
            {
                let value: Value =
                    serde_json::from_str(&line).expect("parse realtime rpc stub request");
                let id = value["id"].clone();
                let method = value["method"]
                    .as_str()
                    .expect("realtime rpc stub request method");
                let response = match method {
                    "initialize" => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "server": "realtime-stub",
                        }
                    }),
                    method if method == expected_method => {
                        if let Some(tx) = captured_tx.take() {
                            let _ = tx.send(value["params"].clone());
                        }
                        match &rpc_result {
                            Ok(result) => json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": result,
                            }),
                            Err(error) => json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": error,
                            }),
                        }
                    }
                    other => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32601,
                            "message": format!("unexpected method: {other}"),
                        }
                    }),
                };
                write_half
                    .write_all(response.to_string().as_bytes())
                    .await
                    .expect("write realtime rpc stub response");
                write_half
                    .write_all(b"\n")
                    .await
                    .expect("terminate realtime rpc stub response");
                write_half
                    .flush()
                    .await
                    .expect("flush realtime rpc stub response");
                if method == expected_method {
                    break;
                }
            }
        });
        (addr, captured_rx, task)
    }

    struct MockLlmClient;

    struct ErrorLlmClient;

    #[async_trait]
    impl LlmClient for MockLlmClient {
        fn stream<'a>(
            &'a self,
            _request: &'a LlmRequest,
        ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: "ok".to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
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

    #[async_trait]
    impl LlmClient for ErrorLlmClient {
        fn stream<'a>(
            &'a self,
            _request: &'a LlmRequest,
        ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
            Box::pin(stream::iter(vec![Err(LlmError::Unknown {
                message: "boom".to_string(),
            })]))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    fn hooks_override_fixture() -> HookRunOverrides {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../test-fixtures/hooks/run_override.json");
        let payload = std::fs::read_to_string(path).expect("hook override fixture must exist");
        serde_json::from_str::<HookRunOverrides>(&payload)
            .expect("hook override fixture must deserialize")
    }

    fn self_hosted_test_config(inline_video: bool) -> Config {
        let mut config = Config::default();
        config.self_hosted.servers.insert(
            "local".to_string(),
            SelfHostedServerConfig {
                transport: SelfHostedTransport::OpenAiCompatible,
                base_url: "http://127.0.0.1:11434".to_string(),
                api_style: SelfHostedApiStyle::ChatCompletions,
                bearer_token: None,
                bearer_token_env: None,
            },
        );
        config.self_hosted.models.insert(
            "gemma-4-e2b".to_string(),
            serde_json::from_value(json!({
                "server": "local",
                "remote_model": "gemma4:e2b",
                "display_name": "Gemma 4 E2B",
                "family": "gemma-4",
                "tier": "supported",
                "context_window": 128000,
                "max_output_tokens": 8192,
                "vision": true,
                "image_tool_results": true,
                "inline_video": inline_video,
                "supports_temperature": true,
                "supports_thinking": true,
                "supports_reasoning": true,
                "call_timeout_secs": 600
            }))
            .expect("self-hosted model config"),
        );
        config
    }

    fn inline_video_prompt() -> ContentInput {
        ContentInput::Blocks(vec![meerkat_core::ContentBlock::Video {
            media_type: "video/mp4".to_string(),
            duration_ms: 1_000,
            data: meerkat_core::VideoData::Inline {
                data: "AAAA".to_string(),
            },
        }])
    }

    #[tokio::test]
    async fn test_app_state_default() {
        let temp = TempDir::new().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        assert!(!state.default_model.is_empty());
        assert!(state.max_tokens > 0);
        // runtime_adapter is always present (non-optional)
    }

    #[tokio::test]
    async fn validate_prompt_video_input_accepts_self_hosted_alias_from_runtime_registry() {
        let temp = TempDir::new().unwrap();
        let store: Arc<dyn meerkat_core::ConfigStore> =
            Arc::new(MemoryConfigStore::new(self_hosted_test_config(true)));
        let config_runtime =
            meerkat_core::ConfigRuntime::new(store, temp.path().join("config_state.json"));

        let identity = resolve_validation_identity(&config_runtime, "gemma-4-e2b", None)
            .await
            .expect("self-hosted alias should resolve");
        assert_eq!(identity.provider, Provider::SelfHosted);

        validate_prompt_video_input(&config_runtime, &inline_video_prompt(), &identity)
            .await
            .expect("self-hosted aliases should validate inline video against the active registry");
    }

    #[tokio::test]
    async fn validation_identity_rejects_explicit_provider_that_contradicts_catalog_owner() {
        let temp = TempDir::new().unwrap();
        let store: Arc<dyn meerkat_core::ConfigStore> =
            Arc::new(MemoryConfigStore::new(Config::default()));
        let config_runtime =
            meerkat_core::ConfigRuntime::new(store, temp.path().join("config_state.json"));

        let err =
            resolve_validation_identity(&config_runtime, "gpt-5.4", Some(Provider::Anthropic))
                .await
                .expect_err("validation identity should fail closed for wrong-provider overrides");

        assert!(
            err.contains("registered for provider 'openai'")
                && err.contains("not provider 'anthropic'")
                && err.contains("gpt-5.4"),
            "error should identify the rejected provider/model pair: {err}"
        );
    }

    #[test]
    fn test_error_response_serialization() {
        let err = ErrorResponse {
            error: "test error".to_string(),
            code: "TEST_ERROR".to_string(),
            details: None,
        };
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("test error"));
        assert!(json.contains("TEST_ERROR"));
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_normalize_rest_comms_send_error_includes_structured_details() {
        let err = normalize_rest_comms_send_error(
            Some("peer-a"),
            &meerkat_core::comms::SendError::PeerOffline,
        );
        match err {
            ApiError::InternalWithData {
                message,
                code,
                details,
            } => {
                assert!(message.starts_with("peer_unreachable:"));
                assert_eq!(code, "peer_unreachable");
                assert_eq!(details.get("peer").and_then(Value::as_str), Some("peer-a"));
                assert_eq!(
                    details.get("reason").and_then(Value::as_str),
                    Some("offline_or_no_ack")
                );
            }
            other => panic!("expected structured internal error, got {other:?}"),
        }
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_normalize_rest_comms_send_error_fallback_is_structured() {
        let err = normalize_rest_comms_send_error(
            Some("peer-a"),
            &meerkat_core::comms::SendError::Internal("boom".to_string()),
        );
        match err {
            ApiError::InternalWithData {
                message,
                code,
                details,
            } => {
                assert_eq!(message, "send_failed: internal: boom");
                assert_eq!(code, "send_failed");
                assert_eq!(
                    details.get("code").and_then(Value::as_str),
                    Some("send_failed")
                );
                assert_eq!(
                    details.get("message").and_then(Value::as_str),
                    Some("internal: boom")
                );
            }
            other => panic!("expected structured internal error, got {other:?}"),
        }
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_comms_send_receipt_json_peer_request_uses_envelope_id_as_request_id() {
        let envelope_id = uuid::Uuid::new_v4();
        let interaction_id = meerkat_core::interaction::InteractionId(uuid::Uuid::new_v4());

        let payload = comms_send_receipt_json(meerkat_core::comms::SendReceipt::PeerRequestSent {
            envelope_id,
            interaction_id,
            stream_reserved: true,
        });

        assert_eq!(
            payload["request_id"],
            serde_json::json!(envelope_id.to_string())
        );
        assert_eq!(
            payload["interaction_id"],
            serde_json::json!(interaction_id.0.to_string())
        );
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_comms_peers_payload_uses_typed_core_wire_contract() {
        let payload = comms_peers_payload(vec![sample_peer_directory_entry()]);

        assert_peer_directory_wire(&payload);
    }

    #[cfg(feature = "comms")]
    fn sample_peer_directory_entry() -> meerkat_core::comms::PeerDirectoryEntry {
        meerkat_core::comms::PeerDirectoryEntry {
            peer_id: meerkat_core::comms::PeerId::new(),
            name: meerkat_core::comms::PeerName::new("agent").unwrap(),
            address: meerkat_core::comms::PeerAddress::new(
                meerkat_core::comms::PeerTransport::Inproc,
                "agent",
            ),
            source: meerkat_core::comms::PeerDirectorySource::Inproc,
            sendable_kinds: vec![
                meerkat_core::comms::PeerSendability::PeerMessage,
                meerkat_core::comms::PeerSendability::PeerRequest,
            ],
            capabilities: meerkat_core::comms::PeerCapabilitySet::default()
                .with_extension("vendor.echo", serde_json::json!({ "enabled": true })),
            reachability: meerkat_core::comms::PeerReachability::Reachable,
            last_unreachable_reason: None,
            meta: meerkat_core::PeerMeta::default(),
        }
    }

    #[cfg(feature = "comms")]
    fn assert_peer_directory_wire(result: &Value) {
        let peer = &result["peers"][0];

        assert_eq!(peer["source"], "inproc");
        assert_eq!(
            peer["sendable_kinds"],
            serde_json::json!(["peer_message", "peer_request"])
        );
        assert_eq!(peer["capabilities"]["version"], 1);
        assert_eq!(
            peer["capabilities"]["extensions"]["vendor.echo"]["enabled"],
            true
        );
    }

    #[tokio::test]
    async fn test_app_state_builtins_disabled_by_default() {
        let temp = TempDir::new().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        assert!(!state.enable_builtins);
        assert!(!state.enable_shell);
    }

    fn missing_target_schedule_tool_args() -> Value {
        json!({
            "name": "missing-target",
            "description": "create a due schedule through the tool surface",
            "trigger": {
                "type": "once",
                "due_at_utc": (Utc::now() - Duration::seconds(1)).to_rfc3339(),
            },
            "target": {
                "target_kind": "session",
                "type": "exact_session",
                "session_id": SessionId::new(),
                "action": {
                    "type": "prompt",
                    "prompt": "scheduled hello"
                }
            },
            "missing_target_policy": "mark_misfired",
            "planning_horizon_days": 1,
            "planning_horizon_occurrences": 1
        })
    }

    async fn wait_for_missing_target_misfire(
        service: &meerkat::ScheduleService,
        schedule_id: &ScheduleId,
    ) -> Option<meerkat::Occurrence> {
        for _ in 0..40 {
            let occurrences = service
                .list_occurrences(schedule_id)
                .await
                .expect("list occurrences");
            if let Some(occurrence) = occurrences.into_iter().find(|occurrence| {
                occurrence.phase == OccurrencePhase::Misfired
                    && occurrence.failure_class == Some(OccurrenceFailureClass::TargetMissing)
            }) {
                return Some(occurrence);
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        None
    }

    #[tokio::test]
    async fn schedule_call_starts_host_and_services_due_schedule() {
        let temp = TempDir::new().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();

        let Json(created) = schedule_call(
            State(state.clone()),
            Json(ScheduleToolCallRequest {
                name: "meerkat_schedule_create".into(),
                arguments: missing_target_schedule_tool_args(),
            }),
        )
        .await
        .expect("schedule tool create should succeed");

        let schedule_id = ScheduleId::parse(
            created["schedule_id"]
                .as_str()
                .expect("schedule_id should be returned"),
        )
        .expect("valid schedule id");

        let occurrence = wait_for_missing_target_misfire(&state.schedule_service, &schedule_id)
            .await
            .expect("schedule/call should start the host and service due work");
        assert_eq!(occurrence.phase, OccurrencePhase::Misfired);
    }

    #[tokio::test]
    async fn test_config_envelope_redacts_paths_by_default() {
        let temp = TempDir::new().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        let Json(envelope) = get_config(State(state)).await.unwrap();
        assert!(envelope.resolved_paths.is_none());
    }

    #[tokio::test]
    async fn test_config_envelope_includes_paths_when_enabled() {
        let temp = TempDir::new().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state.expose_paths = true;
        let Json(envelope) = get_config(State(state)).await.unwrap();
        assert!(envelope.resolved_paths.is_some());
    }

    #[tokio::test]
    async fn test_config_set_and_patch_roundtrip_parity() {
        let temp = TempDir::new().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();

        let Json(initial) = get_config(State(state.clone())).await.unwrap();
        let mut config_value = serde_json::to_value(&initial.config).expect("serialize config");
        config_value["max_tokens"] = serde_json::json!(2048);
        let updated_config: Config =
            serde_json::from_value(config_value).expect("deserialize config update");

        let Json(after_set) = set_config(
            State(state.clone()),
            Json(SetConfigRequest::Wrapped {
                config: updated_config,
                expected_generation: None,
            }),
        )
        .await
        .expect("config set");
        assert_eq!(after_set.config.max_tokens, 2048);

        let Json(after_patch) = patch_config(
            State(state.clone()),
            Json(PatchConfigRequest::Wrapped {
                patch: serde_json::json!({"max_tokens": 3072}),
                expected_generation: None,
            }),
        )
        .await
        .expect("config patch");
        assert_eq!(after_patch.config.max_tokens, 3072);
    }

    #[tokio::test]
    async fn test_config_set_and_patch_reject_invalid_config() {
        let temp = TempDir::new().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();

        let Json(initial) = get_config(State(state.clone())).await.unwrap();
        let mut config_value = serde_json::to_value(&initial.config).expect("serialize config");
        config_value["max_tokens"] = serde_json::json!(0);
        let invalid_config: Config =
            serde_json::from_value(config_value).expect("deserialize invalid config");

        let set_err = set_config(
            State(state.clone()),
            Json(SetConfigRequest::Wrapped {
                config: invalid_config,
                expected_generation: None,
            }),
        )
        .await
        .expect_err("set should reject invalid config");
        assert!(matches!(set_err, ApiError::BadRequest(_)));

        let patch_err = patch_config(
            State(state),
            Json(PatchConfigRequest::Wrapped {
                patch: serde_json::json!({"max_tokens": 0}),
                expected_generation: None,
            }),
        )
        .await
        .expect_err("patch should reject invalid config");
        assert!(matches!(patch_err, ApiError::BadRequest(_)));
    }

    #[test]
    fn test_create_session_request_parsing_with_keep_alive() {
        let req_json = serde_json::json!({
            "prompt": "Hello",
            "keep_alive": true,
            "comms_name": "test-agent"
        });

        let req: CreateSessionRequest = serde_json::from_value(req_json).unwrap();
        assert_eq!(req.prompt, ContentInput::Text("Hello".to_string()));
        assert_eq!(req.keep_alive, Some(true));
        assert_eq!(req.comms_name, Some("test-agent".to_string()));
    }

    #[test]
    fn test_create_session_request_keep_alive_defaults_to_none() {
        let req_json = serde_json::json!({
            "prompt": "Hello"
        });

        let req: CreateSessionRequest = serde_json::from_value(req_json).unwrap();
        assert_eq!(req.keep_alive, None);
        assert!(req.comms_name.is_none());
    }

    #[tokio::test]
    async fn test_create_session_route_rejects_reserved_mob_peer_meta_labels() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        let mock_client: Arc<dyn LlmClient> = Arc::new(MockLlmClient);
        state.llm_client_override = Some(mock_client.clone());
        state.mob_state = Arc::new(
            meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
                state.session_service.clone(),
                Some(state.runtime_adapter.clone()),
            )
            .with_persistent_storage_root(Some(temp.path().to_path_buf()))
            .with_default_llm_client(Some(mock_client)),
        );
        let app = router(state);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "prompt": "Hello",
                            "peer_meta": {
                                "labels": {
                                    "mob_id": "team"
                                }
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            payload["error"]
                .as_str()
                .is_some_and(|msg| msg.contains("reserved") && msg.contains("mob_id")),
            "reserved mob label rejection should explain the trust boundary: {}",
            String::from_utf8_lossy(&body)
        );
    }

    #[test]
    fn test_create_session_request_rejects_reserved_surface_metadata_keys() {
        let app_context = serde_json::json!({
            "meerkat.runtime_id": "spoof"
        });
        let result = validate_public_surface_metadata(None, Some(&app_context));

        assert!(result.is_err());
        match result.unwrap_err() {
            ApiError::BadRequest(message) => assert!(message.contains("meerkat.runtime_id")),
            other => panic!("expected bad request, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_continue_session_route_rejects_reserved_mob_peer_meta_labels() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state.llm_client_override = Some(Arc::new(MockLlmClient));
        let session_service = state.session_service.clone();
        let created = session_service
            .create_session(SvcCreateSessionRequest {
                model: state.default_model.to_string(),
                prompt: "Hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(state.max_tokens),
                event_tx: None,

                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    llm_client_override: state
                        .llm_client_override
                        .clone()
                        .map(encode_llm_client_override_for_service),
                    ..Default::default()
                }),
                labels: None,
            })
            .await
            .expect("deferred session create should succeed");
        let session_id = created.session_id.to_string();
        let app = router(state);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/sessions/{session_id}/messages"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "session_id": session_id,
                            "prompt": "Continue",
                            "peer_meta": {
                                "labels": {
                                    "mob_id": "team"
                                }
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            payload["error"]
                .as_str()
                .is_some_and(|msg| msg.contains("reserved for Meerkat-owned runtime facts")),
            "reserved mob label rejection should explain the trust boundary: {}",
            String::from_utf8_lossy(&body)
        );
    }

    #[tokio::test]
    async fn test_append_system_context_route_returns_staged_status() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state.llm_client_override = Some(Arc::new(MockLlmClient));
        let session_service = state.session_service.clone();
        let pre_session = Session::new();
        let bindings = state
            .runtime_adapter
            .prepare_bindings(pre_session.id().clone())
            .await
            .expect("runtime bindings should prepare");
        let create_result = session_service
            .create_session(SvcCreateSessionRequest {
                model: state.default_model.to_string(),
                prompt: "Hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(state.max_tokens),
                event_tx: None,

                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    resume_session: Some(pre_session),
                    llm_client_override: state
                        .llm_client_override
                        .clone()
                        .map(encode_llm_client_override_for_service),
                    runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                    ..Default::default()
                }),
                labels: None,
            })
            .await
            .expect("deferred session create should succeed");
        let app = router(state);
        let session_id = create_result.session_id.to_string();

        let inject_request = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/sessions/{session_id}/system_context"))
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "text": "Coordinate with the orchestrator.",
                    "source": "mob",
                    "idempotency_key": "ctx-rest-test"
                })
                .to_string(),
            ))
            .unwrap();
        let inject_response = app.clone().oneshot(inject_request).await.unwrap();
        let inject_status = inject_response.status();
        let inject_body = inject_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(
            inject_status,
            StatusCode::OK,
            "append system context failed: {}",
            String::from_utf8_lossy(&inject_body)
        );
        let inject_payload: serde_json::Value = serde_json::from_slice(&inject_body).unwrap();
        assert_eq!(inject_payload["status"], "staged");
    }

    #[tokio::test]
    async fn test_session_status_route_is_available_for_live_sessions() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state.llm_client_override = Some(Arc::new(MockLlmClient));
        let session_service = state.session_service.clone();
        let pre_session = Session::new();
        let bindings = state
            .runtime_adapter
            .prepare_bindings(pre_session.id().clone())
            .await
            .expect("runtime bindings should prepare");
        let create_result = session_service
            .create_session(SvcCreateSessionRequest {
                model: state.default_model.to_string(),
                prompt: "Hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(state.max_tokens),
                event_tx: None,

                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    resume_session: Some(pre_session),
                    llm_client_override: state
                        .llm_client_override
                        .clone()
                        .map(encode_llm_client_override_for_service),
                    runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                    ..Default::default()
                }),
                labels: None,
            })
            .await
            .expect("deferred session create should succeed");
        let app = router(state);
        let session_id = create_result.session_id.to_string();

        let request = axum::http::Request::builder()
            .method("GET")
            .uri(format!("/sessions/{session_id}/status"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();

        assert_eq!(
            status,
            StatusCode::OK,
            "runtime state request failed: {}",
            String::from_utf8_lossy(&body)
        );
    }

    #[tokio::test]
    async fn test_realtime_attachment_status_route_reads_runtime_owned_status() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state.llm_client_override = Some(Arc::new(MockLlmClient));
        let session_service = state.session_service.clone();
        let pre_session = Session::new();
        let bindings = state
            .runtime_adapter
            .prepare_bindings(pre_session.id().clone())
            .await
            .expect("runtime bindings should prepare");
        let create_result = session_service
            .create_session(SvcCreateSessionRequest {
                model: state.default_model.to_string(),
                prompt: "Hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(state.max_tokens),
                event_tx: None,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    resume_session: Some(pre_session),
                    llm_client_override: state
                        .llm_client_override
                        .clone()
                        .map(encode_llm_client_override_for_service),
                    runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
                    ..Default::default()
                }),
                labels: None,
            })
            .await
            .expect("deferred session create should succeed");
        state
            .runtime_adapter
            .project_realtime_attachment_intent(&create_result.session_id, true)
            .await
            .expect("intent projection should succeed");

        let app = router(state);
        let session_id = create_result.session_id.to_string();
        let request = axum::http::Request::builder()
            .method("GET")
            .uri(format!("/sessions/{session_id}/realtime-attachment-status"))
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();

        assert_eq!(
            status,
            StatusCode::OK,
            "runtime live-attachment-status request failed: {}",
            String::from_utf8_lossy(&body)
        );
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("response body should be valid json");
        assert_eq!(payload["status"], "intent_present_unbound");
    }

    #[tokio::test]
    async fn test_realtime_capabilities_route_proxies_to_realtime_rpc_host() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state.llm_client_override = Some(Arc::new(MockLlmClient));
        let session_service = state.session_service.clone();
        let created = session_service
            .create_session(SvcCreateSessionRequest {
                model: state.default_model.to_string(),
                prompt: "Hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(state.max_tokens),
                event_tx: None,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    llm_client_override: state
                        .llm_client_override
                        .clone()
                        .map(encode_llm_client_override_for_service),
                    ..Default::default()
                }),
                labels: None,
            })
            .await
            .expect("deferred session create should succeed");

        let expected = json!({
            "capabilities": {
                "input_kinds": ["text", "audio"],
                "output_kinds": ["text", "audio"],
                "turning_modes": ["provider_managed"],
                "interrupt_supported": true,
                "transcript_supported": true,
                "tool_lifecycle_events_supported": false,
                "video_supported": false,
            }
        });
        let (addr, captured_rx, task) =
            spawn_realtime_rpc_stub("realtime/capabilities", Ok(expected.clone())).await;
        state.realtime_rpc_tcp_addr = Some(addr);

        let app = router(state);
        let request_body = json!({
            "target": {
                "type": "session_target",
                "session_id": created.session_id.to_string(),
            }
        });
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/realtime/capabilities")
            .header("content-type", "application/json")
            .body(Body::from(request_body.to_string()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            status,
            StatusCode::OK,
            "realtime capabilities route failed: {}",
            String::from_utf8_lossy(&body)
        );
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("response body should be valid json");
        assert_eq!(payload, expected);

        let forwarded = captured_rx
            .await
            .expect("realtime proxy request should be captured");
        assert_eq!(forwarded, request_body);
        task.await.expect("realtime rpc stub should join");
    }

    #[tokio::test]
    async fn test_realtime_status_route_reports_transport_unavailable_without_rpc_host() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        let app = router(state);
        let request_body = json!({
            "target": {
                "type": "session_target",
                "session_id": SessionId::new().to_string(),
            }
        });
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/realtime/status")
            .header("content-type", "application/json")
            .body(Body::from(request_body.to_string()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            status,
            StatusCode::NOT_IMPLEMENTED,
            "realtime status should require the realtime RPC host: {}",
            String::from_utf8_lossy(&body)
        );
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("response body should be valid json");
        assert_eq!(payload["code"], "CAPABILITY_UNAVAILABLE");
        assert_eq!(payload["category"], "capability");
    }

    #[tokio::test]
    async fn test_realtime_status_route_proxies_to_realtime_rpc_host() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        let session_id = SessionId::new();
        let expected = json!({
            "status": {
                "state": "reconnecting",
                "attempt_count": 5,
                "next_retry_at": "2026-04-26T12:34:56Z",
                "deadline_at": "2026-04-26T12:35:56Z",
                "reason": "transport_rebind",
            }
        });
        let (addr, captured_rx, task) =
            spawn_realtime_rpc_stub("realtime/status", Ok(expected.clone())).await;
        state.realtime_rpc_tcp_addr = Some(addr);

        let app = router(state);
        let request_body = json!({
            "target": {
                "type": "session_target",
                "session_id": session_id.to_string(),
            }
        });
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/realtime/status")
            .header("content-type", "application/json")
            .body(Body::from(request_body.to_string()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();

        assert_eq!(
            status,
            StatusCode::OK,
            "realtime status proxy should succeed: {}",
            String::from_utf8_lossy(&body)
        );
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("response body should be valid json");
        assert_eq!(payload, expected);

        let forwarded = captured_rx
            .await
            .expect("realtime proxy request should be captured");
        assert_eq!(forwarded, request_body);
        task.await.expect("realtime rpc stub should join");
    }

    #[tokio::test]
    async fn test_runtime_host_routes_report_read_only_projection() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        let app = router(state);

        let request = axum::http::Request::builder()
            .method("GET")
            .uri("/runtime/host_info")
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            status,
            StatusCode::OK,
            "runtime host info failed: {}",
            String::from_utf8_lossy(&body)
        );
        let payload: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["process_name"], "meerkat-rest");
        assert!(
            payload["host_id"].as_str().unwrap().starts_with("process:")
                || payload["host_id"]
                    .as_str()
                    .unwrap()
                    .starts_with("realm-instance:")
        );
        assert!(
            matches!(
                payload["host_id_scope"].as_str(),
                Some("process" | "realm_instance")
            ),
            "unexpected host id scope: {payload}"
        );
        assert_eq!(
            payload["capabilities"]["features"]["runtime_backed_sessions"],
            true
        );
        assert_eq!(payload["capabilities"]["features"]["event_replay"], false);
        assert_eq!(payload["capabilities"]["features"]["artifacts"], false);
        assert_eq!(payload["capabilities"]["features"]["approvals"], false);

        let text = serde_json::to_string(&payload).unwrap();
        for forbidden in ["topology", "registry", "lease", "claim", "project"] {
            assert!(
                !text.contains(forbidden),
                "runtime host projection must not claim topology authority token `{forbidden}`: {text}"
            );
        }

        let request = axum::http::Request::builder()
            .method("GET")
            .uri("/runtime/health")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let health: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(health["status"], "ok");
    }

    #[tokio::test]
    async fn test_realtime_open_info_route_reports_transport_unavailable() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state.llm_client_override = Some(Arc::new(MockLlmClient));
        let session_service = state.session_service.clone();
        let created = session_service
            .create_session(SvcCreateSessionRequest {
                model: state.default_model.to_string(),
                prompt: "Hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(state.max_tokens),
                event_tx: None,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    llm_client_override: state
                        .llm_client_override
                        .clone()
                        .map(encode_llm_client_override_for_service),
                    ..Default::default()
                }),
                labels: None,
            })
            .await
            .expect("deferred session create should succeed");

        let app = router(state);
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/realtime/open_info")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "target": {
                        "type": "session_target",
                        "session_id": created.session_id.to_string(),
                    },
                    "role": "primary",
                    "turning_mode": "provider_managed",
                })
                .to_string(),
            ))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            status,
            StatusCode::NOT_IMPLEMENTED,
            "realtime open-info should stay unavailable until websocket host lands: {}",
            String::from_utf8_lossy(&body)
        );
    }

    #[tokio::test]
    async fn test_realtime_open_info_route_proxies_to_realtime_rpc_host() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state.llm_client_override = Some(Arc::new(MockLlmClient));
        let session_service = state.session_service.clone();
        let created = session_service
            .create_session(SvcCreateSessionRequest {
                model: state.default_model.to_string(),
                prompt: "Hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(state.max_tokens),
                event_tx: None,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    llm_client_override: state
                        .llm_client_override
                        .clone()
                        .map(encode_llm_client_override_for_service),
                    ..Default::default()
                }),
                labels: None,
            })
            .await
            .expect("deferred session create should succeed");

        let expected = json!({
            "ws_url": "ws://127.0.0.1:43210/realtime/ws",
            "open_token": "rest-proxy-token",
            "expires_at": "2026-04-15T12:00:00Z",
            "target": {
                "type": "session_target",
                "session_id": created.session_id.to_string(),
            },
            "supported_protocol_versions": ["2026-04-01"],
            "default_protocol_version": "2026-04-01",
            "capabilities": {
                "input_kinds": ["text", "audio"],
                "output_kinds": ["text", "audio"],
                "turning_modes": ["provider_managed"],
                "interrupt_supported": true,
                "transcript_supported": true,
                "tool_lifecycle_events_supported": false,
                "video_supported": false,
            }
        });
        let (addr, captured_rx, task) =
            spawn_realtime_rpc_stub("realtime/open_info", Ok(expected.clone())).await;
        state.realtime_rpc_tcp_addr = Some(addr);

        let app = router(state);
        let request_body = json!({
            "target": {
                "type": "session_target",
                "session_id": created.session_id.to_string(),
            },
            "role": "primary",
            "turning_mode": "provider_managed",
        });
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/realtime/open_info")
            .header("content-type", "application/json")
            .body(Body::from(request_body.to_string()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();

        assert_eq!(
            status,
            StatusCode::OK,
            "realtime open-info proxy should succeed: {}",
            String::from_utf8_lossy(&body)
        );
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("response body should be valid json");
        assert_eq!(payload, expected);

        let forwarded = captured_rx
            .await
            .expect("realtime proxy request should be captured");
        assert_eq!(forwarded, request_body);
        task.await.expect("realtime rpc stub should join");
    }

    #[tokio::test]
    async fn test_realtime_open_info_route_preserves_rpc_error_code() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        let request_body = json!({
            "target": {
                "type": "session_target",
                "session_id": SessionId::new().to_string(),
            },
            "role": "primary",
            "turning_mode": "explicit_commit",
        });
        let rpc_error = json!({
            "code": ErrorCode::CapabilityUnavailable.jsonrpc_code(),
            "message": "turning mode 'explicit_commit' is not supported for this realtime target",
            "data": {
                "turning_mode": "explicit_commit"
            }
        });
        let (addr, captured_rx, task) =
            spawn_realtime_rpc_stub("realtime/open_info", Err(rpc_error)).await;
        state.realtime_rpc_tcp_addr = Some(addr);

        let app = router(state);
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/realtime/open_info")
            .header("content-type", "application/json")
            .body(Body::from(request_body.to_string()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();

        assert_eq!(
            status,
            StatusCode::NOT_IMPLEMENTED,
            "canonical capability error should map through typed REST envelope: {}",
            String::from_utf8_lossy(&body)
        );
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("response body should be valid json");
        assert_eq!(payload["code"], "CAPABILITY_UNAVAILABLE");
        assert_eq!(payload["category"], "capability");
        assert_eq!(payload["details"]["turning_mode"], "explicit_commit");

        let forwarded = captured_rx
            .await
            .expect("realtime proxy request should be captured");
        assert_eq!(forwarded, request_body);
        task.await.expect("realtime rpc stub should join");
    }

    #[tokio::test]
    async fn test_get_session_history_route_returns_messages_for_live_and_archived_sessions() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state.llm_client_override = Some(Arc::new(MockLlmClient));
        let session_service = state.session_service.clone();
        let created = session_service
            .create_session(SvcCreateSessionRequest {
                model: state.default_model.to_string(),
                prompt: "Hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(state.max_tokens),
                event_tx: None,

                skill_references: None,
                initial_turn: InitialTurnPolicy::RunImmediately,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    llm_client_override: state
                        .llm_client_override
                        .clone()
                        .map(encode_llm_client_override_for_service),
                    ..Default::default()
                }),
                labels: None,
            })
            .await
            .expect("session create should succeed");
        let session_id = created.session_id.to_string();

        session_service
            .start_turn(
                &created.session_id,
                meerkat_core::service::StartTurnRequest {
                    prompt: "Follow up".to_string().into(),
                    system_prompt: None,
                    render_metadata: None,
                    handling_mode: meerkat_core::types::HandlingMode::Queue,
                    event_tx: None,

                    skill_references: None,
                    flow_tool_overlay: None,
                    turn_metadata: None,
                },
            )
            .await
            .expect("second turn should succeed");

        let app = router(state.clone());
        let request = axum::http::Request::builder()
            .method("GET")
            .uri(format!("/sessions/{session_id}/history?offset=1&limit=2"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(status, StatusCode::OK);
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["session_id"], session_id);
        assert!(
            payload["message_count"].as_u64().unwrap_or(0) >= 4,
            "history should expose the full multi-turn transcript: {payload}"
        );
        assert_eq!(payload["offset"], 1);
        assert_eq!(payload["limit"], 2);
        assert_eq!(payload["has_more"], true);
        assert_eq!(payload["messages"].as_array().unwrap().len(), 2);

        session_service
            .archive(&created.session_id)
            .await
            .expect("archive should succeed");

        let archived_request = axum::http::Request::builder()
            .method("GET")
            .uri(format!("/sessions/{session_id}/history"))
            .body(Body::empty())
            .unwrap();
        let archived_response = app.oneshot(archived_request).await.unwrap();
        let archived_status = archived_response.status();
        let archived_body = archived_response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(archived_status, StatusCode::OK);
        let archived_payload: serde_json::Value = serde_json::from_slice(&archived_body).unwrap();
        assert!(
            archived_payload["message_count"].as_u64().unwrap_or(0) >= 4,
            "archived history should preserve the transcript: {archived_payload}"
        );
        assert!(
            archived_payload["messages"].as_array().unwrap().len() >= 4,
            "archived history should return the full transcript"
        );
    }

    #[tokio::test]
    async fn test_create_session_route_completes_in_runtime_backed_mode() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state.llm_client_override = Some(Arc::new(MockLlmClient));
        let app = router(state);

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            app.oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "prompt": "Remember RuntimeRouteFox and reply briefly."
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            ),
        )
        .await
        .expect("runtime-backed create route timed out")
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(payload["session_id"].is_string());
        assert_eq!(payload["text"], "ok");
    }

    #[test]
    fn test_comms_send_request_peer_request_invalid_stream_rejected_at_serde() {
        let json = format!(
            r#"{{"session_id":"sid_123","kind":"peer_request","to":"{}","intent":"ask","stream":"invalid"}}"#,
            uuid::Uuid::new_v4()
        );
        let err = serde_json::from_str::<CommsSendRequest>(&json)
            .expect_err("invalid stream must fail deserialization");
        assert!(
            err.to_string().contains("stream") || err.to_string().contains("invalid"),
            "expected serde error mentioning stream, got: {err}"
        );
    }

    #[test]
    fn test_comms_send_request_peer_response_invalid_status_rejected_at_serde() {
        let json = format!(
            r#"{{"session_id":"sid_123","kind":"peer_response","to":"{}","in_reply_to":"{}","status":"almost-done"}}"#,
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4()
        );
        let err = serde_json::from_str::<CommsSendRequest>(&json)
            .expect_err("invalid status must fail deserialization");
        assert!(
            err.to_string().contains("status") || err.to_string().contains("almost-done"),
            "expected serde error mentioning status, got: {err}"
        );
    }

    #[test]
    fn test_comms_send_request_invalid_source_rejected_at_serde() {
        let json = r#"{"session_id":"sid_123","kind":"input","body":"hi","source":"webhookd"}"#;
        let err = serde_json::from_str::<CommsSendRequest>(json)
            .expect_err("invalid source must fail deserialization");
        assert!(
            err.to_string().contains("source") || err.to_string().contains("webhookd"),
            "expected serde error mentioning source, got: {err}"
        );
    }

    #[cfg(not(feature = "comms"))]
    #[test]
    fn test_resolve_keep_alive_rejects_when_comms_disabled() {
        let err = resolve_keep_alive(Some(true)).expect_err("keep_alive should be rejected");
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_resolve_keep_alive_allows_when_comms_enabled() {
        assert_eq!(resolve_keep_alive(Some(true)).unwrap(), Some(true));
        assert_eq!(resolve_keep_alive(Some(false)).unwrap(), Some(false));
        assert_eq!(resolve_keep_alive(None).unwrap(), None);
    }

    #[test]
    fn test_create_session_request_accepts_hooks_override_fixture() {
        let hooks_override = hooks_override_fixture();
        let req_json = serde_json::json!({
            "prompt": "Hello",
            "hooks_override": hooks_override,
        });

        let req: CreateSessionRequest = serde_json::from_value(req_json).unwrap();
        assert!(req.hooks_override.is_some());
        let overrides = req
            .hooks_override
            .expect("hooks override should be present");
        assert_eq!(overrides.entries.len(), 2);
        assert_eq!(
            overrides.entries[0].point,
            meerkat_core::HookPoint::PreToolExecution
        );
    }

    #[test]
    fn test_continue_session_request_accepts_hooks_override_fixture() {
        let hooks_override = hooks_override_fixture();
        let req_json = serde_json::json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "Continue",
            "hooks_override": hooks_override,
        });

        let req: ContinueSessionRequest = serde_json::from_value(req_json).unwrap();
        assert!(req.hooks_override.is_some());
        let overrides = req
            .hooks_override
            .expect("hooks override should be present");
        assert_eq!(overrides.entries.len(), 2);
        assert_eq!(
            overrides.entries[1].mode,
            meerkat_core::HookExecutionMode::Background
        );
    }

    #[test]
    fn test_rest_continue_requires_rebuild_matches_surface_contract() {
        let mut req = ContinueSessionRequest {
            session_id: "01234567-89ab-cdef-0123-456789abcdef".to_string(),
            prompt: ContentInput::Text("Continue".to_string()),
            system_prompt: None,
            output_schema: None,
            structured_output_retries: None,
            keep_alive: None,
            comms_name: None,
            peer_meta: None,
            verbose: false,
            model: None,
            provider: None,
            max_tokens: None,
            hooks_override: None,
            skill_refs: None,
            flow_tool_overlay: None,
            additional_instructions: None,
        };
        assert!(!rest_continue_requires_rebuild(&req));

        req.model = Some("gpt-5.4".into());
        assert!(rest_continue_requires_rebuild(&req));
        req.model = None;

        req.flow_tool_overlay = Some(meerkat_core::service::TurnToolOverlay::default());
        assert!(
            !rest_continue_requires_rebuild(&req),
            "flow tool overlay stays on the live path"
        );
        req.flow_tool_overlay = None;

        req.additional_instructions = Some(vec!["extra".to_string()]);
        assert!(
            !rest_continue_requires_rebuild(&req),
            "additional instructions stay on the live path"
        );
        req.additional_instructions = None;

        req.comms_name = Some("agent-a".to_string());
        assert!(rest_continue_requires_rebuild(&req));
    }

    #[tokio::test]
    async fn test_continue_session_invalid_keep_alive_is_side_effect_free() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state.llm_client_override = Some(Arc::new(MockLlmClient));
        let session_service = state.session_service.clone();
        let created = session_service
            .create_session(SvcCreateSessionRequest {
                model: state.default_model.to_string(),
                prompt: "Hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(state.max_tokens),
                event_tx: None,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    llm_client_override: state
                        .llm_client_override
                        .clone()
                        .map(encode_llm_client_override_for_service),
                    ..Default::default()
                }),
                labels: None,
            })
            .await
            .expect("deferred session create should succeed");
        let session_id = created.session_id.to_string();
        let app = router(state);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/sessions/{session_id}/messages"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "session_id": session_id,
                            "prompt": "Continue",
                            "keep_alive": true
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["error"], "keep_alive requires comms_name");

        let session = session_service
            .load_authoritative_session(&created.session_id)
            .await
            .expect("load should succeed")
            .expect("session should still exist");
        let metadata = session.session_metadata().expect("metadata should exist");
        assert!(
            !metadata.keep_alive,
            "failed request must not persist keep_alive"
        );
        assert!(metadata.comms_name.is_none());
    }

    #[tokio::test]
    async fn test_create_session_route_returns_identity_on_post_commit_turn_failure() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state.llm_client_override = Some(Arc::new(ErrorLlmClient));
        let app = router(state);

        // Bound the oneshot in a timeout matching the sibling at lib.rs:5370
        // (`test_create_session_route_completes_in_runtime_backed_mode`). The
        // post-commit-failure path currently deadlocks when `ErrorLlmClient`
        // surfaces the failure — see #32 Class B in the triage doc at
        // `docs/wave-d-prep/workspace-runtime-cascade-triage.md`. The timeout
        // converts the hang into a visible `Elapsed(())` panic so workspace
        // nextest runs don't stall indefinitely. The underlying deadlock in
        // the runtime-backed create-session error branch is a separate
        // root-cause fix.
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            app.oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "prompt": "Trigger failure"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            ),
        )
        .await
        .expect("post-commit-failure create route timed out")
        .unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["code"], "SESSION_CREATED_WITH_TURN_FAILURE");
        assert_eq!(payload["details"]["session_created"], true);
        assert_eq!(payload["details"]["resumable"], true);
        assert!(payload["details"]["session_id"].is_string());
        assert!(payload["details"]["session_ref"].is_string());
    }

    #[test]
    fn test_skill_entry_uses_canonical_source_identity_records() {
        use meerkat_core::skills::{
            SkillDescriptor, SkillIntrospectionEntry, SkillKey, SkillName, SkillScope,
            SourceIdentityRecord, SourceIdentityStatus, SourceTransportKind, SourceUuid,
        };

        let source_uuid =
            SourceUuid::parse("33333333-3333-4333-8333-333333333333").expect("source uuid");
        let shadow_uuid =
            SourceUuid::parse("44444444-4444-4444-8444-444444444444").expect("shadow uuid");
        let key = SkillKey::new(
            source_uuid.clone(),
            SkillName::parse("demo-skill").expect("skill name"),
        );
        let mut descriptor = SkillDescriptor::new(key, "Demo Skill", "Demo description");
        descriptor.scope = SkillScope::Project;
        descriptor.source_name = "canonical-source".to_string();

        let source_identity = SourceIdentityRecord {
            source_uuid: source_uuid.clone(),
            display_name: "canonical-source".to_string(),
            transport_kind: SourceTransportKind::Git,
            fingerprint: "repo-canonical-source".to_string(),
            status: SourceIdentityStatus::Retired,
        };
        let shadow_identity = SourceIdentityRecord {
            source_uuid: shadow_uuid.clone(),
            display_name: "shadow-source".to_string(),
            transport_kind: SourceTransportKind::Http,
            fingerprint: "repo-shadow-source".to_string(),
            status: SourceIdentityStatus::Disabled,
        };

        let entry = SkillIntrospectionEntry {
            descriptor,
            source_identity: Some(source_identity.clone()),
            shadowed_by: Some("shadow-source".to_string()),
            shadowed_by_identity: Some(shadow_identity.clone()),
            shadowed_by_source_uuid: Some(shadow_uuid),
            is_active: false,
        };

        let wire = skill_entry(&entry).expect("skill entry");

        assert_eq!(wire.source.identity, source_identity);
        assert_eq!(
            wire.shadowed_by.expect("shadowed by").identity,
            shadow_identity
        );
    }

    #[test]
    fn test_validate_config_for_commit_rejects_invalid_skills_identity() {
        let mut config = Config::default();
        let source_uuid =
            meerkat_core::skills::SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                .expect("uuid");
        config.skills.repositories = vec![
            meerkat_core::skills_config::SkillRepositoryConfig {
                name: "a".to_string(),
                source_uuid: source_uuid.clone(),
                transport: meerkat_core::skills_config::SkillRepoTransport::Filesystem {
                    path: "/tmp/a".to_string(),
                },
            },
            meerkat_core::skills_config::SkillRepositoryConfig {
                name: "b".to_string(),
                source_uuid,
                transport: meerkat_core::skills_config::SkillRepoTransport::Filesystem {
                    path: "/tmp/b".to_string(),
                },
            },
        ];

        let err = validate_config_for_commit_with_roots(&config, None, None)
            .expect_err("duplicate source uuid");
        assert!(matches!(&err, ApiError::BadRequest(_)));
        if let ApiError::BadRequest(message) = err {
            assert!(message.contains("Invalid skills source-identity config"));
        }
    }

    #[test]
    fn test_run_result_to_response_carries_skill_diagnostics() {
        let session_id = SessionId::new();
        let result = meerkat_core::RunResult {
            text: "ok".to_string(),
            session_id: session_id.clone(),
            usage: Default::default(),
            turns: 1,
            tool_calls: 0,
            structured_output: None,
            schema_warnings: None,
            skill_diagnostics: Some(meerkat_core::skills::SkillRuntimeDiagnostics {
                source_health: meerkat_core::skills::SourceHealthSnapshot {
                    state: meerkat_core::skills::SourceHealthState::Degraded,
                    invalid_ratio: 0.50,
                    invalid_count: 1,
                    total_count: 2,
                    failure_streak: 3,
                    handshake_failed: false,
                },
                quarantined: vec![],
            }),
        };

        let realm = meerkat_core::RealmId::parse("test-realm").expect("valid test realm id");
        let response = run_result_to_response(result, &realm);
        assert!(response.skill_diagnostics.is_some());
        assert_eq!(
            response
                .skill_diagnostics
                .as_ref()
                .expect("skill_diagnostics")
                .source_health
                .state,
            meerkat_core::skills::SourceHealthState::Degraded
        );
        assert_eq!(
            response.session_ref.as_deref().expect("session_ref"),
            format_session_ref(&realm, &session_id)
        );
    }

    #[test]
    fn completion_outcome_to_api_result_surfaces_callback_pending_payload() {
        let session_id = SessionId::new();
        let realm = meerkat_core::RealmId::parse("test-realm").expect("valid test realm id");
        let err = completion_outcome_to_api_result(
            meerkat_runtime::completion::CompletionOutcome::CallbackPending {
                tool_name: "external_mock".to_string(),
                args: json!({ "value": "browser" }),
            },
            &session_id,
            &realm,
            false,
        )
        .expect_err("callback pending should map to an API error");

        let ApiError::InternalWithData {
            message,
            code,
            details,
        } = err
        else {
            panic!("expected InternalWithData callback error");
        };

        assert_eq!(message, "callback pending for tool 'external_mock'");
        assert_eq!(code, "CALLBACK_PENDING");
        assert_eq!(details["session_id"], session_id.to_string());
        assert_eq!(
            details["session_ref"],
            format_session_ref(&realm, &session_id)
        );
        assert_eq!(details["resumable"], true);
        assert_eq!(details["tool_name"], "external_mock");
        assert_eq!(details["args"], json!({ "value": "browser" }));
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_compatibility_mob_routes_are_not_found() {
        use axum::body::Body;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        let app = router(state);

        let tools_req = axum::http::Request::builder()
            .method("GET")
            .uri("/mob/tools")
            .body(Body::empty())
            .unwrap();
        let tools_resp = app.clone().oneshot(tools_req).await.unwrap();
        assert_eq!(tools_resp.status(), StatusCode::NOT_FOUND);

        let call_req = axum::http::Request::builder()
            .method("POST")
            .uri("/mob/call")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "name": "mob_create",
                    "arguments": { "definition": { "id": "test_mob", "profiles": { "worker": { "model": "claude-sonnet-4-6", "tools": { "comms": true } } } } }
                })
                .to_string(),
            ))
            .unwrap();
        let call_resp = app.oneshot(call_req).await.unwrap();
        assert_eq!(call_resp.status(), StatusCode::NOT_FOUND);
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_mob_wait_kickoff_route_returns_member_snapshots() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        let definition = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"test_mob\"\n\n[profiles.worker]\nmodel = \"claude-sonnet-4-6\"\n\n[profiles.worker.tools]\ncomms = true\n",
        )
        .expect("minimal mob definition");
        let mob_id = state
            .mob_state
            .mob_create_definition(definition)
            .await
            .expect("create mob");

        let app = router(state);
        let request = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/mob/{mob_id}/wait-kickoff"))
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
        let payload: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let members = payload["members"]
            .as_array()
            .expect("members should be an array");
        assert!(
            members.is_empty(),
            "empty mob should yield no member snapshots"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_mob_wait_kickoff_route_respects_member_filter() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        let definition = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"test_mob\"\n\n[profiles.worker]\nmodel = \"claude-sonnet-4-6\"\n\n[profiles.worker.tools]\ncomms = true\n",
        )
        .expect("minimal mob definition");
        let mob_id = state
            .mob_state
            .mob_create_definition(definition)
            .await
            .expect("create mob");

        let app = router(state);
        let body = serde_json::json!({
            "member_ids": ["lead-filter"],
            "timeout_ms": 10_000
        });
        let request = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/mob/{mob_id}/wait-kickoff"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
        let payload: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let members = payload["members"]
            .as_array()
            .expect("members should be an array");
        assert_eq!(members.len(), 1);
        assert_eq!(members[0]["agent_identity"], "lead-filter");
        assert_eq!(members[0]["status"], "unknown");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    #[ignore = "requires ANTHROPIC_API_KEY; run with cargo e2e-system"]
    async fn test_mob_spawn_helper_route_returns_identity_native_fields() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state.llm_client_override = Some(Arc::new(MockLlmClient));
        let definition = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"test_mob\"\n\n[profiles.worker]\nmodel = \"claude-sonnet-4-6\"\n\n[profiles.worker.tools]\nbuiltins = true\ncomms = true\n",
        )
        .expect("minimal mob definition");
        let mob_id = state
            .mob_state
            .mob_create_definition(definition)
            .await
            .expect("create mob");

        let app = router(state);
        let request = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/mob/{mob_id}/spawn-helper"))
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "prompt": "Hello from helper",
                    "agent_identity": "helper-rest",
                    "role_name": "worker",
                })
                .to_string(),
            ))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            status,
            StatusCode::OK,
            "spawn helper route failed: {}",
            String::from_utf8_lossy(&body)
        );
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["agent_identity"], "helper-rest");
        assert!(
            payload["member_ref"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "member_ref must be populated"
        );
        assert!(
            payload.get("agent_runtime_id").is_none(),
            "binding-era agent_runtime_id must not leak to app-facing responses"
        );
    }

    // -----------------------------------------------------------------------
    // Live MCP tests
    // -----------------------------------------------------------------------

    #[cfg(feature = "mcp")]
    mod mcp_tests {
        use super::*;
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        async fn make_test_state() -> (AppState, TempDir) {
            let temp = TempDir::new().unwrap();
            let state = AppState::load_from(temp.path().to_path_buf())
                .await
                .unwrap();
            (state, temp)
        }

        #[tokio::test]
        async fn test_mcp_nonexistent_session_404() {
            let (state, _temp) = make_test_state().await;
            let fake_id = SessionId::new();
            let app = router(state);
            let body = serde_json::json!({
                "session_id": fake_id.to_string(),
                "server_name": "test-server",
                "server_config": {"command": "echo", "args": ["hello"]}
            });
            let request = axum::http::Request::builder()
                .method("POST")
                .uri(format!("/sessions/{fake_id}/mcp/add"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap();
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }

        #[tokio::test]
        async fn test_mcp_add_empty_server_name_400() {
            let (state, _temp) = make_test_state().await;
            // Create a session first (even though it won't run an LLM call,
            // the MCP adapter is attached at creation time).
            let session_id = SessionId::new();
            {
                let mut map = state.mcp_sessions.write().await;
                let adapter = Arc::new(McpRouterAdapter::new(McpRouter::new()));
                let (tx, rx) = mpsc::unbounded_channel();
                map.insert(
                    session_id.clone(),
                    SessionMcpState {
                        adapter,
                        turn_counter: 0,
                        lifecycle_tx: tx,
                        lifecycle_rx: rx,
                        drain_task_running: Arc::new(AtomicBool::new(false)),
                    },
                );
            }

            let app = router(state);
            let body = serde_json::json!({
                "session_id": session_id.to_string(),
                "server_name": "  ",
                "server_config": {"command": "echo"}
            });
            let request = axum::http::Request::builder()
                .method("POST")
                .uri(format!("/sessions/{session_id}/mcp/add"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap();
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
            let error: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
            assert!(
                error["error"]
                    .as_str()
                    .unwrap()
                    .contains("server_name cannot be empty")
            );
        }

        #[tokio::test]
        async fn test_mcp_routes_registered() {
            // With mcp feature enabled, the routes should exist.
            let (state, _temp) = make_test_state().await;
            let fake_id = SessionId::new();
            let app = router(state);

            // POST to /sessions/X/mcp/add — route exists (will get 404 for missing session).
            let body = serde_json::json!({
                "session_id": fake_id.to_string(),
                "server_name": "srv",
                "server_config": {"command": "echo"}
            });
            let request = axum::http::Request::builder()
                .method("POST")
                .uri(format!("/sessions/{fake_id}/mcp/add"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap();
            let response = app.oneshot(request).await.unwrap();
            // 404 NOT_FOUND (session not found), not 405 (route not found).
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
            let error: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
            assert_eq!(error["code"].as_str().unwrap(), "NOT_FOUND");
        }

        #[tokio::test]
        async fn test_mcp_response_shape() {
            // Verify the response builder produces correct shape.
            let resp = meerkat::surface::mcp_live_response(
                "sid_123".to_string(),
                meerkat_contracts::McpLiveOperation::Add,
                Some("test-server".to_string()),
                false,
            );
            assert_eq!(resp.session_id, "sid_123");
            assert_eq!(resp.operation, meerkat_contracts::McpLiveOperation::Add);
            assert_eq!(resp.server_name, Some("test-server".to_string()));
            assert_eq!(resp.status, meerkat_contracts::McpLiveOpStatus::Staged);
            assert!(!resp.persisted);
            assert!(resp.applied_at_turn.is_none());
        }

        #[test]
        fn test_mcp_response_shape_tracks_persisted_flag() {
            let resp = meerkat::surface::mcp_live_response(
                "sid_123".to_string(),
                meerkat_contracts::McpLiveOperation::Add,
                Some("test-server".to_string()),
                true,
            );
            assert!(resp.persisted);
        }

        #[test]
        fn test_conflict_error_is_409() {
            let err = ApiError::Conflict("test conflict".to_string());
            let response = err.into_response();
            assert_eq!(response.status(), StatusCode::CONFLICT);
        }

        #[tokio::test]
        async fn test_duplicate_input_error_returns_409_with_existing_id() {
            let err = ApiError::DuplicateInput {
                existing_id: "input-abc-123".to_string(),
            };
            let response = err.into_response();
            assert_eq!(response.status(), StatusCode::CONFLICT);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["error"], "duplicate_input");
            assert_eq!(json["code"], "DUPLICATE_INPUT");
            assert_eq!(json["existing_id"], "input-abc-123");
        }
    }

    mod request_cancel_tests {
        use super::*;
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        #[tokio::test]
        async fn test_missing_header_works_normally() {
            let temp = TempDir::new().unwrap();
            let mut state = AppState::load_from(temp.path().to_path_buf())
                .await
                .unwrap();
            state.llm_client_override = Some(Arc::new(MockLlmClient));
            let app = router(state);

            let response = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                app.oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/sessions")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            serde_json::json!({"prompt": "Hello"}).to_string(),
                        ))
                        .unwrap(),
                ),
            )
            .await
            .expect("should not timeout")
            .unwrap();

            assert_eq!(response.status(), StatusCode::OK);
        }

        #[tokio::test]
        async fn test_empty_request_id_returns_400() {
            let temp = TempDir::new().unwrap();
            let mut state = AppState::load_from(temp.path().to_path_buf())
                .await
                .unwrap();
            state.llm_client_override = Some(Arc::new(MockLlmClient));
            let app = router(state);

            let response = app
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/sessions")
                        .header("content-type", "application/json")
                        .header("x-meerkat-request-id", "")
                        .body(Body::from(
                            serde_json::json!({"prompt": "Hello"}).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body = response.into_body().collect().await.unwrap().to_bytes();
            let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert!(
                payload["error"]
                    .as_str()
                    .unwrap()
                    .contains("must not be empty")
            );
        }

        #[tokio::test]
        async fn test_cancel_unknown_returns_404() {
            let temp = TempDir::new().unwrap();
            let state = AppState::load_from(temp.path().to_path_buf())
                .await
                .unwrap();
            let app = router(state);

            let response = app
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/requests/nonexistent-req/cancel")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }

        #[tokio::test]
        async fn test_publish_terminal_after_cancel_returns_request_cancelled() {
            let executor = SurfaceRequestExecutor::new(std::time::Duration::from_millis(1));
            let ctx = executor.begin_request("rest-cancel-before-publish", noop_request_action());

            assert_eq!(
                executor.cancel_request(ctx.key()).await,
                meerkat::surface::CancelOutcome::Cancelled
            );

            let result = with_request_lifecycle(
                &executor,
                Some(ctx),
                RequestTerminal::Publish(Err(ApiError::Internal(
                    "publish response must not leak after cancel".to_string(),
                ))),
            )
            .await;

            assert!(matches!(
                result,
                Err(ApiError::RequestCancelled { details: None })
            ));
            assert_eq!(executor.phase("rest-cancel-before-publish"), None);
        }

        #[derive(Clone)]
        struct RequestLifecycleProbeState {
            executor: SurfaceRequestExecutor,
        }

        async fn publish_after_cancel_probe(
            State(state): State<RequestLifecycleProbeState>,
            headers: axum::http::HeaderMap,
        ) -> Result<Json<SessionResponse>, ApiError> {
            let ctx = extract_request_context(&headers, &state.executor)?;
            if let Some(ctx) = ctx.as_ref() {
                let _ = state.executor.cancel_request(ctx.key()).await;
            }
            with_request_lifecycle(
                &state.executor,
                ctx,
                RequestTerminal::Publish(Err(ApiError::Internal(
                    "publish response must not cross HTTP after cancel".to_string(),
                ))),
            )
            .await
        }

        #[tokio::test]
        async fn test_publish_terminal_after_cancel_returns_http_499() {
            let app = Router::new()
                .route("/probe", post(publish_after_cancel_probe))
                .with_state(RequestLifecycleProbeState {
                    executor: SurfaceRequestExecutor::new(std::time::Duration::from_millis(1)),
                });

            let response = app
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/probe")
                        .header("x-meerkat-request-id", "rest-http-cancel-before-publish")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(
                response.status(),
                StatusCode::from_u16(499).expect("499 should be a valid status")
            );
            let body = response.into_body().collect().await.unwrap().to_bytes();
            let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(payload["code"], "REQUEST_CANCELLED");
        }
    }

    /// Verify MCP routes are NOT registered when the feature is off.
    #[cfg(not(feature = "mcp"))]
    mod mcp_feature_off_tests {
        use super::*;
        use axum::body::Body;
        use tower::ServiceExt;

        #[tokio::test]
        async fn test_mcp_routes_not_registered_without_feature() {
            let temp = TempDir::new().unwrap();
            let state = AppState::load_from(temp.path().to_path_buf())
                .await
                .unwrap();
            let app = router(state);

            let body = serde_json::json!({
                "session_id": "fake",
                "server_name": "srv",
                "server_config": {"command": "echo"}
            });
            let request = axum::http::Request::builder()
                .method("POST")
                .uri("/sessions/fake/mcp/add")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap();
            let response = app.oneshot(request).await.unwrap();
            // Should be 405 Method Not Allowed (route doesn't exist for POST).
            assert_ne!(response.status(), StatusCode::OK);
        }
    }
}
