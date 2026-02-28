//! # 024 — Host Mode & Event Mesh (Rust)
//!
//! Host mode keeps an agent alive to receive and process incoming messages
//! between turns. Combined with event streaming, it enables reactive systems
//! where agents respond to external events in real time.
//!
//! ## What this example demonstrates
//! - `EphemeralSessionService`: the same session infrastructure used by all
//!   Meerkat surfaces (CLI, REST, RPC, MCP Server)
//! - Multi-turn event-driven processing: the agent stays alive between turns
//! - Event streaming via `AgentEvent` across multiple injected turns
//! - Reading session state to observe accumulating context
//!
//! ## How host mode works
//! Under the hood, `EphemeralSessionService` spawns a dedicated tokio task per
//! session. That task exclusively owns the `Agent` and processes commands via
//! channels. `create_session()` runs the first turn; subsequent `start_turn()`
//! calls inject new prompts as if they were external events. The agent retains
//! full conversation history across turns -- each new message builds on prior
//! context.
//!
//! With the `comms` feature, the agent loop (`run_host_mode_inner`) goes
//! further: after the initial prompt it enters a poll loop, draining its comms
//! inbox and processing peer messages/requests as they arrive, until the budget
//! is exhausted or a DISMISS signal is received. This example demonstrates the
//! multi-turn pattern without requiring the comms infrastructure.
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=... cargo run --example 024-host-mode-event-mesh --features jsonl-store
//! ```

use std::sync::Arc;

use meerkat::{
    AgentEvent, AgentFactory, Config, CreateSessionRequest, EphemeralSessionService,
    FactoryAgentBuilder, SessionService, StartTurnRequest,
};
use meerkat_core::EventEnvelope;
use meerkat_core::service::InitialTurnPolicy;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    // ── Architecture overview ──────────────────────────────────────────────

    println!(
        r#"=== Host Mode & Event Mesh ===

Standard mode:
  User prompt --> Agent runs --> Agent stops

Host mode (via SessionService):
  create_session() --> Agent runs turn 1 --> Session task STAYS ALIVE
                                                |
  start_turn("event 1") -----> Agent processes --> Session task stays alive
  start_turn("event 2") -----> Agent processes --> Session task stays alive
  ...                          (full history retained across turns)
  archive()            -----> Session task exits

With comms (--host-mode --comms-name "processor"):
  Initial prompt --> Agent runs --> Enters host loop
                                      |
  Peer message arrives  --> Agent processes --> Stays in loop
  Webhook POST          --> Agent processes --> Stays in loop
  Budget exhausted / DISMISS --> Agent exits loop

This example uses EphemeralSessionService -- the same infrastructure backing
CLI, REST, RPC, and MCP Server surfaces. Each session gets a dedicated tokio
task that owns the Agent exclusively (no mutex needed).
"#
    );

    // ── 1. Build the session service ───────────────────────────────────────

    // AgentFactory handles provider resolution, prompt assembly, and tool
    // dispatcher setup. The factory reads ANTHROPIC_API_KEY from the
    // environment to authenticate LLM calls.
    let _tmp = tempfile::tempdir()?;
    let store_dir = _tmp.path().join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    let factory = AgentFactory::new(store_dir);
    let config = Config::default();

    // FactoryAgentBuilder bridges AgentFactory into the SessionAgentBuilder
    // trait used by EphemeralSessionService. We can optionally inject a
    // default LLM client so all sessions use it without per-request overrides.
    let llm_client: Arc<dyn meerkat_client::LlmClient> =
        Arc::new(meerkat::AnthropicClient::new(api_key)?);

    let mut builder = FactoryAgentBuilder::new(factory, config);
    builder.default_llm_client = Some(llm_client);

    // EphemeralSessionService: in-memory session lifecycle. Each session gets
    // a dedicated tokio task. Max 4 concurrent sessions.
    let service = Arc::new(EphemeralSessionService::new(builder, 4));

    // ── 2. Create session (first turn) ─────────────────────────────────────

    println!("--- Turn 1: Initial alert ---\n");

    let (event_tx, event_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(256);
    let event_collector = spawn_event_collector(event_rx);

    let result = service
        .create_session(CreateSessionRequest {
            model: "claude-sonnet-4-5".to_string(),
            prompt: "An alert just fired: 'CPU usage on prod-web-03 exceeded 95% for \
                     5 minutes.' Acknowledge the alert and describe your initial triage \
                     steps. Keep your response to 2-3 sentences."
                .to_string(),
            system_prompt: Some(
                "You are a concise incident-response coordinator. \
                 You maintain context across multiple event injections, building an \
                 evolving picture of the incident. When you receive new information, \
                 integrate it with what you already know and adjust your response plan. \
                 Always be brief: 2-3 sentences max."
                    .to_string(),
            ),
            max_tokens: Some(256),
            event_tx: Some(event_tx),
            host_mode: false,
            skill_references: None,
            initial_turn: InitialTurnPolicy::RunImmediately,
            build: None,
            labels: None,
        })
        .await?;

    let session_id = result.session_id.clone();
    let events = event_collector.await?;
    print_turn_summary(1, &result.text, &events);

    // ── 3. Inject event: new monitoring data ───────────────────────────────

    println!("\n--- Turn 2: Monitoring event injected ---\n");

    let (event_tx, event_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(256);
    let event_collector = spawn_event_collector(event_rx);

    // start_turn injects a new prompt into the live session. The agent has
    // full access to the prior conversation history. This is exactly how
    // host-mode agents receive events in production: webhooks, peer messages,
    // and CLI input all flow through start_turn on the SessionService.
    let result = service
        .start_turn(
            &session_id,
            StartTurnRequest {
                prompt: "[MONITORING EVENT] Memory usage on prod-web-03 is now at 89%. \
                         Three other nodes in the cluster show normal metrics. \
                         The deployment log shows a new release was pushed 12 minutes ago."
                    .to_string(),
                event_tx: Some(event_tx),
                host_mode: false,
                skill_references: None,
                flow_tool_overlay: None,
                additional_instructions: None,
            },
        )
        .await?;

    let events = event_collector.await?;
    print_turn_summary(2, &result.text, &events);

    // ── 4. Read session state to show accumulated context ──────────────────

    let view = service.read(&session_id).await?;
    println!(
        "  [Session state: {} messages, {} tokens, active={}]\n",
        view.state.message_count, view.billing.total_tokens, view.state.is_active,
    );

    // ── 5. Inject event: resolution update ─────────────────────────────────

    println!("--- Turn 3: Resolution event injected ---\n");

    let (event_tx, event_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(256);
    let event_collector = spawn_event_collector(event_rx);

    let result = service
        .start_turn(
            &session_id,
            StartTurnRequest {
                prompt: "[RESOLUTION EVENT] The team rolled back the release on prod-web-03. \
                         CPU is back to 40%, memory at 52%. All health checks passing. \
                         Summarize the full incident timeline and close it out."
                    .to_string(),
                event_tx: Some(event_tx),
                host_mode: false,
                skill_references: None,
                flow_tool_overlay: None,
                additional_instructions: None,
            },
        )
        .await?;

    let events = event_collector.await?;
    print_turn_summary(3, &result.text, &events);

    // ── 6. Final session state ─────────────────────────────────────────────

    let view = service.read(&session_id).await?;
    println!(
        "  [Final session: {} messages, {} total tokens]\n",
        view.state.message_count, view.billing.total_tokens,
    );

    // ── 7. Archive (clean shutdown) ────────────────────────────────────────

    service.archive(&session_id).await?;
    println!("  Session archived (task stopped).\n");

    // ── Architecture reference ─────────────────────────────────────────────

    println!(
        r#"
=== Event Mesh Architecture ===

The event mesh connects agents, external systems, and users:

+--------------------------------------------------------------+
|                       EVENT MESH                             |
|                                                              |
|  +----------+    +----------+    +----------+                |
|  | Agent A  |<-->| Agent B  |<-->| Agent C  |                |
|  | (host)   |    | (host)   |    | (host)   |                |
|  +----+-----+    +----+-----+    +----+-----+                |
|       |               |               |                      |
|       v               v               v                      |
|  +----------+    +----------+    +----------+                |
|  | Webhook  |    | Timer    |    | User CLI |                |
|  | Source   |    | Source   |    | Input    |                |
|  +----------+    +----------+    +----------+                |
+--------------------------------------------------------------+

Event sources that feed into a host-mode agent:
  1. Peer messages (agent-to-agent via comms, Ed25519-signed)
  2. External webhooks (REST POST -> agent inbox)
  3. Timer events (scheduled processing)
  4. User input (CLI --stdin, SDK start_turn, RPC turn/start)
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
  const result = await client.createSession({{
      prompt: "Process incoming events",
      host_mode: true,
      comms_name: "processor",
  }});

  # Inject events via REST
  curl -X POST http://localhost:8000/sessions/{{id}}/turn \
    -H "Content-Type: application/json" \
    -d '{{"prompt": "New order received"}}'

  # Or via JSON-RPC
  {{"method": "turn/start", "params": {{"session_id": "...", "prompt": "..."}}}}

Host mode is essential for:
  - Mob orchestrators (need to receive worker reports)
  - Chat interfaces (bidirectional communication)
  - Event processors (react to webhooks/triggers)
  - Long-running agents (monitoring, ops automation)
"#
    );

    Ok(())
}

/// Spawn a task that collects events and returns them when the channel closes.
fn spawn_event_collector(
    mut event_rx: mpsc::Receiver<EventEnvelope<AgentEvent>>,
) -> tokio::task::JoinHandle<Vec<AgentEvent>> {
    tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(envelope) = event_rx.recv().await {
            events.push(envelope.payload);
        }
        events
    })
}

/// Print a summary of a turn: the response text and event statistics.
fn print_turn_summary(turn: usize, text: &str, events: &[AgentEvent]) {
    let mut text_deltas = 0usize;
    let mut delta_bytes = 0usize;
    let mut turns_started = 0usize;
    let mut turns_completed = 0usize;

    for event in events {
        match event {
            AgentEvent::TextDelta { delta } => {
                text_deltas += 1;
                delta_bytes += delta.len();
            }
            AgentEvent::TurnStarted { .. } => turns_started += 1,
            AgentEvent::TurnCompleted { .. } => turns_completed += 1,
            _ => {}
        }
    }

    println!("  Turn {turn} response: {text}");
    println!(
        "  Events: {} total ({} text deltas, {} bytes streamed, {} turns started, {} completed)",
        events.len(),
        text_deltas,
        delta_bytes,
        turns_started,
        turns_completed,
    );
}
