//! Contract tests for EphemeralSessionService.
//!
//! These tests verify the SessionService contract using a mock agent builder.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use async_trait::async_trait;
use futures::StreamExt;
use meerkat_core::compact::{CompactionContext, CompactionResult, Compactor};
use meerkat_core::event::AgentEvent;
use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::service::{
    AppendSystemContextRequest, AppendSystemContextStatus, CreateSessionRequest,
    DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions, SessionError,
    SessionHistoryQuery, SessionQuery, SessionService, SessionServiceControlExt,
    SessionServiceHistoryExt, StartTurnRequest, TurnToolOverlay,
};
use meerkat_core::types::{AssistantBlock, HandlingMode, RunResult, SessionId, StopReason, Usage};
use meerkat_core::{
    HookDecision, HookEngine, HookExecutionReport, HookId, HookInvocation, HookOutcome, HookPoint,
    HookReasonCode, Session, SessionDeferredTurnState,
};
use meerkat_session::ephemeral::SessionSnapshot;
use meerkat_session::{EphemeralSessionService, SessionAgent, SessionAgentBuilder};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Mock agent
// ---------------------------------------------------------------------------

struct MockAgent {
    session_id: SessionId,
    message_count: usize,
    delay_ms: Option<u64>,
    callback_pending: bool,
    fail_overlay_clear: bool,
    overlay_updates: Arc<std::sync::Mutex<Vec<Option<TurnToolOverlay>>>>,
    system_context_state: Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>,
}

#[async_trait]
impl SessionAgent for MockAgent {
    async fn run_with_events(
        &mut self,
        _prompt: meerkat_core::types::ContentInput,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        if let Some(delay) = self.delay_ms {
            tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
        }

        let _ = event_tx
            .send(AgentEvent::RunStarted {
                session_id: self.session_id.clone(),
                prompt: meerkat_core::ContentInput::Text("test".to_string()),
            })
            .await;

        if self.callback_pending {
            return Err(meerkat_core::error::AgentError::CallbackPending {
                tool_name: "external_mock".into(),
                args: json!({ "value": "browser" }),
            });
        }

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
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: None,
        })
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

    fn hot_swap_llm_identity(
        &mut self,
        _client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        _identity: meerkat_core::SessionLlmIdentity,
        _request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Ok(())
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

struct MockAgentBuilder {
    delay_ms: Option<u64>,
    build_delay_ms: Option<u64>,
    callback_pending: bool,
    fail_overlay_clear: bool,
    overlay_updates: Arc<std::sync::Mutex<Vec<Option<TurnToolOverlay>>>>,
}

impl MockAgentBuilder {
    fn new() -> Self {
        Self {
            delay_ms: None,
            build_delay_ms: None,
            callback_pending: false,
            fail_overlay_clear: false,
            overlay_updates: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn with_delay(delay_ms: u64) -> Self {
        Self {
            delay_ms: Some(delay_ms),
            build_delay_ms: None,
            callback_pending: false,
            fail_overlay_clear: false,
            overlay_updates: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn with_build_delay(build_delay_ms: u64) -> Self {
        Self {
            delay_ms: None,
            build_delay_ms: Some(build_delay_ms),
            callback_pending: false,
            fail_overlay_clear: false,
            overlay_updates: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn with_callback_pending() -> Self {
        Self {
            delay_ms: None,
            build_delay_ms: None,
            callback_pending: true,
            fail_overlay_clear: false,
            overlay_updates: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn with_overlay_clear_failure() -> Self {
        Self {
            delay_ms: None,
            build_delay_ms: None,
            callback_pending: false,
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
            callback_pending: self.callback_pending,
            fail_overlay_clear: self.fail_overlay_clear,
            overlay_updates: self.overlay_updates.clone(),
            system_context_state: Arc::new(std::sync::Mutex::new(Default::default())),
        })
    }
}

// ---------------------------------------------------------------------------
// Real agent fixtures (runtime boundary assertions)
// ---------------------------------------------------------------------------

struct DenyNextPreLlmHookEngine {
    deny_next: AtomicBool,
}

impl DenyNextPreLlmHookEngine {
    fn new() -> Self {
        Self {
            deny_next: AtomicBool::new(true),
        }
    }
}

#[async_trait]
impl HookEngine for DenyNextPreLlmHookEngine {
    async fn execute(
        &self,
        invocation: HookInvocation,
        _overrides: Option<&meerkat_core::config::HookRunOverrides>,
    ) -> Result<HookExecutionReport, meerkat_core::HookEngineError> {
        if invocation.point != HookPoint::PreLlmRequest
            || !self.deny_next.swap(false, Ordering::AcqRel)
        {
            return Ok(HookExecutionReport::default());
        }

        let decision = HookDecision::deny(
            HookId::new("deny-pre-llm"),
            HookReasonCode::PolicyViolation,
            "pre-llm turn denied",
            None,
        );

        Ok(HookExecutionReport {
            outcomes: vec![HookOutcome {
                hook_id: HookId::new("deny-pre-llm"),
                point: HookPoint::PreLlmRequest,
                priority: 0,
                registration_index: 0,
                decision: Some(decision.clone()),
                patches: Vec::new(),
                published_patches: Vec::new(),
                error: None,
                duration_ms: None,
            }],
            decision: Some(decision),
            patches: Vec::new(),
            published_patches: Vec::new(),
        })
    }
}

fn session_for_request(req: &CreateSessionRequest) -> Session {
    let mut session = Session::new();
    if let Some(system_prompt) = &req.system_prompt {
        session.set_system_prompt(system_prompt.clone());
    }
    session
}

fn session_snapshot(session: &Session) -> SessionSnapshot {
    SessionSnapshot {
        created_at: session.created_at(),
        updated_at: session.updated_at(),
        message_count: session.messages().len(),
        total_tokens: session.total_tokens(),
        usage: session.total_usage(),
        last_assistant_text: session.last_assistant_text(),
    }
}

fn session_clone_with_system_context(
    session: &Session,
    state: &Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>,
) -> Session {
    let mut clone = session.clone();
    clone
        .set_system_context_state(
            state
                .lock()
                .expect("system-context state lock poisoned")
                .clone(),
        )
        .expect("serialize system-context state");
    clone
}

fn sync_session_context_state(
    session: &Session,
    state: &Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>,
) {
    if let Some(session_state) = session.system_context_state() {
        *state.lock().expect("system-context state lock poisoned") = session_state;
    }
}

fn sync_shared_system_context_to_session(
    session: &mut Session,
    state: &Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>,
) {
    let state = state
        .lock()
        .expect("system-context state lock poisoned")
        .clone();
    session
        .set_system_context_state(state)
        .expect("serialize system-context state");
}

fn successful_run_result(session: &Session, text: impl Into<String>) -> RunResult {
    RunResult {
        text: text.into(),
        session_id: session.id().clone(),
        usage: Usage::default(),
        turns: 1,
        tool_calls: 0,
        terminal_cause_kind: None,
        structured_output: None,
        extraction_error: None,
        schema_warnings: None,
        skill_diagnostics: None,
    }
}

fn filtered_tool_names(overlay: &Option<TurnToolOverlay>) -> Vec<String> {
    let mut names = vec!["alpha".to_string(), "beta".to_string()];
    if let Some(overlay) = overlay {
        if let Some(allowed) = &overlay.allowed_tools {
            names.retain(|name| allowed.iter().any(|allowed| allowed == name));
        }
        if let Some(blocked) = &overlay.blocked_tools {
            names.retain(|name| !blocked.iter().any(|blocked| blocked == name));
        }
    }
    names
}

fn rendered_system_prompts(
    session: &Session,
    appends: &[meerkat_core::PendingSystemContextAppend],
) -> Vec<String> {
    let mut session = session.clone();
    session.append_system_context_blocks(appends);
    session
        .messages()
        .iter()
        .filter_map(|message| match message {
            meerkat_core::types::Message::System(system) => Some(system.content.clone()),
            _ => None,
        })
        .collect()
}

struct RealSessionAgent {
    session: Session,
    provider_visible_tools: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    provider_visible_system_prompts: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    delay_ms: Option<u64>,
    hook_engine: Option<Arc<dyn HookEngine>>,
    flow_tool_overlay: Option<TurnToolOverlay>,
    system_context_state: Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>,
    cancel_after_boundary_requested: Arc<AtomicBool>,
}

impl RealSessionAgent {
    fn take_pending_system_context_boundary(
        &mut self,
    ) -> Vec<meerkat_core::PendingSystemContextAppend> {
        let pending = {
            let mut state = self
                .system_context_state
                .lock()
                .expect("system-context state lock poisoned");
            if state.pending.is_empty() {
                return Vec::new();
            }
            let pending = state.pending.clone();
            state.mark_pending_applied();
            pending
        };

        sync_shared_system_context_to_session(&mut self.session, &self.system_context_state);
        pending
    }
}

#[async_trait]
impl SessionAgent for RealSessionAgent {
    async fn run_with_events(
        &mut self,
        prompt: meerkat_core::types::ContentInput,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        self.session.append_external_user_content(prompt.clone());

        if let Some(hook_engine) = &self.hook_engine {
            let report = hook_engine
                .execute(
                    HookInvocation::new(HookPoint::PreLlmRequest, self.session.id().clone()),
                    None,
                )
                .await
                .map_err(
                    |error| meerkat_core::error::AgentError::HookExecutionFailed {
                        hook_id: meerkat_core::HookId::new("test-pre-llm"),
                        reason: error.to_string(),
                    },
                )?;
            if let Some(HookDecision::Deny {
                hook_id,
                reason_code,
                message,
                payload,
            }) = report.decision
            {
                return Err(meerkat_core::error::AgentError::HookDenied {
                    hook_id,
                    point: HookPoint::PreLlmRequest,
                    reason_code,
                    message,
                    payload,
                });
            }
        }

        let boundary_system_context = self.take_pending_system_context_boundary();

        self.provider_visible_tools
            .lock()
            .expect("provider_visible_tools lock poisoned")
            .push(filtered_tool_names(&self.flow_tool_overlay));
        self.provider_visible_system_prompts
            .lock()
            .expect("provider_visible_system_prompts lock poisoned")
            .push(rendered_system_prompts(
                &self.session,
                &boundary_system_context,
            ));

        if let Some(delay_ms) = self.delay_ms {
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        }

        self.session.append_external_assistant_blocks(
            vec![AssistantBlock::Text {
                text: "ok".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
            Usage::default(),
        );

        Ok(successful_run_result(&self.session, "ok"))
    }

    fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {
        // The session-service contract tests only need to verify that the call
        // is admitted and does not alter provider-visible tool/system context.
    }

    fn set_flow_tool_overlay(
        &mut self,
        overlay: Option<TurnToolOverlay>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.flow_tool_overlay = overlay;
        Ok(())
    }

    fn cancel(&mut self) {
        self.cancel_after_boundary_requested
            .store(true, Ordering::Release);
    }

    fn cancel_after_boundary_handle(&self) -> Option<Arc<AtomicBool>> {
        Some(Arc::clone(&self.cancel_after_boundary_requested))
    }

    fn hot_swap_llm_identity(
        &mut self,
        _client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        _identity: meerkat_core::SessionLlmIdentity,
        _request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Ok(())
    }

    fn session_id(&self) -> SessionId {
        self.session.id().clone()
    }

    fn snapshot(&self) -> SessionSnapshot {
        session_snapshot(&self.session)
    }

    fn session_clone(&self) -> meerkat_core::Session {
        session_clone_with_system_context(&self.session, &self.system_context_state)
    }

    fn apply_runtime_system_context(
        &mut self,
        appends: &[meerkat_core::PendingSystemContextAppend],
    ) {
        self.session.append_system_context_blocks(appends);
        sync_session_context_state(&self.session, &self.system_context_state);
    }

    fn system_context_state(
        &self,
    ) -> Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>> {
        Arc::clone(&self.system_context_state)
    }

    fn sync_system_context_state(&mut self) {
        sync_session_context_state(&self.session, &self.system_context_state);
    }
}

struct CompactionSessionAgent {
    session: Session,
    seen_last_user_messages: Arc<std::sync::Mutex<Vec<String>>>,
    compactor: Arc<TrackingCompactor>,
    boundary_index: u64,
    system_context_state: Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>>,
    cancel_after_boundary_requested: Arc<AtomicBool>,
}

#[async_trait]
impl SessionAgent for CompactionSessionAgent {
    async fn run_with_events(
        &mut self,
        prompt: meerkat_core::types::ContentInput,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        let context = CompactionContext {
            last_input_tokens: 0,
            message_count: self.session.messages().len(),
            estimated_history_tokens: 0,
            last_compaction_boundary_index: None,
            session_boundary_index: self.boundary_index,
        };
        if self.compactor.should_compact(&context) {
            self.seen_last_user_messages
                .lock()
                .expect("seen_last_user_messages lock poisoned")
                .push(self.compactor.compaction_prompt().to_string());
        }
        self.boundary_index += 1;

        let prompt_text = prompt.text_content();
        self.seen_last_user_messages
            .lock()
            .expect("seen_last_user_messages lock poisoned")
            .push(prompt_text);
        self.session.append_external_user_content(prompt);
        self.session.append_external_assistant_blocks(
            vec![AssistantBlock::Text {
                text: "ok".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
            Usage::default(),
        );

        Ok(successful_run_result(&self.session, "ok"))
    }

    fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {
        // No-op for the compaction-focused test agent.
    }

    fn set_flow_tool_overlay(
        &mut self,
        _overlay: Option<TurnToolOverlay>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Ok(())
    }

    fn cancel(&mut self) {
        self.cancel_after_boundary_requested
            .store(true, Ordering::Release);
    }

    fn cancel_after_boundary_handle(&self) -> Option<Arc<AtomicBool>> {
        Some(Arc::clone(&self.cancel_after_boundary_requested))
    }

    fn hot_swap_llm_identity(
        &mut self,
        _client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        _identity: meerkat_core::SessionLlmIdentity,
        _request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Ok(())
    }

    fn session_id(&self) -> SessionId {
        self.session.id().clone()
    }

    fn snapshot(&self) -> SessionSnapshot {
        session_snapshot(&self.session)
    }

    fn session_clone(&self) -> meerkat_core::Session {
        session_clone_with_system_context(&self.session, &self.system_context_state)
    }

    fn apply_runtime_system_context(
        &mut self,
        appends: &[meerkat_core::PendingSystemContextAppend],
    ) {
        self.session.append_system_context_blocks(appends);
        sync_session_context_state(&self.session, &self.system_context_state);
    }

    fn system_context_state(
        &self,
    ) -> Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>> {
        Arc::clone(&self.system_context_state)
    }

    fn sync_system_context_state(&mut self) {
        sync_session_context_state(&self.session, &self.system_context_state);
    }
}

struct RealAgentBuilder {
    provider_visible_tools: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    provider_visible_system_prompts: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    llm_delay_ms: Option<u64>,
    hook_engine: Option<Arc<dyn HookEngine>>,
}

struct TrackingCompactor {
    compact_on_boundary: Option<u64>,
    seen_contexts: Arc<std::sync::Mutex<Vec<CompactionContext>>>,
}

impl TrackingCompactor {
    fn new(compact_on_boundary: Option<u64>) -> Self {
        Self {
            compact_on_boundary,
            seen_contexts: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn seen_boundaries(&self) -> Vec<u64> {
        self.seen_contexts
            .lock()
            .expect("seen_contexts lock poisoned")
            .iter()
            .map(|ctx| ctx.session_boundary_index)
            .collect()
    }
}

impl Compactor for TrackingCompactor {
    fn should_compact(&self, ctx: &CompactionContext) -> bool {
        self.seen_contexts
            .lock()
            .expect("seen_contexts lock poisoned")
            .push(ctx.clone());
        self.compact_on_boundary == Some(ctx.session_boundary_index)
    }

    fn compaction_prompt(&self) -> &'static str {
        "COMPACT NOW"
    }

    fn max_summary_tokens(&self) -> u32 {
        32
    }

    fn rebuild_history(
        &self,
        messages: &[meerkat_core::types::Message],
        _summary: &str,
    ) -> CompactionResult {
        CompactionResult {
            messages: messages.to_vec(),
            discarded: Vec::new(),
        }
    }
}

struct CompactionAgentBuilder {
    seen_last_user_messages: Arc<std::sync::Mutex<Vec<String>>>,
    compactor: Arc<TrackingCompactor>,
}

#[async_trait]
impl SessionAgentBuilder for CompactionAgentBuilder {
    type Agent = CompactionSessionAgent;

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<CompactionSessionAgent, SessionError> {
        Ok(CompactionSessionAgent {
            session: session_for_request(req),
            seen_last_user_messages: Arc::clone(&self.seen_last_user_messages),
            compactor: Arc::clone(&self.compactor),
            boundary_index: 0,
            system_context_state: Arc::new(std::sync::Mutex::new(Default::default())),
            cancel_after_boundary_requested: Arc::new(AtomicBool::new(false)),
        })
    }
}

#[async_trait]
impl SessionAgentBuilder for RealAgentBuilder {
    type Agent = RealSessionAgent;

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RealSessionAgent, SessionError> {
        Ok(RealSessionAgent {
            session: session_for_request(req),
            provider_visible_tools: Arc::clone(&self.provider_visible_tools),
            provider_visible_system_prompts: Arc::clone(&self.provider_visible_system_prompts),
            delay_ms: self.llm_delay_ms,
            hook_engine: self.hook_engine.as_ref().map(Arc::clone),
            flow_tool_overlay: None,
            system_context_state: Arc::new(std::sync::Mutex::new(Default::default())),
            cancel_after_boundary_requested: Arc::new(AtomicBool::new(false)),
        })
    }
}

fn make_service(builder: MockAgentBuilder) -> Arc<EphemeralSessionService<MockAgentBuilder>> {
    Arc::new(EphemeralSessionService::new(builder, 10))
}

fn create_req(prompt: &str) -> CreateSessionRequest {
    CreateSessionRequest {
        model: "mock".to_string(),
        prompt: prompt.to_string().into(),
        render_metadata: None,
        system_prompt: None,
        max_tokens: None,
        event_tx: None,

        skill_references: None,
        initial_turn: InitialTurnPolicy::RunImmediately,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
        build: None,
        labels: None,
    }
}

fn create_req_deferred(prompt: &str) -> CreateSessionRequest {
    CreateSessionRequest {
        initial_turn: InitialTurnPolicy::Defer,
        deferred_prompt_policy: DeferredPromptPolicy::Stage,
        ..create_req(prompt)
    }
}

fn turn_req(prompt: &str) -> StartTurnRequest {
    StartTurnRequest {
        prompt: prompt.to_string().into(),
        system_prompt: None,
        event_tx: None,
        runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
    }
}

fn runtime_content_turn_req(prompt: &str) -> StartTurnRequest {
    let mut req = turn_req(prompt);
    req.runtime.turn_metadata = Some(
        meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
            execution_kind: Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn),
            ..Default::default()
        },
    );
    req
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
        .start_turn(&session_id, turn_req("now run"))
        .await
        .expect("start_turn should run after deferred create");
    assert!(started.text.contains("Hello from mock"));
}

#[tokio::test]
async fn test_recovered_session_does_not_rearm_consumed_first_turn_override_window() {
    let service = make_service(MockAgentBuilder::new());
    let mut recovered = Session::new();
    let mut deferred = SessionDeferredTurnState::default();
    deferred.mark_initial_turn_pending();
    assert!(
        deferred.mark_initial_turn_started(),
        "pending deferred phase should transition to consumed"
    );
    recovered
        .set_deferred_turn_state(deferred)
        .expect("consumed deferred turn state");

    let mut request = create_req("recovered session");
    request.initial_turn = InitialTurnPolicy::Defer;
    request.deferred_prompt_policy = DeferredPromptPolicy::Discard;
    request.build = Some(SessionBuildOptions {
        resume_session: Some(recovered),
        ..Default::default()
    });

    let created = service
        .create_session(request)
        .await
        .expect("materialize recovered session");

    let error = service
        .start_turn(
            &created.session_id,
            StartTurnRequest {
                system_prompt: Some("late override".to_string()),
                ..turn_req("resume turn")
            },
        )
        .await
        .expect_err("recovered consumed first turn must not allow build-only overrides");

    assert!(
        matches!(error, SessionError::Unsupported(ref message) if message == "system_prompt override is only allowed on a deferred session's first turn"),
        "unexpected error: {error:?}"
    );
}

#[tokio::test]
async fn test_template_resume_session_still_arms_deferred_first_turn_override_window() {
    let service = make_service(MockAgentBuilder::new());
    let template = Session::new();

    let mut request = create_req("template-backed deferred session");
    request.initial_turn = InitialTurnPolicy::Defer;
    request.deferred_prompt_policy = DeferredPromptPolicy::Discard;
    request.build = Some(SessionBuildOptions {
        resume_session: Some(template),
        ..Default::default()
    });

    let created = service
        .create_session(request)
        .await
        .expect("create deferred session from template");

    let deferred_state = service
        .deferred_turn_state(&created.session_id)
        .await
        .expect("template-backed deferred session should expose deferred state");
    let allows_override = deferred_state
        .lock()
        .expect("deferred-turn state lock poisoned")
        .allows_initial_turn_overrides();
    assert!(
        allows_override,
        "template-backed deferred session should still arm the first-turn override window"
    );

    let started = service
        .start_turn(&created.session_id, turn_req("resume turn"))
        .await
        .expect("template-backed deferred session should still run its first turn");
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
        .start_turn(&sid, turn_req("trigger"))
        .await
        .expect("start turn");

    let first = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
        .await
        .expect("timed out waiting for session event")
        .expect("stream closed unexpectedly");
    assert_eq!(first.source_session_id(), Some(&sid));
    assert_eq!(first.source_id, format!("session:{sid}"));
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
        .start_turn(&session_id, turn_req("Follow up"))
        .await
        .unwrap();
    assert!(result2.text.contains("Hello from mock"));
}

#[tokio::test]
async fn test_follow_up_start_turn_can_compact_before_first_llm_call() {
    let seen_last_user_messages = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let compactor = Arc::new(TrackingCompactor::new(Some(1)));
    let service = Arc::new(EphemeralSessionService::new(
        CompactionAgentBuilder {
            seen_last_user_messages: Arc::clone(&seen_last_user_messages),
            compactor: Arc::clone(&compactor),
        },
        10,
    ));

    let created = service
        .create_session(create_req("first"))
        .await
        .expect("initial session run should succeed");

    service
        .start_turn(&created.session_id, turn_req("follow up"))
        .await
        .expect("follow-up start_turn should succeed");

    assert_eq!(compactor.seen_boundaries(), vec![0, 1]);
    assert_eq!(
        seen_last_user_messages
            .lock()
            .expect("seen_last_user_messages lock poisoned")
            .clone(),
        vec![
            "first".to_string(),
            "COMPACT NOW".to_string(),
            "follow up".to_string()
        ]
    );
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
        .start_turn(&session_id, turn_req("After archive"))
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code(), "SESSION_NOT_FOUND");
}

#[tokio::test]
async fn test_read_history_on_archived_session_returns_persistence_disabled() {
    let service = make_service(MockAgentBuilder::new());
    let _ = service.create_session(create_req("Hello")).await.unwrap();

    let sessions = service.list(SessionQuery::default()).await.unwrap();
    let session_id = sessions[0].session_id.clone();

    service.archive(&session_id).await.unwrap();

    let err = service
        .read_history(
            &session_id,
            SessionHistoryQuery {
                offset: 0,
                limit: None,
            },
        )
        .await
        .expect_err("ephemeral archived history should be unavailable");
    assert_eq!(err.code(), "SESSION_PERSISTENCE_DISABLED");
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
    let _handle =
        tokio::spawn(async move { service_clone.start_turn(&sid_clone, turn_req("Slow")).await });

    // Give the turn time to start running
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Try to start another turn
    let result = service.start_turn(&session_id, turn_req("Fast")).await;

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
    let _handle =
        tokio::spawn(async move { service_clone.start_turn(&sid_clone, turn_req("Slow")).await });

    // Give the turn time to start
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Interrupt should succeed
    let result = service.interrupt(&session_id).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_cancel_after_boundary_is_accepted_for_real_inflight_turn() {
    let service = Arc::new(EphemeralSessionService::new(
        RealAgentBuilder {
            provider_visible_tools: Arc::new(std::sync::Mutex::new(Vec::new())),
            provider_visible_system_prompts: Arc::new(std::sync::Mutex::new(Vec::new())),
            llm_delay_ms: Some(200),
            hook_engine: None,
        },
        10,
    ));

    let _ = service.create_session(create_req("Hello")).await.unwrap();

    let session_id = service.list(SessionQuery::default()).await.unwrap()[0]
        .session_id
        .clone();

    let service_clone = Arc::clone(&service);
    let sid_clone = session_id.clone();
    let turn =
        tokio::spawn(async move { service_clone.start_turn(&sid_clone, turn_req("Slow")).await });

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    service
        .cancel_after_boundary(&session_id)
        .await
        .expect("boundary cancel should be accepted while running");

    let result = turn.await.expect("turn join should succeed");
    assert!(
        result.is_ok(),
        "simple single-turn runs may still complete when no later cancellable boundary exists"
    );
}

#[tokio::test]
async fn test_flow_tool_overlay_is_cleared_after_canceled_turn() {
    let overlay_updates = Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = Arc::new(EphemeralSessionService::new(
        MockAgentBuilder {
            delay_ms: Some(500),
            build_delay_ms: None,
            callback_pending: false,
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
                    runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                        None,
                        HandlingMode::Queue,
                        None,
                        Some(overlay),
                        Vec::new(),
                        None,
                    ),
                    ..turn_req("Slow with overlay")
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
            provider_visible_system_prompts: Arc::new(std::sync::Mutex::new(Vec::new())),
            llm_delay_ms: None,
            hook_engine: None,
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
                runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                    None,
                    HandlingMode::Queue,
                    None,
                    Some(TurnToolOverlay {
                        allowed_tools: Some(vec!["alpha".to_string(), "beta".to_string()]),
                        blocked_tools: Some(vec!["beta".to_string()]),
                    }),
                    Vec::new(),
                    None,
                ),
                ..turn_req("overlayed turn")
            },
        )
        .await
        .expect("turn with overlay should run");

    service
        .start_turn(&session_id, turn_req("baseline turn"))
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
                runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                    None,
                    HandlingMode::Queue,
                    None,
                    Some(TurnToolOverlay {
                        allowed_tools: Some(vec!["alpha".to_string()]),
                        blocked_tools: None,
                    }),
                    Vec::new(),
                    None,
                ),
                ..turn_req("overlay clear fails")
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
async fn test_apply_runtime_turn_returns_callback_pending_terminal() -> Result<(), String> {
    let service = make_service(MockAgentBuilder::with_callback_pending());
    let _ = service
        .create_session(create_req_deferred("Hello"))
        .await
        .expect("create deferred session");
    let session_id = service
        .list(SessionQuery::default())
        .await
        .expect("list sessions")[0]
        .session_id
        .clone();
    let run_id = meerkat_core::lifecycle::RunId::new();
    let contributing_input_ids = vec![meerkat_core::lifecycle::InputId::new()];

    let output = service
        .apply_runtime_turn(
            &session_id,
            run_id.clone(),
            runtime_content_turn_req("needs callback"),
            RunApplyBoundary::RunStart,
            contributing_input_ids.clone(),
        )
        .await
        .expect("runtime apply should surface callback pending as terminal");

    assert_eq!(output.receipt.run_id, run_id);
    assert_eq!(output.receipt.boundary, RunApplyBoundary::RunStart);
    assert_eq!(
        output.receipt.contributing_input_ids,
        contributing_input_ids
    );
    assert!(output.session_snapshot.is_some());
    let Some(CoreApplyTerminal::CallbackPending { tool_name, args }) = output.terminal else {
        return Err("expected callback pending terminal".to_string());
    };
    assert_eq!(tool_name, "external_mock");
    assert_eq!(args, json!({ "value": "browser" }));
    Ok(())
}

#[tokio::test]
async fn test_apply_runtime_turn_rejects_missing_execution_kind_before_no_pending_terminal()
-> Result<(), String> {
    let service = make_service(MockAgentBuilder::new());
    let mut create = create_req_deferred("Hello");
    create.deferred_prompt_policy = DeferredPromptPolicy::Discard;
    let _ = service
        .create_session(create)
        .await
        .expect("create deferred session");
    let session_id = service
        .list(SessionQuery::default())
        .await
        .expect("list sessions")[0]
        .session_id
        .clone();

    let error = service
        .apply_runtime_turn(
            &session_id,
            meerkat_core::lifecycle::RunId::new(),
            turn_req(""),
            RunApplyBoundary::RunStart,
            vec![meerkat_core::lifecycle::InputId::new()],
        )
        .await
        .expect_err("runtime apply must reject missing execution kind before no-pending commit");

    if !error.to_string().contains("runtime_execution_kind not set") {
        return Err(format!("unexpected error: {error}"));
    }
    Ok(())
}

#[tokio::test]
async fn test_apply_runtime_turn_resume_pending_no_boundary_is_typed_terminal() -> Result<(), String>
{
    let service = make_service(MockAgentBuilder::new());
    let _ = service
        .create_session(create_req_deferred("Hello"))
        .await
        .expect("create deferred session");
    let session_id = service
        .list(SessionQuery::default())
        .await
        .expect("list sessions")[0]
        .session_id
        .clone();
    let run_id = meerkat_core::lifecycle::RunId::new();
    let contributing_input_ids = vec![meerkat_core::lifecycle::InputId::new()];
    let mut req = turn_req("");
    req.runtime.turn_metadata = Some(
        meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
            execution_kind: Some(meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending),
            ..Default::default()
        },
    );

    let output = service
        .apply_runtime_turn(
            &session_id,
            run_id.clone(),
            req,
            RunApplyBoundary::RunStart,
            contributing_input_ids.clone(),
        )
        .await
        .expect("runtime apply should surface no-pending as terminal");

    assert_eq!(output.receipt.run_id, run_id);
    assert_eq!(
        output.receipt.contributing_input_ids,
        contributing_input_ids
    );
    assert!(output.session_snapshot.is_some());
    assert!(matches!(
        output.terminal,
        Some(CoreApplyTerminal::NoPendingBoundary)
    ));
    Ok(())
}

#[tokio::test]
async fn test_append_system_context_stages_dedupes_and_conflicts_per_session() {
    let service = make_service(MockAgentBuilder::new());
    let _ = service
        .create_session(create_req_deferred("Hello"))
        .await
        .expect("create deferred session");
    let session_id = service
        .list(SessionQuery::default())
        .await
        .expect("list sessions")[0]
        .session_id
        .clone();

    let request = AppendSystemContextRequest {
        text: "Observe the orchestrator handoff.".to_string(),
        source: Some("mob".to_string()),
        idempotency_key: Some("ctx-1".to_string()),
    };

    let first = service
        .append_system_context(&session_id, request.clone())
        .await
        .expect("stage first append");
    assert_eq!(first.status, AppendSystemContextStatus::Staged);

    let duplicate = service
        .append_system_context(&session_id, request.clone())
        .await
        .expect("duplicate append should be accepted");
    assert_eq!(duplicate.status, AppendSystemContextStatus::Duplicate);

    let conflict = service
        .append_system_context(
            &session_id,
            AppendSystemContextRequest {
                text: "Different content".to_string(),
                source: Some("mob".to_string()),
                idempotency_key: Some("ctx-1".to_string()),
            },
        )
        .await
        .expect_err("same key with different content must conflict");
    assert_eq!(conflict.code(), "SESSION_SYSTEM_CONTEXT_CONFLICT");

    let state = service
        .system_context_state(&session_id)
        .await
        .expect("shared system-context state");
    let state = state.lock().expect("system-context state lock poisoned");
    assert_eq!(state.pending.len(), 1, "duplicate must not enqueue twice");
    assert_eq!(state.pending[0].text, "Observe the orchestrator handoff.");
    assert_eq!(state.pending[0].source.as_deref(), Some("mob"));
    let seen = state
        .seen
        .get("ctx-1")
        .expect("idempotency key should be tracked");
    assert_eq!(seen.state, meerkat_core::SeenSystemContextState::Pending);
}

#[tokio::test]
async fn test_staged_system_context_applies_at_next_llm_boundary() {
    let provider_visible_tools = Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
    let provider_visible_system_prompts =
        Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
    let service = Arc::new(EphemeralSessionService::new(
        RealAgentBuilder {
            provider_visible_tools: Arc::clone(&provider_visible_tools),
            provider_visible_system_prompts: Arc::clone(&provider_visible_system_prompts),
            llm_delay_ms: None,
            hook_engine: None,
        },
        10,
    ));

    let mut request = create_req_deferred("runtime tool scope");
    request.system_prompt = Some("Base system prompt".to_string());
    let _ = service
        .create_session(request)
        .await
        .expect("create deferred session");
    let session_id = service
        .list(SessionQuery::default())
        .await
        .expect("list sessions")[0]
        .session_id
        .clone();

    let staged = service
        .append_system_context(
            &session_id,
            AppendSystemContextRequest {
                text: "You are coordinating with an external orchestrator.".to_string(),
                source: Some("mob".to_string()),
                idempotency_key: Some("ctx-boundary".to_string()),
            },
        )
        .await
        .expect("append system context");
    assert_eq!(staged.status, AppendSystemContextStatus::Staged);

    let state = service
        .system_context_state(&session_id)
        .await
        .expect("shared system-context state");
    assert_eq!(
        state
            .lock()
            .expect("system-context state lock poisoned")
            .pending
            .len(),
        1,
        "append should remain pending until the next LLM boundary"
    );

    service
        .start_turn(&session_id, turn_req("apply staged context"))
        .await
        .expect("turn should run");

    let prompts = provider_visible_system_prompts
        .lock()
        .expect("provider_visible_system_prompts lock poisoned")
        .clone();
    assert_eq!(prompts.len(), 1, "expected one provider call");
    assert_eq!(
        prompts[0].len(),
        1,
        "expected a single canonical system prompt"
    );
    let system_prompt = &prompts[0][0];
    assert!(system_prompt.contains("Base system prompt"));
    assert!(system_prompt.contains("[Runtime System Context]"));
    assert!(system_prompt.contains("source: mob"));
    assert!(system_prompt.contains("You are coordinating with an external orchestrator."));
    assert!(
        system_prompt.contains(meerkat_core::SYSTEM_CONTEXT_SEPARATOR),
        "runtime append should be rendered with the canonical separator"
    );

    let state = service
        .system_context_state(&session_id)
        .await
        .expect("shared system-context state");
    let state = state.lock().expect("system-context state lock poisoned");
    assert!(
        state.pending.is_empty(),
        "boundary application should clear the pending queue"
    );
    let seen = state
        .seen
        .get("ctx-boundary")
        .expect("idempotency key should remain tracked");
    assert_eq!(seen.state, meerkat_core::SeenSystemContextState::Applied);
}

#[tokio::test]
async fn test_staged_system_context_is_not_replayed_on_later_turns() {
    let provider_visible_system_prompts =
        Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
    let service = Arc::new(EphemeralSessionService::new(
        RealAgentBuilder {
            provider_visible_tools: Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new())),
            provider_visible_system_prompts: Arc::clone(&provider_visible_system_prompts),
            llm_delay_ms: None,
            hook_engine: None,
        },
        10,
    ));

    let mut request = create_req_deferred("runtime tool scope");
    request.system_prompt = Some("Base system prompt".to_string());
    let _ = service
        .create_session(request)
        .await
        .expect("create deferred session");
    let session_id = service
        .list(SessionQuery::default())
        .await
        .expect("list sessions")[0]
        .session_id
        .clone();

    service
        .append_system_context(
            &session_id,
            AppendSystemContextRequest {
                text: "You are coordinating with an external orchestrator.".to_string(),
                source: Some("mob".to_string()),
                idempotency_key: Some("ctx-boundary-replay".to_string()),
            },
        )
        .await
        .expect("append system context");

    service
        .start_turn(&session_id, turn_req("apply staged context"))
        .await
        .expect("first turn should run");

    service
        .start_turn(&session_id, turn_req("follow-up turn"))
        .await
        .expect("second turn should run");

    let prompts = provider_visible_system_prompts
        .lock()
        .expect("provider_visible_system_prompts lock poisoned")
        .clone();
    assert_eq!(prompts.len(), 2, "expected one provider call per turn");
    assert!(
        prompts[0][0].contains("You are coordinating with an external orchestrator."),
        "first turn should include staged context"
    );
    assert!(
        !prompts[1][0].contains("You are coordinating with an external orchestrator."),
        "second turn must not replay single-use staged context"
    );
}

#[tokio::test]
async fn test_staged_system_context_appended_during_active_turn_waits_for_next_turn() {
    let provider_visible_system_prompts =
        Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
    let service = Arc::new(EphemeralSessionService::new(
        RealAgentBuilder {
            provider_visible_tools: Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new())),
            provider_visible_system_prompts: Arc::clone(&provider_visible_system_prompts),
            llm_delay_ms: Some(200),
            hook_engine: None,
        },
        10,
    ));

    let mut request = create_req_deferred("runtime tool scope");
    request.system_prompt = Some("Base system prompt".to_string());
    let _ = service
        .create_session(request)
        .await
        .expect("create deferred session");
    let session_id = service
        .list(SessionQuery::default())
        .await
        .expect("list sessions")[0]
        .session_id
        .clone();

    let first_turn = {
        let service = Arc::clone(&service);
        let session_id = session_id.clone();
        tokio::spawn(async move {
            service
                .start_turn(&session_id, turn_req("first turn"))
                .await
        })
    };

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    service
        .append_system_context(
            &session_id,
            AppendSystemContextRequest {
                text: "Late staged context".to_string(),
                source: Some("mob".to_string()),
                idempotency_key: Some("ctx-during-active-turn".to_string()),
            },
        )
        .await
        .expect("append during active turn");

    first_turn
        .await
        .expect("first turn join")
        .expect("first turn should finish");

    service
        .start_turn(&session_id, turn_req("second turn"))
        .await
        .expect("second turn should run");

    let prompts = provider_visible_system_prompts
        .lock()
        .expect("provider_visible_system_prompts lock poisoned")
        .clone();
    assert_eq!(prompts.len(), 2, "expected one provider call per turn");
    assert!(
        !prompts[0][0].contains("Late staged context"),
        "context appended during an active turn must not enter the in-flight provider request"
    );
    assert!(
        prompts[1][0].contains("Late staged context"),
        "context appended during an active turn must stage for the subsequent eligible turn"
    );
}

#[tokio::test]
async fn test_pre_llm_denied_turn_does_not_consume_staged_system_context() {
    let provider_visible_system_prompts =
        Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
    let service = Arc::new(EphemeralSessionService::new(
        RealAgentBuilder {
            provider_visible_tools: Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new())),
            provider_visible_system_prompts: Arc::clone(&provider_visible_system_prompts),
            llm_delay_ms: None,
            hook_engine: Some(Arc::new(DenyNextPreLlmHookEngine::new())),
        },
        10,
    ));

    let mut request = create_req_deferred("runtime tool scope");
    request.system_prompt = Some("Base system prompt".to_string());
    let _ = service
        .create_session(request)
        .await
        .expect("create deferred session");
    let session_id = service
        .list(SessionQuery::default())
        .await
        .expect("list sessions")[0]
        .session_id
        .clone();

    service
        .append_system_context(
            &session_id,
            AppendSystemContextRequest {
                text: "You are coordinating with an external orchestrator.".to_string(),
                source: Some("mob".to_string()),
                idempotency_key: Some("ctx-pre-llm-deny".to_string()),
            },
        )
        .await
        .expect("append system context");

    let denied = service
        .start_turn(&session_id, turn_req("denied turn"))
        .await;
    assert!(denied.is_err(), "pre-llm hook should deny the first turn");

    let prompts = provider_visible_system_prompts
        .lock()
        .expect("provider_visible_system_prompts lock poisoned")
        .clone();
    assert!(
        prompts.is_empty(),
        "a denied pre-llm turn must not reach the provider"
    );

    service
        .start_turn(&session_id, turn_req("eligible turn"))
        .await
        .expect("next eligible turn should run");

    let prompts = provider_visible_system_prompts
        .lock()
        .expect("provider_visible_system_prompts lock poisoned")
        .clone();
    assert_eq!(
        prompts.len(),
        1,
        "only the eligible turn should reach the provider"
    );
    assert!(
        prompts[0][0].contains("You are coordinating with an external orchestrator."),
        "staged context must survive a denied pre-llm turn and apply on the next eligible turn"
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
async fn test_cancel_after_boundary_when_idle_returns_not_running() {
    let service = Arc::new(EphemeralSessionService::new(
        RealAgentBuilder {
            provider_visible_tools: Arc::new(std::sync::Mutex::new(Vec::new())),
            provider_visible_system_prompts: Arc::new(std::sync::Mutex::new(Vec::new())),
            llm_delay_ms: None,
            hook_engine: None,
        },
        10,
    ));
    let _ = service.create_session(create_req("Hello")).await.unwrap();

    let sessions = service.list(SessionQuery::default()).await.unwrap();
    let session_id = sessions[0].session_id.clone();

    let result = service.cancel_after_boundary(&session_id).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code(), "SESSION_NOT_RUNNING");
}

// ---------------------------------------------------------------------------
// Session labels tests
// ---------------------------------------------------------------------------

fn create_req_with_labels(
    prompt: &str,
    labels: std::collections::BTreeMap<String, String>,
) -> CreateSessionRequest {
    CreateSessionRequest {
        labels: Some(labels),
        ..create_req(prompt)
    }
}

#[tokio::test]
async fn test_session_labels_set_at_creation_appear_in_read() {
    let service = make_service(MockAgentBuilder::new());
    let mut labels = std::collections::BTreeMap::new();
    labels.insert("env".to_string(), "staging".to_string());
    labels.insert("team".to_string(), "infra".to_string());

    let result = service
        .create_session(create_req_with_labels("Hello", labels.clone()))
        .await
        .unwrap();

    let view = service.read(&result.session_id).await.unwrap();
    assert_eq!(view.state.labels, labels);
}

#[tokio::test]
async fn test_session_labels_appear_in_list() {
    let service = make_service(MockAgentBuilder::new());
    let mut labels = std::collections::BTreeMap::new();
    labels.insert("env".to_string(), "prod".to_string());

    let _ = service
        .create_session(create_req_with_labels("Hello", labels.clone()))
        .await
        .unwrap();

    let sessions = service.list(SessionQuery::default()).await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].labels, labels);
}

#[tokio::test]
async fn test_session_list_label_filter() {
    let service = make_service(MockAgentBuilder::new());

    let mut labels_a = std::collections::BTreeMap::new();
    labels_a.insert("env".to_string(), "prod".to_string());
    labels_a.insert("team".to_string(), "frontend".to_string());

    let mut labels_b = std::collections::BTreeMap::new();
    labels_b.insert("env".to_string(), "staging".to_string());
    labels_b.insert("team".to_string(), "backend".to_string());

    let _ = service
        .create_session(create_req_with_labels("A", labels_a.clone()))
        .await
        .unwrap();
    let _ = service
        .create_session(create_req_with_labels("B", labels_b.clone()))
        .await
        .unwrap();

    // Filter by env=prod — should match only A
    let mut filter = std::collections::BTreeMap::new();
    filter.insert("env".to_string(), "prod".to_string());
    let sessions = service
        .list(SessionQuery {
            labels: Some(filter),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0].labels.get("team").map(String::as_str),
        Some("frontend")
    );

    // Filter by team=backend — should match only B
    let mut filter = std::collections::BTreeMap::new();
    filter.insert("team".to_string(), "backend".to_string());
    let sessions = service
        .list(SessionQuery {
            labels: Some(filter),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0].labels.get("env").map(String::as_str),
        Some("staging")
    );

    // Filter by env=prod AND team=backend — should match neither
    let mut filter = std::collections::BTreeMap::new();
    filter.insert("env".to_string(), "prod".to_string());
    filter.insert("team".to_string(), "backend".to_string());
    let sessions = service
        .list(SessionQuery {
            labels: Some(filter),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(sessions.is_empty());

    // No filter — should return both
    let sessions = service.list(SessionQuery::default()).await.unwrap();
    assert_eq!(sessions.len(), 2);
}

#[tokio::test]
async fn test_session_labels_empty_default() {
    let service = make_service(MockAgentBuilder::new());
    let result = service.create_session(create_req("Hello")).await.unwrap();

    let view = service.read(&result.session_id).await.unwrap();
    assert!(view.state.labels.is_empty());

    let sessions = service.list(SessionQuery::default()).await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].labels.is_empty());
}
