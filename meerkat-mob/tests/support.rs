use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, Notify, RwLock};

use meerkat_core::Session;
use meerkat_core::agent::CommsRuntime;
use meerkat_core::comms::{PeerDirectoryEntry, PeerDirectorySource, SendError, TrustedPeerSpec};
use meerkat_core::service::{
    CreateSessionRequest, SessionError, SessionInfo, SessionQuery, SessionService, SessionSummary,
    SessionUsage, SessionView, StartTurnRequest,
};
use meerkat_core::types::{Message, RunResult, SessionId, UserMessage};
use meerkat_store::{SessionFilter, SessionStore};

#[derive(Clone, Default)]
pub struct TestCommsRuntime {
    peers: Arc<RwLock<std::collections::BTreeMap<String, String>>>,
    trusted: Arc<RwLock<std::collections::BTreeSet<String>>>,
    notify: Arc<Notify>,
}

impl TestCommsRuntime {
    pub async fn register_peer(&self, name: impl Into<String>) {
        let name = name.into();
        let mut peers = self.peers.write().await;
        peers
            .entry(name.clone())
            .or_insert_with(|| format!("peer:{name}"));
    }
}

#[async_trait]
impl CommsRuntime for TestCommsRuntime {
    async fn add_trusted_peer(&self, peer: TrustedPeerSpec) -> Result<(), SendError> {
        self.peers
            .write()
            .await
            .insert(peer.name, peer.peer_id.clone());
        self.trusted.write().await.insert(peer.peer_id);
        Ok(())
    }

    async fn remove_trusted_peer(&self, peer_id: &str) -> Result<bool, SendError> {
        let removed = self.trusted.write().await.remove(peer_id);
        Ok(removed)
    }

    async fn peers(&self) -> Vec<PeerDirectoryEntry> {
        let peers = self.peers.read().await;
        peers
            .iter()
            .filter_map(|(name, peer_id)| {
                let peer_name = match meerkat_core::comms::PeerName::new(name.clone()) {
                    Ok(value) => value,
                    Err(_) => return None,
                };
                Some(PeerDirectoryEntry {
                    name: peer_name,
                    peer_id: peer_id.clone(),
                    address: format!("inproc://{name}"),
                    source: PeerDirectorySource::Inproc,
                    sendable_kinds: Vec::new(),
                    capabilities: serde_json::Value::Null,
                    meta: meerkat_core::PeerMeta::default(),
                })
            })
            .collect()
    }

    async fn drain_messages(&self) -> Vec<String> {
        Vec::new()
    }

    fn inbox_notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }
}

#[derive(Clone)]
pub struct TestSessionService {
    sessions: Arc<dyn SessionStore>,
    lock: Arc<Mutex<()>>,
    comms: Arc<TestCommsRuntime>,
}

impl TestSessionService {
    pub fn new(sessions: Arc<dyn SessionStore>, comms: Arc<TestCommsRuntime>) -> Self {
        Self {
            sessions,
            lock: Arc::new(Mutex::new(())),
            comms,
        }
    }

    async fn append_prompt(session: &mut Session, prompt: String, system_prompt: Option<String>) {
        if let Some(prompt) = system_prompt {
            session.set_system_prompt(prompt);
        }
        if !prompt.is_empty() {
            session.push(Message::User(UserMessage { content: prompt }));
        }
    }

    fn run_result(session: &Session, text: impl Into<String>) -> RunResult {
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

#[async_trait]
impl SessionService for TestSessionService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        let _guard = self.lock.lock().await;
        let mut session = Session::new();
        Self::append_prompt(&mut session, req.prompt.clone(), req.system_prompt).await;
        if let Some(build) = req.build.as_ref()
            && let Some(name) = build.comms_name.as_ref()
        {
            self.comms.register_peer(name.clone()).await;
        }
        let result = Self::run_result(&session, req.prompt);
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
        let result = Self::run_result(&session, format!("{} [mocked]", req.prompt));
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
