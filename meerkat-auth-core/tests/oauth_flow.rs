//! Integration tests for Phase 4a OAuth helpers:
//! - Loopback callback (success + state mismatch + user-denied)
//! - Token exchange (authorization_code with PKCE; refresh_token)
//! - Device-code flow (request → poll pending → poll success)

#![cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::{Form, State};
use axum::response::Json;
use axum::routing::post;
use reqwest::Client;
use serde::Deserialize;
use tokio::net::TcpListener;

use meerkat_auth_core::auth_oauth::{
    DevicePollOutcome, OAuthEndpoints, OAuthTokenRequestFormat, PkcePair,
    exchange_authorization_code, exchange_authorization_code_with_state, exchange_refresh_token,
    poll_device_code, request_device_code, run_loopback_callback,
};

// --- Loopback callback ------------------------------------------------

#[tokio::test]
async fn loopback_delivers_code_and_state_on_valid_callback() {
    let handle = run_loopback_callback("state-xyz".into(), "/callback")
        .await
        .unwrap();
    let url = handle.redirect_url.clone();
    // Simulate the browser hitting the callback URL.
    let client = Client::new();
    tokio::spawn(async move {
        let _ = client
            .get(format!("{url}?code=auth-code-abc&state=state-xyz"))
            .send()
            .await;
    });
    let outcome = handle.wait(Duration::from_secs(5)).await.unwrap();
    assert_eq!(outcome.code, "auth-code-abc");
    assert_eq!(outcome.state, "state-xyz");
}

#[tokio::test]
async fn loopback_rejects_state_mismatch() {
    let handle = run_loopback_callback("expected".into(), "/callback")
        .await
        .unwrap();
    let url = handle.redirect_url.clone();
    let client = Client::new();
    tokio::spawn(async move {
        let _ = client.get(format!("{url}?code=x&state=other")).send().await;
    });
    let err = handle.wait(Duration::from_secs(5)).await.unwrap_err();
    assert!(
        matches!(
            err,
            meerkat_auth_core::auth_oauth::OAuthError::StateMismatch
        ),
        "got {err:?}"
    );
}

#[tokio::test]
async fn loopback_surfaces_access_denied() {
    let handle = run_loopback_callback("state".into(), "/callback")
        .await
        .unwrap();
    let url = handle.redirect_url.clone();
    let client = Client::new();
    tokio::spawn(async move {
        let _ = client
            .get(format!("{url}?error=access_denied"))
            .send()
            .await;
    });
    let err = handle.wait(Duration::from_secs(5)).await.unwrap_err();
    assert!(
        matches!(err, meerkat_auth_core::auth_oauth::OAuthError::UserDenied),
        "got {err:?}"
    );
}

#[tokio::test]
async fn loopback_times_out_if_no_callback_fires() {
    let handle = run_loopback_callback("state".into(), "/callback")
        .await
        .unwrap();
    let err = handle.wait(Duration::from_millis(200)).await.unwrap_err();
    assert!(
        matches!(err, meerkat_auth_core::auth_oauth::OAuthError::Timeout),
        "got {err:?}"
    );
}

// --- Token exchange ---------------------------------------------------

#[derive(Deserialize, Clone, Debug)]
struct TokenForm {
    grant_type: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    code_verifier: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
}

#[derive(Clone, Default)]
struct TokenMock {
    captured: Arc<Mutex<Vec<TokenForm>>>,
}

async fn token_handler(
    State(state): State<TokenMock>,
    Form(form): Form<TokenForm>,
) -> Json<serde_json::Value> {
    state.captured.lock().unwrap().push(form);
    Json(serde_json::json!({
        "access_token": "access-xyz",
        "refresh_token": "refresh-abc",
        "expires_in": 3600,
        "scope": "read write",
        "id_token": "jwt.payload.sig",
    }))
}

async fn start_token_mock() -> (String, Arc<Mutex<Vec<TokenForm>>>) {
    let state = TokenMock::default();
    let captured = state.captured.clone();
    let app = Router::new()
        .route("/token", post(token_handler))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/token"), captured)
}

#[derive(Clone, Default)]
struct JsonTokenMock {
    captured: Arc<Mutex<Vec<serde_json::Value>>>,
}

async fn json_token_handler(
    State(state): State<JsonTokenMock>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    state.captured.lock().unwrap().push(body);
    Json(serde_json::json!({
        "access_token": "json-access",
        "refresh_token": "json-refresh",
        "expires_in": 3600,
    }))
}

async fn start_json_token_mock() -> (String, Arc<Mutex<Vec<serde_json::Value>>>) {
    let state = JsonTokenMock::default();
    let captured = state.captured.clone();
    let app = Router::new()
        .route("/token", post(json_token_handler))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/token"), captured)
}

fn endpoints(token_url: String) -> OAuthEndpoints {
    OAuthEndpoints {
        client_id: "cid".into(),
        authorize_url: "https://example.com/authorize".into(),
        token_url,
        device_code_url: Some("https://example.com/device".into()),
        redirect_uri: "http://127.0.0.1:0/cb".into(),
        scopes: vec!["read".into(), "write".into()],
        extra_authorize_params: Vec::new(),
        token_request_format: OAuthTokenRequestFormat::FormUrlEncoded,
        include_state_in_token_exchange: false,
        refresh_scopes: Vec::new(),
        extra_headers: Vec::new(),
    }
}

#[tokio::test]
async fn auth_code_exchange_with_pkce_returns_full_bundle() {
    let (token_url, captured) = start_token_mock().await;
    let pkce = PkcePair::generate_s256();
    let result = exchange_authorization_code(
        &Client::new(),
        &endpoints(token_url),
        "code-xyz",
        pkce.verifier.secret(),
        None,
    )
    .await
    .unwrap();

    assert_eq!(result.access_token, "access-xyz");
    assert_eq!(result.refresh_token.as_deref(), Some("refresh-abc"));
    assert_eq!(result.id_token.as_deref(), Some("jwt.payload.sig"));
    assert_eq!(result.expires_in_secs, Some(3600));
    assert_eq!(result.scope.as_deref(), Some("read write"));

    let captured = captured.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].grant_type, "authorization_code");
    assert_eq!(captured[0].code.as_deref(), Some("code-xyz"));
    assert_eq!(
        captured[0].code_verifier.as_deref(),
        Some(pkce.verifier.secret().as_str())
    );
    assert_eq!(captured[0].client_id.as_deref(), Some("cid"));
}

#[tokio::test]
async fn refresh_token_exchange_posts_grant_type_refresh_token() {
    let (token_url, captured) = start_token_mock().await;
    let result = exchange_refresh_token(
        &Client::new(),
        &endpoints(token_url),
        "rt-abc",
        Some("client-secret"),
    )
    .await
    .unwrap();
    assert_eq!(result.access_token, "access-xyz");
    let captured = captured.lock().unwrap();
    assert_eq!(captured[0].grant_type, "refresh_token");
    assert_eq!(captured[0].refresh_token.as_deref(), Some("rt-abc"));
}

#[tokio::test]
async fn json_token_exchange_posts_state_and_refresh_scope() {
    let (token_url, captured) = start_json_token_mock().await;
    let mut endpoints = endpoints(token_url);
    endpoints.token_request_format = OAuthTokenRequestFormat::Json;
    endpoints.include_state_in_token_exchange = true;
    endpoints.refresh_scopes = vec!["org:create_api_key".into(), "user:profile".into()];
    let pkce = PkcePair::generate_s256();

    let missing_state = exchange_authorization_code(
        &Client::new(),
        &endpoints,
        "code-xyz",
        pkce.verifier.secret(),
        None,
    )
    .await
    .expect_err("providers that require token state must reject missing state");
    assert!(matches!(
        missing_state,
        meerkat_auth_core::auth_oauth::OAuthError::InvalidConfig(_)
    ));

    let result = exchange_authorization_code_with_state(
        &Client::new(),
        &endpoints,
        "code-xyz",
        pkce.verifier.secret(),
        None,
        Some("state-abc"),
    )
    .await
    .unwrap();
    assert_eq!(result.access_token, "json-access");

    let refresh_result = exchange_refresh_token(&Client::new(), &endpoints, "refresh-abc", None)
        .await
        .unwrap();
    assert_eq!(refresh_result.access_token, "json-access");

    let captured = captured.lock().unwrap();
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0]["grant_type"], "authorization_code");
    assert_eq!(captured[0]["state"], "state-abc");
    assert_eq!(
        captured[0]["code_verifier"],
        pkce.verifier.secret().as_str()
    );
    assert_eq!(captured[1]["grant_type"], "refresh_token");
    assert_eq!(captured[1]["scope"], "org:create_api_key user:profile");
}

// --- Device-code flow -------------------------------------------------

#[derive(Clone)]
struct DeviceMock {
    poll_count: Arc<AtomicUsize>,
}

async fn device_code_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "device_code": "dc-xyz",
        "user_code": "ABCD-EFGH",
        "verification_uri": "https://example.com/activate",
        "verification_uri_complete": "https://example.com/activate?user_code=ABCD-EFGH",
        "expires_in": 600,
        "interval": 1,
    }))
}

async fn device_poll_handler(
    State(state): State<DeviceMock>,
    Form(_form): Form<serde_json::Value>,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    let n = state.poll_count.fetch_add(1, Ordering::SeqCst);
    if n < 2 {
        (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "authorization_pending"})),
        )
    } else {
        (
            axum::http::StatusCode::OK,
            Json(serde_json::json!({
                "access_token": "dev-access",
                "refresh_token": "dev-refresh",
                "expires_in": 3600,
            })),
        )
    }
}

async fn start_device_mock() -> (String, String, Arc<AtomicUsize>) {
    let poll_count = Arc::new(AtomicUsize::new(0));
    let state = DeviceMock {
        poll_count: poll_count.clone(),
    };
    let app = Router::new()
        .route("/device_code", post(device_code_handler))
        .route("/token", post(device_poll_handler))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        format!("http://{addr}/device_code"),
        format!("http://{addr}/token"),
        poll_count,
    )
}

#[tokio::test]
async fn device_code_request_returns_device_and_user_codes() {
    let (device_url, token_url, _) = start_device_mock().await;
    let mut ep = endpoints(token_url);
    ep.device_code_url = Some(device_url);
    let resp = request_device_code(&Client::new(), &ep).await.unwrap();
    assert_eq!(resp.device_code, "dc-xyz");
    assert_eq!(resp.user_code, "ABCD-EFGH");
    assert_eq!(resp.verification_uri, "https://example.com/activate");
    assert_eq!(resp.interval, 1);
    assert_eq!(resp.expires_in, 600);
}

#[tokio::test]
async fn device_poll_transitions_pending_to_ready() {
    let (device_url, token_url, counter) = start_device_mock().await;
    let mut ep = endpoints(token_url);
    ep.device_code_url = Some(device_url);

    // Poll 1 + 2 → Pending; poll 3 → Ready.
    for expected in &[false, false, true] {
        let outcome = poll_device_code(&Client::new(), &ep, "dc-xyz", None)
            .await
            .unwrap();
        match (outcome, *expected) {
            (DevicePollOutcome::Pending, false) => {}
            (DevicePollOutcome::Ready(r), true) => {
                assert_eq!(r.access_token, "dev-access");
                assert_eq!(r.refresh_token.as_deref(), Some("dev-refresh"));
            }
            (other, expected_ready) => {
                panic!(
                    "unexpected outcome (expected_ready={expected_ready}): {}",
                    match other {
                        DevicePollOutcome::Pending => "Pending",
                        DevicePollOutcome::SlowDown => "SlowDown",
                        DevicePollOutcome::AccessDenied => "AccessDenied",
                        DevicePollOutcome::Expired => "Expired",
                        DevicePollOutcome::Ready(_) => "Ready",
                    }
                );
            }
        }
    }
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}
