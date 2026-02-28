//! meerkat-mcp-server - MCP server exposing Meerkat as a tool
//!
//! This crate provides an MCP server that exposes Meerkat agent capabilities
//! as MCP tools: meerkat_run and meerkat_resume.

#[cfg(test)]
use meerkat::SessionStore;
use meerkat::{
    AgentFactory, FactoryAgentBuilder, OutputSchema, PersistentSessionService, ToolError,
    ToolResult,
};
use meerkat_contracts::SkillsParams;
use meerkat_core::error::invalid_session_id_message;
use meerkat_core::service::{
    CreateSessionRequest, InitialTurnPolicy, SessionBuildOptions, SessionError, SessionService,
    StartTurnRequest,
};
use meerkat_core::{
    AgentEvent, Config, ConfigDelta, ConfigEnvelope, ConfigEnvelopePolicy, ConfigRuntimeError,
    ConfigStore, EventEnvelope, FileConfigStore, HookRunOverrides, Provider, RealmSelection,
    RuntimeBootstrap, Session, ToolCallView, format_verbose_event,
};
use meerkat_mcp::{McpReloadTarget, McpRouter};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

use futures::StreamExt;
use tokio::sync::Mutex;

/// Tool definition provided by the MCP client
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpToolDef {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// JSON Schema for tool input
    pub input_schema: Value,
    /// Handler type: "callback" means the tool result will be provided via meerkat_resume
    #[serde(default)]
    pub handler: Option<String>,
}

impl McpToolDef {
    pub fn handler_kind(&self) -> &str {
        self.handler.as_deref().unwrap_or("callback")
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProviderInput {
    Anthropic,
    Openai,
    Gemini,
    Other,
}

impl ProviderInput {
    pub fn to_provider(self) -> Provider {
        match self {
            ProviderInput::Anthropic => Provider::Anthropic,
            ProviderInput::Openai => Provider::OpenAI,
            ProviderInput::Gemini => Provider::Gemini,
            ProviderInput::Other => Provider::Other,
        }
    }
}

/// Input schema for meerkat_run tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatRunInput {
    pub prompt: String,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub provider: Option<ProviderInput>,
    /// JSON schema for structured output extraction (wrapper or raw schema).
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// Max retries for structured output validation (default: 2).
    #[serde(default = "default_structured_output_retries")]
    pub structured_output_retries: u32,
    /// Stream agent events to the MCP client via notifications.
    #[serde(default)]
    pub stream: bool,
    /// Enable verbose event logging (server-side).
    #[serde(default)]
    pub verbose: bool,
    /// Tool definitions for the agent to use
    #[serde(default)]
    pub tools: Vec<McpToolDef>,
    /// Enable built-in tools (task management, shell, etc.)
    #[serde(default)]
    pub enable_builtins: bool,
    /// Configuration for built-in tools (only used when enable_builtins is true)
    #[serde(default)]
    pub builtin_config: Option<BuiltinConfigInput>,
    /// Run in host mode: process prompt then stay alive listening for comms messages.
    /// Requires comms_name to be set.
    #[serde(default)]
    pub host_mode: bool,
    /// Agent name for inter-agent communication. Required for host_mode.
    #[serde(default)]
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery (name, description, labels).
    #[serde(default)]
    pub peer_meta: Option<meerkat_core::PeerMeta>,
    /// Optional run-scoped hook overrides.
    #[serde(default)]
    pub hooks_override: Option<HookRunOverrides>,
    /// Enable sub-agent tools.
    #[serde(default)]
    pub enable_subagents: Option<bool>,
    /// Enable semantic memory.
    #[serde(default)]
    pub enable_memory: Option<bool>,
    /// Enable mob tools.
    #[serde(default)]
    pub enable_mob: Option<bool>,
    /// Provider-specific parameters (e.g., thinking config).
    #[serde(default)]
    pub provider_params: Option<serde_json::Value>,
    /// Explicit budget limits for this run.
    #[serde(default)]
    pub budget_limits: Option<BudgetLimitsInput>,
    /// Skills to preload into the system prompt.
    #[serde(default)]
    pub preload_skills: Option<Vec<String>>,
    /// Structured refs for per-turn skill injection.
    #[serde(default)]
    pub skill_refs: Option<Vec<meerkat_core::skills::SkillRef>>,
    /// Legacy compatibility skill refs for per-turn injection.
    #[serde(default)]
    pub skill_references: Option<Vec<String>>,
}

fn default_structured_output_retries() -> u32 {
    2
}

/// Configuration options for built-in tools
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct BuiltinConfigInput {
    /// Enable shell tools (default: false)
    #[serde(default)]
    pub enable_shell: bool,
    /// Default timeout for shell commands in seconds (default: 30)
    #[serde(default)]
    pub shell_timeout_secs: Option<u64>,
}

/// Actions supported by the config tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigAction {
    Get,
    Set,
    Patch,
}

/// Input schema for meerkat_config tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatConfigInput {
    pub action: ConfigAction,
    #[serde(default)]
    pub config: Option<Value>,
    #[serde(default)]
    pub patch: Option<Value>,
    #[serde(default)]
    pub expected_generation: Option<u64>,
}

/// Structured MCP tool-call error payload.
#[derive(Debug, Clone)]
pub struct ToolCallError {
    pub code: i32,
    pub message: String,
    pub data: Option<Value>,
}

impl ToolCallError {
    fn new(code: i32, message: impl Into<String>, data: Option<Value>) -> Self {
        Self {
            code,
            message: message.into(),
            data,
        }
    }

    fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(-32602, message, None)
    }

    fn method_not_found(message: impl Into<String>) -> Self {
        Self::new(-32601, message, None)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new(-32603, message, None)
    }
}

fn map_config_runtime_error(err: ConfigRuntimeError) -> ToolCallError {
    match err {
        ConfigRuntimeError::GenerationConflict { expected, current } => ToolCallError::new(
            -32602,
            format!("generation conflict: expected {expected}, current {current}"),
            Some(json!({
                "type": "generation_conflict",
                "expected_generation": expected,
                "current_generation": current
            })),
        ),
        other => ToolCallError::new(
            -32603,
            other.to_string(),
            Some(json!({ "type": "config_runtime_error" })),
        ),
    }
}

async fn load_config_async(
    realm_id: &str,
    realms_root: &std::path::Path,
    backend_hint: Option<meerkat_store::RealmBackend>,
    origin_hint: Option<meerkat_store::RealmOrigin>,
    instance_id: Option<&str>,
) -> Config {
    let store = match realm_config_store(
        realm_id,
        realms_root,
        backend_hint,
        origin_hint,
        instance_id,
    )
    .await
    {
        Ok(store) => store,
        Err(_) => return Config::default(),
    };
    let mut config = store.get().await.unwrap_or_else(|_| Config::default());
    let _ = config.apply_env_overrides();
    config
}

fn resolve_host_mode(requested: bool) -> Result<bool, String> {
    #[cfg(feature = "comms")]
    {
        meerkat::surface::resolve_host_mode(requested)
    }
    #[cfg(not(feature = "comms"))]
    {
        if requested {
            return Err(
                "host_mode requires comms support (build with --features comms)".to_string(),
            );
        }
        Ok(false)
    }
}

fn tagged_realm_config_store(
    realms_root: &std::path::Path,
    realm_id: &str,
    backend: meerkat_store::RealmBackend,
    instance_id: Option<&str>,
) -> Arc<dyn ConfigStore> {
    let paths = meerkat_store::realm_paths_in(realms_root, realm_id);
    let base: Arc<dyn ConfigStore> = Arc::new(FileConfigStore::new(paths.config_path.clone()));
    let tagged = meerkat_core::TaggedConfigStore::new(
        base,
        meerkat_core::ConfigStoreMetadata {
            realm_id: Some(realm_id.to_string()),
            instance_id: instance_id.map(ToOwned::to_owned),
            backend: Some(backend.as_str().to_string()),
            resolved_paths: Some(meerkat_core::ConfigResolvedPaths {
                root: paths.root.display().to_string(),
                manifest_path: paths.manifest_path.display().to_string(),
                config_path: paths.config_path.display().to_string(),
                sessions_redb_path: paths.sessions_redb_path.display().to_string(),
                sessions_jsonl_dir: paths.sessions_jsonl_dir.display().to_string(),
            }),
        },
    );
    Arc::new(tagged)
}

async fn realm_config_store(
    realm_id: &str,
    realms_root: &std::path::Path,
    backend_hint: Option<meerkat_store::RealmBackend>,
    origin_hint: Option<meerkat_store::RealmOrigin>,
    instance_id: Option<&str>,
) -> Result<Arc<dyn ConfigStore>, String> {
    let (manifest, _) = meerkat_store::open_realm_session_store_in(
        realms_root,
        realm_id,
        backend_hint.or(Some(meerkat_store::RealmBackend::Redb)),
        origin_hint,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(tagged_realm_config_store(
        realms_root,
        realm_id,
        manifest.backend,
        instance_id,
    ))
}

fn realm_store_path(
    realms_root: &std::path::Path,
    realm_id: &str,
    backend: meerkat_store::RealmBackend,
) -> PathBuf {
    let paths = meerkat_store::realm_paths_in(realms_root, realm_id);
    match backend {
        meerkat_store::RealmBackend::Jsonl => paths.sessions_jsonl_dir,
        meerkat_store::RealmBackend::Redb => paths.root,
    }
}

/// Shared state for the MCP server, holding the session service.
///
/// The service is configured once with max-permissive factory flags
/// (`builtins: true`, `shell: true`). Per-request tool configuration is
/// controlled via `override_builtins` / `override_shell` in `SessionBuildOptions`.
pub struct MeerkatMcpState {
    service: PersistentSessionService<FactoryAgentBuilder>,
    realm_id: String,
    backend: String,
    instance_id: Option<String>,
    expose_paths: bool,
    config_runtime: Arc<meerkat_core::ConfigRuntime>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
    mcp_adapters: Arc<Mutex<HashMap<String, Arc<meerkat_mcp::McpRouterAdapter>>>>,
    session_event_streams: Arc<Mutex<HashMap<String, Arc<SessionEventStreamHandle>>>>,
    #[cfg(feature = "comms")]
    comms_streams: Arc<Mutex<HashMap<String, Arc<CommsStreamHandle>>>>,
    _realm_lease: Option<meerkat_store::RealmLeaseGuard>,
}

struct SessionEventStreamHandle {
    stream: Mutex<meerkat_core::EventStream>,
}

#[cfg(feature = "comms")]
struct CommsStreamHandle {
    stream: Mutex<meerkat_core::comms::EventStream>,
}

impl MeerkatMcpState {
    /// Create a new MCP state with a session service backed by `AgentFactory`.
    pub async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_bootstrap_and_options(RuntimeBootstrap::default(), false).await
    }

    pub async fn new_with_bootstrap(
        bootstrap: RuntimeBootstrap,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::new_with_bootstrap_and_options(bootstrap, false).await
    }

    pub async fn new_with_bootstrap_and_options(
        bootstrap: RuntimeBootstrap,
        expose_paths: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let locator = bootstrap.realm.resolve_locator()?;
        let realm_id = locator.realm_id;
        let realms_root = locator.state_root;
        let backend_hint = bootstrap
            .realm
            .backend_hint
            .as_deref()
            .and_then(parse_backend_hint)
            .or(Some(meerkat_store::RealmBackend::Redb));
        let origin_hint = Some(realm_origin_from_selection(&bootstrap.realm.selection));
        let config = load_config_async(
            &realm_id,
            &realms_root,
            backend_hint,
            origin_hint,
            bootstrap.realm.instance_id.as_deref(),
        )
        .await;
        let (manifest, session_store) = meerkat_store::open_realm_session_store_in(
            &realms_root,
            &realm_id,
            backend_hint,
            origin_hint,
        )
        .await?;
        let store_path = realm_store_path(&realms_root, &realm_id, manifest.backend);
        let realm_paths = meerkat_store::realm_paths_in(&realms_root, &realm_id);
        let conventions_context_root = bootstrap.context.context_root.clone();
        let project_root = conventions_context_root
            .clone()
            .unwrap_or_else(|| realm_paths.root.clone());
        let config_store = tagged_realm_config_store(
            &realms_root,
            &realm_id,
            manifest.backend,
            bootstrap.realm.instance_id.as_deref(),
        );
        let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
            Arc::clone(&config_store),
            realm_paths.root.join("config_state.json"),
        ));
        let lease = meerkat_store::start_realm_lease_in(
            &realms_root,
            &realm_id,
            bootstrap.realm.instance_id.as_deref(),
            "rkat-mcp",
        )
        .await?;

        // Create factory with max-permissive flags; per-request overrides
        // in SessionBuildOptions control what tools are actually enabled.
        let mut factory = AgentFactory::new(store_path)
            .session_store(session_store.clone())
            .runtime_root(realm_paths.root)
            .project_root(project_root)
            .builtins(true)
            .shell(true);
        if let Some(context_root) = conventions_context_root {
            factory = factory.context_root(context_root);
        }
        if let Some(user_root) = bootstrap.context.user_config_root.clone() {
            factory = factory.user_config_root(user_root);
        }

        let skill_runtime = factory.build_skill_runtime(&config).await;

        let builder = FactoryAgentBuilder::new_with_config_store(factory, config, config_store);
        let service = PersistentSessionService::new(builder, 100, session_store);

        Ok(Self {
            service,
            realm_id,
            backend: manifest.backend.as_str().to_string(),
            instance_id: bootstrap.realm.instance_id,
            expose_paths,
            config_runtime,
            skill_runtime,
            mob_state: meerkat_mob_mcp::MobMcpState::new_in_memory(),
            mcp_adapters: Arc::new(Mutex::new(HashMap::new())),
            session_event_streams: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(feature = "comms")]
            comms_streams: Arc::new(Mutex::new(HashMap::new())),
            _realm_lease: Some(lease),
        })
    }

    /// Test constructor that accepts an injected store (avoids opening redb at platform data dir).
    #[cfg(test)]
    pub(crate) async fn new_with_store(store: Arc<dyn SessionStore>) -> Self {
        let bootstrap = RuntimeBootstrap::default();
        let locator = match bootstrap.realm.resolve_locator() {
            Ok(locator) => locator,
            Err(_) => meerkat_core::RealmLocator {
                state_root: meerkat_core::default_state_root(),
                realm_id: meerkat_core::generate_realm_id(),
            },
        };
        let realm_id = locator.realm_id;
        let realms_root = locator.state_root;
        let config = load_config_async(
            &realm_id,
            &realms_root,
            Some(meerkat_store::RealmBackend::Redb),
            Some(meerkat_store::RealmOrigin::Generated),
            bootstrap.realm.instance_id.as_deref(),
        )
        .await;
        let store_path =
            realm_store_path(&realms_root, &realm_id, meerkat_store::RealmBackend::Redb);
        let realm_paths = meerkat_store::realm_paths_in(&realms_root, &realm_id);
        let project_root = realm_paths.root.clone();
        let config_store = tagged_realm_config_store(
            &realms_root,
            &realm_id,
            meerkat_store::RealmBackend::Redb,
            bootstrap.realm.instance_id.as_deref(),
        );
        let config_runtime = Arc::new(meerkat_core::ConfigRuntime::new(
            Arc::clone(&config_store),
            realm_paths.root.join("config_state.json"),
        ));

        let mut factory = AgentFactory::new(store_path)
            .runtime_root(realm_paths.root)
            .project_root(project_root)
            .builtins(true)
            .shell(true);
        if let Some(user_root) = bootstrap.context.user_config_root.clone() {
            factory = factory.user_config_root(user_root);
        }

        let builder = FactoryAgentBuilder::new_with_config_store(factory, config, config_store);
        let service = PersistentSessionService::new(builder, 100, store);

        Self {
            service,
            realm_id,
            backend: "redb".to_string(),
            instance_id: bootstrap.realm.instance_id,
            expose_paths: false,
            config_runtime,
            skill_runtime: None,
            mob_state: meerkat_mob_mcp::MobMcpState::new_in_memory(),
            mcp_adapters: Arc::new(Mutex::new(HashMap::new())),
            session_event_streams: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(feature = "comms")]
            comms_streams: Arc::new(Mutex::new(HashMap::new())),
            _realm_lease: None,
        }
    }

    pub fn realm_id(&self) -> &str {
        &self.realm_id
    }

    pub fn backend(&self) -> &str {
        &self.backend
    }

    pub fn expose_paths(&self) -> bool {
        self.expose_paths
    }

    async fn upsert_mcp_adapter(
        &self,
        session_id: &meerkat::SessionId,
        adapter: Arc<meerkat_mcp::McpRouterAdapter>,
    ) {
        self.mcp_adapters
            .lock()
            .await
            .insert(session_id.to_string(), adapter);
    }

    async fn mcp_adapter_for_session(
        &self,
        session_id: &meerkat::SessionId,
    ) -> Result<Arc<meerkat_mcp::McpRouterAdapter>, String> {
        if self.service.read(session_id).await.is_err() {
            return Err(format!("Session not found: {session_id}"));
        }
        self.mcp_adapters
            .lock()
            .await
            .get(&session_id.to_string())
            .cloned()
            .ok_or_else(|| {
                "Live MCP unavailable for this session. Resume the session with this MCP server before staging live MCP changes.".to_string()
            })
    }
}

fn parse_backend_hint(raw: &str) -> Option<meerkat_store::RealmBackend> {
    match raw {
        "jsonl" => Some(meerkat_store::RealmBackend::Jsonl),
        "redb" => Some(meerkat_store::RealmBackend::Redb),
        _ => None,
    }
}

fn realm_origin_from_selection(selection: &RealmSelection) -> meerkat_store::RealmOrigin {
    match selection {
        RealmSelection::Explicit { .. } => meerkat_store::RealmOrigin::Explicit,
        RealmSelection::Isolated => meerkat_store::RealmOrigin::Generated,
        RealmSelection::WorkspaceDerived { .. } => meerkat_store::RealmOrigin::Workspace,
    }
}

/// Input schema for meerkat_resume tool
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatResumeInput {
    pub session_id: String,
    pub prompt: String,
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Stream agent events to the MCP client via notifications.
    #[serde(default)]
    pub stream: bool,
    /// Enable verbose event logging (server-side).
    #[serde(default)]
    pub verbose: bool,
    /// Tool definitions for the agent to use (should match the original run)
    #[serde(default)]
    pub tools: Vec<McpToolDef>,
    /// Tool results to provide for pending tool calls
    #[serde(default)]
    pub tool_results: Vec<ToolResultInput>,
    /// Enable built-in tools (task management, shell, etc.)
    #[serde(default)]
    pub enable_builtins: bool,
    /// Configuration for built-in tools (only used when enable_builtins is true)
    #[serde(default)]
    pub builtin_config: Option<BuiltinConfigInput>,
    /// Run in host mode: process prompt then stay alive listening for comms messages.
    #[serde(default)]
    pub host_mode: bool,
    /// Agent name for inter-agent communication. Required for host_mode.
    #[serde(default)]
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery.
    #[serde(default)]
    pub peer_meta: Option<meerkat_core::PeerMeta>,
    /// Optional model override for resume.
    #[serde(default)]
    pub model: Option<String>,
    /// Optional max_tokens override for resume.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Optional provider override for resume.
    #[serde(default)]
    pub provider: Option<ProviderInput>,
    /// JSON schema for structured output extraction (wrapper or raw schema).
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// Max retries for structured output validation (default: 2).
    #[serde(default = "default_structured_output_retries")]
    pub structured_output_retries: u32,
    /// Optional run-scoped hook overrides.
    #[serde(default)]
    pub hooks_override: Option<HookRunOverrides>,
    /// Enable sub-agent tools.
    #[serde(default)]
    pub enable_subagents: Option<bool>,
    /// Enable semantic memory.
    #[serde(default)]
    pub enable_memory: Option<bool>,
    /// Enable mob tools.
    #[serde(default)]
    pub enable_mob: Option<bool>,
    /// Provider-specific parameters (e.g., thinking config).
    #[serde(default)]
    pub provider_params: Option<serde_json::Value>,
    /// Explicit budget limits for this resumed run.
    #[serde(default)]
    pub budget_limits: Option<BudgetLimitsInput>,
    /// Skills to preload into the system prompt.
    #[serde(default)]
    pub preload_skills: Option<Vec<String>>,
    /// Structured refs for per-turn skill injection.
    #[serde(default)]
    pub skill_refs: Option<Vec<meerkat_core::skills::SkillRef>>,
    /// Legacy compatibility skill refs for per-turn injection.
    #[serde(default)]
    pub skill_references: Option<Vec<String>>,
    /// Optional per-turn tool overlay.
    #[serde(default)]
    pub flow_tool_overlay: Option<TurnToolOverlayInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionIdInput {
    pub session_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionListInput {
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatMcpAddInput {
    pub session_id: String,
    pub server_name: String,
    pub server_config: Value,
    #[serde(default)]
    pub persisted: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatMcpRemoveInput {
    pub session_id: String,
    pub server_name: String,
    #[serde(default)]
    pub persisted: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatMcpReloadInput {
    pub session_id: String,
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default)]
    pub persisted: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSkillsInput {
    pub action: String,
    #[serde(default)]
    pub skill_id: Option<String>,
    /// Optional source selector for inspect action.
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionEventStreamOpenInput {
    pub session_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionEventStreamReadInput {
    pub stream_id: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatSessionEventStreamCloseInput {
    pub stream_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatCommsPeersInput {
    pub session_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatCommsSendInput {
    pub session_id: String,
    pub kind: String,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub stream: Option<String>,
    #[serde(default)]
    pub allow_self_session: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatCommsStreamOpenInput {
    pub session_id: String,
    #[serde(default = "default_comms_stream_scope")]
    pub scope: String,
    #[serde(default)]
    pub interaction_id: Option<String>,
}

fn default_comms_stream_scope() -> String {
    "session".to_string()
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatCommsStreamReadInput {
    pub stream_id: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MeerkatCommsStreamCloseInput {
    pub stream_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BudgetLimitsInput {
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default)]
    pub max_duration_secs: Option<u64>,
    #[serde(default)]
    pub max_tool_calls: Option<usize>,
}

impl From<BudgetLimitsInput> for meerkat_core::BudgetLimits {
    fn from(value: BudgetLimitsInput) -> Self {
        meerkat_core::BudgetLimits {
            max_tokens: value.max_tokens,
            max_duration: value.max_duration_secs.map(std::time::Duration::from_secs),
            max_tool_calls: value.max_tool_calls,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TurnToolOverlayInput {
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    pub blocked_tools: Option<Vec<String>>,
}

impl From<TurnToolOverlayInput> for meerkat_core::service::TurnToolOverlay {
    fn from(value: TurnToolOverlayInput) -> Self {
        Self {
            allowed_tools: value.allowed_tools,
            blocked_tools: value.blocked_tools,
        }
    }
}

/// Tool result provided by the MCP client
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ToolResultInput {
    /// ID of the tool call this is a result for
    pub tool_use_id: String,
    /// Result content (or error message)
    pub content: String,
    /// Whether this is an error result
    #[serde(default)]
    pub is_error: bool,
}

pub type EventNotifier = Arc<dyn Fn(&str, &EventEnvelope<AgentEvent>) + Send + Sync>;
type EventEnvelopeSender = mpsc::Sender<EventEnvelope<AgentEvent>>;
type EventEnvelopeReceiver = mpsc::Receiver<EventEnvelope<AgentEvent>>;

fn spawn_event_forwarder(
    mut rx: EventEnvelopeReceiver,
    session_id: String,
    verbose: bool,
    notifier: Option<EventNotifier>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let Some(ref notify) = notifier {
                notify(&session_id, &event);
            }

            if !verbose {
                continue;
            }

            if let Some(line) = format_verbose_event(&event.payload) {
                tracing::info!("{}", line);
            }
        }
    })
}

fn maybe_event_channel(
    verbose: bool,
    stream: bool,
) -> (Option<EventEnvelopeSender>, Option<EventEnvelopeReceiver>) {
    if !verbose && !stream {
        return (None, None);
    }
    let (tx, rx) = mpsc::channel(100);
    (Some(tx), Some(rx))
}

/// Format an agent run result (success, callback pending, or error) into an MCP response.
///
/// The `session_id` parameter is the pre-claimed ID, used as fallback when the
/// result is a `CallbackPending` error (which doesn't carry a session ID).
fn format_agent_result(
    result: Result<meerkat_core::types::RunResult, SessionError>,
    session_id: &meerkat::SessionId,
) -> Result<Value, String> {
    match result {
        Ok(result) => {
            let payload = json!({
                "content": [{
                    "type": "text",
                    "text": result.text
                }],
                "session_id": result.session_id.to_string(),
                "turns": result.turns,
                "tool_calls": result.tool_calls,
                "structured_output": result.structured_output,
                "schema_warnings": result.schema_warnings,
                "skill_diagnostics": result.skill_diagnostics
            });
            Ok(wrap_tool_payload(payload))
        }
        Err(SessionError::Agent(meerkat::AgentError::CallbackPending { tool_name, args })) => {
            let payload = json!({
                "content": [{
                    "type": "text",
                    "text": "Agent is waiting for tool results"
                }],
                "session_id": session_id.to_string(),
                "status": "pending_tool_call",
                "pending_tool_calls": [{
                    "tool_name": tool_name,
                    "args": args
                }]
            });
            Ok(wrap_tool_payload(payload))
        }
        Err(e) => Err(format!("Agent error: {e}")),
    }
}

/// Returns the list of tools exposed by this MCP server
pub fn tools_list() -> Vec<Value> {
    #[cfg(feature = "comms")]
    let mut tools = vec![
        json!({
            "name": "meerkat_run",
            "description": "Run a new Meerkat agent with the given prompt. Returns the agent's response. If tools are provided and the agent requests a tool call, the response will include pending_tool_calls that must be fulfilled via meerkat_resume.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatRunInput>()
        }),
        json!({
            "name": "meerkat_resume",
            "description": "Resume an existing Meerkat session. Use this to continue a conversation or provide tool results for pending tool calls.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatResumeInput>()
        }),
        json!({
            "name": "meerkat_config",
            "description": "Get or update Meerkat config for this MCP server instance.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatConfigInput>()
        }),
        json!({
            "name": "meerkat_capabilities",
            "description": "Get the list of capabilities available in this Meerkat runtime.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "meerkat_skills",
            "description": "List or inspect available skills. Use action 'list' to see all skills, or 'inspect' with a skill_id to see its full content.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSkillsInput>()
        }),
        json!({
            "name": "meerkat_mob_prefabs",
            "description": "List built-in mob prefab templates.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "meerkat_read",
            "description": "Read current session state.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionIdInput>()
        }),
        json!({
            "name": "meerkat_sessions",
            "description": "List sessions in the active realm.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionListInput>()
        }),
        json!({
            "name": "meerkat_interrupt",
            "description": "Interrupt an in-flight turn for a session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionIdInput>()
        }),
        json!({
            "name": "meerkat_archive",
            "description": "Archive (remove) a session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionIdInput>()
        }),
        json!({
            "name": "meerkat_mcp_add",
            "description": "Stage a live MCP server add operation on an active session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatMcpAddInput>()
        }),
        json!({
            "name": "meerkat_mcp_remove",
            "description": "Stage a live MCP server removal on an active session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatMcpRemoveInput>()
        }),
        json!({
            "name": "meerkat_mcp_reload",
            "description": "Stage a live MCP server reload on an active session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatMcpReloadInput>()
        }),
        json!({
            "name": "meerkat_event_stream_open",
            "description": "Open a session-level agent event stream.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionEventStreamOpenInput>()
        }),
        json!({
            "name": "meerkat_event_stream_read",
            "description": "Read the next item from an open session-level event stream.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionEventStreamReadInput>()
        }),
        json!({
            "name": "meerkat_event_stream_close",
            "description": "Close a previously opened session-level event stream.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionEventStreamCloseInput>()
        }),
    ];

    #[cfg(not(feature = "comms"))]
    let tools = vec![
        json!({
            "name": "meerkat_run",
            "description": "Run a new Meerkat agent with the given prompt. Returns the agent's response. If tools are provided and the agent requests a tool call, the response will include pending_tool_calls that must be fulfilled via meerkat_resume.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatRunInput>()
        }),
        json!({
            "name": "meerkat_resume",
            "description": "Resume an existing Meerkat session. Use this to continue a conversation or provide tool results for pending tool calls.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatResumeInput>()
        }),
        json!({
            "name": "meerkat_config",
            "description": "Get or update Meerkat config for this MCP server instance.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatConfigInput>()
        }),
        json!({
            "name": "meerkat_capabilities",
            "description": "Get the list of capabilities available in this Meerkat runtime.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "meerkat_skills",
            "description": "List or inspect available skills. Use action 'list' to see all skills, or 'inspect' with a skill_id to see its full content.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSkillsInput>()
        }),
        json!({
            "name": "meerkat_mob_prefabs",
            "description": "List built-in mob prefab templates.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "meerkat_read",
            "description": "Read current session state.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionIdInput>()
        }),
        json!({
            "name": "meerkat_sessions",
            "description": "List sessions in the active realm.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionListInput>()
        }),
        json!({
            "name": "meerkat_interrupt",
            "description": "Interrupt an in-flight turn for a session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionIdInput>()
        }),
        json!({
            "name": "meerkat_archive",
            "description": "Archive (remove) a session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionIdInput>()
        }),
        json!({
            "name": "meerkat_mcp_add",
            "description": "Stage a live MCP server add operation on an active session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatMcpAddInput>()
        }),
        json!({
            "name": "meerkat_mcp_remove",
            "description": "Stage a live MCP server removal on an active session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatMcpRemoveInput>()
        }),
        json!({
            "name": "meerkat_mcp_reload",
            "description": "Stage a live MCP server reload on an active session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatMcpReloadInput>()
        }),
        json!({
            "name": "meerkat_event_stream_open",
            "description": "Open a session-level agent event stream.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionEventStreamOpenInput>()
        }),
        json!({
            "name": "meerkat_event_stream_read",
            "description": "Read the next item from an open session-level event stream.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionEventStreamReadInput>()
        }),
        json!({
            "name": "meerkat_event_stream_close",
            "description": "Close a previously opened session-level event stream.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatSessionEventStreamCloseInput>()
        }),
    ];

    tools.extend(meerkat_mob_mcp::tools_list());

    #[cfg(feature = "comms")]
    tools.extend([
        json!({
            "name": "meerkat_comms_send",
            "description": "Send a canonical comms command to a session.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatCommsSendInput>()
        }),
        json!({
            "name": "meerkat_comms_peers",
            "description": "List peers visible to a session's comms runtime.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatCommsPeersInput>()
        }),
        json!({
            "name": "meerkat_comms_stream_open",
            "description": "Open a comms event stream for a session or interaction scope.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatCommsStreamOpenInput>()
        }),
        json!({
            "name": "meerkat_comms_stream_read",
            "description": "Read the next event from an open comms stream (optional timeout).",
            "inputSchema": meerkat_tools::schema_for::<MeerkatCommsStreamReadInput>()
        }),
        json!({
            "name": "meerkat_comms_stream_close",
            "description": "Close a previously opened comms stream.",
            "inputSchema": meerkat_tools::schema_for::<MeerkatCommsStreamCloseInput>()
        }),
    ]);

    tools
}

fn is_mob_tool_name(name: &str) -> bool {
    meerkat_mob_mcp::tools_list().iter().any(|tool| {
        tool.get("name")
            .and_then(Value::as_str)
            .is_some_and(|candidate| candidate == name)
    })
}

/// Handle a tools/call request
pub async fn handle_tools_call(
    state: &MeerkatMcpState,
    tool_name: &str,
    arguments: &Value,
) -> Result<Value, ToolCallError> {
    handle_tools_call_with_notifier(state, tool_name, arguments, None).await
}

/// Handle a tools/call request with optional event notifications.
pub async fn handle_tools_call_with_notifier(
    state: &MeerkatMcpState,
    tool_name: &str,
    arguments: &Value,
    notifier: Option<EventNotifier>,
) -> Result<Value, ToolCallError> {
    match tool_name {
        "meerkat_run" => {
            let input: MeerkatRunInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_run(state, input, notifier)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_resume" => {
            let input: MeerkatResumeInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_resume(state, input, notifier)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_config" => {
            let input: MeerkatConfigInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_config(state, input).await
        }
        "meerkat_capabilities" => handle_meerkat_capabilities(state)
            .await
            .map_err(ToolCallError::internal),
        "meerkat_skills" => {
            let input: MeerkatSkillsInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_skills(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_mob_prefabs" => handle_meerkat_mob_prefabs()
            .await
            .map_err(ToolCallError::internal),
        "meerkat_read" => {
            let input: MeerkatSessionIdInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_read(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_sessions" => {
            let input: MeerkatSessionListInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_sessions(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_interrupt" => {
            let input: MeerkatSessionIdInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_interrupt(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_archive" => {
            let input: MeerkatSessionIdInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_archive(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_mcp_add" => {
            let input: MeerkatMcpAddInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_mcp_add(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_mcp_remove" => {
            let input: MeerkatMcpRemoveInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_mcp_remove(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_mcp_reload" => {
            let input: MeerkatMcpReloadInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_mcp_reload(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_event_stream_open" => {
            let input: MeerkatSessionEventStreamOpenInput =
                serde_json::from_value(arguments.clone()).map_err(|e| {
                    ToolCallError::invalid_params(format!("Invalid arguments: {e}"))
                })?;
            handle_meerkat_event_stream_open(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_event_stream_read" => {
            let input: MeerkatSessionEventStreamReadInput =
                serde_json::from_value(arguments.clone()).map_err(|e| {
                    ToolCallError::invalid_params(format!("Invalid arguments: {e}"))
                })?;
            handle_meerkat_event_stream_read(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        "meerkat_event_stream_close" => {
            let input: MeerkatSessionEventStreamCloseInput =
                serde_json::from_value(arguments.clone()).map_err(|e| {
                    ToolCallError::invalid_params(format!("Invalid arguments: {e}"))
                })?;
            handle_meerkat_event_stream_close(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        #[cfg(feature = "comms")]
        "meerkat_comms_send" => {
            let input: MeerkatCommsSendInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_comms_send(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        #[cfg(feature = "comms")]
        "meerkat_comms_peers" => {
            let input: MeerkatCommsPeersInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_comms_peers(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        #[cfg(feature = "comms")]
        "meerkat_comms_stream_open" => {
            let input: MeerkatCommsStreamOpenInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_comms_stream_open(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        #[cfg(feature = "comms")]
        "meerkat_comms_stream_read" => {
            let input: MeerkatCommsStreamReadInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_comms_stream_read(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        #[cfg(feature = "comms")]
        "meerkat_comms_stream_close" => {
            let input: MeerkatCommsStreamCloseInput = serde_json::from_value(arguments.clone())
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid arguments: {e}")))?;
            handle_meerkat_comms_stream_close(state, input)
                .await
                .map_err(ToolCallError::internal)
        }
        _ if is_mob_tool_name(tool_name) => {
            match meerkat_mob_mcp::handle_tools_call(&state.mob_state, tool_name, arguments).await
            {
                Ok(value) => Ok(wrap_tool_payload(value)),
                Err(err) if err.code == -32601 => Err(ToolCallError::method_not_found(format!(
                    "Unknown tool: {tool_name}"
                ))),
                Err(err) => Err(ToolCallError::new(err.code, err.message, err.data)),
            }
        }
        _ => Err(ToolCallError::method_not_found(format!(
            "Unknown tool: {tool_name}"
        ))),
    }
}

async fn handle_meerkat_skills(
    state: &MeerkatMcpState,
    input: MeerkatSkillsInput,
) -> Result<Value, String> {
    let runtime = state
        .skill_runtime
        .as_ref()
        .ok_or_else(|| "skills not enabled".to_string())?;

    match input.action.as_str() {
        "list" => {
            let entries = runtime
                .list_all_with_provenance(&meerkat_core::skills::SkillFilter::default())
                .await
                .map_err(|e| format!("skill list failed: {e}"))?;
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
            serde_json::to_value(meerkat_contracts::SkillListResponse { skills: wire })
                .map_err(|e| format!("serialization failed: {e}"))
        }
        "inspect" => {
            let skill_id = input
                .skill_id
                .as_deref()
                .ok_or_else(|| "missing 'skill_id' for inspect action".to_string())?;
            let doc = runtime
                .load_from_source(
                    &meerkat_core::skills::SkillId::from(skill_id),
                    input.source.as_deref(),
                )
                .await
                .map_err(|e| format!("skill inspect failed: {e}"))?;
            serde_json::to_value(meerkat_contracts::SkillInspectResponse {
                id: doc.descriptor.id.0.clone(),
                name: doc.descriptor.name.clone(),
                description: doc.descriptor.description.clone(),
                scope: doc.descriptor.scope.to_string(),
                source: doc.descriptor.source_name.clone(),
                body: doc.body,
            })
            .map_err(|e| format!("serialization failed: {e}"))
        }
        _ => Err(format!("unknown action: {}", input.action)),
    }
}

async fn handle_meerkat_capabilities(state: &MeerkatMcpState) -> Result<Value, String> {
    let config = state
        .config_runtime
        .get()
        .await
        .map(|snapshot| snapshot.config)
        .unwrap_or_default();
    let response = meerkat::surface::build_capabilities_response(&config);
    serde_json::to_value(&response).map_err(|e| format!("Serialization failed: {e}"))
}

async fn handle_meerkat_mob_prefabs() -> Result<Value, String> {
    let prefabs: Vec<Value> = meerkat_mob::Prefab::all()
        .into_iter()
        .map(|prefab| {
            json!({
                "key": prefab.key(),
                "toml_template": prefab.toml_template(),
            })
        })
        .collect();
    Ok(json!({ "prefabs": prefabs }))
}

async fn handle_meerkat_config(
    state: &MeerkatMcpState,
    input: MeerkatConfigInput,
) -> Result<Value, ToolCallError> {
    match input.action {
        ConfigAction::Get => {
            let snapshot = state
                .config_runtime
                .get()
                .await
                .map_err(map_config_runtime_error)?;
            config_envelope_value(snapshot, state.expose_paths())
        }
        ConfigAction::Set => {
            let config = input.config.ok_or_else(|| {
                ToolCallError::invalid_params("config is required for action=set")
            })?;
            let config: Config = serde_json::from_value(config)
                .map_err(|e| ToolCallError::invalid_params(format!("Invalid config: {e}")))?;
            validate_config_for_commit(&config)?;
            let snapshot = state
                .config_runtime
                .set(config, input.expected_generation)
                .await
                .map_err(map_config_runtime_error)?;
            config_envelope_value(snapshot, state.expose_paths())
        }
        ConfigAction::Patch => {
            let patch = input.patch.ok_or_else(|| {
                ToolCallError::invalid_params("patch is required for action=patch")
            })?;
            let current = state
                .config_runtime
                .get()
                .await
                .map_err(map_config_runtime_error)?;
            let preview = apply_patch_preview(&current.config, patch.clone())?;
            validate_config_for_commit(&preview)?;
            let snapshot = state
                .config_runtime
                .patch(ConfigDelta(patch), input.expected_generation)
                .await
                .map_err(map_config_runtime_error)?;
            config_envelope_value(snapshot, state.expose_paths())
        }
    }
}

fn config_envelope_value(
    snapshot: meerkat_core::ConfigSnapshot,
    expose_paths: bool,
) -> Result<Value, ToolCallError> {
    let policy = if expose_paths {
        ConfigEnvelopePolicy::Diagnostic
    } else {
        ConfigEnvelopePolicy::Public
    };
    serde_json::to_value(ConfigEnvelope::from_snapshot(snapshot, policy))
        .map_err(|e| ToolCallError::internal(e.to_string()))
}

fn validate_config_for_commit(config: &Config) -> Result<(), ToolCallError> {
    config
        .validate()
        .map_err(|e| ToolCallError::invalid_params(format!("Invalid config: {e}")))?;
    config
        .skills
        .build_source_identity_registry()
        .map_err(|e| {
            ToolCallError::invalid_params(format!("Invalid skills source-identity config: {e}"))
        })?;
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

fn apply_patch_preview(config: &Config, patch: Value) -> Result<Config, ToolCallError> {
    let mut value = serde_json::to_value(config)
        .map_err(|e| ToolCallError::internal(format!("Failed to serialize config: {e}")))?;
    merge_patch(&mut value, patch);
    serde_json::from_value(value).map_err(|e| ToolCallError::invalid_params(format!("{e}")))
}

fn canonical_skill_keys(
    config: &Config,
    skill_refs: Option<Vec<meerkat_core::skills::SkillRef>>,
    skill_references: Option<Vec<String>>,
) -> Result<Option<Vec<meerkat_core::skills::SkillKey>>, String> {
    let registry = config
        .skills
        .build_source_identity_registry()
        .map_err(|e| format!("Invalid skills config: {e}"))?;
    let params = SkillsParams {
        preload_skills: None,
        skill_refs,
        skill_references,
    };
    params
        .canonical_skill_keys_with_registry(&registry)
        .map_err(|e| format!("Invalid skill refs: {e}"))
}

fn preload_skill_ids(
    preload_skills: Option<Vec<String>>,
) -> Option<Vec<meerkat_core::skills::SkillId>> {
    preload_skills.map(|ids| ids.into_iter().map(meerkat_core::skills::SkillId).collect())
}

async fn handle_meerkat_read(
    state: &MeerkatMcpState,
    input: MeerkatSessionIdInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let view = state
        .service
        .read(&session_id)
        .await
        .map_err(|e| format!("Failed to read session: {e}"))?;
    let payload = json!({
        "session_id": session_id.to_string(),
        "session_ref": meerkat_contracts::format_session_ref(&state.realm_id, &session_id),
        "state": view.state,
        "billing": view.billing
    });
    Ok(wrap_tool_payload(payload))
}

async fn handle_meerkat_interrupt(
    state: &MeerkatMcpState,
    input: MeerkatSessionIdInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    state
        .service
        .interrupt(&session_id)
        .await
        .map_err(|e| format!("Failed to interrupt session: {e}"))?;
    Ok(wrap_tool_payload(json!({
        "session_id": session_id.to_string(),
        "interrupted": true
    })))
}

async fn handle_meerkat_sessions(
    state: &MeerkatMcpState,
    input: MeerkatSessionListInput,
) -> Result<Value, String> {
    let query = meerkat_core::service::SessionQuery {
        limit: input.limit,
        offset: input.offset,
    };
    let sessions = state
        .service
        .list(query)
        .await
        .map_err(|e| format!("Failed to list sessions: {e}"))?;
    let payload = json!({
        "sessions": sessions.into_iter().map(|s| {
            json!({
                "session_id": s.session_id.to_string(),
                "session_ref": meerkat_contracts::format_session_ref(&state.realm_id, &s.session_id),
                "created_at": s.created_at,
                "updated_at": s.updated_at,
                "message_count": s.message_count,
                "total_tokens": s.total_tokens,
                "is_active": s.is_active
            })
        }).collect::<Vec<_>>()
    });
    Ok(wrap_tool_payload(payload))
}

async fn handle_meerkat_archive(
    state: &MeerkatMcpState,
    input: MeerkatSessionIdInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    state
        .service
        .archive(&session_id)
        .await
        .map_err(|e| format!("Failed to archive session: {e}"))?;
    Ok(wrap_tool_payload(json!({
        "session_id": session_id.to_string(),
        "archived": true
    })))
}

async fn handle_meerkat_mcp_add(
    state: &MeerkatMcpState,
    input: MeerkatMcpAddInput,
) -> Result<Value, String> {
    if input.server_name.trim().is_empty() {
        return Err("server_name cannot be empty".to_string());
    }
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let adapter = state.mcp_adapter_for_session(&session_id).await?;

    let mut server_config = input.server_config;
    if let Some(obj) = server_config.as_object_mut() {
        obj.insert(
            "name".to_string(),
            serde_json::Value::String(input.server_name.clone()),
        );
    }
    let config: meerkat_core::McpServerConfig =
        serde_json::from_value(server_config).map_err(|e| format!("invalid server_config: {e}"))?;
    adapter
        .stage_add(config)
        .await
        .map_err(|e| format!("failed to stage add: {e}"))?;

    Ok(wrap_tool_payload(json!({
        "session_id": input.session_id,
        "operation": "add",
        "server_name": input.server_name,
        "status": "staged",
        "persisted": false
    })))
}

async fn handle_meerkat_mcp_remove(
    state: &MeerkatMcpState,
    input: MeerkatMcpRemoveInput,
) -> Result<Value, String> {
    if input.server_name.trim().is_empty() {
        return Err("server_name cannot be empty".to_string());
    }
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let adapter = state.mcp_adapter_for_session(&session_id).await?;
    adapter
        .stage_remove(input.server_name.clone())
        .await
        .map_err(|e| format!("failed to stage remove: {e}"))?;

    Ok(wrap_tool_payload(json!({
        "session_id": input.session_id,
        "operation": "remove",
        "server_name": input.server_name,
        "status": "staged",
        "persisted": false
    })))
}

async fn handle_meerkat_mcp_reload(
    state: &MeerkatMcpState,
    input: MeerkatMcpReloadInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let adapter = state.mcp_adapter_for_session(&session_id).await?;

    match input.server_name.clone() {
        Some(name) => {
            if name.trim().is_empty() {
                return Err("server_name cannot be empty".to_string());
            }
            adapter
                .stage_reload(McpReloadTarget::ServerName(name))
                .await
                .map_err(|e| format!("failed to stage reload: {e}"))?;
        }
        None => {
            for name in adapter.active_server_names().await {
                adapter
                    .stage_reload(McpReloadTarget::ServerName(name))
                    .await
                    .map_err(|e| format!("failed to stage reload: {e}"))?;
            }
        }
    }

    Ok(wrap_tool_payload(json!({
        "session_id": input.session_id,
        "operation": "reload",
        "server_name": input.server_name,
        "status": "staged",
        "persisted": false
    })))
}

async fn handle_meerkat_event_stream_open(
    state: &MeerkatMcpState,
    input: MeerkatSessionEventStreamOpenInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let stream = state
        .service
        .subscribe_session_events(&session_id)
        .await
        .map_err(|e| format!("Failed to open session event stream: {e}"))?;
    let stream_id = meerkat::SessionId::new().to_string();
    state.session_event_streams.lock().await.insert(
        stream_id.clone(),
        Arc::new(SessionEventStreamHandle {
            stream: Mutex::new(stream),
        }),
    );

    Ok(wrap_tool_payload(json!({
        "stream_id": stream_id,
        "session_id": session_id.to_string()
    })))
}

async fn handle_meerkat_event_stream_read(
    state: &MeerkatMcpState,
    input: MeerkatSessionEventStreamReadInput,
) -> Result<Value, String> {
    let handle = state
        .session_event_streams
        .lock()
        .await
        .get(&input.stream_id)
        .cloned()
        .ok_or_else(|| format!("Stream not found: {}", input.stream_id))?;

    let timeout_ms = input.timeout_ms.unwrap_or(0);
    let next_event = {
        let mut stream = handle.stream.lock().await;
        if timeout_ms == 0 {
            stream.next().await
        } else {
            match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), stream.next())
                .await
            {
                Ok(item) => item,
                Err(_) => {
                    return Ok(wrap_tool_payload(json!({
                        "stream_id": input.stream_id,
                        "status": "timeout"
                    })));
                }
            }
        }
    };

    match next_event {
        Some(envelope) => {
            let envelope_json = serde_json::to_value(&envelope)
                .map_err(|e| format!("Failed to serialize stream event: {e}"))?;
            Ok(wrap_tool_payload(json!({
                "stream_id": input.stream_id,
                "status": "event",
                "event": envelope_json
            })))
        }
        None => {
            state
                .session_event_streams
                .lock()
                .await
                .remove(&input.stream_id);
            Ok(wrap_tool_payload(json!({
                "stream_id": input.stream_id,
                "status": "closed"
            })))
        }
    }
}

async fn handle_meerkat_event_stream_close(
    state: &MeerkatMcpState,
    input: MeerkatSessionEventStreamCloseInput,
) -> Result<Value, String> {
    let removed = state
        .session_event_streams
        .lock()
        .await
        .remove(&input.stream_id);
    Ok(wrap_tool_payload(json!({
        "stream_id": input.stream_id,
        "closed": removed.is_some()
    })))
}

#[cfg(feature = "comms")]
fn build_comms_receipt_json(receipt: meerkat_core::comms::SendReceipt) -> Value {
    match receipt {
        meerkat_core::comms::SendReceipt::InputAccepted {
            interaction_id,
            stream_reserved,
        } => json!({
            "kind": "input_accepted",
            "interaction_id": interaction_id.0.to_string(),
            "stream_reserved": stream_reserved,
        }),
        meerkat_core::comms::SendReceipt::PeerMessageSent { envelope_id, acked } => json!({
            "kind": "peer_message_sent",
            "envelope_id": envelope_id.to_string(),
            "acked": acked,
        }),
        meerkat_core::comms::SendReceipt::PeerRequestSent {
            envelope_id,
            interaction_id,
            stream_reserved,
        } => json!({
            "kind": "peer_request_sent",
            "envelope_id": envelope_id.to_string(),
            "interaction_id": interaction_id.0.to_string(),
            "stream_reserved": stream_reserved,
        }),
        meerkat_core::comms::SendReceipt::PeerResponseSent {
            envelope_id,
            in_reply_to,
        } => json!({
            "kind": "peer_response_sent",
            "envelope_id": envelope_id.to_string(),
            "in_reply_to": in_reply_to.0.to_string(),
        }),
    }
}

#[cfg(feature = "comms")]
async fn handle_meerkat_comms_send(
    state: &MeerkatMcpState,
    input: MeerkatCommsSendInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let comms = state
        .service
        .comms_runtime(&session_id)
        .await
        .ok_or_else(|| format!("Session not found or comms not enabled: {session_id}"))?;
    let request = meerkat_core::comms::CommsCommandRequest {
        kind: input.kind,
        to: input.to,
        body: input.body,
        intent: input.intent,
        params: input.params,
        in_reply_to: input.in_reply_to,
        status: input.status,
        result: input.result,
        source: input.source,
        stream: input.stream,
        allow_self_session: input.allow_self_session,
    };
    let cmd = request.parse(&session_id).map_err(|errors| {
        let details = meerkat_core::comms::CommsCommandRequest::validation_errors_to_json(&errors);
        format!("Command validation failed: {details:?}")
    })?;
    let receipt = comms
        .send(cmd)
        .await
        .map_err(|e| format!("Failed to send comms command: {e}"))?;
    Ok(wrap_tool_payload(
        json!({ "receipt": build_comms_receipt_json(receipt) }),
    ))
}

#[cfg(feature = "comms")]
async fn handle_meerkat_comms_peers(
    state: &MeerkatMcpState,
    input: MeerkatCommsPeersInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let comms = state
        .service
        .comms_runtime(&session_id)
        .await
        .ok_or_else(|| format!("Session not found or comms not enabled: {session_id}"))?;
    let peers = comms.peers().await;
    let payload = json!({
        "peers": peers.iter().map(|p| {
            json!({
                "name": p.name.to_string(),
                "peer_id": p.peer_id,
                "address": p.address,
                "source": format!("{:?}", p.source),
                "sendable_kinds": p.sendable_kinds,
                "capabilities": p.capabilities,
                "meta": p.meta,
            })
        }).collect::<Vec<_>>()
    });
    Ok(wrap_tool_payload(payload))
}

#[cfg(feature = "comms")]
async fn handle_meerkat_comms_stream_open(
    state: &MeerkatMcpState,
    input: MeerkatCommsStreamOpenInput,
) -> Result<Value, String> {
    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let comms = state
        .service
        .comms_runtime(&session_id)
        .await
        .ok_or_else(|| format!("Session not found or comms not enabled: {session_id}"))?;

    let scope_name = input.scope.clone();
    let interaction_id_for_payload = input.interaction_id.clone();
    let scope = match scope_name.as_str() {
        "session" => meerkat_core::comms::StreamScope::Session(session_id.clone()),
        "interaction" => {
            let interaction_id = input
                .interaction_id
                .ok_or_else(|| "interaction_id is required when scope='interaction'".to_string())?;
            let parsed = uuid::Uuid::parse_str(&interaction_id)
                .map_err(|e| format!("invalid interaction_id '{interaction_id}': {e}"))?;
            meerkat_core::comms::StreamScope::Interaction(meerkat_core::InteractionId(parsed))
        }
        other => {
            return Err(format!(
                "Invalid scope '{other}'. Use 'session' or 'interaction'."
            ));
        }
    };

    let stream = comms
        .stream(scope)
        .map_err(|e| format!("Failed to open comms stream: {e}"))?;
    let stream_id = meerkat::SessionId::new().to_string();
    state.comms_streams.lock().await.insert(
        stream_id.clone(),
        Arc::new(CommsStreamHandle {
            stream: Mutex::new(stream),
        }),
    );

    Ok(wrap_tool_payload(json!({
        "stream_id": stream_id,
        "session_id": session_id.to_string(),
        "scope": scope_name,
        "interaction_id": interaction_id_for_payload
    })))
}

#[cfg(feature = "comms")]
async fn handle_meerkat_comms_stream_read(
    state: &MeerkatMcpState,
    input: MeerkatCommsStreamReadInput,
) -> Result<Value, String> {
    let handle = state
        .comms_streams
        .lock()
        .await
        .get(&input.stream_id)
        .cloned()
        .ok_or_else(|| format!("Stream not found: {}", input.stream_id))?;

    let timeout_ms = input.timeout_ms.unwrap_or(0);
    let next_event = {
        let mut stream = handle.stream.lock().await;
        if timeout_ms == 0 {
            stream.next().await
        } else {
            match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), stream.next())
                .await
            {
                Ok(item) => item,
                Err(_) => {
                    return Ok(wrap_tool_payload(json!({
                        "stream_id": input.stream_id,
                        "status": "timeout"
                    })));
                }
            }
        }
    };

    match next_event {
        Some(envelope) => {
            let envelope_json = serde_json::to_value(&envelope)
                .map_err(|e| format!("Failed to serialize stream event: {e}"))?;
            Ok(wrap_tool_payload(json!({
                "stream_id": input.stream_id,
                "status": "event",
                "event": envelope_json
            })))
        }
        None => {
            state.comms_streams.lock().await.remove(&input.stream_id);
            Ok(wrap_tool_payload(json!({
                "stream_id": input.stream_id,
                "status": "closed"
            })))
        }
    }
}

#[cfg(feature = "comms")]
async fn handle_meerkat_comms_stream_close(
    state: &MeerkatMcpState,
    input: MeerkatCommsStreamCloseInput,
) -> Result<Value, String> {
    let removed = state.comms_streams.lock().await.remove(&input.stream_id);
    Ok(wrap_tool_payload(json!({
        "stream_id": input.stream_id,
        "closed": removed.is_some()
    })))
}

async fn handle_meerkat_run(
    state: &MeerkatMcpState,
    input: MeerkatRunInput,
    notifier: Option<EventNotifier>,
) -> Result<Value, String> {
    let host_mode = resolve_host_mode(input.host_mode)?;
    if host_mode
        && input
            .comms_name
            .as_ref()
            .is_none_or(|name| name.trim().is_empty())
    {
        return Err("host_mode requires comms_name".to_string());
    }
    let config = state
        .config_runtime
        .get()
        .await
        .map(|snapshot| snapshot.config)
        .unwrap_or_default();
    let model = input.model.unwrap_or_else(|| config.agent.model.clone());

    // Build composed external tools:
    // - callback tools supplied by the MCP client
    // - per-session live MCP router adapter (for add/remove/reload parity)
    let callback_tools: Option<Arc<dyn AgentToolDispatcher>> = if input.tools.is_empty() {
        None
    } else {
        Some(Arc::new(MpcToolDispatcher::new(&input.tools)))
    };
    let mcp_adapter = Arc::new(meerkat_mcp::McpRouterAdapter::new(McpRouter::new()));
    let mcp_tools: Arc<dyn AgentToolDispatcher> = mcp_adapter.clone();
    let external_tools = compose_external_tool_dispatchers(callback_tools, Some(mcp_tools))
        .map_err(|e| format!("failed to compose external tools: {e}"))?;

    let enable_shell = input
        .builtin_config
        .as_ref()
        .is_some_and(|c| c.enable_shell);
    let preload_skills = preload_skill_ids(input.preload_skills.clone());
    let skill_references = canonical_skill_keys(
        &config,
        input.skill_refs.clone(),
        input.skill_references.clone(),
    )?;

    // Parse output schema if provided
    let output_schema = match input.output_schema.clone() {
        Some(schema) => Some(
            OutputSchema::from_json_value(schema)
                .map_err(|e| format!("Invalid output_schema: {e}"))?,
        ),
        None => None,
    };

    // Pre-create a session to claim a stable session_id
    let session = Session::new();
    let session_id = session.id().clone();

    // Set up event forwarding
    let (event_tx, event_rx) = maybe_event_channel(input.verbose, input.stream);
    let event_task = event_rx.map(|rx| {
        let stream_notifier = if input.stream { notifier.clone() } else { None };
        spawn_event_forwarder(rx, session_id.to_string(), input.verbose, stream_notifier)
    });

    let current_generation = state.config_runtime.get().await.ok().map(|s| s.generation);
    let build = SessionBuildOptions {
        provider: input.provider.map(ProviderInput::to_provider),
        output_schema,
        structured_output_retries: input.structured_output_retries,
        hooks_override: input.hooks_override.clone().unwrap_or_default(),
        comms_name: input.comms_name.clone(),
        peer_meta: input.peer_meta.clone(),
        resume_session: Some(session),
        budget_limits: input.budget_limits.clone().map(Into::into),
        provider_params: input.provider_params.clone(),
        external_tools,
        llm_client_override: None,
        scoped_event_tx: None,
        scoped_event_path: None,
        override_builtins: Some(input.enable_builtins),
        override_shell: Some(input.enable_builtins && enable_shell),
        override_subagents: input.enable_subagents,
        override_memory: input.enable_memory,
        override_mob: input.enable_mob,
        preload_skills,
        realm_id: Some(state.realm_id.clone()),
        instance_id: state.instance_id.clone(),
        backend: Some(state.backend.clone()),
        config_generation: current_generation,
        checkpointer: None,
        silent_comms_intents: Vec::new(),
        max_inline_peer_notifications: None,
    };

    let req = CreateSessionRequest {
        model,
        prompt: input.prompt,
        system_prompt: input.system_prompt,
        max_tokens: input.max_tokens,
        event_tx: event_tx.clone(),
        host_mode,
        skill_references,
        initial_turn: InitialTurnPolicy::RunImmediately,
        build: Some(build),
    };

    let result = state.service.create_session(req).await;
    drop(event_tx);
    if let Some(task) = event_task
        && let Err(e) = task.await
    {
        tracing::warn!("event task panicked: {e}");
    }

    if state.service.read(&session_id).await.is_ok() {
        state.upsert_mcp_adapter(&session_id, mcp_adapter).await;
    }

    format_agent_result(result, &session_id)
}

async fn handle_meerkat_resume(
    state: &MeerkatMcpState,
    input: MeerkatResumeInput,
    notifier: Option<EventNotifier>,
) -> Result<Value, String> {
    let config = state
        .config_runtime
        .get()
        .await
        .map(|snapshot| snapshot.config)
        .unwrap_or_default();

    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let mut session = state
        .service
        .load_persisted(&session_id)
        .await
        .map_err(|e| format!("Failed to load session: {e}"))?
        .ok_or_else(|| format!("Session not found: {}", input.session_id))?;

    // Inject tool results into the session before resuming
    if !input.tool_results.is_empty() {
        let results: Vec<ToolResult> = input
            .tool_results
            .iter()
            .map(|r| ToolResult::new(r.tool_use_id.clone(), r.content.clone(), r.is_error))
            .collect();
        session.push(Message::ToolResults { results });
    }

    // Resolve settings from stored metadata, falling back to input overrides
    let stored_metadata = session.session_metadata();
    let enable_builtins = input.enable_builtins
        || stored_metadata
            .as_ref()
            .is_some_and(|meta| meta.tooling.builtins);
    let enable_shell = input
        .builtin_config
        .as_ref()
        .map(|c| c.enable_shell)
        .unwrap_or_else(|| {
            stored_metadata
                .as_ref()
                .is_some_and(|meta| meta.tooling.shell)
        });
    let enable_mob = input
        .enable_mob
        .or_else(|| stored_metadata.as_ref().map(|meta| meta.tooling.mob));
    let enable_subagents = input
        .enable_subagents
        .or_else(|| stored_metadata.as_ref().map(|meta| meta.tooling.subagents));
    let enable_memory = input.enable_memory;
    let host_mode_requested =
        input.host_mode || stored_metadata.as_ref().is_some_and(|meta| meta.host_mode);
    let host_mode = resolve_host_mode(host_mode_requested)?;
    let comms_name = input.comms_name.clone().or_else(|| {
        stored_metadata
            .as_ref()
            .and_then(|meta| meta.comms_name.clone())
    });
    if host_mode
        && comms_name
            .as_ref()
            .is_none_or(|name| name.trim().is_empty())
    {
        return Err("host_mode requires comms_name".to_string());
    }
    let model = input
        .model
        .or_else(|| stored_metadata.as_ref().map(|meta| meta.model.clone()))
        .unwrap_or_else(|| config.agent.model.clone());
    let max_tokens = input
        .max_tokens
        .or_else(|| stored_metadata.as_ref().map(|meta| meta.max_tokens));
    let provider = input
        .provider
        .map(ProviderInput::to_provider)
        .or_else(|| stored_metadata.as_ref().map(|meta| meta.provider));

    // Build composed external tools:
    // - callback tools supplied by the MCP client
    // - per-session live MCP router adapter
    let callback_tools: Option<Arc<dyn AgentToolDispatcher>> = if input.tools.is_empty() {
        None
    } else {
        Some(Arc::new(MpcToolDispatcher::new(&input.tools)))
    };
    let existing_adapter = state
        .mcp_adapters
        .lock()
        .await
        .get(&session_id.to_string())
        .cloned();
    let mcp_adapter = existing_adapter
        .clone()
        .unwrap_or_else(|| Arc::new(meerkat_mcp::McpRouterAdapter::new(McpRouter::new())));
    let mcp_tools: Arc<dyn AgentToolDispatcher> = mcp_adapter.clone();
    let external_tools = compose_external_tool_dispatchers(callback_tools, Some(mcp_tools))
        .map_err(|e| format!("failed to compose external tools: {e}"))?;

    // Use empty prompt when only providing tool results
    let prompt = if input.prompt.is_empty() && !input.tool_results.is_empty() {
        String::new()
    } else {
        input.prompt
    };
    let preload_skills = preload_skill_ids(input.preload_skills.clone());
    let skill_references = canonical_skill_keys(
        &config,
        input.skill_refs.clone(),
        input.skill_references.clone(),
    )?;

    // Set up event forwarding
    let (event_tx, event_rx) = maybe_event_channel(input.verbose, input.stream);
    let event_task = event_rx.map(|rx| {
        let stream_notifier = if input.stream { notifier.clone() } else { None };
        spawn_event_forwarder(rx, session_id.to_string(), input.verbose, stream_notifier)
    });

    let current_generation = state.config_runtime.get().await.ok().map(|s| s.generation);
    let output_schema = match input.output_schema.clone() {
        Some(schema) => Some(
            OutputSchema::from_json_value(schema)
                .map_err(|e| format!("Invalid output_schema: {e}"))?,
        ),
        None => None,
    };
    let build = SessionBuildOptions {
        provider,
        output_schema,
        structured_output_retries: input.structured_output_retries,
        hooks_override: input.hooks_override.clone().unwrap_or_default(),
        comms_name,
        resume_session: Some(session),
        budget_limits: input.budget_limits.clone().map(Into::into),
        provider_params: input.provider_params.clone(),
        external_tools,
        llm_client_override: None,
        scoped_event_tx: None,
        scoped_event_path: None,
        override_builtins: Some(enable_builtins),
        override_shell: Some(enable_builtins && enable_shell),
        override_subagents: enable_subagents,
        override_memory: enable_memory,
        override_mob: enable_mob,
        preload_skills,
        peer_meta: input
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

    let needs_rebuild = existing_adapter.is_none()
        || input.system_prompt.is_some()
        || input.output_schema.is_some()
        || input.structured_output_retries != default_structured_output_retries();

    let result = if needs_rebuild {
        let req = CreateSessionRequest {
            model,
            prompt,
            system_prompt: input.system_prompt.clone(),
            max_tokens,
            event_tx: event_tx.clone(),
            host_mode,
            skill_references,
            initial_turn: InitialTurnPolicy::RunImmediately,
            build: Some(build),
        };
        state.service.create_session(req).await
    } else {
        // Try start_turn on the live session first (it may still be alive
        // from a prior meerkat_run in the same MCP server process).
        let turn_req = StartTurnRequest {
            prompt: prompt.clone(),
            event_tx: event_tx.clone(),
            host_mode,
            skill_references: skill_references.clone(),
            flow_tool_overlay: input.flow_tool_overlay.clone().map(Into::into),
        };
        match state.service.start_turn(&session_id, turn_req).await {
            Ok(run_result) => Ok(run_result),
            Err(SessionError::NotFound { .. }) => {
                let req = CreateSessionRequest {
                    model,
                    prompt,
                    system_prompt: input.system_prompt.clone(),
                    max_tokens,
                    event_tx: event_tx.clone(),
                    host_mode,
                    skill_references,
                    initial_turn: InitialTurnPolicy::RunImmediately,
                    build: Some(build),
                };

                state.service.create_session(req).await
            }
            Err(other) => Err(other),
        }
    };

    drop(event_tx);
    if let Some(task) = event_task
        && let Err(e) = task.await
    {
        tracing::warn!("event task panicked: {e}");
    }

    if state.service.read(&session_id).await.is_ok() {
        state.upsert_mcp_adapter(&session_id, mcp_adapter).await;
    }

    format_agent_result(result, &session_id)
}

fn wrap_tool_payload(payload: Value) -> Value {
    let text = serde_json::to_string(&payload).unwrap_or_default();
    json!({
        "content": [{
            "type": "text",
            "text": text
        }]
    })
}

fn compose_external_tool_dispatchers(
    primary: Option<Arc<dyn AgentToolDispatcher>>,
    secondary: Option<Arc<dyn AgentToolDispatcher>>,
) -> Result<Option<Arc<dyn AgentToolDispatcher>>, String> {
    match (primary, secondary) {
        (None, None) => Ok(None),
        (Some(dispatcher), None) | (None, Some(dispatcher)) => Ok(Some(dispatcher)),
        (Some(a), Some(b)) => {
            let primary_names: HashSet<String> = a.tools().iter().map(|t| t.name.clone()).collect();
            let secondary_tools = b.tools();
            let secondary_unique: Vec<String> = secondary_tools
                .iter()
                .map(|t| t.name.clone())
                .filter(|name| !primary_names.contains(name))
                .collect();

            if secondary_unique.is_empty() {
                return Ok(Some(a));
            }

            let secondary: Arc<dyn AgentToolDispatcher> =
                if secondary_unique.len() == secondary_tools.len() {
                    b
                } else {
                    Arc::new(meerkat_core::FilteredToolDispatcher::new(
                        b,
                        secondary_unique,
                    ))
                };

            let gateway = meerkat_core::ToolGatewayBuilder::new()
                .add_dispatcher(a)
                .add_dispatcher(secondary)
                .build()
                .map_err(|e| format!("failed to compose external tools: {e}"))?;
            Ok(Some(Arc::new(gateway)))
        }
    }
}

// Adapter types needed for the MCP server

use async_trait::async_trait;
use meerkat::{AgentToolDispatcher, Message, ToolDef};

/// MCP tool dispatcher - exposes tools to the LLM and handles callback tools
/// by returning a special error that signals the MCP client needs to handle the tool call
pub struct MpcToolDispatcher {
    tools: Arc<[Arc<ToolDef>]>,
    callback_tools: HashSet<String>,
}

impl MpcToolDispatcher {
    /// Create a new tool dispatcher from MCP tool definitions
    pub fn new(mcp_tools: &[McpToolDef]) -> Self {
        let tools: Arc<[Arc<ToolDef>]> = mcp_tools
            .iter()
            .map(|t| {
                Arc::new(ToolDef {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    input_schema: t.input_schema.clone(),
                })
            })
            .collect::<Vec<_>>()
            .into();

        let callback_tools = mcp_tools
            .iter()
            .filter(|t| t.handler_kind() == "callback")
            .map(|t| t.name.clone())
            .collect();

        Self {
            tools,
            callback_tools,
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for MpcToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        let args: Value = serde_json::from_str(call.args.get())
            .unwrap_or_else(|_| Value::String(call.args.get().to_string()));
        // Check if this is a callback tool
        if self.callback_tools.contains(call.name) {
            // Return a special error that signals the agent loop should pause
            Err(ToolError::callback_pending(call.name, args))
        } else {
            Err(ToolError::not_found(call.name))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn hooks_override_fixture() -> HookRunOverrides {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../test-fixtures/hooks/run_override.json");
        let payload = std::fs::read_to_string(path).expect("hook override fixture must exist");
        serde_json::from_str::<HookRunOverrides>(&payload)
            .expect("hook override fixture must deserialize")
    }

    #[test]
    fn test_config_envelope_value_redacts_paths_by_default() {
        let snapshot = meerkat_core::ConfigSnapshot {
            config: Config::default(),
            generation: 1,
            metadata: Some(meerkat_core::ConfigStoreMetadata {
                realm_id: Some("team".to_string()),
                instance_id: Some("mcp-1".to_string()),
                backend: Some("redb".to_string()),
                resolved_paths: Some(meerkat_core::ConfigResolvedPaths {
                    root: "/tmp/root".to_string(),
                    manifest_path: "/tmp/root/realm_manifest.json".to_string(),
                    config_path: "/tmp/root/config.toml".to_string(),
                    sessions_redb_path: "/tmp/root/sessions.redb".to_string(),
                    sessions_jsonl_dir: "/tmp/root/sessions_jsonl".to_string(),
                }),
            }),
        };

        let value = config_envelope_value(snapshot, false).expect("envelope value");
        assert!(value.get("resolved_paths").is_none());
    }

    #[test]
    fn test_config_envelope_value_includes_paths_when_enabled() {
        let snapshot = meerkat_core::ConfigSnapshot {
            config: Config::default(),
            generation: 1,
            metadata: Some(meerkat_core::ConfigStoreMetadata {
                realm_id: Some("team".to_string()),
                instance_id: Some("mcp-1".to_string()),
                backend: Some("redb".to_string()),
                resolved_paths: Some(meerkat_core::ConfigResolvedPaths {
                    root: "/tmp/root".to_string(),
                    manifest_path: "/tmp/root/realm_manifest.json".to_string(),
                    config_path: "/tmp/root/config.toml".to_string(),
                    sessions_redb_path: "/tmp/root/sessions.redb".to_string(),
                    sessions_jsonl_dir: "/tmp/root/sessions_jsonl".to_string(),
                }),
            }),
        };

        let value = config_envelope_value(snapshot, true).expect("envelope value");
        assert!(value.get("resolved_paths").is_some());
    }

    #[test]
    fn test_config_runtime_error_generation_conflict_is_typed() {
        let err = map_config_runtime_error(ConfigRuntimeError::GenerationConflict {
            expected: 2,
            current: 5,
        });
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("generation conflict"));
        let data = err.data.expect("generation conflict should include data");
        assert_eq!(data["type"], "generation_conflict");
        assert_eq!(data["expected_generation"], 2);
        assert_eq!(data["current_generation"], 5);
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
        assert_eq!(err.code, -32602);
        assert!(
            err.message
                .contains("Invalid skills source-identity config")
        );
    }

    #[test]
    fn test_tools_list_schema() {
        let tools = tools_list();
        let mob_tools = meerkat_mob_mcp::tools_list();
        #[cfg(feature = "comms")]
        assert_eq!(tools.len(), 21 + mob_tools.len());
        #[cfg(not(feature = "comms"))]
        assert_eq!(tools.len(), 16 + mob_tools.len());

        let tool_names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();

        let run_tool = &tools[0];
        assert_eq!(run_tool["name"], "meerkat_run");
        assert!(run_tool["inputSchema"]["properties"]["prompt"].is_object());
        assert!(run_tool["inputSchema"]["properties"]["verbose"].is_object());
        assert!(
            run_tool["inputSchema"]["properties"]
                .get("output_schema")
                .is_some()
        );
        assert!(
            run_tool["inputSchema"]["properties"]
                .get("structured_output_retries")
                .is_some()
        );

        let resume_tool = &tools[1];
        assert_eq!(resume_tool["name"], "meerkat_resume");
        assert!(resume_tool["inputSchema"]["properties"]["session_id"].is_object());
        assert!(resume_tool["inputSchema"]["properties"]["verbose"].is_object());

        let config_tool = &tools[2];
        assert_eq!(config_tool["name"], "meerkat_config");
        assert!(config_tool["inputSchema"]["properties"]["action"].is_object());

        let capabilities_tool = &tools[3];
        assert_eq!(capabilities_tool["name"], "meerkat_capabilities");

        let mob_prefabs_tool = &tools[5];
        assert_eq!(mob_prefabs_tool["name"], "meerkat_mob_prefabs");
        assert_eq!(mob_prefabs_tool["inputSchema"]["type"], "object");
        assert!(
            mob_prefabs_tool["inputSchema"]["properties"]
                .as_object()
                .is_some_and(|props| props.is_empty())
        );

        let read_tool = &tools[6];
        assert_eq!(read_tool["name"], "meerkat_read");
        let sessions_tool = &tools[7];
        assert_eq!(sessions_tool["name"], "meerkat_sessions");
        let interrupt_tool = &tools[8];
        assert_eq!(interrupt_tool["name"], "meerkat_interrupt");
        let archive_tool = &tools[9];
        assert_eq!(archive_tool["name"], "meerkat_archive");
        let mcp_add_tool = &tools[10];
        assert_eq!(mcp_add_tool["name"], "meerkat_mcp_add");
        let mcp_remove_tool = &tools[11];
        assert_eq!(mcp_remove_tool["name"], "meerkat_mcp_remove");
        let mcp_reload_tool = &tools[12];
        assert_eq!(mcp_reload_tool["name"], "meerkat_mcp_reload");
        let event_stream_open_tool = &tools[13];
        assert_eq!(event_stream_open_tool["name"], "meerkat_event_stream_open");
        let event_stream_read_tool = &tools[14];
        assert_eq!(event_stream_read_tool["name"], "meerkat_event_stream_read");
        let event_stream_close_tool = &tools[15];
        assert_eq!(
            event_stream_close_tool["name"],
            "meerkat_event_stream_close"
        );
        assert!(tool_names.contains(&"meerkat_mob_prefabs"));
        assert!(tool_names.contains(&"mob_create"));
        assert!(tool_names.contains(&"mob_list"));
        assert!(tool_names.contains(&"mob_lifecycle"));

        #[cfg(feature = "comms")]
        {
            assert!(tool_names.contains(&"meerkat_comms_send"));
            assert!(tool_names.contains(&"meerkat_comms_peers"));
            assert!(tool_names.contains(&"meerkat_comms_stream_open"));
            assert!(tool_names.contains(&"meerkat_comms_stream_read"));
            assert!(tool_names.contains(&"meerkat_comms_stream_close"));
        }
    }

    #[cfg(not(feature = "comms"))]
    #[test]
    fn test_tools_list_omits_comms_tools_when_feature_disabled() {
        let tools = tools_list();
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert!(!names.contains(&"meerkat_comms_send"));
        assert!(!names.contains(&"meerkat_comms_peers"));
        assert!(!names.contains(&"meerkat_comms_stream_open"));
        assert!(!names.contains(&"meerkat_comms_stream_read"));
        assert!(!names.contains(&"meerkat_comms_stream_close"));
        assert!(names.contains(&"meerkat_event_stream_open"));
        assert!(names.contains(&"meerkat_event_stream_read"));
        assert!(names.contains(&"meerkat_event_stream_close"));
    }

    #[test]
    fn test_meerkat_run_input_parsing() {
        let input_json = json!({
            "prompt": "Hello",
            "model": "claude-sonnet-4"
        });

        let input: MeerkatRunInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.prompt, "Hello");
        assert_eq!(input.model, Some("claude-sonnet-4".to_string()));
        assert_eq!(input.max_tokens, None);
        assert_eq!(input.structured_output_retries, 2);
        assert!(input.output_schema.is_none());
        assert!(!input.verbose);
    }

    #[test]
    fn test_meerkat_resume_input_parsing() {
        let input_json = json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "Continue"
        });

        let input: MeerkatResumeInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.session_id, "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(input.prompt, "Continue");
        assert!(input.system_prompt.is_none());
        assert!(input.output_schema.is_none());
        assert_eq!(input.structured_output_retries, 2);
        assert!(!input.verbose);
    }

    #[test]
    fn test_meerkat_run_input_with_tools() {
        let input_json = json!({
            "prompt": "Hello",
            "tools": [
                {
                    "name": "get_weather",
                    "description": "Get weather for a city",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "city": {"type": "string"}
                        },
                        "required": ["city"]
                    }
                }
            ]
        });

        let input: MeerkatRunInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.prompt, "Hello");
        assert_eq!(input.tools.len(), 1);
        assert_eq!(input.tools[0].name, "get_weather");
        assert_eq!(input.tools[0].handler_kind(), "callback"); // default
    }

    #[test]
    fn test_meerkat_resume_input_with_tool_results() {
        let input_json = json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "",
            "tool_results": [
                {
                    "tool_use_id": "tc_123",
                    "content": "Sunny, 72°F"
                }
            ]
        });

        let input: MeerkatResumeInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.session_id, "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(input.tool_results.len(), 1);
        assert_eq!(input.tool_results[0].tool_use_id, "tc_123");
        assert_eq!(input.tool_results[0].content, "Sunny, 72°F");
        assert!(!input.tool_results[0].is_error);
    }

    #[test]
    fn test_meerkat_run_input_accepts_hooks_override_fixture() {
        let input_json = json!({
            "prompt": "Hello",
            "hooks_override": hooks_override_fixture(),
        });

        let input: MeerkatRunInput = serde_json::from_value(input_json).unwrap();
        assert!(input.hooks_override.is_some());
        let overrides = input
            .hooks_override
            .expect("hooks override should be present");
        assert_eq!(overrides.entries.len(), 2);
        assert_eq!(
            overrides.entries[0].point,
            meerkat_core::HookPoint::PreToolExecution
        );
    }

    #[test]
    fn test_meerkat_resume_input_accepts_hooks_override_fixture() {
        let input_json = json!({
            "session_id": "01234567-89ab-cdef-0123-456789abcdef",
            "prompt": "Resume",
            "hooks_override": hooks_override_fixture(),
        });

        let input: MeerkatResumeInput = serde_json::from_value(input_json).unwrap();
        assert!(input.hooks_override.is_some());
        let overrides = input
            .hooks_override
            .expect("hooks override should be present");
        assert_eq!(overrides.entries.len(), 2);
        assert_eq!(
            overrides.entries[1].mode,
            meerkat_core::HookExecutionMode::Background
        );
    }

    #[test]
    fn test_format_agent_result_includes_skill_diagnostics() {
        let session_id = meerkat::SessionId::new();
        let result = meerkat_core::types::RunResult {
            text: "ok".to_string(),
            session_id: session_id.clone(),
            usage: Default::default(),
            turns: 1,
            tool_calls: 0,
            structured_output: None,
            schema_warnings: None,
            skill_diagnostics: Some(meerkat_core::skills::SkillRuntimeDiagnostics {
                source_health: meerkat_core::skills::SourceHealthSnapshot {
                    state: meerkat_core::skills::SourceHealthState::Unhealthy,
                    invalid_ratio: 0.9,
                    invalid_count: 9,
                    total_count: 10,
                    failure_streak: 10,
                    handshake_failed: true,
                },
                quarantined: vec![],
            }),
        };

        let payload = format_agent_result(Ok(result), &session_id).expect("formatted payload");
        let raw = payload["content"][0]["text"]
            .as_str()
            .expect("wrapped MCP payload text");
        let decoded: serde_json::Value = serde_json::from_str(raw).expect("valid wrapped JSON");
        assert_eq!(decoded["content"][0]["text"], "ok");
        assert_eq!(
            decoded["skill_diagnostics"]["source_health"]["state"],
            "unhealthy"
        );
    }

    #[test]
    fn test_mpc_tool_dispatcher_creates_tool_defs() {
        let mcp_tools = vec![
            McpToolDef {
                name: "get_weather".to_string(),
                description: "Get weather".to_string(),
                input_schema: meerkat_tools::empty_object_schema(),
                handler: Some("callback".to_string()),
            },
            McpToolDef {
                name: "search".to_string(),
                description: "Search".to_string(),
                input_schema: meerkat_tools::empty_object_schema(),
                handler: Some("callback".to_string()),
            },
        ];

        let dispatcher = MpcToolDispatcher::new(&mcp_tools);
        let tool_defs = dispatcher.tools();

        assert_eq!(tool_defs.len(), 2);
        assert_eq!(tool_defs[0].name, "get_weather");
        assert_eq!(tool_defs[1].name, "search");
    }

    #[tokio::test]
    async fn test_mpc_tool_dispatcher_returns_callback_error() {
        let mcp_tools = vec![McpToolDef {
            name: "get_weather".to_string(),
            description: "Get weather".to_string(),
            input_schema: meerkat_tools::empty_object_schema(),
            handler: Some("callback".to_string()),
        }];

        let dispatcher = MpcToolDispatcher::new(&mcp_tools);
        let args_raw =
            serde_json::value::RawValue::from_string(json!({"city": "Tokyo"}).to_string()).unwrap();
        let call = ToolCallView {
            id: "test-1",
            name: "get_weather",
            args: &args_raw,
        };
        let result = dispatcher.dispatch(call).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.is_callback_pending(), "Expected CallbackPending error");
        if let Some((tool_name, args)) = err.as_callback_pending() {
            assert_eq!(tool_name, "get_weather");
            assert_eq!(args["city"], "Tokyo");
        }
    }

    #[test]
    fn test_tools_list_has_tools_parameter() {
        let tools = tools_list();
        let run_tool = &tools[0];

        // Verify tools parameter exists in the schema
        assert!(run_tool["inputSchema"]["properties"]["tools"].is_object());
        assert_eq!(
            run_tool["inputSchema"]["properties"]["tools"]["type"],
            "array"
        );
    }

    #[test]
    fn test_tools_list_has_tool_results_parameter() {
        let tools = tools_list();
        let resume_tool = &tools[1];

        // Verify tool_results parameter exists in the schema
        let tool_results = &resume_tool["inputSchema"]["properties"]["tool_results"];
        assert!(tool_results.is_object(), "tool_results should be an object");
        assert_eq!(
            tool_results["type"], "array",
            "tool_results should be an array"
        );
        // Items may use $ref for the ToolResultInput schema
        assert!(
            tool_results["items"].is_object(),
            "tool_results items should be defined"
        );
    }

    #[test]
    fn test_tools_list_skills_schema_has_source_selector() {
        let tools = tools_list();
        let skills_tool = &tools[4];
        assert_eq!(skills_tool["name"], "meerkat_skills");
        assert!(
            skills_tool["inputSchema"]["properties"]["source"].is_object(),
            "skills inspect should expose optional source selector"
        );
    }

    #[cfg(not(feature = "comms"))]
    #[test]
    fn test_resolve_host_mode_rejects_when_comms_disabled() {
        let err = resolve_host_mode(true).expect_err("host mode should be rejected");
        assert!(err.contains("host_mode requires comms support"));
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_resolve_host_mode_allows_when_comms_enabled() {
        assert!(resolve_host_mode(true).expect("host mode should be enabled"));
        assert!(!resolve_host_mode(false).expect("host mode should be disabled"));
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_meerkat_run_input_with_host_mode() {
        let input_json = json!({
            "prompt": "Hello",
            "host_mode": true,
            "comms_name": "test-agent"
        });

        let input: MeerkatRunInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.prompt, "Hello");
        assert!(input.host_mode);
        assert_eq!(input.comms_name, Some("test-agent".to_string()));
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_meerkat_run_input_host_mode_defaults_to_false() {
        let input_json = json!({
            "prompt": "Hello"
        });

        let input: MeerkatRunInput = serde_json::from_value(input_json).unwrap();
        assert!(!input.host_mode);
        assert!(input.comms_name.is_none());
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn test_handle_meerkat_run_host_mode_requires_comms_name() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let result = handle_meerkat_run(
            &state,
            MeerkatRunInput {
                prompt: "test".to_string(),
                system_prompt: None,
                model: Some("claude-opus-4-6".to_string()),
                max_tokens: Some(4096),
                provider: None,
                output_schema: None,
                structured_output_retries: 2,
                stream: false,
                verbose: false,
                tools: vec![],
                enable_builtins: false,
                builtin_config: None,
                host_mode: true,
                comms_name: None, // Missing!
                peer_meta: None,
                hooks_override: None,
                enable_subagents: None,
                enable_memory: None,
                enable_mob: None,
                provider_params: None,
                budget_limits: None,
                preload_skills: None,
                skill_refs: None,
                skill_references: None,
            },
            None,
        )
        .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("comms_name"));
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_tools_list_has_host_mode_parameter() {
        let tools = tools_list();
        let run_tool = &tools[0];

        // Verify host_mode parameter exists in the schema
        assert!(run_tool["inputSchema"]["properties"]["host_mode"].is_object());
        assert_eq!(
            run_tool["inputSchema"]["properties"]["host_mode"]["type"],
            "boolean"
        );

        // Verify comms_name parameter exists in the schema
        assert!(run_tool["inputSchema"]["properties"]["comms_name"].is_object());
        let comms_name_type = &run_tool["inputSchema"]["properties"]["comms_name"]["type"];
        assert!(
            comms_name_type == "string"
                || comms_name_type
                    .as_array()
                    .is_some_and(|types| types.contains(&json!("string"))),
            "unexpected comms_name type: {comms_name_type}"
        );
    }

    #[tokio::test]
    async fn test_handle_tools_call_mob_prefabs_returns_prefabs() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let response = handle_tools_call(&state, "meerkat_mob_prefabs", &json!({}))
            .await
            .expect("mob prefabs tool call should succeed");
        let payload: Value = if response.get("prefabs").is_some() {
            response
        } else {
            let wrapped = response["content"][0]["text"]
                .as_str()
                .expect("wrapped text payload");
            serde_json::from_str(wrapped).expect("wrapped payload JSON")
        };
        let prefabs = payload["prefabs"].as_array().expect("prefabs array");
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

    #[tokio::test]
    async fn test_handle_tools_call_unknown_tool_still_errors() {
        let store: Arc<dyn SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let state = MeerkatMcpState::new_with_store(store).await;
        let err = handle_tools_call(&state, "meerkat_mob_prefabz_typo", &json!({}))
            .await
            .expect_err("unknown tool must error");
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("Unknown tool"));
    }
}
