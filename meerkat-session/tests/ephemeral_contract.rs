//! Contract tests for EphemeralSessionService.
//!
//! These tests verify the SessionService contract using a mock agent builder.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use async_trait::async_trait;
use futures::StreamExt;
use meerkat_core::compact::{
    COMPACTION_SUMMARY_PREFIX, CompactionContext, CompactionResult, CompactionSummary, Compactor,
};
use meerkat_core::event::AgentEvent;
use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions,
    SessionError, SessionHistoryQuery, SessionQuery, SessionService, SessionServiceHistoryExt,
    StartTurnRequest, TurnToolOverlay,
};
use meerkat_core::types::{AssistantBlock, HandlingMode, RunResult, SessionId, StopReason, Usage};
use meerkat_core::{
    CancelAfterBoundaryCommand, CancelAfterBoundarySender, ContentInput, HookDecision, HookEngine,
    HookInvocation, HookPoint, RunId, Session, SessionDeferredTurnState,
    TransientTurnContextStateHandle,
};
use meerkat_session::ephemeral::SessionSnapshot;
use meerkat_session::{EphemeralSessionService, SessionAgent, SessionAgentBuilder};
use serde_json::json;
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
    callback_pending: bool,
    fail_overlay_clear: bool,
    reject_system_messages: bool,
    overlay_updates: Arc<std::sync::Mutex<Vec<Option<TurnToolOverlay>>>>,
    durable_identity: Option<meerkat_core::SessionLlmIdentity>,
    transient_turn_context_state: TransientTurnContextStateHandle,
}

fn transient_turn_context_handle_for_test() -> TransientTurnContextStateHandle {
    TransientTurnContextStateHandle::new()
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
                input: meerkat_core::types::RunInput::Content {
                    content: meerkat_core::ContentInput::Text("test".to_string()),
                },
            })
            .await;

        if self.callback_pending {
            return Err(meerkat_core::error::AgentError::CallbackPending {
                tool_use_id: "call-1".to_string(),
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

    fn set_turn_tool_overlay(
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

    fn session_clone(&self) -> Result<meerkat_core::Session, meerkat_core::AgentError> {
        Ok(meerkat_core::Session::with_id(self.session_id.clone()))
    }

    fn session_transcript_authority(
        &self,
    ) -> Result<
        meerkat_session::ephemeral::SessionTranscriptAuthoritySnapshot,
        meerkat_core::AgentError,
    > {
        let session = self.session_clone()?;
        meerkat_session::ephemeral::SessionTranscriptAuthoritySnapshot::from_session(&session)
    }

    fn durable_llm_identity(&self) -> Option<meerkat_core::SessionLlmIdentity> {
        self.durable_identity.clone()
    }

    fn observed_session_tail(&self) -> meerkat_core::pending_continuation::ObservedSessionTailKind {
        meerkat_core::pending_continuation::observe_session_tail(
            self.session_clone()
                .expect("test session clone should succeed")
                .messages(),
        )
    }

    fn transient_turn_context_state(&self) -> TransientTurnContextStateHandle {
        self.transient_turn_context_state.clone()
    }

    fn append_system_messages(
        &mut self,
        contents: Vec<String>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        if self.reject_system_messages {
            return Err(meerkat_core::error::AgentError::ConfigError(
                "reject System batch before mutation".to_string(),
            ));
        }
        self.message_count += contents.len();
        Ok(())
    }
}

struct MockAgentBuilder {
    delay_ms: Option<u64>,
    build_delay_ms: Option<u64>,
    callback_pending: bool,
    fail_overlay_clear: bool,
    reject_system_messages: bool,
    overlay_updates: Arc<std::sync::Mutex<Vec<Option<TurnToolOverlay>>>>,
    durable_identity: Option<meerkat_core::SessionLlmIdentity>,
}

impl MockAgentBuilder {
    fn new() -> Self {
        Self {
            delay_ms: None,
            build_delay_ms: None,
            callback_pending: false,
            fail_overlay_clear: false,
            reject_system_messages: false,
            overlay_updates: Arc::new(std::sync::Mutex::new(Vec::new())),
            durable_identity: Some(test_llm_identity("mock")),
        }
    }

    fn with_delay(delay_ms: u64) -> Self {
        Self {
            delay_ms: Some(delay_ms),
            build_delay_ms: None,
            callback_pending: false,
            fail_overlay_clear: false,
            reject_system_messages: false,
            overlay_updates: Arc::new(std::sync::Mutex::new(Vec::new())),
            durable_identity: Some(test_llm_identity("mock")),
        }
    }

    fn with_build_delay(build_delay_ms: u64) -> Self {
        Self {
            delay_ms: None,
            build_delay_ms: Some(build_delay_ms),
            callback_pending: false,
            fail_overlay_clear: false,
            reject_system_messages: false,
            overlay_updates: Arc::new(std::sync::Mutex::new(Vec::new())),
            durable_identity: Some(test_llm_identity("mock")),
        }
    }

    fn with_callback_pending() -> Self {
        Self {
            delay_ms: None,
            build_delay_ms: None,
            callback_pending: true,
            fail_overlay_clear: false,
            reject_system_messages: false,
            overlay_updates: Arc::new(std::sync::Mutex::new(Vec::new())),
            durable_identity: Some(test_llm_identity("mock")),
        }
    }

    fn with_overlay_clear_failure() -> Self {
        Self {
            delay_ms: None,
            build_delay_ms: None,
            callback_pending: false,
            fail_overlay_clear: true,
            reject_system_messages: false,
            overlay_updates: Arc::new(std::sync::Mutex::new(Vec::new())),
            durable_identity: Some(test_llm_identity("mock")),
        }
    }

    fn without_durable_identity() -> Self {
        Self {
            durable_identity: None,
            ..Self::new()
        }
    }

    fn rejecting_system_messages() -> Self {
        Self {
            reject_system_messages: true,
            ..Self::new()
        }
    }
}

#[async_trait]
impl SessionAgentBuilder for MockAgentBuilder {
    type Agent = MockAgent;

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<MockAgent, SessionError> {
        if let Some(delay) = self.build_delay_ms {
            tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
        }
        Ok(MockAgent {
            session_id: req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.as_ref())
                .map(|session| session.id().clone())
                .unwrap_or_default(),
            message_count: 0,
            delay_ms: self.delay_ms,
            callback_pending: self.callback_pending,
            fail_overlay_clear: self.fail_overlay_clear,
            reject_system_messages: self.reject_system_messages,
            overlay_updates: self.overlay_updates.clone(),
            durable_identity: self.durable_identity.clone(),
            transient_turn_context_state: transient_turn_context_handle_for_test(),
        })
    }
}

// ---------------------------------------------------------------------------
// Real agent fixtures (runtime boundary assertions)
// ---------------------------------------------------------------------------

fn session_for_request(req: &CreateSessionRequest) -> Session {
    let mut session = req
        .build
        .as_ref()
        .and_then(|build| build.resume_session.clone())
        .unwrap_or_default();
    if let Some(system_prompt) = req.system_prompt.as_set_prompt() {
        session.append_system_message(system_prompt.to_string());
    }
    session
}

fn test_llm_identity(model: &str) -> meerkat_core::SessionLlmIdentity {
    meerkat_core::SessionLlmIdentity {
        model: model.to_string(),
        provider: meerkat_core::Provider::Other,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    }
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

fn rendered_system_prompts(session: &Session) -> Vec<String> {
    session
        .messages_for_model_boundary()
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
    turn_tool_overlay: Option<TurnToolOverlay>,
    transient_turn_context_state: TransientTurnContextStateHandle,
    cancel_after_boundary_tx: CancelAfterBoundarySender,
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

        self.provider_visible_tools
            .lock()
            .expect("provider_visible_tools lock poisoned")
            .push(filtered_tool_names(&self.turn_tool_overlay));
        self.provider_visible_system_prompts
            .lock()
            .expect("provider_visible_system_prompts lock poisoned")
            .push(rendered_system_prompts(&self.session));

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

    fn set_turn_tool_overlay(
        &mut self,
        overlay: Option<TurnToolOverlay>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.turn_tool_overlay = overlay;
        Ok(())
    }

    fn cancel(&mut self) {
        let _ = self
            .cancel_after_boundary_tx
            .send(CancelAfterBoundaryCommand::for_run(RunId::new()));
    }

    fn cancel_after_boundary_handle(&self) -> Option<CancelAfterBoundarySender> {
        Some(self.cancel_after_boundary_tx.clone())
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

    fn session_clone(&self) -> Result<meerkat_core::Session, meerkat_core::AgentError> {
        Ok(self.session.clone())
    }

    fn session_transcript_authority(
        &self,
    ) -> Result<
        meerkat_session::ephemeral::SessionTranscriptAuthoritySnapshot,
        meerkat_core::AgentError,
    > {
        meerkat_session::ephemeral::SessionTranscriptAuthoritySnapshot::from_session(&self.session)
    }

    fn durable_llm_identity(&self) -> Option<meerkat_core::SessionLlmIdentity> {
        Some(test_llm_identity("mock"))
    }

    fn observed_session_tail(&self) -> meerkat_core::pending_continuation::ObservedSessionTailKind {
        meerkat_core::pending_continuation::observe_session_tail(self.session.messages())
    }

    fn transient_turn_context_state(&self) -> TransientTurnContextStateHandle {
        self.transient_turn_context_state.clone()
    }

    fn append_system_messages(
        &mut self,
        contents: Vec<String>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        for content in contents {
            self.session.append_system_message(content);
        }
        Ok(())
    }
}

struct CompactionSessionAgent {
    session: Session,
    seen_last_user_messages: Arc<std::sync::Mutex<Vec<String>>>,
    compactor: Arc<TrackingCompactor>,
    boundary_index: u64,
    transient_turn_context_state: TransientTurnContextStateHandle,
    cancel_after_boundary_tx: CancelAfterBoundarySender,
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
            estimated_request_bytes: 0,
            provider_request_pressure: None,
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

    fn set_turn_tool_overlay(
        &mut self,
        _overlay: Option<TurnToolOverlay>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        Ok(())
    }

    fn cancel(&mut self) {
        let _ = self
            .cancel_after_boundary_tx
            .send(CancelAfterBoundaryCommand::for_run(RunId::new()));
    }

    fn cancel_after_boundary_handle(&self) -> Option<CancelAfterBoundarySender> {
        Some(self.cancel_after_boundary_tx.clone())
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

    fn session_clone(&self) -> Result<meerkat_core::Session, meerkat_core::AgentError> {
        Ok(self.session.clone())
    }

    fn session_transcript_authority(
        &self,
    ) -> Result<
        meerkat_session::ephemeral::SessionTranscriptAuthoritySnapshot,
        meerkat_core::AgentError,
    > {
        meerkat_session::ephemeral::SessionTranscriptAuthoritySnapshot::from_session(&self.session)
    }

    fn durable_llm_identity(&self) -> Option<meerkat_core::SessionLlmIdentity> {
        Some(test_llm_identity("mock"))
    }

    fn observed_session_tail(&self) -> meerkat_core::pending_continuation::ObservedSessionTailKind {
        meerkat_core::pending_continuation::observe_session_tail(self.session.messages())
    }

    fn transient_turn_context_state(&self) -> TransientTurnContextStateHandle {
        self.transient_turn_context_state.clone()
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
        summary: &str,
    ) -> CompactionResult {
        let summary_message = meerkat_core::types::Message::User(
            meerkat_core::types::UserMessage::compaction_summary(format!(
                "{COMPACTION_SUMMARY_PREFIX}{summary}"
            )),
        );
        CompactionResult {
            messages: messages.to_vec(),
            summary: CompactionSummary::new(0, summary_message),
            retained: messages
                .iter()
                .cloned()
                .enumerate()
                .map(|(offset, message)| {
                    let offset = u64::try_from(offset).unwrap_or(u64::MAX);
                    meerkat_core::compact::CompactionRetained::new(offset, offset, message)
                })
                .collect(),
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
        let (cancel_after_boundary_tx, _cancel_after_boundary_rx) =
            tokio::sync::mpsc::unbounded_channel();
        Ok(CompactionSessionAgent {
            session: session_for_request(req),
            seen_last_user_messages: Arc::clone(&self.seen_last_user_messages),
            compactor: Arc::clone(&self.compactor),
            boundary_index: 0,
            transient_turn_context_state: transient_turn_context_handle_for_test(),
            cancel_after_boundary_tx,
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
        let (cancel_after_boundary_tx, _cancel_after_boundary_rx) =
            tokio::sync::mpsc::unbounded_channel();
        Ok(RealSessionAgent {
            session: session_for_request(req),
            provider_visible_tools: Arc::clone(&self.provider_visible_tools),
            provider_visible_system_prompts: Arc::clone(&self.provider_visible_system_prompts),
            delay_ms: self.llm_delay_ms,
            hook_engine: self.hook_engine.as_ref().map(Arc::clone),
            turn_tool_overlay: None,
            transient_turn_context_state: transient_turn_context_handle_for_test(),
            cancel_after_boundary_tx,
        })
    }
}

fn make_service(builder: MockAgentBuilder) -> Arc<EphemeralSessionService<MockAgentBuilder>> {
    Arc::new(EphemeralSessionService::new(builder, 10))
}

fn create_req(prompt: &str) -> CreateSessionRequest {
    CreateSessionRequest {
        injected_context: Vec::new(),
        model: "mock".to_string(),
        prompt: prompt.to_string().into(),
        system_prompt: meerkat_core::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,

        initial_turn: InitialTurnPolicy::RunImmediately,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
        build: None,
        labels: None,
    }
}

fn create_req_deferred(prompt: &str) -> CreateSessionRequest {
    CreateSessionRequest {
        injected_context: Vec::new(),
        initial_turn: InitialTurnPolicy::Defer,
        deferred_prompt_policy: DeferredPromptPolicy::Stage,
        ..create_req(prompt)
    }
}

fn turn_req(prompt: &str) -> StartTurnRequest {
    StartTurnRequest {
        injected_context: Vec::new(),
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
async fn test_create_session_rejects_without_durable_llm_identity() {
    let service = make_service(MockAgentBuilder::without_durable_identity());
    let result = service
        .create_session(create_req_deferred("missing identity"))
        .await;

    let message = match result {
        Err(SessionError::Agent(meerkat_core::error::AgentError::ConfigError(message))) => {
            Some(message)
        }
        _ => None,
    };
    assert!(
        message
            .as_deref()
            .is_some_and(|message| message.contains("durable LLM identity"))
    );
}

#[tokio::test]
async fn test_recovered_session_accepts_system_message_after_consumed_first_turn() {
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

    let result = service
        .start_turn(
            &created.session_id,
            StartTurnRequest {
                injected_context: Vec::new(),
                system_prompt: Some("late override".to_string()),
                ..turn_req("resume turn")
            },
        )
        .await
        .expect("System messages are valid on every admitted turn");

    assert_eq!(result.text, "Hello from mock");
    let view = service
        .read(&created.session_id)
        .await
        .expect("read session");
    assert_eq!(
        view.state.message_count, 3,
        "one System plus the turn's user and assistant messages must commit"
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
    let handle =
        tokio::spawn(async move { service_clone.start_turn(&sid_clone, turn_req("Slow")).await });

    // Wait for generated turn-admission authority to publish the claim. This
    // is the exact Busy decision boundary; wall-clock sleeps are not evidence
    // that the spawned task was scheduled under a loaded nextest runner.
    tokio::time::timeout(tokio::time::Duration::from_secs(1), async {
        loop {
            if service
                .read(&session_id)
                .await
                .expect("read session admission projection")
                .state
                .is_active
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("slow turn should claim admission");

    // Try to start another turn
    let result = service.start_turn(&session_id, turn_req("Fast")).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code(), "SESSION_BUSY");
    handle
        .await
        .expect("slow turn task should not panic")
        .expect("slow turn should complete");
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
async fn test_cancel_after_boundary_is_unsupported_without_exact_turn_state() {
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
    let error = service
        .cancel_after_boundary(&session_id)
        .await
        .expect_err("a busy standalone fixture must not synthesize exact run authority");
    assert!(matches!(
        error,
        SessionError::Unsupported(operation)
            if operation == "cancel_after_boundary_exact_run_authority"
    ));

    let result = turn.await.expect("turn join should succeed");
    assert!(
        result.is_ok(),
        "simple single-turn runs may still complete when no later cancellable boundary exists"
    );
}

#[tokio::test]
async fn test_turn_tool_overlay_is_cleared_after_canceled_turn() {
    let overlay_updates = Arc::new(std::sync::Mutex::new(Vec::new()));
    let service = Arc::new(EphemeralSessionService::new(
        MockAgentBuilder {
            delay_ms: Some(500),
            build_delay_ms: None,
            callback_pending: false,
            fail_overlay_clear: false,
            reject_system_messages: false,
            overlay_updates: overlay_updates.clone(),
            durable_identity: Some(test_llm_identity("mock")),
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
        allowed_tools: Some(vec!["alpha".into()]),
        blocked_tools: Some(vec!["beta".into()]),
        dispatch_context: Default::default(),
    };
    let turn = tokio::spawn(async move {
        service_clone
            .start_turn(
                &sid_clone,
                StartTurnRequest {
                    injected_context: Vec::new(),
                    runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                        HandlingMode::Queue,
                        Some(overlay),
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
        allowed_tools: Some(vec!["alpha".into()]),
        blocked_tools: Some(vec!["beta".into()]),
        dispatch_context: Default::default(),
    })));
    assert_eq!(updates.last().cloned(), Some(None));
}

#[tokio::test]
async fn test_turn_tool_overlay_enforced_by_runtime_and_resets_next_turn() {
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
                injected_context: Vec::new(),
                runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                    HandlingMode::Queue,
                    Some(TurnToolOverlay {
                        allowed_tools: Some(vec!["alpha".into(), "beta".into()]),
                        blocked_tools: Some(vec!["beta".into()]),
                        dispatch_context: Default::default(),
                    }),
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
                injected_context: Vec::new(),
                runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                    HandlingMode::Queue,
                    Some(TurnToolOverlay {
                        allowed_tools: Some(vec!["alpha".into()]),
                        blocked_tools: None,
                        dispatch_context: Default::default(),
                    }),
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
            allowed_tools: Some(vec!["alpha".into()]),
            blocked_tools: None,
            dispatch_context: Default::default(),
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
    assert!(
        output
            .whole_blob_bytes()
            .expect("callback-pending snapshot should encode")
            .is_some()
    );
    let Some(CoreApplyTerminal::CallbackPending {
        tool_use_id,
        tool_name,
        args,
    }) = output.terminal
    else {
        return Err("expected callback pending terminal".to_string());
    };
    assert_eq!(tool_use_id, "call-1");
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
    assert!(
        output
            .whole_blob_bytes()
            .expect("no-pending snapshot should encode")
            .is_some()
    );
    assert!(matches!(
        output.terminal,
        Some(CoreApplyTerminal::NoPendingBoundary)
    ));
    Ok(())
}

#[tokio::test]
async fn test_runtime_and_request_system_messages_append_once_in_order() {
    let provider_visible_system_prompts =
        Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
    let service = Arc::new(EphemeralSessionService::new(
        RealAgentBuilder {
            provider_visible_tools: Arc::new(std::sync::Mutex::new(Vec::new())),
            provider_visible_system_prompts: Arc::clone(&provider_visible_system_prompts),
            llm_delay_ms: None,
            hook_engine: None,
        },
        10,
    ));

    let created = service
        .create_session(create_req_deferred("ordered System messages"))
        .await
        .expect("create deferred session");
    let mut request = turn_req("run");
    request.runtime.turn_metadata = Some(
        meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
            system_prompts: vec!["runtime one".to_string(), "runtime two".to_string()],
            ..Default::default()
        },
    );
    request.system_prompt = Some("request System".to_string());

    service
        .start_turn(&created.session_id, request)
        .await
        .expect("ordered System messages should be admitted");

    assert_eq!(
        provider_visible_system_prompts
            .lock()
            .expect("provider prompt capture lock poisoned")
            .as_slice(),
        &[vec![
            "runtime one".to_string(),
            "runtime two".to_string(),
            "request System".to_string(),
        ]],
        "runtime metadata precedes the request field and no message is duplicated"
    );
}

#[tokio::test]
async fn test_rejected_system_batch_leaves_turn_boundary_unchanged() {
    let service = make_service(MockAgentBuilder::rejecting_system_messages());
    let created = service
        .create_session(create_req_deferred("atomic System batch"))
        .await
        .expect("create deferred session");
    let mut request = turn_req("must not append");
    request.injected_context = vec![ContentInput::Text("must not inject".to_string())];
    request.runtime.turn_metadata = Some(
        meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
            system_prompts: vec!["first".to_string(), "second".to_string()],
            ..Default::default()
        },
    );

    service
        .start_turn(&created.session_id, request)
        .await
        .expect_err("agent should reject the complete System batch");

    let view = service
        .read(&created.session_id)
        .await
        .expect("read rejected turn");
    assert_eq!(
        view.state.message_count, 0,
        "batch rejection must precede all System, injected-context, user, and assistant appends"
    );
}

#[tokio::test]
async fn test_eager_initial_turn_appends_runtime_system_messages_once_in_order() {
    let provider_visible_system_prompts =
        Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
    let service = EphemeralSessionService::new(
        RealAgentBuilder {
            provider_visible_tools: Arc::new(std::sync::Mutex::new(Vec::new())),
            provider_visible_system_prompts: Arc::clone(&provider_visible_system_prompts),
            llm_delay_ms: None,
            hook_engine: None,
        },
        10,
    );
    let mut request = create_req("eager turn");
    request.build = Some(SessionBuildOptions {
        initial_turn_metadata: Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                system_prompts: vec!["eager one".to_string(), "eager two".to_string()],
                ..Default::default()
            },
        ),
        ..Default::default()
    });

    service
        .create_session(request)
        .await
        .expect("eager turn with ordered System messages");

    assert_eq!(
        provider_visible_system_prompts
            .lock()
            .expect("provider prompt capture lock poisoned")
            .as_slice(),
        &[vec!["eager one".to_string(), "eager two".to_string()]],
        "the eager create path must apply initial metadata Systems exactly once"
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
async fn test_cancel_after_boundary_when_idle_is_unsupported_without_exact_turn_state() {
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
    assert!(matches!(
        err,
        SessionError::Unsupported(operation)
            if operation == "cancel_after_boundary_exact_run_authority"
    ));
}

// ---------------------------------------------------------------------------
// Session labels tests
// ---------------------------------------------------------------------------

fn create_req_with_labels(
    prompt: &str,
    labels: std::collections::BTreeMap<String, String>,
) -> CreateSessionRequest {
    CreateSessionRequest {
        injected_context: Vec::new(),
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
