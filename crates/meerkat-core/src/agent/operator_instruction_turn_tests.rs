//! Rows for operator instructions at the agent's run start, through the real
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
async fn incapable_anthropic_model_takes_the_entry_in_its_leading_prefix() {
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
    assert!(matches!(
        outcome,
        RunStartInstructionOutcome::Applied { .. }
    ));
    agent.run("next".into()).await.unwrap();
    let requests = client.requests();
    let request = &requests[0];
    let first_non_system = request
        .iter()
        .position(|message| !matches!(message, Message::System(_)))
        .unwrap();
    let entry_index = request.iter().position(is_entry).unwrap();
    assert!(
        entry_index < first_non_system,
        "the entry reaches the model inside the leading System prefix: {request:?}"
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

/// Realms for the backend rows below: `chatgpt_realm`, whose OpenAI binding
/// is the ChatGPT backend (its Responses wire carries exactly one leading
/// System row, the session's prompt), and `public_realm`, whose OpenAI and
/// Anthropic bindings are the public APIs.
fn backend_realms_config() -> meerkat_core::Config {
    let mut chatgpt =
        meerkat_core::RealmConfigSection::from_inline_api_keys(&[("openai", "test-key")]);
    chatgpt
        .backend
        .get_mut("default_openai")
        .expect("OpenAI backend")
        .backend_kind = "chatgpt_backend".to_string();
    chatgpt
        .auth
        .get_mut("default_openai")
        .expect("OpenAI auth")
        .auth_method = "managed_chatgpt_oauth".to_string();
    let mut config = meerkat_core::Config::default();
    config.realm.insert("chatgpt_realm".to_string(), chatgpt);
    config.realm.insert(
        "public_realm".to_string(),
        meerkat_core::RealmConfigSection::from_inline_api_keys(&[
            ("openai", "test-key"),
            ("anthropic", "test-key"),
        ]),
    );
    config
}

fn configured_binding(realm: &str, binding: &str) -> meerkat_core::AuthBindingRef {
    meerkat_core::AuthBindingRef {
        realm: meerkat_core::RealmId::parse(realm).unwrap(),
        binding: meerkat_core::BindingId::parse(binding).unwrap(),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    }
}

fn chatgpt_binding() -> meerkat_core::AuthBindingRef {
    configured_binding("chatgpt_realm", "default_openai")
}

fn llm_identity(
    provider: meerkat_core::Provider,
    model: &str,
    auth_binding: Option<meerkat_core::AuthBindingRef>,
) -> meerkat_core::SessionLlmIdentity {
    meerkat_core::SessionLlmIdentity {
        model: model.to_string(),
        provider,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding,
    }
}

fn openai_identity(
    auth_binding: Option<meerkat_core::AuthBindingRef>,
) -> meerkat_core::SessionLlmIdentity {
    llm_identity(
        meerkat_core::Provider::OpenAI,
        crate::model_profile::test_catalog::OPENAI_MODEL,
        auth_binding,
    )
}

fn backend_test_registry(config: &meerkat_core::Config) -> meerkat_core::ModelRegistry {
    meerkat_core::ModelRegistry::from_config(
        config,
        *crate::model_profile::test_catalog::TEST_CATALOG,
    )
    .unwrap()
}

/// The ChatGPT backend's wire carries exactly one leading System row, the
/// session's prompt, so it carries no operator-instruction entry: a model
/// served through that binding lowers none, whether a build resolves the
/// lowering from the config or a model fallback activates the witness
/// judged through that binding. A backend that carries more leading System
/// rows keeps what the model row lowers (the test catalog's rows lower
/// System rows only as a leading prefix).
#[tokio::test]
async fn a_chatgpt_backend_binding_lowers_no_operator_instruction_entry() {
    let config = backend_realms_config();
    let registry = backend_test_registry(&config);
    let lowering = |identity: &meerkat_core::SessionLlmIdentity| {
        registry.instruction_lowering_for_identity(&config, identity)
    };
    assert_eq!(
        lowering(&openai_identity(Some(chatgpt_binding()))),
        InstructionLowering::Unavailable,
        "a build on the ChatGPT backend lowers no entry"
    );
    assert_eq!(
        lowering(&openai_identity(Some(configured_binding(
            "missing_realm",
            "default_openai"
        )))),
        InstructionLowering::Unavailable,
        "a binding the config does not resolve has an unknown backend, and an OpenAI \
         backend may carry only one leading System row"
    );
    let env_default = meerkat_core::AuthBindingRef {
        realm: meerkat_core::RealmId::parse("env_default").unwrap(),
        binding: meerkat_core::BindingId::parse("default").unwrap(),
        profile: None,
        origin: meerkat_core::BindingOrigin::SyntheticEnvDefault,
    };
    for (identity, why) in [
        (
            openai_identity(Some(configured_binding("public_realm", "default_openai"))),
            "the public API carries every leading System row",
        ),
        (
            openai_identity(None),
            "an identity that names no binding is served by the API-key backend",
        ),
        (
            openai_identity(Some(env_default)),
            "the env-var default names the API-key backend",
        ),
        (
            llm_identity(
                meerkat_core::Provider::Anthropic,
                crate::model_profile::test_catalog::ANTHROPIC_MODEL,
                Some(configured_binding("missing_realm", "default_anthropic")),
            ),
            "every Anthropic backend carries every leading System row",
        ),
    ] {
        assert_eq!(
            lowering(&identity),
            InstructionLowering::LeadingPrefixOnly,
            "{why}"
        );
    }

    // A model fallback activates the target's witness, judged through the
    // binding its client uses, and leaves the agent to derive the lowering
    // from it: the two assignments of the fallback's publication.
    let witness = registry
        .profile_witness_for_identity(&config, &openai_identity(Some(chatgpt_binding())))
        .expect("the test catalog row");
    let client = Arc::new(RecordingClient::default());
    let mut agent = build_agent(
        conversed_session(),
        &client,
        Some(InstructionLowering::MidConversation),
    )
    .await;
    agent.active_model_profile = Some(witness);
    agent.operator_instruction_lowering = None;
    assert_eq!(
        agent.operator_instruction_lowering(),
        InstructionLowering::Unavailable,
        "the fallback target's witness states its backend's lowering"
    );
    agent
        .session_mut()
        .set_session_instruction(write("tone", 1, "Answer formally."))
        .unwrap();
    assert_eq!(
        agent
            .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
            .unwrap(),
        RunStartInstructionOutcome::Deferred(InstructionDeferReason::LoweringUnavailable)
    );
    assert!(entry_rows(agent.session().messages()).is_empty());
}

/// A conversed session whose durable identity is `identity`.
fn conversed_session_served_by(identity: &meerkat_core::SessionLlmIdentity) -> Session {
    let mut session = conversed_session();
    session
        .set_session_metadata(meerkat_core::SessionMetadata {
            model_fallback: None,
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: identity.model.clone(),
            max_tokens: 16_384,
            structured_output_retries: 2,
            provider: identity.provider,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: meerkat_core::SessionTooling::default(),
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: identity.auth_binding.clone(),
            mob_member_binding: None,
        })
        .expect("test session metadata should serialize");
    session
}

/// Swap the agent's model as a live hot swap does: the request policy
/// carries the lowering the host resolved for `identity`, with the config
/// that resolves its binding.
fn hot_swap(
    agent: &mut TestAgent,
    client: &Arc<RecordingClient>,
    config: &meerkat_core::Config,
    registry: &meerkat_core::ModelRegistry,
    identity: meerkat_core::SessionLlmIdentity,
) {
    let request_policy = meerkat_core::SessionLlmRequestPolicy {
        model: identity.model.clone(),
        credential_identity: None,
        provider_params: None,
        provider_tool_defaults: None,
        provider_native_tools: meerkat_core::ProviderNativeToolPolicy::Inherit,
        operator_instruction_lowering: registry
            .instruction_lowering_for_identity(config, &identity),
    };
    agent
        .hot_swap_llm_identity(Arc::clone(client), identity, request_policy)
        .expect("the hot swap commits");
}

/// A live model hot swap installs the lowering the host resolved for the
/// new identity's binding. Onto the ChatGPT backend's binding, operator
/// instructions lower nowhere: a pending target stays pending and no entry
/// row is appended. Onto a named public API binding, an identity naming no
/// binding, or an Anthropic binding, the swap keeps what the model row
/// lowers (the test catalog's rows lower System rows only as a leading
/// prefix).
#[tokio::test]
async fn a_hot_swap_onto_a_chatgpt_backend_binding_defers_operator_instructions() {
    let config = backend_realms_config();
    let registry = Arc::new(backend_test_registry(&config));
    let client = Arc::new(RecordingClient::default());
    let mut agent = AgentBuilder::new()
        .resume_session(conversed_session_served_by(&openai_identity(Some(
            configured_binding("public_realm", "default_openai"),
        ))))
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .with_effective_model_registry(Arc::clone(&registry))
        .with_operator_instruction_lowering(InstructionLowering::LeadingPrefixOnly)
        .build_standalone(Arc::clone(&client), Arc::new(NoTools), Arc::new(NoopStore))
        .await;
    agent
        .session_mut()
        .set_session_instruction(write("tone", 1, "Answer formally."))
        .unwrap();

    hot_swap(
        &mut agent,
        &client,
        &config,
        &registry,
        openai_identity(Some(chatgpt_binding())),
    );
    assert_eq!(
        agent.operator_instruction_lowering(),
        InstructionLowering::Unavailable,
        "a hot swap onto the ChatGPT backend's binding claims no entry lowering"
    );
    let before = request_json(agent.session().messages());
    assert_eq!(
        agent
            .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
            .unwrap(),
        RunStartInstructionOutcome::Deferred(InstructionDeferReason::LoweringUnavailable)
    );
    assert_eq!(
        request_json(agent.session().messages()),
        before,
        "no entry row is appended"
    );
    let state = agent
        .session()
        .instruction_target_state()
        .unwrap()
        .expect("the write is recorded");
    assert!(
        state.slots()[0].pending_body.is_some(),
        "the target stays pending"
    );

    hot_swap(
        &mut agent,
        &client,
        &config,
        &registry,
        openai_identity(Some(configured_binding("public_realm", "default_openai"))),
    );
    assert_eq!(
        agent.operator_instruction_lowering(),
        InstructionLowering::LeadingPrefixOnly,
        "a named public API binding keeps what the model row lowers"
    );
    hot_swap(
        &mut agent,
        &client,
        &config,
        &registry,
        openai_identity(None),
    );
    assert_eq!(
        agent.operator_instruction_lowering(),
        InstructionLowering::LeadingPrefixOnly,
        "an identity naming no binding is served by the API-key backend"
    );
    hot_swap(
        &mut agent,
        &client,
        &config,
        &registry,
        llm_identity(
            meerkat_core::Provider::Anthropic,
            crate::model_profile::test_catalog::ANTHROPIC_MODEL,
            Some(configured_binding("public_realm", "default_anthropic")),
        ),
    );
    assert_eq!(
        agent.operator_instruction_lowering(),
        InstructionLowering::LeadingPrefixOnly,
        "every Anthropic backend carries every leading System row"
    );
}

/// The entry rows a session wrote itself are the ones its model change
/// judges; a fork child's copied rows are left out wherever its model cannot
/// lower them. A recorded target is no row. An entry applied after the
/// conversation is carried by a model that lowers System rows anywhere, and
/// by one that lowers them only inside the leading System prefix, where the
/// session's request places it; a wire that carries one leading System row
/// takes none, and a request's rows as they stand are judged unplaced. A
/// fork child that copied the entry holds none of its own.
#[tokio::test]
async fn own_operator_instruction_entries_are_the_rows_the_session_wrote() {
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
    assert!(
        agent
            .session()
            .own_operator_instruction_entry_placement()
            .unwrap()
            .is_empty(),
        "a recorded target is no row"
    );
    agent
        .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
        .unwrap();
    agent.run("next".into()).await.unwrap();
    let placement = agent
        .session()
        .own_operator_instruction_entry_placement()
        .unwrap();
    assert!(
        !placement.is_empty(),
        "the applied entry is the session's own"
    );
    assert_eq!(
        placement.refusal(InstructionLowering::MidConversation),
        None
    );
    assert_eq!(
        placement.refusal(InstructionLowering::LeadingPrefixOnly),
        None,
        "the session's request places its own entry inside the leading System prefix"
    );
    assert_eq!(
        placement.refusal(InstructionLowering::Unavailable),
        Some(meerkat_core::InstructionEntryLoweringRefusal::AfterConversationRow),
        "a wire that carries one leading System row takes no placed entry"
    );
    // The same rows as a request holds them, unplaced: a per-request model
    // fallback hands them to its target as they stand.
    let request = meerkat_core::InstructionEntryPlacement::of_request(agent.session().messages());
    assert_eq!(request.refusal(InstructionLowering::MidConversation), None);
    for lowering in [
        InstructionLowering::LeadingPrefixOnly,
        InstructionLowering::Unavailable,
    ] {
        assert_eq!(
            request.refusal(lowering),
            Some(meerkat_core::InstructionEntryLoweringRefusal::AfterConversationRow),
            "a request's entry after the conversation is not carried by a {lowering:?} model"
        );
    }

    let child = agent.session().fork();
    assert_eq!(
        entry_rows(child.messages()).len(),
        1,
        "the child copied the row"
    );
    assert!(
        child
            .own_operator_instruction_entry_placement()
            .unwrap()
            .is_empty(),
        "a copied row is history, not the child's own"
    );
}

/// A build that resumes a session counts a prompt row it may author beside
/// the session's own entries (the facade's `resumed_build_entry_placement`)
/// before it knows the exact prompt; the builder then appends nothing when
/// the prompt is already the newest prompt row. Counting that row changes no
/// verdict: the newest prompt row is already another System row of the
/// placement, on every lowering, with the entry in the leading System prefix
/// or after a conversation row.
#[test]
fn a_resume_prompt_the_builder_does_not_append_changes_no_entry_verdict() {
    let prompt = "You are a careful assistant.";
    for (case, mut session) in [
        ("leading prefix", fresh_session()),
        ("after a conversation row", conversed_session()),
    ] {
        session
            .append_operator_instruction_activation_for_test("app.example", "tone", 1, "Formal.")
            .unwrap();
        assert!(
            session.newest_prompt_row_is(prompt),
            "{case}: a resume with the session's prompt appends nothing"
        );
        let placement = session.own_operator_instruction_entry_placement().unwrap();
        assert!(!placement.is_empty(), "{case}: the entry is judged");
        for lowering in [
            InstructionLowering::MidConversation,
            InstructionLowering::LeadingPrefixOnly,
            InstructionLowering::Unavailable,
        ] {
            assert_eq!(
                placement.with_another_system_row().refusal(lowering),
                placement.refusal(lowering),
                "{case}, {lowering:?}: the counted prompt row changes no verdict"
            );
        }
    }
}

/// On a wire that carries one leading System row, an entry the session wrote
/// itself is carried only as the request's one System row: an entry beside
/// the prompt, or beside another entry (with or without a prompt), is not.
/// A row a fork copied is neither the session's own entry nor another System
/// row (the request leaves it out on such a wire), so a fork child's own
/// entry beside copied rows alone is carried. A request's placement judges
/// every entry row it holds.
#[test]
fn an_entry_beside_another_system_row_is_carried_only_where_the_wire_takes_them_all() {
    let mut session = fresh_session();
    session
        .append_operator_instruction_activation_for_test("app.example", "tone", 1, "Formal.")
        .unwrap();
    let beside_the_prompt = session.own_operator_instruction_entry_placement().unwrap();
    assert_eq!(
        beside_the_prompt.refusal(InstructionLowering::Unavailable),
        Some(meerkat_core::InstructionEntryLoweringRefusal::BesideAnotherSystemRow)
    );
    for lowering in [
        InstructionLowering::MidConversation,
        InstructionLowering::LeadingPrefixOnly,
    ] {
        assert_eq!(beside_the_prompt.refusal(lowering), None);
    }

    let mut alone = Session::new();
    alone
        .append_operator_instruction_activation_for_test("app.example", "tone", 1, "Formal.")
        .unwrap();
    let placement = alone.own_operator_instruction_entry_placement().unwrap();
    assert_eq!(
        placement.refusal(InstructionLowering::Unavailable),
        None,
        "the request's only System row is carried as its leading row"
    );
    assert_eq!(
        placement
            .with_another_system_row()
            .refusal(InstructionLowering::Unavailable),
        Some(meerkat_core::InstructionEntryLoweringRefusal::BesideAnotherSystemRow),
        "a prompt the build adds stands beside it"
    );
    assert_eq!(
        meerkat_core::InstructionEntryPlacement::of_request(alone.messages())
            .refusal(InstructionLowering::Unavailable),
        None
    );

    // Two entries of its own and no prompt: each stands beside the other.
    let mut two_entries = Session::new();
    for (key, body) in [("tone", "Formal."), ("units", "Metric.")] {
        two_entries
            .append_operator_instruction_activation_for_test("app.example", key, 1, body)
            .unwrap();
    }
    assert_eq!(
        two_entries
            .own_operator_instruction_entry_placement()
            .unwrap()
            .refusal(InstructionLowering::Unavailable),
        Some(meerkat_core::InstructionEntryLoweringRefusal::BesideAnotherSystemRow),
        "two entries are two System rows"
    );

    // A fork child's own entry beside the row it copied, and no prompt.
    let mut child = alone.fork();
    child
        .append_operator_instruction_activation_for_test("app.example", "units", 1, "Metric.")
        .unwrap();
    assert_eq!(
        entry_rows(child.messages()).len(),
        2,
        "the copied row and the child's own"
    );
    let placement = child.own_operator_instruction_entry_placement().unwrap();
    assert!(!placement.is_empty(), "the child's own entry is judged");
    assert_eq!(
        placement.refusal(InstructionLowering::Unavailable),
        None,
        "a copied row is neither an own entry nor another System row"
    );
}

// ---------------------------------------------------------------------------
// A model that carries System rows only inside the leading System prefix
// takes a changed instruction at its next run, as every other model does.
// The entry is appended at the transcript tail (nothing the session already
// holds is rewritten or moved), and the model request places the session's
// own entries inside the leading System prefix.
// ---------------------------------------------------------------------------

/// How many rows the leading System prefix of `messages` holds.
fn leading_system_rows(messages: &[Message]) -> usize {
    messages
        .iter()
        .position(|message| !matches!(message, Message::System(_)))
        .unwrap_or(messages.len())
}

/// Every System row of `request` stands inside its leading System prefix:
/// what a model that carries System rows only there can be asked with.
fn assert_system_rows_lead(request: &[Message]) {
    let leading = leading_system_rows(request);
    for (index, message) in request.iter().enumerate() {
        assert!(
            index < leading || !matches!(message, Message::System(_)),
            "a System row stands after a conversation row at {index}: {request:?}"
        );
    }
}

fn in_effect(session: &Session, key: &str) -> Option<(u64, String)> {
    let effective = session.effective_operator_instruction_set().unwrap();
    effective.slot(&slot(key)).and_then(|state| {
        state
            .body()
            .map(|body| (state.revision().get(), body.to_string()))
    })
}

fn holds_a_compaction_summary(messages: &[Message]) -> bool {
    messages.iter().any(
        |message| matches!(message, Message::User(user) if user.transcript_role.is_compaction_summary()),
    )
}

#[tokio::test]
async fn a_prefix_only_model_with_a_conversation_takes_a_change_at_its_next_run() {
    let client = Arc::new(RecordingClient::default());
    let mut agent = build_agent(
        conversed_session(),
        &client,
        Some(InstructionLowering::LeadingPrefixOnly),
    )
    .await;
    agent
        .session_mut()
        .set_session_instruction(write("tone", 1, "Answer formally."))
        .unwrap();
    let before = agent.session().messages().to_vec();
    let outcome = agent
        .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
        .unwrap();
    assert_eq!(
        outcome,
        RunStartInstructionOutcome::Applied {
            entry_sequence: 1,
            target_generation: 1,
        },
        "the change applies at the next fresh run start, with conversation rows in the transcript"
    );
    agent.run("next".into()).await.unwrap();

    // What the model was asked with: the entry inside the leading System
    // prefix, the conversation after it, the run's own input last.
    let requests = client.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    let entries = entry_rows(request);
    assert_eq!(entries.len(), 1);
    let Message::System(entry) = entries[0] else {
        unreachable!()
    };
    assert!(entry.content.contains("Answer formally."));
    assert_system_rows_lead(request);
    assert_eq!(
        leading_system_rows(request),
        2,
        "the prompt, then the entry"
    );
    assert_eq!(last_user_text(request).as_deref(), Some("next"));

    // What the session holds: every earlier row as it was, the entry
    // appended after them, and no compaction.
    let stored = agent.session().messages();
    assert_eq!(
        request_json(&stored[..before.len()]),
        request_json(&before),
        "no row the session already held is rewritten or moved"
    );
    assert!(
        is_entry(&stored[before.len()]),
        "the entry is appended at the transcript tail, before the run's input"
    );
    assert!(!holds_a_compaction_summary(stored));
    assert_eq!(
        in_effect(agent.session(), "tone"),
        Some((1, "Answer formally.".to_string()))
    );
}

#[tokio::test]
async fn two_changes_in_a_row_on_a_prefix_only_model_each_apply_at_the_next_run() {
    let client = Arc::new(RecordingClient::default());
    let mut agent = build_agent(
        conversed_session(),
        &client,
        Some(InstructionLowering::LeadingPrefixOnly),
    )
    .await;
    for (revision, body, input) in [
        (1, "Answer formally.", "first"),
        (2, "Answer warmly.", "second"),
    ] {
        agent
            .session_mut()
            .set_session_instruction(write("tone", revision, body))
            .unwrap();
        let outcome = agent
            .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
            .unwrap();
        assert!(
            matches!(outcome, RunStartInstructionOutcome::Applied { .. }),
            "revision {revision}: {outcome:?}"
        );
        agent.run(input.into()).await.unwrap();
        agent.conclude_run_start_operator_instructions().unwrap();
    }
    let requests = client.requests();
    let second = requests.last().unwrap();
    assert_system_rows_lead(second);
    let entries = entry_rows(second);
    assert_eq!(
        entries.len(),
        2,
        "one entry per change, both in the request"
    );
    let sequences: Vec<u64> = entries
        .iter()
        .map(|entry| {
            let Message::System(entry) = entry else {
                unreachable!()
            };
            entry.instruction_entry.as_ref().unwrap().entry_sequence()
        })
        .collect();
    assert_eq!(sequences, [1, 2], "the entries keep their order");
    assert_eq!(
        in_effect(agent.session(), "tone"),
        Some((2, "Answer warmly.".to_string()))
    );

    // Two more changes before one run are one entry, as on every model.
    for (key, revision, body) in [
        ("tone", 3, "Answer briefly."),
        ("units", 1, "Use metric units."),
    ] {
        agent
            .session_mut()
            .set_session_instruction(write(key, revision, body))
            .unwrap();
    }
    assert!(matches!(
        agent
            .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
            .unwrap(),
        RunStartInstructionOutcome::Applied { .. }
    ));
    agent.run("third".into()).await.unwrap();
    let requests = client.requests();
    let third = requests.last().unwrap();
    assert_system_rows_lead(third);
    assert_eq!(
        entry_rows(third).len(),
        3,
        "two changes coalesced into one entry"
    );
    assert_eq!(
        in_effect(agent.session(), "tone"),
        Some((3, "Answer briefly.".to_string()))
    );
    assert_eq!(
        in_effect(agent.session(), "units"),
        Some((1, "Use metric units.".to_string()))
    );
    // The transcript holds each entry where its run started.
    let stored = agent.session().messages();
    let stored_entries: Vec<usize> = stored
        .iter()
        .enumerate()
        .filter_map(|(index, message)| is_entry(message).then_some(index))
        .collect();
    assert_eq!(stored_entries, [3, 6, 9], "{stored:?}");
}

/// A resumed run applies nothing, on this model as on every other; the next
/// fresh run start applies the change, also in a session a new process read
/// back from its store.
#[tokio::test]
async fn a_resumed_prefix_only_session_takes_a_change_at_its_next_fresh_run() {
    let client = Arc::new(RecordingClient::default());
    let mut agent = build_agent(
        conversed_session(),
        &client,
        Some(InstructionLowering::LeadingPrefixOnly),
    )
    .await;
    agent
        .session_mut()
        .set_session_instruction(write("tone", 1, "Answer formally."))
        .unwrap();
    for run_start in [InstructionRunStart::ResumeRun, InstructionRunStart::NoRun] {
        assert_eq!(
            agent
                .apply_run_start_operator_instructions(run_start, None)
                .unwrap(),
            RunStartInstructionOutcome::NotNeeded,
            "{run_start:?}"
        );
    }
    assert!(entry_rows(agent.session().messages()).is_empty());

    // The session as a store keeps it, read back by a new process.
    let bytes = agent.session().to_persisted_bytes().unwrap();
    let reloaded = Session::from_persisted_bytes(&bytes).unwrap();
    let resumed_client = Arc::new(RecordingClient::default());
    let mut resumed = build_agent(
        reloaded,
        &resumed_client,
        Some(InstructionLowering::LeadingPrefixOnly),
    )
    .await;
    assert!(
        matches!(
            resumed
                .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
                .unwrap(),
            RunStartInstructionOutcome::Applied { .. }
        ),
        "the reloaded session's next fresh run start applies the pending change"
    );
    resumed.run("after the restart".into()).await.unwrap();
    let request = &resumed_client.requests()[0];
    assert_system_rows_lead(request);
    assert_eq!(entry_rows(request).len(), 1);
    assert_eq!(
        in_effect(resumed.session(), "tone"),
        Some((1, "Answer formally.".to_string()))
    );

    // Read back once more, the applied entry is not applied again and the
    // request is the same.
    let bytes = resumed.session().to_persisted_bytes().unwrap();
    let again = Session::from_persisted_bytes(&bytes).unwrap();
    let again_client = Arc::new(RecordingClient::default());
    let mut again = build_agent(
        again,
        &again_client,
        Some(InstructionLowering::LeadingPrefixOnly),
    )
    .await;
    assert_eq!(
        again
            .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
            .unwrap(),
        RunStartInstructionOutcome::NotNeeded
    );
    again.run("once more".into()).await.unwrap();
    let request = &again_client.requests()[0];
    assert_system_rows_lead(request);
    assert_eq!(entry_rows(request).len(), 1);
}

/// A fork child copies its parent's entry row as history: the row is not in
/// effect in the child and its request leaves it out. The child's own change
/// applies at the child's next run and is placed like any own entry.
#[tokio::test]
async fn a_fork_of_a_prefix_only_session_takes_its_own_change_at_its_next_run() {
    let client = Arc::new(RecordingClient::default());
    let mut parent = build_agent(
        conversed_session(),
        &client,
        Some(InstructionLowering::LeadingPrefixOnly),
    )
    .await;
    parent
        .session_mut()
        .set_session_instruction(write("tone", 1, "Answer formally."))
        .unwrap();
    assert!(matches!(
        parent
            .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
            .unwrap(),
        RunStartInstructionOutcome::Applied { .. }
    ));
    parent.run("first".into()).await.unwrap();
    let parent_rows = request_json(parent.session().messages());

    let child = parent.session().fork();
    assert_eq!(entry_rows(child.messages()).len(), 1, "the copied row");
    assert_eq!(
        in_effect(&child, "tone"),
        None,
        "a copied row is not in effect in the child"
    );
    let child_client = Arc::new(RecordingClient::default());
    let mut child = build_agent(
        child,
        &child_client,
        Some(InstructionLowering::LeadingPrefixOnly),
    )
    .await;
    child
        .session_mut()
        .set_session_instruction(write("tone", 1, "Answer briefly."))
        .unwrap();
    assert!(matches!(
        child
            .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
            .unwrap(),
        RunStartInstructionOutcome::Applied { .. }
    ));
    child.run("in the fork".into()).await.unwrap();

    let request = &child_client.requests()[0];
    assert_system_rows_lead(request);
    let entries = entry_rows(request);
    assert_eq!(
        entries.len(),
        1,
        "the child's own entry; the copied row is left out: {request:?}"
    );
    let Message::System(entry) = entries[0] else {
        unreachable!()
    };
    assert!(entry.content.contains("Answer briefly."));
    assert!(!request_json(request).contains("Answer formally."));
    assert_eq!(
        in_effect(child.session(), "tone"),
        Some((1, "Answer briefly.".to_string()))
    );
    assert_eq!(
        request_json(parent.session().messages()),
        parent_rows,
        "the parent is untouched"
    );
}

/// A rewind is a fork of the session at an earlier boundary. The child takes
/// no recorded target from its source, neither the revision the source had
/// applied nor the one it still held pending: the source's target is session
/// authority, which a fork does not copy. What the child applies is what its
/// owner records in it afterwards.
#[tokio::test]
async fn a_rewind_child_takes_no_recorded_target_and_applies_what_is_recorded_in_it() {
    let mut source = conversed_session();
    source
        .set_session_instruction(write("tone", 1, "Answer formally."))
        .unwrap();
    assert!(matches!(
        source
            .apply_run_start_instructions(
                InstructionRunStart::FreshRun,
                InstructionLowering::MidConversation,
                None,
            )
            .unwrap(),
        RunStartInstructionOutcome::Applied { .. }
    ));
    source.push(Message::User(UserMessage::text("later")));
    source
        .set_session_instruction(write("tone", 2, "Answer warmly."))
        .unwrap();
    let source_state = source.instruction_target_state().unwrap().unwrap();
    assert_eq!(source_state.target_generation(), 2);
    assert!(source_state.slots()[0].pending_body.is_some());

    // Before the entry: the child copies no entry row and records no state.
    let before_the_entry = source.fork_at_complete_boundary(3).unwrap();
    assert!(entry_rows(before_the_entry.messages()).is_empty());
    assert_eq!(before_the_entry.instruction_target_state().unwrap(), None);
    assert_eq!(in_effect(&before_the_entry, "tone"), None);

    // After the entry: the child copies the row and records an unowned,
    // empty target; nothing is pending and nothing is in effect.
    let after_the_entry = source.fork_at_complete_boundary(4).unwrap();
    assert_eq!(entry_rows(after_the_entry.messages()).len(), 1);
    let state = after_the_entry
        .instruction_target_state()
        .unwrap()
        .expect("a child that copied an entry row records the state key");
    assert_eq!(
        state.owner(),
        &meerkat_core::InstructionTargetOwner::Unowned
    );
    assert_eq!(state.target_generation(), 0);
    assert!(
        state.slots().is_empty(),
        "no recorded target, pending or applied"
    );
    assert_eq!(in_effect(&after_the_entry, "tone"), None);

    // The child's own target is the one recorded in it; on a model that
    // carries System rows only in the leading prefix it applies at the
    // child's next run.
    for child in [before_the_entry, after_the_entry] {
        let client = Arc::new(RecordingClient::default());
        let mut agent =
            build_agent(child, &client, Some(InstructionLowering::LeadingPrefixOnly)).await;
        agent
            .session_mut()
            .set_session_instruction(write("tone", 1, "Answer warmly."))
            .unwrap();
        assert!(matches!(
            agent
                .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
                .unwrap(),
            RunStartInstructionOutcome::Applied { .. }
        ));
        agent.run("after the rewind".into()).await.unwrap();
        let request = &client.requests()[0];
        assert_system_rows_lead(request);
        assert_eq!(entry_rows(request).len(), 1, "{request:?}");
        assert!(!request_json(request).contains("Answer formally."));
        assert_eq!(
            in_effect(agent.session(), "tone"),
            Some((1, "Answer warmly.".to_string()))
        );
    }
}

/// The stored form of a session of such a model after an application is the
/// form a model that carries System rows anywhere has always left: the same
/// rows in the same order and the same recorded state. No stored byte takes
/// a new form; only the model request differs. An older release reads the
/// session, and its request, which is the unplaced projection, holds the
/// entry after the conversation rows.
#[tokio::test]
async fn the_stored_form_of_an_applied_change_is_the_one_every_model_leaves() {
    let source = conversed_session();
    let mut stored = Vec::new();
    for lowering in [
        InstructionLowering::MidConversation,
        InstructionLowering::LeadingPrefixOnly,
    ] {
        // Two sessions with the same rows: the same bytes, read twice.
        let session = Session::from_persisted_bytes(&source.to_persisted_bytes().unwrap()).unwrap();
        let client = Arc::new(RecordingClient::default());
        let mut agent = build_agent(session, &client, Some(lowering)).await;
        agent
            .session_mut()
            .set_session_instruction(write("tone", 1, "Answer formally."))
            .unwrap();
        assert!(
            matches!(
                agent
                    .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
                    .unwrap(),
                RunStartInstructionOutcome::Applied { .. }
            ),
            "{lowering:?}"
        );
        let bytes = agent.session().to_persisted_bytes().unwrap();
        let read_back = Session::from_persisted_bytes(&bytes).unwrap();
        let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        stored.push((lowering, read_back, document));
    }
    let (_, mid, mid_document) = &stored[0];
    let (_, prefix_only, prefix_only_document) = &stored[1];

    // The same document shape: version and top-level keys.
    assert_eq!(mid_document["version"], prefix_only_document["version"]);
    let keys = |document: &serde_json::Value| {
        document
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(keys(mid_document), keys(prefix_only_document));
    // The same rows in the same order: the entry is the last row of both.
    let shape = |session: &Session| {
        session
            .messages()
            .iter()
            .map(|message| match message {
                Message::System(system) => match system.instruction_entry.as_ref() {
                    Some(identity) => {
                        format!("entry {} {}", identity.entry_sequence(), system.content)
                    }
                    None => format!("system {}", system.content),
                },
                Message::User(user) => format!("user {}", user.text_content()),
                other => format!("{:?}", std::mem::discriminant(other)),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(shape(mid), shape(prefix_only));
    assert!(is_entry(prefix_only.messages().last().unwrap()));
    // The same recorded state.
    assert_eq!(
        mid.instruction_target_state().unwrap(),
        prefix_only.instruction_target_state().unwrap()
    );
    // What a release that does not place entries builds its request from.
    let unplaced = prefix_only.messages_for_model_boundary().unwrap();
    let entry_index = unplaced.iter().position(is_entry).unwrap();
    assert!(
        entry_index >= leading_system_rows(&unplaced),
        "the stored order is the transcript's: the entry follows the conversation rows"
    );
    assert_eq!(
        in_effect(prefix_only, "tone"),
        Some((1, "Answer formally.".to_string()))
    );
}

/// A session that applied a change on a model that carries System rows
/// anywhere can be switched to one that carries them only in the leading
/// prefix: the request places the session's own entries there. A wire that
/// carries one leading System row still cannot take them.
#[tokio::test]
async fn a_model_change_to_a_prefix_only_target_carries_the_sessions_own_entries() {
    let config = backend_realms_config();
    let registry = Arc::new(backend_test_registry(&config));
    let client = Arc::new(RecordingClient::default());
    let mut agent = AgentBuilder::new()
        .resume_session(conversed_session_served_by(&openai_identity(Some(
            configured_binding("public_realm", "default_openai"),
        ))))
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .with_effective_model_registry(Arc::clone(&registry))
        .with_operator_instruction_lowering(InstructionLowering::MidConversation)
        .build_standalone(Arc::clone(&client), Arc::new(NoTools), Arc::new(NoopStore))
        .await;
    agent
        .session_mut()
        .set_session_instruction(write("tone", 1, "Answer formally."))
        .unwrap();
    assert!(matches!(
        agent
            .apply_run_start_operator_instructions(InstructionRunStart::FreshRun, None)
            .unwrap(),
        RunStartInstructionOutcome::Applied { .. }
    ));
    agent.run("first".into()).await.unwrap();
    agent.conclude_run_start_operator_instructions().unwrap();

    let swap = |agent: &mut TestAgent, identity: meerkat_core::SessionLlmIdentity| {
        let request_policy = meerkat_core::SessionLlmRequestPolicy {
            model: identity.model.clone(),
            credential_identity: None,
            provider_params: None,
            provider_tool_defaults: None,
            provider_native_tools: meerkat_core::ProviderNativeToolPolicy::Inherit,
            operator_instruction_lowering: registry
                .instruction_lowering_for_identity(&config, &identity),
        };
        agent.hot_swap_llm_identity(Arc::clone(&client), identity, request_policy)
    };

    // A wire that carries one leading System row: refused, as before.
    let refused = swap(&mut agent, openai_identity(Some(chatgpt_binding())));
    assert!(
        refused.is_err(),
        "an entry beside the prompt is not carried by a one-row wire"
    );

    // A model that carries System rows only in the leading prefix: carried.
    let anthropic = llm_identity(
        meerkat_core::Provider::Anthropic,
        crate::model_profile::test_catalog::ANTHROPIC_MODEL,
        Some(configured_binding("public_realm", "default_anthropic")),
    );
    let swapped = swap(&mut agent, anthropic);
    assert!(
        swapped.is_ok(),
        "a prefix-only target carries the session's own entries: {swapped:?}"
    );
    assert_eq!(
        agent.operator_instruction_lowering(),
        InstructionLowering::LeadingPrefixOnly
    );
    agent.run("second".into()).await.unwrap();
    let requests = client.requests();
    let request = requests.last().unwrap();
    assert_system_rows_lead(request);
    assert_eq!(entry_rows(request).len(), 1);
    assert_eq!(last_user_text(request).as_deref(), Some("second"));
}
