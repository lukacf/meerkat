//! EphemeralSessionService — in-memory session lifecycle with no persistence.
//!
//! Each session gets a dedicated tokio task that exclusively owns the `Agent`.
//! Communication happens through channels, generalized from `SessionRuntime` in
//! `meerkat-rpc`.

use async_trait::async_trait;
use indexmap::IndexMap;
use meerkat_core::event::AgentEvent;
use meerkat_core::service::{
    CreateSessionRequest, SessionError, SessionInfo, SessionQuery, SessionService, SessionSummary,
    SessionUsage, SessionView, StartTurnRequest,
};
use meerkat_core::types::{RunResult, SessionId, Usage};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore, mpsc, oneshot, watch};

/// Capacity for the internal agent event channel.
const EVENT_CHANNEL_CAPACITY: usize = 256;

/// Capacity for session command channel.
const COMMAND_CHANNEL_CAPACITY: usize = 8;

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Observable state of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionState {
    Idle,
    Running,
    ShuttingDown,
}

/// Snapshot of session metadata for read/list operations.
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub message_count: usize,
    pub total_tokens: u64,
    pub usage: Usage,
    pub last_assistant_text: Option<String>,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Commands sent from the service to a session task.
enum SessionCommand {
    StartTurn {
        prompt: String,
        host_mode: bool,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
        result_tx: oneshot::Sender<Result<RunResult, meerkat_core::error::AgentError>>,
        skill_references: Option<Vec<meerkat_core::skills::SkillId>>,
    },
    Interrupt {
        ack_tx: oneshot::Sender<Result<(), SessionError>>,
    },
    ReadSnapshot {
        reply_tx: oneshot::Sender<SessionSnapshot>,
    },
    /// Export the full session (messages + metadata) for persistence.
    ExportSession {
        reply_tx: oneshot::Sender<meerkat_core::Session>,
    },
    Shutdown,
}

/// Lightweight summary updated after each turn, readable without querying the task.
struct SessionSummaryCache {
    updated_at: SystemTime,
    message_count: usize,
    total_tokens: u64,
}

/// Handle stored in the sessions map.
struct SessionHandle {
    command_tx: mpsc::Sender<SessionCommand>,
    state_rx: watch::Receiver<SessionState>,
    summary_rx: watch::Receiver<SessionSummaryCache>,
    /// Atomic turn-admission lock. Set to `true` by the caller before sending
    /// `StartTurn`, guaranteeing that only one turn is admitted at a time.
    /// Reset to `false` by the session task after the turn completes.
    turn_lock: Arc<AtomicBool>,
    _capacity_permit: OwnedSemaphorePermit,
    created_at: SystemTime,
    /// Subscribable event injector for pushing external events.
    /// Extracted from the agent before it moves into its task.
    event_injector: Option<Arc<dyn meerkat_core::SubscribableInjector>>,
    /// Optional comms runtime for host-mode commands and stream attachment.
    comms_runtime: Option<Arc<dyn meerkat_core::agent::CommsRuntime>>,
}

// ---------------------------------------------------------------------------
// Agent abstraction
// ---------------------------------------------------------------------------

/// Trait for building agents from session creation requests.
#[async_trait]
pub trait SessionAgentBuilder: Send + Sync {
    /// The concrete agent type.
    type Agent: SessionAgent + Send + 'static;

    /// Build an agent for a new session.
    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, SessionError>;
}

/// Trait abstracting over the agent's run/cancel interface.
#[async_trait]
pub trait SessionAgent: Send {
    /// Run the agent with the given prompt, streaming events.
    async fn run_with_events(
        &mut self,
        prompt: String,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError>;

    /// Run the agent in host mode: process prompt then stay alive for comms.
    ///
    /// Event streaming should use the agent's build-time configured event channel.
    async fn run_host_mode(
        &mut self,
        prompt: String,
    ) -> Result<RunResult, meerkat_core::error::AgentError>;

    /// Stage skill references to resolve and inject on the next turn.
    fn set_skill_references(&mut self, refs: Option<Vec<meerkat_core::skills::SkillId>>);

    /// Cancel the currently running turn.
    fn cancel(&mut self);

    /// Get the session ID.
    fn session_id(&self) -> SessionId;

    /// Take a snapshot of the current session state.
    fn snapshot(&self) -> SessionSnapshot;

    /// Clone the full session (messages + metadata) for persistence.
    ///
    /// This is more expensive than `snapshot()` because it includes the
    /// full message history. Only called by `PersistentSessionService`
    /// after each turn.
    fn session_clone(&self) -> meerkat_core::Session;

    /// Get a subscribable event injector for pushing external events.
    ///
    /// Called once before the agent moves into its dedicated task. The returned
    /// injector is stored in the session handle for surfaces to access.
    /// Callers can use `inject()` for fire-and-forget or
    /// `inject_with_subscription()` for interaction-scoped streaming.
    fn event_injector(&self) -> Option<Arc<dyn meerkat_core::SubscribableInjector>> {
        None
    }

    /// Get the comms runtime used by this agent, if any.
    ///
    /// Called once before the agent moves into its dedicated task. The returned
    /// runtime is stored in the session handle for surfaces that need comms
    /// command execution and stream attachment.
    fn comms_runtime(&self) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        None
    }
}

// ---------------------------------------------------------------------------
// EphemeralSessionService
// ---------------------------------------------------------------------------

/// In-memory session service with no persistence.
///
/// Sessions are kept alive as tokio tasks. All state is lost on process exit.
pub struct EphemeralSessionService<B: SessionAgentBuilder> {
    sessions: RwLock<IndexMap<SessionId, SessionHandle>>,
    builder: B,
    max_sessions: usize,
    session_capacity: Arc<Semaphore>,
    /// Notified when a new session handle is stored. Used by CLI --stdin
    /// to avoid polling for the session to appear.
    session_registered: tokio::sync::Notify,
}

impl<B: SessionAgentBuilder + 'static> EphemeralSessionService<B> {
    /// Create a new ephemeral session service.
    pub fn new(builder: B, max_sessions: usize) -> Self {
        Self {
            sessions: RwLock::new(IndexMap::new()),
            builder,
            max_sessions,
            session_capacity: Arc::new(Semaphore::new(max_sessions)),
            session_registered: tokio::sync::Notify::new(),
        }
    }

    /// Export the full session (messages + metadata) for persistence.
    ///
    /// Returns the complete `Session` including message history. Used by
    /// `PersistentSessionService` to save full snapshots after each turn.
    pub async fn export_session(
        &self,
        id: &SessionId,
    ) -> Result<meerkat_core::Session, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;

        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::ExportSession { reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;

        reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped the reply channel".to_string(),
            ))
        })
    }

    /// Get the subscribable event injector for a session, if available.
    ///
    /// Returns `None` if the session doesn't exist, has no comms runtime,
    /// or the comms runtime doesn't support event injection.
    ///
    /// Use `inject()` for fire-and-forget or `inject_with_subscription()`
    /// for interaction-scoped streaming.
    pub async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::SubscribableInjector>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .and_then(|h| h.event_injector.clone())
    }

    /// Get the comms runtime for a session, if available.
    pub async fn comms_runtime(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .and_then(|h| h.comms_runtime.clone())
    }

    /// Wait for a session to be registered.
    ///
    /// Returns when the next session handle is stored. Used by CLI `--stdin`
    /// to wait for the session to become available before starting the stdin reader.
    pub async fn wait_session_registered(&self) {
        self.session_registered.notified().await;
    }

    /// Shut down all sessions.
    pub async fn shutdown(&self) {
        let mut sessions = self.sessions.write().await;
        for (_id, handle) in sessions.drain(..) {
            let _ = handle.command_tx.send(SessionCommand::Shutdown).await;
        }
    }

    /// Acquire the turn lock atomically. Returns Err(Busy) if already locked.
    fn try_acquire_turn(id: &SessionId, handle: &SessionHandle) -> Result<(), SessionError> {
        match handle
            .turn_lock
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => Ok(()),
            Err(_) => Err(SessionError::Busy { id: id.clone() }),
        }
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> SessionService for EphemeralSessionService<B> {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        // Reserve capacity up front so two concurrent create_session calls cannot race
        // past max_sessions between check and insert.
        let capacity_permit = match self.session_capacity.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                let active = self.sessions.read().await.len();
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "Max sessions reached ({}/{})",
                        active, self.max_sessions
                    )),
                ));
            }
        };

        let prompt = req.prompt.clone();
        let caller_event_tx = req.event_tx.clone();

        // Create the permanent event channel for this session.
        let (agent_event_tx, agent_event_rx) = mpsc::channel::<AgentEvent>(EVENT_CHANNEL_CAPACITY);

        // Build the agent
        let agent = self
            .builder
            .build_agent(&req, agent_event_tx.clone())
            .await?;

        let session_id = agent.session_id();
        let created_at = SystemTime::now();
        let turn_lock = Arc::new(AtomicBool::new(false));

        // Extract the event injector before the agent moves into its task.
        let event_injector = agent.event_injector();
        let comms_runtime = agent.comms_runtime();

        // Create session task channels
        let (command_tx, command_rx) = mpsc::channel::<SessionCommand>(COMMAND_CHANNEL_CAPACITY);
        let (state_tx, state_rx) = watch::channel(SessionState::Idle);
        let (summary_tx, summary_rx) = watch::channel(SessionSummaryCache {
            updated_at: created_at,
            message_count: 0,
            total_tokens: 0,
        });

        // Spawn the session task
        let task_turn_lock = turn_lock.clone();
        tokio::spawn(session_task(
            agent,
            agent_event_tx,
            agent_event_rx,
            command_rx,
            state_tx,
            summary_tx,
            task_turn_lock,
        ));

        // Store the handle
        let handle = SessionHandle {
            command_tx: command_tx.clone(),
            state_rx,
            summary_rx,
            turn_lock: turn_lock.clone(),
            _capacity_permit: capacity_permit,
            created_at,
            event_injector,
            comms_runtime,
        };

        // Acquire turn lock for the first turn (cannot fail — fresh session)
        turn_lock.store(true, Ordering::Release);

        let inserted = {
            let mut sessions = self.sessions.write().await;
            if sessions.contains_key(&session_id) {
                false
            } else {
                sessions.insert(session_id.clone(), handle);
                // Notify waiters (e.g., CLI --stdin) that a session is available.
                self.session_registered.notify_waiters();
                true
            }
        };
        if !inserted {
            // Duplicate IDs are unexpected but can happen if the builder returns a reused ID.
            // Stop the task so it does not leak in the background.
            let _ = command_tx.send(SessionCommand::Shutdown).await;
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(format!(
                    "Duplicate session ID generated: {session_id}"
                )),
            ));
        }

        // Run the first turn
        let host_mode = req.host_mode;
        let (result_tx, result_rx) = oneshot::channel();
        if command_tx
            .send(SessionCommand::StartTurn {
                prompt,
                host_mode,
                event_tx: caller_event_tx,
                result_tx,
                skill_references: req.skill_references,
            })
            .await
            .is_err()
        {
            turn_lock.store(false, Ordering::Release);
            let mut sessions = self.sessions.write().await;
            sessions.swap_remove(&session_id);
            return Err(SessionError::Agent(
                meerkat_core::error::AgentError::InternalError(
                    "Session task exited before first turn".to_string(),
                ),
            ));
        }

        let result = match result_rx.await {
            Ok(result) => result,
            Err(_) => {
                let mut sessions = self.sessions.write().await;
                sessions.swap_remove(&session_id);
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(
                        "Session task dropped the result channel".to_string(),
                    ),
                ));
            }
        };

        result.map_err(SessionError::Agent)
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        let (result_tx, result_rx) = oneshot::channel();

        {
            let sessions = self.sessions.read().await;
            let handle = sessions
                .get(id)
                .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;

            // Atomic busy check via compare-and-swap. This is the single
            // point of admission — if two callers race, exactly one wins.
            Self::try_acquire_turn(id, handle)?;

            handle
                .command_tx
                .send(SessionCommand::StartTurn {
                    prompt: req.prompt,
                    host_mode: req.host_mode,
                    event_tx: req.event_tx,
                    result_tx,
                    skill_references: req.skill_references,
                })
                .await
                .map_err(|_| {
                    handle.turn_lock.store(false, Ordering::Release);
                    SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                        "Session task has exited".to_string(),
                    ))
                })?;
        }

        let result = result_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped the result channel".to_string(),
            ))
        })?;

        result.map_err(SessionError::Agent)
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;

        // Check turn_lock atomically — if false, no turn is running.
        // This avoids the TOCTOU race of checking state_rx then sending.
        if !handle.turn_lock.load(Ordering::Acquire) {
            return Err(SessionError::NotRunning { id: id.clone() });
        }

        let (ack_tx, ack_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::Interrupt { ack_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;

        let _ = ack_rx.await;
        Ok(())
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions
            .get(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;

        let (reply_tx, reply_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::ReadSnapshot { reply_tx })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task has exited".to_string(),
                ))
            })?;

        let snapshot = reply_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped the reply channel".to_string(),
            ))
        })?;

        let is_active = *handle.state_rx.borrow() == SessionState::Running;
        Ok(SessionView {
            state: SessionInfo {
                session_id: id.clone(),
                created_at: snapshot.created_at,
                updated_at: snapshot.updated_at,
                message_count: snapshot.message_count,
                is_active,
                last_assistant_text: snapshot.last_assistant_text,
            },
            billing: SessionUsage {
                total_tokens: snapshot.total_tokens,
                usage: snapshot.usage,
            },
        })
    }

    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        let sessions = self.sessions.read().await;
        let mut summaries: Vec<SessionSummary> = sessions
            .iter()
            .map(|(session_id, h)| {
                let state = *h.state_rx.borrow();
                let cache = h.summary_rx.borrow();
                SessionSummary {
                    session_id: session_id.clone(),
                    created_at: h.created_at,
                    updated_at: cache.updated_at,
                    message_count: cache.message_count,
                    total_tokens: cache.total_tokens,
                    is_active: state == SessionState::Running,
                }
            })
            .collect();

        if let Some(offset) = query.offset {
            if offset < summaries.len() {
                summaries = summaries.split_off(offset);
            } else {
                summaries.clear();
            }
        }
        if let Some(limit) = query.limit {
            summaries.truncate(limit);
        }

        Ok(summaries)
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        let mut sessions = self.sessions.write().await;
        let handle = sessions
            .swap_remove(id)
            .ok_or_else(|| SessionError::NotFound { id: id.clone() })?;

        let _ = handle.command_tx.send(SessionCommand::Shutdown).await;
        Ok(())
    }

    async fn comms_runtime(
        &self,
        id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.comms_runtime(id).await
    }
}

// ---------------------------------------------------------------------------
// Session task
// ---------------------------------------------------------------------------

/// Long-lived task that exclusively owns a session agent and processes commands.
///
/// The `turn_lock` is released after each turn completes, allowing the next
/// `start_turn` call to proceed.
async fn session_task<A: SessionAgent>(
    mut agent: A,
    agent_event_tx: mpsc::Sender<AgentEvent>,
    mut agent_event_rx: mpsc::Receiver<AgentEvent>,
    mut commands: mpsc::Receiver<SessionCommand>,
    state_tx: watch::Sender<SessionState>,
    summary_tx: watch::Sender<SessionSummaryCache>,
    turn_lock: Arc<AtomicBool>,
) {
    while let Some(cmd) = commands.recv().await {
        match cmd {
            SessionCommand::StartTurn {
                prompt,
                host_mode,
                event_tx,
                result_tx,
                skill_references,
            } => {
                agent.set_skill_references(skill_references);
                state_tx.send_replace(SessionState::Running);
                let mut event_stream_open = true;

                // Scope the pinned future so its mutable borrow of `agent` is
                // released before we call `agent.snapshot()`.
                let result = {
                    let run_fut: std::pin::Pin<
                        Box<
                            dyn std::future::Future<
                                    Output = Result<RunResult, meerkat_core::error::AgentError>,
                                > + Send
                                + '_,
                        >,
                    > = if host_mode {
                        Box::pin(agent.run_host_mode(prompt))
                    } else {
                        Box::pin(agent.run_with_events(prompt, agent_event_tx.clone()))
                    };
                    // run_fut is already Pin<Box<...>>, no tokio::pin! needed.
                    let mut run_fut = run_fut;

                    let r = loop {
                        tokio::select! {
                            result = &mut run_fut => break result,
                            Some(event) = agent_event_rx.recv() => {
                                if event_stream_open
                                    && let Some(ref tx) = event_tx
                                    && tx.send(event).await.is_err()
                                {
                                    event_stream_open = false;
                                    tracing::warn!("session event stream receiver dropped; continuing without streaming events");
                                }
                            }
                        }
                    };

                    // Drain any remaining events
                    while let Ok(event) = agent_event_rx.try_recv() {
                        if event_stream_open
                            && let Some(ref tx) = event_tx
                            && tx.send(event).await.is_err()
                        {
                            event_stream_open = false;
                            tracing::warn!(
                                "session event stream receiver dropped while draining events"
                            );
                        }
                    }

                    r
                }; // run_fut dropped here

                // Update cached summary
                let snap = agent.snapshot();
                summary_tx.send_replace(SessionSummaryCache {
                    updated_at: snap.updated_at,
                    message_count: snap.message_count,
                    total_tokens: snap.total_tokens,
                });

                state_tx.send_replace(SessionState::Idle);
                // Release the turn lock AFTER setting state to Idle and
                // updating the summary, so the next caller sees consistent state.
                turn_lock.store(false, Ordering::Release);
                let _ = result_tx.send(result);
            }
            SessionCommand::Interrupt { ack_tx } => {
                agent.cancel();
                let _ = ack_tx.send(Ok(()));
            }
            SessionCommand::ReadSnapshot { reply_tx } => {
                let _ = reply_tx.send(agent.snapshot());
            }
            SessionCommand::ExportSession { reply_tx } => {
                let _ = reply_tx.send(agent.session_clone());
            }
            SessionCommand::Shutdown => {
                state_tx.send_replace(SessionState::ShuttingDown);
                break;
            }
        }
    }
}
