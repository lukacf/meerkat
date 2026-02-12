//! Agent comms helpers (host mode).

use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::interaction::InteractionContent;
use crate::types::{Message, RunResult, Usage, UserMessage};
use std::sync::atomic::AtomicBool;
use tokio::sync::mpsc;

use crate::agent::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, CommsRuntime};
use crate::interaction::InboxInteraction;
use crate::session::Session;

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
    ///
    /// No-op when `host_drain_active` is set — in host mode, the host loop
    /// owns the inbox drain cycle via `drain_interactions()` to preserve
    /// interaction-scoped subscriber correlation.
    pub(super) async fn drain_comms_inbox(&mut self) -> bool {
        if self.host_drain_active {
            return false;
        }

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

        // Host loop owns the inbox drain cycle — suppress inner-loop drains
        // to preserve interaction-scoped subscriber correlation.
        self.host_drain_active = true;

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
                let mut batched_texts = Vec::new();

                for interaction in interactions {
                    // Response interactions: inject into session, never run through LLM
                    if matches!(&interaction.content, InteractionContent::Response { .. }) {
                        inject_response_into_session(&mut self.session, &interaction);
                        continue;
                    }

                    // Consume subscriber to avoid leaks (no streaming in non-events mode)
                    let _ = comms.interaction_subscriber(&interaction.id);

                    match &interaction.content {
                        InteractionContent::Message { .. } => {
                            batched_texts.push(interaction.rendered_text);
                        }
                        InteractionContent::Request { .. } => {
                            // Process requests individually
                            match self.run(interaction.rendered_text).await {
                                Ok(result) => last_result = result,
                                Err(e) => {
                                    if e.is_graceful() {
                                        tracing::info!("Host mode: graceful exit - {}", e);
                                        return Ok(last_result);
                                    }
                                    return Err(e);
                                }
                            }
                        }
                        InteractionContent::Response { .. } => unreachable!("handled above"),
                    }
                }

                // Process batched messages as one run
                if !batched_texts.is_empty() {
                    let combined = batched_texts.join("\n\n");
                    tracing::debug!("Host mode: processing {} batched message(s)", batched_texts.len());
                    match self.run(combined).await {
                        Ok(result) => last_result = result,
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

        // Host loop owns the inbox drain cycle — suppress inner-loop drains
        // to preserve interaction-scoped subscriber correlation.
        self.host_drain_active = true;

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
            crate::event_tap::tap_emit(
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
                let mut batched_texts = Vec::new();
                let mut individual: Vec<(InboxInteraction, Option<mpsc::Sender<AgentEvent>>)> =
                    Vec::new();

                for interaction in interactions {
                    // Response interactions: inject into session, never run through LLM
                    if matches!(&interaction.content, InteractionContent::Response { .. }) {
                        inject_response_into_session(&mut self.session, &interaction);
                        continue;
                    }

                    let subscriber = comms.interaction_subscriber(&interaction.id);
                    if subscriber.is_some() {
                        // Any interaction with a subscriber gets individual processing with tap
                        individual.push((interaction, subscriber));
                    } else {
                        match &interaction.content {
                            InteractionContent::Message { .. } => {
                                batched_texts.push(interaction.rendered_text);
                            }
                            InteractionContent::Request { .. } => {
                                individual.push((interaction, None));
                            }
                            InteractionContent::Response { .. } => {
                                unreachable!("handled above")
                            }
                        }
                    }
                }

                // Process individual interactions (requests, or any with subscribers)
                for (interaction, subscriber) in individual {
                    // Install tap if subscriber present
                    if let Some(tx) = subscriber {
                        let mut guard = self.event_tap.lock();
                        *guard = Some(crate::event_tap::EventTapState {
                            tx,
                            truncated: AtomicBool::new(false),
                        });
                    }

                    let has_tap = self.event_tap.lock().is_some();

                    match self
                        .run_with_events(interaction.rendered_text, event_tx.clone())
                        .await
                    {
                        Ok(result) => {
                            if has_tap {
                                crate::event_tap::tap_send_terminal(
                                    &self.event_tap,
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
                            if has_tap {
                                crate::event_tap::tap_send_terminal(
                                    &self.event_tap,
                                    AgentEvent::InteractionFailed {
                                        interaction_id: interaction.id,
                                        error: e.to_string(),
                                    },
                                )
                                .await;
                            }
                            // Clear tap before returning
                            self.event_tap.lock().take();

                            if e.is_graceful() {
                                tracing::info!("Host mode: graceful exit - {}", e);
                                return Ok(last_result);
                            }
                            return Err(e);
                        }
                    }

                    // Clear tap after each interaction
                    self.event_tap.lock().take();
                }

                // Process batched messages as one run (no tap)
                if !batched_texts.is_empty() {
                    let combined = batched_texts.join("\n\n");
                    tracing::debug!(
                        "Host mode: processing {} batched message(s)",
                        batched_texts.len()
                    );
                    match self
                        .run_with_events(combined, event_tx.clone())
                        .await
                    {
                        Ok(result) => last_result = result,
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
}

/// Inject a Response interaction into the session as a user message.
///
/// Response interactions are never processed through the LLM loop.
/// They are injected inline so the agent has the response context for subsequent turns.
fn inject_response_into_session(session: &mut Session, interaction: &InboxInteraction) {
    session.push(Message::User(UserMessage {
        content: interaction.rendered_text.clone(),
    }));
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
    use crate::types::{AssistantBlock, StopReason, ToolCallView, ToolDef, ToolResult};
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

    // Advanced mock for testing interaction-aware host mode (uses parking_lot for sync subscriber access).
    // Auto-dismisses: after the first drain that returned interactions, the next empty drain
    // sets dismiss=true so the host loop exits cleanly.
    struct SyncInteractionMockCommsRuntime {
        interactions: Mutex<Vec<crate::interaction::InboxInteraction>>,
        subscribers: parking_lot::Mutex<
            std::collections::HashMap<crate::interaction::InteractionId, mpsc::Sender<AgentEvent>>,
        >,
        notify: Arc<Notify>,
        dismiss: std::sync::atomic::AtomicBool,
        had_interactions: std::sync::atomic::AtomicBool,
    }

    impl SyncInteractionMockCommsRuntime {
        fn with_interactions(interactions: Vec<crate::interaction::InboxInteraction>) -> Self {
            Self {
                interactions: Mutex::new(interactions),
                subscribers: parking_lot::Mutex::new(std::collections::HashMap::new()),
                notify: Arc::new(Notify::new()),
                dismiss: std::sync::atomic::AtomicBool::new(false),
                had_interactions: std::sync::atomic::AtomicBool::new(false),
            }
        }

        fn register_subscriber(
            &self,
            id: crate::interaction::InteractionId,
            tx: mpsc::Sender<AgentEvent>,
        ) {
            self.subscribers.lock().insert(id, tx);
        }
    }

    #[async_trait]
    impl CommsRuntime for SyncInteractionMockCommsRuntime {
        async fn drain_messages(&self) -> Vec<String> {
            vec![]
        }

        fn inbox_notify(&self) -> Arc<Notify> {
            self.notify.clone()
        }

        async fn drain_interactions(&self) -> Vec<crate::interaction::InboxInteraction> {
            let mut interactions = self.interactions.lock().await;
            let result = std::mem::take(&mut *interactions);
            if !result.is_empty() {
                self.had_interactions.store(true, Ordering::SeqCst);
            } else if self.had_interactions.load(Ordering::SeqCst) {
                // Previously had interactions, now empty → auto-dismiss
                self.dismiss.store(true, Ordering::SeqCst);
            }
            result
        }

        fn interaction_subscriber(
            &self,
            id: &crate::interaction::InteractionId,
        ) -> Option<mpsc::Sender<AgentEvent>> {
            self.subscribers.lock().remove(id)
        }

        fn dismiss_received(&self) -> bool {
            self.dismiss.load(Ordering::SeqCst)
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

    // --- Phase 2: Host loop interaction-aware tests ---

    fn make_interaction(
        content: InteractionContent,
        rendered_text: &str,
    ) -> crate::interaction::InboxInteraction {
        crate::interaction::InboxInteraction {
            id: crate::interaction::InteractionId(uuid::Uuid::new_v4()),
            from: "test-peer".into(),
            content,
            rendered_text: rendered_text.to_string(),
        }
    }

    #[tokio::test]
    async fn test_response_interaction_injected_into_session_not_run() {
        let response_id = crate::interaction::InteractionId(uuid::Uuid::new_v4());
        let response = crate::interaction::InboxInteraction {
            id: crate::interaction::InteractionId(uuid::Uuid::new_v4()),
            from: "peer".into(),
            content: InteractionContent::Response {
                in_reply_to: response_id,
                status: "completed".into(),
                result: serde_json::json!({"ok": true}),
            },
            rendered_text: "[Response] completed: {\"ok\":true}".into(),
        };

        let comms = Arc::new(SyncInteractionMockCommsRuntime::with_interactions(vec![
            response,
        ]));

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms.clone() as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        let result = agent.run_host_mode(String::new()).await.unwrap();

        // Response should have been injected into session as a user message
        let user_msgs: Vec<_> = agent
            .session
            .messages()
            .iter()
            .filter(|m| matches!(m, Message::User(_)))
            .collect();
        assert_eq!(user_msgs.len(), 1);
        match &user_msgs[0] {
            Message::User(u) => assert!(u.content.contains("completed")),
            _ => unreachable!(),
        }

        // Result should be the empty initial (no LLM was called for the response)
        assert_eq!(result.turns, 0);
    }

    #[tokio::test]
    async fn test_non_events_host_mode_consumes_subscriber() {
        let interaction = make_interaction(
            InteractionContent::Message {
                body: "hello".into(),
            },
            "hello",
        );
        let interaction_id = interaction.id;

        let (sub_tx, mut sub_rx) = mpsc::channel::<AgentEvent>(16);

        let comms = Arc::new(SyncInteractionMockCommsRuntime::with_interactions(vec![
            interaction,
        ]));
        comms.register_subscriber(interaction_id, sub_tx);

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms.clone() as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        let _result = agent.run_host_mode("".into()).await.unwrap();

        // Subscriber should have been consumed (removed from registry)
        assert!(comms.subscribers.lock().is_empty());

        // In non-events mode, subscriber is dropped immediately, so receiver should be closed
        // (no events sent, channel just closes)
        assert!(sub_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn test_events_host_mode_subscriber_receives_terminal_event() {
        let interaction = make_interaction(
            InteractionContent::Message {
                body: "hello".into(),
            },
            "hello",
        );
        let interaction_id = interaction.id;

        let (sub_tx, mut sub_rx) = mpsc::channel::<AgentEvent>(4096);

        let comms = Arc::new(SyncInteractionMockCommsRuntime::with_interactions(vec![
            interaction,
        ]));
        comms.register_subscriber(interaction_id, sub_tx);

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms.clone() as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(4096);

        let _result = agent
            .run_host_mode_with_events("".into(), event_tx)
            .await
            .unwrap();

        // Subscriber should have been consumed
        assert!(comms.subscribers.lock().is_empty());

        // Collect all events from the subscriber channel
        let mut sub_events = Vec::new();
        while let Ok(event) = sub_rx.try_recv() {
            sub_events.push(event);
        }

        // Should have received a terminal InteractionComplete event
        let terminal = sub_events
            .iter()
            .find(|e| matches!(e, AgentEvent::InteractionComplete { .. }));
        assert!(
            terminal.is_some(),
            "Expected InteractionComplete, got events: {:?}",
            sub_events
        );

        match terminal.unwrap() {
            AgentEvent::InteractionComplete {
                interaction_id: id,
                result,
            } => {
                assert_eq!(*id, interaction_id);
                assert_eq!(result, "Done");
            }
            _ => unreachable!(),
        }

        // Primary event channel should also have events (RunStarted, lifecycle, RunCompleted)
        let mut primary_events = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            primary_events.push(event);
        }
        assert!(
            primary_events
                .iter()
                .any(|e| matches!(e, AgentEvent::RunStarted { .. })),
            "Primary channel should have RunStarted"
        );
    }

    #[tokio::test]
    async fn test_events_host_mode_request_without_subscriber_processed_individually() {
        let request = make_interaction(
            InteractionContent::Request {
                intent: "review".into(),
                params: serde_json::json!({}),
            },
            "Please review this code",
        );

        let comms = Arc::new(SyncInteractionMockCommsRuntime::with_interactions(vec![
            request,
        ]));

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        let (event_tx, _event_rx) = mpsc::channel::<AgentEvent>(4096);

        let result = agent
            .run_host_mode_with_events("".into(), event_tx)
            .await
            .unwrap();

        // Request should have been processed (LLM called)
        assert!(result.turns > 0);
        assert_eq!(result.text, "Done");
    }

    #[tokio::test]
    async fn test_events_host_mode_messages_without_subscriber_are_batched() {
        let msg1 = make_interaction(
            InteractionContent::Message {
                body: "msg1".into(),
            },
            "Message from Alice",
        );
        let msg2 = make_interaction(
            InteractionContent::Message {
                body: "msg2".into(),
            },
            "Message from Bob",
        );

        let comms = Arc::new(SyncInteractionMockCommsRuntime::with_interactions(vec![
            msg1, msg2,
        ]));

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        let (event_tx, _event_rx) = mpsc::channel::<AgentEvent>(4096);

        let result = agent
            .run_host_mode_with_events("".into(), event_tx)
            .await
            .unwrap();

        // Both messages batched into one run
        assert!(result.turns > 0);
    }

    #[tokio::test]
    async fn test_events_host_mode_tap_cleared_after_interaction() {
        let interaction = make_interaction(
            InteractionContent::Message {
                body: "hello".into(),
            },
            "hello",
        );
        let interaction_id = interaction.id;

        let (sub_tx, _sub_rx) = mpsc::channel::<AgentEvent>(4096);

        let comms = Arc::new(SyncInteractionMockCommsRuntime::with_interactions(vec![
            interaction,
        ]));
        comms.register_subscriber(interaction_id, sub_tx);

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        let (event_tx, _event_rx) = mpsc::channel::<AgentEvent>(4096);

        let _result = agent
            .run_host_mode_with_events("".into(), event_tx)
            .await
            .unwrap();

        // Tap should be cleared (no active subscriber)
        assert!(agent.event_tap.lock().is_none());
    }

    #[tokio::test]
    async fn test_inject_response_into_session_helper() {
        let mut session = Session::new();

        let response_id = crate::interaction::InteractionId(uuid::Uuid::new_v4());
        let interaction = crate::interaction::InboxInteraction {
            id: crate::interaction::InteractionId(uuid::Uuid::new_v4()),
            from: "peer".into(),
            content: InteractionContent::Response {
                in_reply_to: response_id,
                status: "ok".into(),
                result: serde_json::json!("result data"),
            },
            rendered_text: "[Response] ok: result data".into(),
        };

        inject_response_into_session(&mut session, &interaction);

        let msgs = session.messages();
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            Message::User(u) => assert_eq!(u.content, "[Response] ok: result data"),
            _ => panic!("Expected User message"),
        }
    }
}
