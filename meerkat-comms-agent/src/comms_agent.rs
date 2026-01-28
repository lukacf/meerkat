//! CommsAgent - Agent wrapper with inbox integration.
//!
//! CommsAgent wraps a Meerkat Agent and integrates the comms inbox,
//! injecting incoming messages into the session at turn boundaries.

use std::sync::Arc;

use meerkat_core::budget::BudgetLimits;
use meerkat_core::error::AgentError;
use meerkat_core::retry::RetryPolicy;
use meerkat_core::session::Session;
use meerkat_core::types::RunResult;
use meerkat_core::{Agent, AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};
use tokio::sync::watch;

use crate::manager::CommsManager;
use crate::types::CommsMessage;

/// Agent wrapper that integrates comms inbox.
///
/// On each run, CommsAgent:
/// 1. Drains the inbox for new messages
/// 2. Injects them into the user prompt
/// 3. Runs the underlying agent
pub struct CommsAgent<C, T, S>
where
    C: AgentLlmClient + 'static,
    T: AgentToolDispatcher + 'static,
    S: AgentSessionStore + 'static,
{
    /// The underlying agent.
    agent: Agent<C, T, S>,
    /// The comms manager for inbox access.
    comms_manager: CommsManager,
}

impl<C, T, S> CommsAgent<C, T, S>
where
    C: AgentLlmClient + 'static,
    T: AgentToolDispatcher + 'static,
    S: AgentSessionStore + 'static,
{
    /// Create a new CommsAgent wrapping the given agent and comms manager.
    pub fn new(agent: Agent<C, T, S>, comms_manager: CommsManager) -> Self {
        Self {
            agent,
            comms_manager,
        }
    }

    /// Get a reference to the underlying agent.
    pub fn agent(&self) -> &Agent<C, T, S> {
        &self.agent
    }

    /// Get a mutable reference to the underlying agent.
    pub fn agent_mut(&mut self) -> &mut Agent<C, T, S> {
        &mut self.agent
    }

    /// Get a reference to the comms manager.
    pub fn comms_manager(&self) -> &CommsManager {
        &self.comms_manager
    }

    /// Get a mutable reference to the comms manager.
    pub fn comms_manager_mut(&mut self) -> &mut CommsManager {
        &mut self.comms_manager
    }

    /// Run the agent with a user message, injecting any pending inbox messages.
    ///
    /// This method:
    /// 1. Drains the inbox for new comms messages
    /// 2. Prepends them to the user input
    /// 3. Runs the underlying agent
    pub async fn run(&mut self, user_input: String) -> Result<RunResult, AgentError> {
        // Drain inbox messages
        let inbox_messages = self.comms_manager.drain_messages();

        // Build the combined prompt
        let combined_input = if inbox_messages.is_empty() {
            user_input
        } else {
            let comms_text = format_inbox_messages(&inbox_messages);
            if user_input.is_empty() {
                comms_text
            } else {
                format!("{}\n\n---\n\n{}", comms_text, user_input)
            }
        };

        // Run the underlying agent
        self.agent.run(combined_input).await
    }

    /// Run the agent once, then stay alive processing incoming messages.
    ///
    /// This method:
    /// 1. Runs the initial prompt (if any)
    /// 2. Waits for incoming inbox messages
    /// 3. Processes each message as a new turn
    /// 4. Exits on a special "DISMISS" message or cancellation
    ///
    /// If `cancel_rx` is provided, any change to `true` triggers shutdown.
    pub async fn run_stay_alive(
        &mut self,
        initial_prompt: String,
        mut cancel_rx: Option<watch::Receiver<bool>>,
    ) -> Result<RunResult, AgentError> {
        let mut last_result = self.run(initial_prompt).await?;

        loop {
            // If cancellation already requested, exit cleanly.
            if let Some(ref rx) = cancel_rx {
                if *rx.borrow() {
                    return Err(AgentError::Cancelled);
                }
            }

            // Wait for a message or cancellation.
            let first_msg = if let Some(ref mut rx) = cancel_rx {
                tokio::select! {
                    _ = rx.changed() => {
                        if *rx.borrow() {
                            return Err(AgentError::Cancelled);
                        }
                        // If changed to false, continue waiting for messages.
                        continue;
                    }
                    msg = self.comms_manager.recv_message() => {
                        msg.ok_or_else(|| AgentError::ToolError("Inbox closed".to_string()))?
                    }
                }
            } else {
                self.comms_manager
                    .recv_message()
                    .await
                    .ok_or_else(|| AgentError::ToolError("Inbox closed".to_string()))?
            };

            // Drain any additional messages that arrived.
            let mut messages = vec![first_msg];
            messages.extend(self.comms_manager.drain_messages());

            // Exit if a DISMISS message is received.
            if contains_dismiss(&messages) {
                return Ok(last_result);
            }

            // Process messages as a new turn.
            let comms_text = format_inbox_messages(&messages);
            last_result = self.agent.run(comms_text).await?;
        }
    }

    /// Run the agent with only inbox messages (no user input).
    ///
    /// Useful for agent-to-agent communication where the agent is
    /// processing incoming messages autonomously.
    pub async fn run_inbox_only(&mut self) -> Result<Option<RunResult>, AgentError> {
        let inbox_messages = self.comms_manager.drain_messages();

        if inbox_messages.is_empty() {
            return Ok(None);
        }

        let comms_text = format_inbox_messages(&inbox_messages);
        let result = self.agent.run(comms_text).await?;
        Ok(Some(result))
    }

    /// Wait for a message and then run the agent.
    ///
    /// This blocks until at least one message arrives in the inbox,
    /// then processes all available messages.
    pub async fn wait_and_run(&mut self) -> Result<RunResult, AgentError> {
        // Wait for at least one message
        let first_msg = self
            .comms_manager
            .recv_message()
            .await
            .ok_or_else(|| AgentError::ToolError("Inbox closed".to_string()))?;

        // Drain any additional messages that arrived
        let mut messages = vec![first_msg];
        messages.extend(self.comms_manager.drain_messages());

        let comms_text = format_inbox_messages(&messages);
        self.agent.run(comms_text).await
    }
}

/// Return true if any message is a "DISMISS" command.
fn contains_dismiss(messages: &[CommsMessage]) -> bool {
    messages.iter().any(|m| match &m.content {
        crate::types::CommsContent::Message { body } => body.trim().eq_ignore_ascii_case("DISMISS"),
        _ => false,
    })
}

/// Format inbox messages for injection into the agent prompt.
fn format_inbox_messages(messages: &[CommsMessage]) -> String {
    messages
        .iter()
        .map(|m| m.to_user_message_text())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Builder for CommsAgent.
pub struct CommsAgentBuilder {
    model: Option<String>,
    system_prompt: Option<String>,
    max_tokens_per_turn: Option<u32>,
    budget_limits: Option<BudgetLimits>,
    retry_policy: Option<RetryPolicy>,
    session: Option<Session>,
}

impl CommsAgentBuilder {
    /// Create a new CommsAgentBuilder.
    pub fn new() -> Self {
        Self {
            model: None,
            system_prompt: None,
            max_tokens_per_turn: None,
            budget_limits: None,
            retry_policy: None,
            session: None,
        }
    }

    /// Set the model.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the system prompt.
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set max tokens per turn.
    pub fn max_tokens_per_turn(mut self, tokens: u32) -> Self {
        self.max_tokens_per_turn = Some(tokens);
        self
    }

    /// Set budget limits.
    pub fn budget(mut self, limits: BudgetLimits) -> Self {
        self.budget_limits = Some(limits);
        self
    }

    /// Set retry policy.
    pub fn retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = Some(policy);
        self
    }

    /// Resume from an existing session.
    pub fn resume_session(mut self, session: Session) -> Self {
        self.session = Some(session);
        self
    }

    /// Build the CommsAgent.
    pub fn build<C, T, S>(
        self,
        client: Arc<C>,
        tools: Arc<T>,
        store: Arc<S>,
        comms_manager: CommsManager,
    ) -> CommsAgent<C, T, S>
    where
        C: AgentLlmClient + 'static,
        T: AgentToolDispatcher + 'static,
        S: AgentSessionStore + 'static,
    {
        let mut builder = AgentBuilder::new();

        if let Some(model) = self.model {
            builder = builder.model(model);
        }
        if let Some(prompt) = self.system_prompt {
            builder = builder.system_prompt(prompt);
        }
        if let Some(tokens) = self.max_tokens_per_turn {
            builder = builder.max_tokens_per_turn(tokens);
        }
        if let Some(limits) = self.budget_limits {
            builder = builder.budget(limits);
        }
        if let Some(policy) = self.retry_policy {
            builder = builder.retry_policy(policy);
        }
        if let Some(session) = self.session {
            builder = builder.resume_session(session);
        }

        let agent = builder.build(client, tools, store);
        CommsAgent::new(agent, comms_manager)
    }
}

impl Default for CommsAgentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::CommsManagerConfig;
    use crate::types::{CommsContent, CommsStatus};
    use meerkat_comms::{
        Envelope, InboxItem, MessageKind, PubKey, Signature, TrustedPeer, TrustedPeers,
    };
    use meerkat_core::agent::LlmStreamResult;
    use meerkat_core::types::{StopReason, ToolDef, Usage};
    use std::sync::Mutex;
    use uuid::Uuid;

    // Mock LLM client for testing
    struct MockLlmClient {
        responses: Mutex<Vec<LlmStreamResult>>,
        received_prompts: Mutex<Vec<String>>,
    }

    impl MockLlmClient {
        fn new(responses: Vec<LlmStreamResult>) -> Self {
            Self {
                responses: Mutex::new(responses),
                received_prompts: Mutex::new(vec![]),
            }
        }

        fn received_prompts(&self) -> Vec<String> {
            self.received_prompts.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl AgentLlmClient for MockLlmClient {
        async fn stream_response(
            &self,
            messages: &[meerkat_core::types::Message],
            _tools: &[ToolDef],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&serde_json::Value>,
        ) -> Result<LlmStreamResult, AgentError> {
            // Record the last user message
            for msg in messages.iter().rev() {
                if let meerkat_core::types::Message::User(user_msg) = msg {
                    self.received_prompts
                        .lock()
                        .unwrap()
                        .push(user_msg.content.clone());
                    break;
                }
            }

            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                Ok(LlmStreamResult {
                    content: "Default response".to_string(),
                    tool_calls: vec![],
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                })
            } else {
                Ok(responses.remove(0))
            }
        }

        fn provider(&self) -> &'static str {
            "mock"
        }
    }

    // Mock tool dispatcher
    struct MockToolDispatcher;

    #[async_trait::async_trait]
    impl AgentToolDispatcher for MockToolDispatcher {
        fn tools(&self) -> Vec<ToolDef> {
            vec![]
        }

        async fn dispatch(
            &self,
            _name: &str,
            _args: &serde_json::Value,
        ) -> Result<serde_json::Value, meerkat_tools::ToolError> {
            Ok(serde_json::Value::String("mock result".to_string()))
        }
    }

    // Mock session store
    struct MockSessionStore;

    #[async_trait::async_trait]
    impl AgentSessionStore for MockSessionStore {
        async fn save(&self, _session: &Session) -> Result<(), AgentError> {
            Ok(())
        }

        async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
            Ok(None)
        }
    }

    fn make_keypair() -> meerkat_comms::Keypair {
        meerkat_comms::Keypair::generate()
    }

    fn make_trusted_peers(name: &str, pubkey: &PubKey) -> TrustedPeers {
        TrustedPeers {
            peers: vec![TrustedPeer {
                name: name.to_string(),
                pubkey: *pubkey,
                addr: "tcp://127.0.0.1:4200".to_string(),
            }],
        }
    }

    #[test]
    fn test_comms_agent_struct() {
        let config = CommsManagerConfig::new();
        let comms_manager = CommsManager::new(config);

        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher);
        let store = Arc::new(MockSessionStore);

        let agent = AgentBuilder::new()
            .model("test")
            .build(client, tools, store);

        let comms_agent = CommsAgent::new(agent, comms_manager);

        // Verify accessors work
        let _ = comms_agent.agent();
        let _ = comms_agent.comms_manager();
    }

    #[tokio::test]
    async fn test_comms_agent_injects_inbox() {
        let sender = make_keypair();
        let sender_pubkey = sender.public_key();
        let our_keypair = make_keypair();
        let our_pubkey = our_keypair.public_key();
        let trusted = make_trusted_peers("sender-agent", &sender_pubkey);

        let config = CommsManagerConfig::with_keypair(our_keypair).trusted_peers(trusted);
        let comms_manager = CommsManager::new(config);

        // Send a message to the inbox
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: sender_pubkey,
            to: our_pubkey,
            kind: MessageKind::Message {
                body: "Hello from sender!".to_string(),
            },
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(&sender);

        comms_manager
            .inbox_sender()
            .send(InboxItem::External { envelope })
            .unwrap();

        // Create mock LLM that records prompts
        let client = Arc::new(MockLlmClient::new(vec![LlmStreamResult {
            content: "I received the message".to_string(),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        }]));

        let tools = Arc::new(MockToolDispatcher);
        let store = Arc::new(MockSessionStore);

        let agent = AgentBuilder::new()
            .model("test")
            .build(client.clone(), tools, store);

        let mut comms_agent = CommsAgent::new(agent, comms_manager);

        // Run with user input
        let result = comms_agent.run("Process this".to_string()).await.unwrap();
        assert_eq!(result.text, "I received the message");

        // Check that the inbox message was injected
        let prompts = client.received_prompts();
        assert!(!prompts.is_empty());
        let prompt = &prompts[0];
        assert!(prompt.contains("COMMS MESSAGE"));
        assert!(prompt.contains("sender-agent"));
        assert!(prompt.contains("Hello from sender!"));
        assert!(prompt.contains("Process this"));
    }

    #[test]
    fn test_comms_agent_builder() {
        let config = CommsManagerConfig::new();
        let comms_manager = CommsManager::new(config);

        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher);
        let store = Arc::new(MockSessionStore);

        let comms_agent = CommsAgentBuilder::new()
            .model("test-model")
            .system_prompt("You are a helpful assistant")
            .max_tokens_per_turn(1000)
            .build(client, tools, store, comms_manager);

        // Should compile and create agent
        let _ = comms_agent.agent();
    }

    #[test]
    fn test_format_inbox_messages() {
        let messages = vec![
            CommsMessage {
                envelope_id: Uuid::new_v4(),
                from_peer: "agent-a".to_string(),
                from_pubkey: PubKey::new([1u8; 32]),
                content: CommsContent::Message {
                    body: "First message".to_string(),
                },
            },
            CommsMessage {
                envelope_id: Uuid::new_v4(),
                from_peer: "agent-b".to_string(),
                from_pubkey: PubKey::new([2u8; 32]),
                content: CommsContent::Response {
                    in_reply_to: Uuid::new_v4(),
                    status: CommsStatus::Completed,
                    result: serde_json::json!({"done": true}),
                },
            },
        ];

        let formatted = format_inbox_messages(&messages);

        assert!(formatted.contains("COMMS MESSAGE"));
        assert!(formatted.contains("agent-a"));
        assert!(formatted.contains("First message"));
        assert!(formatted.contains("COMMS RESPONSE"));
        assert!(formatted.contains("agent-b"));
    }
}
