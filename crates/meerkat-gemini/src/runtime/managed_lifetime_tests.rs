//! A retained managed client must observe its actual credential owner on reuse.
//! Parent-scoped HTTP plus a child process keeps existing OAuth fixture env local.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::GoogleProviderRuntime;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use meerkat_auth_core::{EphemeralTokenStore, InMemoryCoordinator};
use meerkat_core::auth::{
    PersistedAuthMode, PersistedTokens, ProviderAuthPersistence, TokenKey, TokenStore,
    TokenStoreError,
};
use meerkat_core::handles::{
    AuthLeasePhase, CredentialUseDisposition, CredentialUseIntent, LeaseKey,
};
use meerkat_core::{
    AuthBindingRef, AuthCredentialIdentity, AuthProfile, BackendProfile, BindingId, BindingOrigin,
    BindingPolicy, CredentialSourceSpec, Message, Provider, RealmId, UserMessage,
};
use meerkat_llm_core::provider_runtime::ProviderRuntimeCatalog;
use meerkat_llm_core::provider_runtime::registry::ResolverEnvironment;
use meerkat_llm_core::provider_runtime::runtime::ProviderRuntime;
use meerkat_llm_core::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const CHILD: &str = "runtime::managed_lifetime_tests::managed_lifetime_child";
const MODE_ENV: &str = "MEERKAT_N1_GEMINI_LIFETIME_CASE";
const BASE_ENV: &str = "MEERKAT_N1_GEMINI_LIFETIME_BASE";
const OLD: &str = "managed-pinned-access-old";
const NEW: &str = "managed-pinned-access-new";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StoreCounts {
    loads: usize,
    saves: usize,
    clears: usize,
    lists: usize,
}

struct CountingTokenStore {
    inner: EphemeralTokenStore,
    loads: AtomicUsize,
    saves: AtomicUsize,
    clears: AtomicUsize,
    lists: AtomicUsize,
}

impl CountingTokenStore {
    fn new() -> Self {
        Self {
            inner: EphemeralTokenStore::new(),
            loads: AtomicUsize::new(0),
            saves: AtomicUsize::new(0),
            clears: AtomicUsize::new(0),
            lists: AtomicUsize::new(0),
        }
    }

    fn counts(&self) -> StoreCounts {
        StoreCounts {
            loads: self.loads.load(Ordering::SeqCst),
            saves: self.saves.load(Ordering::SeqCst),
            clears: self.clears.load(Ordering::SeqCst),
            lists: self.lists.load(Ordering::SeqCst),
        }
    }
}

// Count attempted operations, then delegate unchanged to the real memory store.
#[async_trait::async_trait]
impl TokenStore for CountingTokenStore {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        self.inner.load(key).await
    }

    async fn save(&self, key: &TokenKey, tokens: &PersistedTokens) -> Result<(), TokenStoreError> {
        self.saves.fetch_add(1, Ordering::SeqCst);
        self.inner.save(key, tokens).await
    }

    async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        self.clears.fetch_add(1, Ordering::SeqCst);
        self.inner.clear(key).await
    }

    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        self.lists.fetch_add(1, Ordering::SeqCst);
        self.inner.list().await
    }

    fn backend_name(&self) -> &'static str {
        self.inner.backend_name()
    }
}

#[derive(Default, Debug)]
struct Wire {
    model_headers: Vec<String>,
    external_headers: Vec<Option<String>>,
    setup_requests: Vec<(String, serde_json::Value)>,
    model_bodies: Vec<serde_json::Value>,
    refresh_forms: Vec<String>,
}

async fn token(State(wire): State<Arc<Mutex<Wire>>>, body: String) -> Response {
    wire.lock().unwrap().refresh_forms.push(body);
    Json(serde_json::json!({"access_token":NEW,"refresh_token":"managed-refresh-new","token_type":"Bearer","expires_in":10800})).into_response()
}

async fn model(
    State(wire): State<Arc<Mutex<Wire>>>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let mut record = wire.lock().unwrap();
    record.model_headers.push(
        headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string(),
    );
    record.external_headers.push(
        headers
            .get("x-external-owner")
            .map(|h| h.to_str().unwrap().to_string()),
    );
    record.model_bodies.push(body);
    drop(record);
    let event = serde_json::json!({"response":{"candidates":[{"content":{"parts":[{"text":"managed lifetime complete"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":1}},"traceId":"managed-lifetime"});
    (
        StatusCode::OK,
        [("content-type", "text/event-stream")],
        format!("data: {event}\n\n"),
    )
        .into_response()
}

async fn load_project(
    State(wire): State<Arc<Mutex<Wire>>>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    wire.lock().unwrap().setup_requests.push((
        headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string(),
        body,
    ));
    Json(serde_json::json!({
        "cloudaicompanionProject":"managed-project",
        "currentTier":{"id":"standard-tier"}
    }))
    .into_response()
}

async fn run_parent(mode: &str) {
    let wire = Arc::new(Mutex::new(Wire::default()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/google/token", axum::routing::post(token))
        .route(
            "/v1internal:streamGenerateContent",
            axum::routing::post(model),
        )
        .route(
            "/v1internal:loadCodeAssist",
            axum::routing::post(load_project),
        )
        .with_state(Arc::clone(&wire));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", CHILD, "--ignored", "--nocapture"])
        .env(MODE_ENV, mode)
        .env(BASE_ENV, &base)
        .env("MEERKAT_TEST_OAUTH_ENDPOINT_OVERRIDE", "1")
        .env("MEERKAT_TEST_OAUTH_BASE_URL", &base)
        .kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(15), command.output()).await;
    server.abort();
    let output = output
        .expect("child completes without waiting on a production host")
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let record = wire.lock().unwrap();
    assert!(
        stdout.contains("running 1 test"),
        "nonzero child selector: {stdout}\n{stderr}"
    );
    assert!(
        output.status.success(),
        "case={mode}; actual wire={record:?}; child stdout={stdout}; stderr={stderr}"
    );
    let expected_headers = if mode == "expired" {
        vec![format!("Bearer {OLD}"), format!("Bearer {NEW}")]
    } else if mode == "released" {
        vec![format!("Bearer {OLD}")]
    } else {
        vec![format!("Bearer {OLD}"), format!("Bearer {OLD}")]
    };
    assert_eq!(
        record.model_headers, expected_headers,
        "same pinned client's actual wire"
    );
    assert_eq!(record.model_bodies.len(), expected_headers.len());
    assert!(
        record
            .model_bodies
            .iter()
            .all(|body| body["model"] == "gemini-2.5-flash")
    );
    assert!(record.external_headers.iter().all(|external| {
        external.as_deref()
            == if mode == "external-current" {
                Some("retained-external")
            } else {
                None
            }
    }));
    assert!(record.model_bodies.iter().all(|body| {
        body["project"] == "managed-project"
            && body
                .get("request")
                .and_then(|r| r.get("contents"))
                .is_some()
            && body.get("contents").is_none()
    }));
    assert_eq!(
        record.setup_requests.len(),
        usize::from(mode != "external-current")
    );
    for (bearer, body) in &record.setup_requests {
        assert_eq!(bearer, &format!("Bearer {OLD}"));
        assert_eq!(body["metadata"]["pluginType"], "GEMINI");
    }
    assert_eq!(record.refresh_forms.len(), usize::from(mode == "expired"));
    for body in &record.refresh_forms {
        let fields: std::collections::BTreeMap<_, _> = body
            .split('&')
            .map(|item| item.split_once('=').unwrap())
            .collect();
        assert_eq!(fields.get("grant_type"), Some(&"refresh_token"));
        assert_eq!(fields.get("refresh_token"), Some(&"managed-refresh-old"));
    }
}

#[tokio::test]
async fn managed_pinned_current_client_uses_no_refresh() {
    run_parent("current").await;
}
#[tokio::test]
async fn managed_pinned_second_use_refreshes_expired_owner() {
    run_parent("expired").await;
}
#[tokio::test]
async fn managed_pinned_released_owner_cannot_send_or_resurrect() {
    run_parent("released").await;
}
#[tokio::test]
async fn external_pinned_current_client_does_not_gain_managed_refresh() {
    run_parent("external-current").await;
}

struct ExternalFixture;

#[async_trait::async_trait]
impl meerkat_llm_core::provider_runtime::registry::ExternalAuthResolverHandle for ExternalFixture {
    async fn resolve(
        &self,
        binding: &meerkat_llm_core::provider_runtime::binding::ValidatedBinding,
    ) -> Result<meerkat_core::ResolvedAuthEnvelope, meerkat_core::AuthError> {
        assert_eq!(binding.auth_binding_ref().binding.as_str(), "pinned-route");
        let metadata = meerkat_core::AuthMetadata {
            provider_metadata: Some(meerkat_core::ProviderAuthMetadata::Google(
                meerkat_core::GoogleAuthMetadata {
                    project_id: Some("managed-project".into()),
                    code_assist_tier: Some("standard-tier".into()),
                    ..Default::default()
                },
            )),
            ..Default::default()
        };
        Ok(meerkat_core::ResolvedAuthEnvelope::StaticHeaders {
            headers: vec![
                ("Authorization".into(), format!("Bearer {OLD}")),
                ("x-external-owner".into(), "retained-external".into()),
            ],
            metadata,
            expires_at: None,
        })
    }
}

async fn consume(client: &Arc<dyn LlmClient>) -> Result<(), String> {
    let request = LlmRequest::new(
        "gemini-2.5-flash",
        vec![Message::User(UserMessage::text(
            "Complete this small request.",
        ))],
    );
    let mut stream = client.stream(&request);
    let mut completions = 0;
    let mut text = String::new();
    while let Some(event) = stream.next().await {
        match event.map_err(|error| error.to_string())? {
            LlmEvent::TextDelta { delta, .. } => text.push_str(&delta),
            LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { .. },
            } => completions += 1,
            LlmEvent::Done {
                outcome: LlmDoneOutcome::Error { error },
            } => return Err(error.to_string()),
            _ => {}
        }
    }
    assert_eq!(completions, 1, "actual provider stream completes once");
    assert_eq!(
        text, "managed lifetime complete",
        "actual provider codec control"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "subprocess entrypoint with per-process OAuth fixture environment"]
async fn managed_lifetime_child() {
    let mode = std::env::var(MODE_ENV).expect("only the parent starts this child");
    let base = std::env::var(BASE_ENV).unwrap();
    let external = mode == "external-current";
    let account: AuthCredentialIdentity = serde_json::from_value(
        serde_json::json!({"realm":"managed-lifetime", "account":"account-x"}),
    )
    .unwrap();
    let reference = AuthBindingRef {
        realm: RealmId::parse("managed-lifetime").unwrap(),
        binding: BindingId::parse("pinned-route").unwrap(),
        profile: None,
        origin: BindingOrigin::Configured,
    };
    let binding = ProviderRuntimeCatalog::validate_binding_with_credential_identity(
        &reference,
        account.clone(),
        &BackendProfile {
            id: "managed-backend".into(),
            provider: Provider::Gemini,
            backend_kind: "google_code_assist".into(),
            base_url: Some(base),
            options: serde_json::Value::Null,
            server: None,
        },
        &AuthProfile {
            id: "managed-auth".into(),
            provider: Provider::Gemini,
            auth_method: if external {
                "external_authorizer"
            } else {
                "google_oauth"
            }
            .into(),
            source: if external {
                CredentialSourceSpec::ExternalResolver {
                    handle: "retained-external".into(),
                }
            } else {
                CredentialSourceSpec::ManagedStore
            },
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
        &BindingPolicy::default(),
    )
    .unwrap();
    assert_eq!(binding.credential_identity(), &account);
    let key = TokenKey::from_credential_identity(&account);
    let lease = LeaseKey::from_credential_identity(&account);
    let raw_owner = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    let owner =
        meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(
            raw_owner,
        )
        .unwrap();
    let counting_store = Arc::new(CountingTokenStore::new());
    let store: Arc<dyn TokenStore> = counting_store.clone();
    let persistence =
        ProviderAuthPersistence::new(Arc::clone(&store), Arc::new(InMemoryCoordinator::new()));
    let started = Utc::now();
    let clock = Arc::new(AtomicI64::new(started.timestamp()));
    let expires = started + chrono::Duration::hours(1);
    let original = PersistedTokens {
        auth_mode: PersistedAuthMode::GoogleOauth,
        primary_secret: Some(OLD.into()),
        refresh_token: Some("managed-refresh-old".into()),
        // Synthetic unsigned claims exercise metadata projection, not identity verification.
        id_token: Some("header.eyJzdWIiOiJtYW5hZ2VkLXByb3ZpZGVyLWFjY291bnQifQ.signature".into()),
        expires_at: Some(expires),
        last_refresh: Some(started),
        scopes: Vec::new(),
        account_id: None,
        metadata: serde_json::Value::Null,
    };
    let mut environment = ResolverEnvironment::testing()
        .with_provider_auth_persistence(persistence.clone())
        .with_auth_lease_handle(owner.clone());
    let read_clock = Arc::clone(&clock);
    environment.now = Arc::new(move || {
        DateTime::<Utc>::from_timestamp(read_clock.load(Ordering::SeqCst), 0).unwrap()
    });
    if external {
        environment =
            environment.with_external_resolver("retained-external", Arc::new(ExternalFixture));
        let absent = owner.snapshot(&lease);
        assert_eq!(absent.phase, None);
        assert!(store.load(&key).await.unwrap().is_none());
        let connection = GoogleProviderRuntime
            .resolve_binding(&binding, &environment)
            .await
            .unwrap();
        let client = GoogleProviderRuntime.build_client(connection).unwrap();
        let pin = Arc::clone(&client);
        consume(&client).await.unwrap();
        let before_cached_reuse = counting_store.counts();
        assert!(
            before_cached_reuse.loads > 0,
            "decorator observed actual absent-store fixture read: {before_cached_reuse:?}"
        );
        consume(&client).await.unwrap();
        assert_eq!(
            counting_store.counts(),
            before_cached_reuse,
            "external retained second use must make zero TokenStore operations"
        );
        assert!(Arc::ptr_eq(&client, &pin));
        assert_eq!(owner.snapshot(&lease), absent);
        assert!(store.load(&key).await.unwrap().is_none());
        return;
    }
    let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease).await;
    let transition = owner
        .acquire_lease(&lease, expires.timestamp() as u64)
        .unwrap();
    let original =
        meerkat_core::mark_tokens_lifecycle_published_for_transition(&key, &original, &transition)
            .unwrap();
    store.save(&key, &original).await.unwrap();
    drop(guard);
    let connection = GoogleProviderRuntime
        .resolve_binding(&binding, &environment)
        .await
        .unwrap();
    assert_eq!(connection.credential_identity, account);
    let selected_lease = Arc::clone(&connection.auth_lease);
    let selected_metadata = selected_lease.metadata().clone();
    assert_eq!(
        selected_metadata.account_id.as_deref(),
        Some("managed-provider-account")
    );
    assert!(matches!(
        &connection.auth_lease.metadata().provider_metadata,
        Some(meerkat_core::ProviderAuthMetadata::Google(metadata))
            if metadata.project_id.as_deref() == Some("managed-project")
                && metadata.code_assist_tier.as_deref() == Some("standard-tier")
    ));
    let client = GoogleProviderRuntime.build_client(connection).unwrap();
    let original_pin = Arc::clone(&client);
    consume(&client).await.unwrap();
    let first_snapshot = owner.snapshot(&lease);
    assert_eq!(first_snapshot.phase, Some(AuthLeasePhase::Valid));
    assert_eq!(store.load(&key).await.unwrap(), Some(original.clone()));
    if mode == "expired" {
        clock.store(expires.timestamp() + 1, Ordering::SeqCst);
        owner
            .observe_credential_freshness(&lease, (expires.timestamp() + 1) as u64, 60)
            .unwrap();
        assert_eq!(owner.snapshot(&lease).phase, Some(AuthLeasePhase::Expired));
        assert_eq!(
            owner
                .resolve_credential_use_admission(&lease, CredentialUseIntent::HoldAuthority)
                .unwrap(),
            CredentialUseDisposition::RefreshRequired
        );
    } else if mode == "released" {
        meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated_for_identity(
            persistence,
            owner.clone(),
            account.clone(),
        )
        .await
        .unwrap();
        assert!(store.load(&key).await.unwrap().is_none());
        assert_eq!(
            owner
                .resolve_credential_use_admission(&lease, CredentialUseIntent::HoldAuthority)
                .unwrap(),
            CredentialUseDisposition::LeaseAbsent
        );
    }
    assert!(
        Arc::ptr_eq(&client, &original_pin),
        "reuse the identical pinned client"
    );
    // Exclude fixture setup/readback and cold first use from the measured interval.
    let before_cached_reuse = counting_store.counts();
    let second = consume(&client).await;
    if mode == "current" || mode == "external-current" {
        assert!(
            before_cached_reuse.loads > 0 && before_cached_reuse.saves > 0,
            "decorator observed actual seeded store setup: {before_cached_reuse:?}"
        );
        assert_eq!(
            counting_store.counts(),
            before_cached_reuse,
            "case={mode}: retained healthy second use must make zero TokenStore operations"
        );
    }
    if mode == "released" {
        assert!(
            second.is_err(),
            "released managed owner must refuse before model HTTP"
        );
        assert!(
            store.load(&key).await.unwrap().is_none(),
            "no resurrected durable token"
        );
        assert_eq!(
            owner
                .resolve_credential_use_admission(&lease, CredentialUseIntent::HoldAuthority)
                .unwrap(),
            CredentialUseDisposition::LeaseAbsent
        );
    } else {
        second.unwrap();
        let final_tokens = store.load(&key).await.unwrap().unwrap();
        if mode == "expired" {
            assert_eq!(
                final_tokens.primary_secret.as_deref(),
                Some(NEW),
                "same client's actual use must refresh through its retained managed owner"
            );
            assert_eq!(
                final_tokens.refresh_token.as_deref(),
                Some("managed-refresh-new")
            );
            assert_eq!(final_tokens.account_id, original.account_id);
            assert!(
                final_tokens.id_token.is_none(),
                "refresh omits optional claims"
            );
            assert_eq!(selected_lease.metadata(), &selected_metadata);
            assert!(owner.snapshot(&lease).generation > first_snapshot.generation);
            assert_eq!(owner.snapshot(&lease).phase, Some(AuthLeasePhase::Valid));
        } else {
            assert_eq!(final_tokens, original);
            assert_eq!(owner.snapshot(&lease), first_snapshot);
        }
    }
}
