#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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
use meerkat::{
    AgentBuilder, AgentFactory, AgentToolDispatcher, AnthropicClient, ToolDef, ToolError,
    ToolResult,
};
use meerkat_core::ToolCallView;
use meerkat_store::JsonlStore;
use meerkat_store::StoreAdapter;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Clone, JsonSchema, Deserialize)]
#[allow(dead_code)]
struct BinaryMathArgs {
    #[schemars(description = "First number")]
    a: f64,
    #[schemars(description = "Second number")]
    b: f64,
}

// Custom tool dispatcher that handles our tools
struct MathToolDispatcher;

#[async_trait]
impl AgentToolDispatcher for MathToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![
            Arc::new(ToolDef {
                name: "add".to_string(),
                description: "Add two numbers together".to_string(),
                input_schema: meerkat_tools::schema_for::<BinaryMathArgs>(),
            }),
            Arc::new(ToolDef {
                name: "multiply".to_string(),
                description: "Multiply two numbers".to_string(),
                input_schema: meerkat_tools::schema_for::<BinaryMathArgs>(),
            }),
        ]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        let args: BinaryMathArgs = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
        let value = match call.name {
            "add" => json!(args.a + args.b),
            "multiply" => json!(args.a * args.b),
            _ => return Err(ToolError::not_found(call.name)),
        };
        Ok(ToolResult::new(
            call.id.to_string(),
            value.to_string(),
            false,
        ))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Get API key from environment
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "ANTHROPIC_API_KEY environment variable must be set")?;

    let store_dir = std::env::current_dir()?.join(".rkat").join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    let factory = AgentFactory::new(store_dir.clone());

    // Create components using shared factory helpers
    let client = Arc::new(AnthropicClient::new(api_key)?);
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4").await;

    let store = Arc::new(JsonlStore::new(store_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));

    let tools = Arc::new(MathToolDispatcher);

    // Build and run the agent
    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4")
        .system_prompt("You are a math assistant. Use the provided tools to perform calculations.")
        .max_tokens_per_turn(1024)
        .build(Arc::new(llm), tools, store)
        .await;

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
