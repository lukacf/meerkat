//! meerkat-rest - REST API server for Meerkat
//!
//! Provides HTTP endpoints for running and managing Meerkat agents:
//! - POST /sessions - Create and run a new agent
//! - POST /sessions/:id/messages - Continue an existing session
//! - GET /sessions/:id - Get session details
//! - GET /sessions/:id/events - SSE stream for agent events
//!
//! # Built-in Tools
//! Built-in tools are configured via the REST config store.
//! When enabled, the REST instance uses its instance-scoped data directory
//! as the project root for task storage and shell working directory.

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{Path, State},
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
    AgentBuilder, AgentEvent, AgentToolDispatcher, JsonlStore, OutputSchema, SessionId,
    SessionMeta, SessionStore, ToolDef, ToolError, ToolResult, build_comms_runtime_from_config,
    compose_tools_with_comms,
};
use meerkat_client::{LlmClient, LlmClientAdapter, ProviderResolver};
use meerkat_core::agent::CommsRuntime as CommsRuntimeTrait;
use meerkat_core::error::invalid_session_id_message;
use meerkat_core::{
    Config, ConfigDelta, ConfigStore, FileConfigStore, Provider, SchemaWarning, SessionMetadata,
    SessionTooling, SystemPromptConfig, ToolCallView, format_verbose_event,
};
use meerkat_store::StoreAdapter;
use meerkat_tools::builtin::{
    BuiltinToolConfig, CompositeDispatcher, FileTaskStore, ToolPolicyLayer, ensure_rkat_dir,
    shell::ShellConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

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
}

impl Default for AppState {
    fn default() -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let instance_root = rest_instance_root();
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(FileConfigStore::new(instance_root.join("config.toml")));

        let config = Config::default();
        let store_path = config
            .store
            .sessions_path
            .clone()
            .or_else(|| config.storage.directory.clone())
            .unwrap_or_else(|| {
                dirs::data_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("meerkat")
                    .join("sessions")
            });

        Self {
            store_path,
            default_model: Cow::Owned(config.agent.model.to_string()),
            max_tokens: config.agent.max_tokens_per_turn,
            rest_host: Cow::Owned(config.rest.host.clone()),
            rest_port: config.rest.port,
            enable_builtins: config.tools.builtins_enabled,
            enable_shell: config.tools.shell_enabled,
            project_root: Some(instance_root),
            llm_client_override: None,
            config_store,
            event_tx,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionEvent {
    session_id: SessionId,
    event: AgentEvent,
}

impl AppState {
    pub async fn load() -> Self {
        Self::load_from(rest_instance_root()).await
    }

    async fn load_from(instance_root: PathBuf) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(FileConfigStore::new(instance_root.join("config.toml")));

        let mut config = config_store
            .get()
            .await
            .unwrap_or_else(|_| Config::default());
        if let Err(err) = config.apply_env_overrides() {
            tracing::warn!("Failed to apply env overrides: {}", err);
        }

        let store_path = config
            .store
            .sessions_path
            .clone()
            .or_else(|| config.storage.directory.clone())
            .unwrap_or_else(|| {
                dirs::data_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("meerkat")
                    .join("sessions")
            });

        let enable_builtins = config.tools.builtins_enabled;
        let enable_shell = config.tools.shell_enabled;

        let default_model = Cow::Owned(config.agent.model.to_string());
        let max_tokens = config.agent.max_tokens_per_turn;
        let rest_host = Cow::Owned(config.rest.host.clone());
        let rest_port = config.rest.port;

        Self {
            store_path,
            default_model,
            max_tokens,
            rest_host,
            rest_port,
            enable_builtins,
            enable_shell,
            project_root: Some(instance_root),
            llm_client_override: None,
            config_store,
            event_tx,
        }
    }
}

fn rest_instance_root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("meerkat")
        .join("rest")
}

async fn load_config_from_store(store: &dyn ConfigStore) -> Config {
    let mut config = store.get().await.unwrap_or_else(|_| Config::default());
    if let Err(err) = config.apply_env_overrides() {
        tracing::warn!("Failed to apply env overrides: {}", err);
    }
    config
}

fn resolve_provider(input: Option<Provider>, model: &str) -> Result<Provider, ApiError> {
    match input {
        Some(provider) => Ok(provider),
        None => {
            let inferred = ProviderResolver::infer_from_model(model);
            if inferred == Provider::Other {
                Err(ApiError::BadRequest(format!(
                    "Cannot infer provider from model '{}'. Please specify a provider explicitly.",
                    model
                )))
            } else {
                Ok(inferred)
            }
        }
    }
}

fn provider_key(provider: Provider) -> &'static str {
    match provider {
        Provider::Anthropic => "anthropic",
        Provider::OpenAI => "openai",
        Provider::Gemini => "gemini",
        Provider::Other => "other",
    }
}

/// Create session request
#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub prompt: String,
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
}

fn default_structured_output_retries() -> u32 {
    2
}

/// Continue session request
#[derive(Debug, Deserialize)]
pub struct ContinueSessionRequest {
    pub session_id: String,
    pub prompt: String,
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
    /// Enable verbose event logging (server-side).
    #[serde(default)]
    pub verbose: bool,
    #[serde(default)]
    pub model: Option<Cow<'static, str>>,
    #[serde(default)]
    pub provider: Option<Provider>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

/// Session response
#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub session_id: String,
    pub text: String,
    pub turns: u32,
    pub tool_calls: u32,
    pub usage: UsageResponse,
    /// Validated structured output (if output_schema was provided)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<Value>,
    /// Warnings produced during schema compilation (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_warnings: Option<Vec<SchemaWarning>>,
}

/// Usage response
#[derive(Debug, Serialize)]
pub struct UsageResponse {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

/// Session details response
#[derive(Debug, Serialize)]
pub struct SessionDetailsResponse {
    pub session_id: String,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: usize,
    pub total_tokens: u64,
}

/// API error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: String,
}

/// Build the REST API router
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/sessions", post(create_session))
        .route("/sessions/{id}", get(get_session))
        .route("/sessions/{id}/messages", post(continue_session))
        .route("/sessions/{id}/events", get(session_events))
        .route(
            "/config",
            get(get_config).put(set_config).patch(patch_config),
        )
        .route("/health", get(health_check))
        .with_state(state)
}

/// Health check endpoint
async fn health_check() -> &'static str {
    "ok"
}

/// Get the current config
async fn get_config(State(state): State<AppState>) -> Result<Json<Config>, ApiError> {
    let config = state
        .config_store
        .get()
        .await
        .map_err(|e| ApiError::Configuration(e.to_string()))?;
    Ok(Json(config))
}

/// Replace the current config
async fn set_config(
    State(state): State<AppState>,
    Json(config): Json<Config>,
) -> Result<Json<Config>, ApiError> {
    state
        .config_store
        .set(config.clone())
        .await
        .map_err(|e| ApiError::Configuration(e.to_string()))?;
    Ok(Json(config))
}

/// Patch the current config using a JSON merge patch
async fn patch_config(
    State(state): State<AppState>,
    Json(delta): Json<Value>,
) -> Result<Json<Config>, ApiError> {
    let updated = state
        .config_store
        .patch(ConfigDelta(delta))
        .await
        .map_err(|e| ApiError::Configuration(e.to_string()))?;
    Ok(Json(updated))
}

/// Create a tool dispatcher based on configuration
///
/// When builtins are enabled, creates a CompositeDispatcher with task tools
/// and optionally shell tools. Otherwise returns an EmptyToolDispatcher.
fn create_tool_dispatcher(
    project_root: Option<&PathBuf>,
    enable_builtins: bool,
    enable_shell: bool,
) -> Result<(Arc<dyn AgentToolDispatcher>, String), ApiError> {
    if !enable_builtins {
        return Ok((Arc::new(EmptyToolDispatcher), String::new()));
    }

    // Need project root for file-based task store
    let project_root = project_root.ok_or_else(|| {
        ApiError::Configuration("project_root required when built-in tools are enabled".to_string())
    })?;

    // Create file-based task store
    let task_store = Arc::new(FileTaskStore::in_project(project_root));

    // Create shell config if shell is enabled
    let shell_config = if enable_shell {
        Some(ShellConfig::with_project_root(project_root.clone()))
    } else {
        None
    };

    // Create builtin tool config - if shell is enabled, enable shell tools in policy
    let config = if enable_shell {
        BuiltinToolConfig {
            policy: ToolPolicyLayer::new()
                .enable_tool("shell")
                .enable_tool("shell_job_status")
                .enable_tool("shell_jobs")
                .enable_tool("shell_job_cancel"),
            ..Default::default()
        }
    } else {
        BuiltinToolConfig::default()
    };

    // Create composite dispatcher
    let dispatcher = CompositeDispatcher::new(task_store, &config, shell_config, None, None)
        .map_err(|e| ApiError::Internal(format!("Failed to create tool dispatcher: {}", e)))?;

    let tool_usage_instructions = dispatcher.usage_instructions();
    Ok((Arc::new(dispatcher), tool_usage_instructions))
}

async fn ensure_rkat_dir_async(project_root: &std::path::Path) -> Result<(), ApiError> {
    let project_root = project_root.to_path_buf();
    tokio::task::spawn_blocking(move || ensure_rkat_dir(&project_root))
        .await
        .map_err(|e| ApiError::Internal(format!("Directory creation task failed: {}", e)))?
        .map_err(|e| ApiError::Internal(format!("Failed to create .rkat directory: {}", e)))?;
    Ok(())
}

/// Create and run a new session
async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<SessionResponse>, ApiError> {
    // Validate host mode requirements
    if req.host_mode && req.comms_name.is_none() {
        return Err(ApiError::BadRequest(
            "host_mode requires comms_name to be set".to_string(),
        ));
    }

    let model = req.model.unwrap_or_else(|| state.default_model.clone());
    let max_tokens = req.max_tokens.unwrap_or(state.max_tokens);
    let config = load_config_from_store(state.config_store.as_ref()).await;
    let provider = resolve_provider(req.provider, &model)?;

    let store = JsonlStore::new(state.store_path.clone());
    store
        .init()
        .await
        .map_err(|e| ApiError::Internal(format!("Store init failed: {}", e)))?;

    let llm_client = match state.llm_client_override.clone() {
        Some(client) => client,
        None => {
            if ProviderResolver::api_key_for(provider).is_none() {
                return Err(ApiError::Configuration(format!(
                    "API key not set for provider '{}'",
                    provider_key(provider)
                )));
            }

            let base_url = config
                .providers
                .base_urls
                .as_ref()
                .and_then(|map| map.get(provider_key(provider)).cloned());
            ProviderResolver::client_for(provider, base_url)
        }
    };
    let (agent_event_tx, mut agent_event_rx) = mpsc::channel::<AgentEvent>(100);
    let verbose = req.verbose;
    let llm_adapter = Arc::new(LlmClientAdapter::with_event_channel(
        llm_client,
        model.to_string(),
        agent_event_tx.clone(),
    ));
    if state.enable_builtins {
        let project_root = state.project_root.as_ref().ok_or_else(|| {
            ApiError::Configuration(
                "project_root required when built-in tools are enabled".to_string(),
            )
        })?;
        ensure_rkat_dir_async(project_root).await?;
    }
    let (mut tools, mut tool_usage_instructions) = create_tool_dispatcher(
        state.project_root.as_ref(),
        state.enable_builtins,
        state.enable_shell,
    )?;
    let store_adapter = Arc::new(StoreAdapter::new(Arc::new(store)));

    // Create comms runtime if host_mode is enabled
    // Note: inproc_only() already registers in InprocRegistry, no need to start listeners
    let comms_runtime = if req.host_mode {
        let comms_name = req.comms_name.as_ref().ok_or_else(|| {
            ApiError::Configuration("comms_name required when host_mode is enabled".to_string())
        })?;
        let base_dir = state
            .project_root
            .clone()
            .or_else(|| state.store_path.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(rest_instance_root);
        let runtime = build_comms_runtime_from_config(&config, base_dir, comms_name)
            .await
            .map_err(ApiError::Internal)?;
        Some(runtime)
    } else {
        None
    };

    if let Some(ref runtime) = comms_runtime {
        let composed = compose_tools_with_comms(tools, tool_usage_instructions, runtime)
            .map_err(|e| ApiError::Internal(format!("Failed to compose comms tools: {}", e)))?;
        tools = composed.0;
        tool_usage_instructions = composed.1;
    }

    let mut builder = AgentBuilder::new()
        .model(model.clone())
        .max_tokens_per_turn(max_tokens)
        .budget(config.budget_limits())
        .structured_output_retries(req.structured_output_retries);

    // Add output schema if provided
    if let Some(ref schema) = req.output_schema {
        builder = builder.output_schema(schema.clone());
    }

    // Use caller-provided system prompt if set, otherwise use default
    let mut system_prompt = match req.system_prompt.clone() {
        Some(prompt) => prompt,
        None => SystemPromptConfig::new().compose().await,
    };
    if !tool_usage_instructions.is_empty() {
        system_prompt.push_str("\n\n");
        system_prompt.push_str(&tool_usage_instructions);
    }
    builder = builder.system_prompt(system_prompt);

    // Add comms runtime if enabled
    if let Some(runtime) = comms_runtime {
        builder = builder.with_comms_runtime(Arc::new(runtime) as Arc<dyn CommsRuntimeTrait>);
    }

    let mut agent = builder.build(llm_adapter, tools, store_adapter).await;
    let session_id = agent.session().id().clone();
    let broadcast_tx = state.event_tx.clone();
    let forward_task = tokio::spawn(async move {
        while let Some(event) = agent_event_rx.recv().await {
            if verbose {
                if let Some(line) = format_verbose_event(&event) {
                    tracing::info!("{}", line);
                }
            }
            let _ = broadcast_tx.send(SessionEvent {
                session_id: session_id.clone(),
                event,
            });
        }
    });
    let metadata = SessionMetadata {
        model: model.to_string(),
        max_tokens,
        provider,
        tooling: SessionTooling {
            builtins: state.enable_builtins,
            shell: state.enable_shell,
            comms: req.host_mode,
            subagents: false,
        },
        host_mode: req.host_mode,
        comms_name: req.comms_name.clone(),
    };
    if let Err(err) = agent.session_mut().set_session_metadata(metadata) {
        tracing::warn!("Failed to store session metadata: {}", err);
    }

    // Run agent based on mode
    let result = if req.host_mode {
        agent
            .run_host_mode_with_events(req.prompt, agent_event_tx.clone())
            .await
            .map_err(|e| ApiError::Agent(format!("{}", e)))?
    } else {
        agent
            .run_with_events(req.prompt, agent_event_tx.clone())
            .await
            .map_err(|e| ApiError::Agent(format!("{}", e)))?
    };
    drop(agent);
    drop(agent_event_tx);
    let _ = forward_task.await;

    Ok(Json(SessionResponse {
        session_id: result.session_id.to_string(),
        text: result.text,
        turns: result.turns,
        tool_calls: result.tool_calls,
        usage: UsageResponse {
            input_tokens: result.usage.input_tokens,
            output_tokens: result.usage.output_tokens,
            total_tokens: result.usage.total_tokens(),
        },
        structured_output: result.structured_output,
        schema_warnings: result.schema_warnings,
    }))
}

/// Get session details
async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionDetailsResponse>, ApiError> {
    let session_id =
        SessionId::parse(&id).map_err(|e| ApiError::BadRequest(invalid_session_id_message(e)))?;

    let store = JsonlStore::new(state.store_path);
    store
        .init()
        .await
        .map_err(|e| ApiError::Internal(format!("Store init failed: {}", e)))?;

    let session = store
        .load(&session_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to load session: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("Session not found: {}", id)))?;

    let meta = SessionMeta::from(&session);
    let created_at: DateTime<Utc> = meta.created_at.into();
    let updated_at: DateTime<Utc> = meta.updated_at.into();

    Ok(Json(SessionDetailsResponse {
        session_id: meta.id.to_string(),
        created_at: created_at.to_rfc3339(),
        updated_at: updated_at.to_rfc3339(),
        message_count: meta.message_count,
        total_tokens: meta.total_tokens,
    }))
}

/// Continue an existing session
async fn continue_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ContinueSessionRequest>,
) -> Result<Json<SessionResponse>, ApiError> {
    if req.session_id != id {
        return Err(ApiError::BadRequest(format!(
            "Session ID mismatch: path={} body={}",
            id, req.session_id
        )));
    }

    let session_id = SessionId::parse(&req.session_id)
        .map_err(|e| ApiError::BadRequest(invalid_session_id_message(e)))?;

    let store = JsonlStore::new(state.store_path.clone());
    store
        .init()
        .await
        .map_err(|e| ApiError::Internal(format!("Store init failed: {}", e)))?;

    let session = store
        .load(&session_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to load session: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("Session not found: {}", id)))?;

    let config = load_config_from_store(state.config_store.as_ref()).await;
    let stored_metadata = session.session_metadata();
    let tooling = stored_metadata
        .as_ref()
        .map(|meta| meta.tooling.clone())
        .unwrap_or(SessionTooling {
            builtins: state.enable_builtins,
            shell: state.enable_shell,
            comms: false,
            subagents: false,
        });
    let model = req
        .model
        .or_else(|| {
            stored_metadata
                .as_ref()
                .map(|meta| meta.model.clone().into())
        })
        .unwrap_or_else(|| state.default_model.clone());
    let max_tokens = req
        .max_tokens
        .or_else(|| stored_metadata.as_ref().map(|meta| meta.max_tokens))
        .unwrap_or(state.max_tokens);
    let provider = resolve_provider(
        req.provider
            .or_else(|| stored_metadata.as_ref().map(|meta| meta.provider)),
        &model,
    )?;
    let host_mode = req.host_mode
        || stored_metadata
            .as_ref()
            .map(|meta| meta.host_mode)
            .unwrap_or(false);
    let comms_name = req.comms_name.clone().or_else(|| {
        stored_metadata
            .as_ref()
            .and_then(|meta| meta.comms_name.clone())
    });

    if host_mode && comms_name.is_none() {
        return Err(ApiError::BadRequest(
            "host_mode requires comms_name to be set".to_string(),
        ));
    }

    let llm_client = match state.llm_client_override.clone() {
        Some(client) => client,
        None => {
            if ProviderResolver::api_key_for(provider).is_none() {
                return Err(ApiError::Configuration(format!(
                    "API key not set for provider '{}'",
                    provider_key(provider)
                )));
            }

            let base_url = config
                .providers
                .base_urls
                .as_ref()
                .and_then(|map| map.get(provider_key(provider)).cloned());
            ProviderResolver::client_for(provider, base_url)
        }
    };
    let (agent_event_tx, mut agent_event_rx) = mpsc::channel::<AgentEvent>(100);
    let verbose = req.verbose;
    let llm_adapter = Arc::new(LlmClientAdapter::with_event_channel(
        llm_client,
        model.to_string(),
        agent_event_tx.clone(),
    ));
    if tooling.builtins {
        let project_root = state.project_root.as_ref().ok_or_else(|| {
            ApiError::Configuration(
                "project_root required when built-in tools are enabled".to_string(),
            )
        })?;
        ensure_rkat_dir_async(project_root).await?;
    }
    let (mut tools, mut tool_usage_instructions) =
        create_tool_dispatcher(state.project_root.as_ref(), tooling.builtins, tooling.shell)?;
    let store_adapter = Arc::new(StoreAdapter::new(Arc::new(store)));

    let comms_runtime = if host_mode {
        let comms_name = comms_name.as_ref().ok_or_else(|| {
            ApiError::Configuration("comms_name required when host_mode is enabled".to_string())
        })?;
        let base_dir = state
            .project_root
            .clone()
            .or_else(|| state.store_path.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(rest_instance_root);
        let runtime = build_comms_runtime_from_config(&config, base_dir, comms_name)
            .await
            .map_err(ApiError::Internal)?;
        Some(runtime)
    } else {
        None
    };

    if let Some(ref runtime) = comms_runtime {
        let composed = compose_tools_with_comms(tools, tool_usage_instructions, runtime)
            .map_err(|e| ApiError::Internal(format!("Failed to compose comms tools: {}", e)))?;
        tools = composed.0;
        tool_usage_instructions = composed.1;
    }

    let mut builder = AgentBuilder::new()
        .model(model.clone())
        .max_tokens_per_turn(max_tokens)
        .budget(config.budget_limits())
        .structured_output_retries(req.structured_output_retries)
        .resume_session(session);

    // Add output schema if provided
    if let Some(ref schema) = req.output_schema {
        builder = builder.output_schema(schema.clone());
    }

    let mut system_prompt = match req.system_prompt.clone() {
        Some(prompt) => prompt,
        None => SystemPromptConfig::new().compose().await,
    };
    if !tool_usage_instructions.is_empty() {
        system_prompt.push_str("\n\n");
        system_prompt.push_str(&tool_usage_instructions);
    }
    builder = builder.system_prompt(system_prompt);

    if let Some(runtime) = comms_runtime {
        builder = builder.with_comms_runtime(Arc::new(runtime) as Arc<dyn CommsRuntimeTrait>);
    }

    let mut agent = builder.build(llm_adapter, tools, store_adapter).await;
    let session_id = agent.session().id().clone();
    let broadcast_tx = state.event_tx.clone();
    let forward_task = tokio::spawn(async move {
        while let Some(event) = agent_event_rx.recv().await {
            if verbose {
                if let Some(line) = format_verbose_event(&event) {
                    tracing::info!("{}", line);
                }
            }
            let _ = broadcast_tx.send(SessionEvent {
                session_id: session_id.clone(),
                event,
            });
        }
    });
    let metadata = SessionMetadata {
        model: model.to_string(),
        max_tokens,
        provider,
        tooling,
        host_mode,
        comms_name,
    };
    if let Err(err) = agent.session_mut().set_session_metadata(metadata) {
        tracing::warn!("Failed to store session metadata: {}", err);
    }

    let result = if host_mode {
        agent
            .run_host_mode_with_events(req.prompt, agent_event_tx.clone())
            .await
            .map_err(|e| ApiError::Agent(format!("{}", e)))?
    } else {
        agent
            .run_with_events(req.prompt, agent_event_tx.clone())
            .await
            .map_err(|e| ApiError::Agent(format!("{}", e)))?
    };
    drop(agent);
    drop(agent_event_tx);
    let _ = forward_task.await;

    Ok(Json(SessionResponse {
        session_id: result.session_id.to_string(),
        text: result.text,
        turns: result.turns,
        tool_calls: result.tool_calls,
        usage: UsageResponse {
            input_tokens: result.usage.input_tokens,
            output_tokens: result.usage.output_tokens,
            total_tokens: result.usage.total_tokens(),
        },
        structured_output: result.structured_output,
        schema_warnings: result.schema_warnings,
    }))
}

/// SSE endpoint for streaming session events
async fn session_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let session_id =
        SessionId::parse(&id).map_err(|e| ApiError::BadRequest(invalid_session_id_message(e)))?;

    let store = JsonlStore::new(state.store_path);
    store
        .init()
        .await
        .map_err(|e| ApiError::Internal(format!("Store init failed: {}", e)))?;

    let session = store
        .load(&session_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to load session: {}", e)))?
        .ok_or_else(|| ApiError::NotFound(format!("Session not found: {}", id)))?;

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

                    let json = serde_json::to_value(&payload.event).unwrap_or_else(|_| {
                        json!({
                            "type": "unknown"
                        })
                    });
                    let event_type = json
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("agent_event");

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

/// API error types
#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    NotFound(String),
    Configuration(String),
    Agent(String),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, "BAD_REQUEST", msg),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, "NOT_FOUND", msg),
            ApiError::Configuration(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "CONFIGURATION_ERROR",
                msg,
            ),
            ApiError::Agent(msg) => (StatusCode::INTERNAL_SERVER_ERROR, "AGENT_ERROR", msg),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", msg),
        };

        let body = Json(ErrorResponse {
            error: message,
            code: code.to_string(),
        });

        (status, body).into_response()
    }
}

/// Empty tool dispatcher
#[derive(Debug)]
pub struct EmptyToolDispatcher;

#[async_trait]
impl AgentToolDispatcher for EmptyToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([])
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        Err(ToolError::not_found(call.name))
    }
}

/// Tool dispatcher that can be either empty or composite
///
/// This enum allows us to use different tool dispatcher implementations
/// while still satisfying the generic type requirements of AgentBuilder.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_app_state_default() {
        let temp = TempDir::new().unwrap();
        let state = AppState::load_from(temp.path().to_path_buf()).await;
        assert!(!state.default_model.is_empty());
        assert!(state.max_tokens > 0);
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
        let state = AppState::load_from(temp.path().to_path_buf()).await;
        assert!(!state.enable_builtins);
        assert!(!state.enable_shell);
    }

    #[test]
    fn test_create_tool_dispatcher_without_builtins() {
        let result = create_tool_dispatcher(None, false, false);
        assert!(result.is_ok());
        let (dispatcher, _) = result.unwrap();
        // Empty dispatcher should have no tools
        assert!(dispatcher.tools().is_empty());
    }

    #[test]
    fn test_create_tool_dispatcher_with_builtins() {
        let temp = TempDir::new().unwrap();
        let temp_dir = temp.path().to_path_buf();

        let result = create_tool_dispatcher(Some(&temp_dir), true, false);
        assert!(result.is_ok());
        let (dispatcher, _) = result.unwrap();
        // Should have task tools
        let tools = dispatcher.tools();
        let tool_names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(tool_names.contains(&"task_create"));
        assert!(tool_names.contains(&"task_list"));
        // Should not have shell tools (not enabled)
        assert!(!tool_names.contains(&"shell"));

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_create_tool_dispatcher_with_shell() {
        let temp_dir = std::env::temp_dir().join("meerkat-rest-test-shell");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let state = AppState {
            enable_builtins: true,
            enable_shell: true,
            project_root: Some(temp_dir.clone()),
            ..Default::default()
        };
        let result = create_tool_dispatcher(
            state.project_root.as_ref(),
            state.enable_builtins,
            state.enable_shell,
        );
        assert!(result.is_ok());
        let (dispatcher, _) = result.unwrap();
        // Should have both task and shell tools
        let tools = dispatcher.tools();
        let tool_names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(tool_names.contains(&"task_create"));
        assert!(tool_names.contains(&"shell"));
        assert!(tool_names.contains(&"shell_job_status"));

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_create_tool_dispatcher_requires_project_root() {
        let state = AppState {
            enable_builtins: true,
            enable_shell: false,
            project_root: None,
            ..Default::default()
        };
        let result = create_tool_dispatcher(
            state.project_root.as_ref(),
            state.enable_builtins,
            state.enable_shell,
        );
        assert!(result.is_err());
        let err = result.err().expect("expected configuration error");
        assert!(matches!(err, ApiError::Configuration(_)));
    }

    #[test]
    fn test_create_session_request_parsing_with_host_mode() {
        let req_json = serde_json::json!({
            "prompt": "Hello",
            "host_mode": true,
            "comms_name": "test-agent"
        });

        let req: CreateSessionRequest = serde_json::from_value(req_json).unwrap();
        assert_eq!(req.prompt, "Hello");
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
}
