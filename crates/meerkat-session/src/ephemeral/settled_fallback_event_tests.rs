//! An event the agent publishes while its turn settles reaches the session's
//! event stream with that turn.
//!
//! After a run's drain the session task settles a sticky model fallback whose
//! run future it dropped, and the agent then publishes the committed
//! `ModelFallbackCommitted` on its own event channel. Nothing reads that
//! channel between turns, so the session task forwards what the settlement
//! queued before the turn answers: the event reaches the session's event
//! stream, and the turn's own, right after the run, not at the start of the
//! next turn.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use super::{
    AgentError, AgentEvent, ContentInput, CreateSessionRequest, EphemeralSessionService,
    ObservedSessionTailKind, RunResult, SessionAgent, SessionAgentBuilder, SessionError, SessionId,
    SessionSnapshot, SessionTranscriptAuthoritySnapshot, SystemTime, TurnToolOverlay, Usage, mpsc,
};
use async_trait::async_trait;
use futures::StreamExt as _;
use meerkat_core::SessionLlmIdentity;
use meerkat_core::event::agent_event_type;
use meerkat_core::service::{
    DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions, SessionService, StartTurnRequest,
    StartTurnRuntimeSemantics,
};

/// How long a stream must stay quiet before what it delivered counts as
/// everything it delivered for a turn.
const QUIET: Duration = Duration::from_millis(500);

fn llm_identity(model: &str) -> SessionLlmIdentity {
    SessionLlmIdentity {
        model: model.to_string(),
        provider: meerkat_core::Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    }
}

/// What a settled sticky fallback publishes.
fn fallback_committed(model: &str) -> AgentEvent {
    AgentEvent::ModelFallbackCommitted {
        retry: meerkat_core::LlmRetrySchedule {
            failure: meerkat_core::LlmRetryFailure {
                provider: "openai".to_string(),
                kind: meerkat_core::LlmRetryFailureKind::RetryableProviderError,
                retry_after_ms: None,
                duration_ms: None,
                message: "the primary model is unavailable".to_string(),
            },
            plan: meerkat_core::LlmRetryPlan {
                attempt: 1,
                max_retries: 1,
                computed_delay_ms: 0,
                selected_delay_ms: 0,
                retry_after_hint_ms: None,
                rate_limit_floor_applied: false,
                budget_capped: false,
            },
        },
        previous: llm_identity(model),
        target: llm_identity("fallback-model"),
    }
}

struct SettleProbeBuilder;

/// Runs complete at once; the first turn's settlement publishes one committed
/// fallback on the agent's own event channel, as a runtime-backed agent does
/// for a sticky fallback its interrupted run left in flight.
struct SettleProbeAgent {
    session: meerkat_core::Session,
    identity: SessionLlmIdentity,
    /// The agent's own event channel, which the session hands to the build.
    agent_events: mpsc::Sender<AgentEvent>,
    settlement_owed: bool,
    transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionAgentBuilder for SettleProbeBuilder {
    type Agent = SettleProbeAgent;

    fn session_comms_identity_root(&self) -> meerkat_core::SessionCommsIdentityRoot {
        meerkat_core::SessionCommsIdentityRoot::NotKept
    }

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, SessionError> {
        Ok(SettleProbeAgent {
            session: meerkat_core::Session::new(),
            identity: llm_identity(&req.model),
            agent_events: event_tx,
            settlement_owed: true,
            transient_turn_context_state: meerkat_core::TransientTurnContextStateHandle::new(),
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionAgent for SettleProbeAgent {
    async fn run_with_events(
        &mut self,
        prompt: ContentInput,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError> {
        let session_id = self.session.id().clone();
        let _ = event_tx
            .send(AgentEvent::RunStarted {
                reasoning_effort: None,
                request_reasoning: None,
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                session_id: session_id.clone(),
                input: meerkat_core::types::RunInput::Content { content: prompt },
            })
            .await;
        let result = RunResult {
            reasoning_effort: None,
            text: "ran".to_string(),
            session_id: session_id.clone(),
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
        };
        let _ = event_tx
            .send(AgentEvent::RunCompleted {
                identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                session_id,
                result: result.text.clone(),
                structured_output: None,
                extraction_required: false,
                usage: result.usage.clone().into(),
                terminal_cause_kind: None,
                assistant_message_id: None,
            })
            .await;
        Ok(result)
    }

    async fn settle_inflight_sticky_model_fallback(&mut self) -> Result<(), AgentError> {
        if std::mem::take(&mut self.settlement_owed) {
            let _ = self
                .agent_events
                .send(fallback_committed(&self.identity.model))
                .await;
        }
        Ok(())
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
        self.session.id().clone()
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

    fn observed_session_tail(&self) -> ObservedSessionTailKind {
        ObservedSessionTailKind::Empty
    }

    fn durable_llm_identity(&self) -> Option<SessionLlmIdentity> {
        Some(self.identity.clone())
    }

    fn transient_turn_context_state(&self) -> meerkat_core::TransientTurnContextStateHandle {
        self.transient_turn_context_state.clone()
    }
}

fn turn_request(
    event_tx: Option<mpsc::Sender<meerkat_core::EventEnvelope<AgentEvent>>>,
) -> StartTurnRequest {
    StartTurnRequest {
        injected_context: Vec::new(),
        prompt: ContentInput::Text("go".to_string()),
        system_prompt: None,
        event_tx,
        runtime: StartTurnRuntimeSemantics::default(),
    }
}

/// The type of every event `stream` delivers until it stays quiet.
async fn delivered(stream: &mut meerkat_core::comms::EventStream) -> Vec<&'static str> {
    let mut types = Vec::new();
    while let Ok(Some(envelope)) = tokio::time::timeout(QUIET, stream.next()).await {
        types.push(agent_event_type(&envelope.payload));
    }
    types
}

/// The committed fallback a turn's settlement publishes reaches the session's
/// event stream and the turn's own stream right after the run's
/// `run_completed`, before the turn answers, and the next turn delivers no
/// held-back copy of it.
#[tokio::test]
async fn a_fallback_committed_while_the_turn_settles_arrives_with_that_turn() {
    let service = EphemeralSessionService::new(SettleProbeBuilder, 4);
    let created = service
        .create_session(CreateSessionRequest {
            injected_context: Vec::new(),
            model: "primary-model".to_string(),
            prompt: ContentInput::Text(String::new()),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions::default()),
            labels: None,
        })
        .await
        .expect("create the deferred session");
    let session_id = created.session_id;
    let mut session_stream = service
        .subscribe_session_events(&session_id)
        .await
        .expect("subscribe to the session's events");

    let (turn_tx, mut turn_rx) = mpsc::channel(64);
    service
        .start_turn(&session_id, turn_request(Some(turn_tx)))
        .await
        .expect("the first turn runs");
    let first_turn = delivered(&mut session_stream).await;
    let run_completed = first_turn
        .iter()
        .position(|kind| *kind == "run_completed")
        .unwrap_or_else(|| {
            panic!("setup: the run completed on the session stream: {first_turn:?}")
        });
    let committed: Vec<usize> = first_turn
        .iter()
        .enumerate()
        .filter(|(_, kind)| **kind == "model_fallback_committed")
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        committed.len(),
        1,
        "the committed fallback reaches the session stream once, with its turn: {first_turn:?}"
    );
    assert!(
        committed[0] > run_completed,
        "the committed fallback follows the run's run_completed: {first_turn:?}"
    );
    let mut turn_stream = Vec::new();
    while let Ok(envelope) = turn_rx.try_recv() {
        turn_stream.push(agent_event_type(&envelope.payload));
    }
    assert!(
        turn_stream.contains(&"model_fallback_committed"),
        "the turn's own stream carries the committed fallback too: {turn_stream:?}"
    );

    service
        .start_turn(&session_id, turn_request(None))
        .await
        .expect("the second turn runs");
    let second_turn = delivered(&mut session_stream).await;
    assert!(
        second_turn.contains(&"run_completed"),
        "setup: the second run completed on the session stream: {second_turn:?}"
    );
    assert!(
        !second_turn.contains(&"model_fallback_committed"),
        "the next turn delivers no held-back committed fallback: {second_turn:?}"
    );
}
