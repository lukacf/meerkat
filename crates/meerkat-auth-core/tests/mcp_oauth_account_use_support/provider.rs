//! Literal-loopback provider for the native selected-account owner regression.
//! Only provider protocol state lives here; native stores own all host state.
use axum::{
    Form, Json, Router,
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::{sync::oneshot, task::JoinHandle};

pub const ACCOUNTS: [&str; 2] = ["subject-a", "subject-b"];
pub const SCOPES: [&str; 2] = ["profile", "protected:read"];
const CLIENT: &str = "registered-local-fixture";

pub struct SelectedProfile {
    pub issuer: String,
    pub client: String,
    pub resource: String,
    pub scopes: BTreeSet<String>,
    pub expected_account: String,
    pub strategy_id: String,
}

pub fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .build()
        .unwrap()
}

struct Grant {
    account: usize,
    redirect: String,
    challenge: String,
    scopes: BTreeSet<String>,
}
struct Token {
    account: usize,
    scopes: BTreeSet<String>,
}
#[derive(Default)]
struct ProviderVault {
    codes: HashMap<String, Grant>,
    tokens: HashMap<String, Token>,
}
struct ProviderState {
    base: String,
    vault: Mutex<ProviderVault>,
    registered_redirects: Mutex<BTreeSet<String>>,
}

async fn authorize(
    State(state): State<Arc<ProviderState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let Some(account) = query
        .get("fixture_account")
        .and_then(|value| ACCOUNTS.iter().position(|account| account == value))
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let scopes: BTreeSet<String> = query
        .get("scope")
        .map(|value| value.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
    let Some(redirect) = query
        .get("redirect_uri")
        .and_then(|value| reqwest::Url::parse(value).ok())
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if query.get("client_id").map(String::as_str) != Some(CLIENT)
        || query.get("resource") != Some(&format!("{}/mcp", state.base))
        || query.get("response_type").map(String::as_str) != Some("code")
        || query.get("code_challenge_method").map(String::as_str) != Some("S256")
        || query.get("code_challenge").is_none_or(String::is_empty)
        || query.get("state").is_none_or(String::is_empty)
        || scopes != SCOPES.iter().map(|value| (*value).to_owned()).collect()
        || redirect.scheme() != "http"
        || redirect.host_str() != Some("127.0.0.1")
        || redirect.port().is_none()
        || redirect.path() != "/mcp/oauth/callback"
        || redirect.query().is_some()
        || redirect.fragment().is_some()
        || !state
            .registered_redirects
            .lock()
            .unwrap()
            .contains(redirect.as_str())
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let code = format!("fixture-code-{}", uuid::Uuid::new_v4());
    let mut granted = scopes;
    if query.get("fixture_partial").map(String::as_str) == Some("yes") {
        granted.remove("protected:read");
    }
    let mut vault = state.vault.lock().unwrap();
    vault.codes.insert(
        code.clone(),
        Grant {
            account,
            redirect: redirect.to_string(),
            challenge: query["code_challenge"].clone(),
            scopes: granted,
        },
    );
    drop(vault);
    let mut location = redirect;
    location
        .query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("state", &query["state"]);
    (StatusCode::FOUND, [("location", location.to_string())]).into_response()
}
async fn exchange(
    State(state): State<Arc<ProviderState>>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let mut vault = state.vault.lock().unwrap();
    let Some(code) = form.get("code") else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(grant) = vault.codes.remove(code) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(verifier) = form.get("code_verifier") else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if form.get("grant_type").map(String::as_str) != Some("authorization_code")
        || form.get("client_id").map(String::as_str) != Some(CLIENT)
        || form.get("resource") != Some(&format!("{}/mcp", state.base))
        || form.get("redirect_uri") != Some(&grant.redirect)
        || URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) != grant.challenge
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let access = format!("fixture-access-{}", uuid::Uuid::new_v4());
    let refresh = format!("fixture-refresh-{}", uuid::Uuid::new_v4());
    let id = format!("fixture-id-{}", uuid::Uuid::new_v4());
    let scope = grant.scopes.iter().cloned().collect::<Vec<_>>().join(" ");
    vault.tokens.insert(
        access.clone(),
        Token {
            account: grant.account,
            scopes: grant.scopes,
        },
    );
    Json(json!({"access_token":access,"refresh_token":refresh,"id_token":id,"token_type":"Bearer","expires_in":3600,"scope":scope})).into_response()
}
async fn account(State(state): State<Arc<ProviderState>>, headers: HeaderMap) -> Response {
    let Some(token) = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let vault = state.vault.lock().unwrap();
    let Some(token) = vault.tokens.get(token) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    Json(json!({"account":ACCOUNTS[token.account], "scopes":token.scopes})).into_response()
}

// Actual native MCP discovery and public-client registration fixture routes.
async fn protected_resource(State(state): State<Arc<ProviderState>>) -> Response {
    Json(json!({
        "resource": format!("{}/mcp", state.base),
        "authorization_servers": [state.base],
        "scopes_supported": SCOPES,
    }))
    .into_response()
}

async fn authorization_metadata(State(state): State<Arc<ProviderState>>) -> Response {
    Json(json!({
        "issuer": state.base,
        "authorization_endpoint": format!("{}/authorize", state.base),
        "token_endpoint": format!("{}/token", state.base),
        "registration_endpoint": format!("{}/register", state.base),
        "code_challenge_methods_supported": ["S256"],
    }))
    .into_response()
}

async fn register(State(state): State<Arc<ProviderState>>, Json(body): Json<Value>) -> Response {
    let Some(redirects) = body.get("redirect_uris").and_then(Value::as_array) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if redirects.len() != 1
        || body
            .get("token_endpoint_auth_method")
            .and_then(Value::as_str)
            != Some("none")
        || body.get("grant_types") != Some(&json!(["authorization_code", "refresh_token"]))
        || body.get("response_types") != Some(&json!(["code"]))
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(redirect) = redirects[0]
        .as_str()
        .and_then(|value| reqwest::Url::parse(value).ok())
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if redirect.scheme() != "http"
        || redirect.host_str() != Some("127.0.0.1")
        || redirect.port().is_none()
        || redirect.path() != "/mcp/oauth/callback"
        || !redirect.username().is_empty()
        || redirect.password().is_some()
        || redirect.query().is_some()
        || redirect.fragment().is_some()
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    state
        .registered_redirects
        .lock()
        .unwrap()
        .insert(redirect.to_string());
    Json(json!({"client_id": CLIENT, "token_endpoint_auth_method": "none"})).into_response()
}

pub struct Fixture {
    pub base: String,
    http: Option<JoinHandle<Result<(), std::io::Error>>>,
    stop: Option<oneshot::Sender<()>>,
}
impl Fixture {
    pub async fn start() -> Self {
        // Both fallible listener operations precede the first task acquisition.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(ProviderState {
            base: base.clone(),
            vault: Mutex::new(ProviderVault::default()),
            registered_redirects: Mutex::new(BTreeSet::new()),
        });
        let router = Router::new()
            .route(
                "/.well-known/oauth-protected-resource/mcp",
                get(protected_resource),
            )
            .route(
                "/.well-known/oauth-authorization-server",
                get(authorization_metadata),
            )
            .route("/register", post(register))
            .route("/authorize", get(authorize))
            .route("/token", post(exchange))
            .route("/account", get(account))
            .layer(DefaultBodyLimit::max(16 * 1024))
            .with_state(state);
        let (stop, stopped) = oneshot::channel();
        let http = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        });
        Self {
            base,
            http: Some(http),
            stop: Some(stop),
        }
    }
    pub fn profile(&self, account: usize) -> SelectedProfile {
        SelectedProfile {
            issuer: self.base.clone(),
            client: CLIENT.into(),
            resource: format!("{}/mcp", self.base),
            scopes: SCOPES.iter().map(|scope| (*scope).into()).collect(),
            expected_account: ACCOUNTS[account].into(),
            strategy_id: "fixture-authenticated-account-v1".into(),
        }
    }
    pub async fn close(&mut self) -> Vec<String> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let mut errors = Vec::new();
        // Retain the handle through its await. Emergency Drop signals shutdown
        // but only this explicit close supplies a joined-owner receipt.
        if let Some(task) = self.http.as_mut() {
            if !matches!(task.await, Ok(Ok(()))) {
                errors.push("provider HTTP join failed".into());
            }
            self.http.take();
        }
        errors
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}
