//! Simple example demonstrating basic Meerkat usage
//!
//! Run with:
//! ```bash
//! ANTHROPIC_API_KEY=your-key cargo run --example simple
//! ```

use std::sync::Arc;

use meerkat::{AgentBuilder, AgentFactory, AnthropicClient, JsonlStore};
use meerkat_store::StoreAdapter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Get API key from environment
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .expect("ANTHROPIC_API_KEY environment variable must be set");

    let store_dir = std::env::current_dir()?.join(".rkat").join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    let factory = AgentFactory::new(store_dir.clone());

    let client = Arc::new(AnthropicClient::new(api_key));
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4");

    let store = Arc::new(JsonlStore::new(store_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));

    let tools = Arc::new(meerkat_tools::EmptyToolDispatcher);

    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4")
        .system_prompt("You are a helpful assistant. Be concise in your responses.")
        .max_tokens_per_turn(1024)
        .build(Arc::new(llm), tools, store);

    let result = agent
        .run("What is the capital of France? Answer in one sentence.".to_string())
        .await?;

    println!("Response: {}", result.text);
    println!("\n--- Stats ---");
    println!("Session ID: {}", result.session_id);
    println!("Turns: {}", result.turns);
    println!("Total tokens: {}", result.usage.total_tokens());

    Ok(())
}
