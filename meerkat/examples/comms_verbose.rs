#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Verbose diagnostic for comms flow - shows actual API calls, tool calls, and network traffic

use async_trait::async_trait;
use futures::StreamExt;
use meerkat::*;
use meerkat_core::{ToolCallView, ToolResult};
use meerkat_comms::agent::{
    CommsAgent, CommsManager, CommsManagerConfig, CommsToolDispatcher, spawn_tcp_listener,
};
use meerkat_comms::{Keypair, TrustedPeer, TrustedPeers};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::RwLock;

static API_CALL_COUNT: AtomicUsize = AtomicUsize::new(0);
static TOOL_CALL_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Adapter that wraps an LlmClient to implement AgentLlmClient with logging
pub struct LoggingLlmAdapter {
    client: AnthropicClient,
    model: String,
    agent_name: String,
}

#[derive(Debug, Default)]
struct ToolCallBuffer {
    id: String,
    name: Option<String>,
    args_json: String,
}

impl ToolCallBuffer {
    fn new(id: String) -> Self {
        Self {
            id,
            name: None,
            args_json: String::new(),
        }
    }

    fn try_complete(&self) -> Option<ToolCall> {
        let name = self.name.as_ref()?;
        let args: serde_json::Value = serde_json::from_str(&self.args_json).ok()?;
        Some(ToolCall::new(self.id.clone(), name.clone(), args))
    }
}

#[async_trait]
impl AgentLlmClient for LoggingLlmAdapter {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&serde_json::Value>,
    ) -> Result<LlmStreamResult, AgentError> {
        let call_num = API_CALL_COUNT.fetch_add(1, Ordering::SeqCst) + 1;

        println!("\n╔══════════════════════════════════════════════════════════════╗");
        println!("║  ANTHROPIC API CALL #{} - {}", call_num, self.agent_name);
        println!("╠══════════════════════════════════════════════════════════════╣");
        println!("║ Model: {}", self.model);
        println!(
            "║ Tools provided: {:?}",
            tools.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
        println!("║ Messages in context: {}", messages.len());

        // Show last user message
        for msg in messages.iter().rev() {
            if let Message::User(u) = msg {
                let preview: String = u.content.chars().take(150).collect();
                println!("║ Last user message: {}...", preview);
                break;
            }
        }
        println!("╚══════════════════════════════════════════════════════════════╝");

        let request = LlmRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            max_tokens,
            temperature,
            stop_sequences: None,
            provider_params: provider_params.cloned(),
        };

        let mut stream = self.client.stream(&request);
        // ...

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut tool_call_buffers: HashMap<String, ToolCallBuffer> = HashMap::new();
        let mut stop_reason = StopReason::EndTurn;
        let mut usage = Usage::default();

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => match event {
                    LlmEvent::TextDelta { delta, .. } => {
                        content.push_str(&delta);
                    }
                    LlmEvent::ToolCallDelta {
                        id,
                        name,
                        args_delta,
                    } => {
                        let buffer = tool_call_buffers
                            .entry(id.clone())
                            .or_insert_with(|| ToolCallBuffer::new(id));

                        if let Some(n) = name {
                            buffer.name = Some(n);
                        }
                        buffer.args_json.push_str(&args_delta);
                    }
                    LlmEvent::ToolCallComplete { id, name, args, .. } => {
                        println!("\n┌─── LLM REQUESTED TOOL: {} ───┐", name);
                        println!(
                            "│ Args: {}",
                            serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string())
                        );
                        println!("└────────────────────────────────────────┘");
                        tool_calls.push(ToolCall::new(id, name, args));
                    }
                    LlmEvent::ReasoningDelta { .. } | LlmEvent::ReasoningComplete { .. } => {}
                    LlmEvent::UsageUpdate { usage: u } => {
                        usage = u;
                    }
                    LlmEvent::Done { outcome } => match outcome {
                        LlmDoneOutcome::Success { stop_reason: sr } => {
                            stop_reason = sr;
                        }
                        LlmDoneOutcome::Error { error } => {
                            return Err(AgentError::llm(
                                self.client.provider(),
                                error.failure_reason(),
                                error.to_string(),
                            ));
                        }
                    },
                },
                Err(e) => {
                    return Err(AgentError::llm(
                        self.client.provider(),
                        e.failure_reason(),
                        e.to_string(),
                    ));
                }
            }
        }

        // Complete any buffered tool calls
        for (_, buffer) in tool_call_buffers {
            if let Some(tc) = buffer.try_complete() {
                if !tool_calls.iter().any(|t| t.id == tc.id) {
                    println!("\n┌─── LLM REQUESTED TOOL: {} ───┐", tc.name);
                    println!(
                        "│ Args: {}",
                        serde_json::to_string(&tc.args).unwrap_or_else(|_| "{}".to_string())
                    );
                    println!("└────────────────────────────────────────┘");
                    tool_calls.push(tc);
                }
            }
        }

        println!("\n>>> {} LLM response: {}", self.agent_name, content);
        println!(
            ">>> {} tool calls requested: {}",
            self.agent_name,
            tool_calls.len()
        );

        let mut blocks = Vec::new();
        if !content.is_empty() {
            blocks.push(meerkat_core::AssistantBlock::Text {
                text: content,
                meta: None,
            });
        }
        for tc in tool_calls {
            let args_raw = serde_json::value::RawValue::from_string(
                serde_json::to_string(&tc.args).unwrap_or_else(|_| "{}".to_string()),
            )
            .unwrap_or_else(|_| serde_json::value::RawValue::from_string("{}".to_string()).unwrap());
            blocks.push(meerkat_core::AssistantBlock::ToolUse {
                id: tc.id,
                name: tc.name,
                args: args_raw,
                meta: None,
            });
        }
        Ok(LlmStreamResult::new(blocks, stop_reason, usage))
    }

    fn provider(&self) -> &'static str {
        "anthropic"
    }
}

/// Wrapper to log tool dispatches
struct LoggingToolDispatcher {
    inner: CommsToolDispatcher,
    agent_name: String,
}

#[async_trait]
impl AgentToolDispatcher for LoggingToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.inner.tools()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        let call_num = TOOL_CALL_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        let args_value: serde_json::Value = serde_json::from_str(call.args.get())
            .unwrap_or_else(|_| serde_json::Value::String(call.args.get().to_string()));

        println!("\n┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓");
        println!(
            "┃  TOOL EXECUTION #{} - {} calling '{}'",
            call_num, self.agent_name, call.name
        );
        println!("┣━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┫");
        println!(
            "┃ Args: {}",
            serde_json::to_string_pretty(&args_value).unwrap_or_else(|_| "{}".to_string())
        );

        let result = self.inner.dispatch(call).await;

        match &result {
            Ok(r) => {
                println!("┃ SUCCESS: {}", r.content);
            }
            Err(e) => {
                println!("┃ ERROR: {}", e);
            }
        }
        println!("┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛");

        result
    }
}

struct NoStore;

#[async_trait]
impl AgentSessionStore for NoStore {
    async fn save(&self, _: &Session) -> Result<(), AgentError> {
        Ok(())
    }
    async fn load(&self, _: &str) -> Result<Option<Session>, AgentError> {
        Ok(None)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // We need an API key
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "ANTHROPIC_API_KEY environment variable must be set")?;

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║     MEERKAT COMMS VERBOSE DIAGNOSTIC                         ║");
    println!("║     Two separate agent instances communicating via TCP       ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // Generate keypairs for two independent agents
    let keypair_a = Keypair::generate();
    let keypair_b = Keypair::generate();
    let pubkey_a = keypair_a.public_key();
    let pubkey_b = keypair_b.public_key();

    println!("=== IDENTITY SETUP (separate keypairs for each agent) ===");
    println!("Agent A pubkey: {:?}", pubkey_a);
    println!("Agent B pubkey: {:?}", pubkey_b);

    // Each agent gets its own TCP port
    let listener_a = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr_a = listener_a.local_addr()?;
    drop(listener_a);

    let listener_b = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr_b = listener_b.local_addr()?;
    drop(listener_b);

    println!("\n=== NETWORK SETUP (each agent has own TCP listener) ===");
    println!("Agent A listening on: tcp://{}", addr_a);
    println!("Agent B listening on: tcp://{}", addr_b);

    // Each agent knows about the other as a trusted peer
    let trusted_for_a = TrustedPeers {
        peers: vec![TrustedPeer {
            name: "agent-b".to_string(),
            pubkey: pubkey_b,
            addr: format!("tcp://{}", addr_b),
        }],
    };

    let trusted_for_b = TrustedPeers {
        peers: vec![TrustedPeer {
            name: "agent-a".to_string(),
            pubkey: pubkey_a,
            addr: format!("tcp://{}", addr_a),
        }],
    };

    // Create separate CommsManagers (each agent has its own)
    println!("\n=== CREATING SEPARATE COMMS MANAGERS ===");
    let config_a = CommsManagerConfig::with_keypair(keypair_a).trusted_peers(trusted_for_a.clone());
    let comms_manager_a = CommsManager::new(config_a)?;
    println!("Agent A CommsManager created");

    let config_b = CommsManagerConfig::with_keypair(keypair_b).trusted_peers(trusted_for_b.clone());
    let comms_manager_b = CommsManager::new(config_b)?;
    println!("Agent B CommsManager created");

    // Start TCP listeners (these accept incoming connections)
    println!("\n=== STARTING TCP LISTENERS ===");

    // Create shared trusted peers for each agent (allows dynamic updates)
    let trusted_a_shared = Arc::new(RwLock::new(trusted_for_a.clone()));
    let trusted_b_shared = Arc::new(RwLock::new(trusted_for_b.clone()));

    let _handle_a = spawn_tcp_listener(
        &addr_a.to_string(),
        comms_manager_a.keypair_arc(),
        trusted_a_shared.clone(),
        comms_manager_a.inbox_sender().clone(),
    )
    .await?;
    println!("Agent A TCP listener started on {}", addr_a);

    let _handle_b = spawn_tcp_listener(
        &addr_b.to_string(),
        comms_manager_b.keypair_arc(),
        trusted_b_shared.clone(),
        comms_manager_b.inbox_sender().clone(),
    )
    .await?;
    println!("Agent B TCP listener started on {}", addr_b);

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Show available tools
    let tools_check =
        CommsToolDispatcher::new(comms_manager_a.router().clone(), trusted_a_shared.clone());
    println!("\n=== COMMS TOOLS AVAILABLE TO EACH AGENT ===");
    for tool in tools_check.tools().iter() {
        println!("  • {} - {}", tool.name, tool.description);
    }

    // Create logging tool dispatchers (separate for each agent)
    let tools_a = Arc::new(LoggingToolDispatcher {
        inner: CommsToolDispatcher::new(comms_manager_a.router().clone(), trusted_a_shared),
        agent_name: "Agent A".to_string(),
    });

    let tools_b = Arc::new(LoggingToolDispatcher {
        inner: CommsToolDispatcher::new(comms_manager_b.router().clone(), trusted_b_shared),
        agent_name: "Agent B".to_string(),
    });

    // Create separate LLM clients for each agent
    let model =
        std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-3-sonnet-20240229".to_string());
    println!("\n=== LLM SETUP ===");
    println!("Model: {}", model);
    println!("Each agent has its own AnthropicClient instance");

    let llm_a = Arc::new(LoggingLlmAdapter {
        client: AnthropicClient::new(api_key.clone())?,
        model: model.clone(),
        agent_name: "Agent A".to_string(),
    });

    let llm_b = Arc::new(LoggingLlmAdapter {
        client: AnthropicClient::new(api_key)?,
        model: model.clone(),
        agent_name: "Agent B".to_string(),
    });

    let store = Arc::new(NoStore);

    // Build separate agent instances
    println!("\n=== BUILDING SEPARATE AGENT INSTANCES ===");
    let agent_a_inner = AgentBuilder::new()
        .model(&model)
        .max_tokens_per_turn(1024)
        .system_prompt(
            "You are Agent A. You can communicate with other agents using the send_message tool. \\
             Use list_peers to see available peers.",
        )
        .build(llm_a, tools_a, store.clone())
        .await;
    println!("Agent A built with system prompt and comms tools");

    let agent_b_inner = AgentBuilder::new()
        .model(&model)
        .max_tokens_per_turn(1024)
        .system_prompt(
            "You are Agent B. When you receive messages from other agents, acknowledge them.",
        )
        .build(llm_b, tools_b, store)
        .await;
    println!("Agent B built with system prompt and comms tools");

    let mut agent_a = CommsAgent::new(agent_a_inner, comms_manager_a);
    let mut agent_b = CommsAgent::new(agent_b_inner, comms_manager_b);

    // === PHASE 1: Agent A sends message ===
    println!("\n");
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  PHASE 1: AGENT A SENDS MESSAGE TO AGENT B                   ║");
    println!("║  (This will: call Anthropic API → LLM returns tool call →    ║");
    println!("║   execute send_message → TCP connection to Agent B)          ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    let result_a = agent_a
        .run(
            "Send the message 'Hello from Agent A!' to agent-b using the send_message tool."
                .to_string(),
        )
        .await?;

    println!("\n═══════════════════════════════════════════════════════════════");
    println!("AGENT A FINAL RESULT:");
    println!("  Response text: {}", result_a.text);
    println!("  Tool calls executed: {}", result_a.tool_calls);
    println!("  Turns: {}", result_a.turns);
    println!("═══════════════════════════════════════════════════════════════");

    // Wait for TCP message delivery
    println!("\n... waiting 500ms for TCP message to be delivered to Agent B ...\n");
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // === PHASE 2: Check Agent B's inbox ===
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  PHASE 2: CHECKING AGENT B'S INBOX                           ║");
    println!("║  (Messages received via TCP from Agent A)                    ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    // Check what's in Agent B's inbox
    let inbox_messages = agent_b.comms_manager_mut().drain_messages();
    println!("\nMessages in Agent B's inbox: {}", inbox_messages.len());

    for (i, msg) in inbox_messages.iter().enumerate() {
        println!("\n  ┌─── Inbox Message #{} ───┐", i + 1);
        println!("  │ From peer: {}", msg.from_peer);
        println!("  │ Envelope ID: {}", msg.envelope_id);
        println!("  │ Content: {:?}", msg.content);
        println!("  └──────────────────────────┘");
    }

    if inbox_messages.is_empty() {
        println!("\n⚠️  NO MESSAGES IN INBOX!");
        println!("   This means the TCP delivery failed or the message wasn't sent.");
        return Ok(());
    }

    // === PHASE 3: Agent B processes inbox ===
    println!("\n");
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  PHASE 3: AGENT B PROCESSES INBOX MESSAGE                    ║");
    println!("║  (This will: inject message into prompt → call Anthropic API ║");
    println!("║   → LLM generates response)                                  ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    // Re-inject messages for processing (since we drained them to inspect)
    for msg in inbox_messages {
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

    println!("\n═══════════════════════════════════════════════════════════════");
    println!("AGENT B FINAL RESULT:");
    println!("  Response text: {}", result_b.text);
    println!("  Tool calls executed: {}", result_b.tool_calls);
    println!("  Turns: {}", result_b.turns);
    println!("═══════════════════════════════════════════════════════════════");

    println!("\n");
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║                      SUMMARY                                 ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!(
        "║  Total Anthropic API calls: {}                                ",
        API_CALL_COUNT.load(Ordering::SeqCst)
    );
    println!(
        "║  Total tool executions: {}                                    ",
        TOOL_CALL_COUNT.load(Ordering::SeqCst)
    );
    println!("║  Message successfully delivered via TCP: YES                 ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
