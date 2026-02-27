//! # 013 — Context Compaction (Rust)
//!
//! Long conversations eventually exceed the LLM's context window. Meerkat's
//! compaction system automatically summarizes old messages to keep the agent
//! running indefinitely without losing important context.
//!
//! ## What you'll learn
//! - How the `DefaultCompactor` works
//! - Configuring compaction thresholds
//! - The compaction flow within the agent loop
//! - Preserving critical messages during compaction
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=... cargo run --example 013-context-compaction --features jsonl-store,session-compaction
//! ```

use std::sync::Arc;

use meerkat::{AgentBuilder, AgentEvent, AgentFactory, AnthropicClient, DefaultCompactor};
use meerkat_core::compact::CompactionConfig;
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::EmptyToolDispatcher;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    let _tmp = tempfile::tempdir()?;
    let store_dir = _tmp.path().join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    let factory = AgentFactory::new(store_dir.clone());
    let client = Arc::new(AnthropicClient::new(api_key)?);
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4-5").await;

    let store = Arc::new(JsonlStore::new(store_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));

    // ── Configure the compactor ────────────────────────────────────────────
    //
    // We set a LOW token threshold (2000) so compaction triggers during this
    // short demo. In production you'd use a much higher value (e.g. 100_000).
    let compaction_config = CompactionConfig {
        auto_compact_threshold: 2000,
        recent_turn_budget: 2,
        max_summary_tokens: 1024,
        min_turns_between_compactions: 2,
    };
    let compactor = Arc::new(DefaultCompactor::new(compaction_config));

    println!("=== Context Compaction Demo ===");
    println!("Compaction threshold: 2000 tokens (low for demo purposes)");
    println!("Recent turns preserved: 2\n");

    // Build agent with the compactor wired in.
    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-5")
        .system_prompt(
            "You are a patient tutor. Build on previous conversation context. \
             Always reference earlier topics when relevant. Give thorough, \
             detailed explanations with code examples.",
        )
        .max_tokens_per_turn(1024)
        .compactor(compactor)
        .build(Arc::new(llm), Arc::new(EmptyToolDispatcher), store)
        .await;

    // Simulate a long conversation that will trigger compaction.
    // Each prompt asks for detailed explanations to accumulate tokens quickly.
    let topics = [
        "Explain Rust's ownership model in detail with code examples.",
        "Now explain how lifetimes relate to what you just said about ownership. Include examples.",
        "How do smart pointers like Box, Rc, and Arc fit into this picture? Show code.",
        "Give me a practical example combining ownership, lifetimes, and Arc.",
        "Summarize everything we've discussed about Rust's memory model.",
    ];

    let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(256);

    // Monitor for compaction events — these will fire when the compactor
    // detects that accumulated tokens exceed our 2000-token threshold.
    let monitor = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match &event {
                AgentEvent::CompactionStarted { .. } => {
                    println!("\n[COMPACTION] Context compaction triggered!");
                }
                AgentEvent::CompactionCompleted { .. } => {
                    println!("[COMPACTION] Compaction completed — context window refreshed.\n");
                }
                _ => {}
            }
        }
    });

    for (i, topic) in topics.iter().enumerate() {
        println!("\n=== Turn {} ===", i + 1);
        println!("User: {topic}\n");

        let result = agent
            .run_with_events(topic.to_string(), event_tx.clone())
            .await?;

        println!("Assistant: {}", &result.text[..result.text.len().min(200)]);
        if result.text.len() > 200 {
            println!("...");
        }
        println!(
            "(input tokens: {}, output tokens: {}, total: {})",
            result.usage.input_tokens,
            result.usage.output_tokens,
            result.usage.total_tokens()
        );
    }

    drop(event_tx);
    let _ = monitor.await;

    // ── Compaction configuration reference ─────────────────────────────────

    println!("\n\n=== Compaction Configuration Reference ===\n");
    println!(
        r"# .rkat/config.toml

[compaction]
# Trigger compaction when cumulative tokens exceed this threshold
token_threshold = 50000

# Maximum tokens for the compaction summary
summary_max_tokens = 1024

# Always preserve the system prompt during compaction
preserve_system = true

# Always preserve the N most recent message pairs
preserve_most_recent_n = 2

# How compaction works:
#
# 1. Agent loop detects token count > token_threshold
# 2. Compactor selects messages to summarize (excluding preserved ones)
# 3. LLM generates a concise summary of the selected messages
# 4. Old messages are replaced with the summary
# 5. Agent continues with reduced context but preserved knowledge
#
# The result: agents that can run indefinitely without losing important
# context or exceeding the LLM's context window.
"
    );

    Ok(())
}
