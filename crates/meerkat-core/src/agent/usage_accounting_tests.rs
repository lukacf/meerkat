//! Observable usage accounting of the agent loop.
//!
//! These tests pin the worked example in
//! `docs/reference/usage-accounting.mdx` against the loop itself, not against
//! [`crate::CumulativeUsage`] arithmetic in isolation. They exist because the
//! two accounts a consumer can read off the event stream have different
//! coverage:
//!
//! - `turn_completed` carries exactly one provider call, and every committed
//!   agent-loop call publishes one: each tool-loop call and the call that
//!   closes the run.
//! - `run_completed` carries the session-cumulative total over every provider
//!   call recorded on the session, so it keeps growing across runs of the same
//!   session and must never be summed with the turn rows.
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
                reasoning_tokens: None,
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
            // A measured run publishes a row per committed call. An absent row
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
async fn turn_rows_cover_every_call_while_the_run_total_is_session_cumulative() {
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
        observed_first
            .turn_rows
            .iter()
            .map(TurnUsage::presented_tokens)
            .collect::<Vec<_>>(),
        vec![5000, 4300, 4420],
        "a tool-using run publishes one turn row per provider call, in order"
    );
    assert_eq!(
        observed_first
            .turn_rows
            .iter()
            .map(|row| row.output_tokens)
            .collect::<Vec<_>>(),
        vec![200, 150, 90]
    );
    for row in &observed_first.turn_rows {
        assert_eq!(
            row.accounting().provider,
            Provider::Anthropic,
            "each turn row attributes itself without a session-metadata join"
        );
        assert_eq!(row.accounting().model, DOCUMENTED_MODEL);
    }
    let closing_row = &observed_first.turn_rows[2];
    assert_eq!(
        closing_row.input_tokens, 120,
        "the raw Anthropic counter on the closing call excludes cached input"
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
    let first_rows_input: u64 = observed_first
        .turn_rows
        .iter()
        .map(TurnUsage::presented_tokens)
        .sum();
    assert_eq!(
        first_rows_input, first_total.input_tokens,
        "on a fresh session the turn rows of a tool loop reconcile with the run total"
    );
    assert_eq!(
        first.run_usage.as_ref().map(|usage| usage.total_tokens()),
        Some(14_160),
        "run_usage is the run's own delta"
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
    let second_run_usage = second.run_usage.as_ref().expect("second run_usage");
    assert_eq!(second_run_usage.input_tokens, 4700);
    assert_eq!(second_run_usage.output_tokens, 60);
    assert_eq!(second_run_usage.cache_read_tokens, Some(4500));

    // ---- Run-scoped views: every provider call, and only this run's. -------
    let first_run = first.run_usage.as_ref().expect("first run usage");
    assert_eq!(
        first_run.input_tokens, 13_720,
        "a fresh session's run usage is its total"
    );
    assert_eq!(first_run.output_tokens, 440);
    assert_eq!(first_run.cache_creation_tokens, Some(4_000));
    assert_eq!(first_run.cache_read_tokens, Some(8_300));
    assert_eq!(
        first.request_usage.len(),
        3,
        "request_usage has a row per provider call, tool-loop calls included"
    );
    assert_eq!(
        first
            .request_usage
            .iter()
            .map(TurnUsage::presented_tokens)
            .sum::<u64>(),
        first_run.input_tokens,
        "the request rows reconcile with the run's own usage"
    );
    let second_run = second.run_usage.as_ref().expect("second run usage");
    assert_eq!(second_run.input_tokens, 4_700, "only the second run's call");
    assert_eq!(second_run.output_tokens, 60);
    assert_eq!(second_run.cache_read_tokens, Some(4_500));
    assert_eq!(second.request_usage.len(), 1);
    assert_eq!(second.request_usage[0].presented_tokens(), 4_700);
    assert_eq!(second_total.cache_read_tokens, Some(12_800));

    // ---- The documented aggregations, right and wrong. --------------------
    let all_rows = observed_first
        .turn_rows
        .iter()
        .chain(observed_second.turn_rows.iter())
        .collect::<Vec<_>>();
    let attributed_input: u64 = all_rows.iter().map(|row| row.presented_tokens()).sum();
    let attributed_output: u64 = all_rows.iter().map(|row| row.output_tokens).sum();
    assert_eq!(attributed_input, 18_420);
    assert_eq!(attributed_output, 500);
    assert_eq!(
        attributed_input + attributed_output,
        second_total.total_tokens(),
        "with no extraction or compaction, the turn rows attribute the whole session total"
    );

    let naive_raw_input: u64 = all_rows.iter().map(|row| row.input_tokens).sum();
    assert_eq!(
        naive_raw_input, 1620,
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

/// Sessions saved before 0.8.22 summed raw per-call cache counters into the
/// stored total, so cache reads can exceed the input total. Loaded through the
/// real serde path, the normalized total must honour the subset invariant and
/// stay monotone across the next recorded turn, so run deltas and compaction
/// rollback deltas never go backwards.
#[test]
fn legacy_session_totals_normalize_to_the_invariant_and_stay_monotone() {
    let mut encoded = serde_json::to_value(crate::Session::new()).expect("serialize session");
    encoded["usage"] = serde_json::json!({
        "input_tokens": 1000,
        "output_tokens": 50,
        "cache_creation_tokens": 4000,
        "cache_read_tokens": 50000
    });
    let mut session: crate::Session =
        serde_json::from_value(encoded).expect("legacy session loads");
    assert_eq!(
        session.total_usage().cache_read_tokens,
        Some(50_000),
        "the stored value is left untouched"
    );
    let baseline = crate::CumulativeUsage::from_usage(session.total_usage()).into_inner();
    assert_eq!(baseline.cache_read_tokens, Some(1000));
    assert_eq!(
        baseline.cache_creation_tokens,
        Some(0),
        "reads and writes are clamped jointly to disjoint parts of input"
    );

    session.record_turn_usage(&TurnUsage::new(
        Usage {
            input_tokens: 300,
            output_tokens: 10,
            cache_creation_tokens: Some(0),
            cache_read_tokens: Some(4000),
            ..Default::default()
        },
        crate::ProviderTokenAccounting::anthropic("claude-opus-5", 300, 0, 4000),
    ));
    let live = crate::CumulativeUsage::from_usage(session.total_usage()).into_inner();
    let delta = live
        .cumulative_delta_since(&baseline)
        .expect("normalized legacy totals stay monotone");
    assert_eq!(delta.input_tokens, 4300);
    assert_eq!(delta.cache_read_tokens, Some(4000));
    assert_eq!(delta.cache_creation_tokens, Some(0));
    assert!(live.cache_read_tokens.unwrap() <= live.input_tokens);
}

/// A structured-output run: one tool call, the answer, then an extraction
/// attempt that fails validation and a retry that passes.
fn structured_output_script() -> Vec<DocumentedCall> {
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
            requests_tool: false,
        },
        DocumentedCall {
            uncached_input: 50,
            cache_creation_input: 0,
            cache_read_input: 4300,
            output: 30,
            requests_tool: false,
        },
        DocumentedCall {
            uncached_input: 70,
            cache_creation_input: 0,
            cache_read_input: 4350,
            output: 40,
            requests_tool: false,
        },
    ]
}

/// Plays `structured_output_script`, answering the extraction calls with the
/// given texts in order.
struct StructuredOutputClient {
    inner: ScriptedAnthropicClient,
    extraction_texts: Vec<&'static str>,
}

#[async_trait]
impl AgentLlmClient for StructuredOutputClient {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        let result = self
            .inner
            .stream_response(messages, tools, max_tokens, temperature, provider_params)
            .await?;
        let index = self.inner.calls_made() - 1;
        // Calls 0 and 1 are the agentic loop; later calls are extraction.
        let Some(text) = index
            .checked_sub(2)
            .and_then(|attempt| self.extraction_texts.get(attempt))
        else {
            return Ok(result);
        };
        let (_, stop_reason, usage) = result.into_parts();
        Ok(LlmStreamResult::new(
            vec![AssistantBlock::Text {
                text: (*text).to_string(),
                meta: None,
            }],
            stop_reason,
            usage,
        ))
    }

    fn provider(&self) -> Provider {
        self.inner.provider()
    }

    fn model(&self) -> &'static str {
        DOCUMENTED_MODEL
    }
}

#[tokio::test]
async fn every_request_of_a_structured_output_run_publishes_one_usage_row() {
    let schema = meerkat_core::OutputSchema::new(serde_json::json!({
        "type": "object",
        "properties": { "answer": { "type": "string" } },
        "required": ["answer"]
    }))
    .expect("valid schema");
    let client = Arc::new(StructuredOutputClient {
        inner: ScriptedAnthropicClient::new(structured_output_script()),
        extraction_texts: vec![r#"{"wrong": 1}"#, r#"{"answer": "42"}"#],
    });
    let mut agent = AgentBuilder::new()
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .output_schema(schema)
        .structured_output_retries(1)
        .build_standalone(client.clone(), Arc::new(LookupTool), Arc::new(NoopStore))
        .await;

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let result = agent
        .run_with_events("structured".to_string().into(), tx)
        .await
        .expect("structured output run should complete");
    assert_eq!(
        client.inner.calls_made(),
        4,
        "tool call, answer, two extraction attempts"
    );
    assert_eq!(
        result.structured_output,
        Some(serde_json::json!({"answer": "42"}))
    );

    let mut turn_rows = Vec::new();
    let mut turn_stop_reasons = Vec::new();
    let mut extraction_rows = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::TurnCompleted {
                stop_reason,
                usage: Some(usage),
            } => {
                turn_stop_reasons.push(stop_reason);
                turn_rows.push(usage);
            }
            AgentEvent::ExtractionSucceeded { request_usage, .. } => {
                extraction_rows = request_usage;
            }
            _ => {}
        }
    }
    assert_eq!(
        turn_stop_reasons,
        vec![StopReason::ToolUse, StopReason::EndTurn],
        "the tool-loop call and the closing call each complete a turn; extraction does not"
    );
    assert_eq!(
        extraction_rows
            .iter()
            .map(TurnUsage::presented_tokens)
            .collect::<Vec<_>>(),
        vec![4350, 4420],
        "each extraction attempt, including the one that failed validation, publishes a row"
    );

    // Folding every published row the way the session does reproduces the
    // run's own usage exactly: nothing the run was charged is missing from
    // the event stream.
    let mut folded = meerkat_core::CumulativeUsage::default();
    for row in turn_rows.iter().chain(extraction_rows.iter()) {
        folded.add_turn(row);
    }
    let run_usage = result.run_usage.expect("run_usage");
    assert_eq!(folded.as_usage(), &run_usage);
    assert_eq!(run_usage.input_tokens, 5000 + 4300 + 4350 + 4420);
    assert_eq!(
        result.request_usage.len(),
        turn_rows.len() + extraction_rows.len(),
        "the event rows and the result rows describe the same requests"
    );
}

/// Guards the agent's normalized run baseline: on a pre-0.8.22 session the
/// first run's `run_usage` is that run's calls, not `None` and not the legacy
/// raw sums.
#[tokio::test]
async fn legacy_session_first_run_reports_its_own_run_usage() {
    let client = Arc::new(ScriptedAnthropicClient::new(vec![DocumentedCall {
        uncached_input: 300,
        cache_creation_input: 0,
        cache_read_input: 4000,
        output: 10,
        requests_tool: false,
    }]));
    let mut agent = AgentBuilder::new()
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .build_standalone(client, Arc::new(LookupTool), Arc::new(NoopStore))
        .await;
    // Load a pre-0.8.22 raw summed usage total into the agent's own session.
    let mut encoded = serde_json::to_value(agent.session()).expect("serialize session");
    encoded["usage"] = serde_json::json!({
        "input_tokens": 1000,
        "output_tokens": 50,
        "cache_creation_tokens": 4000,
        "cache_read_tokens": 50000
    });
    *agent.session_mut() = serde_json::from_value(encoded).expect("legacy session loads");

    let result = agent
        .run("resume".to_string().into())
        .await
        .expect("run on a legacy session");
    let run_usage = result
        .run_usage
        .expect("a legacy session's first run still reports its own usage");
    assert_eq!(run_usage.input_tokens, 4300);
    assert_eq!(run_usage.output_tokens, 10);
    assert_eq!(run_usage.cache_read_tokens, Some(4000));
    assert_eq!(run_usage.cache_creation_tokens, Some(0));
    assert_eq!(result.usage.input_tokens, 5300);
    assert_eq!(result.usage.cache_read_tokens, Some(5000));
    assert!(
        result.usage.cache_read_tokens.unwrap() + result.usage.cache_creation_tokens.unwrap()
            <= result.usage.input_tokens,
        "the reported total keeps reads and writes disjoint parts of input"
    );
}
