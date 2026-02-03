//! Agent comms helpers (host mode).

use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::types::{Message, RunResult, Usage, UserMessage};
use tokio::sync::mpsc;

use crate::agent::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, CommsRuntime};

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized + 'static,
    T: AgentToolDispatcher + ?Sized + 'static,
    S: AgentSessionStore + ?Sized + 'static,
{
    /// Get the comms runtime, if enabled.
    pub fn comms(&self) -> Option<&dyn CommsRuntime> {
        self.comms_runtime.as_deref()
    }

    /// Drain comms inbox and inject messages into session.
    /// Returns true if any messages were injected.
    pub(super) async fn drain_comms_inbox(&mut self) -> bool {
        let comms = match &self.comms_runtime {
            Some(c) => c.clone(),
            None => return false,
        };

        let messages = comms.drain_messages().await;
        if messages.is_empty() {
            return false;
        }

        tracing::debug!("Injecting {} comms messages into session", messages.len());
        let combined = messages.join("\n\n");
        self.session
            .push(Message::User(UserMessage { content: combined }));
        true
    }

    /// Run the agent in host mode: process initial prompt, then stay alive for comms messages.
    pub async fn run_host_mode(&mut self, initial_prompt: String) -> Result<RunResult, AgentError> {
        use std::time::Duration;

        let comms = self.comms_runtime.clone().ok_or_else(|| {
            AgentError::ConfigError("Host mode requires comms to be enabled".to_string())
        })?;

        let has_pending_user_message = self
            .session
            .messages()
            .last()
            .is_some_and(|m| matches!(m, Message::User(_)));

        let mut last_result = if !initial_prompt.trim().is_empty() {
            self.run(initial_prompt).await?
        } else if has_pending_user_message {
            self.run_loop(None).await?
        } else {
            RunResult {
                text: String::new(),
                session_id: self.session.id().clone(),
                turns: 0,
                tool_calls: 0,
                usage: Usage::default(),
                structured_output: None,
            }
        };

        let inbox_notify = comms.inbox_notify();
        const POLL_INTERVAL: Duration = Duration::from_secs(60);

        loop {
            if self.budget.is_exhausted() {
                tracing::info!("Host mode: budget exhausted, exiting");
                return Ok(last_result);
            }

            let timeout = self.budget.remaining_duration().unwrap_or(POLL_INTERVAL);
            let notified = inbox_notify.notified();

            let messages = comms.drain_messages().await;

            if !messages.is_empty() {
                let combined_input = messages.join("\n\n");
                tracing::debug!("Host mode: processing {} comms message(s)", messages.len());

                match self.run(combined_input).await {
                    Ok(result) => {
                        last_result = result;
                    }
                    Err(e) => {
                        if e.is_graceful() {
                            tracing::info!("Host mode: graceful exit - {}", e);
                            return Ok(last_result);
                        }
                        return Err(e);
                    }
                }
                continue;
            }

            tokio::select! {
                _ = notified => {} //
                _ = tokio::time::sleep(timeout) => {
                    tracing::trace!("Host mode: timeout, checking budget");
                }
            }
        }
    }

    /// Run in host mode with event streaming.
    pub async fn run_host_mode_with_events(
        &mut self,
        initial_prompt: String,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        use std::time::Duration;

        let comms = self.comms_runtime.clone().ok_or_else(|| {
            AgentError::ConfigError("Host mode requires comms to be enabled".to_string())
        })?;

        let has_pending_user_message = self
            .session
            .messages()
            .last()
            .is_some_and(|m| matches!(m, Message::User(_)));

        let mut last_result = if !initial_prompt.trim().is_empty() {
            self.run_with_events(initial_prompt, event_tx.clone())
                .await?
        } else if has_pending_user_message {
            let run_prompt = self
                .session
                .messages()
                .last()
                .and_then(|msg| match msg {
                    Message::User(user) => Some(user.content.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            let _ = event_tx
                .send(AgentEvent::RunStarted {
                    session_id: self.session.id().clone(),
                    prompt: run_prompt,
                })
                .await;
            self.run_loop(Some(event_tx.clone())).await?
        } else {
            RunResult {
                text: String::new(),
                session_id: self.session.id().clone(),
                turns: 0,
                tool_calls: 0,
                usage: Usage::default(),
                structured_output: None,
            }
        };

        let inbox_notify = comms.inbox_notify();
        const POLL_INTERVAL: Duration = Duration::from_secs(60);

        loop {
            if self.budget.is_exhausted() {
                tracing::info!("Host mode: budget exhausted, exiting");
                return Ok(last_result);
            }

            let timeout = self.budget.remaining_duration().unwrap_or(POLL_INTERVAL);
            let notified = inbox_notify.notified();

            let messages = comms.drain_messages().await;

            if !messages.is_empty() {
                let combined_input = messages.join("\n\n");
                tracing::debug!("Host mode: processing {} comms message(s)", messages.len());

                match self.run_with_events(combined_input, event_tx.clone()).await {
                    Ok(result) => {
                        last_result = result;
                    }
                    Err(e) => {
                        if e.is_graceful() {
                            tracing::info!("Host mode: graceful exit - {}", e);
                            return Ok(last_result);
                        }
                        return Err(e);
                    }
                }
                continue;
            }

            tokio::select! {
                _ = notified => {} //
                _ = tokio::time::sleep(timeout) => {
                    tracing::trace!("Host mode: timeout, checking budget");
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::agent::{
        AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, LlmStreamResult,
    };
    use crate::error::AgentError;
    use crate::session::Session;
    use crate::types::{StopReason, ToolDef};
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::{Mutex, Notify};

    // Mock CommsRuntime for testing
    struct MockCommsRuntime {
        messages: Mutex<Vec<String>>,
        notify: Arc<Notify>,
        drain_count: AtomicUsize,
    }

    impl MockCommsRuntime {
        fn new() -> Self {
            Self {
                messages: Mutex::new(vec![]),
                notify: Arc::new(Notify::new()),
                drain_count: AtomicUsize::new(0),
            }
        }

        fn with_messages(msgs: Vec<String>) -> Self {
            Self {
                messages: Mutex::new(msgs),
                notify: Arc::new(Notify::new()),
                drain_count: AtomicUsize::new(0),
            }
        }

        async fn push_message(&self, msg: String) {
            self.messages.lock().await.push(msg);
            self.notify.notify_one();
        }

        fn drain_count(&self) -> usize {
            self.drain_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl CommsRuntime for MockCommsRuntime {
        async fn drain_messages(&self) -> Vec<String> {
            self.drain_count.fetch_add(1, Ordering::SeqCst);
            let mut msgs = self.messages.lock().await;
            std::mem::take(&mut *msgs)
        }

        fn inbox_notify(&self) -> Arc<Notify> {
            self.notify.clone()
        }
    }

    // Mock LLM client that returns empty response
    struct MockLlmClient;

    #[async_trait]
    impl AgentLlmClient for MockLlmClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&Value>,
        ) -> Result<LlmStreamResult, AgentError> {
            Ok(LlmStreamResult::new(
                "Done".to_string(),
                vec![],
                StopReason::EndTurn,
                crate::types::Usage::default(),
            ))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }
    }

    // Mock tool dispatcher with no tools
    struct MockToolDispatcher;

    #[async_trait]
    impl AgentToolDispatcher for MockToolDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::new([])
        }

        async fn dispatch(
            &self,
            name: &str,
            _args: &Value,
        ) -> Result<Value, crate::error::ToolError> {
            Err(crate::error::ToolError::NotFound {
                name: name.to_string(),
            })
        }
    }

    // Mock session store
    struct MockSessionStore;

    #[async_trait]
    impl AgentSessionStore for MockSessionStore {
        async fn save(&self, _session: &Session) -> Result<(), AgentError> {
            Ok(())
        }

        async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn test_drain_comms_inbox_no_runtime_returns_false() {
        let mut agent = AgentBuilder::new()
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        // No comms runtime set, should return false
        let drained = agent.drain_comms_inbox().await;
        assert!(!drained);
    }

    #[tokio::test]
    async fn test_drain_comms_inbox_empty_returns_false() {
        let comms = Arc::new(MockCommsRuntime::new());

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms.clone() as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        // Empty inbox should return false
        let drained = agent.drain_comms_inbox().await;
        assert!(!drained);
        assert_eq!(comms.drain_count(), 1);
    }

    #[tokio::test]
    async fn test_drain_comms_inbox_with_messages_returns_true() {
        let comms = Arc::new(MockCommsRuntime::with_messages(vec![
            "Hello from peer".to_string(),
            "Another message".to_string(),
        ]));

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms.clone() as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        // Should return true and inject messages
        let drained = agent.drain_comms_inbox().await;
        assert!(drained);

        // Check that messages were injected into session
        let messages = agent.session.messages();
        assert!(!messages.is_empty());

        // Last message should be a User message with combined content
        let last = messages.last().unwrap();
        match last {
            Message::User(user) => {
                assert!(user.content.contains("Hello from peer"));
                assert!(user.content.contains("Another message"));
            }
            _ => panic!("Expected User message, got {:?}", last),
        }
    }

    #[tokio::test]
    async fn test_drain_comms_inbox_clears_inbox() {
        let comms = Arc::new(MockCommsRuntime::with_messages(vec![
            "Message 1".to_string(),
        ]));

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms.clone() as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        // First drain should return true
        assert!(agent.drain_comms_inbox().await);

        // Second drain should return false (inbox is now empty)
        assert!(!agent.drain_comms_inbox().await);

        // Verify drain was called twice
        assert_eq!(comms.drain_count(), 2);
    }

    #[tokio::test]
    async fn test_drain_comms_inbox_multiple_calls_accumulate() {
        let comms = Arc::new(MockCommsRuntime::new());

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms.clone() as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        // First drain - empty
        assert!(!agent.drain_comms_inbox().await);

        // Add a message
        comms.push_message("First message".to_string()).await;

        // Second drain - has message
        assert!(agent.drain_comms_inbox().await);

        // Add more messages
        comms.push_message("Second message".to_string()).await;
        comms.push_message("Third message".to_string()).await;

        // Third drain - has messages
        assert!(agent.drain_comms_inbox().await);

        // Session should have two user messages (one from each successful drain)
        let user_messages: Vec<_> = agent
            .session
            .messages()
            .iter()
            .filter(|m| matches!(m, Message::User(_)))
            .collect();
        assert_eq!(user_messages.len(), 2);
    }
}
