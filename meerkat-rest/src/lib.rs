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
use meerkat_contracts::{SessionLocator, format_session_ref};
use meerkat_core::service::{
    CreateSessionRequest as SvcCreateSessionRequest, InitialTurnPolicy, SessionBuildOptions,
    SessionError, StartTurnRequest as SvcStartTurnRequest,
};
use meerkat_core::EventEnvelope;
use meerkat_core::{
    Config, ConfigDelta, ConfigEnvelope, ConfigEnvelopePolicy, ConfigStore, FileConfigStore,
    HookRunOverrides, Provider, RealmSelection, RuntimeBootstrap, SessionTooling,
    format_verbose_event,
};
use meerkat_store::{RealmBackend, RealmOrigin};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

#[cfg(feature = "mcp")]
use meerkat::{
    AgentToolDispatcher, McpLifecycleAction, McpReloadTarget, McpRouter, McpRouterAdapter,
};
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
    /// Per-session MCP adapter state (live MCP mutation).
    #[cfg(feature = "mcp")]
    pub mcp_sessions: Arc<RwLock<std::collections::HashMap<SessionId, SessionMcpState>>>,
}

#[derive(Debug, Clone)]
pub struct SessionEvent {
    session_id: SessionId,
    event: EventEnvelope<AgentEvent>,
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

        let store_path = match manifest.backend {
            meerkat_store::RealmBackend::Jsonl => realm_paths.sessions_jsonl_dir.clone(),
            meerkat_store::RealmBackend::Redb => realm_paths.root.clone(),
        };

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
            expose_paths,
            config_runtime,
            realm_lease: Arc::new(tokio::sync::Mutex::new(Some(lease))),
            skill_runtime,
            #[cfg(feature = "mcp")]
            mcp_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
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
    pub session_ref: String,
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
    let r = Router::new()
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
        .route("/skills", get(list_skills))
        .route("/skills/{id}", get(inspect_skill))
        .route("/capabilities", get(get_capabilities));

    #[cfg(feature = "mcp")]
    let r = r
        .route("/sessions/{id}/mcp/add", post(mcp_add))
        .route("/sessions/{id}/mcp/remove", post(mcp_remove))
        .route("/sessions/{id}/mcp/reload", post(mcp_reload));

    r.with_state(state)
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
    let session_id = resolve_session_id_for_state(&id, &state)?;

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
) -> Result<Json<meerkat_contracts::SkillInspectResponse>, ApiError> {
    let runtime = state
        .skill_runtime
        .as_ref()
        .ok_or_else(|| ApiError::NotFound("skills not enabled".into()))?;

    let doc = runtime
        .load_from_source(&meerkat_core::skills::SkillId::from(id.as_str()), None)
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

/// Map a `SessionError` to an `ApiError`, handling `CallbackPending` specially
/// by returning a successful response with the pre-created session_id.
fn session_error_to_api_result(
    err: SessionError,
    fallback_session_id: &SessionId,
    realm_id: &str,
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
                    session_ref: Some(format_session_ref(realm_id, fallback_session_id)),
                    text: "Agent is waiting for tool results".to_string(),
                    turns: 0,
                    tool_calls: 0,
                    usage: UsageResponse::default(),
                    structured_output: None,
                    schema_warnings: None,
                    skill_diagnostics: None,
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
        budget_limits: None,
        provider_params: None,
        external_tools: mcp_external_tools,
        llm_client_override: state
            .llm_client_override
            .clone()
            .map(encode_llm_client_override_for_service),
        scoped_event_tx: None,
        scoped_event_path: None,
        override_builtins: req.enable_builtins,
        override_shell: req.enable_shell,
        override_subagents: req.enable_subagents,
        override_memory: req.enable_memory,
        override_mob: None,
        preload_skills: None,
        realm_id: Some(state.realm_id.clone()),
        instance_id: state.instance_id.clone(),
        backend: Some(state.backend.clone()),
        config_generation: current_generation,
        checkpointer: None,
        silent_comms_intents: Vec::new(),
        max_inline_peer_notifications: None,
    };

    let svc_req = SvcCreateSessionRequest {
        model: model.to_string(),
        prompt: req.prompt,
        system_prompt: req.system_prompt,
        max_tokens: Some(max_tokens),
        event_tx: Some(caller_event_tx),
        host_mode,
        skill_references: None,
        initial_turn: InitialTurnPolicy::RunImmediately,
        build: Some(build),
    };

    let result = state.session_service.create_session(svc_req).await;

    // Wait for the event forwarder to drain
    let _ = forward_task.await;

    match result {
        Ok(run_result) => Ok(Json(run_result_to_response(run_result, &state.realm_id))),
        Err(err) => {
            // Clean up MCP adapter state on session creation failure.
            #[cfg(feature = "mcp")]
            cleanup_mcp_session(&state, &session_id).await;
            session_error_to_api_result(err, &session_id, &state.realm_id)
        }
    }
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
    }))
}

/// Continue an existing session
async fn continue_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ContinueSessionRequest>,
) -> Result<Json<SessionResponse>, ApiError> {
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

    // Apply staged MCP operations at the turn boundary.
    #[allow(unused_mut)]
    let mut turn_prompt = req.prompt.clone();
    #[cfg(feature = "mcp")]
    apply_mcp_boundary(&state, &session_id, &caller_event_tx, &mut turn_prompt).await?;

    // First, try to start a turn on a live session in the service.
    let svc_req = SvcStartTurnRequest {
        prompt: turn_prompt,
        event_tx: Some(caller_event_tx.clone()),
        host_mode,
        skill_references: None,
        flow_tool_overlay: None,
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
                .ok_or_else(|| ApiError::NotFound(format!("Session not found: {id}")))?;

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
                    mob: false,
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
            let continue_host_mode_requested =
                host_mode_requested || stored_metadata.as_ref().is_some_and(|meta| meta.host_mode);
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
                scoped_event_tx: None,
                scoped_event_path: None,
                override_builtins: Some(tooling.builtins),
                override_shell: Some(tooling.shell),
                override_subagents: Some(tooling.subagents),
                override_memory: None,
                override_mob: Some(tooling.mob),
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
                checkpointer: None,
                silent_comms_intents: Vec::new(),
                max_inline_peer_notifications: None,
            };

            let svc_req = SvcCreateSessionRequest {
                model: model.to_string(),
                prompt: req.prompt,
                system_prompt: req.system_prompt,
                max_tokens: Some(max_tokens),
                event_tx: Some(caller_event_tx.clone()),
                host_mode: continue_host_mode,
                skill_references: None,
                initial_turn: InitialTurnPolicy::RunImmediately,
                build: Some(build),
            };

            state
                .session_service
                .create_session(svc_req)
                .await
                .map_err(|e| ApiError::Agent(format!("{e}")))
        }
        Err(err) => return session_error_to_api_result(err, &session_id, &state.realm_id),
    };

    // Drop the sender so the forwarder sees channel closure and can drain.
    drop(caller_event_tx);

    // Wait for the event forwarder to drain
    let _ = forward_task.await;

    match final_result {
        Ok(run_result) => Ok(Json(run_result_to_response(run_result, &state.realm_id))),
        Err(err) => Err(err),
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

                    let json = serde_json::to_value(&payload.event).unwrap_or_else(|_| {
                        json!({
                            "payload": {"type": "unknown"}
                        })
                    });
                    let event_type = json
                        .get("payload")
                        .and_then(|v| v.get("type"))
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
    let delta = adapter
        .apply_staged()
        .await
        .map_err(|e| ApiError::Internal(format!("failed to apply staged MCP operations: {e}")))?;

    // Spawn drain task if any removals started.
    if delta
        .lifecycle_actions
        .iter()
        .any(|a| matches!(a, McpLifecycleAction::RemovingStarted { .. }))
    {
        spawn_mcp_drain_task(adapter, drain_task_running, lifecycle_tx);
    }

    queued_actions.extend(delta.lifecycle_actions);
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
