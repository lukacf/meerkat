#![cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Provider protocol components through the generic connector owner: Slack
//! user tokens and Salesforce, against a local fixture that answers in each
//! provider's shape. Covers login, refresh rules, pinned evidence, the
//! identity proofs, and the exact-credential rejection seam.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use meerkat_auth_core::auth_store::{
    CredentialMutationError, CredentialMutationFn, CredentialMutationOutcome, PersistedTokens,
    ProviderAuthPersistence, RefreshCoordinator, RefreshError, RefreshFn, TokenKey, TokenStore,
};
use meerkat_auth_core::connector_login::{
    ConnectorClientAuthProfile, ConnectorLoginError, ConnectorOAuthAuthority,
    ConnectorOAuthCallback, ConnectorOAuthTarget, ConnectorRejectionOutcome, ConnectorStrategies,
};
use meerkat_auth_core::connector_oauth::{AccountSelection, ConnectorCredentialMetadata};
use meerkat_auth_core::connector_protocol::{
    ConnectorProtocols, SALESFORCE_IDENTITY_STRATEGY_ID, SALESFORCE_PROTOCOL_ID,
    SLACK_AUTH_TEST_STRATEGY_ID, SLACK_USER_PROTOCOL_ID, SalesforceIdentityStrategy,
    SalesforceProtocol, SlackAuthTestStrategy, SlackUserProtocol,
};
use meerkat_auth_core::{EphemeralTokenStore, InMemoryCoordinator};
use meerkat_core::AuthCredentialIdentity;
use meerkat_core::connection::{CredentialAccountId, CredentialAccountRef, RealmId};
use meerkat_runtime::handles::RuntimeOAuthFlowHandle;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::net::TcpListener;

const SF_ORG: &str = "00D000000000001AAA";
const SF_USER: &str = "005000000000001AAA";

#[derive(Default)]
struct Provider {
    base: Mutex<String>,
    /// Next code-exchange and refresh bodies, per provider.
    slack_exchange: Mutex<Option<Value>>,
    slack_refresh: Mutex<Option<Value>>,
    sf_exchange: Mutex<Option<Value>>,
    sf_refresh: Mutex<Option<Value>>,
    /// Access token -> (team, user) for `auth.test`.
    slack_tokens: Mutex<HashMap<String, (String, String)>>,
    /// Access tokens the Salesforce identity service accepts, and whether it
    /// reports them as the asserted user.
    sf_tokens: Mutex<HashMap<String, bool>>,
    refreshes: AtomicUsize,
    /// Hold each refresh answer this long.
    refresh_delay_ms: std::sync::atomic::AtomicU64,
    forms: Mutex<Vec<HashMap<String, String>>>,
}

async fn slack_token(
    State(state): State<Arc<Provider>>,
    Form(form): Form<HashMap<String, String>>,
) -> Json<Value> {
    state.forms.lock().push(form.clone());
    let body = if form.get("grant_type").map(String::as_str) == Some("refresh_token") {
        state.refreshes.fetch_add(1, Ordering::SeqCst);
        let delay = state.refresh_delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
        state.slack_refresh.lock().take()
    } else {
        state.slack_exchange.lock().take()
    };
    // Slack answers errors with HTTP 200 and `ok: false`.
    Json(body.unwrap_or_else(|| json!({ "ok": false, "error": "invalid_refresh_token" })))
}

async fn slack_auth_test(State(state): State<Arc<Provider>>, headers: HeaderMap) -> Json<Value> {
    let token = bearer(&headers);
    match state.slack_tokens.lock().get(&token) {
        Some((team, user)) => Json(json!({ "ok": true, "team_id": team, "user_id": user })),
        None => Json(json!({ "ok": false, "error": "invalid_auth" })),
    }
}

async fn sf_token(
    State(state): State<Arc<Provider>>,
    Form(form): Form<HashMap<String, String>>,
) -> (axum::http::StatusCode, Json<Value>) {
    state.forms.lock().push(form.clone());
    let body = if form.get("grant_type").map(String::as_str) == Some("refresh_token") {
        state.refreshes.fetch_add(1, Ordering::SeqCst);
        state.sf_refresh.lock().take()
    } else {
        state.sf_exchange.lock().take()
    };
    match body {
        Some(body) => (axum::http::StatusCode::OK, Json(body)),
        None => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_grant" })),
        ),
    }
}

async fn sf_identity(
    State(state): State<Arc<Provider>>,
    Path((org, user)): Path<(String, String)>,
    headers: HeaderMap,
) -> (axum::http::StatusCode, Json<Value>) {
    let token = bearer(&headers);
    match state.sf_tokens.lock().get(&token) {
        Some(asserted) => (
            axum::http::StatusCode::OK,
            Json(json!({
                "organization_id": org,
                "user_id": user,
                "asserted_user": asserted,
                "active": true,
            })),
        ),
        None => (axum::http::StatusCode::FORBIDDEN, Json(json!({}))),
    }
}

fn bearer(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_owned()
}

/// Runs before the next coordinated refresh: a logout and new login landing
/// between a rejection's mark and the refresh reacquiring the slot.
type BeforeRefresh =
    Box<dyn FnOnce() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send>;

#[derive(Default)]
struct HookCoordinator {
    inner: InMemoryCoordinator,
    before_refresh: Mutex<Option<BeforeRefresh>>,
}

#[async_trait::async_trait]
impl RefreshCoordinator for HookCoordinator {
    async fn with_exclusive_mutation(
        &self,
        key: TokenKey,
        mutation_fn: CredentialMutationFn,
    ) -> Result<CredentialMutationOutcome, CredentialMutationError> {
        self.inner.with_exclusive_mutation(key, mutation_fn).await
    }

    async fn with_refresh(
        &self,
        key: TokenKey,
        refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError> {
        let hook = self.before_refresh.lock().take();
        if let Some(hook) = hook {
            hook().await;
        }
        self.inner.with_refresh(key, refresh_fn).await
    }
}

struct Fixture {
    base: String,
    state: Arc<Provider>,
    authority: ConnectorOAuthAuthority,
    store: Arc<dyn TokenStore>,
    persistence: ProviderAuthPersistence,
    flows: Arc<RuntimeOAuthFlowHandle>,
    coordinator: Arc<HookCoordinator>,
}

/// The installed profiles: `(profile id, protocol id)`.
const PROFILES: [(&str, Option<&str>); 3] = [
    ("slack", Some(SLACK_USER_PROTOCOL_ID)),
    ("salesforce", Some(SALESFORCE_PROTOCOL_ID)),
    ("salesforce-rotating", Some("salesforce-rotating")),
];

impl Fixture {
    /// An owner over the same store, lifecycle and flows, with `profiles`
    /// installed (as after a host reconfiguration).
    fn authority_with(&self, profiles: &[(&str, Option<&str>)]) -> ConnectorOAuthAuthority {
        self.authority_configured(
            profiles,
            false,
            SalesforceProtocol::new().allowing_loopback(),
        )
    }

    /// [`Self::authority_with`] with the profiles' resource-indicator choice
    /// and the `salesforce-v1` protocol's configuration.
    fn authority_configured(
        &self,
        profiles: &[(&str, Option<&str>)],
        resource_indicator: bool,
        salesforce: SalesforceProtocol,
    ) -> ConnectorOAuthAuthority {
        let base = &self.base;
        ConnectorOAuthAuthority::with_http(
            self.persistence.clone(),
            self.flows.clone(),
            ConnectorStrategies::default()
                .with(Arc::new(
                    SlackAuthTestStrategy::new().with_base(base.clone()),
                ))
                .with(Arc::new(
                    SalesforceIdentityStrategy::new().allowing_loopback(),
                )),
            reqwest::Client::new(),
        )
        .unwrap()
        .with_client_auth_profiles(profiles.iter().map(|(id, protocol)| {
            ConnectorClientAuthProfile {
                id: (*id).to_owned(),
                issuer: base.clone(),
                client: "client-1".to_owned(),
                client_secret: None,
                authorize_params: Vec::new(),
                resource_indicator,
                protocol: protocol.map(str::to_owned),
                registered_redirect_uris: Vec::new(),
            }
        }))
        .unwrap()
        .with_connector_protocols(
            ConnectorProtocols::default()
                .with(Arc::new(SlackUserProtocol::new().with_base(base.clone())))
                .with(Arc::new(salesforce))
                .with(Arc::new(
                    SalesforceProtocol::new()
                        .allowing_loopback()
                        .with_protocol_id("salesforce-rotating")
                        .with_refresh_token_rotation(),
                )),
        )
    }
}

async fn fixture() -> Fixture {
    let state = Arc::new(Provider::default());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    *state.base.lock() = base.clone();
    let app = Router::new()
        .route("/api/oauth.v2.access", post(slack_token))
        .route("/api/auth.test", post(slack_auth_test))
        .route("/services/oauth2/token", post(sf_token))
        .route("/id/{org}/{user}", get(sf_identity))
        .with_state(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
    let coordinator = Arc::new(HookCoordinator::default());
    let persistence = ProviderAuthPersistence::new(store.clone(), coordinator.clone());
    let lifecycle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    let flows = Arc::new(RuntimeOAuthFlowHandle::new_with_auth_lease(
        std::time::Duration::from_mins(5),
        lifecycle,
    ));
    let mut fixture = Fixture {
        base,
        state,
        authority: ConnectorOAuthAuthority::with_http(
            persistence.clone(),
            flows.clone(),
            ConnectorStrategies::default(),
            reqwest::Client::new(),
        )
        .unwrap(),
        store,
        persistence,
        flows,
        coordinator,
    };
    fixture.authority = fixture.authority_with(&PROFILES);
    fixture
}

fn slot(name: &str) -> CredentialAccountRef {
    CredentialAccountRef {
        realm: RealmId::parse("connect").unwrap(),
        account: CredentialAccountId::parse(name).unwrap(),
    }
}

const REDIRECT: &str = "http://127.0.0.1:41011/connector/oauth/callback";

impl Fixture {
    fn target(&self, slot: &CredentialAccountRef, provider: &str) -> ConnectorOAuthTarget {
        let (strategy, scopes): (&str, &[&str]) = match provider {
            "slack" => (
                SLACK_AUTH_TEST_STRATEGY_ID,
                &["search:read", "channels:history"],
            ),
            _ => (SALESFORCE_IDENTITY_STRATEGY_ID, &["api", "refresh_token"]),
        };
        ConnectorOAuthTarget {
            slot: slot.clone(),
            issuer: self.base.clone(),
            client: "client-1".into(),
            resource: self.base.clone(),
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            strategy_id: strategy.into(),
            account: AccountSelection::Discover,
            client_auth_profile: Some(provider.into()),
        }
    }

    async fn login(
        &self,
        slot: &CredentialAccountRef,
        provider: &str,
    ) -> Result<meerkat_auth_core::connector_login::ConnectorLoginComplete, ConnectorLoginError>
    {
        let start = self
            .authority
            .login_start(&self.target(slot, provider), REDIRECT)
            .await?;
        self.authority
            .login_complete(
                slot,
                ConnectorOAuthCallback {
                    redirect_uri: REDIRECT.to_owned(),
                    state: start.state,
                    code: "code-1".to_owned(),
                    host_attempt_handle: None,
                },
            )
            .await
    }

    async fn stored_metadata(&self, slot: &CredentialAccountRef) -> ConnectorCredentialMetadata {
        let tokens = self
            .store
            .load(&TokenKey::from_credential_identity(
                &AuthCredentialIdentity::Account(slot.clone()),
            ))
            .await
            .unwrap()
            .expect("stored credential");
        ConnectorCredentialMetadata::from_tokens(&tokens).expect("connector metadata")
    }

    fn slack_tokens(&self, access: &str) {
        self.state.slack_tokens.lock().insert(
            access.to_owned(),
            ("T0TEAM".to_owned(), "U0USER".to_owned()),
        );
    }

    fn slack_exchange(&self, access: &str, expires_in: u64) {
        self.slack_tokens(access);
        *self.state.slack_exchange.lock() = Some(json!({
            "ok": true,
            "app_id": "A0APP",
            "authed_user": {
                "id": "U0USER",
                "scope": "search:read,channels:history",
                "access_token": access,
                "token_type": "user",
                "refresh_token": format!("refresh-{access}"),
                "expires_in": expires_in,
            },
            "team": { "id": "T0TEAM" },
        }));
    }

    fn slack_refresh(&self, access: &str, refresh: Option<&str>, app: &str) {
        self.slack_tokens(access);
        let mut body = json!({
            "ok": true,
            "app_id": app,
            "scope": "search:read,channels:history",
            "token_type": "user",
            "access_token": access,
            "expires_in": 43200,
            "user_id": "U0USER",
            "team": { "id": "T0TEAM" },
        });
        if let Some(refresh) = refresh {
            body["refresh_token"] = json!(refresh);
        }
        *self.state.slack_refresh.lock() = Some(body);
    }

    fn sf_body(&self, access: &str, refresh: Option<&str>) -> Value {
        let mut body = json!({
            "access_token": access,
            "instance_url": self.base,
            "id": format!("{}/id/{SF_ORG}/{SF_USER}", self.base),
            "token_type": "Bearer",
            "issued_at": "1700000000000",
            "scope": "api refresh_token",
        });
        if let Some(refresh) = refresh {
            body["refresh_token"] = json!(refresh);
        }
        body
    }
}

/// A Slack login sends the scopes as `user_scope`, reads the nested user
/// token, proves the account with `auth.test`, and keeps the app and team as
/// pinned evidence.
#[tokio::test]
async fn slack_login_reads_the_nested_user_token_and_proves_the_account() {
    let fx = fixture().await;
    let slot = slot("slack");
    let start = fx
        .authority
        .login_start(&fx.target(&slot, "slack"), REDIRECT)
        .await
        .unwrap();
    assert!(
        start
            .authorize_url
            .starts_with(&format!("{}/oauth/v2/authorize?", fx.base))
    );
    assert!(
        start
            .authorize_url
            .contains("user_scope=channels%3Ahistory%2Csearch%3Aread")
    );
    // No bot scopes: an empty `scope`, as Slack's user-token flow sends.
    assert!(
        start.authorize_url.contains("&scope=&"),
        "{}",
        start.authorize_url
    );
    fx.authority.login_cancel(&slot, &start.state).unwrap();

    fx.slack_exchange("xoxp-1", 43200);
    let done = fx.login(&slot, "slack").await.unwrap();
    assert_eq!(done.verified_account.subject, "T0TEAM:U0USER");
    assert!(done.has_refresh_token);
    let metadata = fx.stored_metadata(&slot).await;
    let evidence = metadata.protocol_evidence.expect("protocol evidence");
    assert_eq!(
        evidence.pinned.get("app_id").map(String::as_str),
        Some("A0APP")
    );
    assert_eq!(
        evidence.pinned.get("team_id").map(String::as_str),
        Some("T0TEAM")
    );
    assert_eq!(
        fx.authority.bearer_token(&slot).await.unwrap().as_deref(),
        Some("xoxp-1")
    );
}

/// A Slack refresh reads the top-level user token and replaces the refresh
/// token; one without a replacement closes the credential instead of
/// reusing the consumed predecessor; another app is refused.
#[tokio::test]
async fn slack_refresh_replaces_tokens_and_refuses_missing_replacement_or_other_app() {
    let fx = fixture().await;
    let rotated = slot("slack-refresh");
    // Expiring at once, so the next use refreshes.
    fx.slack_exchange("xoxp-1", 1);
    fx.login(&rotated, "slack").await.unwrap();

    fx.slack_refresh("xoxp-2", Some("refresh-2"), "A0APP");
    assert_eq!(
        fx.authority
            .bearer_token(&rotated)
            .await
            .unwrap()
            .as_deref(),
        Some("xoxp-2")
    );
    let forms = fx.state.forms.lock().clone();
    let refresh_form = forms.last().unwrap();
    assert_eq!(refresh_form["grant_type"], "refresh_token");
    assert_eq!(refresh_form["refresh_token"], "refresh-xoxp-1");

    // Another app answering for the credential is a pinned-fact change.
    let other = slot("slack-other-app");
    fx.slack_exchange("xoxp-3", 1);
    fx.login(&other, "slack").await.unwrap();
    fx.slack_refresh("xoxp-4", Some("refresh-4"), "A0OTHER");
    assert!(fx.authority.bearer_token(&other).await.is_err());

    // No replacement refresh token: the credential is closed.
    let missing = slot("slack-no-replacement");
    fx.slack_exchange("xoxp-5", 1);
    fx.login(&missing, "slack").await.unwrap();
    fx.slack_refresh("xoxp-6", None, "A0APP");
    assert!(fx.authority.bearer_token(&missing).await.is_err());
    assert!(
        fx.store
            .load(&TokenKey::from_credential_identity(
                &AuthCredentialIdentity::Account(missing.clone()),
            ))
            .await
            .unwrap()
            .is_none(),
        "the possibly consumed predecessor is never reused"
    );
}

/// The rejection seam: a protocol-classified rejection of the exact current
/// credential refreshes it; an unclassified response refreshes nothing; a
/// rejection of a credential the slot no longer holds changes nothing.
#[tokio::test]
async fn a_classified_rejection_of_the_exact_credential_refreshes_it() {
    let fx = fixture().await;
    let slot = slot("slack-rejected");
    fx.slack_exchange("xoxp-1", 43200);
    fx.login(&slot, "slack").await.unwrap();

    let unclassified = fx
        .authority
        .report_credential_rejection(
            &slot,
            "xoxp-1",
            200,
            br#"{"ok":false,"error":"channel_not_found"}"#,
        )
        .await
        .unwrap();
    assert_eq!(
        unclassified,
        ConnectorRejectionOutcome::NotCredentialRejection
    );
    assert_eq!(fx.state.refreshes.load(Ordering::SeqCst), 0);

    let stale = fx
        .authority
        .report_credential_rejection(
            &slot,
            "xoxp-old",
            200,
            br#"{"ok":false,"error":"invalid_auth"}"#,
        )
        .await
        .unwrap();
    assert_eq!(stale, ConnectorRejectionOutcome::Superseded);
    assert_eq!(fx.state.refreshes.load(Ordering::SeqCst), 0);

    fx.slack_refresh("xoxp-2", Some("refresh-2"), "A0APP");
    let refreshed = fx
        .authority
        .report_credential_rejection(
            &slot,
            "xoxp-1",
            200,
            br#"{"ok":false,"error":"invalid_auth"}"#,
        )
        .await
        .unwrap();
    assert_eq!(
        refreshed,
        ConnectorRejectionOutcome::Refreshed("xoxp-2".to_owned())
    );
    assert_eq!(fx.state.refreshes.load(Ordering::SeqCst), 1);

    // The old token's late rejection does not touch the newer credential.
    let late = fx
        .authority
        .report_credential_rejection(
            &slot,
            "xoxp-1",
            200,
            br#"{"ok":false,"error":"invalid_auth"}"#,
        )
        .await
        .unwrap();
    assert_eq!(late, ConnectorRejectionOutcome::Superseded);
    assert_eq!(fx.state.refreshes.load(Ordering::SeqCst), 1);
}

/// A Salesforce login keeps the provider's unknown expiry, proves the
/// asserted user through the identity service, and pins the org; a
/// non-rotating refresh that omits the refresh token keeps the stored one.
#[tokio::test]
async fn salesforce_login_and_non_rotating_refresh() {
    let fx = fixture().await;
    let slot = slot("salesforce");
    let start = fx
        .authority
        .login_start(&fx.target(&slot, "salesforce"), REDIRECT)
        .await
        .unwrap();
    assert!(
        start
            .authorize_url
            .starts_with(&format!("{}/services/oauth2/authorize?", fx.base))
    );
    assert!(start.authorize_url.contains("scope=api%20refresh_token"));
    fx.authority.login_cancel(&slot, &start.state).unwrap();

    fx.state.sf_tokens.lock().insert("sf-1".to_owned(), true);
    *fx.state.sf_exchange.lock() = Some(fx.sf_body("sf-1", Some("sf-refresh")));
    let done = fx.login(&slot, "salesforce").await.unwrap();
    assert_eq!(done.verified_account.subject, format!("{SF_ORG}:{SF_USER}"));
    assert_eq!(done.expires_at, None, "no synthetic expiry");
    let evidence = fx
        .stored_metadata(&slot)
        .await
        .protocol_evidence
        .expect("protocol evidence");
    assert_eq!(
        evidence.pinned.get("org_id").map(String::as_str),
        Some(SF_ORG)
    );
    assert_eq!(
        evidence.facts.get("instance_url").map(String::as_str),
        Some(fx.base.as_str())
    );

    // An invalid session reported by the API refreshes the exact credential;
    // the non-rotating refresh omits the refresh token, which is kept.
    fx.state.sf_tokens.lock().insert("sf-2".to_owned(), true);
    *fx.state.sf_refresh.lock() = Some(fx.sf_body("sf-2", None));
    let outcome = fx
        .authority
        .report_credential_rejection(
            &slot,
            "sf-1",
            401,
            br#"[{"message":"Session expired or invalid","errorCode":"INVALID_SESSION_ID"}]"#,
        )
        .await
        .unwrap();
    assert_eq!(
        outcome,
        ConnectorRejectionOutcome::Refreshed("sf-2".to_owned())
    );
    let forms = fx.state.forms.lock().clone();
    assert_eq!(forms.last().unwrap()["refresh_token"], "sf-refresh");
    let stored = fx
        .store
        .load(&TokenKey::from_credential_identity(
            &AuthCredentialIdentity::Account(slot.clone()),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.refresh_token.as_deref(), Some("sf-refresh"));
}

/// An identity answer that is not the token's own asserted user refuses the
/// login.
#[tokio::test]
async fn salesforce_login_requires_the_asserted_user() {
    let fx = fixture().await;
    let slot = slot("salesforce-queried");
    fx.state.sf_tokens.lock().insert("sf-1".to_owned(), false);
    *fx.state.sf_exchange.lock() = Some(fx.sf_body("sf-1", Some("sf-refresh")));
    assert!(fx.login(&slot, "salesforce").await.is_err());
}

/// A profile that now names another protocol than the one an attempt was
/// admitted under is refused before the code exchange, and one that names
/// another protocol than a credential was minted under closes its refresh
/// before any request; neither falls back to the default protocol.
#[tokio::test]
async fn same_id_protocol_retargeting_is_refused_before_completion_and_refresh() {
    let fx = fixture().await;
    let attempt = slot("slack-retarget");
    let start = fx
        .authority
        .login_start(&fx.target(&attempt, "slack"), REDIRECT)
        .await
        .unwrap();
    fx.slack_exchange("xoxp-1", 1);
    let retargeted = fx.authority_with(&[("slack", None)]);
    let refused = retargeted
        .login_complete(
            &attempt,
            ConnectorOAuthCallback {
                redirect_uri: REDIRECT.to_owned(),
                state: start.state.clone(),
                code: "code-1".to_owned(),
                host_attempt_handle: None,
            },
        )
        .await;
    assert!(refused.is_err(), "completion under a retargeted profile");
    assert!(
        fx.state.forms.lock().is_empty(),
        "no code exchange was sent"
    );

    let minted = slot("slack-retarget-refresh");
    fx.login(&minted, "slack").await.unwrap();
    let forms_before = fx.state.forms.lock().len();
    let retargeted = fx.authority_with(&[("slack", Some(SALESFORCE_PROTOCOL_ID))]);
    assert!(retargeted.bearer_token(&minted).await.is_err());
    assert_eq!(fx.state.refreshes.load(Ordering::SeqCst), 0);
    assert_eq!(
        fx.state.forms.lock().len(),
        forms_before,
        "no refresh was sent"
    );
}

/// A rejection of a token from before a logout and a new login changes
/// nothing for the new credential.
#[tokio::test]
async fn a_stale_rejected_token_after_a_new_login_changes_nothing() {
    let fx = fixture().await;
    let slot = slot("slack-relogin");
    fx.slack_exchange("xoxp-1", 43200);
    fx.login(&slot, "slack").await.unwrap();
    fx.authority.logout(&slot).await.unwrap();
    fx.slack_exchange("xoxp-2", 43200);
    fx.login(&slot, "slack").await.unwrap();

    let outcome = fx
        .authority
        .report_credential_rejection(
            &slot,
            "xoxp-1",
            200,
            br#"{"ok":false,"error":"invalid_auth"}"#,
        )
        .await
        .unwrap();
    assert_eq!(outcome, ConnectorRejectionOutcome::Superseded);
    assert_eq!(fx.state.refreshes.load(Ordering::SeqCst), 0);
}

/// Concurrent rejections of the exact token join one refresh; a rejection
/// dropped mid-refresh leaves the credential usable for the next call.
#[tokio::test]
async fn concurrent_rejections_join_one_refresh_and_survive_cancellation() {
    let fx = fixture().await;
    let slot = slot("slack-concurrent");
    fx.slack_exchange("xoxp-1", 43200);
    fx.login(&slot, "slack").await.unwrap();
    fx.state.refresh_delay_ms.store(300, Ordering::SeqCst);
    fx.slack_refresh("xoxp-2", Some("refresh-2"), "A0APP");
    let rejected = br#"{"ok":false,"error":"invalid_auth"}"#;
    let (first, second) = tokio::join!(
        fx.authority
            .report_credential_rejection(&slot, "xoxp-1", 200, rejected),
        fx.authority
            .report_credential_rejection(&slot, "xoxp-1", 200, rejected),
    );
    // One refreshes; the other joins it or, once it has finished, finds the
    // replacement and changes nothing.
    for outcome in [first.unwrap(), second.unwrap()] {
        assert!(
            matches!(&outcome, ConnectorRejectionOutcome::Superseded)
                || matches!(
                    &outcome,
                    ConnectorRejectionOutcome::Refreshed(token) if token == "xoxp-2"
                ),
            "{outcome:?}"
        );
    }
    assert_eq!(fx.state.refreshes.load(Ordering::SeqCst), 1, "one refresh");

    // Dropped while the refresh is in flight.
    fx.slack_refresh("xoxp-3", Some("refresh-3"), "A0APP");
    let dropped = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        fx.authority
            .report_credential_rejection(&slot, "xoxp-2", 200, rejected),
    )
    .await;
    assert!(dropped.is_err(), "the rejection was dropped mid-refresh");
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    fx.state.refresh_delay_ms.store(0, Ordering::SeqCst);
    let token = fx
        .authority
        .bearer_token(&slot)
        .await
        .expect("the credential stays usable after a dropped rejection")
        .expect("a credential");
    assert!(token == "xoxp-2" || token == "xoxp-3", "{token}");
}

/// With refresh-token rotation configured, a Salesforce refresh that omits
/// the replacement closes the credential instead of reusing the predecessor.
#[tokio::test]
async fn salesforce_rotation_requires_a_replacement() {
    let fx = fixture().await;
    let slot = slot("salesforce-rotating");
    fx.state.sf_tokens.lock().insert("sf-1".to_owned(), true);
    *fx.state.sf_exchange.lock() = Some(fx.sf_body("sf-1", Some("sf-refresh")));
    fx.login(&slot, "salesforce-rotating").await.unwrap();
    fx.state.sf_tokens.lock().insert("sf-2".to_owned(), true);
    *fx.state.sf_refresh.lock() = Some(fx.sf_body("sf-2", None));
    let outcome = fx
        .authority
        .report_credential_rejection(&slot, "sf-1", 401, b"INVALID_SESSION_ID")
        .await;
    assert!(outcome.is_err());
    assert!(
        fx.store
            .load(&TokenKey::from_credential_identity(
                &AuthCredentialIdentity::Account(slot.clone()),
            ))
            .await
            .unwrap()
            .is_none(),
        "the possibly consumed predecessor is never reused"
    );
}

/// A refresh this call ran is reported as refreshed even when the provider
/// returns the same access token, here with a rotated refresh token that the
/// slot now holds.
#[tokio::test]
async fn a_refresh_returning_the_same_access_token_is_reported_refreshed() {
    let fx = fixture().await;
    let slot = slot("salesforce-rotating");
    fx.state.sf_tokens.lock().insert("sf-1".to_owned(), true);
    *fx.state.sf_exchange.lock() = Some(fx.sf_body("sf-1", Some("sf-refresh-1")));
    fx.login(&slot, "salesforce-rotating").await.unwrap();
    *fx.state.sf_refresh.lock() = Some(fx.sf_body("sf-1", Some("sf-refresh-2")));
    let outcome = fx
        .authority
        .report_credential_rejection(&slot, "sf-1", 401, b"INVALID_SESSION_ID")
        .await
        .unwrap();
    assert_eq!(
        outcome,
        ConnectorRejectionOutcome::Refreshed("sf-1".to_owned())
    );
    assert_eq!(fx.state.refreshes.load(Ordering::SeqCst), 1);
    let stored = fx
        .store
        .load(&TokenKey::from_credential_identity(
            &AuthCredentialIdentity::Account(slot.clone()),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.primary_secret.as_deref(), Some("sf-1"));
    assert_eq!(stored.refresh_token.as_deref(), Some("sf-refresh-2"));
}

/// A protocol whose meaning changed under the same id (here Salesforce
/// refresh-token rotation turned on) refuses the admitted completion before
/// the code exchange and closes the refresh of an existing credential before
/// its request.
#[tokio::test]
async fn a_protocol_meaning_changed_under_the_same_id_is_refused() {
    let fx = fixture().await;
    let attempt = slot("salesforce-meaning");
    let start = fx
        .authority
        .login_start(&fx.target(&attempt, "salesforce"), REDIRECT)
        .await
        .unwrap();
    let rotating = SalesforceProtocol::new()
        .allowing_loopback()
        .with_refresh_token_rotation();
    let changed = fx.authority_configured(&PROFILES, false, rotating.clone());
    fx.state.sf_tokens.lock().insert("sf-1".to_owned(), true);
    *fx.state.sf_exchange.lock() = Some(fx.sf_body("sf-1", Some("sf-refresh")));
    assert!(
        changed
            .login_complete(
                &attempt,
                ConnectorOAuthCallback {
                    redirect_uri: REDIRECT.to_owned(),
                    state: start.state,
                    code: "code-1".to_owned(),
                    host_attempt_handle: None,
                },
            )
            .await
            .is_err()
    );
    assert!(
        fx.state.forms.lock().is_empty(),
        "no code exchange was sent"
    );

    let minted = slot("salesforce-meaning-refresh");
    fx.login(&minted, "salesforce").await.unwrap();
    let forms_before = fx.state.forms.lock().len();
    let changed = fx.authority_configured(&PROFILES, false, rotating);
    let outcome = changed
        .report_credential_rejection(&minted, "sf-1", 401, b"INVALID_SESSION_ID")
        .await;
    assert!(outcome.is_err(), "{outcome:?}");
    assert_eq!(
        fx.state.forms.lock().len(),
        forms_before,
        "no refresh was sent"
    );
}

/// A late rejection of a token from before a logout and a new login starts
/// no work on the new credential, even when that one is itself due for
/// refresh.
#[tokio::test]
async fn a_stale_rejection_does_not_refresh_an_expiring_replacement() {
    let fx = fixture().await;
    let slot = slot("slack-stale-expiring");
    fx.slack_exchange("xoxp-1", 43200);
    fx.login(&slot, "slack").await.unwrap();
    fx.authority.logout(&slot).await.unwrap();
    // The replacement expires at once.
    fx.slack_exchange("xoxp-2", 1);
    fx.login(&slot, "slack").await.unwrap();
    fx.slack_refresh("xoxp-3", Some("refresh-3"), "A0APP");

    let outcome = fx
        .authority
        .report_credential_rejection(
            &slot,
            "xoxp-1",
            200,
            br#"{"ok":false,"error":"invalid_auth"}"#,
        )
        .await
        .unwrap();
    assert_eq!(outcome, ConnectorRejectionOutcome::Superseded);
    assert_eq!(fx.state.refreshes.load(Ordering::SeqCst), 0);
}

/// A logout and new login landing between the rejection's mark and the
/// coordinated refresh: the new credential is left alone and the outcome is
/// superseded, not a refresh of the rejected credential.
#[tokio::test]
async fn a_replacement_between_mark_and_refresh_is_not_reported_as_refreshed() {
    let fx = fixture().await;
    let slot = slot("slack-replaced-mid");
    fx.slack_exchange("xoxp-1", 43200);
    fx.login(&slot, "slack").await.unwrap();
    let authority = fx.authority.clone();
    let hook_slot = slot.clone();
    let target = fx.target(&slot, "slack");
    let state = fx.state.clone();
    *fx.coordinator.before_refresh.lock() = Some(Box::new(move || {
        Box::pin(async move {
            authority.logout(&hook_slot).await.unwrap();
            state.slack_tokens.lock().insert(
                "xoxp-9".to_owned(),
                ("T0TEAM".to_owned(), "U0USER".to_owned()),
            );
            *state.slack_exchange.lock() = Some(json!({
                "ok": true,
                "app_id": "A0APP",
                "authed_user": {
                    "id": "U0USER",
                    "scope": "search:read,channels:history",
                    "access_token": "xoxp-9",
                    "token_type": "user",
                    "refresh_token": "refresh-9",
                    "expires_in": 43200,
                },
                "team": { "id": "T0TEAM" },
            }));
            let start = authority.login_start(&target, REDIRECT).await.unwrap();
            authority
                .login_complete(
                    &hook_slot,
                    ConnectorOAuthCallback {
                        redirect_uri: REDIRECT.to_owned(),
                        state: start.state,
                        code: "code-9".to_owned(),
                        host_attempt_handle: None,
                    },
                )
                .await
                .unwrap();
        })
    }));
    fx.slack_refresh("xoxp-2", Some("refresh-2"), "A0APP");

    let outcome = fx
        .authority
        .report_credential_rejection(
            &slot,
            "xoxp-1",
            200,
            br#"{"ok":false,"error":"invalid_auth"}"#,
        )
        .await
        .unwrap();
    assert_eq!(outcome, ConnectorRejectionOutcome::Superseded);
    assert_eq!(fx.state.refreshes.load(Ordering::SeqCst), 0);
    assert_eq!(
        fx.authority.bearer_token(&slot).await.unwrap().as_deref(),
        Some("xoxp-9")
    );
}

/// A pinned fact the refresh response no longer asserts (the enterprise)
/// is not copied forward: the refresh is closed.
#[tokio::test]
async fn a_pinned_fact_missing_from_the_refresh_closes_it() {
    let fx = fixture().await;
    let slot = slot("slack-enterprise");
    fx.slack_exchange("xoxp-1", 1);
    if let Some(body) = fx.state.slack_exchange.lock().as_mut() {
        body["enterprise"] = json!({ "id": "E0ENT" });
    }
    fx.login(&slot, "slack").await.unwrap();
    let pinned = fx
        .stored_metadata(&slot)
        .await
        .protocol_evidence
        .unwrap()
        .pinned;
    assert_eq!(
        pinned.get("enterprise_id").map(String::as_str),
        Some("E0ENT")
    );
    // The refresh answer names no enterprise.
    fx.slack_refresh("xoxp-2", Some("refresh-2"), "A0APP");
    assert!(fx.authority.bearer_token(&slot).await.is_err());
    assert_eq!(fx.state.refreshes.load(Ordering::SeqCst), 1);
}

/// A Slack refresh missing both its replacement and its scopes is closed
/// durably for the missing replacement, not retained for the scopes.
#[tokio::test]
async fn a_refresh_missing_replacement_and_scopes_is_closed_durably() {
    let fx = fixture().await;
    let slot = slot("slack-bare-refresh");
    fx.slack_exchange("xoxp-1", 1);
    fx.login(&slot, "slack").await.unwrap();
    fx.slack_refresh("xoxp-2", None, "A0APP");
    if let Some(body) = fx.state.slack_refresh.lock().as_mut() {
        body.as_object_mut().unwrap().remove("scope");
    }
    assert!(fx.authority.bearer_token(&slot).await.is_err());
    assert!(
        fx.store
            .load(&TokenKey::from_credential_identity(
                &AuthCredentialIdentity::Account(slot.clone()),
            ))
            .await
            .unwrap()
            .is_none(),
        "the possibly consumed predecessor is never retained"
    );
}

/// A profile that asks for the RFC 8707 resource indicator sends it on the
/// protocol's code exchange and refresh too, not only on authorize.
#[tokio::test]
async fn the_resource_indicator_reaches_protocol_token_requests() {
    let fx = fixture().await;
    let authority = fx.authority_configured(
        &PROFILES,
        true,
        SalesforceProtocol::new().allowing_loopback(),
    );
    let slot = slot("salesforce-resource");
    let start = authority
        .login_start(&fx.target(&slot, "salesforce"), REDIRECT)
        .await
        .unwrap();
    assert!(
        start.authorize_url.contains("resource="),
        "{}",
        start.authorize_url
    );
    fx.state.sf_tokens.lock().insert("sf-1".to_owned(), true);
    *fx.state.sf_exchange.lock() = Some(fx.sf_body("sf-1", Some("sf-refresh")));
    authority
        .login_complete(
            &slot,
            ConnectorOAuthCallback {
                redirect_uri: REDIRECT.to_owned(),
                state: start.state,
                code: "code-1".to_owned(),
                host_attempt_handle: None,
            },
        )
        .await
        .unwrap();
    fx.state.sf_tokens.lock().insert("sf-2".to_owned(), true);
    *fx.state.sf_refresh.lock() = Some(fx.sf_body("sf-2", None));
    authority
        .report_credential_rejection(&slot, "sf-1", 401, b"INVALID_SESSION_ID")
        .await
        .unwrap();
    let forms = fx.state.forms.lock().clone();
    assert_eq!(forms.len(), 2);
    for form in &forms {
        assert_eq!(
            form.get("resource").map(String::as_str),
            Some(fx.base.as_str())
        );
    }
}
