//! # 013 — Context Compaction (Rust)
//!
//! Long conversations eventually exceed the LLM's context window. Meerkat's
//! compaction system automatically summarizes old messages to keep the agent
//! supporting longer conversations without retaining every prior message.
//!
//! ## What you'll learn
//! - How the `DefaultCompactor` works
//! - Configuring compaction thresholds
//! - The compaction flow within the agent loop
//! - Preserving critical messages during compaction
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=... ./scripts/repo-cargo run -p meerkat --example 013-context-compaction --features jsonl-store,session-compaction
//! ```

use std::sync::Arc;

use meerkat::{AgentBuilder, AgentEvent, AgentFactory, AnthropicClient, Config};
use meerkat_core::compact::CompactionConfig;
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::EmptyToolDispatcher;
use tokio::sync::mpsc;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    meerkat_runtime::host_stack::run_host("013-context-compaction", async_main)?
}

async fn async_main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

    // ── Configure compaction through the factory config ────────────────────
    //
    // We set a LOW token threshold (2000) so compaction can trigger during this
    // short demo. In production you'd use a much higher value (e.g. 100_000).
    let compaction_config = CompactionConfig {
        auto_compact_threshold: 2000,
        max_request_bytes: None,
        recent_turn_budget: 2,
        max_summary_tokens: 1024,
        min_turns_between_compactions: 2,
    };
    let mut config = Config::default();
    config.compaction.auto_compact_threshold = compaction_config.auto_compact_threshold;
    config.compaction.recent_turn_budget = compaction_config.recent_turn_budget;
    config.compaction.max_summary_tokens = compaction_config.max_summary_tokens;
    config.compaction.min_turns_between_compactions =
        compaction_config.min_turns_between_compactions;

    println!("=== Context Compaction Demo ===");
    println!("Compaction threshold: 2000 tokens (low for demo purposes)");
    println!("Recent-turn retention budget: up to 2 (subject to progress and capacity)\n");

    let mut session = meerkat_core::Session::new();
    session.set_session_metadata(meerkat_core::SessionMetadata {
        model_fallback: None,
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
        mob_member_binding: None,
    })?;
    session.set_build_state(meerkat_core::SessionBuildState::default())?;

    let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(256);

    // Build through the facade factory so compaction runs with the same
    // session metadata and runtime bindings as production-facing surfaces.
    let mut agent = AgentBuilder::new()
        .with_factory(factory)
        .with_config(config)
        .model("claude-sonnet-4-6")
        .system_prompt(
            "You are a patient tutor. Build on previous conversation context. \
             Always reference earlier topics when relevant. Give thorough, \
             detailed explanations with code examples.",
        )
        .max_tokens_per_turn(1024)
        .resume_session(session)
        .build(Arc::new(llm), Arc::new(EmptyToolDispatcher), store)
        .await?;

    // Simulate a long conversation that may trigger compaction.
    // Each prompt asks for detailed explanations to accumulate tokens quickly.
    let topics = [
        "Explain Rust's ownership model in detail with code examples.",
        "Now explain how lifetimes relate to what you just said about ownership. Include examples.",
        "How do smart pointers like Box, Rc, and Arc fit into this picture? Show code.",
        "Give me a practical example combining ownership, lifetimes, and Arc.",
        "Summarize everything we've discussed about Rust's memory model.",
    ];

    // Monitor for compaction events — these will fire when the compactor
    // detects current context/last-request pressure above our threshold.
    let monitor = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match &event {
                AgentEvent::CompactionStarted { .. } => {
                    println!("\n[COMPACTION] Context compaction triggered!");
                }
                AgentEvent::CompactionCompleted { .. } => {
                    println!("[COMPACTION] Compaction completed — context window refreshed.\n");
                }
                _ => {}
            }
        }
    });

    for (i, topic) in topics.iter().enumerate() {
        println!("\n=== Turn {} ===", i + 1);
        println!("User: {topic}\n");

        let result = agent
            .run_with_events(topic.to_string().into(), event_tx.clone())
            .await?;

        println!("Assistant: {}", response_preview(&result.text, 200));
        println!(
            "(input tokens: {}, output tokens: {}, total: {})",
            result.usage.input_tokens,
            result.usage.output_tokens,
            result.usage.total_tokens()
        );
    }

    drop(event_tx);
    let _ = monitor.await;

    // ── Compaction configuration reference ─────────────────────────────────

    println!("\n\n=== Compaction Configuration Reference ===\n");
    println!(
        r"# .rkat/config.toml

[compaction]
# Trigger on current context/last-request token pressure, not lifetime usage
auto_compact_threshold = 50000

# Maximum tokens for the compaction summary
max_summary_tokens = 1024

# Upper bound on recent turns retained verbatim; compaction must remove at
# least one live turn and may retain fewer under request-capacity pressure.
# Unkeyed System messages and the latest version of each keyed prompt are
# preserved in order; superseded keyed prompt versions can be compacted.
recent_turn_budget = 4

# Minimum number of turns between successive compactions
min_turns_between_compactions = 3

# How compaction works:
#
# 1. Agent loop detects current context/last-request token pressure
# 2. Compactor selects messages to summarize (excluding preserved ones)
# 3. LLM generates a concise summary of the selected messages
# 4. Old messages are replaced with the summary
# 5. Agent continues with reduced context but preserved knowledge
#
# The result: agents can sustain longer conversations by retaining recent and
# summarized context while keeping request size bounded.
"
    );

    Ok(())
}

fn response_preview(text: &str, max_bytes: usize) -> String {
    let end = text.floor_char_boundary(text.len().min(max_bytes));
    if end < text.len() {
        format!("{}...", &text[..end])
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::response_preview;

    #[test]
    fn preview_respects_utf8_and_only_marks_truncated_text() {
        assert_eq!(response_preview("", 200), "");
        assert_eq!(response_preview("—é", 200), "—é");
        assert_eq!(response_preview(&"a".repeat(200), 200), "a".repeat(200));
        assert_eq!(
            response_preview(&"a".repeat(201), 200),
            format!("{}...", "a".repeat(200))
        );
        assert_eq!(
            response_preview(&format!("{}—tail", "a".repeat(199)), 200),
            format!("{}...", "a".repeat(199))
        );
        assert_eq!(response_preview("é", 0), "...");
    }
}
