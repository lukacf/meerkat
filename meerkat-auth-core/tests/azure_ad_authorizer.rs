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
use axum::response::Json;
use axum::routing::post;
use chrono::Utc;
use serde::Deserialize;
use tokio::net::TcpListener;

use meerkat_auth_core::authorizers::{AzureAdAuthorizer, AzureClientCredentials};
use meerkat_core::handles::{AuthLeaseHandle, AuthLeaseSnapshot, DslTransitionError, LeaseKey};
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
    expires_in: u64,
}

async fn token_handler(
    State(state): State<MockState>,
    Form(form): Form<TokenForm>,
) -> Json<serde_json::Value> {
    state.counter.fetch_add(1, Ordering::SeqCst);
    state.captured.lock().unwrap().push(form);
    Json(serde_json::json!({
        "access_token": state.token_value,
        "token_type": "Bearer",
        "expires_in": state.expires_in,
    }))
}

struct MockServer {
    base_url: String,
    counter: Arc<AtomicUsize>,
    captured: Arc<std::sync::Mutex<Vec<TokenForm>>>,
}

#[derive(Default)]
struct RecordingAuthLeaseHandle {
    acquired: Mutex<Vec<(LeaseKey, u64)>>,
}

impl AuthLeaseHandle for RecordingAuthLeaseHandle {
    fn acquire_lease(
        &self,
        lease_key: &LeaseKey,
        expires_at: u64,
    ) -> Result<(), DslTransitionError> {
        self.acquired
            .lock()
            .unwrap()
            .push((lease_key.clone(), expires_at));
        Ok(())
    }

    fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn begin_refresh(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn complete_refresh(
        &self,
        _lease_key: &LeaseKey,
        _new_expires_at: u64,
        _now: u64,
    ) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn refresh_failed(
        &self,
        _lease_key: &LeaseKey,
        _permanent: bool,
    ) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
        AuthLeaseSnapshot {
            phase: None,
            expires_at: None,
        }
    }
}

async fn start_mock(token_value: &str, expires_in: u64) -> MockServer {
    let counter = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
    let state = MockState {
        counter: counter.clone(),
        captured: captured.clone(),
        token_value: token_value.into(),
        expires_in,
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

#[tokio::test]
async fn authorize_adds_bearer_header_from_token_endpoint() {
    let mock = start_mock("azure-access-token-xyz", 3600).await;
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
    authorizer.authorize(&mut req).await.unwrap();

    let auth = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .unwrap();
    assert_eq!(auth.1, "Bearer azure-access-token-xyz");
    assert!(
        authorizer.expires_at().is_some(),
        "Azure AD token expiry must be observable through HttpAuthorizer"
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
async fn token_is_cached_between_authorize_calls() {
    let mock = start_mock("cached-token", 3600).await;
    let authorizer = AzureAdAuthorizer::new(
        "https://cognitiveservices.azure.com/.default",
        creds(&mock.base_url),
    )
    .with_token_url_override(format!("{}/tenant-id/oauth2/v2.0/token", mock.base_url));

    for _ in 0..5 {
        let mut headers = Vec::new();
        let mut req = HttpAuthorizationRequest {
            method: "POST",
            url: "https://example.foundry.azure.com/v1/messages",
            headers: &mut headers,
        };
        authorizer.authorize(&mut req).await.unwrap();
    }
    assert_eq!(
        mock.counter.load(Ordering::SeqCst),
        1,
        "expected 1 token fetch, got {}",
        mock.counter.load(Ordering::SeqCst),
    );
}

#[tokio::test]
async fn short_lived_token_expiry_is_observable_and_refetched() {
    let mock = start_mock("short-lived-token", 30).await;
    let authorizer = AzureAdAuthorizer::new(
        "https://cognitiveservices.azure.com/.default",
        creds(&mock.base_url),
    )
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

    let mut headers = Vec::new();
    let mut req = HttpAuthorizationRequest {
        method: "POST",
        url: "https://example.foundry.azure.com/v1/messages",
        headers: &mut headers,
    };
    authorizer.authorize(&mut req).await.unwrap();

    let acquired = handle.acquired.lock().unwrap();
    assert_eq!(acquired.len(), 1);
    assert_eq!(acquired[0].0, lease_key);
    assert!(
        acquired[0].1 > Utc::now().timestamp().max(0) as u64,
        "published auth-machine expiry must reflect the fetched token"
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
    let authorizer = AzureAdAuthorizer::new(
        "https://cognitiveservices.azure.com/.default",
        creds(&base_url),
    )
    .with_token_url_override(format!("{base_url}/tenant-id/oauth2/v2.0/token"));

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
