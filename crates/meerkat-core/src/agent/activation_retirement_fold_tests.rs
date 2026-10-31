//! Rows for retiring superseded instruction activation bodies at compaction,
//! through the real agent loop. A compaction keeps every activation row in
//! place with its typed identity and timestamp; the body of a row that a
//! later row of its key follows is retired: the row's content becomes a
//! fixed header that names its key, revision and body length, and the row
//! carries a typed marker of the retired body. The newest row of every key
//! keeps its body. No summarizer, curator or compactor reads a superseded
//! body, and no compactor, generic rewrite or restore retires or revives
//! one. A document that holds a retired body is written at envelope 5.
//!
//! The rows assert bytes and serialized JSON, so they read the marker as
//! the `retired_instruction_body` field of a System row's JSON. The
//! compactor here is a host compactor that knows nothing about activation
//! rows: it keeps every System row, as every compactor must.
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
    InstructionActivationId, InstructionActivationIdentity, InstructionContentDigest,
    InstructionKey, InstructionNamespace, InstructionRevisionId, InstructionRevisionRef, SessionId,
    SystemMessage, UserMessage,
};
use meerkat_core::{
    AgentBuilder, AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    AssistantBlock, ForkInstructionPolicy, InstructionActivationError,
    InstructionActivationExpectation, InstructionActivationReadQuery, InstructionActivationRequest,
    LlmStreamResult, Message, Session, StopReason, ToolCallView, ToolDef, ToolResult,
    TranscriptEditError, TranscriptRewriteReason, TranscriptRewriteSelection, TurnUsage, Usage,
};
use serde_json::json;

const NAMESPACE: &str = "app.skills";
const COMPACTION_PROMPT: &str = "COMPACT NOW";
const CALENDAR_ONE: &str = "CALENDAR-ONE: book meetings in the morning only.";
const CALENDAR_TWO: &str = "CALENDAR-TWO: book meetings in the afternoon only.";
const CALENDAR_THREE: &str = "CALENDAR-THREE: book meetings at any hour.";
const BILLING_ONE: &str = "BILLING-ONE: send invoices on the first of the month.";
const RETIRED_HEADER: &str = "[meerkat-retired-instruction-v1]";

#[allow(dead_code)]
mod released_v0_9_8_version_authority {
    include!("../../tests/fixtures/v0_9_8_session_persistence_version_authority.rs.txt");
}

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

/// Answers a compaction summary request with a short summary and any other
/// request with text, and keeps the messages of each, apart.
#[derive(Default)]
struct RecordingClient {
    requests: Mutex<Vec<Vec<Message>>>,
    summaries: Mutex<Vec<Vec<Message>>>,
}

impl RecordingClient {
    fn requests(&self) -> Vec<Vec<Message>> {
        self.requests.lock().unwrap().clone()
    }

    fn summaries(&self) -> Vec<Vec<Message>> {
        self.summaries.lock().unwrap().clone()
    }
}

#[async_trait]
impl AgentLlmClient for RecordingClient {
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
/// it folds. [`Self::forge`] makes its rebuild put another row in place of
/// one source row it keeps, mapping it as retained.
#[derive(Default)]
struct ArmedCompactor {
    countdown: AtomicUsize,
    rebuilds: AtomicUsize,
    forged: Mutex<Option<(usize, Message)>>,
}

impl ArmedCompactor {
    fn arm(&self, boundary: usize) {
        self.countdown.store(boundary, Ordering::SeqCst);
    }

    fn forge(&self, source_offset: usize, row: Message) {
        *self.forged.lock().unwrap() = Some((source_offset, row));
    }

    fn rebuilds(&self) -> usize {
        self.rebuilds.load(Ordering::SeqCst)
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
        let forged = self.forged.lock().unwrap().clone();
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
                match &forged {
                    Some((forged_offset, row)) if *forged_offset == offset => {
                        rebuilt.push(row.clone());
                    }
                    _ => rebuilt.push(message.clone()),
                }
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

type TestAgent = meerkat_core::Agent<RecordingClient, ProbeTool, NoopStore>;

async fn build_agent(
    session: Session,
    client: &Arc<RecordingClient>,
    compactor: &Arc<ArmedCompactor>,
    curator: Option<&Arc<RecordingCurator>>,
) -> TestAgent {
    let compactor: Arc<dyn Compactor> = compactor.clone();
    let builder = AgentBuilder::new()
        .resume_session(session)
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .compactor(compactor);
    let builder = match curator {
        Some(curator) => {
            let curator: Arc<dyn CompactionCurator> = curator.clone();
            builder.compaction_curator(curator)
        }
        None => builder,
    };
    builder
        .build_standalone(
            Arc::clone(client),
            Arc::new(ProbeTool::new()),
            Arc::new(NoopStore),
        )
        .await
}

/// Run one turn of `session` whose first request compacts.
async fn compacted_with(
    session: Session,
    curator: Option<&Arc<RecordingCurator>>,
) -> (TestAgent, Arc<RecordingClient>) {
    let client = Arc::new(RecordingClient::default());
    let compactor = Arc::new(ArmedCompactor::default());
    let mut agent = build_agent(session, &client, &compactor, curator).await;
    compactor.arm(1);
    agent.run("the next question".into()).await.unwrap();
    assert_eq!(compactor.rebuilds(), 1, "the run's first request compacts");
    (agent, client)
}

async fn compacted(session: Session) -> (TestAgent, Arc<RecordingClient>) {
    compacted_with(session, None).await
}

fn request(
    key: &str,
    activation: &str,
    supersedes: Option<&str>,
    body: &str,
) -> InstructionActivationRequest {
    InstructionActivationRequest {
        revision: InstructionRevisionRef {
            namespace: InstructionNamespace::new(NAMESPACE).unwrap(),
            key: InstructionKey::new(key).unwrap(),
            revision_id: InstructionRevisionId::new(format!("rev-{activation}")).unwrap(),
            content_sha256: InstructionContentDigest::for_body(body),
        },
        activation_id: InstructionActivationId::new(activation).unwrap(),
        expectation: supersedes.map_or(InstructionActivationExpectation::Absent, |previous| {
            InstructionActivationExpectation::Effective(
                InstructionActivationId::new(previous).unwrap(),
            )
        }),
        supersedes: supersedes.map(|previous| InstructionActivationId::new(previous).unwrap()),
        body: body.to_string(),
    }
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

/// A session with its prompt only.
fn prompted_session() -> Session {
    let mut session = Session::new();
    session
        .set_build_state(meerkat_core::SessionBuildState::default())
        .expect("test session build state should serialize");
    session.append_system_message("You are a careful assistant.");
    session
}

/// `[prompt, A1(calendar), turn, A2(calendar), turn, A3(billing), turn]`:
/// A2 supersedes A1.
fn session_with_a_superseded_activation() -> Session {
    let mut session = prompted_session();
    session
        .activate_instruction(request("calendar", "act-1", None, CALENDAR_ONE))
        .unwrap();
    exchange(&mut session, "first question", "first answer");
    session
        .activate_instruction(request("calendar", "act-2", Some("act-1"), CALENDAR_TWO))
        .unwrap();
    exchange(&mut session, "second question", "second answer");
    session
        .activate_instruction(request("billing", "act-3", None, BILLING_ONE))
        .unwrap();
    exchange(&mut session, "third question", "third answer");
    session
}

fn activation_rows(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .filter(|message| {
            matches!(message, Message::System(system) if system.instruction_activation.is_some())
        })
        .cloned()
        .collect()
}

fn system_of(message: &Message) -> &SystemMessage {
    let Message::System(system) = message else {
        panic!("an activation row is a System row: {message:?}");
    };
    system
}

fn identity_of(message: &Message) -> InstructionActivationIdentity {
    system_of(message)
        .instruction_activation
        .clone()
        .expect("an activation row")
}

fn json_string(value: &str) -> String {
    serde_json::Value::String(value.to_owned()).to_string()
}

/// The retired form's content: the v1 header lines of the identity,
/// `status=superseded` and the body length, and no body.
fn retired_header(identity: &InstructionActivationIdentity, body_bytes: usize) -> String {
    let supersedes = identity
        .supersedes
        .as_ref()
        .map_or_else(|| "null".to_string(), |value| json_string(value.as_str()));
    format!(
        "{RETIRED_HEADER}\nnamespace={}\nkey={}\nrevision={}\nactivation={}\nsupersedes={}\norigin_session_id={}\ncontent_sha256={}\nstatus=superseded\nbody_bytes={}",
        json_string(identity.revision.namespace.as_str()),
        json_string(identity.revision.key.as_str()),
        json_string(identity.revision.revision_id.as_str()),
        json_string(identity.activation_id.as_str()),
        supersedes,
        json_string(&identity.origin_session_id.to_string()),
        identity.revision.content_sha256,
        body_bytes,
    )
}

/// The typed marker of a retired body, as a System row's JSON carries it.
fn marker(body_bytes: usize) -> serde_json::Value {
    json!({ "render_version": 1, "body_bytes": body_bytes })
}

/// The retired-body marker of a row's JSON; `Null` for a row without one.
fn marker_of(message: &Message) -> serde_json::Value {
    serde_json::to_value(message).unwrap()["retired_instruction_body"].clone()
}

/// `row` with its body retired by hand, through its JSON form.
fn retired_by_hand(row: &Message, body_bytes: usize) -> Message {
    let mut value = serde_json::to_value(row).unwrap();
    value["content"] = json!(retired_header(&identity_of(row), body_bytes));
    value["retired_instruction_body"] = marker(body_bytes);
    serde_json::from_value(value).unwrap()
}

fn text_of(messages: &[Message]) -> String {
    serde_json::to_string(messages).unwrap()
}

fn envelope_version(session: &Session) -> u64 {
    serde_json::from_slice::<serde_json::Value>(&session.to_persisted_bytes().unwrap())
        .unwrap()["version"]
        .as_u64()
        .unwrap()
}

fn key_query(key: &str) -> InstructionActivationReadQuery {
    InstructionActivationReadQuery {
        namespace: Some(InstructionNamespace::new(NAMESPACE).unwrap()),
        key: Some(InstructionKey::new(key).unwrap()),
        ..InstructionActivationReadQuery::default()
    }
}

/// The number of rows before `index` that are not System rows.
fn conversation_rows_before(messages: &[Message], index: usize) -> usize {
    messages[..index]
        .iter()
        .filter(|message| !matches!(message, Message::System(_)))
        .count()
}

fn index_of(messages: &[Message], activation: &str) -> usize {
    messages
        .iter()
        .position(|message| {
            matches!(
                message,
                Message::System(system)
                    if system
                        .instruction_activation
                        .as_ref()
                        .is_some_and(|identity| identity.activation_id.as_str() == activation)
            )
        })
        .unwrap_or_else(|| panic!("no activation row {activation}"))
}

/// A compaction after a superseded activation retires the superseded body in
/// place: the row's content is exactly the retired header and its marker
/// names the body's length. The superseding row and the other key's row keep
/// their bodies, byte for byte.
#[tokio::test]
async fn compaction_retires_superseded_activation_bodies() {
    let session = session_with_a_superseded_activation();
    let before = activation_rows(session.messages());
    let (agent, _) = compacted(session).await;
    let after = activation_rows(agent.session().messages());
    assert_eq!(after.len(), 3, "the compaction keeps every activation row");
    assert_eq!(
        system_of(&after[0]).content,
        retired_header(&identity_of(&before[0]), CALENDAR_ONE.len()),
        "the superseded row holds the retired header in place of its body"
    );
    assert_eq!(
        marker_of(&after[0]),
        marker(CALENDAR_ONE.len()),
        "the superseded row carries the retired-body marker"
    );
    assert_eq!(
        after[1], before[1],
        "the newest calendar row keeps its body"
    );
    assert_eq!(after[2], before[2], "the billing row keeps its body");
}

/// With two keys and three revisions of one, the newest row of each key is
/// byte-identical after the compaction, and the model request carries both
/// of their bodies.
#[tokio::test]
async fn each_keys_newest_activation_keeps_its_body() {
    let mut session = prompted_session();
    session
        .activate_instruction(request("calendar", "act-1", None, CALENDAR_ONE))
        .unwrap();
    exchange(&mut session, "first question", "first answer");
    session
        .activate_instruction(request("calendar", "act-2", Some("act-1"), CALENDAR_TWO))
        .unwrap();
    exchange(&mut session, "second question", "second answer");
    session
        .activate_instruction(request("billing", "act-3", None, BILLING_ONE))
        .unwrap();
    exchange(&mut session, "third question", "third answer");
    session
        .activate_instruction(request("calendar", "act-4", Some("act-2"), CALENDAR_THREE))
        .unwrap();
    exchange(&mut session, "fourth question", "fourth answer");
    let before = session.messages().to_vec();
    let newest = [
        before[index_of(&before, "act-3")].clone(),
        before[index_of(&before, "act-4")].clone(),
    ];

    let (agent, client) = compacted(session).await;
    let after = agent.session().messages().to_vec();
    assert_eq!(after[index_of(&after, "act-3")], newest[0]);
    assert_eq!(after[index_of(&after, "act-4")], newest[1]);
    let request = text_of(client.requests().last().unwrap());
    assert!(
        request.contains(BILLING_ONE) && request.contains(CALENDAR_THREE),
        "the model request carries each key's newest body"
    );
}

/// A retired row keeps its identity, its timestamp, its activation ordinal
/// and its place among the conversation rows; the record's projection
/// witness changes with its bytes.
#[tokio::test]
async fn a_retired_row_keeps_its_identity_ordinal_and_place() {
    let session = session_with_a_superseded_activation();
    let before_record = session
        .instruction_activation_records(key_query("calendar"))
        .unwrap()
        .records[0]
        .clone();
    let before_index = index_of(session.messages(), "act-1");
    let before_row = session.messages()[before_index].clone();
    let before_place = conversation_rows_before(session.messages(), before_index);

    let (agent, _) = compacted(session).await;
    let messages = agent.session().messages();
    let after_index = index_of(messages, "act-1");
    let after_row = &messages[after_index];
    assert_eq!(
        marker_of(after_row),
        marker(CALENDAR_ONE.len()),
        "the row's body is retired"
    );
    assert_eq!(identity_of(after_row), identity_of(&before_row));
    assert_eq!(
        system_of(after_row).created_at,
        system_of(&before_row).created_at
    );
    assert_eq!(
        conversation_rows_before(messages, after_index),
        before_place,
        "the row keeps its place among the conversation rows"
    );
    let after_record = agent
        .session()
        .instruction_activation_records(key_query("calendar"))
        .unwrap()
        .records[0]
        .clone();
    assert_eq!(after_record.identity, before_record.identity);
    assert_eq!(
        after_record.activation_ordinal,
        before_record.activation_ordinal
    );
    assert_ne!(
        after_record.projection_witness, before_record.projection_witness,
        "the retired bytes change the projection witness"
    );
}

/// The model reads a retired row as its header: the request after the
/// compaction carries the header and the superseding body, never the
/// superseded body.
#[tokio::test]
async fn the_model_reads_a_retired_row_as_its_header() {
    let session = session_with_a_superseded_activation();
    let identity = identity_of(&activation_rows(session.messages())[0]);
    let (agent, client) = compacted(session).await;
    let projected = agent.session().messages_for_model_boundary().unwrap();
    assert_eq!(
        system_of(&activation_rows(&projected)[0]).content,
        retired_header(&identity, CALENDAR_ONE.len()),
        "the model boundary projects the stored retired header"
    );
    let request = text_of(client.requests().last().unwrap());
    assert!(
        !request.contains(CALENDAR_ONE),
        "the request carries no superseded body: {request}"
    );
    assert!(request.contains(RETIRED_HEADER));
    assert!(request.contains(CALENDAR_TWO) && request.contains(BILLING_ONE));
}

/// An exact retry of a retired activation answers recorded but not
/// effective, as a live superseded row does; another body under its id is
/// an identity conflict.
#[tokio::test]
async fn exact_retry_of_a_retired_activation_is_recorded_but_not_effective() {
    let (mut agent, _) = compacted(session_with_a_superseded_activation()).await;
    let session = agent.session_mut();
    assert_eq!(
        session.activate_instruction(request("calendar", "act-1", None, CALENDAR_ONE)),
        Err(InstructionActivationError::RecordedButNotEffective)
    );
    assert_eq!(
        session.activate_instruction(request("calendar", "act-1", None, "another body")),
        Err(InstructionActivationError::ActivationIdentityConflict)
    );
}

/// A retired revision stays bound to its digest: its revision id with
/// another body is refused.
#[tokio::test]
async fn a_retired_revision_cannot_bind_another_body() {
    let (mut agent, _) = compacted(session_with_a_superseded_activation()).await;
    let mut rebound = request("calendar", "act-4", Some("act-2"), "another calendar body");
    rebound.revision.revision_id = InstructionRevisionId::new("rev-act-1").unwrap();
    assert_eq!(
        agent.session_mut().activate_instruction(rebound),
        Err(InstructionActivationError::ImmutableRevisionConflict)
    );
}

/// The 1 MiB activation budget counts live bodies only: four 240 KiB
/// revisions of one key leave no room for a fifth until a compaction
/// retires the three superseded bodies.
#[tokio::test]
async fn the_budget_counts_live_bodies_only() {
    let body = |tag: char| tag.to_string().repeat(240 * 1024);
    let mut session = prompted_session();
    for (index, tag) in ['a', 'b', 'c', 'd'].into_iter().enumerate() {
        let activation = format!("act-{}", index + 1);
        let supersedes = (index > 0).then(|| format!("act-{index}"));
        session
            .activate_instruction(request(
                "calendar",
                &activation,
                supersedes.as_deref(),
                &body(tag),
            ))
            .unwrap();
        exchange(&mut session, "a question", "an answer");
    }
    let fifth = request("calendar", "act-5", Some("act-4"), &body('e'));
    assert_eq!(
        session.clone().activate_instruction(fifth.clone()),
        Err(InstructionActivationError::LineageBodyBudgetExceeded {
            max_bytes: meerkat_core::MAX_INSTRUCTION_ACTIVATION_LINEAGE_BYTES,
        }),
        "before a compaction the four bodies leave no room for a fifth"
    );

    let (mut agent, _) = compacted(session).await;
    agent
        .session_mut()
        .activate_instruction(fifth)
        .unwrap_or_else(|error| {
            panic!("after the compaction the fifth activation commits: {error}")
        });
}

/// A generic rewrite cannot retire a body, even a superseded one, and a
/// restore of the revision before a retiring compaction cannot revive it:
/// both are refused and change nothing.
#[tokio::test]
async fn no_generic_rewrite_or_restore_retires_or_revives_a_body() {
    let mut session = session_with_a_superseded_activation();
    let rows = session.messages().to_vec();
    let superseded = index_of(&rows, "act-1");
    let forged = retired_by_hand(&rows[superseded], CALENDAR_ONE.len());
    let rewrite = session.commit_transcript_rewrite(
        TranscriptRewriteSelection::MessageRange {
            start: superseded,
            end: superseded + 1,
        },
        vec![forged],
        TranscriptRewriteReason::new("retire a superseded body by hand"),
        Some("unit-test".to_string()),
        None,
    );
    assert!(
        matches!(
            &rewrite,
            Err(TranscriptEditError::InvalidTranscriptShape(_))
        ),
        "a generic rewrite cannot retire a body: {rewrite:?}"
    );
    assert_eq!(session.messages(), rows.as_slice());

    let (mut agent, _) = compacted(session).await;
    let session = agent.session_mut();
    let compacted_rows = session.messages().to_vec();
    let history = session
        .validated_transcript_history_state()
        .unwrap()
        .expect("the compaction left a rewrite graph");
    let before_compaction = history
        .commits()
        .last()
        .expect("the compaction's commit")
        .parent_revision
        .clone();
    let rows = session
        .retained_transcript_revision_rows(&before_compaction)
        .unwrap()
        .expect("the revision before the compaction is retained");
    let restore = session.commit_transcript_revision_restore(
        rows,
        TranscriptRewriteReason::new("restore before the compaction"),
        Some("unit-test".to_string()),
        None,
    );
    assert!(
        matches!(
            &restore,
            Err(TranscriptEditError::InvalidTranscriptShape(_))
        ),
        "a restore cannot revive a retired body: {restore:?}"
    );
    assert_eq!(session.messages(), compacted_rows.as_slice());
}

/// A compactor's own rebuild cannot retire a body: a host compactor that
/// puts a retired form in place of a row it keeps has its rebuild refused,
/// and the history stays as it was.
#[tokio::test]
async fn no_compactor_rebuild_can_retire_a_body() {
    let session = session_with_a_superseded_activation();
    let rows = session.messages().to_vec();
    let superseded = index_of(&rows, "act-1");
    let client = Arc::new(RecordingClient::default());
    let compactor = Arc::new(ArmedCompactor::default());
    compactor.forge(
        superseded,
        retired_by_hand(&rows[superseded], CALENDAR_ONE.len()),
    );
    let mut agent = build_agent(session, &client, &compactor, None).await;
    compactor.arm(1);
    agent.run("the next question".into()).await.unwrap();
    assert_eq!(compactor.rebuilds(), 1, "the compactor rebuilt the history");
    let messages = agent.session().messages();
    assert_eq!(
        messages[superseded], rows[superseded],
        "the superseded row keeps its body"
    );
    assert!(
        !messages.iter().any(|message| matches!(
            message,
            Message::User(user) if user.transcript_role.is_compaction_summary()
        )),
        "the refused rebuild committed nothing"
    );
}

/// The rewrite graph that holds a retiring compaction record validates
/// when the document is reloaded, the retiring record with every other.
#[tokio::test]
async fn the_reloaded_graph_validates_a_retiring_compaction_record() {
    let (agent, _) = compacted(session_with_a_superseded_activation()).await;
    let reloaded = Session::from_persisted_bytes(&agent.session().to_persisted_bytes().unwrap())
        .expect("the reloaded graph validates the retiring record");
    assert_eq!(reloaded.messages(), agent.session().messages());
    assert_eq!(
        marker_of(&activation_rows(reloaded.messages())[0]),
        marker(CALENDAR_ONE.len()),
        "the reloaded transcript holds the retired row"
    );
    let history = reloaded
        .validated_transcript_history_state()
        .unwrap()
        .expect("the session holds a rewrite graph");
    assert!(history.commit_count() >= 1);
    for commit in history.commits() {
        let parent = history.materialize_rewrite_parent(commit).unwrap();
        let child = history.materialize_rewrite_child(commit).unwrap();
        crate::TranscriptRewriteRecord::new(commit.clone(), parent, child)
            .expect("each rewrite record validates with both of its bodies");
    }
}

/// A fork child reads a copied row as its inherited header, which names the
/// body's length and holds no body, whether the parent retired that body
/// before the fork or not: children forked before and after the parent's
/// compaction read byte-identical headers.
#[tokio::test]
async fn a_fork_child_reads_a_copied_retired_row_as_its_inherited_header() {
    let child_id = SessionId::new();
    let session = session_with_a_superseded_activation();
    let before = session
        .fork_at_complete_boundary_with_identity(session.messages().len(), child_id.clone())
        .unwrap();
    let (agent, _) = compacted(session).await;
    let parent = agent.session();
    let after = parent
        .fork_at_complete_boundary_with_identity(parent.messages().len(), child_id)
        .unwrap();
    let headers = |child: &Session| {
        activation_rows(&child.messages_for_model_boundary().unwrap())
            .iter()
            .map(|row| system_of(row).content.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(headers(&before), headers(&after));
    let after_headers = headers(&after);
    assert!(
        after_headers[0].starts_with("[meerkat-inherited-instruction-v2]")
            && after_headers[0].ends_with(&format!("body_bytes={}", CALENDAR_ONE.len())),
        "the copied retired row reads as the inherited header: {after_headers:?}"
    );
    assert!(
        !after_headers
            .iter()
            .any(|header| header.contains(CALENDAR_ONE))
    );
}

/// A fork whose prefix ends between a retired row and the row that
/// superseded it copies a retired head: re-activating it is refused typed
/// (`head_body_retired`) before anything is committed, while the same fork
/// at the tail re-activates every key.
#[tokio::test]
async fn reactivate_inherited_refuses_a_retired_copied_head() {
    let (agent, _) = compacted(session_with_a_superseded_activation()).await;
    let parent = agent.session();
    let cut = index_of(parent.messages(), "act-1") + 1;
    let mut child = parent.fork_at_complete_boundary(cut).unwrap();
    let before = child.messages().to_vec();
    let refusal = child
        .apply_fork_instruction_policy(parent.id(), ForkInstructionPolicy::ReactivateInherited)
        .expect_err("a fork whose copied head was retired cannot re-activate it");
    assert_eq!(
        serde_json::to_value(&refusal.cause).unwrap()["kind"],
        "head_body_retired",
        "{refusal:?}"
    );
    assert_eq!(
        refusal.keys,
        vec![(
            InstructionNamespace::new(NAMESPACE).unwrap(),
            InstructionKey::new("calendar").unwrap()
        )]
    );
    assert_eq!(child.messages(), before.as_slice(), "nothing is committed");

    let mut tail = parent
        .fork_at_complete_boundary(parent.messages().len())
        .unwrap();
    assert_eq!(
        tail.apply_fork_instruction_policy(parent.id(), ForkInstructionPolicy::ReactivateInherited),
        Ok(2),
        "the same fork at the tail re-activates both keys"
    );
}

/// A fork child's compaction retires the copied rows its own activations
/// supersede, and keeps its own rows' bodies.
#[tokio::test]
async fn a_child_compaction_retires_the_copied_rows_its_activations_supersede() {
    let parent = session_with_a_superseded_activation();
    let mut child = parent.fork();
    child
        .set_build_state(meerkat_core::SessionBuildState::default())
        .expect("test session build state should serialize");
    assert_eq!(
        child
            .apply_fork_instruction_policy(parent.id(), ForkInstructionPolicy::ReactivateInherited),
        Ok(2)
    );
    let own = activation_rows(child.messages())[3..].to_vec();

    let (agent, _) = compacted(child).await;
    let rows = activation_rows(agent.session().messages());
    assert_eq!(rows.len(), 5);
    assert_eq!(marker_of(&rows[0]), marker(CALENDAR_ONE.len()));
    assert_eq!(marker_of(&rows[1]), marker(CALENDAR_TWO.len()));
    assert_eq!(marker_of(&rows[2]), marker(BILLING_ONE.len()));
    assert_eq!(
        rows[3..].to_vec(),
        own,
        "the child's own re-activations keep their bodies"
    );
}

/// A retiring compaction writes a document without operator-instruction
/// state at envelope 5: a release that cannot read a retired body refuses
/// the document typed.
#[tokio::test]
async fn a_retiring_compaction_writes_envelope_5() {
    let session = session_with_a_superseded_activation();
    assert!(!session.carries_operator_instruction_facts());
    assert_eq!(session.persisted_envelope_version(), 4);
    let (agent, _) = compacted(session).await;
    assert!(
        agent.session().carries_operator_instruction_facts(),
        "the retirement records the envelope-5 marker"
    );
    assert_eq!(agent.session().persisted_envelope_version(), 5);
    assert_eq!(envelope_version(agent.session()), 5);
}

/// A compaction that retires nothing (each key has one row) keeps every
/// activation row byte-identical and the document at envelope 4.
#[tokio::test]
async fn a_compaction_that_retires_nothing_keeps_the_document_byte_identical() {
    let mut session = prompted_session();
    session
        .activate_instruction(request("calendar", "act-1", None, CALENDAR_ONE))
        .unwrap();
    exchange(&mut session, "first question", "first answer");
    session
        .activate_instruction(request("billing", "act-2", None, BILLING_ONE))
        .unwrap();
    exchange(&mut session, "second question", "second answer");
    let before = activation_rows(session.messages());

    let (agent, _) = compacted(session).await;
    assert_eq!(activation_rows(agent.session().messages()), before);
    assert!(!agent.session().carries_operator_instruction_facts());
    assert_eq!(agent.session().persisted_envelope_version(), 4);
    let bytes = agent.session().to_persisted_bytes().unwrap();
    let reloaded = Session::from_persisted_bytes(&bytes).unwrap();
    assert_eq!(
        reloaded.to_persisted_bytes().unwrap(),
        bytes,
        "a reload writes back the same envelope-4 bytes"
    );
}

/// The newest released reader refuses a document with a retired body
/// typed, at its envelope version, before reading any row.
#[tokio::test]
async fn the_published_reader_refuses_a_retired_document_typed() {
    let (agent, _) = compacted(session_with_a_superseded_activation()).await;
    let observed = u32::try_from(envelope_version(agent.session())).unwrap();
    let refusal = released_v0_9_8_version_authority::restore_session_envelope_version(observed)
        .expect_err("the released reader refuses a document with a retired body");
    assert!(
        refusal.to_string().contains("rejected SessionEnvelope"),
        "{refusal}"
    );
}

/// No summary request carries a superseded body: the summarizer reads a
/// superseded row as its retired form, so the summary cannot restate it.
#[tokio::test]
async fn no_summary_request_carries_a_superseded_body() {
    let (_, client) = compacted(session_with_a_superseded_activation()).await;
    let summaries = client.summaries();
    assert_eq!(summaries.len(), 1, "one summary request");
    let source = text_of(&summaries[0]);
    assert!(
        !source.contains(CALENDAR_ONE),
        "the summary request carries no superseded body: {source}"
    );
    assert!(source.contains(RETIRED_HEADER));
    assert!(source.contains(CALENDAR_TWO), "the effective body stays");
}

/// No host curator window carries a superseded body either.
#[tokio::test]
async fn no_curator_window_carries_a_superseded_body() {
    let curator = Arc::new(RecordingCurator::default());
    compacted_with(session_with_a_superseded_activation(), Some(&curator)).await;
    let windows = curator.windows();
    assert_eq!(windows.len(), 1, "the curator wrote the summary");
    let window = text_of(&windows[0]);
    assert!(
        !window.contains(CALENDAR_ONE),
        "the curator window carries no superseded body: {window}"
    );
    assert!(window.contains(CALENDAR_TWO), "the effective body stays");
}
