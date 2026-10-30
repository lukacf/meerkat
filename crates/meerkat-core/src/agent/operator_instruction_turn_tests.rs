//! S4 rows: operator instructions at the agent's run start, through the real
//! agent loop. The session task calls
//! `Agent::apply_run_start_operator_instructions` after it resolves a turn's
//! start disposition and before it appends any input of the run; these rows
//! drive the same seam directly and read every request the model receives.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use crate as meerkat_core;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_core::types::{InstructionKey, InstructionNamespace, UserMessage};
use meerkat_core::{
    AgentBuilder, AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    AssistantBlock, InstructionDeferReason, InstructionLowering, InstructionRevision,
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
    lowering: Option<InstructionLowering>,
) -> TestAgent {
    let mut builder = AgentBuilder::new()
        .resume_session(session)
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ));
    if let Some(lowering) = lowering {
        builder = builder.with_operator_instruction_lowering(lowering);
    }
    builder
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

fn slot(key: &str) -> InstructionSlot {
    InstructionSlot {
        namespace: InstructionNamespace::new("app.example").unwrap(),
        key: InstructionKey::new(key).unwrap(),
    }
}

fn write(key: &str, revision: u64, body: &str) -> SessionInstructionWrite {
    SessionInstructionWrite {
        slot: slot(key),
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

fn entry_rows(messages: &[Message]) -> Vec<&Message> {
    messages
        .iter()
        .filter(|message| is_entry(message))
        .collect()
}

fn last_user_text(messages: &[Message]) -> Option<String> {
    messages.iter().rev().find_map(|message| match message {
        Message::User(user) => Some(user.text_content()),
        _ => None,
    })
}

fn request_json(messages: &[Message]) -> String {
    serde_json::to_string(messages).unwrap()
}

#[tokio::test]
async fn three_writes_before_the_next_turn_produce_one_entry() {
    let client = Arc::new(RecordingClient::default());
    let mut agent = build_agent(
        conversed_session(),
        &client,
        Some(InstructionLowering::MidConversation),
    )
    .await;
    for (key, revision, body) in [
        ("tone", 1, "Answer formally."),
        ("style", 1, "Use short sentences."),
        ("tone", 2, "Answer warmly."),
    ] {
        agent
            .session_mut()
            .set_session_instruction(write(key, revision, body))
            .unwrap();
    }
    let outcome = agent
        .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
        .unwrap();
    assert_eq!(
        outcome,
        RunStartInstructionOutcome::Applied {
            entry_sequence: 1,
            target_generation: 3,
        }
    );
    agent.run("next".into()).await.unwrap();

    let requests = client.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    let entries = entry_rows(request);
    assert_eq!(entries.len(), 1, "three writes coalesce into one entry");
    let Message::System(entry) = entries[0] else {
        unreachable!()
    };
    let identity = entry.instruction_entry.as_ref().unwrap();
    assert_eq!(identity.changes().len(), 2);
    assert!(entry.content.contains("Answer warmly."));
    assert!(!entry.content.contains("Answer formally."));
    // The entry follows the history and precedes the run's own input.
    let entry_index = request.iter().position(is_entry).unwrap();
    let user_index = request
        .iter()
        .rposition(|message| matches!(message, Message::User(_)))
        .unwrap();
    assert!(entry_index > 2 && entry_index < user_index);
    assert_eq!(last_user_text(request).as_deref(), Some("next"));
}

#[tokio::test]
async fn resume_run_never_applies_an_entry() {
    let client = Arc::new(RecordingClient::default());
    let mut agent = build_agent(
        conversed_session(),
        &client,
        Some(InstructionLowering::MidConversation),
    )
    .await;
    agent
        .session_mut()
        .set_session_instruction(write("tone", 1, "Answer formally."))
        .unwrap();
    let before = agent.session().messages().to_vec();
    for run_start in [InstructionRunStart::ResumeRun, InstructionRunStart::NoRun] {
        let outcome = agent
            .apply_run_start_operator_instructions(run_start, None)
            .unwrap();
        assert_eq!(
            outcome,
            RunStartInstructionOutcome::NotNeeded,
            "{run_start:?}"
        );
    }
    assert_eq!(
        request_json(agent.session().messages()),
        request_json(&before)
    );
    let state = agent
        .session()
        .instruction_target_state()
        .unwrap()
        .expect("the write is recorded");
    assert!(
        state.slots()[0].pending_body.is_some(),
        "the change stays pending for the next fresh run start"
    );
}

#[tokio::test]
async fn resume_then_next_request_is_byte_identical() {
    let client = Arc::new(RecordingClient::default());
    let mut agent = build_agent(
        conversed_session(),
        &client,
        Some(InstructionLowering::MidConversation),
    )
    .await;
    agent
        .session_mut()
        .set_session_instruction(write("tone", 1, "Answer formally."))
        .unwrap();
    agent
        .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
        .unwrap();
    agent.run("first".into()).await.unwrap();

    // The session as a store keeps it, read back by a new process.
    let bytes = agent.session().to_persisted_bytes().unwrap();
    let reloaded = Session::from_persisted_bytes(&bytes).unwrap();
    let resumed_client = Arc::new(RecordingClient::default());
    let mut resumed = build_agent(
        reloaded,
        &resumed_client,
        Some(InstructionLowering::MidConversation),
    )
    .await;

    for agent in [&mut agent, &mut resumed] {
        assert_eq!(
            agent
                .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
                .unwrap(),
            RunStartInstructionOutcome::NotNeeded,
            "an applied entry is not applied again after a reload"
        );
        agent.run("second".into()).await.unwrap();
    }
    let live = client.requests();
    let reloaded_requests = resumed_client.requests();
    let live_last = live.last().unwrap();
    let reloaded_last = reloaded_requests.last().unwrap();
    // Everything before the turn's own input is the reloaded history; the
    // input row carries its own creation time.
    assert_eq!(live_last.len(), reloaded_last.len());
    let history = live_last.len() - 1;
    assert_eq!(
        request_json(&live_last[..history]),
        request_json(&reloaded_last[..history]),
        "the request after a reload is byte-identical to the live one"
    );
    assert_eq!(last_user_text(live_last), last_user_text(reloaded_last));
    assert_eq!(entry_rows(reloaded_last).len(), 1);
}

fn incapable_anthropic_lowering() -> InstructionLowering {
    let registry = meerkat_core::ModelRegistry::from_config(
        &meerkat_core::Config::default(),
        *crate::model_profile::test_catalog::TEST_CATALOG,
    )
    .unwrap();
    let profile = registry
        .profile_for_identity(
            &meerkat_core::Config::default(),
            &meerkat_core::SessionLlmIdentity {
                model: crate::model_profile::test_catalog::ANTHROPIC_MODEL.to_string(),
                provider: meerkat_core::Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
        )
        .expect("the test catalog row");
    assert!(
        !profile.supports_mid_conversation_system_messages,
        "the row lowers System rows only as a leading prefix"
    );
    InstructionLowering::for_resolved_profile(Some(&profile))
}

#[tokio::test]
async fn incapable_anthropic_model_defers_with_leading_prefix_reason() {
    let lowering = incapable_anthropic_lowering();
    assert_eq!(lowering, InstructionLowering::LeadingPrefixOnly);
    let client = Arc::new(RecordingClient::default());
    let mut agent = build_agent(conversed_session(), &client, Some(lowering)).await;
    agent
        .session_mut()
        .set_session_instruction(write("tone", 1, "Answer formally."))
        .unwrap();
    let outcome = agent
        .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
        .unwrap();
    assert_eq!(
        outcome,
        RunStartInstructionOutcome::Deferred(InstructionDeferReason::LeadingPrefixOnly)
    );
    agent.run("next".into()).await.unwrap();
    let requests = client.requests();
    assert!(
        entry_rows(&requests[0]).is_empty(),
        "no entry reaches a model that cannot lower it here"
    );
}

#[tokio::test]
async fn first_run_of_a_fresh_session_carries_the_entry_in_the_leading_prefix() {
    let client = Arc::new(RecordingClient::default());
    let mut agent = build_agent(
        fresh_session(),
        &client,
        Some(incapable_anthropic_lowering()),
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
    agent.run("first".into()).await.unwrap();
    let request = &client.requests()[0];
    let first_non_system = request
        .iter()
        .position(|message| !matches!(message, Message::System(_)))
        .unwrap();
    let entry_index = request.iter().position(is_entry).unwrap();
    assert!(
        entry_index < first_non_system,
        "the entry is inside the leading System prefix: {request:?}"
    );
}

#[tokio::test]
async fn an_agent_without_a_resolved_profile_keeps_the_leading_prefix() {
    let client = Arc::new(RecordingClient::default());
    let agent = build_agent(conversed_session(), &client, None).await;
    assert_eq!(
        agent.operator_instruction_lowering(),
        InstructionLowering::LeadingPrefixOnly,
        "an unresolved capability claims no mid-conversation System row"
    );
    assert_eq!(
        InstructionLowering::for_resolved_profile(None),
        InstructionLowering::LeadingPrefixOnly
    );
}

#[tokio::test]
async fn a_run_from_a_pending_tail_applies_the_entry_after_that_tail() {
    let client = Arc::new(RecordingClient::default());
    let mut session = conversed_session();
    session.push(Message::User(UserMessage::text("pending question")));
    let mut agent = build_agent(session, &client, Some(InstructionLowering::MidConversation)).await;
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
    agent.set_runtime_execution_kind(Some(
        meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
    ));
    agent
        .run_pending()
        .await
        .expect("the pending tail still runs with the entry after it");
    let request = &client.requests()[0];
    let pending_index = request
        .iter()
        .rposition(|message| matches!(message, Message::User(_)))
        .unwrap();
    let entry_index = request.iter().position(is_entry).unwrap();
    assert!(
        entry_index > pending_index,
        "the entry follows the pending tail"
    );
    assert_eq!(last_user_text(request).as_deref(), Some("pending question"));
}
