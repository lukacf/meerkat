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
use meerkat::{
    AgentEvent, AgentFactory, FactoryAgentBuilder, LlmClient, OutputSchema,
    PersistentSessionService, Session, SessionId, SessionService, SessionServiceCommsExt,
    SessionServiceControlExt, SessionServiceHistoryExt, encode_llm_client_override_for_service,
    open_realm_persistence_in,
};
use meerkat_contracts::{SessionLocator, SkillsParams, format_session_ref};
use meerkat_core::EventEnvelope;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{CoreRenderable, RunApplyBoundary, RunPrimitive};
use meerkat_core::service::HostModeOwner;
use meerkat_core::service::{
    AppendSystemContextRequest as SvcAppendSystemContextRequest,
    CreateSessionRequest as SvcCreateSessionRequest, InitialTurnPolicy, SessionBuildOptions,
    SessionControlError, SessionError, StartTurnRequest as SvcStartTurnRequest,
};
use meerkat_core::{
    Config, ConfigDelta, ConfigEnvelope, ConfigEnvelopePolicy, ConfigStore, ContentInput,
    FileConfigStore, HookRunOverrides, Provider, RealmSelection, RuntimeBootstrap, SessionTooling,
    agent_event_type, format_verbose_event,
};
use meerkat_runtime::SessionServiceRuntimeExt as _;
use meerkat_store::{RealmBackend, RealmOrigin};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
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
    /// Override the resolved LLM client (primarily for tests and embedding).
    pub llm_client_override: Option<Arc<dyn LlmClient>>,
    pub config_store: Arc<dyn ConfigStore>,
    pub event_tx: broadcast::Sender<SessionEvent>,
    /// Session service for managing agent lifecycle.
    pub session_service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    /// Webhook authentication, resolved once at startup from RKAT_WEBHOOK_SECRET.
    pub webhook_auth: webhook::WebhookAuth,
    pub realm_id: String,
    pub instance_id: Option<String>,
    pub backend: String,
    pub resolved_paths: meerkat_core::ConfigResolvedPaths,
    pub expose_paths: bool,
    pub config_runtime: Arc<meerkat_core::ConfigRuntime>,
    pub realm_lease: Arc<tokio::sync::Mutex<Option<meerkat_store::RealmLeaseGuard>>>,
    pub skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    /// Optional v9 runtime adapter for runtime/input endpoints.
    pub runtime_adapter: Arc<meerkat_runtime::RuntimeSessionAdapter>,
    /// Shared in-process mob lifecycle state for protocol mob operations.
    #[cfg(feature = "mob")]
    pub mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
    /// Per-session MCP adapter state (live MCP mutation).
    #[cfg(feature = "mcp")]
    pub mcp_sessions: Arc<RwLock<std::collections::HashMap<SessionId, SessionMcpState>>>,
}

#[derive(Debug, Clone)]
pub struct SessionEvent {
    session_id: SessionId,
    event: EventEnvelope<AgentEvent>,
}

#[derive(Clone)]
struct RestRuntimeExecutorContext {
    default_model: Cow<'static, str>,
    max_tokens: u32,
    enable_builtins: bool,
    enable_shell: bool,
    llm_client_override: Option<Arc<dyn LlmClient>>,
    event_tx: broadcast::Sender<SessionEvent>,
    session_service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    realm_id: String,
    instance_id: Option<String>,
    backend: String,
    config_runtime: Arc<meerkat_core::ConfigRuntime>,
    runtime_adapter: Arc<meerkat_runtime::RuntimeSessionAdapter>,
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
        let realm_id = locator.realm_id;
        let instance_id = bootstrap.realm.instance_id;
        let backend_hint = bootstrap
            .realm
            .backend_hint
            .as_deref()
            .and_then(parse_backend_hint);
        let origin_hint = Some(realm_origin_from_selection(&bootstrap.realm.selection));
        let realms_root = locator.state_root;
        let (manifest, persistence) =
            open_realm_persistence_in(&realms_root, &realm_id, backend_hint, origin_hint).await?;
        let session_store = persistence.session_store();
        let realm_paths = meerkat_store::realm_paths_in(&realms_root, &realm_id);
        let resolved_paths = meerkat_core::ConfigResolvedPaths {
            root: realm_paths.root.display().to_string(),
            manifest_path: realm_paths.manifest_path.display().to_string(),
            config_path: realm_paths.config_path.display().to_string(),
            sessions_redb_path: realm_paths.sessions_redb_path.display().to_string(),
            sessions_sqlite_path: Some(realm_paths.sessions_sqlite_path.display().to_string()),
            sessions_jsonl_dir: realm_paths.sessions_jsonl_dir.display().to_string(),
        };
        let base_config_store: Arc<dyn ConfigStore> =
            Arc::new(FileConfigStore::new(realm_paths.config_path.clone()));
        let config_store: Arc<dyn ConfigStore> = Arc::new(meerkat_core::TaggedConfigStore::new(
            base_config_store,
            meerkat_core::ConfigStoreMetadata {
                realm_id: Some(realm_id.clone()),
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
            &realm_id,
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
                meerkat_store::RealmBackend::Sqlite | meerkat_store::RealmBackend::Redb => {
                    realm_paths.root.clone()
                }
            });

        let enable_builtins = config.tools.builtins_enabled;
        let enable_shell = config.tools.shell_enabled;

        let default_model = Cow::Owned(config.agent.model.clone());
        let max_tokens = config.agent.max_tokens_per_turn;
        let rest_host = Cow::Owned(config.rest.host.clone());
        let rest_port = config.rest.port;

        let mut factory = AgentFactory::new(store_path.clone())
            .session_store(session_store.clone())
            .runtime_root(realm_paths.root.clone())
            .builtins(enable_builtins)
            .shell(enable_shell);
        let conventions_context_root = bootstrap.context.context_root.clone();
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

        let skill_runtime = factory.build_skill_runtime(&config).await;

        let builder =
            FactoryAgentBuilder::new_with_config_store(factory, config, Arc::clone(&config_store));
        let runtime_adapter = persistence.runtime_adapter();
        let (session_store, runtime_store) = persistence.into_parts();
        let session_service = Arc::new(PersistentSessionService::new(
            builder,
            100,
            session_store,
            runtime_store,
        ));
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
            llm_client_override: None,
            config_store,
            event_tx,
            session_service,
            webhook_auth: webhook::WebhookAuth::from_env(),
            realm_id,
            instance_id,
            backend: manifest.backend.as_str().to_string(),
            resolved_paths,
            expose_paths,
            config_runtime,
            realm_lease: Arc::new(tokio::sync::Mutex::new(Some(lease))),
            skill_runtime,
            runtime_adapter,
            #[cfg(feature = "mob")]
            mob_state: Arc::new(meerkat_mob_mcp::MobMcpState::new(mob_session_service)),
            #[cfg(feature = "mcp")]
            mcp_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        })
    }

    fn runtime_executor_context(&self) -> RestRuntimeExecutorContext {
        RestRuntimeExecutorContext {
            default_model: self.default_model.clone(),
            max_tokens: self.max_tokens,
            enable_builtins: self.enable_builtins,
            enable_shell: self.enable_shell,
            llm_client_override: self.llm_client_override.clone(),
            event_tx: self.event_tx.clone(),
            session_service: self.session_service.clone(),
            realm_id: self.realm_id.clone(),
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

fn extract_runtime_prompt(primitive: &RunPrimitive) -> ContentInput {
    match primitive {
        RunPrimitive::StagedInput(staged) => {
            let mut all_blocks = Vec::new();
            for append in &staged.appends {
                match &append.content {
                    CoreRenderable::Text { text } => {
                        all_blocks
                            .push(meerkat_core::types::ContentBlock::Text { text: text.clone() });
                    }
                    CoreRenderable::Blocks { blocks } => {
                        all_blocks.extend(blocks.iter().cloned());
                    }
                    _ => {}
                }
            }
            if all_blocks.len() == 1
                && let meerkat_core::types::ContentBlock::Text { text } = &all_blocks[0]
            {
                return ContentInput::Text(text.clone());
            }
            if all_blocks.is_empty() {
                ContentInput::Text(String::new())
            } else {
                ContentInput::Blocks(all_blocks)
            }
        }
        RunPrimitive::ImmediateAppend(append) => match &append.content {
            CoreRenderable::Text { text } => ContentInput::Text(text.clone()),
            CoreRenderable::Blocks { blocks } => ContentInput::Blocks(blocks.clone()),
            _ => ContentInput::Text(String::new()),
        },
        RunPrimitive::ImmediateContextAppend(append) => match &append.content {
            CoreRenderable::Text { text } => ContentInput::Text(text.clone()),
            CoreRenderable::Blocks { blocks } => ContentInput::Blocks(blocks.clone()),
            _ => ContentInput::Text(String::new()),
        },
        _ => ContentInput::Text(String::new()),
    }
}

async fn apply_runtime_turn(
    context: &RestRuntimeExecutorContext,
    session_id: &SessionId,
    run_id: meerkat_core::lifecycle::RunId,
    primitive: &RunPrimitive,
    prompt: ContentInput,
) -> Result<CoreApplyOutput, SessionError> {
    if let RunPrimitive::StagedInput(staged) = primitive
        && staged.appends.is_empty()
        && !staged.context_appends.is_empty()
        && staged.boundary == RunApplyBoundary::Immediate
    {
        return context
            .session_service
            .apply_runtime_context_appends(
                session_id,
                run_id,
                staged.context_appends.clone(),
                staged.contributing_input_ids.clone(),
            )
            .await;
    }

    let (event_tx, event_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(100);
    let forwarder = spawn_event_forwarder(
        event_rx,
        context.event_tx.clone(),
        session_id.clone(),
        false,
    );
    let host_mode = match primitive
        .turn_metadata()
        .and_then(|metadata| metadata.host_mode)
    {
        Some(host_mode) => host_mode,
        None => context
            .session_service
            .load_persisted(session_id)
            .await
            .ok()
            .flatten()
            .and_then(|session| {
                session
                    .session_metadata()
                    .map(|metadata| metadata.host_mode)
            })
            .unwrap_or(false),
    };

    let svc_req = SvcStartTurnRequest {
        prompt: prompt.clone(),
        render_metadata: None,
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        event_tx: Some(event_tx.clone()),
        host_mode,
        host_mode_owner: HostModeOwner::ExternalRuntime,
        skill_references: primitive
            .turn_metadata()
            .and_then(|meta| meta.skill_references.clone()),
        flow_tool_overlay: primitive
            .turn_metadata()
            .and_then(|meta| meta.flow_tool_overlay.clone()),
        additional_instructions: primitive
            .turn_metadata()
            .and_then(|meta| meta.additional_instructions.clone()),
    };

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
                .load_persisted(session_id)
                .await?
                .ok_or(SessionError::NotFound {
                    id: session_id.clone(),
                })?;
            let stored_metadata = session.session_metadata();
            let tooling = stored_metadata
                .as_ref()
                .map(|meta| meta.tooling.clone())
                .unwrap_or(SessionTooling {
                    builtins: context.enable_builtins,
                    shell: context.enable_shell,
                    comms: false,
                    mob: false,
                    memory: false,
                    active_skills: None,
                });
            let current_generation = context
                .config_runtime
                .get()
                .await
                .ok()
                .map(|s| s.generation);
            let build = SessionBuildOptions {
                provider: stored_metadata.as_ref().map(|meta| meta.provider),
                output_schema: None,
                structured_output_retries: 2,
                hooks_override: HookRunOverrides::default(),
                comms_name: stored_metadata
                    .as_ref()
                    .and_then(|meta| meta.comms_name.clone()),
                peer_meta: stored_metadata
                    .as_ref()
                    .and_then(|meta| meta.peer_meta.clone()),
                resume_session: Some(session),
                budget_limits: None,
                provider_params: None,
                external_tools: None,
                llm_client_override: context
                    .llm_client_override
                    .clone()
                    .map(encode_llm_client_override_for_service),
                override_builtins: Some(tooling.builtins),
                override_shell: Some(tooling.shell),
                override_memory: Some(tooling.memory),
                override_mob: Some(tooling.mob),
                preload_skills: tooling.active_skills.clone(),
                realm_id: stored_metadata
                    .as_ref()
                    .and_then(|meta| meta.realm_id.clone())
                    .or_else(|| Some(context.realm_id.clone())),
                instance_id: stored_metadata
                    .as_ref()
                    .and_then(|meta| meta.instance_id.clone())
                    .or_else(|| context.instance_id.clone()),
                backend: stored_metadata
                    .as_ref()
                    .and_then(|meta| meta.backend.clone())
                    .or_else(|| Some(context.backend.clone())),
                config_generation: current_generation,
                checkpointer: None,
                silent_comms_intents: Vec::new(),
                max_inline_peer_notifications: None,
                app_context: None,
                additional_instructions: None,
                shell_env: None,
            };
            let create_req = SvcCreateSessionRequest {
                model: stored_metadata.as_ref().map_or_else(
                    || context.default_model.to_string(),
                    |meta| meta.model.clone(),
                ),
                prompt,
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(
                    stored_metadata
                        .as_ref()
                        .map_or(context.max_tokens, |meta| meta.max_tokens),
                ),
                event_tx: None,
                host_mode: stored_metadata.as_ref().is_some_and(|meta| meta.host_mode),
                host_mode_owner: HostModeOwner::ExternalRuntime,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                build: Some(build),
                labels: None,
            };

            context.session_service.create_session(create_req).await?;
            let (_, output) = context
                .session_service
                .apply_runtime_turn_with_result(
                    session_id,
                    run_id,
                    SvcStartTurnRequest {
                        prompt: extract_runtime_prompt(primitive),
                        render_metadata: None,
                        handling_mode: meerkat_core::types::HandlingMode::Queue,
                        event_tx: Some(event_tx.clone()),
                        host_mode: primitive
                            .turn_metadata()
                            .and_then(|meta| meta.host_mode)
                            .unwrap_or(false),
                        host_mode_owner: HostModeOwner::ExternalRuntime,
                        skill_references: primitive
                            .turn_metadata()
                            .and_then(|meta| meta.skill_references.clone()),
                        flow_tool_overlay: primitive
                            .turn_metadata()
                            .and_then(|meta| meta.flow_tool_overlay.clone()),
                        additional_instructions: primitive
                            .turn_metadata()
                            .and_then(|meta| meta.additional_instructions.clone()),
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
        let prompt = extract_runtime_prompt(&primitive);

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
        "redb" => Some(RealmBackend::Redb),
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

fn resolve_host_mode(requested: bool) -> Result<bool, ApiError> {
    meerkat::surface::resolve_host_mode(requested).map_err(ApiError::BadRequest)
}

fn validate_public_peer_meta(peer_meta: Option<&meerkat_core::PeerMeta>) -> Result<(), ApiError> {
    meerkat::surface::validate_public_peer_meta(peer_meta).map_err(ApiError::BadRequest)
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
    /// Max retries for structured output validation (default: 2).
    #[serde(default = "default_structured_output_retries")]
    pub structured_output_retries: u32,
    /// Enable verbose event logging (server-side).
    #[serde(default)]
    pub verbose: bool,
    /// Run in host mode: process prompt then stay alive listening for comms messages.
    /// Requires comms_name to be set.
    #[serde(default)]
    pub host_mode: bool,
    /// Agent name for inter-agent communication. Required for host_mode.
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
    pub preload_skills: Option<Vec<String>>,
    /// Structured refs for per-turn skill injection.
    #[serde(default)]
    pub skill_refs: Option<Vec<meerkat_core::skills::SkillRef>>,
    /// Legacy compatibility skill refs for per-turn injection.
    #[serde(default)]
    pub skill_references: Option<Vec<String>>,
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

async fn canonical_skill_keys_for_state(
    state: &AppState,
    skill_refs: Option<Vec<meerkat_core::skills::SkillRef>>,
    skill_references: Option<Vec<String>>,
) -> Result<Option<Vec<meerkat_core::skills::SkillKey>>, ApiError> {
    let params = SkillsParams {
        preload_skills: None,
        skill_refs,
        skill_references,
    };

    let snapshot = state
        .config_runtime
        .get()
        .await
        .map_err(config_runtime_err_to_api)?;
    let registry = snapshot
        .config
        .skills
        .build_source_identity_registry()
        .map_err(|e| ApiError::BadRequest(format!("Invalid skill_refs: {e}")))?;

    params
        .canonical_skill_keys_with_registry(&registry)
        .map_err(|e| ApiError::BadRequest(format!("Invalid skill_refs: {e}")))
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
    /// Max retries for structured output validation (default: 2).
    #[serde(default = "default_structured_output_retries")]
    pub structured_output_retries: u32,
    /// Run in host mode: process prompt then stay alive listening for comms messages.
    #[serde(default)]
    pub host_mode: bool,
    /// Agent name for inter-agent communication. Required for host_mode.
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
    /// Legacy compatibility refs for per-turn skill injection.
    #[serde(default)]
    pub skill_references: Option<Vec<String>>,
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

/// API error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: String,
}

/// Build the REST API router
pub fn router(state: AppState) -> Router {
    let r = Router::new()
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/sessions/{id}", get(get_session).delete(archive_session))
        .route("/sessions/{id}/history", get(get_session_history))
        .route("/sessions/{id}/interrupt", post(interrupt_session))
        .route("/sessions/{id}/system_context", post(append_system_context))
        .route("/sessions/{id}/messages", post(continue_session))
        .route("/sessions/{id}/external-events", post(post_external_event))
        .route("/sessions/{id}/events", get(session_events))
        .route("/comms/send", post(comms_send))
        .route("/comms/peers", get(comms_peers))
        .route(
            "/config",
            get(get_config).put(set_config).patch(patch_config),
        )
        .route("/health", get(health_check))
        .route("/skills", get(list_skills))
        .route("/skills/{id}", get(inspect_skill))
        .route("/capabilities", get(get_capabilities))
        .route("/models/catalog", get(get_models_catalog))
        // v9 runtime/input endpoints
        .route("/runtime/{id}/state", get(runtime_state))
        .route("/runtime/{id}/accept", post(runtime_accept))
        .route("/runtime/{id}/retire", post(runtime_retire))
        .route("/runtime/{id}/reset", post(runtime_reset))
        .route("/input/{id}/list", get(input_list))
        .route("/input/{session_id}/{input_id}", get(input_state));

    #[cfg(feature = "mob")]
    let r = r
        .route("/mob/prefabs", get(mob_prefabs))
        .route("/mob/tools", get(mob_tools))
        .route("/mob/call", post(mob_call))
        .route("/mob/{id}/events", get(mob_event_stream))
        .route("/mob/{id}/spawn-helper", post(mob_spawn_helper))
        .route("/mob/{id}/fork-helper", post(mob_fork_helper))
        .route(
            "/mob/{id}/members/{meerkat_id}/status",
            get(mob_member_status),
        )
        .route(
            "/mob/{id}/members/{meerkat_id}/cancel",
            post(mob_force_cancel),
        )
        .route(
            "/mob/{id}/members/{meerkat_id}/respawn",
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
fn get_runtime_adapter(state: &AppState) -> &Arc<meerkat_runtime::RuntimeSessionAdapter> {
    &state.runtime_adapter
}

fn session_metadata_marks_archived(session: &Session) -> bool {
    session
        .metadata()
        .get("session_archived")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

async fn ensure_runtime_session_registered(
    state: &AppState,
    session_id: &SessionId,
) -> Result<(), Response> {
    let adapter = get_runtime_adapter(state);

    let persisted = state
        .session_service
        .load_persisted(session_id)
        .await
        .ok()
        .flatten();
    if persisted
        .as_ref()
        .is_some_and(session_metadata_marks_archived)
    {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("Session not found: {session_id}")})),
        )
            .into_response());
    }

    let session_exists = if persisted.is_some() {
        true
    } else {
        state.session_service.read(session_id).await.is_ok()
    };

    if !session_exists {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("Session not found: {session_id}")})),
        )
            .into_response());
    }

    let executor = Box::new(RestSessionRuntimeExecutor::new(
        state.runtime_executor_context(),
        session_id.clone(),
    ));
    adapter
        .ensure_session_with_executor(session_id.clone(), executor)
        .await;
    Ok(())
}

/// GET /runtime/{id}/state
async fn runtime_state(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    let sid = SessionId::parse(&id).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
        )
            .into_response()
    })?;
    ensure_runtime_session_registered(&state, &sid).await?;
    let adapter = get_runtime_adapter(&state);
    let rs = adapter.runtime_state(&sid).await.map_err(|e| {
        (StatusCode::NOT_FOUND, Json(json!({"error": e.to_string()}))).into_response()
    })?;
    Ok(Json(json!({"session_id": sid.to_string(), "state": rs})))
}

/// POST /runtime/{id}/accept
async fn runtime_accept(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Response> {
    let sid = SessionId::parse(&id).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
        )
            .into_response()
    })?;
    ensure_runtime_session_registered(&state, &sid).await?;
    let adapter = get_runtime_adapter(&state);
    let input: meerkat_runtime::Input = serde_json::from_value(body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
        )
            .into_response()
    })?;
    let outcome = match adapter.accept_input(&sid, input).await {
        Ok(outcome) => outcome,
        Err(meerkat_runtime::RuntimeDriverError::NotReady { state }) => {
            meerkat_runtime::AcceptOutcome::Rejected {
                reason: format!("runtime not accepting input while in state: {state}"),
            }
        }
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response());
        }
    };
    Ok(Json(serde_json::to_value(outcome).unwrap_or_default()))
}

/// POST /runtime/{id}/retire
async fn runtime_retire(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    let sid = SessionId::parse(&id).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
        )
            .into_response()
    })?;
    ensure_runtime_session_registered(&state, &sid).await?;
    let adapter = get_runtime_adapter(&state);
    let report = adapter.retire_runtime(&sid).await.map_err(|e| {
        (StatusCode::NOT_FOUND, Json(json!({"error": e.to_string()}))).into_response()
    })?;
    Ok(Json(json!({"inputs_abandoned": report.inputs_abandoned})))
}

/// POST /runtime/{id}/reset
async fn runtime_reset(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    let sid = SessionId::parse(&id).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
        )
            .into_response()
    })?;
    ensure_runtime_session_registered(&state, &sid).await?;
    let adapter = get_runtime_adapter(&state);
    let report = adapter.reset_runtime(&sid).await.map_err(|e| {
        (StatusCode::NOT_FOUND, Json(json!({"error": e.to_string()}))).into_response()
    })?;
    Ok(Json(json!({"inputs_abandoned": report.inputs_abandoned})))
}

/// GET /input/{id}/list
async fn input_list(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    let sid = SessionId::parse(&id).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
        )
            .into_response()
    })?;
    ensure_runtime_session_registered(&state, &sid).await?;
    let adapter = get_runtime_adapter(&state);
    let ids = adapter.list_active_inputs(&sid).await.map_err(|e| {
        (StatusCode::NOT_FOUND, Json(json!({"error": e.to_string()}))).into_response()
    })?;
    let id_strs: Vec<String> = ids.iter().map(ToString::to_string).collect();
    Ok(Json(json!({"input_ids": id_strs})))
}

/// GET /input/{session_id}/{input_id}
async fn input_state(
    State(state): State<AppState>,
    Path((session_id_str, input_id_str)): Path<(String, String)>,
) -> Result<Json<Value>, Response> {
    let sid = SessionId::parse(&session_id_str).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
        )
            .into_response()
    })?;
    ensure_runtime_session_registered(&state, &sid).await?;
    let adapter = get_runtime_adapter(&state);
    let input_uuid = uuid::Uuid::parse_str(&input_id_str).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": e.to_string()})),
        )
            .into_response()
    })?;
    let input_id = meerkat_core::lifecycle::InputId::from_uuid(input_uuid);
    let is = adapter.input_state(&sid, &input_id).await.map_err(|e| {
        (StatusCode::NOT_FOUND, Json(json!({"error": e.to_string()}))).into_response()
    })?;
    match is {
        Some(s) => Ok(Json(serde_json::to_value(s).unwrap_or_default())),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "input not found"})),
        )
            .into_response()),
    }
}

// ---------------------------------------------------------------------------

/// Health check endpoint
async fn health_check() -> &'static str {
    "ok"
}

/// GET /mob/prefabs — list built-in mob prefab templates.
#[cfg(feature = "mob")]
async fn mob_prefabs() -> Json<Value> {
    let prefabs: Vec<Value> = meerkat_mob::Prefab::all()
        .into_iter()
        .map(|prefab| {
            json!({
                "key": prefab.key(),
                "toml_template": prefab.toml_template(),
            })
        })
        .collect();
    Json(json!({ "prefabs": prefabs }))
}

/// GET /mob/tools — list protocol-callable mob lifecycle tools.
#[cfg(feature = "mob")]
async fn mob_tools() -> Json<Value> {
    Json(json!({ "tools": meerkat_mob_mcp::tools_list() }))
}

#[derive(Debug, Deserialize)]
#[cfg(feature = "mob")]
struct MobCallRequest {
    name: String,
    #[serde(default)]
    arguments: Value,
}

/// POST /mob/call — invoke a mob lifecycle tool.
#[cfg(feature = "mob")]
async fn mob_call(
    State(state): State<AppState>,
    Json(req): Json<MobCallRequest>,
) -> Result<Json<Value>, ApiError> {
    let result = meerkat_mob_mcp::handle_tools_call(&state.mob_state, &req.name, &req.arguments)
        .await
        .map_err(|e| ApiError::BadRequest(e.message))?;
    Ok(Json(result))
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
/// If `?member=<meerkat_id>` is provided, streams that member's session
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
            let meerkat_id = meerkat_mob::MeerkatId::from(member.as_str());
            let mut event_stream = state
                .mob_state
                .subscribe_agent_events(&mob_id, &meerkat_id)
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
    meerkat_id: Option<String>,
    #[serde(default)]
    profile_name: Option<String>,
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
    let meerkat_id = meerkat_mob::MeerkatId::from(
        req.meerkat_id
            .unwrap_or_else(|| format!("helper-{}", uuid::Uuid::new_v4())),
    );
    let mut options = meerkat_mob::HelperOptions::default();
    if let Some(profile) = req.profile_name {
        options.profile_name = Some(meerkat_mob::ProfileName::from(profile));
    }
    options.runtime_mode = req.runtime_mode;
    options.backend = req.backend;
    let result = state
        .mob_state
        .mob_spawn_helper(&mob_id, meerkat_id, req.prompt, options)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(json!({
        "output": result.output,
        "tokens_used": result.tokens_used,
        "session_id": result.session_id,
    })))
}

#[derive(Debug, Deserialize)]
#[cfg(feature = "mob")]
struct ForkHelperRequest {
    source_member_id: String,
    prompt: String,
    #[serde(default)]
    meerkat_id: Option<String>,
    #[serde(default)]
    profile_name: Option<String>,
    #[serde(default)]
    fork_context: Option<meerkat_mob::ForkContext>,
    #[serde(default)]
    runtime_mode: Option<meerkat_mob::MobRuntimeMode>,
    #[serde(default)]
    backend: Option<meerkat_mob::MobBackendKind>,
}

/// POST /mob/{id}/fork-helper — fork from a member's context, wait, return result.
#[cfg(feature = "mob")]
async fn mob_fork_helper(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ForkHelperRequest>,
) -> Result<Json<Value>, ApiError> {
    let mob_id = meerkat_mob::MobId::from(id.as_str());
    let source_member_id = meerkat_mob::MeerkatId::from(req.source_member_id.as_str());
    let meerkat_id = meerkat_mob::MeerkatId::from(
        req.meerkat_id
            .unwrap_or_else(|| format!("fork-{}", uuid::Uuid::new_v4())),
    );
    let fork_context = req
        .fork_context
        .unwrap_or(meerkat_mob::ForkContext::FullHistory);
    let mut options = meerkat_mob::HelperOptions::default();
    if let Some(profile) = req.profile_name {
        options.profile_name = Some(meerkat_mob::ProfileName::from(profile));
    }
    options.runtime_mode = req.runtime_mode;
    options.backend = req.backend;
    let result = state
        .mob_state
        .mob_fork_helper(
            &mob_id,
            &source_member_id,
            meerkat_id,
            req.prompt,
            fork_context,
            options,
        )
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(json!({
        "output": result.output,
        "tokens_used": result.tokens_used,
        "session_id": result.session_id,
    })))
}

/// GET /mob/{id}/members/{meerkat_id}/status — member execution snapshot.
#[cfg(feature = "mob")]
async fn mob_member_status(
    State(state): State<AppState>,
    Path((id, meerkat_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let mob_id = meerkat_mob::MobId::from(id.as_str());
    let meerkat_id = meerkat_mob::MeerkatId::from(meerkat_id.as_str());
    let snapshot = state
        .mob_state
        .mob_member_status(&mob_id, &meerkat_id)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(json!(snapshot)))
}

/// POST /mob/{id}/members/{meerkat_id}/cancel — force-cancel in-flight turn.
#[cfg(feature = "mob")]
async fn mob_force_cancel(
    State(state): State<AppState>,
    Path((id, meerkat_id)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let mob_id = meerkat_mob::MobId::from(id.as_str());
    let meerkat_id = meerkat_mob::MeerkatId::from(meerkat_id.as_str());
    state
        .mob_state
        .mob_force_cancel(&mob_id, meerkat_id)
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(json!({"cancelled": true})))
}

/// POST /mob/{id}/members/{meerkat_id}/respawn — retire + respawn member.
#[cfg(feature = "mob")]
async fn mob_member_respawn(
    State(state): State<AppState>,
    Path((id, meerkat_id)): Path<(String, String)>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ApiError> {
    let mob_id = meerkat_mob::MobId::from(id.as_str());
    let meerkat_id_val = meerkat_mob::MeerkatId::from(meerkat_id.as_str());
    let initial_message =
        body.and_then(|Json(v)| v.get("initial_message")?.as_str().map(String::from));
    match state
        .mob_state
        .mob_respawn(&mob_id, meerkat_id_val, initial_message)
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
            "failed_peer_ids": failed_peer_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        }))),
        Err(e) => Err(ApiError::BadRequest(e.to_string())),
    }
}

/// Canonical comms send request body.
#[derive(Debug, Deserialize)]
pub struct CommsSendRequest {
    pub session_id: String,
    pub kind: String,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub stream: Option<String>,
    #[serde(default)]
    pub allow_self_session: Option<bool>,
    #[serde(default)]
    pub handling_mode: Option<String>,
}

/// Canonical comms peers request body.
#[derive(Debug, Deserialize)]
pub struct CommsPeersRequest {
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
struct InspectSkillQuery {
    #[serde(default)]
    source: Option<String>,
}

/// POST /comms/send — dispatch a canonical comms command.
async fn comms_send(
    State(state): State<AppState>,
    Json(req): Json<CommsSendRequest>,
) -> Result<Json<Value>, ApiError> {
    use meerkat_core::comms::SendReceipt;

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

    let cmd = build_comms_command(&req, &session_id)?;

    match comms.send(cmd).await {
        Ok(receipt) => {
            let result = match receipt {
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
                SendReceipt::PeerRequestSent {
                    envelope_id,
                    interaction_id,
                    stream_reserved,
                } => json!({
                    "kind": "peer_request_sent",
                    "envelope_id": envelope_id.to_string(),
                    "interaction_id": interaction_id.0.to_string(),
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
            };
            Ok(Json(result))
        }
        Err(e) => Err(ApiError::Internal(e.to_string())),
    }
}

fn build_comms_command(
    req: &CommsSendRequest,
    session_id: &SessionId,
) -> Result<meerkat_core::comms::CommsCommand, ApiError> {
    let request = meerkat_core::comms::CommsCommandRequest {
        kind: req.kind.clone(),
        to: req.to.clone(),
        body: req.body.clone(),
        blocks: None,
        intent: req.intent.clone(),
        params: req.params.clone(),
        in_reply_to: req.in_reply_to.clone(),
        status: req.status.clone(),
        result: req.result.clone(),
        source: req.source.clone(),
        stream: req.stream.clone(),
        allow_self_session: req.allow_self_session,
        handling_mode: req.handling_mode.clone(),
    };

    request.parse(session_id).map_err(|errors| {
        let json_errors =
            meerkat_core::comms::CommsCommandRequest::validation_errors_to_json(&errors);
        let msg = if let Some(first) = json_errors.first() {
            let field = first["field"].as_str().unwrap_or("command");
            let issue = first["issue"].as_str().unwrap_or("invalid");
            let got = first["got"].as_str();
            match (field, issue) {
                ("source", "invalid_value") => got.map_or_else(
                    || "invalid source".to_string(),
                    |got| format!("invalid source: {got}"),
                ),
                ("stream", "invalid_value") => got.map_or_else(
                    || "invalid stream".to_string(),
                    |got| format!("invalid stream: {got}"),
                ),
                ("status", "invalid_value") => got.map_or_else(
                    || "invalid status".to_string(),
                    |got| format!("invalid status: {got}"),
                ),
                ("kind", "unknown_kind") => got.map_or_else(
                    || "unknown kind".to_string(),
                    |got| format!("unknown kind: {got}"),
                ),
                ("body", "required_field") => "input requires 'body'".to_string(),
                ("to", "required_field") => "to is required".to_string(),
                ("intent", "required_field") => "peer_request requires 'intent'".to_string(),
                ("in_reply_to", "required_field") => {
                    "peer_response requires 'in_reply_to'".to_string()
                }
                ("in_reply_to", "invalid_uuid") => got.map_or_else(
                    || "invalid UUID".to_string(),
                    |got| format!("invalid UUID: {got}"),
                ),
                ("to", "invalid_value") => got.map_or_else(
                    || "invalid peer name".to_string(),
                    |got| format!("invalid to: {got}"),
                ),
                _ => issue.to_string(),
            }
        } else {
            "invalid command".to_string()
        };
        ApiError::BadRequest(msg)
    })
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

    let peers = comms.peers().await;
    let entries: Vec<Value> = peers
        .iter()
        .map(|p| {
            json!({
                "name": p.name.to_string(),
                "peer_id": p.peer_id,
                "address": p.address,
                "source": format!("{:?}", p.source),
                "sendable_kinds": p.sendable_kinds,
                "capabilities": p.capabilities,
                "meta": p.meta,
            })
        })
        .collect();

    Ok(Json(json!({ "peers": entries })))
}

fn make_runtime_external_event_input(
    source_name: &str,
    event_type: &str,
    payload: Value,
) -> meerkat_runtime::Input {
    meerkat_runtime::Input::ExternalEvent(meerkat_runtime::ExternalEventInput {
        header: meerkat_runtime::InputHeader {
            id: meerkat_core::lifecycle::InputId::new(),
            timestamp: chrono::Utc::now(),
            source: meerkat_runtime::InputOrigin::External {
                source_name: source_name.to_string(),
            },
            durability: meerkat_runtime::InputDurability::Durable,
            visibility: meerkat_runtime::InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        event_type: event_type.to_string(),
        payload,
    })
}

/// Admit an external event to a session through the runtime-backed path.
///
/// Authentication is controlled by `RKAT_WEBHOOK_SECRET` env var:
/// - If not set: no authentication (suitable for localhost/dev)
/// - If set: requires `X-Webhook-Secret` header with matching value
async fn post_external_event(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<Value>,
) -> Result<(StatusCode, Json<Value>), Response> {
    // Webhook auth resolved once at startup, stored in AppState.
    webhook::verify_webhook(&headers, &state.webhook_auth)
        .map_err(|msg| ApiError::Unauthorized(msg.to_string()).into_response())?;

    let session_id =
        resolve_session_id_for_state(&id, &state).map_err(IntoResponse::into_response)?;
    ensure_runtime_session_registered(&state, &session_id).await?;

    let input = make_runtime_external_event_input("webhook", "webhook", payload);

    match state.runtime_adapter.accept_input(&session_id, input).await {
        Ok(
            meerkat_runtime::AcceptOutcome::Accepted { .. }
            | meerkat_runtime::AcceptOutcome::Deduplicated { .. },
        ) => Ok((StatusCode::ACCEPTED, Json(json!({"queued": true})))),
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
            Json(json!({"error": format!("runtime not accepting input while in state: {state}")})),
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

    let wire: Vec<meerkat_contracts::SkillEntry> = entries
        .iter()
        .map(|e| meerkat_contracts::SkillEntry {
            id: e.descriptor.id.0.clone(),
            name: e.descriptor.name.clone(),
            description: e.descriptor.description.clone(),
            scope: e.descriptor.scope.to_string(),
            source: e.descriptor.source_name.clone(),
            is_active: e.is_active,
            shadowed_by: e.shadowed_by.clone(),
        })
        .collect();
    Ok(Json(meerkat_contracts::SkillListResponse { skills: wire }))
}

/// Inspect a skill by ID.
async fn inspect_skill(
    State(state): State<AppState>,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<InspectSkillQuery>,
) -> Result<Json<meerkat_contracts::SkillInspectResponse>, ApiError> {
    let runtime = state
        .skill_runtime
        .as_ref()
        .ok_or_else(|| ApiError::NotFound("skills not enabled".into()))?;

    let doc = runtime
        .load_from_source(
            &meerkat_core::skills::SkillId::from(id.as_str()),
            query.source.as_deref(),
        )
        .await
        .map_err(|e| ApiError::NotFound(format!("skill inspect failed: {e}")))?;

    Ok(Json(meerkat_contracts::SkillInspectResponse {
        id: doc.descriptor.id.0.clone(),
        name: doc.descriptor.name.clone(),
        description: doc.descriptor.description.clone(),
        scope: doc.descriptor.scope.to_string(),
        source: doc.descriptor.source_name.clone(),
        body: doc.body,
    }))
}

/// Get runtime capabilities with status resolved against config.
async fn get_capabilities(
    State(state): State<AppState>,
) -> Json<meerkat_contracts::CapabilitiesResponse> {
    let config = state.config_store.get().await.unwrap_or_default();
    Json(meerkat::surface::build_capabilities_response(&config))
}

/// Get the compiled-in model catalog.
async fn get_models_catalog() -> Json<meerkat_contracts::ModelsCatalogResponse> {
    Json(meerkat::surface::build_models_catalog_response())
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
    validate_config_for_commit(&config)?;
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
    validate_config_for_commit(&preview)?;
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

fn validate_config_for_commit(config: &Config) -> Result<(), ApiError> {
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
    realm_id: &str,
) -> SessionResponse {
    let mut response: SessionResponse = result.into();
    response.session_ref = Some(format_session_ref(realm_id, &response.session_id));
    response
}

fn resolve_session_id_for_state(input: &str, state: &AppState) -> Result<SessionId, ApiError> {
    let locator = SessionLocator::parse(input)
        .map_err(|e| ApiError::BadRequest(format!("Invalid session locator '{input}': {e}")))?;
    if let Some(realm) = locator.realm_id.as_deref()
        && realm != state.realm_id
    {
        return Err(ApiError::BadRequest(format!(
            "Session locator realm '{}' does not match active realm '{}'",
            realm, state.realm_id
        )));
    }
    Ok(locator.session_id)
}

/// Create and run a new session
async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<SessionResponse>, ApiError> {
    validate_public_peer_meta(req.peer_meta.as_ref())?;
    let host_mode = resolve_host_mode(req.host_mode)?;
    let model = req.model.unwrap_or_else(|| state.default_model.clone());
    let max_tokens = req.max_tokens.unwrap_or(state.max_tokens);
    let skill_references = canonical_skill_keys_for_state(
        &state,
        req.skill_refs.clone(),
        req.skill_references.clone(),
    )
    .await?;

    // Pre-create a session to claim the session_id (needed for CallbackPending handling
    // and event forwarding before the service call returns).
    let pre_session = Session::new();
    let session_id = pre_session.id().clone();

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

    let current_generation = state.config_runtime.get().await.ok().map(|s| s.generation);
    let build = SessionBuildOptions {
        provider: req.provider,
        output_schema: req.output_schema,
        structured_output_retries: req.structured_output_retries,
        hooks_override: req.hooks_override.unwrap_or_default(),
        comms_name: req.comms_name.clone(),
        peer_meta: req.peer_meta.clone(),
        resume_session: Some(pre_session),
        budget_limits: req.budget_limits,
        provider_params: req.provider_params,
        external_tools: mcp_external_tools,
        llm_client_override: state
            .llm_client_override
            .clone()
            .map(encode_llm_client_override_for_service),
        override_builtins: req.enable_builtins,
        override_shell: req.enable_shell,
        override_memory: req.enable_memory,
        override_mob: req.enable_mob,
        preload_skills: req
            .preload_skills
            .map(|ids| ids.into_iter().map(meerkat_core::skills::SkillId).collect()),
        realm_id: Some(state.realm_id.clone()),
        instance_id: state.instance_id.clone(),
        backend: Some(state.backend.clone()),
        config_generation: current_generation,
        checkpointer: None,
        silent_comms_intents: Vec::new(),
        max_inline_peer_notifications: None,
        app_context: req.app_context,
        additional_instructions: req.additional_instructions,
        shell_env: req.shell_env,
    };

    let svc_req = SvcCreateSessionRequest {
        model: model.to_string(),
        prompt: req.prompt.clone(),
        render_metadata: None,
        system_prompt: req.system_prompt,
        max_tokens: Some(max_tokens),
        event_tx: Some(caller_event_tx.clone()),
        host_mode,
        host_mode_owner: HostModeOwner::ExternalRuntime,
        skill_references: skill_references.clone(),
        initial_turn: InitialTurnPolicy::Defer,
        build: Some(build),
        labels: req.labels,
    };

    let adapter = state.runtime_adapter.clone();

    // Create session with Defer, then route through runtime
    let create_result = state
        .session_service
        .create_session(svc_req)
        .await
        .map_err(|err| match err {
            SessionError::NotFound { .. } => ApiError::NotFound(err.to_string()),
            SessionError::Busy { .. } => ApiError::BadRequest(err.to_string()),
            _ => ApiError::Agent(err.to_string()),
        })?;

    // Register executor for the new session
    if let Err(_resp) = ensure_runtime_session_registered(&state, &create_result.session_id).await {
        drop(caller_event_tx);
        drain_event_forwarder(&session_id, forward_task).await;
        #[cfg(feature = "mcp")]
        cleanup_mcp_session(&state, &session_id).await;
        return Err(ApiError::Internal(
            "failed to register runtime executor".to_string(),
        ));
    }

    // Spawn comms drain for host_mode sessions.
    #[cfg(feature = "comms")]
    {
        let comms_rt = state.session_service.comms_runtime(&session_id).await;
        let control = state.session_service.comms_drain_control(&session_id).await;
        adapter.set_comms_drain_control(&session_id, control).await;
        adapter
            .maybe_spawn_comms_drain(&session_id, host_mode, comms_rt)
            .await;
    }

    // Create input and route through runtime
    let input = meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::from_content_input(
        req.prompt,
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                host_mode: if host_mode { Some(true) } else { None },
                skill_references,
                flow_tool_overlay: None,
                additional_instructions: None,
                ..Default::default()
            },
        ),
    ));

    let (outcome, handle) = adapter
        .accept_input_with_completion(&create_result.session_id, input)
        .await
        .map_err(|err| ApiError::Internal(err.to_string()))?;

    let result = match handle {
        Some(handle) => match handle.wait().await {
            meerkat_runtime::completion::CompletionOutcome::Completed(run_result) => Ok(run_result),
            meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult => Err(
                ApiError::Internal("turn completed without result".to_string()),
            ),
            meerkat_runtime::completion::CompletionOutcome::Abandoned(reason) => {
                Err(ApiError::Internal(format!("turn abandoned: {reason}")))
            }
            meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated(reason) => {
                Err(ApiError::Internal(format!("runtime terminated: {reason}")))
            }
        },
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
        Ok(run_result) => Ok(Json(run_result_to_response(run_result, &state.realm_id))),
        Err(err) => {
            // Clean up MCP adapter state on session creation failure.
            #[cfg(feature = "mcp")]
            cleanup_mcp_session(&state, &session_id).await;
            #[cfg(feature = "comms")]
            state.runtime_adapter.abort_comms_drain(&session_id).await;
            Err(err)
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
            let session_ref = format_session_ref(&state.realm_id, &s.session_id);
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
        session_ref: format_session_ref(&state.realm_id, &view.state.session_id),
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
    wire.session_ref = Some(format_session_ref(&state.realm_id, &session_id));
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

/// Continue an existing session
async fn continue_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ContinueSessionRequest>,
) -> Result<Json<SessionResponse>, ApiError> {
    validate_public_peer_meta(req.peer_meta.as_ref())?;
    let path_session_id = resolve_session_id_for_state(&id, &state)?;
    let body_session_id = resolve_session_id_for_state(&req.session_id, &state)?;
    if body_session_id != path_session_id {
        return Err(ApiError::BadRequest(format!(
            "Session ID mismatch: path={} body={}",
            id, req.session_id
        )));
    }
    let session_id = body_session_id;

    // Set up event forwarding: caller channel -> broadcast
    let (caller_event_tx, caller_event_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(100);
    let forward_task = spawn_event_forwarder(
        caller_event_rx,
        state.event_tx.clone(),
        session_id.clone(),
        req.verbose,
    );

    let host_mode_requested = req.host_mode;
    let host_mode = resolve_host_mode(host_mode_requested)?;
    let skill_references = canonical_skill_keys_for_state(
        &state,
        req.skill_refs.clone(),
        req.skill_references.clone(),
    )
    .await?;

    // Apply staged MCP operations at the turn boundary.
    // MCP boundary appends text-only system notices; extract a String for it,
    // then fold any additions back into the final ContentInput.
    #[allow(unused_mut)]
    let mut turn_prompt = req.prompt.clone();
    #[cfg(feature = "mcp")]
    {
        let mut mcp_text = String::new();
        apply_mcp_boundary(&state, &session_id, &caller_event_tx, &mut mcp_text).await?;
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

    // First, try to start a turn on a live session in the service.
    let svc_req = SvcStartTurnRequest {
        prompt: turn_prompt.clone(),
        render_metadata: None,
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        event_tx: Some(caller_event_tx.clone()),
        host_mode,
        host_mode_owner: HostModeOwner::ExternalRuntime,
        skill_references: skill_references.clone(),
        flow_tool_overlay: req.flow_tool_overlay.clone(),
        additional_instructions: req.additional_instructions.clone(),
    };
    // Ensure session is registered with executor for runtime-backed execution.
    if let Err(_resp) = ensure_runtime_session_registered(&state, &session_id).await {
        drop(caller_event_tx);
        drain_event_forwarder(&session_id, forward_task).await;
        return Err(ApiError::NotFound(format!(
            "Session not found: {session_id}"
        )));
    }
    let adapter = state.runtime_adapter.clone();

    // Spawn comms drain for host_mode sessions (idempotent — skips if already running).
    #[cfg(feature = "comms")]
    {
        let comms_rt = state.session_service.comms_runtime(&session_id).await;
        let control = state.session_service.comms_drain_control(&session_id).await;
        adapter.set_comms_drain_control(&session_id, control).await;
        adapter
            .maybe_spawn_comms_drain(&session_id, host_mode, comms_rt)
            .await;
    }

    let input = meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::from_content_input(
        svc_req.prompt.clone(),
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                host_mode: if svc_req.host_mode { Some(true) } else { None },
                skill_references: svc_req.skill_references.clone(),
                flow_tool_overlay: svc_req.flow_tool_overlay.clone(),
                additional_instructions: svc_req.additional_instructions.clone(),
                ..Default::default()
            },
        ),
    ));
    let (outcome, handle) = adapter
        .accept_input_with_completion(&session_id, input)
        .await
        .map_err(|err| ApiError::Internal(err.to_string()))?;

    let final_result = match handle {
        Some(handle) => match handle.wait().await {
            meerkat_runtime::completion::CompletionOutcome::Completed(run_result) => Ok(run_result),
            meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult => Err(
                ApiError::Internal("turn completed without result".to_string()),
            ),
            meerkat_runtime::completion::CompletionOutcome::Abandoned(reason) => {
                Err(ApiError::Internal(format!("turn abandoned: {reason}")))
            }
            meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated(reason) => {
                Err(ApiError::Internal(format!("runtime terminated: {reason}")))
            }
        },
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

    match final_result {
        Ok(run_result) => Ok(Json(run_result_to_response(run_result, &state.realm_id))),
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
            Err(err)
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
        .load_persisted(&session_id)
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

    meerkat::surface::resolve_persisted("mcp/add", req.persisted);

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

    adapter
        .stage_add(config)
        .await
        .map_err(ApiError::Internal)?;

    Ok(Json(meerkat::surface::mcp_live_response(
        req.session_id,
        meerkat_contracts::McpLiveOperation::Add,
        Some(req.server_name),
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

    meerkat::surface::resolve_persisted("mcp/remove", req.persisted);

    let adapter = resolve_mcp_adapter(&state, &session_id).await?;
    adapter
        .stage_remove(req.server_name.clone())
        .await
        .map_err(ApiError::Internal)?;

    Ok(Json(meerkat::surface::mcp_live_response(
        req.session_id,
        meerkat_contracts::McpLiveOperation::Remove,
        Some(req.server_name),
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

    meerkat::surface::resolve_persisted("mcp/reload", req.persisted);

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
    DuplicateInput { existing_id: String },
    Configuration(String),
    Agent(String),
    Internal(String),
    ServiceUnavailable(String),
    Gone(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, "BAD_REQUEST", msg),
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, "UNAUTHORIZED", msg),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, "NOT_FOUND", msg),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, "CONFLICT", msg),
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
                "CONFIGURATION_ERROR",
                msg,
            ),
            ApiError::Agent(msg) => (StatusCode::INTERNAL_SERVER_ERROR, "AGENT_ERROR", msg),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", msg),
            ApiError::ServiceUnavailable(msg) => {
                (StatusCode::SERVICE_UNAVAILABLE, "SERVICE_UNAVAILABLE", msg)
            }
            ApiError::Gone(msg) => (StatusCode::GONE, "GONE", msg),
        };

        let body = Json(ErrorResponse {
            error: message,
            code: code.to_string(),
        });

        (status, body).into_response()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream;
    use meerkat_client::{LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use std::path::PathBuf;
    use std::pin::Pin;
    use tempfile::TempDir;

    struct MockLlmClient;

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

    fn hooks_override_fixture() -> HookRunOverrides {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../test-fixtures/hooks/run_override.json");
        let payload = std::fs::read_to_string(path).expect("hook override fixture must exist");
        serde_json::from_str::<HookRunOverrides>(&payload)
            .expect("hook override fixture must deserialize")
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

    #[test]
    fn test_error_response_serialization() {
        let err = ErrorResponse {
            error: "test error".to_string(),
            code: "TEST_ERROR".to_string(),
        };
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("test error"));
        assert!(json.contains("TEST_ERROR"));
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
    fn test_create_session_request_parsing_with_host_mode() {
        let req_json = serde_json::json!({
            "prompt": "Hello",
            "host_mode": true,
            "comms_name": "test-agent"
        });

        let req: CreateSessionRequest = serde_json::from_value(req_json).unwrap();
        assert_eq!(req.prompt, ContentInput::Text("Hello".to_string()));
        assert!(req.host_mode);
        assert_eq!(req.comms_name, Some("test-agent".to_string()));
    }

    #[test]
    fn test_create_session_request_host_mode_defaults_to_false() {
        let req_json = serde_json::json!({
            "prompt": "Hello"
        });

        let req: CreateSessionRequest = serde_json::from_value(req_json).unwrap();
        assert!(!req.host_mode);
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
        state.llm_client_override = Some(Arc::new(MockLlmClient));
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
                .is_some_and(|msg| msg.contains("mob-managed sessions")),
            "reserved mob label rejection should explain the trust boundary: {}",
            String::from_utf8_lossy(&body)
        );
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
                host_mode: false,
                host_mode_owner: HostModeOwner::ExternalRuntime,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
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
                .is_some_and(|msg| msg.contains("mob-managed sessions")),
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
        let create_result = session_service
            .create_session(SvcCreateSessionRequest {
                model: state.default_model.to_string(),
                prompt: "Hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(state.max_tokens),
                event_tx: None,
                host_mode: false,
                host_mode_owner: HostModeOwner::ExternalRuntime,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
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
    async fn test_runtime_state_route_is_available_for_live_sessions() {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let temp = TempDir::new().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        state.llm_client_override = Some(Arc::new(MockLlmClient));
        let session_service = state.session_service.clone();
        let create_result = session_service
            .create_session(SvcCreateSessionRequest {
                model: state.default_model.to_string(),
                prompt: "Hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: Some(state.max_tokens),
                event_tx: None,
                host_mode: false,
                host_mode_owner: HostModeOwner::ExternalRuntime,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
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
        let session_id = create_result.session_id.to_string();

        let request = axum::http::Request::builder()
            .method("GET")
            .uri(format!("/runtime/{session_id}/state"))
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
                host_mode: false,
                host_mode_owner: HostModeOwner::ExternalRuntime,
                skill_references: None,
                initial_turn: InitialTurnPolicy::RunImmediately,
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
                    render_metadata: None,
                    handling_mode: meerkat_core::types::HandlingMode::Queue,
                    event_tx: None,
                    host_mode: false,
                    host_mode_owner: HostModeOwner::ExternalRuntime,
                    skill_references: None,
                    flow_tool_overlay: None,
                    additional_instructions: None,
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
    fn test_build_comms_command_peer_request_invalid_stream() {
        let req = CommsSendRequest {
            session_id: "sid_123".to_string(),
            kind: "peer_request".to_string(),
            to: Some("alice".to_string()),
            body: None,
            intent: Some("ask".to_string()),
            params: None,
            in_reply_to: None,
            status: None,
            result: None,
            source: None,
            stream: Some("invalid".to_string()),
            allow_self_session: None,
            handling_mode: None,
        };
        let session_id = meerkat_core::SessionId::new();
        let err = build_comms_command(&req, &session_id).expect_err("invalid stream should fail");
        let ApiError::BadRequest(msg) = err else {
            unreachable!("expected bad request");
        };
        assert_eq!(msg, "invalid stream: invalid");
    }

    #[test]
    fn test_build_comms_command_peer_response_invalid_status() {
        let req = CommsSendRequest {
            session_id: "sid_123".to_string(),
            kind: "peer_response".to_string(),
            to: Some("alice".to_string()),
            body: None,
            intent: None,
            params: None,
            in_reply_to: Some(uuid::Uuid::new_v4().to_string()),
            status: Some("almost-done".to_string()),
            result: None,
            source: None,
            stream: None,
            allow_self_session: None,
            handling_mode: None,
        };
        let session_id = meerkat_core::SessionId::new();
        let err = build_comms_command(&req, &session_id).expect_err("invalid status should fail");
        let ApiError::BadRequest(msg) = err else {
            unreachable!("expected bad request");
        };
        assert_eq!(msg, "invalid status: almost-done");
    }

    #[test]
    fn test_build_comms_command_invalid_source() {
        let req = CommsSendRequest {
            session_id: "sid_123".to_string(),
            kind: "input".to_string(),
            to: None,
            body: Some("hi".to_string()),
            intent: None,
            params: None,
            in_reply_to: None,
            status: None,
            result: None,
            source: Some("webhookd".to_string()),
            stream: None,
            allow_self_session: None,
            handling_mode: None,
        };
        let session_id = meerkat_core::SessionId::new();
        let err = build_comms_command(&req, &session_id).expect_err("invalid source should fail");
        let ApiError::BadRequest(msg) = err else {
            unreachable!("expected bad request");
        };
        assert_eq!(msg, "invalid source: webhookd");
    }

    #[cfg(not(feature = "comms"))]
    #[test]
    fn test_resolve_host_mode_rejects_when_comms_disabled() {
        let err = resolve_host_mode(true).expect_err("host mode should be rejected");
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_resolve_host_mode_allows_when_comms_enabled() {
        assert!(resolve_host_mode(true).expect("host mode should be enabled"));
        assert!(!resolve_host_mode(false).expect("host mode should be disabled"));
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

        let err = validate_config_for_commit(&config).expect_err("duplicate source uuid");
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

        let response = run_result_to_response(result, "test-realm");
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
            format_session_ref("test-realm", &session_id)
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_mob_prefabs_handler_returns_builtin_prefabs() {
        let Json(result) = mob_prefabs().await;
        let prefabs = result["prefabs"]
            .as_array()
            .expect("prefabs should be an array");
        assert!(prefabs.len() >= 4);
        let keys: Vec<&str> = prefabs
            .iter()
            .filter_map(|entry| entry["key"].as_str())
            .collect();
        assert!(keys.contains(&"coding_swarm"));
        assert!(keys.contains(&"code_review"));
        assert!(keys.contains(&"research_team"));
        assert!(keys.contains(&"pipeline"));
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_mob_prefabs_route_returns_json() {
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
            .uri("/mob/prefabs")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
        let payload: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let prefabs = payload["prefabs"]
            .as_array()
            .expect("prefabs should be an array");
        assert!(
            prefabs
                .iter()
                .all(|entry| entry["toml_template"].is_string())
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn test_mob_tools_and_call_routes_work() {
        use axum::body::Body;
        use http_body_util::BodyExt;
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
        assert_eq!(tools_resp.status(), StatusCode::OK);
        let tools_bytes = tools_resp.into_body().collect().await.unwrap().to_bytes();
        let tools_payload: serde_json::Value = serde_json::from_slice(&tools_bytes).unwrap();
        assert!(
            tools_payload["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == "mob_create")
        );

        let call_req = axum::http::Request::builder()
            .method("POST")
            .uri("/mob/call")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "name": "mob_create",
                    "arguments": { "prefab": "coding_swarm" }
                })
                .to_string(),
            ))
            .unwrap();
        let call_resp = app.oneshot(call_req).await.unwrap();
        assert_eq!(call_resp.status(), StatusCode::OK);
        let call_bytes = call_resp.into_body().collect().await.unwrap().to_bytes();
        let call_payload: serde_json::Value = serde_json::from_slice(&call_bytes).unwrap();
        assert!(call_payload["mob_id"].as_str().is_some());
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
            );
            assert_eq!(resp.session_id, "sid_123");
            assert_eq!(resp.operation, meerkat_contracts::McpLiveOperation::Add);
            assert_eq!(resp.server_name, Some("test-server".to_string()));
            assert_eq!(resp.status, meerkat_contracts::McpLiveOpStatus::Staged);
            assert!(!resp.persisted);
            assert!(resp.applied_at_turn.is_none());
        }

        #[test]
        fn test_resolve_persisted_warns_and_returns_false() {
            // persisted=true should always return false.
            assert!(!meerkat::surface::resolve_persisted("test", true));
            assert!(!meerkat::surface::resolve_persisted("test", false));
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
