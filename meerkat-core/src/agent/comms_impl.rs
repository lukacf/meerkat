//! Agent comms helpers (host mode).

use crate::interaction::InteractionContent;
use crate::types::{Message, UserMessage};
use std::collections::BTreeMap;

use crate::agent::{
    Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, CommsRuntime,
    InlinePeerNotificationPolicy,
};

/// Presentation cap for explicit peer names in one inline summary.
const PEER_INLINE_NAME_LIMIT: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeerLifecycleState {
    Added,
    Retired,
}

#[derive(Debug, Default)]
struct PeerLifecycleBatch {
    by_peer: BTreeMap<String, PeerLifecycleState>,
}

impl PeerLifecycleBatch {
    fn observe(&mut self, peer: String, next: PeerLifecycleState) {
        match self.by_peer.get(&peer).copied() {
            Some(prev) if prev != next => {
                // Net opposite transitions within the same drain cycle.
                self.by_peer.remove(&peer);
            }
            Some(_) => {}
            None => {
                self.by_peer.insert(peer, next);
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.by_peer.is_empty()
    }

    fn split_lists(&self) -> (Vec<String>, Vec<String>) {
        let mut added = Vec::new();
        let mut retired = Vec::new();
        for (peer, state) in &self.by_peer {
            match state {
                PeerLifecycleState::Added => added.push(peer.clone()),
                PeerLifecycleState::Retired => retired.push(peer.clone()),
            }
        }
        (added, retired)
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

    /// Get a shared handle to the comms runtime, if enabled.
    pub fn comms_arc(&self) -> Option<std::sync::Arc<dyn CommsRuntime>> {
        self.comms_runtime.clone()
    }

    /// Drain comms inbox and inject messages into session.
    /// Returns true if any messages were injected.
    ///
    /// Called at turn boundaries by the agent loop to inject pending comms
    /// messages into the session context before the next LLM call.
    ///
    /// Routes on the stored `PeerInputClass` from ingress classification —
    /// no downstream re-classification.
    pub(super) async fn drain_comms_inbox(&mut self) -> bool {
        // When a dedicated comms drain task is active, suppress turn-boundary
        // draining so the session service is the sole inbox consumer.
        if self
            .comms_drain_active
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return false;
        }

        use crate::interaction::PeerInputClass;

        let comms = match &self.comms_runtime {
            Some(c) => c.clone(),
            None => return false,
        };

        // Try classified drain first; fall back to legacy drain for runtimes
        // that only implement drain_inbox_interactions()/drain_messages().
        let classified = match comms.drain_classified_inbox_interactions().await {
            Ok(v) => v,
            Err(_) => {
                // Legacy runtime — classify by InteractionContent to preserve
                // peer lifecycle batching and response routing semantics.
                let interactions = comms.drain_inbox_interactions().await;
                if interactions.is_empty() {
                    return false;
                }
                let mut content_blocks: Vec<crate::types::ContentBlock> = Vec::new();
                let mut peer_lifecycle_batch = PeerLifecycleBatch::default();

                for interaction in interactions {
                    match &interaction.content {
                        InteractionContent::Request { intent, params }
                            if intent == "mob.peer_added" =>
                        {
                            let peer = params
                                .get("peer")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                                .unwrap_or(interaction.from.as_str())
                                .to_string();
                            peer_lifecycle_batch.observe(peer, PeerLifecycleState::Added);
                        }
                        InteractionContent::Request { intent, params }
                            if intent == "mob.peer_retired" =>
                        {
                            let peer = params
                                .get("peer")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.is_empty())
                                .unwrap_or(interaction.from.as_str())
                                .to_string();
                            peer_lifecycle_batch.observe(peer, PeerLifecycleState::Retired);
                        }
                        _ => {
                            // Turn-boundary drain: all non-lifecycle interactions
                            // are injected as context for the next LLM call.
                            // Preserve multimodal blocks when present.
                            if let InteractionContent::Message {
                                blocks: Some(blocks),
                                ..
                            } = &interaction.content
                                && !blocks.is_empty()
                            {
                                content_blocks.extend(blocks.iter().cloned());
                            } else {
                                content_blocks.push(crate::types::ContentBlock::Text {
                                    text: interaction.rendered_text,
                                });
                            }
                        }
                    }
                }

                if let Some(peer_update) = self
                    .render_peer_lifecycle_update(&comms, &peer_lifecycle_batch)
                    .await
                {
                    content_blocks.push(crate::types::ContentBlock::Text { text: peer_update });
                }

                if content_blocks.is_empty() {
                    return false;
                }

                self.session
                    .push(Message::User(UserMessage::with_blocks(content_blocks)));
                return true;
            }
        };

        if classified.is_empty() {
            return false;
        }

        let mut content_blocks: Vec<crate::types::ContentBlock> = Vec::new();
        let mut peer_lifecycle_batch = PeerLifecycleBatch::default();

        for ci in classified {
            match ci.class {
                PeerInputClass::PeerLifecycleAdded => {
                    if let Some(peer) = ci.lifecycle_peer {
                        peer_lifecycle_batch.observe(peer, PeerLifecycleState::Added);
                    }
                }
                PeerInputClass::PeerLifecycleRetired => {
                    if let Some(peer) = ci.lifecycle_peer {
                        peer_lifecycle_batch.observe(peer, PeerLifecycleState::Retired);
                    }
                }
                PeerInputClass::Ack => {
                    // Filtered at ingress or handled separately
                }
                PeerInputClass::Response
                | PeerInputClass::SilentRequest
                | PeerInputClass::ActionableMessage
                | PeerInputClass::ActionableRequest
                | PeerInputClass::PlainEvent => {
                    // Turn-boundary drain: preserve multimodal blocks when present.
                    if let InteractionContent::Message {
                        blocks: Some(blocks),
                        ..
                    } = &ci.interaction.content
                        && !blocks.is_empty()
                    {
                        content_blocks.extend(blocks.iter().cloned());
                    } else {
                        content_blocks.push(crate::types::ContentBlock::Text {
                            text: ci.interaction.rendered_text,
                        });
                    }
                }
            }
        }

        if let Some(peer_update) = self
            .render_peer_lifecycle_update(&comms, &peer_lifecycle_batch)
            .await
        {
            content_blocks.push(crate::types::ContentBlock::Text { text: peer_update });
        }

        if content_blocks.is_empty() {
            return false;
        }

        tracing::debug!(
            "Injecting {} comms content blocks into session",
            content_blocks.len()
        );
        self.session
            .push(Message::User(UserMessage::with_blocks(content_blocks)));
        true
    }
}

fn render_named_list(mut names: Vec<String>) -> String {
    names.sort();
    if names.len() <= PEER_INLINE_NAME_LIMIT {
        return names.join(", ");
    }

    let extra = names.len() - PEER_INLINE_NAME_LIMIT;
    let displayed = names
        .into_iter()
        .take(PEER_INLINE_NAME_LIMIT)
        .collect::<Vec<_>>();
    format!("{} (+{} more)", displayed.join(", "), extra)
}

fn render_peer_update_summary(batch: &PeerLifecycleBatch) -> Option<String> {
    if batch.is_empty() {
        return None;
    }

    let (added, retired) = batch.split_lists();
    let mut parts = Vec::new();

    if !added.is_empty() {
        let plural = if added.len() == 1 { "" } else { "s" };
        parts.push(format!(
            "{} peer{} connected: {}",
            added.len(),
            plural,
            render_named_list(added)
        ));
    }
    if !retired.is_empty() {
        let plural = if retired.len() == 1 { "" } else { "s" };
        parts.push(format!(
            "{} peer{} retired: {}",
            retired.len(),
            plural,
            render_named_list(retired)
        ));
    }

    Some(format!("[PEER UPDATE] {}", parts.join("; ")))
}

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized + 'static,
    T: AgentToolDispatcher + ?Sized + 'static,
    S: AgentSessionStore + ?Sized + 'static,
{
    async fn render_peer_lifecycle_update(
        &mut self,
        comms: &std::sync::Arc<dyn CommsRuntime>,
        batch: &PeerLifecycleBatch,
    ) -> Option<String> {
        if batch.is_empty() {
            return None;
        }

        let peer_count = comms.peer_count().await;
        let suppress = match self.inline_peer_notification_policy {
            InlinePeerNotificationPolicy::Always => false,
            InlinePeerNotificationPolicy::Never => true,
            InlinePeerNotificationPolicy::AtMost(limit) => peer_count > limit,
        };

        if suppress {
            if !self.peer_notification_suppression_active {
                self.peer_notification_suppression_active = true;
                return Some(format!(
                    "[PEER UPDATE] Peer updates suppressed at current scale ({peer_count} peers). Use peers() to inspect peers."
                ));
            }
            return None;
        }

        let resumed = self.peer_notification_suppression_active;
        self.peer_notification_suppression_active = false;
        let summary = render_peer_update_summary(batch)?;
        if resumed {
            return Some(format!(
                "[PEER UPDATE] Peer updates resumed at current scale ({peer_count} peers).\n{summary}"
            ));
        }
        Some(summary)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::agent::{
        AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, LlmStreamResult,
    };
    use crate::comms::{PeerDirectoryEntry, PeerDirectorySource, PeerName, PeerReachability};
    use crate::error::{AgentError, LlmFailureReason};
    use crate::session::Session;
    use crate::types::{AssistantBlock, StopReason, ToolCallView, ToolDef};
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

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
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

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
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

    // Mock LLM client that always fails.
    #[allow(dead_code)]
    struct FailingLlmClient;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
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
                LlmFailureReason::ProviderError(serde_json::json!({"kind":"test"})),
                "forced failure",
            ))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }
    }

    // Mock tool dispatcher with no tools
    struct MockToolDispatcher;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for MockToolDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::new([])
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, crate::error::ToolError> {
            Err(crate::error::ToolError::NotFound {
                name: call.name.to_string(),
            })
        }
    }

    // Mock session store
    struct MockSessionStore;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentSessionStore for MockSessionStore {
        async fn save(&self, _session: &Session) -> Result<(), AgentError> {
            Ok(())
        }

        async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
            Ok(None)
        }
    }

    // Sequenced interactions runtime for non-host drain path tests.
    struct SequencedInteractionMockCommsRuntime {
        batches: Mutex<Vec<Vec<crate::interaction::InboxInteraction>>>,
        notify: Arc<Notify>,
        peer_count: AtomicUsize,
        peers_calls: AtomicUsize,
    }

    impl SequencedInteractionMockCommsRuntime {
        fn new(batches: Vec<Vec<crate::interaction::InboxInteraction>>, peer_count: usize) -> Self {
            Self {
                batches: Mutex::new(batches),
                notify: Arc::new(Notify::new()),
                peer_count: AtomicUsize::new(peer_count),
                peers_calls: AtomicUsize::new(0),
            }
        }

        fn set_peer_count(&self, peer_count: usize) {
            self.peer_count.store(peer_count, Ordering::SeqCst);
        }

        fn peers_calls(&self) -> usize {
            self.peers_calls.load(Ordering::SeqCst)
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl CommsRuntime for SequencedInteractionMockCommsRuntime {
        async fn drain_messages(&self) -> Vec<String> {
            vec![]
        }

        fn inbox_notify(&self) -> Arc<Notify> {
            self.notify.clone()
        }

        async fn drain_inbox_interactions(&self) -> Vec<crate::interaction::InboxInteraction> {
            let mut batches = self.batches.lock().await;
            if batches.is_empty() {
                Vec::new()
            } else {
                batches.remove(0)
            }
        }

        async fn peers(&self) -> Vec<PeerDirectoryEntry> {
            self.peers_calls.fetch_add(1, Ordering::SeqCst);
            let count = self.peer_count.load(Ordering::SeqCst);
            (0..count)
                .map(|idx| PeerDirectoryEntry {
                    name: PeerName::new(format!("peer-{idx}")).expect("valid peer name"),
                    peer_id: format!("ed25519:peer-{idx}"),
                    address: format!("inproc://peer-{idx}"),
                    source: PeerDirectorySource::Inproc,
                    sendable_kinds: vec!["message".to_string(), "request".to_string()],
                    capabilities: serde_json::json!({}),
                    reachability: PeerReachability::Unknown,
                    last_unreachable_reason: None,
                    meta: crate::peer_meta::PeerMeta::default(),
                })
                .collect()
        }

        async fn peer_count(&self) -> usize {
            self.peer_count.load(Ordering::SeqCst)
        }
    }

    #[allow(dead_code)]
    struct DismissOnlyMockCommsRuntime {
        notify: Arc<Notify>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl CommsRuntime for DismissOnlyMockCommsRuntime {
        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<Notify> {
            self.notify.clone()
        }

        fn dismiss_received(&self) -> bool {
            true
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
                assert!(user.text_content().contains("Hello from peer"));
                assert!(user.text_content().contains("Another message"));
            }
            _ => panic!("Expected User message, got {last:?}"),
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
    async fn test_peer_lifecycle_suppression_threshold_emits_one_time_notice() {
        let first_batch = vec![make_interaction(
            InteractionContent::Request {
                intent: "mob.peer_added".into(),
                params: serde_json::json!({"peer": "worker-1"}),
            },
            "add worker-1",
        )];
        let second_batch = vec![make_interaction(
            InteractionContent::Request {
                intent: "mob.peer_added".into(),
                params: serde_json::json!({"peer": "worker-2"}),
            },
            "add worker-2",
        )];
        let comms = Arc::new(SequencedInteractionMockCommsRuntime::new(
            vec![first_batch, second_batch],
            100,
        ));

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms.clone() as Arc<dyn CommsRuntime>)
            .with_max_inline_peer_notifications(Some(20))
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        assert!(agent.drain_comms_inbox().await);
        assert!(!agent.drain_comms_inbox().await);

        let user_msgs: Vec<_> = agent
            .session
            .messages()
            .iter()
            .filter_map(|m| match m {
                Message::User(u) => Some(u.text_content()),
                _ => None,
            })
            .collect();

        assert_eq!(user_msgs.len(), 1, "suppression notice should emit once");
        assert!(user_msgs[0].contains("Peer updates suppressed"));
        assert!(user_msgs[0].contains("(100 peers)"));
        assert_eq!(
            comms.peers_calls(),
            0,
            "render path should use peer_count()"
        );
    }

    #[tokio::test]
    async fn test_peer_lifecycle_suppression_lift_emits_resume_notice() {
        let first_batch = vec![make_interaction(
            InteractionContent::Request {
                intent: "mob.peer_added".into(),
                params: serde_json::json!({"peer": "worker-1"}),
            },
            "add worker-1",
        )];
        let second_batch = vec![make_interaction(
            InteractionContent::Request {
                intent: "mob.peer_added".into(),
                params: serde_json::json!({"peer": "worker-2"}),
            },
            "add worker-2",
        )];
        let comms = Arc::new(SequencedInteractionMockCommsRuntime::new(
            vec![first_batch, second_batch],
            100,
        ));

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms.clone() as Arc<dyn CommsRuntime>)
            .with_max_inline_peer_notifications(Some(20))
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        assert!(agent.drain_comms_inbox().await);
        comms.set_peer_count(5);
        assert!(agent.drain_comms_inbox().await);

        let user_msgs: Vec<_> = agent
            .session
            .messages()
            .iter()
            .filter_map(|m| match m {
                Message::User(u) => Some(u.text_content()),
                _ => None,
            })
            .collect();
        assert_eq!(user_msgs.len(), 2);
        assert!(user_msgs[0].contains("Peer updates suppressed"));
        assert!(user_msgs[1].contains("Peer updates resumed"));
        assert!(user_msgs[1].contains("1 peer connected"));
    }

    #[tokio::test]
    async fn test_drain_comms_inbox_batches_peer_lifecycle_into_single_entry() {
        let interactions = vec![
            make_interaction(
                InteractionContent::Request {
                    intent: "mob.peer_added".into(),
                    params: serde_json::json!({"peer": "worker-1"}),
                },
                "add worker-1",
            ),
            make_interaction(
                InteractionContent::Request {
                    intent: "mob.peer_added".into(),
                    params: serde_json::json!({"peer": "worker-2"}),
                },
                "add worker-2",
            ),
        ];
        let comms = Arc::new(SequencedInteractionMockCommsRuntime::new(
            vec![interactions],
            2,
        ));
        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        assert!(agent.drain_comms_inbox().await);

        let user_msgs: Vec<_> = agent
            .session
            .messages()
            .iter()
            .filter_map(|m| match m {
                Message::User(u) => Some(u.text_content()),
                _ => None,
            })
            .collect();
        assert_eq!(user_msgs.len(), 1);
        assert!(user_msgs[0].contains("[PEER UPDATE]"));
        assert!(user_msgs[0].contains("2 peers connected"));
    }
}
