//! # 016 — Sub-Agent Orchestration (Rust)
//!
//! A parent agent can spawn child agents to handle subtasks. Sub-agents
//! run independently with their own tools, budgets, and models — then
//! report results back to the parent.
//!
//! ## What you'll learn
//! - How sub-agents are spawned via `SpawnSpec`
//! - Sub-agent budget isolation (children don't drain the parent)
//! - Fork vs Spawn: when to use each pattern
//! - The `SubAgentManager` for lifecycle control
//!
//! ## Run
//! ```bash
//! This is a reference implementation. For runnable examples, see meerkat/examples/.
//! ```

use std::sync::Arc;

use meerkat::{
    AgentBuilder, AgentFactory, AnthropicClient,
    create_dispatcher_with_builtins, BuiltinToolConfig,
};
use meerkat_store::{JsonlStore, StoreAdapter};

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

    // Enable built-in tools which include sub-agent spawning
    let config = BuiltinToolConfig {
        enable_sub_agents: true,
        ..Default::default()
    };

    let tools = create_dispatcher_with_builtins(&factory, config, None, None, None).await?;

    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-5")
        .system_prompt(
            "You are an orchestrator agent. You can spawn sub-agents for specialized tasks. \
             Use spawn_agent to delegate work to specialized child agents. \
             Each sub-agent runs independently with its own context and budget.\n\n\
             Available sub-agent patterns:\n\
             1. **Spawn**: Create an independent child agent with a specific task\n\
             2. **Fork**: Clone your current context into a child (shared history)\n\n\
             When you receive results from sub-agents, synthesize them into a final answer.",
        )
        .max_tokens_per_turn(2048)
        .build(Arc::new(llm), tools, store)
        .await;

    println!("=== Sub-Agent Orchestration ===\n");
    println!("The parent agent will delegate subtasks to specialized child agents.\n");

    let result = agent
        .run(
            "I need a comprehensive analysis of the Rust programming language. \
             Please research these three aspects in parallel using sub-agents:\n\
             1. Performance characteristics compared to C/C++\n\
             2. Memory safety guarantees and how they work\n\
             3. Ecosystem maturity (libraries, tooling, community)\n\n\
             Synthesize the results into a final recommendation."
                .to_string(),
        )
        .await?;

    println!("Final response:\n{}", result.text);
    println!("\n--- Stats ---");
    println!("Turns:      {}", result.turns);
    println!("Tool calls: {}", result.tool_calls);
    println!("Tokens:     {}", result.usage.total_tokens());

    // ── Sub-agent architecture reference ───────────────────────────────────

    println!("\n\n=== Sub-Agent Architecture ===\n");
    println!(
        r#"Sub-agents in Meerkat:

┌──────────────────────────────┐
│        Parent Agent          │
│  (orchestrator model)        │
│                              │
│  spawn_agent("research X")  │───→ ┌─────────────┐
│  spawn_agent("research Y")  │───→ │ Child Agent  │ (independent budget)
│  spawn_agent("research Z")  │───→ │ Child Agent  │ (independent model)
│                              │     │ Child Agent  │ (independent tools)
│  ← results aggregated ←     │←─── └─────────────┘
│                              │
│  Final synthesis             │
└──────────────────────────────┘

Key concepts:
- Sub-agents run in isolated contexts (no shared state)
- Each sub-agent has its own budget (doesn't drain parent)
- Sub-agents can use different models (cheap model for simple tasks)
- Results flow back to the parent as tool results
- The parent orchestrates and synthesizes

Enable in config:
  [tools]
  sub_agents = true

Or in Cargo.toml:
  meerkat = {{ features = ["sub-agents"] }}
"#
    );

    Ok(())
}
