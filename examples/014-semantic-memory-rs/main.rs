//! # 014 — Semantic Memory (Rust)
//!
//! Semantic memory lets agents store and retrieve information across sessions.
//! Unlike session history (which is per-conversation), memory persists and
//! is searchable via similarity matching.
//!
//! ## What you'll learn
//! - Indexing text into the memory store
//! - Searching memory by semantic similarity
//! - How memory integrates with the agent loop
//! - Choosing between `HnswMemoryStore` and `SimpleMemoryStore`
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=your-key cargo run --example 014_semantic_memory
//! ```

use std::sync::Arc;

use meerkat::{
    AgentBuilder, AgentFactory, AnthropicClient,
};
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::EmptyToolDispatcher;

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

    // Build agent with memory-aware system prompt.
    // In production, the MemoryStore is wired into the agent via AgentFactory.build_agent().
    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-5")
        .system_prompt(
            "You are an assistant with long-term memory. When the user teaches you \
             something, remember it. When they ask about past conversations, recall \
             from your memory.",
        )
        .max_tokens_per_turn(1024)
        .build(Arc::new(llm), Arc::new(EmptyToolDispatcher), store)
        .await;

    // ── Simulate memory-augmented conversations ────────────────────────────

    println!("=== Session 1: Teaching the agent ===\n");
    let result = agent
        .run(
            "Remember this: Our team uses Rust for all backend services, \
             Python for data pipelines, and TypeScript for the frontend. \
             Our deployment target is Kubernetes on AWS EKS."
                .to_string(),
        )
        .await?;
    println!("Agent: {}\n", result.text);

    println!("=== Session 1, Turn 2: Teaching more ===\n");
    let result = agent
        .run(
            "Also remember: Our CI uses GitHub Actions. We deploy on Tuesdays \
             and Thursdays. The staging environment is at staging.example.com."
                .to_string(),
        )
        .await?;
    println!("Agent: {}\n", result.text);

    println!("=== Session 1, Turn 3: Recalling ===\n");
    let result = agent
        .run("What languages does our team use and where do we deploy?".to_string())
        .await?;
    println!("Agent: {}\n", result.text);

    // ── Memory configuration reference ─────────────────────────────────────

    println!("=== Semantic Memory Configuration Reference ===\n");
    println!(
        r#"# Enable memory in .rkat/config.toml:

[tools]
memory = true  # Gives the agent memory_store and memory_search tools

# Memory architecture:
#
# ┌──────────────┐     ┌─────────────────┐     ┌──────────────┐
# │  Agent Loop   │────→│  MemoryStore    │────→│  HNSW Index  │
# │              │     │  trait           │     │  (redb)      │
# │ memory_store │     │                 │     │              │
# │ memory_search│     │ - index_text()  │     │ Similarity   │
# └──────────────┘     │ - search()      │     │ search via   │
#                      └─────────────────┘     │ embeddings   │
#                                              └──────────────┘
#
# Two implementations:
#   - HnswMemoryStore: Production-grade, persistent (hnsw_rs + redb)
#   - SimpleMemoryStore: In-memory, for tests and development
#
# Memory is indexed per-session and searchable across sessions.
# The agent can explicitly store and retrieve information using tools.
#
# CLI usage:
#   rkat run --memory "Remember: the API key rotates monthly"
#   rkat session <id> "What did I tell you about the API key?"
"#
    );

    Ok(())
}
