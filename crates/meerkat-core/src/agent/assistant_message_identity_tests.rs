//! Assistant message identity through the provider-neutral agent loop.
//!
//! A console pairs live assistant rows with canonical history by
//! `assistant_message_id` alone. These tests pin the contract end to end on
//! the loop: the id is minted at provider turn start before any delta, the
//! same value is on every live event of the message and on the committed
//! history row, retries reuse it, failures never leave it committed, and
//! identical text never collapses two messages into one.
//!
//! The scripted client streams through the unified request-attempt path and
//! publishes adapter-shaped live events into the run's own event channel,
//! stamped with the id the loop hands to `stream_response`, exactly like
//! `meerkat_llm_core::LlmClientAdapter` does for every provider.
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::too_many_lines
)]

use crate as meerkat_core;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_core::lifecycle::run_primitive::ProviderParamsOverride;
use meerkat_core::{
    AgentBuilder, AgentError, AgentEvent, AgentLlmClient, AgentLlmRequestAttempt,
    AgentSessionStore, AgentToolDispatcher, AssistantBlock, AssistantMessageId, HookDecision,
    HookEngine, HookExecutionReport, HookId, HookInvocation, HookOutcome, HookPoint,
    HookReasonCode, LlmStreamResult, Message, Provider, RequestAttemptAuthority, ServerToolKind,
    StopReason, ToolCallView, ToolDef, ToolResult, Usage,
};
use serde_json::value::RawValue;
use tokio::sync::mpsc;

const MODEL: &str = "identity-model";

/// One adapter-shaped live event a provider stream publishes.
#[derive(Clone)]
enum Live {
    Text(&'static str),
    Reasoning(&'static str),
    ReasoningComplete(&'static str),
    ServerTool,
}

impl Live {
    fn stamped(&self, id: AssistantMessageId) -> AgentEvent {
        let assistant_message_id = Some(id);
        match self {
            Live::Text(delta) => AgentEvent::TextDelta {
                delta: (*delta).to_string(),
                assistant_message_id,
            },
            Live::Reasoning(delta) => AgentEvent::ReasoningDelta {
                delta: (*delta).to_string(),
                assistant_message_id,
            },
            Live::ReasoningComplete(content) => AgentEvent::ReasoningComplete {
                content: (*content).to_string(),
                assistant_message_id,
            },
            Live::ServerTool => AgentEvent::ServerToolContent {
                id: Some("srv-1".to_string()),
                kind: ServerToolKind::WebSearch,
                content: serde_json::json!({"query": "rust"}),
                assistant_message_id,
            },
        }
    }
}

/// How one scripted provider attempt ends.
enum Outcome {
    Reply(Vec<AssistantBlock>, StopReason),
    Retryable,
    Fatal,
}

struct Call {
    live: Vec<Live>,
    outcome: Outcome,
    usage: Usage,
}

impl Call {
    fn reply(live: Vec<Live>, blocks: Vec<AssistantBlock>, stop_reason: StopReason) -> Self {
        Self {
            live,
            outcome: Outcome::Reply(blocks, stop_reason),
            usage: Usage::default(),
        }
    }

    fn text(text: &'static str) -> Self {
        Self::reply(
            vec![Live::Text(text)],
            vec![text_block(text)],
            StopReason::EndTurn,
        )
    }

    fn failing(live: Vec<Live>, outcome: Outcome) -> Self {
        Self {
            live,
            outcome,
            usage: Usage::default(),
        }
    }

    fn with_usage(mut self, input_tokens: u64, output_tokens: u64) -> Self {
        self.usage = Usage {
            input_tokens,
            output_tokens,
            ..Usage::default()
        };
        self
    }
}

fn text_block(text: &str) -> AssistantBlock {
    AssistantBlock::Text {
        text: text.to_string(),
        meta: None,
    }
}

fn tool_use_block(id: &str) -> AssistantBlock {
    AssistantBlock::ToolUse {
        id: id.to_string(),
        name: "lookup".into(),
        args: RawValue::from_string("{}".to_string()).unwrap(),
        meta: None,
    }
}

fn image_block(seed: u128) -> AssistantBlock {
    AssistantBlock::Image {
        image_id: crate::AssistantImageId::new(uuid::Uuid::from_u128(seed)),
        blob_ref: crate::BlobRef {
            blob_id: crate::BlobId::new(format!("image-{seed}")),
            media_type: "image/png".to_string(),
        },
        media_type: crate::MediaType::new("image/png"),
        width: 8,
        height: 8,
        revised_prompt: crate::RevisedPromptDisposition::NotRequested,
        meta: crate::ProviderImageMetadata::NotEmitted,
    }
}

/// Scripted provider on the unified attempt path. Every attempt records the
/// id the loop handed it and publishes its live events stamped with that id.
struct IdentityClient {
    script: Mutex<VecDeque<Call>>,
    events: mpsc::Sender<AgentEvent>,
    attempt_ids: Mutex<Vec<AssistantMessageId>>,
}

impl IdentityClient {
    fn new(events: mpsc::Sender<AgentEvent>, script: Vec<Call>) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            events,
            attempt_ids: Mutex::new(Vec::new()),
        })
    }

    fn attempt_ids(&self) -> Vec<AssistantMessageId> {
        self.attempt_ids.lock().unwrap().clone()
    }

    fn usage(&self, raw: Usage) -> Usage {
        crate::TurnUsage::host_declared(self.provider(), self.model(), raw).into_inner()
    }
}

struct IdentityAttempt {
    client: Arc<IdentityClient>,
}

#[async_trait]
impl AgentLlmRequestAttempt for IdentityAttempt {
    fn request_pressure(&self) -> Result<Option<crate::ProviderRequestPressure>, AgentError> {
        Ok(None)
    }

    async fn stream_response(
        &self,
        assistant_message_id: AssistantMessageId,
    ) -> Result<LlmStreamResult, AgentError> {
        self.client
            .attempt_ids
            .lock()
            .unwrap()
            .push(assistant_message_id);
        let call = self
            .client
            .script
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| AgentError::InternalError("identity script exhausted".to_string()))?;
        for live in &call.live {
            self.client
                .events
                .send(live.stamped(assistant_message_id))
                .await
                .expect("test event channel open");
        }
        match call.outcome {
            Outcome::Reply(blocks, stop_reason) => Ok(LlmStreamResult::new(
                blocks,
                stop_reason,
                self.client.usage(call.usage),
            )),
            Outcome::Retryable => Err(AgentError::llm(
                "mock",
                crate::error::LlmFailureReason::ProviderError(
                    crate::error::LlmProviderError::retryable(
                        crate::error::LlmProviderErrorKind::Unknown,
                        serde_json::json!({"message": "transient"}),
                    ),
                ),
                "transient provider failure",
            )),
            Outcome::Fatal => Err(AgentError::llm(
                "mock",
                crate::error::LlmFailureReason::ProviderError(
                    crate::error::LlmProviderError::non_retryable(
                        crate::error::LlmProviderErrorKind::Unknown,
                        serde_json::json!({"message": "fatal"}),
                    ),
                ),
                "fatal provider failure",
            )),
        }
    }
}

#[async_trait]
impl AgentLlmClient for IdentityClient {
    fn prepare_request_attempt(
        self: Arc<Self>,
        _messages: Arc<Vec<Message>>,
        _tools: Arc<[Arc<ToolDef>]>,
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<ProviderParamsOverride>,
    ) -> Result<Arc<dyn AgentLlmRequestAttempt>, AgentError> {
        Ok(Arc::new(IdentityAttempt { client: self }))
    }

    fn request_attempt_authority(&self) -> RequestAttemptAuthority {
        RequestAttemptAuthority::Unified
    }

    async fn stream_response(
        &self,
        _messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        Err(AgentError::InternalError(
            "the agent loop must stream through the request attempt".to_string(),
        ))
    }

    fn provider(&self) -> Provider {
        Provider::Other
    }

    fn model(&self) -> &'static str {
        MODEL
    }
}

/// `lookup` returns an observation; when `append_image` is set it also asks
/// the loop to append a separate assistant message carrying an image.
struct LookupTool {
    append_image: bool,
}

#[async_trait]
impl AgentToolDispatcher for LookupTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::new([Arc::new(ToolDef::new(
            "lookup",
            "returns a fixed observation",
            serde_json::json!({ "type": "object" }),
        ))])
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        let result = ToolResult::new(call.id.to_string(), "observation".to_string(), false);
        if !self.append_image {
            return Ok(result.into());
        }
        Ok(crate::ops::ToolDispatchOutcome::new(
            result,
            Vec::new(),
            vec![crate::ops::SessionEffect::AppendAssistantBlocks {
                blocks: vec![text_block("rendered chart"), image_block(42)],
            }],
        ))
    }
}

struct NoopStore;

#[async_trait]
impl AgentSessionStore for NoopStore {
    async fn save(&self, _session: &meerkat_core::Session) -> Result<(), AgentError> {
        Ok(())
    }

    async fn load(&self, _id: &str) -> Result<Option<meerkat_core::Session>, AgentError> {
        Ok(None)
    }
}

/// Denies the PreLlmRequest point while `deny` is set.
#[derive(Default)]
struct PreLlmDenyingHooks {
    deny: AtomicBool,
}

#[async_trait]
impl HookEngine for PreLlmDenyingHooks {
    async fn execute(
        &self,
        invocation: HookInvocation,
        _overrides: Option<&meerkat_core::HookRunOverrides>,
    ) -> Result<HookExecutionReport, meerkat_core::HookEngineError> {
        let decision = (invocation.point == HookPoint::PreLlmRequest
            && self.deny.load(Ordering::SeqCst))
        .then(|| {
            HookDecision::deny(
                HookId::new("deny-pre-llm"),
                HookReasonCode::PolicyViolation,
                "provider call blocked",
                None,
            )
        });
        let outcomes = decision
            .iter()
            .map(|decision| HookOutcome {
                hook_id: HookId::new("deny-pre-llm"),
                point: invocation.point,
                priority: 1,
                registration_index: 0,
                decision: Some(decision.clone()),
                failure_reason: None,
                duration_ms: Some(0),
            })
            .collect::<Vec<_>>();
        Ok(HookExecutionReport {
            started: outcomes
                .iter()
                .map(|outcome| outcome.hook_id.clone())
                .collect(),
            outcomes,
            decision,
        })
    }
}

/// Denies one hook point on every invocation.
struct PointDenyingHooks {
    point: HookPoint,
}

#[async_trait]
impl HookEngine for PointDenyingHooks {
    async fn execute(
        &self,
        invocation: HookInvocation,
        _overrides: Option<&meerkat_core::HookRunOverrides>,
    ) -> Result<HookExecutionReport, meerkat_core::HookEngineError> {
        let decision = (invocation.point == self.point).then(|| {
            HookDecision::deny(
                HookId::new("deny-point"),
                HookReasonCode::PolicyViolation,
                "result blocked",
                None,
            )
        });
        let outcomes = decision
            .iter()
            .map(|decision| HookOutcome {
                hook_id: HookId::new("deny-point"),
                point: invocation.point,
                priority: 1,
                registration_index: 0,
                decision: Some(decision.clone()),
                failure_reason: None,
                duration_ms: Some(0),
            })
            .collect::<Vec<_>>();
        Ok(HookExecutionReport {
            started: outcomes
                .iter()
                .map(|outcome| outcome.hook_id.clone())
                .collect(),
            outcomes,
            decision,
        })
    }
}

fn fast_retries(max_retries: u32) -> crate::retry::RetryPolicy {
    crate::retry::RetryPolicy {
        max_retries,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(1),
        multiplier: 1.0,
        call_timeout: None,
        stream_inactivity_timeout: None,
    }
}

fn builder() -> AgentBuilder {
    AgentBuilder::new()
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .retry_policy(fast_retries(2))
}

fn drain(rx: &mut mpsc::Receiver<AgentEvent>) -> Vec<AgentEvent> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

/// `(assistant_message_id, text)` of every committed assistant row, in order.
fn committed_rows(session: &meerkat_core::Session) -> Vec<(Option<AssistantMessageId>, String)> {
    session
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::BlockAssistant(assistant) => {
                Some((assistant.assistant_message_id, assistant.to_string()))
            }
            _ => None,
        })
        .collect()
}

fn turn_started_ids(events: &[AgentEvent]) -> Vec<Option<AssistantMessageId>> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TurnStarted {
                assistant_message_id,
                ..
            } => Some(*assistant_message_id),
            _ => None,
        })
        .collect()
}

fn turn_completed_ids(events: &[AgentEvent]) -> Vec<Option<AssistantMessageId>> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TurnCompleted {
                assistant_message_id,
                ..
            } => Some(*assistant_message_id),
            _ => None,
        })
        .collect()
}

fn run_completed_id(events: &[AgentEvent]) -> Option<AssistantMessageId> {
    let ids = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::RunCompleted {
                assistant_message_id,
                ..
            } => Some(*assistant_message_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 1, "exactly one run_completed");
    ids[0]
}

fn is_live_delta(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::TextDelta { .. }
            | AgentEvent::ReasoningDelta { .. }
            | AgentEvent::ReasoningComplete { .. }
            | AgentEvent::ServerToolContent { .. }
    )
}

#[tokio::test]
async fn one_id_names_every_live_event_and_the_committed_row() {
    let (tx, mut rx) = mpsc::channel(1024);
    let client = IdentityClient::new(
        tx.clone(),
        vec![Call::reply(
            vec![
                Live::Reasoning("think"),
                Live::ReasoningComplete("think"),
                Live::Text("before "),
                Live::ServerTool,
                Live::Text("after"),
            ],
            vec![
                AssistantBlock::Reasoning {
                    text: "think".to_string(),
                    meta: None,
                },
                text_block("before "),
                AssistantBlock::ServerToolContent {
                    id: Some("srv-1".to_string()),
                    kind: ServerToolKind::WebSearch,
                    content: serde_json::json!({"query": "rust"}),
                    meta: None,
                },
                text_block("after"),
                image_block(7),
            ],
            StopReason::EndTurn,
        )],
    );
    let mut agent = builder()
        .build_standalone(
            client.clone(),
            Arc::new(LookupTool {
                append_image: false,
            }),
            Arc::new(NoopStore),
        )
        .await;

    agent
        .run_with_events("hello".to_string().into(), tx)
        .await
        .expect("run completes");
    let events = drain(&mut rx);

    let rows = committed_rows(agent.session());
    assert_eq!(rows.len(), 1);
    let id = rows[0]
        .0
        .expect("the committed row carries the occurrence id");
    assert_eq!(
        client.attempt_ids(),
        vec![id],
        "the adapter streamed under it"
    );

    let started = events
        .iter()
        .position(|event| matches!(event, AgentEvent::TurnStarted { .. }))
        .expect("turn_started");
    let first_delta = events.iter().position(is_live_delta).expect("live deltas");
    assert!(
        started < first_delta,
        "the id is announced before any delta"
    );

    let mut stamped_kinds = Vec::new();
    for event in &events {
        if let Some(event_id) = event.assistant_message_id() {
            assert_eq!(event_id, id, "{event:?} names the committed message");
            stamped_kinds.push(crate::event::agent_event_type(event));
        }
    }
    for kind in [
        "turn_started",
        "reasoning_delta",
        "reasoning_complete",
        "text_delta",
        "server_tool_content",
        "text_complete",
        "assistant_image_appended",
        "turn_completed",
        "run_completed",
    ] {
        assert!(
            stamped_kinds.contains(&kind),
            "{kind} carries the id; stamped: {stamped_kinds:?}"
        );
    }
    for event in events.iter().filter(|event| is_live_delta(event)) {
        assert_eq!(event.assistant_message_id(), Some(id));
    }
}

#[tokio::test]
async fn byte_identical_answers_are_distinct_messages() {
    let (tx, mut rx) = mpsc::channel(1024);
    let client = IdentityClient::new(
        tx.clone(),
        vec![
            // Run one: two tool-loop turns with the same text, then the answer.
            Call::reply(
                vec![Live::Text("same")],
                vec![text_block("same"), tool_use_block("call-1")],
                StopReason::ToolUse,
            ),
            Call::reply(
                vec![Live::Text("same")],
                vec![text_block("same"), tool_use_block("call-2")],
                StopReason::ToolUse,
            ),
            Call::text("same"),
            // Run two: the identical answer again.
            Call::text("same"),
        ],
    );
    let mut agent = builder()
        .build_standalone(
            client.clone(),
            Arc::new(LookupTool {
                append_image: false,
            }),
            Arc::new(NoopStore),
        )
        .await;

    agent
        .run_with_events("first".to_string().into(), tx.clone())
        .await
        .expect("first run");
    let first_events = drain(&mut rx);
    agent
        .run_with_events("second".to_string().into(), tx)
        .await
        .expect("second run");
    let second_events = drain(&mut rx);

    let rows = committed_rows(agent.session());
    assert_eq!(rows.len(), 4);
    assert!(rows.iter().all(|(_, text)| text == "same"));
    let ids = rows
        .iter()
        .map(|(id, _)| id.expect("every committed row has an id"))
        .collect::<Vec<_>>();
    let unique = ids.iter().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        unique.len(),
        4,
        "identical text never shares an id: {ids:?}"
    );

    assert_eq!(
        turn_started_ids(&first_events),
        ids[..3].iter().copied().map(Some).collect::<Vec<_>>()
    );
    assert_eq!(
        turn_started_ids(&first_events),
        turn_completed_ids(&first_events)
    );
    assert_eq!(run_completed_id(&first_events), Some(ids[2]));
    assert_eq!(turn_started_ids(&second_events), vec![Some(ids[3])]);
    assert_eq!(run_completed_id(&second_events), Some(ids[3]));
    assert_eq!(client.attempt_ids(), ids);
}

#[tokio::test]
async fn tool_effect_message_gets_its_own_id_without_turn_started() {
    let (tx, mut rx) = mpsc::channel(1024);
    let client = IdentityClient::new(
        tx.clone(),
        vec![
            Call::reply(
                vec![Live::Text("checking")],
                vec![text_block("checking"), tool_use_block("call-1")],
                StopReason::ToolUse,
            ),
            Call::text("done"),
        ],
    );
    // Session effects are applied against the session build state.
    let mut session = meerkat_core::Session::new();
    session
        .set_build_state(meerkat_core::SessionBuildState::default())
        .expect("build state serializes");
    let mut agent = builder()
        .resume_session(session)
        .build_standalone(
            client.clone(),
            Arc::new(LookupTool { append_image: true }),
            Arc::new(NoopStore),
        )
        .await;

    agent
        .run_with_events("draw".to_string().into(), tx)
        .await
        .expect("run completes");
    let events = drain(&mut rx);

    let rows = committed_rows(agent.session());
    assert_eq!(
        rows.iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>(),
        vec!["checking", "rendered chart", "done"]
    );
    let tool_turn = rows[0].0.expect("tool turn id");
    let effect = rows[1].0.expect("effect message id");
    let answer = rows[2].0.expect("answer id");
    assert!(tool_turn != effect && effect != answer && tool_turn != answer);

    assert_eq!(
        turn_started_ids(&events),
        vec![Some(tool_turn), Some(answer)]
    );
    assert_eq!(
        turn_completed_ids(&events),
        vec![Some(tool_turn), Some(answer)]
    );
    let image_ids = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::AssistantImageAppended {
                assistant_message_id,
                ..
            } => Some(*assistant_message_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        image_ids,
        vec![Some(effect)],
        "the effect image is published under the effect message's id"
    );
    assert_eq!(run_completed_id(&events), Some(answer));
}

#[tokio::test]
async fn retries_reuse_the_id_and_commit_it_once() {
    let (tx, mut rx) = mpsc::channel(1024);
    let client = IdentityClient::new(
        tx.clone(),
        vec![
            // Transient failure after a partial delta.
            Call::failing(vec![Live::Text("partial")], Outcome::Retryable),
            // Empty output is retried too.
            Call::reply(Vec::new(), Vec::new(), StopReason::EndTurn),
            Call::text("complete"),
        ],
    );
    let mut agent = builder()
        .build_standalone(
            client.clone(),
            Arc::new(LookupTool {
                append_image: false,
            }),
            Arc::new(NoopStore),
        )
        .await;

    agent
        .run_with_events("go".to_string().into(), tx)
        .await
        .expect("run completes after retries");
    let events = drain(&mut rx);

    let rows = committed_rows(agent.session());
    assert_eq!(rows.len(), 1, "failed attempts commit nothing");
    let id = rows[0].0.expect("committed id");
    assert_eq!(
        client.attempt_ids(),
        vec![id, id, id],
        "every attempt of the provider turn streams under one id"
    );
    assert_eq!(turn_started_ids(&events), vec![Some(id)]);
    let retry_ids = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Retrying {
                assistant_message_id,
                ..
            } => Some(*assistant_message_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(retry_ids, vec![Some(id), Some(id)]);
    let deltas = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TextDelta {
                delta,
                assistant_message_id,
            } => Some((delta.as_str(), *assistant_message_id)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(deltas, vec![("partial", Some(id)), ("complete", Some(id))]);
    assert_eq!(turn_completed_ids(&events), vec![Some(id)]);
    assert_eq!(run_completed_id(&events), Some(id));
}

#[tokio::test]
async fn failed_provider_turns_never_leave_a_committed_id() {
    let (tx, mut rx) = mpsc::channel(1024);
    let client = IdentityClient::new(
        tx.clone(),
        vec![
            // Run one: a non-retryable failure after a partial delta.
            Call::failing(vec![Live::Text("half")], Outcome::Fatal),
            // Run two: retries exhausted (policy allows two retries).
            Call::failing(vec![Live::Text("a")], Outcome::Retryable),
            Call::failing(vec![Live::Text("b")], Outcome::Retryable),
            Call::failing(vec![Live::Text("c")], Outcome::Retryable),
            // Run three: exhausted max_tokens while thinking.
            Call::reply(
                vec![Live::Reasoning("hmm")],
                vec![AssistantBlock::Reasoning {
                    text: "hmm".to_string(),
                    meta: None,
                }],
                StopReason::MaxTokens,
            ),
            // Run four: a clean answer.
            Call::text("recovered"),
        ],
    );
    let mut agent = builder()
        .build_standalone(
            client.clone(),
            Arc::new(LookupTool {
                append_image: false,
            }),
            Arc::new(NoopStore),
        )
        .await;

    let mut failed_ids = Vec::new();
    for prompt in ["fatal", "exhausted", "thinking"] {
        agent
            .run_with_events(prompt.to_string().into(), tx.clone())
            .await
            .expect_err("the provider turn fails");
        let events = drain(&mut rx);
        let opened = turn_started_ids(&events);
        assert_eq!(opened.len(), 1, "{prompt}: one provider turn opened");
        let opened = opened[0].expect("turn_started carries the id");
        assert!(
            turn_completed_ids(&events).is_empty(),
            "{prompt}: a failed turn is never completed"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::RunFailed { .. })),
            "{prompt}: run_failed closes the open id"
        );
        assert!(
            events
                .iter()
                .filter(|event| is_live_delta(event))
                .all(|event| event.assistant_message_id() == Some(opened)),
            "{prompt}: published deltas carry the uncommitted id"
        );
        failed_ids.push(opened);
    }

    agent
        .run_with_events("again".to_string().into(), tx)
        .await
        .expect("a later run completes");
    let rows = committed_rows(agent.session());
    assert_eq!(rows.len(), 1, "only the clean answer committed");
    let committed = rows[0].0.expect("committed id");
    assert!(
        !failed_ids.contains(&committed),
        "a failed turn's id is never reused: {failed_ids:?} vs {committed}"
    );
    let unique = failed_ids.iter().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), failed_ids.len(), "each run minted its own id");
}

#[tokio::test]
async fn pre_llm_denial_after_turn_started_commits_nothing() {
    let (tx, mut rx) = mpsc::channel(1024);
    let client = IdentityClient::new(tx.clone(), vec![Call::text("allowed")]);
    let hooks = Arc::new(PreLlmDenyingHooks::default());
    hooks.deny.store(true, Ordering::SeqCst);
    let mut agent = builder()
        .with_hook_engine(hooks.clone())
        .build_standalone(
            client.clone(),
            Arc::new(LookupTool {
                append_image: false,
            }),
            Arc::new(NoopStore),
        )
        .await;

    agent
        .run_with_events("blocked".to_string().into(), tx.clone())
        .await
        .expect_err("the pre-LLM hook denies the provider call");
    let denied_events = drain(&mut rx);
    let denied = turn_started_ids(&denied_events);
    assert_eq!(denied.len(), 1);
    let denied = denied[0].expect("id announced before hooks run");
    assert!(client.attempt_ids().is_empty(), "no provider call was made");
    assert!(committed_rows(agent.session()).is_empty());

    hooks.deny.store(false, Ordering::SeqCst);
    agent
        .run_with_events("allowed".to_string().into(), tx)
        .await
        .expect("the next run completes");
    let rows = committed_rows(agent.session());
    assert_eq!(rows.len(), 1);
    assert_ne!(rows[0].0, Some(denied), "the next run mints a fresh id");
}

/// The commit fact is the history row, not `turn_completed`. A run can fail
/// after its terminal row was appended (here a RunCompleted hook denies the
/// result, after the boundary drain): the row stays committed with the id
/// `turn_started` announced, while no `turn_completed` is published for it
/// and `run_failed` ends the run.
#[tokio::test]
async fn a_run_failing_after_the_terminal_push_keeps_the_committed_row_without_turn_completed() {
    let (tx, mut rx) = mpsc::channel(1024);
    let client = IdentityClient::new(tx.clone(), vec![Call::text("final answer")]);
    let mut agent = builder()
        .with_hook_engine(Arc::new(PointDenyingHooks {
            point: HookPoint::RunCompleted,
        }))
        .build_standalone(
            client.clone(),
            Arc::new(LookupTool {
                append_image: false,
            }),
            Arc::new(NoopStore),
        )
        .await;

    agent
        .run_with_events("answer".to_string().into(), tx)
        .await
        .expect_err("the RunCompleted hook denies the result");
    let events = drain(&mut rx);

    let opened = turn_started_ids(&events);
    assert_eq!(opened.len(), 1);
    let opened = opened[0].expect("turn_started carries the id");
    assert!(
        turn_completed_ids(&events).is_empty(),
        "the turn never completed on the live stream"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::RunFailed { .. })),
        "run_failed ends the run"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::TextComplete { assistant_message_id, .. }
            if *assistant_message_id == Some(opened)
    )));
    assert_eq!(
        committed_rows(agent.session()),
        vec![(Some(opened), "final answer".to_string())],
        "the row was committed before the failure and a history read finds it by id"
    );
}

#[tokio::test]
async fn budget_stop_after_streaming_references_the_previous_committed_message() {
    let (tx, mut rx) = mpsc::channel(1024);
    let client = IdentityClient::new(
        tx.clone(),
        vec![
            Call::reply(
                vec![Live::Text("first")],
                vec![text_block("first"), tool_use_block("call-1")],
                StopReason::ToolUse,
            )
            .with_usage(10, 10),
            Call::text("over budget").with_usage(500, 500),
        ],
    );
    let mut agent = builder()
        .budget(crate::budget::BudgetLimits {
            max_tokens: Some(100),
            ..Default::default()
        })
        .build_standalone(
            client.clone(),
            Arc::new(LookupTool {
                append_image: false,
            }),
            Arc::new(NoopStore),
        )
        .await;

    let result = agent
        .run_with_events("spend".to_string().into(), tx)
        .await
        .expect("a budget stop is a successful terminal");
    let events = drain(&mut rx);

    let rows = committed_rows(agent.session());
    assert_eq!(rows.len(), 1, "the over-budget turn is not committed");
    let first = rows[0].0.expect("first row id");
    let opened = turn_started_ids(&events);
    assert_eq!(opened.len(), 2);
    let uncommitted = opened[1].expect("second turn id");
    assert_ne!(uncommitted, first);
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::TextDelta { delta, assistant_message_id }
            if delta == "over budget" && *assistant_message_id == Some(uncommitted)
    )));
    assert_eq!(turn_completed_ids(&events), vec![Some(first)]);
    assert_eq!(result.text, "first");
    assert_eq!(
        run_completed_id(&events),
        Some(first),
        "run_completed names the exact row its result repeats"
    );
}

#[tokio::test]
async fn extraction_turn_has_its_own_id_and_no_turn_started() {
    let (tx, mut rx) = mpsc::channel(1024);
    let client = IdentityClient::new(
        tx.clone(),
        vec![
            Call::text("the answer is forty two"),
            Call::reply(
                vec![Live::Text("{\"value\":42}")],
                vec![text_block("{\"value\":42}")],
                StopReason::EndTurn,
            ),
        ],
    );
    let mut agent = builder()
        .output_schema(
            crate::types::OutputSchema::new(serde_json::json!({
                "type": "object",
                "properties": {"value": {"type": "integer"}},
                "required": ["value"]
            }))
            .unwrap(),
        )
        .build_standalone(
            client.clone(),
            Arc::new(LookupTool {
                append_image: false,
            }),
            Arc::new(NoopStore),
        )
        .await;

    let result = agent
        .run_with_events("extract".to_string().into(), tx)
        .await
        .expect("extraction succeeds");
    assert!(result.structured_output.is_some());
    let events = drain(&mut rx);

    let rows = committed_rows(agent.session());
    assert_eq!(rows.len(), 2);
    let main = rows[0].0.expect("main row id");
    let extraction = rows[1].0.expect("extraction row id");
    assert_ne!(main, extraction);
    assert_eq!(turn_started_ids(&events), vec![Some(main)]);
    assert_eq!(turn_completed_ids(&events), vec![Some(main)]);
    let run_completed = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::RunCompleted {
                assistant_message_id,
                extraction_required,
                ..
            } => Some((*assistant_message_id, *extraction_required)),
            _ => None,
        })
        .expect("run_completed");
    assert_eq!(run_completed, (Some(main), true));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::TextDelta { assistant_message_id, .. }
            if *assistant_message_id == Some(extraction)
    )));
}
