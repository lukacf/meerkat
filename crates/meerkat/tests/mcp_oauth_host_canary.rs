#![cfg(all(feature = "mcp", not(target_arch = "wasm32")))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::large_futures
)]

//! Host-driven MCP OAuth keeps human authentication outside model
//! observation (ADR-001 Toolkit r2, item 3).
//!
//! The host admits a real attempt (so the authorize URL and state exist),
//! an agent runs while the server is still unauthorized, the host completes
//! the login from its loopback callback, and a second agent run uses the
//! authorized MCP tool. Secret canaries (authorize URL, state, code, PKCE
//! challenge, access and refresh tokens, and the ignored DCR secret) must be
//! absent from agent events, the transcript (which carries tool results) and
//! every captured log line.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use meerkat::{
    AgentBuildConfig, AgentFactory, HostAuthService, HostMcpAuthPhase, LlmDoneOutcome, LlmEvent,
    LlmRequest, MCP_INTERACTIVE_LOGIN_TIMEOUT, MCP_OAUTH_CALLBACK_PATH, McpOAuthCallback,
    McpServerIdentity,
};
use meerkat_client::LlmClient;
use meerkat_core::mcp_config::McpServerConfig;
use meerkat_core::{AgentEvent, Config, Message};
use meerkat_providers::auth_store::{
    EphemeralTokenStore, InMemoryCoordinator, ProviderAuthPersistence,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;

const CODE_CANARY: &str = "code-canary-7f3a9c";
const ACCESS_CANARY: &str = "access-canary-51be04";
const REFRESH_CANARY: &str = "refresh-canary-c2d811";
const DCR_SECRET_CANARY: &str = "dcr-secret-canary-0e6f";
const SUBJECT: &str = "oidc-subject-7";
const ECHO_REPLY: &str = "echo-reply-visible-to-agent";

#[derive(Default)]
struct Fixture {
    redirect_uri: Mutex<Option<String>>,
}

fn host(headers: &HeaderMap) -> String {
    headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .unwrap()
        .to_owned()
}

async fn spawn_fixture() -> String {
    let state = Arc::new(Fixture::default());
    let app = Router::new()
        .route("/mcp", post(mcp))
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(|headers: HeaderMap| async move {
                let host = host(&headers);
                Json(json!({
                    "resource": format!("http://{host}/mcp"),
                    "authorization_servers": [format!("http://{host}")],
                }))
            }),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(|headers: HeaderMap| async move {
                Json(json!({
                    "issuer": format!("http://{}", host(&headers)),
                    "code_challenge_methods_supported": ["S256"],
                    "authorization_endpoint": "/authorize",
                    "token_endpoint": "/token",
                    "registration_endpoint": "/register",
                }))
            }),
        )
        .route(
            "/.well-known/openid-configuration",
            get(|headers: HeaderMap| async move {
                let host = host(&headers);
                Json(json!({
                    "issuer": format!("http://{host}"),
                    "userinfo_endpoint": format!("http://{host}/userinfo"),
                }))
            }),
        )
        .route("/register", post(register))
        .route("/authorize", get(authorize))
        .route("/token", post(token))
        .route(
            "/userinfo",
            get(|headers: HeaderMap| async move {
                if bearer(&headers) != Some(ACCESS_CANARY) {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                Json(json!({ "sub": SUBJECT })).into_response()
            }),
        )
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

async fn register(State(state): State<Arc<Fixture>>, Json(body): Json<Value>) -> Json<Value> {
    *state.redirect_uri.lock().unwrap() = body["redirect_uris"][0].as_str().map(ToOwned::to_owned);
    Json(json!({
        "client_id": "client-123",
        "client_secret": DCR_SECRET_CANARY,
        "token_endpoint_auth_method": "none",
    }))
}

async fn authorize(
    State(state): State<Arc<Fixture>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let redirect_uri = state.redirect_uri.lock().unwrap().clone().unwrap();
    Redirect::temporary(&format!(
        "{redirect_uri}?code={CODE_CANARY}&state={}",
        params["state"]
    ))
}

async fn token(Form(body): Form<HashMap<String, String>>) -> Json<Value> {
    assert_eq!(body.get("code").map(String::as_str), Some(CODE_CANARY));
    Json(json!({
        "access_token": ACCESS_CANARY,
        "refresh_token": REFRESH_CANARY,
        "expires_in": 3600,
        "scope": "openid",
    }))
}

async fn mcp(headers: HeaderMap, Json(request): Json<Value>) -> impl IntoResponse {
    if bearer(&headers) != Some(ACCESS_CANARY) {
        return (
            StatusCode::UNAUTHORIZED,
            [(
                "www-authenticate",
                r#"Bearer resource_metadata="/.well-known/oauth-protected-resource/mcp""#,
            )],
        )
            .into_response();
    }
    let Some(id) = request.get("id").cloned() else {
        return StatusCode::ACCEPTED.into_response();
    };
    let result = match request["method"].as_str() {
        Some("initialize") => json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "canary-mcp", "version": "0.1.0" },
        }),
        Some("tools/list") => json!({
            "tools": [{
                "name": "echo",
                "description": "Echo input",
                "inputSchema": { "type": "object", "properties": {} },
            }]
        }),
        Some("tools/call") => json!({
            "content": [{ "type": "text", "text": ECHO_REPLY }],
        }),
        other => {
            return Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("unsupported {other:?}") },
            }))
            .into_response();
        }
    };
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_oauth_secrets_never_reach_agent_observation_or_logs() {
    let logs = LogBuffer::default();
    let writer = logs.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .unwrap();

    let base = spawn_fixture().await;
    let temp = tempfile::tempdir().unwrap();
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let service = HostAuthService::new(
        ProviderAuthPersistence::new(
            Arc::new(EphemeralTokenStore::new()),
            Arc::new(InMemoryCoordinator::new()),
        ),
        runtime.provider_auth_runtime_authority(),
    );
    let mut server =
        McpServerConfig::streamable_http("canary", format!("{base}/mcp"), HashMap::new());
    if let meerkat_core::mcp_config::McpTransportConfig::Http(http) = &mut server.transport {
        http.oauth_account = Some(SUBJECT.to_owned());
    }
    let target = McpServerIdentity::from_config(&server).unwrap();

    // The host admits an attempt; its authorize URL and state now exist.
    let binding = meerkat_providers::auth_oauth::bind_loopback_callback(MCP_OAUTH_CALLBACK_PATH)
        .await
        .unwrap();
    let start = service
        .mcp_login_start(&target, &binding.redirect_url, None)
        .await
        .unwrap();
    let callback = binding.expect_state(start.state.clone());
    let challenge = start
        .authorize_url
        .split("code_challenge=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .unwrap()
        .to_owned();

    // Unauthorized run: typed host status, nothing secret for the agent.
    assert_eq!(
        service.mcp_status(&target).await.unwrap().phase,
        HostMcpAuthPhase::AuthorizationRequired
    );
    let (unauthorized_events, unauthorized_transcript) =
        run_agent(&factory, &service, &server).await;
    assert!(!unauthorized_transcript.contains(ECHO_REPLY));

    // The host's browser follows the authorize URL to its own loopback.
    let browser = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .unwrap();
    browser.get(&start.authorize_url).send().await.unwrap();
    let outcome = callback.wait(MCP_INTERACTIVE_LOGIN_TIMEOUT).await.unwrap();
    let completed = service
        .mcp_login_complete(
            &target,
            McpOAuthCallback {
                redirect_uri: start.redirect_uri.clone(),
                state: outcome.state,
                code: outcome.code,
                client_id: start.client_id.clone(),
                resource_metadata_url: Some(start.resource_metadata_url.clone()),
            },
        )
        .await
        .unwrap();
    assert_eq!(completed.account_id.as_deref(), Some(SUBJECT));
    assert_eq!(
        service.mcp_status(&target).await.unwrap().phase,
        HostMcpAuthPhase::Authorized
    );

    // Authorized run: the agent uses the tool (positive control).
    let (authorized_events, authorized_transcript) = run_agent(&factory, &service, &server).await;
    assert!(
        authorized_transcript.contains(ECHO_REPLY),
        "authorized MCP tool result must reach the agent"
    );

    let captured_logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(
        captured_logs.contains("awaiting human authorization"),
        "the unauthorized connection must have reported the typed status"
    );
    let canaries = [
        start.authorize_url.as_str(),
        start.state.as_str(),
        challenge.as_str(),
        CODE_CANARY,
        ACCESS_CANARY,
        REFRESH_CANARY,
        DCR_SECRET_CANARY,
    ];
    for (surface, observed) in [
        ("unauthorized agent events", &unauthorized_events),
        ("unauthorized transcript", &unauthorized_transcript),
        ("authorized agent events", &authorized_events),
        (
            "authorized transcript and tool results",
            &authorized_transcript,
        ),
        ("logs", &captured_logs),
    ] {
        for canary in canaries {
            assert!(
                !observed.contains(canary),
                "{surface} leaked an OAuth secret canary"
            );
        }
    }
}
