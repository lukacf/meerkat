//! Agent comms helpers (host mode).

use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::event_tap::{EventTapState, tap_emit, tap_send_terminal};
use crate::interaction::{InboxInteraction, InteractionContent};
use crate::types::{Message, RunResult, Usage, UserMessage};
use std::sync::atomic::AtomicBool;
use tokio::sync::mpsc;

use crate::agent::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, CommsRuntime};

fn inject_response_into_session(
    session: &mut crate::session::Session,
    interaction: &InboxInteraction,
) {
    session.push(Message::User(UserMessage {
        content: interaction.rendered_text.clone(),
    }));
}

#[allow(dead_code)]
fn error_to_response_status(e: &AgentError) -> String {
    match e {
        AgentError::CallbackPending { .. } => "accepted".to_string(),
        _ => "failed".to_string(),
    }
}

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

        let interactions = comms.drain_interactions().await;
        let messages: Vec<String> = interactions
            .into_iter()
            .filter_map(|i| match i.content {
                InteractionContent::Message { .. } => Some(i.rendered_text),
                _ => None,
            })
            .collect();
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
                schema_warnings: None,
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

            let interactions = comms.drain_interactions().await;

            if comms.dismiss_received() {
                tracing::info!("Host mode: DISMISS received, exiting");
                return Ok(last_result);
            }

            if !interactions.is_empty() {
                let mut batched_messages = Vec::new();
                let mut individual = Vec::new();
                for interaction in interactions {
                    if matches!(&interaction.content, InteractionContent::Response { .. }) {
                        inject_response_into_session(&mut self.session, &interaction);
                        continue;
                    }

                    let subscriber = comms.interaction_subscriber(&interaction.id);
                    if subscriber.is_some() {
                        individual.push((interaction, subscriber));
                    } else {
                        match &interaction.content {
                            InteractionContent::Message { .. } => {
                                batched_messages.push(interaction)
                            }
                            InteractionContent::Request { .. } => {
                                individual.push((interaction, None))
                            }
                            InteractionContent::Response { .. } => unreachable!("handled above"),
                        }
                    }
                }

                if !batched_messages.is_empty() {
                    let combined_input = batched_messages
                        .iter()
                        .map(|m| m.rendered_text.clone())
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    tracing::debug!(
                        "Host mode: processing {} batched comms message(s)",
                        batched_messages.len()
                    );
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
                }

                for (interaction, subscriber) in individual {
                    if let Some(tx) = subscriber {
                        drop(tx);
                    }
                    match self.run(interaction.rendered_text).await {
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
            let _ = tap_emit(
                &self.event_tap,
                Some(&event_tx),
                AgentEvent::RunStarted {
                    session_id: self.session.id().clone(),
                    prompt: run_prompt,
                },
            )
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
                schema_warnings: None,
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

            let interactions = comms.drain_interactions().await;

            if comms.dismiss_received() {
                tracing::info!("Host mode: DISMISS received, exiting");
                return Ok(last_result);
            }

            if !interactions.is_empty() {
                let mut batched_messages = Vec::new();
                let mut individual = Vec::new();

                for interaction in interactions {
                    if matches!(&interaction.content, InteractionContent::Response { .. }) {
                        inject_response_into_session(&mut self.session, &interaction);
                        continue;
                    }

                    let subscriber = comms.interaction_subscriber(&interaction.id);
                    if subscriber.is_some() {
                        individual.push((interaction, subscriber));
                    } else {
                        match &interaction.content {
                            InteractionContent::Message { .. } => {
                                batched_messages.push(interaction)
                            }
                            InteractionContent::Request { .. } => {
                                individual.push((interaction, None))
                            }
                            InteractionContent::Response { .. } => unreachable!("handled above"),
                        }
                    }
                }

                if !batched_messages.is_empty() {
                    let combined_input = batched_messages
                        .iter()
                        .map(|m| m.rendered_text.clone())
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    tracing::debug!(
                        "Host mode: processing {} batched comms message(s)",
                        batched_messages.len()
                    );
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
                }

                for (interaction, subscriber) in individual {
                    let has_subscriber = subscriber.is_some();
                    if let Some(tx) = subscriber {
                        self.event_tap.lock().replace(EventTapState {
                            tx,
                            truncated: AtomicBool::new(false),
                        });
                    }

                    match self
                        .run_with_events(interaction.rendered_text.clone(), event_tx.clone())
                        .await
                    {
                        Ok(result) => {
                            if has_subscriber {
                                tap_send_terminal(
                                    self.event_tap(),
                                    AgentEvent::InteractionComplete {
                                        interaction_id: interaction.id,
                                        result: result.text.clone(),
                                    },
                                )
                                .await;
                            }
                            last_result = result;
                        }
                        Err(e) => {
                            if has_subscriber {
                                tap_send_terminal(
                                    self.event_tap(),
                                    AgentEvent::InteractionFailed {
                                        interaction_id: interaction.id,
                                        error: e.to_string(),
                                    },
                                )
                                .await;
                            }
                            if e.is_graceful() {
                                tracing::info!("Host mode: graceful exit - {}", e);
                                self.event_tap.lock().take();
                                return Ok(last_result);
                            }
                            self.event_tap.lock().take();
                            return Err(e);
                        }
                    }
                    self.event_tap.lock().take();
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
    use crate::interaction::{InboxInteraction, InteractionContent, InteractionId};
    use crate::session::Session;
    use crate::types::{AssistantBlock, StopReason, ToolCallView, ToolDef, ToolResult};
    use async_trait::async_trait;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::{Mutex, Notify, mpsc};
    use uuid::Uuid;

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
                vec![AssistantBlock::Text {
                    text: "Done".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                crate::types::Usage::default(),
            ))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }
    }

    struct FailingLlmClient;

    #[async_trait]
    impl AgentLlmClient for FailingLlmClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&Value>,
        ) -> Result<LlmStreamResult, AgentError> {
            Err(AgentError::llm(
                "mock",
                crate::error::LlmFailureReason::ProviderError(serde_json::json!({
                    "code": "forced"
                })),
                "forced failure",
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
            call: ToolCallView<'_>,
        ) -> Result<ToolResult, crate::error::ToolError> {
            Err(crate::error::ToolError::NotFound {
                name: call.name.to_string(),
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

    struct InteractionCommsRuntime {
        interactions: parking_lot::Mutex<Vec<InboxInteraction>>,
        subscribers: parking_lot::Mutex<HashMap<InteractionId, mpsc::Sender<AgentEvent>>>,
        notify: Arc<Notify>,
        drain_count: AtomicUsize,
    }

    impl InteractionCommsRuntime {
        fn with_interactions(items: Vec<InboxInteraction>) -> Self {
            Self {
                interactions: parking_lot::Mutex::new(items),
                subscribers: parking_lot::Mutex::new(HashMap::new()),
                notify: Arc::new(Notify::new()),
                drain_count: AtomicUsize::new(0),
            }
        }

        async fn register_subscriber(&self, id: InteractionId, tx: mpsc::Sender<AgentEvent>) {
            self.subscribers.lock().insert(id, tx);
        }
    }

    #[async_trait]
    impl CommsRuntime for InteractionCommsRuntime {
        async fn drain_messages(&self) -> Vec<String> {
            vec![]
        }

        async fn drain_interactions(&self) -> Vec<InboxInteraction> {
            self.drain_count.fetch_add(1, Ordering::SeqCst);
            let mut guard = self.interactions.lock();
            std::mem::take(&mut *guard)
        }

        fn inbox_notify(&self) -> Arc<Notify> {
            self.notify.clone()
        }

        fn dismiss_received(&self) -> bool {
            self.drain_count.load(Ordering::SeqCst) > 1
        }

        fn interaction_subscriber(&self, id: &InteractionId) -> Option<mpsc::Sender<AgentEvent>> {
            self.subscribers.lock().remove(id)
        }
    }

    #[tokio::test]
    async fn test_host_mode_with_events_subscriber_first_message_routes_individual() {
        let subscribed_id = InteractionId(Uuid::now_v7());
        let plain_id = InteractionId(Uuid::now_v7());
        let runtime = Arc::new(InteractionCommsRuntime::with_interactions(vec![
            InboxInteraction {
                id: subscribed_id,
                from: "ext".to_string(),
                content: InteractionContent::Message {
                    body: "subscribed".to_string(),
                },
                rendered_text: "subscribed".to_string(),
            },
            InboxInteraction {
                id: plain_id,
                from: "ext".to_string(),
                content: InteractionContent::Message {
                    body: "plain".to_string(),
                },
                rendered_text: "plain".to_string(),
            },
        ]));
        let (sub_tx, mut sub_rx) = mpsc::channel(128);
        runtime.register_subscriber(subscribed_id, sub_tx).await;

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(runtime as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;
        let (event_tx, _event_rx) = mpsc::channel(128);

        let result = agent
            .run_host_mode_with_events(String::new(), event_tx)
            .await
            .unwrap();
        assert!(result.turns >= 1);

        let user_messages: Vec<String> = agent
            .session
            .messages()
            .iter()
            .filter_map(|m| match m {
                Message::User(u) => Some(u.content.clone()),
                _ => None,
            })
            .collect();
        assert!(user_messages.iter().any(|m| m == "subscribed"));
        assert!(user_messages.iter().any(|m| m == "plain"));
        assert!(
            !user_messages
                .iter()
                .any(|m| m.contains("subscribed") && m.contains("plain")),
            "subscriber message must not be batched with plain message"
        );

        let mut saw_terminal = false;
        while let Ok(event) = sub_rx.try_recv() {
            if let AgentEvent::InteractionComplete { interaction_id, .. } = event {
                assert_eq!(interaction_id, subscribed_id);
                saw_terminal = true;
                break;
            }
        }
        assert!(
            saw_terminal,
            "expected InteractionComplete on subscriber stream"
        );
    }

    #[tokio::test]
    async fn test_host_mode_with_events_response_is_short_circuited() {
        let runtime = Arc::new(InteractionCommsRuntime::with_interactions(vec![
            InboxInteraction {
                id: InteractionId(Uuid::now_v7()),
                from: "peer".to_string(),
                content: InteractionContent::Response {
                    in_reply_to: InteractionId(Uuid::now_v7()),
                    status: "completed".to_string(),
                    result: serde_json::json!({"ok": true}),
                },
                rendered_text: "response payload".to_string(),
            },
        ]));

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(runtime as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;
        let (event_tx, mut event_rx) = mpsc::channel(64);

        let result = agent
            .run_host_mode_with_events(String::new(), event_tx)
            .await
            .unwrap();
        assert_eq!(result.turns, 0);
        assert_eq!(result.text, "");

        let saw_response_in_session = agent.session.messages().iter().any(|m| match m {
            Message::User(u) => u.content == "response payload",
            _ => false,
        });
        assert!(saw_response_in_session);
        assert!(
            event_rx.try_recv().is_err(),
            "response short-circuit should not run the agent loop"
        );
    }

    #[tokio::test]
    async fn test_host_mode_consumes_subscriber_and_closes_receiver_without_events() {
        let id = InteractionId(Uuid::now_v7());
        let runtime = Arc::new(InteractionCommsRuntime::with_interactions(vec![
            InboxInteraction {
                id,
                from: "ext".to_string(),
                content: InteractionContent::Message {
                    body: "non-events path".to_string(),
                },
                rendered_text: "non-events path".to_string(),
            },
        ]));
        let (tx, mut rx) = mpsc::channel(8);
        runtime.register_subscriber(id, tx).await;

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(runtime as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        let _ = agent.run_host_mode(String::new()).await.unwrap();
        assert!(
            rx.recv().await.is_none(),
            "subscriber should be removed/dropped in non-events path"
        );
    }

    #[tokio::test]
    async fn test_host_mode_with_events_request_without_subscriber_routes_individual() {
        let runtime = Arc::new(InteractionCommsRuntime::with_interactions(vec![
            InboxInteraction {
                id: InteractionId(Uuid::now_v7()),
                from: "peer".to_string(),
                content: InteractionContent::Request {
                    intent: "review".to_string(),
                    params: serde_json::json!({"id": 1}),
                },
                rendered_text: "request payload".to_string(),
            },
            InboxInteraction {
                id: InteractionId(Uuid::now_v7()),
                from: "peer".to_string(),
                content: InteractionContent::Message {
                    body: "batched payload".to_string(),
                },
                rendered_text: "batched payload".to_string(),
            },
        ]));

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(runtime as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;
        let (event_tx, _event_rx) = mpsc::channel(64);
        let _ = agent
            .run_host_mode_with_events(String::new(), event_tx)
            .await
            .unwrap();

        let user_messages: Vec<String> = agent
            .session
            .messages()
            .iter()
            .filter_map(|m| match m {
                Message::User(u) => Some(u.content.clone()),
                _ => None,
            })
            .collect();
        assert!(user_messages.iter().any(|m| m == "request payload"));
        assert!(user_messages.iter().any(|m| m == "batched payload"));
    }

    #[tokio::test]
    async fn test_host_mode_with_events_subscriber_receives_interaction_failed() {
        let subscribed_id = InteractionId(Uuid::now_v7());
        let runtime = Arc::new(InteractionCommsRuntime::with_interactions(vec![
            InboxInteraction {
                id: subscribed_id,
                from: "ext".to_string(),
                content: InteractionContent::Message {
                    body: "will fail".to_string(),
                },
                rendered_text: "will fail".to_string(),
            },
        ]));
        let (sub_tx, mut sub_rx) = mpsc::channel(64);
        runtime.register_subscriber(subscribed_id, sub_tx).await;

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(runtime as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(FailingLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;
        let (event_tx, _event_rx) = mpsc::channel(64);

        let result = agent
            .run_host_mode_with_events(String::new(), event_tx)
            .await;
        assert!(result.is_err());

        let mut saw_failed = false;
        while let Ok(event) = sub_rx.try_recv() {
            if let AgentEvent::InteractionFailed { interaction_id, .. } = event {
                assert_eq!(interaction_id, subscribed_id);
                saw_failed = true;
                break;
            }
        }
        assert!(saw_failed);
    }
}
