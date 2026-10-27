//! Request and result forwarding only: no provider calls or fallback-policy decisions.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use meerkat_core::config::{ModelFallbackPolicy, ModelFallbackTrigger};
use meerkat_core::model_fallback::{ModelFallbackRequest, ModelFallbackSkipReason};
use meerkat_core::{
    AgentError, AgentLlmClient, AgentLlmFallbackSkippedTarget, AgentLlmFallbackSwitch,
    AssistantBlock, BlockAssistantMessage, ContentBlock, LlmStreamResult, Message, ModelRegistry,
    OutputSchema, Provider, ProviderNativeToolPolicy, ProviderParamsOverride,
    ProviderRequestPressure, RequestAttemptAuthority, ServerToolKind, SessionLlmIdentity,
    SessionLlmRequestPolicy, StopReason, ToolDef, ToolProvenance, ToolResult, ToolSourceKind,
};
use meerkat_meta::memory::{
    ContentTrustConfig, DispatchTaintSlot, SessionTaintTracker, TaintObservingLlmClient,
};
use meerkat_meta::mob_handle_runtime::ReplaySanitizingAgentLlmClient;
use serde_json::{Value, json};

#[derive(Clone, Copy)]
enum Decoration {
    Replay,
    Taint,
    ReplayOverTaint,
    TaintOverReplay,
}

#[derive(Clone, Copy)]
enum PressureReply {
    Measured,
    Unavailable,
    Failed,
}

struct ProbeClient {
    expected_messages: Value,
    tools: Vec<Arc<ToolDef>>,
    params: Option<ProviderParamsOverride>,
    schema: Option<OutputSchema>,
    temperature: Option<f32>,
    fallback_reply: Result<AgentLlmFallbackSwitch, Vec<AgentLlmFallbackSkippedTarget>>,
    pressure_reply: PressureReply,
    admission_calls: AtomicUsize,
    pressure_calls: AtomicUsize,
    stream_calls: AtomicUsize,
}

impl ProbeClient {
    fn assert_request(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        params: Option<&ProviderParamsOverride>,
    ) {
        assert_eq!(json!(messages), self.expected_messages);
        assert_eq!(max_tokens, 513);
        assert_eq!(
            temperature.map(f32::to_bits),
            self.temperature.map(f32::to_bits)
        );
        assert_eq!(params, self.params.as_ref());
        assert_eq!(tools.len(), self.tools.len());
        for (actual, expected) in tools.iter().zip(&self.tools) {
            assert!(
                Arc::ptr_eq(actual, expected),
                "tool identity must be preserved"
            );
        }
    }
}

#[async_trait::async_trait]
impl AgentLlmClient for ProbeClient {
    fn request_attempt_authority(&self) -> RequestAttemptAuthority {
        RequestAttemptAuthority::Unified
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    fn model(&self) -> &'static str {
        "gpt-5.5"
    }

    fn prepare_model_fallback(
        &self,
        failure: &AgentError,
        request: &ModelFallbackRequest<'_>,
    ) -> Result<AgentLlmFallbackSwitch, Vec<AgentLlmFallbackSkippedTarget>> {
        self.assert_request(
            request.messages,
            request.tools,
            request.max_tokens,
            request.temperature,
            request.provider_params,
        );
        assert!(matches!(failure, AgentError::ConfigError(detail) if detail == "admission probe"));
        assert_eq!(request.attempt, 7);
        assert_eq!(json!(request.output_schema), json!(self.schema));
        self.admission_calls.fetch_add(1, Ordering::SeqCst);
        self.fallback_reply.clone()
    }

    fn request_pressure(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        params: Option<&ProviderParamsOverride>,
    ) -> Result<Option<ProviderRequestPressure>, AgentError> {
        self.assert_request(messages, tools, max_tokens, temperature, params);
        self.pressure_calls.fetch_add(1, Ordering::SeqCst);
        match self.pressure_reply {
            PressureReply::Measured => Ok(Some(pressure())),
            PressureReply::Unavailable => Ok(None),
            PressureReply::Failed => Err(AgentError::ConfigError("pressure probe".to_string())),
        }
    }

    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        self.assert_request(messages, tools, max_tokens, temperature, params);
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        Err(AgentError::ConfigError("stream probe".to_string()))
    }
}

fn pressure() -> ProviderRequestPressure {
    ProviderRequestPressure::new(12_345, Some(98_765))
}

fn identity(model: &str) -> SessionLlmIdentity {
    SessionLlmIdentity {
        model: model.to_string(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    }
}

fn messages(include_transient: bool) -> Vec<Message> {
    let mut blocks = vec![AssistantBlock::Text {
        text: "retained text".to_string(),
        meta: None,
    }];
    if include_transient {
        blocks.push(AssistantBlock::ServerToolContent {
            id: Some("transient".to_string()),
            kind: ServerToolKind::WebSearch,
            content: json!({"type": "response.web_search_call.searching", "item_id": "ws_123"}),
            meta: None,
        });
    }
    blocks.push(AssistantBlock::ServerToolContent {
        id: Some("completed".to_string()),
        kind: ServerToolKind::ProviderNative {
            name: "web_search_call".to_string(),
        },
        content: json!({"type": "web_search_call", "id": "ws_123", "status": "completed"}),
        meta: None,
    });
    blocks.push(AssistantBlock::ToolUse {
        id: "call-1".to_string(),
        name: "scrape_page".to_string(),
        args: serde_json::value::RawValue::from_string("{}".to_string()).expect("tool args"),
        meta: None,
    });
    vec![
        Message::BlockAssistant(BlockAssistantMessage::new(blocks, StopReason::ToolUse)),
        Message::tool_results(vec![ToolResult {
            host_metadata: Default::default(),
            tool_use_id: "call-1".to_string(),
            content: vec![ContentBlock::Text {
                text: "untrusted source".to_string(),
            }],
            is_error: false,
            settlement_failures: Vec::new(),
        }]),
    ]
}

fn proposal() -> AgentLlmFallbackSwitch {
    let registry = ModelRegistry::from_config(
        &meerkat_core::Config::default(),
        meerkat_models::canonical(),
    )
    .expect("catalog registry");
    let target_profile = registry
        .profile_witness_for_provider(Provider::OpenAI, "gpt-5.5")
        .expect("catalog target");
    let fact =
        meerkat_core::context_budget_fact_for_messages(&messages(false), &[], 513, &target_profile)
            .expect("sentinel context fact");
    AgentLlmFallbackSwitch {
        policy: ModelFallbackPolicy {
            cross_provider: true,
            min_context_headroom: 0.25,
            require_tool_parity: false,
            trigger_after_attempts: 7,
            triggers: vec![ModelFallbackTrigger::Capacity],
        },
        previous_identity: identity("gpt-5.4"),
        new_identity: identity("gpt-5.5"),
        request_policy: SessionLlmRequestPolicy {
            model: "gpt-5.5".to_string(),
            credential_identity: None,
            provider_params: Some(ProviderParamsOverride {
                max_output_tokens: Some(777),
                ..Default::default()
            }),
            provider_tool_defaults: None,
            provider_native_tools: ProviderNativeToolPolicy::DisableAll,
        },
        target_profile,
        skipped_targets: vec![AgentLlmFallbackSkippedTarget {
            identity: identity("skipped-sentinel"),
            reason: ModelFallbackSkipReason::ContextFit,
            context: Some(fact),
        }],
    }
}

fn assert_reply(
    actual: Result<AgentLlmFallbackSwitch, Vec<AgentLlmFallbackSkippedTarget>>,
    expected: &Result<AgentLlmFallbackSwitch, Vec<AgentLlmFallbackSkippedTarget>>,
) {
    match (actual, expected) {
        (Ok(actual), Ok(expected)) => {
            assert_eq!(actual.policy, expected.policy);
            assert_eq!(actual.previous_identity, expected.previous_identity);
            assert_eq!(actual.new_identity, expected.new_identity);
            assert_eq!(actual.request_policy, expected.request_policy);
            assert_eq!(
                actual.target_profile.provider(),
                expected.target_profile.provider()
            );
            assert_eq!(
                actual.target_profile.model(),
                expected.target_profile.model()
            );
            assert_eq!(
                actual.target_profile.context_window(),
                expected.target_profile.context_window()
            );
            assert_eq!(
                actual.target_profile.max_input_tokens(),
                expected.target_profile.max_input_tokens()
            );
            assert_eq!(
                actual.target_profile.max_output_tokens(),
                expected.target_profile.max_output_tokens()
            );
            assert_eq!(
                json!(actual.target_profile.profile()),
                json!(expected.target_profile.profile())
            );
            assert_eq!(
                json!(actual.skipped_targets),
                json!(expected.skipped_targets)
            );
        }
        (Err(actual), Err(expected)) => assert_eq!(json!(actual), json!(expected)),
        (actual, expected) => panic!("changed admission result: {actual:?}, expected {expected:?}"),
    }
}

async fn check_decoration(decoration: Decoration) {
    let source = messages(true);
    let original = json!(source);
    let mut expected = source.clone();
    if !matches!(decoration, Decoration::Taint) {
        let Message::BlockAssistant(assistant) = &mut expected[0] else {
            panic!("fixture must begin with the assistant replay");
        };
        assistant.blocks.remove(1);
    }
    let expected_messages = json!(expected);
    let tools = vec![Arc::new(ToolDef {
        audience: Default::default(),
        name: "scrape_page".into(),
        description: "request witness".to_string(),
        input_schema: json!({"type": "object", "properties": {"url": {"type": "string"}}}),
        provenance: Some(ToolProvenance {
            kind: ToolSourceKind::Mcp,
            source_id: "scraper".into(),
            artifact: None,
        }),
    })];
    let proposal = proposal();
    for populated in [true, false] {
        for reply in [
            Ok(proposal.clone()),
            Err(proposal.skipped_targets.clone()),
            Err(Vec::new()),
        ] {
            for pressure_reply in [
                PressureReply::Measured,
                PressureReply::Unavailable,
                PressureReply::Failed,
            ] {
                let params = populated.then(|| ProviderParamsOverride {
                    max_output_tokens: Some(123),
                    top_p: Some(0.75),
                    ..Default::default()
                });
                let schema = populated.then(|| {
                    OutputSchema::new(json!({
                        "type": "object", "properties": {"sentinel": {"type": "boolean"}}
                    }))
                    .expect("request schema")
                });
                let temperature = populated.then_some(0.25);
                let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
                let slot = DispatchTaintSlot::default();
                slot.fill(tracker.clone());
                let inner = Arc::new(ProbeClient {
                    expected_messages: expected_messages.clone(),
                    tools: tools.clone(),
                    params: params.clone(),
                    schema: schema.clone(),
                    temperature,
                    fallback_reply: reply.clone(),
                    pressure_reply,
                    admission_calls: AtomicUsize::new(0),
                    pressure_calls: AtomicUsize::new(0),
                    stream_calls: AtomicUsize::new(0),
                });
                let taint = |client: Arc<dyn AgentLlmClient>| -> Arc<dyn AgentLlmClient> {
                    Arc::new(TaintObservingLlmClient::new(
                        client,
                        "identity:probe".to_string(),
                        slot.clone(),
                    ))
                };
                let wrapped: Arc<dyn AgentLlmClient> = match decoration {
                    Decoration::Replay => ReplaySanitizingAgentLlmClient::wrap(inner.clone()),
                    Decoration::Taint => taint(inner.clone()),
                    Decoration::ReplayOverTaint => {
                        ReplaySanitizingAgentLlmClient::wrap(taint(inner.clone()))
                    }
                    Decoration::TaintOverReplay => {
                        taint(ReplaySanitizingAgentLlmClient::wrap(inner.clone()))
                    }
                };
                assert_eq!(
                    wrapped.request_attempt_authority(),
                    RequestAttemptAuthority::Unified
                );
                let request = ModelFallbackRequest {
                    messages: &source,
                    tools: &tools,
                    max_tokens: 513,
                    temperature,
                    provider_params: params.as_ref(),
                    output_schema: schema.as_ref(),
                    attempt: 7,
                };
                assert_reply(
                    wrapped.prepare_model_fallback(
                        &AgentError::ConfigError("admission probe".to_string()),
                        &request,
                    ),
                    &reply,
                );
                let measured =
                    wrapped.request_pressure(&source, &tools, 513, temperature, params.as_ref());
                match pressure_reply {
                    PressureReply::Measured => {
                        assert_eq!(measured.expect("pressure forwarded"), Some(pressure()));
                    }
                    PressureReply::Unavailable => {
                        assert_eq!(measured.expect("unavailable forwarded"), None);
                    }
                    PressureReply::Failed => assert!(matches!(
                        measured, Err(AgentError::ConfigError(detail)) if detail == "pressure probe"
                    )),
                }
                assert_eq!(inner.admission_calls.load(Ordering::SeqCst), 1);
                assert_eq!(inner.pressure_calls.load(Ordering::SeqCst), 1);
                assert_eq!(inner.stream_calls.load(Ordering::SeqCst), 0);
                assert!(
                    tracker.identity_taint("identity:probe").is_none(),
                    "probes must not mark ingestion"
                );
                let result = wrapped
                    .stream_response(&source, &tools, 513, temperature, params.as_ref())
                    .await;
                assert!(
                    matches!(result, Err(AgentError::ConfigError(detail)) if detail == "stream probe")
                );
                assert_eq!(inner.stream_calls.load(Ordering::SeqCst), 1);
                assert_eq!(
                    tracker.identity_taint("identity:probe").is_some(),
                    !matches!(decoration, Decoration::Replay),
                    "taint tracking must still run at dispatch"
                );
                assert_eq!(json!(source), original, "source transcript must not change");
            }
        }
    }
}

#[tokio::test]
async fn replay_fallback_request_and_pressure_contract() {
    check_decoration(Decoration::Replay).await;
}

#[tokio::test]
async fn taint_fallback_request_and_pressure_contract() {
    check_decoration(Decoration::Taint).await;
}

#[tokio::test]
async fn replay_over_taint_fallback_request_and_pressure_contract() {
    check_decoration(Decoration::ReplayOverTaint).await;
}

#[tokio::test]
async fn taint_over_replay_fallback_request_and_pressure_contract() {
    check_decoration(Decoration::TaintOverReplay).await;
}

#[test]
fn fallback_events_and_resume_hold_keep_typed_console_payloads() {
    use meerkat_core::AgentEvent;
    use meerkat_core::event::{AgentErrorClass, AgentErrorReason, AgentErrorReport};
    use meerkat_core::retry::{
        LlmRetryFailure, LlmRetryFailureKind, LlmRetryPlan, LlmRetrySchedule,
    };
    use meerkat_meta::mob_handle_runtime::console_agent_event_payload;

    let retry = LlmRetrySchedule {
        failure: LlmRetryFailure {
            provider: "openai".to_string(),
            kind: LlmRetryFailureKind::RateLimited,
            retry_after_ms: Some(100),
            duration_ms: None,
            message: "capacity".to_string(),
        },
        plan: LlmRetryPlan {
            attempt: 3,
            max_retries: 5,
            computed_delay_ms: 100,
            selected_delay_ms: 100,
            retry_after_hint_ms: Some(100),
            rate_limit_floor_applied: false,
            budget_capped: false,
        },
    };
    let switch = proposal();
    let report = AgentErrorReport {
        class: AgentErrorClass::Config,
        reason: Some(AgentErrorReason::ModelFallbackResumeHeld {
            provider: Provider::OpenAI,
            model: "gpt-5.5".to_string(),
            reason: ModelFallbackSkipReason::ContextFit,
        }),
        message: "held by upstream admission".to_string(),
    };
    for event in [
        AgentEvent::ModelFallbackSkipped {
            retry: retry.clone(),
            target: switch.skipped_targets[0].clone(),
        },
        AgentEvent::ModelFallbackStaged {
            retry: retry.clone(),
            previous: switch.previous_identity.clone(),
            target: switch.new_identity.clone(),
        },
        AgentEvent::ModelFallbackCommitted {
            retry,
            previous: switch.previous_identity.clone(),
            target: switch.new_identity.clone(),
        },
        AgentEvent::ModelFallbackTargetFailed {
            previous: switch.previous_identity,
            target: switch.new_identity,
            error: report.clone(),
        },
    ] {
        assert_eq!(console_agent_event_payload(&event), json!(event));
    }
    let event = AgentEvent::RunFailed {
        identity: Default::default(),
        session_id: meerkat_core::SessionId::new(),
        error_report: report.clone(),
        terminal_cause_kind: None,
    };
    let payload = console_agent_event_payload(&event);
    assert_eq!(payload["error_report"], json!(report));
    assert_eq!(payload["reason"], "model_fallback_resume_held");
    assert_eq!(payload["error_report"]["reason"]["reason"], "context_fit");
}
