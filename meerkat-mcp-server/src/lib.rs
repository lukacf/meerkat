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
use meerkat_core::error::invalid_session_id_message;
use meerkat_core::service::{
    CreateSessionRequest, InitialTurnPolicy, SessionBuildOptions, SessionError, SessionService,
    StartTurnRequest,
};
use meerkat_core::{
    AgentEvent, Config, ConfigDelta, ConfigEnvelope, ConfigEnvelopePolicy, ConfigRuntimeError,
    ConfigStore, FileConfigStore, HookRunOverrides, Provider, RealmSelection, RuntimeBootstrap,
    Session, ToolCallView, format_verbose_event,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

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
    meerkat::surface::resolve_host_mode(requested)
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
    _realm_lease: Option<meerkat_store::RealmLeaseGuard>,
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

        let builder = FactoryAgentBuilder::new_with_config_store(factory, config, config_store);
        let service = PersistentSessionService::new(builder, 100, session_store);

        Ok(Self {
            service,
            realm_id,
            backend: manifest.backend.as_str().to_string(),
            instance_id: bootstrap.realm.instance_id,
            expose_paths,
            config_runtime,
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
    /// Optional run-scoped hook overrides.
    #[serde(default)]
    pub hooks_override: Option<HookRunOverrides>,
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

pub type EventNotifier = Arc<dyn Fn(&str, &AgentEvent) + Send + Sync>;

fn spawn_event_forwarder(
    mut rx: mpsc::Receiver<AgentEvent>,
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

            if let Some(line) = format_verbose_event(&event) {
                tracing::info!("{}", line);
            }
        }
    })
}

fn maybe_event_channel(
    verbose: bool,
    stream: bool,
) -> (
    Option<mpsc::Sender<AgentEvent>>,
    Option<mpsc::Receiver<AgentEvent>>,
) {
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
        Err(e) => Err(format!("Agent error: {}", e)),
    }
}

/// Returns the list of tools exposed by this MCP server
pub fn tools_list() -> Vec<Value> {
    vec![
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
    ]
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
        _ => Err(ToolCallError::method_not_found(format!(
            "Unknown tool: {tool_name}"
        ))),
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
    let model = input
        .model
        .unwrap_or_else(|| config.agent.model.to_string());

    // Build external tool dispatcher from MCP callback tools (if any)
    let external_tools: Option<Arc<dyn AgentToolDispatcher>> = if input.tools.is_empty() {
        None
    } else {
        Some(Arc::new(MpcToolDispatcher::new(&input.tools)))
    };

    let enable_shell = input
        .builtin_config
        .as_ref()
        .is_some_and(|c| c.enable_shell);

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
        provider: input.provider.map(|p| p.to_provider()),
        output_schema,
        structured_output_retries: input.structured_output_retries,
        hooks_override: input.hooks_override.clone().unwrap_or_default(),
        comms_name: input.comms_name.clone(),
        peer_meta: input.peer_meta.clone(),
        resume_session: Some(session),
        budget_limits: None,
        provider_params: None,
        external_tools,
        llm_client_override: None,
        override_builtins: Some(input.enable_builtins),
        override_shell: Some(input.enable_builtins && enable_shell),
        override_subagents: None,
        override_memory: None,
        preload_skills: None,
        realm_id: Some(state.realm_id.clone()),
        instance_id: state.instance_id.clone(),
        backend: Some(state.backend.clone()),
        config_generation: current_generation,
    };

    let req = CreateSessionRequest {
        model,
        prompt: input.prompt,
        system_prompt: input.system_prompt,
        max_tokens: input.max_tokens,
        event_tx: event_tx.clone(),
        host_mode,
        skill_references: None,
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
        .unwrap_or_else(|| config.agent.model.to_string());
    let max_tokens = input
        .max_tokens
        .or_else(|| stored_metadata.as_ref().map(|meta| meta.max_tokens));
    let provider = input
        .provider
        .map(ProviderInput::to_provider)
        .or_else(|| stored_metadata.as_ref().map(|meta| meta.provider));

    // Build external tool dispatcher from MCP callback tools (if any)
    let external_tools: Option<Arc<dyn AgentToolDispatcher>> = if input.tools.is_empty() {
        None
    } else {
        Some(Arc::new(MpcToolDispatcher::new(&input.tools)))
    };

    // Use empty prompt when only providing tool results
    let prompt = if input.prompt.is_empty() && !input.tool_results.is_empty() {
        String::new()
    } else {
        input.prompt
    };

    // Set up event forwarding
    let (event_tx, event_rx) = maybe_event_channel(input.verbose, input.stream);
    let event_task = event_rx.map(|rx| {
        let stream_notifier = if input.stream { notifier.clone() } else { None };
        spawn_event_forwarder(rx, session_id.to_string(), input.verbose, stream_notifier)
    });

    let current_generation = state.config_runtime.get().await.ok().map(|s| s.generation);
    let build = SessionBuildOptions {
        provider,
        output_schema: None,
        structured_output_retries: 2,
        hooks_override: input.hooks_override.clone().unwrap_or_default(),
        comms_name,
        resume_session: Some(session),
        budget_limits: None,
        provider_params: None,
        external_tools,
        llm_client_override: None,
        override_builtins: Some(enable_builtins),
        override_shell: Some(enable_builtins && enable_shell),
        override_subagents: None,
        override_memory: None,
        preload_skills: None,
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
    };

    // Try start_turn on the live session first (it may still be alive
    // from a prior meerkat_run in the same MCP server process).
    let turn_req = StartTurnRequest {
        prompt: prompt.clone(),
        event_tx: event_tx.clone(),
        host_mode,
        skill_references: None,
    };

    let result = match state.service.start_turn(&session_id, turn_req).await {
        Ok(run_result) => Ok(run_result),
        Err(SessionError::NotFound { .. }) => {
            let req = CreateSessionRequest {
                model,
                prompt,
                system_prompt: None,
                max_tokens,
                event_tx: event_tx.clone(),
                host_mode,
                skill_references: None,
                initial_turn: InitialTurnPolicy::RunImmediately,
                build: Some(build),
            };

            state.service.create_session(req).await
        }
        Err(other) => Err(other),
    };

    drop(event_tx);
    if let Some(task) = event_task
        && let Err(e) = task.await
    {
        tracing::warn!("event task panicked: {e}");
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

// Adapter types needed for the MCP server

use async_trait::async_trait;
use meerkat::{AgentToolDispatcher, Message, ToolDef};

/// MCP tool dispatcher - exposes tools to the LLM and handles callback tools
/// by returning a special error that signals the MCP client needs to handle the tool call
pub struct MpcToolDispatcher {
    tools: Arc<[Arc<ToolDef>]>,
    callback_tools: std::collections::HashSet<String>,
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
    fn test_tools_list_schema() {
        let tools = tools_list();
        assert_eq!(tools.len(), 4);

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
}
