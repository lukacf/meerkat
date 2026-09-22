//! # 001 — Hello Meerkat (Rust)
//!
//! The simplest possible Meerkat session: send one prompt, get one response.
//! This is the Rust SDK equivalent of "Hello, World!"
//!
//! ## What you'll learn
//! - Loading config and building a `SessionService`
//! - Separating volatile service lifecycle from a scoped JSONL transcript store
//! - Running a single-turn session and reading the result
//!
//! Note: Production surfaces (CLI, REST, RPC, MCP) use the runtime-backed path
//! with `PersistentSessionService` + `MeerkatMachine` + session runtime
//! bindings for keep-alive, Queue/Steer routing, and comms. This example uses
//! the explicit standalone path for simplicity.
//!
//! ## Run
//! ```bash
//! # From the repository root
//! ANTHROPIC_API_KEY=sk-... ./scripts/repo-cargo run -p meerkat \
//!   --example 001-hello-meerkat --features jsonl-store
//! ```

use meerkat::{
    AgentFactory, Config, CreateSessionRequest, SessionService, build_ephemeral_service,
};
use meerkat_core::service::InitialTurnPolicy;
use meerkat_store::JsonlStore;
use std::{path::Path, sync::Arc};

type ExampleError = Box<dyn std::error::Error + Send + Sync>;

fn main() -> Result<(), ExampleError> {
    meerkat_runtime::host_stack::run_host("hello-meerkat", run)?
}

async fn scoped_factory(root: &Path) -> Result<AgentFactory, ExampleError> {
    let store = Arc::new(JsonlStore::new(root.join("sessions")));
    store.init().await?;
    Ok(AgentFactory::new(root.to_path_buf())
        .runtime_root(root.to_path_buf())
        .session_store(store))
}

async fn run() -> Result<(), ExampleError> {
    let config = Config::load().await?;
    // Keep the guard until the service has dropped, including on error returns.
    let scratch = tempfile::Builder::new()
        .prefix(".hello-meerkat-")
        .tempdir_in(std::env::current_dir()?)?;
    println!(
        "Temporary JSONL transcripts: {}",
        scratch.path().join("sessions").display()
    );
    let factory = scoped_factory(scratch.path()).await?;
    let service = build_ephemeral_service(factory, config, 16);

    let result = service
        .create_session(CreateSessionRequest {
            injected_context: Vec::new(),
            model: "claude-sonnet-4-6".into(),
            prompt: "What makes Rust's ownership model unique? Answer in two sentences.".into(),
            system_prompt: meerkat::SystemPromptOverride::Set(
                "You are a helpful assistant. Be concise.".into(),
            ),
            max_tokens: Some(512),
            event_tx: None,
            initial_turn: InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        })
        .await?;

    println!("{}", result.text);
    println!("\n--- Stats ---");
    println!("Session:  {}", result.session_id);
    println!("Turns:    {}", result.turns);
    println!("Tokens:   {}", result.usage.total_tokens());

    Ok(())
}

#[cfg(test)]
mod tests;
