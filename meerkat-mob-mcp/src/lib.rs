#[cfg(target_arch = "wasm32")]
mod tokio {
    pub use tokio_with_wasm::alias::*;
}

use async_trait::async_trait;

use meerkat_client::LlmClient;
use meerkat_core::AppendSystemContextStatus;
use meerkat_core::ScopedAgentEvent;
use meerkat_core::agent::{AgentToolDispatcher, CommsRuntime as CoreCommsRuntime};
use meerkat_core::comms::{CommsCommand, SendError, SendReceipt, TrustedPeerSpec};
use meerkat_core::error::ToolError;
use meerkat_core::interaction::InteractionId;
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextResult, CreateSessionRequest,
    SessionControlError, SessionError, SessionInfo, SessionQuery, SessionService,
    SessionServiceCommsExt, SessionServiceControlExt, SessionSummary, SessionUsage, SessionView,
    StartTurnRequest,
};
use meerkat_core::time_compat::{Instant, SystemTime};
use meerkat_core::types::{RunResult, SessionId, ToolCallView, ToolDef, ToolResult, Usage};
use meerkat_core::{AgentEvent, EventEnvelope, EventStream, StreamError};
use meerkat_mob::{
    FlowId, MeerkatId, MobBackendKind, MobBuilder, MobDefinition, MobError, MobHandle, MobId,
    MobRuntimeMode, MobSessionService, MobState, MobStorage, Prefab, ProfileName, RunId,
    SpawnMemberSpec,
};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet, btree_map::Entry};
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use ::tokio::sync::{RwLock, mpsc};
#[cfg(target_arch = "wasm32")]
use tokio::sync::{RwLock, mpsc};

#[derive(Clone)]
struct ManagedMob {
    handle: MobHandle,
}

type DefaultLlmClientProvider = Arc<dyn Fn() -> Option<Arc<dyn LlmClient>> + Send + Sync + 'static>;

/// In-memory MCP state for multiple mobs.
pub struct MobMcpState {
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Option<Arc<meerkat_runtime::RuntimeSessionAdapter>>,
    default_llm_client: Option<Arc<dyn LlmClient>>,
    default_llm_client_provider: Option<DefaultLlmClientProvider>,
    mobs: RwLock<BTreeMap<MobId, ManagedMob>>,
}

impl MobMcpState {
    pub fn new(session_service: Arc<dyn MobSessionService>) -> Self {
        let runtime_adapter = session_service.runtime_adapter();
        Self::new_with_runtime_adapter(session_service, runtime_adapter)
    }

    pub fn new_with_runtime_adapter(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<meerkat_runtime::RuntimeSessionAdapter>>,
    ) -> Self {
        Self {
            session_service,
            runtime_adapter,
            default_llm_client: None,
            default_llm_client_provider: None,
            mobs: RwLock::new(BTreeMap::new()),
        }
    }

    pub fn with_default_llm_client(mut self, client: Option<Arc<dyn LlmClient>>) -> Self {
        self.default_llm_client = client;
        self
    }

    pub fn with_default_llm_client_provider(
        mut self,
        provider: Option<DefaultLlmClientProvider>,
    ) -> Self {
        self.default_llm_client_provider = provider;
        self
    }

    /// Access the underlying session service.
    pub fn session_service(&self) -> Arc<dyn MobSessionService> {
        self.session_service.clone()
    }

    pub async fn mob_create_definition(
        &self,
        definition: MobDefinition,
    ) -> Result<MobId, MobError> {
        let mob_id = definition.id.clone();
        if self.mobs.read().await.contains_key(&mob_id) {
            return Err(MobError::Internal(format!("mob already exists: {mob_id}")));
        }
        let storage = MobStorage::in_memory();
        let mut builder = MobBuilder::new(definition.clone(), storage)
            .with_session_service(self.session_service.clone())
            .allow_ephemeral_sessions(!self.session_service.supports_persistent_sessions());
        if let Some(adapter) = &self.runtime_adapter {
            builder = builder.with_runtime_adapter(adapter.clone());
        }
        let default_llm_client = self.default_llm_client.clone().or_else(|| {
            self.default_llm_client_provider
                .as_ref()
                .and_then(|provider| provider())
        });
        if let Some(client) = default_llm_client {
            builder = builder.with_default_llm_client(client.clone());
        }
        let handle = builder.create().await?;
        match self.mobs.write().await.entry(mob_id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(ManagedMob { handle });
            }
            Entry::Occupied(_) => {
                // Race-safe duplicate guard: avoid leaking the just-created runtime.
                if let Err(error) = handle.destroy().await {
                    tracing::warn!(
                        mob_id = %mob_id,
                        error = %error,
                        "duplicate mob create cleanup failed"
                    );
                }
                return Err(MobError::Internal(format!("mob already exists: {mob_id}")));
            }
        }
        Ok(mob_id)
    }

    pub async fn mob_create_prefab(&self, prefab: Prefab) -> Result<MobId, MobError> {
        self.mob_create_definition(prefab.definition()).await
    }

    /// Register an existing mob handle in this dispatcher state.
    pub async fn mob_insert_handle(&self, mob_id: MobId, handle: MobHandle) {
        self.mobs
            .write()
            .await
            .insert(mob_id, ManagedMob { handle });
    }

    pub async fn mob_list(&self) -> Vec<(MobId, MobState)> {
        self.mobs
            .read()
            .await
            .iter()
            .map(|(id, managed)| (id.clone(), managed.handle.status()))
            .collect()
    }

    pub async fn mob_status(&self, mob_id: &MobId) -> Result<MobState, MobError> {
        let handle = self.handle_for(mob_id).await?;
        Ok(handle.status())
    }

    pub async fn mob_stop(&self, mob_id: &MobId) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.stop().await
    }

    pub async fn mob_resume(&self, mob_id: &MobId) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.resume().await
    }

    pub async fn mob_complete(&self, mob_id: &MobId) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.complete().await
    }

    pub async fn mob_reset(&self, mob_id: &MobId) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.reset().await
    }

    pub async fn mob_destroy(&self, mob_id: &MobId) -> Result<(), MobError> {
        let mut mobs = self.mobs.write().await;
        let managed = mobs
            .get(mob_id)
            .cloned()
            .ok_or_else(|| MobError::Internal(format!("mob not found: {mob_id}")))?;
        managed.handle.destroy().await?;
        mobs.remove(mob_id);
        Ok(())
    }

    pub async fn mob_spawn(
        &self,
        mob_id: &MobId,
        profile: ProfileName,
        meerkat_id: MeerkatId,
        runtime_mode: Option<MobRuntimeMode>,
        backend: Option<MobBackendKind>,
    ) -> Result<meerkat_mob::MemberRef, MobError> {
        let mut spec = SpawnMemberSpec::new(profile, meerkat_id);
        spec.runtime_mode = runtime_mode;
        spec.backend = backend;
        self.mob_spawn_spec(mob_id, spec).await
    }

    pub async fn mob_spawn_spec(
        &self,
        mob_id: &MobId,
        spec: SpawnMemberSpec,
    ) -> Result<meerkat_mob::MemberRef, MobError> {
        let member_ref = self.handle_for(mob_id).await?.spawn_spec(spec).await?;
        Ok(member_ref)
    }

    pub async fn mob_spawn_many(
        &self,
        mob_id: &MobId,
        specs: Vec<SpawnMemberSpec>,
    ) -> Result<Vec<Result<meerkat_mob::MemberRef, MobError>>, MobError> {
        Ok(self.handle_for(mob_id).await?.spawn_many(specs).await)
    }

    pub async fn mob_retire(&self, mob_id: &MobId, meerkat_id: MeerkatId) -> Result<(), MobError> {
        self.handle_for(mob_id)
            .await?
            .retire(meerkat_id.clone())
            .await
    }

    pub async fn retire_member_by_session_id(
        &self,
        session_id: &SessionId,
    ) -> Result<(), MobError> {
        // Derive membership from authoritative live mob roster state rather than
        // maintaining a separate reverse index that can drift during retirement,
        // respawn, or policy-driven auto-spawn flows.
        let mob_ids = self.mobs.read().await.keys().cloned().collect::<Vec<_>>();
        let mut resolved = None;
        for mob_id in mob_ids {
            let members = self.handle_for(&mob_id).await?.list_all_members().await;
            if let Some(member) = members.into_iter().find(|member| {
                member
                    .member_ref
                    .session_id()
                    .is_some_and(|candidate| candidate == session_id)
            }) {
                resolved = Some((mob_id, member.meerkat_id));
                break;
            }
        }
        let Some((mob_id, meerkat_id)) = resolved else {
            return Err(MobError::Internal(format!(
                "session not found in any live mob authority: {session_id}"
            )));
        };
        self.mob_retire(&mob_id, meerkat_id).await
    }

    pub async fn owns_live_session(&self, session_id: &SessionId) -> bool {
        let mob_ids = self.mobs.read().await.keys().cloned().collect::<Vec<_>>();
        for mob_id in mob_ids {
            if let Ok(handle) = self.handle_for(&mob_id).await {
                let members = handle.list_all_members().await;
                if members.into_iter().any(|member| {
                    member
                        .member_ref
                        .session_id()
                        .is_some_and(|candidate| candidate == session_id)
                }) {
                    return true;
                }
            }
        }
        false
    }

    pub async fn owns_persisted_session(&self, session_id: &SessionId) -> bool {
        let Some(comms_name) = self
            .session_service()
            .load_persisted_session(session_id)
            .await
            .ok()
            .flatten()
            .and_then(|session| {
                session
                    .session_metadata()
                    .and_then(|metadata| metadata.comms_name)
            })
        else {
            return false;
        };

        let mut parts = comms_name.split('/');
        let Some(mob_id) = parts.next().filter(|part| !part.is_empty()) else {
            return false;
        };
        let has_profile = parts.next().is_some_and(|part| !part.is_empty());
        let has_member = parts.next().is_some_and(|part| !part.is_empty());
        if !(has_profile && has_member) || parts.next().is_some() {
            return false;
        }

        self.mobs
            .read()
            .await
            .contains_key(&meerkat_mob::MobId::from(mob_id))
    }

    pub async fn mob_wire(
        &self,
        mob_id: &MobId,
        a: MeerkatId,
        b: MeerkatId,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.wire(a, b).await
    }

    pub async fn mob_unwire(
        &self,
        mob_id: &MobId,
        a: MeerkatId,
        b: MeerkatId,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.unwire(a, b).await
    }

    pub async fn mob_list_members(
        &self,
        mob_id: &MobId,
    ) -> Result<Vec<meerkat_mob::RosterEntry>, MobError> {
        self.handle_for(mob_id)
            .await?
            .list_all_members()
            .await
            .pipe(Ok)
    }

    pub async fn mob_append_system_context(
        &self,
        mob_id: &MobId,
        meerkat_id: &MeerkatId,
        req: AppendSystemContextRequest,
    ) -> Result<(SessionId, AppendSystemContextResult), SessionControlError> {
        let members = self.mob_list_members(mob_id).await.map_err(|error| {
            SessionControlError::InvalidRequest {
                message: error.to_string(),
            }
        })?;
        let session_id = members
            .into_iter()
            .find(|member| member.meerkat_id == *meerkat_id)
            .and_then(|member| member.member_ref.session_id().cloned())
            .ok_or_else(|| SessionControlError::InvalidRequest {
                message: format!("member has no session: {meerkat_id}"),
            })?;
        let result = self
            .session_service()
            .append_system_context(&session_id, req)
            .await?;
        Ok((session_id, result))
    }

    pub async fn mob_send_message(
        &self,
        mob_id: &MobId,
        meerkat_id: MeerkatId,
        message: String,
    ) -> Result<SessionId, MobError> {
        self.handle_for(mob_id)
            .await?
            .send_message(meerkat_id, message)
            .await
    }

    pub async fn mob_events(
        &self,
        mob_id: &MobId,
        after_cursor: u64,
        limit: usize,
    ) -> Result<Vec<meerkat_mob::MobEvent>, MobError> {
        self.handle_for(mob_id)
            .await?
            .events()
            .poll(after_cursor, limit)
            .await
    }

    pub async fn mob_list_flows(&self, mob_id: &MobId) -> Result<Vec<String>, MobError> {
        let flows = self.handle_for(mob_id).await?.list_flows();
        Ok(flows
            .into_iter()
            .map(|flow_id| flow_id.to_string())
            .collect())
    }

    pub async fn mob_run_flow(
        &self,
        mob_id: &MobId,
        flow_id: FlowId,
        params: serde_json::Value,
    ) -> Result<RunId, MobError> {
        self.mob_run_flow_with_stream(mob_id, flow_id, params, None)
            .await
    }

    pub async fn mob_run_flow_with_stream(
        &self,
        mob_id: &MobId,
        flow_id: FlowId,
        params: serde_json::Value,
        scoped_event_tx: Option<mpsc::Sender<ScopedAgentEvent>>,
    ) -> Result<RunId, MobError> {
        self.handle_for(mob_id)
            .await?
            .run_flow_with_stream(flow_id, params, scoped_event_tx)
            .await
    }

    pub async fn mob_flow_status(
        &self,
        mob_id: &MobId,
        run_id: RunId,
    ) -> Result<Option<meerkat_mob::MobRun>, MobError> {
        self.handle_for(mob_id).await?.flow_status(run_id).await
    }

    pub async fn mob_cancel_flow(&self, mob_id: &MobId, run_id: RunId) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.cancel_flow(run_id).await
    }

    pub async fn mob_respawn(
        &self,
        mob_id: &MobId,
        meerkat_id: MeerkatId,
        initial_message: Option<String>,
    ) -> Result<(), MobError> {
        self.handle_for(mob_id)
            .await?
            .respawn(meerkat_id, initial_message)
            .await
    }

    /// Subscribe to mob-wide events (all members, continuously updated).
    pub async fn subscribe_mob_events(
        &self,
        mob_id: &MobId,
    ) -> Result<meerkat_mob::MobEventRouterHandle, MobError> {
        Ok(self.handle_for(mob_id).await?.subscribe_mob_events())
    }

    /// Subscribe to agent-level events for a specific member.
    pub async fn subscribe_agent_events(
        &self,
        mob_id: &MobId,
        meerkat_id: &MeerkatId,
    ) -> Result<meerkat_core::comms::EventStream, MobError> {
        self.handle_for(mob_id)
            .await?
            .subscribe_agent_events(meerkat_id)
            .await
    }

    /// Look up the [`MobHandle`] for a given mob ID.
    ///
    /// Returns `MobError::Internal` if the mob is not found.
    pub async fn handle_for(&self, mob_id: &MobId) -> Result<MobHandle, MobError> {
        self.mobs
            .read()
            .await
            .get(mob_id)
            .map(|m| m.handle.clone())
            .ok_or_else(|| MobError::Internal(format!("mob not found: {mob_id}")))
    }

    /// Create MCP state backed by an in-memory local session service.
    pub fn new_in_memory() -> Arc<Self> {
        Arc::new(Self::new_with_runtime_adapter(
            Arc::new(LocalSessionService::new()),
            Some(Arc::new(meerkat_runtime::RuntimeSessionAdapter::ephemeral())),
        ))
    }
}

struct LocalCommsRuntime {
    key: String,
    trusted: RwLock<HashSet<String>>,
    notify: Arc<tokio::sync::Notify>,
}

impl LocalCommsRuntime {
    fn new(name: &str) -> Self {
        Self {
            key: format!("ed25519:{name}"),
            trusted: RwLock::new(HashSet::new()),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CoreCommsRuntime for LocalCommsRuntime {
    fn public_key(&self) -> Option<String> {
        Some(self.key.clone())
    }

    async fn add_trusted_peer(&self, peer: TrustedPeerSpec) -> Result<(), SendError> {
        self.trusted.write().await.insert(peer.peer_id);
        Ok(())
    }

    async fn remove_trusted_peer(&self, peer_id: &str) -> Result<bool, SendError> {
        Ok(self.trusted.write().await.remove(peer_id))
    }

    async fn send(&self, _cmd: CommsCommand) -> Result<SendReceipt, SendError> {
        Ok(SendReceipt::InputAccepted {
            interaction_id: InteractionId(uuid::Uuid::nil()),
            stream_reserved: false,
        })
    }

    async fn drain_messages(&self) -> Vec<String> {
        Vec::new()
    }

    fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
        self.notify.clone()
    }
}

struct LocalSessionService {
    sessions: RwLock<HashMap<SessionId, Arc<LocalCommsRuntime>>>,
    archived_views: RwLock<HashMap<SessionId, SessionView>>,
    pending_context: RwLock<HashMap<SessionId, Vec<AppendSystemContextRequest>>>,
    /// Per-session broadcast channels for event streaming.
    event_txs:
        RwLock<HashMap<SessionId, tokio::sync::broadcast::Sender<EventEnvelope<AgentEvent>>>>,
    counter: std::sync::atomic::AtomicU64,
    archive_delay_ms: std::sync::atomic::AtomicU64,
}

impl LocalSessionService {
    fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            archived_views: RwLock::new(HashMap::new()),
            pending_context: RwLock::new(HashMap::new()),
            event_txs: RwLock::new(HashMap::new()),
            counter: std::sync::atomic::AtomicU64::new(0),
            archive_delay_ms: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn set_archive_delay_ms(&self, delay_ms: u64) {
        self.archive_delay_ms
            .store(delay_ms, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionService for LocalSessionService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        let sid = SessionId::new();
        let n = self
            .counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = req
            .build
            .and_then(|b| b.comms_name)
            .unwrap_or_else(|| format!("session-{n}"));
        self.sessions
            .write()
            .await
            .insert(sid.clone(), Arc::new(LocalCommsRuntime::new(&name)));
        self.pending_context
            .write()
            .await
            .insert(sid.clone(), Vec::new());
        let (tx, _) = tokio::sync::broadcast::channel::<EventEnvelope<AgentEvent>>(256);
        self.event_txs.write().await.insert(sid.clone(), tx);
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
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        // Drain any staged system context so the append_system_context contract
        // is honored (staged context is consumed on the next turn).
        let staged_context = {
            let mut pending = self.pending_context.write().await;
            let entry = pending.entry(id.clone()).or_default();
            std::mem::take(entry)
        };
        let effective_prompt = if staged_context.is_empty() {
            req.prompt.clone()
        } else {
            let staged_sections = staged_context
                .iter()
                .map(|append| match append.source.as_deref() {
                    Some(source) => format!("[SYSTEM CONTEXT:{source}] {}", append.text),
                    None => format!("[SYSTEM CONTEXT] {}", append.text),
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!("{staged_sections}\n\n{}", req.prompt)
        };

        let event_tx = self.event_txs.read().await.get(id).cloned();
        let next_seq = |seq: &mut u64| {
            let current = *seq;
            *seq += 1;
            current
        };
        if let Some(event_tx) = event_tx {
            let source_id = id.to_string();
            let mut seq = 1u64;
            let _ = event_tx.send(EventEnvelope::new(
                source_id.clone(),
                next_seq(&mut seq),
                None,
                AgentEvent::RunStarted {
                    session_id: id.clone(),
                    prompt: effective_prompt.clone(),
                },
            ));
            let _ = event_tx.send(EventEnvelope::new(
                source_id.clone(),
                next_seq(&mut seq),
                None,
                AgentEvent::TurnStarted { turn_number: 1 },
            ));
            let usage = Usage::default();
            let turn_usage = usage.clone();
            let _ = event_tx.send(EventEnvelope::new(
                source_id.clone(),
                next_seq(&mut seq),
                None,
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::types::StopReason::EndTurn,
                    usage: turn_usage,
                },
            ));
            let _ = event_tx.send(EventEnvelope::new(
                source_id,
                next_seq(&mut seq),
                None,
                AgentEvent::RunCompleted {
                    session_id: id.clone(),
                    result: "ok".to_string(),
                    usage,
                },
            ));
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
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        Ok(())
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        if !self.sessions.read().await.contains_key(id) {
            return self
                .archived_views
                .read()
                .await
                .get(id)
                .cloned()
                .ok_or_else(|| SessionError::NotFound { id: id.clone() });
        }
        Ok(SessionView {
            state: SessionInfo {
                session_id: id.clone(),
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                is_active: false,
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
        Ok(self
            .sessions
            .read()
            .await
            .keys()
            .map(|id| SessionSummary {
                session_id: id.clone(),
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                is_active: false,
                labels: Default::default(),
            })
            .collect())
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        let archive_delay_ms = self
            .archive_delay_ms
            .load(std::sync::atomic::Ordering::Relaxed);
        if archive_delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(archive_delay_ms)).await;
        }
        let removed = self.sessions.write().await.remove(id);
        self.pending_context.write().await.remove(id);
        self.event_txs.write().await.remove(id);
        if removed.is_some() {
            self.archived_views.write().await.insert(
                id.clone(),
                SessionView {
                    state: SessionInfo {
                        session_id: id.clone(),
                        created_at: SystemTime::now(),
                        updated_at: SystemTime::now(),
                        message_count: 0,
                        is_active: false,
                        last_assistant_text: None,
                        labels: Default::default(),
                    },
                    billing: SessionUsage {
                        total_tokens: 0,
                        usage: Usage::default(),
                    },
                },
            );
            Ok(())
        } else {
            Err(SessionError::NotFound { id: id.clone() })
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionServiceCommsExt for LocalSessionService {
    async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .map(|session| session.clone() as Arc<dyn CoreCommsRuntime>)
    }

    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        let sessions = self.sessions.read().await;
        let runtime = sessions.get(session_id)?;
        runtime.event_injector()
    }

    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        let sessions = self.sessions.read().await;
        let runtime = sessions.get(session_id)?;
        runtime.interaction_event_injector()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionServiceControlExt for LocalSessionService {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() }.into());
        }
        let mut pending = self.pending_context.write().await;
        let entry = pending.entry(id.clone()).or_default();
        if let Some(key) = req.idempotency_key.as_deref()
            && entry
                .iter()
                .any(|existing| existing.idempotency_key.as_deref() == Some(key))
        {
            return Ok(AppendSystemContextResult {
                status: AppendSystemContextStatus::Staged,
            });
        }
        entry.push(req);
        Ok(AppendSystemContextResult {
            status: AppendSystemContextStatus::Staged,
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobSessionService for LocalSessionService {
    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        let txs = self.event_txs.read().await;
        let tx = txs
            .get(session_id)
            .ok_or_else(|| StreamError::NotFound(format!("session {session_id}")))?;
        let rx = tx.subscribe();
        Ok(Box::pin(futures::stream::unfold(rx, |mut rx| async {
            loop {
                match rx.recv().await {
                    Ok(event) => return Some((event, rx)),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        })))
    }

    fn supports_persistent_sessions(&self) -> bool {
        true
    }

    async fn session_belongs_to_mob(&self, _session_id: &SessionId, _mob_id: &MobId) -> bool {
        true
    }
}

impl MobMcpState {
    pub fn new_in_memory_with_archive_delay(delay_ms: u64) -> Arc<Self> {
        let session_service = Arc::new(LocalSessionService::new());
        session_service.set_archive_delay_ms(delay_ms);
        Arc::new(Self::new(session_service))
    }
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

pub struct MobMcpDispatcher {
    state: Arc<MobMcpState>,
    tools: Arc<[Arc<ToolDef>]>,
}

impl MobMcpDispatcher {
    pub fn new(state: Arc<MobMcpState>) -> Self {
        const PRIMER: &str = "A mob is a managed multi-agent team with shared lifecycle/events: \
            create -> spawn members -> wire/unwire trust -> stop/resume -> complete/destroy.";
        const COMMON: &str = "Use real mob tools (no simulation). Keep and reuse the returned \
            mob_id from mob_create. All mob_* tools except mob_create and mob_list require mob_id.";
        let tools = vec![
            // ── Mob-level (mob_*) ──────────────────────────────────────
            tool(
                "mob_create",
                &format!(
                    "{PRIMER} Create a new mob. Provide exactly one of: prefab or definition. \
                     Returns mob_id."
                ),
                json!({"type":"object","properties":{"prefab":{"type":"string"},"definition":{"type":"object"}}}),
            ),
            tool(
                "mob_list",
                &format!("List mobs or get detail for one. Omit mob_id for summary of all mobs; \
                     provide mob_id for detailed status of that mob. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"}}}),
            ),
            tool(
                "mob_lifecycle",
                &format!("Lifecycle action on a mob. action: stop | resume | reset | complete | destroy. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"action":{"type":"string","enum":["stop","resume","reset","complete","destroy"]}},"required":["mob_id","action"]}),
            ),
            tool(
                "mob_events",
                &format!("Fetch mob lifecycle events. Optional: after_cursor, limit. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"after_cursor":{"type":"integer"},"limit":{"type":"integer"}},"required":["mob_id"]}),
            ),
            tool(
                "mob_run_flow",
                &format!("Start a configured flow run. Required: mob_id, flow_id. Optional params object. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"flow_id":{"type":"string"},"params":{"type":"object"}},"required":["mob_id","flow_id"]}),
            ),
            tool(
                "mob_flow_status",
                &format!("Read flow run status and ledgers by run_id. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"run_id":{"type":"string"}},"required":["mob_id","run_id"]}),
            ),
            tool(
                "mob_cancel_flow",
                &format!("Cancel an in-flight flow run by run_id. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"run_id":{"type":"string"}},"required":["mob_id","run_id"]}),
            ),
            // ── Member-level (meerkat_*) ───────────────────────────────
            tool(
                "meerkat_spawn",
                &format!("Spawn one or more meerkats. Required: mob_id, specs[].profile, specs[].meerkat_id. \
                     Optional per-spec: backend=subagent|external, runtime_mode=autonomous_host|turn_driven, \
                     initial_message, resume_session_id, labels (key-value map), context (opaque JSON). {COMMON}"),
                json!({
                    "type":"object",
                    "properties":{
                        "mob_id":{"type":"string"},
                        "specs":{
                            "type":"array",
                            "items":{
                                "type":"object",
                                "properties":{
                                    "profile":{"type":"string"},
                                    "meerkat_id":{"type":"string"},
                                    "initial_message":{"type":"string"},
                                    "backend":{"type":"string","enum":["subagent","external"]},
                                    "runtime_mode":{"type":"string","enum":["autonomous_host","turn_driven"]},
                                    "resume_session_id":{"type":"string"},
                                    "labels":{"type":"object","additionalProperties":{"type":"string"}},
                                    "context":{"type":"object"}
                                },
                                "required":["profile","meerkat_id"]
                            }
                        }
                    },
                    "required":["mob_id","specs"]
                }),
            ),
            tool(
                "meerkat_retire",
                &format!("Retire a spawned meerkat by ID. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"meerkat_id":{"type":"string"}},"required":["mob_id","meerkat_id"]}),
            ),
            tool(
                "meerkat_list",
                &format!("List current meerkats in a mob. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"}},"required":["mob_id"]}),
            ),
            tool(
                "meerkat_wire",
                &format!("Wire or unwire bidirectional trust between two meerkats. \
                     action: wire | unwire. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"a":{"type":"string"},"b":{"type":"string"},"action":{"type":"string","enum":["wire","unwire"]}},"required":["mob_id","a","b","action"]}),
            ),
            tool(
                "meerkat_message",
                &format!("Send an external message to a spawned meerkat. Required: mob_id, meerkat_id, message. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"meerkat_id":{"type":"string"},"message":{"type":"string"}},"required":["mob_id","meerkat_id","message"]}),
            ),
            tool(
                "mob_respawn",
                &format!("Retire and re-spawn a meerkat with the same profile. \
                     Required: mob_id, meerkat_id. Optional: initial_message. \
                     Returns once retire completes and respawn is enqueued. {COMMON}"),
                json!({"type":"object","properties":{"mob_id":{"type":"string"},"meerkat_id":{"type":"string"},"initial_message":{"type":"string"}},"required":["mob_id","meerkat_id"]}),
            ),
        ]
        .into();
        Self { state, tools }
    }
}

fn tool(name: &str, description: &str, input_schema: serde_json::Value) -> Arc<ToolDef> {
    Arc::new(ToolDef {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
    })
}

fn encode(call: ToolCallView<'_>, payload: serde_json::Value) -> Result<ToolResult, ToolError> {
    let content = serde_json::to_string(&payload)
        .map_err(|e| ToolError::execution_failed(format!("encode result: {e}")))?;
    Ok(ToolResult {
        tool_use_id: call.id.to_string(),
        content,
        is_error: false,
    })
}

fn map_mob_err(call: ToolCallView<'_>, err: MobError) -> ToolError {
    ToolError::execution_failed(format!("tool '{}' failed: {err}", call.name))
}

#[derive(Deserialize)]
struct MobCreateArgs {
    prefab: Option<String>,
    definition: Option<MobDefinition>,
}
#[derive(Deserialize)]
struct MobListArgs {
    #[serde(default)]
    mob_id: Option<String>,
}
#[derive(Deserialize)]
struct LifecycleArgs {
    mob_id: String,
    action: String,
}
#[derive(Deserialize)]
struct MobIdArgs {
    mob_id: String,
}
#[derive(Deserialize)]
struct MobSpawnMeerkatArgs {
    profile: String,
    meerkat_id: String,
    #[serde(default)]
    initial_message: Option<String>,
    #[serde(default)]
    backend: Option<MobBackendKind>,
    #[serde(default)]
    runtime_mode: Option<MobRuntimeMode>,
    #[serde(default)]
    resume_session_id: Option<String>,
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    context: Option<serde_json::Value>,
    #[serde(default)]
    additional_instructions: Option<Vec<String>>,
}
#[derive(Deserialize)]
struct SpawnManyMeerkatsArgs {
    mob_id: String,
    specs: Vec<MobSpawnMeerkatArgs>,
}
#[derive(Deserialize)]
struct RetireArgs {
    mob_id: String,
    meerkat_id: String,
}
#[derive(Deserialize)]
struct WireActionArgs {
    mob_id: String,
    a: String,
    b: String,
    action: String,
}
#[derive(Deserialize)]
struct MessageArgs {
    mob_id: String,
    meerkat_id: String,
    message: String,
}
#[derive(Deserialize)]
struct RunFlowArgs {
    mob_id: String,
    flow_id: String,
    #[serde(default)]
    params: serde_json::Value,
}
#[derive(Deserialize)]
struct FlowStatusArgs {
    mob_id: String,
    run_id: String,
}
#[derive(Deserialize)]
struct EventsArgs {
    mob_id: String,
    #[serde(default)]
    after_cursor: u64,
    #[serde(default = "default_limit")]
    limit: usize,
}
#[derive(Deserialize)]
struct RespawnArgs {
    mob_id: String,
    meerkat_id: String,
    #[serde(default)]
    initial_message: Option<String>,
}
fn default_limit() -> usize {
    100
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for MobMcpDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.tools.clone()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        let started = Instant::now();
        tracing::info!(
            target: "mob_tools",
            "MobMcpDispatcher::dispatch start tool={} tool_use_id={}",
            call.name,
            call.id
        );
        let result = match call.name {
            // ── Mob-level ──────────────────────────────────────────────
            "mob_create" => {
                let args: MobCreateArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                if args.prefab.is_some() && args.definition.is_some() {
                    return Err(ToolError::invalid_arguments(
                        call.name,
                        "provide exactly one of prefab or definition",
                    ));
                }
                let mob_id = if let Some(prefab) = args.prefab {
                    let p = Prefab::from_key(&prefab)
                        .ok_or_else(|| ToolError::invalid_arguments(call.name, "unknown prefab"))?;
                    self.state
                        .mob_create_prefab(p)
                        .await
                        .map_err(|e| map_mob_err(call, e))?
                } else if let Some(definition) = args.definition {
                    self.state
                        .mob_create_definition(definition)
                        .await
                        .map_err(|e| map_mob_err(call, e))?
                } else {
                    return Err(ToolError::invalid_arguments(
                        call.name,
                        "provide prefab or definition",
                    ));
                };
                encode(call, json!({"mob_id": mob_id}))
            }
            "mob_list" => {
                let args: MobListArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                if let Some(mob_id) = args.mob_id {
                    let status = self
                        .state
                        .mob_status(&MobId::from(mob_id))
                        .await
                        .map_err(|e| map_mob_err(call, e))?;
                    encode(call, json!({"status": status.as_str()}))
                } else {
                    let mobs = self.state.mob_list().await;
                    encode(
                        call,
                        json!({"mobs": mobs.into_iter().map(|(id, status)| json!({"mob_id": id, "status": status.as_str()})).collect::<Vec<_>>() }),
                    )
                }
            }
            "mob_lifecycle" => {
                let args: LifecycleArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let mob_id = MobId::from(args.mob_id);
                match args.action.as_str() {
                    "stop" => self
                        .state
                        .mob_stop(&mob_id)
                        .await
                        .map_err(|e| map_mob_err(call, e))?,
                    "resume" => self
                        .state
                        .mob_resume(&mob_id)
                        .await
                        .map_err(|e| map_mob_err(call, e))?,
                    "reset" => self
                        .state
                        .mob_reset(&mob_id)
                        .await
                        .map_err(|e| map_mob_err(call, e))?,
                    "complete" => self
                        .state
                        .mob_complete(&mob_id)
                        .await
                        .map_err(|e| map_mob_err(call, e))?,
                    "destroy" => self
                        .state
                        .mob_destroy(&mob_id)
                        .await
                        .map_err(|e| map_mob_err(call, e))?,
                    other => {
                        return Err(ToolError::invalid_arguments(
                            call.name,
                            format!("unknown lifecycle action: {other}"),
                        ));
                    }
                }
                encode(call, json!({"ok": true}))
            }
            "mob_events" => {
                let args: EventsArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let events = self
                    .state
                    .mob_events(&MobId::from(args.mob_id), args.after_cursor, args.limit)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"events": events}))
            }
            "mob_run_flow" => {
                let args: RunFlowArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let run_id = self
                    .state
                    .mob_run_flow(
                        &MobId::from(args.mob_id),
                        FlowId::from(args.flow_id),
                        args.params,
                    )
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"run_id": run_id}))
            }
            "mob_flow_status" => {
                let args: FlowStatusArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let run_id = args.run_id.parse::<RunId>().map_err(|e| {
                    ToolError::invalid_arguments(call.name, format!("invalid run_id: {e}"))
                })?;
                let run = self
                    .state
                    .mob_flow_status(&MobId::from(args.mob_id), run_id)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"run": run}))
            }
            "mob_cancel_flow" => {
                let args: FlowStatusArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let run_id = args.run_id.parse::<RunId>().map_err(|e| {
                    ToolError::invalid_arguments(call.name, format!("invalid run_id: {e}"))
                })?;
                self.state
                    .mob_cancel_flow(&MobId::from(args.mob_id), run_id)
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"ok": true}))
            }
            // ── Member-level ───────────────────────────────────────────
            "meerkat_spawn" => {
                let args: SpawnManyMeerkatsArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let specs = args
                    .specs
                    .into_iter()
                    .map(|spec| {
                        let resume_session_id = spec
                            .resume_session_id
                            .map(|s| {
                                SessionId::parse(&s).map_err(|e| {
                                    ToolError::invalid_arguments(
                                        call.name,
                                        format!("invalid resume_session_id: {e}"),
                                    )
                                })
                            })
                            .transpose()?;
                        let mut s = SpawnMemberSpec::new(spec.profile, spec.meerkat_id);
                        s.initial_message = spec.initial_message;
                        s.runtime_mode = spec.runtime_mode;
                        s.backend = spec.backend;
                        s.context = spec.context;
                        s.labels = spec.labels;
                        s.resume_session_id = resume_session_id;
                        s.additional_instructions = spec.additional_instructions;
                        Ok(s)
                    })
                    .collect::<Result<Vec<_>, ToolError>>()?;
                // Single-spec fast path returns flat member_ref; multi-spec returns results array.
                if specs.len() == 1 {
                    // SAFETY: len checked above
                    let Some(spec) = specs.into_iter().next() else {
                        unreachable!()
                    };
                    let member_ref = self
                        .state
                        .mob_spawn_spec(&MobId::from(args.mob_id), spec)
                        .await
                        .map_err(|e| map_mob_err(call, e))?;
                    encode(
                        call,
                        json!({"ok": true, "member_ref": member_ref, "session_id": member_ref.session_id()}),
                    )
                } else {
                    let results = self
                        .state
                        .mob_spawn_many(&MobId::from(args.mob_id), specs)
                        .await
                        .map_err(|e| map_mob_err(call, e))?;
                    let results = results
                        .into_iter()
                        .map(|result| match result {
                            Ok(member_ref) => json!({
                                "ok": true,
                                "member_ref": member_ref,
                                "session_id": member_ref.session_id(),
                            }),
                            Err(error) => json!({
                                "ok": false,
                                "error": error.to_string(),
                            }),
                        })
                        .collect::<Vec<_>>();
                    encode(call, json!({"results": results}))
                }
            }
            "meerkat_retire" => {
                let args: RetireArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                self.state
                    .mob_retire(&MobId::from(args.mob_id), MeerkatId::from(args.meerkat_id))
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"ok": true}))
            }
            "meerkat_list" => {
                let args: MobIdArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let rows = self
                    .state
                    .mob_list_members(&MobId::from(args.mob_id))
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"members": rows}))
            }
            "meerkat_wire" => {
                let args: WireActionArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let mob_id = MobId::from(args.mob_id);
                let a = MeerkatId::from(args.a);
                let b = MeerkatId::from(args.b);
                match args.action.as_str() {
                    "wire" => self
                        .state
                        .mob_wire(&mob_id, a, b)
                        .await
                        .map_err(|e| map_mob_err(call, e))?,
                    "unwire" => self
                        .state
                        .mob_unwire(&mob_id, a, b)
                        .await
                        .map_err(|e| map_mob_err(call, e))?,
                    other => {
                        return Err(ToolError::invalid_arguments(
                            call.name,
                            format!("unknown wire action: {other}"),
                        ));
                    }
                }
                encode(call, json!({"ok": true}))
            }
            "meerkat_message" => {
                let args: MessageArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let session_id = self
                    .state
                    .mob_send_message(
                        &MobId::from(args.mob_id),
                        MeerkatId::from(args.meerkat_id),
                        args.message,
                    )
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"ok": true, "session_id": session_id}))
            }
            "mob_respawn" => {
                let args: RespawnArgs = call
                    .parse_args()
                    .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
                let meerkat_id_str = args.meerkat_id;
                self.state
                    .mob_respawn(
                        &MobId::from(args.mob_id),
                        MeerkatId::from(meerkat_id_str.as_str()),
                        args.initial_message,
                    )
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(
                    call,
                    json!({
                        "meerkat_id": meerkat_id_str,
                        "status": "respawn_enqueued"
                    }),
                )
            }
            _ => Err(ToolError::not_found(call.name)),
        };
        match &result {
            Ok(_) => tracing::info!(
                target: "mob_tools",
                "MobMcpDispatcher::dispatch ok tool={} elapsed_ms={}",
                call.name,
                started.elapsed().as_millis()
            ),
            Err(err) => tracing::warn!(
                target: "mob_tools",
                "MobMcpDispatcher::dispatch err tool={} elapsed_ms={} err={}",
                call.name,
                started.elapsed().as_millis(),
                err
            ),
        }
        result
    }
}

#[derive(Debug, Clone)]
pub struct McpToolError {
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

pub fn tools_list() -> Vec<serde_json::Value> {
    let dispatcher = MobMcpDispatcher::new(MobMcpState::new_in_memory());
    dispatcher
        .tools()
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": tool.input_schema
            })
        })
        .collect()
}

pub async fn handle_tools_call(
    state: &Arc<MobMcpState>,
    name: &str,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, McpToolError> {
    let dispatcher = MobMcpDispatcher::new(state.clone());
    let raw = serde_json::value::RawValue::from_string(arguments.to_string()).map_err(|e| {
        McpToolError {
            code: -32602,
            message: format!("invalid arguments: {e}"),
            data: None,
        }
    })?;
    let result = dispatcher
        .dispatch(ToolCallView {
            id: "mcp-tool-call",
            name,
            args: &raw,
        })
        .await
        .map_err(|e| McpToolError {
            code: -32000,
            message: e.to_string(),
            data: None,
        })?;
    serde_json::from_str(&result.content).map_err(|e| McpToolError {
        code: -32603,
        message: format!("invalid tool result payload: {e}"),
        data: Some(json!({"raw": result.content})),
    })
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::collapsible_if,
    clippy::panic
)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::InteractionId;
    use meerkat_core::PlainEventSource;
    use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
    use meerkat_core::comms::{CommsCommand, SendError, SendReceipt};
    use meerkat_core::event::AgentEvent;
    use meerkat_core::event_injector::{
        EventInjector, EventInjectorError, InteractionSubscription, SubscribableInjector,
    };
    use meerkat_core::service::InitialTurnPolicy;
    use meerkat_core::service::SessionService;
    use meerkat_core::service::{
        CreateSessionRequest, SessionError, SessionInfo, SessionQuery, SessionSummary,
        SessionUsage, SessionView, StartTurnRequest,
    };
    use meerkat_core::types::{RunResult, SessionId, Usage};
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::SystemTime;
    use tokio::sync::Notify;
    use tokio::time::{Duration, Instant, sleep};

    struct MockComms {
        key: String,
        trusted: RwLock<HashSet<String>>,
        notify: Arc<Notify>,
    }

    struct MockInjector;

    impl EventInjector for MockInjector {
        fn inject(
            &self,
            _body: String,
            _source: PlainEventSource,
        ) -> Result<(), EventInjectorError> {
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

    impl MockComms {
        fn new(name: &str) -> Self {
            Self {
                key: format!("ed25519:{name}"),
                trusted: RwLock::new(HashSet::new()),
                notify: Arc::new(Notify::new()),
            }
        }
    }

    #[async_trait]
    impl CoreCommsRuntime for MockComms {
        fn public_key(&self) -> Option<String> {
            Some(self.key.clone())
        }

        async fn add_trusted_peer(
            &self,
            peer: meerkat_core::comms::TrustedPeerSpec,
        ) -> Result<(), SendError> {
            self.trusted.write().await.insert(peer.peer_id);
            Ok(())
        }

        async fn remove_trusted_peer(&self, peer_id: &str) -> Result<bool, SendError> {
            Ok(self.trusted.write().await.remove(peer_id))
        }

        async fn send(&self, _cmd: CommsCommand) -> Result<SendReceipt, SendError> {
            Ok(SendReceipt::InputAccepted {
                interaction_id: meerkat_core::interaction::InteractionId(uuid::Uuid::nil()),
                stream_reserved: false,
            })
        }

        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<Notify> {
            self.notify.clone()
        }
    }

    struct MockSessionSvc {
        sessions: RwLock<HashMap<SessionId, Arc<MockComms>>>,
        host_mode_notifiers: RwLock<HashMap<SessionId, Arc<Notify>>>,
        counter: AtomicU64,
        start_turn_delay_ms: AtomicU64,
    }

    impl MockSessionSvc {
        fn new() -> Self {
            Self {
                sessions: RwLock::new(HashMap::new()),
                host_mode_notifiers: RwLock::new(HashMap::new()),
                counter: AtomicU64::new(0),
                start_turn_delay_ms: AtomicU64::new(0),
            }
        }

        fn set_turn_delay_ms(&self, delay_ms: u64) {
            self.start_turn_delay_ms.store(delay_ms, Ordering::Relaxed);
        }
    }

    #[async_trait]
    impl SessionService for MockSessionSvc {
        async fn create_session(
            &self,
            req: CreateSessionRequest,
        ) -> Result<RunResult, SessionError> {
            let sid = SessionId::new();
            let n = self.counter.fetch_add(1, Ordering::Relaxed);
            let name = req
                .build
                .and_then(|b| b.comms_name)
                .unwrap_or_else(|| format!("s-{n}"));
            self.sessions
                .write()
                .await
                .insert(sid.clone(), Arc::new(MockComms::new(&name)));
            self.host_mode_notifiers
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
            if !self.sessions.read().await.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            let delay_ms = self.start_turn_delay_ms.load(Ordering::Relaxed);
            if delay_ms > 0 {
                sleep(Duration::from_millis(delay_ms)).await;
            }
            if req.host_mode {
                let notifier = self
                    .host_mode_notifiers
                    .read()
                    .await
                    .get(id)
                    .cloned()
                    .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
                notifier.notified().await;
                return Ok(RunResult {
                    text: "ok".to_string(),
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
            if let Some(notifier) = self.host_mode_notifiers.read().await.get(id).cloned() {
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
            Ok(self
                .sessions
                .read()
                .await
                .keys()
                .map(|id| SessionSummary {
                    session_id: id.clone(),
                    created_at: SystemTime::now(),
                    updated_at: SystemTime::now(),
                    message_count: 0,
                    total_tokens: 0,
                    is_active: false,
                    labels: Default::default(),
                })
                .collect())
        }

        async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
            self.sessions.write().await.remove(id);
            if let Some(notifier) = self.host_mode_notifiers.write().await.remove(id) {
                notifier.notify_waiters();
            }
            Ok(())
        }
    }

    #[async_trait]
    impl SessionServiceCommsExt for MockSessionSvc {
        async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
            self.sessions
                .read()
                .await
                .get(session_id)
                .map(|s| s.clone() as Arc<dyn CoreCommsRuntime>)
        }

        async fn event_injector(
            &self,
            session_id: &SessionId,
        ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
            if !self.sessions.read().await.contains_key(session_id) {
                return None;
            }
            Some(Arc::new(MockInjector))
        }

        async fn interaction_event_injector(
            &self,
            session_id: &SessionId,
        ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
            if !self.sessions.read().await.contains_key(session_id) {
                return None;
            }
            Some(Arc::new(MockInjector))
        }
    }

    #[async_trait]
    impl SessionServiceControlExt for MockSessionSvc {
        async fn append_system_context(
            &self,
            id: &SessionId,
            _req: AppendSystemContextRequest,
        ) -> Result<AppendSystemContextResult, SessionControlError> {
            if !self.sessions.read().await.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() }.into());
            }
            Ok(AppendSystemContextResult {
                status: AppendSystemContextStatus::Staged,
            })
        }
    }

    #[async_trait]
    impl MobSessionService for MockSessionSvc {
        fn supports_persistent_sessions(&self) -> bool {
            true
        }

        async fn session_belongs_to_mob(&self, _session_id: &SessionId, _mob_id: &MobId) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn local_session_service_persists_appended_context() {
        let service = LocalSessionService::new();
        let run = service
            .create_session(CreateSessionRequest {
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string(),
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                host_mode: false,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                build: None,
                labels: None,
            })
            .await
            .expect("create session");
        let session_id = run.session_id;

        let result = service
            .append_system_context(
                &session_id,
                AppendSystemContextRequest {
                    text: "Remember the customer preference.".to_string(),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-1".to_string()),
                },
            )
            .await
            .expect("append context");
        assert_eq!(result.status, AppendSystemContextStatus::Staged);
        let pending = service.pending_context.read().await;
        assert_eq!(pending.get(&session_id).map(std::vec::Vec::len), Some(1));
    }

    #[tokio::test]
    async fn local_session_service_consumes_staged_context_on_next_turn() {
        use futures::StreamExt;

        let service = LocalSessionService::new();
        let run = service
            .create_session(CreateSessionRequest {
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string(),
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                host_mode: false,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                build: None,
                labels: None,
            })
            .await
            .expect("create session");
        let session_id = run.session_id;

        service
            .append_system_context(
                &session_id,
                AppendSystemContextRequest {
                    text: "Remember the customer preference.".to_string(),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-1".to_string()),
                },
            )
            .await
            .expect("append context");

        let mut stream =
            meerkat_mob::MobSessionService::subscribe_session_events(&service, &session_id)
                .await
                .expect("subscribe events");
        service
            .start_turn(
                &session_id,
                StartTurnRequest {
                    prompt: "hello".to_string(),
                    event_tx: None,
                    host_mode: false,
                    skill_references: None,
                    flow_tool_overlay: None,
                    additional_instructions: None,
                },
            )
            .await
            .expect("start turn");

        let first = stream.next().await.expect("first event");
        match first.payload {
            AgentEvent::RunStarted { prompt, .. } => {
                assert!(prompt.contains("Remember the customer preference."));
                assert!(prompt.contains("hello"));
            }
            other => panic!("expected RunStarted, got {other:?}"),
        }

        let pending = service.pending_context.read().await;
        assert_eq!(pending.get(&session_id).map(std::vec::Vec::len), Some(0));
    }

    #[tokio::test]
    async fn local_session_service_archive_drops_staged_context() {
        let service = LocalSessionService::new();
        let run = service
            .create_session(CreateSessionRequest {
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string(),
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                host_mode: false,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                build: None,
                labels: None,
            })
            .await
            .expect("create session");
        let session_id = run.session_id;

        service
            .append_system_context(
                &session_id,
                AppendSystemContextRequest {
                    text: "Remember the customer preference.".to_string(),
                    source: Some("mob".to_string()),
                    idempotency_key: Some("ctx-archive".to_string()),
                },
            )
            .await
            .expect("append context");

        service.archive(&session_id).await.expect("archive session");

        let pending = service.pending_context.read().await;
        assert!(
            !pending.contains_key(&session_id),
            "archive must drop unapplied staged context"
        );
    }

    #[tokio::test]
    async fn local_session_service_stream_emits_events_on_turn() {
        use futures::StreamExt;

        let service = LocalSessionService::new();
        let run = service
            .create_session(CreateSessionRequest {
                model: "claude-sonnet-4-5".to_string(),
                prompt: "hello".to_string(),
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                host_mode: false,
                skill_references: None,
                initial_turn: InitialTurnPolicy::Defer,
                build: None,
                labels: None,
            })
            .await
            .expect("create session");
        let session_id = run.session_id;

        let mut stream =
            meerkat_mob::MobSessionService::subscribe_session_events(&service, &session_id)
                .await
                .expect("subscribe events");
        let turn = service
            .start_turn(
                &session_id,
                StartTurnRequest {
                    prompt: "hello".to_string(),
                    event_tx: None,
                    host_mode: false,
                    skill_references: None,
                    flow_tool_overlay: None,
                    additional_instructions: None,
                },
            )
            .await;
        assert!(turn.is_ok());

        let first = stream.next().await.expect("first event");
        assert!(matches!(first.payload, AgentEvent::RunStarted { .. }));
        let second = stream.next().await.expect("second event");
        assert!(matches!(second.payload, AgentEvent::TurnStarted { .. }));
    }

    fn mk_call<'a>(name: &'a str, args: &'a serde_json::value::RawValue) -> ToolCallView<'a> {
        ToolCallView {
            id: "t1",
            name,
            args,
        }
    }

    async fn call_tool(
        d: &MobMcpDispatcher,
        name: &str,
        args: serde_json::Value,
    ) -> serde_json::Value {
        let raw = serde_json::value::RawValue::from_string(args.to_string()).expect("raw args");
        let out = d.dispatch(mk_call(name, &raw)).await.expect("tool call");
        serde_json::from_str(&out.content).expect("tool json")
    }

    async fn call_tool_err(d: &MobMcpDispatcher, name: &str, args: serde_json::Value) -> ToolError {
        let raw = serde_json::value::RawValue::from_string(args.to_string()).expect("raw args");
        d.dispatch(mk_call(name, &raw))
            .await
            .expect_err("tool call should fail")
    }

    #[tokio::test]
    async fn test_dispatcher_exposes_13_tools() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));
        let d = MobMcpDispatcher::new(state);
        assert_eq!(d.tools().len(), 13);
    }

    fn flow_enabled_definition() -> MobDefinition {
        MobDefinition::from_toml(
            r#"
[mob]
id = "flow-mob"
orchestrator = "lead"

[profiles.lead]
model = "claude-opus-4-6"
external_addressable = true
peer_description = "Lead"

[profiles.lead.tools]
comms = true
mob = true

[profiles.worker]
model = "claude-sonnet-4-5"
external_addressable = false
peer_description = "Worker"

[profiles.worker.tools]
comms = true
mob = true

[wiring]
auto_wire_orchestrator = false
role_wiring = []

[backend]
default = "subagent"

[flows.demo]
description = "demo flow"

[flows.demo.steps.start]
role = "worker"
message = "run demo"
timeout_ms = 1000
"#,
        )
        .expect("flow mob definition should parse")
    }

    #[tokio::test]
    async fn test_multi_mob_isolation() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));
        let d = MobMcpDispatcher::new(state);

        let a = call_tool(&d, "mob_create", json!({"prefab":"coding_swarm"})).await["mob_id"]
            .as_str()
            .unwrap()
            .to_string();
        let b = call_tool(&d, "mob_create", json!({"prefab":"code_review"})).await["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        call_tool(
            &d,
            "meerkat_spawn",
            json!({"mob_id": a, "specs":[{"profile":"worker", "meerkat_id":"wa"}]}),
        )
        .await;
        call_tool(
            &d,
            "meerkat_spawn",
            json!({"mob_id": b, "specs":[{"profile":"worker", "meerkat_id":"wb"}]}),
        )
        .await;

        let la = call_tool(&d, "meerkat_list", json!({"mob_id": a})).await;
        let lb = call_tool(&d, "meerkat_list", json!({"mob_id": b})).await;
        assert_eq!(la["members"].as_array().unwrap().len(), 1); // wa
        assert_eq!(lb["members"].as_array().unwrap().len(), 1); // wb
    }

    #[tokio::test]
    async fn test_mcp_e2e_flow_and_destroy_removes_mob() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));
        let d = MobMcpDispatcher::new(state);

        let mob_id = call_tool(&d, "mob_create", json!({"prefab":"coding_swarm"})).await["mob_id"]
            .as_str()
            .unwrap()
            .to_string();
        call_tool(
            &d,
            "meerkat_spawn",
            json!({"mob_id": mob_id, "specs":[{"profile":"lead", "meerkat_id":"lead"}]}),
        )
        .await;
        call_tool(
            &d,
            "meerkat_spawn",
            json!({"mob_id": mob_id, "specs":[{"profile":"worker", "meerkat_id":"w1"}]}),
        )
        .await;
        call_tool(
            &d,
            "meerkat_spawn",
            json!({"mob_id": mob_id, "specs":[{"profile":"worker", "meerkat_id":"w2"}]}),
        )
        .await;
        call_tool(
            &d,
            "meerkat_wire",
            json!({"mob_id": mob_id, "a":"w1", "b":"w2", "action":"wire"}),
        )
        .await;
        let _ = call_tool(
            &d,
            "meerkat_message",
            json!({"mob_id": mob_id, "meerkat_id":"lead", "message":"ping"}),
        )
        .await;
        let listed = call_tool(&d, "meerkat_list", json!({"mob_id": mob_id})).await;
        assert_eq!(
            listed["members"].as_array().map(std::vec::Vec::len),
            Some(3)
        );
        call_tool(
            &d,
            "meerkat_wire",
            json!({"mob_id": mob_id, "a":"w1", "b":"w2", "action":"unwire"}),
        )
        .await;
        call_tool(
            &d,
            "meerkat_retire",
            json!({"mob_id": mob_id, "meerkat_id":"w2"}),
        )
        .await;
        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": mob_id, "action":"complete"}),
        )
        .await;
        let events = call_tool(
            &d,
            "mob_events",
            json!({"mob_id": mob_id, "after_cursor":0, "limit":50}),
        )
        .await;
        let events = events["events"].as_array().cloned().unwrap_or_default();
        assert!(
            events
                .iter()
                .any(|e| e["kind"]["type"] == "meerkat_spawned"),
            "expected structural events to include meerkat_spawned"
        );
        assert!(
            events.iter().any(|e| e["kind"]["type"] == "mob_completed"),
            "expected structural events to include mob_completed"
        );
        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": mob_id, "action":"destroy"}),
        )
        .await;
        let listed = call_tool(&d, "mob_list", json!({})).await;
        assert!(listed["mobs"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_mcp_stop_resume_round_trip() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));
        let d = MobMcpDispatcher::new(state);

        let mob_id = call_tool(&d, "mob_create", json!({"prefab":"coding_swarm"})).await["mob_id"]
            .as_str()
            .unwrap()
            .to_string();
        call_tool(
            &d,
            "meerkat_spawn",
            json!({"mob_id": mob_id, "specs":[{"profile":"lead", "meerkat_id":"lead"}]}),
        )
        .await;
        call_tool(
            &d,
            "meerkat_spawn",
            json!({"mob_id": mob_id, "specs":[{"profile":"worker", "meerkat_id":"w1"}]}),
        )
        .await;
        call_tool(
            &d,
            "meerkat_spawn",
            json!({"mob_id": mob_id, "specs":[{"profile":"worker", "meerkat_id":"w2"}]}),
        )
        .await;
        call_tool(
            &d,
            "meerkat_wire",
            json!({"mob_id": mob_id, "a":"w1", "b":"w2", "action":"wire"}),
        )
        .await;
        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": mob_id, "action":"stop"}),
        )
        .await;
        call_tool(
            &d,
            "mob_lifecycle",
            json!({"mob_id": mob_id, "action":"resume"}),
        )
        .await;
        let members = call_tool(&d, "meerkat_list", json!({"mob_id": mob_id})).await;
        assert_eq!(members["members"].as_array().unwrap().len(), 3); // lead + 2 workers
        call_tool(
            &d,
            "meerkat_message",
            json!({"mob_id": mob_id, "meerkat_id":"lead", "message":"status"}),
        )
        .await;
        let status = call_tool(&d, "mob_list", json!({"mob_id": mob_id})).await;
        assert_eq!(status["status"], "Running");
    }

    #[tokio::test]
    async fn test_mcp_flow_tools_dispatch_run_status_cancel() {
        let svc = Arc::new(MockSessionSvc::new());
        svc.set_turn_delay_ms(60_000);
        let state = Arc::new(MobMcpState::new(svc));
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(
            &d,
            "mob_create",
            json!({
                "definition": flow_enabled_definition()
            }),
        )
        .await;
        let mob_id = created["mob_id"]
            .as_str()
            .expect("mob_id should be returned")
            .to_string();

        call_tool(
            &d,
            "meerkat_spawn",
            json!({"mob_id": mob_id, "specs":[{"profile":"worker", "meerkat_id":"w1"}]}),
        )
        .await;

        let started = call_tool(
            &d,
            "mob_run_flow",
            json!({
                "mob_id": mob_id,
                "flow_id": "demo",
                "params": {"ticket": "ABC-123"}
            }),
        )
        .await;
        let run_id = started["run_id"]
            .as_str()
            .expect("run_id should be returned")
            .to_string();

        let status = call_tool(
            &d,
            "mob_flow_status",
            json!({
                "mob_id": mob_id,
                "run_id": run_id
            }),
        )
        .await;
        assert_eq!(status["run"]["run_id"], run_id);
        assert_eq!(status["run"]["flow_id"], "demo");

        let canceled = call_tool(
            &d,
            "mob_cancel_flow",
            json!({
                "mob_id": mob_id,
                "run_id": run_id
            }),
        )
        .await;
        assert_eq!(canceled["ok"], true);

        let deadline = Instant::now() + Duration::from_secs(8);
        let mut terminal_status = None;
        while Instant::now() < deadline {
            let polled = call_tool(
                &d,
                "mob_flow_status",
                json!({
                    "mob_id": mob_id.clone(),
                    "run_id": run_id.clone()
                }),
            )
            .await;
            if let Some(status) = polled
                .get("run")
                .and_then(|run| run.get("status"))
                .and_then(|status| status.as_str())
            {
                if matches!(status, "canceled" | "completed" | "failed") {
                    terminal_status = Some(status.to_string());
                    break;
                }
            }
            sleep(Duration::from_millis(25)).await;
        }
        assert!(
            matches!(terminal_status.as_deref(), Some("canceled" | "failed")),
            "mob_cancel_flow should converge to canceled, or failed if terminal failure won the race first"
        );
    }

    #[tokio::test]
    async fn test_mcp_flow_status_rejects_invalid_run_id() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));
        let d = MobMcpDispatcher::new(state);

        let error = call_tool_err(
            &d,
            "mob_flow_status",
            json!({
                "mob_id": "does-not-matter",
                "run_id": "not-a-uuid"
            }),
        )
        .await;
        assert!(
            matches!(error, ToolError::InvalidArguments { .. }),
            "invalid run_id should map to invalid arguments"
        );
    }

    #[tokio::test]
    async fn test_mob_spawn_backend_arg_returns_backend_member_ref() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(
            &d,
            "mob_create",
            json!({
                "definition": {
                    "id": "ext-mob",
                    "orchestrator": {"profile": "lead"},
                    "profiles": {
                        "lead": {
                            "model": "claude-opus-4-6",
                            "tools": {"comms": true},
                            "external_addressable": true
                        },
                        "worker": {
                            "model": "claude-sonnet-4-5",
                            "tools": {"comms": true},
                            "external_addressable": false
                        }
                    },
                    "mcp_servers": {},
                    "wiring": {"auto_wire_orchestrator": false, "role_wiring": []},
                    "skills": {},
                    "backend": {
                        "default": "subagent",
                        "external": {"address_base": "https://backend.example.invalid/mesh"}
                    }
                }
            }),
        )
        .await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();

        let spawned = call_tool(
            &d,
            "meerkat_spawn",
            json!({
                "mob_id": mob_id,
                "specs": [{"profile": "worker", "meerkat_id": "w-ext", "backend": "external"}]
            }),
        )
        .await;
        assert_eq!(spawned["member_ref"]["kind"], "backend_peer");
    }

    #[tokio::test]
    async fn test_mob_spawn_runtime_mode_defaults_and_override() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"prefab":"coding_swarm"})).await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();

        call_tool(
            &d,
            "meerkat_spawn",
            json!({
                "mob_id": mob_id,
                "specs": [{"profile": "lead", "meerkat_id": "lead-default"}]
            }),
        )
        .await;
        call_tool(
            &d,
            "meerkat_spawn",
            json!({
                "mob_id": mob_id,
                "specs": [{"profile": "worker", "meerkat_id": "worker-turn", "runtime_mode": "turn_driven"}]
            }),
        )
        .await;

        let listed = call_tool(&d, "meerkat_list", json!({"mob_id": mob_id})).await;
        let members = listed["members"].as_array().cloned().unwrap_or_default();
        let lead_mode = members
            .iter()
            .find(|m| m["meerkat_id"] == "lead-default")
            .and_then(|m| m["runtime_mode"].as_str());
        let worker_mode = members
            .iter()
            .find(|m| m["meerkat_id"] == "worker-turn")
            .and_then(|m| m["runtime_mode"].as_str());

        assert_eq!(lead_mode, Some("autonomous_host"));
        assert_eq!(worker_mode, Some("turn_driven"));
    }

    #[tokio::test]
    async fn test_mob_spawn_many_dispatches_batch() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"prefab":"coding_swarm"})).await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();

        let spawned = call_tool(
            &d,
            "meerkat_spawn",
            json!({
                "mob_id": mob_id,
                "specs": [
                    {"profile":"worker","meerkat_id":"w-many-a"},
                    {"profile":"worker","meerkat_id":"w-many-b"}
                ]
            }),
        )
        .await;
        let results = spawned["results"].as_array().expect("results array");
        assert_eq!(results.len(), 2, "expected two batch rows");
        assert!(
            results.iter().all(|row| row["ok"] == json!(true)),
            "all batch spawn rows should succeed"
        );

        let listed = call_tool(&d, "meerkat_list", json!({"mob_id": mob_id})).await;
        let ids = listed["members"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| m["meerkat_id"].as_str().map(ToString::to_string))
            .collect::<std::collections::BTreeSet<_>>();
        assert!(ids.contains("w-many-a"));
        assert!(ids.contains("w-many-b"));
    }

    #[tokio::test]
    async fn test_mob_create_rejects_duplicate_mob_id() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));
        let d = MobMcpDispatcher::new(state);

        let created = call_tool(&d, "mob_create", json!({"prefab":"pipeline"})).await;
        let mob_id = created["mob_id"].as_str().unwrap().to_string();
        let error = call_tool_err(&d, "mob_create", json!({"prefab":"pipeline"})).await;
        assert!(
            matches!(error, ToolError::ExecutionFailed { .. }),
            "duplicate mob creation should fail deterministically"
        );

        let listed = call_tool(&d, "mob_list", json!({})).await;
        let ids: Vec<String> = listed["mobs"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|m| m["mob_id"].as_str().map(ToString::to_string))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            ids,
            vec![mob_id],
            "duplicate create must not replace active mob"
        );
    }

    #[tokio::test]
    async fn test_mob_create_rejects_prefab_and_definition_together() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));
        let d = MobMcpDispatcher::new(state);

        let error = call_tool_err(
            &d,
            "mob_create",
            json!({
                "prefab":"pipeline",
                "definition": {
                    "id": "ignored",
                    "orchestrator": {"profile": "lead"},
                    "profiles": {
                        "lead": {
                            "model": "claude-opus-4-6",
                            "tools": {"comms": true},
                            "external_addressable": true
                        }
                    },
                    "mcp_servers": {},
                    "wiring": {"auto_wire_orchestrator": false, "role_wiring": []},
                    "skills": {},
                    "backend": {"default": "subagent"}
                }
            }),
        )
        .await;
        assert!(
            matches!(error, ToolError::InvalidArguments { .. }),
            "mob_create should reject conflicting inputs"
        );
    }
}
