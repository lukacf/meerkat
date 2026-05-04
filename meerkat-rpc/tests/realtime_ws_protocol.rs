//! Integration tests for the realtime websocket protocol shell.
//!
//! These tests stay at the websocket transport/product layer. They verify the
//! typed frame protocol plus the current session channel-host mapping without
//! promoting the websocket shell into the semantic owner.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    // Phase 1a added ~6 DSL fields to MeerkatMachineState (realtime +
    // live-topology), pushing several test futures past the default
    // clippy::large_futures 16384-byte stack budget. These are integration
    // tests; the heap/stack tradeoff doesn't matter here.
    clippy::large_futures
)]

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::{SinkExt, StreamExt, stream};
use meerkat::{AgentBuildConfig, AgentFactory};
use meerkat_client::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, RealtimeSession,
    RealtimeSessionEvent, RealtimeSessionFactory, realtime_session::RealtimeSessionOpenConfig,
};
use meerkat_contracts::{
    RealtimeAudioChunk, RealtimeCapabilities, RealtimeChannelConfig, RealtimeChannelErrorFrame,
    RealtimeChannelEventFrame, RealtimeChannelInputFrame, RealtimeChannelOpenFrame,
    RealtimeChannelRole, RealtimeChannelState, RealtimeChannelStatus, RealtimeChannelTarget,
    RealtimeClientFrame, RealtimeErrorCode, RealtimeEvent, RealtimeInputChunk, RealtimeInputKind,
    RealtimeOpenInfo, RealtimeOpenRequest, RealtimeOutputKind, RealtimeReconnectPolicy,
    RealtimeServerFrame, RealtimeTextChunk, RealtimeToolTimeoutPolicy, RealtimeTurningMode,
    WireContentInput, WireSessionMessage,
};
use meerkat_core::lifecycle::RunId;
use meerkat_core::lifecycle::core_executor::{
    CoreApplyOutput, CoreExecutor, CoreExecutorError, CoreExecutorInterruptHandle,
};
use meerkat_core::lifecycle::run_primitive::RunPrimitive;
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::ops::ToolDispatchOutcome;
use meerkat_core::session::ToolCategoryOverride;
use meerkat_core::{AgentToolDispatcher, ToolCallView, ToolDef, ToolError, ToolResult};
use meerkat_core::{Config, MemoryConfigStore, SessionHistoryQuery, StopReason};
use meerkat_rpc::session_executor::SessionRuntimeExecutor;
use meerkat_rpc::session_runtime::SessionRuntime;
use meerkat_rpc::{
    REALTIME_WS_PATH, RealtimeOpenGrant, RealtimeWsHost, serve_realtime_ws_listener,
};
use meerkat_runtime::service_ext::SessionServiceRuntimeExt;
use meerkat_runtime::{Input, PromptInput};
use tokio::sync::Notify;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

fn conservative_capabilities(turning_modes: Vec<RealtimeTurningMode>) -> RealtimeCapabilities {
    RealtimeCapabilities {
        input_kinds: vec![RealtimeInputKind::Text, RealtimeInputKind::Audio],
        output_kinds: vec![RealtimeOutputKind::Text, RealtimeOutputKind::Audio],
        turning_modes,
        interrupt_supported: true,
        transcript_supported: true,
        tool_lifecycle_events_supported: false,
        video_supported: false,
        audio_input_format: None,
        audio_output_format: None,
    }
}

fn build_test_runtime() -> (
    tempfile::TempDir,
    Arc<SessionRuntime>,
    Arc<dyn meerkat_core::ConfigStore>,
) {
    let temp = tempfile::tempdir().unwrap();
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let config = Config::default();
    let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let mut runtime = SessionRuntime::new(
        factory,
        config,
        10,
        meerkat::PersistenceBundle::new(
            store,
            None,
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        ),
        meerkat_rpc::router::NotificationSink::noop(),
    );
    runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
    let config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(MemoryConfigStore::new(Config::default()));
    (temp, Arc::new(runtime), config_store)
}

struct MockLlmClient;

#[async_trait::async_trait]
impl LlmClient for MockLlmClient {
    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        Box::pin(stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: "Hello from mock".to_string(),
                meta: None,
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
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

struct HangingToolDispatcher;

#[async_trait::async_trait]
impl AgentToolDispatcher for HangingToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([Arc::new(ToolDef {
            name: "slow".into(),
            description: "hangs until the dispatch timeout fires".to_string(),
            input_schema: serde_json::json!({ "type": "object" }),
            provenance: None,
        })])
    }

    async fn dispatch(&self, _call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        std::future::pending().await
    }
}

struct TimeoutShapedToolErrorDispatcher;

#[async_trait::async_trait]
impl AgentToolDispatcher for TimeoutShapedToolErrorDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([Arc::new(ToolDef {
            name: "spoof_timeout".into(),
            description: "returns an error payload shaped like a timeout".to_string(),
            input_schema: serde_json::json!({ "type": "object" }),
            provenance: None,
        })])
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Ok(ToolDispatchOutcome::sync_result(ToolResult::new(
            call.id.to_string(),
            "{\"error\":\"timeout\",\"message\":\"tool-authored timeout-shaped content\"}"
                .to_string(),
            true,
        )))
    }
}

struct FakeRealtimeSession {
    capabilities: RealtimeCapabilities,
    turning_mode: RealtimeTurningMode,
    scripted_events: std::collections::VecDeque<Result<Option<RealtimeSessionEvent>, LlmError>>,
    seen_inputs: Arc<tokio::sync::Mutex<Vec<RealtimeInputChunk>>>,
    seen_tool_results: Arc<tokio::sync::Mutex<Vec<ToolResult>>>,
    seen_tool_errors: Arc<tokio::sync::Mutex<Vec<(String, String)>>>,
    release_events_after_input: bool,
    release_events_after_tool_submission: bool,
    input_seen: Arc<AtomicBool>,
    input_gate: Arc<Notify>,
    tool_submission_seen: Arc<AtomicBool>,
    tool_submission_gate: Arc<Notify>,
}

#[async_trait::async_trait]
impl RealtimeSession for FakeRealtimeSession {
    fn capabilities(&self) -> &RealtimeCapabilities {
        &self.capabilities
    }

    fn turning_mode(&self) -> RealtimeTurningMode {
        self.turning_mode
    }

    async fn refresh_projection(
        &mut self,
        _open_config: &RealtimeSessionOpenConfig,
    ) -> Result<(), LlmError> {
        Ok(())
    }

    async fn send_input(&mut self, chunk: RealtimeInputChunk) -> Result<(), LlmError> {
        self.seen_inputs.lock().await.push(chunk);
        self.input_seen.store(true, Ordering::SeqCst);
        self.input_gate.notify_waiters();
        Ok(())
    }

    async fn commit_turn(&mut self) -> Result<(), LlmError> {
        Ok(())
    }

    async fn interrupt(&mut self) -> Result<(), LlmError> {
        Ok(())
    }

    async fn truncate_assistant_output(
        &mut self,
        _item_id: String,
        _content_index: u32,
        _audio_played_ms: u64,
    ) -> Result<(), LlmError> {
        Ok(())
    }

    async fn submit_tool_result(&mut self, result: ToolResult) -> Result<(), LlmError> {
        self.seen_tool_results.lock().await.push(result);
        self.tool_submission_seen.store(true, Ordering::SeqCst);
        self.tool_submission_gate.notify_waiters();
        Ok(())
    }

    async fn submit_tool_error(&mut self, call_id: String, error: String) -> Result<(), LlmError> {
        self.seen_tool_errors.lock().await.push((call_id, error));
        self.tool_submission_seen.store(true, Ordering::SeqCst);
        self.tool_submission_gate.notify_waiters();
        Ok(())
    }

    async fn next_event(&mut self) -> Result<Option<RealtimeSessionEvent>, LlmError> {
        if self.release_events_after_input && !self.input_seen.load(Ordering::SeqCst) {
            self.input_gate.notified().await;
        }
        if self.release_events_after_tool_submission
            && !self.tool_submission_seen.load(Ordering::SeqCst)
            && matches!(self.scripted_events.front(), Some(Ok(None)))
        {
            self.tool_submission_gate.notified().await;
        }
        self.scripted_events.pop_front().unwrap_or(Ok(None))
    }

    async fn close(&mut self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct FakeRealtimeSessionFactory {
    capabilities: RealtimeCapabilities,
    opened_sessions:
        tokio::sync::Mutex<std::collections::VecDeque<Result<Box<dyn RealtimeSession>, LlmError>>>,
    open_calls: Arc<tokio::sync::Mutex<usize>>,
    open_configs: Arc<tokio::sync::Mutex<Vec<RealtimeSessionOpenConfig>>>,
    attach_calls: Arc<tokio::sync::Mutex<usize>>,
}

#[async_trait::async_trait]
impl RealtimeSessionFactory for FakeRealtimeSessionFactory {
    fn capabilities(&self) -> RealtimeCapabilities {
        self.capabilities.clone()
    }

    async fn open_session(
        &self,
        open_config: &RealtimeSessionOpenConfig,
    ) -> Result<Box<dyn RealtimeSession>, LlmError> {
        *self.open_calls.lock().await += 1;
        self.open_configs.lock().await.push(open_config.clone());
        self.opened_sessions
            .lock()
            .await
            .pop_front()
            .unwrap_or_else(|| Err(LlmError::ConnectionReset))
    }

    async fn attach_external_session(
        &self,
        _target: &meerkat_client::RealtimeExternalSessionTarget,
        _turning_mode: RealtimeTurningMode,
    ) -> Result<Box<dyn RealtimeSession>, LlmError> {
        *self.attach_calls.lock().await += 1;
        Err(LlmError::InvalidRequest {
            message: "public websocket flow must not use external attach".to_string(),
        })
    }
}

struct NeverAppliedExecutor;

#[async_trait::async_trait]
impl CoreExecutor for NeverAppliedExecutor {
    async fn apply(
        &mut self,
        _run_id: RunId,
        _primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        unreachable!("P3.1 session-target websocket tests never drive apply()")
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

async fn create_realtime_session(runtime: &Arc<SessionRuntime>) -> meerkat_core::SessionId {
    create_realtime_session_with_external_tools(runtime, None).await
}

async fn create_realtime_session_with_external_tools(
    runtime: &Arc<SessionRuntime>,
    external_tools: Option<Arc<dyn AgentToolDispatcher>>,
) -> meerkat_core::SessionId {
    runtime
        .create_session(
            AgentBuildConfig {
                override_builtins: ToolCategoryOverride::Enable,
                external_tools,
                ..AgentBuildConfig::new("gpt-realtime")
            },
            None,
            None,
        )
        .await
        .expect("session should create")
}

async fn create_registered_realtime_session(
    runtime: &Arc<SessionRuntime>,
) -> meerkat_core::SessionId {
    create_registered_realtime_session_with_executor(runtime, Box::new(NeverAppliedExecutor)).await
}

async fn create_registered_realtime_session_with_executor(
    runtime: &Arc<SessionRuntime>,
    executor: Box<dyn CoreExecutor>,
) -> meerkat_core::SessionId {
    let session_id = create_realtime_session(runtime).await;
    register_live_session_with_executor(runtime, session_id.clone(), executor).await;
    session_id
}

async fn register_live_session_with_executor(
    runtime: &Arc<SessionRuntime>,
    session_id: meerkat_core::SessionId,
    executor: Box<dyn CoreExecutor>,
) {
    runtime
        .runtime_adapter()
        .register_session_with_executor(session_id, executor)
        .await;
}

async fn issue_open_info(
    host: &RealtimeWsHost,
    runtime: &Arc<SessionRuntime>,
    session_id: &str,
    role: RealtimeChannelRole,
    turning_mode: RealtimeTurningMode,
) -> RealtimeOpenInfo {
    issue_open_info_with_policy_and_config(
        host,
        runtime,
        session_id,
        role,
        turning_mode,
        None,
        None,
    )
    .await
}

async fn issue_open_info_with_policy(
    host: &RealtimeWsHost,
    runtime: &Arc<SessionRuntime>,
    session_id: &str,
    role: RealtimeChannelRole,
    turning_mode: RealtimeTurningMode,
    reconnect_policy: Option<RealtimeReconnectPolicy>,
) -> RealtimeOpenInfo {
    issue_open_info_with_policy_and_config(
        host,
        runtime,
        session_id,
        role,
        turning_mode,
        reconnect_policy,
        None,
    )
    .await
}

async fn issue_open_info_with_channel_config(
    host: &RealtimeWsHost,
    runtime: &Arc<SessionRuntime>,
    session_id: &str,
    role: RealtimeChannelRole,
    turning_mode: RealtimeTurningMode,
    channel_config: RealtimeChannelConfig,
) -> RealtimeOpenInfo {
    issue_open_info_with_policy_and_config(
        host,
        runtime,
        session_id,
        role,
        turning_mode,
        None,
        Some(channel_config),
    )
    .await
}

async fn issue_open_info_with_policy_and_config(
    host: &RealtimeWsHost,
    runtime: &Arc<SessionRuntime>,
    session_id: &str,
    role: RealtimeChannelRole,
    turning_mode: RealtimeTurningMode,
    reconnect_policy: Option<RealtimeReconnectPolicy>,
    channel_config: Option<RealtimeChannelConfig>,
) -> RealtimeOpenInfo {
    let parsed_session_id =
        meerkat_core::SessionId::parse(session_id).expect("session_id should parse");
    let eligibility = runtime
        .runtime_adapter()
        .realtime_bootstrap_eligibility(&parsed_session_id)
        .await
        .expect("session should be machine-eligible for realtime bootstrap");
    host.issue_open_info(
        RealtimeOpenRequest {
            target: RealtimeChannelTarget::SessionTarget {
                session_id: session_id.to_string(),
            },
            role,
            turning_mode,
            reconnect_policy,
            channel_config,
        },
        RealtimeOpenGrant::from_machine_eligibility(
            eligibility,
            conservative_capabilities(vec![
                RealtimeTurningMode::ProviderManaged,
                RealtimeTurningMode::ExplicitCommit,
            ]),
        ),
        None,
    )
    .await
}

async fn connect_and_open(
    ws_url: &str,
    info: &RealtimeOpenInfo,
    role: RealtimeChannelRole,
    turning_mode: RealtimeTurningMode,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let (mut ws_stream, _response) = connect_async(ws_url).await.expect("ws handshake");
    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelOpen(
                RealtimeChannelOpenFrame {
                    protocol_version: info.default_protocol_version,
                    open_token: info.open_token.clone(),
                    role,
                    turning_mode,
                },
            ))
            .expect("channel.open should serialize")
            .into(),
        ))
        .await
        .expect("channel.open should send");
    ws_stream
}

async fn read_server_frame(
    ws_stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> RealtimeServerFrame {
    let frame = ws_stream
        .next()
        .await
        .expect("expected websocket frame")
        .expect("websocket frame should arrive");
    match frame {
        WsMessage::Text(text) => {
            serde_json::from_str::<RealtimeServerFrame>(&text).expect("frame should deserialize")
        }
        other => panic!("expected websocket text frame, got {other:?}"),
    }
}

fn assert_channel_event(frame: RealtimeServerFrame, expected: RealtimeEvent) {
    match frame {
        RealtimeServerFrame::ChannelEvent(event_frame) => {
            assert_eq!(event_frame.event, expected);
        }
        other => panic!("expected channel.event, got {other:?}"),
    }
}

fn assert_error_frame(
    frame: RealtimeServerFrame,
    code: RealtimeErrorCode,
) -> RealtimeChannelErrorFrame {
    match frame {
        RealtimeServerFrame::ChannelError(error) => {
            assert_eq!(error.code, code);
            error
        }
        other => panic!("expected channel.error, got {other:?}"),
    }
}

async fn read_until_status(
    ws_stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> RealtimeChannelStatus {
    loop {
        match read_server_frame(ws_stream).await {
            RealtimeServerFrame::ChannelStatus(status_frame) => return status_frame.status,
            RealtimeServerFrame::ChannelEvent(_)
            | RealtimeServerFrame::ChannelError(_)
            | RealtimeServerFrame::ChannelClosed(_)
            | RealtimeServerFrame::ChannelOpened(_) => {}
        }
    }
}

async fn read_history(
    runtime: &Arc<SessionRuntime>,
    session_id: &str,
) -> meerkat_contracts::WireSessionHistory {
    let session_id = meerkat_core::SessionId::parse(session_id).expect("session_id should parse");
    runtime
        .read_session_history_rich(
            &session_id,
            SessionHistoryQuery {
                offset: 0,
                limit: None,
            },
        )
        .await
        .expect("session history should be readable")
}

async fn create_materialized_session(runtime: &Arc<SessionRuntime>) -> meerkat_core::SessionId {
    create_materialized_session_with_external_tools(runtime, None).await
}

async fn create_materialized_session_with_external_tools(
    runtime: &Arc<SessionRuntime>,
    external_tools: Option<Arc<dyn AgentToolDispatcher>>,
) -> meerkat_core::SessionId {
    let session_id = create_realtime_session_with_external_tools(runtime, external_tools).await;
    runtime
        .runtime_adapter()
        .register_session_with_executor(
            session_id.clone(),
            Box::new(SessionRuntimeExecutor::new(
                Arc::clone(runtime),
                session_id.clone(),
            )),
        )
        .await;
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(32);
    runtime
        .start_turn(
            &session_id,
            "materialize realtime target".into(),
            event_tx,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("session should materialize");
    session_id
}

#[tokio::test]
async fn channel_open_attaches_runtime_and_reports_opening_status() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_materialized_session(&runtime).await;
    let session_id_text = session_id.to_string();
    runtime
        .runtime_adapter()
        .apply_capability_driven_realtime_transport(&session_id)
        .await
        .expect("runtime should bind realtime-capable session");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let frame = read_server_frame(&mut ws_stream).await;
    match frame {
        RealtimeServerFrame::ChannelOpened(opened) => {
            assert_eq!(opened.protocol_version, open_info.default_protocol_version);
            assert_eq!(opened.role, RealtimeChannelRole::Primary);
            assert_eq!(
                opened.status,
                RealtimeChannelStatus {
                    state: RealtimeChannelState::Opening,
                    attempt_count: 0,
                    next_retry_at: None,
                    deadline_at: None,
                    reason: Some("realtime attachment is pending".to_string()),
                }
            );
        }
        other => panic!("expected channel.opened, got {other:?}"),
    }

    let runtime_status =
        <meerkat_runtime::MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
            runtime.runtime_adapter().as_ref(),
            &session_id,
        )
        .await
        .expect("registered session should expose runtime live status");
    assert_eq!(
        runtime_status,
        meerkat_runtime::RealtimeAttachmentStatus::BindingNotReady
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn channel_open_fails_closed_without_machine_bootstrap_eligibility_on_ws_runtime() {
    let (_mint_temp, mint_runtime, _mint_config_store) = build_test_runtime();
    let session_id = create_materialized_session(&mint_runtime).await;
    let session_id_text = session_id.to_string();
    let (_serve_temp, serve_runtime, config_store) = build_test_runtime();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        &mint_runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&serve_runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let error = assert_error_frame(
        read_server_frame(&mut ws_stream).await,
        RealtimeErrorCode::RuntimeNotReady,
    );
    assert!(
        error
            .message
            .contains("realtime bootstrap eligibility denied"),
        "unexpected channel.open error: {error:?}"
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn open_grant_fails_closed_when_projected_status_lacks_machine_capability_eligibility() {
    let (_temp, runtime, _config_store) = build_test_runtime();
    let session_id = meerkat_core::SessionId::new();
    let session_id_text = session_id.to_string();
    runtime
        .runtime_adapter()
        .register_session(session_id.clone())
        .await;
    runtime
        .runtime_adapter()
        .project_realtime_attachment_intent(&session_id, true)
        .await
        .expect("projected attachment intent should be recorded");
    let status = runtime
        .runtime_adapter()
        .realtime_channel_status(&session_id)
        .await
        .expect("projected channel status should remain readable");
    assert_eq!(status.state, RealtimeChannelState::Opening);

    if let Ok(eligibility) = runtime
        .runtime_adapter()
        .realtime_bootstrap_eligibility(&session_id)
        .await
    {
        let host = RealtimeWsHost::new("ws://127.0.0.1:4900/realtime/ws".to_string());
        let open_info = host
            .issue_open_info(
                RealtimeOpenRequest {
                    target: RealtimeChannelTarget::SessionTarget {
                        session_id: session_id_text,
                    },
                    role: RealtimeChannelRole::Primary,
                    turning_mode: RealtimeTurningMode::ProviderManaged,
                    reconnect_policy: None,
                    channel_config: None,
                },
                RealtimeOpenGrant::from_machine_eligibility(
                    eligibility,
                    conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
                ),
                None,
            )
            .await;
        panic!(
            "projected realtime status minted open_info without machine capability eligibility: {open_info:?}"
        );
    }
}

#[tokio::test]
async fn provider_managed_text_input_emits_transcript_events_and_commits_history() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_materialized_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let baseline_history = read_history(&runtime, &session_id_text).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let _opened = read_server_frame(&mut ws_stream).await;
    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelInput(
                RealtimeChannelInputFrame {
                    chunk: RealtimeInputChunk::TextChunk(RealtimeTextChunk {
                        text: "hello from provider managed".to_string(),
                    }),
                },
            ))
            .expect("channel.input should serialize")
            .into(),
        ))
        .await
        .expect("channel.input should send");
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::TurnStarted,
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::InputTranscriptPartial {
            text: "hello from provider managed".to_string(),
        },
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::InputTranscriptFinal {
            text: "hello from provider managed".to_string(),
            prosody_hint: None,
        },
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::TurnCommitted,
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::TurnCompleted,
    );

    let history = read_history(&runtime, &session_id_text).await;
    assert!(
        history.message_count > baseline_history.message_count,
        "provider-managed input should add committed transcript messages"
    );
    let new_messages = &history.messages[baseline_history.messages.len()..];
    assert!(
        new_messages.iter().any(|message| {
            matches!(
                message,
                WireSessionMessage::User { content: WireContentInput::Text(text), .. } if text == "hello from provider managed"
            )
        }),
        "provider-managed commit should append the user transcript to canonical history"
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn channel_commit_turn_is_rejected_for_provider_managed_channels() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_registered_realtime_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let _opened = read_server_frame(&mut ws_stream).await;
    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelCommitTurn)
                .expect("channel.commit_turn should serialize")
                .into(),
        ))
        .await
        .expect("channel.commit_turn should send");
    assert_error_frame(
        read_server_frame(&mut ws_stream).await,
        RealtimeErrorCode::CommitTurnUnavailable,
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn explicit_commit_text_input_stays_local_until_commit_turn() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_materialized_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let baseline_history = read_history(&runtime, &session_id_text).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ExplicitCommit,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ExplicitCommit,
    )
    .await;
    let _opened = read_server_frame(&mut ws_stream).await;
    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelInput(
                RealtimeChannelInputFrame {
                    chunk: RealtimeInputChunk::TextChunk(RealtimeTextChunk {
                        text: "hello".to_string(),
                    }),
                },
            ))
            .expect("first channel.input should serialize")
            .into(),
        ))
        .await
        .expect("first channel.input should send");
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::TurnStarted,
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::InputTranscriptPartial {
            text: "hello".to_string(),
        },
    );

    let history_before_commit = read_history(&runtime, &session_id_text).await;
    assert_eq!(
        history_before_commit.message_count, baseline_history.message_count,
        "explicit_commit input should not reach canonical history before commit_turn"
    );
    assert_eq!(
        serde_json::to_value(&history_before_commit.messages).unwrap(),
        serde_json::to_value(&baseline_history.messages).unwrap(),
    );

    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelInput(
                RealtimeChannelInputFrame {
                    chunk: RealtimeInputChunk::TextChunk(RealtimeTextChunk {
                        text: " world".to_string(),
                    }),
                },
            ))
            .expect("second channel.input should serialize")
            .into(),
        ))
        .await
        .expect("second channel.input should send");
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::InputTranscriptPartial {
            text: "hello world".to_string(),
        },
    );

    let history_still_uncommitted = read_history(&runtime, &session_id_text).await;
    assert_eq!(
        history_still_uncommitted.message_count,
        baseline_history.message_count
    );
    assert_eq!(
        serde_json::to_value(&history_still_uncommitted.messages).unwrap(),
        serde_json::to_value(&baseline_history.messages).unwrap(),
    );

    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelCommitTurn)
                .expect("channel.commit_turn should serialize")
                .into(),
        ))
        .await
        .expect("channel.commit_turn should send");
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::InputTranscriptFinal {
            text: "hello world".to_string(),
            prosody_hint: None,
        },
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::TurnCommitted,
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::TurnCompleted,
    );

    let history = read_history(&runtime, &session_id_text).await;
    assert!(
        history.message_count > baseline_history.message_count,
        "explicit commit should append canonical transcript messages after commit_turn"
    );
    let new_messages = &history.messages[baseline_history.messages.len()..];
    assert!(
        new_messages.iter().any(|message| {
            matches!(
                message,
                WireSessionMessage::User { content: WireContentInput::Text(text), .. } if text == "hello world"
            )
        }),
        "explicit commit should append the staged transcript to canonical history"
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn channel_open_rejects_unsupported_explicit_commit_turning_mode() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_registered_realtime_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let eligibility = runtime
        .runtime_adapter()
        .realtime_bootstrap_eligibility(&session_id)
        .await
        .expect("session should be machine-eligible for realtime bootstrap");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = host
        .issue_open_info(
            RealtimeOpenRequest {
                target: RealtimeChannelTarget::SessionTarget {
                    session_id: session_id_text,
                },
                role: RealtimeChannelRole::Primary,
                turning_mode: RealtimeTurningMode::ExplicitCommit,
                reconnect_policy: None,
                channel_config: None,
            },
            RealtimeOpenGrant::from_machine_eligibility(
                eligibility,
                conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
            ),
            None,
        )
        .await;
    let server_host = Arc::clone(&host);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, runtime, config_store).await
    });

    let (mut ws_stream, _response) = connect_async(&ws_url).await.expect("ws handshake");
    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelOpen(
                RealtimeChannelOpenFrame {
                    protocol_version: open_info.default_protocol_version,
                    open_token: open_info.open_token.clone(),
                    role: RealtimeChannelRole::Primary,
                    turning_mode: RealtimeTurningMode::ExplicitCommit,
                },
            ))
            .expect("channel.open should serialize")
            .into(),
        ))
        .await
        .expect("channel.open should send");
    assert_error_frame(
        read_server_frame(&mut ws_stream).await,
        RealtimeErrorCode::UnsupportedTurningMode,
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn observer_channels_receive_primary_events_and_remain_read_only() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_materialized_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let primary_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ExplicitCommit,
    )
    .await;
    let observer_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Observer,
        RealtimeTurningMode::ExplicitCommit,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut primary_ws = connect_and_open(
        &ws_url,
        &primary_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ExplicitCommit,
    )
    .await;
    let _primary_opened = read_server_frame(&mut primary_ws).await;

    let mut observer_ws = connect_and_open(
        &ws_url,
        &observer_info,
        RealtimeChannelRole::Observer,
        RealtimeTurningMode::ExplicitCommit,
    )
    .await;
    let _observer_opened = read_server_frame(&mut observer_ws).await;

    primary_ws
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelInput(
                RealtimeChannelInputFrame {
                    chunk: RealtimeInputChunk::TextChunk(RealtimeTextChunk {
                        text: "fanout".to_string(),
                    }),
                },
            ))
            .expect("channel.input should serialize")
            .into(),
        ))
        .await
        .expect("channel.input should send");
    assert_channel_event(
        read_server_frame(&mut primary_ws).await,
        RealtimeEvent::TurnStarted,
    );
    assert_channel_event(
        read_server_frame(&mut primary_ws).await,
        RealtimeEvent::InputTranscriptPartial {
            text: "fanout".to_string(),
        },
    );

    let observer_turn_started =
        tokio::time::timeout(Duration::from_secs(1), read_server_frame(&mut observer_ws))
            .await
            .expect("observer should receive the primary event fanout");
    assert_channel_event(observer_turn_started, RealtimeEvent::TurnStarted);
    let observer_partial =
        tokio::time::timeout(Duration::from_secs(1), read_server_frame(&mut observer_ws))
            .await
            .expect("observer should receive the transcript fanout");
    assert_channel_event(
        observer_partial,
        RealtimeEvent::InputTranscriptPartial {
            text: "fanout".to_string(),
        },
    );

    observer_ws
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelInput(
                RealtimeChannelInputFrame {
                    chunk: RealtimeInputChunk::TextChunk(RealtimeTextChunk {
                        text: "nope".to_string(),
                    }),
                },
            ))
            .expect("observer channel.input should serialize")
            .into(),
        ))
        .await
        .expect("observer channel.input should send");
    assert_error_frame(
        read_server_frame(&mut observer_ws).await,
        RealtimeErrorCode::ObserverReadOnly,
    );

    let _ = primary_ws.close(None).await;
    let _ = observer_ws.close(None).await;
    server.abort();
}

#[tokio::test]
async fn explicit_commit_disconnect_discards_uncommitted_transcript() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_materialized_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let baseline_history = read_history(&runtime, &session_id_text).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ExplicitCommit,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ExplicitCommit,
    )
    .await;
    let _opened = read_server_frame(&mut ws_stream).await;
    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelInput(
                RealtimeChannelInputFrame {
                    chunk: RealtimeInputChunk::TextChunk(RealtimeTextChunk {
                        text: "orphaned realtime input".to_string(),
                    }),
                },
            ))
            .expect("channel.input should serialize")
            .into(),
        ))
        .await
        .expect("channel.input should send");
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::TurnStarted,
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::InputTranscriptPartial {
            text: "orphaned realtime input".to_string(),
        },
    );

    let _ = ws_stream.close(None).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let history_after_disconnect = read_history(&runtime, &session_id_text).await;
    assert_eq!(
        history_after_disconnect.message_count, baseline_history.message_count,
        "disconnecting before commit_turn must not write staged transcript into canonical history"
    );
    assert_eq!(
        serde_json::to_value(&history_after_disconnect.messages).unwrap(),
        serde_json::to_value(&baseline_history.messages).unwrap(),
    );

    server.abort();
}

#[tokio::test]
async fn audio_input_uses_product_session_factory_and_streams_provider_events() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_materialized_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let baseline_history = read_history(&runtime, &session_id_text).await;
    let seen_inputs = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let attach_calls = Arc::new(tokio::sync::Mutex::new(0usize));
    let open_calls = Arc::new(tokio::sync::Mutex::new(0usize));
    let input_seen = Arc::new(AtomicBool::new(false));
    let input_gate = Arc::new(Notify::new());
    let session_factory = Arc::new(FakeRealtimeSessionFactory {
        capabilities: conservative_capabilities(vec![
            RealtimeTurningMode::ProviderManaged,
            RealtimeTurningMode::ExplicitCommit,
        ]),
        opened_sessions: tokio::sync::Mutex::new(std::collections::VecDeque::from(vec![Ok(
            Box::new(FakeRealtimeSession {
                capabilities: conservative_capabilities(vec![
                    RealtimeTurningMode::ProviderManaged,
                    RealtimeTurningMode::ExplicitCommit,
                ]),
                turning_mode: RealtimeTurningMode::ProviderManaged,
                scripted_events: std::collections::VecDeque::from(vec![
                    Ok(Some(RealtimeSessionEvent::TurnStarted)),
                    Ok(Some(RealtimeSessionEvent::InputTranscriptPartial {
                        text: "cedar".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::InputTranscriptFinalForItem {
                        item_id: "item_user".to_string(),
                        previous_item_id: None,
                        content_index: 0,
                        text: "cedar seven".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCommitted)),
                    Ok(Some(RealtimeSessionEvent::OutputTextDeltaForItem {
                        response_id: "resp_assistant".to_string(),
                        delta_id: "evt_assistant_ready".to_string(),
                        item_id: "item_assistant".to_string(),
                        previous_item_id: Some("item_user".to_string()),
                        content_index: 0,
                        delta: "ready".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::OutputTextDeltaForItem {
                        response_id: "resp_assistant".to_string(),
                        delta_id: "evt_assistant_ready".to_string(),
                        item_id: "item_assistant".to_string(),
                        previous_item_id: Some("item_user".to_string()),
                        content_index: 0,
                        delta: "ready".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::OutputAudioChunk {
                        chunk: RealtimeAudioChunk {
                            mime_type: "audio/pcm".to_string(),
                            sample_rate_hz: 24_000,
                            channels: 1,
                            data: "AAEC".to_string(),
                        },
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCompleted {
                        response_id: "resp_assistant".to_string(),
                        stop_reason: StopReason::EndTurn,
                        usage: meerkat_core::types::Usage::default(),
                    })),
                    Ok(None),
                ]),
                seen_inputs: Arc::clone(&seen_inputs),
                seen_tool_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_errors: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                release_events_after_input: true,
                release_events_after_tool_submission: false,
                input_seen: Arc::clone(&input_seen),
                input_gate: Arc::clone(&input_gate),
                tool_submission_seen: Arc::new(AtomicBool::new(false)),
                tool_submission_gate: Arc::new(Notify::new()),
            }) as Box<dyn RealtimeSession>,
        )])),
        open_calls: Arc::clone(&open_calls),
        open_configs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        attach_calls: Arc::clone(&attach_calls),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host =
        Arc::new(RealtimeWsHost::new(ws_url.clone()).with_session_factory(session_factory.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    match read_server_frame(&mut ws_stream).await {
        RealtimeServerFrame::ChannelOpened(opened) => {
            assert_eq!(opened.status.state, RealtimeChannelState::Ready);
        }
        other => panic!("expected channel.opened, got {other:?}"),
    }
    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelInput(
                RealtimeChannelInputFrame {
                    chunk: RealtimeInputChunk::AudioChunk(RealtimeAudioChunk {
                        mime_type: "audio/pcm".to_string(),
                        sample_rate_hz: 24_000,
                        channels: 1,
                        data: "AQID".to_string(),
                    }),
                },
            ))
            .expect("channel.input should serialize")
            .into(),
        ))
        .await
        .expect("channel.input should send");

    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::TurnStarted,
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::InputTranscriptPartial {
            text: "cedar".to_string(),
        },
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::InputTranscriptFinal {
            text: "cedar seven".to_string(),
            prosody_hint: None,
        },
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::TurnCommitted,
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::OutputTextDelta {
            delta: "ready".to_string(),
        },
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::OutputTextDelta {
            delta: "ready".to_string(),
        },
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::OutputAudioChunk {
            chunk: RealtimeAudioChunk {
                mime_type: "audio/pcm".to_string(),
                sample_rate_hz: 24_000,
                channels: 1,
                data: "AAEC".to_string(),
            },
        },
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::TurnCompleted,
    );

    let history = read_history(&runtime, &session_id_text).await;
    assert!(
        history.message_count > baseline_history.message_count,
        "provider session transcript should commit into canonical history"
    );
    let new_messages = &history.messages[baseline_history.messages.len()..];
    assert!(
        new_messages.iter().any(|message| {
            matches!(
                message,
                WireSessionMessage::BlockAssistant { blocks, .. }
                    if blocks.iter().any(|block| matches!(
                        block,
                        meerkat_contracts::WireAssistantBlock::Text { text, .. } if text == "ready"
                    ))
            ) || matches!(
                message,
                WireSessionMessage::Assistant { content, .. } if content == "ready"
            )
        }),
        "provider session output text should commit into canonical assistant history"
    );
    let seen_inputs = seen_inputs.lock().await;
    assert!(seen_inputs.iter().any(|chunk| matches!(
        chunk,
        RealtimeInputChunk::AudioChunk(RealtimeAudioChunk { data, .. }) if data == "AQID"
    )));
    assert_eq!(*attach_calls.lock().await, 0);
    let open_calls = *open_calls.lock().await;
    assert!(
        (1..=2).contains(&open_calls),
        "product-session tool-call continuation may reuse the existing provider session or reopen it once, got open_calls={open_calls}",
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn unkeyed_provider_transcript_events_are_projection_only_for_websocket() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_materialized_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let baseline_history = read_history(&runtime, &session_id_text).await;
    let session_factory = Arc::new(FakeRealtimeSessionFactory {
        capabilities: conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
        opened_sessions: tokio::sync::Mutex::new(std::collections::VecDeque::from(vec![Ok(
            Box::new(FakeRealtimeSession {
                capabilities: conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
                turning_mode: RealtimeTurningMode::ProviderManaged,
                scripted_events: std::collections::VecDeque::from(vec![
                    Ok(Some(RealtimeSessionEvent::TurnStarted)),
                    Ok(Some(RealtimeSessionEvent::InputTranscriptFinal {
                        text: "unkeyed user".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCommitted)),
                    Ok(Some(RealtimeSessionEvent::OutputTextDelta {
                        delta: "unkeyed assistant".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCompleted {
                        response_id: "resp_unkeyed".to_string(),
                        stop_reason: StopReason::EndTurn,
                        usage: meerkat_core::types::Usage::default(),
                    })),
                    Ok(None),
                ]),
                seen_inputs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_errors: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                release_events_after_input: true,
                release_events_after_tool_submission: false,
                input_seen: Arc::new(AtomicBool::new(false)),
                input_gate: Arc::new(Notify::new()),
                tool_submission_seen: Arc::new(AtomicBool::new(false)),
                tool_submission_gate: Arc::new(Notify::new()),
            }) as Box<dyn RealtimeSession>,
        )])),
        open_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
        open_configs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        attach_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()).with_session_factory(session_factory));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    match read_server_frame(&mut ws_stream).await {
        RealtimeServerFrame::ChannelOpened(opened) => {
            assert_eq!(opened.status.state, RealtimeChannelState::Ready);
        }
        other => panic!("expected channel.opened, got {other:?}"),
    }
    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelInput(
                RealtimeChannelInputFrame {
                    chunk: RealtimeInputChunk::AudioChunk(RealtimeAudioChunk {
                        mime_type: "audio/pcm".to_string(),
                        sample_rate_hz: 24_000,
                        channels: 1,
                        data: "AQID".to_string(),
                    }),
                },
            ))
            .expect("channel.input should serialize")
            .into(),
        ))
        .await
        .expect("channel.input should send");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut saw_completed = false;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = tokio::time::timeout(remaining, read_server_frame(&mut ws_stream))
            .await
            .expect("expected provider frames before timeout");
        match frame {
            RealtimeServerFrame::ChannelEvent(event_frame) => {
                if matches!(event_frame.event, RealtimeEvent::TurnCompleted) {
                    saw_completed = true;
                    break;
                }
            }
            RealtimeServerFrame::ChannelStatus(_) | RealtimeServerFrame::ChannelOpened(_) => {}
            RealtimeServerFrame::ChannelClosed(frame) => {
                panic!("unexpected channel close {frame:?}")
            }
            RealtimeServerFrame::ChannelError(error) => {
                panic!("unexpected channel error {error:?}")
            }
        }
    }
    assert!(
        saw_completed,
        "provider public turn completion should surface"
    );

    let history = read_history(&runtime, &session_id_text).await;
    assert_eq!(history.message_count, baseline_history.message_count);
    assert_eq!(
        serde_json::to_value(&history.messages).unwrap(),
        serde_json::to_value(&baseline_history.messages).unwrap(),
        "unkeyed websocket translation must not append canonical transcript content"
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
// Re-enabled 2026-04-19: the underlying race (tool-use subresponse
// boundary sometimes reconstructs the provider session mid-turn under
// CI load) is kept in check by `.config/nextest.toml`'s retries=2
// override until the peer-response / admission cluster changes in this
// PR settle the shell↔admission signal seam end-to-end. The earlier
// `#[ignore]` was CI hygiene from the B2 split (commit 31a2d55c3) and
// was never paired with a functional issue that survived retries.
async fn provider_tool_use_boundary_does_not_surface_public_turn_completed_or_flush_canonical_output()
 {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_materialized_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let baseline_history = read_history(&runtime, &session_id_text).await;
    let open_calls = Arc::new(tokio::sync::Mutex::new(0usize));
    let session_factory = Arc::new(FakeRealtimeSessionFactory {
        capabilities: conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
        opened_sessions: tokio::sync::Mutex::new(std::collections::VecDeque::from(vec![Ok(
            Box::new(FakeRealtimeSession {
                capabilities: conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
                turning_mode: RealtimeTurningMode::ProviderManaged,
                scripted_events: std::collections::VecDeque::from(vec![
                    Ok(Some(RealtimeSessionEvent::TurnStarted)),
                    Ok(Some(RealtimeSessionEvent::InputTranscriptFinalForItem {
                        item_id: "item_user".to_string(),
                        previous_item_id: None,
                        content_index: 0,
                        text: "cedar seven".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCommitted)),
                    Ok(Some(RealtimeSessionEvent::OutputTextDeltaForItem {
                        response_id: "resp_assistant".to_string(),
                        delta_id: "evt_assistant_asking".to_string(),
                        item_id: "item_assistant".to_string(),
                        previous_item_id: Some("item_user".to_string()),
                        content_index: 0,
                        delta: "asking ".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCompleted {
                        response_id: "resp_assistant".to_string(),
                        stop_reason: StopReason::ToolUse,
                        usage: meerkat_core::types::Usage::default(),
                    })),
                    Ok(Some(RealtimeSessionEvent::OutputTextDeltaForItem {
                        response_id: "resp_assistant".to_string(),
                        delta_id: "evt_assistant_done".to_string(),
                        item_id: "item_assistant".to_string(),
                        previous_item_id: Some("item_user".to_string()),
                        content_index: 0,
                        delta: "done".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCompleted {
                        response_id: "resp_assistant".to_string(),
                        stop_reason: StopReason::EndTurn,
                        usage: meerkat_core::types::Usage::default(),
                    })),
                    Ok(None),
                ]),
                seen_inputs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_errors: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                release_events_after_input: true,
                release_events_after_tool_submission: false,
                input_seen: Arc::new(AtomicBool::new(false)),
                input_gate: Arc::new(Notify::new()),
                tool_submission_seen: Arc::new(AtomicBool::new(false)),
                tool_submission_gate: Arc::new(Notify::new()),
            }) as Box<dyn RealtimeSession>,
        )])),
        open_calls: Arc::clone(&open_calls),
        open_configs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        attach_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()).with_session_factory(session_factory));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    match read_server_frame(&mut ws_stream).await {
        RealtimeServerFrame::ChannelOpened(opened) => {
            assert_eq!(opened.status.state, RealtimeChannelState::Ready);
        }
        other => panic!("expected channel.opened, got {other:?}"),
    }
    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelInput(
                RealtimeChannelInputFrame {
                    chunk: RealtimeInputChunk::AudioChunk(RealtimeAudioChunk {
                        mime_type: "audio/pcm".to_string(),
                        sample_rate_hz: 24_000,
                        channels: 1,
                        data: "AQID".to_string(),
                    }),
                },
            ))
            .expect("channel.input should serialize")
            .into(),
        ))
        .await
        .expect("channel.input should send");

    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::TurnStarted,
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::InputTranscriptFinal {
            text: "cedar seven".to_string(),
            prosody_hint: None,
        },
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::TurnCommitted,
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::OutputTextDelta {
            delta: "asking ".to_string(),
        },
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::OutputTextDelta {
            delta: "done".to_string(),
        },
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::TurnCompleted,
    );

    let history = read_history(&runtime, &session_id_text).await;
    assert!(
        history.message_count > baseline_history.message_count,
        "provider-backed final output should commit into canonical history"
    );
    let new_messages = &history.messages[baseline_history.messages.len()..];
    assert!(
        new_messages.iter().any(|message| {
            matches!(
                message,
                WireSessionMessage::BlockAssistant { blocks, .. }
                    if blocks.iter().any(|block| matches!(
                        block,
                        meerkat_contracts::WireAssistantBlock::Text { text, .. } if text == "asking done"
                    ))
            ) || matches!(
                message,
                WireSessionMessage::Assistant { content, .. } if content == "asking done"
            )
        }),
        "tool-use subresponse boundaries must not flush partial assistant text into canonical history"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        *open_calls.lock().await,
        1,
        "tool-use subresponse boundaries must not reconstruct the provider session mid-turn"
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn provider_interrupted_event_is_forwarded_as_public_channel_event() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_materialized_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let session_factory = Arc::new(FakeRealtimeSessionFactory {
        capabilities: conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
        opened_sessions: tokio::sync::Mutex::new(std::collections::VecDeque::from(vec![Ok(
            Box::new(FakeRealtimeSession {
                capabilities: conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
                turning_mode: RealtimeTurningMode::ProviderManaged,
                scripted_events: std::collections::VecDeque::from(vec![
                    Ok(Some(RealtimeSessionEvent::TurnStarted)),
                    Ok(Some(RealtimeSessionEvent::OutputAudioChunk {
                        chunk: RealtimeAudioChunk {
                            mime_type: "audio/pcm".to_string(),
                            sample_rate_hz: 24_000,
                            channels: 1,
                            data: "AAEC".to_string(),
                        },
                    })),
                    Ok(Some(RealtimeSessionEvent::Interrupted {
                        response_id: Some("resp_audio".to_string()),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCompleted {
                        response_id: "resp_audio".to_string(),
                        stop_reason: StopReason::Cancelled,
                        usage: meerkat_core::types::Usage::default(),
                    })),
                    Ok(None),
                ]),
                seen_inputs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_errors: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                release_events_after_input: true,
                release_events_after_tool_submission: false,
                input_seen: Arc::new(AtomicBool::new(false)),
                input_gate: Arc::new(Notify::new()),
                tool_submission_seen: Arc::new(AtomicBool::new(false)),
                tool_submission_gate: Arc::new(Notify::new()),
            }) as Box<dyn RealtimeSession>,
        )])),
        open_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
        open_configs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        attach_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()).with_session_factory(session_factory));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    match read_server_frame(&mut ws_stream).await {
        RealtimeServerFrame::ChannelOpened(opened) => {
            assert_eq!(opened.status.state, RealtimeChannelState::Ready);
        }
        other => panic!("expected channel.opened, got {other:?}"),
    }

    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelInput(
                RealtimeChannelInputFrame {
                    chunk: RealtimeInputChunk::AudioChunk(RealtimeAudioChunk {
                        mime_type: "audio/pcm".to_string(),
                        sample_rate_hz: 24_000,
                        channels: 1,
                        data: "AQID".to_string(),
                    }),
                },
            ))
            .expect("channel.input should serialize")
            .into(),
        ))
        .await
        .expect("channel.input should send");

    let mut saw_output_audio = false;
    let mut saw_interrupted = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = tokio::time::timeout(remaining, read_server_frame(&mut ws_stream))
            .await
            .expect("expected provider event frames before timeout");
        match frame {
            RealtimeServerFrame::ChannelEvent(event_frame) => match event_frame.event {
                RealtimeEvent::OutputAudioChunk { .. } => saw_output_audio = true,
                RealtimeEvent::Interrupted => {
                    saw_interrupted = true;
                    break;
                }
                _ => {}
            },
            RealtimeServerFrame::ChannelStatus(_) | RealtimeServerFrame::ChannelOpened(_) => {}
            RealtimeServerFrame::ChannelClosed(frame) => {
                panic!("unexpected channel close {frame:?}")
            }
            RealtimeServerFrame::ChannelError(error) => {
                panic!("unexpected channel error {error:?}")
            }
        }
    }

    assert!(
        saw_output_audio,
        "provider audio output should reach the public channel"
    );
    assert!(
        saw_interrupted,
        "provider Interrupted event should reach the public channel"
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn cancelled_provider_turn_does_not_surface_public_completion_or_commit_partial_output() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_materialized_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let baseline_history = read_history(&runtime, &session_id_text).await;
    let session_factory = Arc::new(FakeRealtimeSessionFactory {
        capabilities: conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
        opened_sessions: tokio::sync::Mutex::new(std::collections::VecDeque::from(vec![Ok(
            Box::new(FakeRealtimeSession {
                capabilities: conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
                turning_mode: RealtimeTurningMode::ProviderManaged,
                scripted_events: std::collections::VecDeque::from(vec![
                    Ok(Some(RealtimeSessionEvent::TurnStarted)),
                    Ok(Some(RealtimeSessionEvent::OutputTextDelta {
                        delta: "partial".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::Interrupted {
                        response_id: None,
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCompleted {
                        response_id: "resp_cancelled".to_string(),
                        stop_reason: StopReason::Cancelled,
                        usage: meerkat_core::types::Usage::default(),
                    })),
                    Ok(None),
                ]),
                seen_inputs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_errors: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                release_events_after_input: true,
                release_events_after_tool_submission: false,
                input_seen: Arc::new(AtomicBool::new(false)),
                input_gate: Arc::new(Notify::new()),
                tool_submission_seen: Arc::new(AtomicBool::new(false)),
                tool_submission_gate: Arc::new(Notify::new()),
            }) as Box<dyn RealtimeSession>,
        )])),
        open_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
        open_configs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        attach_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()).with_session_factory(session_factory));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    match read_server_frame(&mut ws_stream).await {
        RealtimeServerFrame::ChannelOpened(opened) => {
            assert_eq!(opened.status.state, RealtimeChannelState::Ready);
        }
        other => panic!("expected channel.opened, got {other:?}"),
    }
    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelInput(
                RealtimeChannelInputFrame {
                    chunk: RealtimeInputChunk::AudioChunk(RealtimeAudioChunk {
                        mime_type: "audio/pcm".to_string(),
                        sample_rate_hz: 24_000,
                        channels: 1,
                        data: "AQID".to_string(),
                    }),
                },
            ))
            .expect("channel.input should serialize")
            .into(),
        ))
        .await
        .expect("channel.input should send");

    let mut saw_partial = false;
    let mut saw_interrupted = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = tokio::time::timeout(remaining, read_server_frame(&mut ws_stream))
            .await
            .expect("expected provider cancellation frames before timeout");
        match frame {
            RealtimeServerFrame::ChannelEvent(event_frame) => match event_frame.event {
                RealtimeEvent::OutputTextDelta { delta } => {
                    assert_eq!(delta, "partial");
                    saw_partial = true;
                }
                RealtimeEvent::Interrupted => {
                    saw_interrupted = true;
                    break;
                }
                RealtimeEvent::TurnCompleted => {
                    panic!("cancelled provider turn must not surface as a public completed turn")
                }
                _ => {}
            },
            RealtimeServerFrame::ChannelClosed(_) => break,
            RealtimeServerFrame::ChannelStatus(_) | RealtimeServerFrame::ChannelOpened(_) => {}
            RealtimeServerFrame::ChannelError(error) => {
                panic!("unexpected channel error {error:?}")
            }
        }
    }

    assert!(
        saw_partial,
        "provider partial output should still reach the public channel"
    );
    assert!(
        saw_interrupted,
        "provider cancellation should still surface interruption semantics publicly"
    );
    let post_interrupt_deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    while tokio::time::Instant::now() < post_interrupt_deadline {
        let remaining =
            post_interrupt_deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = match tokio::time::timeout(remaining, read_server_frame(&mut ws_stream)).await {
            Ok(frame) => frame,
            Err(_) => break,
        };
        match frame {
            RealtimeServerFrame::ChannelEvent(RealtimeChannelEventFrame {
                event: RealtimeEvent::TurnCompleted,
            }) => {
                panic!("cancelled provider turn must not surface as a later public completed turn")
            }
            RealtimeServerFrame::ChannelClosed(_) => break,
            _ => {}
        }
    }

    let history_after_cancel = read_history(&runtime, &session_id_text).await;
    assert_eq!(
        history_after_cancel.message_count, baseline_history.message_count,
        "cancelled provider turn must not append partial assistant output into canonical history"
    );
    assert_eq!(
        serde_json::to_value(&history_after_cancel.messages).unwrap(),
        serde_json::to_value(&baseline_history.messages).unwrap(),
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn interrupted_provider_turn_followed_by_new_commit_appends_new_user_turn_and_final_output() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_materialized_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let baseline_history = read_history(&runtime, &session_id_text).await;
    let session_factory = Arc::new(FakeRealtimeSessionFactory {
        capabilities: conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
        opened_sessions: tokio::sync::Mutex::new(std::collections::VecDeque::from(vec![Ok(
            Box::new(FakeRealtimeSession {
                capabilities: conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
                turning_mode: RealtimeTurningMode::ProviderManaged,
                scripted_events: std::collections::VecDeque::from(vec![
                    Ok(Some(RealtimeSessionEvent::TurnStarted)),
                    Ok(Some(RealtimeSessionEvent::InputTranscriptFinalForItem {
                        item_id: "item_user_1".to_string(),
                        previous_item_id: None,
                        content_index: 0,
                        text: "ask analyst for the token".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCommitted)),
                    Ok(Some(RealtimeSessionEvent::OutputTextDeltaForItem {
                        response_id: "resp_interrupted".to_string(),
                        delta_id: "evt_assistant_interrupted".to_string(),
                        item_id: "item_assistant_1".to_string(),
                        previous_item_id: Some("item_user_1".to_string()),
                        content_index: 0,
                        delta: "the token is ".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::Interrupted {
                        response_id: Some("resp_interrupted".to_string()),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnStarted)),
                    Ok(Some(RealtimeSessionEvent::InputTranscriptFinalForItem {
                        item_id: "item_user_2".to_string(),
                        previous_item_id: Some("item_user_1".to_string()),
                        content_index: 0,
                        text: "stop and tell me the codeword and the token".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCommitted)),
                    Ok(Some(RealtimeSessionEvent::OutputTextDeltaForItem {
                        response_id: "resp_final".to_string(),
                        delta_id: "evt_assistant_final_1".to_string(),
                        item_id: "item_assistant_2".to_string(),
                        previous_item_id: Some("item_user_2".to_string()),
                        content_index: 0,
                        delta: "amber lantern. ".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::OutputTextDeltaForItem {
                        response_id: "resp_final".to_string(),
                        delta_id: "evt_assistant_final_2".to_string(),
                        item_id: "item_assistant_2".to_string(),
                        previous_item_id: Some("item_user_2".to_string()),
                        content_index: 0,
                        delta: "birch seventeen.".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCompleted {
                        response_id: "resp_final".to_string(),
                        stop_reason: StopReason::EndTurn,
                        usage: meerkat_core::types::Usage::default(),
                    })),
                    Ok(None),
                ]),
                seen_inputs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_errors: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                release_events_after_input: true,
                release_events_after_tool_submission: false,
                input_seen: Arc::new(AtomicBool::new(false)),
                input_gate: Arc::new(Notify::new()),
                tool_submission_seen: Arc::new(AtomicBool::new(false)),
                tool_submission_gate: Arc::new(Notify::new()),
            }) as Box<dyn RealtimeSession>,
        )])),
        open_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
        open_configs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        attach_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()).with_session_factory(session_factory));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    match read_server_frame(&mut ws_stream).await {
        RealtimeServerFrame::ChannelOpened(opened) => {
            assert_eq!(opened.status.state, RealtimeChannelState::Ready);
        }
        other => panic!("expected channel.opened, got {other:?}"),
    }

    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelInput(
                RealtimeChannelInputFrame {
                    chunk: RealtimeInputChunk::AudioChunk(RealtimeAudioChunk {
                        mime_type: "audio/pcm".to_string(),
                        sample_rate_hz: 24_000,
                        channels: 1,
                        data: "AQID".to_string(),
                    }),
                },
            ))
            .expect("channel.input should serialize")
            .into(),
        ))
        .await
        .expect("channel.input should send");

    let mut saw_interrupted = false;
    let mut saw_second_commit = false;
    let mut saw_turn_completed = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = tokio::time::timeout(remaining, read_server_frame(&mut ws_stream))
            .await
            .expect("expected scripted provider frames before timeout");
        match frame {
            RealtimeServerFrame::ChannelEvent(event_frame) => match event_frame.event {
                RealtimeEvent::Interrupted => saw_interrupted = true,
                RealtimeEvent::TurnCommitted if saw_interrupted => saw_second_commit = true,
                RealtimeEvent::TurnCompleted => {
                    saw_turn_completed = true;
                    break;
                }
                _ => {}
            },
            RealtimeServerFrame::ChannelStatus(_) | RealtimeServerFrame::ChannelOpened(_) => {}
            RealtimeServerFrame::ChannelClosed(frame) => {
                panic!("unexpected channel close {frame:?}")
            }
            RealtimeServerFrame::ChannelError(error) => {
                panic!("unexpected channel error {error:?}")
            }
        }
    }

    assert!(
        saw_interrupted,
        "provider interruption should surface publicly"
    );
    assert!(
        saw_second_commit,
        "post-interrupt committed turn should surface publicly"
    );
    assert!(
        saw_turn_completed,
        "post-interrupt committed turn should complete publicly"
    );

    let history = read_history(&runtime, &session_id_text).await;
    let new_messages = &history.messages[baseline_history.messages.len()..];
    let user_texts = new_messages
        .iter()
        .filter_map(|message| match message {
            WireSessionMessage::User {
                content: WireContentInput::Text(text),
                ..
            } => Some(text.clone()),
            WireSessionMessage::User {
                content: WireContentInput::Blocks(blocks),
                ..
            } => Some(
                blocks
                    .iter()
                    .filter_map(|block| match block {
                        meerkat_contracts::WireContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect::<Vec<_>>();
    let assistant_texts = new_messages
        .iter()
        .filter_map(|message| match message {
            WireSessionMessage::Assistant { content, .. } => Some(content.clone()),
            WireSessionMessage::BlockAssistant { blocks, .. } => Some(
                blocks
                    .iter()
                    .filter_map(|block| match block {
                        meerkat_contracts::WireAssistantBlock::Text { text, .. } => {
                            Some(text.as_str())
                        }
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert!(
        user_texts
            .iter()
            .any(|text| text == "ask analyst for the token"),
        "first provider-managed user turn must commit into canonical history: {history:?}"
    );
    assert!(
        user_texts
            .iter()
            .any(|text| text == "stop and tell me the codeword and the token"),
        "post-interrupt committed user turn must commit into canonical history: {history:?}"
    );
    assert!(
        !assistant_texts
            .iter()
            .any(|text| text.contains("the token is ")),
        "interrupted partial assistant output must not survive into canonical history: {history:?}"
    );
    assert!(
        assistant_texts
            .iter()
            .any(|text| text.contains("amber lantern") && text.contains("birch seventeen")),
        "post-interrupt final assistant output must commit into canonical history: {history:?}"
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn product_session_disconnect_reopens_via_session_factory() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_materialized_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let attach_calls = Arc::new(tokio::sync::Mutex::new(0usize));
    let open_calls = Arc::new(tokio::sync::Mutex::new(0usize));
    let first_input_seen = Arc::new(AtomicBool::new(false));
    let first_input_gate = Arc::new(Notify::new());
    let second_input_seen = Arc::new(AtomicBool::new(false));
    let second_input_gate = Arc::new(Notify::new());
    let session_factory = Arc::new(FakeRealtimeSessionFactory {
        capabilities: conservative_capabilities(vec![
            RealtimeTurningMode::ProviderManaged,
            RealtimeTurningMode::ExplicitCommit,
        ]),
        opened_sessions: tokio::sync::Mutex::new(std::collections::VecDeque::from(vec![
            Ok(Box::new(FakeRealtimeSession {
                capabilities: conservative_capabilities(vec![
                    RealtimeTurningMode::ProviderManaged,
                    RealtimeTurningMode::ExplicitCommit,
                ]),
                turning_mode: RealtimeTurningMode::ProviderManaged,
                scripted_events: std::collections::VecDeque::from(vec![
                    Ok(Some(RealtimeSessionEvent::TurnStarted)),
                    Ok(None),
                ]),
                seen_inputs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_errors: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                release_events_after_input: true,
                release_events_after_tool_submission: false,
                input_seen: Arc::clone(&first_input_seen),
                input_gate: Arc::clone(&first_input_gate),
                tool_submission_seen: Arc::new(AtomicBool::new(false)),
                tool_submission_gate: Arc::new(Notify::new()),
            }) as Box<dyn RealtimeSession>),
            Ok(Box::new(FakeRealtimeSession {
                capabilities: conservative_capabilities(vec![
                    RealtimeTurningMode::ProviderManaged,
                    RealtimeTurningMode::ExplicitCommit,
                ]),
                turning_mode: RealtimeTurningMode::ProviderManaged,
                scripted_events: std::collections::VecDeque::from(vec![
                    Ok(Some(RealtimeSessionEvent::OutputTextDelta {
                        delta: "again".to_string(),
                    })),
                    Ok(None),
                ]),
                seen_inputs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_errors: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                release_events_after_input: true,
                release_events_after_tool_submission: false,
                input_seen: Arc::clone(&second_input_seen),
                input_gate: Arc::clone(&second_input_gate),
                tool_submission_seen: Arc::new(AtomicBool::new(false)),
                tool_submission_gate: Arc::new(Notify::new()),
            }) as Box<dyn RealtimeSession>),
        ])),
        open_calls: Arc::clone(&open_calls),
        open_configs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        attach_calls: Arc::clone(&attach_calls),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host =
        Arc::new(RealtimeWsHost::new(ws_url.clone()).with_session_factory(session_factory.clone()));
    let open_info = issue_open_info_with_policy(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
        Some(RealtimeReconnectPolicy {
            max_attempts: 3,
            initial_backoff_ms: 10,
            max_backoff_ms: 10,
            max_total_ms: 5_000,
        }),
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    match read_server_frame(&mut ws_stream).await {
        RealtimeServerFrame::ChannelOpened(opened) => {
            assert_eq!(opened.status.state, RealtimeChannelState::Ready);
        }
        other => panic!("expected channel.opened, got {other:?}"),
    }

    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelInput(
                RealtimeChannelInputFrame {
                    chunk: RealtimeInputChunk::AudioChunk(RealtimeAudioChunk {
                        mime_type: "audio/pcm".to_string(),
                        sample_rate_hz: 24_000,
                        channels: 1,
                        data: "AQID".to_string(),
                    }),
                },
            ))
            .expect("channel.input should serialize")
            .into(),
        ))
        .await
        .expect("channel.input should send");
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::TurnStarted,
    );

    let reconnecting = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match read_server_frame(&mut ws_stream).await {
                RealtimeServerFrame::ChannelStatus(frame)
                    if frame.status.state == RealtimeChannelState::Reconnecting =>
                {
                    return frame.status;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("channel should enter reconnecting after provider session closes");
    assert_eq!(reconnecting.attempt_count, 1);

    let ready = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match read_server_frame(&mut ws_stream).await {
                RealtimeServerFrame::ChannelStatus(frame)
                    if frame.status.state == RealtimeChannelState::Ready =>
                {
                    return frame.status;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("channel should return to ready after reopening the provider session");
    assert_eq!(ready.state, RealtimeChannelState::Ready);

    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelInput(
                RealtimeChannelInputFrame {
                    chunk: RealtimeInputChunk::AudioChunk(RealtimeAudioChunk {
                        mime_type: "audio/pcm".to_string(),
                        sample_rate_hz: 24_000,
                        channels: 1,
                        data: "BAUG".to_string(),
                    }),
                },
            ))
            .expect("second channel.input should serialize")
            .into(),
        ))
        .await
        .expect("second channel.input should send");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match read_server_frame(&mut ws_stream).await {
                RealtimeServerFrame::ChannelEvent(RealtimeChannelEventFrame {
                    event: RealtimeEvent::OutputTextDelta { delta },
                }) if delta == "again" => break,
                _ => {}
            }
        }
    })
    .await
    .expect("reopened provider session should stream output after the second input");

    assert_eq!(*attach_calls.lock().await, 0);
    let open_calls = *open_calls.lock().await;
    assert!(
        (1..=2).contains(&open_calls),
        "product-session disconnect recovery may reuse the provider session or reopen it once, got open_calls={open_calls}",
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn product_session_tool_call_routes_through_session_service_and_continues_provider() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_materialized_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let attach_calls = Arc::new(tokio::sync::Mutex::new(0usize));
    let open_calls = Arc::new(tokio::sync::Mutex::new(0usize));
    let seen_tool_results = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let seen_tool_errors = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let session_factory = Arc::new(FakeRealtimeSessionFactory {
        capabilities: conservative_capabilities(vec![
            RealtimeTurningMode::ProviderManaged,
            RealtimeTurningMode::ExplicitCommit,
        ]),
        opened_sessions: tokio::sync::Mutex::new(std::collections::VecDeque::from(vec![Ok(
            Box::new(FakeRealtimeSession {
                capabilities: conservative_capabilities(vec![
                    RealtimeTurningMode::ProviderManaged,
                    RealtimeTurningMode::ExplicitCommit,
                ]),
                turning_mode: RealtimeTurningMode::ProviderManaged,
                scripted_events: std::collections::VecDeque::from(vec![
                    Ok(Some(RealtimeSessionEvent::ToolCallRequested {
                        call_id: "call_tool_ok".to_string(),
                        tool_name: "datetime".to_string(),
                        arguments: serde_json::json!({}),
                    })),
                    Ok(None),
                ]),
                seen_inputs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_results: Arc::clone(&seen_tool_results),
                seen_tool_errors: Arc::clone(&seen_tool_errors),
                release_events_after_input: false,
                release_events_after_tool_submission: true,
                input_seen: Arc::new(AtomicBool::new(false)),
                input_gate: Arc::new(Notify::new()),
                tool_submission_seen: Arc::new(AtomicBool::new(false)),
                tool_submission_gate: Arc::new(Notify::new()),
            }) as Box<dyn RealtimeSession>,
        )])),
        open_calls: Arc::clone(&open_calls),
        open_configs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        attach_calls: Arc::clone(&attach_calls),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host =
        Arc::new(RealtimeWsHost::new(ws_url.clone()).with_session_factory(session_factory.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    match read_server_frame(&mut ws_stream).await {
        RealtimeServerFrame::ChannelOpened(opened) => {
            assert_eq!(opened.status.state, RealtimeChannelState::Ready);
        }
        other => panic!("expected channel.opened, got {other:?}"),
    }
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::ToolCallRequested {
            call_id: "call_tool_ok".to_string(),
            tool_name: "datetime".to_string(),
        },
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::ToolCallCompleted {
            call_id: "call_tool_ok".to_string(),
        },
    );

    let seen_tool_results = seen_tool_results.lock().await;
    assert_eq!(seen_tool_results.len(), 1);
    assert_eq!(seen_tool_results[0].tool_use_id, "call_tool_ok");
    assert!(
        seen_tool_results[0].text_content().contains("\"iso8601\""),
        "provider continuation should receive the tool dispatch result"
    );
    assert!(seen_tool_errors.lock().await.is_empty());
    assert_eq!(*attach_calls.lock().await, 0);
    let open_calls = *open_calls.lock().await;
    assert!(
        (1..=2).contains(&open_calls),
        "product-session tool-call continuation may reuse the existing provider session or reopen it once, got open_calls={open_calls}",
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn product_session_tool_call_failures_emit_failed_event_and_submit_provider_error() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_materialized_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let seen_tool_results = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let seen_tool_errors = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let session_factory = Arc::new(FakeRealtimeSessionFactory {
        capabilities: conservative_capabilities(vec![
            RealtimeTurningMode::ProviderManaged,
            RealtimeTurningMode::ExplicitCommit,
        ]),
        opened_sessions: tokio::sync::Mutex::new(std::collections::VecDeque::from(vec![Ok(
            Box::new(FakeRealtimeSession {
                capabilities: conservative_capabilities(vec![
                    RealtimeTurningMode::ProviderManaged,
                    RealtimeTurningMode::ExplicitCommit,
                ]),
                turning_mode: RealtimeTurningMode::ProviderManaged,
                scripted_events: std::collections::VecDeque::from(vec![
                    Ok(Some(RealtimeSessionEvent::ToolCallRequested {
                        call_id: "call_tool_fail".to_string(),
                        tool_name: "missing_tool".to_string(),
                        arguments: serde_json::json!({}),
                    })),
                    Ok(None),
                ]),
                seen_inputs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_results: Arc::clone(&seen_tool_results),
                seen_tool_errors: Arc::clone(&seen_tool_errors),
                release_events_after_input: false,
                release_events_after_tool_submission: true,
                input_seen: Arc::new(AtomicBool::new(false)),
                input_gate: Arc::new(Notify::new()),
                tool_submission_seen: Arc::new(AtomicBool::new(false)),
                tool_submission_gate: Arc::new(Notify::new()),
            }) as Box<dyn RealtimeSession>,
        )])),
        open_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
        open_configs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        attach_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host =
        Arc::new(RealtimeWsHost::new(ws_url.clone()).with_session_factory(session_factory.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    match read_server_frame(&mut ws_stream).await {
        RealtimeServerFrame::ChannelOpened(opened) => {
            assert_eq!(opened.status.state, RealtimeChannelState::Ready);
        }
        other => panic!("expected channel.opened, got {other:?}"),
    }
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::ToolCallRequested {
            call_id: "call_tool_fail".to_string(),
            tool_name: "missing_tool".to_string(),
        },
    );
    let failed = match read_server_frame(&mut ws_stream).await {
        RealtimeServerFrame::ChannelEvent(RealtimeChannelEventFrame {
            event: RealtimeEvent::ToolCallFailed { call_id, error },
        }) => (call_id, error),
        other => panic!("expected ToolCallFailed event, got {other:?}"),
    };
    assert_eq!(failed.0, "call_tool_fail");
    assert!(
        failed.1.contains("missing_tool"),
        "tool failure should surface the rejected tool name, got {}",
        failed.1
    );

    assert!(seen_tool_results.lock().await.is_empty());
    let seen_tool_errors = seen_tool_errors.lock().await;
    assert_eq!(seen_tool_errors.len(), 1);
    assert_eq!(seen_tool_errors[0].0, "call_tool_fail");
    assert!(
        seen_tool_errors[0].1.contains("missing_tool"),
        "provider error payload should include the dispatch failure"
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn product_session_tool_timeout_projects_runtime_terminalization() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_materialized_session_with_external_tools(
        &runtime,
        Some(Arc::new(HangingToolDispatcher) as Arc<dyn AgentToolDispatcher>),
    )
    .await;
    let session_id_text = session_id.to_string();
    let open_calls = Arc::new(tokio::sync::Mutex::new(0usize));
    let attach_calls = Arc::new(tokio::sync::Mutex::new(0usize));
    let seen_tool_results = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let seen_tool_errors = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let session_factory = Arc::new(FakeRealtimeSessionFactory {
        capabilities: conservative_capabilities(vec![
            RealtimeTurningMode::ProviderManaged,
            RealtimeTurningMode::ExplicitCommit,
        ]),
        opened_sessions: tokio::sync::Mutex::new(std::collections::VecDeque::from(vec![Ok(
            Box::new(FakeRealtimeSession {
                capabilities: conservative_capabilities(vec![
                    RealtimeTurningMode::ProviderManaged,
                    RealtimeTurningMode::ExplicitCommit,
                ]),
                turning_mode: RealtimeTurningMode::ProviderManaged,
                scripted_events: std::collections::VecDeque::from(vec![
                    Ok(Some(RealtimeSessionEvent::ToolCallRequested {
                        call_id: "call_tool_timeout".to_string(),
                        tool_name: "slow".to_string(),
                        arguments: serde_json::json!({}),
                    })),
                    Ok(None),
                ]),
                seen_inputs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_results: Arc::clone(&seen_tool_results),
                seen_tool_errors: Arc::clone(&seen_tool_errors),
                release_events_after_input: false,
                release_events_after_tool_submission: true,
                input_seen: Arc::new(AtomicBool::new(false)),
                input_gate: Arc::new(Notify::new()),
                tool_submission_seen: Arc::new(AtomicBool::new(false)),
                tool_submission_gate: Arc::new(Notify::new()),
            }) as Box<dyn RealtimeSession>,
        )])),
        open_calls: Arc::clone(&open_calls),
        open_configs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        attach_calls: Arc::clone(&attach_calls),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host =
        Arc::new(RealtimeWsHost::new(ws_url.clone()).with_session_factory(session_factory.clone()));
    let open_info = issue_open_info_with_channel_config(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
        RealtimeChannelConfig {
            tool_timeout: RealtimeToolTimeoutPolicy::Finite { timeout_ms: 10 },
        },
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    match read_server_frame(&mut ws_stream).await {
        RealtimeServerFrame::ChannelOpened(opened) => {
            assert_eq!(opened.status.state, RealtimeChannelState::Ready);
        }
        other => panic!("expected channel.opened, got {other:?}"),
    }
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::ToolCallRequested {
            call_id: "call_tool_timeout".to_string(),
            tool_name: "slow".to_string(),
        },
    );
    match read_server_frame(&mut ws_stream).await {
        RealtimeServerFrame::ChannelEvent(RealtimeChannelEventFrame {
            event:
                RealtimeEvent::ToolCallTimedOut {
                    call_id,
                    elapsed_ms,
                },
        }) => {
            assert_eq!(call_id, "call_tool_timeout");
            assert!(
                elapsed_ms >= 10,
                "timeout projection should carry elapsed runtime, got {elapsed_ms}ms",
            );
        }
        other => panic!("expected ToolCallTimedOut event, got {other:?}"),
    }

    assert!(seen_tool_results.lock().await.is_empty());
    let seen_tool_errors = seen_tool_errors.lock().await;
    assert_eq!(seen_tool_errors.len(), 1);
    assert_eq!(seen_tool_errors[0].0, "call_tool_timeout");
    let expected = meerkat_core::ops::terminal_tool_outcome_for_error(
        "call_tool_timeout",
        ToolError::timeout("slow", 10),
    )
    .result
    .text_content();
    assert_eq!(
        seen_tool_errors[0].1, expected,
        "provider continuation must receive the runtime-authored canonical timeout payload",
    );
    let payload: serde_json::Value =
        serde_json::from_str(&seen_tool_errors[0].1).expect("timeout payload should be JSON");
    assert_eq!(payload["error"], "timeout");

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn product_session_tool_error_shaped_like_timeout_projects_failed_not_timed_out() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_materialized_session_with_external_tools(
        &runtime,
        Some(Arc::new(TimeoutShapedToolErrorDispatcher) as Arc<dyn AgentToolDispatcher>),
    )
    .await;
    let session_id_text = session_id.to_string();
    let seen_tool_results = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let seen_tool_errors = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let session_factory = Arc::new(FakeRealtimeSessionFactory {
        capabilities: conservative_capabilities(vec![
            RealtimeTurningMode::ProviderManaged,
            RealtimeTurningMode::ExplicitCommit,
        ]),
        opened_sessions: tokio::sync::Mutex::new(std::collections::VecDeque::from(vec![Ok(
            Box::new(FakeRealtimeSession {
                capabilities: conservative_capabilities(vec![
                    RealtimeTurningMode::ProviderManaged,
                    RealtimeTurningMode::ExplicitCommit,
                ]),
                turning_mode: RealtimeTurningMode::ProviderManaged,
                scripted_events: std::collections::VecDeque::from(vec![
                    Ok(Some(RealtimeSessionEvent::ToolCallRequested {
                        call_id: "call_tool_spoof_timeout".to_string(),
                        tool_name: "spoof_timeout".to_string(),
                        arguments: serde_json::json!({}),
                    })),
                    Ok(None),
                ]),
                seen_inputs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_results: Arc::clone(&seen_tool_results),
                seen_tool_errors: Arc::clone(&seen_tool_errors),
                release_events_after_input: false,
                release_events_after_tool_submission: true,
                input_seen: Arc::new(AtomicBool::new(false)),
                input_gate: Arc::new(Notify::new()),
                tool_submission_seen: Arc::new(AtomicBool::new(false)),
                tool_submission_gate: Arc::new(Notify::new()),
            }) as Box<dyn RealtimeSession>,
        )])),
        open_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
        open_configs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        attach_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host =
        Arc::new(RealtimeWsHost::new(ws_url.clone()).with_session_factory(session_factory.clone()));
    let open_info = issue_open_info_with_channel_config(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
        RealtimeChannelConfig {
            tool_timeout: RealtimeToolTimeoutPolicy::Finite { timeout_ms: 1_000 },
        },
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    match read_server_frame(&mut ws_stream).await {
        RealtimeServerFrame::ChannelOpened(opened) => {
            assert_eq!(opened.status.state, RealtimeChannelState::Ready);
        }
        other => panic!("expected channel.opened, got {other:?}"),
    }
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::ToolCallRequested {
            call_id: "call_tool_spoof_timeout".to_string(),
            tool_name: "spoof_timeout".to_string(),
        },
    );
    let failed = match read_server_frame(&mut ws_stream).await {
        RealtimeServerFrame::ChannelEvent(RealtimeChannelEventFrame {
            event: RealtimeEvent::ToolCallFailed { call_id, error },
        }) => (call_id, error),
        other => panic!("expected ToolCallFailed event, got {other:?}"),
    };
    assert_eq!(failed.0, "call_tool_spoof_timeout");
    assert!(
        failed.1.contains("\"error\":\"timeout\""),
        "timeout-shaped tool-authored payload should be surfaced as ordinary failure, got {}",
        failed.1
    );

    assert!(seen_tool_results.lock().await.is_empty());
    let seen_tool_errors = seen_tool_errors.lock().await;
    assert_eq!(seen_tool_errors.len(), 1);
    assert_eq!(seen_tool_errors[0].0, "call_tool_spoof_timeout");
    assert_eq!(seen_tool_errors[0].1, failed.1);

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn client_cannot_submit_tool_results_directly_over_realtime_protocol() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_registered_realtime_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let _opened = read_server_frame(&mut ws_stream).await;
    ws_stream
        .send(WsMessage::Text(
            serde_json::json!({
                "type": "channel.tool_result",
                "call_id": "call_1",
                "output": "client supplied"
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("raw client frame should send");
    assert_error_frame(
        read_server_frame(&mut ws_stream).await,
        RealtimeErrorCode::InvalidFrame,
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn channel_interrupt_routes_to_runtime_control_for_active_session() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct BlockingExecutor {
        apply_started: Arc<Notify>,
        apply_finished: Arc<Notify>,
        allow_finish: Arc<Notify>,
        cancel_calls: Arc<AtomicUsize>,
    }

    struct BlockingExecutorInterruptHandle {
        cancel_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CoreExecutorInterruptHandle for BlockingExecutorInterruptHandle {
        async fn hard_cancel_current_run(&self, _reason: String) -> Result<(), CoreExecutorError> {
            self.cancel_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
        fn interrupt_handle(&self) -> Option<Arc<dyn CoreExecutorInterruptHandle>> {
            Some(Arc::new(BlockingExecutorInterruptHandle {
                cancel_calls: Arc::clone(&self.cancel_calls),
            }))
        }

        async fn apply(
            &mut self,
            run_id: RunId,
            primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            self.apply_started.notify_waiters();
            self.allow_finish.notified().await;
            self.apply_finished.notify_waiters();
            Ok(CoreApplyOutput::without_terminal(
                RunBoundaryReceipt {
                    run_id,
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                    sequence: 0,
                },
                None,
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_realtime_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());
    let cancel_calls = Arc::new(AtomicUsize::new(0));
    runtime
        .runtime_adapter()
        .register_session_with_executor(
            session_id.clone(),
            Box::new(BlockingExecutor {
                apply_started: Arc::clone(&apply_started),
                apply_finished: Arc::clone(&apply_finished),
                allow_finish: Arc::clone(&allow_finish),
                cancel_calls: Arc::clone(&cancel_calls),
            }),
        )
        .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let (_outcome, completion_handle) = runtime
        .runtime_adapter()
        .accept_input_with_completion(
            &session_id,
            Input::Prompt(PromptInput::new("interrupt me", None)),
        )
        .await
        .expect("runtime should accept prompt");
    let completion_handle =
        completion_handle.expect("attached runtime should expose completion handle");
    tokio::time::timeout(std::time::Duration::from_secs(1), apply_started.notified())
        .await
        .expect("executor should start apply");

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let _opened = read_server_frame(&mut ws_stream).await;
    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelInterrupt)
                .expect("channel.interrupt should serialize")
                .into(),
        ))
        .await
        .expect("channel.interrupt should send");
    let maybe_frame =
        tokio::time::timeout(std::time::Duration::from_millis(50), ws_stream.next()).await;
    assert!(
        maybe_frame.is_err(),
        "successful interrupt should not emit an immediate channel.error frame: {maybe_frame:?}"
    );

    allow_finish.notify_waiters();
    tokio::time::timeout(std::time::Duration::from_secs(1), apply_finished.notified())
        .await
        .expect("executor apply should finish after release");
    completion_handle.wait().await;
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if cancel_calls.load(Ordering::SeqCst) == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("interrupt should eventually drain through the runtime control path");

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn reattach_required_primary_channel_retries_and_returns_to_opening_status() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id_value = create_materialized_session(&runtime).await;
    let session_id = session_id_value.to_string();
    runtime
        .runtime_adapter()
        .apply_capability_driven_realtime_transport(&session_id_value)
        .await
        .expect("runtime should bind realtime-capable session");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info_with_policy(
        host.as_ref(),
        &runtime,
        &session_id,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
        Some(RealtimeReconnectPolicy {
            max_attempts: 3,
            initial_backoff_ms: 5,
            max_backoff_ms: 20,
            max_total_ms: 100,
        }),
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let _opened = read_server_frame(&mut ws_stream).await;

    runtime
        .runtime_adapter()
        .require_realtime_attachment_reattach(&session_id_value)
        .await
        .expect("reattach requirement should succeed");

    let reconnecting = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        read_until_status(&mut ws_stream),
    )
    .await
    .expect("channel should surface reconnecting status");
    assert_eq!(reconnecting.state, RealtimeChannelState::Reconnecting);
    assert_eq!(reconnecting.attempt_count, 1);
    assert_eq!(
        reconnecting.reason.as_deref(),
        Some("realtime attachment requires reattach")
    );
    assert!(
        reconnecting.next_retry_at.is_some(),
        "reconnect status should surface the scheduled retry timestamp"
    );

    let reopening = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let status = read_until_status(&mut ws_stream).await;
            if status.state == RealtimeChannelState::Opening {
                break status;
            }
        }
    })
    .await
    .expect("channel should return to opening after a successful retry");
    assert_eq!(reopening.attempt_count, 0);
    assert_eq!(
        reopening.reason.as_deref(),
        Some("realtime attachment is pending")
    );
    assert_eq!(reopening.next_retry_at, None);

    let runtime_status =
        <meerkat_runtime::MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
            runtime.runtime_adapter().as_ref(),
            &session_id_value,
        )
        .await
        .expect("registered session should expose runtime live status");
    assert_eq!(
        runtime_status,
        meerkat_runtime::RealtimeAttachmentStatus::BindingNotReady
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn observer_status_poll_preserves_machine_owned_reconnect_progress() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id_value = create_materialized_session(&runtime).await;
    let session_id = session_id_value.to_string();
    runtime
        .runtime_adapter()
        .apply_capability_driven_realtime_transport(&session_id_value)
        .await
        .expect("runtime should bind realtime-capable session");
    runtime
        .runtime_adapter()
        .require_realtime_attachment_reattach(&session_id_value)
        .await
        .expect("reattach requirement should succeed");
    runtime
        .runtime_adapter()
        .begin_realtime_reconnect_cycle(
            &session_id_value,
            Some(1_700_000_000_000),
            Some(1_700_000_030_000),
        )
        .await
        .expect("reconnect cycle should begin");
    runtime
        .runtime_adapter()
        .schedule_realtime_reconnect_retry(&session_id_value, Some(1_700_000_001_000))
        .await
        .expect("second reconnect attempt should schedule");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id,
        RealtimeChannelRole::Observer,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Observer,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    match read_server_frame(&mut ws_stream).await {
        RealtimeServerFrame::ChannelOpened(opened) => {
            assert_eq!(opened.status.state, RealtimeChannelState::Reconnecting);
            assert_eq!(opened.status.attempt_count, 2);
            assert!(opened.status.next_retry_at.is_some());
            assert!(opened.status.deadline_at.is_some());
        }
        other => panic!("expected channel.opened, got {other:?}"),
    }

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    if let Ok(frame) = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        read_server_frame(&mut ws_stream),
    )
    .await
    {
        match frame {
            RealtimeServerFrame::ChannelStatus(status) => {
                assert_eq!(status.status.state, RealtimeChannelState::Reconnecting);
                assert_eq!(
                    status.status.attempt_count, 2,
                    "observer polling must not emit downgraded retry attempts"
                );
                assert!(
                    status.status.next_retry_at.is_some(),
                    "observer polling must preserve machine-owned next_retry_at"
                );
                assert!(
                    status.status.deadline_at.is_some(),
                    "observer polling must preserve machine-owned deadline_at"
                );
            }
            RealtimeServerFrame::ChannelEvent(_)
            | RealtimeServerFrame::ChannelError(_)
            | RealtimeServerFrame::ChannelClosed(_)
            | RealtimeServerFrame::ChannelOpened(_) => {}
        }
    }

    let machine_status =
        <meerkat_runtime::MeerkatMachine as SessionServiceRuntimeExt>::realtime_channel_status(
            runtime.runtime_adapter().as_ref(),
            &session_id_value,
        )
        .await
        .expect("registered session should expose machine-owned channel status");
    assert_eq!(
        machine_status.attempt_count, 2,
        "observer polling must not clear the canonical reconnect cycle"
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn second_channel_open_frame_yields_unexpected_channel_open() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_registered_realtime_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let _opened = read_server_frame(&mut ws_stream).await;
    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelOpen(
                RealtimeChannelOpenFrame {
                    protocol_version: open_info.default_protocol_version,
                    open_token: open_info.open_token.clone(),
                    role: RealtimeChannelRole::Primary,
                    turning_mode: RealtimeTurningMode::ProviderManaged,
                },
            ))
            .expect("channel.open should serialize")
            .into(),
        ))
        .await
        .expect("second channel.open should send");
    assert_error_frame(
        read_server_frame(&mut ws_stream).await,
        RealtimeErrorCode::UnexpectedChannelOpen,
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn channel_close_detaches_runtime_binding_and_yields_channel_closed() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = create_registered_realtime_session(&runtime).await;
    let session_id_text = session_id.to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        &runtime,
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let mut ws_stream = connect_and_open(
        &ws_url,
        &open_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let _opened = read_server_frame(&mut ws_stream).await;
    ws_stream
        .send(WsMessage::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelClose)
                .expect("channel.close should serialize")
                .into(),
        ))
        .await
        .expect("channel.close should send");
    let frame = read_server_frame(&mut ws_stream).await;
    match frame {
        RealtimeServerFrame::ChannelClosed(closed) => {
            assert_eq!(closed.reason.as_deref(), Some("client_closed"));
        }
        other => panic!("expected channel.closed, got {other:?}"),
    }

    let runtime_status =
        <meerkat_runtime::MeerkatMachine as SessionServiceRuntimeExt>::realtime_attachment_status(
            runtime.runtime_adapter().as_ref(),
            &session_id,
        )
        .await
        .expect("registered session should expose runtime live status");
    assert_eq!(
        runtime_status,
        meerkat_runtime::RealtimeAttachmentStatus::Unattached
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}
