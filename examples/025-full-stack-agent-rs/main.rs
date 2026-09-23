//! # 025 - Composed Agent (Rust)
//!
//! A focused standalone example that composes built-in tools, domain tools,
//! budget limits, inline behavior instructions, a JSONL store, and event
//! streaming. It is not an exhaustive production architecture.
//!
//! ## What you'll learn
//! - Combining built-in and domain-specific tools
//! - The `AgentFactory.build_agent()` pipeline
//! - Applying token and tool-call budgets
//! - Streaming events while writing session state to JSONL
//!
//! ## Run
//! ```bash
//! # From the repository root
//! ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
//!   --example 025-full-stack-agent --features jsonl-store
//! ```

use async_trait::async_trait;
use meerkat::{
    AgentBuilder, AgentEvent, AgentFactory, AgentToolDispatcher, AnthropicClient, BudgetLimits,
    BuiltinToolConfig, EventLoggerConfig, ToolDef, ToolError, ToolResult,
    create_dispatcher_with_builtins, spawn_event_logger,
};
use meerkat_core::ToolCallView;
use meerkat_core::ToolDispatchOutcome;
use meerkat_store::{JsonlStore, StoreAdapter};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;

// ── Domain-specific tools ──────────────────────────────────────────────────

#[derive(Debug, Clone, JsonSchema, Deserialize)]
struct SearchDocsArgs {
    /// Search query
    query: String,
    /// Maximum fixture results to return (0 returns none; total counts all matches)
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    5
}

#[derive(Debug, Clone, JsonSchema, Deserialize)]
struct CreateTicketArgs {
    /// Ticket title
    title: String,
    /// Ticket description
    description: String,
    /// Priority: low, medium, high, critical
    priority: String,
}

struct DomainTools;

#[async_trait]
impl AgentToolDispatcher for DomainTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![
            Arc::new(ToolDef {
                name: "search_docs".into(),
                description: "Search three offline documentation fixtures, not real internal documentation. Results are simulated.".to_string(),
                input_schema: meerkat_tools::schema_for::<SearchDocsArgs>(),
                provenance: None,
            }),
            Arc::new(ToolDef {
                name: "create_ticket".into(),
                description: "Simulate a support ticket using an offline fixture. Does not create anything in an issue tracker.".to_string(),
                input_schema: meerkat_tools::schema_for::<CreateTicketArgs>(),
                provenance: None,
            }),
        ]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        match call.name {
            "search_docs" => {
                let args: SearchDocsArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let fixtures = [
                    json!({"title": "Getting Started Guide", "relevance": 0.95}),
                    json!({"title": "API Reference", "relevance": 0.87}),
                    json!({"title": "Troubleshooting FAQ", "relevance": 0.72}),
                ];
                let results = json!({
                    "simulated": true,
                    "source": "offline fixtures; no documentation service queried",
                    "query": args.query,
                    "results": &fixtures[..args.limit.min(fixtures.len())],
                    "total": fixtures.len()
                });
                Ok(ToolResult::new(call.id.to_string(), results.to_string(), false).into())
            }
            "create_ticket" => {
                let args: CreateTicketArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let ticket = json!({
                    "simulated": true,
                    "external_mutation": false,
                    "notice": "Offline ticket fixture only; no issue tracker ticket was created",
                    "id": "TICKET-1234",
                    "title": args.title,
                    "description": args.description,
                    "priority": args.priority,
                    "status": "open",
                    "created_at": "2026-02-21T00:00:00Z"
                });
                Ok(ToolResult::new(call.id.to_string(), ticket.to_string(), false).into())
            }
            _ => Err(ToolError::not_found(call.name)),
        }
    }
}

// ── Main ───────────────────────────────────────────────────────────────────

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    meerkat_runtime::host_stack::run_host("025-full-stack-agent", async_main)?
}

async fn async_main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    // ── 1. Set up storage ──────────────────────────────────────────────────
    let _tmp = tempfile::tempdir()?;
    let store_dir = _tmp.path().join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    let factory = AgentFactory::new(store_dir.clone());
    let client = Arc::new(AnthropicClient::new(api_key)?);
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4-6").await;

    let store = Arc::new(JsonlStore::new(store_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));

    // ── 2. Build tool dispatcher (builtins + domain tools) ─────────────────
    let builtin_config = BuiltinToolConfig::default();
    let domain_tools: Arc<dyn AgentToolDispatcher> = Arc::new(DomainTools);

    // Compose: builtins + domain tools via the external dispatcher slot
    let tools =
        create_dispatcher_with_builtins(&factory, builtin_config, None, Some(domain_tools), None)
            .await?;

    // ── 3. Configure budget ────────────────────────────────────────────────
    let budget = BudgetLimits::unlimited()
        .with_max_tokens(50_000)
        .with_max_tool_calls(50);

    // ── 4. Build the agent ─────────────────────────────────────────────────
    let behavior_instructions = r"
## Role
You are demonstrating a software support agent with offline domain-tool fixtures.
search_docs returns canned documentation entries, not real search results.
create_ticket only simulates a ticket; it never writes to an issue tracker.
Explicitly label these results as simulated. Never claim a real ticket was created.

## Capabilities
1. Search documentation to answer user questions
2. Create support tickets for unresolved issues
3. Manage tasks for follow-up work
4. Use shell tools for system diagnostics (if enabled)

## Workflow
1. Understand the user's question
2. Search docs first — answer from documentation when possible
3. If docs don't help, investigate further
4. Create a ticket if the issue needs engineering attention
5. Track follow-up tasks with the available task tools

## Tone
Professional, concise, action-oriented. Always provide next steps.
";

    let system_prompt = format!(
        "You are a production support agent.\n\n<behavior_instructions>\n{}\n</behavior_instructions>",
        behavior_instructions.trim()
    );

    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-6")
        .system_prompt(&system_prompt)
        .max_tokens_per_turn(2048)
        .budget(budget)
        .build(Arc::new(llm), tools, store)
        .await?;

    // ── 5. Run with event streaming ────────────────────────────────────────
    println!("=== Composed Agent: Product Support ===\n");
    println!("DEMO: documentation search and ticket creation use offline fixtures.");
    println!("No documentation service is queried and no issue tracker ticket is created.\n");

    let (event_tx, event_rx) = mpsc::channel::<AgentEvent>(256);
    let logger = spawn_event_logger(
        event_rx,
        EventLoggerConfig {
            verbose: true,
            stream: false,
        },
    );

    let result = agent
        .run_with_events(
            "A customer reports that the API is returning 500 errors on the /users endpoint. \
             Search our docs for troubleshooting steps, and if you can't find a solution, \
             create a high-priority ticket for the engineering team."
                .into(),
            event_tx,
        )
        .await?;

    logger.await?;

    println!("\n=== Final Response ===\n");
    println!("{}", result.text);
    println!("\n--- Stats ---");
    println!("Session:    {}", result.session_id);
    println!("Turns:      {}", result.turns);
    println!("Tool calls: {}", result.tool_calls);
    println!("Tokens:     {}", result.usage.total_tokens());

    // Architecture summary

    println!("\n\n=== Composed Agent Architecture ===\n");
    println!(
        r"This example composes a focused set of Meerkat features:

┌────────────────────────────────────────────────────────────┐
│                    COMPOSED AGENT                            │
│                                                            │
│  Model:     claude-sonnet-4-6                              │
│  Behavior:  inline system-prompt instructions              │
│  Budget:    50K tokens / 50 tool calls                      │
│                                                            │
│  Tools:                                                    │
│  ├── Built-in: task_create, task_list, task_update, datetime │
│  ├── Fixtures: search_docs, create_ticket (simulated)       │
│                                                            │
│  Events:    Streaming to event logger (verbose mode)       │
│  Storage:   JsonlStore in a temporary directory             │
└────────────────────────────────────────────────────────────┘

Configured in this program:
  - Budget limits for total tokens and tool calls
  - Event streaming through the built-in logger
  - CompositeDispatcher with built-in and domain tools
  - JSONL session writes for the duration of the temporary directory
  - Inline behavior instructions in the system prompt

Not configured here:
  - Canonical skill resolution, hooks, or structured output
  - Shell, MCP, comms, or delegation tools
  - Runtime-backed recovery after process exit
"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn dispatch(name: &str, args: serde_json::Value) -> serde_json::Value {
        let arguments = serde_json::value::to_raw_value(&args).unwrap();
        let outcome = DomainTools
            .dispatch(ToolCallView {
                id: "fixture-call",
                name,
                args: &arguments,
            })
            .await
            .unwrap();
        let result = outcome.result;
        assert!(!result.is_error);
        serde_json::from_str(&result.text_content()).unwrap()
    }

    #[tokio::test]
    async fn search_limits_bound_results_without_changing_total_matches() {
        for (limit, expected) in [(None, 3), (Some(0), 0), (Some(1), 1), (Some(10), 3)] {
            let mut args = json!({"query": "synthetic query"});
            if let Some(limit) = limit {
                args["limit"] = json!(limit);
            }
            let output = dispatch("search_docs", args).await;
            assert_eq!(output["results"].as_array().unwrap().len(), expected);
            assert_eq!(output["total"], 3);
            assert_eq!(output["simulated"], true);
        }
    }

    #[test]
    fn tool_catalog_discloses_fixtures_and_exposes_limit() {
        let tools = DomainTools.tools();
        let search = tools
            .iter()
            .find(|tool| tool.name == "search_docs")
            .unwrap();
        assert!(search.input_schema["properties"].get("limit").is_some());
        assert!(search.input_schema["properties"].get("_limit").is_none());
        assert!(
            tools
                .iter()
                .all(|tool| tool.description.contains("offline"))
        );
    }

    #[tokio::test]
    async fn ticket_dispatch_returns_an_explicit_simulation_not_an_external_write() {
        let output = dispatch(
            "create_ticket",
            json!({"title": "Synthetic ticket", "description": "Fixture only", "priority": "high"}),
        )
        .await;
        assert_eq!(output["simulated"], true);
        assert_eq!(output["external_mutation"], false);
        assert_eq!(output["title"], "Synthetic ticket");
        assert!(
            output["notice"]
                .as_str()
                .unwrap()
                .contains("no issue tracker")
        );
    }
}
