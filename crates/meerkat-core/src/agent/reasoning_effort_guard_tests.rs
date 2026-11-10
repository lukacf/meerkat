//! The interim fail-closed guard for the provider-neutral reasoning effort
//! level, through the real agent loop.
//!
//! Until a request's level is resolved for the route that serves it, a
//! request whose params carry the level must not reach any client: the turn
//! fails with a configuration error before the client is asked to prepare or
//! stream anything, and so does a compaction summary request. The client
//! here is a plain `AgentLlmClient`, not the provider adapter, so nothing
//! but the agent's own guard stands between the level and the client.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use crate as meerkat_core;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use meerkat_core::compact::{
    COMPACTION_SUMMARY_PREFIX, CompactionContext, CompactionDiscard, CompactionResult,
    CompactionRetained, CompactionSummary, Compactor, SummarizationSource,
    SummarizationSourceBudget,
};
use meerkat_core::lifecycle::run_primitive::ProviderParamsOverride;
use meerkat_core::model_profile::capabilities::EffortLevel;
use meerkat_core::types::UserMessage;
use meerkat_core::{
    AgentBuilder, AgentError, AgentEvent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    AssistantBlock, LlmStreamResult, Message, Session, StopReason, ToolCallView, ToolDef,
    ToolResult, TurnUsage, Usage,
};
use tokio::sync::mpsc;

const REFUSAL: &str = "reasoning_effort cannot be resolved by this build";
const COMPACTION_PROMPT: &str = "COMPACT NOW";

/// Counts every request it is asked to serve, by kind.
#[derive(Default)]
struct CountingClient {
    turn_requests: AtomicUsize,
    summary_requests: AtomicUsize,
}

impl CountingClient {
    fn requests(&self) -> (usize, usize) {
        (
            self.turn_requests.load(Ordering::SeqCst),
            self.summary_requests.load(Ordering::SeqCst),
        )
    }
}

#[async_trait]
impl AgentLlmClient for CountingClient {
    async fn stream_response(
        &self,
        messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        let summary_request = messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .is_some_and(|text| text == COMPACTION_PROMPT);
        let text = if summary_request {
            self.summary_requests.fetch_add(1, Ordering::SeqCst);
            "summary"
        } else {
            self.turn_requests.fetch_add(1, Ordering::SeqCst);
            "done"
        };
        Ok(LlmStreamResult::new(
            vec![AssistantBlock::Text {
                text: text.to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
            TurnUsage::host_declared(
                meerkat_core::Provider::Other,
                "mock-model",
                Usage::default(),
            )
            .into_inner(),
        ))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    fn model(&self) -> &'static str {
        "mock-model"
    }
}

struct NoTools;

#[async_trait]
impl AgentToolDispatcher for NoTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from(Vec::new())
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Ok(ToolResult::new(call.id.to_string(), "unused".to_string(), false).into())
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

fn params(level: Option<EffortLevel>) -> ProviderParamsOverride {
    ProviderParamsOverride {
        temperature: Some(0.5),
        reasoning_effort: level,
        ..Default::default()
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
    session.append_external_assistant_blocks(vec![AssistantBlock::Text {
        text: "an earlier answer".to_string(),
        meta: None,
    }]);
    session
}

struct Outcome {
    result: Result<(), AgentError>,
    turn_requests: usize,
    summary_requests: usize,
    compaction_failures: Vec<String>,
}

async fn run_one_turn(level: Option<EffortLevel>, compacting: bool) -> Outcome {
    let client = Arc::new(CountingClient::default());
    let mut builder = AgentBuilder::new()
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .provider_params(params(level));
    if compacting {
        builder = builder
            .resume_session(session_with_history())
            .with_runtime_execution_kind_for_test(
                meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
            )
            .compactor(Arc::new(EveryBoundaryCompactor));
    }
    let mut agent = builder
        .build_standalone(Arc::clone(&client), Arc::new(NoTools), Arc::new(NoopStore))
        .await;
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let result = agent
        .run_with_events("hello".to_string().into(), tx)
        .await
        .map(|_| ());
    let mut compaction_failures = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::CompactionFailed { reason } = event {
            compaction_failures.push(reason.to_string());
        }
    }
    let (turn_requests, summary_requests) = client.requests();
    Outcome {
        result,
        turn_requests,
        summary_requests,
        compaction_failures,
    }
}

fn assert_refused(result: &Result<(), AgentError>) {
    assert!(
        matches!(result, Err(AgentError::ConfigError(message)) if message == REFUSAL),
        "the turn must fail with the configuration refusal: {result:?}"
    );
}

#[tokio::test]
async fn core_refuses_an_unresolved_level_before_any_client_call() {
    // A turn whose session params carry the level: refused, and the client
    // served nothing.
    let outcome = run_one_turn(Some(EffortLevel::High), false).await;
    assert_refused(&outcome.result);
    assert_eq!((outcome.turn_requests, outcome.summary_requests), (0, 0));

    // The same turn without the level is served.
    let outcome = run_one_turn(None, false).await;
    outcome.result.expect("a turn without the level runs");
    assert_eq!((outcome.turn_requests, outcome.summary_requests), (1, 0));

    // A compaction summary request with the level: the summary is refused
    // (the compaction fails and the history stays), then the turn's own
    // request is refused; the client served neither.
    let outcome = run_one_turn(Some(EffortLevel::High), true).await;
    assert_refused(&outcome.result);
    assert_eq!((outcome.turn_requests, outcome.summary_requests), (0, 0));
    assert_eq!(
        outcome.compaction_failures.len(),
        1,
        "the summary request is refused once: {:?}",
        outcome.compaction_failures
    );

    // The same compacting turn without the level: one summary request, one
    // turn request, no compaction failure.
    let outcome = run_one_turn(None, true).await;
    outcome
        .result
        .expect("a compacting turn without the level runs");
    assert_eq!((outcome.turn_requests, outcome.summary_requests), (1, 1));
    assert!(outcome.compaction_failures.is_empty());
}
