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
//! ANTHROPIC_API_KEY=your-key cargo run --example 013_context_compaction
//! ```

use std::sync::Arc;

use meerkat::{
    AgentBuilder, AgentEvent, AgentFactory, AnthropicClient,
};
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::EmptyToolDispatcher;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    let store_dir = tempfile::tempdir()?.into_path().join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    let factory = AgentFactory::new(store_dir.clone());
    let client = Arc::new(AnthropicClient::new(api_key)?);
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4-5").await;

    let store = Arc::new(JsonlStore::new(store_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));

    // Build agent — compaction is wired in via the session service in production.
    // Here we demonstrate the concept and configuration.
    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-5")
        .system_prompt(
            "You are a patient tutor. Build on previous conversation context. \
             Always reference earlier topics when relevant.",
        )
        .max_tokens_per_turn(1024)
        .build(Arc::new(llm), Arc::new(EmptyToolDispatcher), store)
        .await;

    // Simulate a long conversation that would trigger compaction
    let topics = [
        "Explain Rust's ownership model. Keep it concise.",
        "Now explain how lifetimes relate to what you just said about ownership.",
        "How do smart pointers like Box, Rc, and Arc fit into this picture?",
        "Give me a practical example combining ownership, lifetimes, and Arc.",
        "Summarize everything we've discussed about Rust's memory model.",
    ];

    let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(256);

    // Monitor for compaction events
    let monitor = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            if let AgentEvent::CompactionStarted { .. } = &event {
                println!("[COMPACTION] Context compaction triggered!");
            }
            if let AgentEvent::CompactionCompleted { .. } = &event {
                println!("[COMPACTION] Compaction completed — context window refreshed.");
            }
        }
    });

    for (i, topic) in topics.iter().enumerate() {
        println!("\n=== Turn {} ===", i + 1);
        println!("User: {}\n", topic);

        let result = if i == 0 {
            agent.run(topic.to_string()).await?
        } else {
            agent.run(topic.to_string()).await?
        };

        println!("Assistant: {}", &result.text[..result.text.len().min(200)]);
        if result.text.len() > 200 {
            println!("...");
        }
        println!("(tokens so far: {})", result.usage.total_tokens());
    }

    drop(event_tx);
    let _ = monitor.await;

    // ── Compaction configuration reference ─────────────────────────────────

    println!("\n\n=== Compaction Configuration Reference ===\n");
    println!(
        r#"# .rkat/config.toml

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
"#
    );

    Ok(())
}
