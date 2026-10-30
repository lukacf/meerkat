//! Operator-instruction entries at a run start and in a fork child, through
//! the real agent loop.
//!
//! - An entry names the run the agent itself runs, the same rule a
//!   compaction fold at a run's first request follows, whether or not a
//!   runtime dispatched the run.
//! - A run that never starts (a refusal or a veto before it, an interrupt
//!   before it began) leaves no entry naming it: the entry is taken back and
//!   the recorded target restored, so the changes stay pending.
//! - A fork child whose model lowers System rows only in the leading System
//!   prefix, or carries none besides its prompt, sends its first request
//!   without the copied rows it cannot lower; its transcript keeps them.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use crate as meerkat_core;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_core::types::{InstructionKey, InstructionNamespace, UserMessage};
use meerkat_core::{
    AgentBuilder, AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    AssistantBlock, InstructionEntryIdentity, InstructionLowering, InstructionRevision,
    InstructionRunStart, InstructionSlot, LlmStreamResult, Message, RunStartInstructionOutcome,
    Session, SessionInstructionChange, SessionInstructionWrite, StopReason, ToolCallView, ToolDef,
    TurnUsage, Usage,
};

fn mock_usage() -> Usage {
    TurnUsage::host_declared(
        meerkat_core::Provider::Other,
        "mock-model",
        Usage::default(),
    )
    .into_inner()
}

/// Answers every request with one text block and keeps every request's
/// messages.
#[derive(Default)]
struct RecordingClient {
    requests: Mutex<Vec<Vec<Message>>>,
}

impl RecordingClient {
    fn requests(&self) -> Vec<Vec<Message>> {
        self.requests.lock().unwrap().clone()
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
        self.requests.lock().unwrap().push(messages.to_vec());
        Ok(LlmStreamResult::new(
            vec![AssistantBlock::Text {
                text: "done".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
            mock_usage(),
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
        Err(meerkat_core::ToolError::not_found(call.name))
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

type TestAgent = meerkat_core::Agent<RecordingClient, NoTools, NoopStore>;

async fn build_agent(
    session: Session,
    client: &Arc<RecordingClient>,
    lowering: InstructionLowering,
) -> TestAgent {
    AgentBuilder::new()
        .resume_session(session)
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .with_operator_instruction_lowering(lowering)
        .build_standalone(Arc::clone(client), Arc::new(NoTools), Arc::new(NoopStore))
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

/// A session that has had a conversation.
fn conversed_session() -> Session {
    let mut session = fresh_session();
    session.push(Message::User(UserMessage::text("hello")));
    session.push(Message::BlockAssistant(
        meerkat_core::BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "hi".to_string(),
                meta: None,
            }],
            stop_reason: Some(StopReason::EndTurn),
            identity: meerkat_core::types::TranscriptMessageIdentity::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
            assistant_message_id: None,
        },
    ));
    session
}

fn write(key: &str, revision: u64, body: &str) -> SessionInstructionWrite {
    SessionInstructionWrite {
        slot: InstructionSlot {
            namespace: InstructionNamespace::new("app.example").unwrap(),
            key: InstructionKey::new(key).unwrap(),
        },
        revision: InstructionRevision::new(revision).unwrap(),
        change: SessionInstructionChange::Activate {
            body: body.to_string(),
            content_sha256: None,
        },
    }
}

fn is_entry(message: &Message) -> bool {
    matches!(message, Message::System(system) if system.instruction_entry.is_some())
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

/// The run id the latest run stamped on its output.
fn last_run_id(messages: &[Message]) -> Option<meerkat_core::lifecycle::RunId> {
    messages.iter().rev().find_map(|message| match message {
        Message::BlockAssistant(assistant) => assistant.identity.run_id.clone(),
        _ => None,
    })
}

/// A run start no runtime dispatched still names the run: the run the agent
/// itself starts, which stamps the same id on its output.
#[tokio::test]
async fn a_run_start_entry_names_the_run_the_agent_runs() {
    let client = Arc::new(RecordingClient::default());
    let mut agent = build_agent(
        conversed_session(),
        &client,
        InstructionLowering::MidConversation,
    )
    .await;
    agent
        .session_mut()
        .set_session_instruction(write("tone", 1, "Answer formally."))
        .unwrap();
    let outcome = agent
        .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
        .unwrap();
    assert!(matches!(
        outcome,
        RunStartInstructionOutcome::Applied { .. }
    ));
    agent.run("next".into()).await.unwrap();

    let messages = agent.session().messages();
    let ran = last_run_id(messages);
    assert!(ran.is_some(), "the run stamps its id on its output");
    let entries = entry_identities(messages);
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].run_id(),
        ran.as_ref(),
        "the entry names the run the agent ran"
    );
}

/// How a wire carries System rows, as a provider adapter for such a model
/// enforces it.
#[derive(Debug, Clone, Copy)]
enum SystemRowWire {
    /// System rows only inside the leading System prefix.
    LeadingPrefix,
    /// Exactly one leading System row, the prompt.
    OneLeadingRow,
}

impl SystemRowWire {
    fn refuses(self, request: &[Message]) -> bool {
        let leading = request
            .iter()
            .position(|message| !matches!(message, Message::System(_)))
            .unwrap_or(request.len());
        let later_system_row = request[leading..]
            .iter()
            .any(|message| matches!(message, Message::System(_)));
        match self {
            Self::LeadingPrefix => later_system_row,
            Self::OneLeadingRow => later_system_row || leading > 1,
        }
    }
}

/// A parent applies one entry inside its leading System prefix and one
/// after a turn. A fork child copies both as history. A child whose model
/// lowers System rows only in the leading prefix still sends its first
/// request: the copied row after the conversation stays out of it, the one
/// in the leading prefix stays in. A child whose wire carries no System row
/// besides its prompt sends neither. Each child's transcript keeps both.
#[tokio::test]
async fn a_prefix_only_fork_child_leaves_out_the_copied_rows_it_cannot_lower() {
    let parent_client = Arc::new(RecordingClient::default());
    let mut parent = build_agent(
        fresh_session(),
        &parent_client,
        InstructionLowering::MidConversation,
    )
    .await;
    for (revision, body, input) in [
        (1, "Answer formally.", "first"),
        (2, "Answer warmly.", "second"),
    ] {
        parent
            .session_mut()
            .set_session_instruction(write("tone", revision, body))
            .unwrap();
        assert!(matches!(
            parent
                .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
                .unwrap(),
            RunStartInstructionOutcome::Applied { .. }
        ));
        parent.run(input.into()).await.unwrap();
    }
    let parent_rows = parent.session().messages().to_vec();
    let first_conversation_row = parent_rows
        .iter()
        .position(|message| !matches!(message, Message::System(_)))
        .unwrap();
    let entry_positions = parent_rows
        .iter()
        .enumerate()
        .filter(|(_, message)| is_entry(message))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(entry_positions.len(), 2);
    assert!(
        entry_positions[0] < first_conversation_row && entry_positions[1] > first_conversation_row,
        "one entry in the leading System prefix, one after the conversation: {parent_rows:?}"
    );

    for (lowering, wire) in [
        (
            InstructionLowering::LeadingPrefixOnly,
            SystemRowWire::LeadingPrefix,
        ),
        (
            InstructionLowering::Unavailable,
            SystemRowWire::OneLeadingRow,
        ),
    ] {
        let mut child_session = parent.session().fork();
        child_session
            .set_build_state(meerkat_core::SessionBuildState::default())
            .expect("test session build state should serialize");
        let copied = entry_identities(child_session.messages());
        assert_eq!(copied.len(), 2);
        let child_client = Arc::new(RecordingClient::default());
        let mut child = build_agent(child_session, &child_client, lowering).await;
        assert_eq!(
            child
                .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
                .unwrap(),
            RunStartInstructionOutcome::NotNeeded,
            "{lowering:?}: the child has no target of its own"
        );
        child.run("child question".into()).await.unwrap();

        let requests = child_client.requests();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert!(
            !wire.refuses(request),
            "{lowering:?}: the child's first request is one its wire carries: {request:?}"
        );
        let carried = entry_identities(request);
        match wire {
            SystemRowWire::LeadingPrefix => {
                assert_eq!(
                    carried.len(),
                    1,
                    "the copied row in the leading prefix stays in the request"
                );
                assert_eq!(carried[0].entry_sequence(), copied[0].entry_sequence());
            }
            SystemRowWire::OneLeadingRow => assert!(carried.is_empty()),
        }
        assert_eq!(
            entry_identities(child.session().messages()),
            copied,
            "{lowering:?}: the child's transcript keeps every copied row"
        );
    }
}

fn instruction_state(session: &Session) -> Option<serde_json::Value> {
    session
        .metadata()
        .get(meerkat_core::SESSION_INSTRUCTION_STATE_KEY)
        .cloned()
}

fn request_json(messages: &[Message]) -> String {
    serde_json::to_string(messages).unwrap()
}

/// The session task reports a run that never started (a refusal or a veto
/// before it, an interrupt before it began): the entry its run start
/// appended is taken back and the recorded target restored, so the next
/// fresh run start applies the same change again and the entry names the
/// run that does run.
#[tokio::test]
async fn an_entry_whose_run_never_starts_is_taken_back_and_its_change_stays_pending() {
    let client = Arc::new(RecordingClient::default());
    let mut agent = build_agent(
        conversed_session(),
        &client,
        InstructionLowering::MidConversation,
    )
    .await;
    agent
        .session_mut()
        .set_session_instruction(write("tone", 1, "Answer formally."))
        .unwrap();
    let transcript_before = request_json(agent.session().messages());
    let state_before = instruction_state(agent.session());
    assert!(state_before.is_some(), "the write is recorded");

    let outcome = agent
        .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
        .unwrap();
    assert!(matches!(
        outcome,
        RunStartInstructionOutcome::Applied {
            entry_sequence: 1,
            ..
        }
    ));
    assert_eq!(entry_identities(agent.session().messages()).len(), 1);

    assert!(
        agent
            .conclude_run_start_operator_instructions(false)
            .unwrap(),
        "the entry of a run that never started is taken back"
    );
    assert_eq!(
        request_json(agent.session().messages()),
        transcript_before,
        "no row names a run that never ran"
    );
    assert_eq!(
        instruction_state(agent.session()),
        state_before,
        "the recorded target is as the write left it"
    );
    assert!(
        !agent
            .conclude_run_start_operator_instructions(false)
            .unwrap(),
        "nothing is left to take back"
    );

    let outcome = agent
        .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
        .unwrap();
    assert!(
        matches!(
            outcome,
            RunStartInstructionOutcome::Applied {
                entry_sequence: 1,
                ..
            }
        ),
        "the change is still pending: {outcome:?}"
    );
    agent.run("next".into()).await.unwrap();
    assert!(
        !agent
            .conclude_run_start_operator_instructions(true)
            .unwrap(),
        "a run that started keeps its entry"
    );

    let messages = agent.session().messages();
    let entries = entry_identities(messages);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].run_id(), last_run_id(messages).as_ref());
    let requests = client.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]
            .iter()
            .filter(|message| is_entry(message))
            .count(),
        1
    );
}
