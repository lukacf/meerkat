//! # 015 — Session Persistence (Rust)
//!
//! Sessions can be persisted to disk so agents survive restarts. This example
//! shows the different storage backends and how session lifecycle works.
//!
//! ## What you'll learn
//! - JsonlStore vs MemoryStore vs RedbSessionStore
//! - Saving and loading sessions
//! - Session lifecycle: create → turns → archive
//! - Event sourcing with RedbEventStore
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=your-key cargo run --example 015_session_persistence
//! ```

use std::sync::Arc;

use meerkat::{
    AgentBuilder, AgentFactory, AnthropicClient, SessionFilter, SessionStore,
};
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::EmptyToolDispatcher;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    let store_dir = tempfile::tempdir()?.into_path().join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    // ── JsonlStore: file-based persistence ─────────────────────────────────
    println!("=== Backend 1: JsonlStore (file-based) ===\n");

    let jsonl_store = Arc::new(JsonlStore::new(store_dir.clone()));
    jsonl_store.init().await?;
    let adapted_store = Arc::new(StoreAdapter::new(jsonl_store.clone()));

    let factory = AgentFactory::new(store_dir.clone());
    let client = Arc::new(AnthropicClient::new(api_key.clone())?);
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4-5").await;

    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-5")
        .system_prompt("You are a persistent assistant. You remember across restarts.")
        .max_tokens_per_turn(512)
        .build(Arc::new(llm), Arc::new(EmptyToolDispatcher), adapted_store)
        .await;

    // Turn 1: Create session
    let result = agent
        .run("My project is called Phoenix. Remember that.".to_string())
        .await?;
    let session_id = result.session_id.clone();
    println!("Created session: {}", session_id);
    println!("Response: {}\n", result.text);

    // Session is automatically saved to disk after each turn.
    // List sessions from the store:
    let sessions = jsonl_store.list(SessionFilter::default()).await?;
    println!("Sessions on disk: {}", sessions.len());
    for s in &sessions {
        println!(
            "  {} — created: {:?}, messages: {}",
            s.id, s.created_at, s.message_count
        );
    }

    // Load session from disk (simulating a restart)
    let loaded = jsonl_store.load(&session_id.parse()?).await?;
    if let Some(session) = loaded {
        println!(
            "\nLoaded session {} with {} messages",
            session.id(),
            session.messages().len()
        );
    }

    // Turn 2: Continue the persisted session
    let result = agent
        .run("What's my project called?".to_string())
        .await?;
    println!("\nTurn 2 response: {}", result.text);

    // ── Storage backend comparison ─────────────────────────────────────────

    println!("\n\n=== Storage Backend Comparison ===\n");
    println!(
        r#"| Backend          | Feature Flag    | Persistence | Best For                    |
|------------------|-----------------|-------------|-----------------------------|
| JsonlStore       | jsonl-store     | File (JSONL)| Development, simple deploy  |
| MemoryStore      | memory-store    | None (RAM)  | Tests, ephemeral agents     |
| RedbSessionStore | session-store   | redb (B+)   | Production, multi-session   |

RedbSessionStore also supports:
- Event sourcing via RedbEventStore
- Session projection via SessionProjector
- Realm-based isolation for multi-tenant setups

# Feature flags in Cargo.toml:
[dependencies]
meerkat = {{ version = "0.3", features = ["jsonl-store", "session-store"] }}
"#
    );

    Ok(())
}
