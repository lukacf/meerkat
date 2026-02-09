//! EphemeralSessionService — in-memory session lifecycle with no persistence.
//!
//! Each session gets a dedicated tokio task that exclusively owns the `Agent`.
//! Communication happens through channels, generalized from `SessionRuntime` in
//! `meerkat-rpc`.

use async_trait::async_trait;
use indexmap::IndexMap;
use meerkat_core::event::AgentEvent;
use meerkat_core::service::{
    CreateSessionRequest, SessionError, SessionQuery, SessionService, SessionSummary, SessionView,
    StartTurnRequest,
};
use meerkat_core::types::{RunResult, SessionId, Usage};
use std::time::SystemTime;
use tokio::sync::{RwLock, mpsc, oneshot, watch};

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
    pub session_id: SessionId,
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
        event_tx: Option<mpsc::Sender<AgentEvent>>,
        result_tx: oneshot::Sender<Result<RunResult, meerkat_core::error::AgentError>>,
    },
    Interrupt {
        ack_tx: oneshot::Sender<Result<(), SessionError>>,
    },
    ReadSnapshot {
        reply_tx: oneshot::Sender<SessionSnapshot>,
    },
    Shutdown,
}

/// Handle stored in the sessions map.
struct SessionHandle {
    command_tx: mpsc::Sender<SessionCommand>,
    state_rx: watch::Receiver<SessionState>,
    session_id: SessionId,
    created_at: SystemTime,
}

// ---------------------------------------------------------------------------
// Agent abstraction (used to build sessions without depending on AgentFactory)
// ---------------------------------------------------------------------------

/// Trait for building agents from session creation requests.
///
/// This abstracts over `AgentFactory` so `EphemeralSessionService` doesn't
/// need to depend on the facade crate.
#[async_trait]
pub trait SessionAgentBuilder: Send + Sync {
    /// The concrete agent type. Must support `run_with_events` and `cancel`.
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

    /// Cancel the currently running turn.
    fn cancel(&mut self);

    /// Get the session ID.
    fn session_id(&self) -> SessionId;

    /// Take a snapshot of the current session state.
    fn snapshot(&self) -> SessionSnapshot;
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
}

impl<B: SessionAgentBuilder + 'static> EphemeralSessionService<B> {
    /// Create a new ephemeral session service.
    pub fn new(builder: B, max_sessions: usize) -> Self {
        Self {
            sessions: RwLock::new(IndexMap::new()),
            builder,
            max_sessions,
        }
    }

    /// Shut down all sessions.
    pub async fn shutdown(&self) {
        let mut sessions = self.sessions.write().await;
        for (_id, handle) in sessions.drain(..) {
            let _ = handle.command_tx.send(SessionCommand::Shutdown).await;
        }
    }
}

#[async_trait]
impl<B: SessionAgentBuilder + 'static> SessionService for EphemeralSessionService<B> {
    async fn create_session(
        &self,
        req: CreateSessionRequest,
    ) -> Result<RunResult, SessionError> {
        // Check capacity
        {
            let sessions = self.sessions.read().await;
            if sessions.len() >= self.max_sessions {
                return Err(SessionError::Agent(
                    meerkat_core::error::AgentError::InternalError(format!(
                        "Max sessions reached ({}/{})",
                        sessions.len(),
                        self.max_sessions
                    )),
                ));
            }
        }

        let prompt = req.prompt.clone();
        let caller_event_tx = req.event_tx.clone();

        // Create the permanent event channel for this session.
        let (agent_event_tx, agent_event_rx) =
            mpsc::channel::<AgentEvent>(EVENT_CHANNEL_CAPACITY);

        // Build the agent
        let agent = self
            .builder
            .build_agent(&req, agent_event_tx.clone())
            .await?;

        let session_id = agent.session_id();
        let created_at = SystemTime::now();

        // Create session task channels
        let (command_tx, command_rx) = mpsc::channel::<SessionCommand>(COMMAND_CHANNEL_CAPACITY);
        let (state_tx, state_rx) = watch::channel(SessionState::Idle);

        // Spawn the session task
        tokio::spawn(session_task(
            agent,
            agent_event_tx,
            agent_event_rx,
            command_rx,
            state_tx,
        ));

        // Store the handle
        let handle = SessionHandle {
            command_tx: command_tx.clone(),
            state_rx,
            session_id: session_id.clone(),
            created_at,
        };

        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id.clone(), handle);
        }

        // Run the first turn
        let (result_tx, result_rx) = oneshot::channel();
        command_tx
            .send(SessionCommand::StartTurn {
                prompt,
                event_tx: caller_event_tx,
                result_tx,
            })
            .await
            .map_err(|_| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    "Session task exited before first turn".to_string(),
                ))
            })?;

        let result = result_rx.await.map_err(|_| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                "Session task dropped the result channel".to_string(),
            ))
        })?;

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
            let handle = sessions.get(id).ok_or_else(|| SessionError::NotFound {
                id: id.clone(),
            })?;

            // Check if the session is busy
            let state = *handle.state_rx.borrow();
            if state == SessionState::Running {
                return Err(SessionError::Busy { id: id.clone() });
            }

            handle
                .command_tx
                .send(SessionCommand::StartTurn {
                    prompt: req.prompt,
                    event_tx: req.event_tx,
                    result_tx,
                })
                .await
                .map_err(|_| {
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
        let handle = sessions.get(id).ok_or_else(|| SessionError::NotFound {
            id: id.clone(),
        })?;

        let state = *handle.state_rx.borrow();
        if state != SessionState::Running {
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

        // Wait for acknowledgement
        let _ = ack_rx.await;
        Ok(())
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        let sessions = self.sessions.read().await;
        let handle = sessions.get(id).ok_or_else(|| SessionError::NotFound {
            id: id.clone(),
        })?;

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

        let state = *handle.state_rx.borrow();
        Ok(SessionView {
            session_id: snapshot.session_id,
            created_at: snapshot.created_at,
            updated_at: snapshot.updated_at,
            message_count: snapshot.message_count,
            total_tokens: snapshot.total_tokens,
            usage: snapshot.usage,
            is_active: state == SessionState::Running,
            last_assistant_text: snapshot.last_assistant_text,
        })
    }

    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        let sessions = self.sessions.read().await;
        let mut summaries: Vec<SessionSummary> = sessions
            .values()
            .map(|h| {
                let state = *h.state_rx.borrow();
                SessionSummary {
                    session_id: h.session_id.clone(),
                    created_at: h.created_at,
                    updated_at: h.created_at, // Updated on creation; full snapshot from task would be better
                    message_count: 0,
                    total_tokens: 0,
                    is_active: state == SessionState::Running,
                }
            })
            .collect();

        if let Some(offset) = query.offset {
            summaries = summaries.into_iter().skip(offset).collect();
        }
        if let Some(limit) = query.limit {
            summaries.truncate(limit);
        }

        Ok(summaries)
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        let mut sessions = self.sessions.write().await;
        let handle = sessions.swap_remove(id).ok_or_else(|| SessionError::NotFound {
            id: id.clone(),
        })?;

        // Send shutdown command. Ignore errors if the task already exited.
        let _ = handle.command_tx.send(SessionCommand::Shutdown).await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Session task
// ---------------------------------------------------------------------------

/// Long-lived task that exclusively owns a session agent and processes commands.
async fn session_task<A: SessionAgent>(
    mut agent: A,
    agent_event_tx: mpsc::Sender<AgentEvent>,
    mut agent_event_rx: mpsc::Receiver<AgentEvent>,
    mut commands: mpsc::Receiver<SessionCommand>,
    state_tx: watch::Sender<SessionState>,
) {
    while let Some(cmd) = commands.recv().await {
        match cmd {
            SessionCommand::StartTurn {
                prompt,
                event_tx,
                result_tx,
            } => {
                state_tx.send_replace(SessionState::Running);

                let run_fut = agent.run_with_events(prompt, agent_event_tx.clone());
                tokio::pin!(run_fut);

                let result = loop {
                    tokio::select! {
                        result = &mut run_fut => break result,
                        Some(event) = agent_event_rx.recv() => {
                            if let Some(ref tx) = event_tx {
                                let _ = tx.send(event).await;
                            }
                        }
                    }
                };

                // Drain any remaining events after the run completes
                while let Ok(event) = agent_event_rx.try_recv() {
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(event).await;
                    }
                }

                state_tx.send_replace(SessionState::Idle);
                let _ = result_tx.send(result);
            }
            SessionCommand::Interrupt { ack_tx } => {
                agent.cancel();
                let _ = ack_tx.send(Ok(()));
            }
            SessionCommand::ReadSnapshot { reply_tx } => {
                let _ = reply_tx.send(agent.snapshot());
            }
            SessionCommand::Shutdown => {
                state_tx.send_replace(SessionState::ShuttingDown);
                break;
            }
        }
    }
}
