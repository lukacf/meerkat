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
    AgentBuildConfig, AgentEvent, AgentFactory, EphemeralSessionService, FactoryAgentBuilder,
    JsonlStore, LlmClient, OutputSchema, Session, SessionId, SessionMeta, SessionService,
    SessionStore,
};
use meerkat_core::error::invalid_session_id_message;
use meerkat_core::service::{
    CreateSessionRequest as SvcCreateSessionRequest, SessionError,
    StartTurnRequest as SvcStartTurnRequest,
};
use meerkat_core::{
    Config, ConfigDelta, ConfigStore, FileConfigStore, HookRunOverrides, Provider,
    SessionTooling, format_verbose_event,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast, mpsc};

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
    pub session_service: Arc<EphemeralSessionService<FactoryAgentBuilder>>,
    /// Slot for staging `AgentBuildConfig` before `session_service` calls.
    pub builder_slot: Arc<Mutex<Option<AgentBuildConfig>>>,
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

        let enable_builtins = config.tools.builtins_enabled;
        let enable_shell = config.tools.shell_enabled;

        let mut factory = AgentFactory::new(store_path.clone())
            .builtins(enable_builtins)
            .shell(enable_shell);
        factory = factory.project_root(instance_root.clone());

        let builder = FactoryAgentBuilder::new(factory, config.clone());
        let builder_slot = builder.build_config_slot.clone();
        let session_service = Arc::new(EphemeralSessionService::new(builder, 100));

        Self {
            store_path,
            default_model: Cow::Owned(config.agent.model.to_string()),
            max_tokens: config.agent.max_tokens_per_turn,
            rest_host: Cow::Owned(config.rest.host.clone()),
            rest_port: config.rest.port,
            enable_builtins,
            enable_shell,
            project_root: Some(instance_root),
            llm_client_override: None,
            config_store,
            event_tx,
            session_service,
            builder_slot,
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

        let mut factory = AgentFactory::new(store_path.clone())
            .builtins(enable_builtins)
            .shell(enable_shell);
        factory = factory.project_root(instance_root.clone());

        let builder = FactoryAgentBuilder::new(factory, config);
        let builder_slot = builder.build_config_slot.clone();
        let session_service = Arc::new(EphemeralSessionService::new(builder, 100));

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
            session_service,
            builder_slot,
        }
    }
}

fn rest_instance_root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("meerkat")
        .join("rest")
}

fn resolve_host_mode(requested: bool) -> Result<bool, ApiError> {
    meerkat::surface::resolve_host_mode(requested).map_err(ApiError::BadRequest)
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
    /// Optional run-scoped hook overrides.
    #[serde(default)]
    pub hooks_override: Option<HookRunOverrides>,
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
    /// Optional run-scoped hook overrides.
    #[serde(default)]
    pub hooks_override: Option<HookRunOverrides>,
}

/// Session response — canonical wire type from contracts.
pub type SessionResponse = meerkat_contracts::WireRunResult;

/// Usage response — re-export from contracts.
pub type UsageResponse = meerkat_contracts::WireUsage;

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
        .route("/capabilities", get(get_capabilities))
        .with_state(state)
}

/// Health check endpoint
async fn health_check() -> &'static str {
    "ok"
}

/// Get runtime capabilities with status resolved against config.
async fn get_capabilities(
    State(state): State<AppState>,
) -> Json<meerkat_contracts::CapabilitiesResponse> {
    let config = state.config_store.get().await.unwrap_or_default();
    Json(meerkat::surface::build_capabilities_response(&config))
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

/// Spawn a task that forwards events from an mpsc receiver to the broadcast channel,
/// optionally logging verbose events. Returns the join handle.
fn spawn_event_forwarder(
    mut event_rx: mpsc::Receiver<AgentEvent>,
    broadcast_tx: broadcast::Sender<SessionEvent>,
    session_id: SessionId,
    verbose: bool,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
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
    })
}

/// Convert a `RunResult` into a `SessionResponse` (via contracts `From` impl).
fn run_result_to_response(result: meerkat_core::types::RunResult) -> SessionResponse {
    result.into()
}

/// Map a `SessionError` to an `ApiError`, handling `CallbackPending` specially
/// by returning a successful response with the pre-created session_id.
fn session_error_to_api_result(
    err: SessionError,
    fallback_session_id: &SessionId,
) -> Result<Json<SessionResponse>, ApiError> {
    match err {
        SessionError::Agent(ref agent_err) => {
            if let meerkat_core::error::AgentError::CallbackPending {
                tool_name: _,
                args: _,
            } = agent_err
            {
                // CallbackPending: the agent is waiting for external tool results.
                // Return a success response with the pre-created session_id so the
                // caller can resume via continue_session.
                return Ok(Json(SessionResponse {
                    session_id: fallback_session_id.clone(),
                    text: "Agent is waiting for tool results".to_string(),
                    turns: 0,
                    tool_calls: 0,
                    usage: UsageResponse::default(),
                    structured_output: None,
                    schema_warnings: None,
                }));
            }
            Err(ApiError::Agent(format!("{err}")))
        }
        SessionError::NotFound { .. } => Err(ApiError::NotFound(format!("{err}"))),
        SessionError::Busy { .. } => Err(ApiError::BadRequest(format!("{err}"))),
        _ => Err(ApiError::Agent(format!("{err}"))),
    }
}

/// Create and run a new session
async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<SessionResponse>, ApiError> {
    let host_mode = resolve_host_mode(req.host_mode)?;
    let model = req.model.unwrap_or_else(|| state.default_model.clone());
    let max_tokens = req.max_tokens.unwrap_or(state.max_tokens);

    // Pre-create a session to claim the session_id (needed for CallbackPending handling
    // and event forwarding before the service call returns).
    let pre_session = Session::new();
    let session_id = pre_session.id().clone();

    // Set up event forwarding: caller channel -> broadcast
    let (caller_event_tx, caller_event_rx) = mpsc::channel::<AgentEvent>(100);
    let forward_task = spawn_event_forwarder(
        caller_event_rx,
        state.event_tx.clone(),
        session_id.clone(),
        req.verbose,
    );

    // Build the full AgentBuildConfig and stage it via the builder slot.
    let build_config = AgentBuildConfig {
        model: model.to_string(),
        provider: req.provider,
        max_tokens: Some(max_tokens),
        system_prompt: req.system_prompt,
        output_schema: req.output_schema,
        structured_output_retries: req.structured_output_retries,
        hooks_override: req.hooks_override.unwrap_or_default(),
        host_mode,
        comms_name: req.comms_name.clone(),
        resume_session: Some(pre_session),
        budget_limits: None,
        event_tx: None, // Wired by the session service's builder
        llm_client_override: state.llm_client_override.clone(),
        provider_params: None,
        external_tools: None,
        override_builtins: None,
        override_shell: None,
        override_subagents: None,
        override_memory: None,
            preload_skills: None,
    };

    // Hold the slot lock across staging + create to prevent concurrent
    // requests from overwriting each other's staged config.
    let result = {
        let mut slot = state.builder_slot.lock().await;
        *slot = Some(build_config);

        let svc_req = SvcCreateSessionRequest {
            model: model.to_string(),
            prompt: req.prompt,
            system_prompt: None, // Already in build_config
            max_tokens: Some(max_tokens),
            event_tx: Some(caller_event_tx),
            host_mode,
        };

        state.session_service.create_session(svc_req).await
    };

    // Wait for the event forwarder to drain
    let _ = forward_task.await;

    match result {
        Ok(run_result) => Ok(Json(run_result_to_response(run_result))),
        Err(err) => session_error_to_api_result(err, &session_id),
    }
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

    // Set up event forwarding: caller channel -> broadcast
    let (caller_event_tx, caller_event_rx) = mpsc::channel::<AgentEvent>(100);
    let forward_task = spawn_event_forwarder(
        caller_event_rx,
        state.event_tx.clone(),
        session_id.clone(),
        req.verbose,
    );

    let host_mode_requested = req.host_mode;
    let host_mode = resolve_host_mode(host_mode_requested)?;

    // First, try to start a turn on a live session in the service.
    let svc_req = SvcStartTurnRequest {
        prompt: req.prompt.clone(),
        event_tx: Some(caller_event_tx.clone()),
        host_mode,
    };

    let result = state
        .session_service
        .start_turn(&session_id, svc_req)
        .await;

    let final_result = match result {
        Ok(run_result) => Ok(run_result),
        Err(SessionError::NotFound { .. }) => {
            // The session isn't live in the service. Load it from the store,
            // stage a build config with resume_session, and create a new
            // service session.
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

            let stored_metadata = session.session_metadata();

            // Resolve tooling flags from stored metadata, falling back to server defaults
            let tooling = stored_metadata
                .as_ref()
                .map(|meta| meta.tooling.clone())
                .unwrap_or(SessionTooling {
                    builtins: state.enable_builtins,
                    shell: state.enable_shell,
                    comms: false,
                    subagents: false,
                    active_skills: None,
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
            let provider = req
                .provider
                .or_else(|| stored_metadata.as_ref().map(|meta| meta.provider));
            let continue_host_mode_requested = host_mode_requested
                || stored_metadata
                    .as_ref()
                    .map(|meta| meta.host_mode)
                    .unwrap_or(false);
            let continue_host_mode = resolve_host_mode(continue_host_mode_requested)?;
            let comms_name = req.comms_name.clone().or_else(|| {
                stored_metadata
                    .as_ref()
                    .and_then(|meta| meta.comms_name.clone())
            });

            let build_config = AgentBuildConfig {
                model: model.to_string(),
                provider,
                max_tokens: Some(max_tokens),
                system_prompt: req.system_prompt,
                output_schema: req.output_schema,
                structured_output_retries: req.structured_output_retries,
                hooks_override: req.hooks_override.unwrap_or_default(),
                host_mode: continue_host_mode,
                comms_name,
                resume_session: Some(session),
                budget_limits: None,
                event_tx: None, // Wired by the session service's builder
                llm_client_override: state.llm_client_override.clone(),
                provider_params: None,
                external_tools: None,
                override_builtins: Some(tooling.builtins),
                override_shell: Some(tooling.shell),
                override_subagents: Some(tooling.subagents),
                override_memory: None,
            preload_skills: None,
            };

            // Hold slot lock across staging + create to prevent races.
            let mut slot = state.builder_slot.lock().await;
            *slot = Some(build_config);

            let svc_req = SvcCreateSessionRequest {
                model: model.to_string(),
                prompt: req.prompt,
                system_prompt: None,
                max_tokens: Some(max_tokens),
                event_tx: Some(caller_event_tx.clone()),
                host_mode: continue_host_mode,
            };

            let r = state
                .session_service
                .create_session(svc_req)
                .await
                .map_err(|e| ApiError::Agent(format!("{e}")));
            drop(slot);
            r
        }
        Err(err) => return session_error_to_api_result(err, &session_id),
    };

    // Drop the sender so the forwarder sees channel closure and can drain.
    drop(caller_event_tx);

    // Wait for the event forwarder to drain
    let _ = forward_task.await;

    match final_result {
        Ok(run_result) => Ok(Json(run_result_to_response(run_result))),
        Err(err) => Err(err),
    }
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

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
}
