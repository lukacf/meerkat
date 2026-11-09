//! A deferred session's staged create-time prompt on its first later turn
//! (`DeferredPromptPolicy::Stage`).
//!
//! A runtime-authored turn commits only its typed appends, so the session
//! task must lower the staged prompt into them: folded into the first User
//! append (staged content first), or inserted as a User append after the
//! leading InjectedContext appends when the turn has none. A turn whose input
//! carries directed interaction custody is refused while the prompt is
//! pending, and a turn that delivers nothing leaves the prompt pending, so
//! the prompt lands exactly once.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex as StdMutex};

use super::{
    AgentError, AgentEvent, ContentInput, CreateSessionRequest, EphemeralSessionService,
    ObservedSessionTailKind, RunResult, SessionAgent, SessionAgentBuilder, SessionAgentTurnInput,
    SessionError, SessionId, SessionSnapshot, SessionTranscriptAuthoritySnapshot, SystemTime,
    TurnToolOverlay, Usage, lock_deferred_turn_state, mpsc,
};
use async_trait::async_trait;
use meerkat_core::SessionLlmIdentity;
use meerkat_core::interaction::InteractionId;
use meerkat_core::lifecycle::RuntimeExecutionKind;
use meerkat_core::lifecycle::run_primitive::{
    ConversationAppend, ConversationAppendRole, CoreRenderable, RuntimeTurnMetadata,
    model_projection_content_input_from_conversation_appends,
};
use meerkat_core::service::{
    DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions, SessionService, StartTurnRequest,
    StartTurnRuntimeSemantics,
};
use meerkat_core::types::{HandlingMode, SystemNoticeKind};

const STAGED: &str = "Remember the codeword ORBIT-7 for later.";

/// One turn as the agent received it: the provider prompt and the typed
/// appends (role and rendered text), in order.
#[derive(Debug, Clone, PartialEq)]
struct ObservedTurn {
    prompt: String,
    appends: Vec<(ConversationAppendRole, String)>,
}

/// What the probe agent's next content run does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunBehavior {
    /// Publish `RunStarted`, commit one row per append, and succeed.
    Commit,
    /// Fail before the run starts: no `RunStarted`, nothing committed (the
    /// shape of a run-start hook veto).
    RefuseBeforeStart,
}

#[derive(Clone)]
struct ProbeBuilder {
    observed: Arc<StdMutex<Vec<ObservedTurn>>>,
    pending_runs: Arc<StdMutex<usize>>,
    behavior: Arc<StdMutex<RunBehavior>>,
    tail: Arc<StdMutex<ObservedSessionTailKind>>,
}

impl ProbeBuilder {
    fn new() -> Self {
        Self {
            observed: Arc::new(StdMutex::new(Vec::new())),
            pending_runs: Arc::new(StdMutex::new(0)),
            behavior: Arc::new(StdMutex::new(RunBehavior::Commit)),
            tail: Arc::new(StdMutex::new(ObservedSessionTailKind::Empty)),
        }
    }

    fn observed(&self) -> Vec<ObservedTurn> {
        self.observed.lock().expect("observed turns").clone()
    }

    fn set_behavior(&self, behavior: RunBehavior) {
        *self.behavior.lock().expect("behavior") = behavior;
    }

    fn set_tail(&self, tail: ObservedSessionTailKind) {
        *self.tail.lock().expect("tail") = tail;
    }

    fn pending_runs(&self) -> usize {
        *self.pending_runs.lock().expect("pending runs")
    }
}

struct ProbeAgent {
    session_id: SessionId,
    session: meerkat_core::Session,
    identity: SessionLlmIdentity,
    probe: ProbeBuilder,
    transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle,
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
            probe: self.clone(),
            transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle::new(),
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
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionAgent for ProbeAgent {
    async fn run_with_events(
        &mut self,
        prompt: ContentInput,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        self.probe
            .observed
            .lock()
            .expect("observed turns")
            .push(ObservedTurn {
                prompt: prompt.text_content(),
                appends: Vec::new(),
            });
        self.session.push(meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::text(prompt.text_content()),
        ));
        Ok(self.ok_result())
    }

    async fn run_turn_with_events(
        &mut self,
        input: SessionAgentTurnInput,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        self.probe
            .observed
            .lock()
            .expect("observed turns")
            .push(ObservedTurn {
                prompt: input.prompt.text_content(),
                appends: input
                    .typed_turn_appends
                    .iter()
                    .map(|append| (append.role, append.content.render_text()))
                    .collect(),
            });
        let behavior = *self.probe.behavior.lock().expect("behavior");
        if behavior == RunBehavior::RefuseBeforeStart {
            return Err(AgentError::ConfigError(
                "probe refused the run before it started".to_string(),
            ));
        }
        let _ = event_tx
            .send(AgentEvent::RunStarted {
                session_id: self.session_id.clone(),
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                input: input.prompt.clone().into(),
                request_reasoning: None,
            })
            .await;
        if input.typed_turn_appends.is_empty() {
            self.session.push(meerkat_core::types::Message::User(
                meerkat_core::types::UserMessage::text(input.prompt.text_content()),
            ));
        }
        for append in &input.typed_turn_appends {
            self.session.push(meerkat_core::types::Message::User(
                meerkat_core::types::UserMessage::text(append.content.render_text()),
            ));
        }
        Ok(self.ok_result())
    }

    async fn run_pending_with_events(
        &mut self,
        _transcript_identity: Option<meerkat_core::types::TranscriptMessageIdentity>,
        _execution_kind: Option<RuntimeExecutionKind>,
        _request_contexts: Vec<meerkat_core::lifecycle::TurnRequestContext>,
        _work_authorization: Option<meerkat_core::WorkAuthorizationContext>,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        *self.probe.pending_runs.lock().expect("pending runs") += 1;
        Ok(self.ok_result())
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
        for content in contents {
            self.session.append_system_message(content);
        }
        Ok(())
    }

    fn observed_session_tail(&self) -> ObservedSessionTailKind {
        *self.probe.tail.lock().expect("tail")
    }

    fn durable_llm_identity(&self) -> Option<SessionLlmIdentity> {
        Some(self.identity.clone())
    }

    fn transient_turn_context_state(&self) -> meerkat_core::TransientTurnContextStateHandle {
        self.transient_turn_context_state.clone()
    }
}

async fn staged_session(
    probe: &ProbeBuilder,
) -> (EphemeralSessionService<ProbeBuilder>, SessionId) {
    let service = EphemeralSessionService::new(probe.clone(), 4);
    let created = service
        .create_session(CreateSessionRequest {
            injected_context: Vec::new(),
            model: "staged-prompt-probe".to_string(),
            prompt: ContentInput::Text(STAGED.to_string()),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Stage,
            build: Some(SessionBuildOptions::default()),
            labels: None,
        })
        .await
        .expect("deferred session with a staged prompt");
    (service, created.session_id)
}

fn append(role: ConversationAppendRole, text: &str) -> ConversationAppend {
    ConversationAppend {
        runtime_source: None,
        role,
        content: CoreRenderable::Text {
            text: text.to_string(),
        },
        identity: None,
    }
}

fn notice(text: &str) -> ConversationAppend {
    ConversationAppend {
        runtime_source: None,
        role: ConversationAppendRole::SystemNotice,
        content: CoreRenderable::SystemNotice {
            kind: SystemNoticeKind::Generic,
            body: Some(text.to_string()),
            blocks: Vec::new(),
        },
        identity: None,
    }
}

fn runtime_turn(
    appends: Vec<ConversationAppend>,
    execution_kind: RuntimeExecutionKind,
    directed: bool,
) -> StartTurnRequest {
    let prompt = model_projection_content_input_from_conversation_appends(&appends);
    StartTurnRequest {
        injected_context: Vec::new(),
        prompt,
        system_prompt: None,
        event_tx: None,
        runtime: StartTurnRuntimeSemantics::new(
            HandlingMode::Queue,
            None,
            Some(RuntimeTurnMetadata {
                execution_kind: Some(execution_kind),
                directed_interaction_ids: if directed {
                    vec![InteractionId::new()]
                } else {
                    Vec::new()
                },
                ..Default::default()
            }),
        )
        .with_typed_turn_appends(appends),
    }
}

async fn staged_prompt_pending(
    service: &EphemeralSessionService<ProbeBuilder>,
    id: &SessionId,
) -> Option<String> {
    let state = service
        .deferred_turn_state(id)
        .await
        .expect("live deferred-turn state");
    let guard = lock_deferred_turn_state(&state);
    guard
        .pending_initial_prompt()
        .map(|staged| staged.prompt.text_content())
}

fn rendered(appends: &[ConversationAppend]) -> Vec<(ConversationAppendRole, String)> {
    appends
        .iter()
        .map(|append| (append.role, append.content.render_text()))
        .collect()
}

/// The staged prompt folds into the first User append, staged
/// content first, after the leading InjectedContext rows; the provider prompt
/// is the projection of exactly those final appends.
#[tokio::test]
async fn staged_prompt_folds_into_the_first_user_append_of_a_runtime_turn() {
    let probe = ProbeBuilder::new();
    let (service, id) = staged_session(&probe).await;
    service
        .start_turn(
            &id,
            runtime_turn(
                vec![
                    append(ConversationAppendRole::InjectedContext, "ambient context"),
                    append(ConversationAppendRole::User, "What is the codeword?"),
                    notice("a later notice"),
                ],
                RuntimeExecutionKind::ContentTurn,
                false,
            ),
        )
        .await
        .expect("runtime-authored first turn");

    let expected = vec![
        append(ConversationAppendRole::InjectedContext, "ambient context"),
        append(
            ConversationAppendRole::User,
            &format!("{STAGED}\n\nWhat is the codeword?"),
        ),
        notice("a later notice"),
    ];
    let observed = probe.observed();
    assert_eq!(
        observed,
        vec![ObservedTurn {
            prompt: model_projection_content_input_from_conversation_appends(&expected)
                .text_content(),
            appends: rendered(&expected),
        }],
        "the staged prompt must fold into the first User append, after the leading \
         InjectedContext rows, and the provider prompt must be the projection of exactly \
         the committed appends"
    );
    assert_eq!(staged_prompt_pending(&service, &id).await, None);
}

/// A turn without a User append (a peer or system notice turn)
/// gets one User append carrying the staged prompt, inserted after the
/// leading InjectedContext appends and before every other append.
#[tokio::test]
async fn staged_prompt_inserts_a_user_append_when_the_turn_has_none() {
    let probe = ProbeBuilder::new();
    let (service, id) = staged_session(&probe).await;
    service
        .start_turn(
            &id,
            runtime_turn(
                vec![
                    append(ConversationAppendRole::InjectedContext, "ambient context"),
                    notice("a peer said hello"),
                ],
                RuntimeExecutionKind::ContentTurn,
                false,
            ),
        )
        .await
        .expect("runtime-authored notice turn");

    let expected = vec![
        append(ConversationAppendRole::InjectedContext, "ambient context"),
        append(ConversationAppendRole::User, STAGED),
        notice("a peer said hello"),
    ];
    assert_eq!(
        probe.observed(),
        vec![ObservedTurn {
            prompt: model_projection_content_input_from_conversation_appends(&expected)
                .text_content(),
            appends: rendered(&expected),
        }],
        "the staged prompt must land as one User append after the leading InjectedContext \
         rows, and the notice must stay untouched"
    );
}

/// A turn whose input carries directed interaction custody is refused,
/// typed, before the agent runs; the staged prompt stays pending, and the
/// next plain turn delivers it once.
#[tokio::test]
async fn staged_prompt_refuses_a_directed_turn_and_stays_pending() {
    let probe = ProbeBuilder::new();
    let (service, id) = staged_session(&probe).await;
    let refused = service
        .start_turn(
            &id,
            runtime_turn(
                vec![append(ConversationAppendRole::User, "directed work")],
                RuntimeExecutionKind::ContentTurn,
                true,
            ),
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(SessionError::Agent(
                AgentError::StagedPromptDirectedTurnRefused
            ))
        ),
        "a directed turn must be refused while the staged prompt is pending: {refused:?}"
    );
    assert!(
        probe.observed().is_empty(),
        "the refused directed turn must not reach the agent (no RunStarted)"
    );
    assert_eq!(
        staged_prompt_pending(&service, &id).await.as_deref(),
        Some(STAGED),
        "the refused turn must leave the staged prompt pending"
    );

    service
        .start_turn(
            &id,
            runtime_turn(
                vec![append(ConversationAppendRole::User, "plain work")],
                RuntimeExecutionKind::ContentTurn,
                false,
            ),
        )
        .await
        .expect("plain turn after the refusal");
    assert_eq!(
        probe.observed(),
        vec![ObservedTurn {
            prompt: format!("{STAGED}\n\nplain work"),
            appends: vec![(
                ConversationAppendRole::User,
                format!("{STAGED}\n\nplain work")
            )],
        }],
        "the next plain turn must deliver the staged prompt once"
    );
    assert_eq!(staged_prompt_pending(&service, &id).await, None);
}

/// Independent of the appends: a directed turn whose input projects to
/// no typed appends is refused too, so its `RunStarted` content can never
/// carry the staged text.
#[test]
fn a_directed_turn_without_typed_appends_is_refused_while_a_prompt_is_staged() {
    let staged = ContentInput::Text(STAGED.to_string());
    let lowered = super::lower_staged_initial_prompt(
        Some(&staged),
        ContentInput::Text("directed work".to_string()),
        Vec::new(),
        true,
    );
    assert!(
        matches!(lowered, Err(AgentError::StagedPromptDirectedTurnRefused)),
        "a directed turn must be refused whatever its appends: {lowered:?}"
    );
}

/// A run that fails before it starts (no `RunStarted`, nothing
/// committed, as a run-start hook veto) leaves the staged prompt pending; the
/// retry delivers it exactly once and later turns do not repeat it.
#[tokio::test]
async fn staged_prompt_survives_a_run_that_commits_nothing_and_lands_once() {
    let probe = ProbeBuilder::new();
    let (service, id) = staged_session(&probe).await;
    probe.set_behavior(RunBehavior::RefuseBeforeStart);
    let refused = service
        .start_turn(
            &id,
            runtime_turn(
                vec![append(ConversationAppendRole::User, "first try")],
                RuntimeExecutionKind::ContentTurn,
                false,
            ),
        )
        .await;
    assert!(
        refused.is_err(),
        "the probe refuses the first run: {refused:?}"
    );
    assert_eq!(
        staged_prompt_pending(&service, &id).await.as_deref(),
        Some(STAGED),
        "a run that committed nothing must leave the staged prompt pending"
    );

    probe.set_behavior(RunBehavior::Commit);
    for text in ["retry", "later turn"] {
        service
            .start_turn(
                &id,
                runtime_turn(
                    vec![append(ConversationAppendRole::User, text)],
                    RuntimeExecutionKind::ContentTurn,
                    false,
                ),
            )
            .await
            .expect("committed turn");
    }
    let delivered = probe
        .observed()
        .iter()
        .skip(1)
        .flat_map(|turn| turn.appends.clone())
        .filter(|(_, text)| text.contains("ORBIT-7"))
        .count();
    assert_eq!(
        delivered,
        1,
        "the staged prompt must land exactly once across the retry and later turns: {:?}",
        probe.observed()
    );
    assert_eq!(staged_prompt_pending(&service, &id).await, None);
}

/// A pending continuation commits no user row, so it must not consume the
/// staged prompt: the next content turn delivers it.
#[tokio::test]
async fn pending_continuation_keeps_the_staged_prompt_for_the_next_content_turn() {
    let probe = ProbeBuilder::new();
    let (service, id) = staged_session(&probe).await;
    probe.set_tail(ObservedSessionTailKind::User);
    service
        .start_turn(
            &id,
            runtime_turn(Vec::new(), RuntimeExecutionKind::ResumePending, false),
        )
        .await
        .expect("pending continuation");
    assert_eq!(probe.pending_runs(), 1, "the continuation must run");
    assert_eq!(
        staged_prompt_pending(&service, &id).await.as_deref(),
        Some(STAGED),
        "a pending continuation must leave the staged prompt pending"
    );

    probe.set_tail(ObservedSessionTailKind::BlockAssistant);
    service
        .start_turn(
            &id,
            runtime_turn(
                vec![append(ConversationAppendRole::User, "content turn")],
                RuntimeExecutionKind::ContentTurn,
                false,
            ),
        )
        .await
        .expect("content turn after the continuation");
    assert_eq!(
        probe.observed(),
        vec![ObservedTurn {
            prompt: format!("{STAGED}\n\ncontent turn"),
            appends: vec![(
                ConversationAppendRole::User,
                format!("{STAGED}\n\ncontent turn")
            )],
        }]
    );
}
