//! # 006 — Custom Tools (Rust)
//!
//! Agents become powerful when they can take actions. This example shows how
//! to define custom tools with typed arguments and wire them into the agent loop.
//!
//! ## What you'll learn
//! - Implementing `AgentToolDispatcher` for custom tools
//! - Using `schemars` for automatic JSON Schema generation
//! - Handling tool dispatch with pattern matching
//! - Combining multiple tools in a single dispatcher
//!
//! ## Run
//! ```bash
//! This is a reference implementation. For runnable examples, see meerkat/examples/.
//! ```

use async_trait::async_trait;
use meerkat::{
    AgentBuilder, AgentFactory, AgentToolDispatcher, AnthropicClient, ToolDef, ToolError, ToolResult,
};
use meerkat_core::ToolCallView;
use meerkat_store::{JsonlStore, StoreAdapter};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

// ── Tool argument schemas ──────────────────────────────────────────────────

/// Arguments for looking up weather data.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
struct WeatherArgs {
    /// City name (e.g. "San Francisco")
    city: String,
    /// Temperature unit: "celsius" or "fahrenheit"
    #[serde(default = "default_unit")]
    unit: String,
}

fn default_unit() -> String {
    "celsius".to_string()
}

/// Arguments for unit conversion.
#[derive(Debug, Clone, JsonSchema, Deserialize)]
struct ConvertArgs {
    /// Numeric value to convert
    value: f64,
    /// Source unit (e.g. "km", "miles", "kg", "lbs")
    from: String,
    /// Target unit
    to: String,
}

// ── Tool dispatcher ────────────────────────────────────────────────────────

struct WeatherAndConvertDispatcher;

#[async_trait]
impl AgentToolDispatcher for WeatherAndConvertDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![
            Arc::new(ToolDef {
                name: "get_weather".to_string(),
                description: "Get current weather for a city (simulated data)".to_string(),
                input_schema: meerkat_tools::schema_for::<WeatherArgs>(),
            }),
            Arc::new(ToolDef {
                name: "convert_units".to_string(),
                description: "Convert between measurement units".to_string(),
                input_schema: meerkat_tools::schema_for::<ConvertArgs>(),
            }),
        ]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        match call.name {
            "get_weather" => {
                let args: WeatherArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;

                // Simulated weather data
                let temp = match args.city.to_lowercase().as_str() {
                    "san francisco" => 18.0,
                    "new york" => 25.0,
                    "london" => 14.0,
                    "tokyo" => 28.0,
                    _ => 20.0,
                };
                let display_temp = if args.unit == "fahrenheit" {
                    temp * 9.0 / 5.0 + 32.0
                } else {
                    temp
                };

                let result = json!({
                    "city": args.city,
                    "temperature": display_temp,
                    "unit": args.unit,
                    "condition": "partly cloudy",
                    "humidity": 65,
                });
                Ok(ToolResult::new(call.id.to_string(), result.to_string(), false))
            }

            "convert_units" => {
                let args: ConvertArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;

                let result = match (args.from.as_str(), args.to.as_str()) {
                    ("km", "miles") => args.value * 0.621371,
                    ("miles", "km") => args.value * 1.60934,
                    ("kg", "lbs") => args.value * 2.20462,
                    ("lbs", "kg") => args.value * 0.453592,
                    ("celsius", "fahrenheit") => args.value * 9.0 / 5.0 + 32.0,
                    ("fahrenheit", "celsius") => (args.value - 32.0) * 5.0 / 9.0,
                    _ => {
                        return Ok(ToolResult::new(
                            call.id.to_string(),
                            format!("Cannot convert from '{}' to '{}'", args.from, args.to),
                            true, // is_error = true
                        ));
                    }
                };

                let output = json!({
                    "original": args.value,
                    "from": args.from,
                    "converted": result,
                    "to": args.to,
                });
                Ok(ToolResult::new(call.id.to_string(), output.to_string(), false))
            }

            _ => Err(ToolError::not_found(call.name)),
        }
    }
}

// ── Main ───────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    let store_dir = tempfile::tempdir()?.keep().join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    let factory = AgentFactory::new(store_dir.clone());
    let client = Arc::new(AnthropicClient::new(api_key)?);
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4-5").await;

    let store = Arc::new(JsonlStore::new(store_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));

    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-5")
        .system_prompt(
            "You are a travel assistant. Use the get_weather and convert_units tools \
             to help the user plan trips. Always check weather and convert units when relevant.",
        )
        .max_tokens_per_turn(2048)
        .build(Arc::new(llm), Arc::new(WeatherAndConvertDispatcher), store)
        .await;

    let result = agent
        .run(
            "I'm planning a trip from San Francisco to Tokyo. \
             What's the weather like in both cities? \
             Also, how far is it in miles if it's about 8,280 km?"
                .to_string(),
        )
        .await?;

    println!("Response:\n{}", result.text);
    println!("\n--- Stats ---");
    println!("Tool calls: {}", result.tool_calls);
    println!("Turns:      {}", result.turns);
    println!("Tokens:     {}", result.usage.total_tokens());

    Ok(())
}
