//! Integration tests for the realtime websocket protocol shell.
//!
//! These tests stay at the websocket transport/product layer. They verify the
//! typed frame protocol plus the current channel-host mapping for session and
//! member targets without promoting the websocket shell into the semantic owner.

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
    RealtimeAudioChunk, RealtimeCapabilities, RealtimeChannelErrorFrame, RealtimeChannelEventFrame,
    RealtimeChannelInputFrame, RealtimeChannelOpenFrame, RealtimeChannelRole, RealtimeChannelState,
    RealtimeChannelStatus, RealtimeChannelTarget, RealtimeClientFrame, RealtimeErrorCode,
    RealtimeEvent, RealtimeInputChunk, RealtimeInputKind, RealtimeOpenInfo, RealtimeOpenRequest,
    RealtimeOutputKind, RealtimeReconnectPolicy, RealtimeServerFrame, RealtimeTextChunk,
    RealtimeTurningMode, WireContentInput, WireSessionMessage,
};
use meerkat_core::ToolResult;
use meerkat_core::interaction::ResponseStatus;
use meerkat_core::lifecycle::RunId;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::RunPrimitive;
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::session::ToolCategoryOverride;
use meerkat_core::{
    CommsCommand, CommsRuntime as CoreCommsRuntime, Config, InputStreamMode, MemoryConfigStore,
    Message, PeerName, SendReceipt, SessionHistoryQuery, StopReason,
};
use meerkat_mob::definition::{
    BackendConfig as MobBackendConfig, OrchestratorConfig, RoleWiringRule, WiringRules,
};
use meerkat_mob::{
    AgentIdentity as MobAgentIdentity, MobId, MobRuntimeMode, Profile, ProfileBinding, ProfileName,
    ToolConfig,
};
use meerkat_rpc::session_executor::SessionRuntimeExecutor;
use meerkat_rpc::session_runtime::SessionRuntime;
use meerkat_rpc::{REALTIME_WS_PATH, RealtimeWsHost, serve_realtime_ws_listener};
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

    async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

async fn register_live_session(runtime: &Arc<SessionRuntime>, session_id: &str) {
    register_live_session_with_executor(runtime, session_id, Box::new(NeverAppliedExecutor)).await;
}

async fn register_live_session_with_executor(
    runtime: &Arc<SessionRuntime>,
    session_id: &str,
    executor: Box<dyn CoreExecutor>,
) {
    let session_id = meerkat_core::SessionId::parse(session_id).expect("session_id should parse");
    runtime
        .runtime_adapter()
        .register_session_with_executor(session_id, executor)
        .await;
}

async fn issue_open_info(
    host: &RealtimeWsHost,
    session_id: &str,
    role: RealtimeChannelRole,
    turning_mode: RealtimeTurningMode,
) -> RealtimeOpenInfo {
    issue_open_info_with_policy(host, session_id, role, turning_mode, None).await
}

async fn issue_open_info_with_policy(
    host: &RealtimeWsHost,
    session_id: &str,
    role: RealtimeChannelRole,
    turning_mode: RealtimeTurningMode,
    reconnect_policy: Option<RealtimeReconnectPolicy>,
) -> RealtimeOpenInfo {
    host.issue_open_info(
        RealtimeOpenRequest {
            target: RealtimeChannelTarget::SessionTarget {
                session_id: session_id.to_string(),
            },
            role,
            turning_mode,
            reconnect_policy,
            channel_config: None,
        },
        conservative_capabilities(vec![
            RealtimeTurningMode::ProviderManaged,
            RealtimeTurningMode::ExplicitCommit,
        ]),
        None,
    )
    .await
}

async fn issue_member_open_info(
    host: &RealtimeWsHost,
    mob_id: &str,
    agent_identity: &str,
    role: RealtimeChannelRole,
    turning_mode: RealtimeTurningMode,
) -> RealtimeOpenInfo {
    host.issue_open_info(
        RealtimeOpenRequest {
            target: RealtimeChannelTarget::MobMemberTarget {
                mob_id: mob_id.to_string(),
                agent_identity: agent_identity.to_string(),
            },
            role,
            turning_mode,
            reconnect_policy: None,
            channel_config: None,
        },
        conservative_capabilities(vec![
            RealtimeTurningMode::ProviderManaged,
            RealtimeTurningMode::ExplicitCommit,
        ]),
        None,
    )
    .await
}

fn sample_mob_definition(mob_id: &str) -> meerkat_mob::MobDefinition {
    let mut definition = meerkat_mob::MobDefinition::explicit(mob_id);
    let mut profiles = std::collections::BTreeMap::new();
    profiles.insert(
        ProfileName::from("lead"),
        ProfileBinding::Inline(Profile {
            model: "claude-opus-4-6".into(),
            skills: vec![],
            tools: ToolConfig {
                builtins: true,
                shell: false,
                comms: true,
                memory: false,
                mob: true,
                mob_tasks: false,
                schedule: false,
                mcp: vec![],
                rust_bundles: vec![],
            },
            peer_description: "The lead".into(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::AutonomousHost,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        }),
    );
    profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Profile {
            model: "claude-sonnet-4-5".into(),
            skills: vec![],
            tools: ToolConfig {
                comms: true,
                ..ToolConfig::default()
            },
            peer_description: "A worker".into(),
            external_addressable: false,
            backend: None,
            runtime_mode: MobRuntimeMode::AutonomousHost,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        }),
    );
    definition.orchestrator = Some(OrchestratorConfig {
        profile: ProfileName::from("lead"),
    });
    definition.profiles = profiles;
    definition.wiring = WiringRules::default();
    definition.backend = MobBackendConfig::default();
    definition
}

fn sample_role_wired_mob_definition(mob_id: &str) -> meerkat_mob::MobDefinition {
    let mut definition = sample_mob_definition(mob_id);
    definition.wiring = WiringRules {
        auto_wire_orchestrator: false,
        role_wiring: vec![RoleWiringRule {
            a: ProfileName::from("lead"),
            b: ProfileName::from("worker"),
        }],
    };
    definition
}

async fn register_live_member(
    runtime: &Arc<SessionRuntime>,
    mob_id: &str,
    agent_identity: &str,
) -> Arc<meerkat_mob_mcp::MobMcpState> {
    let mob_state = Arc::new(
        meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
            runtime.session_service(),
            Some(Arc::clone(&runtime.runtime_adapter())),
        )
        .with_default_llm_client(Some(Arc::new(MockLlmClient))),
    );
    runtime.set_mob_state(Arc::clone(&mob_state));
    mob_state
        .mob_create_definition(sample_mob_definition(mob_id))
        .await
        .expect("mob should create");
    let _spawn = mob_state
        .mob_spawn(
            &MobId::from(mob_id),
            ProfileName::from("worker"),
            MobAgentIdentity::from(agent_identity),
            None,
            None,
        )
        .await
        .expect("member should spawn");
    mob_state
}

async fn register_role_wired_live_members(
    runtime: &Arc<SessionRuntime>,
    mob_id: &str,
    lead_identity: &str,
    worker_identity: &str,
) -> Arc<meerkat_mob_mcp::MobMcpState> {
    let mob_state = Arc::new(
        meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
            runtime.session_service(),
            Some(Arc::clone(&runtime.runtime_adapter())),
        )
        .with_default_llm_client(Some(Arc::new(MockLlmClient))),
    );
    runtime.set_mob_state(Arc::clone(&mob_state));
    mob_state
        .mob_create_definition(sample_role_wired_mob_definition(mob_id))
        .await
        .expect("wired mob should create");
    let _lead = mob_state
        .mob_spawn(
            &MobId::from(mob_id),
            ProfileName::from("lead"),
            MobAgentIdentity::from(lead_identity),
            None,
            None,
        )
        .await
        .expect("lead should spawn");
    let _worker = mob_state
        .mob_spawn(
            &MobId::from(mob_id),
            ProfileName::from("worker"),
            MobAgentIdentity::from(worker_identity),
            None,
            None,
        )
        .await
        .expect("worker should spawn");
    mob_state
}

async fn register_turn_driven_role_wired_live_members(
    runtime: &Arc<SessionRuntime>,
    mob_id: &str,
    operator_identity: &str,
    analyst_identity: &str,
) -> Arc<meerkat_mob_mcp::MobMcpState> {
    let mut definition = sample_role_wired_mob_definition(mob_id);
    let lead = definition
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .expect("lead profile should exist");
    let ProfileBinding::Inline(lead_profile) = lead else {
        panic!("lead profile should be inline");
    };
    lead_profile.runtime_mode = MobRuntimeMode::TurnDriven;

    let mob_state = Arc::new(
        meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
            runtime.session_service(),
            Some(Arc::clone(&runtime.runtime_adapter())),
        )
        .with_default_llm_client(Some(Arc::new(MockLlmClient))),
    );
    runtime.set_mob_state(Arc::clone(&mob_state));
    mob_state
        .mob_create_definition(definition)
        .await
        .expect("wired mob should create");
    let _lead = mob_state
        .mob_spawn(
            &MobId::from(mob_id),
            ProfileName::from("lead"),
            MobAgentIdentity::from(operator_identity),
            None,
            None,
        )
        .await
        .expect("operator should spawn");
    let _worker = mob_state
        .mob_spawn(
            &MobId::from(mob_id),
            ProfileName::from("worker"),
            MobAgentIdentity::from(analyst_identity),
            None,
            None,
        )
        .await
        .expect("analyst should spawn");
    mob_state
}

async fn resolve_member_bridge_session(
    mob_state: &Arc<meerkat_mob_mcp::MobMcpState>,
    mob_id: &str,
    agent_identity: &str,
) -> meerkat_core::SessionId {
    mob_state
        .handle_for(&MobId::from(mob_id))
        .await
        .expect("mob handle should resolve")
        .resolve_bridge_session_id(&MobAgentIdentity::from(agent_identity))
        .await
        .expect("member should have a bridge session")
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
                    protocol_version: info.default_protocol_version.clone(),
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
    let session_id = runtime
        .create_session(
            AgentBuildConfig {
                override_builtins: ToolCategoryOverride::Enable,
                ..AgentBuildConfig::new("claude-sonnet-4-5")
            },
            None,
            None,
        )
        .await
        .expect("session should create");
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
    let session_id = "01234567-89ab-cdef-0123-456789abcdef";
    register_live_session(&runtime, session_id).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        session_id,
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
            &meerkat_core::SessionId::parse(session_id).expect("session_id should parse"),
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
                WireSessionMessage::User {
                    content: WireContentInput::Text(text),
                } if text == "hello from provider managed"
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
    let session_id = "01234567-89ab-cdef-0123-456789abcdef";
    register_live_session(&runtime, session_id).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        session_id,
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
    assert_eq!(history_before_commit.messages, baseline_history.messages);

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
        history_still_uncommitted.messages,
        baseline_history.messages
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
                WireSessionMessage::User {
                    content: WireContentInput::Text(text),
                } if text == "hello world"
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = host
        .issue_open_info(
            RealtimeOpenRequest {
                target: RealtimeChannelTarget::SessionTarget {
                    session_id: "01234567-89ab-cdef-0123-456789abcdef".to_string(),
                },
                role: RealtimeChannelRole::Primary,
                turning_mode: RealtimeTurningMode::ExplicitCommit,
                reconnect_policy: None,
                channel_config: None,
            },
            conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
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
                    protocol_version: open_info.default_protocol_version.clone(),
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
        &session_id_text,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ExplicitCommit,
    )
    .await;
    let observer_info = issue_open_info(
        host.as_ref(),
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
    assert_eq!(history_after_disconnect.messages, baseline_history.messages);

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
                    Ok(Some(RealtimeSessionEvent::InputTranscriptFinal {
                        text: "cedar seven".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCommitted)),
                    Ok(Some(RealtimeSessionEvent::OutputTextDelta {
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
    assert_eq!(*open_calls.lock().await, 1);

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
#[ignore = "flaky: tracked in task #62 — tool-use subresponse boundary sometimes reconstructs provider session mid-turn under CI load"]
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
                    Ok(Some(RealtimeSessionEvent::InputTranscriptFinal {
                        text: "cedar seven".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCommitted)),
                    Ok(Some(RealtimeSessionEvent::OutputTextDelta {
                        delta: "asking ".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCompleted {
                        stop_reason: StopReason::ToolUse,
                        usage: meerkat_core::types::Usage::default(),
                    })),
                    Ok(Some(RealtimeSessionEvent::OutputTextDelta {
                        delta: "done".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCompleted {
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
                    Ok(Some(RealtimeSessionEvent::Interrupted)),
                    Ok(Some(RealtimeSessionEvent::TurnCompleted {
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
                    Ok(Some(RealtimeSessionEvent::Interrupted)),
                    Ok(Some(RealtimeSessionEvent::TurnCompleted {
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
    assert_eq!(history_after_cancel.messages, baseline_history.messages);

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
                    Ok(Some(RealtimeSessionEvent::InputTranscriptFinal {
                        text: "ask analyst for the token".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCommitted)),
                    Ok(Some(RealtimeSessionEvent::OutputTextDelta {
                        delta: "the token is ".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::Interrupted)),
                    Ok(Some(RealtimeSessionEvent::TurnStarted)),
                    Ok(Some(RealtimeSessionEvent::InputTranscriptFinal {
                        text: "stop and tell me the codeword and the token".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCommitted)),
                    Ok(Some(RealtimeSessionEvent::OutputTextDelta {
                        delta: "amber lantern. ".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::OutputTextDelta {
                        delta: "birch seventeen.".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCompleted {
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
            } => Some(text.clone()),
            WireSessionMessage::User {
                content: WireContentInput::Blocks(blocks),
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
    assert_eq!(*open_calls.lock().await, 2);

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
    assert_eq!(*open_calls.lock().await, 1);

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
async fn client_cannot_submit_tool_results_directly_over_realtime_protocol() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = "01234567-89ab-cdef-0123-456789abcdef";
    register_live_session(&runtime, session_id).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        session_id,
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

    #[async_trait::async_trait]
    impl CoreExecutor for BlockingExecutor {
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

        async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
            if matches!(command, RunControlCommand::CancelCurrentRun { .. }) {
                self.cancel_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    let (_temp, runtime, config_store) = build_test_runtime();
    let session_id = "01234567-89ab-cdef-0123-456789abcdef";
    let apply_started = Arc::new(Notify::new());
    let apply_finished = Arc::new(Notify::new());
    let allow_finish = Arc::new(Notify::new());
    let cancel_calls = Arc::new(AtomicUsize::new(0));
    runtime
        .runtime_adapter()
        .register_session_with_executor(
            meerkat_core::SessionId::parse(session_id).expect("session_id should parse"),
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
        session_id,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let server_host = Arc::clone(&host);
    let server_runtime = Arc::clone(&runtime);
    let server = tokio::spawn(async move {
        serve_realtime_ws_listener(listener, server_host, server_runtime, config_store).await
    });

    let session_id_value =
        meerkat_core::SessionId::parse(session_id).expect("session_id should parse");
    let (_outcome, completion_handle) = runtime
        .runtime_adapter()
        .accept_input_with_completion(
            &session_id_value,
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
    let session_id = "01234567-89ab-cdef-0123-456789abcdef";
    register_live_session(&runtime, session_id).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info_with_policy(
        host.as_ref(),
        session_id,
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

    let session_id_value =
        meerkat_core::SessionId::parse(session_id).expect("session_id should parse");
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
async fn second_channel_open_frame_yields_unexpected_channel_open() {
    let (_temp, runtime, config_store) = build_test_runtime();
    register_live_session(&runtime, "01234567-89ab-cdef-0123-456789abcdef").await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        "01234567-89ab-cdef-0123-456789abcdef",
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
                    protocol_version: open_info.default_protocol_version.clone(),
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
    let session_id = "01234567-89ab-cdef-0123-456789abcdef";
    register_live_session(&runtime, session_id).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_open_info(
        host.as_ref(),
        session_id,
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
            &meerkat_core::SessionId::parse(session_id).expect("session_id should parse"),
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

#[tokio::test]
async fn member_target_channel_open_sets_durable_intent_and_reports_runtime_backed_status() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let mob_id = "realtime-mob";
    let agent_identity = "w-live";
    let mob_state = register_live_member(&runtime, mob_id, agent_identity).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_member_open_info(
        host.as_ref(),
        mob_id,
        agent_identity,
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

    let member_status = mob_state
        .mob_member_status(
            &MobId::from(mob_id),
            &MobAgentIdentity::from(agent_identity),
        )
        .await
        .expect("member status should resolve after channel open");
    assert_eq!(
        serde_json::to_value(member_status.realtime_attachment_status)
            .expect("member live attachment status should serialize"),
        serde_json::Value::String("binding_not_ready".to_string())
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn member_target_provider_managed_text_input_commits_to_current_bridge_session() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let mob_id = "realtime-mob-input";
    let agent_identity = "w-input";
    let mob_state = register_live_member(&runtime, mob_id, agent_identity).await;
    let bridge_session_id = resolve_member_bridge_session(&mob_state, mob_id, agent_identity).await;
    let bridge_session_id_text = bridge_session_id.to_string();
    let baseline_history = read_history(&runtime, &bridge_session_id_text).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_member_open_info(
        host.as_ref(),
        mob_id,
        agent_identity,
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
                        text: "hello from member target".to_string(),
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
            text: "hello from member target".to_string(),
        },
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::InputTranscriptFinal {
            text: "hello from member target".to_string(),
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

    let history = read_history(&runtime, &bridge_session_id_text).await;
    assert!(
        history.message_count > baseline_history.message_count,
        "member-target realtime input should append committed transcript messages"
    );
    let new_messages = &history.messages[baseline_history.messages.len()..];
    assert!(
        new_messages.iter().any(|message| {
            matches!(
                message,
                WireSessionMessage::User {
                    content: WireContentInput::Text(text),
                } if text == "hello from member target"
            )
        }),
        "member-target provider-managed input should append the user transcript to canonical history"
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn member_target_provider_output_commits_canonical_assistant_preview() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let mob_id = "realtime-mob-provider-output";
    let agent_identity = "w-provider-output";
    let mob_state = register_live_member(&runtime, mob_id, agent_identity).await;
    let bridge_session_id = resolve_member_bridge_session(&mob_state, mob_id, agent_identity).await;
    let bridge_session_id_text = bridge_session_id.to_string();
    let baseline_history = read_history(&runtime, &bridge_session_id_text).await;
    let open_configs = Arc::new(tokio::sync::Mutex::new(Vec::new()));
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
                    Ok(Some(RealtimeSessionEvent::InputTranscriptFinal {
                        text: "hello from member provider".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCommitted)),
                    Ok(Some(RealtimeSessionEvent::OutputTextDelta {
                        delta: "member ready".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCompleted {
                        stop_reason: StopReason::EndTurn,
                        usage: meerkat_core::types::Usage::default(),
                    })),
                    Ok(None),
                ]),
                seen_inputs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_errors: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                release_events_after_input: false,
                release_events_after_tool_submission: false,
                input_seen: Arc::new(AtomicBool::new(false)),
                input_gate: Arc::new(Notify::new()),
                tool_submission_seen: Arc::new(AtomicBool::new(false)),
                tool_submission_gate: Arc::new(Notify::new()),
            }) as Box<dyn RealtimeSession>,
        )])),
        open_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
        open_configs: Arc::clone(&open_configs),
        attach_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()).with_session_factory(session_factory));
    let open_info = issue_member_open_info(
        host.as_ref(),
        mob_id,
        agent_identity,
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
    let captured = open_configs.lock().await.clone();
    assert_eq!(
        captured.len(),
        1,
        "member-target provider path should open one provider session"
    );
    let provider_open = &captured[0];
    assert_eq!(provider_open.llm_identity.model, "claude-sonnet-4-5");
    assert!(
        provider_open.visible_tools.is_empty(),
        "unwired member-target provider open config should reflect the canonical empty comms surface: {:?}",
        provider_open.visible_tools
    );
    assert!(
        provider_open
            .seed_messages
            .iter()
            .any(|message| matches!(message, Message::System(_))),
        "member-target provider session should be seeded from canonical session history"
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::TurnStarted,
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::InputTranscriptFinal {
            text: "hello from member provider".to_string(),
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
            delta: "member ready".to_string(),
        },
    );
    assert_channel_event(
        read_server_frame(&mut ws_stream).await,
        RealtimeEvent::TurnCompleted,
    );

    let history = read_history(&runtime, &bridge_session_id_text).await;
    let new_messages = &history.messages[baseline_history.messages.len()..];
    assert!(
        new_messages.iter().any(|message| {
            matches!(
                message,
                WireSessionMessage::BlockAssistant { blocks, .. }
                    if blocks.iter().any(|block| matches!(
                        block,
                        meerkat_contracts::WireAssistantBlock::Text { text, .. } if text == "member ready"
                    ))
            ) || matches!(
                message,
                WireSessionMessage::Assistant { content, .. } if content == "member ready"
            )
        }),
        "member-target realtime provider output should commit into canonical assistant history"
    );
    let member_status = mob_state
        .mob_member_status(
            &MobId::from(mob_id),
            &MobAgentIdentity::from(agent_identity),
        )
        .await
        .expect("member status should resolve after provider output completes");
    assert_eq!(
        member_status.output_preview.as_deref(),
        Some("member ready"),
        "member-target realtime output should project through canonical output_preview"
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn member_target_interrupted_provider_turn_followed_by_new_commit_appends_new_user_turn_and_final_output()
 {
    let (_temp, runtime, config_store) = build_test_runtime();
    let mob_id = "realtime-mob-interrupted-member";
    let agent_identity = "w-provider-interrupted";
    let mob_state = register_live_member(&runtime, mob_id, agent_identity).await;
    let bridge_session_id = resolve_member_bridge_session(&mob_state, mob_id, agent_identity).await;
    let bridge_session_id_text = bridge_session_id.to_string();
    let baseline_history = read_history(&runtime, &bridge_session_id_text).await;
    let session_factory = Arc::new(FakeRealtimeSessionFactory {
        capabilities: conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
        opened_sessions: tokio::sync::Mutex::new(std::collections::VecDeque::from(vec![Ok(
            Box::new(FakeRealtimeSession {
                capabilities: conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
                turning_mode: RealtimeTurningMode::ProviderManaged,
                scripted_events: std::collections::VecDeque::from(vec![
                    Ok(Some(RealtimeSessionEvent::TurnStarted)),
                    Ok(Some(RealtimeSessionEvent::InputTranscriptFinal {
                        text: "ask analyst for the token".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCommitted)),
                    Ok(Some(RealtimeSessionEvent::OutputTextDelta {
                        delta: "the token is ".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::Interrupted)),
                    Ok(Some(RealtimeSessionEvent::TurnStarted)),
                    Ok(Some(RealtimeSessionEvent::InputTranscriptFinal {
                        text: "stop and tell me the codeword and the token".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCommitted)),
                    Ok(Some(RealtimeSessionEvent::OutputTextDelta {
                        delta: "amber lantern. ".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::OutputTextDelta {
                        delta: "birch seventeen.".to_string(),
                    })),
                    Ok(Some(RealtimeSessionEvent::TurnCompleted {
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
    let open_info = issue_member_open_info(
        host.as_ref(),
        mob_id,
        agent_identity,
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

    let history = read_history(&runtime, &bridge_session_id_text).await;
    let new_messages = &history.messages[baseline_history.messages.len()..];
    let user_texts = new_messages
        .iter()
        .filter_map(|message| match message {
            WireSessionMessage::User {
                content: WireContentInput::Text(text),
            } => Some(text.clone()),
            WireSessionMessage::User {
                content: WireContentInput::Blocks(blocks),
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
async fn member_target_provider_open_config_includes_wired_peer_comms_tools() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let mob_id = "realtime-mob-provider-wired-tools";
    let lead_identity = "lead-provider-tools";
    let worker_identity = "worker-provider-tools";
    let _mob_state =
        register_role_wired_live_members(&runtime, mob_id, lead_identity, worker_identity).await;
    let open_configs = Arc::new(tokio::sync::Mutex::new(Vec::new()));
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
                scripted_events: std::collections::VecDeque::from(vec![Ok(None)]),
                seen_inputs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_errors: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                release_events_after_input: false,
                release_events_after_tool_submission: false,
                input_seen: Arc::new(AtomicBool::new(false)),
                input_gate: Arc::new(Notify::new()),
                tool_submission_seen: Arc::new(AtomicBool::new(false)),
                tool_submission_gate: Arc::new(Notify::new()),
            }) as Box<dyn RealtimeSession>,
        )])),
        open_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
        open_configs: Arc::clone(&open_configs),
        attach_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()).with_session_factory(session_factory));
    let open_info = issue_member_open_info(
        host.as_ref(),
        mob_id,
        lead_identity,
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
    let captured = open_configs.lock().await.clone();
    assert_eq!(
        captured.len(),
        1,
        "wired member-target should open one provider session"
    );
    let provider_open = &captured[0];
    let visible_tool_names = provider_open
        .visible_tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert!(
        visible_tool_names.contains(&"peers"),
        "wired member-target realtime open config should carry canonical peers visibility: {visible_tool_names:?}"
    );
    assert!(
        visible_tool_names.contains(&"send_request"),
        "wired member-target realtime open config should carry canonical comms request visibility: {visible_tool_names:?}"
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}

#[tokio::test]
async fn member_target_reopen_projection_includes_terminal_peer_response_after_async_comms() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let mob_id = "realtime-mob-provider-reopen-terminal-peer-response";
    let operator_identity = "operator-rt";
    let analyst_identity = "analyst-rt";
    let mob_state = register_turn_driven_role_wired_live_members(
        &runtime,
        mob_id,
        operator_identity,
        analyst_identity,
    )
    .await;
    let operator_bridge_session =
        resolve_member_bridge_session(&mob_state, mob_id, operator_identity).await;
    let analyst_bridge_session =
        resolve_member_bridge_session(&mob_state, mob_id, analyst_identity).await;
    let operator_peer_name = format!("{mob_id}/lead/{operator_identity}");
    let analyst_peer_name = format!("{mob_id}/worker/{analyst_identity}");

    let open_configs = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let session_factory = Arc::new(FakeRealtimeSessionFactory {
        capabilities: conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
        opened_sessions: tokio::sync::Mutex::new(std::collections::VecDeque::from(vec![
            Ok(Box::new(FakeRealtimeSession {
                capabilities: conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
                turning_mode: RealtimeTurningMode::ProviderManaged,
                scripted_events: std::collections::VecDeque::new(),
                seen_inputs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_errors: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                release_events_after_input: true,
                release_events_after_tool_submission: false,
                input_seen: Arc::new(AtomicBool::new(false)),
                input_gate: Arc::new(Notify::new()),
                tool_submission_seen: Arc::new(AtomicBool::new(false)),
                tool_submission_gate: Arc::new(Notify::new()),
            }) as Box<dyn RealtimeSession>),
            Ok(Box::new(FakeRealtimeSession {
                capabilities: conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
                turning_mode: RealtimeTurningMode::ProviderManaged,
                scripted_events: std::collections::VecDeque::new(),
                seen_inputs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_errors: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                release_events_after_input: true,
                release_events_after_tool_submission: false,
                input_seen: Arc::new(AtomicBool::new(false)),
                input_gate: Arc::new(Notify::new()),
                tool_submission_seen: Arc::new(AtomicBool::new(false)),
                tool_submission_gate: Arc::new(Notify::new()),
            }) as Box<dyn RealtimeSession>),
            Ok(Box::new(FakeRealtimeSession {
                capabilities: conservative_capabilities(vec![RealtimeTurningMode::ProviderManaged]),
                turning_mode: RealtimeTurningMode::ProviderManaged,
                scripted_events: std::collections::VecDeque::new(),
                seen_inputs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                seen_tool_errors: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                release_events_after_input: true,
                release_events_after_tool_submission: false,
                input_seen: Arc::new(AtomicBool::new(false)),
                input_gate: Arc::new(Notify::new()),
                tool_submission_seen: Arc::new(AtomicBool::new(false)),
                tool_submission_gate: Arc::new(Notify::new()),
            }) as Box<dyn RealtimeSession>),
        ])),
        open_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
        open_configs: Arc::clone(&open_configs),
        attach_calls: Arc::new(tokio::sync::Mutex::new(0usize)),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()).with_session_factory(session_factory));

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let operator_status = mob_state
                .mob_member_status(
                    &MobId::from(mob_id),
                    &MobAgentIdentity::from(operator_identity),
                )
                .await
                .expect("operator status should resolve");
            let analyst_status = mob_state
                .mob_member_status(
                    &MobId::from(mob_id),
                    &MobAgentIdentity::from(analyst_identity),
                )
                .await
                .expect("analyst status should resolve");
            let operator_reachable = operator_status
                .peer_connectivity
                .as_ref()
                .map(|snapshot| snapshot.reachable_peer_count)
                == Some(1);
            let analyst_reachable = analyst_status
                .peer_connectivity
                .as_ref()
                .map(|snapshot| snapshot.reachable_peer_count)
                == Some(1);
            if operator_reachable && analyst_reachable {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("wired member statuses should become reachable before realtime attach");

    let open_info = issue_member_open_info(
        host.as_ref(),
        mob_id,
        operator_identity,
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

    let initial_updated_at = runtime
        .read_session(&operator_bridge_session)
        .await
        .expect("operator bridge session should read")
        .state
        .updated_at;
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut last_updated_at = initial_updated_at;
        loop {
            match tokio::time::timeout(
                Duration::from_millis(300),
                runtime.wait_for_session_mutation_after(&operator_bridge_session, last_updated_at),
            )
            .await
            {
                Ok(Ok(updated_at)) => last_updated_at = updated_at,
                Ok(Err(error)) => panic!("operator bridge mutation watcher failed: {error}"),
                Err(_) => break,
            }
        }
    })
    .await
    .expect("operator bridge session should quiesce after bootstrap chatter");
    let baseline_updated_at = runtime
        .read_session(&operator_bridge_session)
        .await
        .expect("operator bridge session should read after bootstrap quiescence")
        .state
        .updated_at;
    let baseline_history = read_history(&runtime, &operator_bridge_session.to_string()).await;

    let session_service = runtime.session_service();
    let operator_comms = session_service
        .comms_runtime(&operator_bridge_session)
        .await
        .expect("operator bridge session should expose comms runtime");
    let analyst_comms = session_service
        .comms_runtime(&analyst_bridge_session)
        .await
        .expect("analyst bridge session should expose comms runtime");

    let request_receipt = CoreCommsRuntime::send(
        &*operator_comms,
        CommsCommand::PeerRequest {
            to: PeerName::new(analyst_peer_name.clone())
                .expect("analyst peer name should be valid"),
            intent: "checksum_token".to_string(),
            params: serde_json::json!({ "subject": "alpha beta gamma" }),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            stream: InputStreamMode::None,
        },
    )
    .await
    .expect("peer request should succeed");
    let request_interaction_id = match request_receipt {
        SendReceipt::PeerRequestSent { interaction_id, .. } => interaction_id,
        other => panic!("expected PeerRequestSent, got {other:?}"),
    };

    let response_receipt = CoreCommsRuntime::send(
        &*analyst_comms,
        CommsCommand::PeerResponse {
            to: PeerName::new(operator_peer_name.clone())
                .expect("operator peer name should be valid"),
            in_reply_to: request_interaction_id,
            status: ResponseStatus::Completed,
            result: serde_json::json!({
                "request_intent": "checksum_token",
                "token": "birch seventeen",
            }),
            handling_mode: None,
        },
    )
    .await
    .expect("peer response should succeed");
    assert!(
        matches!(response_receipt, SendReceipt::PeerResponseSent { .. }),
        "expected PeerResponseSent, got {response_receipt:?}"
    );

    tokio::time::timeout(
        Duration::from_secs(5),
        runtime.wait_for_session_mutation_after(&operator_bridge_session, baseline_updated_at),
    )
    .await
    .expect("operator bridge session should mutate after the terminal peer response")
    .expect("operator bridge mutation watcher should succeed");
    let post_response_history = read_history(&runtime, &operator_bridge_session.to_string()).await;

    let authoritative_open_config_result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let open_config = runtime
                .realtime_session_open_config(
                    &operator_bridge_session,
                    RealtimeTurningMode::ProviderManaged,
                )
                .await
                .expect("realtime_session_open_config");
            let projected = open_config
                .seed_messages
                .iter()
                .filter_map(|message| match message {
                    Message::System(system) => Some(system.content.to_lowercase()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if projected.iter().any(|text| {
                text.contains("peer_response_terminal")
                    && text.contains("birch seventeen")
                    && text.contains("checksum_token")
            }) {
                break open_config;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    let authoritative_open_config = match authoritative_open_config_result {
        Ok(open_config) => open_config,
        Err(_) => {
            let history = read_history(&runtime, &operator_bridge_session.to_string()).await;
            let persisted = runtime
                .load_persisted_session(&operator_bridge_session)
                .await
                .expect("load_persisted_session for diagnostics");
            let projected = runtime
                .realtime_session_open_config(
                    &operator_bridge_session,
                    RealtimeTurningMode::ProviderManaged,
                )
                .await
                .expect("realtime_session_open_config for diagnostics");
            panic!(
                "member bridge projection should include terminal peer response; \
                 baseline_history={baseline_history:?}; \
                 post_response_history={post_response_history:?}; \
                 history={history:?}; \
                 persisted_messages={:?}; \
                 persisted_system_context={:?}; \
                 projected_seed_messages={:?}",
                persisted
                    .as_ref()
                    .map(|session| session.messages().to_vec()),
                persisted
                    .as_ref()
                    .and_then(meerkat::Session::system_context_state),
                projected.seed_messages,
            );
        }
    };
    let authoritative_projection = authoritative_open_config
        .seed_messages
        .iter()
        .filter_map(|message| match message {
            Message::System(system) => Some(system.content.to_lowercase()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        authoritative_projection.iter().any(|text| {
            text.contains("peer_response_terminal")
                && text.contains("birch seventeen")
                && text.contains("checksum_token")
        }),
        "member bridge realtime open config should expose the authoritative terminal peer response before reopen: {authoritative_projection:?}"
    );

    ws_stream
        .close(None)
        .await
        .expect("channel should close cleanly");

    let reopen_info = issue_member_open_info(
        host.as_ref(),
        mob_id,
        operator_identity,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    let mut reopened = connect_and_open(
        &ws_url,
        &reopen_info,
        RealtimeChannelRole::Primary,
        RealtimeTurningMode::ProviderManaged,
    )
    .await;
    match read_server_frame(&mut reopened).await {
        RealtimeServerFrame::ChannelOpened(opened) => {
            assert_eq!(opened.status.state, RealtimeChannelState::Ready);
        }
        other => panic!("expected reopened channel.opened, got {other:?}"),
    }

    let captured = open_configs.lock().await.clone();
    assert!(
        captured.len() >= 2,
        "reopen flow should open a second provider session: {captured:?}"
    );
    let reopened_projection = captured
        .last()
        .expect("reopen should capture a provider open config")
        .seed_messages
        .iter()
        .filter_map(|message| match message {
            Message::System(system) => Some(system.content.to_lowercase()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        reopened_projection.iter().any(|text| {
            text.contains("peer_response_terminal")
                && text.contains("birch seventeen")
                && text.contains("checksum_token")
        }),
        "reopened member-target provider projection should retain the authoritative terminal peer response: {reopened_projection:?}"
    );

    let _ = reopened.close(None).await;
    server.abort();
}

#[tokio::test]
async fn member_target_channel_close_clears_durable_intent_and_detaches_runtime_binding() {
    let (_temp, runtime, config_store) = build_test_runtime();
    let mob_id = "realtime-mob-close";
    let agent_identity = "w-close";
    let mob_state = register_live_member(&runtime, mob_id, agent_identity).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let ws_url = format!("ws://{addr}{REALTIME_WS_PATH}");
    let host = Arc::new(RealtimeWsHost::new(ws_url.clone()));
    let open_info = issue_member_open_info(
        host.as_ref(),
        mob_id,
        agent_identity,
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
    let member_status_before_close = mob_state
        .mob_member_status(
            &MobId::from(mob_id),
            &MobAgentIdentity::from(agent_identity),
        )
        .await
        .expect("member status should resolve before channel close");
    assert_eq!(
        serde_json::to_value(member_status_before_close.realtime_attachment_status)
            .expect("member live attachment status should serialize"),
        serde_json::Value::String("binding_not_ready".to_string())
    );
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

    let member_status = mob_state
        .mob_member_status(
            &MobId::from(mob_id),
            &MobAgentIdentity::from(agent_identity),
        )
        .await
        .expect("member status should resolve after channel close");
    assert_eq!(
        serde_json::to_value(member_status.realtime_attachment_status)
            .expect("member live attachment status should serialize"),
        serde_json::Value::String("unattached".to_string())
    );

    let _ = ws_stream.close(None).await;
    server.abort();
}
