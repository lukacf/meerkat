//! Connection-boundary evidence using real loopback HTTP requests. The scripted
//! resolver proves dispatch ordering and exact selection propagation only. Native
//! account verification and durable credential admission are tested separately.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use futures::FutureExt;
use meerkat_auth_core::connector_oauth::ConnectorOAuthRefusal;
use meerkat_auth_core::{McpAuthMode, McpOAuthError, McpServerIdentity};
use meerkat_core::McpServerConfig;
use meerkat_core::mcp_config::{McpHttpTransport, McpTransportConfig};
use meerkat_mcp::{McpAuthResolver, McpClientServiceFactory, McpConnection, McpError};
use rmcp::service::DynService;
use rmcp::{RoleClient, ServiceExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const LIMIT: Duration = Duration::from_secs(10);
const ACCOUNT: &str = "fixture-subject-a";
const TOKEN: &str = "fixture-only-bearer-a";

#[derive(Clone, Debug, PartialEq, Eq)]
enum Event {
    Factory(McpServerConfig),
    Stored(McpServerIdentity),
    Login(McpServerIdentity, Option<String>),
    Http(Option<String>),
}

type Trace = Arc<Mutex<Vec<Event>>>;

struct Endpoint {
    url: String,
    trace: Trace,
    stop: oneshot::Sender<()>,
    task: JoinHandle<std::io::Result<()>>,
}

impl Endpoint {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let trace = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .fallback(observe_http)
            .with_state(trace.clone());
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        });
        Self {
            url,
            trace,
            stop,
            task,
        }
    }

    async fn shutdown(self) -> Result<(), String> {
        let Self { stop, mut task, .. } = self;
        let _ = stop.send(());
        match tokio::time::timeout(LIMIT, &mut task).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(error))) => Err(format!("HTTP fixture failed: {error}")),
            Ok(Err(error)) => Err(format!("HTTP fixture join failed: {error}")),
            Err(_) => {
                task.abort();
                let joined = task.await;
                Err(format!(
                    "HTTP fixture shutdown timed out; abort joined: {joined:?}"
                ))
            }
        }
    }
}

async fn observe_http(State(trace): State<Trace>, headers: HeaderMap) -> StatusCode {
    trace.lock().unwrap().push(Event::Http(
        headers
            .get("authorization")
            .map(|value| value.to_str().unwrap().to_string()),
    ));
    // A non-auth response ends the initialize attempt deterministically. No MCP
    // success, provider identity, or supported tool operation is inferred here.
    StatusCode::BAD_REQUEST
}

struct Factory(Trace);

impl McpClientServiceFactory for Factory {
    fn create(
        &self,
        config: &McpServerConfig,
    ) -> Result<Box<dyn DynService<RoleClient>>, McpError> {
        self.0.lock().unwrap().push(Event::Factory(config.clone()));
        Ok(().into_dyn())
    }
}

#[derive(Clone, Copy)]
enum Stored {
    Missing,
    Token,
    Mismatch,
    ReauthRequired,
    /// The stored token's refresh was refused by a token endpoint whose body
    /// echoes secrets, rendered exactly as the refresh path renders it.
    RefreshRefusedEchoingSecrets,
}

const REFRESH_BODY_CANARY: &str = "mcp-refresh-error-body-secret-canary";

#[derive(Clone, Copy)]
enum Login {
    Token,
    Mismatch,
}

struct Resolver {
    trace: Trace,
    stored: Stored,
    login: Login,
}

#[async_trait]
impl McpAuthResolver for Resolver {
    async fn stored_bearer_token(
        &self,
        target: &McpServerIdentity,
    ) -> Result<Option<String>, McpOAuthError> {
        self.trace
            .lock()
            .unwrap()
            .push(Event::Stored(target.clone()));
        match self.stored {
            Stored::Missing => Ok(None),
            Stored::Token => Ok(Some(TOKEN.into())),
            Stored::Mismatch => Err(ConnectorOAuthRefusal::AccountMismatch.into()),
            Stored::ReauthRequired => Err(McpOAuthError::ReauthRequired {
                server_name: target.server_name().to_string(),
            }),
            Stored::RefreshRefusedEchoingSecrets => Err(McpOAuthError::RefreshFailed {
                server_name: target.server_name().to_string(),
                reason: meerkat_auth_core::auth_oauth::OAuthError::TokenEndpoint {
                    status: 500,
                    body: format!(
                        r#"{{"error":"server_error","error_description":"{REFRESH_BODY_CANARY}","refresh_token":"{REFRESH_BODY_CANARY}"}}"#
                    ),
                }
                .to_string(),
            }),
        }
    }

    async fn interactive_login(
        &self,
        target: &McpServerIdentity,
        challenge: Option<&str>,
    ) -> Result<String, McpOAuthError> {
        self.trace
            .lock()
            .unwrap()
            .push(Event::Login(target.clone(), challenge.map(str::to_string)));
        match self.login {
            Login::Token => Ok(TOKEN.into()),
            Login::Mismatch => Err(ConnectorOAuthRefusal::AccountMismatch.into()),
        }
    }
}

fn selected(endpoint: &Endpoint, account: &str) -> McpServerConfig {
    let mut config =
        McpServerConfig::streamable_http("account-fixture", endpoint.url.clone(), HashMap::new());
    let McpTransportConfig::Http(http) = &mut config.transport else {
        unreachable!();
    };
    http.oauth_account = Some(account.to_string());
    config
}

/// All assertions run after the owned HTTP task is joined. Unexpected MCP
/// success also closes its connection before the assertion fails. Panic and
/// timeout paths still stop and join the loopback fixture.
async fn observe(
    endpoint: Endpoint,
    config: &McpServerConfig,
    mode: McpAuthMode,
    resolver_script: Option<(Stored, Login)>,
) -> (McpError, Vec<Event>) {
    let trace = endpoint.trace.clone();
    let resolver = resolver_script.map(|(stored, login)| {
        Arc::new(Resolver {
            trace: trace.clone(),
            stored,
            login,
        }) as Arc<dyn McpAuthResolver>
    });
    let factory = Arc::new(Factory(trace.clone()));
    let result = AssertUnwindSafe(async {
        match tokio::time::timeout(
            LIMIT,
            McpConnection::connect_with_services(config, mode, resolver, Some(factory)),
        )
        .await
        {
            Ok(Err(error)) => Ok(error),
            Ok(Ok(connection)) => {
                let closed = tokio::time::timeout(LIMIT, connection.close()).await;
                Err(format!("unexpected MCP success; close outcome: {closed:?}"))
            }
            Err(_) => Err("MCP connection did not return within the fixture deadline".into()),
        }
    })
    .catch_unwind()
    .await;
    let shutdown = endpoint.shutdown().await;
    if let Err(panic) = result {
        if let Err(error) = shutdown {
            eprintln!("cleanup after connection panic: {error}");
        }
        std::panic::resume_unwind(panic);
    }
    shutdown.expect("HTTP fixture must be joined successfully");
    let error = result
        .unwrap()
        .expect("fixture must return a connection refusal");
    let events = trace.lock().unwrap().clone();
    (error, events)
}

fn assert_account_mismatch(error: &McpError) {
    assert!(
        matches!(
            error,
            McpError::OAuthAccountRejected(McpOAuthError::Verification(
                ConnectorOAuthRefusal::AccountMismatch
            ))
        ),
        "expected typed account mismatch, got {error:?}"
    );
}

#[tokio::test]
async fn selected_static_authorization_refuses_before_factory_or_http() {
    let endpoint = Endpoint::start().await;
    let mut config = selected(&endpoint, ACCOUNT);
    let McpTransportConfig::Http(http) = &mut config.transport else {
        unreachable!();
    };
    http.headers
        .insert("aUtHoRiZaTiOn".into(), "Bearer fixture-static".into());
    let (error, events) = observe(
        endpoint,
        &config,
        McpAuthMode::Interactive,
        Some((Stored::Token, Login::Token)),
    )
    .await;
    assert!(matches!(
        error,
        McpError::OAuthAccountRejected(McpOAuthError::UnsupportedAccountSelection)
    ));
    assert!(
        events.is_empty(),
        "refusal must precede all fixture effects: {events:?}"
    );
}

#[tokio::test]
async fn selected_sse_refuses_before_factory_or_http() {
    let endpoint = Endpoint::start().await;
    let mut config = selected(&endpoint, ACCOUNT);
    let McpTransportConfig::Http(http) = &mut config.transport else {
        unreachable!();
    };
    http.transport = Some(McpHttpTransport::Sse);
    let (error, events) = observe(
        endpoint,
        &config,
        McpAuthMode::Interactive,
        Some((Stored::Token, Login::Token)),
    )
    .await;
    assert!(matches!(
        error,
        McpError::OAuthAccountRejected(McpOAuthError::UnsupportedAccountSelection)
    ));
    assert!(
        events.is_empty(),
        "refusal must precede all fixture effects: {events:?}"
    );
}

#[tokio::test]
async fn invalid_selected_account_refuses_before_factory_or_http() {
    for account in [
        "".to_string(),
        "   ".to_string(),
        "subject\nother".to_string(),
        "x".repeat(4097),
    ] {
        let endpoint = Endpoint::start().await;
        let config = selected(&endpoint, &account);
        let (error, events) = observe(
            endpoint,
            &config,
            McpAuthMode::Interactive,
            Some((Stored::Token, Login::Token)),
        )
        .await;
        assert!(matches!(
            error,
            McpError::OAuthAccountRejected(McpOAuthError::InvalidAccountSelection)
        ));
        assert!(
            events.is_empty(),
            "refusal must precede all fixture effects: {events:?}"
        );
    }
}

#[tokio::test]
async fn selected_without_resolver_refuses_before_factory_or_http() {
    let endpoint = Endpoint::start().await;
    let config = selected(&endpoint, ACCOUNT);
    let (error, events) = observe(endpoint, &config, McpAuthMode::Interactive, None).await;
    assert!(matches!(
        error,
        McpError::OAuthAccountRejected(McpOAuthError::UnsupportedAccountSelection)
    ));
    assert!(
        events.is_empty(),
        "refusal must precede all fixture effects: {events:?}"
    );
}

#[tokio::test]
async fn selected_missing_stored_credential_never_contacts_mcp_server() {
    let endpoint = Endpoint::start().await;
    let config = selected(&endpoint, ACCOUNT);
    let target = McpServerIdentity::from_config(&config).unwrap();
    let (error, events) = observe(
        endpoint,
        &config,
        McpAuthMode::Stored,
        Some((Stored::Missing, Login::Token)),
    )
    .await;
    assert!(matches!(
        error,
        McpError::OAuthAccountRejected(McpOAuthError::MissingStoredToken { .. })
    ));
    assert_eq!(events, [Event::Factory(config), Event::Stored(target)]);
}

#[tokio::test]
async fn selected_interactive_login_precedes_first_mcp_request() {
    let endpoint = Endpoint::start().await;
    let config = selected(&endpoint, ACCOUNT);
    let target = McpServerIdentity::from_config(&config).unwrap();
    let (error, events) = observe(
        endpoint,
        &config,
        McpAuthMode::Interactive,
        Some((Stored::Missing, Login::Token)),
    )
    .await;
    assert!(matches!(error, McpError::ConnectionFailed { .. }));
    assert_eq!(target.expected_account(), Some(ACCOUNT));
    assert_eq!(
        events,
        [
            Event::Factory(config),
            Event::Stored(target.clone()),
            Event::Login(target, None),
            Event::Http(Some(format!("Bearer {TOKEN}"))),
        ]
    );
}

#[tokio::test]
async fn selected_stored_token_keeps_exact_selection_and_bearer() {
    let endpoint = Endpoint::start().await;
    let config = selected(&endpoint, ACCOUNT);
    let target = McpServerIdentity::from_config(&config).unwrap();
    let (error, events) = observe(
        endpoint,
        &config,
        McpAuthMode::Stored,
        Some((Stored::Token, Login::Token)),
    )
    .await;
    assert!(matches!(error, McpError::ConnectionFailed { .. }));
    assert_eq!(
        events,
        [
            Event::Factory(config),
            Event::Stored(target),
            Event::Http(Some(format!("Bearer {TOKEN}"))),
        ]
    );
}

#[tokio::test]
async fn selected_stored_account_mismatch_is_typed_without_login_or_http() {
    let endpoint = Endpoint::start().await;
    let config = selected(&endpoint, ACCOUNT);
    let target = McpServerIdentity::from_config(&config).unwrap();
    let (error, events) = observe(
        endpoint,
        &config,
        McpAuthMode::Interactive,
        Some((Stored::Mismatch, Login::Token)),
    )
    .await;
    assert_account_mismatch(&error);
    assert_eq!(events, [Event::Factory(config), Event::Stored(target)]);
}

/// A refused refresh never puts the token endpoint's body into the MCP
/// connection failure: that error's text is the agent-visible connection
/// notice (`ExternalToolDelta` detail) and tool error.
#[tokio::test]
async fn refused_refresh_body_never_reaches_the_connection_failure() {
    let endpoint = Endpoint::start().await;
    let config = selected(&endpoint, ACCOUNT);
    let (error, _events) = observe(
        endpoint,
        &config,
        McpAuthMode::Stored,
        Some((Stored::RefreshRefusedEchoingSecrets, Login::Token)),
    )
    .await;
    let rendered = format!("{error} {error:?}");
    assert!(
        !rendered.contains(REFRESH_BODY_CANARY),
        "the refresh error body reached the connection failure: {rendered}"
    );
    assert!(
        rendered.contains("status=500 error=server_error"),
        "{rendered}"
    );
}

#[tokio::test]
async fn selected_interactive_account_mismatch_is_typed_without_http() {
    let endpoint = Endpoint::start().await;
    let config = selected(&endpoint, ACCOUNT);
    let target = McpServerIdentity::from_config(&config).unwrap();
    let (error, events) = observe(
        endpoint,
        &config,
        McpAuthMode::Interactive,
        Some((Stored::Missing, Login::Mismatch)),
    )
    .await;
    assert_account_mismatch(&error);
    assert_eq!(
        events,
        [
            Event::Factory(config),
            Event::Stored(target.clone()),
            Event::Login(target, None),
        ]
    );
}

#[tokio::test]
async fn selected_reauth_required_allows_login_before_first_mcp_request() {
    let endpoint = Endpoint::start().await;
    let config = selected(&endpoint, ACCOUNT);
    let target = McpServerIdentity::from_config(&config).unwrap();
    let (error, events) = observe(
        endpoint,
        &config,
        McpAuthMode::Interactive,
        Some((Stored::ReauthRequired, Login::Token)),
    )
    .await;
    assert!(matches!(error, McpError::ConnectionFailed { .. }));
    assert_eq!(
        events,
        [
            Event::Factory(config),
            Event::Stored(target.clone()),
            Event::Login(target, None),
            Event::Http(Some(format!("Bearer {TOKEN}"))),
        ]
    );
}
