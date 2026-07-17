use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use meerkat_core::event::AgentEvent;
use meerkat_core::event_injector::{
    EventInjector, EventInjectorError, InteractionSubscription, SubscribableInjector,
};
use meerkat_core::service::{CreateSessionRequest, SessionError, TurnToolOverlay};
use meerkat_core::types::{
    ContentInput, HandlingMode, RenderMetadata, RunResult, SessionId, Usage,
};
use meerkat_core::{
    InteractionId, PlainEventSource, Provider, Session, SessionLlmIdentity, SystemContextStateError,
};
use meerkat_mob::{
    AgentIdentity, MobBackendKind, MobBuilder, MobDefinition, MobId, MobRuntimeMode, MobStorage,
    SpawnMemberSpec,
};
use meerkat_session::{
    EphemeralSessionService, SessionAgent, SessionAgentBuilder, SessionSnapshot,
};
use tokio::sync::mpsc;

struct MockSessionAgentBuilder {
    start_turn_calls: Arc<AtomicU64>,
    inject_calls: Arc<AtomicU64>,
}

struct MockSessionAgent {
    session: Session,
    llm_identity: SessionLlmIdentity,
    comms_runtime: Arc<meerkat_comms::CommsRuntime>,
    keep_alive: bool,
    start_turn_calls: Arc<AtomicU64>,
    inject_calls: Arc<AtomicU64>,
    system_context_state: meerkat_core::SystemContextStateHandle,
}

impl MockSessionAgent {
    async fn run(&mut self) -> Result<RunResult, meerkat_core::error::AgentError> {
        if self.keep_alive {
            // The autonomous host owns this long-lived turn. The native
            // session task drops this future through its ordinary interrupt
            // path during teardown.
            std::future::pending::<()>().await;
        }
        Ok(RunResult {
            text: "ok".to_string(),
            session_id: self.session.id().clone(),
            usage: Usage::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: None,
        })
    }
}

#[async_trait]
impl SessionAgentBuilder for MockSessionAgentBuilder {
    type Agent = MockSessionAgent;

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, SessionError> {
        let requested_session_id = req.build.as_ref().and_then(|build| {
            build
                .resume_session
                .as_ref()
                .map(|session| session.id().clone())
                .or_else(|| match &build.runtime_build_mode {
                    meerkat_core::RuntimeBuildMode::SessionOwned(bindings) => {
                        Some(bindings.session_id().clone())
                    }
                    meerkat_core::RuntimeBuildMode::StandaloneEphemeral => None,
                })
        });
        let session = req
            .build
            .as_ref()
            .and_then(|build| build.resume_session.clone())
            .unwrap_or_else(|| Session::with_id(requested_session_id.unwrap_or_default()));
        let system_context_state = meerkat_core::SystemContextStateHandle::new(
            session.system_context_state().unwrap_or_default(),
        )
        .map_err(|error| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to restore phase2 test system-context state: {error}"
            )))
        })?;
        let build = req.build.as_ref();
        let comms_name = build
            .and_then(|build| build.comms_name.as_deref())
            .expect("phase2 routing member should request a comms runtime");
        let comms_runtime = Arc::new(
            meerkat_comms::CommsRuntime::inproc_only(comms_name)
                .expect("create phase2 routing comms runtime"),
        );
        if let Some(meerkat_core::RuntimeBuildMode::SessionOwned(bindings)) =
            build.map(|build| &build.runtime_build_mode)
        {
            bindings
                .install_peer_comms_on(comms_runtime.as_ref())
                .map_err(|error| {
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                        "install phase2 routing peer-comms authority: {error}"
                    )))
                })?;
        }
        Ok(MockSessionAgent {
            session,
            llm_identity: SessionLlmIdentity {
                model: req.model.clone(),
                provider: build
                    .and_then(|build| build.provider)
                    .unwrap_or(Provider::Other),
                self_hosted_server_id: build.and_then(|build| build.self_hosted_server_id.clone()),
                provider_params: build.and_then(|build| build.provider_params.clone()),
                auth_binding: build.and_then(|build| build.auth_binding.clone()),
            },
            comms_runtime,
            keep_alive: build.is_some_and(|build| build.keep_alive),
            start_turn_calls: Arc::clone(&self.start_turn_calls),
            inject_calls: Arc::clone(&self.inject_calls),
            system_context_state,
        })
    }
}

#[async_trait]
impl SessionAgent for MockSessionAgent {
    async fn run_with_events(
        &mut self,
        _prompt: ContentInput,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        self.run().await
    }

    async fn run_turn_with_events(
        &mut self,
        _input: meerkat_session::ephemeral::SessionAgentTurnInput,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        self.start_turn_calls.fetch_add(1, Ordering::Relaxed);
        self.run().await
    }

    fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

    fn set_turn_tool_overlay(
        &mut self,
        _overlay: Option<TurnToolOverlay>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Ok(())
    }

    fn cancel(&mut self) {}

    fn hot_swap_llm_identity(
        &mut self,
        _client: Arc<dyn meerkat_core::AgentLlmClient>,
        identity: SessionLlmIdentity,
        _request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.llm_identity = identity;
        Ok(())
    }

    fn session_id(&self) -> SessionId {
        self.session.id().clone()
    }

    fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            created_at: self.session.created_at(),
            updated_at: self.session.updated_at(),
            message_count: self.session.messages().len(),
            total_tokens: self.session.total_tokens(),
            usage: self.session.total_usage(),
            last_assistant_text: None,
        }
    }

    fn session_clone(&self) -> Result<Session, SystemContextStateError> {
        let mut session = self.session.clone();
        session
            .set_system_context_state(self.system_context_state.snapshot())
            .map_err(SystemContextStateError::SystemContext)?;
        Ok(session)
    }

    fn durable_llm_identity(&self) -> Option<SessionLlmIdentity> {
        Some(self.llm_identity.clone())
    }

    fn observed_session_tail(&self) -> meerkat_core::pending_continuation::ObservedSessionTailKind {
        meerkat_core::pending_continuation::observe_session_tail(self.session.messages())
    }

    fn update_keep_alive(&mut self, keep_alive: bool) {
        self.keep_alive = keep_alive;
    }

    fn apply_runtime_system_context(
        &mut self,
        appends: &[meerkat_core::PendingSystemContextAppend],
    ) {
        self.session.append_system_context_blocks(appends);
        let _ = self.system_context_state.replace_from_generated_restore(
            self.session.system_context_state().unwrap_or_default(),
        );
    }

    fn system_context_state(&self) -> meerkat_core::SystemContextStateHandle {
        self.system_context_state.clone()
    }

    fn event_injector(&self) -> Option<Arc<dyn EventInjector>> {
        Some(Arc::new(MockInjector {
            inject_calls: Arc::clone(&self.inject_calls),
        }))
    }

    fn interaction_event_injector(&self) -> Option<Arc<dyn SubscribableInjector>> {
        Some(Arc::new(MockInjector {
            inject_calls: Arc::clone(&self.inject_calls),
        }))
    }

    fn comms_runtime(&self) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        Some(self.comms_runtime.clone())
    }
}

struct MockInjector {
    inject_calls: Arc<AtomicU64>,
}

impl EventInjector for MockInjector {
    fn inject(
        &self,
        _body: ContentInput,
        _source: PlainEventSource,
        _handling_mode: HandlingMode,
        _render_metadata: Option<RenderMetadata>,
    ) -> Result<(), EventInjectorError> {
        self.inject_calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

impl SubscribableInjector for MockInjector {
    fn inject_with_subscription(
        &self,
        body: ContentInput,
        source: PlainEventSource,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<InteractionSubscription, EventInjectorError> {
        self.inject(body, source, handling_mode, render_metadata)?;
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let interaction_id = InteractionId(uuid::Uuid::new_v4());
        let interaction_id_for_task = interaction_id;
        tokio::spawn(async move {
            let _ = tx
                .send(AgentEvent::InteractionComplete {
                    interaction_id: interaction_id_for_task,
                    result: "ok".to_string(),
                    structured_output: None,
                })
                .await;
        });
        Ok(InteractionSubscription {
            id: interaction_id,
            events: rx,
        })
    }

    fn inject_with_interaction_id(
        &self,
        _interaction_id: InteractionId,
        body: ContentInput,
        source: PlainEventSource,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<(), EventInjectorError> {
        self.inject(body, source, handling_mode, render_metadata)
    }
}

#[tokio::test]
async fn test_phase2_external_turn_routing_by_runtime_mode() {
    let start_turn_calls = Arc::new(AtomicU64::new(0));
    let inject_calls = Arc::new(AtomicU64::new(0));
    let service = Arc::new(EphemeralSessionService::new(
        MockSessionAgentBuilder {
            start_turn_calls: Arc::clone(&start_turn_calls),
            inject_calls: Arc::clone(&inject_calls),
        },
        16,
    ));
    let mut definition = MobDefinition::from_toml(
        "[mob]\nid = \"phase2-routing\"\norchestrator = \"lead\"\n\n[profiles.lead]\nmodel = \"claude-opus-4-8\"\nexternal_addressable = true\n\n[profiles.lead.tools]\nbuiltins = true\ncomms = true\nmob = true\n\n[profiles.worker]\nmodel = \"claude-sonnet-4-6\"\n\n[profiles.worker.tools]\nbuiltins = true\ncomms = true\n",
    )
    .expect("phase2 mob definition");
    definition.id = MobId::from("phase2-routing");
    definition.backend.default = MobBackendKind::Session;
    definition.wiring.auto_wire_orchestrator = false;
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service)
        .allow_ephemeral_sessions(true)
        .create()
        .await
        .expect("create");

    handle
        .spawn_spec(SpawnMemberSpec::new("lead", "lead-auto"))
        .await
        .expect("spawn autonomous");
    let mut turn_spec = SpawnMemberSpec::new("lead", "lead-turn");
    turn_spec.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    handle
        .spawn_spec(turn_spec)
        .await
        .expect("spawn turn-driven");
    let start_before = start_turn_calls.load(Ordering::Relaxed);
    let inject_before = inject_calls.load(Ordering::Relaxed);

    handle
        .member(&AgentIdentity::from("lead-auto"))
        .await
        .expect("member auto")
        .send("auto".to_string(), meerkat_core::types::HandlingMode::Queue)
        .await
        .expect("external autonomous ingress accepted");
    handle
        .member(&AgentIdentity::from("lead-turn"))
        .await
        .expect("member turn")
        .send("turn".to_string(), meerkat_core::types::HandlingMode::Queue)
        .await
        .expect("external turn-driven ingress accepted");

    // Runtime-backed external delivery acknowledges ingress admission, not
    // executor completion. Wait for the two native routing effects instead of
    // relying on the synchronous behavior of the former hand-written mock.
    let deadline = Instant::now() + Duration::from_secs(2);
    while start_turn_calls.load(Ordering::Relaxed) <= start_before {
        assert!(
            Instant::now() < deadline,
            "turn-driven path should execute a native session turn"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    while inject_calls.load(Ordering::Relaxed) <= inject_before {
        assert!(
            Instant::now() < deadline,
            "autonomous path should invoke event injector"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let start_after = start_turn_calls.load(Ordering::Relaxed);
    let inject_after = inject_calls.load(Ordering::Relaxed);
    assert!(
        start_after > start_before,
        "turn-driven path should execute a native session turn"
    );
    assert!(
        inject_after > inject_before,
        "autonomous path should invoke event injector"
    );
}
