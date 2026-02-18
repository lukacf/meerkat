use std::collections::{BTreeMap, BTreeSet, HashSet, TryReserveError};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::RwLock as StdRwLock;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::{mpsc, oneshot, Mutex, Notify};
use uuid::Uuid;

use meerkat_core::agent::{AgentToolDispatcher, CommsRuntime};
use meerkat_core::comms::TrustedPeerSpec;
use meerkat_core::error::ToolError;
use meerkat_core::service::{
    CreateSessionRequest, SessionError, SessionInfo, SessionQuery, SessionService, SessionSummary,
    SessionUsage, SessionView, StartTurnRequest,
};
use meerkat_core::types::{Message, RunResult, SessionId, ToolCallView, ToolDef, ToolResult, UserMessage};
use meerkat_core::Session;
use meerkat_store::SessionFilter;

use crate::build::{build_agent_config, hydrate_skills_content, to_create_session_request};
use crate::definition::{MobDefinition, OrchestratorConfig};
use crate::error::MobError;
use crate::event::{MobEvent, MobEventKind, NewMobEvent};
use crate::ids::{MeerkatId, ProfileName};
use crate::roster::{Roster, RosterEntry};
use crate::storage::MobStorage;
use crate::tasks::{MobTask, TaskBoard, TaskStatus};
use crate::tools::{MobTaskToolDispatcher, MobToolDispatcher};
use crate::validate::validate;

#[derive(Clone)]
struct MockSessionService {
    sessions: Arc<dyn meerkat_store::SessionStore>,
    lock: Arc<Mutex<()>>,
}

impl MockSessionService {
    fn new(sessions: Arc<dyn meerkat_store::SessionStore>) -> Self {
        Self {
            sessions,
            lock: Arc::new(Mutex::new(())),
        }
    }
}

impl MockSessionService {
    async fn append_prompt(
        session: &mut Session,
        prompt: String,
        system_prompt: Option<String>,
    ) {
        if let Some(prompt) = system_prompt {
            session.set_system_prompt(prompt);
        }
        if !prompt.is_empty() {
            session.push(Message::User(UserMessage { content: prompt }));
        }
    }

    fn mock_run_result(session: &Session, text: impl Into<String>) -> RunResult {
        RunResult {
            text: text.into(),
            session_id: session.id().clone(),
            usage: session.total_usage(),
            turns: 1,
            tool_calls: 0,
            structured_output: None,
            schema_warnings: None,
        }
    }
}

#[derive(Clone, Default)]
struct NoopCommsRuntime {
    notify: Arc<Notify>,
}

#[async_trait::async_trait]
impl CommsRuntime for NoopCommsRuntime {
    async fn drain_messages(&self) -> Vec<String> {
        Vec::new()
    }

    fn inbox_notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }
}

#[async_trait]
impl meerkat_core::service::SessionService for MockSessionService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        let _guard = self.lock.lock().await;
        let mut session = Session::new();
        Self::append_prompt(&mut session, req.prompt.clone(), req.system_prompt).await;
        let _session_id = session.id().clone();
        let result = Self::mock_run_result(&session, req.prompt);
        self.sessions
            .save(&session)
            .await
            .map_err(|err| SessionError::Store(Box::new(err)))?;
        Ok(result)
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        let _guard = self.lock.lock().await;
        let mut session = self
            .sessions
            .load(id)
            .await
            .map_err(|err| SessionError::Store(Box::new(err)))?
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        session.push(Message::User(UserMessage {
            content: req.prompt.clone(),
        }));
        session.touch();
        let result = Self::mock_run_result(&session, format!("{} [mocked]", req.prompt));
        self.sessions
            .save(&session)
            .await
            .map_err(|err| SessionError::Store(Box::new(err)))?;
        Ok(result)
    }

    async fn interrupt(&self, _id: &SessionId) -> Result<(), SessionError> {
        Ok(())
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        let session = self
            .sessions
            .load(id)
            .await
            .map_err(|err| SessionError::Store(Box::new(err)))?
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;
        Ok(SessionView {
            state: SessionInfo {
                session_id: session.id().clone(),
                created_at: session.created_at(),
                updated_at: session.updated_at(),
                message_count: session.messages().len(),
                is_active: false,
                last_assistant_text: session.last_assistant_text(),
            },
            billing: SessionUsage {
                total_tokens: session.total_tokens(),
                usage: session.total_usage(),
            },
        })
    }

    async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        let mut sessions = self
            .sessions
            .list(SessionFilter::default())
            .await
            .map_err(|err| SessionError::Store(Box::new(err)))?
            .into_iter()
            .map(|meta| SessionSummary {
                session_id: meta.id,
                created_at: meta.created_at,
                updated_at: meta.updated_at,
                message_count: meta.message_count,
                total_tokens: meta.total_tokens,
                is_active: false,
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(sessions)
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        self.sessions
            .delete(id)
            .await
            .map_err(|err| SessionError::Store(Box::new(err)))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum MobState {
    Creating = 0,
    Running = 1,
    Stopped = 2,
    Completed = 3,
    Destroyed = 4,
}

impl fmt::Display for MobState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl MobState {
    fn can_transition(self, to: Self) -> bool {
        use MobState::*;
        matches!(
            (self, to),
            (Creating, Running)
                | (Creating, Destroyed)
                | (Running, Stopped)
                | (Running, Completed)
                | (Running, Destroyed)
                | (Stopped, Running)
                | (Stopped, Destroyed)
                | (Completed, Destroyed)
        )
    }
}

impl TryFrom<u8> for MobState {
    type Error = MobError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => Self::Creating,
            1 => Self::Running,
            2 => Self::Stopped,
            3 => Self::Completed,
            4 => Self::Destroyed,
            _ => {
                return Err(MobError::Internal(format!(
                    "invalid mob state value: {value}"
                )))
            }
        })
    }
}

impl From<MobState> for u8 {
    fn from(state: MobState) -> Self {
        state as u8
    }
}

pub(crate) enum MobCommand {
    Spawn {
        profile: ProfileName,
        key: MeerkatId,
        initial_message: Option<String>,
        reply: oneshot::Sender<Result<MeerkatId, MobError>>,
    },
    Retire {
        meerkat_id: MeerkatId,
        reply: oneshot::Sender<Result<(), MobError>>,
    },
    Wire {
        a: MeerkatId,
        b: MeerkatId,
        reply: oneshot::Sender<Result<(), MobError>>,
    },
    Unwire {
        a: MeerkatId,
        b: MeerkatId,
        reply: oneshot::Sender<Result<(), MobError>>,
    },
    ExternalTurn {
        meerkat_id: MeerkatId,
        message: String,
        reply: oneshot::Sender<Result<meerkat_core::types::RunResult, MobError>>,
    },
    InternalTurn {
        meerkat_id: MeerkatId,
        message: String,
        reply: oneshot::Sender<Result<meerkat_core::types::RunResult, MobError>>,
    },
    TaskCreate {
        subject: String,
        description: String,
        blocked_by: Vec<String>,
        reply: oneshot::Sender<Result<String, MobError>>,
    },
    TaskUpdate {
        task_id: String,
        status: Option<TaskStatus>,
        owner: Option<MeerkatId>,
        reply: oneshot::Sender<Result<(), MobError>>,
    },
    Stop {
        reply: oneshot::Sender<Result<(), MobError>>,
    },
    Complete {
        reply: oneshot::Sender<Result<(), MobError>>,
    },
    Destroy {
        reply: oneshot::Sender<Result<(), MobError>>,
    },
}

pub struct MobBuilder {
    definition: Option<MobDefinition>,
    storage: MobStorage,
    session_service: Option<Arc<dyn SessionService>>,
    comms: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
    _rust_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
}

impl MobBuilder {
    pub fn new(definition: MobDefinition, storage: MobStorage) -> Self {
        Self {
            definition: Some(definition),
            storage,
            session_service: None,
            comms: None,
            _rust_bundles: BTreeMap::new(),
        }
    }

    pub fn for_resume(storage: MobStorage) -> Self {
        Self {
            definition: None,
            storage,
            session_service: None,
            comms: None,
            _rust_bundles: BTreeMap::new(),
        }
    }

    pub fn with_session_service(mut self, service: Arc<dyn SessionService>) -> Self {
        self.session_service = Some(service);
        self
    }

    pub fn with_comms_runtime(mut self, comms: Arc<dyn meerkat_core::agent::CommsRuntime>) -> Self {
        self.comms = Some(comms);
        self
    }

    pub fn register_tool_bundle(
        mut self,
        name: String,
        dispatcher: Arc<dyn AgentToolDispatcher>,
    ) -> Self {
        self._rust_bundles.insert(name, dispatcher);
        self
    }

    fn resolve_session_service(&self) -> Arc<dyn SessionService> {
        self.session_service
            .clone()
            .unwrap_or_else(|| Arc::new(MockSessionService::new(self.storage.sessions.clone())))
    }

    fn resolve_comms_runtime(
        &self,
    ) -> Arc<dyn CommsRuntime> {
        self.comms
            .clone()
            .unwrap_or_else(|| Arc::new(NoopCommsRuntime::default()))
    }

    fn parse_definition_from_events(&self, events: &[MobEvent]) -> Option<MobDefinition> {
        events.iter().find_map(|event| match &event.kind {
            MobEventKind::MobCreated { definition } => Some(definition.clone()),
            _ => None,
        })
    }

    fn reconcile_definition_state(events: &[MobEvent]) -> Result<MobState, MobError> {
        if events
            .iter()
            .any(|event| matches!(event.kind, MobEventKind::MobCompleted))
        {
            return Ok(MobState::Completed);
        }
        Ok(MobState::Stopped)
    }

    pub async fn create(mut self) -> Result<MobHandle, MobError> {
        let definition = self.definition.take().ok_or_else(|| {
            MobError::Internal("create requires a definition in MobBuilder".to_string())
        })?;
        validate(&definition)?;

        let skills_content = hydrate_skills_content(&definition).map_err(|err| {
            MobError::Internal(format!("failed to load skills: {err}"))
        })?;
        let definition = Arc::new(definition);
        let storage = self.storage.clone();
        let session_service = self.resolve_session_service();
        let comms = self.resolve_comms_runtime();

        let _ = storage
            .events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                kind: MobEventKind::MobCreated {
                    definition: (*definition).clone(),
                },
                timestamp: Some(Utc::now()),
            })
            .await?;

        let state = Arc::new(AtomicU8::new(MobState::Running as u8));
        let (tx, rx) = mpsc::channel(128);
        let roster = Arc::new(StdRwLock::new(Roster::default()));

        let actor = MobActor {
            definition: definition.clone(),
            storage: storage.clone(),
            session_service: session_service.clone(),
            roster: roster.clone(),
            state: state.clone(),
            comms,
            handle: Arc::new(MobHandle {
                tx: tx.clone(),
                roster: roster.clone(),
                definition: definition.clone(),
                state: state.clone(),
                storage: storage.clone(),
            }),
            _rust_bundles: self._rust_bundles,
            skills_content,
            restore_wiring: Vec::new(),
            rx,
        };

        tokio::spawn(actor.run());

        Ok(MobHandle {
            tx,
            roster,
            definition,
            state,
            storage,
        })
    }

    pub async fn resume(mut self) -> Result<MobHandle, MobError> {
        let events = self.storage.events.replay_all().await?;
        let definition = self
            .definition
            .take()
            .or_else(|| self.parse_definition_from_events(&events))
            .ok_or_else(|| MobError::Internal("missing MobCreated event".to_string()))?;
        if Self::reconcile_definition_state(&events)? == MobState::Completed {
            return Err(MobError::InvalidTransition {
                from: MobState::Completed,
                to: MobState::Running,
            });
        }
        validate(&definition)?;
        let mut roster = project_roster_from_events(&events);
        let skills_content = hydrate_skills_content(&definition).map_err(|err| {
            MobError::Internal(format!("failed to load skills: {err}"))
        })?;

        let definition = Arc::new(definition);
        let storage = self.storage.clone();
        let session_service = self.resolve_session_service();
        let comms = self.resolve_comms_runtime();

        // Orphan reconciliation: session exists but no longer in mob history.
        let stored_summaries = session_service
            .list(SessionQuery::default())
            .await
            .map_err(MobError::from)?;
        let stored_session_ids = stored_summaries.into_iter().map(|summary| summary.session_id).collect::<HashSet<_>>();
        let known_session_ids = roster
            .session_ids()
            .into_iter()
            .collect::<HashSet<_>>();
        for session_id in stored_session_ids
            .difference(&known_session_ids)
            .cloned()
            .collect::<Vec<_>>()
        {
            let _ = session_service.archive(&session_id).await;
        }

        // Crash-before-effect: roster contains entries that no longer exist.
        let missing_sessions = {
            roster
                .list()
                .into_iter()
                .filter_map(|entry| {
                    (!stored_session_ids.contains(&entry.session_id)).then_some(entry)
                })
                .collect::<Vec<_>>()
        };
        for missing in missing_sessions {
            let profile = definition
                .profiles
                .get(&missing.profile)
                .ok_or_else(|| MobError::ProfileNotFound {
                    profile: missing.profile.clone(),
                })?;
            let cfg = build_agent_config(
                &definition.id,
                &missing.profile,
                profile,
                &missing.meerkat_id,
                &skills_content,
                "Peer communication is automatic through mob wiring.",
                resolve_rust_bundle_dispatcher(&self._rust_bundles, &profile.tools.rust_bundles)?,
            );
            let request =
                to_create_session_request(cfg, String::new());
            let run_result = session_service.create_session(request).await?;
            let new_session_id = run_result.session_id;
            let mut entry = roster.remove(&missing.meerkat_id).ok_or_else(|| {
                MobError::MeerkatNotFound {
                    meerkat_id: missing.meerkat_id.clone(),
                }
            })?;
            entry.session_id = new_session_id.clone();
            let _ = roster.add(entry);
            storage
                .events
                .append(NewMobEvent {
                    mob_id: definition.id.clone(),
                    kind: MobEventKind::MeerkatSpawned {
                        meerkat_id: missing.meerkat_id.clone(),
                        role: missing.profile.clone(),
                        session_id: new_session_id,
                    },
                    timestamp: Some(Utc::now()),
                })
                .await?;
        }

        let wired_pairs = events
            .iter()
            .filter_map(|event| match &event.kind {
                MobEventKind::PeersWired { a, b } => Some((a.clone(), b.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();

        let state = Arc::new(AtomicU8::new(MobState::Stopped as u8));
        let (tx, rx) = mpsc::channel(128);
        let roster = Arc::new(StdRwLock::new(roster));
        let actor = MobActor {
            definition: definition.clone(),
            storage: storage.clone(),
            session_service: session_service.clone(),
            roster: roster.clone(),
            state: state.clone(),
            comms,
            handle: Arc::new(MobHandle {
                tx: tx.clone(),
                roster: roster.clone(),
                definition: definition.clone(),
                state: state.clone(),
                storage: storage.clone(),
            }),
            _rust_bundles: self._rust_bundles,
            skills_content,
            restore_wiring: wired_pairs,
            rx,
        };

        tokio::spawn(actor.run());

        Ok(MobHandle {
            tx,
            roster,
            definition,
            state,
            storage,
        })
    }
}

pub struct MobHandle {
    tx: mpsc::Sender<MobCommand>,
    roster: Arc<StdRwLock<Roster>>,
    definition: Arc<MobDefinition>,
    state: Arc<AtomicU8>,
    storage: MobStorage,
}

impl Clone for MobHandle {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            roster: self.roster.clone(),
            definition: self.definition.clone(),
            state: self.state.clone(),
            storage: self.storage.clone(),
        }
    }
}

impl MobHandle {
    pub async fn spawn(
        &self,
        profile: &ProfileName,
        key: MeerkatId,
        initial_message: Option<String>,
    ) -> Result<MeerkatId, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(MobCommand::Spawn {
                profile: profile.clone(),
                key,
                initial_message,
                reply: reply_tx,
            })
            .await?;
        reply_rx.await?
    }

    pub async fn retire(&self, meerkat_id: &MeerkatId) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(MobCommand::Retire {
                meerkat_id: meerkat_id.clone(),
                reply: reply_tx,
            })
            .await?;
        reply_rx.await?
    }

    pub async fn wire(&self, a: &MeerkatId, b: &MeerkatId) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(MobCommand::Wire {
                a: a.clone(),
                b: b.clone(),
                reply: reply_tx,
            })
            .await?;
        reply_rx.await?
    }

    pub async fn unwire(&self, a: &MeerkatId, b: &MeerkatId) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(MobCommand::Unwire {
                a: a.clone(),
                b: b.clone(),
                reply: reply_tx,
            })
            .await?;
        reply_rx.await?
    }

    pub async fn external_turn(
        &self,
        meerkat_id: &MeerkatId,
        message: String,
    ) -> Result<meerkat_core::types::RunResult, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(MobCommand::ExternalTurn {
                meerkat_id: meerkat_id.clone(),
                message,
                reply: reply_tx,
            })
            .await?;
        reply_rx.await?
    }

    pub async fn internal_turn(
        &self,
        meerkat_id: &MeerkatId,
        message: String,
    ) -> Result<meerkat_core::types::RunResult, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(MobCommand::InternalTurn {
                meerkat_id: meerkat_id.clone(),
                message,
                reply: reply_tx,
            })
            .await?;
        reply_rx.await?
    }

    pub fn list_meerkats(&self, role: Option<&ProfileName>) -> Vec<RosterEntry> {
        let roster = self.roster.read().unwrap_or_else(|err| err.into_inner());
        match role {
            Some(role) => roster.by_profile(role),
            None => roster.list(),
        }
    }

    pub async fn task_create(
        &self,
        subject: String,
        description: String,
        blocked_by: Vec<String>,
    ) -> Result<String, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(MobCommand::TaskCreate {
                subject,
                description,
                blocked_by,
                reply: reply_tx,
            })
            .await?;
        reply_rx.await?
    }

    pub async fn task_update(
        &self,
        task_id: String,
        status: Option<TaskStatus>,
        owner: Option<MeerkatId>,
    ) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(MobCommand::TaskUpdate {
                task_id,
                status,
                owner,
                reply: reply_tx,
            })
            .await?;
        reply_rx.await?
    }

    pub async fn task_list(&self) -> Result<Vec<MobTask>, MobError> {
        let events = self.storage.events.replay_all().await?;
        Ok(TaskBoard::project(&events).list())
    }

    pub async fn poll_events(
        &self,
        after_cursor: Option<u64>,
        limit: Option<usize>,
    ) -> Result<Vec<crate::event::MobEvent>, MobError> {
        self.storage.events.poll(after_cursor, limit).await
    }

    pub fn status(&self) -> MobState {
        MobState::try_from(self.state.load(Ordering::Acquire)).unwrap_or(MobState::Destroyed)
    }

    pub async fn stop(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx.send(MobCommand::Stop { reply: reply_tx }).await?;
        reply_rx.await?
    }

    pub async fn complete(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(MobCommand::Complete { reply: reply_tx })
            .await?;
        reply_rx.await?
    }

    pub async fn destroy(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx.send(MobCommand::Destroy { reply: reply_tx }).await?;
        reply_rx.await?
    }
}

impl From<TryReserveError> for MobError {
    fn from(value: TryReserveError) -> Self {
        MobError::Internal(format!("hash reserve failed: {value}"))
    }
}

struct MobActor {
    definition: Arc<MobDefinition>,
    storage: MobStorage,
    session_service: Arc<dyn SessionService>,
    roster: Arc<StdRwLock<Roster>>,
    state: Arc<AtomicU8>,
    comms: Arc<dyn meerkat_core::agent::CommsRuntime>,
    handle: Arc<MobHandle>,
    _rust_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    skills_content: BTreeMap<String, String>,
    restore_wiring: Vec<(MeerkatId, MeerkatId)>,
    rx: mpsc::Receiver<MobCommand>,
}

struct CombinedToolDispatcher {
    tool_defs: Arc<[Arc<ToolDef>]>,
    dispatchers: Vec<Arc<dyn AgentToolDispatcher>>,
}

impl CombinedToolDispatcher {
    fn new(dispatchers: Vec<Arc<dyn AgentToolDispatcher>>) -> Self {
        let mut seen = BTreeSet::new();
        let mut tool_defs = Vec::new();
        for dispatcher in &dispatchers {
            for tool in dispatcher.tools().iter() {
                if seen.insert(tool.name.clone()) {
                    tool_defs.push(Arc::clone(tool));
                }
            }
        }
        Self {
            tool_defs: tool_defs.into(),
            dispatchers,
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for CombinedToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tool_defs)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        for dispatcher in &self.dispatchers {
            if dispatcher.tools().iter().any(|tool| tool.name == call.name) {
                return dispatcher.dispatch(call).await;
            }
        }
        Err(ToolError::NotFound {
            name: call.name.to_string(),
        })
    }
}

fn resolve_rust_bundle_dispatcher(
    registered: &BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    requested: &[String],
) -> Result<Option<Arc<dyn AgentToolDispatcher>>, MobError> {
    if requested.is_empty() {
        return Ok(None);
    }
    let mut dispatchers: Vec<Arc<dyn AgentToolDispatcher>> = Vec::new();
    for bundle in requested {
        let dispatcher = registered
            .get(bundle)
            .cloned()
            .ok_or_else(|| MobError::Internal(format!("missing rust tool bundle: {bundle}")))?;
        dispatchers.push(dispatcher);
    }
    if dispatchers.len() == 1 {
        return Ok(dispatchers.into_iter().next());
    }
    Ok(Some(Arc::new(CombinedToolDispatcher::new(dispatchers))))
}

impl MobActor {
    fn resolve_external_tools(
        &self,
        profile: &crate::profile::Profile,
    ) -> Result<Option<Arc<dyn AgentToolDispatcher>>, MobError> {
        let mut dispatchers: Vec<Arc<dyn AgentToolDispatcher>> = Vec::new();
        if profile.tools.mob {
            dispatchers.push(Arc::new(MobToolDispatcher::new(self.handle.clone())));
        }
        if profile.tools.mob_tasks {
            dispatchers.push(Arc::new(MobTaskToolDispatcher::new(self.handle.clone())));
        }
        for bundle in &profile.tools.rust_bundles {
            let dispatcher = self
                ._rust_bundles
                .get(bundle)
                .cloned()
                .ok_or_else(|| MobError::Internal(format!("missing rust tool bundle: {bundle}")))?;
            dispatchers.push(dispatcher);
        }

        if dispatchers.is_empty() {
            return Ok(None);
        }
        if dispatchers.len() == 1 {
            return Ok(dispatchers.into_iter().next());
        }
        Ok(Some(Arc::new(CombinedToolDispatcher::new(dispatchers))))
    }

    async fn run(mut self) {
        // Best-effort trust reconciliation for persisted pairings.
        for (a, b) in self.restore_wiring.clone() {
            let _ = self.ensure_trust_pair(&a, &b).await;
        }

        while let Some(cmd) = self.rx.recv().await {
            match cmd {
                MobCommand::Spawn {
                    profile,
                    key,
                    initial_message,
                    reply,
                } => {
                    let _ = reply.send(self.spawn_meerkat(profile, key, initial_message).await);
                }
                MobCommand::Retire { meerkat_id, reply } => {
                    let _ = reply.send(self.retire_meerkat(&meerkat_id).await);
                }
                MobCommand::Wire { a, b, reply } => {
                    let _ = reply.send(self.wire_pair(&a, &b).await);
                }
                MobCommand::Unwire { a, b, reply } => {
                    let _ = reply.send(self.unwire_pair(&a, &b).await);
                }
                MobCommand::ExternalTurn {
                    meerkat_id,
                    message,
                    reply,
                } => {
                    let _ = reply.send(self.turn(&meerkat_id, message, true).await);
                }
                MobCommand::InternalTurn {
                    meerkat_id,
                    message,
                    reply,
                } => {
                    let _ = reply.send(self.turn(&meerkat_id, message, false).await);
                }
                MobCommand::TaskCreate {
                    subject,
                    description,
                    blocked_by,
                    reply,
                } => {
                    let _ = reply.send(self.task_create(subject, description, blocked_by).await);
                }
                MobCommand::TaskUpdate {
                    task_id,
                    status,
                    owner,
                    reply,
                } => {
                    let _ = reply.send(self.task_update(&task_id, status, owner).await);
                }
                MobCommand::Stop { reply } => {
                    let _ = reply.send(self.transition(MobState::Stopped));
                }
                MobCommand::Complete { reply } => {
                    let result = async {
                        self.transition(MobState::Completed)?;
                        self.archive_all_active_sessions().await?;
                        self.storage
                            .events
                            .append(NewMobEvent {
                                mob_id: self.definition.id.clone(),
                                kind: MobEventKind::MobCompleted,
                                timestamp: Some(Utc::now()),
                            })
                            .await
                            .map(|_| ())
                    }
                    .await;
                    let _ = reply.send(result);
                }
                MobCommand::Destroy { reply } => {
                    let result = self.destroy_runtime().await;
                    let _ = reply.send(result);
                }
            }
        }
    }

    fn transition(&self, target: MobState) -> Result<(), MobError> {
        let current = MobState::try_from(self.state.load(Ordering::Acquire))?;
        if !current.can_transition(target) {
            return Err(MobError::InvalidTransition { from: current, to: target });
        }
        self.state.store(target as u8, Ordering::Release);
        Ok(())
    }

    fn comms_name_for(&self, meerkat_id: &MeerkatId) -> String {
        let profile = self
            .roster
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .get(meerkat_id)
            .map(|entry| entry.profile.as_str().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        format!(
            "{}/{}/{}",
            self.definition.id.as_str(),
            profile,
            meerkat_id.as_str()
        )
    }

    async fn resolve_peer_id(&self, meerkat_id: &MeerkatId) -> Option<String> {
        let comms_name = self.comms_name_for(meerkat_id);
        self.comms
            .peers()
            .await
            .into_iter()
            .find(|entry| entry.name.as_str() == comms_name.as_str())
            .map(|entry| entry.peer_id.clone())
    }

    async fn ensure_trust_pair(&self, a: &MeerkatId, b: &MeerkatId) -> Result<(), MobError> {
        let a_id = self.resolve_peer_id(a).await;
        let b_id = self.resolve_peer_id(b).await;
        if let (Some(a_peer), Some(b_peer)) = (a_id, b_id) {
            let a_spec = TrustedPeerSpec::new(self.comms_name_for(a), b_peer, format!("inproc://{}", self.comms_name_for(b)))
                .map_err(|err| MobError::CommsError(meerkat_core::comms::SendError::Validation(err)))?;
            let b_spec = TrustedPeerSpec::new(self.comms_name_for(b), a_peer, format!("inproc://{}", self.comms_name_for(a)))
                .map_err(|err| MobError::CommsError(meerkat_core::comms::SendError::Validation(err)))?;
            let _ = self.comms.add_trusted_peer(a_spec).await;
            let _ = self.comms.add_trusted_peer(b_spec).await;
        }
        Ok(())
    }

    async fn remove_trust_pair(&self, a: &MeerkatId, b: &MeerkatId) -> Result<(), MobError> {
        if let Some(peer_id) = self.resolve_peer_id(b).await {
            let _ = self.comms.remove_trusted_peer(peer_id.as_str()).await;
        }
        if let Some(peer_id) = self.resolve_peer_id(a).await {
            let _ = self.comms.remove_trusted_peer(peer_id.as_str()).await;
        }
        Ok(())
    }

    fn assert_mutable(&self) -> Result<(), MobError> {
        let state = MobState::try_from(self.state.load(Ordering::Acquire))?;
        match state {
            MobState::Running | MobState::Stopped => Ok(()),
            _ => Err(MobError::InvalidTransition {
                from: state,
                to: MobState::Running,
            }),
        }
    }

    async fn spawn_meerkat(
        &self,
        profile: ProfileName,
        key: MeerkatId,
        initial_message: Option<String>,
    ) -> Result<MeerkatId, MobError> {
        self.assert_mutable()?;
        let profile_def = self
            .definition
            .profiles
            .get(&profile)
            .cloned()
            .ok_or_else(|| MobError::ProfileNotFound { profile: profile.clone() })?;

        let meerkat_exists = {
            let roster = self.roster.write().unwrap_or_else(|err| err.into_inner());
            roster.contains(&key)
        };

        if meerkat_exists {
            return Err(MobError::MeerkatAlreadyExists { meerkat_id: key });
        }

        let cfg = build_agent_config(
            &self.definition.id,
            &profile,
            &profile_def,
            &key,
            &self.skills_content,
            "Peer communication is automatic through mob wiring.",
            self.resolve_external_tools(&profile_def)?,
        );
        let req = to_create_session_request(cfg, initial_message.unwrap_or_default());
        let run_result = self.session_service.create_session(req).await?;
        let session_id = run_result.session_id;

        self.storage
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                kind: MobEventKind::MeerkatSpawned {
                    meerkat_id: key.clone(),
                    role: profile.clone(),
                    session_id: session_id.clone(),
                },
                timestamp: Some(Utc::now()),
            })
            .await?;

        {
            let mut roster = self.roster.write().unwrap_or_else(|err| err.into_inner());
            let _ = roster.add(RosterEntry {
                meerkat_id: key.clone(),
                profile: profile.clone(),
                session_id,
                wired_to: BTreeSet::default(),
            });
        }

        self.apply_auto_wiring(&key, &profile).await;
        Ok(key)
    }

    async fn retire_meerkat(&self, meerkat_id: &MeerkatId) -> Result<(), MobError> {
        self.assert_mutable()?;
        let entry = {
            let mut roster = self.roster.write().unwrap_or_else(|err| err.into_inner());
            roster.remove(meerkat_id).ok_or_else(|| MobError::MeerkatNotFound {
                meerkat_id: meerkat_id.clone(),
            })?
        };

        self.storage
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                kind: MobEventKind::MeerkatRetired {
                    meerkat_id: meerkat_id.clone(),
                    role: entry.profile,
                    session_id: entry.session_id.clone(),
                },
                timestamp: Some(Utc::now()),
            })
            .await?;

        for peer_id in entry.wired_to.iter() {
            let _ = self.unwire_pair(meerkat_id, peer_id).await;
        }
        self.session_service.archive(&entry.session_id).await.ok();
        Ok(())
    }

    async fn wire_pair(&self, a: &MeerkatId, b: &MeerkatId) -> Result<(), MobError> {
        if a == b {
            return Err(MobError::WiringError {
                a: a.clone(),
                b: b.clone(),
                reason: "pair must contain two unique peers".to_string(),
            });
        }
        self.assert_mutable()?;

        let changed = {
            let mut roster = self.roster.write().unwrap_or_else(|err| err.into_inner());
            if !roster.contains(a) || !roster.contains(b) {
                return Err(MobError::MeerkatNotFound {
                    meerkat_id: if !roster.contains(a) { a.clone() } else { b.clone() },
                });
            }
            roster.wire(a, b)
        };

        if changed {
            self.ensure_trust_pair(a, b).await?;
            self.storage
                .events
                .append(NewMobEvent {
                    mob_id: self.definition.id.clone(),
                    kind: MobEventKind::PeersWired {
                        a: a.clone(),
                        b: b.clone(),
                    },
                    timestamp: Some(Utc::now()),
                })
                .await?;
        }
        Ok(())
    }

    async fn unwire_pair(&self, a: &MeerkatId, b: &MeerkatId) -> Result<(), MobError> {
        self.assert_mutable()?;
        let changed = {
            let mut roster = self.roster.write().unwrap_or_else(|err| err.into_inner());
            roster.unwire(a, b)
        };

        let _ = self.remove_trust_pair(a, b).await;
        if changed {
            self.storage
                .events
                .append(NewMobEvent {
                    mob_id: self.definition.id.clone(),
                    kind: MobEventKind::PeersUnwired {
                        a: a.clone(),
                        b: b.clone(),
                    },
                    timestamp: Some(Utc::now()),
                })
                .await?;
        }
        Ok(())
    }

    async fn turn(
        &self,
        meerkat_id: &MeerkatId,
        message: String,
        external: bool,
    ) -> Result<meerkat_core::types::RunResult, MobError> {
        let state = MobState::try_from(self.state.load(Ordering::Acquire))?;
        if !matches!(state, MobState::Running | MobState::Stopped) {
            return Err(MobError::InvalidTransition {
                from: state,
                to: MobState::Running,
            });
        }

        let entry = {
            let roster = self.roster.read().unwrap_or_else(|err| err.into_inner());
            roster
                .get(meerkat_id)
                .ok_or_else(|| MobError::MeerkatNotFound {
                    meerkat_id: meerkat_id.clone(),
                })?
                .clone()
        };

        if external {
            let profile = self
                .definition
                .profiles
                .get(&entry.profile)
                .ok_or_else(|| MobError::ProfileNotFound {
                    profile: entry.profile.clone(),
                })?;
            if !profile.external_addressable {
                return Err(MobError::NotExternallyAddressable {
                    meerkat_id: meerkat_id.clone(),
                });
            }
        }

        let result = self
            .session_service
            .start_turn(
                &entry.session_id,
                StartTurnRequest {
                    prompt: message,
                    event_tx: None,
                    host_mode: true,
                    skill_references: None,
                },
            )
            .await?;
        Ok(result)
    }

    async fn task_create(
        &self,
        subject: String,
        description: String,
        blocked_by: Vec<String>,
    ) -> Result<String, MobError> {
        self.assert_mutable()?;
        let task_id = Uuid::new_v4().to_string();
        self.storage
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                kind: MobEventKind::TaskCreated {
                    task_id: task_id.clone(),
                    subject,
                    description,
                    blocked_by,
                },
                timestamp: Some(Utc::now()),
            })
            .await?;
        Ok(task_id)
    }

    async fn task_update(
        &self,
        task_id: &str,
        status: Option<TaskStatus>,
        owner: Option<MeerkatId>,
    ) -> Result<(), MobError> {
        self.assert_mutable()?;
        if owner.is_some() {
            let events = self.storage.events.replay_all().await?;
            let board = TaskBoard::project(&events);
            let task = board
                .get(task_id)
                .ok_or_else(|| MobError::Internal(format!("task not found: {task_id}")))?;
            let blocking = task
                .blocked_by
                .iter()
                .filter(|blocking_id| {
                    board
                        .get(blocking_id)
                        .map(|blocking_task| !matches!(blocking_task.status, TaskStatus::Done))
                        .unwrap_or(true)
                })
                .cloned()
                .collect::<Vec<_>>();
            if !blocking.is_empty() {
                return Err(MobError::Internal(format!(
                    "task {task_id} is blocked by: {}",
                    blocking.join(", ")
                )));
            }
        }
        let status = status.unwrap_or(TaskStatus::Open);
        self.storage
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                kind: MobEventKind::TaskUpdated {
                    task_id: task_id.to_string(),
                    status,
                    owner,
                },
                timestamp: Some(Utc::now()),
            })
            .await?;
        Ok(())
    }

    async fn archive_all_active_sessions(&self) -> Result<(), MobError> {
        let sessions = self.session_service.list(SessionQuery::default()).await?;
        for session in sessions {
            let _ = self.session_service.archive(&session.session_id).await;
        }
        Ok(())
    }

    async fn apply_auto_wiring(&self, new_meerkat: &MeerkatId, role: &ProfileName) {
        let mut targets = BTreeSet::<MeerkatId>::new();

        if let Some(OrchestratorConfig { profile, .. }) = &self.definition.orchestrator
            && self.definition.wiring.auto_wire_orchestrator
            && *role == *profile
        {
            let peers = self.roster.read().unwrap_or_else(|err| err.into_inner()).list();
            for peer in peers {
                if &peer.meerkat_id != new_meerkat {
                    targets.insert(peer.meerkat_id);
                }
            }
        }

        for pair in &self.definition.wiring.role_wiring {
            if &pair.a == role {
                let peers = self
                    .roster
                    .read()
                    .unwrap_or_else(|err| err.into_inner())
                    .by_profile(&pair.b);
                for peer in peers {
                    targets.insert(peer.meerkat_id);
                }
            } else if &pair.b == role {
                let peers = self
                    .roster
                    .read()
                    .unwrap_or_else(|err| err.into_inner())
                    .by_profile(&pair.a);
                for peer in peers {
                    targets.insert(peer.meerkat_id);
                }
            }
        }

        for peer in targets {
            if &peer != new_meerkat {
                let _ = self.wire_pair(new_meerkat, &peer).await;
            }
        }
    }

    async fn destroy_runtime(&self) -> Result<(), MobError> {
        let meerkat_ids = self
            .roster
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .list()
            .into_iter()
            .map(|entry| entry.meerkat_id)
            .collect::<Vec<_>>();
        for meerkat_id in meerkat_ids {
            let _ = self.retire_meerkat(&meerkat_id).await;
        }
        self.archive_all_active_sessions().await?;
        self.transition(MobState::Destroyed)?;
        Ok(())
    }
}

fn project_roster_from_events(events: &[MobEvent]) -> Roster {
    let mut roster = Roster::default();
    for event in events {
        match &event.kind {
            MobEventKind::MeerkatSpawned {
                meerkat_id,
                role,
                session_id,
            } => {
                let _ = roster.add(RosterEntry {
                    meerkat_id: meerkat_id.clone(),
                    profile: role.clone(),
                    session_id: session_id.clone(),
                    wired_to: BTreeSet::default(),
                });
            }
            MobEventKind::MeerkatRetired { meerkat_id, .. } => {
                let _ = roster.remove(meerkat_id);
            }
            MobEventKind::PeersWired { a, b } => {
                let _ = roster.wire(a, b);
            }
            MobEventKind::PeersUnwired { a, b } => {
                let _ = roster.unwire(a, b);
            }
            _ => {}
        }
    }
    roster
}
