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
    CreateSessionRequest, SessionError, SessionInfo, SessionQuery, SessionService, SessionSummary,
    SessionServiceCommsExt, SessionUsage, SessionView, StartTurnRequest,
};
use meerkat_core::types::{RunResult, SessionId, Usage};
use meerkat_core::{InteractionId, PlainEventSource};
use meerkat_mob::{
    MeerkatId, MobBackendKind, MobBuilder, MobId, MobRuntimeMode, MobSessionService, MobStorage,
    Prefab, ProfileName,
};
use tokio::sync::{Notify, RwLock};

#[derive(Default)]
struct MockSessionService {
    sessions: RwLock<HashMap<SessionId, Arc<Notify>>>,
    start_turn_calls: AtomicU64,
    host_mode_start_turn_calls: AtomicU64,
    inject_calls: Arc<AtomicU64>,
}

struct MockInjector {
    inject_calls: Arc<AtomicU64>,
}

impl EventInjector for MockInjector {
    fn inject(&self, _body: String, _source: PlainEventSource) -> Result<(), EventInjectorError> {
        self.inject_calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

impl SubscribableInjector for MockInjector {
    fn inject_with_subscription(
        &self,
        body: String,
        source: PlainEventSource,
    ) -> Result<InteractionSubscription, EventInjectorError> {
        self.inject(body, source)?;
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
    async fn create_session(&self, _req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        let sid = SessionId::new();
        self.sessions
            .write()
            .await
            .insert(sid.clone(), Arc::new(Notify::new()));
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
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        self.start_turn_calls.fetch_add(1, Ordering::Relaxed);
        if req.host_mode {
            self.host_mode_start_turn_calls
                .fetch_add(1, Ordering::Relaxed);
        }
        let sessions = self.sessions.read().await;
        let Some(notify) = sessions.get(id).cloned() else {
            return Err(SessionError::NotFound { id: id.clone() });
        };
        drop(sessions);
        if req.host_mode {
            notify.notified().await;
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
        if let Some(notify) = self.sessions.read().await.get(id).cloned() {
            notify.notify_waiters();
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
                last_assistant_text: None,
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
        if let Some(notify) = self.sessions.write().await.remove(id) {
            notify.notify_waiters();
        }
        Ok(())
    }
}

#[async_trait]
impl SessionServiceCommsExt for MockSessionService {
    async fn comms_runtime(&self, _session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
        None
    }

    async fn event_injector(
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
impl MobSessionService for MockSessionService {
    fn supports_persistent_sessions(&self) -> bool {
        true
    }

    async fn session_belongs_to_mob(&self, _session_id: &SessionId, _mob_id: &MobId) -> bool {
        true
    }
}

#[tokio::test]
async fn test_phase2_external_turn_routing_by_runtime_mode() {
    let service = Arc::new(MockSessionService::default());
    let mut definition = Prefab::CodingSwarm.definition();
    definition.id = MobId::from("phase2-routing");
    definition.backend.default = MobBackendKind::Subagent;
    definition.wiring.auto_wire_orchestrator = false;
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create");

    handle
        .spawn(
            ProfileName::from("lead"),
            MeerkatId::from("lead-auto"),
            None,
        )
        .await
        .expect("spawn autonomous");
    handle
        .spawn_with_options(
            ProfileName::from("lead"),
            MeerkatId::from("lead-turn"),
            None,
            Some(MobRuntimeMode::TurnDriven),
            None,
        )
        .await
        .expect("spawn turn-driven");
    let start_before = service.start_turn_calls.load(Ordering::Relaxed);
    let host_before = service.host_mode_start_turn_calls.load(Ordering::Relaxed);
    let inject_before = service.inject_calls.load(Ordering::Relaxed);

    handle
        .send_message(MeerkatId::from("lead-auto"), "auto".to_string())
        .await
        .expect("external autonomous");
    handle
        .send_message(MeerkatId::from("lead-turn"), "turn".to_string())
        .await
        .expect("external turn-driven");

    let start_after = service.start_turn_calls.load(Ordering::Relaxed);
    let host_after = service.host_mode_start_turn_calls.load(Ordering::Relaxed);
    let inject_after = service.inject_calls.load(Ordering::Relaxed);
    assert!(
        start_after > start_before,
        "turn-driven path should invoke start_turn"
    );
    assert_eq!(
        host_after, host_before,
        "external turns should not route through host-mode start_turn"
    );
    assert!(
        inject_after > inject_before,
        "autonomous path should invoke event injector"
    );
}
