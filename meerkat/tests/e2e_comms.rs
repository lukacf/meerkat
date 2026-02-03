#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! End-to-end tests for Meerkat inter-agent communication (Gate 1: E2E Scenarios)
//!
//! These tests verify complete agent-to-agent communication flows through the system.
//! They use real LLM providers and test the full comms stack.
//!
//! These tests require API keys and make real API calls.
//! When ANTHROPIC_API_KEY is missing, the tests will skip themselves at runtime.
//!
//! Run with:
//!   ANTHROPIC_API_KEY=... cargo test --package meerkat --test e2e_comms

use async_trait::async_trait;
use futures::StreamExt;
use meerkat::*;
use meerkat_comms::agent::{
    CommsAgent, CommsManager, CommsManagerConfig, CommsToolDispatcher, spawn_tcp_listener,
};
use meerkat_comms::{CommsConfig, Keypair, TrustedPeer, TrustedPeers};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::RwLock;

// ============================================================================
// ADAPTERS - Bridge LlmClient/SessionStore to Agent traits
// ============================================================================

/// Adapter that wraps an LlmClient to implement AgentLlmClient
pub struct LlmClientAdapter<C: LlmClient> {
    client: Arc<C>,
    model: String,
}

impl<C: LlmClient> LlmClientAdapter<C> {
    pub fn new(client: Arc<C>, model: String) -> Self {
        Self { client, model }
    }
}

#[async_trait]
impl<C: LlmClient + 'static> AgentLlmClient for LlmClientAdapter<C> {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&serde_json::Value>,
    ) -> Result<LlmStreamResult, AgentError> {
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

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut tool_call_buffers: HashMap<String, ToolCallBuffer> = HashMap::new();
        let mut stop_reason = StopReason::EndTurn;
        let mut usage = Usage::default();

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => match event {
                    LlmEvent::TextDelta { delta } => {
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
                        tool_calls.push(ToolCall::new(id, name, args));
                    }
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
                    tool_calls.push(tc);
                }
            }
        }

        Ok(LlmStreamResult::new(
            content,
            tool_calls,
            stop_reason,
            usage,
        ))
    }

    fn provider(&self) -> &'static str {
        self.client.provider()
    }
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
        let args: Value = serde_json::from_str(&self.args_json).ok()?;
        Some(ToolCall::new(self.id.clone(), name.clone(), args))
    }
}

/// Adapter that wraps a SessionStore to implement AgentSessionStore
pub struct SessionStoreAdapter<S: SessionStore> {
    store: Arc<S>,
}

impl<S: SessionStore> SessionStoreAdapter<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl<S: SessionStore + 'static> AgentSessionStore for SessionStoreAdapter<S> {
    async fn save(&self, session: &Session) -> Result<(), AgentError> {
        self.store
            .save(session)
            .await
            .map_err(|e| AgentError::StoreError(e.to_string()))
    }

    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError> {
        let session_id = SessionId::parse(id)
            .map_err(|e| AgentError::StoreError(format!("Invalid session ID: {}", e)))?;

        self.store
            .load(&session_id)
            .await
            .map_err(|e| AgentError::StoreError(e.to_string()))
    }
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

fn skip_if_no_anthropic_key() -> Option<String> {
    if std::env::var("MEERKAT_LIVE_API_TESTS").ok().as_deref() != Some("1") {
        return None;
    }
    std::env::var("RKAT_ANTHROPIC_API_KEY")
        .ok()
        .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
}

/// Get the Anthropic model to use in tests (configurable via ANTHROPIC_MODEL env var)
fn anthropic_model() -> String {
    std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string())
}

/// Create a store adapter using JsonlStore with a temp directory
async fn create_temp_store() -> (
    Arc<JsonlStore>,
    Arc<SessionStoreAdapter<JsonlStore>>,
    TempDir,
) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let store = JsonlStore::new(temp_dir.path().to_path_buf());
    store.init().await.expect("Failed to init store");
    let store = Arc::new(store);
    let adapter = Arc::new(SessionStoreAdapter::new(store.clone()));
    (store, adapter, temp_dir)
}

/// Create a pair of agents that can communicate with each other.
/// Returns (agent_a, agent_b, listener_a_handle, listener_b_handle)
async fn create_agent_pair(
    api_key: &str,
) -> (
    CommsAgent<
        LlmClientAdapter<AnthropicClient>,
        CommsToolDispatcher,
        SessionStoreAdapter<JsonlStore>,
    >,
    CommsAgent<
        LlmClientAdapter<AnthropicClient>,
        CommsToolDispatcher,
        SessionStoreAdapter<JsonlStore>,
    >,
    meerkat_comms::agent::ListenerHandle,
    meerkat_comms::agent::ListenerHandle,
    TempDir,
    TempDir,
) {
    // Generate keypairs for both agents
    let keypair_a = Keypair::generate();
    let keypair_b = Keypair::generate();
    let pubkey_a = keypair_a.public_key();
    let pubkey_b = keypair_b.public_key();

    // Find available ports
    let listener_a = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr_a = listener_a.local_addr().unwrap();
    drop(listener_a);

    let listener_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr_b = listener_b.local_addr().unwrap();
    drop(listener_b);

    // Create trusted peers lists
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

    // Create CommsManagers
    let config_a = CommsManagerConfig::with_keypair(keypair_a)
        .trusted_peers(trusted_for_a.clone())
        .comms_config(CommsConfig::default());
    let comms_manager_a = CommsManager::new(config_a).unwrap();

    let config_b = CommsManagerConfig::with_keypair(keypair_b)
        .trusted_peers(trusted_for_b.clone())
        .comms_config(CommsConfig::default());
    let comms_manager_b = CommsManager::new(config_b).unwrap();

    // Create shared trusted peers (allows dynamic updates)
    let trusted_a_shared = Arc::new(RwLock::new(trusted_for_a));
    let trusted_b_shared = Arc::new(RwLock::new(trusted_for_b));

    // Start TCP listeners
    let handle_a = spawn_tcp_listener(
        &addr_a.to_string(),
        comms_manager_a.keypair_arc(),
        trusted_a_shared.clone(),
        comms_manager_a.inbox_sender().clone(),
    )
    .await
    .expect("Failed to start listener A");

    let handle_b = spawn_tcp_listener(
        &addr_b.to_string(),
        comms_manager_b.keypair_arc(),
        trusted_b_shared.clone(),
        comms_manager_b.inbox_sender().clone(),
    )
    .await
    .expect("Failed to start listener B");

    // Give listeners time to start
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Create LLM clients
    let llm_client_a = Arc::new(AnthropicClient::new(api_key.to_string()).unwrap());
    let llm_adapter_a = Arc::new(LlmClientAdapter::new(llm_client_a, anthropic_model()));

    let llm_client_b = Arc::new(AnthropicClient::new(api_key.to_string()).unwrap());
    let llm_adapter_b = Arc::new(LlmClientAdapter::new(llm_client_b, anthropic_model()));

    // Create tool dispatchers
    let tools_a = Arc::new(CommsToolDispatcher::new(
        comms_manager_a.router().clone(),
        trusted_a_shared,
    ));

    let tools_b = Arc::new(CommsToolDispatcher::new(
        comms_manager_b.router().clone(),
        trusted_b_shared,
    ));

    // Create stores
    let (_store_a, store_adapter_a, temp_dir_a) = create_temp_store().await;
    let (_store_b, store_adapter_b, temp_dir_b) = create_temp_store().await;

    // Build agents
    let agent_a_inner = AgentBuilder::new()
        .model(anthropic_model())
        .max_tokens_per_turn(1024)
        .system_prompt(
            "You are Agent A. You can communicate with Agent B using the send_message tool. \
             When you receive a message from another agent, acknowledge it and respond appropriately. \
             Use the list_peers tool to see available peers.",
        )
        .build(llm_adapter_a, tools_a, store_adapter_a)
        .await;

    let agent_b_inner = AgentBuilder::new()
        .model(anthropic_model())
        .max_tokens_per_turn(1024)
        .system_prompt(
            "You are Agent B. You can communicate with Agent A using the send_message tool. \
             When you receive a message from another agent, acknowledge it and respond appropriately. \
             Use the list_peers tool to see available peers.",
        )
        .build(llm_adapter_b, tools_b, store_adapter_b)
        .await;

    let agent_a = CommsAgent::new(agent_a_inner, comms_manager_a);
    let agent_b = CommsAgent::new(agent_b_inner, comms_manager_b);

    (agent_a, agent_b, handle_a, handle_b, temp_dir_a, temp_dir_b)
}

// ============================================================================
// E2E: TWO AGENT MESSAGE EXCHANGE
// ============================================================================

/// E2E: Two agents exchange messages with LLM reasoning
mod two_agent_message {
    use super::*;

    #[tokio::test]
    async fn test_e2e_llm_message_exchange() {
        let Some(api_key) = skip_if_no_anthropic_key() else {
            eprintln!("Skipping: live API tests disabled or missing ANTHROPIC_API_KEY");
            return;
        };

        let (mut agent_a, mut agent_b, handle_a, handle_b, _temp_a, _temp_b) =
            create_agent_pair(&api_key).await;

        // Agent A sends a message to Agent B
        let result_a = agent_a
            .run("Send a greeting message to agent-b saying 'Hello from Agent A!'".to_string())
            .await
            .expect("Agent A run should succeed");

        eprintln!("Agent A response: {}", result_a.text);
        assert!(
            result_a.tool_calls > 0,
            "Agent A should have made tool calls"
        );

        // Give time for message to be delivered
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Agent B processes the incoming message
        let result_b = agent_b
            .run(String::new())
            .await
            .expect("Agent B inbox processing should succeed");

        eprintln!("Agent B response: {}", result_b.text);
        assert!(
            result_b.text.to_lowercase().contains("agent a")
                || result_b.text.to_lowercase().contains("hello")
                || result_b.text.to_lowercase().contains("greeting")
                || result_b.text.to_lowercase().contains("acknowledged")
                || result_b.text.to_lowercase().contains("message"),
            "Agent B should acknowledge the message: {}",
            result_b.text
        );

        // Cleanup
        handle_a.abort();
        handle_b.abort();
    }
}

// ============================================================================
// E2E: REQUEST/RESPONSE FLOW
// ============================================================================

/// E2E: Agent A sends Request, Agent B processes and responds
mod request_response {
    use super::*;

    #[tokio::test]
    async fn test_e2e_llm_request_response() {
        let Some(api_key) = skip_if_no_anthropic_key() else {
            eprintln!("Skipping: live API tests disabled or missing ANTHROPIC_API_KEY");
            return;
        };

        let (mut agent_a, mut agent_b, handle_a, handle_b, _temp_a, _temp_b) =
            create_agent_pair(&api_key).await;

        // Agent A sends a request to Agent B
        let result_a = agent_a
            .run(
                "Send a request to agent-b with intent 'calculate' and params {\"x\": 5, \"y\": 3}. \
                 Use the send_request tool."
                    .to_string(),
            )
            .await
            .expect("Agent A run should succeed");

        eprintln!("Agent A (request) response: {}", result_a.text);
        assert!(
            result_a.tool_calls > 0,
            "Agent A should have used send_request"
        );

        // Give time for message to be delivered
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Agent B processes the request and responds
        let result_b = agent_b
            .run(String::new())
            .await
            .expect("Agent B inbox processing should succeed");

        eprintln!("Agent B (processing request) response: {}", result_b.text);
        // Agent B should acknowledge receiving a request
        assert!(
            result_b.text.to_lowercase().contains("request")
                || result_b.text.to_lowercase().contains("calculate")
                || result_b.text.to_lowercase().contains("received"),
            "Agent B should acknowledge the request: {}",
            result_b.text
        );

        // Cleanup
        handle_a.abort();
        handle_b.abort();
    }
}

// ============================================================================
// E2E: MULTI-TURN CONVERSATION BETWEEN AGENTS
// ============================================================================

/// E2E: Multi-turn conversation between two agents
mod multi_turn_comms {
    use super::*;

    #[tokio::test]
    async fn test_e2e_llm_multi_turn() {
        let Some(api_key) = skip_if_no_anthropic_key() else {
            eprintln!("Skipping: live API tests disabled or missing ANTHROPIC_API_KEY");
            return;
        };

        let (mut agent_a, mut agent_b, handle_a, handle_b, _temp_a, _temp_b) =
            create_agent_pair(&api_key).await;

        // Turn 1: Agent A sends initial message
        let result_a1 = agent_a
            .run("Send a message to agent-b asking 'What is your name?'".to_string())
            .await
            .expect("Agent A turn 1 should succeed");
        eprintln!("Turn 1 - Agent A: {}", result_a1.text);

        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Turn 2: Agent B receives and responds
        let result_b1 = agent_b
            .run(String::new())
            .await
            .expect("Agent B turn 1 should succeed");
        eprintln!("Turn 2 - Agent B: {}", result_b1.text);

        // Agent B sends a follow-up
        let result_b2 = agent_b
            .run("Send a message to agent-a saying 'I am Agent B. What is your name?'".to_string())
            .await
            .expect("Agent B turn 2 should succeed");
        eprintln!("Turn 3 - Agent B: {}", result_b2.text);

        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Turn 3: Agent A receives and responds
        let result_a2 = agent_a
            .run(String::new())
            .await
            .expect("Agent A turn 2 should succeed");
        eprintln!("Turn 4 - Agent A: {}", result_a2.text);
        assert!(
            result_a2.text.to_lowercase().contains("agent b")
                || result_a2.text.to_lowercase().contains("name")
                || result_a2.text.to_lowercase().contains("acknowledging")
                || result_a2.text.to_lowercase().contains("communication"),
            "Agent A should acknowledge Agent B's response: {}",
            result_a2.text
        );

        // Cleanup
        handle_a.abort();
        handle_b.abort();
    }
}

// ============================================================================
// E2E: THREE AGENT COORDINATION
// ============================================================================

/// E2E: Three agents coordinate on a task
mod three_agent_coordination {
    use super::*;

    #[tokio::test]
    async fn test_e2e_llm_three_agent_coordination() {
        let Some(api_key) = skip_if_no_anthropic_key() else {
            eprintln!("Skipping: live API tests disabled or missing ANTHROPIC_API_KEY");
            return;
        };

        // Generate keypairs for all three agents
        let keypair_a = Keypair::generate();
        let keypair_b = Keypair::generate();
        let keypair_c = Keypair::generate();
        let pubkey_a = keypair_a.public_key();
        let pubkey_b = keypair_b.public_key();
        let pubkey_c = keypair_c.public_key();

        // Find available ports
        let listener_a = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_a = listener_a.local_addr().unwrap();
        drop(listener_a);

        let listener_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_b = listener_b.local_addr().unwrap();
        drop(listener_b);

        let listener_c = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_c = listener_c.local_addr().unwrap();
        drop(listener_c);

        // Create trusted peers lists (each agent knows about the other two)
        let trusted_for_a = TrustedPeers {
            peers: vec![
                TrustedPeer {
                    name: "agent-b".to_string(),
                    pubkey: pubkey_b,
                    addr: format!("tcp://{}", addr_b),
                },
                TrustedPeer {
                    name: "agent-c".to_string(),
                    pubkey: pubkey_c,
                    addr: format!("tcp://{}", addr_c),
                },
            ],
        };

        let trusted_for_b = TrustedPeers {
            peers: vec![
                TrustedPeer {
                    name: "agent-a".to_string(),
                    pubkey: pubkey_a,
                    addr: format!("tcp://{}", addr_a),
                },
                TrustedPeer {
                    name: "agent-c".to_string(),
                    pubkey: pubkey_c,
                    addr: format!("tcp://{}", addr_c),
                },
            ],
        };

        let trusted_for_c = TrustedPeers {
            peers: vec![
                TrustedPeer {
                    name: "agent-a".to_string(),
                    pubkey: pubkey_a,
                    addr: format!("tcp://{}", addr_a),
                },
                TrustedPeer {
                    name: "agent-b".to_string(),
                    pubkey: pubkey_b,
                    addr: format!("tcp://{}", addr_b),
                },
            ],
        };

        // Create CommsManagers
        let config_a =
            CommsManagerConfig::with_keypair(keypair_a).trusted_peers(trusted_for_a.clone());
        let comms_manager_a = CommsManager::new(config_a).unwrap();

        let config_b =
            CommsManagerConfig::with_keypair(keypair_b).trusted_peers(trusted_for_b.clone());
        let comms_manager_b = CommsManager::new(config_b).unwrap();

        let config_c =
            CommsManagerConfig::with_keypair(keypair_c).trusted_peers(trusted_for_c.clone());
        let comms_manager_c = CommsManager::new(config_c).unwrap();

        // Create shared trusted peers (allows dynamic updates)
        let trusted_a_shared = Arc::new(RwLock::new(trusted_for_a));
        let trusted_b_shared = Arc::new(RwLock::new(trusted_for_b));
        let trusted_c_shared = Arc::new(RwLock::new(trusted_for_c));

        let handle_a = spawn_tcp_listener(
            &addr_a.to_string(),
            comms_manager_a.keypair_arc(),
            trusted_a_shared.clone(),
            comms_manager_a.inbox_sender().clone(),
        )
        .await
        .expect("Failed to start listener A");

        let handle_b = spawn_tcp_listener(
            &addr_b.to_string(),
            comms_manager_b.keypair_arc(),
            trusted_b_shared.clone(),
            comms_manager_b.inbox_sender().clone(),
        )
        .await
        .expect("Failed to start listener B");

        let handle_c = spawn_tcp_listener(
            &addr_c.to_string(),
            comms_manager_c.keypair_arc(),
            trusted_c_shared.clone(),
            comms_manager_c.inbox_sender().clone(),
        )
        .await
        .expect("Failed to start listener C");

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Create LLM clients and tool dispatchers
        let llm_client_a = Arc::new(AnthropicClient::new(api_key.to_string()).unwrap());
        let llm_adapter_a = Arc::new(LlmClientAdapter::new(llm_client_a, anthropic_model()));
        let tools_a = Arc::new(CommsToolDispatcher::new(
            comms_manager_a.router().clone(),
            trusted_a_shared,
        ));

        let llm_client_b = Arc::new(AnthropicClient::new(api_key.to_string()).unwrap());
        let llm_adapter_b = Arc::new(LlmClientAdapter::new(llm_client_b, anthropic_model()));
        let tools_b = Arc::new(CommsToolDispatcher::new(
            comms_manager_b.router().clone(),
            trusted_b_shared,
        ));

        let llm_client_c = Arc::new(AnthropicClient::new(api_key.to_string()).unwrap());
        let llm_adapter_c = Arc::new(LlmClientAdapter::new(llm_client_c, anthropic_model()));
        let tools_c = Arc::new(CommsToolDispatcher::new(
            comms_manager_c.router().clone(),
            trusted_c_shared,
        ));

        // Create stores
        let (_store_a, store_adapter_a, _temp_a) = create_temp_store().await;
        let (_store_b, store_adapter_b, _temp_b) = create_temp_store().await;
        let (_store_c, store_adapter_c, _temp_c) = create_temp_store().await;

        // Build agents
        let agent_a_inner = AgentBuilder::new()
            .model(anthropic_model())
            .max_tokens_per_turn(1024)
            .system_prompt(
                "You are Agent A, the coordinator. You can communicate with Agent B and Agent C. \
                 Use list_peers to see available peers and send_message to communicate.",
            )
            .build(llm_adapter_a, tools_a, store_adapter_a)
            .await;

        let agent_b_inner = AgentBuilder::new()
            .model(anthropic_model())
            .max_tokens_per_turn(1024)
            .system_prompt(
                "You are Agent B, a worker. You can communicate with Agent A and Agent C.",
            )
            .build(llm_adapter_b, tools_b, store_adapter_b)
            .await;

        let agent_c_inner = AgentBuilder::new()
            .model(anthropic_model())
            .max_tokens_per_turn(1024)
            .system_prompt(
                "You are Agent C, a worker. You can communicate with Agent A and Agent B.",
            )
            .build(llm_adapter_c, tools_c, store_adapter_c)
            .await;

        let mut agent_a = CommsAgent::new(agent_a_inner, comms_manager_a);
        let mut agent_b = CommsAgent::new(agent_b_inner, comms_manager_b);
        let mut agent_c = CommsAgent::new(agent_c_inner, comms_manager_c);

        // Agent A broadcasts to both B and C
        let result_a = agent_a
            .run(
                "Send a message to both agent-b and agent-c saying 'Team meeting in 5 minutes!'"
                    .to_string(),
            )
            .await
            .expect("Agent A broadcast should succeed");

        eprintln!("Agent A (coordinator): {}", result_a.text);
        assert!(
            result_a.tool_calls >= 2,
            "Agent A should have sent messages to at least 2 agents"
        );

        // Give time for messages to be delivered
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

        // Agent B processes
        let result_b = agent_b
            .run(String::new())
            .await
            .expect("Agent B should process");
        eprintln!("Agent B received: {}", result_b.text);

        // Agent C processes
        let result_c = agent_c
            .run(String::new())
            .await
            .expect("Agent C should process");
        eprintln!("Agent C received: {}", result_c.text);

        // Cleanup
        handle_a.abort();
        handle_b.abort();
        handle_c.abort();
    }
}

// ============================================================================
// BASIC SANITY CHECKS (non-API tests)
// ============================================================================

/// Basic sanity checks that don't require API keys
mod sanity {
    use super::*;

    #[test]
    fn test_comms_manager_creation() {
        let keypair = Keypair::generate();
        let config = CommsManagerConfig::with_keypair(keypair);
        let manager = CommsManager::new(config).unwrap();

        // Verify manager is functional
        let _ = manager.keypair();
        let _ = manager.router();
    }

    #[test]
    fn test_trusted_peers_construction() {
        let keypair = Keypair::generate();
        let peers = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "test-peer".to_string(),
                pubkey: keypair.public_key(),
                addr: "tcp://127.0.0.1:4200".to_string(),
            }],
        };
        assert_eq!(peers.peers.len(), 1);
        assert_eq!(peers.peers[0].name, "test-peer");
    }

    #[tokio::test]
    async fn test_comms_tool_dispatcher_has_tools() {
        let keypair = Keypair::generate();
        let peer_keypair = Keypair::generate();
        let trusted = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "peer".to_string(),
                pubkey: peer_keypair.public_key(),
                addr: "tcp://127.0.0.1:4200".to_string(),
            }],
        };

        let config = CommsManagerConfig::with_keypair(keypair).trusted_peers(trusted.clone());
        let manager = CommsManager::new(config).unwrap();

        let trusted = Arc::new(RwLock::new(trusted));
        let dispatcher = CommsToolDispatcher::new(manager.router().clone(), trusted);

        let tools = dispatcher.tools();
        assert!(!tools.is_empty(), "Should have comms tools");

        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(tool_names.contains(&"send_message"));
        assert!(tool_names.contains(&"send_request"));
        assert!(tool_names.contains(&"send_response"));
        assert!(tool_names.contains(&"list_peers"));
    }
}
