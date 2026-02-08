//! SessionRuntime - keeps agents alive between turns.
//!
//! Each session gets a dedicated tokio task that exclusively owns the `Agent`,
//! solving the `cancel(&mut self)` requirement without mutex. Communication
//! between the runtime and session tasks happens through channels.

use std::sync::Arc;

use indexmap::IndexMap;
use meerkat::{AgentBuildConfig, AgentFactory, BuildAgentError, DynAgent};
use meerkat_client::LlmClient;
use meerkat_core::event::AgentEvent;
use meerkat_core::types::{RunResult, SessionId};
use meerkat_core::Config;
use tokio::sync::{RwLock, mpsc, oneshot, watch};

use crate::NOTIFICATION_CHANNEL_CAPACITY;
use crate::error;
use crate::protocol::RpcError;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Observable state of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// The session is idle and ready to accept a new turn.
    Idle,
    /// A turn is currently running.
    Running,
    /// The session is shutting down.
    ShuttingDown,
}

impl SessionState {
    /// Return a stable string representation matching the serde `rename_all` convention.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::ShuttingDown => "shutting_down",
        }
    }
}

/// Summary information about a session.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: SessionId,
    pub state: SessionState,
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

/// Commands sent from the runtime to a session task.
enum SessionCommand {
    StartTurn {
        prompt: String,
        event_tx: mpsc::Sender<AgentEvent>,
        result_tx: oneshot::Sender<Result<RunResult, meerkat_core::AgentError>>,
    },
    Interrupt {
        ack_tx: oneshot::Sender<()>,
    },
    Shutdown,
}

/// Handle stored in the sessions map, used to communicate with a session task.
struct SessionHandle {
    command_tx: mpsc::Sender<SessionCommand>,
    state_rx: watch::Receiver<SessionState>,
    session_id: SessionId,
}

// ---------------------------------------------------------------------------
// SessionRuntime
// ---------------------------------------------------------------------------

/// Core runtime that manages agent sessions.
///
/// Each session is backed by a dedicated tokio task that exclusively owns the
/// `Agent` value. The runtime communicates with session tasks via channels.
pub struct SessionRuntime {
    sessions: RwLock<IndexMap<SessionId, SessionHandle>>,
    factory: AgentFactory,
    config: Config,
    max_sessions: usize,
    /// Override LLM client for all sessions (primarily for testing).
    pub default_llm_client: Option<Arc<dyn LlmClient>>,
}

impl SessionRuntime {
    /// Create a new session runtime.
    pub fn new(factory: AgentFactory, config: Config, max_sessions: usize) -> Self {
        Self {
            sessions: RwLock::new(IndexMap::new()),
            factory,
            config,
            max_sessions,
            default_llm_client: None,
        }
    }

    /// Create a new session with the given build configuration.
    ///
    /// Returns the session ID on success.
    pub async fn create_session(
        &self,
        build_config: AgentBuildConfig,
    ) -> Result<SessionId, RpcError> {
        // Check capacity
        {
            let sessions = self.sessions.read().await;
            if sessions.len() >= self.max_sessions {
                return Err(RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!(
                        "Max sessions reached ({}/{})",
                        sessions.len(),
                        self.max_sessions
                    ),
                    data: None,
                });
            }
        }

        // Inject default LLM client if the caller didn't provide one.
        let build_config = if build_config.llm_client_override.is_none() {
            if let Some(ref client) = self.default_llm_client {
                AgentBuildConfig {
                    llm_client_override: Some(client.clone()),
                    ..build_config
                }
            } else {
                build_config
            }
        } else {
            build_config
        };

        // Create the permanent event channel for this session.
        let (agent_event_tx, agent_event_rx) = mpsc::channel::<AgentEvent>(NOTIFICATION_CHANNEL_CAPACITY);

        // Wire the event_tx into the build config so the LLM adapter streams
        // TextDelta events through it.
        let build_config = AgentBuildConfig {
            event_tx: Some(agent_event_tx.clone()),
            ..build_config
        };

        // Build the agent
        let agent = self
            .factory
            .build_agent(build_config, &self.config)
            .await
            .map_err(build_error_to_rpc)?;

        let session_id = agent.session().id().clone();

        // Create session task channels
        let (command_tx, command_rx) = mpsc::channel::<SessionCommand>(8);
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
            command_tx,
            state_rx,
            session_id: session_id.clone(),
        };

        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session_id.clone(), handle);
        }

        Ok(session_id)
    }

    /// Start a turn on the given session.
    ///
    /// Events are forwarded to `event_tx` during the turn. Returns the
    /// `RunResult` when the turn completes.
    pub async fn start_turn(
        &self,
        session_id: &SessionId,
        prompt: String,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, RpcError> {
        let (result_tx, result_rx) = oneshot::channel();

        {
            let sessions = self.sessions.read().await;
            let handle = sessions.get(session_id).ok_or_else(|| RpcError {
                code: error::SESSION_NOT_FOUND,
                message: format!("Session not found: {session_id}"),
                data: None,
            })?;

            // Check if the session is busy
            let state = *handle.state_rx.borrow();
            if state == SessionState::Running {
                return Err(RpcError {
                    code: error::SESSION_BUSY,
                    message: format!("Session {session_id} is busy"),
                    data: None,
                });
            }

            handle
                .command_tx
                .send(SessionCommand::StartTurn {
                    prompt,
                    event_tx,
                    result_tx,
                })
                .await
                .map_err(|_| RpcError {
                    code: error::INTERNAL_ERROR,
                    message: "Session task has exited".to_string(),
                    data: None,
                })?;
        }

        // Wait for the turn to complete
        let result = result_rx.await.map_err(|_| RpcError {
            code: error::INTERNAL_ERROR,
            message: "Session task dropped the result channel".to_string(),
            data: None,
        })?;

        result.map_err(agent_error_to_rpc)
    }

    /// Interrupt a running turn on the given session.
    ///
    /// If the session is idle, this is a no-op.
    pub async fn interrupt(&self, session_id: &SessionId) -> Result<(), RpcError> {
        let sessions = self.sessions.read().await;
        let handle = sessions.get(session_id).ok_or_else(|| RpcError {
            code: error::SESSION_NOT_FOUND,
            message: format!("Session not found: {session_id}"),
            data: None,
        })?;

        let state = *handle.state_rx.borrow();
        if state != SessionState::Running {
            // Not running - nothing to interrupt
            return Ok(());
        }

        let (ack_tx, ack_rx) = oneshot::channel();
        handle
            .command_tx
            .send(SessionCommand::Interrupt { ack_tx })
            .await
            .map_err(|_| RpcError {
                code: error::INTERNAL_ERROR,
                message: "Session task has exited".to_string(),
                data: None,
            })?;

        // Wait for acknowledgement
        let _ = ack_rx.await;
        Ok(())
    }

    /// Get the current state of a session, or `None` if the session does not exist.
    pub async fn session_state(&self, session_id: &SessionId) -> Option<SessionState> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|h| *h.state_rx.borrow())
    }

    /// Archive (remove) a session.
    pub async fn archive_session(&self, session_id: &SessionId) -> Result<(), RpcError> {
        let mut sessions = self.sessions.write().await;
        let handle = sessions.swap_remove(session_id).ok_or_else(|| RpcError {
            code: error::SESSION_NOT_FOUND,
            message: format!("Session not found: {session_id}"),
            data: None,
        })?;

        // Send shutdown command. Ignore errors if the task already exited.
        let _ = handle.command_tx.send(SessionCommand::Shutdown).await;
        Ok(())
    }

    /// List all active sessions.
    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        let sessions = self.sessions.read().await;
        sessions
            .values()
            .map(|h| SessionInfo {
                session_id: h.session_id.clone(),
                state: *h.state_rx.borrow(),
            })
            .collect()
    }

    /// Shut down the runtime, closing all sessions.
    pub async fn shutdown(&self) {
        let mut sessions = self.sessions.write().await;
        for (_id, handle) in sessions.drain(..) {
            let _ = handle.command_tx.send(SessionCommand::Shutdown).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Session task
// ---------------------------------------------------------------------------

/// Long-lived task that exclusively owns a `DynAgent` and processes commands.
async fn session_task(
    mut agent: DynAgent,
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
                            let _ = event_tx.send(event).await;
                        }
                    }
                };

                // Drain any remaining events after the run completes
                while let Ok(event) = agent_event_rx.try_recv() {
                    let _ = event_tx.send(event).await;
                }

                state_tx.send_replace(SessionState::Idle);
                let _ = result_tx.send(result);
            }
            SessionCommand::Interrupt { ack_tx } => {
                agent.cancel();
                let _ = ack_tx.send(());
            }
            SessionCommand::Shutdown => {
                state_tx.send_replace(SessionState::ShuttingDown);
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

fn agent_error_to_rpc(err: meerkat_core::AgentError) -> RpcError {
    let code = match &err {
        meerkat_core::AgentError::TokenBudgetExceeded { .. }
        | meerkat_core::AgentError::TimeBudgetExceeded { .. }
        | meerkat_core::AgentError::ToolCallBudgetExceeded { .. } => error::BUDGET_EXHAUSTED,
        meerkat_core::AgentError::HookDenied { .. } => error::HOOK_DENIED,
        meerkat_core::AgentError::Llm { .. } => error::PROVIDER_ERROR,
        meerkat_core::AgentError::SessionNotFound(_) => error::SESSION_NOT_FOUND,
        _ => error::INTERNAL_ERROR,
    };
    RpcError {
        code,
        message: err.to_string(),
        data: None,
    }
}

fn build_error_to_rpc(err: BuildAgentError) -> RpcError {
    let code = match &err {
        BuildAgentError::MissingApiKey { .. } | BuildAgentError::UnknownProvider { .. } => {
            error::PROVIDER_ERROR
        }
        _ => error::INTERNAL_ERROR,
    };
    RpcError {
        code,
        message: err.to_string(),
        data: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    use std::pin::Pin;
    use std::sync::Arc;
    use async_trait::async_trait;
    use futures::stream;
    use meerkat::AgentBuildConfig;
    use meerkat_client::{LlmClient, LlmError};
    use meerkat_core::StopReason;

    // -----------------------------------------------------------------------
    // Mock LLM client
    // -----------------------------------------------------------------------

    struct MockLlmClient;

    #[async_trait]
    impl LlmClient for MockLlmClient {
        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<
                dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(stream::iter(vec![
                Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "Hello from mock".to_string(),
                    meta: None,
                }),
                Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: StopReason::EndTurn,
                    },
                }),
            ]))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    /// A mock LLM client that introduces a delay before responding.
    /// Used to test concurrent access (SESSION_BUSY).
    struct SlowMockLlmClient {
        delay_ms: u64,
    }

    impl SlowMockLlmClient {
        fn new(delay_ms: u64) -> Self {
            Self { delay_ms }
        }
    }

    #[async_trait]
    impl LlmClient for SlowMockLlmClient {
        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<
                dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>>
                    + Send
                    + 'a,
            >,
        > {
            let delay_ms = self.delay_ms;
            Box::pin(async_stream::stream! {
                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                yield Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "Slow response".to_string(),
                    meta: None,
                });
                yield Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: StopReason::EndTurn,
                    },
                });
            })
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn temp_factory(temp: &tempfile::TempDir) -> AgentFactory {
        AgentFactory::new(temp.path().join("sessions"))
    }

    fn mock_build_config() -> AgentBuildConfig {
        AgentBuildConfig {
            llm_client_override: Some(Arc::new(MockLlmClient)),
            ..AgentBuildConfig::new("claude-sonnet-4-5")
        }
    }

    fn slow_build_config(delay_ms: u64) -> AgentBuildConfig {
        AgentBuildConfig {
            llm_client_override: Some(Arc::new(SlowMockLlmClient::new(delay_ms))),
            ..AgentBuildConfig::new("claude-sonnet-4-5")
        }
    }

    fn make_runtime(factory: AgentFactory, max_sessions: usize) -> SessionRuntime {
        SessionRuntime::new(factory, Config::default(), max_sessions)
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    /// 1. Creating a session returns a SessionId and the state is Idle.
    #[tokio::test]
    async fn create_session_returns_id_and_idle_state() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime.create_session(mock_build_config()).await.unwrap();

        let state = runtime.session_state(&session_id).await;
        assert_eq!(state, Some(SessionState::Idle));
    }

    /// 2. Starting a turn returns a RunResult with expected text.
    #[tokio::test]
    async fn start_turn_returns_run_result() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime.create_session(mock_build_config()).await.unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);

        let result = runtime
            .start_turn(&session_id, "Hello".to_string(), event_tx)
            .await
            .unwrap();

        assert!(
            result.text.contains("Hello from mock"),
            "Expected mock response text, got: {}",
            result.text
        );
    }

    /// 3. Verify state transitions: Idle -> Running -> Idle during a turn.
    #[tokio::test]
    async fn start_turn_transitions_idle_running_idle() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        // Use a slow mock to give us time to observe Running state
        let session_id = runtime
            .create_session(slow_build_config(100))
            .await
            .unwrap();

        // Verify initial state
        assert_eq!(
            runtime.session_state(&session_id).await,
            Some(SessionState::Idle)
        );

        // Start the turn in a background task so we can observe state mid-run
        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_clone = runtime.clone();
        let sid_clone = session_id.clone();

        let turn_handle = tokio::spawn(async move {
            runtime_clone
                .start_turn(&sid_clone, "Hello".to_string(), event_tx)
                .await
        });

        // Give the session task a moment to transition to Running
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

        let state = runtime.session_state(&session_id).await;
        assert_eq!(
            state,
            Some(SessionState::Running),
            "Session should be Running during a turn"
        );

        // Wait for the turn to complete
        let result = turn_handle.await.unwrap().unwrap();
        assert!(result.text.contains("Slow response"));

        // After completion, state should be Idle again
        let state = runtime.session_state(&session_id).await;
        assert_eq!(
            state,
            Some(SessionState::Idle),
            "Session should be Idle after turn completes"
        );
    }

    /// 4. Starting a second turn while one is running fails with SESSION_BUSY.
    #[tokio::test]
    async fn start_turn_on_busy_session_fails() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        let session_id = runtime
            .create_session(slow_build_config(200))
            .await
            .unwrap();

        // Start first turn in background
        let (event_tx1, _rx1) = mpsc::channel(100);
        let runtime_clone = runtime.clone();
        let sid_clone = session_id.clone();
        let _turn_handle = tokio::spawn(async move {
            runtime_clone
                .start_turn(&sid_clone, "First".to_string(), event_tx1)
                .await
        });

        // Wait for the first turn to start running
        tokio::time::sleep(tokio::time::Duration::from_millis(30)).await;

        // Try to start a second turn
        let (event_tx2, _rx2) = mpsc::channel(100);
        let result = runtime
            .start_turn(&session_id, "Second".to_string(), event_tx2)
            .await;

        assert!(result.is_err(), "Second turn should fail");
        let err = result.unwrap_err();
        assert_eq!(
            err.code,
            error::SESSION_BUSY,
            "Error code should be SESSION_BUSY, got: {}",
            err.code
        );
    }

    /// 5. Agent events are forwarded through the per-turn event channel.
    #[tokio::test]
    async fn start_turn_emits_events() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime.create_session(mock_build_config()).await.unwrap();
        let (event_tx, mut event_rx) = mpsc::channel(100);

        let _result = runtime
            .start_turn(&session_id, "Hello".to_string(), event_tx)
            .await
            .unwrap();

        // Collect received events
        let mut events = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            events.push(event);
        }

        // We should have received at least some events (RunStarted, TextDelta, etc.)
        assert!(
            !events.is_empty(),
            "Should have received at least one event"
        );

        // Check that we got a RunStarted event
        let has_run_started = events.iter().any(|e| {
            matches!(e, AgentEvent::RunStarted { .. })
        });
        assert!(has_run_started, "Should have received a RunStarted event");
    }

    /// 6. Interrupting an idle session is a no-op (no error).
    #[tokio::test]
    async fn interrupt_on_idle_is_noop() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime.create_session(mock_build_config()).await.unwrap();

        // Interrupt while idle should succeed without error
        let result = runtime.interrupt(&session_id).await;
        assert!(result.is_ok(), "Interrupt on idle should not fail");

        // State should still be idle
        assert_eq!(
            runtime.session_state(&session_id).await,
            Some(SessionState::Idle)
        );
    }

    /// 7. Archiving a session removes it from the runtime.
    #[tokio::test]
    async fn archive_session_removes_handle() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime.create_session(mock_build_config()).await.unwrap();

        // Verify it exists
        assert!(runtime.session_state(&session_id).await.is_some());

        // Archive it
        runtime.archive_session(&session_id).await.unwrap();

        // Verify it's gone
        assert!(
            runtime.session_state(&session_id).await.is_none(),
            "Archived session should no longer exist"
        );

        // Archiving again should fail
        let result = runtime.archive_session(&session_id).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, error::SESSION_NOT_FOUND);
    }

    /// 8. Max sessions limit is enforced.
    #[tokio::test]
    async fn max_sessions_enforced() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 2);

        // Create two sessions (the max)
        let _s1 = runtime.create_session(mock_build_config()).await.unwrap();
        let _s2 = runtime.create_session(mock_build_config()).await.unwrap();

        // Third should fail
        let result = runtime.create_session(mock_build_config()).await;
        assert!(result.is_err(), "Third session should fail");
        let err = result.unwrap_err();
        assert!(
            err.message.contains("Max sessions"),
            "Error message should mention max sessions, got: {}",
            err.message
        );
    }

    /// 9. session_state returns None for an unknown session ID.
    #[tokio::test]
    async fn session_state_none_for_unknown() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let unknown_id = SessionId::new();
        assert_eq!(runtime.session_state(&unknown_id).await, None);
    }

    /// 10. Shutdown closes all sessions.
    #[tokio::test]
    async fn shutdown_closes_all_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let s1 = runtime.create_session(mock_build_config()).await.unwrap();
        let s2 = runtime.create_session(mock_build_config()).await.unwrap();

        // Verify both exist
        assert!(runtime.session_state(&s1).await.is_some());
        assert!(runtime.session_state(&s2).await.is_some());

        // Shutdown
        runtime.shutdown().await;

        // Both should be gone from the sessions map
        let sessions = runtime.list_sessions().await;
        assert!(
            sessions.is_empty(),
            "All sessions should be removed after shutdown"
        );
    }
}
