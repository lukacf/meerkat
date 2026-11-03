#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use meerkat_core::{
    AgentError, AgentLlmClient, AgentLlmRequestAttempt, AssistantBlock, AssistantMessageId,
    BlockAssistantMessage, LlmStreamResult, Message, Provider, ProviderParamsOverride,
    ProviderRequestPressure, RequestAttemptAuthority, ServerToolKind, StopReason, ToolDef,
    ToolProvenance, ToolResult, ToolSourceKind, Usage,
};

use super::ReplaySanitizingAgentLlmClient;
use crate::memory::dispatch_taint::{DispatchTaintSlot, TaintObservingLlmClient};
use crate::memory::taint::{ContentTrustConfig, SessionTaintTracker};

const IDENTITY: &str = "prepared-attempt-agent";

struct PreparedArgs {
    messages: Arc<Vec<Message>>,
    tools: Arc<[Arc<ToolDef>]>,
    max_tokens: u32,
    temperature: Option<f32>,
    provider_params: Option<ProviderParamsOverride>,
}

struct ProbeAttempt {
    tracker: SessionTaintTracker,
    dispatches: Mutex<Vec<(AssistantMessageId, bool)>>,
    pressure: Mutex<VecDeque<Result<Option<ProviderRequestPressure>, &'static str>>>,
    fail_dispatch: AtomicBool,
    blocks: Vec<AssistantBlock>,
}

#[async_trait::async_trait]
impl AgentLlmRequestAttempt for ProbeAttempt {
    fn request_pressure(&self) -> Result<Option<ProviderRequestPressure>, AgentError> {
        self.pressure
            .lock()
            .unwrap()
            .pop_front()
            .expect("each pressure observation must reach the prepared attempt")
            .map_err(|message| AgentError::InternalError(message.to_string()))
    }

    async fn stream_response(
        &self,
        assistant_message_id: AssistantMessageId,
    ) -> Result<LlmStreamResult, AgentError> {
        self.dispatches.lock().unwrap().push((
            assistant_message_id,
            self.tracker.identity_taint(IDENTITY).is_some(),
        ));
        if self.fail_dispatch.load(Ordering::SeqCst) {
            return Err(AgentError::InternalError("prepared dispatch failed".into()));
        }
        Ok(LlmStreamResult::new(
            self.blocks.clone(),
            StopReason::EndTurn,
            Usage::default(),
        ))
    }
}

struct ProbeClient {
    prepared: Mutex<Vec<PreparedArgs>>,
    fail_prepare: AtomicBool,
    attempt: Arc<ProbeAttempt>,
}

#[async_trait::async_trait]
impl AgentLlmClient for ProbeClient {
    fn prepare_request_attempt(
        self: Arc<Self>,
        messages: Arc<Vec<Message>>,
        tools: Arc<[Arc<ToolDef>]>,
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<ProviderParamsOverride>,
    ) -> Result<Arc<dyn AgentLlmRequestAttempt>, AgentError> {
        self.prepared.lock().unwrap().push(PreparedArgs {
            messages,
            tools,
            max_tokens,
            temperature,
            provider_params,
        });
        if self.fail_prepare.load(Ordering::SeqCst) {
            return Err(AgentError::InternalError("prepared route refused".into()));
        }
        Ok(self.attempt.clone())
    }

    fn request_attempt_authority(&self) -> RequestAttemptAuthority {
        RequestAttemptAuthority::Unified
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    fn model(&self) -> &'static str {
        "prepared-probe"
    }

    async fn stream_response(
        &self,
        _messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        panic!("a prepared request must not fall back to the identity-free client stream")
    }

    fn request_pressure(
        &self,
        _messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<Option<ProviderRequestPressure>, AgentError> {
        panic!("pressure must use the bound prepared attempt, not another client route")
    }
}

fn probe(blocks: Vec<AssistantBlock>) -> Arc<ProbeClient> {
    Arc::new(ProbeClient {
        prepared: Mutex::new(Vec::new()),
        fail_prepare: AtomicBool::new(false),
        attempt: Arc::new(ProbeAttempt {
            tracker: SessionTaintTracker::new(ContentTrustConfig::default()),
            dispatches: Mutex::new(Vec::new()),
            pressure: Mutex::new(VecDeque::from([
                Ok(Some(ProviderRequestPressure::new(111, Some(4096)))),
                Ok(Some(ProviderRequestPressure::new(222, Some(8192)))),
                Ok(None),
                Err("prepared pressure failed"),
            ])),
            fail_dispatch: AtomicBool::new(false),
            blocks,
        }),
    })
}

fn sentinel() -> AssistantMessageId {
    serde_json::from_value(serde_json::json!("a02b58e1-bc62-4bea-84d1-356fca9f7d02"))
        .expect("serialized core-owned identity fixture")
}

fn unsafe_server_block() -> AssistantBlock {
    AssistantBlock::ServerToolContent {
        id: Some("provider-progress".into()),
        kind: ServerToolKind::WebSearch,
        content: serde_json::json!({"type": "response.web_search_call.searching"}),
        meta: None,
    }
}

fn untrusted_request() -> (Arc<Vec<Message>>, Arc<[Arc<ToolDef>]>) {
    let messages = vec![
        Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::ToolUse {
                id: "scrape-call".into(),
                name: "scrape_page".into(),
                args: serde_json::value::RawValue::from_string("{}".into()).unwrap(),
                meta: None,
            }],
            StopReason::ToolUse,
        )),
        Message::tool_results(vec![ToolResult::new(
            "scrape-call".into(),
            "untrusted result".into(),
            false,
        )]),
    ];
    let tools = vec![Arc::new(ToolDef {
        audience: Default::default(),
        name: "scrape_page".into(),
        description: "Fetch page".into(),
        input_schema: serde_json::json!({"type": "object"}),
        provenance: Some(ToolProvenance {
            kind: ToolSourceKind::Mcp,
            source_id: "scraper".into(),
            artifact: None,
        }),
    })];
    (Arc::new(messages), tools.into())
}

fn wrapped(kind: &str, inner: Arc<ProbeClient>) -> Arc<dyn AgentLlmClient> {
    match kind {
        "replay" => ReplaySanitizingAgentLlmClient::wrap(inner),
        "taint" => Arc::new(TaintObservingLlmClient::new(
            inner,
            IDENTITY.into(),
            DispatchTaintSlot::default(),
        )),
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn prepared_attempt_replay_preserves_inner_attempt_identity_and_sanitized_owned_request() {
    let inner = probe(vec![]);
    let mut history = BlockAssistantMessage::new(
        vec![
            AssistantBlock::Text {
                text: "visible".into(),
                meta: None,
            },
            unsafe_server_block(),
            AssistantBlock::ServerToolContent {
                id: Some("provider-completed".into()),
                kind: ServerToolKind::ProviderNative {
                    name: "web_search_call".into(),
                },
                content: serde_json::json!({"type": "web_search_call", "status": "completed"}),
                meta: None,
            },
        ],
        StopReason::EndTurn,
    );
    history.assistant_message_id = Some(sentinel());
    let original = Arc::new(vec![Message::BlockAssistant(history)]);
    let original_bytes = serde_json::to_value(&original).unwrap();
    let (_, tools) = untrusted_request();
    let params = ProviderParamsOverride {
        top_p: Some(0.75),
        ..Default::default()
    };
    let attempt = wrapped("replay", inner.clone())
        .prepare_request_attempt(
            original.clone(),
            tools.clone(),
            731,
            Some(0.25),
            Some(params.clone()),
        )
        .unwrap();
    let expected_attempt: Arc<dyn AgentLlmRequestAttempt> = inner.attempt.clone();
    assert!(
        Arc::ptr_eq(&attempt, &expected_attempt),
        "replay returns the exact inner route witness"
    );
    {
        let prepared = inner.prepared.lock().unwrap();
        assert_eq!(prepared.len(), 1);
        let seen = &prepared[0];
        assert!(!Arc::ptr_eq(&seen.messages, &original));
        assert!(Arc::ptr_eq(&seen.tools, &tools));
        assert_eq!((seen.max_tokens, seen.temperature), (731, Some(0.25)));
        assert_eq!(seen.provider_params, Some(params));
        let Message::BlockAssistant(assistant) = &seen.messages[0] else {
            panic!("assistant row");
        };
        assert_eq!(assistant.assistant_message_id, Some(sentinel()));
        assert_eq!(assistant.blocks.len(), 2);
        assert!(
            matches!(&assistant.blocks[0], AssistantBlock::Text { text, .. } if text == "visible")
        );
        assert!(
            matches!(&assistant.blocks[1], AssistantBlock::ServerToolContent { id, .. } if id.as_deref() == Some("provider-completed"))
        );
    }
    assert_eq!(
        serde_json::to_value(&original).unwrap(),
        original_bytes,
        "canonical input is not mutated"
    );
    attempt.stream_response(sentinel()).await.unwrap();
    assert_eq!(
        *inner.attempt.dispatches.lock().unwrap(),
        [(sentinel(), false)]
    );
}

#[tokio::test]
async fn prepared_attempt_taint_marks_at_dispatch_with_late_slot_and_preserves_request() {
    let inner = probe(vec![]);
    let slot = DispatchTaintSlot::default();
    let client = Arc::new(TaintObservingLlmClient::new(
        inner.clone(),
        IDENTITY.into(),
        slot.clone(),
    ));
    let (messages, tools) = untrusted_request();
    let params = ProviderParamsOverride {
        max_output_tokens: Some(512),
        ..Default::default()
    };
    let attempt = client
        .prepare_request_attempt(
            messages.clone(),
            tools.clone(),
            512,
            Some(0.5),
            Some(params.clone()),
        )
        .unwrap();
    slot.fill(inner.attempt.tracker.clone());
    assert!(
        inner.attempt.tracker.identity_taint(IDENTITY).is_none(),
        "preparation does not ingest"
    );
    assert_eq!(
        attempt.request_pressure().unwrap(),
        Some(ProviderRequestPressure::new(111, Some(4096)))
    );
    assert!(
        inner.attempt.tracker.identity_taint(IDENTITY).is_none(),
        "pressure does not ingest"
    );
    {
        let prepared = inner.prepared.lock().unwrap();
        assert_eq!(prepared.len(), 1);
        assert!(Arc::ptr_eq(&prepared[0].messages, &messages));
        assert!(Arc::ptr_eq(&prepared[0].tools, &tools));
        assert_eq!(
            (prepared[0].max_tokens, prepared[0].temperature),
            (512, Some(0.5))
        );
        assert_eq!(prepared[0].provider_params, Some(params));
    }
    attempt.stream_response(sentinel()).await.unwrap();
    assert_eq!(
        *inner.attempt.dispatches.lock().unwrap(),
        [(sentinel(), true)],
        "mark precedes actual inner dispatch"
    );
    assert!(
        inner
            .attempt
            .tracker
            .identity_taint(IDENTITY)
            .unwrap()
            .source
            .contains("scraper")
    );
}

#[test]
fn prepared_attempt_wrappers_forward_dynamic_pressure_and_errors_without_repreparing() {
    for kind in ["replay", "taint"] {
        let inner = probe(vec![]);
        let attempt = wrapped(kind, inner.clone())
            .prepare_request_attempt(Arc::new(vec![]), Arc::new([]), 32, None, None)
            .unwrap();
        assert_eq!(
            attempt.request_pressure().unwrap(),
            Some(ProviderRequestPressure::new(111, Some(4096)))
        );
        assert_eq!(
            attempt.request_pressure().unwrap(),
            Some(ProviderRequestPressure::new(222, Some(8192)))
        );
        assert_eq!(attempt.request_pressure().unwrap(), None);
        assert!(
            matches!(attempt.request_pressure(), Err(AgentError::InternalError(message)) if message == "prepared pressure failed")
        );
        assert_eq!(inner.prepared.lock().unwrap().len(), 1);
        assert!(inner.attempt.dispatches.lock().unwrap().is_empty());
    }
}

#[test]
fn prepared_attempt_wrappers_propagate_preparation_failure_without_taint_or_dispatch() {
    for kind in ["replay", "taint"] {
        let inner = probe(vec![]);
        inner.fail_prepare.store(true, Ordering::SeqCst);
        let (messages, tools) = untrusted_request();
        let client: Arc<dyn AgentLlmClient> = if kind == "taint" {
            let slot = DispatchTaintSlot::default();
            slot.fill(inner.attempt.tracker.clone());
            Arc::new(TaintObservingLlmClient::new(
                inner.clone(),
                IDENTITY.into(),
                slot,
            ))
        } else {
            wrapped(kind, inner.clone())
        };
        assert!(
            matches!(client.prepare_request_attempt(messages, tools, 32, None, None),
            Err(AgentError::InternalError(message)) if message == "prepared route refused")
        );
        assert_eq!(inner.prepared.lock().unwrap().len(), 1);
        assert!(inner.attempt.tracker.identity_taint(IDENTITY).is_none());
        assert!(inner.attempt.dispatches.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn prepared_attempt_taint_marks_successful_server_result_before_returning() {
    for fail in [true, false] {
        let inner = probe(vec![unsafe_server_block()]);
        inner.attempt.fail_dispatch.store(fail, Ordering::SeqCst);
        let slot = DispatchTaintSlot::default();
        slot.fill(inner.attempt.tracker.clone());
        let client = Arc::new(TaintObservingLlmClient::new(
            inner.clone(),
            IDENTITY.into(),
            slot,
        ));
        let attempt = client
            .prepare_request_attempt(Arc::new(vec![]), Arc::new([]), 32, None, None)
            .unwrap();
        let result = attempt.stream_response(sentinel()).await;
        assert_eq!(
            *inner.attempt.dispatches.lock().unwrap(),
            [(sentinel(), false)]
        );
        if fail {
            assert!(
                matches!(result, Err(AgentError::InternalError(message)) if message == "prepared dispatch failed")
            );
            assert!(inner.attempt.tracker.identity_taint(IDENTITY).is_none());
        } else {
            assert_eq!(
                serde_json::to_value(result.unwrap().blocks()).unwrap(),
                serde_json::to_value(&inner.attempt.blocks).unwrap()
            );
            assert!(
                inner
                    .attempt
                    .tracker
                    .identity_taint(IDENTITY)
                    .unwrap()
                    .source
                    .contains("web_search")
            );
        }
    }
}

#[tokio::test]
async fn prepared_attempt_taint_retains_ingestion_on_failed_dispatch() {
    let inner = probe(vec![]);
    inner.attempt.fail_dispatch.store(true, Ordering::SeqCst);
    let slot = DispatchTaintSlot::default();
    slot.fill(inner.attempt.tracker.clone());
    let client = Arc::new(TaintObservingLlmClient::new(
        inner.clone(),
        IDENTITY.into(),
        slot,
    ));
    let (messages, tools) = untrusted_request();
    let attempt = client
        .prepare_request_attempt(messages, tools, 32, None, None)
        .unwrap();
    assert!(matches!(attempt.stream_response(sentinel()).await,
        Err(AgentError::InternalError(message)) if message == "prepared dispatch failed"));
    assert_eq!(
        *inner.attempt.dispatches.lock().unwrap(),
        [(sentinel(), true)]
    );
    assert!(inner.attempt.tracker.identity_taint(IDENTITY).is_some());
}

/// Raw provider events have no assistant identity. The real factory adapter
/// must receive the core-minted id through each installed decorator and stamp
/// the live event before the real session commits the same id to history.
struct IdentityChunks {
    requests: Mutex<Vec<Vec<Message>>>,
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for IdentityChunks {
    fn project_replay_messages(
        &self,
        messages: &[Message],
    ) -> Result<Vec<Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        self.requests.lock().unwrap().push(request.messages.clone());
        let mut events = vec![
            meerkat_client::LlmEvent::TextDelta {
                delta: "decorated ".into(),
                meta: None,
            },
            meerkat_client::LlmEvent::TextDelta {
                delta: "reply".into(),
                meta: None,
            },
        ];
        events.extend(super::test_llm_usage::usage_then_done(
            request,
            Provider::OpenAI,
            StopReason::EndTurn,
        ));
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

#[tokio::test]
async fn prepared_attempt_decorated_real_session_joins_provider_deltas_to_committed_identity() {
    use std::time::Duration;

    use meerkat_core::service::{
        CreateSessionRequest, InitialTurnPolicy, SessionBuildOptions, SessionHistoryQuery,
        SessionService, SessionServiceHistoryExt,
    };

    for kind in ["replay", "taint", "taint-over-replay", "replay-over-taint"] {
        let temp = tempfile::tempdir().unwrap();
        let raw = Arc::new(IdentityChunks {
            requests: Mutex::new(Vec::new()),
        });
        let mut builder = meerkat::FactoryAgentBuilder::new(
            meerkat::AgentFactory::new(temp.path()).builtins(false),
            meerkat::Config::default(),
        );
        builder.default_llm_client = Some(raw.clone());
        let service = meerkat_session::EphemeralSessionService::new(builder, 4);
        let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(128);
        let slot = DispatchTaintSlot::default();
        slot.fill(SessionTaintTracker::new(ContentTrustConfig::default()));
        let request = CreateSessionRequest {
            model: "gpt-5.5".into(),
            prompt: meerkat_core::ContentInput::Text("Check the decorated request path.".into()),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: Some(64),
            event_tx: Some(events_tx),
            initial_turn: InitialTurnPolicy::RunImmediately,
            deferred_prompt_policy: Default::default(),
            build: Some(SessionBuildOptions {
                agent_llm_client_decorator: Some(Arc::new(move |client| {
                    let taint = |inner| -> Arc<dyn AgentLlmClient> {
                        Arc::new(TaintObservingLlmClient::new(
                            inner,
                            IDENTITY.into(),
                            slot.clone(),
                        ))
                    };
                    match kind {
                        "replay" => ReplaySanitizingAgentLlmClient::wrap(client),
                        "taint" => taint(client),
                        "taint-over-replay" => taint(ReplaySanitizingAgentLlmClient::wrap(client)),
                        "replay-over-taint" => ReplaySanitizingAgentLlmClient::wrap(taint(client)),
                        _ => unreachable!(),
                    }
                })),
                ..Default::default()
            }),
            labels: None,
        };
        let completed =
            tokio::time::timeout(Duration::from_secs(30), service.create_session(request))
                .await
                .expect("decorated real session completes")
                .expect("real provider turn commits");
        let events = tokio::time::timeout(Duration::from_secs(5), async {
            let mut events = Vec::new();
            while let Some(envelope) = events_rx.recv().await {
                let done = matches!(
                    &envelope.payload,
                    meerkat_core::AgentEvent::RunCompleted { .. }
                );
                events.push(envelope.payload);
                if done {
                    break;
                }
            }
            events
        })
        .await
        .expect("actual session events arrive");
        let history = service
            .read_history(
                &completed.session_id,
                SessionHistoryQuery {
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("committed real session history");
        service.shutdown().await;
        assert_eq!(history.session_id, completed.session_id);
        assert!(!history.has_more);
        assert_eq!(
            raw.requests.lock().unwrap().len(),
            1,
            "one actual provider request for {kind}"
        );
        let assistants = history
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::BlockAssistant(assistant) => Some(assistant),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            assistants.len(),
            1,
            "one canonical provider reply for {kind}"
        );
        let committed_id = assistants[0]
            .assistant_message_id
            .expect("core commits an occurrence identity");
        assert_eq!(
            assistants[0].text_blocks().collect::<String>(),
            "decorated reply"
        );
        let deltas = events
            .iter()
            .filter_map(|event| match event {
                meerkat_core::AgentEvent::TextDelta {
                    delta,
                    assistant_message_id,
                } => Some((delta, assistant_message_id)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            !deltas.is_empty(),
            "the real adapter emits live provider text for {kind}"
        );
        assert_eq!(
            deltas
                .iter()
                .map(|(delta, _)| delta.as_str())
                .collect::<String>(),
            "decorated reply"
        );
        for (_, live_id) in deltas {
            assert_eq!(
                *live_id,
                Some(committed_id),
                "{kind} forwards the core id through the real prepared adapter"
            );
        }
    }
}

/// Records, at dispatch, whether the member's taint was already marked: a
/// decorator that taint wraps sees `true`, one that wraps taint sees `false`.
struct OrderWitness {
    inner: Arc<dyn AgentLlmClient>,
    tracker: SessionTaintTracker,
    marked_at_dispatch: Arc<Mutex<Vec<bool>>>,
}

struct OrderWitnessAttempt {
    inner: Arc<dyn AgentLlmRequestAttempt>,
    tracker: SessionTaintTracker,
    marked_at_dispatch: Arc<Mutex<Vec<bool>>>,
}

#[async_trait::async_trait]
impl AgentLlmRequestAttempt for OrderWitnessAttempt {
    fn request_pressure(&self) -> Result<Option<ProviderRequestPressure>, AgentError> {
        self.inner.request_pressure()
    }

    async fn stream_response(
        &self,
        assistant_message_id: AssistantMessageId,
    ) -> Result<LlmStreamResult, AgentError> {
        self.marked_at_dispatch
            .lock()
            .unwrap()
            .push(self.tracker.identity_taint(IDENTITY).is_some());
        self.inner.stream_response(assistant_message_id).await
    }
}

#[async_trait::async_trait]
impl AgentLlmClient for OrderWitness {
    fn prepare_request_attempt(
        self: Arc<Self>,
        messages: Arc<Vec<Message>>,
        tools: Arc<[Arc<ToolDef>]>,
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<ProviderParamsOverride>,
    ) -> Result<Arc<dyn AgentLlmRequestAttempt>, AgentError> {
        let inner = self.inner.clone().prepare_request_attempt(
            messages,
            tools,
            max_tokens,
            temperature,
            provider_params,
        )?;
        Ok(Arc::new(OrderWitnessAttempt {
            inner,
            tracker: self.tracker.clone(),
            marked_at_dispatch: self.marked_at_dispatch.clone(),
        }))
    }

    fn request_attempt_authority(&self) -> RequestAttemptAuthority {
        self.inner.request_attempt_authority()
    }

    fn provider(&self) -> Provider {
        self.inner.provider()
    }

    fn model(&self) -> &str {
        self.inner.model()
    }

    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        self.inner
            .stream_response(messages, tools, max_tokens, temperature, provider_params)
            .await
    }
}

fn witness_decorator(
    calls: Arc<std::sync::atomic::AtomicUsize>,
    tracker: SessionTaintTracker,
    marked_at_dispatch: Arc<Mutex<Vec<bool>>>,
) -> meerkat_core::AgentLlmClientDecorator {
    Arc::new(move |client| {
        calls.fetch_add(1, Ordering::SeqCst);
        Arc::new(OrderWitness {
            inner: client,
            tracker: tracker.clone(),
            marked_at_dispatch: marked_at_dispatch.clone(),
        })
    })
}

fn member_request(
    decorator: Option<meerkat_core::AgentLlmClientDecorator>,
) -> meerkat_core::service::CreateSessionRequest {
    meerkat_core::service::CreateSessionRequest {
        model: "gpt-5.5".into(),
        prompt: meerkat_core::ContentInput::Text("Build the member.".into()),
        injected_context: Vec::new(),
        system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
        max_tokens: Some(64),
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        deferred_prompt_policy: Default::default(),
        build: Some(meerkat_core::service::SessionBuildOptions {
            mob_member_binding: Some(meerkat_core::MobMemberBinding {
                mob_id: "mob-1".to_string(),
                role: "worker".to_string(),
                member: IDENTITY.to_string(),
            }),
            agent_llm_client_decorator: decorator,
            ..Default::default()
        }),
        labels: None,
    }
}

/// ADR 13:34Z: member taint wraps the EFFECTIVE host decorator, which runs
/// once - the request's own decorator when present, otherwise the session
/// builder's actual default (the cell its factory reads). Without the
/// composition the taint decorator took the request's empty slot and the
/// factory skipped its default entirely.
#[tokio::test]
async fn member_taint_wraps_the_effective_host_decorator_once() {
    use meerkat_core::service::SessionService;
    use std::sync::atomic::AtomicUsize;

    for explicit in [false, true] {
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        let marked = Arc::new(Mutex::new(Vec::new()));
        let default_calls = Arc::new(AtomicUsize::new(0));
        let explicit_calls = Arc::new(AtomicUsize::new(0));
        let temp = tempfile::tempdir().unwrap();
        let mut builder = meerkat::FactoryAgentBuilder::new(
            meerkat::AgentFactory::new(temp.path()).builtins(false),
            meerkat::Config::default(),
        );
        builder.default_llm_client = Some(Arc::new(IdentityChunks {
            requests: Mutex::new(Vec::new()),
        }));
        *builder.default_agent_llm_client_decorator.write().unwrap() = Some(witness_decorator(
            default_calls.clone(),
            tracker.clone(),
            marked.clone(),
        ));
        let slot = DispatchTaintSlot::default();
        slot.fill(tracker.clone());
        slot.bind_host_default_decorator(Arc::clone(&builder.default_agent_llm_client_decorator));
        let explicit_decorator = || {
            explicit
                .then(|| witness_decorator(explicit_calls.clone(), tracker.clone(), marked.clone()))
        };

        // Order: taint is outside the effective decorator, so that decorator
        // sees the member already marked when the request dispatches.
        let mut request = member_request(explicit_decorator());
        crate::memory::dispatch_taint::attach_member_taint_decorator(&mut request, &slot);
        let composed = request
            .build
            .as_ref()
            .and_then(|build| build.agent_llm_client_decorator.clone())
            .expect("the member decorator");
        let (messages, tools) = untrusted_request();
        composed(probe(vec![]))
            .prepare_request_attempt(messages, tools, 32, None, None)
            .unwrap()
            .stream_response(sentinel())
            .await
            .unwrap();
        assert_eq!(
            *marked.lock().unwrap(),
            [true],
            "taint wraps the effective decorator (explicit: {explicit})"
        );

        // Once: a real member build applies exactly the effective decorator.
        default_calls.store(0, Ordering::SeqCst);
        explicit_calls.store(0, Ordering::SeqCst);
        let service = meerkat_session::EphemeralSessionService::new(builder, 4);
        let mut request = member_request(explicit_decorator());
        crate::memory::dispatch_taint::attach_member_taint_decorator(&mut request, &slot);
        service
            .create_session(request)
            .await
            .expect("the member builds");
        service.shutdown().await;
        let (want_default, want_explicit) = if explicit { (0, 1) } else { (1, 0) };
        assert_eq!(
            default_calls.load(Ordering::SeqCst),
            want_default,
            "builder default runs (explicit: {explicit})"
        );
        assert_eq!(
            explicit_calls.load(Ordering::SeqCst),
            want_explicit,
            "request decorator runs (explicit: {explicit})"
        );
    }
}
