//! meerkat-mcp-server - MCP server exposing Meerkat as a tool
//!
//! This crate provides an MCP server that exposes Meerkat agent capabilities
//! as MCP tools: meerkat_run and meerkat_resume.

use meerkat::{
    AgentBuilder, AgentError, AnthropicClient, JsonlStore, LlmClient, Session, SessionStore,
    ToolError,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// Tool definition provided by the MCP client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// JSON Schema for tool input
    pub input_schema: Value,
    /// Handler type: "callback" means the tool result will be provided via meerkat_resume
    #[serde(default = "default_handler_type")]
    pub handler: String,
}

fn default_handler_type() -> String {
    "callback".to_string()
}

/// Input schema for meerkat_run tool
#[derive(Debug, Deserialize)]
pub struct MeerkatRunInput {
    pub prompt: String,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Tool definitions for the agent to use
    #[serde(default)]
    pub tools: Vec<McpToolDef>,
    /// Enable built-in tools (task management, shell, etc.)
    #[serde(default)]
    pub enable_builtins: bool,
    /// Configuration for built-in tools (only used when enable_builtins is true)
    #[serde(default)]
    pub builtin_config: Option<BuiltinConfigInput>,
}

/// Configuration options for built-in tools
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BuiltinConfigInput {
    /// Enable shell tools (default: false)
    #[serde(default)]
    pub enable_shell: bool,
    /// Default timeout for shell commands in seconds (default: 30)
    #[serde(default)]
    pub shell_timeout_secs: Option<u64>,
}

fn default_model() -> String {
    "claude-opus-4-5".to_string()
}

fn default_max_tokens() -> u32 {
    4096
}

/// Input schema for meerkat_resume tool
#[derive(Debug, Deserialize)]
pub struct MeerkatResumeInput {
    pub session_id: String,
    pub prompt: String,
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
}

/// Tool result provided by the MCP client
#[derive(Debug, Clone, Deserialize)]
pub struct ToolResultInput {
    /// ID of the tool call this is a result for
    pub tool_use_id: String,
    /// Result content (or error message)
    pub content: String,
    /// Whether this is an error result
    #[serde(default)]
    pub is_error: bool,
}

/// Returns the list of tools exposed by this MCP server
pub fn tools_list() -> Vec<Value> {
    let tool_def_schema = json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Tool name"
            },
            "description": {
                "type": "string",
                "description": "Tool description"
            },
            "input_schema": {
                "type": "object",
                "description": "JSON Schema for tool input"
            },
            "handler": {
                "type": "string",
                "description": "Handler type: 'callback' means results provided via meerkat_resume (default)"
            }
        },
        "required": ["name", "description", "input_schema"]
    });

    let tool_result_schema = json!({
        "type": "object",
        "properties": {
            "tool_use_id": {
                "type": "string",
                "description": "ID of the tool call this is a result for"
            },
            "content": {
                "type": "string",
                "description": "Result content or error message"
            },
            "is_error": {
                "type": "boolean",
                "description": "Whether this is an error result"
            }
        },
        "required": ["tool_use_id", "content"]
    });

    vec![
        json!({
            "name": "meerkat_run",
            "description": "Run a new Meerkat agent with the given prompt. Returns the agent's response. If tools are provided and the agent requests a tool call, the response will include pending_tool_calls that must be fulfilled via meerkat_resume.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The prompt to send to the agent"
                    },
                    "system_prompt": {
                        "type": "string",
                        "description": "Optional system prompt to configure agent behavior"
                    },
                    "model": {
                        "type": "string",
                        "description": "LLM model to use (default: claude-opus-4-5)"
                    },
                    "max_tokens": {
                        "type": "integer",
                        "description": "Maximum tokens per turn (default: 4096)"
                    },
                    "tools": {
                        "type": "array",
                        "description": "Tool definitions for the agent to use. Tools with handler='callback' will pause execution and return pending_tool_calls.",
                        "items": tool_def_schema.clone()
                    }
                },
                "required": ["prompt"]
            }
        }),
        json!({
            "name": "meerkat_resume",
            "description": "Resume an existing Meerkat session. Use this to continue a conversation or provide tool results for pending tool calls.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "The session ID to resume"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The prompt to continue the conversation (can be empty if only providing tool results)"
                    },
                    "tools": {
                        "type": "array",
                        "description": "Tool definitions (should match the original run)",
                        "items": tool_def_schema
                    },
                    "tool_results": {
                        "type": "array",
                        "description": "Results for pending tool calls from a previous response",
                        "items": tool_result_schema
                    }
                },
                "required": ["session_id", "prompt"]
            }
        }),
    ]
}

/// Handle a tools/call request
pub async fn handle_tools_call(tool_name: &str, arguments: &Value) -> Result<Value, String> {
    match tool_name {
        "meerkat_run" => {
            let input: MeerkatRunInput = serde_json::from_value(arguments.clone())
                .map_err(|e| format!("Invalid arguments: {}", e))?;
            handle_meerkat_run(input).await
        }
        "meerkat_resume" => {
            let input: MeerkatResumeInput = serde_json::from_value(arguments.clone())
                .map_err(|e| format!("Invalid arguments: {}", e))?;
            handle_meerkat_resume(input).await
        }
        _ => Err(format!("Unknown tool: {}", tool_name)),
    }
}

async fn handle_meerkat_run(input: MeerkatRunInput) -> Result<Value, String> {
    // Branch based on whether builtins are enabled
    if input.enable_builtins {
        handle_meerkat_run_with_builtins(input).await
    } else {
        handle_meerkat_run_simple(input).await
    }
}

async fn handle_meerkat_run_simple(input: MeerkatRunInput) -> Result<Value, String> {
    // Get API key from environment
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "ANTHROPIC_API_KEY environment variable not set")?;

    // Create session store
    let store_path = std::env::var("RKAT_STORE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("meerkat")
                .join("sessions")
        });

    let store = JsonlStore::new(store_path);
    store
        .init()
        .await
        .map_err(|e| format!("Store init failed: {}", e))?;

    // Create LLM client
    let llm_client = Arc::new(AnthropicClient::new(api_key));
    let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, input.model.clone()));

    // Create tool dispatcher based on provided tools
    let tools = Arc::new(MpcToolDispatcher::new(&input.tools));
    let store_adapter = Arc::new(SessionStoreAdapter::new(Arc::new(store)));

    // Build agent
    let mut builder = AgentBuilder::new()
        .model(&input.model)
        .max_tokens_per_turn(input.max_tokens);

    if let Some(sys_prompt) = &input.system_prompt {
        builder = builder.system_prompt(sys_prompt);
    }

    let mut agent = builder.build(llm_adapter, tools, store_adapter);

    // Run agent
    let result = agent.run(input.prompt).await;

    match result {
        Ok(result) => Ok(json!({
            "content": [{
                "type": "text",
                "text": result.text
            }],
            "session_id": result.session_id.to_string(),
            "turns": result.turns,
            "tool_calls": result.tool_calls
        })),
        Err(AgentError::ToolError(ref msg)) if msg.starts_with(CALLBACK_TOOL_PREFIX) => {
            // Extract pending tool call info from the error
            let pending_info = &msg[CALLBACK_TOOL_PREFIX.len()..];
            let pending: Value = serde_json::from_str(pending_info).unwrap_or(json!({}));

            // Get session ID from agent state
            let session_id = agent.session().id();

            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": "Agent is waiting for tool results"
                }],
                "session_id": session_id.to_string(),
                "status": "pending_tool_call",
                "pending_tool_calls": [pending]
            }))
        }
        Err(e) => Err(format!("Agent error: {}", e)),
    }
}

async fn handle_meerkat_run_with_builtins(input: MeerkatRunInput) -> Result<Value, String> {
    use meerkat_tools::{
        BuiltinToolConfig, CompositeDispatcher, EnforcedToolPolicy, FileTaskStore, ToolMode,
        ToolPolicyLayer, builtin::shell::ShellConfig, ensure_rkat_dir, find_project_root,
    };

    // Get API key from environment
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "ANTHROPIC_API_KEY environment variable not set")?;

    // Create session store
    let store_path = std::env::var("RKAT_STORE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("meerkat")
                .join("sessions")
        });

    let session_store = JsonlStore::new(store_path);
    session_store
        .init()
        .await
        .map_err(|e| format!("Store init failed: {}", e))?;

    // Create LLM client
    let llm_client = Arc::new(AnthropicClient::new(api_key));
    let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, input.model.clone()));

    // Generate session ID upfront for task tracking
    let meerkat_session_id = meerkat::SessionId::new();

    // Set up built-in tools
    let cwd = std::env::current_dir().map_err(|e| format!("Failed to get current dir: {}", e))?;
    let project_root = find_project_root(&cwd);
    ensure_rkat_dir(&project_root).map_err(|e| format!("Failed to create .rkat dir: {}", e))?;
    let task_store = Arc::new(FileTaskStore::in_project(&project_root));

    // Build builtin tool config - enable shell tools if requested
    let mut policy = ToolPolicyLayer::new().with_mode(ToolMode::AllowAll);
    let enable_shell = input
        .builtin_config
        .as_ref()
        .is_some_and(|c| c.enable_shell);
    if enable_shell {
        policy = policy
            .enable_tool("shell")
            .enable_tool("shell_job_status")
            .enable_tool("shell_jobs")
            .enable_tool("shell_job_cancel");
    }

    let config = BuiltinToolConfig {
        policy,
        enforced: EnforcedToolPolicy::default(),
    };

    // Create shell config if enabled
    let shell_config = if enable_shell {
        let mut shell_cfg = ShellConfig::with_project_root(project_root);
        shell_cfg.enabled = true;
        if let Some(timeout) = input
            .builtin_config
            .as_ref()
            .and_then(|c| c.shell_timeout_secs)
        {
            shell_cfg.default_timeout_secs = timeout;
        }
        Some(shell_cfg)
    } else {
        None
    };

    // Create external dispatcher for MCP callback tools (if any)
    let external: Option<Arc<dyn AgentToolDispatcher>> = if input.tools.is_empty() {
        None
    } else {
        Some(Arc::new(MpcToolDispatcher::new(&input.tools)))
    };

    // Create composite dispatcher
    let tools = Arc::new(
        CompositeDispatcher::new(
            task_store,
            &config,
            shell_config,
            external,
            Some(meerkat_session_id.to_string()),
        )
        .map_err(|e| format!("Failed to create dispatcher: {}", e))?,
    );

    let store_adapter = Arc::new(SessionStoreAdapter::new(Arc::new(session_store)));

    // Build agent
    let mut builder = AgentBuilder::new()
        .model(&input.model)
        .max_tokens_per_turn(input.max_tokens);

    if let Some(sys_prompt) = &input.system_prompt {
        builder = builder.system_prompt(sys_prompt);
    }

    let mut agent = builder.build(llm_adapter, tools, store_adapter);

    // Run agent
    let result = agent.run(input.prompt).await;

    match result {
        Ok(result) => Ok(json!({
            "content": [{
                "type": "text",
                "text": result.text
            }],
            "session_id": result.session_id.to_string(),
            "turns": result.turns,
            "tool_calls": result.tool_calls
        })),
        Err(AgentError::ToolError(ref msg)) if msg.starts_with(CALLBACK_TOOL_PREFIX) => {
            let pending_info = &msg[CALLBACK_TOOL_PREFIX.len()..];
            let pending: Value = serde_json::from_str(pending_info).unwrap_or(json!({}));
            let session_id = agent.session().id();

            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": "Agent is waiting for tool results"
                }],
                "session_id": session_id.to_string(),
                "status": "pending_tool_call",
                "pending_tool_calls": [pending]
            }))
        }
        Err(e) => Err(format!("Agent error: {}", e)),
    }
}

async fn handle_meerkat_resume(input: MeerkatResumeInput) -> Result<Value, String> {
    // Branch based on whether builtins are enabled
    if input.enable_builtins {
        handle_meerkat_resume_with_builtins(input).await
    } else {
        handle_meerkat_resume_simple(input).await
    }
}

async fn handle_meerkat_resume_simple(input: MeerkatResumeInput) -> Result<Value, String> {
    // Get API key from environment
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "ANTHROPIC_API_KEY environment variable not set")?;

    // Create store
    let store_path = std::env::var("RKAT_STORE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("meerkat")
                .join("sessions")
        });

    let store = JsonlStore::new(store_path);
    store
        .init()
        .await
        .map_err(|e| format!("Store init failed: {}", e))?;

    // Load existing session
    let session_id = meerkat::SessionId::parse(&input.session_id)
        .map_err(|e| format!("Invalid session ID: {}", e))?;

    let mut session = store
        .load(&session_id)
        .await
        .map_err(|e| format!("Failed to load session: {}", e))?
        .ok_or_else(|| format!("Session not found: {}", input.session_id))?;

    // If tool results are provided, inject them into the session
    if !input.tool_results.is_empty() {
        use meerkat::ToolResult;
        let results: Vec<ToolResult> = input
            .tool_results
            .iter()
            .map(|r| ToolResult::new(r.tool_use_id.clone(), r.content.clone(), r.is_error))
            .collect();
        session.push(Message::ToolResults { results });
    }

    // Create LLM client
    let llm_client = Arc::new(AnthropicClient::new(api_key));
    let model = "claude-opus-4-5".to_string();
    let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, model.clone()));

    // Create tool dispatcher
    let tools = Arc::new(MpcToolDispatcher::new(&input.tools));
    let store_adapter = Arc::new(SessionStoreAdapter::new(Arc::new(store)));

    // Build agent with resumed session
    let mut agent = AgentBuilder::new()
        .model(&model)
        .max_tokens_per_turn(4096)
        .resume_session(session)
        .build(llm_adapter, tools, store_adapter);

    // Run agent - use empty prompt if only providing tool results
    let prompt = if input.prompt.is_empty() && !input.tool_results.is_empty() {
        // When resuming with tool results, the agent continues from where it left off
        String::new()
    } else {
        input.prompt
    };

    let result = agent.run(prompt).await;

    match result {
        Ok(result) => Ok(json!({
            "content": [{
                "type": "text",
                "text": result.text
            }],
            "session_id": result.session_id.to_string(),
            "turns": result.turns,
            "tool_calls": result.tool_calls
        })),
        Err(AgentError::ToolError(ref msg)) if msg.starts_with(CALLBACK_TOOL_PREFIX) => {
            // Extract pending tool call info from the error
            let pending_info = &msg[CALLBACK_TOOL_PREFIX.len()..];
            let pending: Value = serde_json::from_str(pending_info).unwrap_or(json!({}));

            // Get session ID from agent state
            let session_id = agent.session().id();

            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": "Agent is waiting for tool results"
                }],
                "session_id": session_id.to_string(),
                "status": "pending_tool_call",
                "pending_tool_calls": [pending]
            }))
        }
        Err(e) => Err(format!("Agent error: {}", e)),
    }
}

async fn handle_meerkat_resume_with_builtins(input: MeerkatResumeInput) -> Result<Value, String> {
    use meerkat_tools::{
        BuiltinToolConfig, CompositeDispatcher, EnforcedToolPolicy, FileTaskStore, ToolMode,
        ToolPolicyLayer, builtin::shell::ShellConfig, ensure_rkat_dir, find_project_root,
    };

    // Get API key from environment
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "ANTHROPIC_API_KEY environment variable not set")?;

    // Create store
    let store_path = std::env::var("RKAT_STORE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("meerkat")
                .join("sessions")
        });

    let session_store = JsonlStore::new(store_path);
    session_store
        .init()
        .await
        .map_err(|e| format!("Store init failed: {}", e))?;

    // Load existing session
    let session_id = meerkat::SessionId::parse(&input.session_id)
        .map_err(|e| format!("Invalid session ID: {}", e))?;

    let mut session = session_store
        .load(&session_id)
        .await
        .map_err(|e| format!("Failed to load session: {}", e))?
        .ok_or_else(|| format!("Session not found: {}", input.session_id))?;

    // If tool results are provided, inject them into the session
    if !input.tool_results.is_empty() {
        use meerkat::ToolResult;
        let results: Vec<ToolResult> = input
            .tool_results
            .iter()
            .map(|r| ToolResult::new(r.tool_use_id.clone(), r.content.clone(), r.is_error))
            .collect();
        session.push(Message::ToolResults { results });
    }

    // Create LLM client
    let llm_client = Arc::new(AnthropicClient::new(api_key));
    let model = "claude-opus-4-5".to_string();
    let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, model.clone()));

    // Set up built-in tools
    let cwd = std::env::current_dir().map_err(|e| format!("Failed to get current dir: {}", e))?;
    let project_root = find_project_root(&cwd);
    ensure_rkat_dir(&project_root).map_err(|e| format!("Failed to create .rkat dir: {}", e))?;
    let task_store = Arc::new(FileTaskStore::in_project(&project_root));

    // Build builtin tool config - enable shell tools if requested
    let mut policy = ToolPolicyLayer::new().with_mode(ToolMode::AllowAll);
    let enable_shell = input
        .builtin_config
        .as_ref()
        .is_some_and(|c| c.enable_shell);
    if enable_shell {
        policy = policy
            .enable_tool("shell")
            .enable_tool("shell_job_status")
            .enable_tool("shell_jobs")
            .enable_tool("shell_job_cancel");
    }

    let config = BuiltinToolConfig {
        policy,
        enforced: EnforcedToolPolicy::default(),
    };

    // Create shell config if enabled
    let shell_config = if enable_shell {
        let mut shell_cfg = ShellConfig::with_project_root(project_root);
        shell_cfg.enabled = true;
        if let Some(timeout) = input
            .builtin_config
            .as_ref()
            .and_then(|c| c.shell_timeout_secs)
        {
            shell_cfg.default_timeout_secs = timeout;
        }
        Some(shell_cfg)
    } else {
        None
    };

    // Create external dispatcher for MCP callback tools (if any)
    let external: Option<Arc<dyn AgentToolDispatcher>> = if input.tools.is_empty() {
        None
    } else {
        Some(Arc::new(MpcToolDispatcher::new(&input.tools)))
    };

    // Create composite dispatcher
    let tools = Arc::new(
        CompositeDispatcher::new(
            task_store,
            &config,
            shell_config,
            external,
            Some(session_id.to_string()),
        )
        .map_err(|e| format!("Failed to create dispatcher: {}", e))?,
    );

    let store_adapter = Arc::new(SessionStoreAdapter::new(Arc::new(session_store)));

    // Build agent with resumed session
    let mut agent = AgentBuilder::new()
        .model(&model)
        .max_tokens_per_turn(4096)
        .resume_session(session)
        .build(llm_adapter, tools, store_adapter);

    // Run agent - use empty prompt if only providing tool results
    let prompt = if input.prompt.is_empty() && !input.tool_results.is_empty() {
        String::new()
    } else {
        input.prompt
    };

    let result = agent.run(prompt).await;

    match result {
        Ok(result) => Ok(json!({
            "content": [{
                "type": "text",
                "text": result.text
            }],
            "session_id": result.session_id.to_string(),
            "turns": result.turns,
            "tool_calls": result.tool_calls
        })),
        Err(AgentError::ToolError(ref msg)) if msg.starts_with(CALLBACK_TOOL_PREFIX) => {
            let pending_info = &msg[CALLBACK_TOOL_PREFIX.len()..];
            let pending: Value = serde_json::from_str(pending_info).unwrap_or(json!({}));
            let session_id = agent.session().id();

            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": "Agent is waiting for tool results"
                }],
                "session_id": session_id.to_string(),
                "status": "pending_tool_call",
                "pending_tool_calls": [pending]
            }))
        }
        Err(e) => Err(format!("Agent error: {}", e)),
    }
}

// Adapter types needed for the MCP server

use async_trait::async_trait;
use meerkat::{
    AgentLlmClient, AgentSessionStore, AgentToolDispatcher, LlmStreamResult, Message, ToolDef,
};

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
        use futures_util::StreamExt;
        use meerkat::{LlmEvent, LlmRequest, StopReason, ToolCall, Usage};

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

/// Special error prefix to signal that a callback tool was invoked
pub const CALLBACK_TOOL_PREFIX: &str = "CALLBACK_TOOL_PENDING:";

/// MCP tool dispatcher - exposes tools to the LLM and handles callback tools
/// by returning a special error that signals the MCP client needs to handle the tool call
pub struct MpcToolDispatcher {
    tool_defs: Vec<ToolDef>,
    callback_tools: std::collections::HashSet<String>,
}

impl MpcToolDispatcher {
    /// Create a new tool dispatcher from MCP tool definitions
    pub fn new(mcp_tools: &[McpToolDef]) -> Self {
        let tool_defs = mcp_tools
            .iter()
            .map(|t| ToolDef {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.input_schema.clone(),
            })
            .collect();

        let callback_tools = mcp_tools
            .iter()
            .filter(|t| t.handler == "callback")
            .map(|t| t.name.clone())
            .collect();

        Self {
            tool_defs,
            callback_tools,
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for MpcToolDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        self.tool_defs.clone()
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        // Check if this is a callback tool
        if self.callback_tools.contains(name) {
            // Return a special error that signals the agent loop should pause
            // The error contains serialized info about the pending tool call
            Err(ToolError::other(format!(
                "{}{}",
                CALLBACK_TOOL_PREFIX,
                serde_json::to_string(&json!({
                    "tool_name": name,
                    "args": args
                }))
                .unwrap_or_default()
            )))
        } else {
            Err(ToolError::not_found(name))
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
    fn test_tools_list_schema() {
        let tools = tools_list();
        assert_eq!(tools.len(), 2);

        let run_tool = &tools[0];
        assert_eq!(run_tool["name"], "meerkat_run");
        assert!(run_tool["inputSchema"]["properties"]["prompt"].is_object());

        let resume_tool = &tools[1];
        assert_eq!(resume_tool["name"], "meerkat_resume");
        assert!(resume_tool["inputSchema"]["properties"]["session_id"].is_object());
    }

    #[test]
    fn test_meerkat_run_input_parsing() {
        let input_json = json!({
            "prompt": "Hello",
            "model": "claude-sonnet-4"
        });

        let input: MeerkatRunInput = serde_json::from_value(input_json).unwrap();
        assert_eq!(input.prompt, "Hello");
        assert_eq!(input.model, "claude-sonnet-4");
        assert_eq!(input.max_tokens, 4096); // default
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
        assert_eq!(input.tools[0].handler, "callback"); // default
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
    fn test_mpc_tool_dispatcher_creates_tool_defs() {
        let mcp_tools = vec![
            McpToolDef {
                name: "get_weather".to_string(),
                description: "Get weather".to_string(),
                input_schema: json!({"type": "object"}),
                handler: "callback".to_string(),
            },
            McpToolDef {
                name: "search".to_string(),
                description: "Search".to_string(),
                input_schema: json!({"type": "object"}),
                handler: "callback".to_string(),
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
            input_schema: json!({"type": "object"}),
            handler: "callback".to_string(),
        }];

        let dispatcher = MpcToolDispatcher::new(&mcp_tools);
        let result = dispatcher
            .dispatch("get_weather", &json!({"city": "Tokyo"}))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.starts_with(CALLBACK_TOOL_PREFIX));
        assert!(err.contains("get_weather"));
        assert!(err.contains("Tokyo"));
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
        assert!(resume_tool["inputSchema"]["properties"]["tool_results"].is_object());
        assert_eq!(
            resume_tool["inputSchema"]["properties"]["tool_results"]["type"],
            "array"
        );
    }
}
