//! The compaction summary's source is sized for the model that writes it, and
//! a history no rewrite can get under the threshold compacts at the cadence.
//!
//! A factory-built agent compacts with the `DefaultCompactor`. The source
//! tests run two turns of one session against a scripted client and read the
//! source the summary request carries: the second turn's first boundary
//! compacts the first turn.

use super::*;
use async_trait::async_trait;
use meerkat_client::{LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::config::CustomModelConfig;
use meerkat_core::{Message, StopReason, TurnUsage, Usage};
use std::sync::Mutex;

const MODEL: &str = "summary-budget-test-model";
const MARKER: &str = "MARKER-IN-THE-MIDDLE-OF-THE-FIRST-TURN";
/// Output reserve of the summary call: the config's `max_summary_tokens`.
const MAX_SUMMARY_TOKENS: u64 = 4_096;

/// Answers every turn with `ok`, reporting `input_tokens` of usage, and
/// records the messages of each compaction summary request.
struct SummarySourceRecordingClient {
    input_tokens: u64,
    summary_requests: Mutex<Vec<Vec<Message>>>,
}

#[async_trait]
impl LlmClient for SummarySourceRecordingClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_client::types::LlmStream<'a> {
        let summary_request = matches!(
            request.messages.last(),
            Some(Message::User(user)) if user.text_content().contains("CONTEXT COMPACTION")
        );
        if summary_request {
            self.summary_requests
                .lock()
                .unwrap()
                .push(request.messages.clone());
        }
        let events = vec![
            LlmEvent::TextDelta {
                delta: if summary_request { "summary" } else { "ok" }.to_string(),
                meta: None,
            },
            LlmEvent::UsageUpdate {
                usage: TurnUsage::host_declared(
                    Provider::OpenAI,
                    &request.model,
                    Usage {
                        input_tokens: self.input_tokens,
                        output_tokens: 1,
                        ..Usage::default()
                    },
                ),
            },
            LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::EndTurn),
            },
        ];
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// A first turn of `first_turn_bytes` bytes with [`MARKER`] in its middle.
fn first_turn(first_turn_bytes: usize) -> String {
    let half = "x".repeat(first_turn_bytes / 2);
    format!("{half}{MARKER}{half}")
}

/// Runs two turns on a custom model with `context_window`, a per-build
/// compaction `threshold` and a client reporting `input_tokens`, and returns
/// the source of the one summary request: its messages before the prompt.
async fn summary_source(
    context_window: Option<u32>,
    threshold: u64,
    input_tokens: u64,
    first_turn_bytes: usize,
) -> Vec<Message> {
    let temp = tempfile::tempdir().expect("temp session store");
    let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
    let mut config = Config::default();
    config.models.custom.insert(
        MODEL.to_string(),
        CustomModelConfig {
            provider: Provider::OpenAI,
            display_name: None,
            context_window,
            max_input_tokens: None,
            max_output_tokens: Some(4_096),
            vision: Some(false),
            web_search: Some(false),
            call_timeout_secs: None,
        },
    );
    let session = Session::new();
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let bindings = runtime
        .prepare_bindings(session.id().clone())
        .await
        .expect("session runtime bindings");
    let client = Arc::new(SummarySourceRecordingClient {
        input_tokens,
        summary_requests: Mutex::new(Vec::new()),
    });
    let mut build = AgentBuildConfig::new(MODEL);
    build.provider = Some(Provider::OpenAI);
    build.llm_client_override = Some(client.clone());
    build.resume_session = Some(session);
    build.runtime_build_mode = RuntimeBuildMode::SessionOwned(bindings);
    build.override_builtins = ToolCategoryOverride::Disable;
    build.auto_compact_threshold_override = std::num::NonZeroU64::new(threshold);
    let mut agent = factory
        .build_agent(build, &config)
        .await
        .expect("agent on the custom model");
    agent.set_runtime_execution_kind(Some(
        meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
    ));
    agent
        .run(meerkat_core::ContentInput::Text(first_turn(
            first_turn_bytes,
        )))
        .await
        .expect("first turn");
    // Only the summary request matters; whether the rewrite commits does not.
    agent.set_runtime_execution_kind(Some(
        meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
    ));
    let _second = agent
        .run(meerkat_core::ContentInput::Text("second".into()))
        .await;

    let mut requests = client.summary_requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "the second turn compacts the first once");
    let mut source = requests.remove(0);
    source.pop();
    source
}

fn serialized_bytes(source: &[Message]) -> u64 {
    u64::try_from(serde_json::to_vec(source).unwrap().len()).unwrap()
}

/// A large-window model summarizes the whole first turn, even when a small
/// compaction threshold makes it compact early: the source is sized for the
/// summarizing model, not for the threshold.
#[tokio::test]
async fn summary_source_reads_the_whole_history_a_large_window_holds() {
    let source = summary_source(Some(64_000), 4_000, 5_000, 30_000).await;
    assert!(
        serde_json::to_string(&source).unwrap().contains(MARKER),
        "the {} byte source must keep the whole first turn",
        serialized_bytes(&source)
    );
}

/// A small-window model with a large explicit threshold gets a source its
/// window holds, not one sized for the threshold.
#[tokio::test]
async fn summary_source_fits_a_small_window_under_a_large_threshold() {
    let source = summary_source(Some(16_000), 400_000, 500_000, 60_000).await;
    let window_bytes = 2 * (16_000 - MAX_SUMMARY_TOKENS);
    assert!(
        serialized_bytes(&source) <= window_bytes,
        "the {} byte source must fit the {window_bytes} bytes the window holds",
        serialized_bytes(&source)
    );
}

/// A model without a declared context window keeps the threshold bound: a
/// quarter of the compaction threshold.
#[tokio::test]
async fn summary_source_without_a_declared_window_keeps_the_threshold_bound() {
    let source = summary_source(None, 40_000, 50_000, 30_000).await;
    assert!(
        serialized_bytes(&source) <= 10_000,
        "the {} byte source must stay within a quarter of the 40,000 threshold",
        serialized_bytes(&source)
    );
    assert!(
        !serde_json::to_string(&source).unwrap().contains(MARKER),
        "the bounded source trims the middle of the first turn"
    );
}

/// A System prompt alone over the compaction threshold leaves every rewrite
/// over it: every boundary crosses. After the first rewrite proves that,
/// the crossings wait for the compaction cadence instead of paying for a
/// summary call before every model call.
#[tokio::test]
async fn a_floor_over_the_threshold_compacts_at_the_cadence_not_at_every_boundary() {
    const TURNS: usize = 8;
    let temp = tempfile::tempdir().expect("temp session store");
    let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
    let mut config = Config::default();
    config.models.custom.insert(
        MODEL.to_string(),
        CustomModelConfig {
            provider: Provider::OpenAI,
            display_name: None,
            context_window: Some(200_000),
            max_input_tokens: None,
            max_output_tokens: Some(4_096),
            vision: Some(false),
            web_search: Some(false),
            call_timeout_secs: None,
        },
    );
    let session = Session::new();
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let bindings = runtime
        .prepare_bindings(session.id().clone())
        .await
        .expect("session runtime bindings");
    let client = Arc::new(SummarySourceRecordingClient {
        input_tokens: 0,
        summary_requests: Mutex::new(Vec::new()),
    });
    let mut build = AgentBuildConfig::new(MODEL);
    build.provider = Some(Provider::OpenAI);
    build.llm_client_override = Some(client.clone());
    build.resume_session = Some(session);
    build.runtime_build_mode = RuntimeBuildMode::SessionOwned(bindings);
    build.override_builtins = ToolCategoryOverride::Disable;
    // About 4,000 estimated tokens of System prompt against a 2,000-token
    // threshold: no rewrite can get the history under it.
    build.system_prompt = meerkat_core::SystemPromptOverride::Set("s".repeat(16_000));
    build.auto_compact_threshold_override = std::num::NonZeroU64::new(2_000);
    let mut agent = factory
        .build_agent(build, &config)
        .await
        .expect("agent on the custom model");

    for turn in 0..TURNS {
        agent.set_runtime_execution_kind(Some(
            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
        ));
        agent
            .run(meerkat_core::ContentInput::Text(format!("turn {turn}")))
            .await
            .expect("every turn completes");
    }

    let summary_calls = client.summary_requests.lock().unwrap().len();
    assert_eq!(
        summary_calls, 3,
        "{TURNS} turns cross at every boundary after the first; with the default cadence of 3 \
         the summary runs at boundaries 1, 4 and 7, not at all seven"
    );
}

/// One write of an operator instruction in the session's own target: an
/// activation of `body`, or a revoke without one.
fn instruction_write(
    key: &str,
    revision: u64,
    body: Option<&str>,
) -> meerkat_core::SessionInstructionWrite {
    meerkat_core::SessionInstructionWrite {
        slot: meerkat_core::InstructionSlot {
            namespace: meerkat_core::types::InstructionNamespace::new("app.example").unwrap(),
            key: meerkat_core::types::InstructionKey::new(key).unwrap(),
        },
        revision: meerkat_core::InstructionRevision::new(revision).unwrap(),
        change: match body {
            Some(body) => meerkat_core::SessionInstructionChange::Activate {
                body: body.to_string(),
                content_sha256: None,
            },
            None => meerkat_core::SessionInstructionChange::Revoke,
        },
    }
}

/// Operator-instruction entry rows never reach the `DefaultCompactor`: the
/// summary request of a factory-built agent carries no instruction body,
/// whether the body is in effect, superseded or revoked. The first turn
/// applies two slots in the leading System prefix; before the second, one
/// is superseded and the other revoked, and the second turn's first
/// boundary compacts.
#[tokio::test]
async fn no_default_compactor_summary_request_carries_an_instruction_body() {
    const FORMAL: &str = "Answer formally.";
    const SHORT: &str = "Use short sentences.";
    const WARM: &str = "Answer warmly.";
    let temp = tempfile::tempdir().expect("temp session store");
    let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
    let mut config = Config::default();
    config.models.custom.insert(
        MODEL.to_string(),
        CustomModelConfig {
            provider: Provider::OpenAI,
            display_name: None,
            context_window: Some(64_000),
            max_input_tokens: None,
            max_output_tokens: Some(4_096),
            vision: Some(false),
            web_search: Some(false),
            call_timeout_secs: None,
        },
    );
    let session = Session::new();
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let bindings = runtime
        .prepare_bindings(session.id().clone())
        .await
        .expect("session runtime bindings");
    let client = Arc::new(SummarySourceRecordingClient {
        input_tokens: 5_000,
        summary_requests: Mutex::new(Vec::new()),
    });
    let mut build = AgentBuildConfig::new(MODEL);
    build.provider = Some(Provider::OpenAI);
    build.llm_client_override = Some(client.clone());
    build.resume_session = Some(session);
    build.runtime_build_mode = RuntimeBuildMode::SessionOwned(bindings);
    build.override_builtins = ToolCategoryOverride::Disable;
    build.auto_compact_threshold_override = std::num::NonZeroU64::new(4_000);
    let mut agent = factory
        .build_agent(build, &config)
        .await
        .expect("agent on the custom model");

    for write in [
        instruction_write("tone", 1, Some(FORMAL)),
        instruction_write("style", 1, Some(SHORT)),
    ] {
        agent
            .session_mut()
            .set_session_instruction(write)
            .expect("the session records the write");
    }
    let outcome = agent
        .apply_run_start_operator_instructions(meerkat_core::InstructionRunStart::FreshRun, None)
        .expect("the first run start resolves");
    assert!(
        matches!(
            outcome,
            meerkat_core::RunStartInstructionOutcome::Applied { .. }
        ),
        "a fresh session's first run takes the entry in its leading System prefix: {outcome:?}"
    );
    agent.set_runtime_execution_kind(Some(
        meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
    ));
    agent
        .run(meerkat_core::ContentInput::Text("first".into()))
        .await
        .expect("first turn");

    for write in [
        instruction_write("tone", 2, Some(WARM)),
        instruction_write("style", 2, None),
    ] {
        agent
            .session_mut()
            .set_session_instruction(write)
            .expect("the session records the write");
    }
    // The model's lowering decides whether this run start applies the
    // changes or leaves them to the fold; the summary request is the same.
    agent
        .apply_run_start_operator_instructions(meerkat_core::InstructionRunStart::FreshRun, None)
        .expect("the second run start resolves");
    agent.set_runtime_execution_kind(Some(
        meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
    ));
    // Only the summary request matters; whether the rewrite commits does not.
    let _second = agent
        .run(meerkat_core::ContentInput::Text("second".into()))
        .await;

    let requests = client.summary_requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "the second turn compacts the first once");
    let summary_request = serde_json::to_string(&requests[0]).unwrap();
    assert!(
        summary_request.contains("first"),
        "the summary is written from the first turn: {summary_request}"
    );
    for body in [FORMAL, SHORT, WARM] {
        assert!(
            !summary_request.contains(body),
            "the summary request carries the instruction body {body:?}: {summary_request}"
        );
    }
}
