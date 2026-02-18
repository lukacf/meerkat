//! Mob runtime: actor, builder, handle, and primitives.
//!
//! The mob runtime uses an actor pattern where all mutations (spawn, retire,
//! wire, unwire, etc.) are serialized through a single command channel.
//! Read-only operations bypass the actor and read from shared state directly.

use crate::build;
use crate::definition::MobDefinition;
use crate::error::MobError;
use crate::event::{MobEventKind, NewMobEvent};
use crate::ids::{MeerkatId, MobId, ProfileName};
use crate::roster::{Roster, RosterEntry};
use crate::storage::MobStorage;
use crate::store::MobEventStore;
use crate::tasks::{MobTask, TaskBoard, TaskStatus};
use meerkat_client::LlmClient;
use meerkat_core::ToolGatewayBuilder;
use meerkat_core::agent::{AgentToolDispatcher, CommsRuntime as CoreCommsRuntime};
use meerkat_core::comms::{CommsCommand, InputStreamMode, PeerName, TrustedPeerSpec};
use meerkat_core::error::ToolError;
use meerkat_core::service::SessionService;
use meerkat_core::types::{SessionId, ToolCallView, ToolDef, ToolResult};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::sync::{RwLock, mpsc, oneshot};

// ---------------------------------------------------------------------------
// MobState
// ---------------------------------------------------------------------------

/// Lifecycle state of a mob, stored as `Arc<AtomicU8>` for lock-free reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MobState {
    Creating = 0,
    Running = 1,
    Stopped = 2,
    Completed = 3,
    Destroyed = 4,
}

impl MobState {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Creating,
            1 => Self::Running,
            2 => Self::Stopped,
            3 => Self::Completed,
            4 => Self::Destroyed,
            _ => Self::Creating,
        }
    }

    /// Human-readable name for the state.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "Creating",
            Self::Running => "Running",
            Self::Stopped => "Stopped",
            Self::Completed => "Completed",
            Self::Destroyed => "Destroyed",
        }
    }
}

// ---------------------------------------------------------------------------
// MobCommand
// ---------------------------------------------------------------------------

/// Commands sent from [`MobHandle`] to the [`MobActor`] for serialized processing.
enum MobCommand {
    Spawn {
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        initial_message: Option<String>,
        reply_tx: oneshot::Sender<Result<SessionId, MobError>>,
    },
    Retire {
        meerkat_id: MeerkatId,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Wire {
        a: MeerkatId,
        b: MeerkatId,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Unwire {
        a: MeerkatId,
        b: MeerkatId,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    ExternalTurn {
        meerkat_id: MeerkatId,
        message: String,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    InternalTurn {
        meerkat_id: MeerkatId,
        message: String,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Stop {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    ResumeLifecycle {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Complete {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Destroy {
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    TaskCreate {
        task_id: String,
        subject: String,
        description: String,
        blocked_by: Vec<String>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    TaskUpdate {
        task_id: String,
        status: TaskStatus,
        owner: Option<MeerkatId>,
        reply_tx: oneshot::Sender<Result<(), MobError>>,
    },
    Shutdown {
        reply_tx: oneshot::Sender<()>,
    },
}

// ---------------------------------------------------------------------------
// MobHandle
// ---------------------------------------------------------------------------

/// Clone-cheap, thread-safe handle for interacting with a running mob.
///
/// All mutation commands are sent through an mpsc channel to the actor.
/// Read-only operations (roster, state) bypass the actor and read from
/// shared `Arc` state directly.
#[derive(Clone)]
pub struct MobHandle {
    command_tx: mpsc::Sender<MobCommand>,
    roster: Arc<RwLock<Roster>>,
    definition: Arc<MobDefinition>,
    state: Arc<AtomicU8>,
    events: Arc<dyn MobEventStore>,
    mcp_running: Arc<RwLock<BTreeMap<String, bool>>>,
}

#[derive(Clone)]
pub struct MobEventsView {
    inner: Arc<dyn MobEventStore>,
}

impl MobEventsView {
    pub async fn poll(&self, after_cursor: u64, limit: usize) -> Result<Vec<crate::event::MobEvent>, MobError> {
        self.inner.poll(after_cursor, limit).await
    }

    pub async fn replay_all(&self) -> Result<Vec<crate::event::MobEvent>, MobError> {
        self.inner.replay_all().await
    }
}

impl MobHandle {
    /// Current mob lifecycle state (lock-free read).
    pub fn status(&self) -> MobState {
        MobState::from_u8(self.state.load(Ordering::Acquire))
    }

    /// Access the mob definition.
    pub fn definition(&self) -> &MobDefinition {
        &self.definition
    }

    /// Mob ID.
    pub fn mob_id(&self) -> &MobId {
        &self.definition.id
    }

    /// Snapshot of the current roster.
    pub async fn roster(&self) -> Roster {
        self.roster.read().await.clone()
    }

    /// List all meerkats in the roster.
    pub async fn list_meerkats(&self) -> Vec<RosterEntry> {
        self.roster.read().await.list().cloned().collect()
    }

    /// Get a specific meerkat entry.
    pub async fn get_meerkat(&self, meerkat_id: &MeerkatId) -> Option<RosterEntry> {
        self.roster.read().await.get(meerkat_id).cloned()
    }

    /// Access a read-only events view for polling/replay.
    pub fn events(&self) -> MobEventsView {
        MobEventsView {
            inner: self.events.clone(),
        }
    }

    /// Snapshot of MCP server lifecycle state tracked by this runtime.
    pub async fn mcp_server_states(&self) -> BTreeMap<String, bool> {
        self.mcp_running.read().await.clone()
    }

    /// Spawn a new meerkat from a profile.
    pub async fn spawn(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        initial_message: Option<String>,
    ) -> Result<SessionId, MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Spawn {
                profile_name,
                meerkat_id,
                initial_message,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Retire a meerkat, archiving its session and removing trust.
    pub async fn retire(&self, meerkat_id: MeerkatId) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Retire {
                meerkat_id,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Wire two meerkats together (bidirectional trust).
    pub async fn wire(&self, a: MeerkatId, b: MeerkatId) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Wire { a, b, reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Unwire two meerkats (remove bidirectional trust).
    pub async fn unwire(&self, a: MeerkatId, b: MeerkatId) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Unwire { a, b, reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Send an external turn to a meerkat (enforces external_addressable).
    pub async fn external_turn(
        &self,
        meerkat_id: MeerkatId,
        message: String,
    ) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::ExternalTurn {
                meerkat_id,
                message,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Send an internal turn to a meerkat (no external_addressable check).
    pub async fn internal_turn(
        &self,
        meerkat_id: MeerkatId,
        message: String,
    ) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::InternalTurn {
                meerkat_id,
                message,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Transition Running -> Stopped. Mutation commands are rejected while stopped.
    pub async fn stop(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Stop { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Transition Stopped -> Running.
    pub async fn resume(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::ResumeLifecycle { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Archive all meerkats, emit MobCompleted, and transition to Completed.
    pub async fn complete(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Complete { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Retire active meerkats and clear persisted mob storage.
    pub async fn destroy(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::Destroy { reply_tx })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Create a task in the shared mob task board.
    pub async fn task_create(
        &self,
        task_id: String,
        subject: String,
        description: String,
        blocked_by: Vec<String>,
    ) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::TaskCreate {
                task_id,
                subject,
                description,
                blocked_by,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Update task status/owner in the shared mob task board.
    pub async fn task_update(
        &self,
        task_id: String,
        status: TaskStatus,
        owner: Option<MeerkatId>,
    ) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.command_tx
            .send(MobCommand::TaskUpdate {
                task_id,
                status,
                owner,
                reply_tx,
            })
            .await
            .map_err(|_| MobError::Internal("actor task dropped".into()))?;
        reply_rx
            .await
            .map_err(|_| MobError::Internal("actor reply dropped".into()))?
    }

    /// Project and list tasks from persisted events.
    pub async fn task_list(&self) -> Result<Vec<MobTask>, MobError> {
        let events = self.events.replay_all().await?;
        Ok(TaskBoard::project(&events).list().cloned().collect())
    }

    /// Project and get a task by ID from persisted events.
    pub async fn task_get(&self, task_id: &str) -> Result<Option<MobTask>, MobError> {
        let events = self.events.replay_all().await?;
        Ok(TaskBoard::project(&events).get(task_id).cloned())
    }

    /// Shut down the actor. After this, no more commands are accepted.
    pub async fn shutdown(&self) -> Result<(), MobError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self
            .command_tx
            .send(MobCommand::Shutdown { reply_tx })
            .await;
        let _ = reply_rx.await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// MobBuilder
// ---------------------------------------------------------------------------

/// Builder for creating or resuming a mob.
pub struct MobBuilder {
    mode: BuilderMode,
    storage: MobStorage,
    session_service: Option<Arc<dyn MobSessionService>>,
    tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    default_llm_client: Option<Arc<dyn LlmClient>>,
}

enum BuilderMode {
    Create(Arc<MobDefinition>),
    Resume,
}

struct RuntimeWiring {
    roster: Arc<RwLock<Roster>>,
    state: Arc<AtomicU8>,
    mcp_running: Arc<RwLock<BTreeMap<String, bool>>>,
    command_tx: mpsc::Sender<MobCommand>,
    command_rx: mpsc::Receiver<MobCommand>,
}

impl MobBuilder {
    /// Create a builder for a new mob.
    pub fn new(definition: MobDefinition, storage: MobStorage) -> Self {
        Self {
            mode: BuilderMode::Create(Arc::new(definition)),
            storage,
            session_service: None,
            tool_bundles: BTreeMap::new(),
            default_llm_client: None,
        }
    }

    /// Create a builder that resumes a mob from persisted events.
    pub fn for_resume(storage: MobStorage) -> Self {
        Self {
            mode: BuilderMode::Resume,
            storage,
            session_service: None,
            tool_bundles: BTreeMap::new(),
            default_llm_client: None,
        }
    }

    /// Set the session service for creating meerkat sessions.
    ///
    /// The service must implement both `SessionService` and `MobSessionService`
    /// to provide comms runtime access for wiring operations.
    pub fn with_session_service(mut self, service: Arc<dyn MobSessionService>) -> Self {
        self.session_service = Some(service);
        self
    }

    /// Set a default LLM client override (primarily for testing).
    pub fn with_default_llm_client(mut self, client: Arc<dyn LlmClient>) -> Self {
        self.default_llm_client = Some(client);
        self
    }

    /// Register a named Rust tool bundle for profile `tools.rust_bundles` wiring.
    pub fn register_tool_bundle(
        mut self,
        name: impl Into<String>,
        dispatcher: Arc<dyn AgentToolDispatcher>,
    ) -> Self {
        self.tool_bundles.insert(name.into(), dispatcher);
        self
    }

    /// Create the mob: emit MobCreated event, start the actor, return handle.
    pub async fn create(self) -> Result<MobHandle, MobError> {
        let MobBuilder {
            mode,
            storage,
            session_service,
            tool_bundles,
            default_llm_client,
        } = self;

        let definition = match mode {
            BuilderMode::Create(definition) => definition,
            BuilderMode::Resume => {
                return Err(MobError::Internal(
                    "MobBuilder::create() cannot be used with for_resume(); call resume()"
                        .to_string(),
                ));
            }
        };
        let diagnostics = crate::validate::validate_definition(&definition);
        if !diagnostics.is_empty() {
            return Err(MobError::DefinitionError(diagnostics));
        }
        let session_service = session_service
            .ok_or_else(|| MobError::Internal("session_service is required".into()))?;
        if !session_service.supports_persistent_sessions() {
            return Err(MobError::Internal(
                "session_service must satisfy persistent-session contract (REQ-MOB-030)"
                    .to_string(),
            ));
        }

        // Emit MobCreated event first
        let definition_for_event = (*definition).clone();
        storage
            .events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                kind: MobEventKind::MobCreated {
                    definition: definition_for_event,
                },
            })
            .await?;

        Ok(Self::start_runtime(
            definition,
            Roster::new(),
            MobState::Running,
            storage.events.clone(),
            session_service,
            tool_bundles,
            default_llm_client,
        ))
    }

    /// Resume a mob from persisted events.
    ///
    /// Phase-1 behavior:
    /// - Recover definition from `MobCreated`.
    /// - Rebuild roster by replaying structural events.
    /// - Start actor/runtime in Running state.
    pub async fn resume(self) -> Result<MobHandle, MobError> {
        let MobBuilder {
            mode,
            storage,
            session_service,
            tool_bundles,
            default_llm_client,
        } = self;

        if !matches!(mode, BuilderMode::Resume) {
            return Err(MobError::Internal(
                "MobBuilder::resume() requires MobBuilder::for_resume(storage)".to_string(),
            ));
        }

        let session_service = session_service
            .ok_or_else(|| MobError::Internal("session_service is required".into()))?;
        if !session_service.supports_persistent_sessions() {
            return Err(MobError::Internal(
                "session_service must satisfy persistent-session contract (REQ-MOB-030)"
                    .to_string(),
            ));
        }
        let all_events = storage.events.replay_all().await?;

        let definition = all_events
            .iter()
            .find_map(|event| match &event.kind {
                MobEventKind::MobCreated { definition } => Some(definition.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal(
                    "cannot resume mob: no MobCreated event found in storage".to_string(),
                )
            })?;

        let definition = Arc::new(definition);
        let diagnostics = crate::validate::validate_definition(&definition);
        if !diagnostics.is_empty() {
            return Err(MobError::DefinitionError(diagnostics));
        }
        let mob_events: Vec<_> = all_events
            .into_iter()
            .filter(|event| event.mob_id == definition.id)
            .collect();
        let mut roster = Roster::project(&mob_events);
        let resumed_state = if mob_events
            .iter()
            .any(|event| matches!(event.kind, MobEventKind::MobCompleted))
        {
            MobState::Completed
        } else {
            MobState::Running
        };

        // Prepare shared runtime components early so resume reconciliation can
        // wire tool dispatchers for recreated sessions to the final actor channel.
        let roster_state = Arc::new(RwLock::new(Roster::new()));
        let state = Arc::new(AtomicU8::new(resumed_state as u8));
        let mcp_servers_running = resumed_state == MobState::Running;
        let mcp_running = Arc::new(RwLock::new(
            definition
                .mcp_servers
                .keys()
                .map(|name| (name.clone(), mcp_servers_running))
                .collect::<BTreeMap<_, _>>(),
        ));
        let (command_tx, command_rx) = mpsc::channel(64);
        let wiring = RuntimeWiring {
            roster: roster_state.clone(),
            state: state.clone(),
            mcp_running: mcp_running.clone(),
            command_tx: command_tx.clone(),
            command_rx,
        };
        let preview_handle = MobHandle {
            command_tx: command_tx.clone(),
            roster: roster_state.clone(),
            definition: definition.clone(),
            state: state.clone(),
            events: storage.events.clone(),
            mcp_running: mcp_running.clone(),
        };

        if resumed_state == MobState::Running {
            Self::reconcile_resume(
                &definition,
                &mut roster,
                &session_service,
                default_llm_client.clone(),
                &tool_bundles,
                &preview_handle,
            )
            .await?;
        }

        *wiring.roster.write().await = roster;

        Ok(Self::start_runtime_with_components(
            definition,
            wiring,
            storage.events.clone(),
            session_service,
            tool_bundles,
            default_llm_client,
        ))
    }

    async fn reconcile_resume(
        definition: &Arc<MobDefinition>,
        roster: &mut Roster,
        session_service: &Arc<dyn MobSessionService>,
        default_llm_client: Option<Arc<dyn LlmClient>>,
        tool_bundles: &BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        tool_handle: &MobHandle,
    ) -> Result<(), MobError> {
        let active_sessions = session_service
            .list(meerkat_core::service::SessionQuery::default())
            .await?;
        let active_ids = active_sessions
            .iter()
            .map(|s| s.session_id.clone())
            .collect::<std::collections::HashSet<_>>();

        let roster_entries = roster.list().cloned().collect::<Vec<_>>();
        let roster_session_ids = roster_entries
            .iter()
            .map(|entry| entry.session_id.clone())
            .collect::<std::collections::HashSet<_>>();

        // Archive orphan sessions that are active but not present in the event-projected roster.
        for session_id in active_ids.difference(&roster_session_ids) {
            if session_service
                .session_belongs_to_mob(session_id, &definition.id)
                .await
                && let Err(error) = session_service.archive(session_id).await
                && !matches!(error, meerkat_core::service::SessionError::NotFound { .. })
            {
                return Err(error.into());
            }
        }

        // Recreate missing sessions referenced by MeerkatSpawned events.
        for entry in &roster_entries {
            if !active_ids.contains(&entry.session_id) {
                let profile = definition
                    .profiles
                    .get(&entry.profile)
                    .ok_or_else(|| MobError::ProfileNotFound(entry.profile.to_string()))?;
                let mut config = build::build_agent_config(
                    &definition.id,
                    &entry.profile,
                    &entry.meerkat_id,
                    profile,
                    definition,
                    compose_external_tools_for_profile(
                        profile,
                        tool_bundles,
                        Some(tool_handle.clone()),
                    )?,
                )?;
                if let Some(ref client) = default_llm_client {
                    config.llm_client_override = Some(client.clone());
                }
                let prompt = format!(
                    "You have been spawned as '{}' (role: {}) in mob '{}'.",
                    entry.meerkat_id, entry.profile, definition.id
                );
                let req = build::to_create_session_request(&config, prompt);
                let created = session_service.create_session(req).await?;
                let _ = roster.set_session_id(&entry.meerkat_id, created.session_id);
            }
        }

        // Re-establish trust from projected wiring (idempotent upsert).
        let entries = roster.list().cloned().collect::<Vec<_>>();
        for entry in &entries {
            let comms_a = match session_service.comms_runtime(&entry.session_id).await {
                Some(comms) => comms,
                None => continue,
            };
            let key_a = match comms_a.public_key() {
                Some(key) => key,
                None => continue,
            };
            let name_a = format!("{}/{}/{}", definition.id, entry.profile, entry.meerkat_id);

            for peer_id in &entry.wired_to {
                if entry.meerkat_id.as_str() >= peer_id.as_str() {
                    continue;
                }
                let Some(peer_entry) = roster.get(peer_id).cloned() else {
                    continue;
                };
                let Some(comms_b) = session_service.comms_runtime(&peer_entry.session_id).await
                else {
                    continue;
                };
                let Some(key_b) = comms_b.public_key() else {
                    continue;
                };
                let name_b = format!(
                    "{}/{}/{}",
                    definition.id, peer_entry.profile, peer_entry.meerkat_id
                );

                let spec_b = TrustedPeerSpec::new(&name_b, key_b.clone(), format!("inproc://{name_b}"))
                    .map_err(|e| MobError::WiringError(format!("invalid peer spec: {e}")))?;
                let spec_a = TrustedPeerSpec::new(&name_a, key_a.clone(), format!("inproc://{name_a}"))
                    .map_err(|e| MobError::WiringError(format!("invalid peer spec: {e}")))?;
                comms_a.add_trusted_peer(spec_b).await?;
                comms_b.add_trusted_peer(spec_a).await?;
            }
        }

        // Notify orchestrator that the mob resumed.
        if let Some(orchestrator) = &definition.orchestrator
            && let Some(orchestrator_entry) = roster.by_profile(&orchestrator.profile).next()
        {
            let _ = session_service
                .start_turn(
                    &orchestrator_entry.session_id,
                    meerkat_core::service::StartTurnRequest {
                        prompt: format!(
                            "Mob '{}' resumed. Reconcile worker state and continue orchestration.",
                            definition.id
                        ),
                        event_tx: None,
                        host_mode: true,
                        skill_references: None,
                    },
                )
                .await;
        }

        Ok(())
    }

    fn start_runtime(
        definition: Arc<MobDefinition>,
        initial_roster: Roster,
        initial_state: MobState,
        events: Arc<dyn MobEventStore>,
        session_service: Arc<dyn MobSessionService>,
        tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        default_llm_client: Option<Arc<dyn LlmClient>>,
    ) -> MobHandle {
        let roster = Arc::new(RwLock::new(initial_roster));
        let state = Arc::new(AtomicU8::new(initial_state as u8));
        let mcp_servers_running = initial_state == MobState::Running;
        let mcp_running = Arc::new(RwLock::new(
            definition
                .mcp_servers
                .keys()
                .map(|name| (name.clone(), mcp_servers_running))
                .collect::<BTreeMap<_, _>>(),
        ));
        let (command_tx, command_rx) = mpsc::channel(64);
        let wiring = RuntimeWiring {
            roster,
            state,
            mcp_running,
            command_tx,
            command_rx,
        };

        Self::start_runtime_with_components(
            definition,
            wiring,
            events,
            session_service,
            tool_bundles,
            default_llm_client,
        )
    }

    fn start_runtime_with_components(
        definition: Arc<MobDefinition>,
        wiring: RuntimeWiring,
        events: Arc<dyn MobEventStore>,
        session_service: Arc<dyn MobSessionService>,
        tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
        default_llm_client: Option<Arc<dyn LlmClient>>,
    ) -> MobHandle {
        let RuntimeWiring {
            roster,
            state,
            mcp_running,
            command_tx,
            command_rx,
        } = wiring;
        let handle = MobHandle {
            command_tx: command_tx.clone(),
            roster: roster.clone(),
            definition: definition.clone(),
            state: state.clone(),
            events: events.clone(),
            mcp_running: mcp_running.clone(),
        };

        let actor = MobActor {
            definition,
            roster,
            state,
            events,
            session_service,
            mcp_running,
            command_tx,
            tool_bundles,
            default_llm_client,
        };
        tokio::spawn(actor.run(command_rx));

        handle
    }
}

// ---------------------------------------------------------------------------
// MobActor
// ---------------------------------------------------------------------------

/// The actor that processes mob commands sequentially.
///
/// Owns all mutable state. Runs in a dedicated tokio task.
/// All mutations go through here; reads bypass via shared `Arc` state.
struct MobActor {
    definition: Arc<MobDefinition>,
    roster: Arc<RwLock<Roster>>,
    state: Arc<AtomicU8>,
    events: Arc<dyn MobEventStore>,
    session_service: Arc<dyn MobSessionService>,
    mcp_running: Arc<RwLock<BTreeMap<String, bool>>>,
    command_tx: mpsc::Sender<MobCommand>,
    tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    default_llm_client: Option<Arc<dyn LlmClient>>,
}

impl MobActor {
    fn state(&self) -> MobState {
        MobState::from_u8(self.state.load(Ordering::Acquire))
    }

    fn mob_handle_for_tools(&self) -> MobHandle {
        MobHandle {
            command_tx: self.command_tx.clone(),
            roster: self.roster.clone(),
            definition: self.definition.clone(),
            state: self.state.clone(),
            events: self.events.clone(),
            mcp_running: self.mcp_running.clone(),
        }
    }

    fn expect_state(&self, expected: MobState, to: &'static str) -> Result<(), MobError> {
        let current = self.state();
        if current != expected {
            return Err(MobError::InvalidTransition {
                from: current.as_str(),
                to,
            });
        }
        Ok(())
    }

    async fn notify_orchestrator_lifecycle(&self, message: String) {
        if let Some(orchestrator) = &self.definition.orchestrator
            && let Some(orchestrator_entry) = self
                .roster
                .read()
                .await
                .by_profile(&orchestrator.profile)
                .next()
                .cloned()
        {
            let _ = self
                .session_service
                .start_turn(
                    &orchestrator_entry.session_id,
                    meerkat_core::service::StartTurnRequest {
                        prompt: message,
                        event_tx: None,
                        host_mode: true,
                        skill_references: None,
                    },
                )
                .await;
        }
    }

    async fn stop_mcp_servers(&self) -> Result<(), MobError> {
        let mut running = self.mcp_running.write().await;
        for state in running.values_mut() {
            *state = false;
        }
        Ok(())
    }

    async fn start_mcp_servers(&self) -> Result<(), MobError> {
        let mut running = self.mcp_running.write().await;
        for state in running.values_mut() {
            *state = true;
        }
        Ok(())
    }

    async fn cleanup_namespace(&self) -> Result<(), MobError> {
        self.mcp_running.write().await.clear();
        Ok(())
    }

    /// Main actor loop: process commands sequentially until Shutdown.
    async fn run(self, mut command_rx: mpsc::Receiver<MobCommand>) {
        while let Some(cmd) = command_rx.recv().await {
            match cmd {
                MobCommand::Spawn {
                    profile_name,
                    meerkat_id,
                    initial_message,
                    reply_tx,
                } => {
                    let result = match self.expect_state(MobState::Running, "Running") {
                        Ok(()) => self
                            .handle_spawn(profile_name, meerkat_id, initial_message)
                            .await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Retire {
                    meerkat_id,
                    reply_tx,
                } => {
                    let result = match self.expect_state(MobState::Running, "Running") {
                        Ok(()) => self.handle_retire(meerkat_id).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Wire { a, b, reply_tx } => {
                    let result = match self.expect_state(MobState::Running, "Running") {
                        Ok(()) => self.handle_wire(a, b).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Unwire { a, b, reply_tx } => {
                    let result = match self.expect_state(MobState::Running, "Running") {
                        Ok(()) => self.handle_unwire(a, b).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::ExternalTurn {
                    meerkat_id,
                    message,
                    reply_tx,
                } => {
                    let result = match self.expect_state(MobState::Running, "Running") {
                        Ok(()) => self.handle_external_turn(meerkat_id, message).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::InternalTurn {
                    meerkat_id,
                    message,
                    reply_tx,
                } => {
                    let result = match self.expect_state(MobState::Running, "Running") {
                        Ok(()) => self.handle_internal_turn(meerkat_id, message).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Stop { reply_tx } => {
                    let result = match self.expect_state(MobState::Running, "Stopped") {
                        Ok(()) => {
                            self.notify_orchestrator_lifecycle(format!(
                                "Mob '{}' is stopping.",
                                self.definition.id
                            ))
                            .await;
                            if let Err(error) = self.stop_mcp_servers().await {
                                Err(error)
                            } else {
                                self.state.store(MobState::Stopped as u8, Ordering::Release);
                                Ok(())
                            }
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::ResumeLifecycle { reply_tx } => {
                    let result = match self.expect_state(MobState::Stopped, "Running") {
                        Ok(()) => {
                            if let Err(error) = self.start_mcp_servers().await {
                                Err(error)
                            } else {
                                self.state.store(MobState::Running as u8, Ordering::Release);
                                Ok(())
                            }
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Complete { reply_tx } => {
                    let result = match self.expect_state(MobState::Running, "Completed") {
                        Ok(()) => self.handle_complete().await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Destroy { reply_tx } => {
                    let result = match self.state() {
                        MobState::Running | MobState::Stopped | MobState::Completed => {
                            self.handle_destroy().await
                        }
                        current => Err(MobError::InvalidTransition {
                            from: current.as_str(),
                            to: "Destroyed",
                        }),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::TaskCreate {
                    task_id,
                    subject,
                    description,
                    blocked_by,
                    reply_tx,
                } => {
                    let result = match self.expect_state(MobState::Running, "Running") {
                        Ok(()) => {
                            self.handle_task_create(task_id, subject, description, blocked_by)
                                .await
                        }
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::TaskUpdate {
                    task_id,
                    status,
                    owner,
                    reply_tx,
                } => {
                    let result = match self.expect_state(MobState::Running, "Running") {
                        Ok(()) => self.handle_task_update(task_id, status, owner).await,
                        Err(error) => Err(error),
                    };
                    let _ = reply_tx.send(result);
                }
                MobCommand::Shutdown { reply_tx } => {
                    self.state.store(MobState::Stopped as u8, Ordering::Release);
                    let _ = reply_tx.send(());
                    break;
                }
            }
        }
    }

    /// P1-T04: spawn() creates a real session.
    ///
    /// Effect-first ordering: create session -> emit event -> apply wiring -> update roster.
    async fn handle_spawn(
        &self,
        profile_name: ProfileName,
        meerkat_id: MeerkatId,
        initial_message: Option<String>,
    ) -> Result<SessionId, MobError> {
        // Check meerkat doesn't already exist
        {
            let roster = self.roster.read().await;
            if roster.get(&meerkat_id).is_some() {
                return Err(MobError::MeerkatAlreadyExists(meerkat_id.to_string()));
            }
        }

        // Resolve profile
        let profile = self
            .definition
            .profiles
            .get(&profile_name)
            .ok_or_else(|| MobError::ProfileNotFound(profile_name.to_string()))?;

        // Build agent config
        let external_tools = self.external_tools_for_profile(profile)?;
        let mut config = build::build_agent_config(
            &self.definition.id,
            &profile_name,
            &meerkat_id,
            profile,
            &self.definition,
            external_tools,
        )?;

        // Inject default LLM client if set
        if let Some(ref client) = self.default_llm_client {
            config.llm_client_override = Some(client.clone());
        }

        // Create the session via SessionService (effect-first)
        let prompt = initial_message.unwrap_or_else(|| {
            format!(
                "You have been spawned as '{}' (role: {}) in mob '{}'.",
                meerkat_id, profile_name, self.definition.id
            )
        });
        let req = build::to_create_session_request(&config, prompt);
        let result = self.session_service.create_session(req).await?;
        let session_id = result.session_id;

        // Emit MeerkatSpawned event
        if let Err(append_error) = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                kind: MobEventKind::MeerkatSpawned {
                    meerkat_id: meerkat_id.clone(),
                    role: profile_name.clone(),
                    session_id: session_id.clone(),
                },
            })
            .await
        {
            if let Err(rollback_error) = self.session_service.archive(&session_id).await {
                return Err(MobError::Internal(format!(
                    "spawn append failed for '{meerkat_id}': {append_error}; archive compensation failed for session '{session_id}': {rollback_error}"
                )));
            }
            return Err(append_error);
        }

        // Update roster
        {
            let mut roster = self.roster.write().await;
            roster.add(meerkat_id.clone(), profile_name.clone(), session_id.clone());
        }

        let planned_wiring_targets = self
            .spawn_wiring_targets(&profile_name, &meerkat_id)
            .await;

        if let Err(wiring_error) = self.apply_spawn_wiring(&profile_name, &meerkat_id).await {
            if let Err(rollback_error) = self
                .rollback_failed_spawn(
                    &meerkat_id,
                    &profile_name,
                    &session_id,
                    &planned_wiring_targets,
                )
                .await
            {
                return Err(MobError::Internal(format!(
                    "spawn wiring failed for '{meerkat_id}': {wiring_error}; rollback failed: {rollback_error}"
                )));
            }
            return Err(wiring_error);
        }

        Ok(session_id)
    }

    async fn spawn_wiring_targets(
        &self,
        profile_name: &ProfileName,
        meerkat_id: &MeerkatId,
    ) -> Vec<MeerkatId> {
        let mut targets = Vec::new();

        if self.definition.wiring.auto_wire_orchestrator
            && let Some(orchestrator) = &self.definition.orchestrator
            && profile_name != &orchestrator.profile
        {
            let orchestrator_ids = {
                let roster = self.roster.read().await;
                roster
                    .by_profile(&orchestrator.profile)
                    .map(|entry| entry.meerkat_id.clone())
                    .collect::<Vec<_>>()
            };
            for orchestrator_id in orchestrator_ids {
                if orchestrator_id != *meerkat_id && !targets.contains(&orchestrator_id) {
                    targets.push(orchestrator_id);
                }
            }
        }

        for rule in &self.definition.wiring.role_wiring {
            let target_profile = if &rule.a == profile_name {
                Some(&rule.b)
            } else if &rule.b == profile_name {
                Some(&rule.a)
            } else {
                None
            };
            if let Some(target_profile) = target_profile {
                let target_ids = {
                    let roster = self.roster.read().await;
                    roster
                        .by_profile(target_profile)
                        .filter(|entry| entry.meerkat_id != *meerkat_id)
                        .map(|entry| entry.meerkat_id.clone())
                        .collect::<Vec<_>>()
                };
                for target_id in target_ids {
                    if !targets.contains(&target_id) {
                        targets.push(target_id);
                    }
                }
            }
        }

        targets
    }

    /// P1-T05: retire() removes a meerkat.
    ///
    /// Append retire event -> notify wired peers -> remove trust -> archive session -> update roster.
    ///
    /// Event-first ordering ensures append failures remain retryable without
    /// committing side effects that cannot be replay-projected.
    async fn handle_retire(&self, meerkat_id: MeerkatId) -> Result<(), MobError> {
        // Idempotent: already retired / never existed is success.
        let entry = {
            let roster = self.roster.read().await;
            let Some(entry) = roster.get(&meerkat_id).cloned() else {
                return Ok(());
            };
            entry
        };

        let retire_event_already_present = self
            .retire_event_exists(&meerkat_id, &entry.session_id)
            .await?;
        if !retire_event_already_present {
            self.append_retire_event(&meerkat_id, &entry.profile, &entry.session_id)
                .await?;
        }

        // Notify wired peers and remove trust.
        if !entry.wired_to.is_empty() {
            let retiring_comms = self.session_service_comms(&entry.session_id).await;
            let retiring_key = retiring_comms.as_ref().and_then(|comms| comms.public_key());

            // If a retire event already exists, this can be a retry path after a
            // prior partial cleanup. Missing comms in that case should not block
            // roster convergence.
            let Some(retiring_comms) = retiring_comms else {
                if !retire_event_already_present {
                    return Err(MobError::WiringError(format!(
                        "retire requires comms runtime for '{meerkat_id}'"
                    )));
                }
                if let Err(error) = self.session_service.archive(&entry.session_id).await
                    && !matches!(error, meerkat_core::service::SessionError::NotFound { .. })
                {
                    return Err(error.into());
                }
                let mut roster = self.roster.write().await;
                roster.remove(&meerkat_id);
                return Ok(());
            };
            let Some(retiring_key) = retiring_key else {
                if !retire_event_already_present {
                    return Err(MobError::WiringError(format!(
                        "retire requires public key for '{meerkat_id}'"
                    )));
                }
                if let Err(error) = self.session_service.archive(&entry.session_id).await
                    && !matches!(error, meerkat_core::service::SessionError::NotFound { .. })
                {
                    return Err(error.into());
                }
                let mut roster = self.roster.write().await;
                roster.remove(&meerkat_id);
                return Ok(());
            };

            // Notify wired peers about retirement (required for successful retire).
            for peer_id in &entry.wired_to {
                self.notify_peer_retired(peer_id, &meerkat_id, &entry, &retiring_comms)
                    .await?;
            }

            // Remove trust from wired peers
            for peer_meerkat_id in &entry.wired_to {
                let peer_entry = {
                    let roster = self.roster.read().await;
                    roster.get(peer_meerkat_id).cloned().ok_or_else(|| {
                        MobError::WiringError(format!(
                            "retire cannot remove trust for '{meerkat_id}': peer '{peer_meerkat_id}' missing from roster"
                        ))
                    })?
                };
                let peer_comms = self
                    .session_service_comms(&peer_entry.session_id)
                    .await
                    .ok_or_else(|| {
                        MobError::WiringError(format!(
                            "retire cannot remove trust for '{meerkat_id}': comms runtime missing for peer '{peer_meerkat_id}'"
                        ))
                    })?;
                peer_comms.remove_trusted_peer(&retiring_key).await?;
            }
        }

        // Archive session (required)
        if let Err(error) = self.session_service.archive(&entry.session_id).await
            && !matches!(error, meerkat_core::service::SessionError::NotFound { .. })
        {
            return Err(error.into());
        }

        // Update roster
        {
            let mut roster = self.roster.write().await;
            roster.remove(&meerkat_id);
        }

        Ok(())
    }

    /// P1-T06: wire() establishes bidirectional trust.
    async fn handle_wire(&self, a: MeerkatId, b: MeerkatId) -> Result<(), MobError> {
        // Verify both exist
        {
            let roster = self.roster.read().await;
            if roster.get(&a).is_none() {
                return Err(MobError::MeerkatNotFound(a.to_string()));
            }
            if roster.get(&b).is_none() {
                return Err(MobError::MeerkatNotFound(b.to_string()));
            }
        }

        self.do_wire(&a, &b).await
    }

    /// P1-T07: unwire() removes bidirectional trust.
    async fn handle_unwire(&self, a: MeerkatId, b: MeerkatId) -> Result<(), MobError> {
        // Look up both entries
        let (entry_a, entry_b) = {
            let roster = self.roster.read().await;
            let ea = roster
                .get(&a)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(a.to_string()))?;
            let eb = roster
                .get(&b)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(b.to_string()))?;
            (ea, eb)
        };

        // Get comms and keys for both sides (required for unwire).
        let comms_a = self
            .session_service_comms(&entry_a.session_id)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!("unwire requires comms runtime for '{a}'"))
            })?;
        let comms_b = self
            .session_service_comms(&entry_b.session_id)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!("unwire requires comms runtime for '{b}'"))
            })?;
        let key_a = comms_a.public_key().ok_or_else(|| {
            MobError::WiringError(format!("unwire requires public key for '{a}'"))
        })?;
        let key_b = comms_b.public_key().ok_or_else(|| {
            MobError::WiringError(format!("unwire requires public key for '{b}'"))
        })?;
        let comms_name_a = self.comms_name_for(&entry_a);
        let comms_name_b = self.comms_name_for(&entry_b);
        let spec_a = TrustedPeerSpec::new(
            &comms_name_a,
            key_a.clone(),
            format!("inproc://{comms_name_a}"),
        )
        .map_err(|e| MobError::WiringError(format!("invalid peer spec: {e}")))?;
        let spec_b = TrustedPeerSpec::new(
            &comms_name_b,
            key_b.clone(),
            format!("inproc://{comms_name_b}"),
        )
        .map_err(|e| MobError::WiringError(format!("invalid peer spec: {e}")))?;

        // Notify both peers BEFORE removing trust (need trust to send).
        // Send FROM a TO b: notify b that a is being unwired
        self.notify_peer_retired(&b, &a, &entry_a, &comms_a).await?;
        // Send FROM b TO a: notify a that b is being unwired
        if let Err(second_notification_error) =
            self.notify_peer_retired(&a, &b, &entry_b, &comms_b).await
        {
            if let Err(compensation_error) =
                self.notify_peer_added(&comms_a, &comms_name_b, &a, &entry_a).await
            {
                return Err(MobError::Internal(format!(
                    "unwire second notification failed for '{b}' -> '{a}': {second_notification_error}; compensating mob.peer_added '{a}' -> '{b}' failed: {compensation_error}"
                )));
            }
            return Err(second_notification_error);
        }

        // Remove trust on both sides (required)
        if let Err(remove_a_error) = comms_a.remove_trusted_peer(&key_b).await {
            let mut cleanup_errors = Vec::new();
            if let Err(compensation_error) =
                self.notify_peer_added(&comms_b, &comms_name_a, &b, &entry_b).await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_added '{b}' -> '{a}' failed: {compensation_error}"
                ));
            }
            if let Err(compensation_error) =
                self.notify_peer_added(&comms_a, &comms_name_b, &a, &entry_a).await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_added '{a}' -> '{b}' failed: {compensation_error}"
                ));
            }
            if !cleanup_errors.is_empty() {
                return Err(MobError::Internal(format!(
                    "unwire failed removing trust '{a}' -> '{b}': {remove_a_error}; rollback failures: {}",
                    cleanup_errors.join("; ")
                )));
            }
            return Err(remove_a_error.into());
        }
        if let Err(remove_b_error) = comms_b.remove_trusted_peer(&key_a).await {
            let mut cleanup_errors = Vec::new();
            if let Err(restore_error) = comms_a.add_trusted_peer(spec_b.clone()).await {
                cleanup_errors.push(format!(
                    "restore trust '{a}' -> '{b}' failed after second removal failure: {restore_error}"
                ));
            }
            if let Err(compensation_error) =
                self.notify_peer_added(&comms_b, &comms_name_a, &b, &entry_b).await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_added '{b}' -> '{a}' failed: {compensation_error}"
                ));
            }
            if let Err(compensation_error) =
                self.notify_peer_added(&comms_a, &comms_name_b, &a, &entry_a).await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_added '{a}' -> '{b}' failed: {compensation_error}"
                ));
            }
            if !cleanup_errors.is_empty() {
                return Err(MobError::Internal(format!(
                    "unwire failed removing trust '{b}' -> '{a}': {remove_b_error}; rollback failures: {}",
                    cleanup_errors.join("; ")
                )));
            }
            return Err(remove_b_error.into());
        }

        // Emit PeersUnwired event
        if let Err(append_error) = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                kind: MobEventKind::PeersUnwired {
                    a: a.clone(),
                    b: b.clone(),
                },
            })
            .await
        {
            let mut cleanup_errors = Vec::new();
            if let Err(error) = comms_a.add_trusted_peer(spec_b).await {
                cleanup_errors.push(format!(
                    "restore trust '{a}' -> '{b}' failed during append rollback: {error}"
                ));
            }
            if let Err(error) = comms_b.add_trusted_peer(spec_a).await {
                cleanup_errors.push(format!(
                    "restore trust '{b}' -> '{a}' failed during append rollback: {error}"
                ));
            }
            if let Err(error) = self.notify_peer_added(&comms_b, &comms_name_a, &b, &entry_b).await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_added '{b}' -> '{a}' failed during append rollback: {error}"
                ));
            }
            if let Err(error) = self.notify_peer_added(&comms_a, &comms_name_b, &a, &entry_a).await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_added '{a}' -> '{b}' failed during append rollback: {error}"
                ));
            }
            if !cleanup_errors.is_empty() {
                return Err(MobError::Internal(format!(
                    "unwire append failed for '{a}' <-> '{b}': {append_error}; rollback failures: {}",
                    cleanup_errors.join("; ")
                )));
            }
            return Err(append_error);
        }

        // Update roster
        {
            let mut roster = self.roster.write().await;
            roster.unwire(&a, &b);
        }

        Ok(())
    }

    async fn handle_complete(&self) -> Result<(), MobError> {
        self.notify_orchestrator_lifecycle(format!("Mob '{}' is completing.", self.definition.id))
            .await;
        let ids = {
            let roster = self.roster.read().await;
            roster
                .list()
                .map(|entry| entry.meerkat_id.clone())
                .collect::<Vec<_>>()
        };
        for id in ids {
            self.handle_retire(id).await?;
        }
        self.stop_mcp_servers().await?;

        self.events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                kind: MobEventKind::MobCompleted,
            })
            .await?;
        self.state
            .store(MobState::Completed as u8, Ordering::Release);
        Ok(())
    }

    async fn handle_destroy(&self) -> Result<(), MobError> {
        self.notify_orchestrator_lifecycle(format!("Mob '{}' is destroying.", self.definition.id))
            .await;
        let ids = {
            let roster = self.roster.read().await;
            roster
                .list()
                .map(|entry| entry.meerkat_id.clone())
                .collect::<Vec<_>>()
        };
        for id in ids {
            self.handle_retire(id).await?;
        }
        self.stop_mcp_servers().await?;
        self.events.clear().await?;
        self.cleanup_namespace().await?;
        self.state
            .store(MobState::Destroyed as u8, Ordering::Release);
        Ok(())
    }

    async fn handle_task_create(
        &self,
        task_id: String,
        subject: String,
        description: String,
        blocked_by: Vec<String>,
    ) -> Result<(), MobError> {
        if task_id.trim().is_empty() {
            return Err(MobError::Internal("task_id cannot be empty".to_string()));
        }
        if subject.trim().is_empty() {
            return Err(MobError::Internal("task subject cannot be empty".to_string()));
        }

        let board = TaskBoard::project(&self.events.replay_all().await?);
        if board.get(&task_id).is_some() {
            return Err(MobError::Internal(format!(
                "task '{task_id}' already exists"
            )));
        }

        self.events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                kind: MobEventKind::TaskCreated {
                    task_id,
                    subject,
                    description,
                    blocked_by,
                },
            })
            .await?;
        Ok(())
    }

    async fn handle_task_update(
        &self,
        task_id: String,
        status: TaskStatus,
        owner: Option<MeerkatId>,
    ) -> Result<(), MobError> {
        let board = TaskBoard::project(&self.events.replay_all().await?);
        let task = board
            .get(&task_id)
            .ok_or_else(|| MobError::Internal(format!("task '{task_id}' not found")))?;

        if owner.is_some() {
            if !matches!(status, TaskStatus::InProgress) {
                return Err(MobError::Internal(format!(
                    "task '{task_id}' owner can only be set with in_progress status"
                )));
            }
            let blocked = task.blocked_by.iter().any(|dependency| {
                board.get(dependency).map(|t| t.status) != Some(TaskStatus::Completed)
            });
            if blocked {
                return Err(MobError::Internal(format!(
                    "task '{task_id}' is blocked by incomplete dependencies"
                )));
            }
        }

        self.events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                kind: MobEventKind::TaskUpdated {
                    task_id,
                    status,
                    owner,
                },
            })
            .await?;
        Ok(())
    }

    /// P1-T10: external_turn enforces addressability.
    async fn handle_external_turn(
        &self,
        meerkat_id: MeerkatId,
        message: String,
    ) -> Result<(), MobError> {
        // Look up the entry
        let entry = {
            let roster = self.roster.read().await;
            roster
                .get(&meerkat_id)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(meerkat_id.to_string()))?
        };

        // Check external_addressable
        let profile = self
            .definition
            .profiles
            .get(&entry.profile)
            .ok_or_else(|| MobError::ProfileNotFound(entry.profile.to_string()))?;

        if !profile.external_addressable {
            return Err(MobError::NotExternallyAddressable(meerkat_id.to_string()));
        }

        // Start a turn on the session
        let req = meerkat_core::service::StartTurnRequest {
            prompt: message,
            event_tx: None,
            host_mode: false,
            skill_references: None,
        };
        self.session_service
            .start_turn(&entry.session_id, req)
            .await?;

        Ok(())
    }

    /// Internal-turn path bypasses external_addressable checks.
    async fn handle_internal_turn(
        &self,
        meerkat_id: MeerkatId,
        message: String,
    ) -> Result<(), MobError> {
        let entry = {
            let roster = self.roster.read().await;
            roster
                .get(&meerkat_id)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(meerkat_id.to_string()))?
        };

        let req = meerkat_core::service::StartTurnRequest {
            prompt: message,
            event_tx: None,
            host_mode: false,
            skill_references: None,
        };
        self.session_service
            .start_turn(&entry.session_id, req)
            .await?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Apply auto/role wiring for a newly spawned meerkat.
    async fn apply_spawn_wiring(
        &self,
        profile_name: &ProfileName,
        meerkat_id: &MeerkatId,
    ) -> Result<(), MobError> {
        // P1-T08: Auto-wire to orchestrator if configured
        if self.definition.wiring.auto_wire_orchestrator
            && let Some(ref orch) = self.definition.orchestrator
        {
            let orch_id = {
                let roster = self.roster.read().await;
                roster
                    .by_profile(&orch.profile)
                    .next()
                    .map(|e| e.meerkat_id.clone())
            };
            if let Some(ref orch_meerkat_id) = orch_id
                && *orch_meerkat_id != *meerkat_id
            {
                self.do_wire(meerkat_id, orch_meerkat_id).await.map_err(|e| {
                    MobError::WiringError(format!(
                        "auto_wire_orchestrator failed for {meerkat_id} <-> {orch_meerkat_id}: {e}"
                    ))
                })?;
            }
        }

        // P1-T09: Role-wiring fan-out
        for rule in &self.definition.wiring.role_wiring {
            let target_role = if *profile_name == rule.a {
                Some(&rule.b)
            } else if *profile_name == rule.b {
                Some(&rule.a)
            } else {
                None
            };

            if let Some(target) = target_role {
                let target_ids: Vec<MeerkatId> = {
                    let roster = self.roster.read().await;
                    roster
                        .by_profile(target)
                        .filter(|entry| entry.meerkat_id != *meerkat_id)
                        .map(|entry| entry.meerkat_id.clone())
                        .collect()
                };

                for target_id in target_ids {
                    self.do_wire(meerkat_id, &target_id).await.map_err(|e| {
                        MobError::WiringError(format!(
                            "role_wiring fan-out failed for {meerkat_id} <-> {target_id}: {e}"
                        ))
                    })?;
                }
            }
        }

        Ok(())
    }

    /// Compensate a failed spawn wiring path to avoid partial state.
    async fn rollback_failed_spawn(
        &self,
        meerkat_id: &MeerkatId,
        profile_name: &ProfileName,
        session_id: &SessionId,
        planned_wiring_targets: &[MeerkatId],
    ) -> Result<(), MobError> {
        let retire_event_already_present = self.retire_event_exists(meerkat_id, session_id).await?;
        if !retire_event_already_present {
            self.append_retire_event(meerkat_id, profile_name, session_id)
                .await?;
        }

        let wired_peers = {
            let roster = self.roster.read().await;
            roster
                .get(meerkat_id)
                .map(|entry| entry.wired_to.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        };
        let mut cleanup_peers = wired_peers.clone();
        for peer_id in planned_wiring_targets {
            if peer_id != meerkat_id && !cleanup_peers.contains(peer_id) {
                cleanup_peers.push(peer_id.clone());
            }
        }
        let spawned_entry = {
            let roster = self.roster.read().await;
            roster.get(meerkat_id).cloned()
        };
        let spawned_comms = self.session_service_comms(session_id).await;

        if !wired_peers.is_empty() {
            let spawned_comms = spawned_comms.as_ref().ok_or_else(|| {
                MobError::WiringError(format!(
                    "spawn rollback requires comms runtime for '{meerkat_id}'"
                ))
            })?;
            let spawned_entry = spawned_entry.as_ref().ok_or_else(|| {
                MobError::WiringError(format!(
                    "spawn rollback requires roster entry for '{meerkat_id}'"
                ))
            })?;
            for peer_meerkat_id in &wired_peers {
                self.notify_peer_retired(peer_meerkat_id, meerkat_id, spawned_entry, spawned_comms)
                    .await?;
            }
        }

        let spawned_key = spawned_comms.as_ref().and_then(|comms| comms.public_key());
        if let Some(spawned_key) = spawned_key {
            let mut cleanup_errors = Vec::new();
            for peer_meerkat_id in &cleanup_peers {
                let is_wired_peer = wired_peers.contains(peer_meerkat_id);
                let peer_entry = {
                    let roster = self.roster.read().await;
                    roster.get(peer_meerkat_id).cloned()
                };
                let Some(peer_entry) = peer_entry else {
                    if is_wired_peer {
                        cleanup_errors.push(format!(
                            "spawn rollback cannot remove trust for '{meerkat_id}': wired peer '{peer_meerkat_id}' missing from roster"
                        ));
                    }
                    continue;
                };
                let peer_comms = self
                    .session_service_comms(&peer_entry.session_id)
                    .await;
                let Some(peer_comms) = peer_comms else {
                    if is_wired_peer {
                        cleanup_errors.push(format!(
                            "spawn rollback cannot remove trust for '{meerkat_id}': comms runtime missing for wired peer '{peer_meerkat_id}'"
                        ));
                    }
                    continue;
                };
                if let Err(error) = peer_comms.remove_trusted_peer(&spawned_key).await
                    && is_wired_peer
                {
                    cleanup_errors.push(format!(
                        "spawn rollback cannot remove trust for '{meerkat_id}' from wired peer '{peer_meerkat_id}': {error}"
                    ));
                }
            }
            if !cleanup_errors.is_empty() {
                return Err(MobError::Internal(format!(
                    "spawn rollback cleanup failed for '{meerkat_id}': {}",
                    cleanup_errors.join("; ")
                )));
            }
        }

        if let Err(error) = self.session_service.archive(session_id).await
            && !matches!(error, meerkat_core::service::SessionError::NotFound { .. })
        {
            return Err(error.into());
        }

        {
            let mut roster = self.roster.write().await;
            roster.remove(meerkat_id);
        }

        Ok(())
    }

    /// Resolve profile-declared rust tool bundles to a dispatcher.
    fn external_tools_for_profile(
        &self,
        profile: &crate::profile::Profile,
    ) -> Result<Option<Arc<dyn AgentToolDispatcher>>, MobError> {
        compose_external_tools_for_profile(
            profile,
            &self.tool_bundles,
            Some(self.mob_handle_for_tools()),
        )
    }

    async fn retire_event_exists(
        &self,
        meerkat_id: &MeerkatId,
        session_id: &SessionId,
    ) -> Result<bool, MobError> {
        let events = self.events.replay_all().await?;
        Ok(events.iter().any(|event| {
            matches!(
                &event.kind,
                MobEventKind::MeerkatRetired {
                    meerkat_id: existing_meerkat,
                    session_id: existing_session,
                    ..
                } if existing_meerkat == meerkat_id && existing_session == session_id
            )
        }))
    }

    async fn append_retire_event(
        &self,
        meerkat_id: &MeerkatId,
        profile_name: &ProfileName,
        session_id: &SessionId,
    ) -> Result<(), MobError> {
        self.events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                kind: MobEventKind::MeerkatRetired {
                    meerkat_id: meerkat_id.clone(),
                    role: profile_name.clone(),
                    session_id: session_id.clone(),
                },
            })
            .await?;
        Ok(())
    }

    /// Internal wire operation (used by handle_wire and auto_wire/role_wiring).
    async fn do_wire(&self, a: &MeerkatId, b: &MeerkatId) -> Result<(), MobError> {
        let (entry_a, entry_b) = {
            let roster = self.roster.read().await;
            let ea = roster
                .get(a)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(a.to_string()))?;
            let eb = roster
                .get(b)
                .cloned()
                .ok_or_else(|| MobError::MeerkatNotFound(b.to_string()))?;
            (ea, eb)
        };

        // Establish bidirectional trust via comms (required).
        let comms_a = self
            .session_service_comms(&entry_a.session_id)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!("wire requires comms runtime for '{a}'"))
            })?;
        let comms_b = self
            .session_service_comms(&entry_b.session_id)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!("wire requires comms runtime for '{b}'"))
            })?;

        let key_a = comms_a
            .public_key()
            .ok_or_else(|| MobError::WiringError(format!("wire requires public key for '{a}'")))?;
        let key_b = comms_b
            .public_key()
            .ok_or_else(|| MobError::WiringError(format!("wire requires public key for '{b}'")))?;

        // Get peer info for trust establishment
        let comms_name_a = self.comms_name_for(&entry_a);
        let comms_name_b = self.comms_name_for(&entry_b);

        let spec_b = TrustedPeerSpec::new(
            &comms_name_b,
            key_b.clone(),
            format!("inproc://{comms_name_b}"),
        )
        .map_err(|e| MobError::WiringError(format!("invalid peer spec: {e}")))?;
        let spec_a = TrustedPeerSpec::new(
            &comms_name_a,
            key_a.clone(),
            format!("inproc://{comms_name_a}"),
        )
        .map_err(|e| MobError::WiringError(format!("invalid peer spec: {e}")))?;

        comms_a.add_trusted_peer(spec_b.clone()).await?;
        if let Err(error) = comms_b.add_trusted_peer(spec_a.clone()).await {
            if let Err(cleanup_error) = comms_a.remove_trusted_peer(&key_b).await {
                return Err(MobError::Internal(format!(
                    "wire failed adding trust '{b}' -> '{a}': {error}; rollback failed removing trust '{a}' -> '{b}': {cleanup_error}"
                )));
            }
            return Err(error.into());
        }

        // Notify both peers (required for successful wire):
        // Send FROM b TO a about new peer b
        if let Err(error) = self
            .notify_peer_added(&comms_b, &comms_name_a, b, &entry_b)
            .await
        {
            let mut cleanup_errors = Vec::new();
            if let Err(cleanup_error) = comms_a.remove_trusted_peer(&key_b).await {
                cleanup_errors.push(format!(
                    "remove trust '{a}' -> '{b}' failed during rollback: {cleanup_error}"
                ));
            }
            if let Err(cleanup_error) = comms_b.remove_trusted_peer(&key_a).await {
                cleanup_errors.push(format!(
                    "remove trust '{b}' -> '{a}' failed during rollback: {cleanup_error}"
                ));
            }
            if !cleanup_errors.is_empty() {
                return Err(MobError::Internal(format!(
                    "wire failed sending mob.peer_added '{b}' -> '{a}': {error}; rollback failures: {}",
                    cleanup_errors.join("; ")
                )));
            }
            return Err(error);
        }
        // Send FROM a TO b about new peer a
        if let Err(error) = self
            .notify_peer_added(&comms_a, &comms_name_b, a, &entry_a)
            .await
        {
            let mut cleanup_errors = Vec::new();
            if let Err(compensation_error) =
                self.notify_peer_retired(a, b, &entry_b, &comms_b).await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_retired '{b}' -> '{a}' failed: {compensation_error}"
                ));
            }
            if let Err(cleanup_error) = comms_a.remove_trusted_peer(&key_b).await {
                cleanup_errors.push(format!(
                    "remove trust '{a}' -> '{b}' failed during rollback: {cleanup_error}"
                ));
            }
            if let Err(cleanup_error) = comms_b.remove_trusted_peer(&key_a).await {
                cleanup_errors.push(format!(
                    "remove trust '{b}' -> '{a}' failed during rollback: {cleanup_error}"
                ));
            }
            if !cleanup_errors.is_empty() {
                return Err(MobError::Internal(format!(
                    "wire failed sending mob.peer_added '{a}' -> '{b}': {error}; rollback failures: {}",
                    cleanup_errors.join("; ")
                )));
            }
            return Err(error);
        }

        // Emit PeersWired event
        if let Err(append_error) = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                kind: MobEventKind::PeersWired {
                    a: a.clone(),
                    b: b.clone(),
                },
            })
            .await
        {
            let mut cleanup_errors = Vec::new();
            if let Err(compensation_error) =
                self.notify_peer_retired(a, b, &entry_b, &comms_b).await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_retired '{b}' -> '{a}' failed: {compensation_error}"
                ));
            }
            if let Err(compensation_error) =
                self.notify_peer_retired(b, a, &entry_a, &comms_a).await
            {
                cleanup_errors.push(format!(
                    "compensating mob.peer_retired '{a}' -> '{b}' failed: {compensation_error}"
                ));
            }
            if let Err(cleanup_error) = comms_a.remove_trusted_peer(&key_b).await {
                cleanup_errors.push(format!(
                    "remove trust '{a}' -> '{b}' failed during append rollback: {cleanup_error}"
                ));
            }
            if let Err(cleanup_error) = comms_b.remove_trusted_peer(&key_a).await {
                cleanup_errors.push(format!(
                    "remove trust '{b}' -> '{a}' failed during append rollback: {cleanup_error}"
                ));
            }
            if !cleanup_errors.is_empty() {
                return Err(MobError::Internal(format!(
                    "wire append failed for '{a}' <-> '{b}': {append_error}; rollback failures: {}",
                    cleanup_errors.join("; ")
                )));
            }
            return Err(append_error);
        }

        // Update roster
        {
            let mut roster = self.roster.write().await;
            roster.wire(a, b);
        }

        Ok(())
    }

    /// Get the comms runtime for a session, if available.
    async fn session_service_comms(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn CoreCommsRuntime>> {
        self.session_service.comms_runtime(session_id).await
    }

    /// Generate the comms name for a roster entry.
    fn comms_name_for(&self, entry: &RosterEntry) -> String {
        format!(
            "{}/{}/{}",
            self.definition.id, entry.profile, entry.meerkat_id
        )
    }

    /// Notify a peer that a new peer was added.
    ///
    /// Sends a `PeerRequest` with intent `mob.peer_added` FROM `sender_comms`
    /// TO the peer identified by `recipient_comms_name`. The params contain
    /// the new peer's identity and role.
    ///
    /// REQ-MOB-010/011: Notification is required for successful wiring.
    async fn notify_peer_added(
        &self,
        sender_comms: &Arc<dyn CoreCommsRuntime>,
        recipient_comms_name: &str,
        new_peer_id: &MeerkatId,
        new_peer_entry: &RosterEntry,
    ) -> Result<(), MobError> {
        let peer_description = self
            .definition
            .profiles
            .get(&new_peer_entry.profile)
            .map(|p| p.peer_description.as_str())
            .unwrap_or("");

        let peer_name = PeerName::new(recipient_comms_name).map_err(|error| {
            MobError::WiringError(format!(
                "notify_peer_added: invalid recipient comms name '{recipient_comms_name}': {error}"
            ))
        })?;

        let cmd = CommsCommand::PeerRequest {
            to: peer_name,
            intent: "mob.peer_added".to_string(),
            params: serde_json::json!({
                "peer": new_peer_id.as_str(),
                "role": new_peer_entry.profile.as_str(),
                "description": peer_description,
            }),
            stream: InputStreamMode::None,
        };

        sender_comms.send(cmd).await?;
        Ok(())
    }

    /// Notify a peer that another peer was retired.
    ///
    /// Sends a `PeerRequest` with intent `mob.peer_retired` FROM the retiring
    /// meerkat's comms TO the peer being notified. The params contain the
    /// retired peer's identity and role.
    ///
    /// REQ-MOB-012/013: Notification is required for successful unwire/retire.
    async fn notify_peer_retired(
        &self,
        peer_id: &MeerkatId,
        retired_id: &MeerkatId,
        retired_entry: &RosterEntry,
        retiring_comms: &Arc<dyn CoreCommsRuntime>,
    ) -> Result<(), MobError> {
        let peer_entry = {
            let roster = self.roster.read().await;
            roster.get(peer_id).cloned()
        };

        let peer_entry = peer_entry.ok_or_else(|| {
            MobError::WiringError(format!(
                "notify_peer_retired: peer '{peer_id}' missing from roster"
            ))
        })?;

        let peer_comms_name = self.comms_name_for(&peer_entry);
        let peer_name = PeerName::new(&peer_comms_name).map_err(|error| {
            MobError::WiringError(format!(
                "notify_peer_retired: invalid peer comms name '{peer_comms_name}': {error}"
            ))
        })?;

        let cmd = CommsCommand::PeerRequest {
            to: peer_name,
            intent: "mob.peer_retired".to_string(),
            params: serde_json::json!({
                "peer": retired_id.as_str(),
                "role": retired_entry.profile.as_str(),
            }),
            stream: InputStreamMode::None,
        };

        retiring_comms.send(cmd).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Mob tool dispatcher
// ---------------------------------------------------------------------------

fn compose_external_tools_for_profile(
    profile: &crate::profile::Profile,
    tool_bundles: &BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    mob_handle: Option<MobHandle>,
) -> Result<Option<Arc<dyn AgentToolDispatcher>>, MobError> {
    let mut dispatchers: Vec<Arc<dyn AgentToolDispatcher>> = Vec::new();

    if profile.tools.mob || profile.tools.mob_tasks {
        let handle = mob_handle.ok_or_else(|| {
            MobError::Internal(
                "mob/task tools require a runtime handle for dispatcher wiring".to_string(),
            )
        })?;
        dispatchers.push(Arc::new(MobToolDispatcher::new(
            handle,
            profile.tools.mob,
            profile.tools.mob_tasks,
        )));
    }

    for name in &profile.tools.rust_bundles {
        let dispatcher = tool_bundles.get(name).cloned().ok_or_else(|| {
            MobError::Internal(format!(
                "tool bundle '{name}' is not registered on this mob builder"
            ))
        })?;
        dispatchers.push(dispatcher);
    }

    if dispatchers.is_empty() {
        return Ok(None);
    }
    if dispatchers.len() == 1 {
        return Ok(dispatchers.pop());
    }

    let mut gateway_builder = ToolGatewayBuilder::new();
    for dispatcher in dispatchers {
        gateway_builder = gateway_builder.add_dispatcher(dispatcher);
    }
    let gateway = gateway_builder
        .build()
        .map_err(|e| MobError::Internal(format!("failed to compose tool bundles: {e}")))?;
    Ok(Some(Arc::new(gateway)))
}

struct MobToolDispatcher {
    handle: MobHandle,
    tools: Arc<[Arc<ToolDef>]>,
    enable_mob: bool,
    enable_mob_tasks: bool,
}

impl MobToolDispatcher {
    fn new(handle: MobHandle, enable_mob: bool, enable_mob_tasks: bool) -> Self {
        let mut defs: Vec<Arc<ToolDef>> = Vec::new();
        if enable_mob {
            defs.push(tool_def(
                "spawn_meerkat",
                "Spawn a meerkat from a profile",
                json!({
                    "type": "object",
                    "properties": {
                        "profile": {"type": "string"},
                        "meerkat_id": {"type": "string"},
                        "initial_message": {"type": "string"}
                    },
                    "required": ["profile", "meerkat_id"]
                }),
            ));
            defs.push(tool_def(
                "retire_meerkat",
                "Retire a meerkat and archive its session",
                json!({
                    "type": "object",
                    "properties": {"meerkat_id": {"type": "string"}},
                    "required": ["meerkat_id"]
                }),
            ));
            defs.push(tool_def(
                "wire_peers",
                "Wire two meerkats with bidirectional trust",
                json!({
                    "type": "object",
                    "properties": {"a": {"type": "string"}, "b": {"type": "string"}},
                    "required": ["a", "b"]
                }),
            ));
            defs.push(tool_def(
                "unwire_peers",
                "Unwire two meerkats and revoke bidirectional trust",
                json!({
                    "type": "object",
                    "properties": {"a": {"type": "string"}, "b": {"type": "string"}},
                    "required": ["a", "b"]
                }),
            ));
            defs.push(tool_def(
                "list_meerkats",
                "List all active meerkats",
                json!({
                    "type": "object",
                    "properties": {}
                }),
            ));
        }
        if enable_mob_tasks {
            defs.push(tool_def(
                "mob_task_create",
                "Create a shared mob task",
                json!({
                    "type": "object",
                    "properties": {
                        "task_id": {"type": "string"},
                        "subject": {"type": "string"},
                        "description": {"type": "string"},
                        "blocked_by": {"type": "array", "items": {"type": "string"}}
                    },
                    "required": ["task_id", "subject", "description"]
                }),
            ));
            defs.push(tool_def(
                "mob_task_list",
                "List shared mob tasks",
                json!({
                    "type": "object",
                    "properties": {}
                }),
            ));
            defs.push(tool_def(
                "mob_task_update",
                "Update task status and owner",
                json!({
                    "type": "object",
                    "properties": {
                        "task_id": {"type": "string"},
                        "status": {
                            "type": "string",
                            "enum": ["open", "in_progress", "completed", "cancelled"]
                        },
                        "owner": {"type": "string"}
                    },
                    "required": ["task_id", "status"]
                }),
            ));
            defs.push(tool_def(
                "mob_task_get",
                "Get a shared task by ID",
                json!({
                    "type": "object",
                    "properties": {"task_id": {"type": "string"}},
                    "required": ["task_id"]
                }),
            ));
        }

        Self {
            handle,
            tools: defs.into(),
            enable_mob,
            enable_mob_tasks,
        }
    }

    fn map_mob_error(call: ToolCallView<'_>, error: MobError) -> ToolError {
        ToolError::execution_failed(format!("tool '{}' failed: {error}", call.name))
    }

    fn encode_result(call: ToolCallView<'_>, value: serde_json::Value) -> Result<ToolResult, ToolError> {
        let content = serde_json::to_string(&value)
            .map_err(|error| ToolError::execution_failed(format!("encode tool result: {error}")))?;
        Ok(ToolResult {
            tool_use_id: call.id.to_string(),
            content,
            is_error: false,
        })
    }
}

fn tool_def(name: &str, description: &str, input_schema: serde_json::Value) -> Arc<ToolDef> {
    Arc::new(ToolDef {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
    })
}

#[derive(Deserialize)]
struct SpawnMeerkatArgs {
    profile: String,
    meerkat_id: String,
    #[serde(default)]
    initial_message: Option<String>,
}

#[derive(Deserialize)]
struct RetireMeerkatArgs {
    meerkat_id: String,
}

#[derive(Deserialize)]
struct WirePeersArgs {
    a: String,
    b: String,
}

#[derive(Deserialize)]
struct TaskCreateArgs {
    task_id: String,
    subject: String,
    description: String,
    #[serde(default)]
    blocked_by: Vec<String>,
}

#[derive(Deserialize)]
struct TaskUpdateArgs {
    task_id: String,
    status: TaskStatus,
    owner: Option<String>,
}

#[derive(Deserialize)]
struct TaskGetArgs {
    task_id: String,
}

#[async_trait::async_trait]
impl AgentToolDispatcher for MobToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        match call.name {
            "spawn_meerkat" if self.enable_mob => {
                let args: SpawnMeerkatArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                let session_id = self
                    .handle
                    .spawn(
                        ProfileName::from(args.profile),
                        MeerkatId::from(args.meerkat_id),
                        args.initial_message,
                    )
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({ "session_id": session_id }))
            }
            "retire_meerkat" if self.enable_mob => {
                let args: RetireMeerkatArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                self.handle
                    .retire(MeerkatId::from(args.meerkat_id))
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({"ok": true}))
            }
            "wire_peers" if self.enable_mob => {
                let args: WirePeersArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                self.handle
                    .wire(MeerkatId::from(args.a), MeerkatId::from(args.b))
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({"ok": true}))
            }
            "unwire_peers" if self.enable_mob => {
                let args: WirePeersArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                self.handle
                    .unwire(MeerkatId::from(args.a), MeerkatId::from(args.b))
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({"ok": true}))
            }
            "list_meerkats" if self.enable_mob => {
                let meerkats = self.handle.list_meerkats().await;
                Self::encode_result(call, json!({ "meerkats": meerkats }))
            }
            "mob_task_create" if self.enable_mob_tasks => {
                let args: TaskCreateArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                self.handle
                    .task_create(args.task_id, args.subject, args.description, args.blocked_by)
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({"ok": true}))
            }
            "mob_task_list" if self.enable_mob_tasks => {
                let tasks = self
                    .handle
                    .task_list()
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({ "tasks": tasks }))
            }
            "mob_task_update" if self.enable_mob_tasks => {
                let args: TaskUpdateArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                self.handle
                    .task_update(
                        args.task_id,
                        args.status,
                        args.owner.map(MeerkatId::from),
                    )
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({"ok": true}))
            }
            "mob_task_get" if self.enable_mob_tasks => {
                let args: TaskGetArgs = call
                    .parse_args()
                    .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
                let task = self
                    .handle
                    .task_get(&args.task_id)
                    .await
                    .map_err(|error| Self::map_mob_error(call, error))?;
                Self::encode_result(call, json!({ "task": task }))
            }
            _ => Err(ToolError::not_found(call.name)),
        }
    }
}

// ---------------------------------------------------------------------------
// MobSessionService trait extension
// ---------------------------------------------------------------------------

/// Extension trait for session services that support comms runtime access.
///
/// This bridges the gap between `SessionService` (which doesn't expose comms)
/// and the mob runtime's need to access comms runtimes for wiring.
#[async_trait::async_trait]
pub trait MobSessionService: SessionService {
    /// Get the comms runtime for a session.
    async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>>;

    /// Whether this service satisfies the persistent-session contract required
    /// by REQ-MOB-030.
    fn supports_persistent_sessions(&self) -> bool {
        false
    }

    /// Whether a listed session belongs to the given mob for reconciliation.
    async fn session_belongs_to_mob(&self, _session_id: &SessionId, _mob_id: &MobId) -> bool {
        false
    }
}

#[async_trait::async_trait]
impl<B> MobSessionService for meerkat_session::EphemeralSessionService<B>
where
    B: meerkat_session::SessionAgentBuilder + 'static,
{
    async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
        meerkat_session::EphemeralSessionService::<B>::comms_runtime(self, session_id).await
    }

    fn supports_persistent_sessions(&self) -> bool {
        false
    }

    async fn session_belongs_to_mob(&self, _session_id: &SessionId, _mob_id: &MobId) -> bool {
        false
    }
}

#[async_trait::async_trait]
impl<B> MobSessionService for meerkat_session::PersistentSessionService<B>
where
    B: meerkat_session::SessionAgentBuilder + 'static,
{
    async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
        meerkat_session::PersistentSessionService::<B>::comms_runtime(self, session_id).await
    }

    fn supports_persistent_sessions(&self) -> bool {
        true
    }

    async fn session_belongs_to_mob(&self, _session_id: &SessionId, _mob_id: &MobId) -> bool {
        // Persistent mob storage is expected to be per-mob isolated.
        true
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::event::MobEvent;
    use crate::definition::{MobDefinition, OrchestratorConfig, RoleWiringRule, WiringRules};
    use crate::profile::{Profile, ToolConfig};
    use crate::store::MobEventStore;
    use crate::storage::MobStorage;
    use async_trait::async_trait;
    use chrono::Utc;
    use meerkat_core::Session;
    use meerkat_core::event::AgentEvent;
    use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
    use meerkat_core::comms::{
        CommsCommand, PeerDirectoryEntry, PeerDirectorySource, PeerName, SendError, SendReceipt,
        TrustedPeerSpec,
    };
    use meerkat_core::interaction::InteractionId;
    use meerkat_core::error::ToolError;
    use meerkat_core::service::{
        CreateSessionRequest, SessionError, SessionInfo, SessionQuery, SessionService,
        SessionSummary, SessionUsage, SessionView, StartTurnRequest,
    };
    use meerkat_core::types::{RunResult, SessionId, ToolCallView, ToolResult, Usage};
    use meerkat_session::{SessionAgent, SessionAgentBuilder, SessionSnapshot};
    use meerkat_store::{MemoryStore, SessionStore};
    use serde_json::value::RawValue;
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::sync::atomic::AtomicU64;
    use std::sync::Mutex;
    use std::time::SystemTime;

    // -----------------------------------------------------------------------
    // Mock CommsRuntime
    // -----------------------------------------------------------------------

    #[derive(Clone, Copy, Debug, Default)]
    struct MockCommsBehavior {
        missing_public_key: bool,
        fail_add_trust: bool,
        fail_remove_trust: bool,
        fail_remove_trust_once: bool,
        fail_send_peer_added: bool,
        fail_send_peer_retired: bool,
    }

    struct MockCommsRuntime {
        public_key: std::sync::RwLock<Option<String>>,
        fail_add_trust: bool,
        fail_remove_trust: bool,
        remove_failures_remaining: RwLock<usize>,
        fail_send_peer_added: bool,
        fail_send_peer_retired: bool,
        trusted_peers: RwLock<HashMap<String, String>>,
        sent_intents: RwLock<Vec<String>>,
        inbox_notify: Arc<tokio::sync::Notify>,
    }

    impl MockCommsRuntime {
        fn new(name: &str, behavior: MockCommsBehavior) -> Self {
            Self {
                public_key: std::sync::RwLock::new(if behavior.missing_public_key {
                    None
                } else {
                    Some(format!("ed25519:{name}"))
                }),
                fail_add_trust: behavior.fail_add_trust,
                fail_remove_trust: behavior.fail_remove_trust,
                remove_failures_remaining: RwLock::new(if behavior.fail_remove_trust_once {
                    1
                } else {
                    0
                }),
                fail_send_peer_added: behavior.fail_send_peer_added,
                fail_send_peer_retired: behavior.fail_send_peer_retired,
                trusted_peers: RwLock::new(HashMap::new()),
                sent_intents: RwLock::new(Vec::new()),
                inbox_notify: Arc::new(tokio::sync::Notify::new()),
            }
        }

        fn clear_public_key(&self) {
            let mut key = self
                .public_key
                .write()
                .expect("poisoned public key lock in mock runtime");
            *key = None;
        }

        async fn trusted_peer_names(&self) -> Vec<String> {
            let mut names = self
                .trusted_peers
                .read()
                .await
                .values()
                .cloned()
                .collect::<Vec<_>>();
            names.sort();
            names
        }

        async fn sent_intents(&self) -> Vec<String> {
            self.sent_intents.read().await.clone()
        }
    }

    #[async_trait]
    impl CoreCommsRuntime for MockCommsRuntime {
        fn public_key(&self) -> Option<String> {
            self.public_key
                .read()
                .expect("poisoned public key lock in mock runtime")
                .clone()
        }

        async fn add_trusted_peer(&self, peer: TrustedPeerSpec) -> Result<(), SendError> {
            if self.fail_add_trust {
                return Err(SendError::Unsupported(
                    "mock add_trusted_peer failure".to_string(),
                ));
            }
            let mut peers = self.trusted_peers.write().await;
            peers.insert(peer.peer_id, peer.name);
            Ok(())
        }

        async fn remove_trusted_peer(&self, peer_id: &str) -> Result<bool, SendError> {
            if self.fail_remove_trust {
                return Err(SendError::Unsupported(
                    "mock remove_trusted_peer failure".to_string(),
                ));
            }
            {
                let mut remaining = self.remove_failures_remaining.write().await;
                if *remaining > 0 {
                    *remaining -= 1;
                    return Err(SendError::Unsupported(
                        "mock remove_trusted_peer failure (one-shot)".to_string(),
                    ));
                }
            }
            let mut peers = self.trusted_peers.write().await;
            Ok(peers.remove(peer_id).is_some())
        }

        async fn send(&self, cmd: CommsCommand) -> Result<SendReceipt, SendError> {
            match cmd {
                CommsCommand::PeerRequest { to, intent, .. } => {
                    if intent == "mob.peer_added" && self.fail_send_peer_added {
                        return Err(SendError::Unsupported(
                            "mock mob.peer_added notification failure".to_string(),
                        ));
                    }
                    if intent == "mob.peer_retired" && self.fail_send_peer_retired {
                        return Err(SendError::Unsupported(
                            "mock mob.peer_retired notification failure".to_string(),
                        ));
                    }

                    let trusted = self.trusted_peers.read().await;
                    if !trusted.values().any(|name| name == to.as_str()) {
                        return Err(SendError::PeerNotFound(to.as_string()));
                    }
                    drop(trusted);

                    self.sent_intents.write().await.push(intent);
                    Ok(SendReceipt::PeerRequestSent {
                        envelope_id: uuid::Uuid::new_v4(),
                        interaction_id: InteractionId(uuid::Uuid::new_v4()),
                        stream_reserved: false,
                    })
                }
                unsupported => Err(SendError::Unsupported(format!(
                    "mock send unsupported command kind '{}'",
                    unsupported.command_kind()
                ))),
            }
        }

        async fn peers(&self) -> Vec<PeerDirectoryEntry> {
            let trusted = self.trusted_peers.read().await;
            trusted
                .iter()
                .filter_map(|(peer_id, peer_name)| {
                    let name = PeerName::new(peer_name.clone()).ok()?;
                    Some(PeerDirectoryEntry {
                        name,
                        peer_id: peer_id.clone(),
                        address: format!("inproc://{peer_name}"),
                        source: PeerDirectorySource::Trusted,
                        sendable_kinds: vec![
                            "peer_message".to_string(),
                            "peer_request".to_string(),
                            "peer_response".to_string(),
                        ],
                        capabilities: serde_json::json!({}),
                        meta: meerkat_core::PeerMeta::default(),
                    })
                })
                .collect()
        }

        async fn drain_messages(&self) -> Vec<String> {
            Vec::new()
        }

        fn inbox_notify(&self) -> Arc<tokio::sync::Notify> {
            self.inbox_notify.clone()
        }
    }

    // -----------------------------------------------------------------------
    // Mock SessionService
    // -----------------------------------------------------------------------

    fn mock_run_result(session_id: SessionId, text: String) -> RunResult {
        RunResult {
            text,
            session_id,
            usage: Usage::default(),
            turns: 1,
            tool_calls: 0,
            structured_output: None,
            schema_warnings: None,
        }
    }

    #[derive(Clone, Debug)]
    struct CreateSessionRecord {
        host_mode: bool,
        comms_name: Option<String>,
        peer_meta_labels: BTreeMap<String, String>,
    }

    /// A mock session service that creates sessions with mock comms runtimes.
    struct MockSessionService {
        sessions: RwLock<HashMap<SessionId, Arc<MockCommsRuntime>>>,
        session_comms_names: RwLock<HashMap<SessionId, String>>,
        session_counter: AtomicU64,
        /// Records (session_id, prompt) for each create_session call.
        prompts: RwLock<Vec<(SessionId, String)>>,
        create_requests: RwLock<Vec<CreateSessionRecord>>,
        /// Records whether each create_session had external_tools configured.
        create_with_external_tools: RwLock<Vec<bool>>,
        /// External tool dispatcher captured per session.
        external_tools_by_session: RwLock<HashMap<SessionId, Arc<dyn AgentToolDispatcher>>>,
        /// Optional per-comms-name behavior overrides.
        comms_behaviors: RwLock<HashMap<String, MockCommsBehavior>>,
        /// Sessions whose comms runtime should appear unavailable.
        missing_comms_sessions: RwLock<HashSet<SessionId>>,
        /// Sessions whose archive() call should fail.
        archive_fail_sessions: RwLock<HashSet<SessionId>>,
        /// Comms names whose created sessions should fail archive().
        archive_fail_comms_names: RwLock<HashSet<String>>,
        /// Sent intents for sessions that were archived and removed.
        archived_sent_intents: RwLock<HashMap<SessionId, Vec<String>>>,
    }

    impl MockSessionService {
        fn new() -> Self {
            Self {
                sessions: RwLock::new(HashMap::new()),
                session_comms_names: RwLock::new(HashMap::new()),
                session_counter: AtomicU64::new(0),
                prompts: RwLock::new(Vec::new()),
                create_requests: RwLock::new(Vec::new()),
                create_with_external_tools: RwLock::new(Vec::new()),
                external_tools_by_session: RwLock::new(HashMap::new()),
                comms_behaviors: RwLock::new(HashMap::new()),
                missing_comms_sessions: RwLock::new(HashSet::new()),
                archive_fail_sessions: RwLock::new(HashSet::new()),
                archive_fail_comms_names: RwLock::new(HashSet::new()),
                archived_sent_intents: RwLock::new(HashMap::new()),
            }
        }

        async fn active_session_count(&self) -> usize {
            self.sessions.read().await.len()
        }

        /// Get recorded prompts for inspection in tests.
        async fn recorded_prompts(&self) -> Vec<(SessionId, String)> {
            self.prompts.read().await.clone()
        }

        async fn recorded_create_requests(&self) -> Vec<CreateSessionRecord> {
            self.create_requests.read().await.clone()
        }

        async fn recorded_external_tools_flags(&self) -> Vec<bool> {
            self.create_with_external_tools.read().await.clone()
        }

        async fn external_tool_names(&self, session_id: &SessionId) -> Vec<String> {
            let tools = self.external_tools_by_session.read().await;
            tools
                .get(session_id)
                .map(|dispatcher| {
                    dispatcher
                        .tools()
                        .iter()
                        .map(|tool| tool.name.clone())
                        .collect()
                })
                .unwrap_or_default()
        }

        async fn dispatch_external_tool(
            &self,
            session_id: &SessionId,
            tool_name: &str,
            args: serde_json::Value,
        ) -> Result<ToolResult, ToolError> {
            let dispatcher = {
                let tools = self.external_tools_by_session.read().await;
                tools.get(session_id).cloned()
            }
            .ok_or_else(|| ToolError::not_found(tool_name))?;
            let raw = RawValue::from_string(args.to_string())
                .map_err(|error| ToolError::execution_failed(format!("raw args: {error}")))?;
            dispatcher
                .dispatch(ToolCallView {
                    id: "tool-1",
                    name: tool_name,
                    args: &raw,
                })
                .await
        }

        async fn set_comms_behavior(&self, comms_name: &str, behavior: MockCommsBehavior) {
            self.comms_behaviors
                .write()
                .await
                .insert(comms_name.to_string(), behavior);
        }

        async fn set_missing_comms_runtime(&self, session_id: &SessionId) {
            self.missing_comms_sessions
                .write()
                .await
                .insert(session_id.clone());
        }

        async fn set_archive_failure(&self, session_id: &SessionId) {
            self.archive_fail_sessions
                .write()
                .await
                .insert(session_id.clone());
        }

        async fn set_archive_failure_for_comms_name(&self, comms_name: &str) {
            self.archive_fail_comms_names
                .write()
                .await
                .insert(comms_name.to_string());
        }

        async fn clear_public_key(&self, session_id: &SessionId) {
            let runtime = {
                let sessions = self.sessions.read().await;
                sessions.get(session_id).cloned()
            };
            if let Some(runtime) = runtime {
                runtime.clear_public_key();
            }
        }

        async fn trusted_peer_names(&self, session_id: &SessionId) -> Vec<String> {
            let runtime = {
                let sessions = self.sessions.read().await;
                sessions.get(session_id).cloned()
            };
            match runtime {
                Some(runtime) => runtime.trusted_peer_names().await,
                None => Vec::new(),
            }
        }

        async fn sent_intents(&self, session_id: &SessionId) -> Vec<String> {
            let runtime = {
                let sessions = self.sessions.read().await;
                sessions.get(session_id).cloned()
            };
            match runtime {
                Some(runtime) => runtime.sent_intents().await,
                None => self
                    .archived_sent_intents
                    .read()
                    .await
                    .get(session_id)
                    .cloned()
                    .unwrap_or_default(),
            }
        }

        async fn force_remove_trust(&self, from_session: &SessionId, to_session: &SessionId) {
            let (from_runtime, to_runtime) = {
                let sessions = self.sessions.read().await;
                (
                    sessions.get(from_session).cloned(),
                    sessions.get(to_session).cloned(),
                )
            };
            if let (Some(from_runtime), Some(to_runtime)) = (from_runtime, to_runtime)
                && let Some(to_key) = to_runtime.public_key()
            {
                let _ = from_runtime.remove_trusted_peer(&to_key).await;
            }
        }
    }

    #[async_trait]
    impl SessionService for MockSessionService {
        async fn create_session(
            &self,
            req: CreateSessionRequest,
        ) -> Result<RunResult, SessionError> {
            let session_id = SessionId::new();
            let n = self.session_counter.fetch_add(1, Ordering::Relaxed);

            // Extract comms_name from build options for mock runtime naming
            let comms_name = req
                .build
                .as_ref()
                .and_then(|b| b.comms_name.clone())
                .unwrap_or_else(|| format!("mock-session-{n}"));

            let behavior = self
                .comms_behaviors
                .read()
                .await
                .get(&comms_name)
                .copied()
                .unwrap_or_default();
            let comms = Arc::new(MockCommsRuntime::new(&comms_name, behavior));
            self.sessions
                .write()
                .await
                .insert(session_id.clone(), comms);
            self.session_comms_names
                .write()
                .await
                .insert(session_id.clone(), comms_name.clone());
            if self
                .archive_fail_comms_names
                .read()
                .await
                .contains(&comms_name)
            {
                self.archive_fail_sessions
                    .write()
                    .await
                    .insert(session_id.clone());
            }

            let peer_meta_labels = req
                .build
                .as_ref()
                .and_then(|build| build.peer_meta.as_ref())
                .map(|meta| meta.labels.clone())
                .unwrap_or_default();
            self.create_requests.write().await.push(CreateSessionRecord {
                host_mode: req.host_mode,
                comms_name: req.build.as_ref().and_then(|build| build.comms_name.clone()),
                peer_meta_labels,
            });

            // Record the prompt for test inspection
            self.prompts
                .write()
                .await
                .push((session_id.clone(), req.prompt));
            let external_tools = req
                .build
                .as_ref()
                .and_then(|b| b.external_tools.as_ref())
                .cloned();
            self.create_with_external_tools
                .write()
                .await
                .push(external_tools.is_some());
            if let Some(dispatcher) = external_tools {
                self.external_tools_by_session
                    .write()
                    .await
                    .insert(session_id.clone(), dispatcher);
            }

            Ok(mock_run_result(session_id, "Session created".to_string()))
        }

        async fn start_turn(
            &self,
            id: &SessionId,
            _req: StartTurnRequest,
        ) -> Result<RunResult, SessionError> {
            let sessions = self.sessions.read().await;
            if !sessions.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            Ok(mock_run_result(id.clone(), "Turn completed".to_string()))
        }

        async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
            let sessions = self.sessions.read().await;
            if !sessions.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            Ok(())
        }

        async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
            let sessions = self.sessions.read().await;
            if !sessions.contains_key(id) {
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
            let sessions = self.sessions.read().await;
            Ok(sessions
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
            if self.archive_fail_sessions.read().await.contains(id) {
                return Err(SessionError::Store(Box::new(std::io::Error::other(
                    "mock archive failure",
                ))));
            }
            let mut sessions = self.sessions.write().await;
            if let Some(runtime) = sessions.remove(id) {
                self.session_comms_names.write().await.remove(id);
                self.external_tools_by_session.write().await.remove(id);
                let intents = runtime.sent_intents().await;
                self.archived_sent_intents
                    .write()
                    .await
                    .insert(id.clone(), intents);
            }
            Ok(())
        }
    }

    #[async_trait]
    impl MobSessionService for MockSessionService {
        async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
            if self
                .missing_comms_sessions
                .read()
                .await
                .contains(session_id)
            {
                return None;
            }
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .map(|comms| Arc::clone(comms) as Arc<dyn CoreCommsRuntime>)
        }

        fn supports_persistent_sessions(&self) -> bool {
            true
        }

        async fn session_belongs_to_mob(&self, session_id: &SessionId, mob_id: &MobId) -> bool {
            let names = self.session_comms_names.read().await;
            names
                .get(session_id)
                .map(|name| name.starts_with(&format!("{mob_id}/")))
                .unwrap_or(false)
        }
    }

    struct FaultInjectedMobEventStore {
        events: RwLock<Vec<MobEvent>>,
        fail_on_kind: RwLock<HashSet<&'static str>>,
    }

    impl FaultInjectedMobEventStore {
        fn new() -> Self {
            Self {
                events: RwLock::new(Vec::new()),
                fail_on_kind: RwLock::new(HashSet::new()),
            }
        }

        async fn fail_appends_for(&self, kind: &'static str) {
            self.fail_on_kind.write().await.insert(kind);
        }

        fn kind_label(kind: &MobEventKind) -> &'static str {
            match kind {
                MobEventKind::MobCreated { .. } => "MobCreated",
                MobEventKind::MobCompleted => "MobCompleted",
                MobEventKind::MeerkatSpawned { .. } => "MeerkatSpawned",
                MobEventKind::MeerkatRetired { .. } => "MeerkatRetired",
                MobEventKind::PeersWired { .. } => "PeersWired",
                MobEventKind::PeersUnwired { .. } => "PeersUnwired",
                MobEventKind::TaskCreated { .. } => "TaskCreated",
                MobEventKind::TaskUpdated { .. } => "TaskUpdated",
            }
        }
    }

    #[async_trait]
    impl MobEventStore for FaultInjectedMobEventStore {
        async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobError> {
            let kind_label = Self::kind_label(&event.kind);
            if self.fail_on_kind.read().await.contains(kind_label) {
                return Err(MobError::Internal(format!(
                    "fault-injected append failure for {kind_label}"
                )));
            }

            let mut events = self.events.write().await;
            let cursor = events.len() as u64 + 1;
            let stored = MobEvent {
                cursor,
                timestamp: Utc::now(),
                mob_id: event.mob_id,
                kind: event.kind,
            };
            events.push(stored.clone());
            Ok(stored)
        }

        async fn poll(&self, after_cursor: u64, limit: usize) -> Result<Vec<MobEvent>, MobError> {
            let events = self.events.read().await;
            Ok(events
                .iter()
                .filter(|e| e.cursor > after_cursor)
                .take(limit)
                .cloned()
                .collect())
        }

        async fn replay_all(&self) -> Result<Vec<MobEvent>, MobError> {
            Ok(self.events.read().await.clone())
        }

        async fn clear(&self) -> Result<(), MobError> {
            self.events.write().await.clear();
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn sample_definition() -> MobDefinition {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("lead"),
            Profile {
                model: "claude-opus-4-6".into(),
                skills: vec![],
                tools: ToolConfig {
                    builtins: true,
                    shell: false,
                    comms: true,
                    memory: false,
                    mob: true,
                    mob_tasks: false,
                    mcp: vec![],
                    rust_bundles: vec![],
                },
                peer_description: "The lead".into(),
                external_addressable: true,
            },
        );
        profiles.insert(
            ProfileName::from("worker"),
            Profile {
                model: "claude-sonnet-4-5".into(),
                skills: vec![],
                tools: ToolConfig {
                    comms: true,
                    ..ToolConfig::default()
                },
                peer_description: "A worker".into(),
                external_addressable: false,
            },
        );

        MobDefinition {
            id: MobId::from("test-mob"),
            orchestrator: Some(OrchestratorConfig {
                profile: ProfileName::from("lead"),
            }),
            profiles,
            mcp_servers: BTreeMap::new(),
            wiring: WiringRules::default(),
            skills: BTreeMap::new(),
        }
    }

    fn sample_definition_with_auto_wire() -> MobDefinition {
        let mut def = sample_definition();
        def.wiring.auto_wire_orchestrator = true;
        def
    }

    fn sample_definition_with_role_wiring() -> MobDefinition {
        let mut def = sample_definition();
        def.wiring.role_wiring = vec![RoleWiringRule {
            a: ProfileName::from("worker"),
            b: ProfileName::from("worker"),
        }];
        def
    }

    fn sample_definition_with_cross_role_wiring() -> MobDefinition {
        let mut def = sample_definition();
        def.wiring.role_wiring = vec![RoleWiringRule {
            a: ProfileName::from("lead"),
            b: ProfileName::from("worker"),
        }];
        def
    }

    fn sample_definition_with_tool_bundle(bundle_name: &str) -> MobDefinition {
        let mut def = sample_definition();
        let worker = def
            .profiles
            .get_mut(&ProfileName::from("worker"))
            .expect("worker profile exists");
        worker.tools.rust_bundles = vec![bundle_name.to_string()];
        def
    }

    fn sample_definition_with_mob_tools() -> MobDefinition {
        let mut def = sample_definition();
        let worker = def
            .profiles
            .get_mut(&ProfileName::from("worker"))
            .expect("worker profile exists");
        worker.tools.mob = true;
        worker.tools.mob_tasks = true;
        worker.tools.comms = true;
        def
    }

    fn sample_definition_without_mob_flags() -> MobDefinition {
        let mut def = sample_definition();
        let lead = def
            .profiles
            .get_mut(&ProfileName::from("lead"))
            .expect("lead profile exists");
        lead.tools.mob = false;
        lead.tools.mob_tasks = false;
        def
    }

    fn sample_definition_with_mcp_servers() -> MobDefinition {
        let mut def = sample_definition();
        def.mcp_servers.insert(
            "search".to_string(),
            crate::definition::McpServerConfig {
                command: Some(vec!["mcp-search".to_string()]),
                url: None,
                env: BTreeMap::new(),
            },
        );
        def
    }

    /// Create a MobHandle using the mock session service.
    async fn create_test_mob(definition: MobDefinition) -> (MobHandle, Arc<MockSessionService>) {
        let service = Arc::new(MockSessionService::new());
        let storage = MobStorage::in_memory();
        let handle = MobBuilder::new(definition, storage)
            .with_session_service(service.clone())
            .create()
            .await
            .expect("create mob");

        (handle, service)
    }

    async fn create_test_mob_with_events(
        definition: MobDefinition,
        events: Arc<dyn MobEventStore>,
    ) -> (MobHandle, Arc<MockSessionService>) {
        let service = Arc::new(MockSessionService::new());
        let handle = MobBuilder::new(definition, MobStorage { events })
            .with_session_service(service.clone())
            .create()
            .await
            .expect("create mob");

        (handle, service)
    }

    fn test_comms_name(profile: &str, meerkat_id: &str) -> String {
        format!("test-mob/{profile}/{meerkat_id}")
    }

    struct EchoBundleDispatcher;

    #[async_trait]
    impl AgentToolDispatcher for EchoBundleDispatcher {
        fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
            vec![Arc::new(meerkat_core::ToolDef {
                name: "bundle_echo".to_string(),
                description: "Echo input value".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": { "value": { "type": "string" } },
                    "required": ["value"]
                }),
            })]
            .into()
        }

        async fn dispatch(
            &self,
            call: meerkat_core::ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolResult, meerkat_core::ToolError> {
            if call.name != "bundle_echo" {
                return Err(meerkat_core::ToolError::not_found(call.name));
            }
            let args: serde_json::Value = serde_json::from_str(call.args.get())
                .map_err(|e| meerkat_core::ToolError::invalid_arguments(call.name, e.to_string()))?;
            let echoed = args
                .get("value")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            Ok(meerkat_core::ToolResult {
                tool_use_id: call.id.to_string(),
                content: serde_json::json!({ "echo": echoed }).to_string(),
                is_error: false,
            })
        }
    }

    struct PersistentMockAgent {
        session_id: SessionId,
    }

    #[async_trait]
    impl SessionAgent for PersistentMockAgent {
        async fn run_with_events(
            &mut self,
            _prompt: String,
            _event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
        ) -> Result<RunResult, meerkat_core::error::AgentError> {
            Ok(mock_run_result(
                self.session_id.clone(),
                "Persistent mock turn".to_string(),
            ))
        }

        async fn run_host_mode(
            &mut self,
            prompt: String,
        ) -> Result<RunResult, meerkat_core::error::AgentError> {
            let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
            self.run_with_events(prompt, event_tx).await
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillId>>) {}

        fn cancel(&mut self) {}

        fn session_id(&self) -> SessionId {
            self.session_id.clone()
        }

        fn snapshot(&self) -> SessionSnapshot {
            SessionSnapshot {
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                usage: Usage::default(),
                last_assistant_text: None,
            }
        }

        fn session_clone(&self) -> Session {
            Session::with_id(self.session_id.clone())
        }
    }

    struct PersistentMockBuilder;

    #[async_trait]
    impl SessionAgentBuilder for PersistentMockBuilder {
        type Agent = PersistentMockAgent;

        async fn build_agent(
            &self,
            _req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            Ok(PersistentMockAgent {
                session_id: SessionId::new(),
            })
        }
    }

    async fn create_test_mob_with_persistent_service(definition: MobDefinition) -> MobHandle {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let service = Arc::new(meerkat_session::PersistentSessionService::new(
            PersistentMockBuilder,
            16,
            store,
        ));
        MobBuilder::new(definition, MobStorage::in_memory())
            .with_session_service(service)
            .create()
            .await
            .expect("create mob with persistent session service")
    }

    // -----------------------------------------------------------------------
    // P1-T03: MobBuilder + MobHandle + MobActor wiring
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_mob_create_returns_handle() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        assert_eq!(handle.status(), MobState::Running);
        assert_eq!(handle.mob_id().as_str(), "test-mob");
    }

    #[tokio::test]
    async fn test_mob_builder_accepts_persistent_session_service() {
        let handle = create_test_mob_with_persistent_service(sample_definition()).await;
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn with persistent session service");
        assert!(
            handle.get_meerkat(&MeerkatId::from("w-1")).await.is_some(),
            "spawn should succeed when mob is built with PersistentSessionService"
        );
    }

    #[tokio::test]
    async fn test_mob_builder_rejects_non_persistent_session_service() {
        let service = Arc::new(meerkat_session::EphemeralSessionService::new(
            PersistentMockBuilder,
            16,
        ));
        let result = MobBuilder::new(sample_definition(), MobStorage::in_memory())
            .with_session_service(service)
            .create()
            .await;
        assert!(
            matches!(result, Err(MobError::Internal(message)) if message.contains("persistent-session contract")),
            "create should reject non-persistent session services for REQ-MOB-030"
        );
    }

    #[tokio::test]
    async fn test_mob_handle_is_clone() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        let handle2 = handle.clone();
        assert_eq!(handle2.mob_id().as_str(), "test-mob");
    }

    #[tokio::test]
    async fn test_mob_shutdown() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        handle.shutdown().await.expect("shutdown");
        assert_eq!(handle.status(), MobState::Stopped);
    }

    #[tokio::test]
    async fn test_lifecycle_state_machine_enforcement() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        assert_eq!(handle.status(), MobState::Running);

        handle.stop().await.expect("stop");
        assert_eq!(handle.status(), MobState::Stopped);

        handle.resume().await.expect("resume");
        assert_eq!(handle.status(), MobState::Running);

        handle.complete().await.expect("complete");
        assert_eq!(handle.status(), MobState::Completed);

        let err = handle.resume().await.expect_err("resume must fail from completed");
        assert!(
            matches!(err, MobError::InvalidTransition { .. }),
            "completed -> running must be rejected"
        );

        handle.destroy().await.expect("destroy");
        assert_eq!(handle.status(), MobState::Destroyed);

        let err = handle
            .destroy()
            .await
            .expect_err("destroy from Destroyed must fail");
        assert!(matches!(err, MobError::InvalidTransition { .. }));
    }

    #[tokio::test]
    async fn test_lifecycle_updates_mcp_server_states() {
        let (handle, _service) = create_test_mob(sample_definition_with_mcp_servers()).await;
        let initial = handle.mcp_server_states().await;
        assert!(
            initial.values().all(|running| *running),
            "mcp servers should start in running state"
        );

        handle.stop().await.expect("stop");
        let stopped = handle.mcp_server_states().await;
        assert!(
            stopped.values().all(|running| !*running),
            "stop should mark all mcp servers as stopped"
        );

        handle.resume().await.expect("resume");
        let resumed = handle.mcp_server_states().await;
        assert!(
            resumed.values().all(|running| *running),
            "resume should mark all mcp servers as running"
        );

        handle.complete().await.expect("complete");
        let completed = handle.mcp_server_states().await;
        assert!(
            completed.values().all(|running| !*running),
            "complete should stop all mcp servers"
        );
    }

    #[tokio::test]
    async fn test_stop_persists_all_state_and_rejects_mutations() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn");
        let event_count_before = handle.events().replay_all().await.expect("replay").len();

        handle.stop().await.expect("stop");
        assert_eq!(handle.status(), MobState::Stopped);
        assert_eq!(
            service.active_session_count().await,
            1,
            "stop should not archive active sessions"
        );
        assert_eq!(
            handle.events().replay_all().await.expect("replay").len(),
            event_count_before,
            "stop should not delete persisted events"
        );

        let err = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
            .await
            .expect_err("spawn must be rejected while stopped");
        assert!(matches!(err, MobError::InvalidTransition { .. }));
    }

    #[tokio::test]
    async fn test_register_tool_bundle_is_wired_into_spawn() {
        let definition = sample_definition_with_tool_bundle("bundle-a");
        let service = Arc::new(MockSessionService::new());
        let storage = MobStorage::in_memory();
        let handle = MobBuilder::new(definition, storage)
            .with_session_service(service.clone())
            .register_tool_bundle("bundle-a", Arc::new(EchoBundleDispatcher))
            .create()
            .await
            .expect("create mob");

        let session_id = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn");

        let flags = service.recorded_external_tools_flags().await;
        assert_eq!(
            flags,
            vec![true],
            "spawn should pass registered rust bundle as external_tools"
        );

        let tool_names = service.external_tool_names(&session_id).await;
        assert!(
            tool_names.contains(&"bundle_echo".to_string()),
            "registered rust bundle tool should be exposed"
        );
        let result = service
            .dispatch_external_tool(
                &session_id,
                "bundle_echo",
                serde_json::json!({"value": "hello"}),
            )
            .await
            .expect("bundle tool dispatch");
        let payload: serde_json::Value =
            serde_json::from_str(&result.content).expect("bundle payload");
        assert_eq!(payload["echo"], "hello");
    }

    #[tokio::test]
    async fn test_spawn_fails_when_tool_bundle_not_registered() {
        let definition = sample_definition_with_tool_bundle("missing-bundle");
        let service = Arc::new(MockSessionService::new());
        let storage = MobStorage::in_memory();
        let handle = MobBuilder::new(definition, storage)
            .with_session_service(service.clone())
            .create()
            .await
            .expect("create mob");

        let result = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await;
        assert!(matches!(result, Err(MobError::Internal(_))));

        assert!(handle.list_meerkats().await.is_empty());
        let events = handle.events().replay_all().await.expect("replay");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::MeerkatSpawned { .. })),
            "failed spawn should not emit MeerkatSpawned"
        );
    }

    #[tokio::test]
    async fn test_mob_management_tools_dispatch_to_handle() {
        let (handle, service) = create_test_mob(sample_definition_with_mob_tools()).await;
        let sid_1 = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn w-1");

        let tool_names = service.external_tool_names(&sid_1).await;
        for required in [
            "spawn_meerkat",
            "retire_meerkat",
            "wire_peers",
            "unwire_peers",
            "list_meerkats",
        ] {
            assert!(
                tool_names.contains(&required.to_string()),
                "expected tool '{required}' to be present"
            );
        }

        service
            .dispatch_external_tool(
                &sid_1,
                "spawn_meerkat",
                serde_json::json!({"profile": "worker", "meerkat_id": "w-2"}),
            )
            .await
            .expect("spawn_meerkat tool");
        assert!(
            handle.get_meerkat(&MeerkatId::from("w-2")).await.is_some(),
            "spawn_meerkat should create a new roster entry"
        );

        service
            .dispatch_external_tool(
                &sid_1,
                "wire_peers",
                serde_json::json!({"a": "w-1", "b": "w-2"}),
            )
            .await
            .expect("wire peers");
        let w1 = handle
            .get_meerkat(&MeerkatId::from("w-1"))
            .await
            .expect("w-1");
        assert!(
            w1.wired_to.contains(&MeerkatId::from("w-2")),
            "wire_peers should update roster wiring"
        );

        let listed = service
            .dispatch_external_tool(&sid_1, "list_meerkats", serde_json::json!({}))
            .await
            .expect("list_meerkats");
        let listed_json: serde_json::Value =
            serde_json::from_str(&listed.content).expect("list content json");
        assert!(
            listed_json["meerkats"].as_array().map(|v| v.len()).unwrap_or(0) >= 2,
            "list_meerkats should include spawned peers"
        );

        service
            .dispatch_external_tool(
                &sid_1,
                "unwire_peers",
                serde_json::json!({"a": "w-1", "b": "w-2"}),
            )
            .await
            .expect("unwire peers");
        service
            .dispatch_external_tool(
                &sid_1,
                "retire_meerkat",
                serde_json::json!({"meerkat_id": "w-2"}),
            )
            .await
            .expect("retire_meerkat");
        assert!(
            handle.get_meerkat(&MeerkatId::from("w-2")).await.is_none(),
            "retire_meerkat should remove roster entry"
        );
    }

    #[tokio::test]
    async fn test_mob_task_tools_dispatch_and_blocked_by_enforcement() {
        let (handle, service) = create_test_mob(sample_definition_with_mob_tools()).await;
        let sid_1 = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn w-1");

        for required in [
            "mob_task_create",
            "mob_task_list",
            "mob_task_update",
            "mob_task_get",
        ] {
            assert!(
                service.external_tool_names(&sid_1).await.contains(&required.to_string()),
                "expected task tool '{required}'"
            );
        }

        service
            .dispatch_external_tool(
                &sid_1,
                "mob_task_create",
                serde_json::json!({
                    "task_id": "t1",
                    "subject": "Foundations",
                    "description": "Prepare prerequisites"
                }),
            )
            .await
            .expect("create t1");
        service
            .dispatch_external_tool(
                &sid_1,
                "mob_task_create",
                serde_json::json!({
                    "task_id": "t2",
                    "subject": "Main work",
                    "description": "Depends on t1",
                    "blocked_by": ["t1"]
                }),
            )
            .await
            .expect("create t2");

        let blocked = service
            .dispatch_external_tool(
                &sid_1,
                "mob_task_update",
                serde_json::json!({"task_id": "t2", "status": "in_progress", "owner": "w-1"}),
            )
            .await
            .expect_err("blocked task should not be claimable");
        assert!(
            blocked.to_string().contains("blocked"),
            "blocked-by enforcement should reject task claim"
        );

        let invalid_claim = service
            .dispatch_external_tool(
                &sid_1,
                "mob_task_update",
                serde_json::json!({"task_id": "t2", "status": "completed", "owner": "w-1"}),
            )
            .await
            .expect_err("owner set with non-in_progress status must fail");
        assert!(
            invalid_claim.to_string().contains("in_progress"),
            "claim semantics should require in_progress when owner is set"
        );

        service
            .dispatch_external_tool(
                &sid_1,
                "mob_task_update",
                serde_json::json!({"task_id": "t1", "status": "completed"}),
            )
            .await
            .expect("complete dependency");
        service
            .dispatch_external_tool(
                &sid_1,
                "mob_task_update",
                serde_json::json!({"task_id": "t2", "status": "in_progress", "owner": "w-1"}),
            )
            .await
            .expect("claim unblocked task");

        let task = service
            .dispatch_external_tool(
                &sid_1,
                "mob_task_get",
                serde_json::json!({"task_id": "t2"}),
            )
            .await
            .expect("task get");
        let task_json: serde_json::Value =
            serde_json::from_str(&task.content).expect("task get content json");
        assert_eq!(task_json["task"]["status"], "in_progress");
        assert_eq!(task_json["task"]["owner"], "w-1");

        let listed = service
            .dispatch_external_tool(&sid_1, "mob_task_list", serde_json::json!({}))
            .await
            .expect("task list");
        let list_json: serde_json::Value =
            serde_json::from_str(&listed.content).expect("task list content json");
        assert_eq!(
            list_json["tasks"].as_array().map(|v| v.len()).unwrap_or(0),
            2,
            "mob_task_list should project created tasks"
        );
    }

    #[tokio::test]
    async fn test_tool_flag_enforcement_blocks_mob_and_task_tools() {
        let (handle, service) = create_test_mob(sample_definition_without_mob_flags()).await;
        let sid = handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("lead-1"), None)
            .await
            .expect("spawn lead");

        let names = service.external_tool_names(&sid).await;
        assert!(
            !names.iter().any(|name| name == "spawn_meerkat"),
            "tools.mob=false should hide spawn_meerkat"
        );
        assert!(
            !names.iter().any(|name| name == "mob_task_create"),
            "tools.mob_tasks=false should hide mob_task_create"
        );

        let spawn_err = service
            .dispatch_external_tool(
                &sid,
                "spawn_meerkat",
                serde_json::json!({"profile": "worker", "meerkat_id": "w-2"}),
            )
            .await
            .expect_err("spawn tool must be unavailable");
        assert!(matches!(spawn_err, ToolError::NotFound { .. }));

        let task_err = service
            .dispatch_external_tool(
                &sid,
                "mob_task_create",
                serde_json::json!({"task_id": "t1", "subject": "x", "description": "y"}),
            )
            .await
            .expect_err("task tool must be unavailable");
        assert!(matches!(task_err, ToolError::NotFound { .. }));
    }

    #[tokio::test]
    async fn test_for_resume_rebuilds_definition_and_roster() {
        let service = Arc::new(MockSessionService::new());
        let storage = MobStorage::in_memory();
        let events = storage.events.clone();

        let handle = MobBuilder::new(sample_definition(), storage)
            .with_session_service(service.clone())
            .create()
            .await
            .expect("create mob");
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn w-1");
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
            .await
            .expect("spawn w-2");
        handle
            .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
            .await
            .expect("wire");

        let resumed = MobBuilder::for_resume(MobStorage {
            events: events.clone(),
        })
        .with_session_service(service.clone())
        .resume()
        .await
        .expect("resume");

        assert_eq!(resumed.mob_id().as_str(), "test-mob");
        let entry_1 = resumed.get_meerkat(&MeerkatId::from("w-1")).await.unwrap();
        let entry_2 = resumed.get_meerkat(&MeerkatId::from("w-2")).await.unwrap();
        assert!(entry_1.wired_to.contains(&MeerkatId::from("w-2")));
        assert!(entry_2.wired_to.contains(&MeerkatId::from("w-1")));
    }

    #[tokio::test]
    async fn test_resume_reconciles_orphaned_sessions() {
        let service = Arc::new(MockSessionService::new());
        let storage = MobStorage::in_memory();
        let events = storage.events.clone();
        let _handle = MobBuilder::new(sample_definition(), storage)
            .with_session_service(service.clone())
            .create()
            .await
            .expect("create mob");

        let orphan = service
            .create_session(CreateSessionRequest {
                model: "claude-sonnet-4-5".to_string(),
                prompt: "orphan".to_string(),
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                build: Some(meerkat_core::service::SessionBuildOptions {
                    comms_name: Some("test-mob/worker/orphan".to_string()),
                    ..Default::default()
                }),
                host_mode: true,
                skill_references: None,
            })
            .await
            .expect("create orphan");
        assert_eq!(service.active_session_count().await, 1);

        let _resumed = MobBuilder::for_resume(MobStorage { events })
            .with_session_service(service.clone())
            .resume()
            .await
            .expect("resume");

        let sessions = service
            .list(SessionQuery::default())
            .await
            .expect("list sessions");
        assert!(
            !sessions
                .iter()
                .any(|s| s.session_id == orphan.session_id),
            "resume should archive orphaned sessions"
        );
    }

    #[tokio::test]
    async fn test_resume_recreates_missing_sessions() {
        let service = Arc::new(MockSessionService::new());
        let storage = MobStorage::in_memory();
        let events = storage.events.clone();
        let handle = MobBuilder::new(sample_definition(), storage)
            .with_session_service(service.clone())
            .create()
            .await
            .expect("create mob");
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn");
        handle.stop().await.expect("stop");

        let old_sid = handle
            .get_meerkat(&MeerkatId::from("w-1"))
            .await
            .expect("roster entry")
            .session_id;
        service.archive(&old_sid).await.expect("archive missing session");

        let resumed = MobBuilder::for_resume(MobStorage { events })
            .with_session_service(service.clone())
            .resume()
            .await
            .expect("resume");

        let new_sid = resumed
            .get_meerkat(&MeerkatId::from("w-1"))
            .await
            .expect("recreated entry")
            .session_id;
        assert_ne!(new_sid, old_sid, "resume should recreate missing session");
        assert_eq!(service.active_session_count().await, 1);
    }

    #[tokio::test]
    async fn test_resume_recreates_missing_sessions_with_tool_wiring() {
        let mut definition = sample_definition_with_mob_tools();
        let worker = definition
            .profiles
            .get_mut(&ProfileName::from("worker"))
            .expect("worker profile");
        worker.tools.rust_bundles = vec!["bundle-a".to_string()];

        let service = Arc::new(MockSessionService::new());
        let storage = MobStorage::in_memory();
        let events = storage.events.clone();
        let handle = MobBuilder::new(definition.clone(), storage)
            .with_session_service(service.clone())
            .register_tool_bundle("bundle-a", Arc::new(EchoBundleDispatcher))
            .create()
            .await
            .expect("create mob");
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn");
        handle.stop().await.expect("stop");

        let old_sid = handle
            .get_meerkat(&MeerkatId::from("w-1"))
            .await
            .expect("roster entry")
            .session_id;
        service.archive(&old_sid).await.expect("archive missing session");

        let resumed = MobBuilder::for_resume(MobStorage { events })
            .with_session_service(service.clone())
            .register_tool_bundle("bundle-a", Arc::new(EchoBundleDispatcher))
            .resume()
            .await
            .expect("resume");

        let new_sid = resumed
            .get_meerkat(&MeerkatId::from("w-1"))
            .await
            .expect("recreated entry")
            .session_id;
        assert_ne!(new_sid, old_sid, "resume should recreate missing session");

        let names = service.external_tool_names(&new_sid).await;
        assert!(
            names.contains(&"spawn_meerkat".to_string()),
            "mob tools must be wired on recreated sessions"
        );
        assert!(
            names.contains(&"mob_task_create".to_string()),
            "mob task tools must be wired on recreated sessions"
        );
        assert!(
            names.contains(&"bundle_echo".to_string()),
            "rust bundle tools must be wired on recreated sessions"
        );
    }

    #[tokio::test]
    async fn test_resume_reestablishes_missing_trust() {
        let service = Arc::new(MockSessionService::new());
        let storage = MobStorage::in_memory();
        let events = storage.events.clone();
        let handle = MobBuilder::new(sample_definition(), storage)
            .with_session_service(service.clone())
            .create()
            .await
            .expect("create mob");
        let sid_1 = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn w-1");
        let sid_2 = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
            .await
            .expect("spawn w-2");
        handle
            .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
            .await
            .expect("wire");
        handle.stop().await.expect("stop");

        service.force_remove_trust(&sid_1, &sid_2).await;
        service.force_remove_trust(&sid_2, &sid_1).await;

        let resumed = MobBuilder::for_resume(MobStorage { events })
            .with_session_service(service.clone())
            .resume()
            .await
            .expect("resume");
        let resumed_sid_1 = resumed
            .get_meerkat(&MeerkatId::from("w-1"))
            .await
            .expect("w-1")
            .session_id;
        let resumed_sid_2 = resumed
            .get_meerkat(&MeerkatId::from("w-2"))
            .await
            .expect("w-2")
            .session_id;
        let trusted_1 = service.trusted_peer_names(&resumed_sid_1).await;
        let trusted_2 = service.trusted_peer_names(&resumed_sid_2).await;
        assert!(
            trusted_1.contains(&test_comms_name("worker", "w-2")),
            "resume should restore trust from w-1 to w-2"
        );
        assert!(
            trusted_2.contains(&test_comms_name("worker", "w-1")),
            "resume should restore trust from w-2 to w-1"
        );
    }

    #[tokio::test]
    async fn test_complete_archives_and_emits_mob_completed() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn w-1");
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
            .await
            .expect("spawn w-2");

        handle.complete().await.expect("complete");
        assert_eq!(handle.status(), MobState::Completed);
        assert!(
            handle.list_meerkats().await.is_empty(),
            "complete should retire all active meerkats"
        );
        assert_eq!(
            service.active_session_count().await,
            0,
            "complete should archive all sessions"
        );
        let events = handle.events().replay_all().await.expect("replay");
        assert!(
            events
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::MobCompleted)),
            "complete should emit MobCompleted"
        );
    }

    #[tokio::test]
    async fn test_destroy_deletes_storage() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn");
        handle.destroy().await.expect("destroy");
        assert_eq!(handle.status(), MobState::Destroyed);

        assert!(
            handle.list_meerkats().await.is_empty(),
            "destroy should clear roster"
        );
        assert_eq!(
            service.active_session_count().await,
            0,
            "destroy should archive active sessions"
        );
        assert!(
            handle.events().replay_all().await.expect("replay").is_empty(),
            "destroy should delete persisted events"
        );
    }

    #[tokio::test]
    async fn test_destroy_cleans_mcp_namespace() {
        let (handle, _service) = create_test_mob(sample_definition_with_mcp_servers()).await;
        handle.destroy().await.expect("destroy");
        assert!(
            handle.mcp_server_states().await.is_empty(),
            "destroy should clean up mcp namespace state"
        );
    }

    #[tokio::test]
    async fn test_for_resume_requires_mob_created_event() {
        let service = Arc::new(MockSessionService::new());
        let result = MobBuilder::for_resume(MobStorage::in_memory())
            .with_session_service(service)
            .resume()
            .await;
        assert!(matches!(result, Err(MobError::Internal(_))));
    }

    // -----------------------------------------------------------------------
    // P1-T04: spawn() creates a real session
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_spawn_creates_session() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        let session_id = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn should succeed");

        // Verify roster updated
        let meerkats = handle.list_meerkats().await;
        assert_eq!(meerkats.len(), 1);
        assert_eq!(meerkats[0].meerkat_id.as_str(), "w-1");
        assert_eq!(meerkats[0].profile.as_str(), "worker");
        assert_eq!(meerkats[0].session_id, session_id);
    }

    #[tokio::test]
    async fn test_spawn_create_session_request_sets_host_mode_and_peer_meta_labels() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn");

        let create_requests = service.recorded_create_requests().await;
        assert_eq!(create_requests.len(), 1, "exactly one create_session expected");
        let req = &create_requests[0];
        assert!(req.host_mode, "spawn must create host_mode sessions");
        assert_eq!(req.comms_name.as_deref(), Some("test-mob/worker/w-1"));
        assert_eq!(
            req.peer_meta_labels.get("mob_id").map(String::as_str),
            Some("test-mob")
        );
        assert_eq!(
            req.peer_meta_labels.get("role").map(String::as_str),
            Some("worker")
        );
        assert_eq!(
            req.peer_meta_labels.get("meerkat_id").map(String::as_str),
            Some("w-1")
        );
    }

    #[tokio::test]
    async fn test_spawn_emits_meerkat_spawned_event() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        let session_id = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn");

        let events = handle.events().replay_all().await.expect("replay");
        // Should have MobCreated + MeerkatSpawned
        assert!(
            events.len() >= 2,
            "expected at least 2 events, got {}",
            events.len()
        );
        let spawned = events
            .iter()
            .find(|e| matches!(e.kind, MobEventKind::MeerkatSpawned { .. }));
        assert!(spawned.is_some(), "should have MeerkatSpawned event");
        if let MobEventKind::MeerkatSpawned {
            meerkat_id,
            role,
            session_id: sid,
            ..
        } = &spawned.unwrap().kind
        {
            assert_eq!(meerkat_id.as_str(), "w-1");
            assert_eq!(role.as_str(), "worker");
            assert_eq!(*sid, session_id);
        }
    }

    #[tokio::test]
    async fn test_spawn_duplicate_meerkat_id_fails() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("first spawn");

        let result = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await;
        assert!(matches!(result, Err(MobError::MeerkatAlreadyExists(_))));
    }

    #[tokio::test]
    async fn test_spawn_unknown_profile_fails() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        let result = handle
            .spawn(
                ProfileName::from("nonexistent"),
                MeerkatId::from("x-1"),
                None,
            )
            .await;
        assert!(matches!(result, Err(MobError::ProfileNotFound(_))));
    }

    #[tokio::test]
    async fn test_spawn_fails_when_profile_comms_disabled() {
        let mut definition = sample_definition();
        let worker = definition
            .profiles
            .get_mut(&ProfileName::from("worker"))
            .expect("worker profile exists");
        worker.tools.comms = false;

        let (handle, service) = create_test_mob(definition).await;
        let result = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await;
        assert!(
            matches!(result, Err(MobError::WiringError(_))),
            "tools.comms=false must be rejected by spawn path"
        );
        assert_eq!(
            service.recorded_prompts().await.len(),
            0,
            "spawn should fail before create_session when comms is disabled"
        );
        assert!(handle.get_meerkat(&MeerkatId::from("w-1")).await.is_none());
    }

    #[tokio::test]
    async fn test_spawn_append_failure_rolls_back_runtime_state() {
        let events = Arc::new(FaultInjectedMobEventStore::new());
        events.fail_appends_for("MeerkatSpawned").await;
        let (handle, service) = create_test_mob_with_events(sample_definition(), events).await;

        let result = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await;
        assert!(
            matches!(result, Err(MobError::Internal(_))),
            "spawn should surface append failure"
        );
        assert!(
            handle.get_meerkat(&MeerkatId::from("w-1")).await.is_none(),
            "spawn append failure must leave roster untouched"
        );
        assert_eq!(
            service.active_session_count().await,
            0,
            "spawn append failure must archive the just-created session"
        );

        let recorded = handle.events().replay_all().await.expect("replay");
        assert!(
            !recorded
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::MeerkatSpawned { .. })),
            "failed spawn append must not persist MeerkatSpawned"
        );
    }

    // -----------------------------------------------------------------------
    // P1-T05: retire() removes a meerkat
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_retire_removes_from_roster() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn");

        handle.retire(MeerkatId::from("w-1")).await.expect("retire");

        assert!(handle.list_meerkats().await.is_empty());
    }

    #[tokio::test]
    async fn test_retire_emits_event() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn");

        handle.retire(MeerkatId::from("w-1")).await.expect("retire");

        let events = handle.events().replay_all().await.expect("replay");
        let retired = events
            .iter()
            .find(|e| matches!(e.kind, MobEventKind::MeerkatRetired { .. }));
        assert!(retired.is_some(), "should have MeerkatRetired event");
    }

    #[tokio::test]
    async fn test_retire_nonexistent_is_idempotent() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        let result = handle.retire(MeerkatId::from("nope")).await;
        assert!(result.is_ok(), "retire should be idempotent");
    }

    #[tokio::test]
    async fn test_retire_removes_wiring_from_peers() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn w-1");
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
            .await
            .expect("spawn w-2");

        handle
            .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
            .await
            .expect("wire");

        // Verify wired
        let entry = handle.get_meerkat(&MeerkatId::from("w-2")).await.unwrap();
        assert!(entry.wired_to.contains(&MeerkatId::from("w-1")));

        // Retire w-1
        handle.retire(MeerkatId::from("w-1")).await.expect("retire");

        // w-2 should no longer be wired to w-1
        let entry = handle.get_meerkat(&MeerkatId::from("w-2")).await.unwrap();
        assert!(!entry.wired_to.contains(&MeerkatId::from("w-1")));
    }

    #[tokio::test]
    async fn test_retire_archive_failure_is_not_silent() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        let session_id = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn w-1");
        service.set_archive_failure(&session_id).await;

        let result = handle.retire(MeerkatId::from("w-1")).await;
        assert!(
            matches!(result, Err(MobError::SessionError(SessionError::Store(_)))),
            "retire should surface archive failures"
        );

        assert!(
            handle.get_meerkat(&MeerkatId::from("w-1")).await.is_some(),
            "failed retire must not remove roster entry"
        );
        let events = handle.events().replay_all().await.expect("replay");
        assert!(
            events
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::MeerkatRetired { .. })),
            "retire event should be persisted before archive so cleanup remains retryable"
        );
    }

    #[tokio::test]
    async fn test_retire_trust_removal_failure_is_not_silent() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        service
            .set_comms_behavior(
                &test_comms_name("worker", "w-2"),
                MockCommsBehavior {
                    fail_remove_trust: true,
                    ..MockCommsBehavior::default()
                },
            )
            .await;

        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn w-1");
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
            .await
            .expect("spawn w-2");
        handle
            .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
            .await
            .expect("wire");

        let result = handle.retire(MeerkatId::from("w-1")).await;
        assert!(
            matches!(result, Err(MobError::CommsError(_))),
            "retire should surface trust-removal failures"
        );

        assert!(
            handle.get_meerkat(&MeerkatId::from("w-1")).await.is_some(),
            "failed retire must not remove roster entry"
        );
        let events = handle.events().replay_all().await.expect("replay");
        assert!(
            events
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::MeerkatRetired { .. })),
            "retire event should persist even when trust-removal cleanup fails"
        );
    }

    #[tokio::test]
    async fn test_retire_fails_when_peer_retired_notification_fails_without_side_effects() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        service
            .set_comms_behavior(
                &test_comms_name("worker", "w-1"),
                MockCommsBehavior {
                    fail_send_peer_retired: true,
                    ..MockCommsBehavior::default()
                },
            )
            .await;

        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn w-1");
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
            .await
            .expect("spawn w-2");
        handle
            .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
            .await
            .expect("wire");

        let result = handle.retire(MeerkatId::from("w-1")).await;
        assert!(
            matches!(result, Err(MobError::CommsError(_))),
            "retire should fail when required notification fails"
        );

        assert!(
            handle.get_meerkat(&MeerkatId::from("w-1")).await.is_some(),
            "failed retire must keep roster entry"
        );
        let entry_w2 = handle
            .get_meerkat(&MeerkatId::from("w-2"))
            .await
            .expect("w-2 should stay in roster");
        assert!(
            entry_w2.wired_to.contains(&MeerkatId::from("w-1")),
            "failed retire must preserve wiring"
        );

        let events = handle.events().replay_all().await.expect("replay");
        assert!(
            events
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::MeerkatRetired { .. })),
            "retire event should persist even when notification cleanup fails"
        );
    }

    #[tokio::test]
    async fn test_retire_append_failure_is_retryable_without_side_effects() {
        let events = Arc::new(FaultInjectedMobEventStore::new());
        events.fail_appends_for("MeerkatRetired").await;
        let (handle, service) = create_test_mob_with_events(sample_definition(), events).await;

        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn w-1");
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
            .await
            .expect("spawn w-2");
        handle
            .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
            .await
            .expect("wire");

        let result = handle.retire(MeerkatId::from("w-1")).await;
        assert!(
            matches!(result, Err(MobError::Internal(_))),
            "retire should surface append failure"
        );
        assert!(
            handle.get_meerkat(&MeerkatId::from("w-1")).await.is_some(),
            "retire append failure must keep roster state retryable"
        );
        let entry_w2 = handle
            .get_meerkat(&MeerkatId::from("w-2"))
            .await
            .expect("w-2 should remain in roster");
        assert!(
            entry_w2.wired_to.contains(&MeerkatId::from("w-1")),
            "retire append failure must not apply trust cleanup before event persistence"
        );
        assert_eq!(
            service.active_session_count().await,
            2,
            "retire append failure must not archive sessions"
        );

        let recorded = handle.events().replay_all().await.expect("replay");
        assert!(
            !recorded
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::MeerkatRetired { .. })),
            "failed retire append must not persist MeerkatRetired"
        );
    }

    // -----------------------------------------------------------------------
    // P1-T06: wire() establishes bidirectional trust
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_wire_establishes_bidirectional() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");

        handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("wire");

        // Check roster wiring
        let entry_l = handle.get_meerkat(&MeerkatId::from("l-1")).await.unwrap();
        assert!(entry_l.wired_to.contains(&MeerkatId::from("w-1")));

        let entry_w = handle.get_meerkat(&MeerkatId::from("w-1")).await.unwrap();
        assert!(entry_w.wired_to.contains(&MeerkatId::from("l-1")));
    }

    #[tokio::test]
    async fn test_wire_emits_peers_wired_event() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn");
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn");

        handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("wire");

        let events = handle.events().replay_all().await.expect("replay");
        let wired = events
            .iter()
            .find(|e| matches!(e.kind, MobEventKind::PeersWired { .. }));
        assert!(wired.is_some(), "should have PeersWired event");
    }

    #[tokio::test]
    async fn test_wire_unknown_meerkat_fails() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn");

        let result = handle
            .wire(MeerkatId::from("w-1"), MeerkatId::from("nonexistent"))
            .await;
        assert!(matches!(result, Err(MobError::MeerkatNotFound(_))));
    }

    #[tokio::test]
    async fn test_wire_fails_when_comms_runtime_missing_without_side_effects() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        let _sid_l = handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        let sid_w = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");
        service.set_missing_comms_runtime(&sid_w).await;

        let result = handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await;
        assert!(matches!(result, Err(MobError::WiringError(_))));

        let entry_l = handle.get_meerkat(&MeerkatId::from("l-1")).await.unwrap();
        let entry_w = handle.get_meerkat(&MeerkatId::from("w-1")).await.unwrap();
        assert!(
            entry_l.wired_to.is_empty(),
            "failed wire must not update roster"
        );
        assert!(
            entry_w.wired_to.is_empty(),
            "failed wire must not update roster"
        );

        let events = handle.events().replay_all().await.expect("replay");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::PeersWired { .. })),
            "failed wire must not emit PeersWired"
        );
    }

    #[tokio::test]
    async fn test_wire_fails_when_public_key_missing_without_side_effects() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        service
            .set_comms_behavior(
                &test_comms_name("worker", "w-1"),
                MockCommsBehavior {
                    missing_public_key: true,
                    ..MockCommsBehavior::default()
                },
            )
            .await;

        handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");

        let result = handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await;
        assert!(matches!(result, Err(MobError::WiringError(_))));

        let entry_l = handle.get_meerkat(&MeerkatId::from("l-1")).await.unwrap();
        let entry_w = handle.get_meerkat(&MeerkatId::from("w-1")).await.unwrap();
        assert!(
            entry_l.wired_to.is_empty(),
            "failed wire must not update roster"
        );
        assert!(
            entry_w.wired_to.is_empty(),
            "failed wire must not update roster"
        );

        let events = handle.events().replay_all().await.expect("replay");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::PeersWired { .. })),
            "failed wire must not emit PeersWired"
        );
    }

    #[tokio::test]
    async fn test_wire_fails_when_peer_added_notification_fails_without_side_effects() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        service
            .set_comms_behavior(
                &test_comms_name("worker", "w-1"),
                MockCommsBehavior {
                    fail_send_peer_added: true,
                    ..MockCommsBehavior::default()
                },
            )
            .await;

        let sid_l = handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        let sid_w = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");

        let result = handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await;
        assert!(
            matches!(result, Err(MobError::CommsError(_))),
            "wire should fail when required notification fails"
        );

        let entry_l = handle.get_meerkat(&MeerkatId::from("l-1")).await.unwrap();
        let entry_w = handle.get_meerkat(&MeerkatId::from("w-1")).await.unwrap();
        assert!(entry_l.wired_to.is_empty(), "failed wire must not mutate roster");
        assert!(entry_w.wired_to.is_empty(), "failed wire must not mutate roster");

        let trusted_by_lead = service.trusted_peer_names(&sid_l).await;
        let trusted_by_worker = service.trusted_peer_names(&sid_w).await;
        assert!(
            !trusted_by_lead.contains(&test_comms_name("worker", "w-1")),
            "failed wire must rollback lead trust"
        );
        assert!(
            !trusted_by_worker.contains(&test_comms_name("lead", "l-1")),
            "failed wire must rollback worker trust"
        );

        let events = handle.events().replay_all().await.expect("replay");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::PeersWired { .. })),
            "failed wire must not emit PeersWired"
        );
    }

    #[tokio::test]
    async fn test_wire_establishes_comms_trust() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        let sid_l = handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        let sid_w = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");

        handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("wire should succeed");

        let trusted_by_lead = service.trusted_peer_names(&sid_l).await;
        let trusted_by_worker = service.trusted_peer_names(&sid_w).await;
        assert!(
            trusted_by_lead.contains(&test_comms_name("worker", "w-1")),
            "lead runtime must trust worker runtime"
        );
        assert!(
            trusted_by_worker.contains(&test_comms_name("lead", "l-1")),
            "worker runtime must trust lead runtime"
        );

        let intents_lead = service.sent_intents(&sid_l).await;
        let intents_worker = service.sent_intents(&sid_w).await;
        assert!(
            intents_lead.iter().any(|intent| intent == "mob.peer_added"),
            "lead should send mob.peer_added during wire"
        );
        assert!(
            intents_worker.iter().any(|intent| intent == "mob.peer_added"),
            "worker should send mob.peer_added during wire"
        );
    }

    #[tokio::test]
    async fn test_wire_append_failure_rolls_back_runtime_state() {
        let events = Arc::new(FaultInjectedMobEventStore::new());
        events.fail_appends_for("PeersWired").await;
        let (handle, service) = create_test_mob_with_events(sample_definition(), events).await;

        let sid_l = handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        let sid_w = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");

        let result = handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await;
        assert!(
            matches!(result, Err(MobError::Internal(_))),
            "wire should surface append failure"
        );

        let entry_l = handle.get_meerkat(&MeerkatId::from("l-1")).await.unwrap();
        let entry_w = handle.get_meerkat(&MeerkatId::from("w-1")).await.unwrap();
        assert!(
            entry_l.wired_to.is_empty(),
            "wire append failure must rollback roster wiring"
        );
        assert!(
            entry_w.wired_to.is_empty(),
            "wire append failure must rollback roster wiring"
        );

        let trusted_by_lead = service.trusted_peer_names(&sid_l).await;
        let trusted_by_worker = service.trusted_peer_names(&sid_w).await;
        assert!(
            !trusted_by_lead.contains(&test_comms_name("worker", "w-1")),
            "wire append failure must rollback trust on lead"
        );
        assert!(
            !trusted_by_worker.contains(&test_comms_name("lead", "l-1")),
            "wire append failure must rollback trust on worker"
        );

        let recorded = handle.events().replay_all().await.expect("replay");
        assert!(
            !recorded
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::PeersWired { .. })),
            "failed wire append must not persist PeersWired"
        );
    }

    // -----------------------------------------------------------------------
    // P1-T07: unwire() removes bidirectional trust
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_unwire_removes_bidirectional() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn");
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn");

        handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("wire");

        handle
            .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("unwire");

        let entry_l = handle.get_meerkat(&MeerkatId::from("l-1")).await.unwrap();
        assert!(entry_l.wired_to.is_empty());

        let entry_w = handle.get_meerkat(&MeerkatId::from("w-1")).await.unwrap();
        assert!(entry_w.wired_to.is_empty());
    }

    #[tokio::test]
    async fn test_unwire_emits_peers_unwired_event() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn");
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn");
        handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("wire");

        handle
            .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("unwire");

        let events = handle.events().replay_all().await.expect("replay");
        let unwired = events
            .iter()
            .find(|e| matches!(e.kind, MobEventKind::PeersUnwired { .. }));
        assert!(unwired.is_some(), "should have PeersUnwired event");
    }

    #[tokio::test]
    async fn test_unwire_fails_when_comms_runtime_missing_without_side_effects() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        let sid_w = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");
        handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("wire");
        service.set_missing_comms_runtime(&sid_w).await;

        let result = handle
            .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await;
        assert!(matches!(result, Err(MobError::WiringError(_))));

        let entry_l = handle.get_meerkat(&MeerkatId::from("l-1")).await.unwrap();
        let entry_w = handle.get_meerkat(&MeerkatId::from("w-1")).await.unwrap();
        assert!(
            entry_l.wired_to.contains(&MeerkatId::from("w-1")),
            "failed unwire must not mutate roster"
        );
        assert!(
            entry_w.wired_to.contains(&MeerkatId::from("l-1")),
            "failed unwire must not mutate roster"
        );

        let events = handle.events().replay_all().await.expect("replay");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::PeersUnwired { .. })),
            "failed unwire must not emit PeersUnwired"
        );
    }

    #[tokio::test]
    async fn test_unwire_fails_when_public_key_missing_without_side_effects() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        let sid_w = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");
        handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("wire");
        service.clear_public_key(&sid_w).await;

        let result = handle
            .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await;
        assert!(matches!(result, Err(MobError::WiringError(_))));

        let entry_l = handle.get_meerkat(&MeerkatId::from("l-1")).await.unwrap();
        let entry_w = handle.get_meerkat(&MeerkatId::from("w-1")).await.unwrap();
        assert!(
            entry_l.wired_to.contains(&MeerkatId::from("w-1")),
            "failed unwire must not mutate roster"
        );
        assert!(
            entry_w.wired_to.contains(&MeerkatId::from("l-1")),
            "failed unwire must not mutate roster"
        );

        let events = handle.events().replay_all().await.expect("replay");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::PeersUnwired { .. })),
            "failed unwire must not emit PeersUnwired"
        );
    }

    #[tokio::test]
    async fn test_unwire_second_trust_removal_failure_restores_first_side() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        service
            .set_comms_behavior(
                &test_comms_name("worker", "w-1"),
                MockCommsBehavior {
                    fail_remove_trust: true,
                    ..MockCommsBehavior::default()
                },
            )
            .await;

        let sid_l = handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        let sid_w = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");
        handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("wire");

        let result = handle
            .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await;
        assert!(
            matches!(result, Err(MobError::CommsError(_))),
            "unwire should surface second trust-removal failure"
        );

        let entry_l = handle.get_meerkat(&MeerkatId::from("l-1")).await.unwrap();
        let entry_w = handle.get_meerkat(&MeerkatId::from("w-1")).await.unwrap();
        assert!(
            entry_l.wired_to.contains(&MeerkatId::from("w-1")),
            "failed unwire must keep roster wiring on lead"
        );
        assert!(
            entry_w.wired_to.contains(&MeerkatId::from("l-1")),
            "failed unwire must keep roster wiring on worker"
        );

        let trusted_by_lead = service.trusted_peer_names(&sid_l).await;
        let trusted_by_worker = service.trusted_peer_names(&sid_w).await;
        assert!(
            trusted_by_lead.contains(&test_comms_name("worker", "w-1")),
            "failed unwire must restore first trust removal on lead"
        );
        assert!(
            trusted_by_worker.contains(&test_comms_name("lead", "l-1")),
            "failed unwire must keep worker trust unchanged"
        );
    }

    #[tokio::test]
    async fn test_unwire_fails_when_peer_retired_notification_fails_without_side_effects() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        service
            .set_comms_behavior(
                &test_comms_name("lead", "l-1"),
                MockCommsBehavior {
                    fail_send_peer_retired: true,
                    ..MockCommsBehavior::default()
                },
            )
            .await;

        handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");
        handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("wire");

        let result = handle
            .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await;
        assert!(
            matches!(result, Err(MobError::CommsError(_))),
            "unwire should fail when required notification fails"
        );

        let entry_l = handle.get_meerkat(&MeerkatId::from("l-1")).await.unwrap();
        let entry_w = handle.get_meerkat(&MeerkatId::from("w-1")).await.unwrap();
        assert!(
            entry_l.wired_to.contains(&MeerkatId::from("w-1")),
            "failed unwire must not mutate roster"
        );
        assert!(
            entry_w.wired_to.contains(&MeerkatId::from("l-1")),
            "failed unwire must not mutate roster"
        );

        let events = handle.events().replay_all().await.expect("replay");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::PeersUnwired { .. })),
            "failed unwire must not emit PeersUnwired"
        );
    }

    #[tokio::test]
    async fn test_unwire_second_notification_failure_compensates_and_preserves_state() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        service
            .set_comms_behavior(
                &test_comms_name("worker", "w-1"),
                MockCommsBehavior {
                    fail_send_peer_retired: true,
                    ..MockCommsBehavior::default()
                },
            )
            .await;

        let sid_l = handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        let sid_w = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");
        handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("wire");

        let peer_added_count = |intents: &[String]| {
            intents.iter().filter(|intent| intent.as_str() == "mob.peer_added").count()
        };
        let lead_peer_added_before = peer_added_count(&service.sent_intents(&sid_l).await);
        let worker_peer_added_before = peer_added_count(&service.sent_intents(&sid_w).await);

        let result = handle
            .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await;
        assert!(
            matches!(result, Err(MobError::CommsError(_))),
            "unwire should fail when second required notification fails"
        );

        let lead_peer_added_after = peer_added_count(&service.sent_intents(&sid_l).await);
        let worker_peer_added_after = peer_added_count(&service.sent_intents(&sid_w).await);
        assert_eq!(
            lead_peer_added_after,
            lead_peer_added_before + 1,
            "second-notification failure should compensate with mob.peer_added from lead"
        );
        assert_eq!(
            worker_peer_added_after, worker_peer_added_before,
            "worker should not emit extra mob.peer_added on second-notification failure"
        );

        let entry_l = handle.get_meerkat(&MeerkatId::from("l-1")).await.unwrap();
        let entry_w = handle.get_meerkat(&MeerkatId::from("w-1")).await.unwrap();
        assert!(
            entry_l.wired_to.contains(&MeerkatId::from("w-1")),
            "failed unwire must preserve lead wiring"
        );
        assert!(
            entry_w.wired_to.contains(&MeerkatId::from("l-1")),
            "failed unwire must preserve worker wiring"
        );

        let trusted_by_lead = service.trusted_peer_names(&sid_l).await;
        let trusted_by_worker = service.trusted_peer_names(&sid_w).await;
        assert!(
            trusted_by_lead.contains(&test_comms_name("worker", "w-1")),
            "failed unwire must keep lead trust"
        );
        assert!(
            trusted_by_worker.contains(&test_comms_name("lead", "l-1")),
            "failed unwire must keep worker trust"
        );

        let events = handle.events().replay_all().await.expect("replay");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::PeersUnwired { .. })),
            "failed unwire must not emit PeersUnwired"
        );
    }

    #[tokio::test]
    async fn test_unwire_first_trust_removal_failure_compensates_and_preserves_state() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        service
            .set_comms_behavior(
                &test_comms_name("lead", "l-1"),
                MockCommsBehavior {
                    fail_remove_trust: true,
                    ..MockCommsBehavior::default()
                },
            )
            .await;

        let sid_l = handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        let sid_w = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");
        handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("wire");

        let peer_added_count = |intents: &[String]| {
            intents.iter().filter(|intent| intent.as_str() == "mob.peer_added").count()
        };
        let lead_peer_added_before = peer_added_count(&service.sent_intents(&sid_l).await);
        let worker_peer_added_before = peer_added_count(&service.sent_intents(&sid_w).await);

        let result = handle
            .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await;
        assert!(
            matches!(result, Err(MobError::CommsError(_))),
            "unwire should surface first trust-removal failure"
        );

        let lead_peer_added_after = peer_added_count(&service.sent_intents(&sid_l).await);
        let worker_peer_added_after = peer_added_count(&service.sent_intents(&sid_w).await);
        assert_eq!(
            lead_peer_added_after,
            lead_peer_added_before + 1,
            "first-removal failure should compensate with mob.peer_added from lead"
        );
        assert_eq!(
            worker_peer_added_after,
            worker_peer_added_before + 1,
            "first-removal failure should compensate with mob.peer_added from worker"
        );

        let entry_l = handle.get_meerkat(&MeerkatId::from("l-1")).await.unwrap();
        let entry_w = handle.get_meerkat(&MeerkatId::from("w-1")).await.unwrap();
        assert!(
            entry_l.wired_to.contains(&MeerkatId::from("w-1")),
            "failed unwire must preserve lead wiring"
        );
        assert!(
            entry_w.wired_to.contains(&MeerkatId::from("l-1")),
            "failed unwire must preserve worker wiring"
        );

        let trusted_by_lead = service.trusted_peer_names(&sid_l).await;
        let trusted_by_worker = service.trusted_peer_names(&sid_w).await;
        assert!(
            trusted_by_lead.contains(&test_comms_name("worker", "w-1")),
            "failed unwire must keep lead trust"
        );
        assert!(
            trusted_by_worker.contains(&test_comms_name("lead", "l-1")),
            "failed unwire must keep worker trust"
        );

        let events = handle.events().replay_all().await.expect("replay");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::PeersUnwired { .. })),
            "failed unwire must not emit PeersUnwired"
        );
    }

    #[tokio::test]
    async fn test_unwire_append_failure_restores_runtime_state() {
        let events = Arc::new(FaultInjectedMobEventStore::new());
        events.fail_appends_for("PeersUnwired").await;
        let (handle, service) = create_test_mob_with_events(sample_definition(), events).await;

        let sid_l = handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        let sid_w = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");
        handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("wire");

        let result = handle
            .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await;
        assert!(
            matches!(result, Err(MobError::Internal(_))),
            "unwire should surface append failure"
        );

        let entry_l = handle.get_meerkat(&MeerkatId::from("l-1")).await.unwrap();
        let entry_w = handle.get_meerkat(&MeerkatId::from("w-1")).await.unwrap();
        assert!(
            entry_l.wired_to.contains(&MeerkatId::from("w-1")),
            "unwire append failure must restore roster wiring"
        );
        assert!(
            entry_w.wired_to.contains(&MeerkatId::from("l-1")),
            "unwire append failure must restore roster wiring"
        );

        let trusted_by_lead = service.trusted_peer_names(&sid_l).await;
        let trusted_by_worker = service.trusted_peer_names(&sid_w).await;
        assert!(
            trusted_by_lead.contains(&test_comms_name("worker", "w-1")),
            "unwire append failure must restore trust on lead"
        );
        assert!(
            trusted_by_worker.contains(&test_comms_name("lead", "l-1")),
            "unwire append failure must restore trust on worker"
        );

        let recorded = handle.events().replay_all().await.expect("replay");
        assert!(
            !recorded
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::PeersUnwired { .. })),
            "failed unwire append must not persist PeersUnwired"
        );
    }

    // -----------------------------------------------------------------------
    // P1-T08: auto_wire_orchestrator on spawn
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_auto_wire_orchestrator() {
        let (handle, _service) = create_test_mob(sample_definition_with_auto_wire()).await;

        // Spawn orchestrator first
        handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");

        // Spawn worker — should be auto-wired to orchestrator
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");

        // Verify wiring
        let entry_l = handle.get_meerkat(&MeerkatId::from("l-1")).await.unwrap();
        assert!(
            entry_l.wired_to.contains(&MeerkatId::from("w-1")),
            "orchestrator should be wired to worker"
        );

        let entry_w = handle.get_meerkat(&MeerkatId::from("w-1")).await.unwrap();
        assert!(
            entry_w.wired_to.contains(&MeerkatId::from("l-1")),
            "worker should be wired to orchestrator"
        );
    }

    #[tokio::test]
    async fn test_auto_wire_orchestrator_updates_real_comms_peers() {
        let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
        let (handle, service) =
            create_test_mob_with_real_comms(sample_definition_with_auto_wire()).await;

        let sid_l = handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        let sid_w = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");

        let comms_l = service.real_comms(&sid_l).await.expect("comms for l-1");
        let comms_w = service.real_comms(&sid_w).await.expect("comms for w-1");

        let peers_l = CoreCommsRuntime::peers(&*comms_l).await;
        let peers_w = CoreCommsRuntime::peers(&*comms_w).await;
        assert!(
            peers_l
                .iter()
                .any(|entry| entry.name.as_str() == test_comms_name("worker", "w-1")),
            "auto-wire should add worker to lead peers()"
        );
        assert!(
            peers_w
                .iter()
                .any(|entry| entry.name.as_str() == test_comms_name("lead", "l-1")),
            "auto-wire should add lead to worker peers()"
        );
    }

    #[tokio::test]
    async fn test_auto_wire_orchestrator_not_wired_to_self() {
        let (handle, _service) = create_test_mob(sample_definition_with_auto_wire()).await;

        // Spawn orchestrator — should NOT wire to itself
        handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");

        let entry_l = handle.get_meerkat(&MeerkatId::from("l-1")).await.unwrap();
        assert!(
            entry_l.wired_to.is_empty(),
            "orchestrator should not be wired to itself"
        );
    }

    #[tokio::test]
    async fn test_auto_wire_failure_is_returned_to_spawn_caller() {
        let (handle, service) = create_test_mob(sample_definition_with_auto_wire()).await;
        service
            .set_comms_behavior(
                &test_comms_name("worker", "w-1"),
                MockCommsBehavior {
                    missing_public_key: true,
                    ..MockCommsBehavior::default()
                },
            )
            .await;

        handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");

        let result = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await;
        assert!(
            matches!(result, Err(MobError::WiringError(_))),
            "spawn must fail when auto-wire fails"
        );

        assert!(
            handle.get_meerkat(&MeerkatId::from("w-1")).await.is_none(),
            "failed spawn should rollback roster entry"
        );
        let lead_entry = handle
            .get_meerkat(&MeerkatId::from("l-1"))
            .await
            .expect("lead should remain in roster");
        assert!(
            lead_entry.wired_to.is_empty(),
            "failed spawn should not leave partial wiring on lead"
        );
    }

    #[tokio::test]
    async fn test_auto_wire_failure_after_partial_wire_cleans_peer_trust_via_spawn_rollback() {
        let (handle, service) = create_test_mob(sample_definition_with_auto_wire()).await;
        service
            .set_comms_behavior(
                &test_comms_name("lead", "l-1"),
                MockCommsBehavior {
                    fail_send_peer_added: true,
                    fail_remove_trust_once: true,
                    ..MockCommsBehavior::default()
                },
            )
            .await;

        let sid_l = handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");

        let result = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await;
        assert!(
            matches!(result, Err(MobError::WiringError(_))),
            "spawn must surface wiring failure after rollback cleanup"
        );

        assert!(
            handle.get_meerkat(&MeerkatId::from("w-1")).await.is_none(),
            "failed spawn should rollback worker roster entry"
        );
        let lead_entry = handle
            .get_meerkat(&MeerkatId::from("l-1"))
            .await
            .expect("lead should remain in roster");
        assert!(
            lead_entry.wired_to.is_empty(),
            "failed spawn should not leave partial roster wiring on lead"
        );

        let trusted_by_lead = service.trusted_peer_names(&sid_l).await;
        assert!(
            !trusted_by_lead.contains(&test_comms_name("worker", "w-1")),
            "spawn rollback must remove leaked trust from lead"
        );
    }

    #[tokio::test]
    async fn test_spawn_rollback_ignores_missing_comms_for_non_wired_planned_targets() {
        let (handle, service) = create_test_mob(sample_definition_with_auto_wire()).await;
        service
            .set_comms_behavior(
                &test_comms_name("worker", "w-1"),
                MockCommsBehavior {
                    missing_public_key: true,
                    ..MockCommsBehavior::default()
                },
            )
            .await;

        let sid_l = handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        service.set_missing_comms_runtime(&sid_l).await;

        let result = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await;
        assert!(
            matches!(result, Err(MobError::WiringError(_))),
            "spawn should surface original wiring failure instead of rollback failure"
        );
        assert!(
            handle.get_meerkat(&MeerkatId::from("w-1")).await.is_none(),
            "rollback should still remove failed worker entry"
        );
    }

    #[tokio::test]
    async fn test_spawn_rollback_ignores_missing_comms_for_non_wired_planned_targets_with_spawned_key() {
        let (handle, service) = create_test_mob(sample_definition_with_role_wiring()).await;

        let sid_w1 = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn w-1");
        let sid_w2 = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
            .await
            .expect("spawn w-2");
        service
            .set_comms_behavior(
                &test_comms_name("worker", "w-1"),
                MockCommsBehavior {
                    fail_send_peer_added: true,
                    fail_remove_trust_once: true,
                    ..MockCommsBehavior::default()
                },
            )
            .await;
        service.set_missing_comms_runtime(&sid_w2).await;

        let result = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-3"), None)
            .await;
        assert!(
            matches!(result, Err(MobError::WiringError(_))),
            "spawn should surface role-wiring failure, not rollback failure"
        );
        assert!(
            handle.get_meerkat(&MeerkatId::from("w-3")).await.is_none(),
            "failed spawn should not keep w-3 in roster"
        );

        let trusted_by_w1 = service.trusted_peer_names(&sid_w1).await;
        assert!(
            !trusted_by_w1.contains(&test_comms_name("worker", "w-3")),
            "rollback should remove leaked trust for first failed planned target"
        );
    }

    #[tokio::test]
    async fn test_spawn_rollback_archive_failure_keeps_spawned_entry_and_persists_retired_event() {
        let (handle, service) = create_test_mob(sample_definition_with_auto_wire()).await;
        service
            .set_comms_behavior(
                &test_comms_name("worker", "w-1"),
                MockCommsBehavior {
                    missing_public_key: true,
                    ..MockCommsBehavior::default()
                },
            )
            .await;
        service
            .set_archive_failure_for_comms_name(&test_comms_name("worker", "w-1"))
            .await;

        handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");

        let result = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await;
        assert!(
            matches!(result, Err(MobError::Internal(_))),
            "spawn should surface rollback archive failure"
        );
        assert!(
            handle.get_meerkat(&MeerkatId::from("w-1")).await.is_some(),
            "rollback archive failure must not remove spawned roster entry"
        );

        let events = handle.events().replay_all().await.expect("replay");
        assert!(
            events
                .iter()
                .any(|e| matches!(e.kind, MobEventKind::MeerkatRetired { .. })),
            "rollback must persist MeerkatRetired before side-effect cleanup so retries stay replay-safe"
        );
    }

    // -----------------------------------------------------------------------
    // P1-T09: role_wiring fan-out on spawn
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_role_wiring_fan_out() {
        let (handle, _service) = create_test_mob(sample_definition_with_role_wiring()).await;

        // Spawn 3 workers
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn w-1");
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
            .await
            .expect("spawn w-2");
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-3"), None)
            .await
            .expect("spawn w-3");

        // w-1 was spawned first (no other workers existed), so no wiring for it at spawn time.
        // w-2 was spawned second, rule worker<->worker matches, wired to w-1.
        // w-3 was spawned third, rule worker<->worker matches, wired to w-1 and w-2.

        let entry_1 = handle.get_meerkat(&MeerkatId::from("w-1")).await.unwrap();
        let entry_2 = handle.get_meerkat(&MeerkatId::from("w-2")).await.unwrap();
        let entry_3 = handle.get_meerkat(&MeerkatId::from("w-3")).await.unwrap();

        // w-2 should be wired to w-1 (and vice versa)
        assert!(entry_2.wired_to.contains(&MeerkatId::from("w-1")));
        assert!(entry_1.wired_to.contains(&MeerkatId::from("w-2")));

        // w-3 should be wired to both w-1 and w-2
        assert!(entry_3.wired_to.contains(&MeerkatId::from("w-1")));
        assert!(entry_3.wired_to.contains(&MeerkatId::from("w-2")));

        // w-1 should be wired to both w-2 and w-3
        // Need to re-read since roster may have been updated after w-3 spawn
        let entry_1 = handle.get_meerkat(&MeerkatId::from("w-1")).await.unwrap();
        assert!(entry_1.wired_to.contains(&MeerkatId::from("w-3")));
    }

    #[tokio::test]
    async fn test_role_wiring_cross_role_fans_out_to_three_existing_targets() {
        let (handle, _service) = create_test_mob(sample_definition_with_cross_role_wiring()).await;

        for worker_id in ["w-1", "w-2", "w-3"] {
            handle
                .spawn(
                    ProfileName::from("worker"),
                    MeerkatId::from(worker_id),
                    None,
                )
                .await
                .expect("spawn worker");
        }

        handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");

        let lead = handle
            .get_meerkat(&MeerkatId::from("l-1"))
            .await
            .expect("lead should be in roster");
        assert_eq!(
            lead.wired_to.len(),
            3,
            "new role_x meerkat should fan out to all 3 existing role_y peers"
        );
        for worker_id in ["w-1", "w-2", "w-3"] {
            assert!(
                lead.wired_to.contains(&MeerkatId::from(worker_id)),
                "lead should be wired to {worker_id}"
            );
            let worker = handle
                .get_meerkat(&MeerkatId::from(worker_id))
                .await
                .expect("worker should remain in roster");
            assert!(
                worker.wired_to.contains(&MeerkatId::from("l-1")),
                "{worker_id} should be wired back to lead"
            );
        }

        let events = handle.events().replay_all().await.expect("replay");
        let cross_role_wire_events = events
            .iter()
            .filter(|event| {
                matches!(
                    &event.kind,
                    MobEventKind::PeersWired { a, b }
                        if (a == &MeerkatId::from("l-1")
                            && ["w-1", "w-2", "w-3"]
                                .iter()
                                .any(|w| b == &MeerkatId::from(*w)))
                            || (b == &MeerkatId::from("l-1")
                                && ["w-1", "w-2", "w-3"]
                                    .iter()
                                    .any(|w| a == &MeerkatId::from(*w)))
                )
            })
            .count();
        assert_eq!(
            cross_role_wire_events, 3,
            "fan-out should execute three wire() operations for three existing role_y peers"
        );
    }

    #[tokio::test]
    async fn test_role_wiring_failure_is_returned_to_spawn_caller() {
        let (handle, service) = create_test_mob(sample_definition_with_role_wiring()).await;
        service
            .set_comms_behavior(
                &test_comms_name("worker", "w-2"),
                MockCommsBehavior {
                    missing_public_key: true,
                    ..MockCommsBehavior::default()
                },
            )
            .await;

        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn w-1");

        let result = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
            .await;
        assert!(
            matches!(result, Err(MobError::WiringError(_))),
            "spawn must fail when role-wiring fan-out fails"
        );

        assert!(
            handle.get_meerkat(&MeerkatId::from("w-2")).await.is_none(),
            "failed spawn should rollback roster entry"
        );
        let entry_w1 = handle
            .get_meerkat(&MeerkatId::from("w-1"))
            .await
            .expect("w-1 should remain in roster");
        assert!(
            !entry_w1.wired_to.contains(&MeerkatId::from("w-2")),
            "failed spawn should not leave partial role wiring"
        );
    }

    // -----------------------------------------------------------------------
    // P1-T10: external_turn enforces addressability
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_external_turn_addressable_succeeds() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead (external_addressable=true)");

        let result = handle
            .external_turn(MeerkatId::from("l-1"), "Hello from outside".into())
            .await;
        assert!(
            result.is_ok(),
            "external_turn to addressable meerkat should succeed"
        );
    }

    #[tokio::test]
    async fn test_external_turn_not_addressable_fails() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker (external_addressable=false)");

        let result = handle
            .external_turn(MeerkatId::from("w-1"), "Hello from outside".into())
            .await;
        assert!(
            matches!(result, Err(MobError::NotExternallyAddressable(_))),
            "external_turn to non-addressable meerkat should fail"
        );
    }

    #[tokio::test]
    async fn test_external_turn_unknown_meerkat_fails() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        let result = handle
            .external_turn(MeerkatId::from("nonexistent"), "Hello".into())
            .await;
        assert!(matches!(result, Err(MobError::MeerkatNotFound(_))));
    }

    #[tokio::test]
    async fn test_internal_turn_bypasses_external_addressable_check() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");

        let result = handle
            .internal_turn(MeerkatId::from("w-1"), "internal message".into())
            .await;
        assert!(
            result.is_ok(),
            "internal_turn should bypass external_addressable=false"
        );
    }

    #[tokio::test]
    async fn test_internal_turn_unknown_meerkat_fails() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        let result = handle
            .internal_turn(MeerkatId::from("nonexistent"), "Hello".into())
            .await;
        assert!(matches!(result, Err(MobError::MeerkatNotFound(_))));
    }

    // -----------------------------------------------------------------------
    // P1-T11: MobCreated event stores definition
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_mob_created_event_stores_definition() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        let events = handle.events().replay_all().await.expect("replay");

        // First event should be MobCreated
        assert!(!events.is_empty(), "should have at least one event");
        match &events[0].kind {
            MobEventKind::MobCreated { definition } => {
                assert_eq!(definition.id.as_str(), "test-mob");
                assert_eq!(definition.profiles.len(), 2);
                assert!(definition.profiles.contains_key(&ProfileName::from("lead")));
                assert!(
                    definition
                        .profiles
                        .contains_key(&ProfileName::from("worker"))
                );
            }
            other => panic!("first event should be MobCreated, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_mob_created_definition_roundtrips_through_json() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        let events = handle.events().replay_all().await.expect("replay");

        if let MobEventKind::MobCreated { definition } = &events[0].kind {
            let json = serde_json::to_string(definition).expect("serialize");
            let parsed: MobDefinition = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*definition, parsed);
        }
    }

    // -----------------------------------------------------------------------
    // CHOKE-MOB-005: MobHandle concurrent command serialization
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_concurrent_spawns_serialized() {
        let (handle, _service) = create_test_mob(sample_definition()).await;

        // Spawn 10 workers concurrently
        let mut join_handles = Vec::new();
        for i in 0..10 {
            let h = handle.clone();
            let jh = tokio::spawn(async move {
                h.spawn(
                    ProfileName::from("worker"),
                    MeerkatId::from(format!("w-{i}")),
                    None,
                )
                .await
            });
            join_handles.push(jh);
        }

        for jh in join_handles {
            jh.await.expect("join").expect("spawn");
        }

        let meerkats = handle.list_meerkats().await;
        assert_eq!(meerkats.len(), 10, "all 10 concurrent spawns should succeed");
        let ids = meerkats
            .iter()
            .map(|entry| entry.meerkat_id.as_str().to_string())
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), 10, "serialized actor path must avoid duplicate IDs");
        for expected in [
            "w-0", "w-1", "w-2", "w-3", "w-4", "w-5", "w-6", "w-7", "w-8", "w-9",
        ] {
            assert!(ids.contains(expected), "missing spawned meerkat {expected}");
        }

        let events = handle.events().replay_all().await.expect("replay");
        let spawned_count = events
            .iter()
            .filter(|event| matches!(event.kind, MobEventKind::MeerkatSpawned { .. }))
            .count();
        assert_eq!(
            spawned_count, 10,
            "actor serialization should emit exactly one MeerkatSpawned per request"
        );
    }

    #[tokio::test]
    async fn test_concurrent_spawn_and_retire_same_meerkat_is_serialized() {
        let (handle, _service) = create_test_mob(sample_definition()).await;
        let h1 = handle.clone();
        let h2 = handle.clone();

        let spawn = tokio::spawn(async move {
            h1.spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
                .await
        });
        let retire = tokio::spawn(async move { h2.retire(MeerkatId::from("w-1")).await });

        let _ = spawn.await.expect("spawn join");
        retire.await.expect("retire join").expect("retire");

        let roster = handle.list_meerkats().await;
        assert!(
            roster.is_empty() || (roster.len() == 1 && roster[0].meerkat_id.as_str() == "w-1"),
            "serialized spawn/retire should never corrupt roster"
        );
    }

    // -----------------------------------------------------------------------
    // Additional: MobState
    // -----------------------------------------------------------------------

    #[test]
    fn test_mob_state_roundtrip() {
        for state in [
            MobState::Creating,
            MobState::Running,
            MobState::Stopped,
            MobState::Completed,
            MobState::Destroyed,
        ] {
            assert_eq!(MobState::from_u8(state as u8), state);
        }
    }

    #[test]
    fn test_mob_state_as_str() {
        assert_eq!(MobState::Creating.as_str(), "Creating");
        assert_eq!(MobState::Running.as_str(), "Running");
        assert_eq!(MobState::Stopped.as_str(), "Stopped");
        assert_eq!(MobState::Completed.as_str(), "Completed");
        assert_eq!(MobState::Destroyed.as_str(), "Destroyed");
    }

    // -----------------------------------------------------------------------
    // P1-T04b: spawn() with initial_message uses custom message
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_spawn_with_custom_initial_message() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        let custom_msg = "You are the orchestrator. Begin planning.";

        handle
            .spawn(
                ProfileName::from("lead"),
                MeerkatId::from("l-1"),
                Some(custom_msg.to_string()),
            )
            .await
            .expect("spawn with custom message");

        // Verify the custom message was passed to create_session
        let prompts = service.recorded_prompts().await;
        assert_eq!(prompts.len(), 1, "should have exactly one recorded prompt");
        assert_eq!(
            prompts[0].1, custom_msg,
            "spawn should use the custom initial_message, not the default"
        );
    }

    #[tokio::test]
    async fn test_spawn_without_initial_message_uses_default() {
        let (handle, service) = create_test_mob(sample_definition()).await;

        handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn with default message");

        let prompts = service.recorded_prompts().await;
        assert_eq!(prompts.len(), 1);
        assert!(
            prompts[0].1.contains("You have been spawned as 'w-1'"),
            "default message should contain meerkat id, got: '{}'",
            prompts[0].1
        );
        assert!(
            prompts[0].1.contains("role: worker"),
            "default message should contain role, got: '{}'",
            prompts[0].1
        );
        assert!(
            prompts[0].1.contains("mob 'test-mob'"),
            "default message should contain mob id, got: '{}'",
            prompts[0].1
        );
    }

    // -----------------------------------------------------------------------
    // Behavioral wire test: wire() enables PeerRequest communication
    //
    // Uses real CommsRuntime (inproc_only) to prove add_trusted_peer was
    // actually called by verifying that a PeerRequest sent from A arrives
    // in B's inbox.
    // -----------------------------------------------------------------------

    /// Mock session service that creates sessions with REAL CommsRuntime
    /// instances (inproc_only) instead of MockCommsRuntime. This enables
    /// actual PeerRequest delivery between wired meerkats.
    struct RealCommsSessionService {
        sessions: RwLock<HashMap<SessionId, Arc<meerkat_comms::CommsRuntime>>>,
        session_comms_names: RwLock<HashMap<SessionId, String>>,
        session_counter: AtomicU64,
    }

    impl RealCommsSessionService {
        fn new() -> Self {
            Self {
                sessions: RwLock::new(HashMap::new()),
                session_comms_names: RwLock::new(HashMap::new()),
                session_counter: AtomicU64::new(0),
            }
        }
    }

    #[async_trait]
    impl SessionService for RealCommsSessionService {
        async fn create_session(
            &self,
            req: CreateSessionRequest,
        ) -> Result<RunResult, SessionError> {
            let session_id = SessionId::new();
            let n = self.session_counter.fetch_add(1, Ordering::Relaxed);

            let comms_name = req
                .build
                .as_ref()
                .and_then(|b| b.comms_name.clone())
                .unwrap_or_else(|| format!("real-comms-session-{n}"));

            let comms = meerkat_comms::CommsRuntime::inproc_only(&comms_name)
                .expect("create inproc CommsRuntime");
            self.sessions
                .write()
                .await
                .insert(session_id.clone(), Arc::new(comms));
            self.session_comms_names
                .write()
                .await
                .insert(session_id.clone(), comms_name);

            Ok(mock_run_result(session_id, "Session created".to_string()))
        }

        async fn start_turn(
            &self,
            id: &SessionId,
            _req: StartTurnRequest,
        ) -> Result<RunResult, SessionError> {
            let sessions = self.sessions.read().await;
            if !sessions.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            Ok(mock_run_result(id.clone(), "Turn completed".to_string()))
        }

        async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
            let sessions = self.sessions.read().await;
            if !sessions.contains_key(id) {
                return Err(SessionError::NotFound { id: id.clone() });
            }
            Ok(())
        }

        async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
            let sessions = self.sessions.read().await;
            if !sessions.contains_key(id) {
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
            let sessions = self.sessions.read().await;
            Ok(sessions
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
            let mut sessions = self.sessions.write().await;
            sessions.remove(id);
            self.session_comms_names.write().await.remove(id);
            Ok(())
        }
    }

    #[async_trait]
    impl MobSessionService for RealCommsSessionService {
        async fn comms_runtime(&self, session_id: &SessionId) -> Option<Arc<dyn CoreCommsRuntime>> {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .map(|c| Arc::clone(c) as Arc<dyn CoreCommsRuntime>)
        }

        fn supports_persistent_sessions(&self) -> bool {
            true
        }

        async fn session_belongs_to_mob(&self, session_id: &SessionId, mob_id: &MobId) -> bool {
            let names = self.session_comms_names.read().await;
            names
                .get(session_id)
                .map(|name| name.starts_with(&format!("{mob_id}/")))
                .unwrap_or(false)
        }
    }

    impl RealCommsSessionService {
        /// Get the real CommsRuntime for a session (for test inspection).
        async fn real_comms(
            &self,
            session_id: &SessionId,
        ) -> Option<Arc<meerkat_comms::CommsRuntime>> {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).cloned()
        }
    }

    /// Create a MobHandle with real comms (for behavioral wire tests).
    async fn create_test_mob_with_real_comms(
        definition: MobDefinition,
    ) -> (MobHandle, Arc<RealCommsSessionService>) {
        let service = Arc::new(RealCommsSessionService::new());
        let storage = MobStorage::in_memory();
        let handle = MobBuilder::new(definition, storage)
            .with_session_service(service.clone())
            .create()
            .await
            .expect("create mob");

        (handle, service)
    }

    fn has_request_intent_for_peer(
        interactions: &[meerkat_core::InboxInteraction],
        expected_intent: &str,
        expected_peer: &str,
    ) -> bool {
        interactions.iter().any(|interaction| {
            matches!(
                &interaction.content,
                meerkat_core::InteractionContent::Request { intent, params }
                    if intent == expected_intent && params["peer"] == expected_peer
            )
        })
    }

    static REAL_COMMS_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[tokio::test]
    async fn test_wire_enables_peer_request_delivery() {
        let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
        let (handle, service) = create_test_mob_with_real_comms(sample_definition()).await;

        let sid_a = handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        let sid_b = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");

        handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("wire should succeed");

        let comms_a = service.real_comms(&sid_a).await.expect("comms for l-1");
        let comms_b = service.real_comms(&sid_b).await.expect("comms for w-1");

        let entry_a = handle
            .get_meerkat(&MeerkatId::from("l-1"))
            .await
            .expect("l-1 should be in roster");
        let entry_b = handle
            .get_meerkat(&MeerkatId::from("w-1"))
            .await
            .expect("w-1 should be in roster");
        let comms_name_a = format!(
            "{}/{}/{}",
            handle.mob_id(),
            entry_a.profile,
            entry_a.meerkat_id
        );
        let comms_name_b = format!(
            "{}/{}/{}",
            handle.mob_id(),
            entry_b.profile,
            entry_b.meerkat_id
        );

        let peers_a = CoreCommsRuntime::peers(&*comms_a).await;
        let peers_b = CoreCommsRuntime::peers(&*comms_b).await;
        assert!(
            peers_a.iter().any(|entry| entry.name.as_str() == comms_name_b),
            "wire should expose worker in lead peers()"
        );
        assert!(
            peers_b.iter().any(|entry| entry.name.as_str() == comms_name_a),
            "wire should expose lead in worker peers()"
        );

        let notify_for_a = CoreCommsRuntime::drain_inbox_interactions(&*comms_a).await;
        let notify_for_b = CoreCommsRuntime::drain_inbox_interactions(&*comms_b).await;
        assert!(
            has_request_intent_for_peer(&notify_for_a, "mob.peer_added", "w-1"),
            "lead should receive mob.peer_added for worker"
        );
        assert!(
            has_request_intent_for_peer(&notify_for_b, "mob.peer_added", "l-1"),
            "worker should receive mob.peer_added for lead"
        );

        let peer_name = meerkat_core::comms::PeerName::new(&comms_name_b).expect("valid peer name");
        let cmd = meerkat_core::comms::CommsCommand::PeerRequest {
            to: peer_name,
            intent: "mob.test_ping".to_string(),
            params: serde_json::json!({"test": true}),
            stream: meerkat_core::comms::InputStreamMode::None,
        };
        let receipt = CoreCommsRuntime::send(&*comms_a, cmd)
            .await
            .expect("PeerRequest from l-1 to w-1 should succeed after wire()");
        assert!(
            matches!(
                receipt,
                meerkat_core::comms::SendReceipt::PeerRequestSent { .. }
            ),
            "expected PeerRequestSent, got: {receipt:?}"
        );

        let interactions = CoreCommsRuntime::drain_inbox_interactions(&*comms_b).await;
        assert_eq!(
            interactions.len(),
            1,
            "w-1 should have received exactly one interaction after wire"
        );
        match &interactions[0].content {
            meerkat_core::InteractionContent::Request { intent, params } => {
                assert_eq!(intent, "mob.test_ping");
                assert_eq!(params["test"], true);
            }
            other => panic!("expected Request interaction, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_unwire_updates_peers_and_sends_retired_notifications() {
        let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
        let (handle, service) = create_test_mob_with_real_comms(sample_definition()).await;

        let sid_a = handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        let sid_b = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");
        handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("wire");

        let comms_a = service.real_comms(&sid_a).await.expect("comms for l-1");
        let comms_b = service.real_comms(&sid_b).await.expect("comms for w-1");
        let _ = CoreCommsRuntime::drain_inbox_interactions(&*comms_a).await;
        let _ = CoreCommsRuntime::drain_inbox_interactions(&*comms_b).await;

        handle
            .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("unwire");

        let peers_a = CoreCommsRuntime::peers(&*comms_a).await;
        let peers_b = CoreCommsRuntime::peers(&*comms_b).await;
        assert!(
            !peers_a
                .iter()
                .any(|entry| entry.name.as_str() == test_comms_name("worker", "w-1")),
            "unwire should remove worker from lead peers()"
        );
        assert!(
            !peers_b
                .iter()
                .any(|entry| entry.name.as_str() == test_comms_name("lead", "l-1")),
            "unwire should remove lead from worker peers()"
        );

    }

    #[tokio::test]
    async fn test_retire_updates_peers_and_sends_retired_notifications() {
        let _serial = REAL_COMMS_TEST_LOCK.lock().expect("real-comms test lock");
        let (handle, service) = create_test_mob_with_real_comms(sample_definition()).await;

        let sid_a = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn w-1");
        let sid_b = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
            .await
            .expect("spawn w-2");
        handle
            .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
            .await
            .expect("wire");

        let comms_a = service.real_comms(&sid_a).await.expect("comms for w-1");
        let comms_b = service.real_comms(&sid_b).await.expect("comms for w-2");
        let _ = CoreCommsRuntime::drain_inbox_interactions(&*comms_a).await;
        let _ = CoreCommsRuntime::drain_inbox_interactions(&*comms_b).await;

        handle.retire(MeerkatId::from("w-1")).await.expect("retire");

        assert!(
            handle.get_meerkat(&MeerkatId::from("w-1")).await.is_none(),
            "retired meerkat should leave roster"
        );
        let entry_w2 = handle
            .get_meerkat(&MeerkatId::from("w-2"))
            .await
            .expect("w-2 remains active");
        assert!(
            !entry_w2.wired_to.contains(&MeerkatId::from("w-1")),
            "retire should remove wiring from remaining peer"
        );

        let peers_w2 = CoreCommsRuntime::peers(&*comms_b).await;
        assert!(
            !peers_w2
                .iter()
                .any(|entry| entry.name.as_str() == test_comms_name("worker", "w-1")),
            "retire should remove retired peer from peers()"
        );
    }

    #[tokio::test]
    async fn test_unwire_sends_required_peer_retired_notifications() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        let sid_l = handle
            .spawn(ProfileName::from("lead"), MeerkatId::from("l-1"), None)
            .await
            .expect("spawn lead");
        let sid_w = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn worker");
        handle
            .wire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("wire");

        handle
            .unwire(MeerkatId::from("l-1"), MeerkatId::from("w-1"))
            .await
            .expect("unwire");

        let intents_lead = service.sent_intents(&sid_l).await;
        let intents_worker = service.sent_intents(&sid_w).await;
        assert!(
            intents_lead.iter().any(|intent| intent == "mob.peer_retired"),
            "lead should send mob.peer_retired during unwire"
        );
        assert!(
            intents_worker.iter().any(|intent| intent == "mob.peer_retired"),
            "worker should send mob.peer_retired during unwire"
        );
    }

    #[tokio::test]
    async fn test_retire_sends_required_peer_retired_notifications() {
        let (handle, service) = create_test_mob(sample_definition()).await;
        let sid_w1 = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-1"), None)
            .await
            .expect("spawn w-1");
        let sid_w2 = handle
            .spawn(ProfileName::from("worker"), MeerkatId::from("w-2"), None)
            .await
            .expect("spawn w-2");
        handle
            .wire(MeerkatId::from("w-1"), MeerkatId::from("w-2"))
            .await
            .expect("wire");

        handle.retire(MeerkatId::from("w-1")).await.expect("retire");

        let intents_w1 = service.sent_intents(&sid_w1).await;
        let intents_w2 = service.sent_intents(&sid_w2).await;
        assert!(
            intents_w1.iter().any(|intent| intent == "mob.peer_retired"),
            "retiring peer should send mob.peer_retired notifications"
        );
        assert!(
            !intents_w2.iter().any(|intent| intent == "mob.peer_retired"),
            "non-retiring peer should not be the sender for retire notifications"
        );
    }
}
