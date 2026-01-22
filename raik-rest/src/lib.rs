//! raik-rest - REST API server for RAIK
//!
//! Provides HTTP endpoints for running and managing RAIK agents:
//! - POST /sessions - Create and run a new agent
//! - POST /sessions/:id/messages - Continue an existing session
//! - GET /sessions/:id - Get session details
//! - GET /sessions/:id/events - SSE stream for agent events

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{
        sse::{Event, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use futures::stream::Stream;
use raik::{
    AgentBuilder, AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    AnthropicClient, JsonlStore, LlmClient, LlmStreamResult, Message, Session, SessionId,
    SessionMeta, SessionStore, ToolDef,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub store_path: PathBuf,
    pub default_model: String,
    pub max_tokens: u32,
}

impl Default for AppState {
    fn default() -> Self {
        let store_path = std::env::var("RAIK_STORE_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::data_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("raik")
                    .join("sessions")
            });

        Self {
            store_path,
            default_model: std::env::var("RAIK_MODEL")
                .unwrap_or_else(|_| "claude-opus-4-5".to_string()),
            max_tokens: std::env::var("RAIK_MAX_TOKENS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(4096),
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

/// Create and run a new session
async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<SessionResponse>, ApiError> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| ApiError::Configuration("ANTHROPIC_API_KEY not set".to_string()))?;

    let model = req.model.unwrap_or(state.default_model);
    let max_tokens = req.max_tokens.unwrap_or(state.max_tokens);

    let store = JsonlStore::new(state.store_path);
    store
        .init()
        .await
        .map_err(|e| ApiError::Internal(format!("Store init failed: {}", e)))?;

    let llm_client = Arc::new(AnthropicClient::new(api_key));
    let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, model.clone()));
    let tools = Arc::new(EmptyToolDispatcher);
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
    let tools = Arc::new(EmptyToolDispatcher);
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
            ApiError::Configuration(msg) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "CONFIGURATION_ERROR", msg)
            }
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
use raik::{LlmEvent, LlmRequest, StopReason, ToolCall, Usage};

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
    ) -> Result<LlmStreamResult, AgentError> {
        use futures::StreamExt;

        let request = LlmRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            max_tokens,
            temperature: None,
            stop_sequences: None,
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
                    LlmEvent::ToolCallComplete { id, name, args } => {
                        tool_calls.push(ToolCall { id, name, args });
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
pub struct EmptyToolDispatcher;

#[async_trait]
impl AgentToolDispatcher for EmptyToolDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        Vec::new()
    }

    async fn dispatch(&self, name: &str, _args: &Value) -> Result<String, String> {
        Err(format!("Unknown tool: {}", name))
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
        let session_id = raik::SessionId::parse(id)
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
}
