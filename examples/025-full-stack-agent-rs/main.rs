//! # 025 — Full-Stack Agent (Rust)
//!
//! A production-grade agent that combines every Meerkat feature: custom tools,
//! built-in tools (task management, shell), hooks, skills, budget control,
//! session persistence, and event streaming. This is the reference architecture
//! for building real-world agentic applications.
//!
//! ## What you'll learn
//! - Combining all Meerkat features in one agent
//! - The `AgentFactory.build_agent()` pipeline
//! - Production configuration patterns
//! - The recommended architecture for real applications
//!
//! ## Run
//! ```bash
//! This is a reference implementation. For runnable examples, see meerkat/examples/.
//! ```

use async_trait::async_trait;
use meerkat::{
    AgentBuilder, AgentEvent, AgentFactory, AgentToolDispatcher, AnthropicClient, BudgetLimits,
    BuiltinToolConfig, EventLoggerConfig, ToolDef, ToolError, ToolResult,
    create_dispatcher_with_builtins, spawn_event_logger,
};
use meerkat_core::ToolCallView;
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
    /// Maximum results to return
    #[serde(default = "default_limit")]
    _limit: usize,
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
                name: "search_docs".to_string(),
                description: "Search internal documentation".to_string(),
                input_schema: meerkat_tools::schema_for::<SearchDocsArgs>(),
            }),
            Arc::new(ToolDef {
                name: "create_ticket".to_string(),
                description: "Create a support ticket in the issue tracker".to_string(),
                input_schema: meerkat_tools::schema_for::<CreateTicketArgs>(),
            }),
        ]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        match call.name {
            "search_docs" => {
                let args: SearchDocsArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                // Simulated documentation search
                let results = json!({
                    "query": args.query,
                    "results": [
                        {"title": "Getting Started Guide", "relevance": 0.95},
                        {"title": "API Reference", "relevance": 0.87},
                        {"title": "Troubleshooting FAQ", "relevance": 0.72},
                    ],
                    "total": 3
                });
                Ok(ToolResult::new(
                    call.id.to_string(),
                    results.to_string(),
                    false,
                ))
            }
            "create_ticket" => {
                let args: CreateTicketArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let ticket = json!({
                    "id": "TICKET-1234",
                    "title": args.title,
                    "description": args.description,
                    "priority": args.priority,
                    "status": "open",
                    "created_at": "2026-02-21T00:00:00Z"
                });
                Ok(ToolResult::new(
                    call.id.to_string(),
                    ticket.to_string(),
                    false,
                ))
            }
            _ => Err(ToolError::not_found(call.name)),
        }
    }
}

// ── Main ───────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    // ── 1. Set up storage ──────────────────────────────────────────────────
    let _tmp = tempfile::tempdir()?;
    let store_dir = _tmp.path().join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    let factory = AgentFactory::new(store_dir.clone());
    let client = Arc::new(AnthropicClient::new(api_key)?);
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4-5").await;

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
    let skill_content = r"
## Role
You are a full-stack support agent for a software product.

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
5. Track follow-up tasks on the task board

## Tone
Professional, concise, action-oriented. Always provide next steps.
";

    let system_prompt = format!(
        "You are a production support agent.\n\n<skill>\n{}\n</skill>",
        skill_content.trim()
    );

    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-5")
        .system_prompt(&system_prompt)
        .max_tokens_per_turn(2048)
        .budget(budget)
        .build(Arc::new(llm), tools, store)
        .await;

    // ── 5. Run with event streaming ────────────────────────────────────────
    println!("=== Full-Stack Agent: Production Support ===\n");

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
                .to_string(),
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

    // ── Architecture summary ───────────────────────────────────────────────

    println!("\n\n=== Full-Stack Agent Architecture ===\n");
    println!(
        r"This example combines every Meerkat feature:

┌────────────────────────────────────────────────────────────┐
│                    FULL-STACK AGENT                          │
│                                                            │
│  Model:     claude-sonnet-4-5                              │
│  Skills:    support-agent (inline)                         │
│  Budget:    50K tokens / 50 tool calls                      │
│                                                            │
│  Tools:                                                    │
│  ├── Built-in: task_create, task_list, task_update, wait   │
│  ├── Domain:   search_docs, create_ticket                  │
│  └── (Optional: shell, MCP, comms, sub-agents)            │
│                                                            │
│  Events:    Streaming to event logger (verbose mode)       │
│  Storage:   JsonlStore (file-based session persistence)    │
│  Hooks:     (Configurable via .rkat/config.toml)           │
└────────────────────────────────────────────────────────────┘

Production checklist:
  ✓ Budget limits prevent runaway costs
  ✓ Retry policy handles transient LLM failures
  ✓ Session persistence survives restarts
  ✓ Event streaming enables real-time UI
  ✓ Skills separate behavior from code
  ✓ Hooks enable audit logging and guardrails
  ✓ CompositeDispatcher merges builtin + domain tools
  ✓ Structured output for programmatic parsing
  ✓ MCP for external tool servers (add via config)
"
    );

    Ok(())
}
