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
//! ANTHROPIC_API_KEY=... ./scripts/repo-cargo run -p meerkat --example 014-semantic-memory --features jsonl-store,memory-store-session
//! ```

use std::sync::Arc;

use meerkat::{AgentBuilder, AgentFactory, AnthropicClient, ToolGatewayBuilder};
use meerkat_core::memory::{
    MemoryIndexRequest, MemoryIndexScope, MemoryMetadata, MemoryStore as _,
};
use meerkat_memory::{MemorySearchDispatcher, SimpleMemoryStore};
use meerkat_store::{JsonlStore, StoreAdapter};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    let _tmp = tempfile::tempdir()?;
    let store_dir = _tmp.path().join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    let factory = AgentFactory::new(store_dir.clone());
    let client = Arc::new(AnthropicClient::new(api_key)?);
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4-6").await;

    let store = Arc::new(JsonlStore::new(store_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));

    // ── Step 1: Create the memory store ──────────────────────────────────────
    //
    // SimpleMemoryStore is an in-memory implementation that uses keyword matching.
    // For production, use HnswMemoryStore which provides true vector-embedding
    // similarity search backed by hnsw_rs + SQLite persistence.

    let memory_store = Arc::new(SimpleMemoryStore::new());
    let mut memory_session = meerkat_core::Session::new();
    let memory_session_id = memory_session.id().clone();
    memory_session.set_session_metadata(meerkat_core::SessionMetadata {
        schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
        model: "claude-sonnet-4-6".to_string(),
        max_tokens: 1024,
        structured_output_retries: 2,
        provider: meerkat_core::Provider::Anthropic,
        self_hosted_server_id: None,
        provider_params: None,
        tooling: meerkat_core::SessionTooling::default(),
        keep_alive: false,
        comms_name: None,
        peer_meta: None,
        realm_id: None,
        instance_id: None,
        backend: None,
        config_generation: None,
        auth_binding: None,
    })?;
    memory_session.set_build_state(meerkat_core::SessionBuildState::default())?;

    // ── Step 2: Pre-seed memory with facts ───────────────────────────────────
    //
    // In normal operation, memory is populated during context compaction:
    // when the agent loop compacts old messages to save context space,
    // the discarded text is indexed into the memory store. Here we simulate
    // that by scoped indexing of some facts.

    let now = std::time::SystemTime::now();
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
            session_id: memory_session_id.clone(),
            turn: Some(i as u32 + 1),
            indexed_at: now,
        };
        let request = MemoryIndexRequest::new(
            MemoryIndexScope::for_session(memory_session_id.clone()),
            (*fact).to_string(),
            metadata,
        )?;
        memory_store.index_scoped(request).await?;
    }

    println!("Indexed {} facts into SimpleMemoryStore\n", facts.len());

    // ── Step 3: Create the memory search tool dispatcher ─────────────────────
    //
    // MemorySearchDispatcher wraps a MemoryStore and exposes it as the
    // `memory_search` tool. The agent can call this tool with a natural
    // language query and get back scored results.

    let memory_dispatcher = MemorySearchDispatcher::for_session(
        Arc::clone(&memory_store) as Arc<dyn meerkat_core::memory::MemoryStore>,
        memory_session_id,
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

    // ── Step 5: Build the agent with memory search wired in ─────────────────
    //
    // The ToolGateway contains MemorySearchDispatcher, so the agent can call
    // `memory_search` to retrieve indexed content while construction still
    // runs through the facade factory pipeline.

    let mut agent = AgentBuilder::new()
        .with_factory(factory)
        .model("claude-sonnet-4-6")
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
        .resume_session(memory_session)
        .build(Arc::new(llm), Arc::new(gateway), store)
        .await?;

    // ── Step 6: Ask the agent to recall from memory ──────────────────────────

    println!("=== Asking the agent to recall from semantic memory ===\n");
    let result = agent
        .run(
            "What programming languages does our team use? \
             And what database do we run? Search your memory to find out."
                .into(),
        )
        .await?;
    println!("Agent: {}\n", result.text);

    println!("=== Asking about deployment schedule ===\n");
    let result = agent
        .run("When do we deploy and where? Check your memory.".into())
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
  - HnswMemoryStore:   Persistent, vector embeddings (hnsw_rs + SQLite)

Wiring in the factory (AgentFactory::build_agent):
  1. Creates HnswMemoryStore from .rkat/memory/ directory
  2. Passes it into the core agent loop for compaction indexing
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
