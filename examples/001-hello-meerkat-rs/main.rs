//! # 001 — Hello Meerkat (Rust)
//!
//! The simplest possible runtime-backed Meerkat session: send one prompt, get one response.
//! This is the Rust SDK equivalent of "Hello, World!"
//!
//! ## What you'll learn
//! - Loading config for the runtime-backed Rust surface
//! - Building a `SessionService` with `build_ephemeral_service`
//! - Running a single-turn session and reading the result
//!
//! ## Run
//! ```bash
//! This is a reference implementation. For runnable examples, see meerkat/examples/.
//! ```

use meerkat::{
    AgentFactory, Config, CreateSessionRequest, SessionService, build_ephemeral_service,
};
use meerkat_core::service::InitialTurnPolicy;
use meerkat_store::realm_paths;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::load().await?;
    let realm = realm_paths("examples/hello-meerkat");
    let factory = AgentFactory::new(realm.root.clone()).runtime_root(realm.root);
    let service = build_ephemeral_service(factory, config, 16);

    let result = service
        .create_session(CreateSessionRequest {
            model: "claude-sonnet-4-5".into(),
            prompt: "What makes Rust's ownership model unique? Answer in two sentences.".into(),
            render_metadata: None,
            system_prompt: Some("You are a helpful assistant. Be concise.".into()),
            max_tokens: Some(512),
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::RunImmediately,
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
