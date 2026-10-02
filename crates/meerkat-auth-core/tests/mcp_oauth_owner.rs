#![cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

// The test crate and runtime depend on the SAME normal auth-core library.
// Keeping these cases in lib-test would duplicate its OAuth trait identity.
use async_trait::async_trait;
use chrono::Utc;
use meerkat_auth_core::auth_oauth::OAuthTokenResult;
use meerkat_auth_core::auth_store::{
    PersistedAuthMode, PersistedTokens, ProviderAuthPersistence, TokenKey, TokenStore,
};
use meerkat_auth_core::connector_oauth::{
    ConnectorAccountObservation, ConnectorOAuthDescriptor, ConnectorOAuthRefusal,
};
use meerkat_auth_core::mcp_oauth::{
    MCP_INTERACTIVE_LOGIN_TIMEOUT, MCP_OAUTH_CALLBACK_PATH, McpOAuthAccountStrategy,
    McpOAuthAuthority, McpOAuthBrowserLaunch, McpOAuthCallback, McpOAuthCeremonyContext,
    McpOAuthError, McpOAuthLoginDisposition, McpOAuthLoginStart, McpOAuthLoopbackBegin,
    McpServerIdentity,
};
use meerkat_core::generated::auth_lease_durable_lifecycle_marker as durable_marker;
use meerkat_core::handles::{AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, GeneratedAuthLeaseHandle};
use meerkat_runtime::handles::RuntimeOAuthFlowHandle;
use reqwest::Client;
use std::sync::Arc;
use std::time::Duration;

// This fixture retains existing native handles; it defines no auth state,
// terminal-owner marker, alternate registry, or production API.
/// Test-side browser. Production hosts own the browser; the native authority
/// no longer opens one.
#[async_trait]
trait TestBrowser: Send + Sync {
    async fn open(&self, url: &str) -> Result<(), McpOAuthError>;
}

/// Retires an abandoned attempt if the host's wait is cancelled or fails.
struct PendingLoginGuard<'a> {
    authority: &'a McpOAuthAuthority,
    target: &'a McpServerIdentity,
    start: Option<McpOAuthLoginStart>,
}

impl Drop for PendingLoginGuard<'_> {
    fn drop(&mut self) {
        if let Some(start) = self.start.take() {
            let _ = self.authority.login_cancel(self.target, &start);
        }
    }
}

/// The host role for the split seam: own the loopback listener and browser,
/// admit through `login_start`, deliver the callback to `login_complete`.
async fn host_login(
    authority: &McpOAuthAuthority,
    browser: &dyn TestBrowser,
    target: &McpServerIdentity,
    www_authenticate: Option<&str>,
) -> Result<String, McpOAuthError> {
    let exchange_failed = |reason: String| McpOAuthError::TokenExchangeFailed {
        server_name: target.server_name().to_owned(),
        reason,
    };
    let binding = meerkat_auth_core::auth_oauth::bind_loopback_callback(MCP_OAUTH_CALLBACK_PATH)
        .await
        .map_err(|error| exchange_failed(error.to_string()))?;
    let start = match authority
        .login_start(target, &binding.redirect_url, www_authenticate)
        .await
    {
        Ok(start) => start,
        Err(error) => {
            let _ = binding.cancel().await;
            return Err(error);
        }
    };
    let mut guard = PendingLoginGuard {
        authority,
        target,
        start: Some(start.clone()),
    };
    let callback = binding.expect_state(start.state.clone());
    if let Err(error) = browser.open(&start.authorize_url).await {
        let _ = callback.cancel().await;
        return Err(error);
    }
    let outcome = callback
        .wait(MCP_INTERACTIVE_LOGIN_TIMEOUT)
        .await
        .map_err(|error| exchange_failed(error.to_string()))?;
    guard.start = None;
    authority
        .login_complete(
            target,
            McpOAuthCallback {
                redirect_uri: start.redirect_uri,
                state: outcome.state,
                code: outcome.code,
            },
        )
        .await?;
    authority.require_stored_bearer_token(target).await
}

#[derive(Clone)]
struct FixtureAuthority {
    native: McpOAuthAuthority,
    auth_lease: GeneratedAuthLeaseHandle,
    flows: Arc<RuntimeOAuthFlowHandle>,
    browser: Arc<dyn TestBrowser>,
}

impl std::ops::Deref for FixtureAuthority {
    type Target = McpOAuthAuthority;
    fn deref(&self) -> &Self::Target {
        &self.native
    }
}

impl FixtureAuthority {
    fn with_fixture_http(
        persistence: ProviderAuthPersistence,
        browser: Arc<dyn TestBrowser>,
        http: Client,
        owner: TestAuthAuthority,
    ) -> Self {
        let flows = Arc::new(RuntimeOAuthFlowHandle::new_with_auth_lease(
            MCP_INTERACTIVE_LOGIN_TIMEOUT,
            owner.lifecycle,
        ));
        let native = McpOAuthAuthority::with_http(persistence, http, owner.generated.clone())
            .with_interactive_strategy(flows.clone(), Arc::new(FixtureAccountStrategy))
            .expect("fixture uses the actual matched runtime flow owner");
        Self {
            native,
            auth_lease: owner.generated,
            flows,
            browser,
        }
    }

    async fn interactive_login(
        &self,
        target: &McpServerIdentity,
        www_authenticate: Option<&str>,
    ) -> Result<String, McpOAuthError> {
        host_login(
            &self.native,
            self.browser.as_ref(),
            target,
            www_authenticate,
        )
        .await
    }

    fn with_test_http(
        token_store: Arc<EphemeralTokenStore>,
        browser: Arc<dyn TestBrowser>,
        http: Client,
        owner: TestAuthAuthority,
    ) -> Self {
        Self::with_fixture_http(
            ProviderAuthPersistence::new(token_store, Arc::new(InMemoryCoordinator::new())),
            browser,
            http,
            owner,
        )
    }

    fn publish_login_tokens_via_lease(
        &self,
        target: &McpServerIdentity,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<PersistedTokens, McpOAuthError> {
        let lease_key = target.lease_key()?;
        let lifecycle_err =
            |error: meerkat_core::handles::DslTransitionError| McpOAuthError::AuthLifecycle {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            };
        // Clear the credential side before publishing the freshly minted
        // credential. This transition is fallible and therefore participates
        // in the caller's snapshot-backed transaction; it is never ignored.
        self.auth_lease
            .release_credential_lifecycle(&lease_key)
            .map_err(lifecycle_err)?;
        let expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(tokens);
        let transition = self
            .auth_lease
            .acquire_lease(&lease_key, expires_at)
            .map_err(lifecycle_err)?;
        meerkat_core::mark_tokens_lifecycle_published_for_transition(key, tokens, &transition)
            .map_err(|error| McpOAuthError::AuthLifecycle {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            })
    }
}

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use meerkat_auth_core::{EphemeralTokenStore, InMemoryCoordinator};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::net::TcpListener;
use tokio::sync::Notify;

/// A certified `AuthMachine` lease handle for tests. `meerkat-runtime` is a
/// dev-dependency, so tests can mint the same generated lease the CLI
/// injects in production (the lib cannot, to avoid a dep cycle).
#[derive(Clone)]
struct TestAuthAuthority {
    lifecycle: Arc<meerkat_runtime::RuntimeAuthLeaseHandle>,
    generated: GeneratedAuthLeaseHandle,
}
impl std::ops::Deref for TestAuthAuthority {
    type Target = GeneratedAuthLeaseHandle;
    fn deref(&self) -> &Self::Target {
        &self.generated
    }
}
fn test_auth_lease() -> TestAuthAuthority {
    let lifecycle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    let generated =
        meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(
            lifecycle.clone(),
        )
        .unwrap();
    TestAuthAuthority {
        lifecycle,
        generated,
    }
}

struct FixtureAccountStrategy;
#[async_trait]
impl McpOAuthAccountStrategy for FixtureAccountStrategy {
    fn descriptor(
        &self,
        _target: &McpServerIdentity,
        context: &McpOAuthCeremonyContext<'_>,
    ) -> Result<ConnectorOAuthDescriptor, ConnectorOAuthRefusal> {
        meerkat_auth_core::connector_oauth::ConnectorOAuthParameters {
            issuer: context.issuer.to_owned(),
            client: context.client.to_owned(),
            resource: context.resource.to_owned(),
            redirect_uri: context.redirect_uri.to_owned(),
            scopes: ["mcp.read".to_owned()].into(),
            expected_account: "fixture-account-42".into(),
            strategy_id: "test-provider-authenticated-evidence".into(),
        }
        .try_into()
    }
    async fn observe_account(
        &self,
        _descriptor: &ConnectorOAuthDescriptor,
        tokens: &OAuthTokenResult,
    ) -> Result<ConnectorAccountObservation, ConnectorOAuthRefusal> {
        // This is fixture-provider evidence, not a general issuer strategy.
        // The exact token and scope must be produced by its real HTTP flow.
        if tokens.access_token != "access-token" || tokens.scope.as_deref() != Some("mcp.read") {
            return Err(ConnectorOAuthRefusal::VerificationUnavailable);
        }
        Ok(ConnectorAccountObservation {
            account: "fixture-account-42".into(),
            granted_scopes: ["mcp.read".to_owned()].into(),
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
enum AuthorizeOutcome {
    #[default]
    Success,
    StateMismatch,
    Denied,
    NoCallback,
}

#[derive(Default)]
struct TestState {
    opened_url: Mutex<Option<String>>,
    redirect_uri: Mutex<Option<String>>,
    registration_requests: Mutex<Vec<Value>>,
    token_requests: Mutex<Vec<Value>>,
    token_error_responses: Mutex<Vec<Value>>,
    include_registration_endpoint: Mutex<bool>,
    omit_token_endpoint_auth_method: Mutex<bool>,
    resource_override: Mutex<Option<String>>,
    issuer_override: Mutex<Option<String>>,
    authorize_outcome: Mutex<AuthorizeOutcome>,
    token_fails: Mutex<bool>,
    token_transiently_fails: Mutex<bool>,
    /// The refresh is refused with a body that echoes secrets (a
    /// non-conforming authorization server).
    token_refresh_echoes_secrets: Mutex<bool>,
    token_scope_override: Mutex<Option<String>>,
    userinfo_sub: Mutex<Option<String>>,
    omit_userinfo_endpoint: Mutex<bool>,
    userinfo_endpoint_override: Mutex<Option<String>>,
    request_paths: Mutex<Vec<String>>,
    redirect_authorization_metadata: AtomicBool,
    redirect_token: AtomicBool,
    userinfo_requests: Mutex<Vec<Option<String>>>,
    pause_refresh: AtomicBool,
    refresh_started: Notify,
    refresh_release: Notify,
}

struct RecordingBrowser {
    state: Arc<TestState>,
    http: Client,
}

#[async_trait]
impl TestBrowser for RecordingBrowser {
    async fn open(&self, url: &str) -> Result<(), McpOAuthError> {
        *self.state.opened_url.lock() = Some(url.to_string());
        let _response = self.http.get(url).send().await.unwrap();
        Ok(())
    }
}

struct NoCallbackBrowser {
    state: Arc<TestState>,
}

struct FailNextSaveStore {
    inner: Arc<EphemeralTokenStore>,
    fail_next_save: AtomicBool,
}

#[cfg(feature = "file-lock")]
struct FailNextSaveDynStore {
    inner: Arc<dyn TokenStore>,
    fail_next_save: AtomicBool,
}

struct FailClearStore {
    inner: Arc<EphemeralTokenStore>,
}

#[async_trait]
impl TokenStore for FailClearStore {
    async fn load(
        &self,
        key: &TokenKey,
    ) -> Result<Option<PersistedTokens>, meerkat_auth_core::auth_store::TokenStoreError> {
        self.inner.load(key).await
    }

    async fn save(
        &self,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), meerkat_auth_core::auth_store::TokenStoreError> {
        self.inner.save(key, tokens).await
    }

    async fn clear(
        &self,
        _key: &TokenKey,
    ) -> Result<(), meerkat_auth_core::auth_store::TokenStoreError> {
        Err(meerkat_auth_core::auth_store::TokenStoreError::Io(
            "injected clear failure".to_string(),
        ))
    }

    async fn list(&self) -> Result<Vec<TokenKey>, meerkat_auth_core::auth_store::TokenStoreError> {
        self.inner.list().await
    }

    fn backend_name(&self) -> &'static str {
        "fail-clear"
    }
}

#[cfg(feature = "file-lock")]
#[async_trait]
impl TokenStore for FailNextSaveDynStore {
    async fn load(
        &self,
        key: &TokenKey,
    ) -> Result<Option<PersistedTokens>, meerkat_auth_core::auth_store::TokenStoreError> {
        self.inner.load(key).await
    }

    async fn save(
        &self,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), meerkat_auth_core::auth_store::TokenStoreError> {
        if self.fail_next_save.swap(false, Ordering::SeqCst) {
            return Err(meerkat_auth_core::auth_store::TokenStoreError::Io(
                "injected save failure".to_string(),
            ));
        }
        self.inner.save(key, tokens).await
    }

    async fn clear(
        &self,
        key: &TokenKey,
    ) -> Result<(), meerkat_auth_core::auth_store::TokenStoreError> {
        self.inner.clear(key).await
    }

    async fn list(&self) -> Result<Vec<TokenKey>, meerkat_auth_core::auth_store::TokenStoreError> {
        self.inner.list().await
    }

    fn backend_name(&self) -> &'static str {
        "fail-next-save-dyn"
    }
}

#[async_trait]
impl TokenStore for FailNextSaveStore {
    async fn load(
        &self,
        key: &TokenKey,
    ) -> Result<Option<PersistedTokens>, meerkat_auth_core::auth_store::TokenStoreError> {
        self.inner.load(key).await
    }

    async fn save(
        &self,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), meerkat_auth_core::auth_store::TokenStoreError> {
        if self.fail_next_save.swap(false, Ordering::SeqCst) {
            return Err(meerkat_auth_core::auth_store::TokenStoreError::Io(
                "injected save failure".to_string(),
            ));
        }
        self.inner.save(key, tokens).await
    }

    async fn clear(
        &self,
        key: &TokenKey,
    ) -> Result<(), meerkat_auth_core::auth_store::TokenStoreError> {
        self.inner.clear(key).await
    }

    async fn list(&self) -> Result<Vec<TokenKey>, meerkat_auth_core::auth_store::TokenStoreError> {
        self.inner.list().await
    }

    fn backend_name(&self) -> &'static str {
        "fail-next-save"
    }
}

#[async_trait]
impl TestBrowser for NoCallbackBrowser {
    async fn open(&self, url: &str) -> Result<(), McpOAuthError> {
        *self.state.opened_url.lock() = Some(url.to_string());
        // All real discovery and DCR I/O already completed. Only now pause
        // Tokio so the unchanged native 300-second callback timer can elapse.
        // No callback result is substituted, and AuthMachine remains real.
        tokio::time::pause();
        Ok(())
    }
}

async fn spawn_oauth_fixture() -> (String, Arc<TestState>) {
    let state = Arc::new(TestState {
        include_registration_endpoint: Mutex::new(true),
        ..TestState::default()
    });
    let app = Router::new()
        .route("/mcp", post(mcp_endpoint))
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_metadata),
        )
        .route("/register", post(register_client))
        .route("/authorize", get(authorize))
        .route("/token", post(token))
        .route(
            "/.well-known/openid-configuration",
            get(openid_configuration),
        )
        .route("/userinfo", get(userinfo))
        .route("/redirected", get(redirected).post(redirected))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            record_request,
        ))
        .with_state(Arc::clone(&state));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), state)
}

async fn record_request(
    State(state): State<Arc<TestState>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    state
        .request_paths
        .lock()
        .push(request.uri().path().to_owned());
    next.run(request).await
}

async fn redirected() -> impl IntoResponse {
    (StatusCode::OK, "redirect target must never be reached")
}

async fn mcp_endpoint(headers: HeaderMap) -> impl IntoResponse {
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        == Some("Bearer access-token")
    {
        return (
            StatusCode::OK,
            Json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{}})),
        )
            .into_response();
    }
    (
        StatusCode::UNAUTHORIZED,
        [(
            "www-authenticate",
            r#"Bearer error="invalid_request", resource_metadata="/.well-known/oauth-protected-resource/mcp""#,
        )],
        "",
    )
        .into_response()
}

async fn protected_resource(
    State(state): State<Arc<TestState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let host = headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .unwrap();
    let resource = state
        .resource_override
        .lock()
        .clone()
        .unwrap_or_else(|| format!("http://{host}/mcp"));
    Json(serde_json::json!({
        "resource": resource,
        "authorization_servers": [format!("http://{host}")],
        "scopes_supported": ["mcp.read"]
    }))
}

async fn authorization_metadata(
    State(state): State<Arc<TestState>>,
    headers: HeaderMap,
) -> axum::response::Response {
    if state.redirect_authorization_metadata.load(Ordering::SeqCst) {
        return Redirect::temporary("/redirected").into_response();
    }
    let host = headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .unwrap();
    let issuer = state
        .issuer_override
        .lock()
        .clone()
        .unwrap_or_else(|| format!("http://{host}"));
    let mut body = serde_json::json!({
        "issuer": issuer,
        "code_challenge_methods_supported": ["S256"],
        "authorization_endpoint": "/authorize",
        "token_endpoint": "/token",
    });
    if *state.include_registration_endpoint.lock() {
        body["registration_endpoint"] = serde_json::json!("/register");
    }
    Json(body).into_response()
}

async fn register_client(
    State(state): State<Arc<TestState>>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let redirect_uri = body["redirect_uris"][0].as_str().unwrap().to_string();
    state.registration_requests.lock().push(body);
    *state.redirect_uri.lock() = Some(redirect_uri);
    Json(serde_json::json!({
        "client_id": "client-123",
        "client_secret": "ignored-secret-for-public-client",
        "token_endpoint_auth_method": if *state.omit_token_endpoint_auth_method.lock() {
            Value::Null
        } else {
            Value::String("none".to_string())
        }
    }))
}

async fn authorize(
    State(state): State<Arc<TestState>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let redirect_uri = state.redirect_uri.lock().clone().unwrap();
    let state_param = params.get("state").unwrap();
    match *state.authorize_outcome.lock() {
        AuthorizeOutcome::Success => Redirect::temporary(&format!(
            "{redirect_uri}?code=fixture-code&state={state_param}"
        ))
        .into_response(),
        AuthorizeOutcome::StateMismatch => {
            Redirect::temporary(&format!("{redirect_uri}?code=fixture-code&state=wrong"))
                .into_response()
        }
        AuthorizeOutcome::Denied => Redirect::temporary(&format!(
            "{redirect_uri}?error=access_denied&state={state_param}"
        ))
        .into_response(),
        AuthorizeOutcome::NoCallback => {
            (StatusCode::OK, "authorization left pending").into_response()
        }
    }
}

const TOKEN_EXCHANGE_ERROR_CANARY: &str = "synthetic-provider-error-secret-canary";
const REFRESH_ERROR_BODY_CANARY: &str = "synthetic-refresh-error-body-secret-canary";

async fn token(
    State(state): State<Arc<TestState>>,
    Form(body): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    state
        .token_requests
        .lock()
        .push(serde_json::to_value(&body).unwrap());
    if state.redirect_token.load(Ordering::SeqCst) {
        return Redirect::temporary("/redirected").into_response();
    }
    if body.get("grant_type").map(String::as_str) == Some("refresh_token")
        && state.pause_refresh.load(Ordering::SeqCst)
    {
        state.refresh_started.notify_one();
        state.refresh_release.notified().await;
    }
    if *state.token_fails.lock() {
        // RFC 6749 §5.2 error response: a JSON body carrying the typed
        // `error` code. The refresh-failure classifier parses this `error`
        // field to decide reauth-vs-retry.
        let body = serde_json::json!({
            "error": "invalid_grant",
            "error_description": TOKEN_EXCHANGE_ERROR_CANARY,
        });
        state.token_error_responses.lock().push(serde_json::json!({
            "status": StatusCode::BAD_REQUEST.as_u16(),
            "body": body,
        }));
        return (StatusCode::BAD_REQUEST, Json(body)).into_response();
    }
    if *state.token_refresh_echoes_secrets.lock() {
        let body = serde_json::json!({
            // Not a well-formed RFC 6749 code: never rendered either.
            "error": format!("server_error {REFRESH_ERROR_BODY_CANARY}"),
            "error_description": REFRESH_ERROR_BODY_CANARY,
            "echoed_grant": body.get("refresh_token"),
        });
        state.token_error_responses.lock().push(serde_json::json!({
            "status": StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            "body": body,
        }));
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response();
    }
    if *state.token_transiently_fails.lock() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "temporarily_unavailable" })),
        )
            .into_response();
    }
    let scope = state
        .token_scope_override
        .lock()
        .clone()
        .unwrap_or_else(|| "mcp.read".to_owned());
    Json(serde_json::json!({
        "access_token": "access-token",
        "refresh_token": "refresh-token",
        "expires_in": 3600,
        "scope": scope
    }))
    .into_response()
}

async fn openid_configuration(
    State(state): State<Arc<TestState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let host = headers
        .get("host")
        .and_then(|value| value.to_str().ok())
        .unwrap();
    let mut body = serde_json::json!({ "issuer": format!("http://{host}") });
    if !*state.omit_userinfo_endpoint.lock() {
        let endpoint = state
            .userinfo_endpoint_override
            .lock()
            .clone()
            .unwrap_or_else(|| format!("http://{host}/userinfo"));
        body["userinfo_endpoint"] = serde_json::json!(endpoint);
    }
    Json(body)
}

async fn userinfo(State(state): State<Arc<TestState>>, headers: HeaderMap) -> impl IntoResponse {
    let authorization = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    state.userinfo_requests.lock().push(authorization.clone());
    if authorization.as_deref() != Some("Bearer access-token") {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let sub = state
        .userinfo_sub
        .lock()
        .clone()
        .unwrap_or_else(|| "oidc-subject-7".to_owned());
    Json(serde_json::json!({ "sub": sub })).into_response()
}

#[tokio::test]
async fn interactive_login_token_write_is_lease_published() {
    // Row #349 gate (2): the post-login durable token write carries the
    // AuthMachine lease lifecycle marker — the credential write is owned by
    // a committed lease transition, not a bare store.save.
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let browser = recording_browser(Arc::clone(&state));
    let auth_lease = test_auth_lease();
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), auth_lease.clone());
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();

    authority
        .interactive_login(&target, None)
        .await
        .expect("login succeeds");

    let stored = store
        .load(&target.token_key().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(
        meerkat_core::tokens_lifecycle_published(&stored),
        "post-login token write must be wrapped in an AuthMachine lease transition"
    );
    // The pre-existing MCP metadata survives alongside the marker.
    assert_eq!(stored.metadata["client"]["client_id"], "client-123");
}

#[tokio::test]
async fn refreshed_token_write_is_lease_published() {
    // The refresh-success persist mirrors the login path: refreshed
    // tokens carry the AuthMachine lease lifecycle marker stamped at the
    // refreshed credential's own expiry — never a bare store.save that
    // drops the lifecycle-published marker.
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let browser = recording_browser(Arc::clone(&state));
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), test_auth_lease());
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();

    authority
        .interactive_login(&target, None)
        .await
        .expect("initial login succeeds");
    let key = target.token_key().unwrap();
    republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
        tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
    })
    .await;

    let token = authority
        .stored_bearer_token(&target)
        .await
        .expect("expired token refreshes")
        .expect("refresh yields a bearer token");
    assert_eq!(token, "access-token");

    let refreshed = store.load(&key).await.unwrap().unwrap();
    assert!(
        meerkat_core::tokens_lifecycle_published(&refreshed),
        "post-refresh token write must be wrapped in an AuthMachine lease transition"
    );
    // The marker is stamped against the refreshed credential's expiry.
    let publication = meerkat_core::tokens_lifecycle_publication(&refreshed)
        .expect("refreshed tokens carry a lifecycle publication");
    assert_eq!(
        publication.expires_at,
        meerkat_core::persisted_token_expires_at_epoch_secs(&refreshed),
        "lifecycle marker expiry must match the refreshed token expiry"
    );
    // MCP metadata survives the refresh publish.
    assert_eq!(refreshed.metadata["client"]["client_id"], "client-123");
}

#[tokio::test]
async fn unmarked_non_expiring_stored_token_is_dead_data() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let auth_lease = test_auth_lease();
    let authority = FixtureAuthority::with_fixture_http(
        ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
        recording_browser(Arc::clone(&state)),
        Client::new(),
        auth_lease.clone(),
    );
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();

    authority.interactive_login(&target, None).await.unwrap();
    let key = target.token_key().unwrap();
    let mut stored = store.load(&key).await.unwrap().unwrap();
    // Rebuild only the original MCP metadata fields. This deliberately drops
    // the lifecycle publication, exactly as the old private-struct roundtrip.
    assert_eq!(stored.metadata["server_name"], target.server_name());
    assert_eq!(stored.metadata["server_url"], target.server_url());
    let metadata = serde_json::json!({
        "server_name": stored.metadata["server_name"],
        "server_url": stored.metadata["server_url"],
        "discovery": stored.metadata["discovery"],
        "client": stored.metadata["client"],
    });
    stored.expires_at = None;
    stored.metadata = metadata;
    store.save(&key, &stored).await.unwrap();
    auth_lease
        .release_lease(&target.lease_key().unwrap())
        .unwrap();

    let error = authority
        .stored_bearer_token(&target)
        .await
        .expect_err("unmarked persisted bytes must never be admitted");
    assert!(matches!(error, McpOAuthError::ReauthRequired { .. }));
    assert_eq!(
        state.token_requests.lock().len(),
        1,
        "dead stored bytes must not reach the refresh endpoint"
    );
}

#[tokio::test]
async fn marked_non_expiring_stored_token_restores_and_is_admitted() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let auth_lease = test_auth_lease();
    let authority = FixtureAuthority::with_test_http(
        store.clone(),
        recording_browser(Arc::clone(&state)),
        Client::new(),
        auth_lease.clone(),
    );
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();

    authority.interactive_login(&target, None).await.unwrap();
    republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
        tokens.expires_at = None;
    })
    .await;
    let lease_key = target.lease_key().unwrap();
    let stored = store
        .load(&target.token_key().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(durable_marker::marker_payload_valid_for_tokens(
        &stored,
        &target.token_key().unwrap()
    ));
    let restored_auth_lease = test_auth_lease();
    let restored_authority = FixtureAuthority::with_test_http(
        store.clone(),
        recording_browser(Arc::clone(&state)),
        Client::new(),
        restored_auth_lease.clone(),
    );

    let token = restored_authority
        .stored_bearer_token(&target)
        .await
        .unwrap()
        .expect("marked token is present");
    assert_eq!(token, "access-token");
    let snapshot = restored_auth_lease.snapshot(&lease_key);
    assert_eq!(
        snapshot.phase,
        Some(meerkat_core::handles::AuthLeasePhase::Valid)
    );
    assert!(snapshot.credential_present);
    assert_eq!(state.token_requests.lock().len(), 1);
}

#[tokio::test]
async fn concurrent_expired_reads_share_one_refresh_transaction() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = FixtureAuthority::with_test_http(
        store.clone(),
        recording_browser(Arc::clone(&state)),
        Client::new(),
        test_auth_lease(),
    );
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();
    authority.interactive_login(&target, None).await.unwrap();
    republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
        tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
    })
    .await;

    let mut reads = Vec::new();
    for _ in 0..8 {
        let authority = authority.clone();
        let target = target.clone();
        reads.push(tokio::spawn(async move {
            authority.stored_bearer_token(&target).await
        }));
    }
    for read in reads {
        assert_eq!(
            read.await.unwrap().unwrap().as_deref(),
            Some("access-token")
        );
    }
    let requests = state.token_requests.lock();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request["grant_type"] == "refresh_token")
            .count(),
        1,
        "same-key readers must share one OAuth refresh"
    );
}

#[cfg(feature = "file-lock")]
#[tokio::test]
async fn interactive_login_commit_waits_for_cross_process_refresh_transaction() {
    use meerkat_auth_core::auth_store::{FileLockCoordinator, FileTokenStore};

    let (base, state) = spawn_oauth_fixture().await;
    let temp = tempfile::tempdir().unwrap();
    let lock_dir = temp.path().join("locks");
    let store: Arc<dyn TokenStore> = Arc::new(FileTokenStore::new(temp.path().join("credentials")));
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();
    let seed = FixtureAuthority::with_fixture_http(
        ProviderAuthPersistence::new(
            Arc::clone(&store),
            Arc::new(FileLockCoordinator::new(lock_dir.clone())),
        ),
        recording_browser(Arc::clone(&state)),
        Client::new(),
        test_auth_lease(),
    );
    seed.interactive_login(&target, None).await.unwrap();
    republish_stored_tokens(&seed, store.as_ref(), &target, |tokens| {
        tokens.primary_secret = Some("stale-access-token".to_string());
        tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
    })
    .await;

    state.pause_refresh.store(true, Ordering::SeqCst);
    let refresh = FixtureAuthority::with_fixture_http(
        ProviderAuthPersistence::new(
            Arc::clone(&store),
            Arc::new(FileLockCoordinator::new(lock_dir.clone())),
        ),
        recording_browser(Arc::clone(&state)),
        Client::new(),
        test_auth_lease(),
    );
    let refresh_target = target.clone();
    let refresh_task =
        tokio::spawn(async move { refresh.stored_bearer_token(&refresh_target).await });
    state.refresh_started.notified().await;

    let login_lease = test_auth_lease();
    let login = FixtureAuthority::with_fixture_http(
        ProviderAuthPersistence::new(
            Arc::clone(&store),
            Arc::new(FileLockCoordinator::new(lock_dir)),
        ),
        recording_browser(Arc::clone(&state)),
        Client::new(),
        login_lease.clone(),
    );
    let login_target = target.clone();
    let mut login_task =
        tokio::spawn(async move { login.interactive_login(&login_target, None).await });
    wait_for_token_grant_count(&state, "authorization_code", 2).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut login_task)
            .await
            .is_err(),
        "login commit must wait behind the refresh mutation lock"
    );

    state.refresh_release.notify_one();
    assert_eq!(
        refresh_task.await.unwrap().unwrap().as_deref(),
        Some("access-token")
    );
    assert_eq!(login_task.await.unwrap().unwrap(), "access-token");

    let key = target.token_key().unwrap();
    let committed = store.load(&key).await.unwrap().unwrap();
    assert_eq!(committed.primary_secret.as_deref(), Some("access-token"));
    assert_eq!(
        durable_marker::marker_relation_for_tokens_and_snapshot(
            &committed,
            &login_lease.snapshot(&target.lease_key().unwrap()),
            &key,
        ),
        durable_marker::AuthLeaseDurableMarkerRelation::Matches
    );
}

#[cfg(feature = "file-lock")]
#[tokio::test]
async fn failed_login_after_paused_refresh_restores_refresh_winner() {
    use meerkat_auth_core::auth_store::{FileLockCoordinator, FileTokenStore};

    let (base, state) = spawn_oauth_fixture().await;
    let temp = tempfile::tempdir().unwrap();
    let lock_dir = temp.path().join("locks");
    let durable_store: Arc<dyn TokenStore> =
        Arc::new(FileTokenStore::new(temp.path().join("credentials")));
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();
    let seed = FixtureAuthority::with_fixture_http(
        ProviderAuthPersistence::new(
            Arc::clone(&durable_store),
            Arc::new(FileLockCoordinator::new(lock_dir.clone())),
        ),
        recording_browser(Arc::clone(&state)),
        Client::new(),
        test_auth_lease(),
    );
    seed.interactive_login(&target, None).await.unwrap();
    republish_stored_tokens(&seed, durable_store.as_ref(), &target, |tokens| {
        tokens.primary_secret = Some("stale-access-token".to_string());
        tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
    })
    .await;

    state.pause_refresh.store(true, Ordering::SeqCst);
    let refresh = FixtureAuthority::with_fixture_http(
        ProviderAuthPersistence::new(
            Arc::clone(&durable_store),
            Arc::new(FileLockCoordinator::new(lock_dir.clone())),
        ),
        recording_browser(Arc::clone(&state)),
        Client::new(),
        test_auth_lease(),
    );
    let refresh_target = target.clone();
    let refresh_task =
        tokio::spawn(async move { refresh.stored_bearer_token(&refresh_target).await });
    state.refresh_started.notified().await;

    let failing_store = Arc::new(FailNextSaveDynStore {
        inner: Arc::clone(&durable_store),
        fail_next_save: AtomicBool::new(true),
    });
    let login_lease = test_auth_lease();
    let login = FixtureAuthority::with_fixture_http(
        ProviderAuthPersistence::new(
            failing_store.clone(),
            Arc::new(FileLockCoordinator::new(lock_dir)),
        ),
        recording_browser(Arc::clone(&state)),
        Client::new(),
        login_lease.clone(),
    );
    let login_target = target.clone();
    let mut login_task =
        tokio::spawn(async move { login.interactive_login(&login_target, None).await });
    wait_for_token_grant_count(&state, "authorization_code", 2).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut login_task)
            .await
            .is_err(),
        "failed login transaction must still wait behind refresh"
    );

    state.refresh_release.notify_one();
    assert_eq!(
        refresh_task.await.unwrap().unwrap().as_deref(),
        Some("access-token")
    );
    let error = login_task
        .await
        .unwrap()
        .expect_err("injected login commit must fail");
    assert_compensated_save_failure(&error, &target);
    assert!(
        !failing_store.fail_next_save.load(Ordering::SeqCst),
        "the actual injected store failure must have been consumed"
    );

    let key = target.token_key().unwrap();
    let restored = durable_store.load(&key).await.unwrap().unwrap();
    assert_eq!(
        restored.primary_secret.as_deref(),
        Some("access-token"),
        "login rollback must not restore the stale pre-refresh credential"
    );
    assert_eq!(
        durable_marker::marker_relation_for_tokens_and_snapshot(
            &restored,
            &login_lease.snapshot(&target.lease_key().unwrap()),
            &key,
        ),
        durable_marker::AuthLeaseDurableMarkerRelation::Matches
    );
}

#[tokio::test]
async fn refresh_save_failure_restores_previous_token_and_machine_snapshot() {
    let (base, state) = spawn_oauth_fixture().await;
    let inner = Arc::new(EphemeralTokenStore::new());
    let store = Arc::new(FailNextSaveStore {
        inner: Arc::clone(&inner),
        fail_next_save: AtomicBool::new(false),
    });
    let auth_lease = test_auth_lease();
    let authority = FixtureAuthority::with_fixture_http(
        ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
        recording_browser(Arc::clone(&state)),
        Client::new(),
        auth_lease.clone(),
    );
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();
    authority.interactive_login(&target, None).await.unwrap();
    let previous = republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
        tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
    })
    .await;
    store.fail_next_save.store(true, Ordering::SeqCst);

    let error = authority
        .stored_bearer_token(&target)
        .await
        .expect_err("injected refresh commit failure must surface");
    assert!(matches!(error, McpOAuthError::RefreshFailed { .. }));
    let key = target.token_key().unwrap();
    let restored = inner.load(&key).await.unwrap().unwrap();
    assert_eq!(restored.primary_secret, previous.primary_secret);
    assert_eq!(restored.expires_at, previous.expires_at);
    let snapshot = auth_lease.snapshot(&target.lease_key().unwrap());
    assert_ne!(
        snapshot.phase,
        Some(meerkat_core::handles::AuthLeasePhase::Refreshing),
        "failed persistence must not strand AuthMachine in Refreshing"
    );
    assert_eq!(
        durable_marker::marker_relation_for_tokens_and_snapshot(&restored, &snapshot, &key),
        durable_marker::AuthLeaseDurableMarkerRelation::Matches,
        "compensation must restore token and machine as one lifecycle publication"
    );
}

#[tokio::test]
async fn interactive_relogin_save_failure_restores_previous_token_and_machine_snapshot() {
    let (base, state) = spawn_oauth_fixture().await;
    let inner = Arc::new(EphemeralTokenStore::new());
    let store = Arc::new(FailNextSaveStore {
        inner: Arc::clone(&inner),
        fail_next_save: AtomicBool::new(false),
    });
    let seed_authority = FixtureAuthority::with_fixture_http(
        ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
        recording_browser(Arc::clone(&state)),
        Client::new(),
        test_auth_lease(),
    );
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();
    seed_authority
        .interactive_login(&target, None)
        .await
        .unwrap();
    let previous = republish_stored_tokens(&seed_authority, store.as_ref(), &target, |tokens| {
        tokens.primary_secret = Some("previous-access-token".to_string());
        tokens.refresh_token = Some("previous-refresh-token".to_string());
    })
    .await;
    let auth_lease = test_auth_lease();
    let authority = FixtureAuthority::with_fixture_http(
        ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
        recording_browser(Arc::clone(&state)),
        Client::new(),
        auth_lease.clone(),
    );
    store.fail_next_save.store(true, Ordering::SeqCst);

    let error = authority
        .interactive_login(&target, None)
        .await
        .expect_err("injected relogin commit failure must surface");
    assert_compensated_save_failure(&error, &target);
    assert!(
        !store.fail_next_save.load(Ordering::SeqCst),
        "the actual injected store failure must have been consumed"
    );

    let key = target.token_key().unwrap();
    let restored = inner.load(&key).await.unwrap().unwrap();
    assert_eq!(restored.primary_secret, previous.primary_secret);
    assert_eq!(restored.refresh_token, previous.refresh_token);
    assert_eq!(restored.expires_at, previous.expires_at);
    assert_eq!(restored.metadata["client"], previous.metadata["client"]);
    let snapshot = auth_lease.snapshot(&target.lease_key().unwrap());
    assert_eq!(
        durable_marker::marker_relation_for_tokens_and_snapshot(&restored, &snapshot, &key),
        durable_marker::AuthLeaseDurableMarkerRelation::Matches,
        "failed relogin must restore one admitted token/lifecycle publication"
    );
    assert_eq!(
        authority
            .stored_bearer_token(&target)
            .await
            .unwrap()
            .as_deref(),
        Some("previous-access-token"),
        "the previously usable credential must remain usable after rollback"
    );
}

// The shared terminal browser transaction reports the compensated operation
// through AuthLifecycle. Do not accept an arbitrary lifecycle failure: require
// this exact injected store cause and completed compensation, then independently
// inspect the restored durable credential and native lifecycle below.
fn assert_compensated_save_failure(error: &McpOAuthError, target: &McpServerIdentity) {
    let McpOAuthError::AuthLifecycle {
        server_name,
        reason,
    } = error
    else {
        panic!("expected the coordinated login failure classification");
    };
    assert_eq!(server_name, target.server_name());
    assert_eq!(
        reason,
        "TokenStore save failed after OAuth consume: io error: injected save failure; acquired lease rolled back"
    );
}

fn recording_browser(state: Arc<TestState>) -> Arc<dyn TestBrowser> {
    Arc::new(RecordingBrowser {
        state,
        http: Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .unwrap(),
    })
}

#[cfg(feature = "file-lock")]
async fn wait_for_token_grant_count(state: &TestState, grant_type: &str, expected: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let observed = state
                .token_requests
                .lock()
                .iter()
                .filter(|request| request["grant_type"] == grant_type)
                .count();
            if observed >= expected {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("expected OAuth token request was not observed");
}

async fn republish_stored_tokens(
    authority: &FixtureAuthority,
    store: &dyn TokenStore,
    target: &McpServerIdentity,
    mutate: impl FnOnce(&mut PersistedTokens),
) -> PersistedTokens {
    let key = target.token_key().unwrap();
    let mut tokens = store.load(&key).await.unwrap().unwrap();
    mutate(&mut tokens);
    let published = authority
        .publish_login_tokens_via_lease(target, &key, &tokens)
        .unwrap();
    store.save(&key, &published).await.unwrap();
    published
}

async fn assert_login_fails_closed(
    state: Arc<TestState>,
    store: Arc<EphemeralTokenStore>,
    target: &McpServerIdentity,
    result: Result<String, McpOAuthError>,
    expected_reason: &str,
) {
    let err = result.expect_err("login should fail");
    assert!(
        err.to_string().contains(expected_reason),
        "expected {expected_reason:?} in error, got {err}"
    );
    assert!(
        store
            .load(&target.token_key().unwrap())
            .await
            .unwrap()
            .is_none(),
        "failed login must not persist MCP OAuth tokens"
    );
    assert!(
        state.token_requests.lock().len() <= 1,
        "failed login should not retry token exchange unexpectedly"
    );
}

#[tokio::test]
async fn interactive_login_discovers_registers_exchanges_and_stores_token() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let browser = recording_browser(Arc::clone(&state));
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), test_auth_lease());
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();
    let token = authority
        .interactive_login(
            &target,
            Some(r#"Bearer resource_metadata="/.well-known/oauth-protected-resource/mcp""#),
        )
        .await
        .expect("login succeeds");
    assert_eq!(token, "access-token");
    let stored = store
        .load(&target.token_key().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.auth_mode, PersistedAuthMode::McpOauth);
    assert_eq!(stored.primary_secret.as_deref(), Some("access-token"));
    assert_eq!(stored.account_id.as_deref(), Some("fixture-account-42"));
    assert_eq!(stored.scopes, vec!["mcp.read".to_owned()]);
    assert!(stored.metadata["client"]["client_id"] == "client-123");
    assert!(
        stored.metadata["client"].get("client_secret").is_none(),
        "MCP DCR requests a public PKCE client and must not persist a DCR secret"
    );
    assert_eq!(
        stored.metadata["client"]["token_endpoint_auth_method"],
        "none"
    );
    assert_eq!(
        stored.metadata["discovery"]["resource"],
        format!("{base}/mcp")
    );

    let opened_url = state.opened_url.lock().clone().unwrap();
    assert!(
        opened_url.contains("resource="),
        "authorize URL should carry an OAuth resource indicator"
    );
    assert!(
        opened_url.contains("scope=mcp.read"),
        "MCP OAuth requests the explicit host-declared scope"
    );
    let registration = state.registration_requests.lock();
    assert_eq!(
        registration[0]["token_endpoint_auth_method"], "none",
        "DCR should explicitly request a public token endpoint auth method"
    );
    let token_requests = state.token_requests.lock();
    assert_eq!(
        token_requests[0]["resource"],
        format!("{base}/mcp"),
        "authorization-code exchange should carry the MCP resource indicator"
    );
    assert!(
        token_requests[0].get("client_secret").is_none(),
        "public-client token exchange must not send a DCR secret"
    );
    assert!(
        token_requests[0].get("scope").is_none(),
        "authorization-code exchange should not request every advertised supported scope"
    );
}

#[tokio::test]
async fn interactive_login_uses_well_known_fallback_without_challenge_header() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let browser = recording_browser(Arc::clone(&state));
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), test_auth_lease());
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();

    let token = authority
        .interactive_login(&target, None)
        .await
        .expect("well-known discovery fallback should login");

    assert_eq!(token, "access-token");
    let stored = store
        .load(&target.token_key().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.metadata["discovery"]["resource_metadata_url"],
        format!("{base}/.well-known/oauth-protected-resource/mcp")
    );
}

#[tokio::test]
async fn stored_token_refresh_uses_resource_and_invalid_grant_requires_reauth() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let browser = recording_browser(Arc::clone(&state));
    let auth_lease = test_auth_lease();
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), auth_lease.clone());
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();

    authority
        .interactive_login(&target, None)
        .await
        .expect("initial login succeeds");
    republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
        tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
    })
    .await;
    *state.token_fails.lock() = true;

    let error = authority
        .stored_bearer_token(&target)
        .await
        .expect_err("invalid refresh grant should require reauth");

    assert!(matches!(error, McpOAuthError::ReauthRequired { .. }));
    assert_eq!(
        auth_lease.snapshot(&target.lease_key().unwrap()).phase,
        Some(meerkat_core::handles::AuthLeasePhase::ReauthRequired),
        "invalid_grant must close Refreshing through AuthRefreshFailed"
    );
    assert!(
        store
            .load(&target.token_key().unwrap())
            .await
            .unwrap()
            .is_none(),
        "permanently rejected credentials must remain dead across processes"
    );
    let requests_before_restart = {
        let token_requests = state.token_requests.lock();
        assert_eq!(
            token_requests.last().unwrap()["grant_type"],
            "refresh_token"
        );
        assert_eq!(
            token_requests.last().unwrap()["resource"],
            format!("{base}/mcp"),
            "refresh exchange should carry the MCP resource indicator"
        );
        token_requests.len()
    };
    let restarted = FixtureAuthority::with_test_http(
        store.clone(),
        recording_browser(Arc::clone(&state)),
        Client::new(),
        test_auth_lease(),
    );
    assert!(
        restarted
            .stored_bearer_token(&target)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        state.token_requests.lock().len(),
        requests_before_restart,
        "a restarted authority must not retry a durably cleared permanent credential"
    );
}

#[tokio::test]
async fn permanent_refresh_clear_failure_is_typed_and_closes_machine() {
    let (base, state) = spawn_oauth_fixture().await;
    let inner = Arc::new(EphemeralTokenStore::new());
    let store = Arc::new(FailClearStore {
        inner: Arc::clone(&inner),
    });
    let auth_lease = test_auth_lease();
    let authority = FixtureAuthority::with_fixture_http(
        ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
        recording_browser(Arc::clone(&state)),
        Client::new(),
        auth_lease.clone(),
    );
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();
    authority.interactive_login(&target, None).await.unwrap();
    republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
        tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
    })
    .await;
    *state.token_fails.lock() = true;

    let error = authority
        .stored_bearer_token(&target)
        .await
        .expect_err("durable clear failure must surface");
    assert!(matches!(&error, McpOAuthError::TokenStore(_)));
    assert!(error.to_string().contains("injected clear failure"));
    assert_eq!(
        auth_lease.snapshot(&target.lease_key().unwrap()).phase,
        Some(meerkat_core::handles::AuthLeasePhase::ReauthRequired),
        "even a failed durable clear must close the begun refresh"
    );
    assert!(
        inner
            .load(&target.token_key().unwrap())
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn durable_clear_crash_point_reconciles_stranded_refreshing_lifecycle() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let auth_lease = test_auth_lease();
    let authority = FixtureAuthority::with_test_http(
        store.clone(),
        recording_browser(state),
        Client::new(),
        auth_lease.clone(),
    );
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();
    authority.interactive_login(&target, None).await.unwrap();
    republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
        tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
    })
    .await;
    let lease_key = target.lease_key().unwrap();
    auth_lease
        .observe_credential_freshness(
            &lease_key,
            u64::try_from(Utc::now().timestamp())
                .expect("fixture timestamp is after the Unix epoch"),
            AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
        )
        .unwrap();
    auth_lease.begin_refresh(&lease_key).unwrap();
    store.clear(&target.token_key().unwrap()).await.unwrap();
    assert_eq!(
        auth_lease.snapshot(&lease_key).phase,
        Some(meerkat_core::handles::AuthLeasePhase::Refreshing)
    );

    assert!(
        authority
            .stored_bearer_token(&target)
            .await
            .unwrap()
            .is_none()
    );
    let reconciled = auth_lease.snapshot(&lease_key);
    assert!(
        reconciled.phase.is_none(),
        "released lifecycle projects no active phase"
    );
    assert!(!reconciled.credential_present);
}

#[tokio::test]
async fn transient_refresh_failure_closes_machine_back_to_expiring() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let auth_lease = test_auth_lease();
    let authority = FixtureAuthority::with_test_http(
        store.clone(),
        recording_browser(Arc::clone(&state)),
        Client::new(),
        auth_lease.clone(),
    );
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();
    authority.interactive_login(&target, None).await.unwrap();
    republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
        tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
    })
    .await;
    *state.token_transiently_fails.lock() = true;

    let error = authority
        .stored_bearer_token(&target)
        .await
        .expect_err("transient refresh failure must surface");
    assert!(matches!(error, McpOAuthError::RefreshFailed { .. }));
    assert_eq!(
        auth_lease.snapshot(&target.lease_key().unwrap()).phase,
        Some(meerkat_core::handles::AuthLeasePhase::Expiring),
        "transient boundary evidence must close Refreshing through AuthRefreshFailed"
    );
}

/// A refresh refused with a body that echoes secrets (the grant itself and a
/// canary, including inside a malformed `error` code) yields a typed
/// `RefreshFailed` whose text and Debug never carry that body. That text is
/// what becomes the agent-visible MCP connection failure and its notice.
#[tokio::test]
async fn refused_refresh_never_renders_the_token_endpoint_body() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = FixtureAuthority::with_test_http(
        store.clone(),
        recording_browser(Arc::clone(&state)),
        Client::new(),
        test_auth_lease(),
    );
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();
    authority.interactive_login(&target, None).await.unwrap();
    republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
        tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
    })
    .await;
    *state.token_refresh_echoes_secrets.lock() = true;

    let error = authority
        .stored_bearer_token(&target)
        .await
        .expect_err("a refused refresh must surface");
    // Positive control: the endpoint really answered with the secrets.
    let responses = state.token_error_responses.lock().clone();
    let echoed = responses.last().expect("the refresh reached the endpoint");
    assert_eq!(
        echoed["body"]["error_description"],
        REFRESH_ERROR_BODY_CANARY
    );
    assert_eq!(echoed["body"]["echoed_grant"], "refresh-token");

    assert!(
        matches!(error, McpOAuthError::RefreshFailed { .. }),
        "{error}"
    );
    let rendered = format!("{error} {error:?}");
    for secret in [REFRESH_ERROR_BODY_CANARY, "refresh-token", "echoed_grant"] {
        assert!(
            !rendered.contains(secret),
            "refresh failure text carries `{secret}`: {rendered}"
        );
    }
    assert!(rendered.contains("status=500"), "{rendered}");
}

#[tokio::test]
async fn stored_token_metadata_must_match_requested_target() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let browser = recording_browser(Arc::clone(&state));
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), test_auth_lease());
    let original = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();
    let other = McpServerIdentity::from_server_config("glean", format!("{base}/other-mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();

    authority
        .interactive_login(&original, None)
        .await
        .expect("initial login succeeds");
    let stored = store
        .load(&original.token_key().unwrap())
        .await
        .unwrap()
        .unwrap();
    store
        .save(&other.token_key().unwrap(), &stored)
        .await
        .unwrap();

    let error = authority
        .stored_bearer_token(&other)
        .await
        .expect_err("mismatched stored metadata should not be trusted");

    assert!(matches!(error, McpOAuthError::ReauthRequired { .. }));
}

#[tokio::test]
async fn interactive_login_missing_dcr_fails_closed() {
    let (base, state) = spawn_oauth_fixture().await;
    *state.include_registration_endpoint.lock() = false;
    let store = Arc::new(EphemeralTokenStore::new());
    let browser = recording_browser(Arc::clone(&state));
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), test_auth_lease());
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();

    let result = authority.interactive_login(&target, None).await;

    assert_login_fails_closed(
        Arc::clone(&state),
        store,
        &target,
        result,
        "registration_endpoint",
    )
    .await;
    assert!(
        state.opened_url.lock().is_none(),
        "browser should not open when discovery cannot find DCR"
    );
}

#[tokio::test]
async fn interactive_login_missing_dcr_auth_method_fails_closed() {
    let (base, state) = spawn_oauth_fixture().await;
    *state.omit_token_endpoint_auth_method.lock() = true;
    let store = Arc::new(EphemeralTokenStore::new());
    let browser = recording_browser(Arc::clone(&state));
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), test_auth_lease());
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();

    let result = authority.interactive_login(&target, None).await;

    assert_login_fails_closed(
        Arc::clone(&state),
        store,
        &target,
        result,
        "token_endpoint_auth_method missing",
    )
    .await;
    assert!(
        state.opened_url.lock().is_none(),
        "browser should not open when DCR does not explicitly confirm public-client auth"
    );
}

#[tokio::test]
async fn interactive_login_rejects_mismatched_protected_resource_metadata() {
    let (base, state) = spawn_oauth_fixture().await;
    *state.resource_override.lock() = Some(format!("{base}/other-mcp"));
    let store = Arc::new(EphemeralTokenStore::new());
    let browser = recording_browser(Arc::clone(&state));
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), test_auth_lease());
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();

    let result = authority.interactive_login(&target, None).await;

    assert_login_fails_closed(
        Arc::clone(&state),
        store,
        &target,
        result,
        "does not match MCP server",
    )
    .await;
    assert!(
        state.opened_url.lock().is_none(),
        "browser should not open when protected resource metadata is mismatched"
    );
}

#[tokio::test]
async fn interactive_login_rejects_relative_protected_resource_metadata() {
    let (base, state) = spawn_oauth_fixture().await;
    *state.resource_override.lock() = Some("/mcp".to_string());
    let store = Arc::new(EphemeralTokenStore::new());
    let browser = recording_browser(Arc::clone(&state));
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), test_auth_lease());
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();

    let result = authority.interactive_login(&target, None).await;

    assert_login_fails_closed(
        Arc::clone(&state),
        store,
        &target,
        result,
        "does not match MCP server",
    )
    .await;
}

#[tokio::test]
async fn interactive_login_rejects_remote_http_protected_resource() {
    let (_base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let browser = recording_browser(Arc::clone(&state));
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), test_auth_lease());
    let target = McpServerIdentity::from_server_config("glean", "http://mcp.example.test/mcp")
        .with_expected_account("fixture-account-42")
        .unwrap();

    let result = authority.interactive_login(&target, None).await;

    assert_login_fails_closed(
        Arc::clone(&state),
        store,
        &target,
        result,
        "MCP protected resource must use https",
    )
    .await;
    assert!(
        state.opened_url.lock().is_none(),
        "browser should not open for remote HTTP protected resources"
    );
}

#[tokio::test]
async fn interactive_login_rejects_mismatched_authorization_server_issuer() {
    let (base, state) = spawn_oauth_fixture().await;
    *state.issuer_override.lock() = Some("https://issuer.example.invalid".to_string());
    let store = Arc::new(EphemeralTokenStore::new());
    let browser = recording_browser(Arc::clone(&state));
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), test_auth_lease());
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();

    let result = authority.interactive_login(&target, None).await;

    assert_login_fails_closed(
        Arc::clone(&state),
        store,
        &target,
        result,
        "does not match discovered issuer",
    )
    .await;
    assert!(
        state.opened_url.lock().is_none(),
        "browser should not open when authorization-server metadata is mismatched"
    );
}

#[tokio::test]
async fn interactive_login_rejects_issuer_without_exact_codepoint_match() {
    let (base, state) = spawn_oauth_fixture().await;
    *state.issuer_override.lock() = Some(format!("{base}/"));
    let store = Arc::new(EphemeralTokenStore::new());
    let browser = recording_browser(Arc::clone(&state));
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), test_auth_lease());
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();

    let result = authority.interactive_login(&target, None).await;

    assert_login_fails_closed(
        Arc::clone(&state),
        store,
        &target,
        result,
        "does not match discovered issuer",
    )
    .await;
}

#[tokio::test]
async fn interactive_login_token_exchange_failure_fails_closed() {
    let (base, state) = spawn_oauth_fixture().await;
    *state.token_fails.lock() = true;
    let store = Arc::new(EphemeralTokenStore::new());
    let browser = recording_browser(Arc::clone(&state));
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), test_auth_lease());
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();

    let result = authority.interactive_login(&target, None).await;

    let error = result
        .as_ref()
        .expect_err("the real token endpoint rejected the code");
    assert!(matches!(
        error,
        McpOAuthError::TokenExchangeFailed { server_name, reason }
            if server_name == target.server_name()
                && reason == "authorization-code exchange failed"
    ));
    let display = error.to_string();
    let debug = format!("{error:?}");
    for private_detail in [TOKEN_EXCHANGE_ERROR_CANARY, "invalid_grant", "fixture-code"] {
        assert!(!display.contains(private_detail));
        assert!(!debug.contains(private_detail));
    }
    // These are observations from the actual /token handler, not a mock error
    // supplied to the caller. Keep endpoint failure distinct from any earlier
    // discovery, account-verification, callback or persistence refusal.
    {
        let requests = state.token_requests.lock();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["grant_type"], "authorization_code");
        assert_eq!(requests[0]["code"], "fixture-code");
        let responses = state.token_error_responses.lock();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["status"], StatusCode::BAD_REQUEST.as_u16());
        assert_eq!(responses[0]["body"]["error"], "invalid_grant");
        assert_eq!(
            responses[0]["body"]["error_description"],
            TOKEN_EXCHANGE_ERROR_CANARY
        );
    }
    assert_login_fails_closed(
        Arc::clone(&state),
        store,
        &target,
        result,
        "authorization-code exchange failed",
    )
    .await;
}

#[tokio::test]
async fn interactive_login_state_mismatch_fails_closed() {
    let (base, state) = spawn_oauth_fixture().await;
    *state.authorize_outcome.lock() = AuthorizeOutcome::StateMismatch;
    let store = Arc::new(EphemeralTokenStore::new());
    let browser = recording_browser(Arc::clone(&state));
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), test_auth_lease());
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();

    let result = authority.interactive_login(&target, None).await;

    assert_login_fails_closed(Arc::clone(&state), store, &target, result, "state mismatch").await;
}

#[tokio::test]
async fn interactive_login_denied_auth_fails_closed() {
    let (base, state) = spawn_oauth_fixture().await;
    *state.authorize_outcome.lock() = AuthorizeOutcome::Denied;
    let store = Arc::new(EphemeralTokenStore::new());
    let browser = recording_browser(Arc::clone(&state));
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), test_auth_lease());
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();

    let result = authority.interactive_login(&target, None).await;

    assert_login_fails_closed(Arc::clone(&state), store, &target, result, "user denied").await;
}

#[tokio::test]
async fn interactive_login_timeout_fails_closed() {
    let (base, state) = spawn_oauth_fixture().await;
    *state.authorize_outcome.lock() = AuthorizeOutcome::NoCallback;
    let store = Arc::new(EphemeralTokenStore::new());
    let browser: Arc<dyn TestBrowser> = Arc::new(NoCallbackBrowser {
        state: Arc::clone(&state),
    });
    assert_eq!(MCP_INTERACTIVE_LOGIN_TIMEOUT, Duration::from_secs(300));
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), test_auth_lease());
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();

    let result = authority.interactive_login(&target, None).await;
    tokio::time::resume();

    assert_login_fails_closed(Arc::clone(&state), store, &target, result, "timeout").await;
    assert!(
        state.opened_url.lock().is_some(),
        "timeout test should still reach the browser-open boundary"
    );
}
struct NeverOpenBrowser;
#[async_trait]
impl TestBrowser for NeverOpenBrowser {
    async fn open(&self, _url: &str) -> Result<(), McpOAuthError> {
        panic!("missing strategy must fail before browser work")
    }
}

#[tokio::test]
async fn absent_account_strategy_refuses_before_discovery_or_browser() {
    let store = Arc::new(EphemeralTokenStore::new());
    let owner = test_auth_lease();
    let authority = McpOAuthAuthority::new(
        ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
        owner.generated,
    );
    let target = McpServerIdentity::from_server_config("untrusted-label", "not-even-a-url")
        .with_expected_account("fixture-account-42")
        .unwrap();
    assert!(matches!(
        host_login(&authority, &NeverOpenBrowser, &target, None).await,
        Err(McpOAuthError::Verification(
            ConnectorOAuthRefusal::VerificationUnavailable
        ))
    ));
    assert!(store.list().await.unwrap().is_empty());
}

struct MismatchedAccountStrategy {
    missing_scopes: bool,
}
#[async_trait]
impl McpOAuthAccountStrategy for MismatchedAccountStrategy {
    fn descriptor(
        &self,
        target: &McpServerIdentity,
        context: &McpOAuthCeremonyContext<'_>,
    ) -> Result<ConnectorOAuthDescriptor, ConnectorOAuthRefusal> {
        FixtureAccountStrategy.descriptor(target, context)
    }
    async fn observe_account(
        &self,
        descriptor: &ConnectorOAuthDescriptor,
        tokens: &OAuthTokenResult,
    ) -> Result<ConnectorAccountObservation, ConnectorOAuthRefusal> {
        let mut actual = FixtureAccountStrategy
            .observe_account(descriptor, tokens)
            .await?;
        if self.missing_scopes {
            actual.granted_scopes.clear();
        } else {
            actual.account = "different-actual-account".into();
        }
        Ok(actual)
    }
}

#[tokio::test]
async fn actual_mcp_exchange_refuses_crossed_account_and_scope_downgrade_and_retires_owner() {
    for missing_scopes in [false, true] {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let mut authority = FixtureAuthority::with_test_http(
            store.clone(),
            recording_browser(state.clone()),
            Client::new(),
            test_auth_lease(),
        );
        let owner = authority.flows.clone();
        authority.native = authority
            .native
            .with_interactive_strategy(
                owner.clone(),
                Arc::new(MismatchedAccountStrategy { missing_scopes }),
            )
            .unwrap();
        let target =
            McpServerIdentity::from_server_config("label-is-not-an-account", format!("{base}/mcp"))
                .with_expected_account("fixture-account-42")
                .unwrap();
        let expected = if missing_scopes {
            ConnectorOAuthRefusal::MissingScopes
        } else {
            ConnectorOAuthRefusal::AccountMismatch
        };
        assert!(
            matches!(authority.interactive_login(&target, None).await, Err(McpOAuthError::Verification(error)) if error == expected)
        );
        assert!(
            store
                .load(&target.token_key().unwrap())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            state.token_requests.lock().len(),
            1,
            "the native HTTP exchange must actually be reached"
        );
        let opened = state.opened_url.lock().clone().unwrap();
        let url = reqwest::Url::parse(&opened).unwrap();
        let flow_state = url
            .query_pairs()
            .find(|(name, _)| name == "state")
            .unwrap()
            .1
            .into_owned();
        let redirect = state.redirect_uri.lock().clone().unwrap();
        let descriptor = FixtureAccountStrategy
            .descriptor(
                &target,
                &McpOAuthCeremonyContext {
                    issuer: &base,
                    client: "client-123",
                    resource: target.server_url(),
                    redirect_uri: &redirect,
                },
            )
            .unwrap();
        assert!(
            owner
                .verify(
                    &flow_state,
                    &target.auth_binding_ref().unwrap().into(),
                    descriptor,
                    &redirect
                )
                .is_err(),
            "rejected completion must retire its canonical attempt"
        );
    }
}

#[tokio::test]
async fn oauth_expiry_cancelling_actual_mcp_login_erases_persisted_attempt_without_tokens() {
    use meerkat_auth_core::oauth_flow::OAuthFlowRegistrySnapshot;
    use meerkat_runtime::store::{RuntimeStore, memory::InMemoryRuntimeStore};

    // Exercise host cancellation (`login_cancel` from the host's drop guard)
    // while the callback wait is still pending. This browser reports the URL
    // only to the driver.
    struct PendingBrowser(tokio::sync::mpsc::UnboundedSender<String>);
    #[async_trait]
    impl TestBrowser for PendingBrowser {
        async fn open(&self, url: &str) -> Result<(), McpOAuthError> {
            self.0.send(url.to_owned()).unwrap();
            Ok(())
        }
    }

    let (base, state) = spawn_oauth_fixture().await;
    let tokens = Arc::new(EphemeralTokenStore::new());
    // SQLite erasure/reopen is covered in the runtime owner tests. Here the
    // injected store makes persisted-row observation independent of the owner.
    let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let auth = test_auth_lease();
    let flows = Arc::new(
        RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
            Duration::from_millis(500),
            auth.lifecycle,
            &runtime_store,
        ),
    );
    let (opened, mut launches) = tokio::sync::mpsc::unbounded_channel();
    let native = McpOAuthAuthority::with_http(
        ProviderAuthPersistence::new(tokens.clone(), Arc::new(InMemoryCoordinator::new())),
        Client::new(),
        auth.generated,
    )
    .with_interactive_strategy(flows, Arc::new(FixtureAccountStrategy))
    .unwrap();
    let browser = PendingBrowser(opened);
    let target = McpServerIdentity::from_server_config("expiry-cancel", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();
    let login_target = target.clone();
    let login =
        tokio::spawn(async move { host_login(&native, &browser, &login_target, None).await });
    let _opened = tokio::time::timeout(Duration::from_secs(5), launches.recv())
        .await
        .unwrap()
        .unwrap();
    let pending: OAuthFlowRegistrySnapshot = serde_json::from_slice(
        &runtime_store
            .load_auth_oauth_flow_snapshot()
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(pending.browser.len(), 1);
    assert!(pending.device.is_empty());
    tokio::time::sleep(Duration::from_millis(550)).await;
    assert!(
        !login.is_finished(),
        "the real callback wait must still be pending"
    );
    login.abort();
    assert!(login.await.unwrap_err().is_cancelled());

    // Read storage directly before any verify/reopen could perform cleanup.
    let retired: OAuthFlowRegistrySnapshot = serde_json::from_slice(
        &runtime_store
            .load_auth_oauth_flow_snapshot()
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert!(
        retired.browser.is_empty(),
        "cancellation after expiry must erase the persisted private attempt"
    );
    assert!(retired.device.is_empty());
    assert!(
        tokens
            .load(&target.token_key().unwrap())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        state.token_requests.lock().is_empty(),
        "cancelled login must not reach code exchange"
    );
}

#[test]
fn selected_account_partitions_token_and_lifecycle_identity_without_debug_disclosure() {
    let legacy = McpServerIdentity::from_server_config("same-server", "https://example.test/mcp");
    let a = legacy
        .clone()
        .with_expected_account("subject-a-private")
        .unwrap();
    let a_again = legacy
        .clone()
        .with_expected_account("subject-a-private")
        .unwrap();
    let b = legacy
        .clone()
        .with_expected_account("subject-b-private")
        .unwrap();
    assert_eq!(a, a_again);
    assert_eq!(a.token_key().unwrap(), a_again.token_key().unwrap());
    assert_eq!(a.lease_key().unwrap(), a_again.lease_key().unwrap());
    assert_ne!(a.token_key().unwrap(), b.token_key().unwrap());
    assert_ne!(a.lease_key().unwrap(), b.lease_key().unwrap());
    assert_ne!(a.auth_binding_ref().unwrap(), b.auth_binding_ref().unwrap());
    assert_ne!(a.token_key().unwrap(), legacy.token_key().unwrap());
    assert_ne!(a.lease_key().unwrap(), legacy.lease_key().unwrap());
    assert_eq!(legacy.expected_account(), None);
    assert_eq!(a.expected_account(), Some("subject-a-private"));
    let diagnostic = format!("{a:?} {a:#?} {:?}", a.token_key().unwrap());
    assert!(!diagnostic.contains("subject-a-private"));
    assert!(!diagnostic.contains("subject-b-private"));
    assert!(diagnostic.contains("account_selected"));
}

#[test]
fn selected_account_validation_rejects_empty_and_control_values_without_normalizing_subject() {
    for account in ["", "  ", "subject\nother", "subject\0other"] {
        let result = McpServerIdentity::from_server_config("server", "https://example.test/mcp")
            .with_expected_account(account);
        assert!(matches!(
            result,
            Err(McpOAuthError::InvalidAccountSelection)
        ));
    }
    assert!(matches!(
        McpServerIdentity::from_server_config("server", "https://example.test/mcp")
            .with_expected_account("x".repeat(4097)),
        Err(McpOAuthError::InvalidAccountSelection)
    ));
    let padded = McpServerIdentity::from_server_config("server", "https://example.test/mcp")
        .with_expected_account(" subject ")
        .unwrap();
    let plain = McpServerIdentity::from_server_config("server", "https://example.test/mcp")
        .with_expected_account("subject")
        .unwrap();
    assert_eq!(padded.expected_account(), Some(" subject "));
    assert_ne!(padded.token_key().unwrap(), plain.token_key().unwrap());
}

#[tokio::test]
async fn missing_explicit_account_refuses_before_discovery_browser_or_stored_use() {
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = FixtureAuthority::with_test_http(
        store.clone(),
        Arc::new(NeverOpenBrowser),
        Client::new(),
        test_auth_lease(),
    );
    // Deliberately unselected. The unusable URL discriminates local selection
    // refusal from reaching discovery; the browser panics if it is entered.
    let target = McpServerIdentity::from_server_config("fixture-account-42", "not-even-a-url");
    assert!(matches!(
        authority.interactive_login(&target, None).await,
        Err(McpOAuthError::AccountSelectionRequired)
    ));
    assert!(matches!(
        authority.stored_bearer_token(&target).await,
        Err(McpOAuthError::AccountSelectionRequired)
    ));
    assert!(store.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn descriptor_account_must_match_explicit_target_before_browser_or_exchange() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = FixtureAuthority::with_test_http(
        store.clone(),
        Arc::new(NeverOpenBrowser),
        Client::new(),
        test_auth_lease(),
    );
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("different-selected-account")
        .unwrap();
    assert!(matches!(
        authority.interactive_login(&target, None).await,
        Err(McpOAuthError::Verification(
            ConnectorOAuthRefusal::AccountMismatch
        ))
    ));
    assert_eq!(
        state.registration_requests.lock().len(),
        1,
        "the provider-derived descriptor must actually have been reached"
    );
    assert!(state.token_requests.lock().is_empty());
    assert!(state.opened_url.lock().is_none());
    assert!(store.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn selected_account_never_falls_back_to_admissible_legacy_credentials() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = FixtureAuthority::with_test_http(
        store.clone(),
        recording_browser(state.clone()),
        Client::new(),
        test_auth_lease(),
    );
    let selected = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();
    authority.interactive_login(&selected, None).await.unwrap();
    let selected_key = selected.token_key().unwrap();
    let tokens = store.load(&selected_key).await.unwrap().unwrap();
    // Preserve an intentionally unselected old identity, with a valid native
    // lifecycle marker for its own key. This is a positive legacy-use control.
    let legacy = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
    let legacy_key = legacy.token_key().unwrap();
    let legacy_tokens = authority
        .publish_login_tokens_via_lease(&legacy, &legacy_key, &tokens)
        .unwrap();
    store.save(&legacy_key, &legacy_tokens).await.unwrap();
    store.clear(&selected_key).await.unwrap();
    let legacy_only = McpOAuthAuthority::new(
        ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
        authority.auth_lease.clone(),
    );
    assert_eq!(
        legacy_only
            .stored_bearer_token(&legacy)
            .await
            .unwrap()
            .as_deref(),
        Some("access-token")
    );
    assert_eq!(
        authority.stored_bearer_token(&selected).await.unwrap(),
        None
    );
    assert!(matches!(
        authority.stored_bearer_token(&legacy).await,
        Err(McpOAuthError::AccountSelectionRequired)
    ));
    assert_eq!(store.load(&legacy_key).await.unwrap(), Some(legacy_tokens));
    assert_eq!(
        state.token_requests.lock().len(),
        1,
        "missing selected credentials must not refresh or reopen the browser"
    );
}

#[tokio::test]
async fn selected_account_refuses_wrong_or_missing_persisted_subject_without_mutation() {
    for account in [None, Some("different-account")] {
        for expired in [false, true] {
            let (base, state) = spawn_oauth_fixture().await;
            let store = Arc::new(EphemeralTokenStore::new());
            let owner = test_auth_lease();
            let authority = FixtureAuthority::with_test_http(
                store.clone(),
                recording_browser(state.clone()),
                Client::new(),
                owner.clone(),
            );
            let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
                .with_expected_account("fixture-account-42")
                .unwrap();
            authority.interactive_login(&target, None).await.unwrap();
            let rejected = republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
                tokens.account_id = account.map(str::to_owned);
                if expired {
                    tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
                }
            })
            .await;
            let key = target.token_key().unwrap();
            assert!(
                durable_marker::marker_payload_valid_for_tokens(&rejected, &key),
                "the negative must reach subject admission, not invalid-marker refusal"
            );
            let phase = owner.snapshot(&target.lease_key().unwrap()).phase;
            assert!(matches!(
                authority.stored_bearer_token(&target).await,
                Err(McpOAuthError::Verification(
                    ConnectorOAuthRefusal::AccountMismatch
                ))
            ));
            assert_eq!(owner.snapshot(&target.lease_key().unwrap()).phase, phase);
            assert_eq!(store.load(&key).await.unwrap(), Some(rejected));
            assert_eq!(
                state.token_requests.lock().len(),
                1,
                "subject mismatch must not refresh or revoke another account"
            );
        }
    }
}

// An adversarial durable-store change precisely at the second native load.
// The same fixture covers rehydration and reload under the real coordinator.
struct ReplaceOnSecondLoadStore {
    inner: Arc<EphemeralTokenStore>,
    loads: std::sync::atomic::AtomicUsize,
    replacement: Mutex<Option<(TokenKey, PersistedTokens)>>,
}
#[async_trait]
impl TokenStore for ReplaceOnSecondLoadStore {
    async fn load(
        &self,
        key: &TokenKey,
    ) -> Result<Option<PersistedTokens>, meerkat_auth_core::auth_store::TokenStoreError> {
        let replacement = if self.loads.fetch_add(1, Ordering::SeqCst) == 1 {
            self.replacement.lock().take()
        } else {
            None
        };
        if let Some((expected, tokens)) = replacement {
            assert_eq!(
                key, &expected,
                "replacement must meet the exact selected key"
            );
            self.inner.save(key, &tokens).await?;
        }
        self.inner.load(key).await
    }
    async fn save(
        &self,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), meerkat_auth_core::auth_store::TokenStoreError> {
        self.inner.save(key, tokens).await
    }
    async fn clear(
        &self,
        key: &TokenKey,
    ) -> Result<(), meerkat_auth_core::auth_store::TokenStoreError> {
        self.inner.clear(key).await
    }
    async fn list(&self) -> Result<Vec<TokenKey>, meerkat_auth_core::auth_store::TokenStoreError> {
        self.inner.list().await
    }
    fn backend_name(&self) -> &'static str {
        "replace-on-second-load"
    }
}

#[tokio::test]
async fn selected_subject_is_rechecked_after_rehydrate_and_inside_refresh_coordinator() {
    for reopened in [false, true] {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(ReplaceOnSecondLoadStore {
            inner: Arc::new(EphemeralTokenStore::new()),
            loads: std::sync::atomic::AtomicUsize::new(0),
            replacement: Mutex::new(None),
        });
        let authority = FixtureAuthority::with_fixture_http(
            ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
            recording_browser(state.clone()),
            Client::new(),
            test_auth_lease(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
            .with_expected_account("fixture-account-42")
            .unwrap();
        authority.interactive_login(&target, None).await.unwrap();
        let mut replacement =
            republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
                if !reopened {
                    tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
                }
            })
            .await;
        replacement.account_id = Some("other-account-at-reload".into());
        let key = target.token_key().unwrap();
        assert!(durable_marker::marker_payload_valid_for_tokens(
            &replacement,
            &key
        ));
        let reader = if reopened {
            FixtureAuthority::with_fixture_http(
                ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
                Arc::new(NeverOpenBrowser),
                Client::new(),
                test_auth_lease(),
            )
        } else {
            authority
        };
        *store.replacement.lock() = Some((key.clone(), replacement.clone()));
        store.loads.store(0, Ordering::SeqCst);
        let result = reader.stored_bearer_token(&target).await;
        assert!(
            matches!(
                result,
                Err(McpOAuthError::Verification(
                    ConnectorOAuthRefusal::AccountMismatch
                ))
            ),
            "both native reload boundaries must retain typed account refusal"
        );
        assert_eq!(
            store.loads.load(Ordering::SeqCst),
            2,
            "the test must reach exactly the intended native reload"
        );
        assert!(store.replacement.lock().is_none());
        assert_eq!(store.inner.load(&key).await.unwrap(), Some(replacement));
        assert_eq!(
            state.token_requests.lock().len(),
            1,
            "reload mismatch must refuse before refresh HTTP effects"
        );
    }
}

struct SubstituteRefreshResultCoordinator {
    inner: InMemoryCoordinator,
    substitute: AtomicBool,
}
#[async_trait]
impl meerkat_auth_core::auth_store::RefreshCoordinator for SubstituteRefreshResultCoordinator {
    async fn with_exclusive_mutation(
        &self,
        key: TokenKey,
        mutation_fn: meerkat_auth_core::auth_store::CredentialMutationFn,
    ) -> Result<
        meerkat_auth_core::auth_store::CredentialMutationOutcome,
        meerkat_auth_core::auth_store::CredentialMutationError,
    > {
        self.inner.with_exclusive_mutation(key, mutation_fn).await
    }
    async fn with_refresh(
        &self,
        key: TokenKey,
        refresh_fn: meerkat_auth_core::auth_store::RefreshFn,
    ) -> Result<PersistedTokens, meerkat_auth_core::auth_store::RefreshError> {
        let mut returned = self.inner.with_refresh(key, refresh_fn).await?;
        if self.substitute.swap(false, Ordering::SeqCst) {
            // Simulate an incorrectly coalesced return value after the real
            // native refresh committed. The durable correct row is untouched.
            returned.account_id = Some("other-coordinator-waiter".into());
        }
        Ok(returned)
    }
}

#[tokio::test]
async fn selected_subject_is_checked_on_exact_coordinator_return_without_revoking_committed_row() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let coordinator = Arc::new(SubstituteRefreshResultCoordinator {
        inner: InMemoryCoordinator::new(),
        substitute: AtomicBool::new(false),
    });
    let owner = test_auth_lease();
    let authority = FixtureAuthority::with_fixture_http(
        ProviderAuthPersistence::new(store.clone(), coordinator.clone()),
        recording_browser(state.clone()),
        Client::new(),
        owner.clone(),
    );
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap();
    authority.interactive_login(&target, None).await.unwrap();
    republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
        tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
    })
    .await;
    coordinator.substitute.store(true, Ordering::SeqCst);
    assert!(matches!(
        authority.stored_bearer_token(&target).await,
        Err(McpOAuthError::Verification(
            ConnectorOAuthRefusal::AccountMismatch
        ))
    ));
    assert!(
        !coordinator.substitute.load(Ordering::SeqCst),
        "the substituted coordinator result must actually have been returned"
    );
    assert_eq!(
        state.token_requests.lock().len(),
        2,
        "login and the real successful refresh must both occur"
    );
    let committed = store
        .load(&target.token_key().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(committed.account_id.as_deref(), Some("fixture-account-42"));
    assert_eq!(
        owner.snapshot(&target.lease_key().unwrap()).phase,
        Some(meerkat_core::handles::AuthLeasePhase::Valid)
    );
    assert_eq!(
        authority
            .stored_bearer_token(&target)
            .await
            .unwrap()
            .as_deref(),
        Some("access-token")
    );
    assert_eq!(
        state.token_requests.lock().len(),
        2,
        "refusing one wrong return value must not revoke or refresh the correct row"
    );
}

// --- Host start/complete split -------------------------------------------

/// A host-chosen loopback redirect. The split never binds it: the host owns
/// the listener, so these tests deliver the callback directly.
const SPLIT_REDIRECT: &str = "http://127.0.0.1:9/mcp/oauth/callback";

fn split_authority(state: &Arc<TestState>, store: Arc<EphemeralTokenStore>) -> FixtureAuthority {
    FixtureAuthority::with_test_http(
        store,
        recording_browser(Arc::clone(state)),
        no_redirect_client(),
        test_auth_lease(),
    )
}

fn split_target(base: &str, name: &str) -> McpServerIdentity {
    McpServerIdentity::from_server_config(name, format!("{base}/mcp"))
        .with_expected_account("fixture-account-42")
        .unwrap()
}

fn split_callback(start: &McpOAuthLoginStart, state: &str) -> McpOAuthCallback {
    McpOAuthCallback {
        redirect_uri: start.redirect_uri.clone(),
        state: state.to_owned(),
        code: "fixture-code".to_owned(),
    }
}

#[tokio::test]
async fn split_start_is_host_only_and_completion_summary_is_secret_free() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = split_authority(&state, store.clone());
    let target = split_target(&base, "split-success");

    let start = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .expect("admission succeeds");
    assert_eq!(start.target, target);
    assert_eq!(start.redirect_uri, SPLIT_REDIRECT);
    assert!(
        start
            .authorize_url
            .starts_with(&format!("{base}/authorize?"))
    );
    assert!(
        start
            .authorize_url
            .contains(&format!("state={}", start.state))
    );
    assert!(start.authorize_url.contains("code_challenge_method=S256"));
    assert!(
        state.token_requests.lock().is_empty(),
        "start performs no token request"
    );
    assert!(
        store.list().await.unwrap().is_empty(),
        "start persists no credential"
    );

    let start_debug = format!("{start:?}");
    assert!(
        !start_debug.contains(&start.state),
        "Debug must redact state"
    );
    assert!(
        !start_debug.contains("code_challenge"),
        "Debug must redact the authorize URL"
    );
    let callback = split_callback(&start, &start.state);
    let callback_debug = format!("{callback:?}");
    assert!(!callback_debug.contains(&start.state));
    assert!(!callback_debug.contains("fixture-code"));

    let complete = authority
        .login_complete(&target, callback)
        .await
        .expect("completion succeeds");
    assert_eq!(complete.target, target);
    assert_eq!(complete.account_id.as_deref(), Some("fixture-account-42"));
    assert!(complete.has_refresh_token);
    assert_eq!(complete.scopes, vec!["mcp.read".to_owned()]);
    assert!(complete.expires_at.is_some());
    let complete_debug = format!("{complete:?}");
    for secret in [
        "access-token",
        "refresh-token",
        "fixture-code",
        start.state.as_str(),
    ] {
        assert!(
            !complete_debug.contains(secret),
            "completion summary leaked {secret:?}"
        );
    }

    let token_requests = state.token_requests.lock().clone();
    assert_eq!(token_requests.len(), 1);
    assert_eq!(token_requests[0]["grant_type"], "authorization_code");
    assert_eq!(token_requests[0]["redirect_uri"], SPLIT_REDIRECT);
    assert!(
        token_requests[0]["code_verifier"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "exchange uses the verifier retained by the flow owner"
    );
    assert!(token_requests[0].get("client_secret").is_none());
    assert_eq!(
        authority
            .stored_bearer_token(&target)
            .await
            .unwrap()
            .as_deref(),
        Some("access-token")
    );
}

#[tokio::test]
async fn split_completion_cannot_be_replayed() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = split_authority(&state, store.clone());
    let target = split_target(&base, "split-replay");
    let start = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .unwrap();
    authority
        .login_complete(&target, split_callback(&start, &start.state))
        .await
        .unwrap();
    let replay = authority
        .login_complete(&target, split_callback(&start, &start.state))
        .await;
    assert!(
        matches!(replay, Err(McpOAuthError::Flow(_))),
        "consumed attempt must refuse replay, got {replay:?}"
    );
    assert_eq!(
        state.token_requests.lock().len(),
        1,
        "replay must not reach the token endpoint"
    );
}

#[tokio::test]
async fn split_completion_refuses_unknown_state_before_exchange() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = split_authority(&state, store.clone());
    let target = split_target(&base, "split-forged-state");
    let start = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .unwrap();
    let forged = authority
        .login_complete(&target, split_callback(&start, "forged-state"))
        .await;
    assert!(
        matches!(forged, Err(McpOAuthError::Flow(_))),
        "got {forged:?}"
    );
    assert!(state.token_requests.lock().is_empty());
    assert!(store.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn split_completion_refuses_substituted_echo_before_exchange() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = split_authority(&state, store.clone());
    let target = split_target(&base, "split-substituted");
    let start = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .unwrap();

    let mut other_redirect = split_callback(&start, &start.state);
    other_redirect.redirect_uri = "http://127.0.0.1:10/mcp/oauth/callback".to_owned();
    let other_target = split_target(&base, "some-other-server");
    for (target, callback) in [
        (&target, other_redirect),
        (&other_target, split_callback(&start, &start.state)),
    ] {
        let result = authority.login_complete(target, callback).await;
        assert!(result.is_err(), "substituted completion must be refused");
    }
    assert!(
        state.token_requests.lock().is_empty(),
        "no substituted completion may reach the token endpoint"
    );
    assert!(store.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn split_completion_refuses_issuer_drift_after_start() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = split_authority(&state, store.clone());
    let target = split_target(&base, "split-drift");
    let start = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .unwrap();
    *state.issuer_override.lock() = Some(format!("{base}/drifted"));
    let drifted = authority
        .login_complete(&target, split_callback(&start, &start.state))
        .await;
    assert!(
        matches!(drifted, Err(McpOAuthError::DiscoveryFailed { .. })),
        "issuer drift must be refused, got {drifted:?}"
    );
    assert!(state.token_requests.lock().is_empty());
    assert!(store.list().await.unwrap().is_empty());
    *state.issuer_override.lock() = None;
    assert_attempt_retired(&authority, &target, &start).await;
}

#[tokio::test]
async fn split_cancel_retires_the_admitted_attempt() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = split_authority(&state, store.clone());
    let target = split_target(&base, "split-cancel");
    let start = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .unwrap();
    authority.login_cancel(&target, &start).unwrap();
    let late = authority
        .login_complete(&target, split_callback(&start, &start.state))
        .await;
    assert!(matches!(late, Err(McpOAuthError::Flow(_))), "got {late:?}");
    assert!(state.token_requests.lock().is_empty());
    assert!(store.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn split_start_requires_strategy_and_selected_account() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let without_strategy = McpOAuthAuthority::new(
        ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
        test_auth_lease().generated,
    );
    assert!(matches!(
        without_strategy
            .login_start(&split_target(&base, "no-strategy"), SPLIT_REDIRECT, None)
            .await,
        Err(McpOAuthError::Verification(
            ConnectorOAuthRefusal::VerificationUnavailable
        ))
    ));
    let authority = split_authority(&state, store.clone());
    let unselected = McpServerIdentity::from_server_config("no-account", format!("{base}/mcp"));
    assert!(matches!(
        authority
            .login_start(&unselected, SPLIT_REDIRECT, None)
            .await,
        Err(McpOAuthError::AccountSelectionRequired)
    ));
    assert!(state.registration_requests.lock().is_empty());
}

// --- Production OIDC UserInfo account strategy --------------------------

fn oidc_authority(store: Arc<EphemeralTokenStore>) -> McpOAuthAuthority {
    let owner = test_auth_lease();
    let flows = Arc::new(RuntimeOAuthFlowHandle::new_with_auth_lease(
        MCP_INTERACTIVE_LOGIN_TIMEOUT,
        owner.lifecycle,
    ));
    McpOAuthAuthority::with_http(
        ProviderAuthPersistence::new(store, Arc::new(InMemoryCoordinator::new())),
        Client::new(),
        owner.generated,
    )
    .with_interactive_strategy(
        flows,
        Arc::new(meerkat_auth_core::mcp_oauth::OidcUserInfoAccountStrategy::new()),
    )
    .unwrap()
}

fn oidc_target(base: &str, account: &str) -> McpServerIdentity {
    McpServerIdentity::from_server_config("oidc", format!("{base}/mcp"))
        .with_expected_account(account)
        .unwrap()
}

#[tokio::test]
async fn oidc_userinfo_strategy_binds_subject_and_requests_openid() {
    let (base, state) = spawn_oauth_fixture().await;
    *state.token_scope_override.lock() = Some("openid".to_owned());
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = oidc_authority(store.clone());
    let target = oidc_target(&base, "oidc-subject-7");
    let start = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .unwrap();
    assert!(start.authorize_url.contains("scope=openid"));
    let complete = authority
        .login_complete(&target, split_callback(&start, &start.state))
        .await
        .expect("OIDC subject matches the selected account");
    assert_eq!(complete.account_id.as_deref(), Some("oidc-subject-7"));
    assert_eq!(complete.scopes, vec!["openid".to_owned()]);
    assert_eq!(
        state.userinfo_requests.lock().clone(),
        vec![Some("Bearer access-token".to_owned())]
    );
}

#[tokio::test]
async fn oidc_userinfo_strategy_refuses_other_subject_without_persisting() {
    let (base, state) = spawn_oauth_fixture().await;
    *state.token_scope_override.lock() = Some("openid".to_owned());
    *state.userinfo_sub.lock() = Some("someone-else".to_owned());
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = oidc_authority(store.clone());
    let target = oidc_target(&base, "oidc-subject-7");
    let start = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .unwrap();
    let refused = authority
        .login_complete(&target, split_callback(&start, &start.state))
        .await;
    assert!(
        matches!(refused, Err(McpOAuthError::Verification(_))),
        "got {refused:?}"
    );
    assert!(store.list().await.unwrap().is_empty());
    // The failed completion retired the attempt.
    assert!(matches!(
        authority
            .login_complete(&target, split_callback(&start, &start.state))
            .await,
        Err(McpOAuthError::Flow(_))
    ));
}

#[tokio::test]
async fn oidc_userinfo_strategy_refuses_missing_userinfo_or_openid_grant() {
    for (omit_userinfo, scope) in [(true, "openid"), (false, "mcp.read")] {
        let (base, state) = spawn_oauth_fixture().await;
        *state.omit_userinfo_endpoint.lock() = omit_userinfo;
        *state.token_scope_override.lock() = Some(scope.to_owned());
        let store = Arc::new(EphemeralTokenStore::new());
        let authority = oidc_authority(store.clone());
        let target = oidc_target(&base, "oidc-subject-7");
        let start = authority
            .login_start(&target, SPLIT_REDIRECT, None)
            .await
            .unwrap();
        let refused = authority
            .login_complete(&target, split_callback(&start, &start.state))
            .await;
        assert!(
            matches!(refused, Err(McpOAuthError::Verification(_))),
            "omit_userinfo={omit_userinfo} scope={scope}: got {refused:?}"
        );
        assert!(store.list().await.unwrap().is_empty());
    }
}

// --- Join, typed cancel and advisory launch -------------------------------

async fn follow_authorize(url: &str) {
    Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .unwrap()
        .get(url)
        .send()
        .await
        .unwrap();
}

#[tokio::test]
async fn split_second_start_joins_the_pending_attempt() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = split_authority(&state, store.clone());
    let target = split_target(&base, "split-join");
    let first = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .unwrap();
    assert_eq!(first.disposition, McpOAuthLoginDisposition::Started);
    let joined = authority
        .login_start(&target, "http://127.0.0.1:10/mcp/oauth/callback", None)
        .await
        .unwrap();
    assert_eq!(joined.disposition, McpOAuthLoginDisposition::Joined);
    assert_eq!(joined.state, first.state);
    assert_eq!(joined.authorize_url, first.authorize_url);
    assert_eq!(joined.redirect_uri, first.redirect_uri);
    assert_eq!(
        state.registration_requests.lock().len(),
        1,
        "a join registers no second client"
    );
    authority
        .login_complete(&target, split_callback(&joined, &joined.state))
        .await
        .expect("the joined projection completes the single attempt");
    let next = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .unwrap();
    assert_eq!(
        next.disposition,
        McpOAuthLoginDisposition::Started,
        "a consumed attempt is not joined"
    );
    assert_ne!(next.state, first.state);
}

#[tokio::test]
async fn split_start_after_cancel_admits_a_fresh_attempt() {
    let (base, state) = spawn_oauth_fixture().await;
    let authority = split_authority(&state, Arc::new(EphemeralTokenStore::new()));
    let target = split_target(&base, "split-cancel-restart");
    let first = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .unwrap();
    authority.login_cancel(&target, &first).unwrap();
    let second = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .unwrap();
    assert_eq!(second.disposition, McpOAuthLoginDisposition::Started);
    assert_ne!(second.state, first.state);
}

#[tokio::test]
async fn pending_loopback_cancel_retires_binding_and_attempt() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = split_authority(&state, store.clone());
    let target = split_target(&base, "loopback-cancel");
    let McpOAuthLoopbackBegin::Started(pending) =
        authority.begin_loopback_login(&target, None).await.unwrap()
    else {
        panic!("first loopback login admits the attempt");
    };
    let start = pending.start().clone();
    pending.cancel().await.unwrap();

    assert!(
        Client::new()
            .get(format!(
                "{}?code=x&state={}",
                start.redirect_uri, start.state
            ))
            .send()
            .await
            .is_err(),
        "the callback binding is retired"
    );
    assert!(matches!(
        authority
            .login_complete(&target, split_callback(&start, &start.state))
            .await,
        Err(McpOAuthError::Flow(_))
    ));
    assert!(state.token_requests.lock().is_empty());
    assert!(store.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn dropping_a_pending_loopback_login_retires_the_attempt() {
    let (base, state) = spawn_oauth_fixture().await;
    let authority = split_authority(&state, Arc::new(EphemeralTokenStore::new()));
    let target = split_target(&base, "loopback-drop");
    let McpOAuthLoopbackBegin::Started(pending) =
        authority.begin_loopback_login(&target, None).await.unwrap()
    else {
        panic!("first loopback login admits the attempt");
    };
    let start = pending.start().clone();
    drop(pending);
    assert!(matches!(
        authority
            .login_complete(&target, split_callback(&start, &start.state))
            .await,
        Err(McpOAuthError::Flow(_))
    ));
}

#[tokio::test]
async fn second_loopback_login_joins_without_a_second_attempt() {
    let (base, state) = spawn_oauth_fixture().await;
    let authority = split_authority(&state, Arc::new(EphemeralTokenStore::new()));
    let target = split_target(&base, "loopback-join");
    let McpOAuthLoopbackBegin::Started(pending) =
        authority.begin_loopback_login(&target, None).await.unwrap()
    else {
        panic!("first loopback login admits the attempt");
    };
    let McpOAuthLoopbackBegin::Joined(joined) =
        authority.begin_loopback_login(&target, None).await.unwrap()
    else {
        panic!("second loopback login joins");
    };
    assert_eq!(joined.state, pending.start().state);
    assert_eq!(joined.redirect_uri, pending.start().redirect_uri);
    // The owner of the original binding still completes the one attempt.
    follow_authorize(&pending.start().authorize_url).await;
    pending
        .complete(MCP_INTERACTIVE_LOGIN_TIMEOUT)
        .await
        .expect("original listener completes");
}

#[tokio::test]
async fn failed_browser_launch_is_advisory_and_runs_off_the_runtime() {
    let (base, state) = spawn_oauth_fixture().await;
    let authority = split_authority(&state, Arc::new(EphemeralTokenStore::new()));
    let target = split_target(&base, "launch-advisory");
    let McpOAuthLoopbackBegin::Started(pending) =
        authority.begin_loopback_login(&target, None).await.unwrap()
    else {
        panic!("first loopback login admits the attempt");
    };

    // The opener blocks until a task on this (current-thread) runtime
    // releases it: if the launch ran on the runtime, this would deadlock.
    let (release, released) = std::sync::mpsc::channel::<()>();
    let releaser = tokio::spawn(async move { release.send(()).unwrap() });
    let launch = tokio::time::timeout(
        Duration::from_secs(5),
        pending.launch_browser(move |_url| {
            released.recv().unwrap();
            Err(std::io::Error::other("opener refused"))
        }),
    )
    .await
    .expect("launch must not block the async runtime");
    releaser.await.unwrap();
    assert_eq!(launch, McpOAuthBrowserLaunch::Failed);

    // A failed launch neither cancels nor retries: the attempt still
    // completes once the user's browser reaches the callback.
    follow_authorize(&pending.start().authorize_url).await;
    pending
        .complete(MCP_INTERACTIVE_LOGIN_TIMEOUT)
        .await
        .expect("attempt survives a failed launch");
    assert_eq!(state.registration_requests.lock().len(), 1);
}

#[tokio::test]
async fn oidc_subject_match_is_exact() {
    let (base, state) = spawn_oauth_fixture().await;
    *state.token_scope_override.lock() = Some("openid".to_owned());
    *state.userinfo_sub.lock() = Some("OIDC-SUBJECT-7".to_owned());
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = oidc_authority(store.clone());
    let target = oidc_target(&base, "oidc-subject-7");
    let start = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .unwrap();
    assert!(matches!(
        authority
            .login_complete(&target, split_callback(&start, &start.state))
            .await,
        Err(McpOAuthError::Verification(_))
    ));
    assert!(store.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn oidc_userinfo_endpoint_must_be_https_or_loopback_from_issuer_metadata() {
    let (base, state) = spawn_oauth_fixture().await;
    *state.token_scope_override.lock() = Some("openid".to_owned());
    *state.userinfo_endpoint_override.lock() = Some("http://userinfo.example/userinfo".into());
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = oidc_authority(store.clone());
    let target = oidc_target(&base, "oidc-subject-7");
    let start = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .unwrap();
    assert!(matches!(
        authority
            .login_complete(&target, split_callback(&start, &start.state))
            .await,
        Err(McpOAuthError::Verification(
            ConnectorOAuthRefusal::VerificationUnavailable
        ))
    ));
    assert!(
        state.userinfo_requests.lock().is_empty(),
        "the bearer token is never sent to a non-loopback http endpoint"
    );
    assert!(store.list().await.unwrap().is_empty());
}

// --- Review fixes: state first, retirement on every failure, drop guard ---

fn no_redirect_client() -> Client {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// A retired attempt cannot complete, even with the right callback.
async fn assert_attempt_retired(
    authority: &McpOAuthAuthority,
    target: &McpServerIdentity,
    start: &McpOAuthLoginStart,
) {
    assert!(
        matches!(
            authority
                .login_complete(target, split_callback(start, &start.state))
                .await,
            Err(McpOAuthError::Flow(_))
        ),
        "the failed completion must have retired the attempt"
    );
}

#[tokio::test]
async fn completion_with_unknown_state_makes_no_network_call() {
    let (base, state) = spawn_oauth_fixture().await;
    let authority = split_authority(&state, Arc::new(EphemeralTokenStore::new()));
    let target = split_target(&base, "no-network");
    let start = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .unwrap();
    let before = state.request_paths.lock().len();
    let forged = authority
        .login_complete(&target, split_callback(&start, "forged-state"))
        .await;
    assert!(
        matches!(forged, Err(McpOAuthError::Flow(_))),
        "got {forged:?}"
    );
    assert_eq!(
        state.request_paths.lock().len(),
        before,
        "an unproven state must cause no network request at all"
    );
}

#[tokio::test]
async fn completion_fetches_only_the_recorded_issuer_metadata() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let authority = split_authority(&state, store.clone());
    let target = split_target(&base, "issuer-only");
    let start = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .unwrap();
    state.request_paths.lock().clear();
    authority
        .login_complete(&target, split_callback(&start, &start.state))
        .await
        .unwrap();
    let paths = state.request_paths.lock().clone();
    assert_eq!(
        paths,
        vec![
            "/.well-known/oauth-authorization-server".to_owned(),
            "/token".to_owned()
        ],
        "completion takes the issuer, resource and client from the admitted attempt"
    );
}

#[tokio::test]
async fn every_completion_failure_stage_retires_the_attempt() {
    // Redirect mismatch (owner verification).
    {
        let (base, state) = spawn_oauth_fixture().await;
        let authority = split_authority(&state, Arc::new(EphemeralTokenStore::new()));
        let target = split_target(&base, "fail-verify");
        let start = authority
            .login_start(&target, SPLIT_REDIRECT, None)
            .await
            .unwrap();
        let mut wrong = split_callback(&start, &start.state);
        wrong.redirect_uri = "http://127.0.0.1:10/mcp/oauth/callback".to_owned();
        assert!(authority.login_complete(&target, wrong).await.is_err());
        assert_attempt_retired(&authority, &target, &start).await;
    }
    // Authorization-server metadata unavailable.
    {
        let (base, state) = spawn_oauth_fixture().await;
        let authority = split_authority(&state, Arc::new(EphemeralTokenStore::new()));
        let target = split_target(&base, "fail-discovery");
        let start = authority
            .login_start(&target, SPLIT_REDIRECT, None)
            .await
            .unwrap();
        state
            .redirect_authorization_metadata
            .store(true, Ordering::SeqCst);
        assert!(matches!(
            authority
                .login_complete(&target, split_callback(&start, &start.state))
                .await,
            Err(McpOAuthError::DiscoveryFailed { .. })
        ));
        state
            .redirect_authorization_metadata
            .store(false, Ordering::SeqCst);
        assert_attempt_retired(&authority, &target, &start).await;
    }
    // Token exchange failure.
    {
        let (base, state) = spawn_oauth_fixture().await;
        let authority = split_authority(&state, Arc::new(EphemeralTokenStore::new()));
        let target = split_target(&base, "fail-exchange");
        let start = authority
            .login_start(&target, SPLIT_REDIRECT, None)
            .await
            .unwrap();
        *state.token_fails.lock() = true;
        assert!(matches!(
            authority
                .login_complete(&target, split_callback(&start, &start.state))
                .await,
            Err(McpOAuthError::TokenExchangeFailed { .. })
        ));
        *state.token_fails.lock() = false;
        assert_attempt_retired(&authority, &target, &start).await;
    }
    // Persistence failure in the commit.
    {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(FailNextSaveStore {
            inner: Arc::new(EphemeralTokenStore::new()),
            fail_next_save: AtomicBool::new(true),
        });
        let authority = FixtureAuthority::with_fixture_http(
            ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
            recording_browser(Arc::clone(&state)),
            no_redirect_client(),
            test_auth_lease(),
        );
        let target = split_target(&base, "fail-persist");
        let start = authority
            .login_start(&target, SPLIT_REDIRECT, None)
            .await
            .unwrap();
        assert!(
            authority
                .login_complete(&target, split_callback(&start, &start.state))
                .await
                .is_err()
        );
        assert_attempt_retired(&authority, &target, &start).await;
    }
}

#[tokio::test]
async fn dropping_complete_mid_wait_retires_binding_and_attempt() {
    let (base, state) = spawn_oauth_fixture().await;
    let authority = split_authority(&state, Arc::new(EphemeralTokenStore::new()));
    let target = split_target(&base, "drop-mid-wait");
    let McpOAuthLoopbackBegin::Started(pending) =
        authority.begin_loopback_login(&target, None).await.unwrap()
    else {
        panic!("first loopback login admits the attempt");
    };
    let start = pending.start().clone();
    let waiting = tokio::spawn(pending.complete(MCP_INTERACTIVE_LOGIN_TIMEOUT));
    tokio::time::sleep(Duration::from_millis(50)).await;
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while Client::new()
        .get(format!(
            "{}?code=x&state={}",
            start.redirect_uri, start.state
        ))
        .send()
        .await
        .is_ok()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the callback binding was not retired"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_attempt_retired(&authority, &target, &start).await;
}

#[tokio::test]
async fn concurrent_starts_admit_one_attempt_with_one_registration() {
    let (base, state) = spawn_oauth_fixture().await;
    let authority = split_authority(&state, Arc::new(EphemeralTokenStore::new()));
    let target = split_target(&base, "concurrent");
    let (a, b) = tokio::join!(
        authority.login_start(&target, SPLIT_REDIRECT, None),
        authority.login_start(&target, "http://127.0.0.1:10/mcp/oauth/callback", None)
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    let mut dispositions = [a.disposition, b.disposition];
    dispositions.sort_by_key(|d| *d == McpOAuthLoginDisposition::Joined);
    assert_eq!(
        dispositions,
        [
            McpOAuthLoginDisposition::Started,
            McpOAuthLoginDisposition::Joined
        ]
    );
    assert_eq!(a.state, b.state);
    assert_eq!(state.registration_requests.lock().len(), 1);
}

#[tokio::test]
async fn redirects_from_discovery_and_token_endpoints_are_refused() {
    let (base, state) = spawn_oauth_fixture().await;
    let authority = split_authority(&state, Arc::new(EphemeralTokenStore::new()));
    state
        .redirect_authorization_metadata
        .store(true, Ordering::SeqCst);
    assert!(matches!(
        authority
            .login_start(
                &split_target(&base, "redirect-discovery"),
                SPLIT_REDIRECT,
                None
            )
            .await,
        Err(McpOAuthError::DiscoveryFailed { .. })
    ));
    state
        .redirect_authorization_metadata
        .store(false, Ordering::SeqCst);

    let target = split_target(&base, "redirect-token");
    let start = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .unwrap();
    state.redirect_token.store(true, Ordering::SeqCst);
    assert!(matches!(
        authority
            .login_complete(&target, split_callback(&start, &start.state))
            .await,
        Err(McpOAuthError::TokenExchangeFailed { .. })
    ));
    assert!(
        !state
            .request_paths
            .lock()
            .iter()
            .any(|path| path == "/redirected"),
        "no redirect may be followed"
    );
}

#[tokio::test]
async fn non_loopback_redirect_is_refused_before_any_network() {
    let (base, state) = spawn_oauth_fixture().await;
    let authority = split_authority(&state, Arc::new(EphemeralTokenStore::new()));
    for redirect in [
        "https://app.example/callback",
        "http://192.0.2.1:8080/callback",
        "not a url",
    ] {
        assert!(matches!(
            authority
                .login_start(&split_target(&base, "bad-redirect"), redirect, None)
                .await,
            Err(McpOAuthError::Verification(
                ConnectorOAuthRefusal::InvalidDescriptor
            ))
        ));
    }
    assert!(state.request_paths.lock().is_empty());
}

#[tokio::test]
async fn wire_style_cancel_by_state_retires_the_attempt() {
    let (base, state) = spawn_oauth_fixture().await;
    let authority = split_authority(&state, Arc::new(EphemeralTokenStore::new()));
    let target = split_target(&base, "cancel-by-state");
    let start = authority
        .login_start(&target, SPLIT_REDIRECT, None)
        .await
        .unwrap();
    assert!(matches!(
        authority.cancel_attempt(&target, "unknown-state"),
        Err(McpOAuthError::Flow(_))
    ));
    authority.cancel_attempt(&target, &start.state).unwrap();
    assert_attempt_retired(&authority, &target, &start).await;
}
