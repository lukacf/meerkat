//! Real cloud HTTP and generated-owner controls for request-free maintenance.
//! These assertions use only APIs present before the maintenance repair.
#![cfg(all(
    not(target_arch = "wasm32"),
    any(feature = "azure-ad", feature = "gcp-auth")
))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use meerkat_core::handles::{
    AuthLeaseHandle, AuthLeasePhase, CredentialUseDisposition, CredentialUseIntent, LeaseKey,
};
use meerkat_core::{AuthError, BindingId, HttpAuthorizationRequest, HttpAuthorizer, RealmId};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;
use tokio::time::{Duration, timeout};

#[derive(Clone, Copy, Debug)]
enum Cloud {
    #[cfg(feature = "azure-ad")]
    Azure,
    #[cfg(feature = "gcp-auth")]
    Google,
}

#[derive(Clone)]
struct Endpoint {
    calls: Arc<AtomicUsize>,
    wire: Arc<Mutex<Vec<(String, String)>>>,
    arrived: Arc<Semaphore>,
    resume: Arc<Semaphore>,
    gate_second: bool,
    fail_second: bool,
    expired_first: bool,
}

async fn token_endpoint(
    State(state): State<Endpoint>,
    method: axum::http::Method,
    headers: axum::http::HeaderMap,
    body: String,
) -> Response {
    let index = state.calls.fetch_add(1, Ordering::SeqCst) + 1;
    if method == axum::http::Method::GET {
        assert_eq!(headers.get("metadata-flavor").unwrap(), "Google");
    }
    state.wire.lock().unwrap().push((method.to_string(), body));
    if index == 2 && state.gate_second {
        state.arrived.add_permits(1);
        state.resume.acquire().await.unwrap().forget();
    }
    if index == 2 && state.fail_second {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error":"invalid_grant"})),
        )
            .into_response();
    }
    Json(serde_json::json!({
        "access_token": format!("cloud-token-{index}"),
        "token_type":"Bearer",
        "expires_in": if index == 1 && state.expired_first { 0 } else { 3600 },
    }))
    .into_response()
}

struct Fixture {
    authorizer: Arc<dyn HttpAuthorizer>,
    owner: Arc<meerkat_runtime::RuntimeAuthLeaseHandle>,
    key: LeaseKey,
    endpoint: Endpoint,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new(cloud: Cloud, expired_first: bool, gate_second: bool, fail_second: bool) -> Self {
        let endpoint = Endpoint {
            calls: Arc::new(AtomicUsize::new(0)),
            wire: Arc::new(Mutex::new(Vec::new())),
            arrived: Arc::new(Semaphore::new(0)),
            resume: Arc::new(Semaphore::new(0)),
            gate_second,
            fail_second,
            expired_first,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/token", listener.local_addr().unwrap());
        let app = Router::new()
            .route(
                "/token",
                axum::routing::get(token_endpoint).post(token_endpoint),
            )
            .with_state(endpoint.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let owner = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
        let generated = meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(Arc::clone(&owner)).unwrap();
        let key = LeaseKey::new(
            RealmId::parse("cloud-maintenance").unwrap(),
            BindingId::parse(format!("binding-{}", uuid::Uuid::new_v4())).unwrap(),
            None,
        );
        let authorizer: Arc<dyn HttpAuthorizer> = match cloud {
            #[cfg(feature = "azure-ad")]
            Cloud::Azure => Arc::new(
                meerkat_auth_core::authorizers::AzureAdAuthorizer::new(
                    "https://resource.test/.default",
                    meerkat_auth_core::authorizers::AzureClientCredentials {
                        tenant_id: "tenant".into(),
                        client_id: "client".into(),
                        client_secret: "secret".into(),
                        authority_host: "https://unused.test".into(),
                    },
                )
                .with_token_url_override(url)
                .with_auth_lease_observer(generated, key.clone()),
            ),
            #[cfg(feature = "gcp-auth")]
            Cloud::Google => Arc::new(
                meerkat_auth_core::authorizers::GoogleAuthAuthorizer::with_env_lookup(
                    meerkat_auth_core::authorizers::GoogleAuthChain::ComputeOnly,
                    Arc::new(|_| None),
                )
                .with_metadata_url_override(url)
                .with_auth_lease_observer(generated, key.clone()),
            ),
        };
        Self {
            authorizer,
            owner,
            key,
            endpoint,
            server,
        }
    }

    async fn authorize(&self) -> Result<Vec<(String, String)>, AuthError> {
        authorize(Arc::clone(&self.authorizer)).await
    }

    fn calls(&self) -> usize {
        self.endpoint.calls.load(Ordering::SeqCst)
    }

    fn assert_wire(&self, cloud: Cloud) {
        let wire = self.endpoint.wire.lock().unwrap();
        assert!(!wire.is_empty());
        for (method, body) in wire.iter() {
            match cloud {
                #[cfg(feature = "azure-ad")]
                Cloud::Azure => {
                    assert_eq!(method, "POST");
                    let fields: std::collections::BTreeMap<_, _> = body
                        .split('&')
                        .map(|field| field.split_once('=').unwrap())
                        .collect();
                    assert_eq!(fields.get("grant_type"), Some(&"client_credentials"));
                    assert_eq!(fields.get("client_id"), Some(&"client"));
                    assert_eq!(fields.get("client_secret"), Some(&"secret"));
                    assert_eq!(
                        fields.get("scope"),
                        Some(&"https%3A%2F%2Fresource.test%2F.default")
                    );
                }
                #[cfg(feature = "gcp-auth")]
                Cloud::Google => {
                    assert_eq!(method, "GET");
                    assert!(body.is_empty());
                }
            }
        }
    }
}

async fn authorize(
    authorizer: Arc<dyn HttpAuthorizer>,
) -> Result<Vec<(String, String)>, AuthError> {
    let mut headers = Vec::new();
    authorizer
        .authorize(&mut HttpAuthorizationRequest {
            method: "POST",
            url: "https://resource.test/model",
            headers: &mut headers,
        })
        .await?;
    Ok(headers)
}

fn assert_bearer(headers: &[(String, String)], token: &str) {
    assert_eq!(
        headers,
        &[("Authorization".to_owned(), format!("Bearer {token}"))]
    );
}

async fn initial_and_current(cloud: Cloud) {
    let fixture = Fixture::new(cloud, false, false, false).await;
    assert_eq!(fixture.owner.snapshot(&fixture.key).phase, None);
    assert_bearer(&fixture.authorize().await.unwrap(), "cloud-token-1");
    let current = fixture.owner.snapshot(&fixture.key);
    assert_eq!(current.phase, Some(AuthLeasePhase::Valid));
    fixture.authorizer.prepare_request().await.unwrap();
    assert_bearer(&fixture.authorize().await.unwrap(), "cloud-token-1");
    assert_eq!(fixture.owner.snapshot(&fixture.key), current);
    assert_eq!(fixture.calls(), 1, "current maintenance cannot add HTTP");
    fixture.assert_wire(cloud);
}

async fn expired_maintenance(cloud: Cloud) {
    let fixture = Fixture::new(cloud, true, false, false).await;
    fixture.authorize().await.unwrap();
    fixture
        .owner
        .observe_credential_freshness(&fixture.key, chrono::Utc::now().timestamp() as u64, 60)
        .unwrap();
    assert_eq!(
        fixture.owner.snapshot(&fixture.key).phase,
        Some(AuthLeasePhase::Expired)
    );
    let old_generation = fixture.owner.snapshot(&fixture.key).generation;
    fixture.authorizer.prepare_request().await.unwrap();
    assert_eq!(
        fixture.calls(),
        2,
        "request-free maintenance must make the real refresh request"
    );
    assert_eq!(
        fixture.owner.snapshot(&fixture.key).phase,
        Some(AuthLeasePhase::Valid)
    );
    assert!(fixture.owner.snapshot(&fixture.key).generation > old_generation);
    assert_bearer(&fixture.authorize().await.unwrap(), "cloud-token-2");
    assert_eq!(fixture.calls(), 2);
    fixture.assert_wire(cloud);
}

async fn absence_and_release(cloud: Cloud) {
    let fixture = Fixture::new(cloud, false, false, false).await;
    let absent = fixture.owner.snapshot(&fixture.key);
    assert!(matches!(
        fixture.authorizer.prepare_request().await,
        Err(AuthError::RefreshRequired)
    ));
    assert_eq!(fixture.owner.snapshot(&fixture.key), absent);
    assert_eq!(fixture.calls(), 0, "maintenance cannot initially acquire");
    // Ordinary authorization still legitimately acquires the initial credential.
    fixture.authorize().await.unwrap();
    fixture.owner.release_lease(&fixture.key).unwrap();
    let released = fixture.owner.snapshot(&fixture.key);
    assert!(matches!(
        fixture.authorizer.prepare_request().await,
        Err(AuthError::RefreshRequired)
    ));
    assert_eq!(fixture.owner.snapshot(&fixture.key), released);
    assert_eq!(
        fixture.calls(),
        1,
        "maintenance cannot resurrect a released credential"
    );
}

async fn release_before_start(cloud: Cloud) {
    let fixture = Fixture::new(cloud, true, false, false).await;
    fixture.authorize().await.unwrap();
    let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&fixture.key).await;
    let mut pending = Box::pin(fixture.authorizer.prepare_request());
    assert!(
        matches!(futures::poll!(&mut pending), std::task::Poll::Pending),
        "maintenance must consult actual lease custody before starting"
    );
    fixture
        .owner
        .release_lease_with_guard(&fixture.key, &guard)
        .unwrap();
    let released = fixture.owner.snapshot(&fixture.key);
    drop(guard);
    let result = timeout(Duration::from_secs(2), pending)
        .await
        .expect("maintenance resumes");
    assert!(
        matches!(result, Err(AuthError::RefreshRequired)),
        "{result:?}"
    );
    assert_eq!(fixture.owner.snapshot(&fixture.key), released);
    assert_eq!(fixture.calls(), 1, "release won before any refresh HTTP");
}

async fn reauth_control(cloud: Cloud) {
    let fixture = Fixture::new(cloud, false, false, false).await;
    fixture.authorize().await.unwrap();
    fixture.owner.mark_reauth_required(&fixture.key).unwrap();
    let reauth = fixture.owner.snapshot(&fixture.key);
    assert!(matches!(
        fixture.authorizer.prepare_request().await,
        Err(AuthError::UserReauthRequired)
    ));
    assert_eq!(fixture.owner.snapshot(&fixture.key), reauth);
    assert_eq!(fixture.calls(), 1);
}

#[derive(Clone, Copy, Debug)]
enum Replacement {
    Released,
    Reacquired,
    NewRefresh,
}

async fn stale_http_result(cloud: Cloud, fail: bool) {
    for replacement in [
        Replacement::Released,
        Replacement::Reacquired,
        Replacement::NewRefresh,
    ] {
        let fixture = Fixture::new(cloud, true, true, fail).await;
        fixture.authorize().await.unwrap();
        let pending = tokio::spawn(authorize(Arc::clone(&fixture.authorizer)));
        timeout(Duration::from_secs(3), fixture.endpoint.arrived.acquire())
            .await
            .expect("actual second HTTP request")
            .unwrap()
            .forget();
        assert_eq!(
            fixture.owner.snapshot(&fixture.key).phase,
            Some(AuthLeasePhase::Refreshing)
        );
        // This must succeed while HTTP is held: no lifecycle custody across I/O.
        let guard = timeout(
            Duration::from_secs(2),
            meerkat_core::acquire_auth_login_lifecycle_guard(&fixture.key),
        )
        .await
        .expect("HTTP cannot retain lease custody");
        fixture
            .owner
            .release_lease_with_guard(&fixture.key, &guard)
            .unwrap();
        if !matches!(replacement, Replacement::Released) {
            fixture.owner.acquire_lease(&fixture.key, u64::MAX).unwrap();
        }
        if matches!(replacement, Replacement::NewRefresh) {
            fixture.owner.begin_refresh(&fixture.key).unwrap();
        }
        let actual_replacement = fixture.owner.snapshot(&fixture.key);
        drop(guard);
        fixture.endpoint.resume.add_permits(1);
        let result = timeout(Duration::from_secs(3), pending)
            .await
            .expect("HTTP task finishes")
            .expect("no task panic");
        assert!(
            matches!(result, Err(AuthError::StaleCredential)),
            "{cloud:?}/{replacement:?}/failure={fail}: {result:?}"
        );
        assert_eq!(
            fixture.owner.snapshot(&fixture.key),
            actual_replacement,
            "stale result cannot mutate its replacement"
        );
        assert_eq!(fixture.calls(), 2);
        // Retire the test's explicit new refresh, then a fresh legitimate
        // acquisition must fetch new bytes, never return the old HTTP result.
        let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&fixture.key).await;
        fixture
            .owner
            .release_lease_with_guard(&fixture.key, &guard)
            .unwrap();
        fixture.owner.acquire_lease(&fixture.key, u64::MAX).unwrap();
        drop(guard);
        assert_bearer(&fixture.authorize().await.unwrap(), "cloud-token-3");
        assert_eq!(fixture.calls(), 3);
        assert_eq!(
            fixture
                .owner
                .resolve_credential_use_admission(&fixture.key, CredentialUseIntent::HoldAuthority)
                .unwrap(),
            CredentialUseDisposition::Authorized
        );
        fixture.assert_wire(cloud);
    }
}

macro_rules! cloud_cases {
    ($module:ident, $cloud:expr) => {
        mod $module {
            use super::*;
            #[tokio::test]
            async fn initial_acquisition_and_current_maintenance() {
                initial_and_current($cloud).await;
            }
            #[tokio::test]
            async fn expired_request_free_maintenance() {
                expired_maintenance($cloud).await;
            }
            #[tokio::test]
            async fn maintenance_cannot_acquire_or_resurrect() {
                absence_and_release($cloud).await;
            }
            #[tokio::test]
            async fn release_before_maintenance_start() {
                release_before_start($cloud).await;
            }
            #[tokio::test]
            async fn reauth_is_not_maintenance() {
                reauth_control($cloud).await;
            }
            #[tokio::test]
            async fn stale_success_preserves_replacement() {
                stale_http_result($cloud, false).await;
            }
            #[tokio::test]
            async fn stale_failure_preserves_replacement() {
                stale_http_result($cloud, true).await;
            }
        }
    };
}
#[cfg(feature = "azure-ad")]
cloud_cases!(azure, Cloud::Azure);
#[cfg(feature = "gcp-auth")]
cloud_cases!(google, Cloud::Google);
