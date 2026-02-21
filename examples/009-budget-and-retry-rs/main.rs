//! # 009 — Budget & Retry Policies (Rust)
//!
//! Production agents need guardrails. This example shows how to set token
//! budgets, time limits, and retry policies to prevent runaway costs.
//!
//! ## What you'll learn
//! - Configuring `BudgetLimits` (max tokens, max turns, time limits)
//! - Setting up `RetryPolicy` for transient LLM failures
//! - Handling `AgentError::BudgetExhausted`
//! - Using `BudgetPool` for shared budgets across agents
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=your-key cargo run --example 009_budget_and_retry
//! ```

use std::sync::Arc;
use std::time::Duration;

use meerkat::{
    AgentBuilder, AgentFactory, AnthropicClient, BudgetLimits, RetryPolicy,
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

    // ── Example 1: Token budget ──

    println!("=== Example 1: Token budget ===\n");
    let budget = BudgetLimits::builder()
        .max_total_tokens(2000)     // Hard cap on total tokens
        .max_turns(5)               // Max agent loop iterations
        .max_tool_calls(10)         // Max tool invocations
        .build();

    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-5")
        .system_prompt("You are a concise assistant. Keep responses under 100 words.")
        .max_tokens_per_turn(512)
        .budget_limits(budget)
        .build(
            Arc::new(llm),
            Arc::new(EmptyToolDispatcher),
            store.clone(),
        )
        .await;

    let result = agent
        .run("Explain quantum computing in simple terms.".to_string())
        .await?;

    println!("Response: {}", result.text);
    println!("Tokens used: {}", result.usage.total_tokens());

    // ── Example 2: Retry policy ──

    println!("\n=== Example 2: Retry policy ===\n");

    let retry = RetryPolicy::builder()
        .max_retries(3)                                    // Retry up to 3 times
        .initial_backoff(Duration::from_millis(500))       // Start at 500ms
        .max_backoff(Duration::from_secs(10))              // Cap at 10s
        .backoff_multiplier(2.0)                           // Double each time
        .build();

    println!("Retry policy: {:?}", retry);
    println!("Retries are automatic — transient 429/500 errors trigger backoff.");

    // ── Example 3: Budget exhaustion handling ──

    println!("\n=== Example 3: Tight budget (will be exhausted) ===\n");

    let tight_budget = BudgetLimits::builder()
        .max_total_tokens(100)  // Very tight budget
        .max_turns(1)
        .build();

    let store2_dir = tempfile::tempdir()?.into_path().join("sessions");
    std::fs::create_dir_all(&store2_dir)?;
    let factory2 = AgentFactory::new(store2_dir.clone());
    let client2 = Arc::new(AnthropicClient::new(
        std::env::var("ANTHROPIC_API_KEY")?,
    )?);
    let llm2 = factory2.build_llm_adapter(client2, "claude-sonnet-4-5").await;
    let store2 = Arc::new(JsonlStore::new(store2_dir));
    store2.init().await?;
    let store2 = Arc::new(StoreAdapter::new(store2));

    let mut agent2 = AgentBuilder::new()
        .model("claude-sonnet-4-5")
        .max_tokens_per_turn(50)
        .budget_limits(tight_budget)
        .build(Arc::new(llm2), Arc::new(EmptyToolDispatcher), store2)
        .await;

    match agent2
        .run("Write a 500-word essay about machine learning.".to_string())
        .await
    {
        Ok(result) => {
            println!("Response (truncated): {}...", &result.text[..result.text.len().min(100)]);
            println!("Stop reason: {:?}", result.stop_reason);
        }
        Err(e) => {
            println!("Budget exhausted (expected): {}", e);
            println!("This is how you catch runaway agents in production.");
        }
    }

    Ok(())
}
