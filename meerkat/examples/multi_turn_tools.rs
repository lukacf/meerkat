//! Example demonstrating multi-turn conversation with tool usage
//!
//! This example shows how an agent can use different tools across
//! multiple conversation turns, maintaining context between calls.
//!
//! Run with:
//! ```bash
//! ANTHROPIC_API_KEY=your-key cargo run --example multi_turn_tools
//! ```

use async_trait::async_trait;
use meerkat::{
    AgentBuilder, AgentFactory, AgentToolDispatcher, AnthropicClient, ToolDef, ToolError,
};
use meerkat_store::{JsonlStore, StoreAdapter};
use serde_json::{Value, json};
use std::sync::Arc;

// Tool dispatcher with multiple specialized tools
struct MultiToolDispatcher {
    // Simulated state that tools can modify
    state: std::sync::Mutex<AppState>,
}

struct AppState {
    notes: Vec<String>,
    calculations: Vec<f64>,
}

impl MultiToolDispatcher {
    fn new() -> Self {
        Self {
            state: std::sync::Mutex::new(AppState {
                notes: Vec::new(),
                calculations: Vec::new(),
            }),
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for MultiToolDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        vec![
            // Calculator tool
            ToolDef {
                name: "calculate".to_string(),
                description: "Perform arithmetic calculations. Supports +, -, *, / operations."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "expression": {
                            "type": "string",
                            "description": "A simple arithmetic expression like '2 + 3' or '10 * 5'"
                        }
                    },
                    "required": ["expression"]
                }),
            },
            // Note-taking tool
            ToolDef {
                name: "save_note".to_string(),
                description: "Save a note for later reference".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "note": {
                            "type": "string",
                            "description": "The note content to save"
                        }
                    },
                    "required": ["note"]
                }),
            },
            // Note retrieval tool
            ToolDef {
                name: "get_notes".to_string(),
                description: "Retrieve all saved notes".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            // History tool
            ToolDef {
                name: "get_calculation_history".to_string(),
                description: "Get the history of all calculations performed".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
        ]
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        match name {
            "calculate" => {
                let expr = args["expression"].as_str().ok_or_else(|| {
                    ToolError::invalid_arguments(name, "Missing 'expression' argument")
                })?;

                // Simple expression parser (for demo purposes)
                let result = parse_and_calculate(expr).map_err(ToolError::execution_failed)?;

                // Store in history
                let mut state = self.state.lock().unwrap();
                state.calculations.push(result);

                Ok(json!(format!("{} = {}", expr, result)))
            }
            "save_note" => {
                let note = args["note"]
                    .as_str()
                    .ok_or_else(|| ToolError::invalid_arguments(name, "Missing 'note' argument"))?;

                let mut state = self.state.lock().unwrap();
                state.notes.push(note.to_string());

                Ok(json!("Note saved"))
            }
            "get_notes" => {
                let state = self.state.lock().unwrap();
                Ok(json!(state.notes))
            }
            "get_calculation_history" => {
                let state = self.state.lock().unwrap();
                Ok(json!(state.calculations))
            }
            _ => Err(ToolError::not_found(name)),
        }
    }
}

// Simple expression parser for demo purposes
fn parse_and_calculate(expr: &str) -> Result<f64, String> {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.len() != 3 {
        return Err(format!("Invalid expression format: {}", expr));
    }

    let a: f64 = parts[0].parse().map_err(|_| "Invalid first number")?;
    let b: f64 = parts[2].parse().map_err(|_| "Invalid second number")?;

    match parts[1] {
        "+" => Ok(a + b),
        "-" => Ok(a - b),
        "*" => Ok(a * b),
        "/" => {
            if b == 0.0 {
                Err("Division by zero".to_string())
            } else {
                Ok(a / b)
            }
        }
        op => Err(format!("Unknown operator: {}", op)),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("ANTHROPIC_API_KEY environment variable must be set");

    let store_dir = std::env::current_dir()?.join(".rkat").join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    let factory = AgentFactory::new(store_dir.clone());

    // Create components - tools maintain state across turns
    let client = Arc::new(AnthropicClient::new(api_key));
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4");

    let store = Arc::new(JsonlStore::new(store_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));

    let tools = Arc::new(MultiToolDispatcher::new());

    // Build the agent
    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4")
        .system_prompt(
            "You are a helpful assistant with access to tools for calculations and note-taking. \
             Use tools to help the user with their requests. When asked to perform calculations \
             or save notes, always use the appropriate tool.",
        )
        .max_tokens_per_turn(2048)
        .build(Arc::new(llm), tools, store);

    println!("=== Multi-Turn Tool Usage Example ===\n");

    // Turn 1: Initial calculation and note
    println!("--- Turn 1: Calculate and save a note ---");
    let result = agent
        .run("Calculate 15 * 8, then save a note about the result.".to_string())
        .await?;
    println!("Response: {}", result.text);
    println!("Tool calls: {}", result.tool_calls);
    println!();

    // Turn 2: More calculations
    println!("--- Turn 2: More calculations ---");
    let result = agent
        .run("Now calculate 100 / 4 and 25 + 37.".to_string())
        .await?;
    println!("Response: {}", result.text);
    println!("Tool calls: {}", result.tool_calls);
    println!();

    // Turn 3: Review history
    println!("--- Turn 3: Review calculation history and notes ---");
    let result = agent
        .run("Show me all the calculations we've done and any notes we've saved.".to_string())
        .await?;
    println!("Response: {}", result.text);
    println!("Tool calls: {}", result.tool_calls);
    println!();

    // Turn 4: Save summary note
    println!("--- Turn 4: Save a summary ---");
    let result = agent
        .run("Save a note summarizing our calculation session.".to_string())
        .await?;
    println!("Response: {}", result.text);
    println!("Tool calls: {}", result.tool_calls);
    println!();

    // Final stats
    println!("=== Final Statistics ===");
    println!("Session ID: {}", result.session_id);
    println!("Total turns: {}", result.turns);
    println!("Total tokens: {}", result.usage.total_tokens());

    Ok(())
}
