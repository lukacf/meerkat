//! # 024 — Host Mode & Event Mesh (Rust)
//!
//! Host mode keeps an agent alive to receive and process incoming messages
//! from peers. Combined with the event mesh, it enables reactive multi-agent
//! systems where agents respond to external events in real time.
//!
//! ## What you'll learn
//! - Host mode: agent stays alive for incoming events
//! - Event mesh: external events injected into agent context
//! - The InputStreamMode for continuous message processing
//! - Building reactive, event-driven agent systems
//!
//! ## Run
//! ```bash
//! This is a reference implementation. For runnable examples, see meerkat/examples/.
//! ```

use std::sync::Arc;

use meerkat::{
    AgentBuilder, AgentFactory, AnthropicClient,
};
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::EmptyToolDispatcher;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    println!("=== Host Mode & Event Mesh ===\n");

    // ── Architecture overview ──────────────────────────────────────────────

    println!(
        r#"Host Mode: Agents That Listen

Standard mode:
  User prompt → Agent runs → Agent stops

Host mode:
  User prompt → Agent runs → Agent STAYS ALIVE
                                ↓
              Incoming messages → Agent processes → Agent stays alive
              External events  → Agent processes → Agent stays alive
              ...repeat until explicitly stopped...

This enables:
  1. Reactive agents that respond to peer messages
  2. Event-driven processing (webhook → agent → action)
  3. Long-running orchestrators in mob systems
  4. Chat-like interfaces with bidirectional communication
"#
    );

    // ── Build a basic agent to demonstrate the concept ─────────────────────

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
            "You are an event-processing agent. Explain how host mode and \
             event meshes work in multi-agent systems."
        )
        .max_tokens_per_turn(1024)
        .build(Arc::new(llm), Arc::new(EmptyToolDispatcher), store)
        .await;

    let result = agent
        .run(
            "Describe three real-world scenarios where an agent needs to stay alive \
             and process incoming events rather than just responding to a single prompt."
                .to_string(),
        )
        .await?;

    println!("Agent's analysis:\n{}\n", result.text);

    // ── Event mesh architecture ────────────────────────────────────────────

    println!("=== Event Mesh Architecture ===\n");
    println!(
        r#"The event mesh connects agents, external systems, and users:

┌──────────────────────────────────────────────────────────────┐
│                       EVENT MESH                              │
│                                                              │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐              │
│  │ Agent A   │←──→│ Agent B   │←──→│ Agent C   │              │
│  │ (host)    │    │ (host)    │    │ (host)    │              │
│  └────┬─────┘    └────┬─────┘    └────┬─────┘              │
│       │               │               │                      │
│       ↕               ↕               ↕                      │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐              │
│  │ Webhook   │    │ Timer     │    │ User CLI  │              │
│  │ Source    │    │ Source    │    │ Input     │              │
│  └──────────┘    └──────────┘    └──────────┘              │
└──────────────────────────────────────────────────────────────┘

Event sources:
  1. Peer messages (agent-to-agent via comms)
  2. External webhooks (REST POST → agent inbox)
  3. Timer events (scheduled processing)
  4. User input (CLI or SDK `start_turn`)
  5. System events (compaction, budget alerts)

Configuration:

  # CLI: Start agent in host mode
  rkat run --host-mode --comms-name "processor" "Process incoming events"

  # Python SDK
  result = await client.create_session(
      "Process incoming events",
      host_mode=True,
      comms_name="processor",
  )

  # TypeScript SDK
  const result = await client.createSession({
      prompt: "Process incoming events",
      host_mode: true,
      comms_name: "processor",
  });

  # REST: Send external event to agent
  curl -X POST http://localhost:8000/webhooks/comms-message \
    -H "Content-Type: application/json" \
    -d '{{"target": "processor", "body": "New order received"}}'

  # Comms event stream (scoped):
  #   RPC method: comms/stream_open
  #   Notification: comms/stream_event
  #   RPC method: comms/stream_close

Host mode is essential for:
  - Mob orchestrators (need to receive worker reports)
  - Chat interfaces (bidirectional communication)
  - Event processors (react to webhooks/triggers)
  - Long-running agents (monitoring, ops automation)
"#
    );

    Ok(())
}
