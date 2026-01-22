//! meerkat-mcp-server - MCP server exposing Meerkat as a tool
//!
//! This crate provides an MCP server that exposes Meerkat agent capabilities
//! as MCP tools: meerkat_run and meerkat_resume.

use meerkat::{
    AgentBuilder, AgentError, AnthropicClient, JsonlStore, LlmClient, Session, SessionStore,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

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
}

/// Returns the list of tools exposed by this MCP server
pub fn tools_list() -> Vec<Value> {
    vec![
        json!({
            "name": "meerkat_run",
            "description": "Run a new Meerkat agent with the given prompt. Returns the agent's response.",
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
                    }
                },
                "required": ["prompt"]
            }
        }),
        json!({
            "name": "meerkat_resume",
            "description": "Resume an existing Meerkat session with a new prompt.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "The session ID to resume"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The prompt to continue the conversation"
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

    // Create LLM client
    let llm_client = Arc::new(AnthropicClient::new(api_key));
    let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, input.model.clone()));

    // Create empty tool dispatcher
    let tools = Arc::new(EmptyToolDispatcher);
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
    let result = agent
        .run(input.prompt)
        .await
        .map_err(|e| format!("Agent error: {}", e))?;

    Ok(json!({
        "content": [{
            "type": "text",
            "text": result.text
        }],
        "session_id": result.session_id.to_string(),
        "turns": result.turns,
        "tool_calls": result.tool_calls
    }))
}

async fn handle_meerkat_resume(input: MeerkatResumeInput) -> Result<Value, String> {
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

    let session = store
        .load(&session_id)
        .await
        .map_err(|e| format!("Failed to load session: {}", e))?
        .ok_or_else(|| format!("Session not found: {}", input.session_id))?;

    // Create LLM client
    let llm_client = Arc::new(AnthropicClient::new(api_key));
    let model = "claude-opus-4-5".to_string();
    let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, model.clone()));

    // Create empty tool dispatcher
    let tools = Arc::new(EmptyToolDispatcher);
    let store_adapter = Arc::new(SessionStoreAdapter::new(Arc::new(store)));

    // Build agent with resumed session
    let mut agent = AgentBuilder::new()
        .model(&model)
        .max_tokens_per_turn(4096)
        .resume_session(session)
        .build(llm_adapter, tools, store_adapter);

    // Run agent
    let result = agent
        .run(input.prompt)
        .await
        .map_err(|e| format!("Agent error: {}", e))?;

    Ok(json!({
        "content": [{
            "type": "text",
            "text": result.text
        }],
        "session_id": result.session_id.to_string(),
        "turns": result.turns,
        "tool_calls": result.tool_calls
    }))
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
    ) -> Result<LlmStreamResult, AgentError> {
        use futures_util::StreamExt;
        use meerkat::{LlmEvent, LlmRequest, StopReason, ToolCall, Usage};

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
}
