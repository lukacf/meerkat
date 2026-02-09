//! meerkat-mcp-server - MCP server exposing Meerkat as a tool
//!
//! This crate provides an MCP server that exposes Meerkat agent capabilities
//! as MCP tools: meerkat_run and meerkat_resume.

use meerkat::{
    AgentBuildConfig, AgentFactory, EphemeralSessionService, FactoryAgentBuilder, JsonlStore,
    OutputSchema, SessionStore, ToolError, ToolResult,
};
use meerkat_core::error::invalid_session_id_message;
use meerkat_core::service::{CreateSessionRequest, SessionError, SessionService};
use meerkat_core::{
    AgentEvent, Config, ConfigDelta, ConfigStore, FileConfigStore, HookRunOverrides, Provider,
    Session, ToolCallView, format_verbose_event,
};
use meerkat_tools::find_project_root;
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
}

fn resolve_project_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    find_project_root(&cwd).unwrap_or(cwd)
}

async fn load_config_async() -> Config {
    let project_root = resolve_project_root();
    let store = FileConfigStore::project(&project_root);
    let mut config = store.get().await.unwrap_or_else(|_| Config::default());
    let _ = config.apply_env_overrides();
    config
}

fn resolve_host_mode(requested: bool) -> Result<bool, String> {
    if requested && !cfg!(feature = "comms") {
        return Err("host_mode requires comms support (build with --features comms)".to_string());
    }
    Ok(requested && cfg!(feature = "comms"))
}

fn resolve_store_path(config: &Config) -> PathBuf {
    config
        .store
        .sessions_path
        .clone()
        .or_else(|| config.storage.directory.clone())
        .unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("meerkat")
                .join("sessions")
        })
}

/// Shared state for the MCP server, holding the session service.
///
/// The service is configured once with max-permissive factory flags
/// (`builtins: true`, `shell: true`). Per-request tool configuration is
/// controlled via `override_builtins` / `override_shell` in `AgentBuildConfig`.
pub struct MeerkatMcpState {
    service: EphemeralSessionService<FactoryAgentBuilder>,
    build_config_slot: Arc<tokio::sync::Mutex<Option<AgentBuildConfig>>>,
}

impl MeerkatMcpState {
    /// Create a new MCP state with a session service backed by `AgentFactory`.
    pub async fn new() -> Self {
        let config = load_config_async().await;
        let store_path = resolve_store_path(&config);
        let project_root = resolve_project_root();

        // Create factory with max-permissive flags; per-request overrides
        // in AgentBuildConfig control what tools are actually enabled.
        let factory = AgentFactory::new(store_path)
            .project_root(project_root)
            .builtins(true)
            .shell(true);

        let builder = FactoryAgentBuilder::new(factory, config);
        let build_config_slot = builder.build_config_slot.clone();
        let service = EphemeralSessionService::new(builder, 100);

        Self {
            service,
            build_config_slot,
        }
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
                "schema_warnings": result.schema_warnings
            });
            Ok(wrap_tool_payload(payload))
        }
        Err(SessionError::Agent(meerkat::AgentError::CallbackPending {
            tool_name,
            args,
        })) => {
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
    ]
}

/// Handle a tools/call request
pub async fn handle_tools_call(
    state: &MeerkatMcpState,
    tool_name: &str,
    arguments: &Value,
) -> Result<Value, String> {
    handle_tools_call_with_notifier(state, tool_name, arguments, None).await
}

/// Handle a tools/call request with optional event notifications.
pub async fn handle_tools_call_with_notifier(
    state: &MeerkatMcpState,
    tool_name: &str,
    arguments: &Value,
    notifier: Option<EventNotifier>,
) -> Result<Value, String> {
    match tool_name {
        "meerkat_run" => {
            let input: MeerkatRunInput = serde_json::from_value(arguments.clone())
                .map_err(|e| format!("Invalid arguments: {}", e))?;
            handle_meerkat_run(state, input, notifier).await
        }
        "meerkat_resume" => {
            let input: MeerkatResumeInput = serde_json::from_value(arguments.clone())
                .map_err(|e| format!("Invalid arguments: {}", e))?;
            handle_meerkat_resume(state, input, notifier).await
        }
        "meerkat_config" => {
            let input: MeerkatConfigInput = serde_json::from_value(arguments.clone())
                .map_err(|e| format!("Invalid arguments: {}", e))?;
            handle_meerkat_config(input).await
        }
        _ => Err(format!("Unknown tool: {}", tool_name)),
    }
}

async fn handle_meerkat_config(input: MeerkatConfigInput) -> Result<Value, String> {
    match input.action {
        ConfigAction::Get => {
            let store = FileConfigStore::project(resolve_project_root());
            let config = store.get().await.map_err(|e| e.to_string())?;
            Ok(json!({ "config": config }))
        }
        ConfigAction::Set => {
            let config = input
                .config
                .ok_or_else(|| "config is required for action=set".to_string())?;
            let config: Config =
                serde_json::from_value(config).map_err(|e| format!("Invalid config: {e}"))?;
            let store = FileConfigStore::project(resolve_project_root());
            let config_clone = config.clone();
            store.set(config_clone).await.map_err(|e| e.to_string())?;
            Ok(json!({ "config": config }))
        }
        ConfigAction::Patch => {
            let patch = input
                .patch
                .ok_or_else(|| "patch is required for action=patch".to_string())?;
            let store = FileConfigStore::project(resolve_project_root());
            let updated = store
                .patch(ConfigDelta(patch))
                .await
                .map_err(|e| e.to_string())?;
            Ok(json!({ "config": updated }))
        }
    }
}

async fn handle_meerkat_run(
    state: &MeerkatMcpState,
    input: MeerkatRunInput,
    notifier: Option<EventNotifier>,
) -> Result<Value, String> {
    let host_mode = resolve_host_mode(input.host_mode)?;
    let config = load_config_async().await;
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
        spawn_event_forwarder(
            rx,
            session_id.to_string(),
            input.verbose,
            stream_notifier,
        )
    });

    // Build config with per-request tool overrides
    let build_config = AgentBuildConfig {
        model: model.clone(),
        provider: input.provider.map(|p| p.to_provider()),
        max_tokens: input.max_tokens,
        system_prompt: input.system_prompt.clone(),
        output_schema,
        structured_output_retries: input.structured_output_retries,
        hooks_override: input.hooks_override.clone().unwrap_or_default(),
        host_mode,
        comms_name: input.comms_name.clone(),
        resume_session: Some(session),
        budget_limits: None,
        event_tx: None, // wired by the service
        llm_client_override: None,
        provider_params: None,
        external_tools,
        override_builtins: Some(input.enable_builtins),
        override_shell: Some(input.enable_builtins && enable_shell),
    };

    // Stage the build config for the builder to pick up
    {
        let mut slot = state.build_config_slot.lock().await;
        *slot = Some(build_config);
    }

    // Route through the session service
    let req = CreateSessionRequest {
        model,
        prompt: input.prompt,
        system_prompt: None, // already in the staged build config
        max_tokens: None,    // already in the staged build config
        event_tx: event_tx.clone(),
        host_mode,
    };

    let result = state.service.create_session(req).await;
    drop(event_tx);
    if let Some(task) = event_task {
        let _ = task.await;
    }

    format_agent_result(result, &session_id)
}

async fn handle_meerkat_resume(
    state: &MeerkatMcpState,
    input: MeerkatResumeInput,
    notifier: Option<EventNotifier>,
) -> Result<Value, String> {
    let config = load_config_async().await;
    let store_path = resolve_store_path(&config);

    // Load existing session from persistent store
    let session_store = JsonlStore::new(store_path);
    session_store
        .init()
        .await
        .map_err(|e| format!("Store init failed: {}", e))?;

    let session_id =
        meerkat::SessionId::parse(&input.session_id).map_err(invalid_session_id_message)?;
    let mut session = session_store
        .load(&session_id)
        .await
        .map_err(|e| format!("Failed to load session: {}", e))?
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
        spawn_event_forwarder(
            rx,
            session_id.to_string(),
            input.verbose,
            stream_notifier,
        )
    });

    // Build config with resume_session and per-request tool overrides
    let build_config = AgentBuildConfig {
        model: model.clone(),
        provider,
        max_tokens,
        system_prompt: None,
        output_schema: None,
        structured_output_retries: 2,
        hooks_override: input.hooks_override.clone().unwrap_or_default(),
        host_mode,
        comms_name,
        resume_session: Some(session),
        budget_limits: None,
        event_tx: None, // wired by the service
        llm_client_override: None,
        provider_params: None,
        external_tools,
        override_builtins: Some(enable_builtins),
        override_shell: Some(enable_builtins && enable_shell),
    };

    // Stage the build config for the builder to pick up
    {
        let mut slot = state.build_config_slot.lock().await;
        *slot = Some(build_config);
    }

    // Route through the session service. Because we set resume_session, the
    // builder will reuse the existing session ID and messages.
    let req = CreateSessionRequest {
        model,
        prompt,
        system_prompt: None,
        max_tokens: None,
        event_tx: event_tx.clone(),
        host_mode,
    };

    let result = state.service.create_session(req).await;
    drop(event_tx);
    if let Some(task) = event_task {
        let _ = task.await;
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
    fn test_tools_list_schema() {
        let tools = tools_list();
        assert_eq!(tools.len(), 3);

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
        let state = MeerkatMcpState::new().await;
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
