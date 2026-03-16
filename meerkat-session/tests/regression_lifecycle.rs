//! Regression tests for EphemeralSessionService lifecycle invariants.
//!
//! These tests lock down create/turn/read/archive/interrupt/event/label
//! semantics using mock agents. Deterministic — no API keys, no `#[ignore]`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use async_trait::async_trait;
use futures::StreamExt;
use meerkat_core::event::AgentEvent;
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextStatus, CreateSessionRequest, InitialTurnPolicy,
    SessionError, SessionQuery, SessionService, SessionServiceControlExt, StartTurnRequest,
    TurnToolOverlay,
};
use meerkat_core::types::{RunResult, SessionId, Usage};
use meerkat_session::ephemeral::SessionSnapshot;
use meerkat_session::{EphemeralSessionService, SessionAgent, SessionAgentBuilder};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Mock agent (copied from ephemeral_contract.rs pattern)
// ---------------------------------------------------------------------------

struct MockAgent {
    session_id: SessionId,
    message_count: usize,
    delay_ms: Option<u64>,
    should_fail: bool,
    total_input_tokens: u64,
    total_output_tokens: u64,
    system_context_state: Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>,
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

        if self.should_fail {
            let _ = event_tx
                .send(AgentEvent::RunFailed {
                    session_id: self.session_id.clone(),
                    error: "simulated failure".to_string(),
                })
                .await;
            return Err(meerkat_core::error::AgentError::InternalError(
                "simulated agent failure".to_string(),
            ));
        }

        let _ = event_tx
            .send(AgentEvent::RunStarted {
                session_id: self.session_id.clone(),
                prompt: "test".to_string(),
            })
            .await;

        self.message_count += 2; // user + assistant
        self.total_input_tokens += 10;
        self.total_output_tokens += 5;

        let usage = Usage {
            input_tokens: 10,
            output_tokens: 5,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        };

        let _ = event_tx
            .send(AgentEvent::RunCompleted {
                session_id: self.session_id.clone(),
                result: "Hello from mock".to_string(),
                usage: usage.clone(),
            })
            .await;

        Ok(RunResult {
            text: "Hello from mock".to_string(),
            session_id: self.session_id.clone(),
            usage,
            turns: 1,
            tool_calls: 0,
            structured_output: None,
            schema_warnings: None,
            skill_diagnostics: None,
        })
    }

    async fn run_host_mode(
        &mut self,
        prompt: String,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        let (event_tx, _event_rx) = mpsc::channel(16);
        self.run_with_events(prompt, event_tx).await
    }

    fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

    fn set_flow_tool_overlay(
        &mut self,
        _overlay: Option<TurnToolOverlay>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Ok(())
    }

    fn cancel(&mut self) {}

    fn session_id(&self) -> SessionId {
        self.session_id.clone()
    }

    fn snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            created_at: SystemTime::now(),
            updated_at: SystemTime::now(),
            message_count: self.message_count,
            total_tokens: self.total_input_tokens + self.total_output_tokens,
            usage: Usage {
                input_tokens: self.total_input_tokens,
                output_tokens: self.total_output_tokens,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
            last_assistant_text: Some("Hello from mock".to_string()),
        }
    }

    fn session_clone(&self) -> meerkat_core::Session {
        let mut session = meerkat_core::Session::with_id(self.session_id.clone());
        session
            .set_system_context_state(
                self.system_context_state
                    .lock()
                    .expect("system-context lock poisoned")
                    .clone(),
            )
            .expect("serialize system-context state");
        session
    }

    fn apply_runtime_system_context(
        &mut self,
        appends: &[meerkat_core::PendingSystemContextAppend],
    ) {
        let mut session = self.session_clone();
        session.append_system_context_blocks(appends);
        self.message_count = session.messages().len();
    }

    fn system_context_state(
        &self,
    ) -> Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>> {
        Arc::clone(&self.system_context_state)
    }
}

// ---------------------------------------------------------------------------
// Mock builders
// ---------------------------------------------------------------------------

struct MockAgentBuilder;

#[async_trait]
impl SessionAgentBuilder for MockAgentBuilder {
    type Agent = MockAgent;

    async fn build_agent(
        &self,
        _req: &CreateSessionRequest,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<MockAgent, SessionError> {
        Ok(MockAgent {
            session_id: SessionId::new(),
            message_count: 0,
            delay_ms: None,
            should_fail: false,
            total_input_tokens: 0,
            total_output_tokens: 0,
            system_context_state: Arc::new(std::sync::Mutex::new(Default::default())),
        })
    }
}

/// Builder that produces agents with a configurable delay per turn.
struct SlowMockAgentBuilder {
    delay_ms: u64,
}

#[async_trait]
impl SessionAgentBuilder for SlowMockAgentBuilder {
    type Agent = MockAgent;

    async fn build_agent(
        &self,
        _req: &CreateSessionRequest,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<MockAgent, SessionError> {
        Ok(MockAgent {
            session_id: SessionId::new(),
            message_count: 0,
            delay_ms: Some(self.delay_ms),
            should_fail: false,
            total_input_tokens: 0,
            total_output_tokens: 0,
            system_context_state: Arc::new(std::sync::Mutex::new(Default::default())),
        })
    }
}

/// Builder that produces agents which always error on run.
struct FailingMockAgentBuilder;

#[async_trait]
impl SessionAgentBuilder for FailingMockAgentBuilder {
    type Agent = MockAgent;

    async fn build_agent(
        &self,
        _req: &CreateSessionRequest,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<MockAgent, SessionError> {
        Ok(MockAgent {
            session_id: SessionId::new(),
            message_count: 0,
            delay_ms: None,
            should_fail: true,
            total_input_tokens: 0,
            total_output_tokens: 0,
            system_context_state: Arc::new(std::sync::Mutex::new(Default::default())),
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_service(builder: MockAgentBuilder) -> Arc<EphemeralSessionService<MockAgentBuilder>> {
    Arc::new(EphemeralSessionService::new(builder, 10))
}

fn make_slow_service(delay_ms: u64) -> Arc<EphemeralSessionService<SlowMockAgentBuilder>> {
    Arc::new(EphemeralSessionService::new(
        SlowMockAgentBuilder { delay_ms },
        10,
    ))
}

fn make_failing_service() -> Arc<EphemeralSessionService<FailingMockAgentBuilder>> {
    Arc::new(EphemeralSessionService::new(FailingMockAgentBuilder, 10))
}

fn create_req(prompt: &str) -> CreateSessionRequest {
    CreateSessionRequest {
        model: "mock".to_string(),
        prompt: prompt.to_string().into(),
        system_prompt: None,
        max_tokens: None,
        event_tx: None,
        host_mode: false,
        skill_references: None,
        initial_turn: InitialTurnPolicy::RunImmediately,
        build: None,
        labels: None,
    }
}

fn create_req_deferred(prompt: &str) -> CreateSessionRequest {
    CreateSessionRequest {
        initial_turn: InitialTurnPolicy::Defer,
        ..create_req(prompt)
    }
}

fn create_req_with_labels(prompt: &str, labels: BTreeMap<String, String>) -> CreateSessionRequest {
    CreateSessionRequest {
        labels: Some(labels),
        ..create_req(prompt)
    }
}

fn turn_req(prompt: &str) -> StartTurnRequest {
    StartTurnRequest {
        prompt: prompt.to_string().into(),
        event_tx: None,
        host_mode: false,
        skill_references: None,
        flow_tool_overlay: None,
        additional_instructions: None,
    }
}

// ---------------------------------------------------------------------------
// 1. Full lifecycle: create -> turn -> turn -> read -> archive -> list -> read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_lifecycle_create_turns_read_archive_gone() {
    let service = make_service(MockAgentBuilder);

    // Create session (runs first turn)
    let r1 = service.create_session(create_req("Hello")).await.unwrap();
    let sid = r1.session_id.clone();

    // Second turn
    let _r2 = service
        .start_turn(&sid, turn_req("Follow up"))
        .await
        .unwrap();

    // Read — at least 4 messages (2 turns * 2 messages each)
    let view = service.read(&sid).await.unwrap();
    assert!(
        view.state.message_count >= 4,
        "expected >= 4 messages after 2 turns, got {}",
        view.state.message_count
    );

    // Archive
    service.archive(&sid).await.unwrap();

    // List should be empty (active sessions)
    let sessions = service.list(SessionQuery::default()).await.unwrap();
    assert!(
        sessions.is_empty(),
        "archived session should not appear in list"
    );

    // Read after archive still works (returns archived view)
    let archived_view = service.read(&sid).await;
    assert!(
        archived_view.is_ok(),
        "read after archive should return the archived view"
    );
    let archived_view = archived_view.unwrap();
    assert!(!archived_view.state.is_active);
}

// ---------------------------------------------------------------------------
// 2. Read returns all SessionView fields
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_returns_all_session_view_fields() {
    let service = make_service(MockAgentBuilder);

    let result = service
        .create_session(create_req("Check fields"))
        .await
        .unwrap();
    let sid = result.session_id.clone();

    let view = service.read(&sid).await.unwrap();

    // SessionInfo fields
    assert_eq!(view.state.session_id, sid);
    assert!(view.state.created_at <= SystemTime::now());
    assert!(view.state.updated_at <= SystemTime::now());
    assert!(
        view.state.message_count > 0,
        "message_count should be > 0 after a turn"
    );
    assert!(
        !view.state.is_active,
        "session should be idle after turn completes"
    );
    assert!(
        view.state.last_assistant_text.is_some(),
        "last_assistant_text should be set after a turn"
    );

    // SessionUsage fields
    assert!(view.billing.total_tokens > 0, "total_tokens should be > 0");
    assert!(
        view.billing.usage.input_tokens > 0,
        "input_tokens should be > 0"
    );
    assert!(
        view.billing.usage.output_tokens > 0,
        "output_tokens should be > 0"
    );
}

// ---------------------------------------------------------------------------
// 3. Usage accumulates across turns
// ---------------------------------------------------------------------------

#[tokio::test]
async fn usage_accumulates_across_turns() {
    let service = make_service(MockAgentBuilder);

    let r1 = service.create_session(create_req("Turn 1")).await.unwrap();
    let sid = r1.session_id.clone();

    let view1 = service.read(&sid).await.unwrap();
    let tokens_after_1 = view1.billing.total_tokens;
    assert!(tokens_after_1 > 0, "should have tokens after first turn");

    // Second turn
    let _r2 = service.start_turn(&sid, turn_req("Turn 2")).await.unwrap();

    let view2 = service.read(&sid).await.unwrap();
    let tokens_after_2 = view2.billing.total_tokens;
    assert!(
        tokens_after_2 > tokens_after_1,
        "tokens should accumulate: {tokens_after_2} should be > {tokens_after_1}"
    );
}

// ---------------------------------------------------------------------------
// 4. Interrupt during slow turn returns promptly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn interrupt_during_slow_turn_returns_promptly() {
    let service = make_slow_service(500);

    // Defer initial turn so we can control the slow turn
    let created = service
        .create_session(CreateSessionRequest {
            initial_turn: InitialTurnPolicy::Defer,
            model: "mock".to_string(),
            prompt: "slow".to_string().into(),
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            host_mode: false,
            skill_references: None,
            build: None,
            labels: None,
        })
        .await
        .unwrap();
    let sid = created.session_id;

    // Start the slow turn in the background
    let svc = service.clone();
    let sid2 = sid.clone();
    let turn_handle =
        tokio::spawn(async move { svc.start_turn(&sid2, turn_req("Slow turn")).await });

    // Give the turn time to start
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Interrupt and measure how quickly it returns
    let start = tokio::time::Instant::now();
    let interrupt_result = service.interrupt(&sid).await;
    let elapsed = start.elapsed();

    assert!(interrupt_result.is_ok(), "interrupt should succeed");
    assert!(
        elapsed < tokio::time::Duration::from_millis(200),
        "interrupt should return promptly, took {elapsed:?}"
    );

    // The turn result should be an error (cancelled)
    let turn_result = turn_handle.await.unwrap();
    assert!(turn_result.is_err(), "interrupted turn should return error");
}

// ---------------------------------------------------------------------------
// 5. Interrupt idle session returns NotRunning
// ---------------------------------------------------------------------------

#[tokio::test]
async fn interrupt_idle_session_returns_not_running() {
    let service = make_service(MockAgentBuilder);

    let result = service
        .create_session(create_req("Idle test"))
        .await
        .unwrap();
    let sid = result.session_id;

    // Session is idle now — interrupt should fail
    let err = service.interrupt(&sid).await.unwrap_err();
    assert_eq!(err.code(), "SESSION_NOT_RUNNING");
}

// ---------------------------------------------------------------------------
// 6. Interrupt host-mode session returns without blocking
// ---------------------------------------------------------------------------

#[tokio::test]
async fn interrupt_host_mode_returns_without_blocking() {
    let service = make_slow_service(600);

    // Create with deferred turn so we can start host_mode turn manually
    let created = service
        .create_session(CreateSessionRequest {
            initial_turn: InitialTurnPolicy::Defer,
            model: "mock".to_string(),
            prompt: "host".to_string().into(),
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            host_mode: false,
            skill_references: None,
            build: None,
            labels: None,
        })
        .await
        .unwrap();
    let sid = created.session_id;

    // Start a host-mode turn in the background
    let svc = service.clone();
    let sid2 = sid.clone();
    let _turn_handle = tokio::spawn(async move {
        svc.start_turn(
            &sid2,
            StartTurnRequest {
                prompt: "Host mode turn".to_string().into(),
                event_tx: None,
                host_mode: true,
                skill_references: None,
                flow_tool_overlay: None,
                additional_instructions: None,
            },
        )
        .await
    });

    // Give the turn time to start
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Interrupt should return promptly
    let start = tokio::time::Instant::now();
    let interrupt_result = service.interrupt(&sid).await;
    let elapsed = start.elapsed();

    assert!(interrupt_result.is_ok(), "interrupt should succeed");
    assert!(
        elapsed < tokio::time::Duration::from_millis(200),
        "interrupt should return promptly for host-mode, took {elapsed:?}"
    );
}

// ---------------------------------------------------------------------------
// 7. Concurrent turn on busy session returns Busy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn concurrent_turn_on_busy_session_returns_busy() {
    let service = make_slow_service(200);

    let created = service
        .create_session(CreateSessionRequest {
            initial_turn: InitialTurnPolicy::Defer,
            model: "mock".to_string(),
            prompt: "busy".to_string().into(),
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            host_mode: false,
            skill_references: None,
            build: None,
            labels: None,
        })
        .await
        .unwrap();
    let sid = created.session_id;

    // Start a slow turn
    let svc = service.clone();
    let sid2 = sid.clone();
    let _handle = tokio::spawn(async move { svc.start_turn(&sid2, turn_req("Slow")).await });

    // Give the turn time to start
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Second turn should be rejected
    let result = service.start_turn(&sid, turn_req("Fast")).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code(), "SESSION_BUSY");
}

// ---------------------------------------------------------------------------
// 8. Event stream emits RunStarted and RunCompleted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn event_stream_emits_run_started_and_completed() {
    let service = make_service(MockAgentBuilder);

    // Create deferred so we can subscribe before the turn
    let created = service
        .create_session(create_req_deferred("event test"))
        .await
        .unwrap();
    let sid = created.session_id;

    // Subscribe to events
    let mut stream = service
        .subscribe_session_events(&sid)
        .await
        .expect("should subscribe to session events");

    // Run a turn
    service
        .start_turn(&sid, turn_req("trigger events"))
        .await
        .unwrap();

    // Collect events with a timeout
    let mut events = Vec::new();
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(2);
    while let Ok(Some(envelope)) = tokio::time::timeout_at(deadline, stream.next()).await {
        events.push(envelope);
    }

    let has_run_started = events
        .iter()
        .any(|e| matches!(e.payload, AgentEvent::RunStarted { .. }));
    let has_run_completed = events
        .iter()
        .any(|e| matches!(e.payload, AgentEvent::RunCompleted { .. }));

    assert!(
        has_run_started,
        "events should contain RunStarted: {events:?}"
    );
    assert!(
        has_run_completed,
        "events should contain RunCompleted: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// 9. Event ordering: RunStarted before RunCompleted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn event_ordering_run_started_before_completed() {
    let service = make_service(MockAgentBuilder);

    let created = service
        .create_session(create_req_deferred("order test"))
        .await
        .unwrap();
    let sid = created.session_id;

    let mut stream = service
        .subscribe_session_events(&sid)
        .await
        .expect("should subscribe to session events");

    service.start_turn(&sid, turn_req("trigger")).await.unwrap();

    let mut events = Vec::new();
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(2);
    while let Ok(Some(envelope)) = tokio::time::timeout_at(deadline, stream.next()).await {
        events.push(envelope);
    }

    let started_idx = events
        .iter()
        .position(|e| matches!(e.payload, AgentEvent::RunStarted { .. }));
    let completed_idx = events
        .iter()
        .position(|e| matches!(e.payload, AgentEvent::RunCompleted { .. }));

    assert!(started_idx.is_some(), "RunStarted should be present");
    assert!(completed_idx.is_some(), "RunCompleted should be present");
    assert!(
        started_idx.unwrap() < completed_idx.unwrap(),
        "RunStarted (idx={}) should come before RunCompleted (idx={})",
        started_idx.unwrap(),
        completed_idx.unwrap()
    );

    // Also verify monotonic seq ordering
    if let (Some(si), Some(ci)) = (started_idx, completed_idx) {
        assert!(
            events[si].seq < events[ci].seq,
            "RunStarted seq ({}) should be < RunCompleted seq ({})",
            events[si].seq,
            events[ci].seq
        );
    }
}

// ---------------------------------------------------------------------------
// 10. Concurrent creates produce distinct sessions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn concurrent_creates_produce_distinct_sessions() {
    let service = Arc::new(EphemeralSessionService::new(MockAgentBuilder, 10));

    let mut handles = Vec::new();
    for i in 0..5 {
        let svc = service.clone();
        handles.push(tokio::spawn(async move {
            svc.create_session(create_req(&format!("concurrent-{i}")))
                .await
        }));
    }

    let mut session_ids = Vec::new();
    for handle in handles {
        let result = handle.await.unwrap().unwrap();
        session_ids.push(result.session_id);
    }

    // All IDs should be distinct
    let unique: std::collections::HashSet<_> = session_ids.iter().collect();
    assert_eq!(
        unique.len(),
        5,
        "all 5 sessions should have distinct IDs, got: {session_ids:?}"
    );

    // List should return all 5
    let sessions = service.list(SessionQuery::default()).await.unwrap();
    assert_eq!(sessions.len(), 5);
}

// ---------------------------------------------------------------------------
// 11. Labels: create and filter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn labels_create_and_filter() {
    let service = make_service(MockAgentBuilder);

    let mut test_labels = BTreeMap::new();
    test_labels.insert("env".to_string(), "test".to_string());
    let _ = service
        .create_session(create_req_with_labels("test session", test_labels))
        .await
        .unwrap();

    let mut prod_labels = BTreeMap::new();
    prod_labels.insert("env".to_string(), "prod".to_string());
    let _ = service
        .create_session(create_req_with_labels("prod session", prod_labels))
        .await
        .unwrap();

    // Filter for env=test
    let mut filter = BTreeMap::new();
    filter.insert("env".to_string(), "test".to_string());
    let results = service
        .list(SessionQuery {
            limit: None,
            offset: None,
            labels: Some(filter),
        })
        .await
        .unwrap();

    assert_eq!(results.len(), 1, "should only return the test session");
    assert_eq!(results[0].labels.get("env").unwrap(), "test");
}

// ---------------------------------------------------------------------------
// 12. Labels preserved in read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn labels_preserved_in_read() {
    let service = make_service(MockAgentBuilder);

    let mut labels = BTreeMap::new();
    labels.insert("team".to_string(), "platform".to_string());
    labels.insert("tier".to_string(), "critical".to_string());

    let result = service
        .create_session(create_req_with_labels("labeled session", labels.clone()))
        .await
        .unwrap();
    let sid = result.session_id;

    let view = service.read(&sid).await.unwrap();
    assert_eq!(
        view.state.labels, labels,
        "labels should be preserved in read"
    );
}

// ---------------------------------------------------------------------------
// 13. Inject context applied when idle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inject_context_applied_when_idle() {
    let service = make_service(MockAgentBuilder);

    let created = service
        .create_session(create_req_deferred("context test"))
        .await
        .unwrap();
    let sid = created.session_id;

    let result = service
        .append_system_context(
            &sid,
            AppendSystemContextRequest {
                text: "New runtime context".to_string(),
                source: Some("test".to_string()),
                idempotency_key: Some("ctx-idle-1".to_string()),
            },
        )
        .await
        .expect("append should succeed on idle session");

    // When idle, context is staged (pending for next turn boundary)
    assert_eq!(
        result.status,
        AppendSystemContextStatus::Staged,
        "context on idle session should be staged"
    );
}

// ---------------------------------------------------------------------------
// 14. Inject context duplicate idempotent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inject_context_duplicate_idempotent() {
    let service = make_service(MockAgentBuilder);

    let created = service
        .create_session(create_req_deferred("dedup test"))
        .await
        .unwrap();
    let sid = created.session_id;

    let req = AppendSystemContextRequest {
        text: "Idempotent context".to_string(),
        source: Some("test".to_string()),
        idempotency_key: Some("ctx-dedup-1".to_string()),
    };

    // First append
    let first = service
        .append_system_context(&sid, req.clone())
        .await
        .expect("first append should succeed");
    assert_eq!(first.status, AppendSystemContextStatus::Staged);

    // Second append with same key and same content
    let second = service
        .append_system_context(&sid, req.clone())
        .await
        .expect("duplicate append should succeed");
    assert_eq!(
        second.status,
        AppendSystemContextStatus::Duplicate,
        "second append with same key should be duplicate"
    );
}

// ---------------------------------------------------------------------------
// 15. Failed turn returns agent error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn failed_turn_returns_agent_error() {
    let service = make_failing_service();

    // Defer so we can control the failing turn
    let created = service
        .create_session(CreateSessionRequest {
            initial_turn: InitialTurnPolicy::Defer,
            model: "mock".to_string(),
            prompt: "will fail".to_string().into(),
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            host_mode: false,
            skill_references: None,
            build: None,
            labels: None,
        })
        .await
        .unwrap();
    let sid = created.session_id;

    let result = service.start_turn(&sid, turn_req("fail")).await;
    assert!(result.is_err(), "failing agent should produce an error");

    let err = result.unwrap_err();
    assert_eq!(
        err.code(),
        "AGENT_ERROR",
        "error code should be AGENT_ERROR, got: {}",
        err.code()
    );
}
