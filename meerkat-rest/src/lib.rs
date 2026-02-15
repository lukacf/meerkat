//! meerkat-rest - REST API server for Meerkat
//!
//! Provides HTTP endpoints for running and managing Meerkat agents:
//! - POST /sessions - Create and run a new agent
//! - POST /sessions/:id/messages - Continue an existing session
//! - POST /comms/send - Send a canonical comms command
//! - GET /comms/peers - List peers visible to a session
//! - POST /sessions/:id/event - (legacy) Push an external event to a session
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
    AgentEvent, AgentFactory, FactoryAgentBuilder, LlmClient, OutputSchema,
    PersistentSessionService, Session, SessionId, SessionService,
    encode_llm_client_override_for_service,
};
use meerkat_core::error::invalid_session_id_message;
use meerkat_core::service::{
    CreateSessionRequest as SvcCreateSessionRequest, SessionBuildOptions, SessionError,
    StartTurnRequest as SvcStartTurnRequest,
};
use meerkat_core::{
    Config, ConfigDelta, ConfigEnvelope, ConfigStore, FileConfigStore, HookRunOverrides, Provider,
    RealmSelection, RuntimeBootstrap, SessionTooling, format_verbose_event,
};
use meerkat_store::{RealmBackend, RealmOrigin};
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
    /// Session service for managing agent lifecycle.
    pub session_service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    /// Webhook authentication, resolved once at startup from RKAT_WEBHOOK_SECRET.
    pub webhook_auth: webhook::WebhookAuth,
    pub realm_id: String,
    pub instance_id: Option<String>,
    pub backend: String,
    pub resolved_paths: meerkat_core::ConfigResolvedPaths,
    pub config_runtime: Arc<meerkat_core::ConfigRuntime>,
}

#[derive(Debug, Clone)]
pub struct SessionEvent {
    session_id: SessionId,
    event: AgentEvent,
}

impl AppState {
    pub async fn load() -> Result<Self, Box<dyn std::error::Error>> {
        Self::load_with_bootstrap(RuntimeBootstrap::default()).await
    }

    #[cfg(test)]
    async fn load_from(instance_root: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let mut bootstrap = RuntimeBootstrap::default();
        bootstrap.realm.state_root = Some(instance_root.join("realms"));
        bootstrap.context.context_root = Some(instance_root.clone());
        Self::load_from_with_bootstrap(instance_root, bootstrap).await
    }

    pub async fn load_with_bootstrap(
        bootstrap: RuntimeBootstrap,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::load_from_with_bootstrap(rest_instance_root(), bootstrap).await
    }

    async fn load_from_with_bootstrap(
        instance_root: PathBuf,
        bootstrap: RuntimeBootstrap,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (event_tx, _) = broadcast::channel(256);
        let locator = bootstrap.realm.resolve_locator()?;
        let realm_id = locator.realm_id;
        let instance_id = bootstrap.realm.instance_id;
        let backend_hint = bootstrap
            .realm
            .backend_hint
            .as_deref()
            .and_then(parse_backend_hint)
            .or(Some(RealmBackend::Redb));
        let origin_hint = Some(realm_origin_from_selection(&bootstrap.realm.selection));
        let realms_root = locator.state_root;
        let (manifest, session_store) = meerkat_store::open_realm_session_store_in(
            &realms_root,
            &realm_id,
            backend_hint,
            origin_hint,
        )
        .await?;
        let realm_paths = meerkat_store::realm_paths_in(&realms_root, &realm_id);
        let resolved_paths = meerkat_core::ConfigResolvedPaths {
            root: realm_paths.root.display().to_string(),
            manifest_path: realm_paths.manifest_path.display().to_string(),
            config_path: realm_paths.config_path.display().to_string(),
            sessions_redb_path: realm_paths.sessions_redb_path.display().to_string(),
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

        let mut config = config_store
            .get()
            .await
            .unwrap_or_else(|_| Config::default());
        if let Err(err) = config.apply_env_overrides() {
            tracing::warn!("Failed to apply env overrides: {}", err);
        }

        let store_path = match manifest.backend {
            meerkat_store::RealmBackend::Jsonl => realm_paths.sessions_jsonl_dir.clone(),
            meerkat_store::RealmBackend::Redb => realm_paths.root.clone(),
        };

        let enable_builtins = config.tools.builtins_enabled;
        let enable_shell = config.tools.shell_enabled;

        let default_model = Cow::Owned(config.agent.model.to_string());
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

        let builder =
            FactoryAgentBuilder::new_with_config_store(factory, config, Arc::clone(&config_store));
        let session_service = Arc::new(PersistentSessionService::new(builder, 100, session_store));

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
            config_runtime,
        })
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
    /// Enable sub-agent tools. Omit to use factory defaults.
    #[serde(default)]
    pub enable_subagents: Option<bool>,
    /// Enable semantic memory. Omit to use factory defaults.
    #[serde(default)]
    pub enable_memory: Option<bool>,
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
        .route("/comms/send", post(comms_send))
        .route("/comms/peers", get(comms_peers))
        // BRIDGE(M11→M12): Legacy event push endpoint.
        .route("/sessions/{id}/event", post(push_event))
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
}

/// Canonical comms peers request body.
#[derive(Debug, Deserialize)]
pub struct CommsPeersRequest {
    pub session_id: String,
}

/// POST /comms/send — dispatch a canonical comms command.
async fn comms_send(
    State(state): State<AppState>,
    Json(req): Json<CommsSendRequest>,
) -> Result<Json<Value>, ApiError> {
    use meerkat_core::comms::SendReceipt;

    let session_id = SessionId::parse(&req.session_id)
        .map_err(|e| ApiError::BadRequest(invalid_session_id_message(e)))?;

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
        intent: req.intent.clone(),
        params: req.params.clone(),
        in_reply_to: req.in_reply_to.clone(),
        status: req.status.clone(),
        result: req.result.clone(),
        source: req.source.clone(),
        stream: req.stream.clone(),
        allow_self_session: req.allow_self_session,
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
    let session_id = SessionId::parse(&params.session_id)
        .map_err(|e| ApiError::BadRequest(invalid_session_id_message(e)))?;

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
            })
        })
        .collect();

    Ok(Json(json!({ "peers": entries })))
}

/// Push an external event to a session's inbox.
///
/// The event is queued for processing at the next turn boundary — it does NOT
/// trigger an immediate LLM call. Returns 202 Accepted on success.
///
/// Authentication is controlled by `RKAT_WEBHOOK_SECRET` env var:
/// - If not set: no authentication (suitable for localhost/dev)
/// - If set: requires `X-Webhook-Secret` header with matching value
async fn push_event(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    // Webhook auth resolved once at startup, stored in AppState.
    webhook::verify_webhook(&headers, &state.webhook_auth)
        .map_err(|msg| ApiError::Unauthorized(msg.to_string()))?;

    // Validate session ID
    let session_id =
        SessionId::parse(&id).map_err(|e| ApiError::BadRequest(invalid_session_id_message(e)))?;

    // Get event injector for this session
    let injector = state
        .session_service
        .event_injector(&session_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Session not found: {session_id}")))?;

    // Format payload as pretty JSON for the agent
    let body = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string());

    // Inject the event
    match injector.inject(body, meerkat_core::PlainEventSource::Webhook) {
        Ok(()) => Ok((StatusCode::ACCEPTED, Json(json!({"queued": true})))),
        Err(meerkat_core::EventInjectorError::Full) => Err(ApiError::ServiceUnavailable(
            "Event inbox is full — try again later".to_string(),
        )),
        Err(meerkat_core::EventInjectorError::Closed) => {
            Err(ApiError::Gone("Session has been shut down".to_string()))
        }
    }
}

/// Get runtime capabilities with status resolved against config.
async fn get_capabilities(
    State(state): State<AppState>,
) -> Json<meerkat_contracts::CapabilitiesResponse> {
    let config = state.config_store.get().await.unwrap_or_default();
    Json(meerkat::surface::build_capabilities_response(&config))
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
    Ok(Json(snapshot.into()))
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
    let snapshot = state
        .config_runtime
        .set(config, expected_generation)
        .await
        .map_err(config_runtime_err_to_api)?;
    Ok(Json(snapshot.into()))
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
    let snapshot = state
        .config_runtime
        .patch(ConfigDelta(delta), expected_generation)
        .await
        .map_err(config_runtime_err_to_api)?;
    Ok(Json(snapshot.into()))
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

    let current_generation = state.config_runtime.get().await.ok().map(|s| s.generation);
    let build = SessionBuildOptions {
        provider: req.provider,
        output_schema: req.output_schema,
        structured_output_retries: req.structured_output_retries,
        hooks_override: req.hooks_override.unwrap_or_default(),
        comms_name: req.comms_name.clone(),
        peer_meta: req.peer_meta.clone(),
        resume_session: Some(pre_session),
        budget_limits: None,
        provider_params: None,
        external_tools: None,
        llm_client_override: state
            .llm_client_override
            .clone()
            .map(encode_llm_client_override_for_service),
        override_builtins: req.enable_builtins,
        override_shell: req.enable_shell,
        override_subagents: req.enable_subagents,
        override_memory: req.enable_memory,
        preload_skills: None,
        realm_id: Some(state.realm_id.clone()),
        instance_id: state.instance_id.clone(),
        backend: Some(state.backend.clone()),
        config_generation: current_generation,
    };

    let svc_req = SvcCreateSessionRequest {
        model: model.to_string(),
        prompt: req.prompt,
        system_prompt: req.system_prompt,
        max_tokens: Some(max_tokens),
        event_tx: Some(caller_event_tx),
        host_mode,
        skill_references: None,
        build: Some(build),
    };

    let result = state.session_service.create_session(svc_req).await;

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

    let view = state
        .session_service
        .read(&session_id)
        .await
        .map_err(|e| match e {
            SessionError::NotFound { .. } => {
                ApiError::NotFound(format!("Session not found: {}", id))
            }
            _ => ApiError::Internal(format!("{e}")),
        })?;

    let created_at: DateTime<Utc> = view.state.created_at.into();
    let updated_at: DateTime<Utc> = view.state.updated_at.into();

    Ok(Json(SessionDetailsResponse {
        session_id: view.state.session_id.to_string(),
        created_at: created_at.to_rfc3339(),
        updated_at: updated_at.to_rfc3339(),
        message_count: view.state.message_count,
        total_tokens: view.billing.total_tokens,
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
        skill_references: None,
    };

    let result = state.session_service.start_turn(&session_id, svc_req).await;

    let final_result = match result {
        Ok(run_result) => Ok(run_result),
        Err(SessionError::NotFound { .. }) => {
            // The session isn't live in the service. Load it from the persistent
            // store, stage a build config with resume_session, and create a new
            // service session.
            let session = state
                .session_service
                .load_persisted(&session_id)
                .await
                .map_err(|e| ApiError::Internal(format!("{e}")))?
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

            let current_generation = state.config_runtime.get().await.ok().map(|s| s.generation);
            let build = SessionBuildOptions {
                provider,
                output_schema: req.output_schema,
                structured_output_retries: req.structured_output_retries,
                hooks_override: req.hooks_override.unwrap_or_default(),
                comms_name,
                resume_session: Some(session),
                budget_limits: None,
                provider_params: None,
                external_tools: None,
                llm_client_override: state
                    .llm_client_override
                    .clone()
                    .map(encode_llm_client_override_for_service),
                override_builtins: Some(tooling.builtins),
                override_shell: Some(tooling.shell),
                override_subagents: Some(tooling.subagents),
                override_memory: None,
                preload_skills: None,
                peer_meta: req
                    .peer_meta
                    .clone()
                    .or_else(|| stored_metadata.as_ref().and_then(|m| m.peer_meta.clone())),
                realm_id: stored_metadata
                    .as_ref()
                    .and_then(|m| m.realm_id.clone())
                    .or_else(|| Some(state.realm_id.clone())),
                instance_id: stored_metadata
                    .as_ref()
                    .and_then(|m| m.instance_id.clone())
                    .or_else(|| state.instance_id.clone()),
                backend: stored_metadata
                    .as_ref()
                    .and_then(|m| m.backend.clone())
                    .or_else(|| Some(state.backend.clone())),
                config_generation: current_generation,
            };

            let svc_req = SvcCreateSessionRequest {
                model: model.to_string(),
                prompt: req.prompt,
                system_prompt: req.system_prompt,
                max_tokens: Some(max_tokens),
                event_tx: Some(caller_event_tx.clone()),
                host_mode: continue_host_mode,
                skill_references: None,
                build: Some(build),
            };

            state
                .session_service
                .create_session(svc_req)
                .await
                .map_err(|e| ApiError::Agent(format!("{e}")))
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

    // load_persisted() only checks the redb store. A session on its first
    // in-progress turn (not yet persisted) will return 404. Future improvement:
    // try read() first for message_count (works for live sessions), fall back
    // to load_persisted() for inactive sessions.
    let session = state
        .session_service
        .load_persisted(&session_id)
        .await
        .map_err(|e| ApiError::Internal(format!("{e}")))?
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
    Unauthorized(String),
    NotFound(String),
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
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
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
        let state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
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
}
