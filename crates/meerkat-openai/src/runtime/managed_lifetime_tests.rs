//! A retained managed client must observe its actual credential owner on reuse.
//! Parent-scoped HTTP plus a child process keeps existing OAuth fixture env local.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::OpenAiProviderRuntime;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use base64::Engine;
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
const MODE_ENV: &str = "MEERKAT_N1_MANAGED_LIFETIME_CASE";
const BASE_ENV: &str = "MEERKAT_N1_MANAGED_LIFETIME_BASE";
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
    account_headers: Vec<String>,
    model_bodies: Vec<serde_json::Value>,
    refresh_forms: Vec<String>,
    refresh_id_token: Option<String>,
    fedramp_headers: Vec<Option<String>>,
}

async fn token(State(wire): State<Arc<Mutex<Wire>>>, body: String) -> Response {
    let id_token = {
        let mut record = wire.lock().unwrap();
        record.refresh_forms.push(body);
        record.refresh_id_token.clone()
    };
    let mut response = serde_json::json!({"access_token":NEW,"refresh_token":"managed-refresh-new","token_type":"Bearer","expires_in":10800});
    if let Some(id_token) = id_token {
        response["id_token"] = serde_json::Value::String(id_token);
    }
    Json(response).into_response()
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
    record.account_headers.push(
        headers
            .get("chatgpt-account-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string(),
    );
    record.fedramp_headers.push(
        headers
            .get(meerkat_core::provider_matrix::openai_auth::FEDRAMP_HEADER)
            .map(|value| value.to_str().unwrap().to_string()),
    );
    record.model_bodies.push(body);
    drop(record);
    let event = serde_json::json!({"type":"response.completed","response":{"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"managed lifetime complete"}]}],"usage":{"input_tokens":1,"output_tokens":1}}});
    (
        StatusCode::OK,
        [("content-type", "text/event-stream")],
        format!("data: {event}\n\ndata: [DONE]\n\n"),
    )
        .into_response()
}

async fn run_parent(mode: &str) {
    let wire = Arc::new(Mutex::new(Wire {
        refresh_id_token: changed_route_claims(mode)
            .map(|(account, fedramp)| route_id_token(account, fedramp)),
        ..Wire::default()
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/openai/token", axum::routing::post(token))
        .route("/responses", axum::routing::post(model))
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
    let expected_headers = if mode == "forced-current" {
        vec![format!("Bearer {NEW}"), format!("Bearer {NEW}")]
    } else if mode == "expired" {
        vec![format!("Bearer {OLD}"), format!("Bearer {NEW}")]
    } else if matches!(mode, "released" | "prepare-expired" | "prepare-release")
        || changed_route_claims(mode).is_some()
    {
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
            .account_headers
            .iter()
            .all(|id| id == "managed-provider-account")
    );
    assert!(
        record
            .model_bodies
            .iter()
            .all(|body| body["model"] == "gpt-5.5")
    );
    assert_eq!(
        record.refresh_forms.len(),
        usize::from(
            matches!(mode, "expired" | "prepare-expired" | "forced-current")
                || changed_route_claims(mode).is_some()
        )
    );
    if changed_route_claims(mode).is_some() {
        assert_eq!(
            record.model_bodies.len(),
            1,
            "no further model HTTP after the first successful pinned request"
        );
        assert_eq!(
            record.fedramp_headers,
            vec![None],
            "the original route is explicitly non-FedRamp"
        );
    }
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

#[tokio::test]
async fn managed_prepare_refreshes_without_a_model_request() {
    run_parent("prepare-expired").await;
}
#[tokio::test]
async fn managed_prepare_does_not_admit_a_later_released_owner() {
    run_parent("prepare-release").await;
}
#[tokio::test]
async fn managed_force_is_consumed_at_initial_resolution_only() {
    run_parent("forced-current").await;
}

// These are provider response fixtures, decoded through the actual JWT/claim
// owners. They neither install a lifecycle row nor replace the pinned client.
fn changed_route_claims(mode: &str) -> Option<(&'static str, bool)> {
    match mode {
        "account-changed" => Some(("managed-provider-account-other", false)),
        "fedramp-changed" => Some(("managed-provider-account", true)),
        _ => None,
    }
}

fn route_id_token(account: &str, fedramp: bool) -> String {
    let encode = |value: &serde_json::Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap())
    };
    let header = encode(&serde_json::json!({"alg":"none"}));
    let payload = encode(&serde_json::json!({
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account,
            "chatgpt_account_is_fedramp": fedramp,
        },
    }));
    format!("{header}.{payload}.fixture-signature")
}

#[tokio::test]
async fn managed_pinned_refresh_rejects_changed_account_route() {
    run_parent("account-changed").await;
}

#[tokio::test]
async fn managed_pinned_refresh_rejects_changed_fedramp_route() {
    run_parent("fedramp-changed").await;
}

async fn require_typed_route_change(client: &Arc<dyn LlmClient>) {
    let request = LlmRequest::new(
        "gpt-5.5",
        vec![Message::User(UserMessage::text(
            "Complete this small request.",
        ))],
    );
    let mut stream = client.stream(&request);
    let mut errors = Vec::new();
    while let Some(event) = stream.next().await {
        match event {
            Err(error)
            | Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Error { error },
            }) => errors.push(error),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { .. },
            }) => {
                panic!("a changed account route cannot complete another pinned model request");
            }
            _ => {}
        }
    }
    assert!(
        matches!(
            errors.as_slice(),
            [meerkat_llm_core::LlmError::AuthorizationRouteChanged { .. }]
        ),
        "one typed ResolveRequired projection, got {errors:?}"
    );
}

async fn consume(client: &Arc<dyn LlmClient>) -> Result<(), String> {
    let request = LlmRequest::new(
        "gpt-5.5",
        vec![Message::User(UserMessage::text(
            "Complete this small request.",
        ))],
    );
    let mut stream = client.stream(&request);
    let mut completions = 0;
    while let Some(event) = stream.next().await {
        match event.map_err(|error| error.to_string())? {
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
            provider: Provider::OpenAI,
            backend_kind: "chatgpt_backend".into(),
            base_url: Some(base),
            options: serde_json::Value::Null,
            server: None,
        },
        &AuthProfile {
            id: "managed-auth".into(),
            provider: Provider::OpenAI,
            auth_method: if external {
                "external_chatgpt_tokens"
            } else {
                "managed_chatgpt_oauth"
            }
            .into(),
            source: CredentialSourceSpec::ManagedStore,
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
        auth_mode: if external {
            PersistedAuthMode::ExternalTokens
        } else {
            PersistedAuthMode::ChatgptOauth
        },
        primary_secret: Some(OLD.into()),
        refresh_token: if external {
            None
        } else {
            Some("managed-refresh-old".into())
        },
        id_token: changed_route_claims(&mode)
            .map(|_| route_id_token("managed-provider-account", false)),
        expires_at: Some(expires),
        last_refresh: Some(started),
        scopes: Vec::new(),
        account_id: Some("managed-provider-account".into()),
        metadata: serde_json::Value::Null,
    };
    let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease).await;
    let transition = owner
        .acquire_lease(&lease, expires.timestamp() as u64)
        .unwrap();
    let original =
        meerkat_core::mark_tokens_lifecycle_published_for_transition(&key, &original, &transition)
            .unwrap();
    store.save(&key, &original).await.unwrap();
    drop(guard);
    let mut environment = ResolverEnvironment::testing()
        .with_provider_auth_persistence(persistence.clone())
        .with_auth_lease_handle(owner.clone());
    let read_clock = Arc::clone(&clock);
    environment.now = Arc::new(move || {
        DateTime::<Utc>::from_timestamp(read_clock.load(Ordering::SeqCst), 0).unwrap()
    });
    environment.force_refresh = mode == "forced-current";
    let connection = OpenAiProviderRuntime
        .resolve_binding(&binding, &environment)
        .await
        .unwrap();
    assert_eq!(connection.credential_identity, account);
    let authorizer = connection.resolved_authorizer();
    let client = OpenAiProviderRuntime.build_client(connection).unwrap();
    let original_pin = Arc::clone(&client);
    consume(&client).await.unwrap();
    let first_snapshot = owner.snapshot(&lease);
    assert_eq!(first_snapshot.phase, Some(AuthLeasePhase::Valid));
    let after_first_tokens = store.load(&key).await.unwrap().unwrap();
    if mode == "forced-current" {
        assert_eq!(after_first_tokens.primary_secret.as_deref(), Some(NEW));
    } else {
        assert_eq!(after_first_tokens, original);
    }
    if matches!(mode.as_str(), "expired" | "prepare-expired")
        || changed_route_claims(&mode).is_some()
    {
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
    }
    if let Some((expected_account, expected_fedramp)) = changed_route_claims(&mode) {
        assert!(
            Arc::ptr_eq(&client, &original_pin),
            "keep the exact original pin"
        );
        require_typed_route_change(&client).await;
        let refreshed = store.load(&key).await.unwrap().unwrap();
        assert_eq!(refreshed.primary_secret.as_deref(), Some(NEW));
        assert_eq!(
            refreshed.refresh_token.as_deref(),
            Some("managed-refresh-new")
        );
        // The real refresh path preserves the old stored account while its
        // new ID token can explicitly conflict. Exercise that precise edge.
        assert_eq!(refreshed.account_id, original.account_id);
        let decoded = meerkat_auth_core::auth_oauth::jwt::decode_payload(
            refreshed
                .id_token
                .as_deref()
                .expect("actual response ID token"),
        )
        .unwrap();
        let claims = super::oauth::ChatGptIdClaims::lift_from_claims(&decoded.raw);
        assert_eq!(claims.account_id.as_deref(), Some(expected_account));
        assert_eq!(claims.is_fedramp, Some(expected_fedramp));
        let refreshed_owner = owner.snapshot(&lease);
        assert_eq!(refreshed_owner.phase, Some(AuthLeasePhase::Valid));
        assert!(refreshed_owner.generation > first_snapshot.generation);
        assert_eq!(
            meerkat_core::generated::auth_lease_durable_lifecycle_marker::marker_relation_for_tokens_and_snapshot(
                &refreshed, &refreshed_owner, &key,
            ),
            meerkat_core::generated::auth_lease_durable_lifecycle_marker::AuthLeaseDurableMarkerRelation::Matches,
        );
        let error = authorizer
            .expect("managed client retains its actual authorizer")
            .prepare_request()
            .await
            .expect_err("the same pin still requires resolution");
        assert!(
            matches!(error, meerkat_core::AuthError::ResolveRequired(_)),
            "retain the canonical auth kind: {error:?}"
        );
        assert_eq!(owner.snapshot(&lease), refreshed_owner);
        assert_eq!(store.load(&key).await.unwrap().unwrap(), refreshed);
        return;
    }
    if mode == "prepare-expired" {
        authorizer
            .expect("managed resolution retains its actual HttpAuthorizer")
            .prepare_request()
            .await
            .unwrap();
        let refreshed = store.load(&key).await.unwrap().unwrap();
        assert_eq!(refreshed.primary_secret.as_deref(), Some(NEW));
        assert_eq!(refreshed.account_id, original.account_id);
        assert_eq!(owner.snapshot(&lease).phase, Some(AuthLeasePhase::Valid));
        assert!(owner.snapshot(&lease).generation > first_snapshot.generation);
        return;
    }
    if mode == "prepare-release" {
        let before_prepare = counting_store.counts();
        authorizer
            .expect("managed resolution retains its actual HttpAuthorizer")
            .prepare_request()
            .await
            .unwrap();
        assert_eq!(
            counting_store.counts(),
            before_prepare,
            "current prepare must perform no TokenStore operation"
        );
    }
    if matches!(mode.as_str(), "released" | "prepare-release") {
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
    if matches!(
        mode.as_str(),
        "current" | "external-current" | "forced-current"
    ) {
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
    if matches!(mode.as_str(), "released" | "prepare-release") {
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
            assert!(owner.snapshot(&lease).generation > first_snapshot.generation);
            assert_eq!(owner.snapshot(&lease).phase, Some(AuthLeasePhase::Valid));
        } else {
            assert_eq!(final_tokens, after_first_tokens);
            assert_eq!(owner.snapshot(&lease), first_snapshot);
        }
    }
}
