//! Rows for operator instructions at the session task: a write that
//! arrives while a turn runs waits for the turn (it never lands inside it),
//! and the next fresh run start appends the one entry before the run's own
//! input. An ephemeral service keeps no durable session document, so its
//! public operator-instruction surface refuses `durability_unavailable`
//! (a mob member's snapshot is recorded only by the mob runtime's sealed
//! delivery, not through this surface).

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex as StdMutex};

use super::{
    AgentError, AgentEvent, ContentInput, CreateSessionRequest, EphemeralSessionService,
    ObservedSessionTailKind, RunResult, SessionAgent, SessionAgentBuilder, SessionAgentTurnInput,
    SessionError, SessionId, SessionSnapshot, SessionTranscriptAuthoritySnapshot, SystemTime,
    TurnToolOverlay, Usage, mpsc,
};
use async_trait::async_trait;
use meerkat_core::service::{
    DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions, SessionInstructionError,
    SessionService, SessionServiceInstructionExt, StartTurnRequest, StartTurnRuntimeSemantics,
};
use meerkat_core::types::{InstructionKey, InstructionNamespace, Message};
use meerkat_core::{
    InstructionLowering, InstructionRevision, InstructionRunStart, InstructionSetErrorCode,
    InstructionSlot, InstructionWriteDisposition, RunStartInstructionOutcome,
    SessionInstructionChange, SessionInstructionWrite, SessionLlmIdentity,
};

/// What every run of the probe agent saw as its request, and the gate a run
/// waits on while it is held.
struct RunProbe {
    requests: StdMutex<Vec<Vec<Message>>>,
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
    gated: std::sync::atomic::AtomicBool,
    /// The next run is refused before it starts, as a run-start veto or a
    /// refusal of the turn's input refuses it.
    refuse_next_run: std::sync::atomic::AtomicBool,
    /// The next System append of a turn fails.
    fail_next_system_append: std::sync::atomic::AtomicBool,
    /// The next run fails once it started: its model read the request, then
    /// the call fails.
    fail_next_run_after_start: std::sync::atomic::AtomicBool,
}

impl Default for RunProbe {
    fn default() -> Self {
        Self {
            requests: StdMutex::default(),
            entered: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
            gated: std::sync::atomic::AtomicBool::default(),
            refuse_next_run: std::sync::atomic::AtomicBool::default(),
            fail_next_system_append: std::sync::atomic::AtomicBool::default(),
            fail_next_run_after_start: std::sync::atomic::AtomicBool::default(),
        }
    }
}

impl RunProbe {
    fn requests(&self) -> Vec<Vec<Message>> {
        self.requests.lock().expect("requests").clone()
    }

    fn set_gated(&self, gated: bool) {
        self.gated.store(gated, std::sync::atomic::Ordering::SeqCst);
    }

    fn refuse_next_run(&self) {
        self.refuse_next_run
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn fail_next_system_append(&self) {
        self.fail_next_system_append
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn fail_next_run_after_start(&self) {
        self.fail_next_run_after_start
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[derive(Clone)]
struct ProbeBuilder {
    probe: Arc<RunProbe>,
}

struct ProbeAgent {
    session_id: SessionId,
    session: meerkat_core::Session,
    identity: SessionLlmIdentity,
    probe: Arc<RunProbe>,
    transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle,
    /// What the last run start's entry changed, until the session task
    /// concludes the run start.
    run_start_application: Option<meerkat_core::RunStartInstructionApplication>,
    /// The run of the last run start started: the agent's own fact.
    run_started: bool,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionAgentBuilder for ProbeBuilder {
    type Agent = ProbeAgent;

    fn session_comms_identity_root(&self) -> meerkat_core::SessionCommsIdentityRoot {
        meerkat_core::SessionCommsIdentityRoot::NotKept
    }

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, SessionError> {
        let session = req
            .build
            .as_ref()
            .and_then(|build| build.resume_session.clone())
            .unwrap_or_default();
        Ok(ProbeAgent {
            session_id: session.id().clone(),
            session,
            identity: SessionLlmIdentity {
                model: req.model.clone(),
                provider: meerkat_core::Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
            probe: Arc::clone(&self.probe),
            transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle::new(),
            run_start_application: None,
            run_started: false,
        })
    }
}

impl ProbeAgent {
    fn ok_result(&self) -> RunResult {
        RunResult {
            text: "ok".to_string(),
            session_id: self.session_id.clone(),
            usage: Usage::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: None,
            run_usage: None,
            request_usage: Vec::new(),
        }
    }

    /// One run: its input row, its request (what the model would read),
    /// the held gate, then the answer. A refused run starts nothing; a run
    /// that fails after it started fails once its model read the request.
    async fn run(&mut self, prompt: String) -> Result<RunResult, AgentError> {
        if self
            .probe
            .refuse_next_run
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(AgentError::ConfigError(
                "the run is refused before it starts".to_string(),
            ));
        }
        self.run_started = true;
        self.session
            .push(Message::User(meerkat_core::types::UserMessage::text(
                prompt,
            )));
        self.probe
            .requests
            .lock()
            .expect("requests")
            .push(self.session.messages().to_vec());
        self.probe.entered.add_permits(1);
        if self.probe.gated.load(std::sync::atomic::Ordering::SeqCst) {
            self.probe
                .release
                .acquire()
                .await
                .expect("the release gate is never closed")
                .forget();
        }
        if self
            .probe
            .fail_next_run_after_start
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(AgentError::InternalError(
                "the model call fails after the run started".to_string(),
            ));
        }
        self.session.push(Message::BlockAssistant(
            meerkat_core::types::BlockAssistantMessage {
                blocks: vec![meerkat_core::types::AssistantBlock::Text {
                    text: "ok".to_string(),
                    meta: None,
                }],
                stop_reason: Some(meerkat_core::types::StopReason::EndTurn),
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                created_at: meerkat_core::types::message_timestamp_now(),
                assistant_message_id: None,
            },
        ));
        Ok(self.ok_result())
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionAgent for ProbeAgent {
    async fn run_with_events(
        &mut self,
        prompt: ContentInput,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        self.run(prompt.text_content()).await
    }

    async fn run_turn_with_events(
        &mut self,
        input: SessionAgentTurnInput,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        self.run(input.prompt.text_content()).await
    }

    fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

    fn set_turn_tool_overlay(
        &mut self,
        _overlay: Option<TurnToolOverlay>,
    ) -> Result<(), AgentError> {
        Ok(())
    }

    fn hot_swap_llm_identity(
        &mut self,
        _client: Arc<dyn meerkat_core::AgentLlmClient>,
        _identity: SessionLlmIdentity,
        _request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), AgentError> {
        Ok(())
    }

    fn cancel(&mut self) {}

    fn session_id(&self) -> SessionId {
        self.session_id.clone()
    }

    fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            created_at: SystemTime::now(),
            updated_at: SystemTime::now(),
            message_count: self.session.messages().len(),
            total_tokens: 0,
            usage: Usage::default(),
            last_assistant_text: None,
        }
    }

    fn session_clone(&self) -> Result<meerkat_core::Session, AgentError> {
        Ok(self.session.clone())
    }

    fn session_transcript_authority(
        &self,
    ) -> Result<SessionTranscriptAuthoritySnapshot, AgentError> {
        SessionTranscriptAuthoritySnapshot::from_session(&self.session)
    }

    fn append_system_messages(&mut self, contents: Vec<String>) -> Result<(), AgentError> {
        if self
            .probe
            .fail_next_system_append
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(AgentError::ConfigError(
                "the turn's System append is refused".to_string(),
            ));
        }
        for content in contents {
            self.session.append_system_message(content);
        }
        Ok(())
    }

    fn observed_session_tail(&self) -> ObservedSessionTailKind {
        meerkat_core::pending_continuation::observe_session_tail(self.session.messages())
    }

    fn durable_llm_identity(&self) -> Option<SessionLlmIdentity> {
        Some(self.identity.clone())
    }

    fn transient_turn_context_state(&self) -> meerkat_core::TransientTurnContextStateHandle {
        self.transient_turn_context_state.clone()
    }

    fn set_session_instruction_control(
        &mut self,
        write: SessionInstructionWrite,
    ) -> Result<meerkat_core::SessionInstructionReceipt, SessionInstructionError> {
        self.session
            .set_session_instruction(write)
            .map_err(SessionInstructionError::from)
    }

    fn apply_run_start_operator_instructions(
        &mut self,
        run_start: InstructionRunStart,
        run_id: Option<meerkat_core::lifecycle::RunId>,
    ) -> Result<RunStartInstructionOutcome, AgentError> {
        let (outcome, application) = self
            .session
            .apply_run_start_instructions_reversibly(
                run_start,
                InstructionLowering::MidConversation,
                run_id,
            )
            .map_err(|error| AgentError::InternalError(error.to_string()))?;
        self.run_start_application = application;
        self.run_started = false;
        Ok(outcome)
    }

    fn conclude_run_start_operator_instructions(&mut self) -> Result<bool, AgentError> {
        let Some(application) = self.run_start_application.take() else {
            return Ok(false);
        };
        if self.run_started {
            return Ok(false);
        }
        self.session
            .revert_run_start_instructions(application)
            .map_err(AgentError::InternalError)?;
        Ok(true)
    }
}

fn create_request() -> CreateSessionRequest {
    CreateSessionRequest {
        injected_context: Vec::new(),
        model: "operator-instruction-probe".to_string(),
        prompt: ContentInput::Text(String::new()),
        system_prompt: meerkat_core::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: InitialTurnPolicy::Defer,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
        build: Some(SessionBuildOptions::default()),
        labels: None,
    }
}

fn turn(prompt: &str) -> StartTurnRequest {
    StartTurnRequest {
        prompt: prompt.into(),
        injected_context: Vec::new(),
        system_prompt: None,
        event_tx: None,
        runtime: StartTurnRuntimeSemantics::default(),
    }
}

fn write_tone() -> SessionInstructionWrite {
    SessionInstructionWrite {
        slot: InstructionSlot {
            namespace: InstructionNamespace::new("app.example").expect("namespace"),
            key: InstructionKey::new("tone").expect("key"),
        },
        revision: InstructionRevision::new(1).expect("revision"),
        change: SessionInstructionChange::Activate {
            body: "Answer formally.".to_string(),
            content_sha256: None,
        },
    }
}

fn is_entry(message: &Message) -> bool {
    matches!(message, Message::System(system) if system.instruction_entry.is_some())
}

#[tokio::test]
async fn write_during_a_turn_leaves_its_requests_unchanged_and_applies_at_the_next_run_start() {
    let probe = Arc::new(RunProbe::default());
    probe.set_gated(true);
    let service = Arc::new(EphemeralSessionService::new(
        ProbeBuilder {
            probe: Arc::clone(&probe),
        },
        2,
    ));
    let id = service
        .create_session(create_request())
        .await
        .expect("create a deferred session")
        .session_id;

    let first = tokio::spawn({
        let service = Arc::clone(&service);
        let id = id.clone();
        async move { service.start_turn(&id, turn("first")).await }
    });
    probe
        .entered
        .acquire()
        .await
        .expect("the first run starts")
        .forget();

    let written = tokio::spawn({
        let service = Arc::clone(&service);
        let id = id.clone();
        async move {
            service
                .set_session_instruction_control(&id, write_tone())
                .await
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        !written.is_finished(),
        "a write never lands inside a running turn"
    );

    probe.set_gated(false);
    probe.release.add_permits(1);
    first
        .await
        .expect("join the first turn")
        .expect("the first turn completes");
    let receipt = written
        .await
        .expect("join the write")
        .expect("the write records once the turn ends");
    assert_eq!(receipt.disposition, InstructionWriteDisposition::Recorded);

    let requests = probe.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        !requests[0].iter().any(is_entry),
        "the in-flight turn's request is unchanged by the write"
    );

    service
        .start_turn(&id, turn("second"))
        .await
        .expect("the second turn completes");
    let requests = probe.requests();
    let second = requests.last().expect("the second request");
    let entry_index = second
        .iter()
        .position(is_entry)
        .expect("the next fresh run start applies the write");
    assert_eq!(
        entry_index + 2,
        second.len(),
        "the entry immediately precedes the run's own input"
    );
    assert_eq!(second.iter().filter(|message| is_entry(message)).count(), 1);
}

#[tokio::test]
async fn ephemeral_service_refuses_durability_unavailable() {
    let probe = Arc::new(RunProbe::default());
    let service = Arc::new(EphemeralSessionService::new(ProbeBuilder { probe }, 1));
    let id = service
        .create_session(create_request())
        .await
        .expect("create a deferred session")
        .session_id;

    let refused = Arc::clone(&service)
        .set_session_instruction(&id, write_tone())
        .await
        .expect_err("an ephemeral session keeps no durable target");
    assert!(matches!(
        refused,
        SessionInstructionError::DurabilityUnavailable
    ));
    assert_eq!(
        refused.code(),
        Some(InstructionSetErrorCode::DurabilityUnavailable)
    );

    assert!(matches!(
        service.read_session_instructions(&id).await,
        Err(SessionInstructionError::DurabilityUnavailable)
    ));
}

/// A turn whose run never starts leaves no operator-instruction entry
/// naming it: a run refused before it starts, and a turn whose own System
/// append fails after its run start. The change stays pending, and the
/// next fresh run start applies it once, right before the run's own input.
#[tokio::test]
async fn a_turn_whose_run_never_starts_leaves_no_entry_and_the_next_run_start_applies_it() {
    let probe = Arc::new(RunProbe::default());
    let service = EphemeralSessionService::new(
        ProbeBuilder {
            probe: Arc::clone(&probe),
        },
        1,
    );
    let id = service
        .create_session(create_request())
        .await
        .expect("create a deferred session")
        .session_id;
    let receipt = service
        .set_session_instruction_control(&id, write_tone())
        .await
        .expect("the write records");
    assert_eq!(receipt.disposition, InstructionWriteDisposition::Recorded);

    probe.refuse_next_run();
    service
        .start_turn(&id, turn("refused"))
        .await
        .expect_err("the run is refused before it starts");
    let session = service.export_session(&id).await.expect("export");
    assert!(
        !session.messages().iter().any(is_entry),
        "no entry names a run that never started: {:?}",
        session.messages()
    );

    probe.fail_next_system_append();
    let mut with_system_prompt = turn("system append refused");
    with_system_prompt.system_prompt = Some("Keep answers short.".to_string());
    service
        .start_turn(&id, with_system_prompt)
        .await
        .expect_err("the turn's System append fails");
    let session = service.export_session(&id).await.expect("export");
    assert!(
        !session.messages().iter().any(is_entry),
        "no entry names a run that never started: {:?}",
        session.messages()
    );
    assert!(probe.requests().is_empty(), "no run reached the model");

    service
        .start_turn(&id, turn("next"))
        .await
        .expect("the next turn runs");
    let requests = probe.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    let entry_index = request
        .iter()
        .position(is_entry)
        .expect("the next fresh run start applies the pending change");
    assert_eq!(
        entry_index + 2,
        request.len(),
        "the entry immediately precedes the run's own input"
    );
    assert_eq!(
        request.iter().filter(|message| is_entry(message)).count(),
        1
    );
    let Message::System(entry) = &request[entry_index] else {
        unreachable!()
    };
    assert!(entry.content.contains("Answer formally."));
    let session = service.export_session(&id).await.expect("export");
    assert_eq!(
        session
            .messages()
            .iter()
            .filter(|message| is_entry(message))
            .count(),
        1
    );
}

/// A turn whose run started and then failed keeps the operator-instruction
/// entry naming it: its model already read the entry. The agent reports
/// whether its run started; the session task does not read that from the
/// event stream, where this agent publishes nothing. The change is in
/// effect, so the next run start applies nothing new.
#[tokio::test]
async fn a_turn_whose_run_started_and_then_failed_keeps_its_entry() {
    let probe = Arc::new(RunProbe::default());
    let service = EphemeralSessionService::new(
        ProbeBuilder {
            probe: Arc::clone(&probe),
        },
        1,
    );
    let id = service
        .create_session(create_request())
        .await
        .expect("create a deferred session")
        .session_id;
    let receipt = service
        .set_session_instruction_control(&id, write_tone())
        .await
        .expect("the write records");
    assert_eq!(receipt.disposition, InstructionWriteDisposition::Recorded);

    probe.fail_next_run_after_start();
    service
        .start_turn(&id, turn("fails"))
        .await
        .expect_err("the run fails after it started");
    let requests = probe.requests();
    assert_eq!(requests.len(), 1, "the failed run reached its model");
    assert_eq!(
        requests[0]
            .iter()
            .filter(|message| is_entry(message))
            .count(),
        1,
        "the model read the entry"
    );
    let session = service.export_session(&id).await.expect("export");
    assert_eq!(
        session
            .messages()
            .iter()
            .filter(|message| is_entry(message))
            .count(),
        1,
        "the entry the model read is kept: {:?}",
        session.messages()
    );

    service
        .start_turn(&id, turn("next"))
        .await
        .expect("the next turn runs");
    let requests = probe.requests();
    let next = requests.last().expect("the next request");
    assert_eq!(
        next.iter().filter(|message| is_entry(message)).count(),
        1,
        "the change applied once: the next run start applies nothing new"
    );
}
