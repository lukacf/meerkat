//! Contract tests for EphemeralSessionService.
//!
//! These tests verify the SessionService contract using a mock agent builder.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use async_trait::async_trait;
use futures::StreamExt;
use meerkat_core::error::ToolError;
use meerkat_core::event::AgentEvent;
use meerkat_core::service::{
    CreateSessionRequest, InitialTurnPolicy, SessionError, SessionQuery, SessionService,
    StartTurnRequest, TurnToolOverlay,
};
use meerkat_core::types::{
    AssistantBlock, RunResult, SessionId, StopReason, ToolCallView, ToolDef, ToolResult, Usage,
};
use meerkat_core::{
    Agent, AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, LlmStreamResult,
};
use meerkat_session::ephemeral::SessionSnapshot;
use meerkat_session::{EphemeralSessionService, SessionAgent, SessionAgentBuilder};
use serde_json::{Value, json};
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
    fail_overlay_clear: bool,
    overlay_updates: Arc<std::sync::Mutex<Vec<Option<TurnToolOverlay>>>>,
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
            skill_diagnostics: None,
        })
    }

    async fn run_host_mode(
        &mut self,
        prompt: String,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        // Mock host mode delegates to regular run for testing purposes.
        // Session task forwards events from the shared internal channel.
        let (event_tx, _event_rx) = mpsc::channel(16);
        self.run_with_events(prompt, event_tx).await
    }

    fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {
        // No-op for mock
    }

    fn set_flow_tool_overlay(
        &mut self,
        overlay: Option<TurnToolOverlay>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        if overlay.is_none() && self.fail_overlay_clear {
            return Err(meerkat_core::error::AgentError::InternalError(
                "simulated flow overlay clear failure".to_string(),
            ));
        }
        self.overlay_updates
            .lock()
            .expect("overlay updates lock poisoned")
            .push(overlay);
        Ok(())
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
    fail_overlay_clear: bool,
    overlay_updates: Arc<std::sync::Mutex<Vec<Option<TurnToolOverlay>>>>,
}

impl MockAgentBuilder {
    fn new() -> Self {
        Self {
            delay_ms: None,
            build_delay_ms: None,
            fail_overlay_clear: false,
            overlay_updates: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn with_delay(delay_ms: u64) -> Self {
        Self {
            delay_ms: Some(delay_ms),
            build_delay_ms: None,
            fail_overlay_clear: false,
            overlay_updates: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn with_build_delay(build_delay_ms: u64) -> Self {
        Self {
            delay_ms: None,
            build_delay_ms: Some(build_delay_ms),
            fail_overlay_clear: false,
            overlay_updates: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn with_overlay_clear_failure() -> Self {
        Self {
            delay_ms: None,
            build_delay_ms: None,
            fail_overlay_clear: true,
            overlay_updates: Arc::new(std::sync::Mutex::new(Vec::new())),
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
            fail_overlay_clear: self.fail_overlay_clear,
            overlay_updates: self.overlay_updates.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Real agent fixtures (runtime boundary assertions)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct NoopSessionStore;

#[async_trait]
impl AgentSessionStore for NoopSessionStore {
    async fn save(
        &self,
        _session: &meerkat_core::Session,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Ok(())
    }

    async fn load(
        &self,
        _id: &str,
    ) -> Result<Option<meerkat_core::Session>, meerkat_core::error::AgentError> {
        Ok(None)
    }
}

struct StaticToolDispatcher {
    tools: Arc<[Arc<ToolDef>]>,
}

impl StaticToolDispatcher {
    fn new(tool_names: &[&str]) -> Self {
        let defs: Vec<Arc<ToolDef>> = tool_names
            .iter()
            .map(|name| {
                Arc::new(ToolDef {
                    name: (*name).to_string(),
                    description: format!("{name} tool"),
                    input_schema: json!({
                        "type": "object",
                        "properties": {},
                    }),
                })
            })
            .collect();
        Self { tools: defs.into() }
    }
}

#[async_trait]
impl AgentToolDispatcher for StaticToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        Err(ToolError::not_found(call.name))
    }
}

struct RecordingLlmClient {
    provider_visible_tools: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
}

#[async_trait]
impl AgentLlmClient for RecordingLlmClient {
    async fn stream_response(
        &self,
        _messages: &[meerkat_core::types::Message],
        tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&Value>,
    ) -> Result<LlmStreamResult, meerkat_core::error::AgentError> {
        let names = tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
        self.provider_visible_tools
            .lock()
            .expect("provider_visible_tools lock poisoned")
            .push(names);

        Ok(LlmStreamResult::new(
            vec![AssistantBlock::Text {
                text: "ok".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
            Usage::default(),
        ))
    }

    fn provider(&self) -> &'static str {
        "recording-mock"
    }
}

type RealInnerAgent = Agent<RecordingLlmClient, StaticToolDispatcher, NoopSessionStore>;

struct RealSessionAgent {
    agent: RealInnerAgent,
}

#[async_trait]
impl SessionAgent for RealSessionAgent {
    async fn run_with_events(
        &mut self,
        prompt: String,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        self.agent.run_with_events(prompt, event_tx).await
    }

    async fn run_host_mode(
        &mut self,
        prompt: String,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        self.agent.run_host_mode(prompt).await
    }

    fn set_skill_references(&mut self, refs: Option<Vec<meerkat_core::skills::SkillKey>>) {
        self.agent.pending_skill_references = refs;
    }

    fn set_flow_tool_overlay(
        &mut self,
        overlay: Option<TurnToolOverlay>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.agent
            .set_flow_tool_overlay(overlay)
            .map_err(|error| meerkat_core::error::AgentError::ConfigError(error.to_string()))
    }

    fn cancel(&mut self) {
        self.agent.cancel();
    }

    fn session_id(&self) -> SessionId {
        self.agent.session().id().clone()
    }

    fn snapshot(&self) -> SessionSnapshot {
        let session = self.agent.session();
        SessionSnapshot {
            created_at: session.created_at(),
            updated_at: session.updated_at(),
            message_count: session.messages().len(),
            total_tokens: session.total_tokens(),
            usage: session.total_usage(),
            last_assistant_text: session.last_assistant_text(),
        }
    }

    fn session_clone(&self) -> meerkat_core::Session {
        self.agent.session().clone()
    }
}

struct RealAgentBuilder {
    provider_visible_tools: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
}

#[async_trait]
impl SessionAgentBuilder for RealAgentBuilder {
    type Agent = RealSessionAgent;

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RealSessionAgent, SessionError> {
        let mut builder = AgentBuilder::new().model(req.model.clone());
        if let Some(max_tokens) = req.max_tokens {
            builder = builder.max_tokens_per_turn(max_tokens);
        }
        if let Some(system_prompt) = &req.system_prompt {
            builder = builder.system_prompt(system_prompt.clone());
        }

        let client = Arc::new(RecordingLlmClient {
            provider_visible_tools: Arc::clone(&self.provider_visible_tools),
        });
        let tools = Arc::new(StaticToolDispatcher::new(&["alpha", "beta"]));
        let store = Arc::new(NoopSessionStore);
        let agent = builder.build(client, tools, store).await;

        Ok(RealSessionAgent { agent })
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
        skill_references: None,
        initial_turn: InitialTurnPolicy::RunImmediately,
        build: None,
    }
}

fn create_req_deferred(prompt: &str) -> CreateSessionRequest {
    CreateSessionRequest {
        initial_turn: InitialTurnPolicy::Defer,
        ..create_req(prompt)
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
async fn test_create_session_can_defer_initial_turn() {
    let service = make_service(MockAgentBuilder::new());
    let result = service
        .create_session(create_req_deferred("defer first turn"))
        .await
        .expect("create_session should register deferred session");

    assert_eq!(result.text, "");
    assert_eq!(result.turns, 0);
    assert_eq!(result.tool_calls, 0);

    let sessions = service
        .list(SessionQuery::default())
        .await
        .expect("list sessions");
    assert_eq!(sessions.len(), 1);
    let session_id = sessions[0].session_id.clone();

    let view = service
        .read(&session_id)
        .await
        .expect("read deferred session");
    assert_eq!(view.state.message_count, 0);
    assert!(!view.state.is_active);

    let started = service
        .start_turn(
            &session_id,
            StartTurnRequest {
                host_mode: false,
                skill_references: None,
                flow_tool_overlay: None,
                prompt: "now run".to_string(),
                event_tx: None,
            },
        )
        .await
        .expect("start_turn should run after deferred create");
    assert!(started.text.contains("Hello from mock"));
}

#[tokio::test]
async fn test_subscribe_session_events_available_before_first_turn() {
    let service = make_service(MockAgentBuilder::new());
    let created = service
        .create_session(create_req_deferred("defer stream"))
        .await
        .expect("create deferred session");
    let sid = created.session_id;

    let mut stream = service
        .subscribe_session_events(&sid)
        .await
        .expect("session stream should attach immediately after registration");

    service
        .start_turn(
            &sid,
            StartTurnRequest {
                host_mode: false,
                skill_references: None,
                flow_tool_overlay: None,
                prompt: "trigger".to_string(),
                event_tx: None,
            },
        )
        .await
        .expect("start turn");

    let first = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
        .await
        .expect("timed out waiting for session event")
        .expect("stream closed unexpectedly");
    assert!(
        matches!(
            first.payload,
            AgentEvent::RunStarted { .. } | AgentEvent::RunCompleted { .. }
        ),
        "expected run lifecycle event, got: {first:?}"
    );
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
                skill_references: None,
                flow_tool_overlay: None,
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
    let _ = service
        .create_session(create_req_deferred("Hello"))
        .await
        .unwrap();

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
    let _ = service
        .create_session(create_req_deferred("Hello"))
        .await
        .unwrap();

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
                skill_references: None,
                flow_tool_overlay: None,
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
                    skill_references: None,
                    flow_tool_overlay: None,
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
                skill_references: None,
                flow_tool_overlay: None,
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
                    skill_references: None,
                    flow_tool_overlay: None,
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
async fn test_flow_tool_overlay_is_cleared_after_canceled_turn() {
    let overlay_updates = Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = Arc::new(EphemeralSessionService::new(
        MockAgentBuilder {
            delay_ms: Some(500),
            build_delay_ms: None,
            fail_overlay_clear: false,
            overlay_updates: overlay_updates.clone(),
        },
        10,
    ));

    let _ = service.create_session(create_req("Hello")).await.unwrap();
    let session_id = service.list(SessionQuery::default()).await.unwrap()[0]
        .session_id
        .clone();
    overlay_updates
        .lock()
        .expect("overlay updates lock poisoned")
        .clear();

    let service_clone = service.clone();
    let sid_clone = session_id.clone();
    let overlay = TurnToolOverlay {
        allowed_tools: Some(vec!["alpha".to_string()]),
        blocked_tools: Some(vec!["beta".to_string()]),
    };
    let turn = tokio::spawn(async move {
        service_clone
            .start_turn(
                &sid_clone,
                StartTurnRequest {
                    host_mode: false,
                    skill_references: None,
                    flow_tool_overlay: Some(overlay),
                    prompt: "Slow with overlay".to_string(),
                    event_tx: None,
                },
            )
            .await
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    service.interrupt(&session_id).await.expect("interrupt");
    let result = turn.await.unwrap();
    assert!(result.is_err(), "interrupted turn should return an error");

    let updates = overlay_updates
        .lock()
        .expect("overlay updates lock poisoned")
        .clone();
    assert!(updates.contains(&Some(TurnToolOverlay {
        allowed_tools: Some(vec!["alpha".to_string()]),
        blocked_tools: Some(vec!["beta".to_string()]),
    })));
    assert_eq!(updates.last().cloned(), Some(None));
}

#[tokio::test]
async fn test_flow_tool_overlay_enforced_by_runtime_and_resets_next_turn() {
    let provider_visible_tools = Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
    let service = Arc::new(EphemeralSessionService::new(
        RealAgentBuilder {
            provider_visible_tools: Arc::clone(&provider_visible_tools),
        },
        10,
    ));

    let _ = service
        .create_session(create_req_deferred("runtime tool scope"))
        .await
        .expect("create deferred session");
    let session_id = service
        .list(SessionQuery::default())
        .await
        .expect("list sessions")[0]
        .session_id
        .clone();

    service
        .start_turn(
            &session_id,
            StartTurnRequest {
                host_mode: false,
                skill_references: None,
                flow_tool_overlay: Some(TurnToolOverlay {
                    allowed_tools: Some(vec!["alpha".to_string(), "beta".to_string()]),
                    blocked_tools: Some(vec!["beta".to_string()]),
                }),
                prompt: "overlayed turn".to_string(),
                event_tx: None,
            },
        )
        .await
        .expect("turn with overlay should run");

    service
        .start_turn(
            &session_id,
            StartTurnRequest {
                host_mode: false,
                skill_references: None,
                flow_tool_overlay: None,
                prompt: "baseline turn".to_string(),
                event_tx: None,
            },
        )
        .await
        .expect("turn without overlay should run");

    let calls = provider_visible_tools
        .lock()
        .expect("provider_visible_tools lock poisoned")
        .clone();
    assert_eq!(calls.len(), 2, "expected one provider call per turn");
    assert_eq!(
        calls[0],
        vec!["alpha".to_string()],
        "overlayed turn must include allowed alpha and exclude blocked beta"
    );
    assert_eq!(
        calls[1],
        vec!["alpha".to_string(), "beta".to_string()],
        "next turn without overlay must restore baseline visibility"
    );
}

#[tokio::test]
async fn test_start_turn_returns_error_when_overlay_clear_fails() {
    let overlay_updates = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut builder = MockAgentBuilder::with_overlay_clear_failure();
    builder.overlay_updates = overlay_updates.clone();
    let service = Arc::new(EphemeralSessionService::new(builder, 10));

    let _ = service
        .create_session(create_req_deferred("Hello"))
        .await
        .unwrap();
    let session_id = service.list(SessionQuery::default()).await.unwrap()[0]
        .session_id
        .clone();
    overlay_updates
        .lock()
        .expect("overlay updates lock poisoned")
        .clear();

    let result = service
        .start_turn(
            &session_id,
            StartTurnRequest {
                host_mode: false,
                skill_references: None,
                flow_tool_overlay: Some(TurnToolOverlay {
                    allowed_tools: Some(vec!["alpha".to_string()]),
                    blocked_tools: None,
                }),
                prompt: "overlay clear fails".to_string(),
                event_tx: None,
            },
        )
        .await;

    assert!(result.is_err(), "clear failure must fail closed");
    let err = result.expect_err("expected overlay clear failure");
    assert_eq!(err.code(), "AGENT_ERROR");

    let updates = overlay_updates
        .lock()
        .expect("overlay updates lock poisoned")
        .clone();
    assert_eq!(
        updates,
        vec![Some(TurnToolOverlay {
            allowed_tools: Some(vec!["alpha".to_string()]),
            blocked_tools: None,
        })]
    );
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

#[tokio::test]
async fn test_interrupt_host_mode_returns_without_waiting_for_ack() {
    let service = Arc::new(EphemeralSessionService::new(
        MockAgentBuilder::with_delay(600),
        10,
    ));
    let _ = service.create_session(create_req("Hello")).await.unwrap();

    let sessions = service.list(SessionQuery::default()).await.unwrap();
    let session_id = sessions[0].session_id.clone();

    let service_clone = service.clone();
    let sid_clone = session_id.clone();
    let host_turn = tokio::spawn(async move {
        service_clone
            .start_turn(
                &sid_clone,
                StartTurnRequest {
                    host_mode: true,
                    skill_references: None,
                    flow_tool_overlay: None,
                    prompt: "host turn".to_string(),
                    event_tx: None,
                },
            )
            .await
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    let started = std::time::Instant::now();
    service.interrupt(&session_id).await.unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < tokio::time::Duration::from_millis(200),
        "interrupt should return promptly for host_mode turns (elapsed={elapsed:?})"
    );

    let _ = host_turn.await.unwrap();
}
