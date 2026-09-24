//! What an absent or disputed accounting fact may terminalize.
//!
//! A fault may only terminalize what it actually invalidates. These tests pin
//! both halves of that line at the one boundary where the loop turns provider
//! usage into agent state:
//!
//! - Accounting ABSENT. No number exists, so nothing may be recorded and the
//!   token axis must not move - but the model already answered, so the turn
//!   completes, the assistant message commits, and the absence is published as
//!   a typed marker instead of a failure.
//! - Accounting identity DISPUTED. A number exists and is internally
//!   consistent, so the axis still advances on it; only attribution is
//!   published as contested, and never repaired.
//!
//! The two must not collapse into one path: treating a dispute as absence
//! would drop real tokens on the floor, and treating absence as a dispute
//! would require inventing the tokens to dispute.
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::too_many_lines
)]

use crate as meerkat_core;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use meerkat_core::{
    AgentBuilder, AgentError, AgentEvent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    AssistantBlock, LlmStreamResult, Message, Provider, ProviderTokenAccounting, StopReason,
    ToolDef, TurnUsage, UnmeasuredTurnUsageAccounting, Usage,
};
use tokio::sync::mpsc;

const MODEL: &str = "claude-opus-5";

/// How one scripted provider call accounts for itself.
#[derive(Clone, Copy)]
enum ScriptedAccounting {
    /// Ordinary: normalized accounting minted under the requested identity.
    Measured,
    /// The provider stream ended without ever sending a usage event, so the
    /// adapter had nothing to normalize. Raw counters are deliberately left
    /// NON-ZERO here: a fallback to `Usage::input_tokens` would move the token
    /// axis by 999 and these tests would see it.
    Absent,
    /// Normalized accounting arrived naming a different model than the request
    /// it answered.
    DisputedModel(&'static str),
}

struct ScriptedCall {
    text: &'static str,
    presented_input: u64,
    output: u64,
    accounting: ScriptedAccounting,
}

impl ScriptedCall {
    fn usage(&self) -> Usage {
        let raw = Usage {
            input_tokens: self.presented_input,
            output_tokens: self.output,
            ..Usage::default()
        };
        match self.accounting {
            ScriptedAccounting::Measured => TurnUsage::new(
                raw,
                ProviderTokenAccounting::anthropic(MODEL, self.presented_input, 0, 0),
            )
            .into_inner(),
            ScriptedAccounting::Absent => Usage {
                input_tokens: 999,
                output_tokens: 999,
                ..Usage::default()
            },
            ScriptedAccounting::DisputedModel(reported) => TurnUsage::new(
                raw,
                ProviderTokenAccounting::anthropic(reported, self.presented_input, 0, 0),
            )
            .into_inner(),
        }
    }

    fn stream_result(&self) -> LlmStreamResult {
        LlmStreamResult::new(
            vec![AssistantBlock::Text {
                text: self.text.to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
            self.usage(),
        )
    }
}

struct ScriptedClient {
    script: Vec<ScriptedCall>,
    next: AtomicUsize,
}

impl ScriptedClient {
    fn new(script: Vec<ScriptedCall>) -> Self {
        Self {
            script,
            next: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl AgentLlmClient for ScriptedClient {
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
        Ok(call.stream_result())
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    fn model(&self) -> &'static str {
        MODEL
    }
}

struct NoTools;

#[async_trait]
impl AgentToolDispatcher for NoTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::new([])
    }

    async fn dispatch(
        &self,
        _call: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Err(meerkat_core::ToolError::NotFound {
            name: "none".to_string(),
        })
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

/// Everything a host could learn about one run from the event stream alone.
#[derive(Default)]
struct Observed {
    turn_completed: Vec<Option<TurnUsage>>,
    unmeasured: Vec<UnmeasuredTurnUsageAccounting>,
    disputes: Vec<meerkat_core::DisputedTurnUsageAccountingIdentity>,
    run_totals: Vec<meerkat_core::CumulativeUsage>,
}

fn drain(rx: &mut mpsc::Receiver<AgentEvent>) -> Observed {
    let mut observed = Observed::default();
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::TurnCompleted { usage, .. } => observed.turn_completed.push(usage),
            AgentEvent::TurnUsageAccountingUnmeasured { unmeasured, .. } => {
                observed.unmeasured.push(unmeasured);
            }
            AgentEvent::TurnUsageAccountingIdentityDisputed { dispute, .. } => {
                observed.disputes.push(dispute);
            }
            AgentEvent::RunCompleted { usage, .. } => observed.run_totals.push(usage),
            _ => {}
        }
    }
    observed
}

async fn scripted_agent(
    script: Vec<ScriptedCall>,
    limits: crate::budget::BudgetLimits,
) -> meerkat_core::Agent<ScriptedClient, NoTools, NoopStore> {
    AgentBuilder::new()
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .budget(limits)
        .build_standalone(
            Arc::new(ScriptedClient::new(script)),
            Arc::new(NoTools),
            Arc::new(NoopStore),
        )
        .await
}

/// A measured turn followed by an unaccounted one, on the same session.
///
/// Two turns are load-bearing: with only the unaccounted turn every axis would
/// read zero, and "did not advance" would be indistinguishable from "was never
/// set". The first turn puts a non-zero value on every axis so the second can
/// be required to leave it EXACTLY there.
fn measured_then_unmeasured() -> Vec<ScriptedCall> {
    vec![
        ScriptedCall {
            text: "measured answer",
            presented_input: 1000,
            output: 100,
            accounting: ScriptedAccounting::Measured,
        },
        ScriptedCall {
            text: "unaccounted answer",
            presented_input: 0,
            output: 0,
            accounting: ScriptedAccounting::Absent,
        },
    ]
}

/// The owner-reported P0: the model answered, the caller read the answer, and
/// the loop then failed the turn because a number was missing.
#[tokio::test]
async fn absent_accounting_completes_the_turn_and_commits_the_transcript() {
    let mut agent = scripted_agent(
        measured_then_unmeasured(),
        crate::budget::BudgetLimits::unlimited(),
    )
    .await;

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(128);
    agent
        .run_with_events("first".to_string().into(), tx)
        .await
        .expect("the measured turn completes");
    drop(drain(&mut rx));

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(128);
    let second = agent
        .run_with_events("second".to_string().into(), tx)
        .await
        .expect("an absent accounting fact must not fail a completed turn");
    let observed = drain(&mut rx);

    assert_eq!(
        second.text, "unaccounted answer",
        "the caller must receive the answer it already streamed"
    );
    // The aggravator: `TextDelta` reaches the caller in the adapter, while the
    // assistant message is committed after this gate. A turn the user has read
    // must not be missing from the durable transcript.
    assert!(
        agent
            .session()
            .messages()
            .iter()
            .any(|message| message.as_indexable_text().contains("unaccounted answer")),
        "the committed transcript must contain the turn the caller read: {:?}",
        agent.session().messages()
    );

    let [unmeasured] = observed.unmeasured.as_slice() else {
        panic!(
            "the absence must be published as a typed marker, not only logged: {:?}",
            observed.unmeasured
        );
    };
    assert_eq!(unmeasured.marker(), "unmeasured:turn_usage_accounting");
    assert_eq!(unmeasured.provider, Provider::Anthropic);
    assert_eq!(
        unmeasured.model, MODEL,
        "the marker must name the address of the missing measurement"
    );

    let [turn_row] = observed.turn_completed.as_slice() else {
        panic!(
            "the turn completion is a semantic fact and is published either way: {:?}",
            observed.turn_completed
        );
    };
    assert!(
        turn_row.is_none(),
        "absence must be carried as absence, never as a fabricated row: {turn_row:?}"
    );
    assert!(
        observed.disputes.is_empty(),
        "absent accounting is not an identity dispute: {:?}",
        observed.disputes
    );
}

/// The hard constraint: an unaccounted turn moves no accounting axis, and does
/// not reset one either.
#[tokio::test]
async fn absent_accounting_leaves_every_token_axis_exactly_where_it_was() {
    let mut agent = scripted_agent(
        measured_then_unmeasured(),
        crate::budget::BudgetLimits::unlimited().with_max_tokens(1_000_000),
    )
    .await;

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(128);
    agent
        .run_with_events("first".to_string().into(), tx)
        .await
        .expect("the measured turn completes");
    let measured = drain(&mut rx);
    let session_usage_before = agent.session().total_usage();
    let budget_before = agent.budget.token_usage().expect("a token limit is set").0;
    let last_input_tokens_before = agent.last_input_tokens;

    assert_eq!(
        last_input_tokens_before, 1000,
        "the measured turn establishes a non-zero axis to hold"
    );
    assert_eq!(budget_before, 1100);
    assert_eq!(session_usage_before.input_tokens, 1000);

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(128);
    agent
        .run_with_events("second".to_string().into(), tx)
        .await
        .expect("an absent accounting fact must not fail a completed turn");
    let unmeasured = drain(&mut rx);

    assert_eq!(
        agent.last_input_tokens, last_input_tokens_before,
        "an unaccounted turn must neither advance nor reset the presented-token axis"
    );
    assert_eq!(
        agent.budget.token_usage().expect("a token limit is set").0,
        budget_before,
        "no measurement means nothing to charge the budget"
    );
    assert_eq!(
        agent.session().total_usage(),
        session_usage_before,
        "the session account must be unchanged by a turn nobody measured"
    );

    let [measured_total] = measured.run_totals.as_slice() else {
        panic!("one run total per run: {:?}", measured.run_totals);
    };
    let [unmeasured_total] = unmeasured.run_totals.as_slice() else {
        panic!("one run total per run: {:?}", unmeasured.run_totals);
    };
    assert_eq!(
        unmeasured_total, measured_total,
        "the cumulative account a host reads must not move on an unaccounted turn"
    );
    assert_ne!(
        unmeasured_total.input_tokens, 999,
        "raw `input_tokens` must never be substituted for presented tokens"
    );
}

/// The degrade path must not disarm the enforcement it sits beside: a turn
/// that IS measured and does cross the limit still terminalizes.
#[tokio::test]
async fn budget_enforcement_survives_when_accounting_is_present() {
    let mut agent = scripted_agent(
        vec![ScriptedCall {
            text: "measured answer",
            presented_input: 1000,
            output: 100,
            accounting: ScriptedAccounting::Measured,
        }],
        crate::budget::BudgetLimits::unlimited().with_max_tokens(500),
    )
    .await;

    let result = agent
        .run("first".to_string().into())
        .await
        .expect("a budget stop is a terminal outcome, not an error");
    assert_eq!(
        result.terminal_cause_kind,
        Some(meerkat_core::TurnTerminalCauseKind::BudgetExhausted),
        "a measured turn over the limit must still terminalize on the budget"
    );
    assert_eq!(
        agent.budget.token_usage().expect("a token limit is set").0,
        1100,
        "the measured turn is charged before the limit is observed"
    );
}

/// An unaccounted turn adds nothing to the budget, and equally launders
/// nothing off it: a limit an earlier measured turn already crossed stays
/// crossed.
#[tokio::test]
async fn an_unaccounted_turn_does_not_relieve_an_already_exceeded_budget() {
    let mut agent = scripted_agent(
        measured_then_unmeasured(),
        crate::budget::BudgetLimits::unlimited().with_max_tokens(500),
    )
    .await;

    let first = agent
        .run("first".to_string().into())
        .await
        .expect("a budget stop is a terminal outcome, not an error");
    assert_eq!(
        first.terminal_cause_kind,
        Some(meerkat_core::TurnTerminalCauseKind::BudgetExhausted)
    );

    let second = agent
        .run("second".to_string().into())
        .await
        .expect("a budget stop is a terminal outcome, not an error");
    assert_eq!(
        second.terminal_cause_kind,
        Some(meerkat_core::TurnTerminalCauseKind::BudgetExhausted),
        "an unmeasured turn must not read as budget headroom"
    );
}

/// The other side of the asymmetry: a disputed identity keeps its counters, so
/// the axis advances, and the disagreement is published rather than repaired.
#[tokio::test]
async fn disputed_identity_advances_the_axis_and_publishes_both_sides() {
    let mut agent = scripted_agent(
        vec![ScriptedCall {
            text: "answer",
            presented_input: 700,
            output: 30,
            accounting: ScriptedAccounting::DisputedModel("some-other-model"),
        }],
        crate::budget::BudgetLimits::unlimited().with_max_tokens(1_000_000),
    )
    .await;

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(128);
    let result = agent
        .run_with_events("first".to_string().into(), tx)
        .await
        .expect("a contested attribution must not fail a completed turn");
    let observed = drain(&mut rx);

    assert_eq!(result.text, "answer");
    assert_eq!(
        agent.last_input_tokens, 700,
        "the counters are internally consistent, so the axis still advances"
    );
    assert_eq!(
        agent.budget.token_usage().expect("a token limit is set").0,
        730
    );

    let [dispute] = observed.disputes.as_slice() else {
        panic!(
            "the disagreement must reach the host as a typed fact: {:?}",
            observed.disputes
        );
    };
    assert_eq!(dispute.marker(), "disputed:turn_usage_accounting_identity");
    assert_eq!(dispute.active_model, MODEL);
    assert_eq!(
        dispute.reported_model, "some-other-model",
        "the reported identity is published verbatim; overwriting it would launder a guess as agreement"
    );

    let [turn_row] = observed.turn_completed.as_slice() else {
        panic!("one turn row: {:?}", observed.turn_completed);
    };
    let turn_row = turn_row
        .as_ref()
        .expect("a disputed identity still carries its measurement");
    assert_eq!(
        turn_row.accounting().model,
        "some-other-model",
        "the published row must keep the identity its author minted"
    );
    assert!(
        observed.unmeasured.is_empty(),
        "a dispute is not an absence: {:?}",
        observed.unmeasured
    );
}
