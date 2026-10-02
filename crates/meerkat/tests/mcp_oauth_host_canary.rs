#![cfg(all(
    feature = "mcp",
    feature = "test-mcp-oauth-fixtures",
    not(target_arch = "wasm32")
))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::large_futures
)]

//! Host-driven MCP OAuth keeps human authentication outside model
//! observation (ADR-001 Toolkit r2, item 3).
//!
//! Through the facade host API this exercises a successful login, a failed
//! completion, an explicit cancel, a dropped pending login and the advisory
//! launch path, plus agent runs before and after authorization. Every
//! attempt's authorize URL, state, PKCE challenge and PKCE verifier, and every
//! secret the fixture issues (code, access/refresh/ID token, DCR secret), must
//! be absent from agent events, transcripts (which carry tool results) and
//! every captured log record, `tracing` and `log` alike. Positive controls
//! prove each surface was actually captured.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

use meerkat::test_fixtures::mcp_oauth::{
    ECHO_REPLY, ISSUED_SECRET_CANARIES, McpOAuthFixture, SUBJECT, follow_authorize_url,
};
use meerkat::{
    AgentBuildConfig, AgentFactory, HostAuthService, HostMcpAuthPhase, LlmDoneOutcome, LlmEvent,
    LlmRequest, MCP_INTERACTIVE_LOGIN_TIMEOUT, McpOAuthBrowserLaunch, McpOAuthLoginStart,
    McpOAuthLoopbackBegin, McpServerIdentity,
};
use meerkat_client::LlmClient;
use meerkat_core::mcp_config::McpServerConfig;
use meerkat_core::{AgentEvent, Config, Message};
use meerkat_providers::auth_store::{
    EphemeralTokenStore, InMemoryCoordinator, ProviderAuthPersistence,
};
use serde_json::json;

/// Calls the MCP echo tool once when offered, then answers in text.
struct EchoCallingClient;

#[async_trait::async_trait]
impl LlmClient for EchoCallingClient {
    fn project_replay_messages(
        &self,
        messages: &[Message],
    ) -> Result<Vec<Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_llm_core::LlmStream<'a> {
        let answered = request
            .messages
            .iter()
            .any(|message| matches!(message, Message::ToolResults { .. }));
        let echo = request
            .tools
            .iter()
            .find(|tool| tool.name.contains("echo"))
            .map(|tool| tool.name.to_string());
        let mut events = Vec::new();
        let stop_reason = match echo {
            Some(name) if !answered => {
                events.push(LlmEvent::ToolCallComplete {
                    id: "call-echo".into(),
                    name,
                    args: json!({}),
                    meta: None,
                });
                meerkat_core::StopReason::ToolUse
            }
            _ => {
                events.push(LlmEvent::TextDelta {
                    delta: "done".into(),
                    meta: None,
                });
                meerkat_core::StopReason::EndTurn
            }
        };
        events.push(LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::Other,
                &request.model,
                meerkat_core::Usage::default(),
            ),
        });
        events.push(LlmEvent::Done {
            outcome: LlmDoneOutcome::Success { stop_reason },
        });
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Capture every `tracing` event and, through the log bridge, every `log`
/// record at TRACE.
fn capture_all_logs() -> LogBuffer {
    use tracing_subscriber::util::SubscriberInitExt;
    let logs = LogBuffer::default();
    let writer = logs.clone();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish()
        .try_init()
        .expect("canary owns the process-wide log capture");
    logs
}

fn host_service(runtime: &meerkat_runtime::MeerkatMachine) -> HostAuthService {
    HostAuthService::new(
        ProviderAuthPersistence::new(
            Arc::new(EphemeralTokenStore::new()),
            Arc::new(InMemoryCoordinator::new()),
        ),
        runtime.provider_auth_runtime_authority(),
    )
}

fn selected_server(url: String) -> McpServerConfig {
    let mut server = McpServerConfig::streamable_http("canary", url, HashMap::new());
    if let meerkat_core::mcp_config::McpTransportConfig::Http(http) = &mut server.transport {
        http.oauth_account = Some(SUBJECT.to_owned());
    }
    server
}

/// Run one agent turn; returns the serialized events and transcript.
async fn run_agent(
    factory: &AgentFactory,
    service: &HostAuthService,
    server: &McpServerConfig,
) -> (String, String) {
    let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
    build.llm_client_override = Some(Arc::new(EchoCallingClient));
    build.mcp_servers = vec![server.clone()];
    build.wait_for_mcp = true;
    build.mcp_auth_resolver = Some(Arc::new(service.mcp_oauth_authority().unwrap()));
    let mut agent = factory
        .build_agent(build, &Config::default())
        .await
        .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(1024);
    agent
        .run_with_events("use the echo tool".to_string().into(), tx)
        .await
        .unwrap();
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(serde_json::to_string(&event).unwrap());
    }
    let transcript = serde_json::to_string(agent.session().messages()).unwrap();
    (events.join("\n"), transcript)
}

/// Every secret of one admitted attempt: authorize URL, state, PKCE
/// challenge and the PKCE verifier held by the flow owner.
fn attempt_canaries(
    runtime: &meerkat_runtime::MeerkatMachine,
    target: &McpServerIdentity,
    start: &McpOAuthLoginStart,
) -> Vec<String> {
    let challenge = start
        .authorize_url
        .split("code_challenge=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .unwrap()
        .to_owned();
    let identity: meerkat_core::AuthCredentialIdentity = target.auth_binding_ref().unwrap().into();
    let verifier = runtime
        .provider_auth_runtime_authority()
        .oauth_flow_authority()
        .admitted_connector_browser_attempt(&start.state, &identity)
        .unwrap()
        .expect("the attempt is admitted")
        .pkce_verifier;
    vec![
        start.authorize_url.clone(),
        start.state.clone(),
        challenge,
        verifier,
    ]
}

async fn begin(
    service: &HostAuthService,
    target: &McpServerIdentity,
) -> meerkat::McpOAuthPendingLogin {
    match service
        .mcp_begin_loopback_login(target, None)
        .await
        .unwrap()
    {
        McpOAuthLoopbackBegin::Started(pending) => pending,
        McpOAuthLoopbackBegin::Joined(_) => panic!("no attempt should be pending"),
    }
}

/// The host's browser: follows the authorize URL to its own loopback.
fn fixture_browser(url: String) -> std::io::Result<()> {
    tokio::runtime::Handle::current().block_on(follow_authorize_url(&url))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_oauth_secrets_never_reach_agent_observation_or_logs() {
    let logs = capture_all_logs();
    let fixture = McpOAuthFixture::spawn().await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let service = host_service(&runtime);
    let server = selected_server(fixture.mcp_url());
    let target = McpServerIdentity::from_config(&server).unwrap();
    let mut canaries: Vec<String> = ISSUED_SECRET_CANARIES
        .iter()
        .map(|canary| (*canary).to_owned())
        .collect();

    // Cancel path, with the advisory launch exercised.
    let cancelled = begin(&service, &target).await;
    canaries.extend(attempt_canaries(&runtime, &target, cancelled.start()));
    let launched_urls = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&launched_urls);
    assert_eq!(
        cancelled
            .launch_browser(move |url| {
                recorder.lock().unwrap().push(url);
                Ok(())
            })
            .await,
        McpOAuthBrowserLaunch::Launched
    );
    assert_eq!(
        launched_urls.lock().unwrap().as_slice(),
        [cancelled.start().authorize_url.clone()],
        "positive control: the launch path received the authorize URL"
    );
    cancelled.cancel().await.unwrap();

    // Drop path.
    let dropped = begin(&service, &target).await;
    canaries.extend(attempt_canaries(&runtime, &target, dropped.start()));
    drop(dropped);

    // Error path: the provider refuses the exchange after the user approved.
    let failing = begin(&service, &target).await;
    canaries.extend(attempt_canaries(&runtime, &target, failing.start()));
    fixture.fail_token_exchange(true);
    assert_eq!(
        failing.launch_browser(fixture_browser).await,
        McpOAuthBrowserLaunch::Launched
    );
    assert!(
        failing
            .complete(MCP_INTERACTIVE_LOGIN_TIMEOUT)
            .await
            .is_err()
    );
    fixture.fail_token_exchange(false);

    // Unauthorized agent run: typed host status, nothing secret for the agent.
    assert_eq!(
        service.mcp_status(&target).await.unwrap().phase,
        HostMcpAuthPhase::AuthorizationRequired
    );
    let (unauthorized_events, unauthorized_transcript) =
        run_agent(&factory, &service, &server).await;
    assert!(!unauthorized_transcript.contains(ECHO_REPLY));

    // Success path.
    let succeeding = begin(&service, &target).await;
    canaries.extend(attempt_canaries(&runtime, &target, succeeding.start()));
    assert_eq!(
        succeeding.launch_browser(fixture_browser).await,
        McpOAuthBrowserLaunch::Launched
    );
    let completed = succeeding
        .complete(MCP_INTERACTIVE_LOGIN_TIMEOUT)
        .await
        .expect("login completes");
    assert_eq!(completed.account_id.as_deref(), Some(SUBJECT));
    assert_eq!(
        service.mcp_status(&target).await.unwrap().phase,
        HostMcpAuthPhase::Authorized
    );

    // Authorized agent run: the agent uses the tool (positive control).
    let (authorized_events, authorized_transcript) = run_agent(&factory, &service, &server).await;
    assert!(
        authorized_transcript.contains(ECHO_REPLY),
        "positive control: the authorized MCP tool result reaches the agent"
    );

    let captured = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(
        captured.contains("awaiting human authorization"),
        "positive control: tracing events are captured"
    );
    assert!(
        captured.contains("reqwest"),
        "positive control: log-crate records are captured through the bridge"
    );
    for (surface, observed) in [
        ("unauthorized agent events", &unauthorized_events),
        ("unauthorized transcript", &unauthorized_transcript),
        ("authorized agent events", &authorized_events),
        (
            "authorized transcript and tool results",
            &authorized_transcript,
        ),
        ("logs", &captured),
    ] {
        for canary in &canaries {
            assert!(
                !observed.contains(canary.as_str()),
                "{surface} leaked an OAuth secret canary"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_http_mcp_server_without_account_connects_under_the_native_resolver() {
    let fixture = McpOAuthFixture::spawn().await.unwrap();
    let temp = tempfile::tempdir().unwrap();
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let service = host_service(&runtime);
    // No `oauth_account`: an unselected server must keep connecting without
    // credentials, exactly as before a resolver was installed.
    let server =
        McpServerConfig::streamable_http("public", fixture.public_mcp_url(), HashMap::new());
    let (_events, transcript) = run_agent(&factory, &service, &server).await;
    assert!(
        transcript.contains(ECHO_REPLY),
        "a public MCP server must stay usable under the default resolver"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unselected_server_demanding_oauth_is_refused_with_typed_account_selection() {
    let fixture = McpOAuthFixture::spawn().await.unwrap();
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let service = host_service(&runtime);
    let server = McpServerConfig::streamable_http("guarded", fixture.mcp_url(), HashMap::new());
    let resolver: Arc<dyn meerkat::McpAuthResolver> =
        Arc::new(service.mcp_oauth_authority().unwrap());
    let error = match meerkat::McpConnection::connect_with_mcp_auth(
        &server,
        meerkat::McpAuthMode::Interactive,
        Some(resolver),
    )
    .await
    {
        Ok(_) => panic!("an OAuth-protected server must not connect without credentials"),
        Err(error) => error,
    };
    assert!(
        matches!(
            error,
            meerkat::McpError::OAuthAccountRejected(
                meerkat::McpOAuthError::AccountSelectionRequired
            )
        ),
        "got {error:?}"
    );
}
