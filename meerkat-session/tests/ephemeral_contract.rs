//! Contract tests for EphemeralSessionService.
//!
//! These tests verify the SessionService contract using a mock agent builder.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use async_trait::async_trait;
use meerkat_core::event::AgentEvent;
use meerkat_core::service::{
    CreateSessionRequest, SessionError, SessionQuery, SessionService, StartTurnRequest,
};
use meerkat_core::types::{RunResult, SessionId, Usage};
use meerkat_session::ephemeral::SessionSnapshot;
use meerkat_session::{EphemeralSessionService, SessionAgent, SessionAgentBuilder};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Mock agent
// ---------------------------------------------------------------------------

struct MockAgent {
    session_id: SessionId,
    message_count: usize,
    delay_ms: Option<u64>,
}

#[async_trait]
impl SessionAgent for MockAgent {
    async fn run_with_events(
        &mut self,
        _prompt: String,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        if let Some(delay) = self.delay_ms {
            tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
        }

        let _ = event_tx
            .send(AgentEvent::RunStarted {
                session_id: self.session_id.clone(),
                prompt: "test".to_string(),
            })
            .await;

        self.message_count += 2; // user + assistant

        Ok(RunResult {
            text: "Hello from mock".to_string(),
            session_id: self.session_id.clone(),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
            turns: 1,
            tool_calls: 0,
            structured_output: None,
            schema_warnings: None,
        })
    }

    async fn run_host_mode_with_events(
        &mut self,
        prompt: String,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        // Mock host mode delegates to regular run for testing purposes
        self.run_with_events(prompt, event_tx).await
    }

    fn cancel(&mut self) {
        // No-op for mock
    }

    fn session_id(&self) -> SessionId {
        self.session_id.clone()
    }

    fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            created_at: SystemTime::now(),
            updated_at: SystemTime::now(),
            message_count: self.message_count,
            total_tokens: 15,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
            last_assistant_text: Some("Hello from mock".to_string()),
        }
    }

    fn session_clone(&self) -> meerkat_core::Session {
        meerkat_core::Session::with_id(self.session_id.clone())
    }
}

struct MockAgentBuilder {
    delay_ms: Option<u64>,
    build_delay_ms: Option<u64>,
}

impl MockAgentBuilder {
    fn new() -> Self {
        Self {
            delay_ms: None,
            build_delay_ms: None,
        }
    }

    fn with_delay(delay_ms: u64) -> Self {
        Self {
            delay_ms: Some(delay_ms),
            build_delay_ms: None,
        }
    }

    fn with_build_delay(build_delay_ms: u64) -> Self {
        Self {
            delay_ms: None,
            build_delay_ms: Some(build_delay_ms),
        }
    }
}

#[async_trait]
impl SessionAgentBuilder for MockAgentBuilder {
    type Agent = MockAgent;

    async fn build_agent(
        &self,
        _req: &CreateSessionRequest,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<MockAgent, SessionError> {
        if let Some(delay) = self.build_delay_ms {
            tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
        }
        Ok(MockAgent {
            session_id: SessionId::new(),
            message_count: 0,
            delay_ms: self.delay_ms,
        })
    }
}

fn make_service(builder: MockAgentBuilder) -> Arc<EphemeralSessionService<MockAgentBuilder>> {
    Arc::new(EphemeralSessionService::new(builder, 10))
}

fn create_req(prompt: &str) -> CreateSessionRequest {
    CreateSessionRequest {
        model: "mock".to_string(),
        prompt: prompt.to_string(),
        system_prompt: None,
        max_tokens: None,
        event_tx: None,
        host_mode: false,
    }
}

// ---------------------------------------------------------------------------
// Contract tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_create_and_run_turn() {
    let service = make_service(MockAgentBuilder::new());
    let result = service.create_session(create_req("Hello")).await.unwrap();
    assert!(result.text.contains("Hello from mock"));
}

#[tokio::test]
async fn test_start_turn_on_existing_session() {
    let service = make_service(MockAgentBuilder::new());
    let result = service.create_session(create_req("Hello")).await.unwrap();
    assert!(result.text.contains("Hello from mock"));

    // List sessions to find the ID
    let sessions = service.list(SessionQuery::default()).await.unwrap();
    assert_eq!(sessions.len(), 1);
    let session_id = sessions[0].session_id.clone();

    // Start another turn
    let result2 = service
        .start_turn(
            &session_id,
            StartTurnRequest {
                host_mode: false,
                prompt: "Follow up".to_string(),
                event_tx: None,
            },
        )
        .await
        .unwrap();
    assert!(result2.text.contains("Hello from mock"));
}

#[tokio::test]
async fn test_read_active_session() {
    let service = make_service(MockAgentBuilder::new());
    let _ = service.create_session(create_req("Hello")).await.unwrap();

    let sessions = service.list(SessionQuery::default()).await.unwrap();
    let session_id = sessions[0].session_id.clone();

    let view = service.read(&session_id).await.unwrap();
    assert_eq!(view.state.session_id, session_id);
    assert!(!view.state.is_active); // Should be idle after turn completes
}

#[tokio::test]
async fn test_list_sessions() {
    let service = make_service(MockAgentBuilder::new());
    let _ = service.create_session(create_req("A")).await.unwrap();
    let _ = service.create_session(create_req("B")).await.unwrap();

    let sessions = service.list(SessionQuery::default()).await.unwrap();
    assert_eq!(sessions.len(), 2);
}

#[tokio::test]
async fn test_create_session_capacity_is_atomic() {
    let service = Arc::new(EphemeralSessionService::new(
        MockAgentBuilder::with_build_delay(100),
        1,
    ));

    let s1 = service.clone();
    let t1 = tokio::spawn(async move { s1.create_session(create_req("A")).await });
    let s2 = service.clone();
    let t2 = tokio::spawn(async move { s2.create_session(create_req("B")).await });

    let r1 = t1.await.unwrap();
    let r2 = t2.await.unwrap();

    let mut ok_count = 0;
    let mut err_count = 0;
    for result in [r1, r2] {
        match result {
            Ok(_) => ok_count += 1,
            Err(err) => {
                assert_eq!(err.code(), "AGENT_ERROR");
                err_count += 1;
            }
        }
    }

    assert_eq!(ok_count, 1);
    assert_eq!(err_count, 1);

    let sessions = service.list(SessionQuery::default()).await.unwrap();
    assert_eq!(sessions.len(), 1);
}

#[tokio::test]
async fn test_archive_session() {
    let service = make_service(MockAgentBuilder::new());
    let _ = service.create_session(create_req("Hello")).await.unwrap();

    let sessions = service.list(SessionQuery::default()).await.unwrap();
    let session_id = sessions[0].session_id.clone();

    // Archive it
    service.archive(&session_id).await.unwrap();

    // Should be gone
    let sessions = service.list(SessionQuery::default()).await.unwrap();
    assert!(sessions.is_empty());
}

#[tokio::test]
async fn test_turn_on_archived_session_returns_not_found() {
    let service = make_service(MockAgentBuilder::new());
    let _ = service.create_session(create_req("Hello")).await.unwrap();

    let sessions = service.list(SessionQuery::default()).await.unwrap();
    let session_id = sessions[0].session_id.clone();

    service.archive(&session_id).await.unwrap();

    let result = service
        .start_turn(
            &session_id,
            StartTurnRequest {
                host_mode: false,
                prompt: "After archive".to_string(),
                event_tx: None,
            },
        )
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code(), "SESSION_NOT_FOUND");
}

#[tokio::test]
async fn test_concurrent_turns_return_busy() {
    let service = Arc::new(EphemeralSessionService::new(
        MockAgentBuilder::with_delay(200),
        10,
    ));

    let _ = service.create_session(create_req("Hello")).await.unwrap();

    let sessions = service.list(SessionQuery::default()).await.unwrap();
    let session_id = sessions[0].session_id.clone();

    // Start a slow turn in the background
    let service_clone = service.clone();
    let sid_clone = session_id.clone();
    let _handle = tokio::spawn(async move {
        service_clone
            .start_turn(
                &sid_clone,
                StartTurnRequest {
                    host_mode: false,
                    prompt: "Slow".to_string(),
                    event_tx: None,
                },
            )
            .await
    });

    // Give the turn time to start running
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Try to start another turn
    let result = service
        .start_turn(
            &session_id,
            StartTurnRequest {
                host_mode: false,
                prompt: "Fast".to_string(),
                event_tx: None,
            },
        )
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code(), "SESSION_BUSY");
}

#[tokio::test]
async fn test_interrupt_cancels_inflight_turn() {
    let service = Arc::new(EphemeralSessionService::new(
        MockAgentBuilder::with_delay(500),
        10,
    ));

    let _ = service.create_session(create_req("Hello")).await.unwrap();

    let sessions = service.list(SessionQuery::default()).await.unwrap();
    let session_id = sessions[0].session_id.clone();

    // Start a slow turn
    let service_clone = service.clone();
    let sid_clone = session_id.clone();
    let _handle = tokio::spawn(async move {
        service_clone
            .start_turn(
                &sid_clone,
                StartTurnRequest {
                    host_mode: false,
                    prompt: "Slow".to_string(),
                    event_tx: None,
                },
            )
            .await
    });

    // Give the turn time to start
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Interrupt should succeed
    let result = service.interrupt(&session_id).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_interrupt_when_idle_returns_not_running() {
    let service = make_service(MockAgentBuilder::new());
    let _ = service.create_session(create_req("Hello")).await.unwrap();

    let sessions = service.list(SessionQuery::default()).await.unwrap();
    let session_id = sessions[0].session_id.clone();

    let result = service.interrupt(&session_id).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code(), "SESSION_NOT_RUNNING");
}
