//! # 005 — Streaming Events (Rust)
//!
//! Meerkat agents emit a rich event stream during execution. This example
//! shows how to tap into that stream for real-time UIs, logging, or metrics.
//!
//! ## What you'll learn
//! - Setting up an event channel with `tokio::sync::mpsc`
//! - Processing `AgentEvent` variants (text deltas, tool calls, etc.)
//! - Using the SDK `spawn_event_logger` helper
//!
//! ## Run
//! ```bash
//! This is a reference implementation. For runnable examples, see meerkat/examples/.
//! ```

use std::sync::Arc;

use meerkat::{
    AgentBuilder, AgentEvent, AgentFactory, AnthropicClient, EventLoggerConfig, spawn_event_logger,
};
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::EmptyToolDispatcher;
use tokio::sync::mpsc;

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
        .system_prompt("You are a creative writing assistant.")
        .max_tokens_per_turn(1024)
        .build(Arc::new(llm), Arc::new(EmptyToolDispatcher), store)
        .await;

    // --- Option A: Use the built-in event logger (simplest) ---
    println!("=== Streaming with built-in logger ===\n");
    let (event_tx, event_rx) = mpsc::channel::<AgentEvent>(256);
    let logger_handle = spawn_event_logger(
        event_rx,
        EventLoggerConfig {
            verbose: false,
            stream: true, // Print text deltas to stdout in real time
        },
    );

    let result = agent
        .run_with_events(
            "Write a short poem about a meerkat standing guard.".to_string(),
            event_tx,
        )
        .await?;

    logger_handle.await?;
    println!("\n\n--- Stats ---");
    println!("Tokens: {}", result.usage.total_tokens());

    // --- Option B: Custom event processing ---
    println!("\n=== Custom event handler ===\n");
    let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(256);

    let processor = tokio::spawn(async move {
        let mut text_bytes = 0usize;
        let mut tool_calls = 0usize;

        while let Some(event) = event_rx.recv().await {
            match &event {
                AgentEvent::TextDelta { delta } => {
                    text_bytes += delta.len();
                    print!("{delta}");
                }
                AgentEvent::ToolCallRequested { name, .. } => {
                    tool_calls += 1;
                    eprintln!("\n[tool requested: {name}]");
                }
                AgentEvent::TurnCompleted { .. } => {
                    eprintln!("\n[turn completed — {text_bytes} bytes, {tool_calls} tool calls]");
                }
                _ => {} // Many more event types available
            }
        }
    });

    let result = agent
        .run_with_events(
            "Now write a limerick about the same meerkat.".to_string(),
            event_tx,
        )
        .await?;

    processor.await?;
    println!("\n\nSession: {}", result.session_id);

    Ok(())
}
