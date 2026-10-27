//! Host-owned HTTP egress controls: a private initialize, tools/list and
//! close under a required egress profile, and each refusal the profile owns
//! (host, port, scheme, redirect escape, resolved or literal loopback without
//! deliberate authority, an SSE-selected endpoint, the protected-call client,
//! an unbound credential owner, the ambient proxy), and the per-call outcome
//! of a refused credential operation. Every refusal is local and typed, and
//! the alternate destination records zero arrivals.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::disallowed_methods
)]

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use meerkat_auth_core::McpAuthMode;
use meerkat_core::McpServerConfig;
use rmcp::model::{CallToolRequest, CallToolRequestParams, ClientJsonRpcMessage, NumberOrString};
use rmcp::transport::streamable_http_client::StreamableHttpClient as _;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::transport::EgressGuard;
use crate::transport::protected::ProtectedMetadata;
use crate::transport::sse::{ReqwestSseClient, SseClient as _};
use crate::transport::streamable_http::{
    AuthChallengeRecorder, RequestDispatch, RequestDisposition, ReqwestStreamableHttpClient,
};
use crate::{
    HttpEgressError, HttpEgressOrigin, HttpEgressPolicy, HttpEgressProfile, HttpEgressRefusal,
    HttpEgressUnavailable, McpAuthResolver, McpConnection, McpError, McpStdioLaunchProfile,
};

const BOUND: Duration = Duration::from_secs(20);

fn origin(text: &str) -> HttpEgressOrigin {
    HttpEgressOrigin::parse(text).unwrap()
}

fn loopback() -> IpAddr {
    "127.0.0.1".parse().unwrap()
}

/// A profile that deliberately admits exactly one local service.
fn local_profile(port: u16) -> HttpEgressProfile {
    HttpEgressProfile::required(
        HttpEgressPolicy::new()
            .allow_local(origin(&format!("http://127.0.0.1:{port}")), [loopback()]),
    )
}

fn http_config(name: &str, url: String) -> McpServerConfig {
    let mut config = McpServerConfig::streamable_http(name, url, HashMap::new());
    config.connect_timeout_secs = Some(5);
    config
}

#[derive(Default)]
struct McpFixture {
    arrivals: AtomicUsize,
    tool_calls: AtomicUsize,
    redirect_initialize_to: Option<String>,
    redirect_calls_to: Option<String>,
}

/// A minimal streamable HTTP MCP server that counts every request it
/// receives.
async fn spawn_mcp_fixture(redirect_initialize_to: Option<String>) -> (u16, Arc<McpFixture>) {
    serve_mcp_fixture(McpFixture {
        redirect_initialize_to,
        ..McpFixture::default()
    })
    .await
}

async fn serve_mcp_fixture(fixture: McpFixture) -> (u16, Arc<McpFixture>) {
    let fixture = Arc::new(fixture);
    let app = Router::new()
        .route("/mcp", post(mcp_post))
        .with_state(Arc::clone(&fixture));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (port, fixture)
}

async fn mcp_post(State(fixture): State<Arc<McpFixture>>, Json(request): Json<Value>) -> Response {
    fixture.arrivals.fetch_add(1, Ordering::SeqCst);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if method.as_deref() == Some("tools/call") {
        fixture.tool_calls.fetch_add(1, Ordering::SeqCst);
    }
    let redirect = match method.as_deref() {
        Some("initialize") => fixture.redirect_initialize_to.as_ref(),
        Some("tools/call") => fixture.redirect_calls_to.as_ref(),
        _ => None,
    };
    if let Some(location) = redirect {
        return (
            StatusCode::TEMPORARY_REDIRECT,
            [(header::LOCATION, location.clone())],
        )
            .into_response();
    }
    let Some(id) = request.get("id").cloned() else {
        return StatusCode::ACCEPTED.into_response();
    };
    let result = match method.as_deref() {
        Some("initialize") => json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "egress-fixture", "version": "0.1.0" }
        }),
        Some("tools/list") => json!({
            "tools": [{
                "name": "echo",
                "description": "Echo input",
                "inputSchema": { "type": "object", "properties": { "message": { "type": "string" } } }
            }]
        }),
        Some("tools/call") => json!({ "content": [{ "type": "text", "text": "ok" }] }),
        _ => {
            return Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": "unsupported" }
            }))
            .into_response();
        }
    };
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

/// A raw TCP destination that counts every accepted connection and answers
/// whatever arrives with a plain HTTP error.
async fn counting_listener() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let arrivals = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&arrivals);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buffer = [0_u8; 8192];
                let _ = stream.read(&mut buffer).await;
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
            });
        }
    });
    (port, arrivals)
}

/// A legacy SSE server whose `endpoint` event names `endpoint`; it counts
/// accepted connections and holds each stream open.
async fn sse_fixture(endpoint: String) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let arrivals = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&arrivals);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
            let endpoint = endpoint.clone();
            tokio::spawn(async move {
                let mut buffer = [0_u8; 8192];
                let _ = stream.read(&mut buffer).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\nevent: endpoint\ndata: {endpoint}\n\n"
                );
                let _ = stream.write_all(head.as_bytes()).await;
                tokio::time::sleep(Duration::from_mins(1)).await;
            });
        }
    });
    (port, arrivals)
}

async fn connect_under(
    config: &McpServerConfig,
    profile: &HttpEgressProfile,
    resolver: Option<Arc<dyn McpAuthResolver>>,
) -> Result<(McpConnection, Vec<Arc<meerkat_core::ToolDef>>), McpError> {
    tokio::time::timeout(
        BOUND,
        McpConnection::connect_and_enumerate_with_profiles(
            config,
            McpAuthMode::Stored,
            resolver,
            None,
            &McpStdioLaunchProfile::trusted_host(),
            profile,
        ),
    )
    .await
    .expect("the attempt is bounded")
}

fn assert_refused(
    result: Result<(McpConnection, Vec<Arc<meerkat_core::ToolDef>>), McpError>,
    expected: HttpEgressRefusal,
) {
    match result {
        Err(McpError::EgressRefused { refusal, .. }) => assert_eq!(refusal, expected),
        Err(other) => panic!("expected egress refusal {expected:?}, got {other:?}"),
        Ok(_) => panic!("expected egress refusal {expected:?}, but the connection succeeded"),
    }
}

/// First acceptance: an actual private initialize, tools/list and close under
/// a required profile that deliberately admits exactly this local service.
#[tokio::test]
async fn a_private_inspection_runs_under_an_exactly_admitting_profile() {
    let (port, fixture) = spawn_mcp_fixture(None).await;
    let profile = local_profile(port);
    let config = http_config("egress-allowed", format!("http://127.0.0.1:{port}/mcp"));
    let (connection, tools) = connect_under(&config, &profile, None).await.unwrap();
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["echo"]
    );
    assert_eq!(
        connection.server_info().unwrap().server_info.name,
        "egress-fixture"
    );
    let blocks = connection
        .call_tool("echo", &json!({ "message": "hi" }))
        .await
        .expect("an admitted tools/call is sent");
    assert_eq!(meerkat_core::types::text_content(&blocks), "ok");
    connection.close().await.unwrap();
    assert!(fixture.arrivals.load(Ordering::SeqCst) >= 3);
}

#[tokio::test]
async fn a_host_outside_the_profile_is_refused_before_any_arrival() {
    let (port, fixture) = spawn_mcp_fixture(None).await;
    // `localhost` reaches the same listener, but the profile names only the
    // literal address.
    let config = http_config("egress-host", format!("http://localhost:{port}/mcp"));
    assert_refused(
        connect_under(&config, &local_profile(port), None).await,
        HttpEgressRefusal::HostNotAdmitted,
    );
    assert_eq!(fixture.arrivals.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_port_outside_the_profile_is_refused_with_zero_alternate_arrivals() {
    let (port, _fixture) = spawn_mcp_fixture(None).await;
    let (alternate, arrivals) = counting_listener().await;
    let config = http_config("egress-port", format!("http://127.0.0.1:{alternate}/mcp"));
    assert_refused(
        connect_under(&config, &local_profile(port), None).await,
        HttpEgressRefusal::PortNotAdmitted,
    );
    assert_eq!(arrivals.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_scheme_outside_the_profile_is_refused_with_zero_alternate_arrivals() {
    let (alternate, arrivals) = counting_listener().await;
    let config = http_config(
        "egress-scheme",
        format!("https://127.0.0.1:{alternate}/mcp"),
    );
    assert_refused(
        connect_under(&config, &local_profile(alternate), None).await,
        HttpEgressRefusal::SchemeNotAdmitted,
    );
    assert_eq!(arrivals.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_redirect_escape_is_stopped_with_zero_alternate_arrivals() {
    let (alternate, arrivals) = counting_listener().await;
    let (port, fixture) =
        spawn_mcp_fixture(Some(format!("http://127.0.0.1:{alternate}/mcp"))).await;
    let config = http_config("egress-redirect", format!("http://127.0.0.1:{port}/mcp"));
    let result = connect_under(&config, &local_profile(port), None).await;
    // The first hop was admitted and sent; its redirect answer is refused by
    // status, not as an egress refusal.
    match result {
        Err(McpError::ConnectionFailed { .. }) => {}
        Err(other) => panic!("expected a refused redirect answer, got {other:?}"),
        Ok(_) => panic!("a redirect outside the origin must not serve the connection"),
    }
    assert_eq!(
        fixture.arrivals.load(Ordering::SeqCst),
        1,
        "only the first hop was sent"
    );
    assert_eq!(arrivals.load(Ordering::SeqCst), 0);
}

/// Streamable HTTP: a tools/call answered with a redirect was sent to the
/// admitted server. The redirect is not followed: the call records
/// `SentRedirected` and fails `RedirectedOutcomeUncertain` (never an egress
/// refusal), and the alternate destination records zero arrivals. A received
/// 3xx does not prove the server performed no effect. (Legacy SSE refuses a
/// 3xx answer with a generic `ToolCallFailed`, which claims nothing either
/// way.)
#[tokio::test]
async fn a_redirected_tool_call_stays_sent_and_uncertain_with_zero_alternate_arrivals() {
    let (alternate, arrivals) = counting_listener().await;
    let (port, fixture) = serve_mcp_fixture(McpFixture {
        redirect_calls_to: Some(format!("http://127.0.0.1:{alternate}/mcp")),
        ..McpFixture::default()
    })
    .await;
    let config = http_config(
        "egress-call-redirect",
        format!("http://127.0.0.1:{port}/mcp"),
    );
    let (connection, _) = connect_under(&config, &local_profile(port), None)
        .await
        .unwrap();
    let result = connection
        .call_tool("echo", &json!({ "message": "hi" }))
        .await;
    assert!(
        matches!(result, Err(McpError::RedirectedOutcomeUncertain { .. })),
        "{result:?}"
    );
    assert_eq!(fixture.tool_calls.load(Ordering::SeqCst), 1);
    assert_eq!(arrivals.load(Ordering::SeqCst), 0);
    connection.close().await.unwrap();
}

/// The connector's own resolver refuses a host name that resolves to
/// loopback without deliberate local authority.
#[tokio::test]
async fn resolved_loopback_without_deliberate_authority_is_refused_by_the_resolver() {
    let (port, fixture) = spawn_mcp_fixture(None).await;
    let profile = HttpEgressProfile::required(
        HttpEgressPolicy::new().allow_public(origin(&format!("http://localhost:{port}"))),
    );
    let config = http_config("egress-resolved", format!("http://localhost:{port}/mcp"));
    assert_refused(
        connect_under(&config, &profile, None).await,
        HttpEgressRefusal::AddressNotAdmitted,
    );
    assert_eq!(fixture.arrivals.load(Ordering::SeqCst), 0);

    // The same host name with deliberate authority for its exact addresses.
    let deliberate = HttpEgressProfile::required(HttpEgressPolicy::new().allow_local(
        origin(&format!("http://localhost:{port}")),
        [loopback(), "::1".parse().unwrap()],
    ));
    let (connection, tools) = connect_under(&config, &deliberate, None).await.unwrap();
    assert_eq!(tools.len(), 1);
    connection.close().await.unwrap();
}

#[tokio::test]
async fn a_literal_loopback_without_deliberate_authority_is_refused_at_admission() {
    let (port, fixture) = spawn_mcp_fixture(None).await;
    let profile = HttpEgressProfile::required(
        HttpEgressPolicy::new().allow_public(origin(&format!("http://127.0.0.1:{port}"))),
    );
    let config = http_config("egress-literal", format!("http://127.0.0.1:{port}/mcp"));
    assert_refused(
        connect_under(&config, &profile, None).await,
        HttpEgressRefusal::AddressNotAdmitted,
    );
    assert_eq!(fixture.arrivals.load(Ordering::SeqCst), 0);
}

/// The server-selected SSE message endpoint is admitted like any other URI.
/// The transport's own same-origin endpoint check may refuse it first, as a
/// local connection failure naming the configured origin; either way nothing
/// reaches the alternate destination, and no other failure is accepted.
#[tokio::test]
async fn an_sse_selected_endpoint_outside_the_profile_never_receives_a_post() {
    let (alternate, arrivals) = counting_listener().await;
    let (port, stream_arrivals) =
        sse_fixture(format!("http://127.0.0.1:{alternate}/messages")).await;
    let mut config = McpServerConfig::sse(
        "egress-sse",
        format!("http://127.0.0.1:{port}/sse"),
        HashMap::new(),
    );
    config.connect_timeout_secs = Some(5);
    let result = connect_under(&config, &local_profile(port), None).await;
    match result {
        Err(McpError::EgressRefused { refusal, .. }) => {
            assert_eq!(refusal, HttpEgressRefusal::PortNotAdmitted);
        }
        Err(McpError::ConnectionFailed { reason })
            if reason.contains("not on the configured origin") => {}
        Err(other) => panic!("expected the endpoint to be refused locally, got {other:?}"),
        Ok(_) => panic!("an endpoint outside the profile must not serve the connection"),
    }
    assert_eq!(stream_arrivals.load(Ordering::SeqCst), 1);
    assert_eq!(arrivals.load(Ordering::SeqCst), 0);
}

/// Client-level control: the SSE client admits the selected endpoint and
/// records a tools/call as affirmatively unsent.
#[tokio::test]
async fn the_sse_client_refuses_an_unadmitted_endpoint_typed_and_unsent() {
    let (alternate, arrivals) = counting_listener().await;
    let guard = EgressGuard::new(local_profile(alternate.wrapping_add(1)));
    let client = ReqwestSseClient::new(Default::default()).with_egress(guard.clone());
    let dispatch = RequestDispatch::default();
    let mut call = CallToolRequest::new(CallToolRequestParams::new("echo"));
    call.extensions.insert(dispatch.clone());
    let message = ClientJsonRpcMessage::request(call.into(), NumberOrString::Number(7));
    let uri: http::Uri = format!("http://127.0.0.1:{alternate}/messages")
        .parse()
        .unwrap();
    assert!(client.post_message(uri, message, None).await.is_err());
    assert_eq!(
        dispatch.disposition(),
        Some(RequestDisposition::RefusedEgress(HttpEgressError::Refused(
            HttpEgressRefusal::PortNotAdmitted
        )))
    );
    assert_eq!(
        guard.recorded(),
        Some(HttpEgressError::Refused(HttpEgressRefusal::PortNotAdmitted))
    );
    assert_eq!(arrivals.load(Ordering::SeqCst), 0);
}

/// The protected-call client is the profile's own: its resolver refuses a
/// destination outside the address set, so the protected body is never sent.
#[tokio::test]
async fn the_protected_call_client_enforces_the_profile_resolver() {
    let (port, fixture) = spawn_mcp_fixture(None).await;
    let profile = HttpEgressProfile::required(
        HttpEgressPolicy::new().allow_public(origin(&format!("http://localhost:{port}"))),
    );
    let guard = EgressGuard::new(profile);
    let client = ReqwestStreamableHttpClient::new_with_auth_challenge(
        Default::default(),
        AuthChallengeRecorder::default(),
    )
    .with_egress(guard.clone());
    let dispatch = RequestDispatch::default();
    let mut call = CallToolRequest::new(CallToolRequestParams::new("echo"));
    call.extensions.insert(dispatch.clone());
    call.extensions
        .insert(ProtectedMetadata(serde_json::Map::from_iter([(
            "connector-credential".to_owned(),
            json!("harmless-sentinel"),
        )])));
    let message = ClientJsonRpcMessage::request(call.into(), NumberOrString::Number(9));
    let result = client
        .post_message(
            Arc::from(format!("http://localhost:{port}/mcp")),
            message,
            None,
            None,
            HashMap::new(),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(
        dispatch.disposition(),
        Some(RequestDisposition::RefusedEgress(HttpEgressError::Refused(
            HttpEgressRefusal::AddressNotAdmitted
        )))
    );
    assert_eq!(fixture.arrivals.load(Ordering::SeqCst), 0);
}

/// The credential owner's answer to its next credential read.
#[derive(Clone, Copy, Debug)]
enum BearerOutcome {
    Token,
    Refused(HttpEgressRefusal),
    Unavailable(HttpEgressUnavailable),
}

struct BoundResolver {
    egress: Option<HttpEgressProfile>,
    reads: AtomicUsize,
    outcome: std::sync::Mutex<BearerOutcome>,
}

impl BoundResolver {
    fn new(egress: Option<HttpEgressProfile>) -> Arc<Self> {
        Arc::new(Self {
            egress,
            reads: AtomicUsize::new(0),
            outcome: std::sync::Mutex::new(BearerOutcome::Token),
        })
    }

    fn answer(&self, outcome: BearerOutcome) {
        *self.outcome.lock().unwrap() = outcome;
    }
}

#[async_trait::async_trait]
impl McpAuthResolver for BoundResolver {
    async fn stored_bearer_token(
        &self,
        target: &meerkat_auth_core::McpServerIdentity,
    ) -> Result<Option<String>, meerkat_auth_core::McpOAuthError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let server_name = target.server_name().to_owned();
        let outcome = *self.outcome.lock().unwrap();
        match outcome {
            BearerOutcome::Token => Ok(Some("harmless-sentinel".to_owned())),
            BearerOutcome::Refused(refusal) => {
                Err(meerkat_auth_core::McpOAuthError::EgressRefused {
                    server_name,
                    refusal,
                })
            }
            BearerOutcome::Unavailable(reason) => {
                Err(meerkat_auth_core::McpOAuthError::EgressUnavailable {
                    server_name,
                    reason,
                })
            }
        }
    }

    async fn interactive_login(
        &self,
        target: &meerkat_auth_core::McpServerIdentity,
        _www_authenticate: Option<&str>,
    ) -> Result<String, meerkat_auth_core::McpOAuthError> {
        Err(
            meerkat_auth_core::McpOAuthError::HumanAuthorizationRequired {
                server_name: target.server_name().to_owned(),
            },
        )
    }

    fn http_egress(&self) -> Option<HttpEgressProfile> {
        self.egress.clone()
    }
}

/// The credential owner's traffic must enforce the same profile: an unbound
/// resolver is refused before its first credential read or any arrival; one
/// bound to the same profile connects.
#[tokio::test]
async fn an_unbound_credential_owner_is_unavailable_before_any_arrival() {
    let (port, fixture) = spawn_mcp_fixture(None).await;
    let profile = local_profile(port);
    let config = http_config("egress-oauth", format!("http://127.0.0.1:{port}/mcp"));
    for unbound in [None, Some(local_profile(port))] {
        let resolver = BoundResolver::new(unbound);
        match connect_under(&config, &profile, Some(resolver.clone())).await {
            Err(McpError::EgressUnavailable { reason, .. }) => {
                assert_eq!(reason, HttpEgressUnavailable::CredentialOwnerNotBound);
            }
            Err(other) => panic!("expected an unbound credential owner, got {other:?}"),
            Ok(_) => panic!("an unbound credential owner must not connect"),
        }
        assert_eq!(resolver.reads.load(Ordering::SeqCst), 0);
    }
    assert_eq!(fixture.arrivals.load(Ordering::SeqCst), 0);

    let bound = BoundResolver::new(Some(profile.clone()));
    let (connection, _) = connect_under(&config, &profile, Some(bound.clone()))
        .await
        .unwrap();
    connection.close().await.unwrap();
    assert!(bound.reads.load(Ordering::SeqCst) > 0);
    assert!(fixture.arrivals.load(Ordering::SeqCst) > 0);
}

/// Per call: after a successful initialize and enumeration, the bound
/// credential owner refuses its next credential operation (its own refresh
/// outside the profile, or an owner that cannot enforce it). That call fails
/// with its own typed egress outcome and is affirmatively unsent: no
/// tools/call POST and one credential read (no automatic retry). The next
/// permitted call on the same connection is sent.
#[tokio::test]
async fn a_refused_credential_operation_fails_its_own_call_typed_and_unsent() {
    let (port, fixture) = spawn_mcp_fixture(None).await;
    let profile = local_profile(port);
    let resolver = BoundResolver::new(Some(profile.clone()));
    let config = http_config("egress-call", format!("http://127.0.0.1:{port}/mcp"));
    let (connection, tools) = connect_under(&config, &profile, Some(resolver.clone()))
        .await
        .unwrap();
    assert_eq!(tools.len(), 1);
    for outcome in [
        BearerOutcome::Refused(HttpEgressRefusal::HostNotAdmitted),
        BearerOutcome::Unavailable(HttpEgressUnavailable::ClientUnavailable),
    ] {
        resolver.answer(outcome);
        let reads = resolver.reads.load(Ordering::SeqCst);
        let result = connection
            .call_tool("echo", &json!({ "message": "hi" }))
            .await;
        match (outcome, &result) {
            (BearerOutcome::Refused(expected), Err(McpError::EgressRefused { refusal, .. })) => {
                assert_eq!(*refusal, expected);
            }
            (
                BearerOutcome::Unavailable(expected),
                Err(McpError::EgressUnavailable { reason, .. }),
            ) => assert_eq!(*reason, expected),
            _ => panic!("expected the call's own {outcome:?} outcome, got {result:?}"),
        }
        assert_eq!(fixture.tool_calls.load(Ordering::SeqCst), 0);
        assert_eq!(resolver.reads.load(Ordering::SeqCst), reads + 1);
    }
    resolver.answer(BearerOutcome::Token);
    let blocks = connection
        .call_tool("echo", &json!({ "message": "hi" }))
        .await
        .expect("a permitted call is sent");
    assert_eq!(meerkat_core::types::text_content(&blocks), "ok");
    assert_eq!(fixture.tool_calls.load(Ordering::SeqCst), 1);
    connection.close().await.unwrap();
}

/// A required profile never uses the ambient proxy configuration. Proxy
/// variables are process state, so the connection runs in a child process
/// whose environment points every proxy variable at a counting listener.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_required_profile_never_uses_the_ambient_proxy() {
    const CHILD_ENV: &str = "MEERKAT_MCP_EGRESS_PROXY_CHILD";
    const CHILD_COMPLETE: &str = "meerkat-mcp-egress-proxy-child-complete";
    if std::env::var_os(CHILD_ENV).as_deref() != Some(std::ffi::OsStr::new("1")) {
        let (proxy_port, proxy_arrivals) = counting_listener().await;
        let proxy = format!("http://127.0.0.1:{proxy_port}");
        let child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "egress_tests::a_required_profile_never_uses_the_ambient_proxy",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ENV, "1")
            .env("HTTP_PROXY", &proxy)
            .env("http_proxy", &proxy)
            .env("HTTPS_PROXY", &proxy)
            .env("https_proxy", &proxy)
            .env("ALL_PROXY", &proxy)
            .env("all_proxy", &proxy)
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .env_remove("REQUEST_METHOD")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .output();
        let output = tokio::time::timeout(BOUND * 3, child)
            .await
            .expect("isolated proxy fixture timed out")
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "isolated proxy fixture failed: {stdout}"
        );
        // An exact-name mismatch must not pass by running zero tests.
        assert!(stdout.contains(CHILD_COMPLETE), "{stdout}");
        assert_eq!(proxy_arrivals.load(Ordering::SeqCst), 0);
        return;
    }

    let (port, fixture) = spawn_mcp_fixture(None).await;
    let config = http_config("egress-proxy", format!("http://127.0.0.1:{port}/mcp"));
    let (connection, tools) = connect_under(&config, &local_profile(port), None)
        .await
        .expect("a required profile connects directly");
    assert_eq!(tools.len(), 1);
    connection.close().await.unwrap();
    assert!(fixture.arrivals.load(Ordering::SeqCst) > 0);
    println!("{CHILD_COMPLETE}");
}

/// A router retains the profile for its immediate and staged connections.
#[cfg(feature = "workspace-tests")]
#[tokio::test]
async fn a_router_retains_the_profile_for_every_connection() {
    let (port, fixture) = spawn_mcp_fixture(None).await;
    let handle = Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral());
    let mut router = crate::McpRouter::new_with_surface_handle_and_stdio_profile(
        handle,
        crate::McpStdioLaunchProfile::trusted_host(),
    )
    .with_http_egress_profile(local_profile(port));
    let refused = router
        .add_server(http_config(
            "egress-router",
            format!("http://localhost:{port}/mcp"),
        ))
        .await;
    assert!(
        matches!(
            refused,
            Err(McpError::EgressRefused {
                refusal: HttpEgressRefusal::HostNotAdmitted,
                ..
            })
        ),
        "{refused:?}"
    );

    router
        .stage_add(http_config(
            "egress-staged",
            format!("http://localhost:{port}/mcp"),
        ))
        .unwrap();
    router.apply_staged().await.unwrap();
    let notice = tokio::time::timeout(BOUND, async {
        loop {
            let update = router.take_external_updates();
            if let Some(notice) = update.notices.into_iter().find(|notice| {
                notice.target == "egress-staged" && notice.phase == crate::McpLifecyclePhase::Failed
            }) {
                return notice;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a refused staged connection settles as failed");
    assert!(
        notice
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("egress refused"),
        "{:?}",
        notice.detail
    );
    assert_eq!(fixture.arrivals.load(Ordering::SeqCst), 0);

    router
        .add_server(http_config(
            "egress-admitted",
            format!("http://127.0.0.1:{port}/mcp"),
        ))
        .await
        .expect("an admitted destination installs");
    assert!(fixture.arrivals.load(Ordering::SeqCst) > 0);
    router.shutdown().await;
}

/// Characterization (no RED: existing behavior): a stdio launch that asks for
/// one exact network endpoint is unsupported by both shipped backends. It is
/// refused locally before any process exists, under a required HTTP egress
/// profile too, and never downgraded to unrestricted or trusted execution.
#[tokio::test]
async fn a_stdio_per_endpoint_connect_requirement_is_refused_without_downgrade() {
    use meerkat_core::confinement::{
        ConfinementRefusal, ConfinementSpec, ExecutionConfinement, FilesystemAccess,
        IpNetworkAccess, PathAccess, PlatformBaseline,
    };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let requirement: ExecutionConfinement = ConfinementSpec {
        baseline: PlatformBaseline::CommandRuntimeV1,
        read: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.clone())]),
        write: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.clone())]),
        deny_read: vec![],
        deny_write: vec![],
        network: IpNetworkAccess::Connect(vec!["127.0.0.1:8931".parse().unwrap()]),
        unix_connect: vec![],
        require_descendant_termination: false,
    }
    .try_into()
    .unwrap();
    let launch = McpStdioLaunchProfile::required(requirement, root.clone());
    assert!(matches!(
        launch.capabilities(),
        Err(ConfinementRefusal::UnsupportedRequirement)
    ));
    let config = McpServerConfig::stdio(
        "egress-stdio",
        "/bin/sh",
        vec!["-c".into(), "printf entered > \"$MARKER\"".into()],
        HashMap::from([("MARKER".into(), root.join("entered").display().to_string())]),
    );
    let result = tokio::time::timeout(
        BOUND,
        McpConnection::connect_and_enumerate_with_profiles(
            &config,
            McpAuthMode::Stored,
            None,
            None,
            &launch,
            &local_profile(1),
        ),
    )
    .await
    .expect("pre-spawn refusal is bounded");
    assert!(matches!(
        result,
        Err(McpError::Confinement(
            ConfinementRefusal::UnsupportedRequirement
        ))
    ));
    assert!(!root.join("entered").exists());
}
