//! T10 — Google Auth authorizer contract (Phase 4a).
//!
//! Choke-points:
//! - Service-account path: reads `GOOGLE_APPLICATION_CREDENTIALS` JSON,
//!   RS256-signs a JWT with `scope=cloud-platform`, POSTs to token URL,
//!   populates `Authorization: Bearer <access_token>`.
//! - User-ADC path: reads `~/.config/gcloud/application_default_credentials.json`,
//!   POSTs `grant_type=refresh_token` to token URL.
//! - Metadata path: GETs `metadata.google.internal/.../default/token` with
//!   `Metadata-Flavor: Google`.
//! - ComputeOnly chain: skips SA + user-ADC, only hits metadata.
//! - Token caching: repeat `authorize` calls reuse the cached token.
//!
//! All three endpoints are mocked via axum. The SA path uses an RSA
//! private key fixture in `tests/fixtures/test_sa_key.pem`.

#![cfg(all(not(target_arch = "wasm32"), feature = "gcp-auth"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Form, State};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use chrono::Utc;
use serde::Deserialize;
use tokio::net::TcpListener;

use meerkat_auth_core::authorizers::{GoogleAuthAuthorizer, GoogleAuthChain};
use meerkat_core::handles::{
    AuthLeaseHandle, AuthLeasePhase, AuthLeaseSnapshot, DslTransitionError, LeaseKey,
};
use meerkat_core::{BindingId, HttpAuthorizationRequest, HttpAuthorizer, ProfileId, RealmId};

const TEST_PRIVATE_KEY: &str = include_str!("fixtures/test_sa_key.pem");

#[derive(Deserialize, Clone, Debug)]
#[allow(dead_code)]
struct OAuthForm {
    grant_type: String,
    #[serde(default)]
    assertion: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Clone)]
struct MockState {
    counter: Arc<AtomicUsize>,
    captured: Arc<Mutex<Vec<OAuthForm>>>,
    token_value: String,
    expires_in_by_call: Vec<u64>,
    failure: Option<MockFailure>,
}

#[derive(Clone)]
struct MockFailure {
    from_call: usize,
    status: axum::http::StatusCode,
    body: serde_json::Value,
}

async fn token_endpoint(State(state): State<MockState>, Form(form): Form<OAuthForm>) -> Response {
    let call = state.counter.fetch_add(1, Ordering::SeqCst) + 1;
    state.captured.lock().unwrap().push(form);
    if let Some(failure) = &state.failure
        && call >= failure.from_call
    {
        return (failure.status, Json(failure.body.clone())).into_response();
    }
    let expires_in = state
        .expires_in_by_call
        .get(call.saturating_sub(1))
        .or_else(|| state.expires_in_by_call.last())
        .copied()
        .unwrap_or(3600);
    Json(serde_json::json!({
        "access_token": state.token_value,
        "token_type": "Bearer",
        "expires_in": expires_in,
    }))
    .into_response()
}

async fn metadata_endpoint(
    State(state): State<MockState>,
    headers: axum::http::HeaderMap,
) -> Response {
    // GCE metadata server requires Metadata-Flavor: Google header.
    if headers.get("metadata-flavor").and_then(|v| v.to_str().ok()) != Some("Google") {
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }
    let call = state.counter.fetch_add(1, Ordering::SeqCst) + 1;
    if let Some(failure) = &state.failure
        && call >= failure.from_call
    {
        return (failure.status, Json(failure.body.clone())).into_response();
    }
    let expires_in = state
        .expires_in_by_call
        .get(call.saturating_sub(1))
        .or_else(|| state.expires_in_by_call.last())
        .copied()
        .unwrap_or(3600);
    Json(serde_json::json!({
        "access_token": state.token_value,
        "token_type": "Bearer",
        "expires_in": expires_in,
    }))
    .into_response()
}

struct MockServer {
    base_url: String,
    counter: Arc<AtomicUsize>,
    captured: Arc<Mutex<Vec<OAuthForm>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LeaseEvent {
    Snapshot,
    Acquire(LeaseKey, u64),
    BeginRefresh(LeaseKey),
    CompleteRefresh(LeaseKey, u64),
    RefreshFailed(LeaseKey, bool),
    MarkExpiring(LeaseKey),
    MarkReauthRequired(LeaseKey),
    Release(LeaseKey),
}

struct RecordingAuthLeaseHandle {
    events: Mutex<Vec<LeaseEvent>>,
    snapshot: Mutex<AuthLeaseSnapshot>,
    fail_action: Mutex<Option<&'static str>>,
}

impl Default for RecordingAuthLeaseHandle {
    fn default() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            snapshot: Mutex::new(AuthLeaseSnapshot {
                phase: None,
                expires_at: None,
            }),
            fail_action: Mutex::new(None),
        }
    }
}

impl RecordingAuthLeaseHandle {
    fn fail_on(action: &'static str) -> Self {
        Self {
            fail_action: Mutex::new(Some(action)),
            ..Self::default()
        }
    }

    fn events(&self) -> Vec<LeaseEvent> {
        self.events.lock().unwrap().clone()
    }

    fn acquired(&self) -> Vec<(LeaseKey, u64)> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                LeaseEvent::Acquire(lease_key, expires_at) => Some((lease_key, expires_at)),
                _ => None,
            })
            .collect()
    }

    fn maybe_fail(&self, action: &'static str) -> Result<(), DslTransitionError> {
        if self.fail_action.lock().unwrap().as_deref() == Some(action) {
            return Err(DslTransitionError::new(
                action,
                "injected auth lease observer failure",
            ));
        }
        Ok(())
    }
}

impl AuthLeaseHandle for RecordingAuthLeaseHandle {
    fn acquire_lease(
        &self,
        lease_key: &LeaseKey,
        expires_at: u64,
    ) -> Result<(), DslTransitionError> {
        self.maybe_fail("acquire_lease")?;
        self.events
            .lock()
            .unwrap()
            .push(LeaseEvent::Acquire(lease_key.clone(), expires_at));
        *self.snapshot.lock().unwrap() = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(expires_at),
        };
        Ok(())
    }

    fn mark_expiring(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        self.maybe_fail("mark_expiring")?;
        self.events
            .lock()
            .unwrap()
            .push(LeaseEvent::MarkExpiring(lease_key.clone()));
        self.snapshot.lock().unwrap().phase = Some(AuthLeasePhase::Expiring);
        Ok(())
    }

    fn begin_refresh(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        self.maybe_fail("begin_refresh")?;
        self.events
            .lock()
            .unwrap()
            .push(LeaseEvent::BeginRefresh(lease_key.clone()));
        self.snapshot.lock().unwrap().phase = Some(AuthLeasePhase::Refreshing);
        Ok(())
    }

    fn complete_refresh(
        &self,
        lease_key: &LeaseKey,
        new_expires_at: u64,
        _now: u64,
    ) -> Result<(), DslTransitionError> {
        self.maybe_fail("complete_refresh")?;
        self.events
            .lock()
            .unwrap()
            .push(LeaseEvent::CompleteRefresh(
                lease_key.clone(),
                new_expires_at,
            ));
        *self.snapshot.lock().unwrap() = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(new_expires_at),
        };
        Ok(())
    }

    fn refresh_failed(
        &self,
        lease_key: &LeaseKey,
        permanent: bool,
    ) -> Result<(), DslTransitionError> {
        self.maybe_fail("refresh_failed")?;
        self.events
            .lock()
            .unwrap()
            .push(LeaseEvent::RefreshFailed(lease_key.clone(), permanent));
        self.snapshot.lock().unwrap().phase = Some(if permanent {
            AuthLeasePhase::ReauthRequired
        } else {
            AuthLeasePhase::Expiring
        });
        Ok(())
    }

    fn mark_reauth_required(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        self.maybe_fail("mark_reauth_required")?;
        self.events
            .lock()
            .unwrap()
            .push(LeaseEvent::MarkReauthRequired(lease_key.clone()));
        self.snapshot.lock().unwrap().phase = Some(AuthLeasePhase::ReauthRequired);
        Ok(())
    }

    fn release_lease(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        self.maybe_fail("release_lease")?;
        self.events
            .lock()
            .unwrap()
            .push(LeaseEvent::Release(lease_key.clone()));
        *self.snapshot.lock().unwrap() = AuthLeaseSnapshot {
            phase: None,
            expires_at: None,
        };
        Ok(())
    }

    fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
        self.events.lock().unwrap().push(LeaseEvent::Snapshot);
        self.snapshot.lock().unwrap().clone()
    }
}

async fn start_mock(token_value: &str) -> MockServer {
    start_mock_with_expiry(token_value, 3600).await
}

async fn start_mock_with_expiry(token_value: &str, expires_in: u64) -> MockServer {
    start_mock_with_config(token_value, vec![expires_in], None).await
}

async fn start_mock_with_config(
    token_value: &str,
    expires_in_by_call: Vec<u64>,
    failure: Option<MockFailure>,
) -> MockServer {
    let counter = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(Mutex::new(Vec::new()));
    let state = MockState {
        counter: counter.clone(),
        captured: captured.clone(),
        token_value: token_value.into(),
        expires_in_by_call,
        failure,
    };
    let app = Router::new()
        .route("/token", post(token_endpoint))
        .route(
            "/computeMetadata/v1/instance/service-accounts/default/token",
            get(metadata_endpoint),
        )
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    MockServer {
        base_url,
        counter,
        captured,
    }
}

fn write_sa_key(dir: &std::path::Path, token_url: &str) -> std::path::PathBuf {
    let json = serde_json::json!({
        "type": "service_account",
        "project_id": "test-project",
        "private_key": TEST_PRIVATE_KEY,
        "client_email": "test@test-project.iam.gserviceaccount.com",
        "token_uri": token_url,
    });
    let path = dir.join("sa.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&json).unwrap()).unwrap();
    path
}

fn write_user_adc(dir: &std::path::Path, token_url: &str) -> std::path::PathBuf {
    let json = serde_json::json!({
        "client_id": "user-client-id.apps.googleusercontent.com",
        "client_secret": "user-client-secret",
        "refresh_token": "user-refresh-token",
        "token_uri": token_url,
        "type": "authorized_user",
    });
    let gcloud_dir = dir.join(".config").join("gcloud");
    std::fs::create_dir_all(&gcloud_dir).unwrap();
    let path = gcloud_dir.join("application_default_credentials.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&json).unwrap()).unwrap();
    path
}

// --- Service account path ---------------------------------------------

#[tokio::test]
async fn service_account_path_signs_jwt_and_gets_token() {
    let mock = start_mock("sa-access-token").await;
    let tempdir = tempfile::tempdir().unwrap();
    let sa_path = write_sa_key(tempdir.path(), &format!("{}/token", mock.base_url));

    let env_lookup = {
        let sa_path = sa_path.clone();
        Arc::new(move |k: &str| {
            if k == "GOOGLE_APPLICATION_CREDENTIALS" {
                Some(sa_path.to_string_lossy().to_string())
            } else {
                None
            }
        }) as Arc<dyn Fn(&str) -> Option<String> + Send + Sync>
    };

    let authorizer = GoogleAuthAuthorizer::with_env_lookup(GoogleAuthChain::Default, env_lookup)
        .with_home_dir(tempdir.path()) // empty home, no user ADC
        .with_token_url_override(format!("{}/token", mock.base_url));

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://generativelanguage.googleapis.com/v1/models:generateContent",
        headers: &mut headers,
    };
    authorizer.authorize(&mut req).await.unwrap();

    let auth = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .unwrap();
    assert_eq!(auth.1, "Bearer sa-access-token");
    assert!(
        authorizer.expires_at().is_some(),
        "service-account token expiry must be observable through HttpAuthorizer"
    );
    assert_eq!(mock.counter.load(Ordering::SeqCst), 1);
    let captured = mock.captured.lock().unwrap();
    assert_eq!(
        captured[0].grant_type,
        "urn:ietf:params:oauth:grant-type:jwt-bearer"
    );
    assert!(captured[0].assertion.is_some(), "JWT assertion must be set");
    // Assertion is a 3-segment JWT.
    let jwt = captured[0].assertion.as_ref().unwrap();
    assert_eq!(jwt.split('.').count(), 3);
}

// --- User ADC path ----------------------------------------------------

#[tokio::test]
async fn user_adc_path_uses_refresh_token_flow() {
    let mock = start_mock("user-adc-access-token").await;
    let tempdir = tempfile::tempdir().unwrap();
    let _ = write_user_adc(tempdir.path(), &format!("{}/token", mock.base_url));

    let env_lookup =
        Arc::new(|_: &str| None::<String>) as Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

    let authorizer = GoogleAuthAuthorizer::with_env_lookup(GoogleAuthChain::Default, env_lookup)
        .with_home_dir(tempdir.path())
        .with_token_url_override(format!("{}/token", mock.base_url));

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://generativelanguage.googleapis.com/v1/models",
        headers: &mut headers,
    };
    authorizer.authorize(&mut req).await.unwrap();

    let auth = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .unwrap();
    assert_eq!(auth.1, "Bearer user-adc-access-token");
    let captured = mock.captured.lock().unwrap();
    assert_eq!(captured[0].grant_type, "refresh_token");
    assert_eq!(
        captured[0].client_id.as_deref(),
        Some("user-client-id.apps.googleusercontent.com")
    );
    assert_eq!(
        captured[0].refresh_token.as_deref(),
        Some("user-refresh-token")
    );
}

// --- Metadata / ComputeOnly chain -------------------------------------

#[tokio::test]
async fn compute_only_chain_uses_metadata_server() {
    let mock = start_mock("metadata-access-token").await;
    let env_lookup =
        Arc::new(|_: &str| None::<String>) as Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

    let authorizer =
        GoogleAuthAuthorizer::with_env_lookup(GoogleAuthChain::ComputeOnly, env_lookup)
            .with_metadata_url_override(format!(
                "{}/computeMetadata/v1/instance/service-accounts/default/token",
                mock.base_url
            ));

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://generativelanguage.googleapis.com/v1/models",
        headers: &mut headers,
    };
    authorizer.authorize(&mut req).await.unwrap();

    let auth = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .unwrap();
    assert_eq!(auth.1, "Bearer metadata-access-token");
    assert_eq!(mock.counter.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn default_chain_falls_through_to_metadata_when_no_sa_and_no_user_adc() {
    let mock = start_mock("fallthrough-token").await;
    let tempdir = tempfile::tempdir().unwrap();
    // No SA env var, no user ADC file in tempdir/.config/gcloud.
    let env_lookup =
        Arc::new(|_: &str| None::<String>) as Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

    let authorizer = GoogleAuthAuthorizer::with_env_lookup(GoogleAuthChain::Default, env_lookup)
        .with_home_dir(tempdir.path())
        .with_metadata_url_override(format!(
            "{}/computeMetadata/v1/instance/service-accounts/default/token",
            mock.base_url
        ));

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://x.googleapis.com/",
        headers: &mut headers,
    };
    authorizer.authorize(&mut req).await.unwrap();
    let auth = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .unwrap();
    assert_eq!(auth.1, "Bearer fallthrough-token");
}

#[tokio::test]
async fn token_is_cached_between_calls() {
    let mock = start_mock("cached-sa-token").await;
    let tempdir = tempfile::tempdir().unwrap();
    let sa_path = write_sa_key(tempdir.path(), &format!("{}/token", mock.base_url));
    let env_lookup = {
        let sa_path = sa_path.clone();
        Arc::new(move |k: &str| {
            if k == "GOOGLE_APPLICATION_CREDENTIALS" {
                Some(sa_path.to_string_lossy().to_string())
            } else {
                None
            }
        }) as Arc<dyn Fn(&str) -> Option<String> + Send + Sync>
    };
    let authorizer = GoogleAuthAuthorizer::with_env_lookup(GoogleAuthChain::Default, env_lookup)
        .with_home_dir(tempdir.path())
        .with_token_url_override(format!("{}/token", mock.base_url));

    for _ in 0..4 {
        let mut headers = Vec::new();
        let mut req = HttpAuthorizationRequest {
            method: "POST",
            url: "https://x.googleapis.com/",
            headers: &mut headers,
        };
        authorizer.authorize(&mut req).await.unwrap();
    }
    assert_eq!(
        mock.counter.load(Ordering::SeqCst),
        1,
        "expected single fetch with caching"
    );
}

#[tokio::test]
async fn cached_token_authorize_publishes_token_expiry_to_auth_lease_handle() {
    let mock = start_mock("lease-tracked-sa-token").await;
    let tempdir = tempfile::tempdir().unwrap();
    let sa_path = write_sa_key(tempdir.path(), &format!("{}/token", mock.base_url));
    let env_lookup = {
        let sa_path = sa_path.clone();
        Arc::new(move |k: &str| {
            if k == "GOOGLE_APPLICATION_CREDENTIALS" {
                Some(sa_path.to_string_lossy().to_string())
            } else {
                None
            }
        }) as Arc<dyn Fn(&str) -> Option<String> + Send + Sync>
    };
    let handle = Arc::new(RecordingAuthLeaseHandle::default());
    let lease_key = LeaseKey::new(
        RealmId::parse("dev").unwrap(),
        BindingId::parse("gemini").unwrap(),
        Some(ProfileId::parse("google_adc").unwrap()),
    );
    let lease_handle: Arc<dyn AuthLeaseHandle> = handle.clone();
    let authorizer = GoogleAuthAuthorizer::with_env_lookup(GoogleAuthChain::Default, env_lookup)
        .with_home_dir(tempdir.path())
        .with_auth_lease_observer(lease_handle, lease_key.clone())
        .with_token_url_override(format!("{}/token", mock.base_url));

    for _ in 0..2 {
        let mut headers = Vec::new();
        let mut req = HttpAuthorizationRequest {
            method: "POST",
            url: "https://x.googleapis.com/",
            headers: &mut headers,
        };
        authorizer.authorize(&mut req).await.unwrap();
    }

    let acquired = handle.acquired();
    assert_eq!(
        mock.counter.load(Ordering::SeqCst),
        1,
        "second authorize should reuse the cached token"
    );
    assert_eq!(
        acquired.len(),
        1,
        "cached token freshness must be observed through auth-machine snapshot truth"
    );
    assert_eq!(acquired[0].0, lease_key);
    let events = handle.events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, LeaseEvent::Snapshot)),
        "cached token reuse must check auth-machine lease truth"
    );
    assert!(
        acquired[0].1 > Utc::now().timestamp().max(0) as u64,
        "published auth-machine expiry must reflect the fetched token"
    );
}

#[tokio::test]
async fn observer_failure_fails_closed_without_authorization_header() {
    let mock = start_mock("lease-observer-fails").await;
    let tempdir = tempfile::tempdir().unwrap();
    let sa_path = write_sa_key(tempdir.path(), &format!("{}/token", mock.base_url));
    let env_lookup = {
        let sa_path = sa_path.clone();
        Arc::new(move |k: &str| {
            if k == "GOOGLE_APPLICATION_CREDENTIALS" {
                Some(sa_path.to_string_lossy().to_string())
            } else {
                None
            }
        }) as Arc<dyn Fn(&str) -> Option<String> + Send + Sync>
    };
    let handle = Arc::new(RecordingAuthLeaseHandle::fail_on("acquire_lease"));
    let lease_key = LeaseKey::new(
        RealmId::parse("dev").unwrap(),
        BindingId::parse("gemini").unwrap(),
        Some(ProfileId::parse("google_adc").unwrap()),
    );
    let lease_handle: Arc<dyn AuthLeaseHandle> = handle;
    let authorizer = GoogleAuthAuthorizer::with_env_lookup(GoogleAuthChain::Default, env_lookup)
        .with_home_dir(tempdir.path())
        .with_auth_lease_observer(lease_handle, lease_key)
        .with_token_url_override(format!("{}/token", mock.base_url));

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://x.googleapis.com/",
        headers: &mut headers,
    };
    let err = authorizer.authorize(&mut req).await.unwrap_err();

    assert!(
        matches!(err, meerkat_core::AuthError::Other(_)),
        "auth lease publication failure must be visible, got {err:?}"
    );
    assert!(
        headers
            .iter()
            .all(|(name, _)| !name.eq_ignore_ascii_case("authorization")),
        "authorizer must not attach a bearer token when lease truth rejected publication"
    );
}

#[tokio::test]
async fn short_lived_token_expiry_is_observable_and_refetched() {
    let mock = start_mock_with_expiry("short-lived-sa-token", 30).await;
    let tempdir = tempfile::tempdir().unwrap();
    let sa_path = write_sa_key(tempdir.path(), &format!("{}/token", mock.base_url));
    let env_lookup = {
        let sa_path = sa_path.clone();
        Arc::new(move |k: &str| {
            if k == "GOOGLE_APPLICATION_CREDENTIALS" {
                Some(sa_path.to_string_lossy().to_string())
            } else {
                None
            }
        }) as Arc<dyn Fn(&str) -> Option<String> + Send + Sync>
    };
    let authorizer = GoogleAuthAuthorizer::with_env_lookup(GoogleAuthChain::Default, env_lookup)
        .with_home_dir(tempdir.path())
        .with_token_url_override(format!("{}/token", mock.base_url));

    for _ in 0..2 {
        let mut headers = Vec::new();
        let mut req = HttpAuthorizationRequest {
            method: "POST",
            url: "https://x.googleapis.com/",
            headers: &mut headers,
        };
        authorizer.authorize(&mut req).await.unwrap();
    }

    let expires_at = authorizer
        .expires_at()
        .expect("short-lived token expiry should be projected");
    assert!(
        expires_at > Utc::now(),
        "cached expiry should carry the token endpoint's expires_in"
    );
    assert_eq!(
        mock.counter.load(Ordering::SeqCst),
        2,
        "token inside the canonical refresh window must be refetched"
    );
}

#[tokio::test]
async fn token_endpoint_transient_failure_keeps_auth_lease_retryable() {
    let mock = start_mock_with_config(
        "google-transient-token",
        vec![30],
        Some(MockFailure {
            from_call: 2,
            status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
            body: serde_json::json!({"error": "temporarily_unavailable"}),
        }),
    )
    .await;
    let tempdir = tempfile::tempdir().unwrap();
    let sa_path = write_sa_key(tempdir.path(), &format!("{}/token", mock.base_url));
    let env_lookup = {
        let sa_path = sa_path.clone();
        Arc::new(move |k: &str| {
            if k == "GOOGLE_APPLICATION_CREDENTIALS" {
                Some(sa_path.to_string_lossy().to_string())
            } else {
                None
            }
        }) as Arc<dyn Fn(&str) -> Option<String> + Send + Sync>
    };
    let handle = Arc::new(RecordingAuthLeaseHandle::default());
    let lease_key = LeaseKey::new(
        RealmId::parse("dev").unwrap(),
        BindingId::parse("gemini").unwrap(),
        Some(ProfileId::parse("google_adc").unwrap()),
    );
    let lease_handle: Arc<dyn AuthLeaseHandle> = handle.clone();
    let authorizer = GoogleAuthAuthorizer::with_env_lookup(GoogleAuthChain::Default, env_lookup)
        .with_home_dir(tempdir.path())
        .with_auth_lease_observer(lease_handle, lease_key.clone())
        .with_token_url_override(format!("{}/token", mock.base_url));

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://x.googleapis.com/",
        headers: &mut headers,
    };
    authorizer.authorize(&mut req).await.unwrap();

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://x.googleapis.com/",
        headers: &mut headers,
    };
    let err = authorizer.authorize(&mut req).await.unwrap_err();
    assert!(
        matches!(err, meerkat_core::AuthError::RefreshFailed(_)),
        "token endpoint outage should still fail the caller visibly, got {err:?}"
    );

    let snapshot = handle.snapshot(&lease_key);
    assert_eq!(
        snapshot.phase,
        Some(AuthLeasePhase::Expiring),
        "transient Google token endpoint failure must remain retryable in AuthMachine"
    );
    assert!(
        handle
            .events()
            .iter()
            .any(|event| matches!(event, LeaseEvent::RefreshFailed(_, false))),
        "transient Google token endpoint failure should not force reauth"
    );
}

#[tokio::test]
async fn metadata_endpoint_transient_failure_keeps_auth_lease_retryable() {
    let mock = start_mock_with_config(
        "metadata-transient-token",
        vec![30],
        Some(MockFailure {
            from_call: 2,
            status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
            body: serde_json::json!({"error": "temporarily_unavailable"}),
        }),
    )
    .await;
    let env_lookup =
        Arc::new(|_: &str| None::<String>) as Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
    let handle = Arc::new(RecordingAuthLeaseHandle::default());
    let lease_key = LeaseKey::new(
        RealmId::parse("dev").unwrap(),
        BindingId::parse("gemini").unwrap(),
        Some(ProfileId::parse("google_metadata").unwrap()),
    );
    let lease_handle: Arc<dyn AuthLeaseHandle> = handle.clone();
    let authorizer =
        GoogleAuthAuthorizer::with_env_lookup(GoogleAuthChain::ComputeOnly, env_lookup)
            .with_auth_lease_observer(lease_handle, lease_key.clone())
            .with_metadata_url_override(format!(
                "{}/computeMetadata/v1/instance/service-accounts/default/token",
                mock.base_url
            ));

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://generativelanguage.googleapis.com/v1/models",
        headers: &mut headers,
    };
    authorizer.authorize(&mut req).await.unwrap();

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://generativelanguage.googleapis.com/v1/models",
        headers: &mut headers,
    };
    let err = authorizer.authorize(&mut req).await.unwrap_err();
    assert!(
        matches!(err, meerkat_core::AuthError::RefreshFailed(_)),
        "metadata outage should still fail the caller visibly, got {err:?}"
    );

    let snapshot = handle.snapshot(&lease_key);
    assert_eq!(
        snapshot.phase,
        Some(AuthLeasePhase::Expiring),
        "transient metadata failure must remain retryable in AuthMachine"
    );
    assert!(
        handle
            .events()
            .iter()
            .any(|event| matches!(event, LeaseEvent::RefreshFailed(_, false))),
        "transient metadata failure should not force reauth"
    );
}

#[tokio::test]
async fn default_chain_metadata_transient_failure_keeps_auth_lease_retryable() {
    let mock = start_mock_with_config(
        "default-metadata-transient-token",
        vec![30],
        Some(MockFailure {
            from_call: 2,
            status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
            body: serde_json::json!({"error": "temporarily_unavailable"}),
        }),
    )
    .await;
    let tempdir = tempfile::tempdir().unwrap();
    let env_lookup =
        Arc::new(|_: &str| None::<String>) as Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
    let handle = Arc::new(RecordingAuthLeaseHandle::default());
    let lease_key = LeaseKey::new(
        RealmId::parse("dev").unwrap(),
        BindingId::parse("gemini").unwrap(),
        Some(ProfileId::parse("google_default_metadata").unwrap()),
    );
    let lease_handle: Arc<dyn AuthLeaseHandle> = handle.clone();
    let authorizer = GoogleAuthAuthorizer::with_env_lookup(GoogleAuthChain::Default, env_lookup)
        .with_home_dir(tempdir.path())
        .with_auth_lease_observer(lease_handle, lease_key.clone())
        .with_metadata_url_override(format!(
            "{}/computeMetadata/v1/instance/service-accounts/default/token",
            mock.base_url
        ));

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://generativelanguage.googleapis.com/v1/models",
        headers: &mut headers,
    };
    authorizer.authorize(&mut req).await.unwrap();

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://generativelanguage.googleapis.com/v1/models",
        headers: &mut headers,
    };
    let err = authorizer.authorize(&mut req).await.unwrap_err();
    assert!(
        matches!(err, meerkat_core::AuthError::RefreshFailed(_)),
        "metadata outage should still fail the caller visibly, got {err:?}"
    );

    let snapshot = handle.snapshot(&lease_key);
    assert_eq!(
        snapshot.phase,
        Some(AuthLeasePhase::Expiring),
        "Default ADC metadata transient failure must remain retryable in AuthMachine"
    );
    assert!(
        handle
            .events()
            .iter()
            .any(|event| matches!(event, LeaseEvent::RefreshFailed(_, false))),
        "Default ADC metadata transient failure should not force reauth"
    );
}

// --- Error propagation ------------------------------------------------

#[tokio::test]
async fn missing_credentials_surface_missing_secret() {
    // Default chain, no SA file, no user ADC, and metadata URL points at
    // a closed port → all three sources fail.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener); // close so requests fail

    let tempdir = tempfile::tempdir().unwrap();
    let env_lookup =
        Arc::new(|_: &str| None::<String>) as Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
    let authorizer = GoogleAuthAuthorizer::with_env_lookup(GoogleAuthChain::Default, env_lookup)
        .with_home_dir(tempdir.path())
        .with_metadata_url_override(format!(
            "http://{addr}/computeMetadata/v1/instance/service-accounts/default/token"
        ));

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://x.googleapis.com/",
        headers: &mut headers,
    };
    let err = authorizer.authorize(&mut req).await.unwrap_err();
    assert!(
        matches!(err, meerkat_core::AuthError::MissingSecret),
        "got {err:?}"
    );
}

// avoid unused lint on `IntoResponse` import (used implicitly via Router).
#[allow(dead_code)]
fn _ensure_imports() {
    let _: fn() -> axum::response::Response = || ().into_response();
}
