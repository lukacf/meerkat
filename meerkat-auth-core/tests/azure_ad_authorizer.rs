//! T11 — Azure AD authorizer contract (Phase 4a).
//!
//! Choke-point: `AzureAdAuthorizer::authorize` POSTs a
//! `grant_type=client_credentials` form to the tenant's token endpoint,
//! then populates `Authorization: Bearer <access_token>` on the outbound
//! request. Token is cached across calls.
//!
//! The test stands up an axum mock server for the token endpoint, counts
//! the number of token exchanges, and validates the request form + the
//! authorization header the authorizer emits on the real outbound request.

#![cfg(all(not(target_arch = "wasm32"), feature = "azure-ad"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Form, State};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::post;
use chrono::Utc;
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::time::{Duration as TokioDuration, sleep, timeout};

use meerkat_auth_core::authorizers::{AzureAdAuthorizer, AzureClientCredentials};
use meerkat_core::handles::{
    AuthLeaseHandle, AuthLeasePhase, AuthLeaseSnapshot, AuthLeaseTransition, DslTransitionError,
    LeaseKey,
};
use meerkat_core::{BindingId, HttpAuthorizationRequest, HttpAuthorizer, ProfileId, RealmId};

#[derive(Deserialize, Clone)]
struct TokenForm {
    grant_type: String,
    client_id: String,
    client_secret: String,
    scope: String,
}

#[derive(Clone)]
struct MockState {
    counter: Arc<AtomicUsize>,
    captured: Arc<std::sync::Mutex<Vec<TokenForm>>>,
    token_value: String,
    expires_in_by_call: Vec<u64>,
    failure: Option<MockFailure>,
    delay_from_call: Option<usize>,
    delay: TokioDuration,
}

#[derive(Clone)]
struct MockFailure {
    from_call: usize,
    status: axum::http::StatusCode,
    body: serde_json::Value,
}

async fn token_handler(State(state): State<MockState>, Form(form): Form<TokenForm>) -> Response {
    let call = state.counter.fetch_add(1, Ordering::SeqCst) + 1;
    state.captured.lock().unwrap().push(form);
    if state
        .delay_from_call
        .is_some_and(|delay_from_call| call >= delay_from_call)
    {
        sleep(state.delay).await;
    }
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
    captured: Arc<std::sync::Mutex<Vec<TokenForm>>>,
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
    generation: Mutex<u64>,
    fail_action: Mutex<Option<&'static str>>,
}

impl Default for RecordingAuthLeaseHandle {
    fn default() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            snapshot: Mutex::new(AuthLeaseSnapshot {
                phase: None,
                expires_at: None,
                credential_present: false,
                generation: 0,
                credential_published_at_millis: None,
            }),
            generation: Mutex::new(0),
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

    fn next_generation(&self) -> u64 {
        let mut generation = self.generation.lock().unwrap();
        *generation += 1;
        *generation
    }
}

impl AuthLeaseHandle for RecordingAuthLeaseHandle {
    fn acquire_lease(
        &self,
        lease_key: &LeaseKey,
        expires_at: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        self.maybe_fail("acquire_lease")?;
        self.events
            .lock()
            .unwrap()
            .push(LeaseEvent::Acquire(lease_key.clone(), expires_at));
        let generation = self.next_generation();
        *self.snapshot.lock().unwrap() = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(expires_at),
            credential_present: true,
            generation,
            credential_published_at_millis: None,
        };
        Ok(AuthLeaseTransition {
            generation,
            credential_published_at_millis: None,
        })
    }

    fn mark_expiring(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        self.maybe_fail("mark_expiring")?;
        self.events
            .lock()
            .unwrap()
            .push(LeaseEvent::MarkExpiring(lease_key.clone()));
        let mut snapshot = self.snapshot.lock().unwrap();
        snapshot.phase = Some(AuthLeasePhase::Expiring);
        snapshot.generation = self.next_generation();
        Ok(())
    }

    fn begin_refresh(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        self.maybe_fail("begin_refresh")?;
        self.events
            .lock()
            .unwrap()
            .push(LeaseEvent::BeginRefresh(lease_key.clone()));
        let mut snapshot = self.snapshot.lock().unwrap();
        snapshot.phase = Some(AuthLeasePhase::Refreshing);
        snapshot.generation = self.next_generation();
        Ok(())
    }

    fn complete_refresh(
        &self,
        lease_key: &LeaseKey,
        new_expires_at: u64,
        _now: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        self.maybe_fail("complete_refresh")?;
        self.events
            .lock()
            .unwrap()
            .push(LeaseEvent::CompleteRefresh(
                lease_key.clone(),
                new_expires_at,
            ));
        let generation = self.next_generation();
        *self.snapshot.lock().unwrap() = AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: Some(new_expires_at),
            credential_present: true,
            generation,
            credential_published_at_millis: None,
        };
        Ok(AuthLeaseTransition {
            generation,
            credential_published_at_millis: None,
        })
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
        let mut snapshot = self.snapshot.lock().unwrap();
        snapshot.phase = Some(if permanent {
            AuthLeasePhase::ReauthRequired
        } else {
            AuthLeasePhase::Expiring
        });
        snapshot.generation = self.next_generation();
        Ok(())
    }

    fn mark_reauth_required(&self, lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        self.maybe_fail("mark_reauth_required")?;
        self.events
            .lock()
            .unwrap()
            .push(LeaseEvent::MarkReauthRequired(lease_key.clone()));
        let mut snapshot = self.snapshot.lock().unwrap();
        snapshot.phase = Some(AuthLeasePhase::ReauthRequired);
        snapshot.generation = self.next_generation();
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
            credential_present: false,
            generation: self.next_generation(),
            credential_published_at_millis: None,
        };
        Ok(())
    }

    fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
        self.events.lock().unwrap().push(LeaseEvent::Snapshot);
        self.snapshot.lock().unwrap().clone()
    }
}

async fn start_mock(token_value: &str, expires_in: u64) -> MockServer {
    start_mock_with_config(token_value, vec![expires_in], None, None).await
}

async fn start_mock_with_failure(
    token_value: &str,
    expires_in: u64,
    fail_from_call: Option<usize>,
) -> MockServer {
    let failure = fail_from_call.map(|from_call| MockFailure {
        from_call,
        status: axum::http::StatusCode::UNAUTHORIZED,
        body: serde_json::json!({"error": "invalid_client"}),
    });
    start_mock_with_config(token_value, vec![expires_in], failure, None).await
}

async fn start_mock_with_config(
    token_value: &str,
    expires_in_by_call: Vec<u64>,
    failure: Option<MockFailure>,
    delay_from_call: Option<usize>,
) -> MockServer {
    let counter = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let state = MockState {
        counter: counter.clone(),
        captured: captured.clone(),
        token_value: token_value.into(),
        expires_in_by_call,
        failure,
        delay_from_call,
        delay: TokioDuration::from_millis(150),
    };
    let app = Router::new()
        .route("/tenant-id/oauth2/v2.0/token", post(token_handler))
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

fn creds(token_url_host: &str) -> AzureClientCredentials {
    AzureClientCredentials {
        tenant_id: "tenant-id".into(),
        client_id: "app-client-id".into(),
        client_secret: "super-secret".into(),
        authority_host: token_url_host.into(),
    }
}

fn with_recording_auth_lease(
    authorizer: AzureAdAuthorizer,
) -> (AzureAdAuthorizer, Arc<RecordingAuthLeaseHandle>, LeaseKey) {
    let handle = Arc::new(RecordingAuthLeaseHandle::default());
    let lease_key = LeaseKey::new(
        RealmId::parse("dev").unwrap(),
        BindingId::parse("azure_foundry").unwrap(),
        Some(ProfileId::parse("foundry_azure_ad").unwrap()),
    );
    let lease_handle: Arc<dyn AuthLeaseHandle> = handle.clone();
    (
        authorizer.with_auth_lease_observer(lease_handle, lease_key.clone()),
        handle,
        lease_key,
    )
}

#[tokio::test]
async fn authorize_adds_bearer_header_from_token_endpoint() {
    let mock = start_mock("azure-access-token-xyz", 3600).await;
    let (authorizer, _, _) = with_recording_auth_lease(
        AzureAdAuthorizer::new(
            "https://cognitiveservices.azure.com/.default",
            creds(&mock.base_url),
        )
        .with_token_url_override(format!("{}/tenant-id/oauth2/v2.0/token", mock.base_url)),
    );

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://example.foundry.azure.com/v1/messages",
        headers: &mut headers,
    };
    authorizer.authorize(&mut req).await.unwrap();

    let auth = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .unwrap();
    assert_eq!(auth.1, "Bearer azure-access-token-xyz");
    assert!(
        authorizer.expires_at().is_some(),
        "Azure AD expiry must be projected from AuthMachine lease truth"
    );

    let captured = mock.captured.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].grant_type, "client_credentials");
    assert_eq!(captured[0].client_id, "app-client-id");
    assert_eq!(captured[0].client_secret, "super-secret");
    assert_eq!(
        captured[0].scope,
        "https://cognitiveservices.azure.com/.default"
    );
}

#[tokio::test]
async fn authorize_without_auth_lease_observer_fails_closed_before_token_fetch() {
    let mock = start_mock("cached-token", 3600).await;
    let authorizer = AzureAdAuthorizer::new(
        "https://cognitiveservices.azure.com/.default",
        creds(&mock.base_url),
    )
    .with_token_url_override(format!("{}/tenant-id/oauth2/v2.0/token", mock.base_url));

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://example.foundry.azure.com/v1/messages",
        headers: &mut headers,
    };
    let err = authorizer.authorize(&mut req).await.unwrap_err();

    assert!(
        matches!(err, meerkat_core::AuthError::HostOwnedUnavailable),
        "cloud authorizer must fail closed without AuthMachine ownership, got {err:?}"
    );
    assert!(
        headers
            .iter()
            .all(|(name, _)| !name.eq_ignore_ascii_case("authorization")),
        "observer-absent cloud authorizer must not attach an authorization header"
    );
    assert_eq!(
        mock.counter.load(Ordering::SeqCst),
        0,
        "observer-absent cloud authorizer must not fetch a provider-local token"
    );
}

#[tokio::test]
async fn authorize_publishes_token_expiry_to_auth_lease_handle() {
    let mock = start_mock("lease-tracked-token", 3600).await;
    let handle = Arc::new(RecordingAuthLeaseHandle::default());
    let lease_key = LeaseKey::new(
        RealmId::parse("dev").unwrap(),
        BindingId::parse("azure_foundry").unwrap(),
        Some(ProfileId::parse("foundry_azure_ad").unwrap()),
    );
    let lease_handle: Arc<dyn AuthLeaseHandle> = handle.clone();
    let authorizer = AzureAdAuthorizer::new(
        "https://cognitiveservices.azure.com/.default",
        creds(&mock.base_url),
    )
    .with_auth_lease_observer(lease_handle, lease_key.clone())
    .with_token_url_override(format!("{}/tenant-id/oauth2/v2.0/token", mock.base_url));

    for _ in 0..2 {
        let mut headers = Vec::new();
        let mut req = HttpAuthorizationRequest {
            method: "POST",
            url: "https://example.foundry.azure.com/v1/messages",
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
async fn released_and_reacquired_auth_lease_invalidates_private_token_cache() {
    let mock = start_mock("lease-reacquired-token", 3600).await;
    let handle = Arc::new(RecordingAuthLeaseHandle::default());
    let lease_key = LeaseKey::new(
        RealmId::parse("dev").unwrap(),
        BindingId::parse("azure_foundry").unwrap(),
        Some(ProfileId::parse("foundry_azure_ad").unwrap()),
    );
    let lease_handle: Arc<dyn AuthLeaseHandle> = handle.clone();
    let authorizer = AzureAdAuthorizer::new(
        "https://cognitiveservices.azure.com/.default",
        creds(&mock.base_url),
    )
    .with_auth_lease_observer(lease_handle, lease_key.clone())
    .with_token_url_override(format!("{}/tenant-id/oauth2/v2.0/token", mock.base_url));

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://example.foundry.azure.com/v1/messages",
        headers: &mut headers,
    };
    authorizer.authorize(&mut req).await.unwrap();
    let expires_at = handle.acquired()[0].1;

    handle.release_lease(&lease_key).unwrap();
    handle.acquire_lease(&lease_key, expires_at).unwrap();

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://example.foundry.azure.com/v1/messages",
        headers: &mut headers,
    };
    authorizer.authorize(&mut req).await.unwrap();

    assert_eq!(
        mock.counter.load(Ordering::SeqCst),
        2,
        "a private cached token from a previous AuthMachine lease generation must not win when the lease is released and reacquired"
    );
}

#[tokio::test]
async fn observer_failure_fails_closed_without_authorization_header() {
    let mock = start_mock("lease-observer-fails", 3600).await;
    let handle = Arc::new(RecordingAuthLeaseHandle::fail_on("acquire_lease"));
    let lease_key = LeaseKey::new(
        RealmId::parse("dev").unwrap(),
        BindingId::parse("azure_foundry").unwrap(),
        Some(ProfileId::parse("foundry_azure_ad").unwrap()),
    );
    let lease_handle: Arc<dyn AuthLeaseHandle> = handle;
    let authorizer = AzureAdAuthorizer::new(
        "https://cognitiveservices.azure.com/.default",
        creds(&mock.base_url),
    )
    .with_auth_lease_observer(lease_handle, lease_key)
    .with_token_url_override(format!("{}/tenant-id/oauth2/v2.0/token", mock.base_url));

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://example.foundry.azure.com/v1/messages",
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
async fn short_lived_token_refresh_uses_auth_lease_lifecycle() {
    let mock = start_mock("short-lived-lease-token", 30).await;
    let handle = Arc::new(RecordingAuthLeaseHandle::default());
    let lease_key = LeaseKey::new(
        RealmId::parse("dev").unwrap(),
        BindingId::parse("azure_foundry").unwrap(),
        Some(ProfileId::parse("foundry_azure_ad").unwrap()),
    );
    let lease_handle: Arc<dyn AuthLeaseHandle> = handle.clone();
    let authorizer = AzureAdAuthorizer::new(
        "https://cognitiveservices.azure.com/.default",
        creds(&mock.base_url),
    )
    .with_auth_lease_observer(lease_handle, lease_key)
    .with_token_url_override(format!("{}/tenant-id/oauth2/v2.0/token", mock.base_url));

    for _ in 0..2 {
        let mut headers = Vec::new();
        let mut req = HttpAuthorizationRequest {
            method: "POST",
            url: "https://example.foundry.azure.com/v1/messages",
            headers: &mut headers,
        };
        authorizer.authorize(&mut req).await.unwrap();
    }

    let events = handle.events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, LeaseEvent::BeginRefresh(_))),
        "refresh start must be published to AuthMachine"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, LeaseEvent::CompleteRefresh(_, _))),
        "refresh success must complete through AuthMachine"
    );
    assert_eq!(
        handle.acquired().len(),
        1,
        "post-acquire refreshes must use begin/complete instead of repeated acquire"
    );
}

#[tokio::test]
async fn refresh_failure_is_visible_on_auth_lease_snapshot() {
    let mock = start_mock_with_failure("expires-before-refresh", 30, Some(2)).await;
    let handle = Arc::new(RecordingAuthLeaseHandle::default());
    let lease_key = LeaseKey::new(
        RealmId::parse("dev").unwrap(),
        BindingId::parse("azure_foundry").unwrap(),
        Some(ProfileId::parse("foundry_azure_ad").unwrap()),
    );
    let lease_handle: Arc<dyn AuthLeaseHandle> = handle.clone();
    let authorizer = AzureAdAuthorizer::new(
        "https://cognitiveservices.azure.com/.default",
        creds(&mock.base_url),
    )
    .with_auth_lease_observer(lease_handle, lease_key.clone())
    .with_token_url_override(format!("{}/tenant-id/oauth2/v2.0/token", mock.base_url));

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://example.foundry.azure.com/v1/messages",
        headers: &mut headers,
    };
    authorizer.authorize(&mut req).await.unwrap();

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://example.foundry.azure.com/v1/messages",
        headers: &mut headers,
    };
    let err = authorizer.authorize(&mut req).await.unwrap_err();
    assert!(
        matches!(err, meerkat_core::AuthError::RefreshFailed(_)),
        "token endpoint failure should still surface as refresh failure, got {err:?}"
    );
    assert!(
        headers
            .iter()
            .all(|(name, _)| !name.eq_ignore_ascii_case("authorization")),
        "refresh failure must not fall back to the stale provider-local token"
    );

    let snapshot = handle.snapshot(&lease_key);
    assert_eq!(
        snapshot.phase,
        Some(AuthLeasePhase::ReauthRequired),
        "failed cloud token refresh must leave a visible AuthMachine failure state"
    );
    assert!(
        handle
            .events()
            .iter()
            .any(|event| matches!(event, LeaseEvent::RefreshFailed(_, true))),
        "invalid Azure client refresh failure should be marked permanent"
    );
}

#[tokio::test]
async fn transient_token_endpoint_failure_keeps_auth_lease_retryable() {
    let mock = start_mock_with_config(
        "expires-before-transient-refresh",
        vec![30],
        Some(MockFailure {
            from_call: 2,
            status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
            body: serde_json::json!({"error": "temporarily_unavailable"}),
        }),
        None,
    )
    .await;
    let handle = Arc::new(RecordingAuthLeaseHandle::default());
    let lease_key = LeaseKey::new(
        RealmId::parse("dev").unwrap(),
        BindingId::parse("azure_foundry").unwrap(),
        Some(ProfileId::parse("foundry_azure_ad").unwrap()),
    );
    let lease_handle: Arc<dyn AuthLeaseHandle> = handle.clone();
    let authorizer = AzureAdAuthorizer::new(
        "https://cognitiveservices.azure.com/.default",
        creds(&mock.base_url),
    )
    .with_auth_lease_observer(lease_handle, lease_key.clone())
    .with_token_url_override(format!("{}/tenant-id/oauth2/v2.0/token", mock.base_url));

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://example.foundry.azure.com/v1/messages",
        headers: &mut headers,
    };
    authorizer.authorize(&mut req).await.unwrap();

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://example.foundry.azure.com/v1/messages",
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
        "transient Azure endpoint failure must remain retryable in AuthMachine"
    );
    assert!(
        handle
            .events()
            .iter()
            .any(|event| matches!(event, LeaseEvent::RefreshFailed(_, false))),
        "transient Azure endpoint failure should not force reauth"
    );
}

#[tokio::test]
async fn concurrent_refresh_observers_wait_for_inflight_refresh() {
    let mock =
        start_mock_with_config("concurrent-refresh-token", vec![30, 3600], None, Some(2)).await;
    let handle = Arc::new(RecordingAuthLeaseHandle::default());
    let lease_key = LeaseKey::new(
        RealmId::parse("dev").unwrap(),
        BindingId::parse("azure_foundry").unwrap(),
        Some(ProfileId::parse("foundry_azure_ad").unwrap()),
    );
    let lease_handle: Arc<dyn AuthLeaseHandle> = handle.clone();
    let authorizer = Arc::new(
        AzureAdAuthorizer::new(
            "https://cognitiveservices.azure.com/.default",
            creds(&mock.base_url),
        )
        .with_auth_lease_observer(lease_handle, lease_key)
        .with_token_url_override(format!("{}/tenant-id/oauth2/v2.0/token", mock.base_url)),
    );

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://example.foundry.azure.com/v1/messages",
        headers: &mut headers,
    };
    authorizer.authorize(&mut req).await.unwrap();

    let first = {
        let authorizer = authorizer.clone();
        tokio::spawn(async move {
            let mut headers = Vec::new();
            let mut req = HttpAuthorizationRequest {
                method: "POST",
                url: "https://example.foundry.azure.com/v1/messages",
                headers: &mut headers,
            };
            authorizer.authorize(&mut req).await
        })
    };

    timeout(TokioDuration::from_secs(1), async {
        loop {
            if handle
                .events()
                .iter()
                .any(|event| matches!(event, LeaseEvent::BeginRefresh(_)))
            {
                break;
            }
            sleep(TokioDuration::from_millis(5)).await;
        }
    })
    .await
    .expect("first task should enter the refresh lease window");

    let second = {
        let authorizer = authorizer.clone();
        tokio::spawn(async move {
            let mut headers = Vec::new();
            let mut req = HttpAuthorizationRequest {
                method: "POST",
                url: "https://example.foundry.azure.com/v1/messages",
                headers: &mut headers,
            };
            authorizer.authorize(&mut req).await
        })
    };

    let (first, second) = tokio::join!(first, second);
    first.unwrap().unwrap();
    second.unwrap().unwrap();
    assert_eq!(
        mock.counter.load(Ordering::SeqCst),
        2,
        "concurrent observer should wait for the in-flight refresh instead of starting another"
    );
}

#[tokio::test]
async fn separate_authorizers_wait_when_shared_lease_is_refreshing() {
    let mock =
        start_mock_with_config("shared-lease-refresh-token", vec![30, 3600], None, Some(2)).await;
    let handle = Arc::new(RecordingAuthLeaseHandle::default());
    let lease_key = LeaseKey::new(
        RealmId::parse("dev").unwrap(),
        BindingId::parse("azure_foundry").unwrap(),
        Some(ProfileId::parse("foundry_azure_ad").unwrap()),
    );
    let lease_handle_a: Arc<dyn AuthLeaseHandle> = handle.clone();
    let lease_handle_b: Arc<dyn AuthLeaseHandle> = handle.clone();
    let token_url = format!("{}/tenant-id/oauth2/v2.0/token", mock.base_url);
    let authorizer_a = Arc::new(
        AzureAdAuthorizer::new(
            "https://cognitiveservices.azure.com/.default",
            creds(&mock.base_url),
        )
        .with_auth_lease_observer(lease_handle_a, lease_key.clone())
        .with_token_url_override(token_url.clone()),
    );
    let authorizer_b = Arc::new(
        AzureAdAuthorizer::new(
            "https://cognitiveservices.azure.com/.default",
            creds(&mock.base_url),
        )
        .with_auth_lease_observer(lease_handle_b, lease_key)
        .with_token_url_override(token_url),
    );

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://example.foundry.azure.com/v1/messages",
        headers: &mut headers,
    };
    authorizer_a.authorize(&mut req).await.unwrap();

    let first = {
        let authorizer = authorizer_a.clone();
        tokio::spawn(async move {
            let mut headers = Vec::new();
            let mut req = HttpAuthorizationRequest {
                method: "POST",
                url: "https://example.foundry.azure.com/v1/messages",
                headers: &mut headers,
            };
            authorizer.authorize(&mut req).await
        })
    };

    timeout(TokioDuration::from_secs(1), async {
        loop {
            if handle
                .events()
                .iter()
                .any(|event| matches!(event, LeaseEvent::BeginRefresh(_)))
            {
                break;
            }
            sleep(TokioDuration::from_millis(5)).await;
        }
    })
    .await
    .expect("first authorizer should enter the shared refresh lease window");

    let second = {
        let authorizer = authorizer_b.clone();
        tokio::spawn(async move {
            let mut headers = Vec::new();
            let mut req = HttpAuthorizationRequest {
                method: "POST",
                url: "https://example.foundry.azure.com/v1/messages",
                headers: &mut headers,
            };
            authorizer.authorize(&mut req).await
        })
    };

    let (first, second) = tokio::join!(first, second);
    first.unwrap().unwrap();
    second
        .unwrap()
        .expect("shared lease Refreshing must not fail a separate authorizer");
    assert_eq!(
        mock.counter.load(Ordering::SeqCst),
        3,
        "second authorizer has no private cache, but must wait for lease truth before refreshing"
    );
}

#[tokio::test]
async fn from_env_reads_azure_credentials() {
    let c = AzureClientCredentials::from_env(|k| match k {
        "AZURE_TENANT_ID" => Some("env-tenant".into()),
        "AZURE_CLIENT_ID" => Some("env-client".into()),
        "AZURE_CLIENT_SECRET" => Some("env-secret".into()),
        _ => None,
    })
    .unwrap();
    assert_eq!(c.tenant_id, "env-tenant");
    assert_eq!(c.client_id, "env-client");
    assert_eq!(c.client_secret, "env-secret");
    assert_eq!(c.authority_host, "https://login.microsoftonline.com");

    let err = AzureClientCredentials::from_env(|_| None).unwrap_err();
    assert!(matches!(
        err,
        meerkat_auth_core::authorizers::AzureAuthError::MissingEnv("AZURE_TENANT_ID")
    ));
}

#[tokio::test]
async fn token_endpoint_error_propagates_as_refresh_failed() {
    // Point at an endpoint that returns 401 — expect AuthError::RefreshFailed.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = Router::new().route(
        "/tenant-id/oauth2/v2.0/token",
        post(|| async {
            (
                axum::http::StatusCode::UNAUTHORIZED,
                "{\"error\":\"invalid_client\"}",
            )
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let base_url = format!("http://{addr}");
    let (authorizer, _, _) = with_recording_auth_lease(
        AzureAdAuthorizer::new(
            "https://cognitiveservices.azure.com/.default",
            creds(&base_url),
        )
        .with_token_url_override(format!("{base_url}/tenant-id/oauth2/v2.0/token")),
    );

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://x.azure.com/",
        headers: &mut headers,
    };
    let err = authorizer.authorize(&mut req).await.unwrap_err();
    assert!(
        matches!(err, meerkat_core::AuthError::RefreshFailed(_)),
        "got {err:?}"
    );
}
