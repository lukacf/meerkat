//! Public cloud HTTP controls for time spent waiting on lifecycle custody.
//! Uses existing authorizer APIs and real time; no private observer clock hook.
#![cfg(all(
    not(target_arch = "wasm32"),
    any(feature = "azure-ad", feature = "gcp-auth")
))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use meerkat_core::handles::{
    AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, AuthLeaseHandle, AuthLeasePhase, LeaseKey,
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
    first_expiry: u64,
    second_expiry: u64,
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
    Json(serde_json::json!({
        "access_token": format!("cloud-token-{index}"),
        "token_type":"Bearer",
        "expires_in": match index {
            1 => state.first_expiry,
            2 => state.second_expiry,
            _ => 3_600,
        },
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
    async fn new(cloud: Cloud, first_expiry: u64, second_expiry: u64, gate_second: bool) -> Self {
        let endpoint = Endpoint {
            calls: Arc::new(AtomicUsize::new(0)),
            wire: Arc::new(Mutex::new(Vec::new())),
            arrived: Arc::new(Semaphore::new(0)),
            resume: Arc::new(Semaphore::new(0)),
            gate_second,
            first_expiry,
            second_expiry,
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
            RealmId::parse("cloud-custody-clock").unwrap(),
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

async fn cache_wait_crosses_refresh_window(cloud: Cloud) {
    // One second outside the generated owner's current refresh window.
    let fixture = Fixture::new(cloud, AUTH_LEASE_TTL_REFRESH_WINDOW_SECS + 1, 3_600, false).await;
    assert_bearer(&fixture.authorize().await.unwrap(), "cloud-token-1");
    let before = fixture.owner.snapshot(&fixture.key);
    assert_eq!(before.phase, Some(AuthLeasePhase::Valid));
    let expiry = before.expires_at.unwrap();
    assert_bearer(&fixture.authorize().await.unwrap(), "cloud-token-1");
    assert_eq!(
        fixture.calls(),
        1,
        "current cache is a no-HTTP positive control"
    );

    let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&fixture.key).await;
    let before_poll = chrono::Utc::now().timestamp().max(0) as u64;
    assert!(
        before_poll + AUTH_LEASE_TTL_REFRESH_WINDOW_SECS <= expiry,
        "setup missed the current window: now={before_poll}, expiry={expiry}"
    );
    let mut attempt = Box::pin(fixture.authorize());
    std::future::poll_fn(|cx| {
        assert!(std::future::Future::poll(attempt.as_mut(), cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(fixture.calls(), 1);
    assert_eq!(fixture.owner.snapshot(&fixture.key), before);

    let threshold = expiry - AUTH_LEASE_TTL_REFRESH_WINDOW_SECS + 1;
    let threshold_millis = i64::try_from(threshold).unwrap() * 1_000 + 100;
    let wait_millis = (threshold_millis - chrono::Utc::now().timestamp_millis()).max(0);
    assert!(
        wait_millis <= 2_100,
        "bounded threshold wait was {wait_millis}ms"
    );
    tokio::time::sleep(Duration::from_millis(wait_millis as u64)).await;
    let after_wait = chrono::Utc::now().timestamp().max(0) as u64;
    assert!(after_wait + AUTH_LEASE_TTL_REFRESH_WINDOW_SECS > expiry);
    assert!(
        after_wait < expiry,
        "test crossed expiry rather than only refresh window"
    );
    drop(guard);

    let headers = timeout(Duration::from_secs(5), attempt)
        .await
        .unwrap()
        .unwrap();
    assert_bearer(&headers, "cloud-token-2");
    assert_eq!(
        fixture.calls(),
        2,
        "custody wait must not reuse the now-expiring token"
    );
    let after = fixture.owner.snapshot(&fixture.key);
    assert_eq!(after.phase, Some(AuthLeasePhase::Valid));
    assert!(after.generation > before.generation);
    assert!(after.expires_at.unwrap() > expiry);
    fixture.assert_wire(cloud);
}

async fn completion_wait_crosses_response_expiry(cloud: Cloud) {
    let fixture = Fixture::new(cloud, 0, 1, true).await;
    assert_bearer(&fixture.authorize().await.unwrap(), "cloud-token-1");
    assert_eq!(fixture.calls(), 1);
    let mut attempt = Box::pin(fixture.authorize());
    timeout(Duration::from_secs(5), async {
        tokio::select! {
            result = &mut attempt => panic!("refresh completed before gated HTTP: {result:?}"),
            permit = fixture.endpoint.arrived.acquire() => permit.unwrap().forget(),
        }
    })
    .await
    .unwrap();
    let started = fixture.owner.snapshot(&fixture.key);
    assert_eq!(started.phase, Some(AuthLeasePhase::Refreshing));
    assert_eq!(fixture.calls(), 2);

    // The real endpoint is waiting, so this acquisition also proves that no
    // normalized lifecycle guard spans token HTTP.
    let guard = timeout(
        Duration::from_secs(2),
        meerkat_core::acquire_auth_login_lifecycle_guard(&fixture.key),
    )
    .await
    .unwrap();
    fixture.endpoint.resume.add_permits(1);
    // Keep the actual authorizer future polled while the one-second response
    // is delivered and its completion waits on custody. The real-clock test
    // assumes loopback delivery within this 2.2s interval; the separate private
    // observer tests provide deterministic post-acquisition clock coverage.
    tokio::select! {
        result = &mut attempt => panic!("completion bypassed held custody: {result:?}"),
        () = tokio::time::sleep(Duration::from_millis(2_200)) => {},
    }
    assert_eq!(fixture.owner.snapshot(&fixture.key), started);
    drop(guard);

    let result = timeout(Duration::from_secs(5), attempt).await.unwrap();
    assert!(
        matches!(&result, Err(AuthError::Other(_))),
        "expired response must not be accepted after custody wait: {result:?}"
    );
    let closed = fixture.owner.snapshot(&fixture.key);
    assert_eq!(
        fixture.calls(),
        2,
        "no hidden retry may conceal rejected completion"
    );

    // A new explicit request owns exchange three. It must not wait on the
    // already-finished HTTP owner or reuse the rejected response/cache.
    let follow_up = timeout(Duration::from_secs(5), fixture.authorize()).await;
    assert!(
        follow_up.is_ok(),
        "rejected completion stranded its owner: closed={closed:?}, result={follow_up:?}"
    );
    assert_bearer(&follow_up.unwrap().unwrap(), "cloud-token-3");
    assert_eq!(fixture.calls(), 3);
    assert_eq!(closed.phase, Some(AuthLeasePhase::Expiring));
    assert_eq!(closed.generation, started.generation);
    assert_eq!(closed.expires_at, started.expires_at);
    assert_eq!(closed.credential_present, started.credential_present);
    assert_eq!(
        closed.credential_published_at_millis,
        started.credential_published_at_millis
    );
    let current = fixture.owner.snapshot(&fixture.key);
    assert_eq!(current.phase, Some(AuthLeasePhase::Valid));
    assert_eq!(current.generation, started.generation + 1);
    assert!(current.expires_at.unwrap() > started.expires_at.unwrap());
    assert_bearer(&fixture.authorize().await.unwrap(), "cloud-token-3");
    assert_eq!(fixture.calls(), 3, "healthy follow-up is now cached");
    assert_eq!(fixture.owner.snapshot(&fixture.key), current);
    fixture.assert_wire(cloud);
}

#[cfg(feature = "azure-ad")]
mod azure {
    use super::*;

    #[tokio::test]
    async fn current_cache_wait_crosses_refresh_window() {
        cache_wait_crosses_refresh_window(Cloud::Azure).await;
    }

    #[tokio::test]
    async fn refresh_completion_wait_crosses_response_expiry() {
        completion_wait_crosses_response_expiry(Cloud::Azure).await;
    }
}

#[cfg(feature = "gcp-auth")]
mod google {
    use super::*;

    #[tokio::test]
    async fn current_cache_wait_crosses_refresh_window() {
        cache_wait_crosses_refresh_window(Cloud::Google).await;
    }

    #[tokio::test]
    async fn refresh_completion_wait_crosses_response_expiry() {
        completion_wait_crosses_response_expiry(Cloud::Google).await;
    }
}
