use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use async_trait::async_trait;
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::event::AgentEvent;
use meerkat_core::event_injector::{
    EventInjector, EventInjectorError, InteractionSubscription, SubscribableInjector,
};
use meerkat_core::service::{
    CreateSessionRequest, SessionError, SessionHistoryPage, SessionHistoryQuery, SessionInfo,
    SessionQuery, SessionService, SessionServiceCommsExt, SessionSummary, SessionUsage,
    SessionView, StartTurnRequest,
};
use meerkat_core::types::{
    ContentInput, HandlingMode, RenderMetadata, RunResult, SessionId, Usage,
};
use meerkat_core::{InteractionId, PlainEventSource, Provider};
use meerkat_mob::{
    AgentIdentity, MobBackendKind, MobBuilder, MobDefinition, MobId, MobRuntimeMode,
    MobSessionService, MobStorage, SpawnMemberSpec,
};
use tokio::sync::{Notify, RwLock};

struct MockSessionService {
    sessions: RwLock<HashMap<SessionId, ()>>,
    keep_alive_notifiers: RwLock<HashMap<SessionId, Arc<Notify>>>,
    start_turn_calls: AtomicU64,
    inject_calls: Arc<AtomicU64>,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
}

impl Default for MockSessionService {
    fn default() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            keep_alive_notifiers: RwLock::new(HashMap::new()),
            start_turn_calls: AtomicU64::new(0),
            inject_calls: Arc::new(AtomicU64::new(0)),
            runtime_adapter: Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
        }
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
                })
                .await;
        });
        Ok(InteractionSubscription {
            id: interaction_id,
            events: rx,
        })
    }
}

#[async_trait]
impl SessionService for MockSessionService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        let sid = SessionId::new();
        self.sessions.write().await.insert(sid.clone(), ());
        let is_keep_alive = req.build.as_ref().map(|b| b.keep_alive).unwrap_or(false);
        if is_keep_alive {
            self.keep_alive_notifiers
                .write()
                .await
                .insert(sid.clone(), Arc::new(Notify::new()));
        }
        Ok(RunResult {
            text: "ok".to_string(),
            session_id: sid,
            usage: Usage::default(),
            turns: 1,
            tool_calls: 0,
            structured_output: None,
            schema_warnings: None,
            skill_diagnostics: None,
        })
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        _req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        self.start_turn_calls.fetch_add(1, Ordering::Relaxed);
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        // Block indefinitely for keep-alive sessions (autonomous host loop expects this)
        if let Some(notifier) = self.keep_alive_notifiers.read().await.get(id).cloned() {
            notifier.notified().await;
            return Ok(RunResult {
                text: "Host loop interrupted".to_string(),
                session_id: id.clone(),
                usage: Usage::default(),
                turns: 1,
                tool_calls: 0,
                structured_output: None,
                schema_warnings: None,
                skill_diagnostics: None,
            });
        }
        Ok(RunResult {
            text: "ok".to_string(),
            session_id: id.clone(),
            usage: Usage::default(),
            turns: 1,
            tool_calls: 0,
            structured_output: None,
            schema_warnings: None,
            skill_diagnostics: None,
        })
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        if let Some(notifier) = self.keep_alive_notifiers.read().await.get(id).cloned() {
            notifier.notify_waiters();
        }
        Ok(())
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        Ok(SessionView {
            state: SessionInfo {
                session_id: id.clone(),
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                is_active: false,
                model: "claude-sonnet-4-5".to_string(),
                provider: Provider::Anthropic,
                last_assistant_text: None,
                labels: Default::default(),
            },
            billing: SessionUsage {
                total_tokens: 0,
                usage: Usage::default(),
            },
        })
    }

    async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        Ok(Vec::new())
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        self.sessions.write().await.remove(id);
        if let Some(notifier) = self.keep_alive_notifiers.write().await.remove(id) {
            notifier.notify_waiters();
        }
        Ok(())
    }
}

#[async_trait]
impl SessionServiceCommsExt for MockSessionService {
    async fn comms_runtime(&self, _session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
        None
    }

    async fn event_injector(&self, session_id: &SessionId) -> Option<Arc<dyn EventInjector>> {
        if !self.sessions.read().await.contains_key(session_id) {
            return None;
        }
        Some(Arc::new(MockInjector {
            inject_calls: self.inject_calls.clone(),
        }))
    }

    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn SubscribableInjector>> {
        if !self.sessions.read().await.contains_key(session_id) {
            return None;
        }
        Some(Arc::new(MockInjector {
            inject_calls: self.inject_calls.clone(),
        }))
    }
}

#[async_trait]
impl meerkat_core::service::SessionServiceHistoryExt for MockSessionService {
    async fn read_history(
        &self,
        id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Result<SessionHistoryPage, SessionError> {
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        Ok(SessionHistoryPage::from_messages(id.clone(), &[], query))
    }
}

#[async_trait]
impl meerkat_core::service::SessionServiceControlExt for MockSessionService {
    async fn append_system_context(
        &self,
        id: &SessionId,
        _req: meerkat_core::AppendSystemContextRequest,
    ) -> Result<
        meerkat_core::service::AppendSystemContextResult,
        meerkat_core::service::SessionControlError,
    > {
        if !self.sessions.read().await.contains_key(id) {
            return Err(meerkat_core::SessionError::NotFound { id: id.clone() }.into());
        }
        Ok(meerkat_core::service::AppendSystemContextResult {
            status: meerkat_core::service::AppendSystemContextStatus::Staged,
        })
    }
}

#[async_trait]
impl MobSessionService for MockSessionService {
    fn supports_persistent_sessions(&self) -> bool {
        true
    }

    fn runtime_adapter(&self) -> Option<Arc<meerkat_runtime::MeerkatMachine>> {
        Some(self.runtime_adapter.clone())
    }

    async fn session_belongs_to_mob(&self, _session_id: &SessionId, _mob_id: &MobId) -> bool {
        true
    }

    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::RunId,
        req: StartTurnRequest,
        boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary,
        contributing_input_ids: Vec<meerkat_core::InputId>,
    ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, SessionError> {
        <Self as SessionService>::start_turn(self, session_id, req).await?;
        Ok(meerkat_core::lifecycle::core_executor::CoreApplyOutput {
            receipt: meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt {
                run_id,
                boundary,
                contributing_input_ids,
                conversation_digest: None,
                message_count: 0,
                sequence: 0,
            },
            session_snapshot: None,
            terminal: None,
            run_result: None,
        })
    }
}

#[tokio::test]
async fn test_phase2_external_turn_routing_by_runtime_mode() {
    let service = Arc::new(MockSessionService::default());
    let mut definition = MobDefinition::from_toml(
        "[mob]\nid = \"phase2-routing\"\norchestrator = \"lead\"\n\n[profiles.lead]\nmodel = \"claude-opus-4-6\"\nexternal_addressable = true\n\n[profiles.lead.tools]\nbuiltins = true\ncomms = true\nmob = true\n\n[profiles.worker]\nmodel = \"claude-sonnet-4-6\"\n\n[profiles.worker.tools]\nbuiltins = true\ncomms = true\n",
    )
    .expect("phase2 mob definition");
    definition.id = MobId::from("phase2-routing");
    definition.backend.default = MobBackendKind::Session;
    definition.wiring.auto_wire_orchestrator = false;
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
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
    let start_before = service.start_turn_calls.load(Ordering::Relaxed);
    let inject_before = service.inject_calls.load(Ordering::Relaxed);

    handle
        .member(&AgentIdentity::from("lead-auto"))
        .await
        .expect("member auto")
        .send("auto".to_string(), meerkat_core::types::HandlingMode::Queue)
        .await
        .expect("external autonomous");
    handle
        .member(&AgentIdentity::from("lead-turn"))
        .await
        .expect("member turn")
        .send("turn".to_string(), meerkat_core::types::HandlingMode::Queue)
        .await
        .expect("external turn-driven");

    let start_after = service.start_turn_calls.load(Ordering::Relaxed);
    let inject_after = service.inject_calls.load(Ordering::Relaxed);
    assert!(
        start_after > start_before,
        "turn-driven path should invoke start_turn"
    );
    assert!(
        inject_after > inject_before,
        "autonomous path should invoke event injector"
    );
}
