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
    BrowserOpener, MCP_INTERACTIVE_LOGIN_TIMEOUT, McpOAuthAccountStrategy, McpOAuthAuthority,
    McpOAuthCeremonyContext, McpOAuthError, McpServerIdentity,
};
use meerkat_core::generated::auth_lease_durable_lifecycle_marker as durable_marker;
use meerkat_core::handles::{AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, GeneratedAuthLeaseHandle};
use meerkat_runtime::handles::RuntimeOAuthFlowHandle;
use reqwest::Client;
use std::sync::Arc;
use std::time::Duration;

// This fixture retains existing native handles; it defines no auth state,
// terminal-owner marker, alternate registry, or production API.
#[derive(Clone)]
struct FixtureAuthority {
    native: McpOAuthAuthority,
    auth_lease: GeneratedAuthLeaseHandle,
    flows: Arc<RuntimeOAuthFlowHandle>,
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
        browser: Arc<dyn BrowserOpener>,
        http: Client,
        owner: TestAuthAuthority,
    ) -> Self {
        let flows = Arc::new(RuntimeOAuthFlowHandle::new_with_auth_lease(
            MCP_INTERACTIVE_LOGIN_TIMEOUT,
            owner.lifecycle,
        ));
        let native =
            McpOAuthAuthority::with_http(persistence, browser, http, owner.generated.clone())
                .with_interactive_strategy(flows.clone(), Arc::new(FixtureAccountStrategy))
                .expect("fixture uses the actual matched runtime flow owner");
        Self {
            native,
            auth_lease: owner.generated,
            flows,
        }
    }

    fn with_test_http(
        token_store: Arc<EphemeralTokenStore>,
        browser: Arc<dyn BrowserOpener>,
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
    pause_refresh: AtomicBool,
    refresh_started: Notify,
    refresh_release: Notify,
}

struct RecordingBrowser {
    state: Arc<TestState>,
    http: Client,
}

#[async_trait]
impl BrowserOpener for RecordingBrowser {
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
impl BrowserOpener for NoCallbackBrowser {
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
        .with_state(Arc::clone(&state));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), state)
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
) -> impl IntoResponse {
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
    Json(body)
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

async fn token(
    State(state): State<Arc<TestState>>,
    Form(body): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    state
        .token_requests
        .lock()
        .push(serde_json::to_value(&body).unwrap());
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
    if *state.token_transiently_fails.lock() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "temporarily_unavailable" })),
        )
            .into_response();
    }
    Json(serde_json::json!({
        "access_token": "access-token",
        "refresh_token": "refresh-token",
        "expires_in": 3600,
        "scope": "mcp.read"
    }))
    .into_response()
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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
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

fn recording_browser(state: Arc<TestState>) -> Arc<dyn BrowserOpener> {
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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
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

#[tokio::test]
async fn stored_token_metadata_must_match_requested_target() {
    let (base, state) = spawn_oauth_fixture().await;
    let store = Arc::new(EphemeralTokenStore::new());
    let browser = recording_browser(Arc::clone(&state));
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), test_auth_lease());
    let original = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
    let other = McpServerIdentity::from_server_config("glean", format!("{base}/other-mcp"));

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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

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
    let target = McpServerIdentity::from_server_config("glean", "http://mcp.example.test/mcp");

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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

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
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

    let result = authority.interactive_login(&target, None).await;

    assert_login_fails_closed(Arc::clone(&state), store, &target, result, "user denied").await;
}

#[tokio::test]
async fn interactive_login_timeout_fails_closed() {
    let (base, state) = spawn_oauth_fixture().await;
    *state.authorize_outcome.lock() = AuthorizeOutcome::NoCallback;
    let store = Arc::new(EphemeralTokenStore::new());
    let browser: Arc<dyn BrowserOpener> = Arc::new(NoCallbackBrowser {
        state: Arc::clone(&state),
    });
    assert_eq!(MCP_INTERACTIVE_LOGIN_TIMEOUT, Duration::from_secs(300));
    let authority =
        FixtureAuthority::with_test_http(store.clone(), browser, Client::new(), test_auth_lease());
    let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

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
impl BrowserOpener for NeverOpenBrowser {
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
        Arc::new(NeverOpenBrowser),
        owner.generated,
    );
    let target = McpServerIdentity::from_server_config("untrusted-label", "not-even-a-url");
    assert!(matches!(
        authority.interactive_login(&target, None).await,
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
            McpServerIdentity::from_server_config("label-is-not-an-account", format!("{base}/mcp"));
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

    // Exercise the production admitted-attempt Drop path while its callback
    // wait is still pending. This browser reports the URL only to the driver.
    struct PendingBrowser(tokio::sync::mpsc::UnboundedSender<String>);
    #[async_trait]
    impl BrowserOpener for PendingBrowser {
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
        Arc::new(PendingBrowser(opened)),
        Client::new(),
        auth.generated,
    )
    .with_interactive_strategy(flows, Arc::new(FixtureAccountStrategy))
    .unwrap();
    let target = McpServerIdentity::from_server_config("expiry-cancel", format!("{base}/mcp"));
    let login_target = target.clone();
    let login = tokio::spawn(async move { native.interactive_login(&login_target, None).await });
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
