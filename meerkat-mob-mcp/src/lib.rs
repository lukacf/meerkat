use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify, RwLock};

use meerkat_core::Session;
use meerkat_core::agent::CommsRuntime;
use meerkat_core::comms::{PeerDirectoryEntry, PeerDirectorySource, SendError, TrustedPeerSpec};
use meerkat_core::service::{
    CreateSessionRequest, SessionError, SessionInfo, SessionQuery, SessionService, SessionSummary,
    SessionUsage, SessionView, StartTurnRequest,
};
use meerkat_core::types::{Message, RunResult, SessionId, UserMessage};
use meerkat_mob::Prefab;
use meerkat_mob::event::{MobEvent, NewMobEvent};
use meerkat_mob::store::MobEventStore;
use meerkat_mob::{MobBuilder, MobError, MobHandle, MobState, MobStorage, ids::MobId};
use meerkat_store::{JsonlStore, SessionFilter, SessionStore};
use serde_json::to_value;

pub struct MobMcpState {
    mobs: RwLock<std::collections::BTreeMap<String, MobEntry>>,
    storage_root: PathBuf,
}

#[derive(Clone)]
pub(crate) struct MobEntry {
    handle: MobHandle,
    storage: MobStorage,
    session_service: Arc<dyn SessionService>,
    comms_runtime: Arc<dyn CommsRuntime>,
}

#[derive(Clone, Default)]
struct HarnessCommsRuntime {
    peers: Arc<RwLock<std::collections::BTreeMap<String, String>>>,
    trusted: Arc<RwLock<std::collections::BTreeSet<String>>>,
    notify: Arc<Notify>,
}

impl HarnessCommsRuntime {
    async fn register_peer(&self, name: impl Into<String>) {
        let name = name.into();
        let mut peers = self.peers.write().await;
        peers
            .entry(name.clone())
            .or_insert_with(|| format!("peer:{name}"));
    }
}

#[async_trait]
impl CommsRuntime for HarnessCommsRuntime {
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
                    capabilities: Value::Null,
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
struct HarnessSessionService {
    sessions: Arc<dyn SessionStore>,
    lock: Arc<Mutex<()>>,
    comms: Arc<HarnessCommsRuntime>,
}

impl HarnessSessionService {
    fn new(sessions: Arc<dyn SessionStore>, comms: Arc<HarnessCommsRuntime>) -> Self {
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
impl SessionService for HarnessSessionService {
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

fn build_runtime_dependencies(
    storage: &MobStorage,
) -> (Arc<dyn SessionService>, Arc<dyn CommsRuntime>) {
    let comms = Arc::new(HarnessCommsRuntime::default());
    let sessions: Arc<dyn SessionService> = Arc::new(HarnessSessionService::new(
        storage.sessions.clone(),
        comms.clone(),
    ));
    (sessions, comms)
}

struct JsonlMobEventStore {
    mob_id: MobId,
    path: PathBuf,
    lock: Mutex<()>,
}

impl JsonlMobEventStore {
    fn new(mob_id: MobId, path: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            mob_id,
            path,
            lock: Mutex::new(()),
        })
    }

    async fn load_events(&self) -> Result<Vec<MobEvent>, MobError> {
        match tokio::fs::read_to_string(&self.path).await {
            Ok(raw) => {
                if raw.trim().is_empty() {
                    Ok(Vec::new())
                } else {
                    serde_json::from_str::<Vec<MobEvent>>(&raw).map_err(|err| {
                        MobError::Internal(format!(
                            "failed to parse mob event log {}: {err}",
                            self.path.display()
                        ))
                    })
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(err) => Err(MobError::StorageError(Box::new(err))),
        }
    }

    async fn save_events(&self, events: &[MobEvent]) -> Result<(), MobError> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|err| MobError::StorageError(Box::new(err)))?;
        }
        let raw = serde_json::to_string_pretty(events)
            .map_err(|err| MobError::Internal(format!("failed to encode mob event log: {err}")))?;
        tokio::fs::write(&self.path, raw)
            .await
            .map_err(|err| MobError::StorageError(Box::new(err)))
    }
}

#[async_trait]
impl MobEventStore for JsonlMobEventStore {
    async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobError> {
        let _guard = self.lock.lock().await;
        let mut events = self.load_events().await?;
        let cursor = events
            .last()
            .map(|entry| entry.cursor.saturating_add(1))
            .unwrap_or(0);
        let mob_event = MobEvent {
            cursor,
            timestamp: event.timestamp.unwrap_or_else(Utc::now),
            mob_id: self.mob_id.clone(),
            kind: event.kind,
        };
        events.push(mob_event.clone());
        self.save_events(&events).await?;
        Ok(mob_event)
    }

    async fn poll(
        &self,
        after_cursor: Option<u64>,
        limit: Option<usize>,
    ) -> Result<Vec<MobEvent>, MobError> {
        let mut events = self
            .load_events()
            .await?
            .into_iter()
            .filter(|event| after_cursor.is_none_or(|cursor| event.cursor > cursor))
            .collect::<Vec<_>>();
        if let Some(limit) = limit {
            events.truncate(limit);
        }
        Ok(events)
    }

    async fn replay_all(&self) -> Result<Vec<MobEvent>, MobError> {
        self.load_events().await
    }
}

impl MobMcpState {
    pub async fn new(storage_root: PathBuf) -> Self {
        let state = Self {
            mobs: RwLock::new(std::collections::BTreeMap::new()),
            storage_root,
        };
        state.load_existing().await;
        state
    }

    fn mobs_root(&self) -> PathBuf {
        self.storage_root.join("mobs")
    }

    fn mob_root(&self, mob_id: &str) -> PathBuf {
        self.mobs_root().join(mob_id)
    }

    fn sessions_root(&self, mob_id: &str) -> PathBuf {
        self.mob_root(mob_id).join("sessions")
    }

    fn events_path(&self, mob_id: &str) -> PathBuf {
        self.mob_root(mob_id).join("events.json")
    }

    async fn storage_for_mob(&self, mob_id: &str) -> Result<MobStorage, String> {
        let mob_root = self.mob_root(mob_id);
        tokio::fs::create_dir_all(&mob_root)
            .await
            .map_err(|err| format!("failed to create mob root {}: {err}", mob_root.display()))?;

        let session_store: Arc<dyn SessionStore> =
            Arc::new(JsonlStore::new(self.sessions_root(mob_id)));
        let event_store: Arc<dyn MobEventStore> =
            JsonlMobEventStore::new(MobId::from(mob_id), self.events_path(mob_id));
        Ok(MobStorage::new(session_store, event_store))
    }

    async fn load_existing(&self) {
        let mobs_root = self.mobs_root();
        if tokio::fs::create_dir_all(&mobs_root).await.is_err() {
            return;
        }
        let mut entries = match tokio::fs::read_dir(&mobs_root).await {
            Ok(entries) => entries,
            Err(_) => return,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let is_dir = match entry.file_type().await {
                Ok(file_type) => file_type.is_dir(),
                Err(_) => false,
            };
            if !is_dir {
                continue;
            }

            let mob_id = entry.file_name().to_string_lossy().to_string();
            let storage = match self.storage_for_mob(&mob_id).await {
                Ok(storage) => storage,
                Err(_) => continue,
            };
            let (session_service, comms_runtime) = build_runtime_dependencies(&storage);
            let builder = MobBuilder::for_resume(storage.clone())
                .with_session_service(session_service.clone())
                .with_comms_runtime(comms_runtime.clone());
            let handle = match builder.resume().await {
                Ok(handle) => handle,
                Err(_) => continue,
            };
            self.insert(mob_id, handle, storage, session_service, comms_runtime)
                .await;
        }
    }

    pub async fn insert(
        &self,
        id: String,
        handle: MobHandle,
        storage: MobStorage,
        session_service: Arc<dyn SessionService>,
        comms_runtime: Arc<dyn CommsRuntime>,
    ) {
        self.mobs.write().await.insert(
            id,
            MobEntry {
                handle,
                storage,
                session_service,
                comms_runtime,
            },
        );
    }

    pub async fn get(&self, id: &str) -> Option<MobHandle> {
        self.mobs
            .read()
            .await
            .get(id)
            .map(|entry| entry.handle.clone())
    }

    pub(crate) async fn get_entry(&self, id: &str) -> Option<MobEntry> {
        self.mobs.read().await.get(id).cloned()
    }

    pub(crate) async fn remove(&self, id: &str) -> Option<MobEntry> {
        self.mobs.write().await.remove(id)
    }
}

#[derive(Serialize, Deserialize)]
struct RpcRequest {
    id: Value,
    method: String,
    params: Option<Value>,
}

#[derive(Serialize, Deserialize)]
pub struct RpcResponse {
    jsonrpc: String,
    id: Value,
    result: Option<Value>,
    error: Option<Value>,
}

fn respond_err(message: impl Into<String>) -> Value {
    json!({"error": {"message": message.into()}})
}

pub async fn handle_tool_call(state: &MobMcpState, method: &str, params: Option<Value>) -> Value {
    match method {
        "mob_create" => create_mob(state, params).await,
        "mob_prefabs" => mob_prefabs().await,
        "mob_stop" => mob_stop(state, params).await,
        "mob_resume" => mob_resume(state, params).await,
        "mob_destroy" => mob_destroy(state, params).await,
        "mob_complete" => mob_complete(state, params).await,
        "mob_list" => mob_list(state).await,
        "mob_status" => mob_status(state, params).await,
        "mob_spawn" => mob_spawn(state, params).await,
        "mob_retire" => mob_retire(state, params).await,
        "mob_wire" => mob_wire(state, params).await,
        "mob_unwire" => mob_unwire(state, params).await,
        "mob_list_meerkats" => mob_list_meerkats(state, params).await,
        "mob_turn" => mob_external_turn(state, params).await,
        "mob_external_turn" => mob_external_turn(state, params).await,
        "mob_events" => mob_events(state, params).await,
        _ => respond_err("unknown method"),
    }
}

pub async fn handle_rpc_line(state: &MobMcpState, line: &str) -> Value {
    match serde_json::from_str::<RpcRequest>(line) {
        Ok(payload) => {
            let result = handle_tool_call(state, &payload.method, payload.params).await;
            if result.get("error").is_some() {
                json!(RpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: payload.id,
                    result: None,
                    error: result.get("error").cloned(),
                })
            } else {
                json!(RpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: payload.id,
                    result: Some(result),
                    error: None,
                })
            }
        }
        Err(err) => json!(RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Value::Null,
            result: None,
            error: Some(json!({"message": err.to_string()})),
        }),
    }
}

#[derive(Serialize, Deserialize)]
struct CreateParams {
    mob_id: String,
    definition: Option<String>,
    definition_file: Option<String>,
    prefab: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct MobRef {
    mob_id: String,
}

#[derive(Serialize, Deserialize)]
struct SpawnParams {
    mob_id: String,
    profile: String,
    key: String,
    message: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct TurnParams {
    mob_id: String,
    meerkat_id: String,
    message: String,
}

#[derive(Serialize, Deserialize)]
struct RetireParams {
    mob_id: String,
    meerkat_id: String,
}

#[derive(Serialize, Deserialize)]
struct WireParams {
    mob_id: String,
    a: String,
    b: String,
}

#[derive(Serialize, Deserialize)]
struct EventsParams {
    mob_id: String,
    after_cursor: Option<u64>,
    limit: Option<usize>,
}

fn resolve_prefab(name: &str) -> Option<meerkat_mob::MobDefinition> {
    match name.to_lowercase().replace('_', "-").as_str() {
        "coding-swarm" => Some(Prefab::CodingSwarm.definition()),
        "code-review" => Some(Prefab::CodeReview.definition()),
        "research-team" => Some(Prefab::ResearchTeam.definition()),
        "pipeline" => Some(Prefab::Pipeline.definition()),
        _ => None,
    }
}

async fn toml_from_file(path: &Path) -> std::io::Result<String> {
    tokio::fs::read_to_string(path).await
}

async fn create_mob(state: &MobMcpState, params: Option<Value>) -> Value {
    let params = match params.and_then(|raw| serde_json::from_value::<CreateParams>(raw).ok()) {
        Some(p) => p,
        None => return respond_err("invalid params"),
    };

    if state.get(&params.mob_id).await.is_some() {
        return respond_err("mob already exists");
    }

    let definition: meerkat_mob::MobDefinition = if let Some(definition_toml) = params.definition {
        match meerkat_mob::MobDefinition::from_toml(&definition_toml) {
            Ok(def) => def,
            Err(err) => return respond_err(format!("invalid definition: {err}")),
        }
    } else if let Some(path) = params.definition_file {
        match toml_from_file(Path::new(&path)).await {
            Ok(raw) => match meerkat_mob::MobDefinition::from_toml(&raw) {
                Ok(def) => def,
                Err(err) => return respond_err(format!("invalid definition file: {err}")),
            },
            Err(err) => return respond_err(format!("could not read definition_file: {err}")),
        }
    } else if let Some(prefab_name) = params.prefab {
        match resolve_prefab(&prefab_name) {
            Some(def) => def,
            None => return respond_err("unknown prefab"),
        }
    } else {
        return respond_err("missing definition, definition_file, or prefab");
    };

    let storage = match state.storage_for_mob(&params.mob_id).await {
        Ok(storage) => storage,
        Err(err) => return respond_err(err),
    };
    let (session_service, comms_runtime) = build_runtime_dependencies(&storage);

    let handle = match MobBuilder::new(definition, storage.clone())
        .with_session_service(session_service.clone())
        .with_comms_runtime(comms_runtime.clone())
        .create()
        .await
    {
        Ok(handle) => handle,
        Err(err) => return respond_err(format!("{}", err)),
    };

    state
        .insert(
            params.mob_id.clone(),
            handle,
            storage,
            session_service,
            comms_runtime,
        )
        .await;

    json!({"mob_id": params.mob_id})
}

async fn mob_prefabs() -> Value {
    let prefabs = Prefab::list()
        .into_iter()
        .map(|prefab| {
            json!({
                "name": prefab.name,
                "description": prefab.description,
            })
        })
        .collect::<Vec<_>>();
    json!({ "prefabs": prefabs })
}

async fn mob_stop(state: &MobMcpState, params: Option<Value>) -> Value {
    let id = match params.and_then(|raw| serde_json::from_value::<MobRef>(raw).ok()) {
        Some(mob) => mob.mob_id,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&id).await else {
        return respond_err("unknown mob");
    };
    match handle.stop().await {
        Ok(_) => json!({"ok":true}),
        Err(err) => respond_err(err.to_string()),
    }
}

async fn mob_resume(state: &MobMcpState, params: Option<Value>) -> Value {
    let id = match params.and_then(|raw| serde_json::from_value::<MobRef>(raw).ok()) {
        Some(mob) => mob.mob_id,
        None => return respond_err("invalid params"),
    };

    let entry = match state.get_entry(&id).await {
        Some(entry) => entry,
        None => return respond_err("unknown mob"),
    };

    if entry.handle.status() == MobState::Completed {
        return respond_err("cannot resume completed mob");
    }

    if entry.handle.status() == MobState::Stopped {
        return json!({"ok":true});
    }

    let new_handle = match MobBuilder::for_resume(entry.storage.clone())
        .with_session_service(entry.session_service.clone())
        .with_comms_runtime(entry.comms_runtime.clone())
        .resume()
        .await
    {
        Ok(handle) => handle,
        Err(err) => return respond_err(format!("{err}")),
    };

    state
        .insert(
            id,
            new_handle,
            entry.storage,
            entry.session_service,
            entry.comms_runtime,
        )
        .await;

    json!({"ok": true})
}

async fn mob_destroy(state: &MobMcpState, params: Option<Value>) -> Value {
    let id = match params.and_then(|raw| serde_json::from_value::<MobRef>(raw).ok()) {
        Some(mob) => mob.mob_id,
        None => return respond_err("invalid params"),
    };
    let Some(entry) = state.remove(&id).await else {
        return respond_err("unknown mob");
    };
    match entry.handle.destroy().await {
        Ok(_) => {
            let _ = tokio::fs::remove_dir_all(state.mob_root(&id)).await;
            json!({"ok":true})
        }
        Err(err) => {
            state
                .insert(
                    id,
                    entry.handle.clone(),
                    entry.storage,
                    entry.session_service,
                    entry.comms_runtime,
                )
                .await;
            respond_err(err.to_string())
        }
    }
}

async fn mob_complete(state: &MobMcpState, params: Option<Value>) -> Value {
    let id = match params.and_then(|raw| serde_json::from_value::<MobRef>(raw).ok()) {
        Some(mob) => mob.mob_id,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&id).await else {
        return respond_err("unknown mob");
    };
    match handle.complete().await {
        Ok(_) => json!({"ok":true}),
        Err(err) => respond_err(err.to_string()),
    }
}

async fn mob_list(state: &MobMcpState) -> Value {
    let mobs = state
        .mobs
        .read()
        .await
        .iter()
        .map(|(id, entry)| {
            json!({
                "mob_id": id,
                "status": format!("{:?}", entry.handle.status()),
            })
        })
        .collect::<Vec<_>>();
    json!({"mobs": mobs})
}

async fn mob_status(state: &MobMcpState, params: Option<Value>) -> Value {
    let id = match params.and_then(|raw| serde_json::from_value::<MobRef>(raw).ok()) {
        Some(mob) => mob.mob_id,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&id).await else {
        return respond_err("unknown mob");
    };
    let meerkats = handle
        .list_meerkats(None)
        .into_iter()
        .map(|entry| {
            json!({
                "meerkat_id": entry.meerkat_id.as_str(),
                "profile": entry.profile.as_str(),
                "session_id": entry.session_id.to_string(),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "status": format!("{:?}", handle.status()),
        "meerkats": meerkats,
    })
}

async fn mob_spawn(state: &MobMcpState, params: Option<Value>) -> Value {
    let p = match params.and_then(|raw| serde_json::from_value::<SpawnParams>(raw).ok()) {
        Some(v) => v,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&p.mob_id).await else {
        return respond_err("unknown mob");
    };
    match handle
        .spawn(&p.profile.into(), p.key.into(), p.message)
        .await
    {
        Ok(id) => json!({"meerkat_id": id.as_str()}),
        Err(err) => respond_err(err.to_string()),
    }
}

async fn mob_retire(state: &MobMcpState, params: Option<Value>) -> Value {
    let p = match params.and_then(|raw| serde_json::from_value::<RetireParams>(raw).ok()) {
        Some(v) => v,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&p.mob_id).await else {
        return respond_err("unknown mob");
    };
    match handle.retire(&p.meerkat_id.into()).await {
        Ok(_) => json!({"ok":true}),
        Err(err) => respond_err(err.to_string()),
    }
}

async fn mob_wire(state: &MobMcpState, params: Option<Value>) -> Value {
    let p = match params.and_then(|raw| serde_json::from_value::<WireParams>(raw).ok()) {
        Some(v) => v,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&p.mob_id).await else {
        return respond_err("unknown mob");
    };
    match handle.wire(&p.a.into(), &p.b.into()).await {
        Ok(_) => json!({"ok":true}),
        Err(err) => respond_err(err.to_string()),
    }
}

async fn mob_unwire(state: &MobMcpState, params: Option<Value>) -> Value {
    let p = match params.and_then(|raw| serde_json::from_value::<WireParams>(raw).ok()) {
        Some(v) => v,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&p.mob_id).await else {
        return respond_err("unknown mob");
    };
    match handle.unwire(&p.a.into(), &p.b.into()).await {
        Ok(_) => json!({"ok":true}),
        Err(err) => respond_err(err.to_string()),
    }
}

async fn mob_list_meerkats(state: &MobMcpState, params: Option<Value>) -> Value {
    let id = match params.and_then(|raw| serde_json::from_value::<MobRef>(raw).ok()) {
        Some(v) => v.mob_id,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&id).await else {
        return respond_err("unknown mob");
    };
    let meerkats = handle
        .list_meerkats(None)
        .into_iter()
        .map(|entry| {
            json!({
                "meerkat_id": entry.meerkat_id.as_str(),
                "profile": entry.profile.as_str(),
                "session_id": entry.session_id.to_string(),
            })
        })
        .collect::<Vec<_>>();
    json!({"meerkats": meerkats})
}

async fn mob_external_turn(state: &MobMcpState, params: Option<Value>) -> Value {
    let p = match params.and_then(|raw| serde_json::from_value::<TurnParams>(raw).ok()) {
        Some(v) => v,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&p.mob_id).await else {
        return respond_err("unknown mob");
    };
    match handle.external_turn(&p.meerkat_id.into(), p.message).await {
        Ok(run) => json!({"text": run.text}),
        Err(err) => respond_err(err.to_string()),
    }
}

async fn mob_events(state: &MobMcpState, params: Option<Value>) -> Value {
    let p = match params.and_then(|raw| serde_json::from_value::<EventsParams>(raw).ok()) {
        Some(v) => v,
        None => return respond_err("invalid params"),
    };
    let Some(handle) = state.get(&p.mob_id).await else {
        return respond_err("unknown mob");
    };
    let events = match handle.poll_events(p.after_cursor, p.limit).await {
        Ok(v) => v,
        Err(err) => return respond_err(err.to_string()),
    };
    let serialized = events
        .into_iter()
        .map(|event| to_value(event).unwrap_or(Value::Null))
        .collect::<Vec<_>>();
    json!({"events": serialized})
}
