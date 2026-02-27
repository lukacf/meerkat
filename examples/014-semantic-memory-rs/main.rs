//! # 014 -- Semantic Memory (Rust)
//!
//! Semantic memory lets agents store and retrieve information across sessions.
//! Unlike session history (which is per-conversation), memory persists and
//! is searchable via similarity matching.
//!
//! ## What this example actually does
//! - Creates a `SimpleMemoryStore` (in-memory keyword-matching implementation)
//! - Wraps it in a `MemorySearchDispatcher` to expose the `memory_search` tool
//! - Composes the memory tool into the agent's tool dispatcher via `ToolGatewayBuilder`
//! - Wires the memory store into the `AgentBuilder` so compaction can index into it
//! - Pre-seeds the memory store with facts (simulating prior compaction indexing)
//! - Asks the agent to recall those facts using the `memory_search` tool
//!
//! ## What you'll learn
//! - Wiring a `MemoryStore` into an agent
//! - How `MemorySearchDispatcher` exposes `memory_search` as a tool
//! - How `ToolGatewayBuilder` composes multiple dispatchers
//! - The difference between `SimpleMemoryStore` (test) and `HnswMemoryStore` (production)
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=... cargo run --example 014-semantic-memory --features jsonl-store,memory-store-session
//! ```

use std::sync::Arc;

use meerkat::{
    AgentBuilder, AgentFactory, AnthropicClient, ToolGatewayBuilder,
};
use meerkat_core::memory::{MemoryMetadata, MemoryStore as _};
use meerkat_core::types::SessionId;
use meerkat_memory::{MemorySearchDispatcher, SimpleMemoryStore};
use meerkat_store::{JsonlStore, StoreAdapter};

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

    // ── Step 1: Create the memory store ──────────────────────────────────────
    //
    // SimpleMemoryStore is an in-memory implementation that uses keyword matching.
    // For production, use HnswMemoryStore which provides true vector-embedding
    // similarity search backed by hnsw_rs + redb persistence.

    let memory_store = Arc::new(SimpleMemoryStore::new());

    // ── Step 2: Pre-seed memory with facts ───────────────────────────────────
    //
    // In normal operation, memory is populated during context compaction:
    // when the agent loop compacts old messages to save context space,
    // the discarded text is indexed into the memory store. Here we simulate
    // that by directly indexing some facts.

    let now = std::time::SystemTime::now();
    let prior_session = SessionId::new();

    let facts = [
        "Our team uses Rust for all backend services, Python for data pipelines, \
         and TypeScript for the frontend.",
        "Deployment target is Kubernetes on AWS EKS. We deploy on Tuesdays and Thursdays.",
        "CI uses GitHub Actions. The staging environment is at staging.example.com.",
        "The database is PostgreSQL 16 with pgvector for embeddings.",
        "Team lead is Alice. Backend lead is Bob. Frontend lead is Carol.",
    ];

    for (i, fact) in facts.iter().enumerate() {
        let metadata = MemoryMetadata {
            session_id: prior_session.clone(),
            turn: Some(i as u32 + 1),
            indexed_at: now,
        };
        memory_store.index(fact, metadata).await?;
    }

    println!("Indexed {} facts into SimpleMemoryStore\n", facts.len());

    // ── Step 3: Create the memory search tool dispatcher ─────────────────────
    //
    // MemorySearchDispatcher wraps a MemoryStore and exposes it as the
    // `memory_search` tool. The agent can call this tool with a natural
    // language query and get back scored results.

    let memory_dispatcher = MemorySearchDispatcher::new(
        Arc::clone(&memory_store) as Arc<dyn meerkat_core::memory::MemoryStore>,
    );

    // ── Step 4: Compose tool dispatchers ─────────────────────────────────────
    //
    // ToolGatewayBuilder merges multiple dispatchers into one. Here we combine
    // an empty base dispatcher with the memory search dispatcher. In a real
    // agent, the base would be a CompositeDispatcher with shell tools, tasks, etc.

    let base_tools: Arc<dyn meerkat_core::AgentToolDispatcher> =
        Arc::new(meerkat_tools::EmptyToolDispatcher);

    let gateway = ToolGatewayBuilder::new()
        .add_dispatcher(base_tools)
        .add_dispatcher(Arc::new(memory_dispatcher))
        .build()?;

    // ── Step 5: Build the agent with memory wired in ─────────────────────────
    //
    // Two things are wired:
    //   1. `.memory_store()` on AgentBuilder -- so the agent loop can index
    //      discarded messages during compaction
    //   2. The ToolGateway containing MemorySearchDispatcher -- so the agent
    //      can call `memory_search` to retrieve indexed content

    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-5")
        .system_prompt(
            "You are a helpful assistant with access to a semantic memory store.\n\n\
             You have a `memory_search` tool that searches your long-term memory.\n\
             Your memory contains facts from earlier conversations that were \
             compacted away. When the user asks about things you don't see in \
             the current conversation, use `memory_search` to look them up.\n\n\
             Always use the memory_search tool when asked about team details, \
             infrastructure, or deployment information.",
        )
        .max_tokens_per_turn(1024)
        .memory_store(Arc::clone(&memory_store) as Arc<dyn meerkat_core::memory::MemoryStore>)
        .build(Arc::new(llm), Arc::new(gateway), store)
        .await;

    // ── Step 6: Ask the agent to recall from memory ──────────────────────────

    println!("=== Asking the agent to recall from semantic memory ===\n");
    let result = agent
        .run(
            "What programming languages does our team use? \
             And what database do we run? Search your memory to find out."
                .to_string(),
        )
        .await?;
    println!("Agent: {}\n", result.text);

    println!("=== Asking about deployment schedule ===\n");
    let result = agent
        .run(
            "When do we deploy and where? Check your memory.".to_string(),
        )
        .await?;
    println!("Agent: {}\n", result.text);

    // ── Architecture reference ───────────────────────────────────────────────

    println!("=== Semantic Memory Architecture ===\n");
    println!(
        r#"Memory data flow:

  Agent Loop
    |
    |-- (compaction) --> MemoryStore::index()  --> stored entries
    |
    |-- (tool call)  --> memory_search tool
                           |
                           v
                         MemoryStore::search()  --> scored results

Two implementations:
  - SimpleMemoryStore: In-memory, keyword matching (this example)
  - HnswMemoryStore:   Persistent, vector embeddings (hnsw_rs + redb)

Wiring in the factory (AgentFactory::build_agent):
  1. Creates HnswMemoryStore from .rkat/memory/ directory
  2. Passes it to AgentBuilder::memory_store() for compaction indexing
  3. Wraps it in MemorySearchDispatcher for the memory_search tool
  4. Composes into ToolGateway alongside other dispatchers

Enable via config:
  [tools]
  memory = true  # Gives the agent memory_search tool + compaction indexing

CLI usage:
  rkat run --memory "What did I tell you about the API key?"
"#
    );

    Ok(())
}
