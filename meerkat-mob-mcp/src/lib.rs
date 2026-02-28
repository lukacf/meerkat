#[cfg(target_arch = "wasm32")]
mod tokio {
    pub use tokio_with_wasm::alias::*;
}

use async_trait::async_trait;
use meerkat_core::ScopedAgentEvent;
use meerkat_core::agent::{AgentToolDispatcher, CommsRuntime as CoreCommsRuntime};
use meerkat_core::comms::{CommsCommand, SendError, SendReceipt, TrustedPeerSpec};
use meerkat_core::error::ToolError;
use meerkat_core::interaction::InteractionId;
use meerkat_core::service::{
    CreateSessionRequest, SessionError, SessionInfo, SessionQuery, SessionService,
    SessionServiceCommsExt, SessionSummary, SessionUsage, SessionView, StartTurnRequest,
};
use meerkat_core::time_compat::{Instant, SystemTime};
use meerkat_core::types::{RunResult, SessionId, ToolCallView, ToolDef, ToolResult, Usage};
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

/// In-memory MCP state for multiple mobs.
pub struct MobMcpState {
    session_service: Arc<dyn MobSessionService>,
    mobs: RwLock<BTreeMap<MobId, ManagedMob>>,
}

impl MobMcpState {
    pub fn new(session_service: Arc<dyn MobSessionService>) -> Self {
        Self {
            session_service,
            mobs: RwLock::new(BTreeMap::new()),
        }
    }

    /// Access the underlying session service.
    pub fn session_service(&self) -> &dyn MobSessionService {
        &*self.session_service
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
        let handle = MobBuilder::new(definition.clone(), storage)
            .with_session_service(self.session_service.clone())
            .allow_ephemeral_sessions(!self.session_service.supports_persistent_sessions())
            .create()
            .await?;
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
        self.mob_spawn_spec(
            mob_id,
            SpawnMemberSpec {
                profile_name: profile,
                meerkat_id,
                initial_message: None,
                runtime_mode,
                backend,
            },
        )
        .await
    }

    pub async fn mob_spawn_spec(
        &self,
        mob_id: &MobId,
        spec: SpawnMemberSpec,
    ) -> Result<meerkat_mob::MemberRef, MobError> {
        self.handle_for(mob_id)
            .await?
            .spawn_with_options(
                spec.profile_name,
                spec.meerkat_id,
                spec.initial_message,
                spec.runtime_mode,
                spec.backend,
            )
            .await
    }

    pub async fn mob_spawn_many(
        &self,
        mob_id: &MobId,
        specs: Vec<SpawnMemberSpec>,
    ) -> Result<Vec<Result<meerkat_mob::MemberRef, MobError>>, MobError> {
        Ok(self.handle_for(mob_id).await?.spawn_many(specs).await)
    }

    pub async fn mob_retire(&self, mob_id: &MobId, meerkat_id: MeerkatId) -> Result<(), MobError> {
        self.handle_for(mob_id).await?.retire(meerkat_id).await
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

    pub async fn mob_send_message(
        &self,
        mob_id: &MobId,
        meerkat_id: MeerkatId,
        message: String,
    ) -> Result<(), MobError> {
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

    async fn handle_for(&self, mob_id: &MobId) -> Result<MobHandle, MobError> {
        self.mobs
            .read()
            .await
            .get(mob_id)
            .map(|m| m.handle.clone())
            .ok_or_else(|| MobError::Internal(format!("mob not found: {mob_id}")))
    }

    /// Create MCP state backed by an in-memory local session service.
    pub fn new_in_memory() -> Arc<Self> {
        Arc::new(Self::new(Arc::new(LocalSessionService::new())))
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
    counter: std::sync::atomic::AtomicU64,
}

impl LocalSessionService {
    fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            counter: std::sync::atomic::AtomicU64::new(0),
        }
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
        if !self.sessions.read().await.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
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

    async fn interrupt(&self, _id: &SessionId) -> Result<(), SessionError> {
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
            })
            .collect())
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        self.sessions.write().await.remove(id);
        Ok(())
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
    ) -> Option<Arc<dyn meerkat_core::SubscribableInjector>> {
        let sessions = self.sessions.read().await;
        let runtime = sessions.get(session_id)?;
        runtime.event_injector()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobSessionService for LocalSessionService {
    fn supports_persistent_sessions(&self) -> bool {
        true
    }

    async fn session_belongs_to_mob(&self, _session_id: &SessionId, _mob_id: &MobId) -> bool {
        true
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
                     initial_message. {COMMON}"),
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
                                    "runtime_mode":{"type":"string","enum":["autonomous_host","turn_driven"]}
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
                        SpawnMemberSpec::from_wire(
                            spec.profile,
                            spec.meerkat_id,
                            spec.initial_message,
                            spec.runtime_mode,
                            spec.backend,
                        )
                    })
                    .collect::<Vec<_>>();
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
                self.state
                    .mob_send_message(
                        &MobId::from(args.mob_id),
                        MeerkatId::from(args.meerkat_id),
                        args.message,
                    )
                    .await
                    .map_err(|e| map_mob_err(call, e))?;
                encode(call, json!({"ok": true}))
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
        ) -> Option<Arc<dyn meerkat_core::SubscribableInjector>> {
            if !self.sessions.read().await.contains_key(session_id) {
                return None;
            }
            Some(Arc::new(MockInjector))
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
    async fn test_dispatcher_exposes_12_tools() {
        let svc = Arc::new(MockSessionSvc::new());
        let state = Arc::new(MobMcpState::new(svc));
        let d = MobMcpDispatcher::new(state);
        assert_eq!(d.tools().len(), 12);
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
