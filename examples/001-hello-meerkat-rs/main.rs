//! # 001 — Hello Meerkat (Rust)
//!
//! The simplest possible Meerkat agent: send one prompt, get one response.
//! This is the Rust SDK equivalent of "Hello, World!"
//!
//! ## What you'll learn
//! - Creating an LLM client from an API key
//! - Building an agent with `AgentBuilder`
//! - Running a single-turn agent and reading the result
//!
//! ## Run
//! ```bash
//! This is a reference implementation. For runnable examples, see meerkat/examples/.
//! ```

use std::sync::Arc;

use meerkat::{AgentBuilder, AgentFactory, AnthropicClient};
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::EmptyToolDispatcher;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    // 1. Create a temporary session store
    let store_dir = tempfile::tempdir()?.keep().join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    // 2. Wire up the LLM client via AgentFactory
    let factory = AgentFactory::new(store_dir.clone());
    let client = Arc::new(AnthropicClient::new(api_key)?);
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4-5").await;

    // 3. Create a simple session store
    let store = Arc::new(JsonlStore::new(store_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));

    // 4. Build the agent — no tools needed for simple Q&A
    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-5")
        .system_prompt("You are a helpful assistant. Be concise.")
        .max_tokens_per_turn(512)
        .build(Arc::new(llm), Arc::new(EmptyToolDispatcher), store)
        .await;

    // 5. Run a single turn
    let result = agent
        .run("What makes Rust's ownership model unique? Answer in two sentences.".to_string())
        .await?;

    println!("{}", result.text);
    println!("\n--- Stats ---");
    println!("Session:  {}", result.session_id);
    println!("Turns:    {}", result.turns);
    println!("Tokens:   {}", result.usage.total_tokens());

    Ok(())
}
