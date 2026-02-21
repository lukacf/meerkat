//! # 020 — Comms: Peer-to-Peer Messaging (Rust)
//!
//! Meerkat agents can communicate directly using Ed25519-signed messages
//! over TCP, UDS, or in-process channels. This example shows the comms
//! primitives: keypairs, peer discovery, and message exchange.
//!
//! ## What you'll learn
//! - Keypair generation and identity
//! - Trusted peer configuration
//! - Sending messages between agents
//! - In-process vs TCP transport modes
//! - The comms tool surface (send, peers, request)
//!
//! ## Run
//! ```bash
//! This is a reference implementation. For runnable examples, see meerkat/examples/.
//! ```

use std::sync::Arc;

use meerkat::{
    AgentBuilder, AgentFactory, AnthropicClient,
};
#[cfg(feature = "comms")]
use meerkat::CommsRuntime;
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::EmptyToolDispatcher;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    println!("=== Peer-to-Peer Agent Communication ===\n");

    // ── Show the comms architecture ────────────────────────────────────────

    println!(
        r#"Meerkat Comms: Ed25519-signed peer messaging

Architecture:
┌─────────────┐     TCP/UDS/Inproc     ┌─────────────┐
│   Agent A    │◄──────────────────────►│   Agent B    │
│              │    signed envelopes     │              │
│ Keypair A    │                        │ Keypair B    │
│ Tools:       │                        │ Tools:       │
│  - send()    │                        │  - send()    │
│  - peers()   │                        │  - peers()   │
│  - request() │                        │  - request() │
└─────────────┘                        └─────────────┘

Message flow:
1. Agent A calls send(target="agent-b", message="Hello")
2. Message is Ed25519-signed with Agent A's private key
3. Envelope is transmitted over TCP/UDS/inproc
4. Agent B verifies signature using Agent A's trusted public key
5. Message appears as an event in Agent B's inbox
6. Agent B processes the message in its next turn

Transport modes:
  - inproc: Same process, zero-copy channels (fastest, for mobs)
  - tcp:    Cross-process, Ed25519+TCP (production)
  - uds:    Unix domain sockets (same-machine, lower latency than TCP)
"#
    );

    // ── Build a basic agent to show the concept ────────────────────────────

    let store_dir = tempfile::tempdir()?.into_path().join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    let factory = AgentFactory::new(store_dir.clone());
    let client = Arc::new(AnthropicClient::new(api_key)?);
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4-5").await;

    let store = Arc::new(JsonlStore::new(store_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));

    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-5")
        .system_prompt(
            "You are an agent that can communicate with peers. \
             Explain how peer-to-peer agent communication works."
        )
        .max_tokens_per_turn(1024)
        .build(Arc::new(llm), Arc::new(EmptyToolDispatcher), store)
        .await;

    let result = agent
        .run(
            "How do agents communicate securely in a multi-agent system? \
             Describe the key concepts: identity, trust, signing, and transport."
                .to_string(),
        )
        .await?;

    println!("\nAgent's explanation:\n{}", result.text);

    // ── Configuration reference ────────────────────────────────────────────

    println!("\n\n=== Comms Configuration ===\n");
    println!(
        r#"# .rkat/config.toml

[comms]
mode = "tcp"                        # inproc | tcp | uds
address = "127.0.0.1:9100"         # Listen address
auth = true                         # Require Ed25519 signatures

# CLI usage:
rkat run --comms-name "agent-a" --host-mode "Send hello to agent-b"

# Python SDK:
result = await client.create_session(
    prompt="Hello peers!",
    comms_name="agent-a",
    host_mode=True,   # Stay alive for incoming messages
)

# TypeScript SDK:
const result = await client.createSession({
    prompt: "Hello peers!",
    comms_name: "agent-a",
    host_mode: true,
});

# Peer directory:
# Peers discover each other via:
#   1. Static config (trusted_peers in config)
#   2. In-process registry (for mobs)
#   3. Environment-based discovery
"#
    );

    Ok(())
}
