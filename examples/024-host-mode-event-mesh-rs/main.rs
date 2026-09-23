//! # 024 - Multi-Turn Event Processing (Rust)
//!
//! Demonstrates explicit multi-turn processing with `EphemeralSessionService`.
//! The program directly submits each turn and the in-process session retains
//! context between calls.
//!
//! ## What this example demonstrates
//! - `EphemeralSessionService`: standalone in-memory session lifecycle
//! - Multi-turn processing via repeated `start_turn()` calls
//! - Event streaming via `AgentEvent` across multiple injected turns
//! - Reading session state to observe accumulating context
//!
//! ## How it works
//! `EphemeralSessionService` spawns a dedicated tokio task per session. That task
//! exclusively owns the `Agent` and processes commands via channels.
//! `create_session()` runs the first turn; subsequent `start_turn()` calls inject
//! new prompts. The agent retains full conversation history across turns in
//! the current process.
//!
//! This example does not configure comms, external-event ingress, schedules, or
//! process recovery. For those behaviors, use a runtime-backed surface such as
//! `rkat run --keep-alive --comms-name processor`.
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=... ./scripts/repo-cargo run -p meerkat --example 024-host-mode-event-mesh --features jsonl-store
//! ```

use std::sync::Arc;
use std::{io::Write, path::Path};

use meerkat::{
    AgentEvent, AgentFactory, Config, CreateSessionRequest, EphemeralSessionService,
    FactoryAgentBuilder, SessionService, StartTurnRequest, StartTurnRuntimeSemantics,
};
use meerkat_core::EventEnvelope;
use meerkat_core::service::InitialTurnPolicy;
use tokio::sync::mpsc;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    meerkat_runtime::host_stack::run_host("024-host-mode-event-mesh", async_main)?
}

async fn async_main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;
    let directory = tempfile::tempdir_in(".")?;
    run_with_client(
        Arc::new(meerkat::AnthropicClient::new(api_key)?),
        directory.path(),
        &mut std::io::stdout(),
    )
    .await
}

async fn run_with_client(
    client: Arc<dyn meerkat_client::LlmClient>,
    directory: &Path,
    output: &mut (impl Write + Send),
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let service = build_service(client, directory)?;
    run_session(&service, output).await
}

fn build_service(
    client: Arc<dyn meerkat_client::LlmClient>,
    directory: &Path,
) -> Result<EphemeralSessionService<FactoryAgentBuilder>, Box<dyn std::error::Error + Send + Sync>>
{
    let store_dir = directory.join("sessions");
    std::fs::create_dir_all(&store_dir)?;
    let factory = AgentFactory::new(store_dir);
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(client);
    Ok(EphemeralSessionService::new(builder, 4))
}

async fn run_session(
    service: &impl SessionService,
    output: &mut (impl Write + Send),
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    macro_rules! println {
        ($($args:tt)*) => { writeln!(output, $($args)*)? };
    }

    // ── Architecture overview ──────────────────────────────────────────────

    println!(
        r"=== In-Process Multi-Turn Session ===

Single-turn mode:
  User prompt --> Agent runs --> Agent stops

This example:
  create_session() --> run initial prompt
  start_turn(...)  --> run monitoring update with prior context
  start_turn(...)  --> run resolution update with prior context
  archive()        --> stop the in-process session task

The prompts are submitted directly by this program. No webhook, peer message,
timer, or durable external ingress is configured here.
"
    );

    // ── 2. Create session (first turn) ─────────────────────────────────────

    println!("--- Turn 1: Initial alert ---\n");

    let (event_tx, event_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(256);
    let event_collector = spawn_event_collector(event_rx);

    let result = service
        .create_session(CreateSessionRequest {
            injected_context: Vec::new(),
            model: "claude-sonnet-4-6".to_string(),
            prompt: "An alert just fired: 'CPU usage on prod-web-03 exceeded 95% for \
                     5 minutes.' Acknowledge the alert and describe your initial triage \
                     steps. Keep your response to 2-3 sentences."
                .into(),
            system_prompt: meerkat::SystemPromptOverride::Set(
                "You are a concise incident-response coordinator. \
                 You maintain context across multiple event injections, building an \
                 evolving picture of the incident. When you receive new information, \
                 integrate it with what you already know and adjust your response plan. \
                 Always be brief: 2-3 sentences max."
                    .to_string(),
            ),
            max_tokens: Some(256),
            event_tx: Some(event_tx),
            initial_turn: InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        })
        .await;
    let events = event_collector.await?;
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            // A failed eager first turn has no returned session id. This
            // dedicated demo service owns only sessions created by this body.
            match service.list(Default::default()).await {
                Ok(sessions) => {
                    for session in sessions {
                        if let Err(cleanup) = service.archive(&session.session_id).await {
                            eprintln!("Session cleanup failed: {cleanup}");
                        }
                    }
                }
                Err(cleanup) => eprintln!("Session cleanup lookup failed: {cleanup}"),
            }
            return Err(error.into());
        }
    };
    let session_id = result.session_id.clone();
    let turns_result = async {
        print_turn_summary(1, &result.text, &events, output)?;

        // ── 3. Inject event: new monitoring data ───────────────────────────────

        println!("\n--- Turn 2: Monitoring event injected ---\n");

        let (event_tx, event_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(256);
        let event_collector = spawn_event_collector(event_rx);

        // start_turn submits a new prompt to the in-process session. The agent has
        // access to the prior conversation history. This call is made directly by
        // the example; it is not external-event ingress.
        let result = service
            .start_turn(
                &session_id,
                StartTurnRequest {
                    injected_context: Vec::new(),
                    prompt: "[MONITORING EVENT] Memory usage on prod-web-03 is now at 89%. \
                         Three other nodes in the cluster show normal metrics. \
                         The deployment log shows a new release was pushed 12 minutes ago."
                        .into(),
                    system_prompt: None,
                    event_tx: Some(event_tx),
                    runtime: StartTurnRuntimeSemantics::default(),
                },
            )
            .await;

        let events = event_collector.await?;
        let result = result?;
        print_turn_summary(2, &result.text, &events, output)?;

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
                    injected_context: Vec::new(),
                    prompt: "[RESOLUTION EVENT] The team rolled back the release on prod-web-03. \
                         CPU is back to 40%, memory at 52%. All health checks passing. \
                         Summarize the full incident timeline and close it out."
                        .into(),
                    system_prompt: None,
                    event_tx: Some(event_tx),
                    runtime: StartTurnRuntimeSemantics::default(),
                },
            )
            .await;

        let events = event_collector.await?;
        let result = result?;
        print_turn_summary(3, &result.text, &events, output)?;

        // ── 6. Final session state ─────────────────────────────────────────────

        let view = service.read(&session_id).await?;
        println!(
            "  [Final session: {} messages, {} total tokens]\n",
            view.state.message_count, view.billing.total_tokens,
        );
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    }
    .await;

    // ── 7. Archive (clean shutdown) ────────────────────────────────────────

    let cleanup = service.archive(&session_id).await;
    if let Err(error) = &cleanup {
        eprintln!("Session cleanup failed: {error}");
    }
    turns_result?;
    cleanup?;
    println!("  Session archived (task stopped).\n");

    // Scope and runtime boundary

    println!(
        r"
=== Scope and Runtime Boundary ===

Configured here:
  - Direct create_session() and start_turn() calls
  - Context retention within the current process
  - AgentEvent streaming for each submitted turn
  - Explicit archive() cleanup

Not configured here:
  - Comms identity or peer messaging
  - Webhook or external-event ingress
  - Scheduler wakeups
  - Runtime-backed recovery after process exit

Use the runtime-backed CLI, REST, JSON-RPC, MCP, or SDK host when those
operational behaviors are required.
"
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
fn print_turn_summary(
    turn: usize,
    text: &str,
    events: &[AgentEvent],
    output: &mut impl Write,
) -> std::io::Result<()> {
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

    writeln!(output, "  Turn {turn} response: {text}")?;
    writeln!(
        output,
        "  Events: {} total ({} text deltas, {} bytes streamed, {} turns started, {} completed)",
        events.len(),
        text_deltas,
        delta_bytes,
        turns_started,
        turns_completed,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::{Message, Provider, StopReason};
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Default)]
    struct IncidentClient {
        requests: Mutex<Vec<LlmRequest>>,
        fail_on: Option<usize>,
    }

    #[async_trait::async_trait]
    impl LlmClient for IncidentClient {
        fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_client::types::LlmStream<'a> {
            let call = {
                let mut requests = self.requests.lock().unwrap();
                requests.push(request.clone());
                requests.len()
            };
            assert_eq!(request.model, "claude-sonnet-4-6");
            let events = if self.fail_on == Some(call) {
                vec![Err(LlmError::InvalidRequest {
                    message: "synthetic incident failure".into(),
                })]
            } else {
                vec![
                    Ok(LlmEvent::TextDelta {
                        delta: format!("incident-answer-{call}"),
                        meta: None,
                    }),
                    Ok(LlmEvent::UsageUpdate {
                        usage: meerkat_core::TurnUsage::host_declared(
                            Provider::Anthropic,
                            &request.model,
                            meerkat_core::Usage::default(),
                        ),
                    }),
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: StopReason::EndTurn,
                        },
                    }),
                ]
            };
            Box::pin(futures::stream::iter(events))
        }

        fn provider(&self) -> Provider {
            Provider::Anthropic
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    fn run_test<F, Fut>(make: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()>,
    {
        meerkat_runtime::host_stack::HostStackBudget::default_budget()
            .run("incident-demo-test", || async {
                tokio::time::timeout(Duration::from_secs(20), make())
                    .await
                    .unwrap();
            })
            .unwrap();
    }

    #[test]
    fn actual_demo_streams_three_turns_with_history_and_archives() {
        run_test(|| async {
            let directory = tempfile::tempdir_in(".").unwrap();
            let path = directory.path().to_owned();
            let client = Arc::new(IncidentClient::default());
            let mut output = Vec::new();
            run_with_client(client.clone(), &path, &mut output)
                .await
                .unwrap();
            let requests = client.requests.lock().unwrap();
            assert_eq!(requests.len(), 3);
            for (index, request) in requests.iter().enumerate() {
                assert_eq!(
                    request
                        .messages
                        .iter()
                        .filter(|m| matches!(m, Message::User(_)))
                        .count(),
                    index + 1,
                );
                assert_eq!(
                    request
                        .messages
                        .iter()
                        .filter(|m| matches!(m, Message::BlockAssistant(_)))
                        .count(),
                    index,
                );
                let transcript = serde_json::to_string(&request.messages).unwrap();
                assert!(transcript.contains("CPU usage on prod-web-03"));
                for prior in 1..=index {
                    assert!(transcript.contains(&format!("incident-answer-{prior}")));
                }
                if index > 0 {
                    assert!(transcript.contains("[MONITORING EVENT]"));
                }
                if index > 1 {
                    assert!(transcript.contains("[RESOLUTION EVENT]"));
                }
            }
            let output = String::from_utf8(output).unwrap();
            for turn in 1..=3 {
                assert!(output.contains(&format!("Turn {turn} response: incident-answer-{turn}")));
            }
            assert_eq!(output.matches("1 text deltas").count(), 3);
            assert_eq!(output.matches("1 turns started, 1 completed").count(), 3);
            assert!(output.contains("Final session: 7 messages"));
            assert!(output.contains("Session archived (task stopped)."));
            drop(directory);
            assert!(!path.exists());
        });
    }

    #[test]
    fn actual_session_cleanup_and_failure_are_observable_from_service_owner() {
        run_test(|| async {
            for fail_on in [None, Some(1), Some(2), Some(3)] {
                let directory = tempfile::tempdir_in(".").unwrap();
                let client = Arc::new(IncidentClient {
                    fail_on,
                    ..Default::default()
                });
                let service = build_service(client.clone(), directory.path()).unwrap();
                let mut output = Vec::new();
                let result = run_session(&service, &mut output).await;
                assert_eq!(client.requests.lock().unwrap().len(), fail_on.unwrap_or(3));
                if let Some(turn) = fail_on {
                    let error = result.unwrap_err();
                    assert!(
                        error.downcast_ref::<meerkat::SessionError>().is_some(),
                        "{error:?}"
                    );
                    let output = String::from_utf8(output).unwrap();
                    assert!(!output.contains(&format!("Turn {turn} response:")));
                    assert!(!output.contains("Session archived (task stopped)."));
                } else {
                    result.unwrap();
                }
                assert!(
                    service
                        .list(meerkat_core::service::SessionQuery::default())
                        .await
                        .unwrap()
                        .is_empty(),
                    "the real session owner must contain no live demo sessions after cleanup"
                );
            }
        });
    }
}
