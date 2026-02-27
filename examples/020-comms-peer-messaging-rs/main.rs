//! # 020 — Comms: Peer-to-Peer Messaging (Rust)
//!
//! Two Meerkat agents communicate directly using Ed25519-signed messages
//! over TCP. This example shows the comms primitives: keypairs, peer
//! discovery, TCP transport, and message exchange.
//!
//! ## What you'll learn
//! - Keypair generation and identity
//! - Trusted peer configuration
//! - TCP transport setup with `spawn_tcp_listener`
//! - Building agents with `CommsToolDispatcher` for send/peers tools
//! - Sending messages between agents and receiving them in the inbox
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=... cargo run --example 020-comms-peer-messaging --features "jsonl-store,comms"
//! ```

use std::sync::Arc;

use meerkat::{AgentBuilder, AgentFactory, AgentToolDispatcher, AnthropicClient};
use meerkat_comms::agent::{
    CommsAgent, CommsManager, CommsManagerConfig, CommsToolDispatcher, spawn_tcp_listener,
};
use meerkat_comms::{Keypair, TrustedPeer, TrustedPeers};
use meerkat_store::{JsonlStore, StoreAdapter};
use tokio::sync::RwLock;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    println!("=== Peer-to-Peer Agent Communication ===\n");

    // ── 1. Generate Ed25519 keypairs ────────────────────────────────────────
    // Each agent gets its own identity. The public key is shared with peers;
    // the private key signs every outgoing message.

    let keypair_a = Keypair::generate();
    let keypair_b = Keypair::generate();
    let pubkey_a = keypair_a.public_key();
    let pubkey_b = keypair_b.public_key();

    println!("Agent A public key: {pubkey_a:?}");
    println!("Agent B public key: {pubkey_b:?}");

    // ── 2. Bind TCP ports ───────────────────────────────────────────────────
    // Each agent listens on its own port. We bind-then-drop to discover free
    // ports, then pass the addresses to the TCP listeners.

    let listener_a = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr_a = listener_a.local_addr()?;
    drop(listener_a);

    let listener_b = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr_b = listener_b.local_addr()?;
    drop(listener_b);

    println!("\nAgent A listening on: tcp://{addr_a}");
    println!("Agent B listening on: tcp://{addr_b}");

    // ── 3. Configure trusted peers ──────────────────────────────────────────
    // Each agent declares which peers it trusts. The trust entry includes
    // the peer's name, public key, and transport address.

    let trusted_for_a = TrustedPeers {
        peers: vec![TrustedPeer {
            name: "agent-b".to_string(),
            pubkey: pubkey_b,
            addr: format!("tcp://{addr_b}"),
            meta: meerkat_comms::PeerMeta::default(),
        }],
    };

    let trusted_for_b = TrustedPeers {
        peers: vec![TrustedPeer {
            name: "agent-a".to_string(),
            pubkey: pubkey_a,
            addr: format!("tcp://{addr_a}"),
            meta: meerkat_comms::PeerMeta::default(),
        }],
    };

    // ── 4. Create CommsManagers ─────────────────────────────────────────────
    // Each CommsManager owns a keypair, inbox, and router. It is the
    // central hub for all comms operations within one agent.

    let config_a = CommsManagerConfig::with_keypair(keypair_a).trusted_peers(trusted_for_a.clone());
    let comms_a = CommsManager::new(config_a)?;

    let config_b = CommsManagerConfig::with_keypair(keypair_b).trusted_peers(trusted_for_b.clone());
    let comms_b = CommsManager::new(config_b)?;

    // ── 5. Start TCP listeners ──────────────────────────────────────────────
    // TCP listeners accept incoming connections and verify Ed25519 signatures.
    // Messages from untrusted keys are rejected.

    let trusted_a_shared = Arc::new(RwLock::new(trusted_for_a));
    let trusted_b_shared = Arc::new(RwLock::new(trusted_for_b));

    let _listener_a = spawn_tcp_listener(
        &addr_a.to_string(),
        comms_a.keypair_arc(),
        trusted_a_shared.clone(),
        comms_a.inbox_sender().clone(),
    )
    .await?;

    let _listener_b = spawn_tcp_listener(
        &addr_b.to_string(),
        comms_b.keypair_arc(),
        trusted_b_shared.clone(),
        comms_b.inbox_sender().clone(),
    )
    .await?;

    // Brief pause for listeners to bind
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // ── 6. Create comms tool dispatchers ────────────────────────────────────
    // CommsToolDispatcher gives the agent two tools:
    //   - `peers` — list trusted peers
    //   - `send`  — send a signed message to a peer

    let tools_a = Arc::new(CommsToolDispatcher::new(
        comms_a.router().clone(),
        trusted_a_shared,
    ));
    let tools_b = Arc::new(CommsToolDispatcher::new(
        comms_b.router().clone(),
        trusted_b_shared,
    ));

    println!("\nComms tools available to each agent:");
    for tool in tools_a.tools().iter() {
        println!("  - {} : {}", tool.name, tool.description);
    }

    // ── 7. Build two agents with LLM + comms tools ──────────────────────────

    let store_dir = tempfile::tempdir()?.keep().join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    let factory = AgentFactory::new(store_dir.clone());
    let model =
        std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string());

    let client_a = Arc::new(AnthropicClient::new(api_key.clone())?);
    let llm_a = factory.build_llm_adapter(client_a, &model).await;

    let client_b = Arc::new(AnthropicClient::new(api_key)?);
    let llm_b = factory.build_llm_adapter(client_b, &model).await;

    let store_a = Arc::new(JsonlStore::new(store_dir.clone()));
    store_a.init().await?;
    let store_a = Arc::new(StoreAdapter::new(store_a));

    let store_b = Arc::new(JsonlStore::new(store_dir));
    store_b.init().await?;
    let store_b = Arc::new(StoreAdapter::new(store_b));

    let agent_a_inner = AgentBuilder::new()
        .model(&model)
        .max_tokens_per_turn(1024)
        .system_prompt(
            "You are Agent A. You can communicate with other agents using the send tool. \
             Use the peers tool to discover available peers, then send messages to them.",
        )
        .build(Arc::new(llm_a), tools_a, store_a)
        .await;

    let agent_b_inner = AgentBuilder::new()
        .model(&model)
        .max_tokens_per_turn(1024)
        .system_prompt(
            "You are Agent B. When you receive messages from other agents, \
             acknowledge them and respond thoughtfully.",
        )
        .build(Arc::new(llm_b), tools_b, store_b)
        .await;

    // Wrap each agent with its CommsManager so inbox messages are
    // automatically injected into the conversation.
    let mut agent_a = CommsAgent::new(agent_a_inner, comms_a);
    let mut agent_b = CommsAgent::new(agent_b_inner, comms_b);

    // ── 8. Agent A sends a message to Agent B ───────────────────────────────
    // The LLM will call `peers` to discover agent-b, then call `send` to
    // deliver a signed message over TCP.

    println!("\n--- Phase 1: Agent A sends a message ---\n");

    let result_a = agent_a
        .run(
            "Send the message 'Hello from Agent A! How are you today?' \
             to agent-b using the send tool."
                .to_string(),
        )
        .await?;

    println!("Agent A response: {}", result_a.text);
    println!(
        "  (tool calls: {}, turns: {})",
        result_a.tool_calls, result_a.turns
    );

    // ── 9. Wait for TCP delivery, then check Agent B's inbox ────────────────

    println!("\n--- Phase 2: Checking Agent B's inbox ---\n");
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    let inbox = agent_b.comms_manager_mut().drain_messages();
    println!("Messages in Agent B's inbox: {}", inbox.len());
    for (i, msg) in inbox.iter().enumerate() {
        println!("  Message {}: from={}, content={:?}", i + 1, msg.from_peer, msg.content);
    }

    if inbox.is_empty() {
        println!("\nNo messages arrived. The LLM may not have called the send tool correctly.");
        println!("Try running again -- LLM tool use can be non-deterministic.");
        return Ok(());
    }

    // ── 10. Agent B processes the inbox message ─────────────────────────────
    // Re-inject the drained messages so CommsAgent.run() picks them up.

    println!("\n--- Phase 3: Agent B processes the message ---\n");

    for msg in inbox {
        let item = meerkat_comms::InboxItem::External {
            envelope: meerkat_comms::Envelope {
                id: msg.envelope_id,
                from: msg.from_pubkey,
                to: agent_b.comms_manager().keypair().public_key(),
                kind: match msg.content {
                    meerkat_comms::agent::CommsContent::Message { body } => {
                        meerkat_comms::MessageKind::Message { body }
                    }
                    _ => continue,
                },
                sig: meerkat_comms::Signature::new([0u8; 64]),
            },
        };
        let _ = agent_b.comms_manager().inbox_sender().send(item);
    }

    let result_b = agent_b.run(String::new()).await?;

    println!("Agent B response: {}", result_b.text);
    println!(
        "  (tool calls: {}, turns: {})",
        result_b.tool_calls, result_b.turns
    );

    // ── Summary ─────────────────────────────────────────────────────────────

    println!("\n=== Done ===");
    println!("Agent A sent a signed message over TCP to Agent B.");
    println!("Agent B received it, verified the signature, and responded.");

    // ── Architecture reference ──────────────────────────────────────────────

    println!(
        r#"

=== Comms Architecture ===

┌─────────────┐     TCP/UDS/Inproc     ┌─────────────┐
│   Agent A    │◄──────────────────────►│   Agent B    │
│              │    signed envelopes     │              │
│ Keypair A    │                        │ Keypair B    │
│ Tools:       │                        │ Tools:       │
│  - send()    │                        │  - send()    │
│  - peers()   │                        │  - peers()   │
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

    // ── Configuration reference ──────────────────────────────────────────────

    println!("=== Comms Configuration ===\n");
    let config_ref = r#"# .rkat/config.toml

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
"#;
    println!("{config_ref}");

    Ok(())
}
