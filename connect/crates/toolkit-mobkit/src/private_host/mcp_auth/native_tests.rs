//! Real Core registered OAuth through the hosted endpoint, with an isolated
//! loopback provider and no browser, external account or alternate flow owner.
//! These controls use Core's own completion, cancellation and credential owner.

use super::*;
use crate::private_host::{ConsoleLoginSelection, ConsoleLogins, PrivateHostClosed};
use axum::{
    Form, Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures::FutureExt;
use meerkat::{HostMcpAuthPhase, McpAdapterCustody, McpRegisteredRedirectBase, TokenStoreBackend};
use meerkat_core::{
    McpServerConfig,
    auth::{
        PersistedTokens, TokenKey, TokenStore, TokenStoreAccess, TokenStoreError,
        TokenStoreObservation, TokenStoreRefusal,
    },
    mcp_config::McpTransportConfig,
};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, panic::AssertUnwindSafe, time::Duration};
use tokio::{sync::Notify, task::JoinSet};

const SUBJECT: &str = "loopback-fixture-subject";
const CODE: &str = "loopback-fixture-code";
const ACCESS: &str = "loopback-fixture-access";
const WATCHDOG: Duration = Duration::from_secs(15);
// A test observation window, not a new product timeout or cleanup guarantee.
const PENDING_WINDOW: Duration = Duration::from_millis(40);

struct Gate {
    hold: bool,
    entered: Notify,
    release: Semaphore,
}

impl Gate {
    fn new(hold: bool) -> Self {
        Self {
            hold,
            entered: Notify::new(),
            release: Semaphore::new(0),
        }
    }

    async fn enter(&self) {
        self.entered.notify_one();
        if self.hold {
            self.release.acquire().await.unwrap().forget();
        }
    }

    fn release(&self) {
        self.release.add_permits(1);
    }
}

/// Decorates the real ephemeral store only at its actual save boundary. The
/// original Core coordinator still owns, serializes and settles the commit.
struct SaveBarrier {
    inner: Arc<dyn TokenStore>,
    gate: Arc<Gate>,
}

#[async_trait::async_trait]
impl TokenStore for SaveBarrier {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        self.inner.load(key).await
    }
    async fn save(
        &self,
        fence: &meerkat_core::AdmissionGuard,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        self.gate.enter().await;
        self.inner.save(fence, key, tokens).await
    }
    async fn clear(
        &self,
        fence: &meerkat_core::AdmissionGuard,
        key: &TokenKey,
    ) -> Result<(), TokenStoreError> {
        self.inner.clear(fence, key).await
    }
    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        self.inner.list().await
    }
    async fn observe_status(
        &self,
        key: &TokenKey,
    ) -> Result<TokenStoreObservation, TokenStoreError> {
        self.inner.observe_status(key).await
    }
    fn backend_name(&self) -> &'static str {
        "hosted-fixture-save-barrier"
    }
}

// Hold the actual unattended client-profile read used by native Describe.
// The missing-profile answer comes from the real store; the error case injects
// a typed store refusal. Neither case starts OAuth or contacts a provider.
struct DescribeReadBarrier {
    inner: Arc<dyn TokenStore>,
    gate: Arc<Gate>,
    refuse: bool,
    reads: Mutex<Vec<(TokenKey, TokenStoreAccess)>>,
    writes: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl TokenStore for DescribeReadBarrier {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        self.inner.load(key).await
    }
    async fn load_for(
        &self,
        key: &TokenKey,
        access: TokenStoreAccess,
    ) -> Result<Option<PersistedTokens>, TokenStoreError> {
        self.reads.lock().unwrap().push((key.clone(), access));
        self.gate.enter().await;
        if self.refuse {
            return Err(TokenStoreError::refused(
                self.backend_name(),
                access,
                TokenStoreRefusal::Denied,
                "private-describe-store-detail",
            ));
        }
        self.inner.load_for(key, access).await
    }
    async fn save(
        &self,
        fence: &meerkat_core::AdmissionGuard,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.save(fence, key, tokens).await
    }
    async fn clear(
        &self,
        fence: &meerkat_core::AdmissionGuard,
        key: &TokenKey,
    ) -> Result<(), TokenStoreError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.inner.clear(fence, key).await
    }
    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        self.inner.list().await
    }
    async fn observe_status(
        &self,
        key: &TokenKey,
    ) -> Result<TokenStoreObservation, TokenStoreError> {
        self.inner.observe_status(key).await
    }
    fn backend_name(&self) -> &'static str {
        "hosted-describe-read-barrier"
    }
}

#[derive(Clone, Copy)]
enum DescribeLifetime {
    Current,
    Retired,
    PipeClosed,
}

async fn describe_read(refuse: bool, lifetime: DescribeLifetime) {
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let persistence = TokenStoreBackend::Ephemeral
        .open_with_refresh_authority()
        .unwrap();
    let gate = Arc::new(Gate::new(true));
    let store = Arc::new(DescribeReadBarrier {
        inner: persistence.token_store(),
        gate: gate.clone(),
        refuse,
        reads: Mutex::new(Vec::new()),
        writes: std::sync::atomic::AtomicUsize::new(0),
    });
    let realm = meerkat_core::RealmId::parse("hosted-describe-fixture").unwrap();
    let base = McpRegisteredRedirectBase::parse("https://hosted-fixture.invalid/callback").unwrap();
    let service = HostAuthService::new(
        meerkat::ProviderAuthPersistence::new(store.clone(), persistence.refresh_coordinator()),
        runtime.provider_auth_runtime_authority(),
    )
    .with_mcp_realm(realm.clone())
    .with_mcp_registered_redirect_base(base.clone());
    let mut config = McpServerConfig::streamable_http(
        "describe-fixture",
        "https://describe-fixture.invalid/mcp",
        HashMap::new(),
    );
    let McpTransportConfig::Http(http) = &mut config.transport else {
        unreachable!()
    };
    http.oauth_account = Some(SUBJECT.into());
    let target = McpServerIdentity::from_config(&config).unwrap();
    let selection = meerkat::mcp_auth_target_to_wire(&target);
    let expected_redirect = service
        .mcp_oauth_authority()
        .unwrap()
        .registered_redirect_uri(&target, &base)
        .unwrap();
    let binding = target.clone().in_realm(realm).auth_binding_ref().unwrap();
    let expected_key = TokenKey::mcp_oauth_client(binding.realm, binding.binding);
    let endpoint = HostedMcpAuthEndpoint::default();
    endpoint
        .bind(Arc::new(move |_| {
            let service = service.clone();
            let target = target.clone();
            Box::pin(async move { Ok(ResolvedHostedMcpTarget { service, target }) })
        }))
        .unwrap();
    let (pipe, pipe_closed) = watch::channel(false);
    let logins = ConsoleLogins::new(PrivateHostClosed(pipe_closed), None);
    logins
        .enroll(
            7,
            ConsoleLoginSelection {
                issuer: "https://parent-fixture.invalid".into(),
                subject: SUBJECT.into(),
                login_scope_id: "describe-login".into(),
                expires_at_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64
                    + 120_000,
            },
        )
        .unwrap();
    let login = logins.parent_enrollment(7).unwrap();
    let mut tasks = JoinSet::new();
    let checked = AssertUnwindSafe(async {
        request(
            &mut tasks,
            &endpoint,
            &login,
            41,
            HostedMcpCommand::Describe {
                target: selection.clone(),
            },
        );
        tokio::time::timeout(WATCHDOG, gate.entered.notified())
            .await
            .expect("native profile read entered");
        assert_eq!(
            *store.reads.lock().unwrap(),
            vec![(expected_key, TokenStoreAccess::Unattended)]
        );
        assert!(
            tokio::time::timeout(PENDING_WINDOW, tasks.join_next())
                .await
                .is_err(),
            "Describe must await the real native profile read"
        );
        match lifetime {
            DescribeLifetime::Current => {}
            DescribeLifetime::Retired => {
                let retired = logins.retire(7).unwrap().drain().await.unwrap();
                assert!(retired.fenced && retired.reservations_drained && !retired.uncertain);
                assert!(login.check().is_err());
            }
            DescribeLifetime::PipeClosed => {
                pipe.send_replace(true);
                assert!(login.check().is_err());
            }
        }
        gate.release();
        let (id, reply) = next(&mut tasks).await;
        assert_eq!(id, 41);
        match lifetime {
            DescribeLifetime::Current if refuse => {
                let reply = reply.unwrap();
                assert_eq!(
                    reply,
                    json!({"mcp":selection,"auth_error":{"reason":"credential_access_denied"}})
                );
                assert!(!reply.to_string().contains("private-describe-store-detail"));
            }
            DescribeLifetime::Current => assert_eq!(
                reply.unwrap(),
                json!({"mcp":selection,"redirect_uri":expected_redirect})
            ),
            DescribeLifetime::Retired | DescribeLifetime::PipeClosed => {
                assert!(
                    matches!(reply, Err(HostLifetimeError::Denied)),
                    "the same retired login must fence both native success and native error"
                );
            }
        }
        assert_eq!(
            store.reads.lock().unwrap().len(),
            1,
            "no second profile lookup or retry"
        );
        assert_eq!(store.writes.load(Ordering::SeqCst), 0);
        assert!(store.inner.list().await.unwrap().is_empty());
        assert!(
            endpoint.ceremonies.lock().unwrap().is_empty(),
            "Describe admits no OAuth ceremony"
        );
    })
    .catch_unwind()
    .await;
    // Keep and join the exact endpoint task on assertion failure as well.
    gate.release();
    pipe.send_replace(true);
    while tasks.join_next().await.is_some() {}
    let closed = endpoint.close().await;
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
    closed.unwrap();
}

#[tokio::test]
async fn describe_awaits_native_profile_read_and_preserves_success_or_typed_refusal() {
    for refuse in [false, true] {
        describe_read(refuse, DescribeLifetime::Current).await;
    }
}

#[tokio::test]
async fn describe_refuses_late_native_profile_read_after_login_retirement_or_pipe_close() {
    for refuse in [false, true] {
        for lifetime in [DescribeLifetime::Retired, DescribeLifetime::PipeClosed] {
            describe_read(refuse, lifetime).await;
        }
    }
}

#[derive(Deserialize)]
struct Authorization {
    client_id: String,
    redirect_uri: String,
    response_type: String,
    state: String,
    code_challenge: String,
    code_challenge_method: String,
}

struct Provider {
    base: String,
    redirect: Mutex<Option<String>>,
    authorization: Mutex<Option<Authorization>>,
    token_gate: Arc<Gate>,
    token_calls: std::sync::atomic::AtomicUsize,
    userinfo_calls: std::sync::atomic::AtomicUsize,
}

impl Provider {
    // The fixture provider issues a code for the real native authorization
    // request. No HTTP browser navigation to the registered HTTPS host occurs.
    fn callback(&self, started: &Value) -> HostedMcpCommand {
        let uri: Uri = started["authorize_url"].as_str().unwrap().parse().unwrap();
        assert_eq!(uri.scheme_str(), Some("http"));
        assert_eq!(format!("http://{}", uri.authority().unwrap()), self.base);
        assert_eq!(uri.path(), "/authorize");
        let Query(request) = Query::<Authorization>::try_from_uri(&uri).unwrap();
        assert_eq!(request.client_id, "loopback-client");
        assert_eq!(request.response_type, "code");
        assert_eq!(request.code_challenge_method, "S256");
        assert_eq!(
            Some(request.redirect_uri.clone()),
            *self.redirect.lock().unwrap()
        );
        assert_eq!(
            request.redirect_uri,
            started["redirect_uri"].as_str().unwrap()
        );
        assert_eq!(request.state, started["state"].as_str().unwrap());
        let callback = HostedMcpCommand::Complete {
            ceremony_id: started["ceremony_id"].as_u64().unwrap(),
            redirect_uri: request.redirect_uri.clone(),
            state: request.state.clone(),
            code: CODE.into(),
            iss: Some(self.base.clone()),
        };
        assert!(
            self.authorization
                .lock()
                .unwrap()
                .replace(request)
                .is_none()
        );
        callback
    }
}

async fn resource(State(state): State<Arc<Provider>>) -> Json<Value> {
    Json(json!({"resource": format!("{}/mcp", state.base),
        "authorization_servers": [state.base], "scopes_supported": ["openid", "mcp.read"]}))
}

async fn metadata(State(state): State<Arc<Provider>>) -> Json<Value> {
    Json(
        json!({"issuer": state.base, "authorization_endpoint": "/authorize",
        "token_endpoint": "/token", "registration_endpoint": "/register",
        "code_challenge_methods_supported": ["S256"],
        "authorization_response_iss_parameter_supported": true}),
    )
}

async fn openid(State(state): State<Arc<Provider>>) -> Json<Value> {
    Json(json!({"issuer": state.base, "userinfo_endpoint": format!("{}/userinfo", state.base)}))
}

async fn register(State(state): State<Arc<Provider>>, Json(body): Json<Value>) -> Response {
    let Some(redirect) = body["redirect_uris"]
        .as_array()
        .filter(|values| values.len() == 1)
        .and_then(|values| values[0].as_str())
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if !redirect.starts_with("https://hosted-fixture.invalid/callback/") {
        return StatusCode::BAD_REQUEST.into_response();
    }
    *state.redirect.lock().unwrap() = Some(redirect.to_owned());
    Json(json!({"client_id": "loopback-client", "token_endpoint_auth_method": "none"}))
        .into_response()
}

fn challenge(verifier: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let digest = Sha256::digest(verifier.as_bytes());
    let mut out = String::new();
    for bytes in digest.chunks(3) {
        let bits = (u32::from(bytes[0]) << 16)
            | (u32::from(*bytes.get(1).unwrap_or(&0)) << 8)
            | u32::from(*bytes.get(2).unwrap_or(&0));
        for index in 0..bytes.len() + 1 {
            out.push(ALPHABET[((bits >> (18 - index * 6)) & 63) as usize] as char);
        }
    }
    out
}

async fn token(
    State(state): State<Arc<Provider>>,
    Form(body): Form<HashMap<String, String>>,
) -> Response {
    state.token_calls.fetch_add(1, Ordering::SeqCst);
    let valid = state
        .authorization
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|request| {
            body.get("code").map(String::as_str) == Some(CODE)
                && body.get("client_id").map(String::as_str) == Some("loopback-client")
                && body.get("grant_type").map(String::as_str) == Some("authorization_code")
                && body.get("redirect_uri") == Some(&request.redirect_uri)
                && body
                    .get("code_verifier")
                    .is_some_and(|verifier| challenge(verifier) == request.code_challenge)
        });
    if !valid {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid_grant"})),
        )
            .into_response();
    }
    state.token_gate.enter().await;
    Json(
        json!({"access_token": ACCESS, "refresh_token": "loopback-fixture-refresh",
        "token_type": "Bearer", "expires_in": 3600, "scope": "openid mcp.read"}),
    )
    .into_response()
}

// A real streamable HTTP member endpoint. It reads the credential supplied by
// Core's resolver; the fixture never installs a static bearer configuration.
async fn mcp(headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some("Bearer loopback-fixture-access")
    {
        return (StatusCode::UNAUTHORIZED, [("www-authenticate", "Bearer")]).into_response();
    }
    let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let Some(id) = request.get("id").cloned() else {
        return StatusCode::ACCEPTED.into_response();
    };
    let result = match request["method"].as_str() {
        Some("initialize") => json!({"protocolVersion":"2024-11-05", "capabilities":{"tools":{}},
            "serverInfo":{"name":"hosted-fixture", "version":"0.1.0"}}),
        Some("tools/list") => json!({"tools":[{"name":"fixture_read", "description":"Read fixture",
            "inputSchema":{"type":"object", "properties":{}}}]}),
        _ => json!({}),
    };
    Json(json!({"jsonrpc":"2.0", "id":id, "result":result})).into_response()
}

async fn userinfo(State(state): State<Arc<Provider>>, headers: HeaderMap) -> Response {
    state.userinfo_calls.fetch_add(1, Ordering::SeqCst);
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some("Bearer loopback-fixture-access")
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({"sub": SUBJECT})).into_response()
}

type NativeResult = Result<Value, HostLifetimeError>;

fn request(
    tasks: &mut JoinSet<(u64, NativeResult)>,
    endpoint: &HostedMcpAuthEndpoint,
    login: &ConsoleLoginHandle,
    id: u64,
    command: HostedMcpCommand,
) {
    let endpoint = endpoint.clone();
    let login = login.clone();
    tasks.spawn(async move { (id, endpoint.execute(id, login, command).await) });
}

async fn next(tasks: &mut JoinSet<(u64, NativeResult)>) -> (u64, NativeResult) {
    tokio::time::timeout(WATCHDOG, tasks.join_next())
        .await
        .expect("test watchdog; task remains owned for cleanup")
        .unwrap()
        .unwrap()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Complete,
    RetireDuringExchange,
    RetireAfterConsume,
}

async fn exercise(case: Case) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let token_gate = Arc::new(Gate::new(case == Case::RetireDuringExchange));
    let save_gate = Arc::new(Gate::new(case == Case::RetireAfterConsume));
    let provider = Arc::new(Provider {
        base: format!("http://{}", listener.local_addr().unwrap()),
        redirect: Mutex::new(None),
        authorization: Mutex::new(None),
        token_gate: token_gate.clone(),
        token_calls: std::sync::atomic::AtomicUsize::new(0),
        userinfo_calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let app = Router::new()
        .route("/mcp", post(mcp))
        .route("/.well-known/oauth-protected-resource/mcp", get(resource))
        .route("/.well-known/oauth-authorization-server", get(metadata))
        .route("/.well-known/openid-configuration", get(openid))
        .route("/register", post(register))
        .route("/token", post(token))
        .route("/userinfo", get(userinfo))
        .with_state(provider.clone());
    let (stop_server, stopped_server) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = stopped_server.await;
            })
            .await
    });
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let persistence = TokenStoreBackend::Ephemeral
        .open_with_refresh_authority()
        .unwrap();
    let store = persistence.token_store();
    let bound_persistence = meerkat::ProviderAuthPersistence::new(
        Arc::new(SaveBarrier {
            inner: store.clone(),
            gate: save_gate.clone(),
        }),
        persistence.refresh_coordinator(),
    );
    let realm = meerkat_core::RealmId::parse("hosted-mcp-fixture").unwrap();
    let service = HostAuthService::new(
        bound_persistence.clone(),
        runtime.provider_auth_runtime_authority(),
    )
    .with_mcp_realm(realm.clone())
    .with_mcp_registered_redirect_base(
        McpRegisteredRedirectBase::parse("https://hosted-fixture.invalid/callback").unwrap(),
    );
    let mut config = McpServerConfig::streamable_http(
        "hosted-fixture",
        format!("{}/mcp", provider.base),
        HashMap::new(),
    );
    let McpTransportConfig::Http(http) = &mut config.transport else {
        unreachable!()
    };
    http.oauth_account = Some(SUBJECT.into());
    let target = McpServerIdentity::from_config(&config).unwrap();
    let selection = meerkat::mcp_auth_target_to_wire(&target);
    let endpoint = HostedMcpAuthEndpoint::default();
    let resolved_service = service.clone();
    let resolved_target = target.clone();
    let members = MemberMcpCustody::new();
    let mut member = None;
    endpoint
        .bind_parts(
            Arc::new(move |_| {
                let service = resolved_service.clone();
                let target = resolved_target.clone();
                Box::pin(async move { Ok(ResolvedHostedMcpTarget { service, target }) })
            }),
            Some(members.clone()),
        )
        .unwrap();
    let (pipe, pipe_closed) = watch::channel(false);
    let logins = ConsoleLogins::new(PrivateHostClosed(pipe_closed), None);
    let expires_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 120_000;
    for id in [7, 8] {
        logins
            .enroll(
                id,
                ConsoleLoginSelection {
                    issuer: "https://parent-fixture.invalid".into(),
                    subject: SUBJECT.into(),
                    login_scope_id: format!("login-{id}"),
                    expires_at_ms,
                },
            )
            .unwrap();
    }
    let login = logins.parent_enrollment(7).unwrap();
    let other_login = logins.parent_enrollment(8).unwrap();
    let mut tasks = JoinSet::new();
    // On every assertion failure the outer owner releases both barriers, fences
    // the pipe and joins the actual work and server before resuming the panic.
    let checked = AssertUnwindSafe(async {
        if case == Case::Complete {
            let resolver = meerkat::default_mcp_auth_resolver_in_realm(
                Some(bound_persistence.clone()),
                runtime.provider_auth_runtime_authority(),
                realm.clone(),
            )
            .expect("actual native credential resolver");
            let router = meerkat::McpRouter::new_with_surface_handle_and_stdio_profile(
                Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral()),
                meerkat::McpStdioLaunchProfile::trusted_host(),
            )
            .with_mcp_auth(meerkat::McpAuthMode::Interactive, Some(resolver));
            let adapter = Arc::new(meerkat::McpRouterAdapter::new(router));
            // Match the factory seam: adopt and retain cleanup before any add
            // can enter a connection, including on a later assertion failure.
            members.adopt(&adapter).unwrap();
            member = Some(adapter.clone());
            adapter.stage_add(config.clone()).await.unwrap();
            adapter.apply_staged().await.unwrap();
            // The ordinary member is already waiting for this realm's exact
            // account before the hosted login. No test-triggered retry follows.
            tokio::time::timeout(WATCHDOG, async {
                while adapter.servers_awaiting_authorization().await
                    != vec![target.clone().in_realm(realm.clone())]
                {
                    let _ = meerkat_core::AgentToolDispatcher::poll_external_updates(adapter.as_ref()).await;
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("member awaited exact native realm credential");
        }
        request(
            &mut tasks,
            &endpoint,
            &login,
            1,
            HostedMcpCommand::Start {
                target: selection.clone(),
            },
        );
        let (id, started) = next(&mut tasks).await;
        assert_eq!(id, 1);
        let started = started.unwrap();
        assert_eq!(started["disposition"], "started");
        assert_eq!(started["ceremony_id"], 1);
        assert!(started.get("host_attempt_handle").is_none());
        request(
            &mut tasks,
            &endpoint,
            &other_login,
            2,
            HostedMcpCommand::Start {
                target: selection.clone(),
            },
        );
        let joined = next(&mut tasks).await.1.unwrap();
        assert_eq!(joined["disposition"], "joined");
        assert!(joined["ceremony_id"].is_null());
        assert!(joined.get("host_attempt_handle").is_none());
        assert!(matches!(
            endpoint
                .execute(
                    3,
                    other_login.clone(),
                    HostedMcpCommand::Cancel { ceremony_id: 1 }
                )
                .await,
            Err(HostLifetimeError::WrongOwner)
        ));
        assert_eq!(endpoint.ceremonies.lock().unwrap().len(), 1);
        request(
            &mut tasks,
            &endpoint,
            &login,
            4,
            provider.callback(&started),
        );

        if case == Case::Complete {
            let completed = next(&mut tasks).await.1.unwrap();
            assert_eq!(completed["account_id"], SUBJECT);
            assert_eq!(completed["account_verification"], "verified");
            assert_eq!(completed["mcp"], serde_json::to_value(&selection).unwrap());
            assert!(!completed.to_string().contains(ACCESS));
            let cancelled = endpoint
                .execute(
                    5,
                    login.clone(),
                    HostedMcpCommand::Cancel { ceremony_id: 1 },
                )
                .await
                .unwrap();
            assert_eq!(cancelled["cancelled"], false);
            assert_eq!(cancelled["completed"], true);
            let member = member.as_ref().unwrap();
            tokio::time::timeout(WATCHDOG, async {
                loop {
                    let _ =
                        meerkat_core::AgentToolDispatcher::poll_external_updates(member.as_ref())
                            .await;
                    if member.active_server_names().await == ["hosted-fixture"] {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect(
                "completed realm credential reached the existing member without another boundary",
            );
            assert!(
                meerkat_core::AgentToolDispatcher::tools(member.as_ref())
                    .iter()
                    .any(|tool| tool.name.ends_with("fixture_read"))
            );
            assert_eq!(
                endpoint
                    .execute(
                        6,
                        login.clone(),
                        HostedMcpCommand::Release { ceremony_id: 1 }
                    )
                    .await
                    .unwrap(),
                json!({"released":true})
            );
            assert_eq!(
                member.active_server_names().await,
                ["hosted-fixture"],
                "ceremony Cancel/Release must not release the member's credential"
            );
            let retired = logins.retire(7).unwrap().drain().await.unwrap();
            assert!(retired.fenced && retired.reservations_drained && !retired.uncertain);
        } else {
            let gate = if case == Case::RetireDuringExchange {
                &token_gate
            } else {
                &save_gate
            };
            tokio::time::timeout(WATCHDOG, gate.entered.notified())
                .await
                .expect("actual native phase entered");
            let authority = service.mcp_oauth_authority().unwrap();
            assert_eq!(
                authority.pending_attempt(&target).unwrap().is_some(),
                case == Case::RetireDuringExchange
            );
            assert!(store.list().await.unwrap().is_empty());
            // Poll Cancel exactly through its synchronous stop signal, then
            // retire the login before yielding to the native task. The command
            // has entered under the live login, but no late result read can.
            let cancelling_endpoint = endpoint.clone();
            let cancelling_login = login.clone();
            let mut cancel = Box::pin(async move {
                cancelling_endpoint.execute(5, cancelling_login,
                    HostedMcpCommand::Cancel { ceremony_id: 1 }).await
            });
            assert!(futures::poll!(&mut cancel).is_pending());
            let mut retirement = Box::pin(logins.retire(7).unwrap().drain());
            tasks.spawn(async move { (5, cancel.await) });
            assert!(login.check().is_err());
            if case == Case::RetireAfterConsume {
                // Native commit already consumed the attempt. Cancellation
                // cannot undo it; its actual store write stays joined.
                assert!(tokio::time::timeout(PENDING_WINDOW, &mut retirement).await.is_err());
                assert!(tokio::time::timeout(PENDING_WINDOW, tasks.join_next()).await.is_err());
                assert!(endpoint.ceremonies.lock().unwrap().get(&1).unwrap()
                    .outcome.borrow().is_none());
                gate.release();
            }
            // Before consumption, Core's confirmed cancellation now wakes and
            // ends completion without waiting for the provider's token reply.
            let mut outcomes = BTreeMap::new();
            for _ in 0..2 {
                let (id, outcome) = next(&mut tasks).await;
                outcomes.insert(id, outcome);
            }
            let completed = outcomes.remove(&4).unwrap();
            let cancelled = outcomes.remove(&5).unwrap().unwrap();
            let retired = tokio::time::timeout(WATCHDOG, &mut retirement)
                .await
                .unwrap()
                .unwrap();
            assert!(retired.fenced && retired.reservations_drained);
            assert_eq!(cancelled["cancelled"], case == Case::RetireDuringExchange);
            if case == Case::RetireDuringExchange {
                assert!(matches!(completed, Err(HostLifetimeError::NotSettled)));
                assert_eq!(cancelled["completed"], false);
                assert!(!retired.uncertain,
                    "the actual successful native cancel certifies retirement");
                assert!(store.list().await.unwrap().is_empty());
                assert!(authority.pending_attempt(&target).unwrap().is_none());
                assert!(matches!(endpoint.ceremonies.lock().unwrap().get(&1).unwrap()
                    .outcome.borrow().as_ref(), Some(Outcome::NativeFailure { error, cancelled: true })
                    if error.reason == meerkat::contracts::WireAuthErrorReason::AttemptMissing));
                gate.release();
            } else {
                assert!(
                    matches!(completed, Err(HostLifetimeError::Denied)),
                    "retired login cannot read a late committed result"
                );
                assert_eq!(
                    cancelled["completed"], true,
                    "cancel cannot undo an already consumed credential commit"
                );
                assert!(!retired.uncertain);
            }
        }
        assert_eq!(
            provider.token_calls.load(Ordering::SeqCst),
            1,
            "no code replay"
        );
        assert_eq!(provider.userinfo_calls.load(Ordering::SeqCst),
            usize::from(case != Case::RetireDuringExchange));
        let status = service.mcp_status(&target).await.unwrap();
        assert_eq!(
            status.phase,
            if case == Case::RetireDuringExchange {
                HostMcpAuthPhase::AuthorizationRequired
            } else {
                HostMcpAuthPhase::Authorized
            }
        );
        assert_eq!(
            store.list().await.unwrap().len(),
            usize::from(case != Case::RetireDuringExchange)
        );
    })
    .catch_unwind()
    .await;

    token_gate.release();
    save_gate.release();
    pipe.send_replace(true);
    while tasks.join_next().await.is_some() {}
    let closed = endpoint.close().await;
    if let Some(member) = member {
        member.shutdown().await;
    }
    let _ = stop_server.send(());
    server.await.unwrap().unwrap();
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
    closed.unwrap();
}

#[tokio::test]
async fn registered_native_completion_and_joiner_custody_are_exact() {
    exercise(Case::Complete).await;
}

#[tokio::test]
async fn confirmed_native_cancel_ends_exchange_without_waiting_for_provider() {
    exercise(Case::RetireDuringExchange).await;
}

#[tokio::test]
async fn retirement_after_native_consume_does_not_claim_commit_rollback() {
    exercise(Case::RetireAfterConsume).await;
}
