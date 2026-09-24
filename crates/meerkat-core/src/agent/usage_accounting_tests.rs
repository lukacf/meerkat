//! Observable usage accounting of the agent loop.
//!
//! These tests pin the worked example in
//! `docs/reference/usage-accounting.mdx` against the loop itself, not against
//! [`crate::CumulativeUsage`] arithmetic in isolation. They exist because the
//! two accounts a consumer can read off the event stream have different
//! coverage:
//!
//! - `turn_completed` carries exactly one provider call, the assistant turn
//!   that closed the run. Intermediate tool-loop calls emit no usage-bearing
//!   event.
//! - `run_completed` carries the session-cumulative total over every provider
//!   call recorded on the session, so it does not reconcile with the sum of the
//!   turn rows and it keeps growing across runs of the same session.
//!
//! If these numbers change, the documented example is wrong and must change
//! with them (`scripts/test_usage_accounting_docs.py` requires both sides).
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::field_reassign_with_default
)]

use crate as meerkat_core;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use meerkat_core::{
    AgentBuilder, AgentError, AgentEvent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    AssistantBlock, LlmStreamResult, Message, Provider, ProviderTokenAccounting, StopReason,
    ToolCallView, ToolDef, ToolResult, TurnUsage, Usage,
};
use serde_json::value::RawValue;
use tokio::sync::mpsc;

/// The resolved model of every documented call. The loop rejects turn usage
/// whose accounting identity disagrees with the client, so this is also the
/// client's model.
const DOCUMENTED_MODEL: &str = "claude-opus-5";

/// One Anthropic-shaped provider call of the documented worked example.
///
/// Anthropic reports uncached, cache-write, and cache-read input as disjoint
/// components; the presented input for the call is their sum.
struct DocumentedCall {
    uncached_input: u64,
    cache_creation_input: u64,
    cache_read_input: u64,
    output: u64,
    /// Whether the call requests a tool, i.e. whether the loop continues
    /// instead of closing the run on this call.
    requests_tool: bool,
}

impl DocumentedCall {
    fn presented_tokens(&self) -> u64 {
        self.uncached_input + self.cache_creation_input + self.cache_read_input
    }

    fn usage(&self) -> Usage {
        TurnUsage::new(
            Usage {
                input_tokens: self.uncached_input,
                output_tokens: self.output,
                cache_creation_tokens: Some(self.cache_creation_input),
                cache_read_tokens: Some(self.cache_read_input),
                provider_accounting: None,
            },
            ProviderTokenAccounting::anthropic(
                DOCUMENTED_MODEL,
                self.uncached_input,
                self.cache_creation_input,
                self.cache_read_input,
            ),
        )
        .into_inner()
    }

    fn stream_result(&self, call_index: usize) -> Result<LlmStreamResult, AgentError> {
        let blocks = if self.requests_tool {
            let args = RawValue::from_string("{}".to_string())
                .map_err(|error| AgentError::InternalError(error.to_string()))?;
            vec![AssistantBlock::ToolUse {
                id: format!("call-{call_index}"),
                name: "lookup".into(),
                args,
                meta: None,
            }]
        } else {
            vec![AssistantBlock::Text {
                text: "answer".to_string(),
                meta: None,
            }]
        };
        let stop_reason = if self.requests_tool {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };
        Ok(LlmStreamResult::new(blocks, stop_reason, self.usage()))
    }
}

/// The documented script: three calls in the first run (two of them tool
/// calls), then one call in a second run on the same session.
fn documented_script() -> Vec<DocumentedCall> {
    vec![
        DocumentedCall {
            uncached_input: 1000,
            cache_creation_input: 4000,
            cache_read_input: 0,
            output: 200,
            requests_tool: true,
        },
        DocumentedCall {
            uncached_input: 300,
            cache_creation_input: 0,
            cache_read_input: 4000,
            output: 150,
            requests_tool: true,
        },
        DocumentedCall {
            uncached_input: 120,
            cache_creation_input: 0,
            cache_read_input: 4300,
            output: 90,
            requests_tool: false,
        },
        DocumentedCall {
            uncached_input: 200,
            cache_creation_input: 0,
            cache_read_input: 4500,
            output: 60,
            requests_tool: false,
        },
    ]
}

struct ScriptedAnthropicClient {
    script: Vec<DocumentedCall>,
    next: AtomicUsize,
}

impl ScriptedAnthropicClient {
    fn new(script: Vec<DocumentedCall>) -> Self {
        Self {
            script,
            next: AtomicUsize::new(0),
        }
    }

    fn calls_made(&self) -> usize {
        self.next.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl AgentLlmClient for ScriptedAnthropicClient {
    async fn stream_response(
        &self,
        _messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        let index = self.next.fetch_add(1, Ordering::SeqCst);
        let call = self.script.get(index).ok_or_else(|| {
            AgentError::InternalError(format!("scripted client exhausted at call {index}"))
        })?;
        call.stream_result(index)
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    fn model(&self) -> &'static str {
        DOCUMENTED_MODEL
    }
}

struct LookupTool;

#[async_trait]
impl AgentToolDispatcher for LookupTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::new([Arc::new(ToolDef {
            name: "lookup".into(),
            description: "returns a fixed observation".to_string(),
            input_schema: serde_json::json!({ "type": "object" }),
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
    async fn save(&self, _session: &meerkat_core::Session) -> Result<(), AgentError> {
        Ok(())
    }

    async fn load(&self, _id: &str) -> Result<Option<meerkat_core::Session>, AgentError> {
        Ok(None)
    }
}

/// What one run of the loop published for a consumer reading only events.
struct ObservedRun {
    turn_rows: Vec<TurnUsage>,
    run_totals: Vec<meerkat_core::CumulativeUsage>,
}

fn drain(rx: &mut mpsc::Receiver<AgentEvent>) -> ObservedRun {
    let mut turn_rows = Vec::new();
    let mut run_totals = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            // A measured run publishes a row per closing call. An absent row
            // would be a different observation entirely, so this collector
            // refuses to flatten one into the measured sequence.
            AgentEvent::TurnCompleted {
                usage: Some(usage), ..
            } => turn_rows.push(usage),
            AgentEvent::TurnCompleted { usage: None, .. } => {
                panic!("the documented worked example measures every call")
            }
            AgentEvent::RunCompleted { usage, .. } => run_totals.push(usage),
            _ => {}
        }
    }
    ObservedRun {
        turn_rows,
        run_totals,
    }
}

#[tokio::test]
async fn turn_rows_cover_one_call_while_the_run_total_is_session_cumulative() {
    assert_eq!(
        documented_script()
            .iter()
            .map(DocumentedCall::presented_tokens)
            .collect::<Vec<_>>(),
        vec![5000, 4300, 4420, 4700],
        "the scripted calls are the documented worked example"
    );

    let client = Arc::new(ScriptedAnthropicClient::new(documented_script()));
    let mut agent = AgentBuilder::new()
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .build_standalone(client.clone(), Arc::new(LookupTool), Arc::new(NoopStore))
        .await;

    // ---- First run: three provider calls, two of them tool calls. ----------
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(128);
    let first = agent
        .run_with_events("first".to_string().into(), tx)
        .await
        .expect("scripted tool loop should complete");
    assert_eq!(client.calls_made(), 3, "the run made three provider calls");
    let observed_first = drain(&mut rx);

    assert_eq!(
        observed_first.turn_rows.len(),
        1,
        "a tool-using run publishes one turn row, not one per provider call"
    );
    let first_row = &observed_first.turn_rows[0];
    assert_eq!(
        first_row.accounting().provider,
        Provider::Anthropic,
        "the turn row attributes itself without a session-metadata join"
    );
    assert_eq!(first_row.accounting().model, DOCUMENTED_MODEL);
    assert_eq!(
        first_row.presented_tokens(),
        4420,
        "the turn row is the call that closed the run"
    );
    assert_eq!(first_row.output_tokens, 90);
    assert_eq!(
        first_row.input_tokens, 120,
        "the raw Anthropic counter on that call excludes cached input"
    );

    assert_eq!(observed_first.run_totals.len(), 1);
    let first_total = &observed_first.run_totals[0];
    assert_eq!(first_total.input_tokens, 13_720);
    assert_eq!(first_total.output_tokens, 440);
    assert_eq!(first_total.total_tokens(), 14_160);
    assert_eq!(
        first.usage.total_tokens(),
        14_160,
        "RunResult.usage is the same cumulative value as the event"
    );
    assert!(
        first_total.provider_accounting.is_none(),
        "a possibly multi-model aggregate must not claim one per-call convention"
    );
    assert_eq!(
        first_total.input_tokens - first_row.presented_tokens(),
        9300,
        "the input the turn rows of this run do not account for"
    );

    // ---- Second run on the same session: one provider call. ---------------
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(128);
    let second = agent
        .run_with_events("second".to_string().into(), tx)
        .await
        .expect("second run should complete");
    assert_eq!(client.calls_made(), 4);
    let observed_second = drain(&mut rx);

    assert_eq!(observed_second.turn_rows.len(), 1);
    let second_row = &observed_second.turn_rows[0];
    assert_eq!(second_row.presented_tokens(), 4700);
    assert_eq!(second_row.output_tokens, 60);

    assert_eq!(observed_second.run_totals.len(), 1);
    let second_total = &observed_second.run_totals[0];
    assert_eq!(
        second_total.input_tokens, 18_420,
        "the run total carries the earlier run's calls too"
    );
    assert_eq!(second_total.output_tokens, 500);
    assert_eq!(second_total.total_tokens(), 18_920);
    assert_eq!(second.usage.total_tokens(), 18_920);

    // ---- The documented aggregations, right and wrong. --------------------
    let attributed_input = first_row.presented_tokens() + second_row.presented_tokens();
    let attributed_output = first_row.output_tokens + second_row.output_tokens;
    assert_eq!(attributed_input, 9120);
    assert_eq!(attributed_output, 150);
    assert_eq!(
        attributed_input + attributed_output,
        9270,
        "per-model attribution covers a strict subset of the session total"
    );
    assert!(
        attributed_input + attributed_output < second_total.total_tokens(),
        "the turn rows must never be presented as reconciling with the run total"
    );
    assert_eq!(
        second_total.total_tokens() - (attributed_input + attributed_output),
        9650,
        "tokens the session total charges that no turn row attributes"
    );

    let naive_raw_input = first_row.input_tokens + second_row.input_tokens;
    assert_eq!(
        naive_raw_input, 320,
        "summing raw per-call input_tokens is the documented undercount"
    );

    let naive_run_total_sum = first_total.total_tokens() + second_total.total_tokens();
    assert_eq!(
        naive_run_total_sum, 33_080,
        "summing run totals across runs is the documented double count"
    );
    assert!(
        naive_run_total_sum > second_total.total_tokens(),
        "the wrong aggregation must stay observably wrong"
    );
}
