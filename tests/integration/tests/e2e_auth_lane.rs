//! Release-grade auth smoke lane.
//!
//! The deterministic tests use a local OAuth fixture and drive the public RPC
//! auth surface (`auth/login/start`, `auth/login/complete`,
//! `auth/status/get`, `auth/logout`) plus the factory-backed provider resolve
//! path. Live canaries are optional and skip explicitly when their credentials
//! are absent.
//!
//! Run with: `./scripts/repo-cargo e2e-auth` or `make e2e-auth`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::{StreamExt, future::join_all};
use meerkat_auth_core::authorizers::{GoogleAuthAuthorizer, GoogleAuthChain};
use meerkat_client::{
    AnthropicClient, GeminiClient, LlmDoneOutcome, LlmEvent, LlmRequest, types::LlmClient,
};
use meerkat_core::handles::LeaseKey;
use meerkat_core::{
    AuthBindingRef, AuthConstraints, AuthMetadataDefaults, AuthProfileConfig, BackendProfileConfig,
    BindingId, BindingPolicy, BlobStore, Config, ConfigRuntime, ConfigStore, CredentialSourceSpec,
    HttpAuthorizationRequest, HttpAuthorizer, MemoryConfigStore, Message, ProviderBindingConfig,
    RealmConfigSection, RealmConnectionSet, RealmId, UserMessage,
};
use meerkat_providers::ResolverEnvironment;
use meerkat_providers::auth_store::{
    FileTokenStore, InMemoryCoordinator, PersistedTokens, RefreshCoordinator, TokenKey, TokenStore,
};
// RPC-host: this integration test directly drives the JSON-RPC dispatch surface
// (MethodRouter::method_call with RpcRequest/RpcResponse) to exercise RPC method
// behaviour end-to-end without TCP/stdio transport. Wire types
// (RpcId/RpcRequest/RpcResponse/RpcNotification) are inherently RPC-shape;
// SessionRuntime + NotificationSink back the dispatcher. Lifting these would
// remove the test's reason for existing — it asserts the RPC method-call surface
// (auth/login/start, auth/login/complete, auth/status/get, auth/logout), not
// the upstream session_runtime semantics.
use meerkat_rpc::protocol::{RpcId, RpcRequest};
use meerkat_rpc::router::{MethodRouter, NotificationSink};
use meerkat_rpc::session_runtime::SessionRuntime;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, OwnedMutexGuard, oneshot};
use tokio::task::JoinHandle;

const REALM_ID: &str = "dev";
const BINDING_ID: &str = "default_openai";
const REDIRECT_URI: &str = "http://127.0.0.1:48179/oauth/callback";
const OPENAI_PROVIDER: &str = "openai";
const TEST_OVERRIDE_ENV: &str = "MEERKAT_TEST_OAUTH_ENDPOINT_OVERRIDE";
const TEST_BASE_URL_ENV: &str = "MEERKAT_TEST_OAUTH_BASE_URL";

static NEXT_RPC_ID: AtomicI64 = AtomicI64::new(1);
static ENV_LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();

#[derive(Clone, Copy)]
enum CallbackStateMode {
    Echo,
    WrongState,
}

#[derive(Clone, Copy)]
enum TokenMode {
    Success,
    BadCode,
    Malformed,
}

#[derive(Clone, Copy)]
enum RefreshMode {
    Success,
    Failed,
    Malformed,
}

#[derive(Clone, Copy)]
struct MockOAuthConfig {
    callback_state: CallbackStateMode,
    token_mode: TokenMode,
    refresh_mode: RefreshMode,
    initial_expires_in: u64,
    refresh_expires_in: u64,
    refresh_delay: Duration,
}

impl Default for MockOAuthConfig {
    fn default() -> Self {
        Self {
            callback_state: CallbackStateMode::Echo,
            token_mode: TokenMode::Success,
            refresh_mode: RefreshMode::Success,
            initial_expires_in: 3_600,
            refresh_expires_in: 3_600,
            refresh_delay: Duration::ZERO,
        }
    }
}

struct MockOAuthProvider {
    base_url: String,
    state: FixtureState,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl MockOAuthProvider {
    async fn start(config: MockOAuthConfig) -> Self {
        let state = FixtureState::new(config);
        let app = Router::new()
            .route("/{provider}/authorize", get(authorize_handler))
            .route("/{provider}/token", post(token_handler))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local OAuth fixture");
        let addr = listener.local_addr().expect("fixture local addr");
        let (tx, rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let server = axum::serve(listener, app).with_graceful_shutdown(async {
                let _ = rx.await;
            });
            if let Err(err) = server.await {
                eprintln!("mock oauth fixture stopped with error: {err}");
            }
        });
        Self {
            base_url: format!("http://{addr}"),
            state,
            shutdown: Some(tx),
            task,
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn refresh_count(&self) -> usize {
        self.state.inner.refresh_count.load(Ordering::SeqCst)
    }
}

impl Drop for MockOAuthProvider {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        self.task.abort();
    }
}

#[derive(Clone)]
struct FixtureState {
    inner: Arc<FixtureInner>,
}

impl FixtureState {
    fn new(config: MockOAuthConfig) -> Self {
        Self {
            inner: Arc::new(FixtureInner {
                config,
                codes: Mutex::new(HashMap::new()),
                refresh_tokens: Mutex::new(HashSet::new()),
                code_seq: AtomicU64::new(1),
                authorize_count: AtomicUsize::new(0),
                code_exchange_count: AtomicUsize::new(0),
                refresh_count: AtomicUsize::new(0),
            }),
        }
    }
}

struct FixtureInner {
    config: MockOAuthConfig,
    codes: Mutex<HashMap<String, AuthCodeRecord>>,
    refresh_tokens: Mutex<HashSet<String>>,
    code_seq: AtomicU64,
    authorize_count: AtomicUsize,
    code_exchange_count: AtomicUsize,
    refresh_count: AtomicUsize,
}

#[derive(Clone)]
struct AuthCodeRecord {
    provider: String,
    redirect_uri: String,
    code_challenge: String,
    state: String,
}

#[derive(Deserialize)]
struct AuthorizeQuery {
    response_type: String,
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    code_challenge_method: String,
    state: String,
    #[serde(default)]
    scope: Option<String>,
}

async fn authorize_handler(
    State(state): State<FixtureState>,
    AxumPath(provider): AxumPath<String>,
    Query(query): Query<AuthorizeQuery>,
) -> Result<Redirect, (StatusCode, String)> {
    validate_authorize_request(&provider, &query)?;
    state.inner.authorize_count.fetch_add(1, Ordering::SeqCst);
    let seq = state.inner.code_seq.fetch_add(1, Ordering::SeqCst);
    let code = format!("mock-code-{provider}-{seq}");
    state.inner.codes.lock().await.insert(
        code.clone(),
        AuthCodeRecord {
            provider,
            redirect_uri: query.redirect_uri.clone(),
            code_challenge: query.code_challenge,
            state: query.state.clone(),
        },
    );
    let callback_state = match state.inner.config.callback_state {
        CallbackStateMode::Echo => query.state,
        CallbackStateMode::WrongState => "st-fixture-expired-state".to_string(),
    };
    let mut callback = reqwest::Url::parse(&query.redirect_uri).map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid redirect_uri: {err}"),
        )
    })?;
    callback
        .query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("state", &callback_state);
    Ok(Redirect::temporary(callback.as_str()))
}

fn validate_authorize_request(
    provider: &str,
    query: &AuthorizeQuery,
) -> Result<(), (StatusCode, String)> {
    if !matches!(
        provider,
        "openai" | "anthropic" | "anthropic_console_api_key" | "google"
    ) {
        return Err((StatusCode::BAD_REQUEST, "unknown provider".to_string()));
    }
    if query.response_type != "code" {
        return Err((
            StatusCode::BAD_REQUEST,
            "response_type must be code".to_string(),
        ));
    }
    if query.client_id.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "client_id is required".to_string()));
    }
    let redirect = reqwest::Url::parse(&query.redirect_uri).map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid redirect_uri: {err}"),
        )
    })?;
    if redirect.scheme() != "http" || redirect.host_str() != Some("127.0.0.1") {
        return Err((
            StatusCode::BAD_REQUEST,
            "redirect_uri must be loopback http".to_string(),
        ));
    }
    if !query.state.starts_with("st-") {
        return Err((StatusCode::BAD_REQUEST, "state is invalid".to_string()));
    }
    if query.code_challenge.len() < 32 {
        return Err((
            StatusCode::BAD_REQUEST,
            "code_challenge is too short".to_string(),
        ));
    }
    if query.code_challenge_method != "S256" {
        return Err((
            StatusCode::BAD_REQUEST,
            "code_challenge_method must be S256".to_string(),
        ));
    }
    if query.scope.as_deref().unwrap_or_default().trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "scope is required".to_string()));
    }
    Ok(())
}

async fn token_handler(
    State(state): State<FixtureState>,
    AxumPath(provider): AxumPath<String>,
    Form(form): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    match form.get("grant_type").map(String::as_str) {
        Some("authorization_code") => exchange_code(&state, &provider, &form).await,
        Some("refresh_token") => refresh_token(&state, &provider, &form).await,
        _ => oauth_error(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "unsupported grant",
        ),
    }
}

async fn exchange_code(
    state: &FixtureState,
    provider: &str,
    form: &HashMap<String, String>,
) -> (StatusCode, Json<Value>) {
    if matches!(state.inner.config.token_mode, TokenMode::BadCode) {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", "bad code");
    }
    let Some(code) = form.get("code") else {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_request", "missing code");
    };
    let Some(redirect_uri) = form.get("redirect_uri") else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "missing redirect_uri",
        );
    };
    let Some(verifier) = form.get("code_verifier") else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "missing code_verifier",
        );
    };
    let record = match state.inner.codes.lock().await.remove(code) {
        Some(record) => record,
        None => return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", "unknown code"),
    };
    if record.provider != provider {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "provider mismatch",
        );
    }
    if record.redirect_uri != *redirect_uri {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "redirect_uri mismatch",
        );
    }
    if pkce_s256(verifier) != record.code_challenge {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "pkce verifier mismatch",
        );
    }
    if matches!(state.inner.config.token_mode, TokenMode::Malformed) {
        return (StatusCode::OK, Json(json!({ "token_type": "Bearer" })));
    }

    let count = state
        .inner
        .code_exchange_count
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    let refresh = format!("mock-refresh-{count}");
    state
        .inner
        .refresh_tokens
        .lock()
        .await
        .insert(refresh.clone());
    (
        StatusCode::OK,
        Json(json!({
            "access_token": format!("mock-access-{count}"),
            "refresh_token": refresh,
            "expires_in": state.inner.config.initial_expires_in,
            "scope": "openid profile offline_access",
            "token_type": "Bearer",
            "fixture_state": record.state,
        })),
    )
}

async fn refresh_token(
    state: &FixtureState,
    provider: &str,
    form: &HashMap<String, String>,
) -> (StatusCode, Json<Value>) {
    if provider != OPENAI_PROVIDER {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "provider mismatch",
        );
    }
    let Some(refresh_token) = form.get("refresh_token") else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "missing refresh_token",
        );
    };
    if !state
        .inner
        .refresh_tokens
        .lock()
        .await
        .contains(refresh_token)
    {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "unknown refresh token",
        );
    }
    if !state.inner.config.refresh_delay.is_zero() {
        tokio::time::sleep(state.inner.config.refresh_delay).await;
    }
    let count = state.inner.refresh_count.fetch_add(1, Ordering::SeqCst) + 1;
    match state.inner.config.refresh_mode {
        RefreshMode::Failed => oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "refresh token revoked",
        ),
        RefreshMode::Malformed => (StatusCode::OK, Json(json!({ "token_type": "Bearer" }))),
        RefreshMode::Success => {
            let rotated = format!("mock-refresh-token-{count}");
            state
                .inner
                .refresh_tokens
                .lock()
                .await
                .insert(rotated.clone());
            (
                StatusCode::OK,
                Json(json!({
                    "access_token": format!("mock-refresh-access-{count}"),
                    "refresh_token": rotated,
                    "expires_in": state.inner.config.refresh_expires_in,
                    "scope": "openid profile offline_access",
                    "token_type": "Bearer",
                })),
            )
        }
    }
}

fn oauth_error(status: StatusCode, error: &str, description: &str) -> (StatusCode, Json<Value>) {
    (
        status,
        Json(json!({
            "error": error,
            "error_description": description,
        })),
    )
}

fn pkce_s256(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

struct FixtureEnvGuard {
    _guard: OwnedMutexGuard<()>,
    previous_override: Option<String>,
    previous_base_url: Option<String>,
}

impl FixtureEnvGuard {
    async fn install(base_url: &str) -> Self {
        let lock = ENV_LOCK.get_or_init(|| Arc::new(Mutex::new(()))).clone();
        let guard = lock.lock_owned().await;
        let previous_override = env::var(TEST_OVERRIDE_ENV).ok();
        let previous_base_url = env::var(TEST_BASE_URL_ENV).ok();
        // The process environment is global; this guard serializes mutation
        // across the lane so endpoint overrides cannot race other auth tests.
        unsafe {
            env::set_var(TEST_OVERRIDE_ENV, "1");
            env::set_var(TEST_BASE_URL_ENV, base_url);
        }
        Self {
            _guard: guard,
            previous_override,
            previous_base_url,
        }
    }
}

impl Drop for FixtureEnvGuard {
    fn drop(&mut self) {
        // Restore the serialized global test override state.
        unsafe {
            match &self.previous_override {
                Some(value) => env::set_var(TEST_OVERRIDE_ENV, value),
                None => env::remove_var(TEST_OVERRIDE_ENV),
            }
            match &self.previous_base_url {
                Some(value) => env::set_var(TEST_BASE_URL_ENV, value),
                None => env::remove_var(TEST_BASE_URL_ENV),
            }
        }
    }
}

struct AuthHarness {
    _temp: TempDir,
    config: Config,
    token_store: Arc<dyn TokenStore>,
    runtime: Arc<SessionRuntime>,
    router: MethodRouter,
    registry: Arc<meerkat_providers::ProviderRuntimeRegistry>,
}

impl AuthHarness {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        Self::new_with_token_root(temp.path().join("tokens"), temp)
    }

    fn new_for_restart(token_root: PathBuf) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        Self::new_with_token_root(token_root, temp)
    }

    fn new_with_token_root(token_root: PathBuf, temp: TempDir) -> Self {
        let config = openai_oauth_config();
        let token_store: Arc<dyn TokenStore> = Arc::new(FileTokenStore::new(token_root));
        let factory = meerkat::AgentFactory::new(temp.path().join("sessions"))
            .with_token_store(token_store.clone());
        let registry = factory.provider_runtime_registry();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let config_store: Arc<dyn ConfigStore> = Arc::new(MemoryConfigStore::new(config.clone()));
        let mut runtime = SessionRuntime::new(
            factory,
            config.clone(),
            10,
            meerkat::PersistenceBundle::new(store, None, blob_store),
            NotificationSink::noop(),
        );
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            config_store.clone(),
            temp.path().join("config_state.json"),
        )));
        let runtime = Arc::new(runtime);
        let router = MethodRouter::new(runtime.clone(), config_store, NotificationSink::noop());
        Self {
            _temp: temp,
            config,
            token_store,
            runtime,
            router,
            registry,
        }
    }

    fn token_key(&self) -> TokenKey {
        TokenKey::from_auth_binding(&openai_auth_binding())
    }

    async fn stored_tokens(&self) -> Option<PersistedTokens> {
        self.token_store
            .load(&self.token_key())
            .await
            .expect("token load")
    }
}

fn openai_oauth_config() -> Config {
    let mut config = Config::default();
    let mut section = RealmConfigSection {
        backend: BTreeMap::new(),
        auth: BTreeMap::new(),
        binding: BTreeMap::new(),
        default_binding: Some(BINDING_ID.to_string()),
    };
    section.backend.insert(
        "chatgpt_backend".to_string(),
        BackendProfileConfig {
            provider: "openai".to_string(),
            backend_kind: "chatgpt_backend".to_string(),
            base_url: None,
            options: Value::Null,
        },
    );
    section.auth.insert(
        "openai_oauth".to_string(),
        AuthProfileConfig {
            provider: "openai".to_string(),
            auth_method: "managed_chatgpt_oauth".to_string(),
            source: CredentialSourceSpec::ManagedStore,
            constraints: AuthConstraints::default(),
            metadata_defaults: AuthMetadataDefaults::default(),
        },
    );
    section.binding.insert(
        BINDING_ID.to_string(),
        ProviderBindingConfig {
            backend_profile: "chatgpt_backend".to_string(),
            auth_profile: "openai_oauth".to_string(),
            default_model: Some("gpt-5.4".to_string()),
            policy: BindingPolicy::default(),
        },
    );
    config.realm.insert(REALM_ID.to_string(), section);
    config
}

fn openai_auth_binding() -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse(REALM_ID).expect("valid realm"),
        binding: BindingId::parse(BINDING_ID).expect("valid binding"),
        profile: None,
    }
}

fn openai_realm(config: &Config) -> RealmConnectionSet {
    RealmConnectionSet::from_config(REALM_ID, config.realm.get(REALM_ID).expect("realm config"))
        .expect("valid openai realm")
}

async fn rpc_ok<T: DeserializeOwned>(harness: &AuthHarness, method: &str, params: Value) -> T {
    let response = harness
        .router
        .dispatch(rpc_request(method, params))
        .await
        .expect("rpc response");
    if let Some(error) = response.error {
        panic!("RPC {method} failed unexpectedly: {}", error.message);
    }
    let result = response.result.expect("rpc result");
    serde_json::from_str(result.get()).expect("decode rpc result")
}

async fn rpc_error_message(harness: &AuthHarness, method: &str, params: Value) -> String {
    let response = harness
        .router
        .dispatch(rpc_request(method, params))
        .await
        .expect("rpc response");
    response
        .error
        .map(|error| error.message)
        .unwrap_or_else(|| panic!("RPC {method} succeeded unexpectedly"))
}

fn rpc_request(method: &str, params: Value) -> RpcRequest {
    RpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(RpcId::Num(NEXT_RPC_ID.fetch_add(1, Ordering::SeqCst))),
        method: method.to_string(),
        params: Some(serde_json::value::to_raw_value(&params).expect("raw params")),
    }
}

#[derive(Debug, Deserialize)]
struct LoginStartWire {
    authorize_url: String,
    state: String,
    redirect_uri: String,
    provider: String,
}

#[derive(Debug, Deserialize)]
struct LoginReadyWire {
    provider: String,
    has_refresh_token: bool,
    scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AuthStatusWire {
    state: String,
    has_refresh_token: bool,
}

#[derive(Debug, Clone)]
struct BrowserCallback {
    code: String,
    state: String,
}

async fn start_login(harness: &AuthHarness) -> LoginStartWire {
    rpc_ok(
        harness,
        "auth/login/start",
        json!({
            "provider": OPENAI_PROVIDER,
            "redirect_uri": REDIRECT_URI,
            "realm_id": REALM_ID,
            "binding_id": BINDING_ID,
        }),
    )
    .await
}

async fn follow_authorize_redirect(start: &LoginStartWire) -> BrowserCallback {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest client");
    let response = client
        .get(&start.authorize_url)
        .send()
        .await
        .expect("fixture authorize response");
    assert!(
        response.status().is_redirection(),
        "authorize should redirect, got {}",
        response.status()
    );
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .expect("redirect location")
        .to_str()
        .expect("location header utf8");
    let callback = reqwest::Url::parse(location).expect("callback url");
    let pairs: HashMap<String, String> = callback.query_pairs().into_owned().collect();
    BrowserCallback {
        code: pairs.get("code").expect("callback code").clone(),
        state: pairs.get("state").expect("callback state").clone(),
    }
}

async fn complete_login(harness: &AuthHarness, callback: &BrowserCallback) -> LoginReadyWire {
    rpc_ok(
        harness,
        "auth/login/complete",
        json!({
            "provider": OPENAI_PROVIDER,
            "code": callback.code,
            "state": callback.state,
            "redirect_uri": REDIRECT_URI,
            "realm_id": REALM_ID,
            "binding_id": BINDING_ID,
        }),
    )
    .await
}

async fn login_through_browser(harness: &AuthHarness) -> (LoginStartWire, BrowserCallback) {
    let start = start_login(harness).await;
    assert_eq!(start.provider, OPENAI_PROVIDER);
    assert_eq!(start.redirect_uri, REDIRECT_URI);
    let callback = follow_authorize_redirect(&start).await;
    let ready = complete_login(harness, &callback).await;
    assert_eq!(ready.provider, OPENAI_PROVIDER);
    assert!(ready.has_refresh_token);
    assert!(
        ready.scopes.iter().any(|scope| scope == "offline_access"),
        "OAuth login should persist the fixture scopes"
    );
    (start, callback)
}

async fn auth_status(harness: &AuthHarness) -> AuthStatusWire {
    rpc_ok(
        harness,
        "auth/status/get",
        json!({
            "realm_id": REALM_ID,
            "binding_id": BINDING_ID,
        }),
    )
    .await
}

async fn resolve_openai_binding(
    harness: &AuthHarness,
    force_refresh: bool,
    coordinator: Option<Arc<dyn RefreshCoordinator>>,
) -> Result<meerkat_providers::ResolvedConnection, meerkat_providers::ProviderAuthError> {
    let mut env = ResolverEnvironment::testing()
        .with_token_store(harness.token_store.clone())
        .with_auth_lease_handle(harness.runtime.auth_lease_handle())
        .with_force_refresh(force_refresh);
    if let Some(coordinator) = coordinator {
        env = env.with_refresh_coordinator(coordinator);
    }
    harness
        .registry
        .resolve(&openai_realm(&harness.config), &openai_auth_binding(), &env)
        .await
}

#[tokio::test]
#[ignore = "lane:e2e-auth"]
async fn auth_mock_browser_oauth_login_round_trip() {
    let fixture = MockOAuthProvider::start(MockOAuthConfig::default()).await;
    let _env = FixtureEnvGuard::install(fixture.base_url()).await;
    let harness = AuthHarness::new();

    login_through_browser(&harness).await;
    let status = auth_status(&harness).await;
    assert_eq!(status.state, "valid");
    assert!(status.has_refresh_token);

    let stored = harness.stored_tokens().await.expect("stored tokens");
    assert_eq!(stored.primary_secret.as_deref(), Some("mock-access-1"));
    assert!(meerkat_core::tokens_lifecycle_published(&stored));

    let resolved = resolve_openai_binding(&harness, false, None)
        .await
        .expect("binding resolves after login");
    assert_eq!(resolved.resolved_secret().as_deref(), Some("mock-access-1"));
}

#[tokio::test]
#[ignore = "lane:e2e-auth"]
async fn auth_mock_browser_oauth_rejects_wrong_state() {
    let fixture = MockOAuthProvider::start(MockOAuthConfig {
        callback_state: CallbackStateMode::WrongState,
        ..MockOAuthConfig::default()
    })
    .await;
    let _env = FixtureEnvGuard::install(fixture.base_url()).await;
    let harness = AuthHarness::new();

    let start = start_login(&harness).await;
    let callback = follow_authorize_redirect(&start).await;
    assert_ne!(callback.state, start.state);
    let message = rpc_error_message(
        &harness,
        "auth/login/complete",
        json!({
            "provider": OPENAI_PROVIDER,
            "code": callback.code,
            "state": callback.state,
            "redirect_uri": REDIRECT_URI,
            "realm_id": REALM_ID,
            "binding_id": BINDING_ID,
        }),
    )
    .await;
    assert!(
        message.contains("oauth state is missing or expired")
            || message.contains("oauth state verification failed"),
        "unexpected wrong-state error: {message}"
    );
    assert!(
        harness.stored_tokens().await.is_none(),
        "wrong-state login must not persist credentials"
    );
}

#[tokio::test]
#[ignore = "lane:e2e-auth"]
async fn auth_mock_browser_oauth_rejects_double_complete() {
    let fixture = MockOAuthProvider::start(MockOAuthConfig::default()).await;
    let _env = FixtureEnvGuard::install(fixture.base_url()).await;
    let harness = AuthHarness::new();

    let (_, callback) = login_through_browser(&harness).await;
    let message = rpc_error_message(
        &harness,
        "auth/login/complete",
        json!({
            "provider": OPENAI_PROVIDER,
            "code": callback.code,
            "state": callback.state,
            "redirect_uri": REDIRECT_URI,
            "realm_id": REALM_ID,
            "binding_id": BINDING_ID,
        }),
    )
    .await;
    assert!(
        message.contains("oauth state is missing or expired")
            || message.contains("oauth state verification failed"),
        "unexpected double-complete error: {message}"
    );
}

#[tokio::test]
#[ignore = "lane:e2e-auth"]
async fn auth_mock_oauth_expired_token_refreshes_and_persists() {
    let fixture = MockOAuthProvider::start(MockOAuthConfig {
        initial_expires_in: 0,
        refresh_expires_in: 3_600,
        ..MockOAuthConfig::default()
    })
    .await;
    let _env = FixtureEnvGuard::install(fixture.base_url()).await;
    let harness = AuthHarness::new();

    login_through_browser(&harness).await;
    let resolved =
        resolve_openai_binding(&harness, false, Some(Arc::new(InMemoryCoordinator::new())))
            .await
            .expect("expired OAuth token refreshes during resolve");
    assert_eq!(
        resolved.resolved_secret().as_deref(),
        Some("mock-refresh-access-1")
    );
    assert_eq!(fixture.refresh_count(), 1);

    let stored = harness
        .stored_tokens()
        .await
        .expect("stored refreshed tokens");
    assert_eq!(
        stored.primary_secret.as_deref(),
        Some("mock-refresh-access-1")
    );
    assert!(meerkat_core::tokens_lifecycle_published(&stored));
    let status = auth_status(&harness).await;
    assert_eq!(status.state, "valid");
    assert!(status.has_refresh_token);
}

#[tokio::test]
#[ignore = "lane:e2e-auth"]
async fn auth_mock_oauth_failed_refresh_surfaces_reauth_required_or_explicit_auth_error() {
    let fixture = MockOAuthProvider::start(MockOAuthConfig {
        initial_expires_in: 0,
        refresh_mode: RefreshMode::Failed,
        ..MockOAuthConfig::default()
    })
    .await;
    let _env = FixtureEnvGuard::install(fixture.base_url()).await;
    let harness = AuthHarness::new();

    login_through_browser(&harness).await;
    let err = resolve_openai_binding(&harness, false, Some(Arc::new(InMemoryCoordinator::new())))
        .await
        .expect_err("refresh failure must surface an auth error");
    let detail = err.to_string();
    assert!(
        detail.contains("UserReauthRequired")
            || detail.contains("reauth")
            || detail.contains("invalid_grant")
            || detail.contains("refresh token revoked")
            || detail.contains("RefreshFailed"),
        "unexpected refresh failure detail: {detail}"
    );
    let status = auth_status(&harness).await;
    assert!(
        matches!(status.state.as_str(), "reauth_required" | "expiring"),
        "failed permanent refresh should require reauth or remain explicit expiring, got {}",
        status.state
    );
}

#[tokio::test]
#[ignore = "lane:e2e-auth"]
async fn auth_mock_oauth_restart_rehydrates_status_without_fabricating_from_raw_bytes() {
    let fixture = MockOAuthProvider::start(MockOAuthConfig::default()).await;
    let _env = FixtureEnvGuard::install(fixture.base_url()).await;
    let token_temp = tempfile::tempdir().expect("token temp");
    let token_root = token_temp.path().join("tokens");
    {
        let harness = AuthHarness::new_for_restart(token_root.clone());
        login_through_browser(&harness).await;
        assert!(harness.stored_tokens().await.is_some());
        let status = auth_status(&harness).await;
        assert_eq!(status.state, "valid");
    }

    let restarted = AuthHarness::new_for_restart(token_root);
    assert!(
        restarted.stored_tokens().await.is_some(),
        "restart test requires raw persisted token bytes"
    );
    let status = auth_status(&restarted).await;
    assert_eq!(status.state, "unknown");
    assert!(
        !status.has_refresh_token,
        "status must not expose refresh material when AuthMachine lifecycle is absent"
    );
}

#[tokio::test]
#[ignore = "lane:e2e-auth"]
async fn auth_mock_oauth_logout_clears_token_and_releases_authmachine_lifecycle() {
    let fixture = MockOAuthProvider::start(MockOAuthConfig::default()).await;
    let _env = FixtureEnvGuard::install(fixture.base_url()).await;
    let harness = AuthHarness::new();

    login_through_browser(&harness).await;
    assert!(harness.stored_tokens().await.is_some());
    let _: Value = rpc_ok(
        &harness,
        "auth/logout",
        json!({
            "realm_id": REALM_ID,
            "binding_id": BINDING_ID,
        }),
    )
    .await;
    assert!(
        harness.stored_tokens().await.is_none(),
        "logout should clear token store material"
    );
    let status = auth_status(&harness).await;
    assert_eq!(status.state, "unknown");
    assert!(!status.has_refresh_token);
    let lease_key = LeaseKey::from_auth_binding(&openai_auth_binding());
    let snapshot = harness.runtime.auth_lease_handle().snapshot(&lease_key);
    assert_eq!(snapshot.phase, None);
    assert!(!snapshot.credential_present);
}

#[tokio::test]
#[ignore = "lane:e2e-auth"]
async fn auth_mock_concurrent_resolve_dedupes_refresh() {
    let fixture = MockOAuthProvider::start(MockOAuthConfig {
        initial_expires_in: 0,
        refresh_delay: Duration::from_millis(100),
        ..MockOAuthConfig::default()
    })
    .await;
    let _env = FixtureEnvGuard::install(fixture.base_url()).await;
    let harness = AuthHarness::new();
    login_through_browser(&harness).await;

    let coordinator: Arc<dyn RefreshCoordinator> = Arc::new(InMemoryCoordinator::new());
    let realm = Arc::new(openai_realm(&harness.config));
    let mut tasks = Vec::new();
    for _ in 0..5 {
        let registry = harness.registry.clone();
        let realm = realm.clone();
        let store = harness.token_store.clone();
        let auth_lease = harness.runtime.auth_lease_handle();
        let coordinator = coordinator.clone();
        tasks.push(tokio::spawn(async move {
            let env = ResolverEnvironment::testing()
                .with_token_store(store)
                .with_auth_lease_handle(auth_lease)
                .with_refresh_coordinator(coordinator);
            registry
                .resolve(&realm, &openai_auth_binding(), &env)
                .await
                .map(|connection| connection.resolved_secret())
        }));
    }
    let results = join_all(tasks).await;
    for result in results {
        let secret = result
            .expect("task join")
            .expect("concurrent resolve succeeds");
        assert_eq!(secret.as_deref(), Some("mock-refresh-access-1"));
    }
    assert_eq!(
        fixture.refresh_count(),
        1,
        "concurrent resolve should dispatch one refresh"
    );
}

#[tokio::test]
#[ignore = "lane:e2e-auth"]
async fn auth_mock_oauth_fixture_can_return_bad_code_and_malformed_token_errors() {
    for (config, expected) in [
        (
            MockOAuthConfig {
                token_mode: TokenMode::BadCode,
                ..MockOAuthConfig::default()
            },
            "token endpoint returned",
        ),
        (
            MockOAuthConfig {
                token_mode: TokenMode::Malformed,
                ..MockOAuthConfig::default()
            },
            "Token exchange failed",
        ),
    ] {
        let fixture = MockOAuthProvider::start(config).await;
        let _env = FixtureEnvGuard::install(fixture.base_url()).await;
        let harness = AuthHarness::new();
        let start = start_login(&harness).await;
        let callback = follow_authorize_redirect(&start).await;
        let message = rpc_error_message(
            &harness,
            "auth/login/complete",
            json!({
                "provider": OPENAI_PROVIDER,
                "code": callback.code,
                "state": callback.state,
                "redirect_uri": REDIRECT_URI,
                "realm_id": REALM_ID,
                "binding_id": BINDING_ID,
            }),
        )
        .await;
        assert!(
            message.contains(expected),
            "expected {expected:?} in {message:?}"
        );
    }
}

#[tokio::test]
#[ignore = "lane:e2e-auth"]
async fn auth_mock_oauth_fixture_can_return_malformed_refresh_response() {
    let fixture = MockOAuthProvider::start(MockOAuthConfig {
        initial_expires_in: 0,
        refresh_mode: RefreshMode::Malformed,
        ..MockOAuthConfig::default()
    })
    .await;
    let _env = FixtureEnvGuard::install(fixture.base_url()).await;
    let harness = AuthHarness::new();
    login_through_browser(&harness).await;
    let err = resolve_openai_binding(&harness, false, Some(Arc::new(InMemoryCoordinator::new())))
        .await
        .expect_err("malformed refresh should fail");
    assert!(
        err.to_string().contains("decode"),
        "unexpected malformed refresh error: {err}"
    );
}

#[tokio::test]
#[ignore = "lane:e2e-auth"]
async fn auth_live_canary_openai_oauth_seeded_token_bundle() {
    let Some(tokens) = seeded_openai_oauth_tokens() else {
        eprintln!("SKIP: MEERKAT_E2E_AUTH_OPENAI_OAUTH_TOKENS_JSON is not configured");
        return;
    };
    assert!(
        tokens.refresh_token.is_some(),
        "configured OpenAI OAuth token bundle must include refresh_token"
    );
    let harness = AuthHarness::new();
    save_seeded_tokens_with_lifecycle(&harness, &tokens).await;
    let resolved =
        resolve_openai_binding(&harness, true, Some(Arc::new(InMemoryCoordinator::new())))
            .await
            .expect("configured OpenAI OAuth seeded refresh must pass");
    assert!(
        resolved.resolved_secret().is_some(),
        "OpenAI OAuth seeded refresh resolved without access material"
    );
}

#[tokio::test]
#[ignore = "lane:e2e-auth"]
async fn auth_live_canary_anthropic_api() {
    let Some(api_key) = first_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]) else {
        eprintln!("SKIP: no Anthropic API key configured");
        return;
    };
    let client = AnthropicClient::new(api_key).expect("AnthropicClient::new");
    run_tiny_chat(
        &client,
        &first_env(&["MEERKAT_E2E_AUTH_ANTHROPIC_MODEL"])
            .unwrap_or_else(|| "claude-sonnet-4-6".to_string()),
    )
    .await
    .expect("configured Anthropic API key must pass tiny auth canary");
}

#[tokio::test]
#[ignore = "lane:e2e-auth"]
async fn auth_live_canary_gemini_api() {
    let Some(api_key) = first_env(&["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"])
    else {
        eprintln!("SKIP: no Gemini API key configured");
        return;
    };
    let client = GeminiClient::new(api_key);
    run_tiny_chat(
        &client,
        &first_env(&["MEERKAT_E2E_AUTH_GEMINI_MODEL"])
            .unwrap_or_else(|| "gemini-2.0-flash".to_string()),
    )
    .await
    .expect("configured Gemini API key must pass tiny auth canary");
}

#[tokio::test]
#[ignore = "lane:e2e-auth"]
async fn auth_live_canary_gemini_vertex_adc() {
    if env::var("MEERKAT_E2E_AUTH_VERTEX_CANARY").is_err()
        && env::var("GOOGLE_APPLICATION_CREDENTIALS").is_err()
    {
        eprintln!(
            "SKIP: Gemini Vertex ADC canary requires MEERKAT_E2E_AUTH_VERTEX_CANARY=1 or GOOGLE_APPLICATION_CREDENTIALS"
        );
        return;
    }
    let authorizer = GoogleAuthAuthorizer::with_process_env(GoogleAuthChain::Default);
    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://aiplatform.googleapis.com/v1/projects/-/locations/-",
        headers: &mut headers,
    };
    authorizer
        .authorize(&mut req)
        .await
        .expect("configured Gemini Vertex ADC credentials must mint a bearer token");
    assert!(
        headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("authorization") && value.starts_with("Bearer ")
        }),
        "Gemini Vertex ADC canary did not produce an Authorization bearer header"
    );
}

fn first_env(vars: &[&str]) -> Option<String> {
    vars.iter().find_map(|name| match env::var(name) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    })
}

fn seeded_openai_oauth_tokens() -> Option<PersistedTokens> {
    let raw = first_env(&["MEERKAT_E2E_AUTH_OPENAI_OAUTH_TOKENS_JSON"])?;
    let content = if let Some(path) = raw.strip_prefix('@') {
        std::fs::read_to_string(path)
            .unwrap_or_else(|err| panic!("read OpenAI OAuth token bundle {path}: {err}"))
    } else if Path::new(&raw).exists() {
        std::fs::read_to_string(&raw)
            .unwrap_or_else(|err| panic!("read OpenAI OAuth token bundle {raw}: {err}"))
    } else {
        raw
    };
    Some(
        serde_json::from_str(&content)
            .unwrap_or_else(|err| panic!("parse OpenAI OAuth token bundle JSON: {err}")),
    )
}

async fn save_seeded_tokens_with_lifecycle(harness: &AuthHarness, tokens: &PersistedTokens) {
    let lease_key = LeaseKey::from_auth_binding(&openai_auth_binding());
    let transition = harness
        .runtime
        .auth_lease_handle()
        .acquire_lease(
            &lease_key,
            meerkat_core::persisted_token_expires_at_epoch_secs(tokens),
        )
        .expect("seed AuthMachine lifecycle");
    let marked = meerkat_core::mark_tokens_lifecycle_published_for_transition(tokens, transition);
    harness
        .token_store
        .save(&harness.token_key(), &marked)
        .await
        .expect("save seeded tokens");
}

async fn run_tiny_chat(client: &dyn LlmClient, model: &str) -> Result<String, String> {
    let request = LlmRequest::new(
        model,
        vec![Message::User(UserMessage::text(
            "Respond with exactly the word: ready".to_string(),
        ))],
    )
    .with_max_tokens(32);
    let mut stream = client.stream(&request);
    let mut text = String::new();
    while let Some(event) = stream.next().await {
        match event {
            Ok(LlmEvent::TextDelta { delta, .. }) => text.push_str(&delta),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { .. },
            }) => {
                if text.trim().is_empty() {
                    return Err("provider returned empty text".to_string());
                }
                return Ok(text);
            }
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Error { error },
            }) => return Err(format!("provider Done(Error): {error:?}")),
            Ok(_) => {}
            Err(err) => return Err(format!("stream error: {err:?}")),
        }
    }
    Err("stream ended without Done".to_string())
}
