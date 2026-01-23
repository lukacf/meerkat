//! Example demonstrating Meerkat with custom tools
//!
//! This example shows how to use the full AgentBuilder API
//! with custom tool definitions and dispatch.
//!
//! Run with:
//! ```bash
//! ANTHROPIC_API_KEY=your-key cargo run --example with_tools
//! ```

use async_trait::async_trait;
use meerkat::prelude::*;
use meerkat::{
    AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, LlmStreamResult, Session,
    ToolDef, ToolError,
};
use serde_json::{Value, json};
use std::sync::Arc;

// Custom tool dispatcher that handles our tools
struct MathToolDispatcher;

#[async_trait]
impl AgentToolDispatcher for MathToolDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "add".to_string(),
                description: "Add two numbers together".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "a": {"type": "number", "description": "First number"},
                        "b": {"type": "number", "description": "Second number"}
                    },
                    "required": ["a", "b"]
                }),
            },
            ToolDef {
                name: "multiply".to_string(),
                description: "Multiply two numbers".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "a": {"type": "number", "description": "First number"},
                        "b": {"type": "number", "description": "Second number"}
                    },
                    "required": ["a", "b"]
                }),
            },
        ]
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        match name {
            "add" => {
                let a = args["a"]
                    .as_f64()
                    .ok_or_else(|| ToolError::invalid_arguments(name, "Missing 'a' argument"))?;
                let b = args["b"]
                    .as_f64()
                    .ok_or_else(|| ToolError::invalid_arguments(name, "Missing 'b' argument"))?;
                Ok(json!(a + b))
            }
            "multiply" => {
                let a = args["a"]
                    .as_f64()
                    .ok_or_else(|| ToolError::invalid_arguments(name, "Missing 'a' argument"))?;
                let b = args["b"]
                    .as_f64()
                    .ok_or_else(|| ToolError::invalid_arguments(name, "Missing 'b' argument"))?;
                Ok(json!(a * b))
            }
            _ => Err(ToolError::not_found(name)),
        }
    }
}

// LLM adapter that wraps the AnthropicClient
struct AnthropicLlmAdapter {
    client: Arc<AnthropicClient>,
    model: String,
}

impl AnthropicLlmAdapter {
    fn new(api_key: String, model: String) -> Self {
        Self {
            client: Arc::new(AnthropicClient::new(api_key)),
            model,
        }
    }
}

#[async_trait]
impl AgentLlmClient for AnthropicLlmAdapter {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&serde_json::Value>,
    ) -> Result<LlmStreamResult, meerkat::AgentError> {
        use futures::StreamExt;
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
                    return Err(meerkat::AgentError::LlmError(e.to_string()));
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
        "anthropic"
    }
}

// In-memory session store
struct MemoryStore {
    sessions: std::sync::Mutex<std::collections::HashMap<String, Session>>,
}

impl MemoryStore {
    fn new() -> Self {
        Self {
            sessions: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

#[async_trait]
impl AgentSessionStore for MemoryStore {
    async fn save(&self, session: &Session) -> Result<(), meerkat::AgentError> {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.insert(session.id().to_string(), session.clone());
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<Session>, meerkat::AgentError> {
        let sessions = self.sessions.lock().unwrap();
        Ok(sessions.get(id).cloned())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Get API key from environment
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("ANTHROPIC_API_KEY environment variable must be set");

    // Create components
    let llm = Arc::new(AnthropicLlmAdapter::new(
        api_key,
        "claude-sonnet-4".to_string(),
    ));
    let tools = Arc::new(MathToolDispatcher);
    let store = Arc::new(MemoryStore::new());

    // Build and run the agent
    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4")
        .system_prompt("You are a math assistant. Use the provided tools to perform calculations.")
        .max_tokens_per_turn(1024)
        .build(llm, tools, store);

    let result = agent
        .run("What is 25 + 17, and then multiply the result by 3?".to_string())
        .await?;

    println!("Response: {}", result.text);
    println!("\n--- Stats ---");
    println!("Session ID: {}", result.session_id);
    println!("Turns: {}", result.turns);
    println!("Tool calls: {}", result.tool_calls);
    println!("Total tokens: {}", result.usage.total_tokens());

    Ok(())
}
