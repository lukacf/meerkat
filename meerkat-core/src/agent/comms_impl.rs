//! Agent comms helpers (host mode).

use crate::error::AgentError;
use crate::event::AgentEvent;
use crate::interaction::InteractionContent;
use crate::types::{Message, RunResult, Usage, UserMessage};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use tokio::sync::mpsc;

use crate::agent::{
    Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, CommsRuntime,
    InlinePeerNotificationPolicy,
};
use crate::interaction::InboxInteraction;
use crate::session::Session;

/// Presentation cap for explicit peer names in one inline summary.
const PEER_INLINE_NAME_LIMIT: usize = 10;
const PEER_ADDED_INTENT: &str = "mob.peer_added";
const PEER_RETIRED_INTENT: &str = "mob.peer_retired";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeerLifecycleState {
    Added,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CommsInteractionClass {
    PeerLifecycle {
        peer: String,
        state: PeerLifecycleState,
    },
    InlineSessionOnly,
    Passthrough,
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
    /// No-op when `host_drain_active` is set — in host mode, the host loop
    /// owns the inbox drain cycle via `drain_inbox_interactions()` to preserve
    /// interaction-scoped subscriber correlation.
    pub(super) async fn drain_comms_inbox(&mut self) -> bool {
        if self.host_drain_active {
            return false;
        }

        let comms = match &self.comms_runtime {
            Some(c) => c.clone(),
            None => return false,
        };

        let interactions = comms.drain_inbox_interactions().await;
        if interactions.is_empty() {
            return false;
        }

        let mut messages = Vec::new();
        let mut peer_lifecycle_batch = PeerLifecycleBatch::default();

        for interaction in interactions {
            match classify_comms_interaction(&interaction, &self.silent_comms_intents) {
                CommsInteractionClass::PeerLifecycle { peer, state } => {
                    peer_lifecycle_batch.observe(peer, state);
                }
                CommsInteractionClass::InlineSessionOnly | CommsInteractionClass::Passthrough => {
                    // Turn-boundary drain injects both inline-only and passthrough
                    // interactions as context for the next LLM call.
                    messages.push(interaction.rendered_text);
                }
            }
        }

        if let Some(peer_update) = self
            .render_peer_lifecycle_update(&comms, &peer_lifecycle_batch)
            .await
        {
            messages.push(peer_update);
        }

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
        self.run_host_mode_inner(initial_prompt, None).await
    }

    /// Run in host mode with event streaming.
    pub async fn run_host_mode_with_events(
        &mut self,
        initial_prompt: String,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        self.run_host_mode_inner(initial_prompt, Some(event_tx))
            .await
    }

    /// Core host mode implementation shared by `run_host_mode()` and
    /// `run_host_mode_with_events()`.
    ///
    /// Processes the initial prompt, then loops waiting for comms interactions.
    /// When `event_tx` is `Some`, subscriber-bound interactions get individual
    /// tap-scoped processing with terminal events; otherwise subscribers are
    /// consumed and dropped.
    async fn run_host_mode_inner(
        &mut self,
        initial_prompt: String,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<RunResult, AgentError> {
        use std::time::Duration;
        let event_tx = event_tx.or_else(|| self.default_event_tx.clone());

        // Host loop owns the inbox drain cycle — suppress inner-loop drains
        // to preserve interaction-scoped subscriber correlation.
        self.host_drain_active = true;

        let comms = self.comms_runtime.clone().ok_or_else(|| {
            self.host_drain_active = false;
            AgentError::ConfigError("Host mode requires comms to be enabled".to_string())
        })?;

        let has_pending_user_message = self
            .session
            .messages()
            .last()
            .is_some_and(|m| matches!(m, Message::User(_)));

        let mut last_result = if !initial_prompt.trim().is_empty() {
            match &event_tx {
                Some(tx) => self.run_with_events(initial_prompt, tx.clone()).await?,
                None => self.run(initial_prompt).await?,
            }
        } else if has_pending_user_message {
            if let Some(ref tx) = event_tx {
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
                    Some(tx),
                    AgentEvent::RunStarted {
                        session_id: self.session.id().clone(),
                        prompt: run_prompt,
                    },
                )
                .await;
                self.run_loop(Some(tx.clone())).await?
            } else {
                self.run_loop(None).await?
            }
        } else {
            RunResult {
                text: String::new(),
                session_id: self.session.id().clone(),
                turns: 0,
                tool_calls: 0,
                usage: Usage::default(),
                structured_output: None,
                schema_warnings: None,
                skill_diagnostics: None,
            }
        };

        // Checkpoint after the initial host-mode turn so the first run
        // is persisted even if no inbox traffic ever arrives.
        if let Some(ref cp) = self.checkpointer {
            cp.checkpoint(&self.session).await;
        }

        let inbox_notify = comms.inbox_notify();
        const POLL_INTERVAL: Duration = Duration::from_secs(60);

        loop {
            if self.budget.is_exhausted() {
                tracing::info!("Host mode: budget exhausted, exiting");
                self.host_drain_active = false;
                return Ok(last_result);
            }

            let timeout = self.budget.remaining_duration().unwrap_or(POLL_INTERVAL);
            let notified = inbox_notify.notified();

            let interactions = comms.drain_inbox_interactions().await;

            if comms.dismiss_received() {
                tracing::info!("Host mode: DISMISS received, exiting");
                self.host_drain_active = false;
                return Ok(last_result);
            }

            if !interactions.is_empty() {
                // --- Classification phase ---
                //
                // Interactions are classified into individual vs batched processing.
                // This intentionally reorders relative to inbox arrival: individual
                // interactions (requests and subscriber-bound) are processed first,
                // then batched messages. Requests need individual processing for
                // subscriber correlation and isolated error handling; messages are
                // batched for efficiency. The LLM sees all context regardless of
                // processing order.
                let mut batched_texts = Vec::new();
                let mut individual: Vec<(InboxInteraction, Option<mpsc::Sender<AgentEvent>>)> =
                    Vec::new();
                let mut peer_lifecycle_batch = PeerLifecycleBatch::default();
                let mut had_session_injections = false;

                for interaction in interactions {
                    match classify_comms_interaction(&interaction, &self.silent_comms_intents) {
                        CommsInteractionClass::PeerLifecycle { peer, state } => {
                            peer_lifecycle_batch.observe(peer, state);
                        }
                        CommsInteractionClass::InlineSessionOnly => {
                            inject_response_into_session(&mut self.session, &interaction);
                            had_session_injections = true;
                        }
                        CommsInteractionClass::Passthrough => {
                            let subscriber = comms.interaction_subscriber(&interaction.id);

                            if event_tx.is_some() && subscriber.is_some() {
                                // Events mode: subscriber-bound interactions get individual
                                // tap-scoped processing with terminal events.
                                individual.push((interaction, subscriber));
                            } else {
                                // No events or no subscriber — consume subscriber to avoid
                                // leaks, then classify by content type.
                                drop(subscriber);

                                match &interaction.content {
                                    InteractionContent::Message { .. } => {
                                        batched_texts.push(interaction.rendered_text);
                                    }
                                    InteractionContent::Request { .. } => {
                                        individual.push((interaction, None));
                                    }
                                    InteractionContent::Response { .. } => {
                                        unreachable!("passthrough excludes responses")
                                    }
                                }
                            }
                        }
                    }
                }

                if let Some(peer_update) = self
                    .render_peer_lifecycle_update(&comms, &peer_lifecycle_batch)
                    .await
                {
                    self.session.push(Message::User(UserMessage {
                        content: peer_update,
                    }));
                    had_session_injections = true;
                }

                // Checkpoint after inline session injections mutate session state.
                // Responses bypass the LLM loop (no run call), so without
                // this checkpoint they would only be persisted if a later
                // request/message triggers its own checkpoint.
                if had_session_injections && let Some(ref cp) = self.checkpointer {
                    cp.checkpoint(&self.session).await;
                }

                // Process individual interactions (requests, or subscriber-bound)
                for (interaction, subscriber) in individual {
                    let has_tap = match subscriber {
                        Some(tx) => {
                            self.event_tap
                                .lock()
                                .replace(crate::event_tap::EventTapState {
                                    tx,
                                    truncated: AtomicBool::new(false),
                                });
                            true
                        }
                        None => false,
                    };

                    let run_result = match &event_tx {
                        Some(tx) => {
                            self.run_with_events(interaction.rendered_text, tx.clone())
                                .await
                        }
                        None => self.run(interaction.rendered_text).await,
                    };

                    match run_result {
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
                            // Explicitly transition reservation FSM to Completed.
                            comms.mark_interaction_complete(&interaction.id);
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
                            // Explicitly transition reservation FSM to Completed on failure too.
                            comms.mark_interaction_complete(&interaction.id);
                            self.event_tap.lock().take();

                            if e.is_graceful() {
                                tracing::info!("Host mode: graceful exit - {}", e);
                                self.host_drain_active = false;
                                return Ok(last_result);
                            }
                            self.host_drain_active = false;
                            return Err(e);
                        }
                    }

                    // Clear tap after each interaction
                    self.event_tap.lock().take();

                    // Checkpoint after each individual interaction
                    if let Some(ref cp) = self.checkpointer {
                        cp.checkpoint(&self.session).await;
                    }
                }

                // Process batched messages as one run (no tap)
                if !batched_texts.is_empty() {
                    let combined = batched_texts.join("\n\n");
                    tracing::debug!(
                        "Host mode: processing {} batched message(s)",
                        batched_texts.len()
                    );
                    let batch_result = match &event_tx {
                        Some(tx) => self.run_with_events(combined, tx.clone()).await,
                        None => self.run(combined).await,
                    };
                    match batch_result {
                        Ok(result) => {
                            last_result = result;
                            // Checkpoint after batched messages
                            if let Some(ref cp) = self.checkpointer {
                                cp.checkpoint(&self.session).await;
                            }
                        }
                        Err(e) => {
                            if e.is_graceful() {
                                tracing::info!("Host mode: graceful exit - {}", e);
                                self.host_drain_active = false;
                                return Ok(last_result);
                            }
                            self.host_drain_active = false;
                            return Err(e);
                        }
                    }
                }
                continue;
            }

            tokio::select! {
                _ = notified => {}
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

fn extract_peer_lifecycle_update(
    interaction: &InboxInteraction,
) -> Option<(String, PeerLifecycleState)> {
    let (intent, params) = match &interaction.content {
        InteractionContent::Request { intent, params } => (intent.as_str(), params),
        _ => return None,
    };

    let state = match intent {
        PEER_ADDED_INTENT => PeerLifecycleState::Added,
        PEER_RETIRED_INTENT => PeerLifecycleState::Retired,
        _ => return None,
    };

    let peer = params
        .get("peer")
        .and_then(|value| value.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(interaction.from.as_str())
        .to_string();

    Some((peer, state))
}

fn is_silent_request_intent(interaction: &InboxInteraction, silent_intents: &[String]) -> bool {
    match &interaction.content {
        InteractionContent::Request { intent, .. } => silent_intents.iter().any(|s| s == intent),
        _ => false,
    }
}

fn classify_comms_interaction(
    interaction: &InboxInteraction,
    silent_intents: &[String],
) -> CommsInteractionClass {
    if let Some((peer, state)) = extract_peer_lifecycle_update(interaction) {
        return CommsInteractionClass::PeerLifecycle { peer, state };
    }
    if matches!(&interaction.content, InteractionContent::Response { .. })
        || is_silent_request_intent(interaction, silent_intents)
    {
        return CommsInteractionClass::InlineSessionOnly;
    }
    CommsInteractionClass::Passthrough
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
    format!("{}, (+{} more)", displayed.join(", "), extra)
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
                    "[PEER UPDATE] Peer updates suppressed at current scale ({} peers). Use peers() to inspect peers.",
                    peer_count
                ));
            }
            return None;
        }

        let resumed = self.peer_notification_suppression_active;
        self.peer_notification_suppression_active = false;
        let summary = render_peer_update_summary(batch)?;
        if resumed {
            return Some(format!(
                "[PEER UPDATE] Peer updates resumed at current scale ({} peers).\n{}",
                peer_count, summary
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
    use crate::comms::{PeerDirectoryEntry, PeerDirectorySource, PeerName};
    use crate::error::{AgentError, LlmFailureReason};
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

    // Mock LLM client that always fails.
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
        peer_count: AtomicUsize,
    }

    impl SyncInteractionMockCommsRuntime {
        fn with_interactions(interactions: Vec<crate::interaction::InboxInteraction>) -> Self {
            Self {
                interactions: Mutex::new(interactions),
                subscribers: parking_lot::Mutex::new(std::collections::HashMap::new()),
                notify: Arc::new(Notify::new()),
                dismiss: std::sync::atomic::AtomicBool::new(false),
                had_interactions: std::sync::atomic::AtomicBool::new(false),
                peer_count: AtomicUsize::new(0),
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

        async fn drain_inbox_interactions(&self) -> Vec<crate::interaction::InboxInteraction> {
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

        async fn peers(&self) -> Vec<PeerDirectoryEntry> {
            let count = self.peer_count.load(Ordering::SeqCst);
            (0..count)
                .map(|idx| PeerDirectoryEntry {
                    name: PeerName::new(format!("peer-{idx}")).expect("valid peer name"),
                    peer_id: format!("ed25519:peer-{idx}"),
                    address: format!("inproc://peer-{idx}"),
                    source: PeerDirectorySource::Inproc,
                    sendable_kinds: vec!["message".to_string(), "request".to_string()],
                    capabilities: serde_json::json!({}),
                    meta: crate::peer_meta::PeerMeta::default(),
                })
                .collect()
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

    #[async_trait]
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
                    meta: crate::peer_meta::PeerMeta::default(),
                })
                .collect()
        }

        async fn peer_count(&self) -> usize {
            self.peer_count.load(Ordering::SeqCst)
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

    #[test]
    fn test_classify_comms_interaction_peer_lifecycle() {
        let interaction = make_interaction(
            InteractionContent::Request {
                intent: "mob.peer_added".into(),
                params: serde_json::json!({"peer": "worker-1"}),
            },
            "peer add",
        );
        assert!(matches!(
            classify_comms_interaction(&interaction, &[]),
            CommsInteractionClass::PeerLifecycle { .. }
        ));
    }

    #[test]
    fn test_classify_comms_interaction_silent_request_is_inline_only() {
        let interaction = make_interaction(
            InteractionContent::Request {
                intent: "mob.status.ping".into(),
                params: serde_json::json!({}),
            },
            "status ping",
        );
        assert_eq!(
            classify_comms_interaction(&interaction, &["mob.status.ping".to_string()]),
            CommsInteractionClass::InlineSessionOnly
        );
    }

    #[test]
    fn test_classify_comms_interaction_non_silent_request_passthrough() {
        let interaction = make_interaction(
            InteractionContent::Request {
                intent: "review.code".into(),
                params: serde_json::json!({}),
            },
            "review request",
        );
        assert_eq!(
            classify_comms_interaction(&interaction, &["mob.peer_added".to_string()]),
            CommsInteractionClass::Passthrough
        );
    }

    #[tokio::test]
    async fn test_response_interaction_injected_into_session_not_run() {
        let response_id = crate::interaction::InteractionId(uuid::Uuid::new_v4());
        let response = crate::interaction::InboxInteraction {
            id: crate::interaction::InteractionId(uuid::Uuid::new_v4()),
            from: "peer".into(),
            content: InteractionContent::Response {
                in_reply_to: response_id,
                status: crate::interaction::ResponseStatus::Completed,
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
    async fn test_host_mode_uses_default_event_channel_when_configured() {
        let interaction = make_interaction(
            InteractionContent::Message {
                body: "hello".into(),
            },
            "hello",
        );

        let comms = Arc::new(SyncInteractionMockCommsRuntime::with_interactions(vec![
            interaction,
        ]));
        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(4096);

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
            .with_default_event_tx(event_tx)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        let _result = agent.run_host_mode("".into()).await.unwrap();

        let mut saw_run_started = false;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(event, AgentEvent::RunStarted { .. }) {
                saw_run_started = true;
                break;
            }
        }
        assert!(
            saw_run_started,
            "expected RunStarted on default host-mode event channel"
        );
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
    async fn test_events_host_mode_subscriber_receives_interaction_failed_before_error() {
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
                Arc::new(FailingLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        let (event_tx, _event_rx) = mpsc::channel::<AgentEvent>(4096);
        let err = agent
            .run_host_mode_with_events("".into(), event_tx)
            .await
            .expect_err("run should fail");

        // Subscriber should be consumed despite error
        assert!(comms.subscribers.lock().is_empty());

        let mut saw_failed = false;
        while let Ok(event) = sub_rx.try_recv() {
            if let AgentEvent::InteractionFailed {
                interaction_id: id,
                error,
            } = event
            {
                assert_eq!(id, interaction_id);
                assert!(
                    error.contains("forced failure"),
                    "unexpected error payload: {}",
                    error
                );
                saw_failed = true;
                break;
            }
        }
        assert!(
            saw_failed,
            "expected InteractionFailed on subscriber channel"
        );
        assert!(err.to_string().contains("forced failure"));
    }

    #[tokio::test]
    async fn test_silent_intent_injected_not_run() {
        let interaction = make_interaction(
            InteractionContent::Request {
                intent: "mob.peer_added".into(),
                params: serde_json::json!({"peer": "worker-1"}),
            },
            "[mob.peer_added] worker-1 joined",
        );

        let comms = Arc::new(SyncInteractionMockCommsRuntime::with_interactions(vec![
            interaction,
        ]));

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms.clone() as Arc<dyn CommsRuntime>)
            .with_silent_comms_intents(vec!["mob.peer_added".into()])
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        let result = agent.run_host_mode(String::new()).await.unwrap();

        // Silent intent should have been injected into session as a user message
        let user_msgs: Vec<_> = agent
            .session
            .messages()
            .iter()
            .filter(|m| matches!(m, Message::User(_)))
            .collect();
        assert_eq!(user_msgs.len(), 1);
        match &user_msgs[0] {
            Message::User(u) => {
                assert!(u.content.contains("[PEER UPDATE]"));
                assert!(u.content.contains("worker-1"));
            }
            _ => unreachable!(),
        }

        // Result should be the empty initial (no LLM turn for silent intent)
        assert_eq!(result.turns, 0);
    }

    #[tokio::test]
    async fn test_non_silent_intent_processed_normally() {
        let interaction = make_interaction(
            InteractionContent::Request {
                intent: "review.code".into(),
                params: serde_json::json!({}),
            },
            "Please review this code",
        );

        let comms = Arc::new(SyncInteractionMockCommsRuntime::with_interactions(vec![
            interaction,
        ]));

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
            .with_silent_comms_intents(vec!["mob.peer_added".into()])
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        let result = agent.run_host_mode(String::new()).await.unwrap();

        // Non-silent intent should be processed through LLM
        assert!(result.turns > 0);
        assert_eq!(result.text, "Done");
    }

    #[tokio::test]
    async fn test_peer_lifecycle_batching_in_host_mode_collapses_to_one_entry() {
        let interactions = vec![
            make_interaction(
                InteractionContent::Request {
                    intent: "mob.peer_added".into(),
                    params: serde_json::json!({"peer": "worker-1"}),
                },
                "peer add 1",
            ),
            make_interaction(
                InteractionContent::Request {
                    intent: "mob.peer_added".into(),
                    params: serde_json::json!({"peer": "worker-2"}),
                },
                "peer add 2",
            ),
            make_interaction(
                InteractionContent::Request {
                    intent: "mob.peer_added".into(),
                    params: serde_json::json!({"peer": "worker-3"}),
                },
                "peer add 3",
            ),
        ];
        let comms = Arc::new(SyncInteractionMockCommsRuntime::with_interactions(
            interactions,
        ));

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        let result = agent.run_host_mode(String::new()).await.unwrap();
        assert_eq!(result.turns, 0);

        let user_msgs: Vec<_> = agent
            .session
            .messages()
            .iter()
            .filter(|m| matches!(m, Message::User(_)))
            .collect();
        assert_eq!(user_msgs.len(), 1);
        let text = match &user_msgs[0] {
            Message::User(u) => u.content.as_str(),
            _ => unreachable!(),
        };
        assert!(text.contains("[PEER UPDATE]"));
        assert!(text.contains("3 peers connected"));
    }

    #[tokio::test]
    async fn test_peer_lifecycle_net_out_cancels_opposites() {
        let interactions = vec![
            make_interaction(
                InteractionContent::Request {
                    intent: "mob.peer_added".into(),
                    params: serde_json::json!({"peer": "worker-a"}),
                },
                "add a",
            ),
            make_interaction(
                InteractionContent::Request {
                    intent: "mob.peer_retired".into(),
                    params: serde_json::json!({"peer": "worker-a"}),
                },
                "retire a",
            ),
            make_interaction(
                InteractionContent::Request {
                    intent: "mob.peer_added".into(),
                    params: serde_json::json!({"peer": "worker-b"}),
                },
                "add b",
            ),
        ];
        let comms = Arc::new(SyncInteractionMockCommsRuntime::with_interactions(
            interactions,
        ));

        let mut agent = AgentBuilder::new()
            .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
            .build(
                Arc::new(MockLlmClient),
                Arc::new(MockToolDispatcher),
                Arc::new(MockSessionStore),
            )
            .await;

        let result = agent.run_host_mode(String::new()).await.unwrap();
        assert_eq!(result.turns, 0);

        let user_msgs: Vec<_> = agent
            .session
            .messages()
            .iter()
            .filter(|m| matches!(m, Message::User(_)))
            .collect();
        assert_eq!(user_msgs.len(), 1);
        let text = match &user_msgs[0] {
            Message::User(u) => u.content.as_str(),
            _ => unreachable!(),
        };
        assert!(text.contains("1 peer connected"));
        assert!(text.contains("worker-b"));
        assert!(!text.contains("worker-a"));
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
                Message::User(u) => Some(u.content.clone()),
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
                Message::User(u) => Some(u.content.clone()),
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
                Message::User(u) => Some(u.content.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(user_msgs.len(), 1);
        assert!(user_msgs[0].contains("[PEER UPDATE]"));
        assert!(user_msgs[0].contains("2 peers connected"));
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
                status: crate::interaction::ResponseStatus::Completed,
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
