#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Regression tests for host-mode / comms continuation semantics.
//!
//! These are integration-level tests (tests/ dir) that exercise the public API
//! of meerkat-core: `AgentBuilder`, `CommsRuntime`, `run_host_mode()`.
//! No API keys, no `#[ignore]` — runs in `cargo rct`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use meerkat_core::agent::AgentBuilder;
use meerkat_core::error::ToolError;
use meerkat_core::types::{AssistantBlock, Message, StopReason, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{
    AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, CommsRuntime,
    InboxInteraction, InteractionContent, InteractionId, LlmStreamResult, PeerDirectoryEntry,
    PeerDirectorySource, PeerMeta, PeerName, ResponseStatus,
};
use serde_json::Value;
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Mock LLM client — returns a single text block + EndTurn
// ---------------------------------------------------------------------------
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
            meerkat_core::types::Usage::default(),
        ))
    }

    fn provider(&self) -> &'static str {
        "mock"
    }
}

// ---------------------------------------------------------------------------
// Mock tool dispatcher — no tools
// ---------------------------------------------------------------------------
struct MockToolDispatcher;

#[async_trait]
impl AgentToolDispatcher for MockToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::new([])
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        Err(ToolError::NotFound {
            name: call.name.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Mock session store — no-op
// ---------------------------------------------------------------------------
struct MockSessionStore;

#[async_trait]
impl AgentSessionStore for MockSessionStore {
    async fn save(&self, _session: &meerkat_core::Session) -> Result<(), AgentError> {
        Ok(())
    }

    async fn load(&self, _id: &str) -> Result<Option<meerkat_core::Session>, AgentError> {
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Mock CommsRuntime — returns interactions once, then auto-dismisses.
//
// Auto-dismiss: after the first drain that returned interactions, the next
// empty drain sets dismiss=true so the host loop exits cleanly.
// ---------------------------------------------------------------------------
struct MockComms {
    interactions: Mutex<Vec<InboxInteraction>>,
    notify: Arc<Notify>,
    dismiss: AtomicBool,
    had_interactions: AtomicBool,
    peer_count: AtomicUsize,
}

impl MockComms {
    fn with_interactions(interactions: Vec<InboxInteraction>) -> Self {
        Self {
            interactions: Mutex::new(interactions),
            notify: Arc::new(Notify::new()),
            dismiss: AtomicBool::new(false),
            had_interactions: AtomicBool::new(false),
            peer_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl CommsRuntime for MockComms {
    async fn drain_messages(&self) -> Vec<String> {
        vec![]
    }

    fn inbox_notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    async fn drain_inbox_interactions(&self) -> Vec<InboxInteraction> {
        let mut guard = self.interactions.lock().await;
        let result = std::mem::take(&mut *guard);
        if !result.is_empty() {
            self.had_interactions.store(true, Ordering::SeqCst);
        } else if self.had_interactions.load(Ordering::SeqCst) {
            self.dismiss.store(true, Ordering::SeqCst);
        }
        result
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
                meta: PeerMeta::default(),
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Staged mock: returns one batch per drain call, auto-dismisses after all
// batches consumed.
// ---------------------------------------------------------------------------
struct StagedMockComms {
    batches: Mutex<Vec<Vec<InboxInteraction>>>,
    notify: Arc<Notify>,
    dismiss: AtomicBool,
    had_interactions: AtomicBool,
}

impl StagedMockComms {
    fn with_batches(batches: Vec<Vec<InboxInteraction>>) -> Self {
        Self {
            batches: Mutex::new(batches),
            notify: Arc::new(Notify::new()),
            dismiss: AtomicBool::new(false),
            had_interactions: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl CommsRuntime for StagedMockComms {
    async fn drain_messages(&self) -> Vec<String> {
        vec![]
    }

    fn inbox_notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    async fn drain_inbox_interactions(&self) -> Vec<InboxInteraction> {
        let mut batches = self.batches.lock().await;
        if batches.is_empty() {
            if self.had_interactions.load(Ordering::SeqCst) {
                self.dismiss.store(true, Ordering::SeqCst);
            }
            Vec::new()
        } else {
            let batch = batches.remove(0);
            if !batch.is_empty() {
                self.had_interactions.store(true, Ordering::SeqCst);
            }
            batch
        }
    }

    fn dismiss_received(&self) -> bool {
        self.dismiss.load(Ordering::SeqCst)
    }

    async fn peers(&self) -> Vec<PeerDirectoryEntry> {
        vec![]
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_interaction(content: InteractionContent, rendered_text: &str) -> InboxInteraction {
    InboxInteraction {
        id: InteractionId(Uuid::new_v4()),
        from: "test-peer".into(),
        content,
        rendered_text: rendered_text.to_string(),
    }
}

fn make_response(status: ResponseStatus, rendered_text: &str) -> InboxInteraction {
    let reply_to = InteractionId(Uuid::new_v4());
    InboxInteraction {
        id: InteractionId(Uuid::new_v4()),
        from: "peer".into(),
        content: InteractionContent::Response {
            in_reply_to: reply_to,
            status,
            result: serde_json::json!({"ok": true}),
        },
        rendered_text: rendered_text.to_string(),
    }
}

// ---------------------------------------------------------------------------
// 1. completed_response_triggers_continuation_run
// ---------------------------------------------------------------------------
#[tokio::test]
async fn completed_response_triggers_continuation_run() {
    let response = make_response(
        ResponseStatus::Completed,
        "[Response] completed: {\"ok\":true}",
    );
    let comms = Arc::new(MockComms::with_interactions(vec![response]));

    let mut agent = AgentBuilder::new()
        .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
        .build(
            Arc::new(MockLlmClient),
            Arc::new(MockToolDispatcher),
            Arc::new(MockSessionStore),
        )
        .await;

    let result = agent.run_host_mode(String::new().into()).await.unwrap();

    assert!(
        result.turns > 0,
        "completed response should trigger continuation run, got 0 turns"
    );

    let assistant_msgs: Vec<_> = agent
        .session()
        .messages()
        .iter()
        .filter(|m| matches!(m, Message::Assistant(_) | Message::BlockAssistant(_)))
        .collect();
    assert!(
        !assistant_msgs.is_empty(),
        "expected assistant message from continuation run"
    );
}

// ---------------------------------------------------------------------------
// 2. accepted_response_injects_context_no_continuation
// ---------------------------------------------------------------------------
#[tokio::test]
async fn accepted_response_injects_context_no_continuation() {
    let response = make_response(
        ResponseStatus::Accepted,
        "[Response] accepted: {\"ack\":true}",
    );
    let comms = Arc::new(MockComms::with_interactions(vec![response]));

    let mut agent = AgentBuilder::new()
        .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
        .build(
            Arc::new(MockLlmClient),
            Arc::new(MockToolDispatcher),
            Arc::new(MockSessionStore),
        )
        .await;

    let result = agent.run_host_mode(String::new().into()).await.unwrap();

    assert_eq!(
        result.turns, 0,
        "accepted response should not trigger continuation run"
    );

    // But it should still be injected into the session as context.
    let user_msgs: Vec<_> = agent
        .session()
        .messages()
        .iter()
        .filter(|m| matches!(m, Message::User(_)))
        .collect();
    assert_eq!(
        user_msgs.len(),
        1,
        "accepted response should be injected as user message"
    );
}

// ---------------------------------------------------------------------------
// 3. failed_response_triggers_continuation_run
// ---------------------------------------------------------------------------
#[tokio::test]
async fn failed_response_triggers_continuation_run() {
    let response = make_response(
        ResponseStatus::Failed,
        "[Response] failed: {\"error\":\"timeout\"}",
    );
    let comms = Arc::new(MockComms::with_interactions(vec![response]));

    let mut agent = AgentBuilder::new()
        .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
        .build(
            Arc::new(MockLlmClient),
            Arc::new(MockToolDispatcher),
            Arc::new(MockSessionStore),
        )
        .await;

    let result = agent.run_host_mode(String::new().into()).await.unwrap();

    assert!(
        result.turns > 0,
        "failed response should trigger continuation run"
    );
}

// ---------------------------------------------------------------------------
// 4. response_with_passthrough_message_single_turn
// ---------------------------------------------------------------------------
#[tokio::test]
async fn response_with_passthrough_message_single_turn() {
    let response = make_response(
        ResponseStatus::Completed,
        "[Response] completed: {\"ok\":true}",
    );
    let message = make_interaction(
        InteractionContent::Message {
            body: "hello".into(),
            blocks: None,
        },
        "hello from peer",
    );

    let comms = Arc::new(MockComms::with_interactions(vec![response, message]));

    let mut agent = AgentBuilder::new()
        .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
        .build(
            Arc::new(MockLlmClient),
            Arc::new(MockToolDispatcher),
            Arc::new(MockSessionStore),
        )
        .await;

    let result = agent.run_host_mode(String::new().into()).await.unwrap();

    // The passthrough message triggers the run; the response is absorbed
    // as context. Should be exactly 1 turn (the message run), not 2.
    assert_eq!(
        result.turns, 1,
        "response alongside message should not trigger a separate continuation"
    );

    // Response should still be injected into session as context.
    let user_texts: Vec<_> = agent
        .session()
        .messages()
        .iter()
        .filter_map(|m| match m {
            Message::User(u) => Some(u.text_content()),
            _ => None,
        })
        .collect();
    assert!(
        user_texts.iter().any(|c| c.contains("completed")),
        "response should be injected as session context"
    );
}

// ---------------------------------------------------------------------------
// 5. response_after_completed_host_turn_triggers_continuation
// ---------------------------------------------------------------------------
#[tokio::test]
async fn response_after_completed_host_turn_triggers_continuation() {
    let message = make_interaction(
        InteractionContent::Message {
            body: "do something".into(),
            blocks: None,
        },
        "do something",
    );
    let response = make_response(
        ResponseStatus::Completed,
        "[Response] completed: {\"review\":\"looks good\"}",
    );

    // Batch 1: message -> triggers a normal host turn
    // Batch 2: response -> should trigger continuation via run_pending_inner
    let comms = Arc::new(StagedMockComms::with_batches(vec![
        vec![message],
        vec![response],
    ]));

    let mut agent = AgentBuilder::new()
        .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
        .build(
            Arc::new(MockLlmClient),
            Arc::new(MockToolDispatcher),
            Arc::new(MockSessionStore),
        )
        .await;

    let result = agent.run_host_mode(String::new().into()).await.unwrap();

    // result.turns is per-run (the last run), not cumulative.
    // The continuation should have fired (turns > 0).
    assert!(
        result.turns > 0,
        "continuation run should have fired after response injection"
    );

    // Session should have the full sequence:
    // User("do something") -> Assistant -> User("[Response]...") -> Assistant
    let msgs: Vec<String> = agent
        .session()
        .messages()
        .iter()
        .filter_map(|m| match m {
            Message::User(u) => Some(u.text_content()),
            Message::BlockAssistant(_) | Message::Assistant(_) => Some("[assistant]".to_string()),
            _ => None,
        })
        .collect();

    assert!(
        msgs.iter().any(|c| c.contains("looks good")),
        "response should be in session history: {msgs:?}"
    );

    let assistant_count = msgs.iter().filter(|m| m.as_str() == "[assistant]").count();
    assert!(
        assistant_count >= 2,
        "expected at least 2 assistant messages (initial turn + continuation), got {assistant_count}: {msgs:?}"
    );
}

// ---------------------------------------------------------------------------
// 6. peer_lifecycle_batching_collapses_to_one_entry
// ---------------------------------------------------------------------------
#[tokio::test]
async fn peer_lifecycle_batching_collapses_to_one_entry() {
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
    let comms = Arc::new(MockComms::with_interactions(interactions));

    let mut agent = AgentBuilder::new()
        .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
        .build(
            Arc::new(MockLlmClient),
            Arc::new(MockToolDispatcher),
            Arc::new(MockSessionStore),
        )
        .await;

    let result = agent.run_host_mode(String::new().into()).await.unwrap();
    assert_eq!(
        result.turns, 0,
        "peer lifecycle should not trigger LLM turn"
    );

    let user_msgs: Vec<_> = agent
        .session()
        .messages()
        .iter()
        .filter_map(|m| match m {
            Message::User(u) => Some(u.text_content()),
            _ => None,
        })
        .collect();
    assert_eq!(
        user_msgs.len(),
        1,
        "should collapse to single [PEER UPDATE]"
    );
    assert!(user_msgs[0].contains("[PEER UPDATE]"));
    assert!(user_msgs[0].contains("3 peers connected"));
}

// ---------------------------------------------------------------------------
// 7. peer_lifecycle_net_out_cancels_opposites
// ---------------------------------------------------------------------------
#[tokio::test]
async fn peer_lifecycle_net_out_cancels_opposites() {
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
    let comms = Arc::new(MockComms::with_interactions(interactions));

    let mut agent = AgentBuilder::new()
        .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
        .build(
            Arc::new(MockLlmClient),
            Arc::new(MockToolDispatcher),
            Arc::new(MockSessionStore),
        )
        .await;

    let result = agent.run_host_mode(String::new().into()).await.unwrap();
    assert_eq!(result.turns, 0);

    let user_msgs: Vec<_> = agent
        .session()
        .messages()
        .iter()
        .filter_map(|m| match m {
            Message::User(u) => Some(u.text_content()),
            _ => None,
        })
        .collect();
    assert_eq!(user_msgs.len(), 1);
    assert!(user_msgs[0].contains("1 peer connected"));
    assert!(user_msgs[0].contains("worker-b"));
    assert!(
        !user_msgs[0].contains("worker-a"),
        "worker-a add+retire should cancel out"
    );
}

// ---------------------------------------------------------------------------
// 8. silent_comms_intent_no_llm_turn
// ---------------------------------------------------------------------------
#[tokio::test]
async fn silent_comms_intent_no_llm_turn() {
    // mob.peer_added is treated as both peer-lifecycle AND a silent intent here.
    // When it is in the silent intents list AND is a peer lifecycle intent,
    // the peer lifecycle classification takes priority (checked first in classify).
    // So the context is injected as [PEER UPDATE], but no LLM turn fires.
    let interaction = make_interaction(
        InteractionContent::Request {
            intent: "mob.status.ping".into(),
            params: serde_json::json!({}),
        },
        "[mob.status.ping] heartbeat",
    );

    let comms = Arc::new(MockComms::with_interactions(vec![interaction]));

    let mut agent = AgentBuilder::new()
        .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
        .with_silent_comms_intents(vec!["mob.status.ping".into()])
        .build(
            Arc::new(MockLlmClient),
            Arc::new(MockToolDispatcher),
            Arc::new(MockSessionStore),
        )
        .await;

    let result = agent.run_host_mode(String::new().into()).await.unwrap();

    assert_eq!(result.turns, 0, "silent intent should not trigger LLM turn");

    // Context should be injected into the session.
    let user_msgs: Vec<_> = agent
        .session()
        .messages()
        .iter()
        .filter(|m| matches!(m, Message::User(_)))
        .collect();
    assert_eq!(
        user_msgs.len(),
        1,
        "silent intent should inject context into session"
    );
}

// ---------------------------------------------------------------------------
// 9. non_silent_comms_intent_triggers_llm_turn
// ---------------------------------------------------------------------------
#[tokio::test]
async fn non_silent_comms_intent_triggers_llm_turn() {
    let interaction = make_interaction(
        InteractionContent::Request {
            intent: "review.code".into(),
            params: serde_json::json!({}),
        },
        "Please review this code",
    );

    let comms = Arc::new(MockComms::with_interactions(vec![interaction]));

    let mut agent = AgentBuilder::new()
        .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
        .with_silent_comms_intents(vec!["mob.peer_added".into()])
        .build(
            Arc::new(MockLlmClient),
            Arc::new(MockToolDispatcher),
            Arc::new(MockSessionStore),
        )
        .await;

    let result = agent.run_host_mode(String::new().into()).await.unwrap();

    assert!(
        result.turns > 0,
        "non-silent intent should trigger LLM turn"
    );
    assert_eq!(result.text, "Done");
}

// ---------------------------------------------------------------------------
// 10. message_interaction_triggers_host_mode_run
// ---------------------------------------------------------------------------
#[tokio::test]
async fn message_interaction_triggers_host_mode_run() {
    let message = make_interaction(
        InteractionContent::Message {
            body: "hello world".into(),
            blocks: None,
        },
        "hello world",
    );
    let comms = Arc::new(MockComms::with_interactions(vec![message]));

    let mut agent = AgentBuilder::new()
        .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
        .build(
            Arc::new(MockLlmClient),
            Arc::new(MockToolDispatcher),
            Arc::new(MockSessionStore),
        )
        .await;

    let result = agent.run_host_mode(String::new().into()).await.unwrap();

    assert!(
        result.turns > 0,
        "plain message should trigger host mode run"
    );
    assert_eq!(result.text, "Done");
}

// ---------------------------------------------------------------------------
// 11. request_interaction_triggers_host_mode_run
// ---------------------------------------------------------------------------
#[tokio::test]
async fn request_interaction_triggers_host_mode_run() {
    let request = make_interaction(
        InteractionContent::Request {
            intent: "analyze.data".into(),
            params: serde_json::json!({"dataset": "sales"}),
        },
        "Please analyze the sales dataset",
    );
    let comms = Arc::new(MockComms::with_interactions(vec![request]));

    let mut agent = AgentBuilder::new()
        .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
        .build(
            Arc::new(MockLlmClient),
            Arc::new(MockToolDispatcher),
            Arc::new(MockSessionStore),
        )
        .await;

    let result = agent.run_host_mode(String::new().into()).await.unwrap();

    assert!(
        result.turns > 0,
        "request intent should trigger host mode run"
    );
    assert_eq!(result.text, "Done");
}

// ---------------------------------------------------------------------------
// 12. empty_inbox_no_turns
// ---------------------------------------------------------------------------
#[tokio::test]
async fn empty_inbox_no_turns() {
    // Empty interactions list — auto-dismiss fires immediately because
    // had_interactions starts false, so we need a different approach.
    // Use a mock that dismisses on the first empty drain.
    struct ImmediateDismissComms {
        notify: Arc<Notify>,
    }

    #[async_trait]
    impl CommsRuntime for ImmediateDismissComms {
        async fn drain_messages(&self) -> Vec<String> {
            vec![]
        }

        fn inbox_notify(&self) -> Arc<Notify> {
            self.notify.clone()
        }

        async fn drain_inbox_interactions(&self) -> Vec<InboxInteraction> {
            Vec::new()
        }

        fn dismiss_received(&self) -> bool {
            true
        }

        async fn peers(&self) -> Vec<PeerDirectoryEntry> {
            vec![]
        }
    }

    let comms = Arc::new(ImmediateDismissComms {
        notify: Arc::new(Notify::new()),
    });

    let mut agent = AgentBuilder::new()
        .with_comms_runtime(comms as Arc<dyn CommsRuntime>)
        .build(
            Arc::new(MockLlmClient),
            Arc::new(MockToolDispatcher),
            Arc::new(MockSessionStore),
        )
        .await;

    let result = agent.run_host_mode(String::new().into()).await.unwrap();

    assert_eq!(result.turns, 0, "empty inbox should not trigger any turns");
}
