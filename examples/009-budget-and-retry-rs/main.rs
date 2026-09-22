//! # 009 — Budget & Retry Policies (Rust)
//!
//! Production agents need guardrails. This example shows how to set token
//! budgets, time limits, and retry policies to prevent runaway costs.
//!
//! ## What you'll learn
//! - Configuring `BudgetLimits` (max tokens, max tool calls, time limits)
//! - Setting up `RetryPolicy` for transient LLM failures
//! - Applying retry policy to an agent build
//! - Handling budget exhaustion returned by an agent run
//!
//! ## Run
//! ```bash
//! # From the repository root
//! ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
//!   --example 009-budget-and-retry --features jsonl-store
//! ```

use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentBuilder, AgentFactory, AnthropicClient, BudgetLimits, RetryPolicy};
use meerkat_core::{AgentError, RunResult, TurnTerminalCauseKind, TurnTerminalOutcome};
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::EmptyToolDispatcher;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    meerkat_runtime::host_stack::run_host("budget-and-retry", run)?
}

fn scoped_store_dir(
    root: &std::path::Path,
) -> std::io::Result<(tempfile::TempDir, std::path::PathBuf)> {
    let guard = tempfile::tempdir_in(root)?;
    let path = guard.path().join("sessions");
    std::fs::create_dir_all(&path)?;
    Ok((guard, path))
}

/// At most `max_chars` Unicode scalar values, plus an ellipsis only if cut.
fn preview(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

fn describe_run(result: Result<RunResult, AgentError>) -> Result<String, AgentError> {
    match result {
        Ok(result) => {
            let status = match result.terminal_cause_kind {
                Some(TurnTerminalCauseKind::BudgetExhausted) => "Budget exhausted",
                _ => "Run completed without budget exhaustion",
            };
            Ok(format!(
                "{status}\nResponse: {}\nTurns: {}",
                preview(&result.text, 100),
                result.turns
            ))
        }
        Err(AgentError::TimeBudgetExceeded {
            elapsed_secs,
            limit_secs,
        }) => Ok(format!(
            "Time budget exhausted: {elapsed_secs}s > {limit_secs}s"
        )),
        Err(AgentError::TerminalFailure {
            outcome: TurnTerminalOutcome::TimeBudgetExceeded,
            cause_kind: TurnTerminalCauseKind::TimeBudgetExceeded,
            message,
        }) => Ok(format!("Time budget exhausted: {message}")),
        Err(error) => Err(error),
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    let (_tmp, store_dir) = scoped_store_dir(&std::env::current_dir()?)?;

    let factory = AgentFactory::new(store_dir.clone());
    let client = Arc::new(AnthropicClient::new(api_key)?);
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4-6").await;

    let store = Arc::new(JsonlStore::new(store_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));

    // ── Example 1: Token budget ──

    println!("=== Example 1: Token budget ===\n");
    let budget = BudgetLimits::unlimited()
        .with_max_tokens(2000) // Measured exhaustion threshold; a call can overshoot
        .with_max_tool_calls(10) // Max tool invocations
        .with_max_duration(Duration::from_secs(60)); // Agent-lifetime wall-clock budget

    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-6")
        .system_prompt("You are a concise assistant. Keep responses under 100 words.")
        .max_tokens_per_turn(512)
        .budget(budget)
        .build(Arc::new(llm), Arc::new(EmptyToolDispatcher), store.clone())
        .await?;

    let result = agent
        .run("Explain quantum computing in simple terms.".into())
        .await?;

    println!("Response: {}", result.text);
    println!("Tokens used: {}", result.usage.total_tokens());

    // ── Example 2: Retry policy ──

    println!("\n=== Example 2: Retry policy ===\n");

    let retry = RetryPolicy::new()
        .with_max_retries(3) // Retry up to 3 times
        .with_initial_delay(Duration::from_millis(500)) // Start at 500ms
        .with_max_delay(Duration::from_secs(10)) // Cap at 10s
        .with_multiplier(2.0); // Double each time

    println!("Retry policy: {retry:?}");
    println!("Typed retryable provider errors use this backoff policy.");

    // ── Example 3: Budget exhaustion handling ──

    println!("\n=== Example 3: Tight budget (may be exhausted) ===\n");

    let tight_budget = BudgetLimits::unlimited().with_max_tokens(100); // Very tight budget

    let (_tmp2, store2_dir) = scoped_store_dir(&std::env::current_dir()?)?;
    let factory2 = AgentFactory::new(store2_dir.clone());
    let client2 = Arc::new(AnthropicClient::new(std::env::var("ANTHROPIC_API_KEY")?)?);
    let llm2 = factory2
        .build_llm_adapter(client2, "claude-sonnet-4-6")
        .await;
    let store2 = Arc::new(JsonlStore::new(store2_dir));
    store2.init().await?;
    let store2 = Arc::new(StoreAdapter::new(store2));

    let mut agent2 = AgentBuilder::new()
        .model("claude-sonnet-4-6")
        .max_tokens_per_turn(50)
        .budget(tight_budget)
        .retry_policy(retry)
        .build(Arc::new(llm2), Arc::new(EmptyToolDispatcher), store2)
        .await?;

    let result = agent2
        .run("Write a 500-word essay about machine learning.".into())
        .await;
    println!("{}", describe_run(result)?);

    Ok(())
}

#[cfg(test)]
mod tests;
