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
//! ANTHROPIC_API_KEY=your-key cargo run --example 011_hooks_guardrails
//! ```

use std::sync::Arc;

use meerkat::{
    AgentBuilder, AgentFactory, AnthropicClient, Config,
    HookCapability, HookEntryConfig, HookExecutionMode, HookId, HookPoint,
    HookRuntimeConfig, HooksConfig,
    create_default_hook_engine,
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

    // ── Define hooks ───────────────────────────────────────────────────────
    //
    // Hooks are defined in config and processed by the DefaultHookEngine.
    // In production, you'd put these in .rkat/config.toml.

    let hooks_config = HooksConfig {
        entries: vec![
            // Hook 1: Log every LLM call (command-based hook)
            HookEntryConfig {
                id: HookId::new("audit-log"),
                point: HookPoint::TurnBoundary,
                mode: HookExecutionMode::Foreground,
                capability: HookCapability::Observe,
                runtime: HookRuntimeConfig::new(
                    "command",
                    Some(serde_json::json!({
                        "command": "echo",
                        "args": ["[AUDIT] Turn boundary event received"]
                    })),
                )
                .unwrap_or_default(),
                ..Default::default()
            },
            // Hook 2: Content filter on tool calls
            HookEntryConfig {
                id: HookId::new("tool-filter"),
                point: HookPoint::PreToolDispatch,
                mode: HookExecutionMode::Foreground,
                capability: HookCapability::Observe,
                runtime: HookRuntimeConfig::new(
                    "command",
                    Some(serde_json::json!({
                        "command": "echo",
                        "args": ["[FILTER] Pre-tool dispatch check"]
                    })),
                )
                .unwrap_or_default(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    // Create the hook engine from config
    let hook_engine = create_default_hook_engine(hooks_config);

    let mut builder = AgentBuilder::new()
        .model("claude-sonnet-4-5")
        .system_prompt("You are a helpful assistant.")
        .max_tokens_per_turn(1024);

    if let Some(engine) = hook_engine {
        builder = builder.hook_engine(engine);
    }

    let mut agent = builder
        .build(Arc::new(llm), Arc::new(EmptyToolDispatcher), store)
        .await;

    println!("=== Running agent with hooks ===\n");
    let result = agent
        .run("Explain how hooks work in event-driven systems.".to_string())
        .await?;

    println!("Response: {}", result.text);

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
#   turn_boundary      - Between agent loop iterations
#   pre_tool_dispatch  - Before a tool is called
#   post_tool_result   - After a tool returns
#   pre_llm_call       - Before sending to the LLM
#   post_llm_response  - After LLM responds
#   pre_compaction     - Before context compaction
#   post_compaction    - After context compaction

# Hook capabilities:
#   observe  - Read-only, cannot modify the flow
#   gate     - Can Allow/Deny/Rewrite the operation

# Hook runtime kinds:
#   command    - Shell command (receives JSON on stdin, returns JSON on stdout)
#   http       - HTTP endpoint (POST with JSON body)
#   in_process - Registered Rust function (for embedded use)
"#
    );

    Ok(())
}
