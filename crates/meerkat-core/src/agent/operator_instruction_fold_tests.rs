//! Rows for the compaction fold of operator-instruction entry rows, through
//! the real agent loop. A compaction takes every entry row out of the live
//! transcript and puts one snapshot of the folded set right after the
//! leading System prefix, before the summary (none for an empty set). The
//! session-document authority decides what the snapshot renders: what is in
//! effect, or the recorded target when the compaction runs at a fresh run's
//! first request.
//!
//! The compactor here is a host compactor that knows nothing about entry
//! rows: it keeps every System row, as every compactor must, and the fold
//! takes the entry rows out.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use crate as meerkat_core;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_core::compact::{
    COMPACTION_SUMMARY_PREFIX, CompactionContext, CompactionCurator, CompactionCuratorError,
    CompactionDiscard, CompactionResult, CompactionRetained, CompactionSummary, CompactionWindow,
    Compactor, CuratedCompactionSummary, SummarizationSource, SummarizationSourceBudget,
};
use meerkat_core::types::{
    InstructionActivationId, InstructionContentDigest, InstructionKey, InstructionNamespace,
    InstructionRevisionId, InstructionRevisionRef, SystemMessage, UserMessage,
};
use meerkat_core::{
    AgentBuilder, AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    AssistantBlock, InstructionDeferReason, InstructionEffect, InstructionEntryIdentity,
    InstructionEntryKind, InstructionLowering, InstructionRevision, InstructionRunStart,
    InstructionSetErrorCode, InstructionSlot, LlmStreamResult, Message, RunStartInstructionOutcome,
    Session, SessionInstructionChange, SessionInstructionWrite, StopReason, ToolCallView, ToolDef,
    ToolResult, TurnUsage, Usage, effective_operator_instructions,
};
use serde_json::value::RawValue;

const NAMESPACE: &str = "app.example";
const COMPACTION_PROMPT: &str = "COMPACT NOW";

fn mock_usage() -> Usage {
    TurnUsage::host_declared(
        meerkat_core::Provider::Other,
        "mock-model",
        Usage::default(),
    )
    .into_inner()
}

fn text_response(text: &str) -> LlmStreamResult {
    LlmStreamResult::new(
        vec![AssistantBlock::Text {
            text: text.to_string(),
            meta: None,
        }],
        StopReason::EndTurn,
        mock_usage(),
    )
}

/// Answers a compaction summary request with a short summary, and an
/// ordinary request with one tool call while `tool_calls` lasts, then with
/// text. Keeps every summary request's messages and every ordinary
/// request's messages, apart.
#[derive(Default)]
struct FoldClient {
    requests: Mutex<Vec<Vec<Message>>>,
    summaries: Mutex<Vec<Vec<Message>>>,
    tool_calls: AtomicUsize,
    calls: AtomicUsize,
}

impl FoldClient {
    fn requests(&self) -> Vec<Vec<Message>> {
        self.requests.lock().unwrap().clone()
    }

    /// Every compaction summary request: the summary source, then the
    /// compaction prompt.
    fn summaries(&self) -> Vec<Vec<Message>> {
        self.summaries.lock().unwrap().clone()
    }

    fn call_tool_once(&self) {
        self.tool_calls.store(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl AgentLlmClient for FoldClient {
    async fn stream_response(
        &self,
        messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        let summary_request = messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .is_some_and(|text| text == COMPACTION_PROMPT);
        if summary_request {
            self.summaries.lock().unwrap().push(messages.to_vec());
            return Ok(text_response("summary"));
        }
        self.requests.lock().unwrap().push(messages.to_vec());
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.tool_calls.load(Ordering::SeqCst) > 0 {
            self.tool_calls.fetch_sub(1, Ordering::SeqCst);
            return Ok(LlmStreamResult::new(
                vec![AssistantBlock::ToolUse {
                    id: format!("call-{call}"),
                    name: "probe".into(),
                    args: RawValue::from_string("{}".to_string()).unwrap(),
                    meta: None,
                }],
                StopReason::ToolUse,
                mock_usage(),
            ));
        }
        Ok(text_response("done"))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    fn model(&self) -> &'static str {
        "mock-model"
    }
}

struct ProbeTool {
    tools: Arc<[Arc<ToolDef>]>,
}

impl ProbeTool {
    fn new() -> Self {
        Self {
            tools: Arc::from([Arc::new(ToolDef::new(
                "probe",
                "returns",
                serde_json::json!({ "type": "object" }),
            ))]),
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for ProbeTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Ok(ToolResult::new(call.id.to_string(), "probe ok".to_string(), false).into())
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

/// Compacts once, at the `n`-th model boundary after [`Self::arm`]. Its
/// rebuild keeps every System row and a trailing user request, and folds
/// every other row into the summary, which takes the place of the first row
/// it folds.
#[derive(Default)]
struct ArmedCompactor {
    countdown: AtomicUsize,
    rebuilds: AtomicUsize,
    discarded: AtomicUsize,
}

impl ArmedCompactor {
    fn arm(&self, boundary: usize) {
        self.countdown.store(boundary, Ordering::SeqCst);
    }

    fn rebuilds(&self) -> usize {
        self.rebuilds.load(Ordering::SeqCst)
    }

    /// How many source rows the last rebuild folded into its summary.
    fn last_discarded(&self) -> usize {
        self.discarded.load(Ordering::SeqCst)
    }
}

impl Compactor for ArmedCompactor {
    fn should_compact(&self, _ctx: &CompactionContext) -> bool {
        match self.countdown.load(Ordering::SeqCst) {
            0 => false,
            1 => {
                self.countdown.store(0, Ordering::SeqCst);
                true
            }
            remaining => {
                self.countdown.store(remaining - 1, Ordering::SeqCst);
                false
            }
        }
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
        self.rebuilds.fetch_add(1, Ordering::SeqCst);
        let last = messages.len().saturating_sub(1);
        let keep = |offset: usize, message: &Message| {
            matches!(message, Message::System(_))
                || (offset == last
                    && matches!(
                        message,
                        Message::User(user) if user.transcript_role.is_conversational()
                    ))
        };
        let summary_message = Message::User(UserMessage::compaction_summary(format!(
            "{COMPACTION_SUMMARY_PREFIX}{summary}"
        )));
        let mut rebuilt = Vec::new();
        let mut retained = Vec::new();
        let mut discarded = Vec::new();
        let mut summary_mapping = None;
        for (offset, message) in messages.iter().enumerate() {
            let source_offset = u64::try_from(offset).unwrap_or(u64::MAX);
            if keep(offset, message) {
                retained.push(CompactionRetained::new(
                    source_offset,
                    u64::try_from(rebuilt.len()).unwrap_or(u64::MAX),
                    message.clone(),
                ));
                rebuilt.push(message.clone());
                continue;
            }
            if summary_mapping.is_none() {
                summary_mapping = Some(CompactionSummary::new(
                    u64::try_from(rebuilt.len()).unwrap_or(u64::MAX),
                    summary_message.clone(),
                ));
                rebuilt.push(summary_message.clone());
            }
            discarded.push(CompactionDiscard::new(source_offset, message.clone()));
        }
        let summary = match summary_mapping {
            Some(mapping) => mapping,
            None => {
                let mapping = CompactionSummary::new(
                    u64::try_from(rebuilt.len()).unwrap_or(u64::MAX),
                    summary_message.clone(),
                );
                rebuilt.push(summary_message);
                mapping
            }
        };
        self.discarded.store(discarded.len(), Ordering::SeqCst);
        CompactionResult {
            messages: rebuilt,
            summary,
            retained,
            discarded,
        }
    }
}

/// A host curator that writes every summary itself and keeps the window
/// each compaction handed it.
#[derive(Default)]
struct RecordingCurator {
    windows: Mutex<Vec<Vec<Message>>>,
}

impl RecordingCurator {
    fn windows(&self) -> Vec<Vec<Message>> {
        self.windows.lock().unwrap().clone()
    }
}

#[async_trait]
impl CompactionCurator for RecordingCurator {
    async fn curate_summary(
        &self,
        window: CompactionWindow<'_>,
    ) -> Result<CuratedCompactionSummary, CompactionCuratorError> {
        self.windows.lock().unwrap().push(window.messages.to_vec());
        CuratedCompactionSummary::new("curated summary")
    }
}

type TestAgent = meerkat_core::Agent<FoldClient, ProbeTool, NoopStore>;

async fn build_agent(
    session: Session,
    client: &Arc<FoldClient>,
    compactor: &Arc<ArmedCompactor>,
    lowering: InstructionLowering,
) -> TestAgent {
    let compactor: Arc<dyn Compactor> = compactor.clone();
    AgentBuilder::new()
        .resume_session(session)
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .with_operator_instruction_lowering(lowering)
        .compactor(compactor)
        .build_standalone(
            Arc::clone(client),
            Arc::new(ProbeTool::new()),
            Arc::new(NoopStore),
        )
        .await
}

/// [`build_agent`] with a host curator that writes the summaries.
async fn build_curated_agent(
    session: Session,
    client: &Arc<FoldClient>,
    compactor: &Arc<ArmedCompactor>,
    curator: &Arc<RecordingCurator>,
    lowering: InstructionLowering,
) -> TestAgent {
    let compactor: Arc<dyn Compactor> = compactor.clone();
    let curator: Arc<dyn CompactionCurator> = curator.clone();
    AgentBuilder::new()
        .resume_session(session)
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .with_operator_instruction_lowering(lowering)
        .compactor(compactor)
        .compaction_curator(curator)
        .build_standalone(
            Arc::clone(client),
            Arc::new(ProbeTool::new()),
            Arc::new(NoopStore),
        )
        .await
}

/// A session whose transcript is still its leading System prompt.
fn fresh_session() -> Session {
    let mut session = Session::new();
    session
        .set_build_state(meerkat_core::SessionBuildState::default())
        .expect("test session build state should serialize");
    session.append_system_message("You are a careful assistant.");
    session
}

fn exchange(session: &mut Session, user: &str, assistant: &str) {
    session.push(Message::User(UserMessage::text(user)));
    session.push(Message::BlockAssistant(
        meerkat_core::BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: assistant.to_string(),
                meta: None,
            }],
            stop_reason: Some(StopReason::EndTurn),
            identity: meerkat_core::types::TranscriptMessageIdentity::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
            assistant_message_id: None,
        },
    ));
}

/// A session with two exchanges after its prompt and no instruction yet.
fn conversed_session() -> Session {
    let mut session = fresh_session();
    exchange(&mut session, "hello", "hi");
    exchange(&mut session, "how are you", "fine");
    session
}

fn slot(key: &str) -> InstructionSlot {
    InstructionSlot {
        namespace: InstructionNamespace::new(NAMESPACE).unwrap(),
        key: InstructionKey::new(key).unwrap(),
    }
}

fn activate(key: &str, revision: u64, body: &str) -> SessionInstructionWrite {
    SessionInstructionWrite {
        slot: slot(key),
        revision: InstructionRevision::new(revision).unwrap(),
        change: SessionInstructionChange::Activate {
            body: body.to_string(),
            content_sha256: None,
        },
    }
}

fn revoke(key: &str, revision: u64) -> SessionInstructionWrite {
    SessionInstructionWrite {
        slot: slot(key),
        revision: InstructionRevision::new(revision).unwrap(),
        change: SessionInstructionChange::Revoke,
    }
}

fn entry_identities(messages: &[Message]) -> Vec<InstructionEntryIdentity> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::System(system) => system.instruction_entry.clone(),
            _ => None,
        })
        .collect()
}

fn entry_index(messages: &[Message]) -> Option<usize> {
    messages.iter().position(
        |message| matches!(message, Message::System(system) if system.instruction_entry.is_some()),
    )
}

fn entry_row(messages: &[Message]) -> SystemMessage {
    let index = entry_index(messages).expect("an entry row");
    let Message::System(system) = &messages[index] else {
        unreachable!()
    };
    system.clone()
}

fn is_summary(message: &Message) -> bool {
    matches!(message, Message::User(user) if user.transcript_role.is_compaction_summary())
}

/// Every message's text, whatever its kind.
fn transcript_text(messages: &[Message]) -> String {
    serde_json::to_string(messages).unwrap()
}

fn active_slots(identity: &InstructionEntryIdentity) -> Vec<(String, u64)> {
    identity
        .active()
        .iter()
        .map(|active| (active.key.as_str().to_string(), active.revision.get()))
        .collect()
}

async fn apply_and_run(agent: &mut TestAgent, input: &str) -> RunStartInstructionOutcome {
    let outcome = agent
        .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
        .unwrap();
    agent.run(input.into()).await.unwrap();
    outcome
}

/// Two entries, one in the leading System prefix and one after a turn, fold
/// into one snapshot right after the two leading System rows, before the
/// summary.
#[tokio::test]
async fn fold_places_one_snapshot_after_the_leading_system_prefix() {
    let client = Arc::new(FoldClient::default());
    let compactor = Arc::new(ArmedCompactor::default());
    let mut session = fresh_session();
    session.append_system_message("Tools are listed below.");
    let mut agent = build_agent(
        session,
        &client,
        &compactor,
        InstructionLowering::MidConversation,
    )
    .await;
    agent
        .session_mut()
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();
    assert!(matches!(
        apply_and_run(&mut agent, "first").await,
        RunStartInstructionOutcome::Applied {
            entry_sequence: 1,
            ..
        }
    ));
    agent
        .session_mut()
        .set_session_instruction(activate("style", 1, "Use short sentences."))
        .unwrap();
    assert!(matches!(
        apply_and_run(&mut agent, "second").await,
        RunStartInstructionOutcome::Applied {
            entry_sequence: 2,
            ..
        }
    ));
    let prompt_rows = agent.session().messages()[..2].to_vec();

    compactor.arm(1);
    apply_and_run(&mut agent, "third").await;
    assert_eq!(
        compactor.rebuilds(),
        1,
        "the third run's first request compacts"
    );

    let messages = agent.session().messages().to_vec();
    let entries = entry_identities(&messages);
    assert_eq!(entries.len(), 1, "one snapshot replaces both deltas");
    let snapshot = &entries[0];
    assert_eq!(snapshot.kind(), InstructionEntryKind::Snapshot);
    assert_eq!(snapshot.entry_sequence(), 3);
    assert_eq!(
        active_slots(snapshot),
        vec![("style".to_string(), 1), ("tone".to_string(), 1)]
    );
    assert_eq!(
        &messages[..2],
        prompt_rows.as_slice(),
        "the prompt rows stay"
    );
    assert_eq!(
        entry_index(&messages),
        Some(2),
        "the snapshot follows the leading System prefix"
    );
    assert!(is_summary(&messages[3]), "the summary follows the snapshot");
    let row = entry_row(&messages);
    assert!(row.content.contains("Answer formally."));
    assert!(row.content.contains("Use short sentences."));

    let request = client.requests().last().cloned().unwrap();
    assert_eq!(entry_identities(&request).len(), 1);
    assert_eq!(entry_index(&request), Some(2));
    assert_eq!(
        agent
            .session()
            .instruction_target_state()
            .unwrap()
            .unwrap()
            .entry_sequence(),
        3,
        "the target keeps the entry-sequence high-water"
    );
}

/// A superseded body and a revoked body leave the live transcript with the
/// deltas that carried them; only the body in effect is in the snapshot.
#[tokio::test]
async fn fold_drops_superseded_and_revoked_bodies() {
    let client = Arc::new(FoldClient::default());
    let compactor = Arc::new(ArmedCompactor::default());
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &compactor,
        InstructionLowering::MidConversation,
    )
    .await;
    for write in [
        activate("tone", 1, "Answer formally."),
        activate("style", 1, "Use short sentences."),
    ] {
        agent.session_mut().set_session_instruction(write).unwrap();
    }
    apply_and_run(&mut agent, "first").await;
    for write in [activate("tone", 2, "Answer warmly."), revoke("style", 2)] {
        agent.session_mut().set_session_instruction(write).unwrap();
    }
    apply_and_run(&mut agent, "second").await;

    compactor.arm(1);
    apply_and_run(&mut agent, "third").await;
    assert_eq!(compactor.rebuilds(), 1);

    let messages = agent.session().messages().to_vec();
    let entries = entry_identities(&messages);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].kind(), InstructionEntryKind::Snapshot);
    assert_eq!(active_slots(&entries[0]), vec![("tone".to_string(), 2)]);
    let live = transcript_text(&messages);
    assert!(live.contains("Answer warmly."));
    assert!(
        !live.contains("Answer formally."),
        "the superseded body is gone"
    );
    assert!(
        !live.contains("Use short sentences."),
        "the revoked body is gone"
    );
    let request = transcript_text(client.requests().last().unwrap());
    assert!(!request.contains("Answer formally."));
    assert!(!request.contains("Use short sentences."));

    let effective = effective_operator_instructions(agent.session().id(), &messages).unwrap();
    assert_eq!(effective.active_count(), 1);
    assert_eq!(
        effective
            .slot(&slot("tone"))
            .map(|state| state.revision().get()),
        Some(2)
    );
    // The revoked slot's revision stays recorded in the target, so a stale
    // write is still refused after its rows are gone.
    let stale = agent
        .session_mut()
        .set_session_instruction(activate("style", 1, "Use short sentences."))
        .unwrap_err();
    assert_eq!(stale.code(), InstructionSetErrorCode::StaleRevision);
}

fn dia_activation(body: &str) -> meerkat_core::InstructionActivationRequest {
    meerkat_core::InstructionActivationRequest {
        revision: InstructionRevisionRef {
            namespace: InstructionNamespace::new("app.policy").unwrap(),
            key: InstructionKey::new("policy").unwrap(),
            revision_id: InstructionRevisionId::new("rev-1").unwrap(),
            content_sha256: InstructionContentDigest::for_body(body),
        },
        activation_id: InstructionActivationId::new("act-policy").unwrap(),
        expectation: meerkat_core::InstructionActivationExpectation::Absent,
        supersedes: None,
        body: body.to_string(),
    }
}

/// An instruction activation row stays exactly as it was, at its place
/// after the summary; only the operator-instruction entries fold.
#[tokio::test]
async fn fold_keeps_dia_activation_rows_verbatim() {
    let client = Arc::new(FoldClient::default());
    let compactor = Arc::new(ArmedCompactor::default());
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &compactor,
        InstructionLowering::MidConversation,
    )
    .await;
    agent
        .session_mut()
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();
    apply_and_run(&mut agent, "first").await;
    agent
        .session_mut()
        .activate_instruction(dia_activation("Never share credentials."))
        .unwrap();
    let activation_row = agent
        .session()
        .messages()
        .iter()
        .find(|message| {
            matches!(message, Message::System(system) if system.instruction_activation.is_some())
        })
        .cloned()
        .expect("the activation row");
    apply_and_run(&mut agent, "second").await;

    compactor.arm(1);
    apply_and_run(&mut agent, "third").await;
    assert_eq!(compactor.rebuilds(), 1);

    let messages = agent.session().messages().to_vec();
    let activation_rows = messages
        .iter()
        .filter(|message| {
            matches!(message, Message::System(system) if system.instruction_activation.is_some())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        activation_rows,
        vec![&activation_row],
        "the activation row is kept verbatim"
    );
    let activation_index = messages
        .iter()
        .position(|message| message == &activation_row)
        .unwrap();
    let summary_index = messages.iter().position(is_summary).unwrap();
    assert!(
        activation_index > summary_index,
        "the activation row keeps its place after the folded turns"
    );
    let entries = entry_identities(&messages);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].kind(), InstructionEntryKind::Snapshot);
    assert_eq!(entry_index(&messages), Some(1));
}

/// One compaction folds the operator-instruction entries into one snapshot
/// and retires a superseded instruction activation body in the same
/// rewrite: the superseded activation row holds its retired header and
/// marker, the superseding row keeps its body, both keep their places after
/// the summary, and every retained record of the reloaded graph validates.
#[tokio::test]
async fn a_compaction_folds_entries_and_retires_bodies_in_one_pass() {
    const POLICY_ONE: &str = "POLICY-ONE: never share credentials.";
    const POLICY_TWO: &str = "POLICY-TWO: never share keys.";
    let client = Arc::new(FoldClient::default());
    let compactor = Arc::new(ArmedCompactor::default());
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &compactor,
        InstructionLowering::MidConversation,
    )
    .await;
    agent
        .session_mut()
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();
    apply_and_run(&mut agent, "first").await;
    agent
        .session_mut()
        .activate_instruction(dia_activation(POLICY_ONE))
        .unwrap();
    apply_and_run(&mut agent, "second").await;
    let mut superseding = dia_activation(POLICY_TWO);
    superseding.revision.revision_id = InstructionRevisionId::new("rev-2").unwrap();
    superseding.activation_id = InstructionActivationId::new("act-policy-2").unwrap();
    superseding.expectation = meerkat_core::InstructionActivationExpectation::Effective(
        InstructionActivationId::new("act-policy").unwrap(),
    );
    superseding.supersedes = Some(InstructionActivationId::new("act-policy").unwrap());
    agent
        .session_mut()
        .activate_instruction(superseding)
        .unwrap();
    let activation_rows = |messages: &[Message]| {
        messages
            .iter()
            .filter_map(|message| match message {
                Message::System(system) if system.instruction_activation.is_some() => {
                    Some(system.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let before = activation_rows(agent.session().messages());
    assert_eq!(before.len(), 2);

    compactor.arm(1);
    apply_and_run(&mut agent, "third").await;
    assert_eq!(compactor.rebuilds(), 1);

    let messages = agent.session().messages().to_vec();
    let entries = entry_identities(&messages);
    assert_eq!(entries.len(), 1, "the entries fold into one snapshot");
    assert_eq!(entries[0].kind(), InstructionEntryKind::Snapshot);
    let after = activation_rows(&messages);
    assert_eq!(after.len(), 2, "both activation rows stay");
    assert!(
        after[0]
            .content
            .starts_with("[meerkat-retired-instruction-v1]")
            && !after[0].content.contains(POLICY_ONE),
        "the superseded row holds its retired header: {}",
        after[0].content
    );
    assert_eq!(
        after[0]
            .retired_instruction_body
            .map(|marker| marker.body_bytes),
        Some(POLICY_ONE.len() as u64)
    );
    assert_eq!(
        after[0].instruction_activation, before[0].instruction_activation,
        "the retired row keeps its identity"
    );
    assert_eq!(after[1], before[1], "the superseding row keeps its body");
    let summary_index = messages.iter().position(is_summary).unwrap();
    for row in &after {
        let index = messages
            .iter()
            .position(|message| matches!(message, Message::System(system) if system == row))
            .unwrap();
        assert!(
            index > summary_index,
            "each activation row keeps its place after the folded turns"
        );
    }
    assert!(
        !transcript_text(&client.requests().last().cloned().unwrap()).contains(POLICY_ONE),
        "the request carries no superseded body"
    );
    assert!(assert_reload_validates(agent.session()) >= 1);
}

/// A compaction inside a run, after its first response, folds what is in
/// effect: a change the run start left pending stays pending.
#[tokio::test]
async fn mid_run_fold_does_not_apply_a_pending_target() {
    let client = Arc::new(FoldClient::default());
    let compactor = Arc::new(ArmedCompactor::default());
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &compactor,
        InstructionLowering::LeadingPrefixOnly,
    )
    .await;
    agent
        .session_mut()
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();
    assert!(matches!(
        apply_and_run(&mut agent, "first").await,
        RunStartInstructionOutcome::Applied { .. }
    ));
    agent
        .session_mut()
        .set_session_instruction(activate("tone", 2, "Answer warmly."))
        .unwrap();
    let outcome = agent
        .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
        .unwrap();
    assert_eq!(
        outcome,
        RunStartInstructionOutcome::Deferred(InstructionDeferReason::LeadingPrefixOnly)
    );
    client.call_tool_once();
    // The run's first request answers with a tool call; the compaction runs
    // at its second request.
    compactor.arm(2);
    agent.run("second".into()).await.unwrap();
    assert_eq!(
        compactor.rebuilds(),
        1,
        "the run compacts at its second request"
    );

    let messages = agent.session().messages().to_vec();
    let entries = entry_identities(&messages);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].kind(), InstructionEntryKind::Snapshot);
    assert_eq!(
        active_slots(&entries[0]),
        vec![("tone".to_string(), 1)],
        "the snapshot holds what is in effect"
    );
    assert!(
        entries[0].run_id().is_none(),
        "a fold of the effective set applies nothing"
    );
    assert!(entry_row(&messages).content.contains("Answer formally."));
    for request in client.requests() {
        assert!(
            !transcript_text(&request).contains("Answer warmly."),
            "the pending change reaches no request of the run"
        );
    }
    let after_fold = client.requests().last().cloned().unwrap();
    assert_eq!(entry_identities(&after_fold).len(), 1);

    let state = agent.session().instruction_target_state().unwrap().unwrap();
    let tone = &state.slots()[0];
    assert_eq!(tone.revision.get(), 2);
    assert_eq!(
        tone.pending_body.as_deref(),
        Some("Answer warmly."),
        "the change is still pending"
    );
    let effective = effective_operator_instructions(agent.session().id(), &messages).unwrap();
    assert_eq!(
        effective
            .slot(&slot("tone"))
            .map(|state| state.revision().get()),
        Some(1)
    );
}

/// A compaction at a fresh run's first request folds the recorded target:
/// a leading-prefix-only model takes the pending change in the snapshot, in
/// the leading System prefix of that very request.
#[tokio::test]
async fn run_start_fold_applies_a_pending_target_on_a_leading_prefix_only_model() {
    let client = Arc::new(FoldClient::default());
    let compactor = Arc::new(ArmedCompactor::default());
    let mut agent = build_agent(
        conversed_session(),
        &client,
        &compactor,
        InstructionLowering::LeadingPrefixOnly,
    )
    .await;
    agent
        .session_mut()
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();
    compactor.arm(1);
    assert_eq!(
        apply_and_run(&mut agent, "next").await,
        RunStartInstructionOutcome::Deferred(InstructionDeferReason::LeadingPrefixOnly)
    );
    assert_eq!(compactor.rebuilds(), 1, "the run's first request compacts");

    let requests = client.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    let first_non_system = request
        .iter()
        .position(|message| !matches!(message, Message::System(_)))
        .unwrap();
    let index = entry_index(request).expect("the request carries the snapshot");
    assert!(
        index < first_non_system,
        "the snapshot is inside the leading System prefix: {request:?}"
    );
    assert!(is_summary(&request[first_non_system]));

    let messages = agent.session().messages().to_vec();
    let entries = entry_identities(&messages);
    assert_eq!(entries.len(), 1);
    let snapshot = &entries[0];
    assert_eq!(snapshot.kind(), InstructionEntryKind::Snapshot);
    assert_eq!(snapshot.entry_sequence(), 1);
    assert_eq!(active_slots(snapshot), vec![("tone".to_string(), 1)]);
    assert!(entry_row(&messages).content.contains("Answer formally."));
    let run_id = messages.iter().rev().find_map(|message| match message {
        Message::BlockAssistant(assistant) => assistant.identity.run_id.clone(),
        _ => None,
    });
    assert!(run_id.is_some());
    assert_eq!(
        snapshot.run_id(),
        run_id.as_ref(),
        "the snapshot names the run at whose start it applied"
    );

    let state = agent.session().instruction_target_state().unwrap().unwrap();
    assert!(
        state.slots()[0].pending_body.is_none(),
        "the applied body lives in the snapshot"
    );
    assert_eq!(state.entry_sequence(), 1);
    let effective = effective_operator_instructions(agent.session().id(), &messages).unwrap();
    assert_eq!(
        effective
            .slot(&slot("tone"))
            .map(meerkat_core::EffectiveInstructionSlot::effect),
        Some(InstructionEffect::Active)
    );
    assert_eq!(
        agent
            .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
            .unwrap(),
        RunStartInstructionOutcome::NotNeeded,
        "nothing is pending after the fold"
    );
}

fn envelope_version(session: &Session) -> u64 {
    serde_json::from_slice::<serde_json::Value>(&session.to_persisted_bytes().unwrap())
        .unwrap()["version"]
        .as_u64()
        .unwrap()
}

/// A fold whose set is empty (every slot revoked) leaves no entry row; the
/// target keeps the entry-sequence high-water, so the document stays at
/// envelope 5 and the next entry continues the sequence.
#[tokio::test]
async fn empty_active_set_folds_to_no_row() {
    let client = Arc::new(FoldClient::default());
    let compactor = Arc::new(ArmedCompactor::default());
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &compactor,
        InstructionLowering::MidConversation,
    )
    .await;
    agent
        .session_mut()
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();
    apply_and_run(&mut agent, "first").await;
    agent
        .session_mut()
        .set_session_instruction(revoke("tone", 2))
        .unwrap();
    assert!(matches!(
        apply_and_run(&mut agent, "second").await,
        RunStartInstructionOutcome::Applied {
            entry_sequence: 2,
            ..
        }
    ));

    compactor.arm(1);
    apply_and_run(&mut agent, "third").await;
    assert_eq!(compactor.rebuilds(), 1);

    let messages = agent.session().messages().to_vec();
    assert!(
        entry_identities(&messages).is_empty(),
        "an empty set folds to no row"
    );
    assert!(entry_identities(client.requests().last().unwrap()).is_empty());
    let state = agent.session().instruction_target_state().unwrap().unwrap();
    assert_eq!(state.entry_sequence(), 3, "the fold took sequence 3");
    assert_eq!(envelope_version(agent.session()), 5);

    agent
        .session_mut()
        .set_session_instruction(activate("tone", 3, "Answer briefly."))
        .unwrap();
    assert!(matches!(
        apply_and_run(&mut agent, "fourth").await,
        RunStartInstructionOutcome::Applied {
            entry_sequence: 4,
            ..
        }
    ));
}

/// A fork child's copied entry rows are history, not instructions: its
/// compaction takes them out like any entry row, and records the
/// entry-sequence high-water although the child has no target of its own,
/// so the document stays at envelope 5 while its rewrite history holds them.
#[tokio::test]
async fn a_fork_child_fold_takes_out_inherited_rows_and_keeps_envelope_5() {
    let client = Arc::new(FoldClient::default());
    let compactor = Arc::new(ArmedCompactor::default());
    let mut parent = build_agent(
        fresh_session(),
        &client,
        &compactor,
        InstructionLowering::MidConversation,
    )
    .await;
    parent
        .session_mut()
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();
    apply_and_run(&mut parent, "first").await;
    apply_and_run(&mut parent, "second").await;

    let mut child_session = parent.session().fork();
    child_session
        .set_build_state(meerkat_core::SessionBuildState::default())
        .expect("test session build state should serialize");
    assert_eq!(entry_identities(child_session.messages()).len(), 1);
    assert_eq!(
        child_session
            .instruction_target_state()
            .unwrap()
            .expect("a fork that copies entry rows records the instruction state")
            .owner(),
        &meerkat_core::InstructionTargetOwner::Unowned,
        "the child has no target of its own"
    );
    let child_client = Arc::new(FoldClient::default());
    let child_compactor = Arc::new(ArmedCompactor::default());
    let mut child = build_agent(
        child_session,
        &child_client,
        &child_compactor,
        InstructionLowering::MidConversation,
    )
    .await;
    child_compactor.arm(1);
    assert_eq!(
        apply_and_run(&mut child, "child").await,
        RunStartInstructionOutcome::NotNeeded
    );
    assert_eq!(child_compactor.rebuilds(), 1);

    let messages = child.session().messages().to_vec();
    assert!(
        entry_identities(&messages).is_empty(),
        "the inherited rows leave the live transcript"
    );
    assert!(entry_identities(child_client.requests().last().unwrap()).is_empty());
    let state = child
        .session()
        .instruction_target_state()
        .unwrap()
        .expect("the fold records the high-water");
    assert!(
        state.slots().is_empty(),
        "the child has no target of its own"
    );
    assert_eq!(
        state.entry_sequence(),
        1,
        "the child's first fold takes the first sequence of its own origin"
    );
    assert_eq!(envelope_version(child.session()), 5);
}

/// The body of every instruction the summarizer rows below write: the one
/// in effect, a superseded one and a revoked one.
const INSTRUCTION_BODIES: [&str; 3] =
    ["Answer formally.", "Use short sentences.", "Answer warmly."];

/// Two slots apply at one run start; the next run start supersedes one and
/// revokes the other. The first delta then holds a superseded body and a
/// revoked body, and the second the body in effect.
async fn apply_superseded_and_revoked_bodies(agent: &mut TestAgent) {
    for write in [
        activate("tone", 1, "Answer formally."),
        activate("style", 1, "Use short sentences."),
    ] {
        agent.session_mut().set_session_instruction(write).unwrap();
    }
    assert!(matches!(
        apply_and_run(agent, "first").await,
        RunStartInstructionOutcome::Applied {
            entry_sequence: 1,
            ..
        }
    ));
    for write in [activate("tone", 2, "Answer warmly."), revoke("style", 2)] {
        agent.session_mut().set_session_instruction(write).unwrap();
    }
    assert!(matches!(
        apply_and_run(agent, "second").await,
        RunStartInstructionOutcome::Applied {
            entry_sequence: 2,
            ..
        }
    ));
}

fn assert_carries_no_instruction_body(what: &str, messages: &[Message]) {
    let text = transcript_text(messages);
    for body in INSTRUCTION_BODIES {
        assert!(
            !text.contains(body),
            "{what} carries the instruction body {body:?}: {text}"
        );
    }
    assert!(
        entry_identities(messages).is_empty(),
        "{what} carries an operator-instruction entry row"
    );
}

fn is_user_request(message: &Message, text: &str) -> bool {
    matches!(message, Message::User(user) if user.text_content() == text)
}

/// The session reloads from its persisted bytes, which validates its
/// rewrite graph, and every rewrite the reloaded graph retains validates as
/// a record with both of its bodies. Returns how many records validated.
fn assert_reload_validates(session: &Session) -> usize {
    let reloaded = Session::from_persisted_bytes(&session.to_persisted_bytes().unwrap())
        .expect("the reloaded rewrite graph validates");
    assert_eq!(reloaded.messages(), session.messages());
    let history = reloaded
        .validated_transcript_history_state()
        .unwrap()
        .expect("the session holds a rewrite graph");
    let retained = history
        .commits()
        .skip(history.retired_count())
        .cloned()
        .collect::<Vec<_>>();
    for commit in &retained {
        let parent = history.materialize_rewrite_parent(commit).unwrap();
        let child = history.materialize_rewrite_child(commit).unwrap();
        meerkat_core::TranscriptRewriteRecord::new(commit.clone(), parent, child)
            .expect("the rewrite record validates with both of its bodies");
    }
    retained.len()
}

/// No compaction summary request carries an operator-instruction body: not
/// the body in effect, not a superseded one, not a revoked one. A summary
/// is a user row, so a body it paraphrased would outlive the entry that
/// revoked it. The first compaction runs inside a run and folds the rows
/// before the run, its summary written from them; the second runs at a
/// run's first request, with the first fold's snapshot in the transcript it
/// summarizes. The instruction in effect stays in the transcript, in the
/// snapshot.
#[tokio::test]
async fn no_summary_request_carries_an_instruction_body() {
    let client = Arc::new(FoldClient::default());
    let compactor = Arc::new(ArmedCompactor::default());
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &compactor,
        InstructionLowering::MidConversation,
    )
    .await;
    apply_superseded_and_revoked_bodies(&mut agent).await;

    // The third run's first request answers with a tool call; the
    // compaction at its second request folds the rows before the run.
    client.call_tool_once();
    compactor.arm(2);
    apply_and_run(&mut agent, "third").await;
    assert_eq!(compactor.rebuilds(), 1);
    assert_eq!(
        entry_identities(agent.session().messages()).len(),
        1,
        "the first fold leaves its snapshot"
    );
    // The fourth run compacts at its first request.
    compactor.arm(1);
    apply_and_run(&mut agent, "fourth").await;
    assert_eq!(compactor.rebuilds(), 2);

    let summaries = client.summaries();
    assert_eq!(summaries.len(), 2, "each compaction asks for one summary");
    assert!(
        transcript_text(&summaries[0]).contains("second"),
        "the first summary is written from the rows before the run: {:?}",
        summaries[0]
    );
    assert!(
        transcript_text(&summaries[1]).contains("fourth"),
        "the second summary is written from the whole transcript: {:?}",
        summaries[1]
    );
    for (index, summary) in summaries.iter().enumerate() {
        assert_carries_no_instruction_body(&format!("summary request {index}"), summary);
    }

    let messages = agent.session().messages().to_vec();
    let entries = entry_identities(&messages);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].kind(), InstructionEntryKind::Snapshot);
    assert!(
        entry_row(&messages).content.contains("Answer warmly."),
        "the instruction in effect stays in the snapshot"
    );
}

/// A host curator writes the summary from a window that holds no
/// operator-instruction row: no body in effect, superseded or revoked
/// reaches it.
#[tokio::test]
async fn no_curator_window_carries_an_instruction_body() {
    let client = Arc::new(FoldClient::default());
    let compactor = Arc::new(ArmedCompactor::default());
    let curator = Arc::new(RecordingCurator::default());
    let mut agent = build_curated_agent(
        fresh_session(),
        &client,
        &compactor,
        &curator,
        InstructionLowering::MidConversation,
    )
    .await;
    apply_superseded_and_revoked_bodies(&mut agent).await;

    compactor.arm(1);
    apply_and_run(&mut agent, "third").await;
    assert_eq!(compactor.rebuilds(), 1);
    assert!(
        client.summaries().is_empty(),
        "the curator writes the summary"
    );

    let windows = curator.windows();
    assert_eq!(windows.len(), 1);
    assert!(
        transcript_text(&windows[0]).contains("second"),
        "the window holds the history: {:?}",
        windows[0]
    );
    assert_carries_no_instruction_body("the curator window", &windows[0]);
}

/// The fold's snapshot is neutral in the shrink rule, which counts the rows
/// other than operator-instruction entries. On a leading-prefix-only model
/// the first run-start fold inserts a snapshot and takes no entry row out,
/// so a compactor that takes out exactly two rows (its summary takes one
/// back) leaves as many rows as there were. Its own rebuild still shrank
/// the transcript, so the rewrite commits and the reloaded graph validates.
#[tokio::test]
async fn the_first_run_start_fold_commits_when_its_compaction_removes_two_rows() {
    let client = Arc::new(FoldClient::default());
    let compactor = Arc::new(ArmedCompactor::default());
    let mut session = fresh_session();
    exchange(&mut session, "hello", "hi");
    let mut agent = build_agent(
        session,
        &client,
        &compactor,
        InstructionLowering::LeadingPrefixOnly,
    )
    .await;
    agent
        .session_mut()
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();
    compactor.arm(1);
    assert_eq!(
        apply_and_run(&mut agent, "next").await,
        RunStartInstructionOutcome::Deferred(InstructionDeferReason::LeadingPrefixOnly)
    );
    assert_eq!(compactor.rebuilds(), 1, "the run's first request compacts");
    assert_eq!(
        compactor.last_discarded(),
        2,
        "the compactor takes out the earlier exchange"
    );

    let requests = client.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(
        entry_index(request),
        Some(1),
        "the rewrite committed: the snapshot is in the request's leading System prefix: {request:?}"
    );
    assert!(is_summary(&request[2]), "the summary follows the snapshot");
    assert!(is_user_request(&request[3], "next"));
    assert_eq!(
        request.len(),
        4,
        "the prompt, the snapshot, the summary and the run's request: as many rows as before"
    );
    assert!(entry_row(request).content.contains("Answer formally."));

    let state = agent.session().instruction_target_state().unwrap().unwrap();
    assert!(
        state.slots()[0].pending_body.is_none(),
        "the fold applied the target"
    );
    assert_eq!(state.entry_sequence(), 1);
    assert_eq!(
        assert_reload_validates(agent.session()),
        1,
        "the compaction's record validates"
    );
}

/// A run that answers a pending tail starts with that tail, and the entry
/// its run start applied follows the tail. An entry row belongs to the
/// run's request rows, so a compaction inside the run (after its first
/// response) folds only the rows before the tail: the request the run
/// answers is kept verbatim, and the summary is not written from it.
#[tokio::test]
async fn a_mid_run_compaction_keeps_the_pending_tail_request_an_entry_follows() {
    let client = Arc::new(FoldClient::default());
    let compactor = Arc::new(ArmedCompactor::default());
    let mut session = conversed_session();
    session.push(Message::User(UserMessage::text("pending question")));
    let mut agent = build_agent(
        session,
        &client,
        &compactor,
        InstructionLowering::MidConversation,
    )
    .await;
    agent
        .session_mut()
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();
    assert!(matches!(
        agent
            .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
            .unwrap(),
        RunStartInstructionOutcome::Applied { .. }
    ));
    let rows = agent.session().messages().to_vec();
    assert!(is_user_request(&rows[rows.len() - 2], "pending question"));
    assert_eq!(
        entry_index(&rows),
        Some(rows.len() - 1),
        "the entry follows the pending tail"
    );

    // The run's first request answers with a tool call; the compaction
    // runs at its second request.
    client.call_tool_once();
    compactor.arm(2);
    agent.set_runtime_execution_kind(Some(
        meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
    ));
    agent
        .run_pending()
        .await
        .expect("the pending tail runs with the entry after it");
    assert_eq!(
        compactor.rebuilds(),
        1,
        "the run compacts at its second request"
    );

    let after_fold = client.requests().last().cloned().unwrap();
    let summary_index = after_fold
        .iter()
        .position(is_summary)
        .expect("the request after the compaction carries its summary");
    let tail_index = after_fold
        .iter()
        .position(|message| is_user_request(message, "pending question"))
        .unwrap_or_else(|| panic!("the request the run answers is kept verbatim: {after_fold:?}"));
    assert!(summary_index < tail_index);
    assert!(
        agent
            .session()
            .messages()
            .iter()
            .any(|message| is_user_request(message, "pending question")),
        "the transcript keeps the run's request"
    );
    let summaries = client.summaries();
    assert_eq!(summaries.len(), 1);
    assert!(
        !transcript_text(&summaries[0]).contains("pending question"),
        "the summary is not written from the request the run answers"
    );
}

/// A run-start fold applies the recorded target, and an empty folded set
/// leaves no entry row. A leading-prefix-only model takes a pending revoke
/// of its only slot this way, although the entry row the fold takes out
/// still had that slot in effect: the rewrite's record validates, so a
/// compaction that leaves no snapshot cannot be refused merely because the
/// rows it takes out had an instruction in effect.
#[tokio::test]
async fn a_run_start_fold_of_a_fully_revoked_target_leaves_no_entry_row() {
    let client = Arc::new(FoldClient::default());
    let compactor = Arc::new(ArmedCompactor::default());
    let mut agent = build_agent(
        fresh_session(),
        &client,
        &compactor,
        InstructionLowering::LeadingPrefixOnly,
    )
    .await;
    agent
        .session_mut()
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();
    assert!(matches!(
        apply_and_run(&mut agent, "first").await,
        RunStartInstructionOutcome::Applied { .. }
    ));
    agent
        .session_mut()
        .set_session_instruction(revoke("tone", 2))
        .unwrap();
    compactor.arm(1);
    assert_eq!(
        apply_and_run(&mut agent, "second").await,
        RunStartInstructionOutcome::Deferred(InstructionDeferReason::LeadingPrefixOnly)
    );
    assert_eq!(compactor.rebuilds(), 1, "the run's first request compacts");

    let messages = agent.session().messages().to_vec();
    assert!(
        entry_identities(&messages).is_empty(),
        "an empty folded set leaves no row"
    );
    assert!(!transcript_text(&messages).contains("Answer formally."));
    assert!(entry_identities(client.requests().last().unwrap()).is_empty());
    let state = agent.session().instruction_target_state().unwrap().unwrap();
    assert_eq!(state.entry_sequence(), 2, "the fold took sequence 2");
    assert_eq!(
        agent
            .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
            .unwrap(),
        RunStartInstructionOutcome::NotNeeded,
        "the fold applied the revoke"
    );
    assert_eq!(
        assert_reload_validates(agent.session()),
        1,
        "the compaction's record validates"
    );
}

/// The lowering a build resolves for the test catalog's OpenAI row served
/// through a binding whose backend is the ChatGPT backend.
fn chatgpt_backend_lowering() -> InstructionLowering {
    let mut realm =
        meerkat_core::RealmConfigSection::from_inline_api_keys(&[("openai", "test-key")]);
    realm
        .backend
        .get_mut("default_openai")
        .expect("OpenAI backend")
        .backend_kind = "chatgpt_backend".to_string();
    realm
        .auth
        .get_mut("default_openai")
        .expect("OpenAI auth")
        .auth_method = "managed_chatgpt_oauth".to_string();
    let mut config = meerkat_core::Config::default();
    config.realm.insert("chatgpt_realm".to_string(), realm);
    let registry = meerkat_core::ModelRegistry::from_config(
        &config,
        *crate::model_profile::test_catalog::TEST_CATALOG,
    )
    .unwrap();
    registry.instruction_lowering_for_identity(
        &config,
        &meerkat_core::SessionLlmIdentity {
            model: crate::model_profile::test_catalog::OPENAI_MODEL.to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(meerkat_core::AuthBindingRef {
                realm: meerkat_core::RealmId::parse("chatgpt_realm").unwrap(),
                binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
                profile: None,
                origin: meerkat_core::BindingOrigin::Configured,
            }),
        },
    )
}

/// What the ChatGPT backend's Responses wire carries of the System plane:
/// one top-level instructions string, so at most one System row, at the
/// head of the request. The OpenAI client refuses any other request there.
fn carries_at_most_one_leading_system_row(request: &[Message]) -> bool {
    request
        .iter()
        .skip(1)
        .all(|message| !matches!(message, Message::System(_)))
}

/// A session served by the ChatGPT backend: its wire carries exactly one
/// leading System row, the session's prompt, so no entry can be lowered
/// there. A pending target stays pending with `lowering_unavailable` at a
/// fresh session's first run (its transcript is still the prompt) and at a
/// later run whose first request compacts (the fold takes what is in
/// effect, nothing): no entry row and no snapshot is written, and every
/// request keeps the one leading System row that wire carries.
#[tokio::test]
async fn chatgpt_backend_session_defers_every_entry_with_lowering_unavailable() {
    let lowering = chatgpt_backend_lowering();
    assert_eq!(lowering, InstructionLowering::Unavailable);
    let client = Arc::new(FoldClient::default());
    let compactor = Arc::new(ArmedCompactor::default());
    let mut agent = build_agent(fresh_session(), &client, &compactor, lowering).await;
    agent
        .session_mut()
        .set_session_instruction(activate("tone", 1, "Answer formally."))
        .unwrap();

    assert_eq!(
        apply_and_run(&mut agent, "first").await,
        RunStartInstructionOutcome::Deferred(InstructionDeferReason::LoweringUnavailable),
        "the first run of a fresh session"
    );
    compactor.arm(1);
    assert_eq!(
        apply_and_run(&mut agent, "second").await,
        RunStartInstructionOutcome::Deferred(InstructionDeferReason::LoweringUnavailable),
        "a run whose first request compacts"
    );
    assert_eq!(
        compactor.rebuilds(),
        1,
        "the second run's first request compacts"
    );

    let requests = client.requests();
    assert_eq!(requests.len(), 2, "every request reached the model");
    for request in &requests {
        assert!(
            entry_identities(request).is_empty(),
            "no entry reaches a request: {request:?}"
        );
        assert!(
            carries_at_most_one_leading_system_row(request),
            "the request keeps the one leading System row the wire carries: {request:?}"
        );
        assert!(!transcript_text(request).contains("Answer formally."));
    }
    assert!(
        requests[1].iter().any(is_summary),
        "the second request is the compacted one"
    );
    let messages = agent.session().messages().to_vec();
    assert!(
        entry_identities(&messages).is_empty(),
        "no entry row and no snapshot is written"
    );
    let state = agent.session().instruction_target_state().unwrap().unwrap();
    assert_eq!(
        state.slots()[0].pending_body.as_deref(),
        Some("Answer formally."),
        "the target stays pending"
    );
    let effective = effective_operator_instructions(agent.session().id(), &messages).unwrap();
    assert!(
        effective.slot(&slot("tone")).is_none(),
        "nothing is in effect"
    );
}
