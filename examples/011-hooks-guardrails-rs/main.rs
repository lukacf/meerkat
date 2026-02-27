//! # 011 — Hooks & Guardrails (Rust)
//!
//! Hooks let you intercept and modify the agent loop at 7 defined points.
//! Use them for content filtering, audit logging, cost tracking, approval
//! gates, and prompt rewriting — without touching agent code.
//!
//! ## What you'll learn
//! - Defining hooks in configuration (command, HTTP, in-process)
//! - Hook points: pre_llm_call, post_llm_response, pre_tool_dispatch, etc.
//! - Hook decisions: Allow, Deny, Rewrite
//! - Failure policies: Continue, FailOpen, FailClosed
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=... cargo run --example 011-hooks-guardrails --features jsonl-store
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use meerkat::{
    AgentBuilder, AgentEvent, AgentFactory, AgentToolDispatcher, AnthropicClient, HookCapability,
    HookEntryConfig, HookExecutionMode, HookId, HookPoint, HookRuntimeConfig, HooksConfig, ToolDef,
    ToolError, ToolResult,
};
use meerkat_core::ToolCallView;
use meerkat_hooks::{DefaultHookEngine, RuntimeHookResponse};
use meerkat_store::{JsonlStore, StoreAdapter};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;

// ── Simple weather tool ──────────────────────────────────────────────────
//
// We need a tool so the agent makes tool calls and PreToolExecution fires.

#[derive(Debug, Clone, JsonSchema, Deserialize)]
struct WeatherArgs {
    /// City name (e.g. "San Francisco")
    city: String,
}

struct WeatherDispatcher;

#[async_trait]
impl AgentToolDispatcher for WeatherDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef {
            name: "get_weather".to_string(),
            description: "Get current weather for a city (simulated data)".to_string(),
            input_schema: meerkat_tools::schema_for::<WeatherArgs>(),
        })]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        match call.name {
            "get_weather" => {
                let args: WeatherArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;

                let result = json!({
                    "city": args.city,
                    "temperature": 22,
                    "unit": "celsius",
                    "condition": "partly cloudy",
                });
                Ok(ToolResult::new(
                    call.id.to_string(),
                    result.to_string(),
                    false,
                ))
            }
            _ => Err(ToolError::not_found(call.name)),
        }
    }
}

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

    // ── Define hooks ───────────────────────────────────────────────────────
    //
    // Hooks are defined in config and processed by the DefaultHookEngine.
    // In production, you'd put these in .rkat/config.toml.
    //
    // Here we use in-process hooks so their activity is visible on the
    // terminal. Command hooks capture stdout (piped to the hook engine),
    // so they wouldn't produce visible output.

    let hooks_config = HooksConfig {
        entries: vec![
            // Hook 1: Observe every LLM call (in-process)
            HookEntryConfig {
                id: HookId::new("audit-log"),
                point: HookPoint::PreLlmRequest,
                mode: HookExecutionMode::Foreground,
                capability: HookCapability::Observe,
                runtime: HookRuntimeConfig::new(
                    "in_process",
                    Some(serde_json::json!({ "name": "audit-log" })),
                )
                .unwrap_or_default(),
                ..Default::default()
            },
            // Hook 2: Observe tool calls before execution (in-process)
            HookEntryConfig {
                id: HookId::new("tool-filter"),
                point: HookPoint::PreToolExecution,
                mode: HookExecutionMode::Foreground,
                capability: HookCapability::Observe,
                runtime: HookRuntimeConfig::new(
                    "in_process",
                    Some(serde_json::json!({ "name": "tool-filter" })),
                )
                .unwrap_or_default(),
                ..Default::default()
            },
            // Hook 3: Observe turn boundaries (in-process)
            HookEntryConfig {
                id: HookId::new("turn-monitor"),
                point: HookPoint::TurnBoundary,
                mode: HookExecutionMode::Foreground,
                capability: HookCapability::Observe,
                runtime: HookRuntimeConfig::new(
                    "in_process",
                    Some(serde_json::json!({ "name": "turn-monitor" })),
                )
                .unwrap_or_default(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    // Build the hook engine directly so we can register in-process handlers
    // that print to the terminal.
    let engine = DefaultHookEngine::new(hooks_config)
        .with_in_process_handler(
            "audit-log",
            Arc::new(|invocation| {
                Box::pin(async move {
                    println!(
                        "  [HOOK] audit-log: Pre-LLM request (turn {:?})",
                        invocation.turn_number
                    );
                    Ok(RuntimeHookResponse::default())
                })
            }),
        )
        .with_in_process_handler(
            "tool-filter",
            Arc::new(|invocation| {
                Box::pin(async move {
                    let tool_name = invocation
                        .tool_call
                        .as_ref()
                        .map(|tc| tc.name.as_str())
                        .unwrap_or("unknown");
                    println!(
                        "  [HOOK] tool-filter: Pre-tool check for '{}' (turn {:?})",
                        tool_name, invocation.turn_number
                    );
                    Ok(RuntimeHookResponse::default())
                })
            }),
        )
        .with_in_process_handler(
            "turn-monitor",
            Arc::new(|invocation| {
                Box::pin(async move {
                    println!(
                        "  [HOOK] turn-monitor: Turn boundary (turn {:?})",
                        invocation.turn_number
                    );
                    Ok(RuntimeHookResponse::default())
                })
            }),
        );

    let hook_engine: Arc<dyn meerkat::HookEngine> = Arc::new(engine);

    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-5")
        .system_prompt(
            "You are a helpful weather assistant. Use the get_weather tool \
             to answer questions about the weather.",
        )
        .max_tokens_per_turn(1024)
        .with_hook_engine(hook_engine)
        .build(Arc::new(llm), Arc::new(WeatherDispatcher), store)
        .await;

    // ── Run the agent with event monitoring ────────────────────────────────

    let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(256);

    // Spawn a task to display hook lifecycle events as they happen
    let monitor = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match &event {
                AgentEvent::HookStarted { hook_id, point } => {
                    println!("  [EVENT] HookStarted: {hook_id} @ {point:?}");
                }
                AgentEvent::HookCompleted {
                    hook_id,
                    point,
                    duration_ms,
                } => {
                    println!("  [EVENT] HookCompleted: {hook_id} @ {point:?} ({duration_ms}ms)");
                }
                AgentEvent::HookFailed {
                    hook_id,
                    point,
                    error,
                } => {
                    println!("  [EVENT] HookFailed: {hook_id} @ {point:?}: {error}");
                }
                _ => {}
            }
        }
    });

    println!("=== Running agent with hooks ===\n");
    println!("Asking the agent a weather question so it uses the get_weather tool.");
    println!("Watch for [HOOK] and [EVENT] lines showing hook activity.\n");

    let result = agent
        .run_with_events(
            "What's the weather like in San Francisco and Tokyo?".to_string(),
            event_tx.clone(),
        )
        .await?;

    drop(event_tx);
    let _ = monitor.await;

    println!("\nResponse: {}", result.text);
    println!("\n--- Stats ---");
    println!("Tool calls: {}", result.tool_calls);
    println!("Turns:      {}", result.turns);

    // ── Show config-based hook definitions ─────────────────────────────────

    println!("\n=== Hook configuration (for .rkat/config.toml) ===\n");
    println!(
        r#"# .rkat/config.toml hook examples:

# Audit log: observe every turn boundary
[[hooks.entries]]
id = "audit-log"
point = "turn_boundary"
mode = "foreground"
capability = "observe"

[hooks.entries.runtime]
kind = "command"
config = {{ command = "python", args = ["hooks/audit.py"] }}

# Content filter: block dangerous tool calls
[[hooks.entries]]
id = "tool-safety"
point = "pre_tool_dispatch"
mode = "foreground"
capability = "gate"

[hooks.entries.runtime]
kind = "http"
config = {{ url = "http://localhost:8080/filter", method = "POST" }}

# Cost tracker: monitor token usage
[[hooks.entries]]
id = "cost-tracker"
point = "turn_boundary"
mode = "background"
capability = "observe"

[hooks.entries.runtime]
kind = "command"
config = {{ command = "bash", args = ["-c", "echo $HOOK_PAYLOAD >> /tmp/costs.log"] }}

# Available hook points:
#   turn_boundary        - Between agent loop iterations
#   pre_tool_execution   - Before a tool is called
#   post_tool_execution  - After a tool returns
#   pre_llm_request      - Before sending to the LLM
#   post_llm_response    - After LLM responds
#   run_started          - When the agent run begins
#   run_completed        - When the agent run finishes

# Hook capabilities:
#   observe    - Read-only, cannot modify the flow
#   gate       - Can Allow/Deny the operation
#   rewrite    - Can modify request/response payloads
#   guardrail  - Can deny and short-circuit all remaining hooks

# Hook runtime kinds:
#   command    - Shell command (receives JSON on stdin, returns JSON on stdout)
#   http       - HTTP endpoint (POST with JSON body)
#   in_process - Registered Rust function (for embedded use)
"#
    );

    Ok(())
}
