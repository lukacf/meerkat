//! The provider-neutral `reasoning_effort` level through the real agent
//! loop: every request that carries it is resolved against the route of the
//! client that serves it right before it is sent, a level the route does not
//! take is refused with the typed error before any client call, and the run
//! reports what it sent.
//!
//! The client here is a plain `AgentLlmClient` that reports a route, records
//! every request it is handed with the params it carries, and answers from a
//! script. The route is a self-hosted Chat Completions route whose levels
//! the operator declares, so no catalog row is involved.
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::too_many_lines
)]

use crate as meerkat_core;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_core::compact::{
    COMPACTION_SUMMARY_PREFIX, CompactionContext, CompactionDiscard, CompactionResult,
    CompactionRetained, CompactionSummary, Compactor, SummarizationSource,
    SummarizationSourceBudget,
};
use meerkat_core::lifecycle::run_primitive::{
    OpenAiProviderTag, ProviderParamsOverride, ProviderTag, ReasoningBatchDisposition,
    ReasoningEffort, ReasoningLoweringBaseline, ReasoningLoweringOutcome,
    ReasoningNotAppliedReason,
};
use meerkat_core::model_profile::capabilities::EffortLevel;
use meerkat_core::provider_matrix::{OpenAiBackendKind, SelfHostedBackendKind};
use meerkat_core::types::UserMessage;
use meerkat_core::{
    AccountLevelStatement, AgentBuilder, AgentError, AgentEvent, AgentLlmClient, AgentSessionStore,
    AgentToolDispatcher, AssistantBlock, LlmStreamResult, Message, ModelApi, OutputSchema,
    Provider, ReasoningBackend, ReasoningEffortDecidedBy, ReasoningEffortRefusalReason,
    ReasoningEffortReport, ReasoningEffortResolution, ReasoningEffortSent, ReasoningFactsSource,
    ReasoningRoute, ReasoningRouteObservation, Session, StopReason, ToolCallView, ToolDef,
    ToolResult, Usage,
};
use serde_json::json;
use tokio::sync::mpsc;

use super::extraction::DEFAULT_EXTRACTION_PROMPT;

const MODEL: &str = "route-test-model";
const COMPACTION_PROMPT: &str = "COMPACT NOW";

// ---------------------------------------------------------------------------
// The routed, recording client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestKind {
    Turn,
    Extraction,
    Summary,
}

/// One request exactly as the loop handed it to the client.
#[derive(Debug, Clone)]
struct Served {
    kind: RequestKind,
    params: Option<ProviderParamsOverride>,
}

impl Served {
    fn openai_tag(&self) -> &OpenAiProviderTag {
        match self
            .params
            .as_ref()
            .and_then(|params| params.provider_tag.as_ref())
        {
            Some(ProviderTag::OpenAi(tag)) => tag,
            other => panic!("the request must carry an OpenAI tag, got {other:?}"),
        }
    }

    /// The request carries `level` in the route's typed field, marked as
    /// resolved, with no neutral level left on it and `route` as witness.
    fn assert_resolved(&self, level: ReasoningEffort, route: &ReasoningRouteObservation) {
        let params = self.params.as_ref().expect("the request carries params");
        assert_eq!(
            params.reasoning_effort, None,
            "the neutral level never reaches a client: {params:?}"
        );
        assert_eq!(
            self.openai_tag().reasoning_effort,
            Some(level),
            "{:?}",
            self.kind
        );
        assert!(
            self.openai_tag().reasoning_effort_resolved,
            "the level is marked as resolved for the route"
        );
        assert_eq!(params.reasoning_route_witness.as_ref(), Some(route));
    }
}

struct RouteClient {
    provider: Provider,
    route: Mutex<Option<ReasoningRouteObservation>>,
    default_tag: Option<ProviderTag>,
    replies: Mutex<VecDeque<LlmStreamResult>>,
    served: Mutex<Vec<Served>>,
    fallback_proposals: AtomicUsize,
}

impl RouteClient {
    fn new(route: Option<ReasoningRouteObservation>) -> Self {
        Self {
            provider: Provider::SelfHosted,
            route: Mutex::new(route),
            default_tag: None,
            replies: Mutex::new(VecDeque::new()),
            served: Mutex::new(Vec::new()),
            fallback_proposals: AtomicUsize::new(0),
        }
    }

    fn with_replies(self, replies: Vec<LlmStreamResult>) -> Self {
        *self.replies.lock().unwrap() = replies.into();
        self
    }

    fn with_default_tag(mut self, tag: ProviderTag) -> Self {
        self.default_tag = Some(tag);
        self
    }

    fn served(&self) -> Vec<Served> {
        self.served.lock().unwrap().clone()
    }
}

#[async_trait]
impl AgentLlmClient for RouteClient {
    fn reasoning_route(&self) -> Option<ReasoningRouteObservation> {
        self.route.lock().unwrap().clone()
    }

    fn default_provider_tag(&self) -> Option<ProviderTag> {
        self.default_tag.clone()
    }

    async fn stream_response(
        &self,
        messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        let last_user_text = messages.iter().rev().find_map(|message| match message {
            Message::User(user) => Some(user.text_content()),
            _ => None,
        });
        let kind = match last_user_text.as_deref() {
            Some(COMPACTION_PROMPT) => RequestKind::Summary,
            Some(DEFAULT_EXTRACTION_PROMPT) => RequestKind::Extraction,
            _ => RequestKind::Turn,
        };
        self.served.lock().unwrap().push(Served {
            kind,
            params: provider_params.cloned(),
        });
        let scripted = self.replies.lock().unwrap().pop_front();
        Ok(scripted.unwrap_or_else(|| {
            text(match kind {
                RequestKind::Summary => "summary",
                RequestKind::Extraction => r#"{"answer":"done"}"#,
                RequestKind::Turn => "done",
            })
        }))
    }

    fn prepare_model_fallback(
        &self,
        _failure: &AgentError,
        _request: &meerkat_core::model_fallback::ModelFallbackRequest<'_>,
    ) -> Result<
        meerkat_core::AgentLlmFallbackSwitch,
        Vec<meerkat_core::AgentLlmFallbackSkippedTarget>,
    > {
        self.fallback_proposals.fetch_add(1, Ordering::SeqCst);
        Err(Vec::new())
    }

    fn provider(&self) -> Provider {
        self.provider
    }

    fn model(&self) -> &'static str {
        MODEL
    }
}

fn text(reply: &str) -> LlmStreamResult {
    LlmStreamResult::new(
        vec![AssistantBlock::Text {
            text: reply.to_string(),
            meta: None,
        }],
        StopReason::EndTurn,
        Usage::default(),
    )
}

fn tool_call(id: &str) -> LlmStreamResult {
    LlmStreamResult::new(
        vec![AssistantBlock::ToolUse {
            id: id.to_string(),
            name: "lookup".into(),
            args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
            meta: None,
        }],
        StopReason::ToolUse,
        Usage::default(),
    )
}

struct LookupTool;

#[async_trait]
impl AgentToolDispatcher for LookupTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::new([Arc::new(ToolDef {
            audience: Default::default(),
            name: "lookup".into(),
            description: "returns a fixed observation".to_string(),
            input_schema: json!({ "type": "object" }),
            provenance: None,
        })])
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Ok(ToolResult::new(call.id.to_string(), "observation".to_string(), false).into())
    }
}

struct NoopStore;

#[async_trait]
impl AgentSessionStore for NoopStore {
    async fn save(&self, _session: &Session) -> Result<(), AgentError> {
        Ok(())
    }

    async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
        Ok(None)
    }
}

/// Compacts once at every model boundary: the System rows stay, everything
/// else becomes one summary row.
struct EveryBoundaryCompactor;

impl Compactor for EveryBoundaryCompactor {
    fn should_compact(&self, ctx: &CompactionContext) -> bool {
        ctx.last_compaction_boundary_index != Some(ctx.session_boundary_index)
    }

    fn compaction_prompt(&self) -> &str {
        COMPACTION_PROMPT
    }

    fn max_summary_tokens(&self) -> u32 {
        32
    }

    fn prepare_for_summarization(
        &self,
        messages: &[Message],
        _budget: Option<SummarizationSourceBudget>,
    ) -> SummarizationSource {
        SummarizationSource::whole(
            meerkat_core::types::materialize_latest_system_prompt_versions(messages),
        )
    }

    fn rebuild_history(&self, messages: &[Message], summary: &str) -> CompactionResult {
        let mut rebuilt = Vec::new();
        let mut retained = Vec::new();
        let mut discarded = Vec::new();
        for (offset, message) in messages.iter().enumerate() {
            let source_offset = u64::try_from(offset).unwrap_or(u64::MAX);
            if matches!(message, Message::System(_)) {
                retained.push(CompactionRetained::new(
                    source_offset,
                    u64::try_from(rebuilt.len()).unwrap_or(u64::MAX),
                    message.clone(),
                ));
                rebuilt.push(message.clone());
            } else {
                discarded.push(CompactionDiscard::new(source_offset, message.clone()));
            }
        }
        let summary_offset = u64::try_from(rebuilt.len()).unwrap_or(u64::MAX);
        let summary_message = Message::User(UserMessage::compaction_summary(format!(
            "{COMPACTION_SUMMARY_PREFIX}{summary}"
        )));
        rebuilt.push(summary_message.clone());
        CompactionResult {
            messages: rebuilt,
            summary: CompactionSummary::new(summary_offset, summary_message),
            retained,
            discarded,
        }
    }
}

/// A session with one finished exchange, so a compaction at the next run's
/// first model boundary has something to summarize.
fn session_with_history() -> Session {
    let mut session = Session::new();
    session
        .set_build_state(meerkat_core::SessionBuildState::default())
        .expect("test session build state should serialize");
    session.push(Message::User(UserMessage::text("an earlier question")));
    session.push(Message::BlockAssistant(
        meerkat_core::types::BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: "an earlier answer".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
        ),
    ));
    session
}

// ---------------------------------------------------------------------------
// Routes and expectations
// ---------------------------------------------------------------------------

/// A self-hosted Chat Completions route whose operator declares `levels`.
fn operator_route(levels: &[EffortLevel]) -> ReasoningRouteObservation {
    ReasoningRouteObservation {
        backend: ReasoningBackend::SelfHosted(SelfHostedBackendKind::SelfHosted),
        api: ModelApi::ChatCompletions,
        account: AccountLevelStatement::NotApplicable,
        operator_levels: Some(levels.to_vec()),
    }
}

const LOW_AND_HIGH: &[EffortLevel] = &[EffortLevel::Low, EffortLevel::High];

fn level_params(level: EffortLevel) -> ProviderParamsOverride {
    ProviderParamsOverride {
        temperature: Some(0.5),
        reasoning_effort: Some(level),
        ..Default::default()
    }
}

/// What a request sent with `level` on `route` reports.
fn resolution(
    requested: EffortLevel,
    level: EffortLevel,
    route: &ReasoningRouteObservation,
    decided_by: ReasoningEffortDecidedBy,
) -> ReasoningEffortResolution {
    ReasoningEffortResolution {
        requested,
        level,
        provider: Provider::SelfHosted,
        model: MODEL.to_string(),
        route: route.route(),
        fields: vec![
            "reasoning_effort".to_string(),
            "reasoning.effort".to_string(),
        ],
        facts: ReasoningFactsSource::Operator,
        decided_by,
    }
}

fn base_builder(params: ProviderParamsOverride) -> AgentBuilder {
    AgentBuilder::new()
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .provider_params(params)
}

type TestAgent = crate::agent::Agent<RouteClient, LookupTool, NoopStore>;

async fn build(client: &Arc<RouteClient>, builder: AgentBuilder) -> TestAgent {
    builder
        .build_standalone(
            Arc::clone(client),
            Arc::new(LookupTool),
            Arc::new(NoopStore),
        )
        .await
}

async fn run_collecting(
    agent: &mut TestAgent,
    prompt: &str,
) -> (Result<crate::types::RunResult, AgentError>, Vec<AgentEvent>) {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(1024);
    let result = agent.run_with_events(prompt.to_string().into(), tx).await;
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    (result, events)
}

/// The per-request reports of a run, in order: turn number, fallback
/// attempt, resolution.
fn request_reports(
    events: &[AgentEvent],
) -> Vec<(Option<u32>, Option<u32>, ReasoningEffortResolution)> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::RequestReasoningEffortResolved {
                turn_number,
                fallback_attempt,
                resolution,
            } => Some((*turn_number, *fallback_attempt, resolution.clone())),
            _ => None,
        })
        .collect()
}

/// What `run_started` reported, once per `run_started` event.
fn run_started_reports(events: &[AgentEvent]) -> Vec<Option<ReasoningEffortResolution>> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::RunStarted {
                reasoning_effort, ..
            } => Some(reasoning_effort.clone()),
            _ => None,
        })
        .collect()
}

fn refusal_of(error: &AgentError) -> &meerkat_core::ReasoningEffortRefusal {
    match error {
        AgentError::ReasoningEffortRefused(refusal) => refusal,
        other => panic!("expected the typed reasoning effort refusal, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// A turn request, an extraction request and a compaction summary request of
/// a session that sets the level each reach the client with the route's
/// typed field in place of the neutral level, and each is reported once.
#[tokio::test]
async fn every_request_kind_is_resolved_against_its_route() {
    let route = operator_route(LOW_AND_HIGH);
    let sent = resolution(
        EffortLevel::High,
        EffortLevel::High,
        &route,
        ReasoningEffortDecidedBy::Request,
    );

    // A turn request.
    let client = Arc::new(RouteClient::new(Some(route.clone())));
    let mut agent = build(&client, base_builder(level_params(EffortLevel::High))).await;
    let (result, events) = run_collecting(&mut agent, "hello").await;
    result.expect("a turn at a level the route takes runs");
    let served = client.served();
    assert_eq!(served.len(), 1);
    assert_eq!(served[0].kind, RequestKind::Turn);
    served[0].assert_resolved(ReasoningEffort::High, &route);
    assert_eq!(
        served[0].params.as_ref().unwrap().temperature,
        Some(0.5),
        "the request's other params stay"
    );
    assert_eq!(
        request_reports(&events),
        vec![(Some(0), None, sent.clone())]
    );

    // A turn request and the extraction request after it.
    let client =
        Arc::new(RouteClient::new(Some(route.clone())).with_replies(vec![text("not json at all")]));
    let schema = OutputSchema::new(json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}},
        "required": ["answer"]
    }))
    .expect("a valid output schema");
    let mut agent = build(
        &client,
        base_builder(level_params(EffortLevel::High)).output_schema(schema),
    )
    .await;
    let (result, events) = run_collecting(&mut agent, "hello").await;
    result.expect("the extraction request runs at the level");
    let served = client.served();
    assert_eq!(
        served.iter().map(|served| served.kind).collect::<Vec<_>>(),
        vec![RequestKind::Turn, RequestKind::Extraction]
    );
    for request in &served {
        request.assert_resolved(ReasoningEffort::High, &route);
    }
    let reports = request_reports(&events);
    assert_eq!(reports.len(), 2, "one report per request: {reports:?}");
    assert!(
        reports
            .iter()
            .all(|(_, fallback, sent_with)| { fallback.is_none() && *sent_with == sent })
    );

    // A turn that compacts first: the summary request and the turn request.
    let client = Arc::new(RouteClient::new(Some(route.clone())));
    let mut agent = build(
        &client,
        base_builder(level_params(EffortLevel::High))
            .resume_session(session_with_history())
            .with_runtime_execution_kind_for_test(
                meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
            )
            .compactor(Arc::new(EveryBoundaryCompactor)),
    )
    .await;
    let (result, events) = run_collecting(&mut agent, "hello").await;
    let result = result.expect("a compacting turn runs at the level");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::CompactionFailed { .. })),
        "the summary request is resolved, not refused"
    );
    let served = client.served();
    assert_eq!(
        served.iter().map(|served| served.kind).collect::<Vec<_>>(),
        vec![RequestKind::Summary, RequestKind::Turn]
    );
    for request in &served {
        request.assert_resolved(ReasoningEffort::High, &route);
    }
    // The summary request is outside a turn; the turn request is turn 0.
    assert_eq!(
        request_reports(&events),
        vec![(None, None, sent.clone()), (Some(0), None, sent.clone())]
    );
    assert_eq!(
        result
            .reasoning_effort
            .expect("the run reports its level")
            .sent[0]
            .requests,
        2,
        "the summary request counts into the run's report"
    );
}

/// A level the route does not take, and a route nobody observed: the turn
/// fails with the typed refusal, the client is never called, nothing is
/// retried, no fallback is proposed, and the session's identity is as it
/// was.
#[tokio::test]
async fn refusal_makes_no_provider_call_and_changes_no_model_or_route() {
    for (route, reason, supported, facts) in [
        (
            Some(operator_route(LOW_AND_HIGH)),
            ReasoningEffortRefusalReason::UnsupportedLevel,
            Some(LOW_AND_HIGH.to_vec()),
            Some(ReasoningFactsSource::Operator),
        ),
        (
            None,
            ReasoningEffortRefusalReason::SupportedLevelsUnknown,
            None,
            None,
        ),
    ] {
        let client = Arc::new(RouteClient::new(route.clone()));
        let mut agent = build(&client, base_builder(level_params(EffortLevel::Xhigh))).await;
        let identity_before = serde_json::to_value(agent.session().session_metadata()).unwrap();
        let (result, events) = run_collecting(&mut agent, "hello").await;
        let error = result.expect_err("the level is refused");
        let refusal = refusal_of(&error);
        assert_eq!(refusal.requested, EffortLevel::Xhigh);
        assert_eq!(refusal.reason, reason);
        assert_eq!(refusal.supported, supported);
        assert_eq!(refusal.facts, facts);
        assert_eq!(
            refusal.route,
            route.as_ref().map(ReasoningRouteObservation::route),
            "a route nobody observed is never guessed"
        );
        assert_eq!(refusal.provider, Provider::SelfHosted);
        assert_eq!(refusal.model, MODEL);

        assert!(client.served().is_empty(), "no request reaches the client");
        assert_eq!(
            client.fallback_proposals.load(Ordering::SeqCst),
            0,
            "a refusal asks for no fallback"
        );
        assert_eq!(
            serde_json::to_value(agent.session().session_metadata()).unwrap(),
            identity_before,
            "the session's identity is unchanged"
        );
        assert!(
            !events.iter().any(|event| matches!(
                event,
                AgentEvent::Retrying { .. }
                    | AgentEvent::ModelFallbackStaged { .. }
                    | AgentEvent::ModelFallbackCommitted { .. }
                    | AgentEvent::ModelFallbackSkipped { .. }
                    | AgentEvent::RequestReasoningEffortResolved { .. }
            )),
            "a refusal is not retried, starts no fallback and reports no request"
        );
        // The refusal ends the run before it is announced as started.
        assert!(run_started_reports(&events).is_empty(), "no run_started");
        let failed = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::RunFailed { error_report, .. } => Some(error_report.clone()),
                _ => None,
            })
            .expect("the run fails with a report");
        assert_eq!(
            failed.reason,
            Some(
                meerkat_core::event::AgentErrorReason::ReasoningEffortRefused {
                    refusal: Box::new(refusal.clone()),
                }
            )
        );
    }
}

/// `run_started` carries the resolution against the session's route, a run
/// with three requests reports each one, and the run result holds one row
/// with the count. A session that sets no level reports none of it.
#[tokio::test]
async fn run_start_request_events_and_the_run_result_report_the_level() {
    let route = operator_route(LOW_AND_HIGH);
    let sent = resolution(
        EffortLevel::High,
        EffortLevel::High,
        &route,
        ReasoningEffortDecidedBy::Request,
    );
    let client = Arc::new(
        RouteClient::new(Some(route.clone()))
            .with_replies(vec![tool_call("call-1"), tool_call("call-2")]),
    );
    let mut agent = build(&client, base_builder(level_params(EffortLevel::High))).await;
    let (result, events) = run_collecting(&mut agent, "hello").await;
    let result = result.expect("the run completes");

    assert_eq!(run_started_reports(&events), vec![Some(sent.clone())]);
    assert_eq!(client.served().len(), 3);
    assert_eq!(
        request_reports(&events),
        vec![
            (Some(0), None, sent.clone()),
            (Some(1), None, sent.clone()),
            (Some(2), None, sent.clone()),
        ]
    );
    assert_eq!(
        result.reasoning_effort,
        Some(ReasoningEffortReport {
            requested: EffortLevel::High,
            sent: vec![ReasoningEffortSent {
                level: EffortLevel::High,
                provider: Provider::SelfHosted,
                model: MODEL.to_string(),
                route: ReasoningRoute::new(
                    ReasoningBackend::SelfHosted(SelfHostedBackendKind::SelfHosted),
                    ModelApi::ChatCompletions,
                ),
                fields: vec![
                    "reasoning_effort".to_string(),
                    "reasoning.effort".to_string()
                ],
                facts: ReasoningFactsSource::Operator,
                decided_by: ReasoningEffortDecidedBy::Request,
                requests: 3,
            }],
        })
    );

    // The next run starts its own report.
    let (result, _) = run_collecting(&mut agent, "again").await;
    assert_eq!(
        result
            .expect("the second run completes")
            .reasoning_effort
            .unwrap()
            .sent[0]
            .requests,
        1
    );

    // No level: the requests are as they always were and nothing is reported.
    let client = Arc::new(RouteClient::new(Some(route)));
    let mut agent = build(
        &client,
        base_builder(ProviderParamsOverride {
            temperature: Some(0.5),
            ..Default::default()
        }),
    )
    .await;
    let (result, events) = run_collecting(&mut agent, "hello").await;
    assert_eq!(result.expect("the run completes").reasoning_effort, None);
    assert_eq!(run_started_reports(&events), vec![None]);
    assert!(request_reports(&events).is_empty());
    let served = client.served();
    assert_eq!(
        served[0].params,
        Some(ProviderParamsOverride {
            temperature: Some(0.5),
            ..Default::default()
        }),
        "a request without the level is handed on untouched"
    );

    // The same with a client that has a default provider tag: without a
    // level nothing joins that tag to the request. The params arrive as the
    // session set them, with no tag; applying its default is the client's.
    let client = Arc::new(
        RouteClient::new(Some(operator_route(LOW_AND_HIGH))).with_default_tag(ProviderTag::OpenAi(
            OpenAiProviderTag {
                seed: Some(7),
                ..Default::default()
            },
        )),
    );
    let mut agent = build(
        &client,
        base_builder(ProviderParamsOverride {
            temperature: Some(0.5),
            ..Default::default()
        }),
    )
    .await;
    let (result, events) = run_collecting(&mut agent, "hello").await;
    assert_eq!(result.expect("the run completes").reasoning_effort, None);
    assert!(request_reports(&events).is_empty());
    assert_eq!(
        client.served()[0].params,
        Some(ProviderParamsOverride {
            temperature: Some(0.5),
            ..Default::default()
        }),
        "without a level the client's default tag is not written into the request"
    );
}

/// A client's own default tag applies to a request that carries none. A
/// resolved level is written into that tag, not into a new one that would
/// stand in its place, and a setting of the default tag that conflicts with
/// the level is judged like the request's own.
#[tokio::test]
async fn a_resolved_level_joins_the_clients_default_tag() {
    let route = operator_route(LOW_AND_HIGH);
    let default_tag = |effort| {
        ProviderTag::OpenAi(OpenAiProviderTag {
            seed: Some(7),
            reasoning_effort: effort,
            ..Default::default()
        })
    };

    let client =
        Arc::new(RouteClient::new(Some(route.clone())).with_default_tag(default_tag(None)));
    let mut agent = build(&client, base_builder(level_params(EffortLevel::High))).await;
    let (result, _) = run_collecting(&mut agent, "hello").await;
    result.expect("the turn runs");
    let served = client.served();
    served[0].assert_resolved(ReasoningEffort::High, &route);
    assert_eq!(
        served[0].openai_tag().seed,
        Some(7),
        "the client's default fields are sent beside the level"
    );

    // The default tag sets another effort: a conflict, named, nothing sent.
    let client = Arc::new(
        RouteClient::new(Some(route)).with_default_tag(default_tag(Some(ReasoningEffort::Low))),
    );
    let mut agent = build(&client, base_builder(level_params(EffortLevel::High))).await;
    let (result, _) = run_collecting(&mut agent, "hello").await;
    let error = result.expect_err("the default tag's effort conflicts");
    let refusal = refusal_of(&error);
    assert_eq!(
        refusal.reason,
        ReasoningEffortRefusalReason::ConflictingProviderEffort
    );
    assert_eq!(
        refusal.conflicting_param.as_deref(),
        Some("provider_params.provider_tag.reasoning_effort")
    );
    assert_eq!(refusal.conflicting_level, Some(EffortLevel::Low));
    assert!(client.served().is_empty());
}

/// A route that changes between the resolution and the client's own
/// preparation: the client asks for a new preparation, the level is resolved
/// again against the route as it is then, and the one request that is sent
/// carries that resolution. A level the new route does not take is refused
/// instead, and nothing is sent.
#[tokio::test]
async fn a_route_change_resolves_the_level_again_before_sending() {
    /// Like the provider adapter: at preparation it compares the witness of
    /// a request with the route it serves, and refuses a stale one as a
    /// route change. Its route moves right after a chosen read of it.
    struct MovingClient {
        inner: Arc<RouteClient>,
        reads: AtomicUsize,
        move_after_read: usize,
        next_route: Mutex<Option<ReasoningRouteObservation>>,
        stale_preparations: AtomicUsize,
    }

    #[async_trait]
    impl AgentLlmClient for MovingClient {
        fn reasoning_route(&self) -> Option<ReasoningRouteObservation> {
            let observed = self.inner.reasoning_route();
            if self.reads.fetch_add(1, Ordering::SeqCst) + 1 == self.move_after_read
                && let Some(next) = self.next_route.lock().unwrap().take()
            {
                *self.inner.route.lock().unwrap() = Some(next);
            }
            observed
        }

        fn prepare_request_attempt_authorized(
            self: Arc<Self>,
            messages: Arc<Vec<Message>>,
            tools: Arc<[Arc<ToolDef>]>,
            max_tokens: u32,
            temperature: Option<f32>,
            provider_params: Option<ProviderParamsOverride>,
            authorization: Option<meerkat_core::LlmRequestAuthorization>,
        ) -> Result<Arc<dyn meerkat_core::AgentLlmRequestAttempt>, AgentError> {
            let witness = provider_params
                .as_ref()
                .and_then(|params| params.reasoning_route_witness.clone());
            if witness != self.inner.reasoning_route() {
                self.stale_preparations.fetch_add(1, Ordering::SeqCst);
                return Err(AgentError::llm(
                    "self_hosted",
                    crate::error::LlmFailureReason::ProviderError(
                        crate::error::LlmProviderError::retryable(
                            crate::error::LlmProviderErrorKind::AuthorizationRouteChanged,
                            json!({"message": "route changed"}),
                        ),
                    ),
                    "the route changed after the level was resolved",
                ));
            }
            Arc::clone(&self.inner).prepare_request_attempt_authorized(
                messages,
                tools,
                max_tokens,
                temperature,
                provider_params,
                authorization,
            )
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

        fn provider(&self) -> Provider {
            Provider::SelfHosted
        }

        fn model(&self) -> &'static str {
            MODEL
        }
    }

    let first = operator_route(LOW_AND_HIGH);
    for (second, sent_level) in [
        // The endpoint moves: the level is written for the new route.
        (
            ReasoningRouteObservation {
                api: ModelApi::Responses,
                ..operator_route(LOW_AND_HIGH)
            },
            Some(ReasoningEffort::High),
        ),
        // The stated levels change and the level is no longer among them.
        (operator_route(&[EffortLevel::Low]), None),
    ] {
        let client = Arc::new(MovingClient {
            inner: Arc::new(RouteClient::new(Some(first.clone()))),
            reads: AtomicUsize::new(0),
            // The first read is the run-start report, the second the
            // resolution for the request: the route moves right after it.
            move_after_read: 2,
            next_route: Mutex::new(Some(second.clone())),
            stale_preparations: AtomicUsize::new(0),
        });
        let mut agent = base_builder(level_params(EffortLevel::High))
            .build_standalone(
                Arc::clone(&client),
                Arc::new(LookupTool),
                Arc::new(NoopStore),
            )
            .await;
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(1024);
        let result = agent.run_with_events("hello".to_string().into(), tx).await;
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert_eq!(
            client.stale_preparations.load(Ordering::SeqCst),
            1,
            "the first preparation carried the first route's resolution and was refused"
        );
        let served = client.inner.served();
        match sent_level {
            Some(level) => {
                result.expect("the turn runs on the route as it is when the request is sent");
                assert_eq!(served.len(), 1, "one request is sent");
                served[0].assert_resolved(level, &second);
                let reports = request_reports(&events);
                assert_eq!(reports.len(), 1, "{reports:?}");
                assert_eq!(reports[0].2.route, second.route());
                assert_eq!(reports[0].2.fields, vec!["reasoning.effort".to_string()]);
            }
            None => {
                let error = result.expect_err("the level left the route's statement");
                let refusal = refusal_of(&error);
                assert_eq!(
                    refusal.reason,
                    ReasoningEffortRefusalReason::UnsupportedLevel
                );
                assert_eq!(refusal.supported, Some(vec![EffortLevel::Low]));
                assert!(served.is_empty(), "nothing is sent with the old resolution");
                assert!(request_reports(&events).is_empty());
            }
        }
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::Retrying { .. })),
            "a route change costs no retry"
        );
    }
}

/// The turn's reasoning preference and the session's level: where the route
/// takes the preferred level it is sent in place of the session's and
/// reported as decided by the preference; where it does not, and where the
/// turn names its own level, the session's level is sent.
#[tokio::test]
async fn preference_and_strict_level_interplay() {
    let route = operator_route(LOW_AND_HIGH);
    let lowered = |events: &[AgentEvent]| {
        events
            .iter()
            .find_map(|event| match event {
                AgentEvent::RequestReasoningLowered {
                    baseline, outcome, ..
                } => Some((*baseline, *outcome)),
                _ => None,
            })
            .expect("the attempt reports its lowering")
    };
    for (disposition, sent_level, decided_by, outcome) in [
        // The route takes the preferred level.
        (
            ReasoningBatchDisposition::Apply(EffortLevel::Low),
            EffortLevel::Low,
            ReasoningEffortDecidedBy::Preference,
            Some(ReasoningLoweringOutcome::Applied(EffortLevel::Low)),
        ),
        // The route does not take it: the session's level stands.
        (
            ReasoningBatchDisposition::Apply(EffortLevel::Medium),
            EffortLevel::High,
            ReasoningEffortDecidedBy::Request,
            Some(ReasoningLoweringOutcome::NotApplied(
                ReasoningNotAppliedReason::UnsupportedLevel,
            )),
        ),
        // The turn named a level of its own: the preference applies nothing.
        (
            ReasoningBatchDisposition::SupersededByExplicit,
            EffortLevel::High,
            ReasoningEffortDecidedBy::Request,
            None,
        ),
    ] {
        let client = Arc::new(RouteClient::new(Some(route.clone())));
        let mut agent = build(&client, base_builder(level_params(EffortLevel::High))).await;
        agent.set_active_turn_request_reasoning(Some(disposition));
        let (result, events) = run_collecting(&mut agent, "voice turn").await;
        let result = result.expect("the turn runs");

        let served = client.served();
        assert_eq!(served.len(), 1, "{disposition:?}");
        let effort = match sent_level {
            EffortLevel::Low => ReasoningEffort::Low,
            _ => ReasoningEffort::High,
        };
        served[0].assert_resolved(effort, &route);
        assert_eq!(
            lowered(&events),
            (
                ReasoningLoweringBaseline::Explicit(EffortLevel::High),
                outcome
            ),
            "{disposition:?}"
        );
        let expected = resolution(EffortLevel::High, sent_level, &route, decided_by);
        assert_eq!(
            request_reports(&events),
            vec![(Some(0), None, expected.clone())],
            "{disposition:?}"
        );
        let report = result.reasoning_effort.expect("the run reports its level");
        assert_eq!(report.requested, EffortLevel::High);
        assert_eq!(report.sent.len(), 1);
        assert_eq!(report.sent[0].level, sent_level);
        assert_eq!(report.sent[0].decided_by, decided_by);
        // The session's own level is untouched.
        assert_eq!(
            agent.config.provider_params.params.reasoning_effort,
            Some(EffortLevel::High)
        );
    }
}

/// A turn's reasoning preference on a route whose levels an account states
/// is judged by the account's statement: applied where the model's entry
/// lists the level (also for a model no catalog row describes), not applied
/// where it does not or where the account states nothing.
#[tokio::test]
async fn preference_on_a_copilot_route_uses_the_account_list() {
    let copilot = |account| ReasoningRouteObservation {
        backend: ReasoningBackend::OpenAi(OpenAiBackendKind::Copilot),
        api: ModelApi::Responses,
        account,
        operator_levels: None,
    };
    let listed = AccountLevelStatement::Stated(vec![EffortLevel::Low, EffortLevel::High]);

    for (account, preferred, outcome) in [
        (
            listed.clone(),
            EffortLevel::Low,
            ReasoningLoweringOutcome::Applied(EffortLevel::Low),
        ),
        (
            listed,
            EffortLevel::Medium,
            ReasoningLoweringOutcome::NotApplied(ReasoningNotAppliedReason::UnsupportedLevel),
        ),
        (
            AccountLevelStatement::NotStated,
            EffortLevel::Low,
            ReasoningLoweringOutcome::NotApplied(ReasoningNotAppliedReason::UnknownSupportedLevels),
        ),
        (
            AccountLevelStatement::ListUnavailable,
            EffortLevel::Low,
            ReasoningLoweringOutcome::NotApplied(ReasoningNotAppliedReason::UnknownSupportedLevels),
        ),
    ] {
        let mut client = RouteClient::new(Some(copilot(account.clone())));
        client.provider = Provider::OpenAI;
        let client = Arc::new(client);
        let mut agent = build(
            &client,
            base_builder(ProviderParamsOverride {
                temperature: Some(0.5),
                ..Default::default()
            }),
        )
        .await;
        agent.set_active_turn_request_reasoning(Some(ReasoningBatchDisposition::Apply(preferred)));
        let (result, events) = run_collecting(&mut agent, "voice turn").await;
        result.expect("a preference never fails a turn");
        let reported = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::RequestReasoningLowered { outcome, .. } => *outcome,
                _ => None,
            })
            .expect("the attempt reports its lowering");
        assert_eq!(reported, outcome, "{account:?} {preferred:?}");
        let served = client.served();
        let sent_effort = served[0]
            .params
            .as_ref()
            .and_then(|params| params.provider_tag.as_ref())
            .and_then(|tag| match tag {
                ProviderTag::OpenAi(tag) => tag.reasoning_effort,
                _ => None,
            });
        match outcome {
            ReasoningLoweringOutcome::Applied(_) => {
                assert_eq!(sent_effort, Some(ReasoningEffort::Low), "{account:?}");
            }
            ReasoningLoweringOutcome::NotApplied(_) => {
                assert_eq!(
                    sent_effort, None,
                    "{account:?}: the request is the baseline"
                );
            }
        }
        // A preference alone is no neutral level: nothing is reported as one.
        assert!(request_reports(&events).is_empty());
    }
}
