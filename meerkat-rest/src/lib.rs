//! meerkat-rest - REST API server for Meerkat
//!
//! Provides HTTP endpoints for running and managing Meerkat agents:
//! - POST /sessions - Create and run a new agent
//! - POST /sessions/:id/messages - Continue an existing session
//! - GET /sessions/:id - Get session details
//! - GET /sessions/:id/events - SSE stream for agent events
//!
//! # Built-in Tools
//! Set `RKAT_ENABLE_BUILTINS=true` to enable built-in tools (task management, shell).
//! When enabled, also set `RKAT_PROJECT_ROOT` to the project directory.
//!
//! Shell tools require explicit enable: `RKAT_ENABLE_SHELL=true`

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
    AgentBuilder, AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    AnthropicClient, JsonlStore, LlmClient, LlmStreamResult, Message, Session, SessionId,
    SessionMeta, SessionStore, ToolDef, ToolError,
};
use meerkat_tools::builtin::{
    BuiltinToolConfig, CompositeDispatcher, FileTaskStore, ToolPolicyLayer, ensure_rkat_dir,
    find_project_root, shell::ShellConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub store_path: PathBuf,
    pub default_model: String,
    pub max_tokens: u32,
    /// Whether to enable built-in tools (task management, shell)
    pub enable_builtins: bool,
    /// Whether to enable shell tools (requires enable_builtins=true)
    pub enable_shell: bool,
    /// Project root for file-based task store and shell working directory
    pub project_root: Option<PathBuf>,
}

impl Default for AppState {
    fn default() -> Self {
        let store_path = std::env::var("RKAT_STORE_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::data_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("meerkat")
                    .join("sessions")
            });

        let enable_builtins = std::env::var("RKAT_ENABLE_BUILTINS")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let enable_shell = std::env::var("RKAT_ENABLE_SHELL")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let project_root = std::env::var("RKAT_PROJECT_ROOT")
            .map(PathBuf::from)
            .ok()
            .or_else(|| {
                // Try to find project root from current directory
                std::env::current_dir()
                    .ok()
                    .map(|cwd| find_project_root(&cwd))
            });

        Self {
            store_path,
            default_model: std::env::var("RKAT_MODEL")
                .unwrap_or_else(|_| "claude-opus-4-5".to_string()),
            max_tokens: std::env::var("RKAT_MAX_TOKENS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(4096),
            enable_builtins,
            enable_shell,
            project_root,
        }
    }
}

/// Create session request
#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub prompt: String,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

/// Continue session request
#[derive(Debug, Deserialize)]
pub struct ContinueSessionRequest {
    pub prompt: String,
}

/// Session response
#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub session_id: String,
    pub text: String,
    pub turns: u32,
    pub tool_calls: u32,
    pub usage: UsageResponse,
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
        .route("/health", get(health_check))
        .with_state(state)
}

/// Health check endpoint
async fn health_check() -> &'static str {
    "ok"
}

/// Create a tool dispatcher based on configuration
///
/// When builtins are enabled, creates a CompositeDispatcher with task tools
/// and optionally shell tools. Otherwise returns an EmptyToolDispatcher.
fn create_tool_dispatcher(state: &AppState) -> Result<Arc<RestToolDispatcher>, ApiError> {
    if !state.enable_builtins {
        return Ok(Arc::new(RestToolDispatcher::Empty(EmptyToolDispatcher)));
    }

    // Need project root for file-based task store
    let project_root = state.project_root.as_ref().ok_or_else(|| {
        ApiError::Configuration(
            "RKAT_PROJECT_ROOT required when RKAT_ENABLE_BUILTINS=true".to_string(),
        )
    })?;

    // Ensure .rkat directory exists
    ensure_rkat_dir(project_root)
        .map_err(|e| ApiError::Internal(format!("Failed to create .rkat directory: {}", e)))?;

    // Create file-based task store
    let task_store = Arc::new(FileTaskStore::in_project(project_root));

    // Create shell config if shell is enabled
    let shell_config = if state.enable_shell {
        Some(ShellConfig::with_project_root(project_root.clone()))
    } else {
        None
    };

    // Create builtin tool config - if shell is enabled, enable shell tools in policy
    let config = if state.enable_shell {
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
    let dispatcher = CompositeDispatcher::builtins_only(task_store, &config, shell_config, None)
        .map_err(|e| ApiError::Internal(format!("Failed to create tool dispatcher: {}", e)))?;

    Ok(Arc::new(RestToolDispatcher::Composite(dispatcher)))
}

/// Create and run a new session
async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<SessionResponse>, ApiError> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| ApiError::Configuration("ANTHROPIC_API_KEY not set".to_string()))?;

    let model = req.model.unwrap_or(state.default_model.clone());
    let max_tokens = req.max_tokens.unwrap_or(state.max_tokens);

    let store = JsonlStore::new(state.store_path.clone());
    store
        .init()
        .await
        .map_err(|e| ApiError::Internal(format!("Store init failed: {}", e)))?;

    let llm_client = Arc::new(AnthropicClient::new(api_key));
    let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, model.clone()));
    let tools = create_tool_dispatcher(&state)?;
    let store_adapter = Arc::new(SessionStoreAdapter::new(Arc::new(store)));

    let mut builder = AgentBuilder::new()
        .model(&model)
        .max_tokens_per_turn(max_tokens);

    if let Some(sys_prompt) = &req.system_prompt {
        builder = builder.system_prompt(sys_prompt);
    }

    let mut agent = builder.build(llm_adapter, tools, store_adapter);

    let result = agent
        .run(req.prompt)
        .await
        .map_err(|e| ApiError::Agent(format!("{}", e)))?;

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
    }))
}

/// Get session details
async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionDetailsResponse>, ApiError> {
    let session_id = SessionId::parse(&id)
        .map_err(|e| ApiError::BadRequest(format!("Invalid session ID: {}", e)))?;

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
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| ApiError::Configuration("ANTHROPIC_API_KEY not set".to_string()))?;

    let session_id = SessionId::parse(&id)
        .map_err(|e| ApiError::BadRequest(format!("Invalid session ID: {}", e)))?;

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

    let model = state.default_model.clone();
    let llm_client = Arc::new(AnthropicClient::new(api_key));
    let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, model.clone()));
    let tools = create_tool_dispatcher(&state)?;
    let store_adapter = Arc::new(SessionStoreAdapter::new(Arc::new(store)));

    let mut agent = AgentBuilder::new()
        .model(&model)
        .max_tokens_per_turn(state.max_tokens)
        .resume_session(session)
        .build(llm_adapter, tools, store_adapter);

    let result = agent
        .run(req.prompt)
        .await
        .map_err(|e| ApiError::Agent(format!("{}", e)))?;

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
    }))
}

/// SSE endpoint for streaming session events
async fn session_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let session_id = SessionId::parse(&id)
        .map_err(|e| ApiError::BadRequest(format!("Invalid session ID: {}", e)))?;

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

    // Create a stream that sends session info as SSE events
    let stream = async_stream::stream! {
        // Send session loaded event
        let event = Event::default()
            .event("session_loaded")
            .data(serde_json::to_string(&json!({
                "session_id": session_id.to_string(),
                "message_count": session.messages().len(),
            })).unwrap_or_default());
        yield Ok(event);

        // Send each message as an event
        for (idx, msg) in session.messages().iter().enumerate() {
            let msg_data = match msg {
                Message::System(s) => json!({
                    "type": "system",
                    "index": idx,
                    "content": s.content,
                }),
                Message::User(u) => json!({
                    "type": "user",
                    "index": idx,
                    "content": u.content,
                }),
                Message::Assistant(a) => json!({
                    "type": "assistant",
                    "index": idx,
                    "content": a.content,
                    "tool_calls": a.tool_calls.len(),
                }),
                Message::ToolResults { results } => json!({
                    "type": "tool_results",
                    "index": idx,
                    "results": results.len(),
                }),
            };

            let event = Event::default()
                .event("message")
                .data(serde_json::to_string(&msg_data).unwrap_or_default());
            yield Ok(event);
        }

        // Send done event
        let event = Event::default()
            .event("done")
            .data("{}");
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

// Adapter types needed for the REST server

use async_trait::async_trait;
use meerkat::{LlmEvent, LlmRequest, StopReason, ToolCall, Usage};

/// LLM client adapter
pub struct LlmClientAdapter<C: LlmClient> {
    client: Arc<C>,
    model: String,
}

impl<C: LlmClient> LlmClientAdapter<C> {
    pub fn new(client: Arc<C>, model: String) -> Self {
        Self { client, model }
    }
}

#[async_trait]
impl<C: LlmClient + 'static> AgentLlmClient for LlmClientAdapter<C> {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&serde_json::Value>,
    ) -> Result<LlmStreamResult, AgentError> {
        use futures::StreamExt;

        let request = LlmRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            max_tokens,
            temperature,
            stop_sequences: None,
            provider_params: provider_params.cloned(),
        };

        let mut stream = self.client.stream(&request);

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut stop_reason = StopReason::EndTurn;
        let mut usage = Usage::default();

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => match event {
                    LlmEvent::TextDelta { delta } => {
                        content.push_str(&delta);
                    }
                    LlmEvent::ToolCallComplete { id, name, args, .. } => {
                        tool_calls.push(ToolCall::new(id, name, args));
                    }
                    LlmEvent::UsageUpdate { usage: u } => {
                        usage = u;
                    }
                    LlmEvent::Done { stop_reason: sr } => {
                        stop_reason = sr;
                    }
                    _ => {}
                },
                Err(e) => {
                    return Err(AgentError::LlmError(e.to_string()));
                }
            }
        }

        Ok(LlmStreamResult {
            content,
            tool_calls,
            stop_reason,
            usage,
        })
    }

    fn provider(&self) -> &'static str {
        self.client.provider()
    }
}

/// Empty tool dispatcher
#[derive(Debug)]
pub struct EmptyToolDispatcher;

#[async_trait]
impl AgentToolDispatcher for EmptyToolDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        Vec::new()
    }

    async fn dispatch(&self, name: &str, _args: &Value) -> Result<Value, ToolError> {
        Err(ToolError::not_found(name))
    }
}

/// Tool dispatcher that can be either empty or composite
///
/// This enum allows us to use different tool dispatcher implementations
/// while still satisfying the generic type requirements of AgentBuilder.
pub enum RestToolDispatcher {
    Empty(EmptyToolDispatcher),
    Composite(CompositeDispatcher),
}

impl std::fmt::Debug for RestToolDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RestToolDispatcher::Empty(_) => write!(f, "RestToolDispatcher::Empty"),
            RestToolDispatcher::Composite(_) => write!(f, "RestToolDispatcher::Composite"),
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for RestToolDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        match self {
            RestToolDispatcher::Empty(d) => d.tools(),
            RestToolDispatcher::Composite(d) => d.tools(),
        }
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        match self {
            RestToolDispatcher::Empty(d) => d.dispatch(name, args).await,
            RestToolDispatcher::Composite(d) => d.dispatch(name, args).await,
        }
    }
}

/// Session store adapter
pub struct SessionStoreAdapter<S: SessionStore> {
    store: Arc<S>,
}

impl<S: SessionStore> SessionStoreAdapter<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl<S: SessionStore + 'static> AgentSessionStore for SessionStoreAdapter<S> {
    async fn save(&self, session: &Session) -> Result<(), AgentError> {
        self.store
            .save(session)
            .await
            .map_err(|e| AgentError::StoreError(e.to_string()))
    }

    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError> {
        let session_id = meerkat::SessionId::parse(id)
            .map_err(|e| AgentError::StoreError(format!("Invalid session ID: {}", e)))?;

        self.store
            .load(&session_id)
            .await
            .map_err(|e| AgentError::StoreError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_state_default() {
        let state = AppState::default();
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

    #[test]
    fn test_app_state_builtins_disabled_by_default() {
        let state = AppState::default();
        assert!(!state.enable_builtins);
        assert!(!state.enable_shell);
    }

    #[test]
    fn test_create_tool_dispatcher_without_builtins() {
        let state = AppState {
            enable_builtins: false,
            ..Default::default()
        };
        let result = create_tool_dispatcher(&state);
        assert!(result.is_ok());
        let dispatcher = result.unwrap();
        // Empty dispatcher should have no tools
        assert!(dispatcher.tools().is_empty());
    }

    #[test]
    fn test_create_tool_dispatcher_with_builtins() {
        let temp_dir = std::env::temp_dir().join("meerkat-rest-test");
        std::fs::create_dir_all(&temp_dir).unwrap();

        let state = AppState {
            enable_builtins: true,
            enable_shell: false,
            project_root: Some(temp_dir.clone()),
            ..Default::default()
        };
        let result = create_tool_dispatcher(&state);
        assert!(result.is_ok());
        let dispatcher = result.unwrap();
        // Should have task tools
        let tools = dispatcher.tools();
        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
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
        let result = create_tool_dispatcher(&state);
        assert!(result.is_ok());
        let dispatcher = result.unwrap();
        // Should have both task and shell tools
        let tools = dispatcher.tools();
        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
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
        let result = create_tool_dispatcher(&state);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiError::Configuration(_)));
    }
}
