//! # 005 — Streaming Events (Rust)
//!
//! Meerkat agents emit a rich event stream during execution. This example
//! shows how to tap into that stream for real-time UIs, logging, or metrics.
//!
//! ## What you'll learn
//! - Setting up an event channel with `tokio::sync::mpsc` and giving it to
//!   both the model adapter (text deltas) and the run (turn-level events)
//! - Processing `AgentEvent` variants (text deltas, tool calls, etc.)
//! - Using the SDK `spawn_event_logger` helper
//!
//! ## Run
//! ```bash
//! # From the repository root
//! ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
//!   --example 005-streaming-events --features jsonl-store
//! ```

use std::{
    io::{self, Write},
    sync::Arc,
};

use meerkat::{
    AgentBuilder, AgentEvent, AgentFactory, AnthropicClient, EventLoggerConfig, spawn_event_logger,
};
use meerkat_client::LlmClient;
use meerkat_core::RunResult;
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::EmptyToolDispatcher;
use tokio::sync::mpsc;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    meerkat_runtime::host_stack::run_host("streaming-events", run)?
}

async fn process_events(
    mut event_rx: mpsc::Receiver<AgentEvent>,
    mut output: impl Write,
) -> io::Result<()> {
    let mut text_bytes = 0usize;
    let mut tool_calls = 0usize;
    while let Some(event) = event_rx.recv().await {
        match &event {
            AgentEvent::TextDelta { delta, .. } => {
                text_bytes += delta.len();
                output.write_all(delta.as_bytes())?;
                output.flush()?;
            }
            AgentEvent::ToolCallRequested { name, .. } => {
                tool_calls += 1;
                eprintln!("\n[tool requested: {name}]");
            }
            AgentEvent::TurnCompleted { .. } => {
                eprintln!("\n[turn completed — {text_bytes} bytes, {tool_calls} tool calls]");
            }
            _ => {}
        }
    }
    Ok(())
}

const MODEL: &str = "claude-sonnet-4-6";

/// Runs one prompt and sends every event of the run to `event_tx`.
///
/// Text deltas are emitted by the model adapter; the agent emits the
/// turn-level events. Give both the same channel: an adapter built without
/// one (`build_llm_adapter`) emits no text deltas, and `run_with_events` does
/// not add them.
async fn run_streaming(
    factory: &AgentFactory,
    client: Arc<dyn LlmClient>,
    store: Arc<StoreAdapter<JsonlStore>>,
    prompt: &str,
    event_tx: mpsc::Sender<AgentEvent>,
) -> Result<RunResult, Box<dyn std::error::Error + Send + Sync>> {
    let llm = factory
        .build_llm_adapter_with_events(client, MODEL, Some(event_tx.clone()))
        .await;
    let mut agent = AgentBuilder::new()
        .model(MODEL)
        .system_prompt("You are a creative writing assistant.")
        .max_tokens_per_turn(1024)
        .build(Arc::new(llm), Arc::new(EmptyToolDispatcher), store)
        .await?;

    let result = agent.run_with_events(prompt.into(), event_tx).await?;
    // The adapter inside `agent` holds a sender. The agent is dropped when
    // this function returns, which closes the channel and so ends a consumer
    // that reads until the channel is closed.
    Ok(result)
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    let _tmp = tempfile::tempdir()?;
    let store_dir = _tmp.path().join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    let factory = AgentFactory::new(store_dir.clone());
    let client: Arc<dyn LlmClient> = Arc::new(AnthropicClient::new(api_key)?);

    let store = Arc::new(JsonlStore::new(store_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));

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

    let result = run_streaming(
        &factory,
        client.clone(),
        store.clone(),
        "Write a short poem about a meerkat standing guard.",
        event_tx,
    )
    .await?;

    logger_handle.await?;
    println!("\n\n--- Stats ---");
    println!("Tokens: {}", result.usage.total_tokens());

    // --- Option B: Custom event processing ---
    println!("\n=== Custom event handler ===\n");
    let (event_tx, event_rx) = mpsc::channel::<AgentEvent>(256);
    let processor = tokio::spawn(process_events(event_rx, io::stdout()));

    let result = run_streaming(
        &factory,
        client,
        store,
        "Write a limerick about a meerkat standing guard.",
        event_tx,
    )
    .await;

    // Join even if the agent failed. On I/O failure the receiver drops, so the
    // producer is not stranded waiting for an event consumer.
    processor.await??;
    let result = result?;
    println!("\n\nSession: {}", result.session_id);

    Ok(())
}

#[cfg(test)]
mod tests;
