#![cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The Google protocol through the generic connector owner, against a local
//! fixture that answers in Google's shapes: offline consent, a confidential
//! Web client, refresh that keeps the stored refresh token, the UserInfo
//! `sub` account proof and typed failures. No live Google calls.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use meerkat_auth_core::auth_store::{ProviderAuthPersistence, TokenKey, TokenStore};
use meerkat_auth_core::connector_login::{
    ConnectorClientAuthProfile, ConnectorLoginError, ConnectorOAuthAuthority,
    ConnectorOAuthCallback, ConnectorOAuthTarget, ConnectorStrategies,
};
use meerkat_auth_core::connector_oauth::{AccountSelection, ConnectorOAuthRefusal};
use meerkat_auth_core::connector_protocol::{
    ConnectorProtocols, GOOGLE_PROTOCOL_ID, GOOGLE_USERINFO_STRATEGY_ID, GoogleProtocol,
    GoogleUserInfoStrategy,
};
use meerkat_auth_core::{EphemeralTokenStore, InMemoryCoordinator};
use meerkat_core::AuthCredentialIdentity;
use meerkat_core::connection::{CredentialAccountId, CredentialAccountRef, RealmId};
use meerkat_runtime::handles::RuntimeOAuthFlowHandle;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::net::TcpListener;

const GMAIL_READ: &str = "https://www.googleapis.com/auth/gmail.readonly";
const USERINFO_EMAIL: &str = "https://www.googleapis.com/auth/userinfo.email";
const REDIRECT: &str = "http://127.0.0.1:41011/connector/oauth/callback";
const SECRET: &str = "web-client-secret";

#[derive(Default)]
struct Provider {
    exchange: Mutex<Option<(StatusCode, Value)>>,
    refresh: Mutex<Option<(StatusCode, Value)>>,
    /// Access token -> UserInfo body.
    userinfo: Mutex<HashMap<String, Value>>,
    forms: Mutex<Vec<HashMap<String, String>>>,
    probes: std::sync::atomic::AtomicUsize,
}

impl Provider {
    fn userinfo_calls(&self) -> usize {
        self.probes.load(std::sync::atomic::Ordering::SeqCst)
    }
}

async fn token(
    State(state): State<Arc<Provider>>,
    Form(form): Form<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    state.forms.lock().push(form.clone());
    let next = match form.get("grant_type").map(String::as_str) {
        Some("authorization_code") => state.exchange.lock().take(),
        Some("refresh_token") => state.refresh.lock().take(),
        _ => None,
    };
    match next {
        Some((status, body)) => (status, Json(body)),
        None => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant"})),
        ),
    }
}

async fn userinfo(
    State(state): State<Arc<Provider>>,
    headers: HeaderMap,
) -> (StatusCode, Json<Value>) {
    state
        .probes
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let bearer = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_owned();
    match state.userinfo.lock().get(&bearer) {
        Some(body) => (StatusCode::OK, Json(body.clone())),
        None => (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid_token"})),
        ),
    }
}

struct Fixture {
    base: String,
    state: Arc<Provider>,
    authority: ConnectorOAuthAuthority,
    store: Arc<dyn TokenStore>,
}

async fn fixture() -> Fixture {
    fixture_with_domains(None, None).await
}

/// A fixture whose protocol pins `protocol_domain` and whose strategy
/// requires `strategy_domain`.
async fn fixture_with_domains(
    protocol_domain: Option<&str>,
    strategy_domain: Option<&str>,
) -> Fixture {
    let state = Arc::new(Provider::default());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/token", post(token))
        .route("/v1/userinfo", get(userinfo))
        .with_state(state.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
    let persistence =
        ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new()));
    let lifecycle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    let flows = Arc::new(RuntimeOAuthFlowHandle::new_with_auth_lease(
        std::time::Duration::from_mins(5),
        lifecycle,
    ));
    let authority = ConnectorOAuthAuthority::with_http(
        persistence,
        flows,
        ConnectorStrategies::default().with(Arc::new({
            let strategy = GoogleUserInfoStrategy::new()
                .with_loopback_base(&base)
                .unwrap();
            match strategy_domain {
                Some(domain) => strategy.with_hosted_domain(domain),
                None => strategy,
            }
        })),
        reqwest::Client::new(),
    )
    .unwrap()
    .with_client_auth_profiles([ConnectorClientAuthProfile {
        id: "google".to_owned(),
        issuer: base.clone(),
        client: "client.apps.googleusercontent.com".to_owned(),
        client_secret: Some(SECRET.into()),
        authorize_params: Vec::new(),
        resource_indicator: false,
        protocol: Some(GOOGLE_PROTOCOL_ID.to_owned()),
        registered_redirect_uris: Vec::new(),
    }])
    .unwrap()
    .with_connector_protocols(ConnectorProtocols::default().with(Arc::new({
        let protocol = GoogleProtocol::new().with_loopback_base(&base).unwrap();
        match protocol_domain {
            Some(domain) => protocol.with_hosted_domain(domain),
            None => protocol,
        }
    })));
    Fixture {
        base,
        state,
        authority,
        store,
    }
}

fn slot(name: &str) -> CredentialAccountRef {
    CredentialAccountRef {
        realm: RealmId::parse("connect").unwrap(),
        account: CredentialAccountId::parse(name).unwrap(),
    }
}

impl Fixture {
    fn target(
        &self,
        slot: &CredentialAccountRef,
        account: AccountSelection,
    ) -> ConnectorOAuthTarget {
        ConnectorOAuthTarget {
            slot: slot.clone(),
            issuer: self.base.clone(),
            client: "client.apps.googleusercontent.com".into(),
            resource: self.base.clone(),
            scopes: [GMAIL_READ.to_owned()].into_iter().collect(),
            strategy_id: GOOGLE_USERINFO_STRATEGY_ID.into(),
            account,
            client_auth_profile: Some("google".into()),
        }
    }

    fn user(&self, access: &str, sub: &str, email: &str) {
        self.state.userinfo.lock().insert(
            access.to_owned(),
            json!({"sub": sub, "email": email, "email_verified": true}),
        );
    }

    fn domain_user(&self, access: &str, sub: &str, domain: &str) {
        self.state.userinfo.lock().insert(
            access.to_owned(),
            json!({"sub": sub, "email": format!("ada@{domain}"), "email_verified": true, "hd": domain}),
        );
    }

    fn refresh(&self, status: StatusCode, body: Value) {
        *self.state.refresh.lock() = Some((status, body));
    }

    fn refreshed(&self, access: &str) -> Value {
        json!({"access_token": access, "expires_in": 1, "scope": self.granted(), "token_type": "Bearer"})
    }

    fn exchange(&self, access: &str, refresh: Option<&str>, scope: &str, expires_in: u64) {
        let mut body = json!({
            "access_token": access,
            "expires_in": expires_in,
            "scope": scope,
            "token_type": "Bearer",
            "id_token": "eyJhbGciOiJSUzI1NiJ9.e30.sig",
        });
        if let Some(refresh) = refresh {
            body["refresh_token"] = json!(refresh);
        }
        *self.state.exchange.lock() = Some((StatusCode::OK, body));
    }

    fn granted(&self) -> String {
        format!("{GMAIL_READ} openid {USERINFO_EMAIL}")
    }

    async fn login(
        &self,
        slot: &CredentialAccountRef,
        account: AccountSelection,
    ) -> Result<meerkat_auth_core::connector_login::ConnectorLoginComplete, ConnectorLoginError>
    {
        let start = self
            .authority
            .login_start(&self.target(slot, account), REDIRECT)
            .await?;
        self.authority
            .login_complete(
                slot,
                ConnectorOAuthCallback {
                    redirect_uri: REDIRECT.to_owned(),
                    state: start.state,
                    code: "4/0-code".to_owned(),
                    host_attempt_handle: None,
                },
            )
            .await
    }

    async fn stored(&self, slot: &CredentialAccountRef) -> bool {
        self.store
            .load(&TokenKey::from_credential_identity(
                &AuthCredentialIdentity::Account(slot.clone()),
            ))
            .await
            .unwrap()
            .is_some()
    }
}

const ADA: &str = "1098765";
const EVE: &str = "2098765";

/// The authorize request asks for offline consent and incremental scopes
/// with the known `sub` as hint; the confidential exchange sends the
/// profile's secret and configured redirect; UserInfo proves the `sub`.
#[tokio::test]
async fn google_login_is_offline_confidential_and_proves_the_sub() {
    let fx = fixture().await;
    let slot = slot("gmail");
    let start = fx
        .authority
        .login_start(
            &fx.target(&slot, AccountSelection::Known(ADA.into())),
            REDIRECT,
        )
        .await
        .unwrap();
    assert!(
        start
            .authorize_url
            .starts_with(&format!("{}/o/oauth2/v2/auth?", fx.base))
    );
    let url = reqwest::Url::parse(&start.authorize_url).unwrap();
    let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
    assert_eq!(query["access_type"], "offline");
    assert_eq!(query["prompt"], "consent");
    assert_eq!(query["include_granted_scopes"], "true");
    assert_eq!(query["login_hint"], ADA);
    assert_eq!(query["redirect_uri"], REDIRECT);
    assert!(query["scope"].split(' ').any(|scope| scope == GMAIL_READ));
    assert!(query["scope"].split(' ').any(|scope| scope == "openid"));
    assert!(!query.contains_key("client_secret"));
    fx.authority.login_cancel(&slot, &start.state).unwrap();

    fx.exchange("ya29.one", Some("1//refresh-one"), &fx.granted(), 3599);
    fx.user("ya29.one", ADA, "ada@example.com");
    let done = fx
        .login(&slot, AccountSelection::Known(ADA.into()))
        .await
        .unwrap();
    assert_eq!(done.verified_account.subject, ADA);
    assert!(done.has_refresh_token);
    let exchange_form = fx.state.forms.lock()[0].clone();
    assert_eq!(exchange_form["grant_type"], "authorization_code");
    assert_eq!(exchange_form["client_secret"], SECRET);
    assert_eq!(exchange_form["redirect_uri"], REDIRECT);
    assert_eq!(
        fx.authority.bearer_token(&slot).await.unwrap().as_deref(),
        Some("ya29.one")
    );
}

/// A refresh without a refresh token keeps the exact stored one and
/// re-proves the same `sub`; a changed email does not rebind; another `sub`
/// is refused; a provider outage keeps the credential, while invalid_grant
/// requires a new login.
#[tokio::test]
async fn google_refresh_retains_the_refresh_token_and_the_subject() {
    let fx = fixture().await;
    let refreshed = slot("gmail-refresh");
    fx.exchange("ya29.one", Some("1//refresh-one"), &fx.granted(), 1);
    fx.user("ya29.one", ADA, "ada@example.com");
    fx.login(&refreshed, AccountSelection::Discover)
        .await
        .unwrap();

    // Same sub, renamed address: still the same account.
    fx.user("ya29.two", ADA, "ada.renamed@example.com");
    fx.refresh(StatusCode::OK, fx.refreshed("ya29.two"));
    assert_eq!(
        fx.authority
            .bearer_token(&refreshed)
            .await
            .unwrap()
            .as_deref(),
        Some("ya29.two")
    );
    let form = fx.state.forms.lock().last().cloned().unwrap();
    assert_eq!(form["grant_type"], "refresh_token");
    assert_eq!(form["refresh_token"], "1//refresh-one");
    assert_eq!(form["client_secret"], SECRET);

    // A provider outage is not credential evidence: nothing is closed.
    fx.refresh(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"error": "backend_error"}),
    );
    assert!(fx.authority.bearer_token(&refreshed).await.is_err());
    assert!(
        fx.stored(&refreshed).await,
        "an outage keeps the credential"
    );
    fx.user("ya29.three", ADA, "ada@example.com");
    fx.refresh(StatusCode::OK, fx.refreshed("ya29.three"));
    assert_eq!(
        fx.authority
            .bearer_token(&refreshed)
            .await
            .unwrap()
            .as_deref(),
        Some("ya29.three")
    );
    let form = fx.state.forms.lock().last().cloned().unwrap();
    assert_eq!(
        form["refresh_token"], "1//refresh-one",
        "still the exact stored refresh token"
    );

    // Another subject answering for the credential is refused.
    fx.user("ya29.four", EVE, "ada@example.com");
    fx.refresh(StatusCode::OK, fx.refreshed("ya29.four"));
    assert!(fx.authority.bearer_token(&refreshed).await.is_err());

    // A revoked grant: invalid_grant needs a new login.
    let revoked = slot("gmail-revoked");
    fx.exchange("ya29.r", Some("1//refresh-r"), &fx.granted(), 1);
    fx.user("ya29.r", ADA, "ada@example.com");
    fx.login(&revoked, AccountSelection::Discover)
        .await
        .unwrap();
    fx.refresh(StatusCode::BAD_REQUEST, json!({"error": "invalid_grant"}));
    assert!(fx.authority.bearer_token(&revoked).await.is_err());
    // The dead grant was closed: the slot is empty until a new login.
    assert_eq!(fx.authority.bearer_token(&revoked).await.unwrap(), None);
    assert!(!fx.stored(&revoked).await);
}

/// The account must be the known `sub`, the login must establish offline
/// access, and a narrower (granular consent) grant is refused.
#[tokio::test]
async fn google_login_refusals() {
    let fx = fixture().await;
    let refused =
        |result: Result<_, ConnectorLoginError>, expected: ConnectorOAuthRefusal| match result {
            Err(ConnectorLoginError::Verification(refusal)) => assert_eq!(refusal, expected),
            other => panic!("expected {expected:?}, got {other:?}"),
        };

    // The same email under another subject is another account.
    let other = slot("gmail-other");
    fx.exchange("ya29.other", Some("1//r"), &fx.granted(), 3599);
    fx.user("ya29.other", EVE, "ada@example.com");
    refused(
        fx.login(&other, AccountSelection::Known(ADA.into())).await,
        ConnectorOAuthRefusal::AccountMismatch,
    );
    assert!(!fx.stored(&other).await);

    // Refused by the offline-enrollment policy before the account probe.
    let offline = slot("gmail-no-refresh");
    fx.exchange("ya29.online", None, &fx.granted(), 3599);
    fx.user("ya29.online", ADA, "ada@example.com");
    let probes = fx.state.userinfo_calls();
    let offline_result = fx.login(&offline, AccountSelection::Discover).await;
    assert!(
        matches!(
            offline_result,
            Err(ConnectorLoginError::TokenExchangeFailed)
        ),
        "{offline_result:?}"
    );
    assert_eq!(fx.state.userinfo_calls(), probes);
    assert!(!fx.stored(&offline).await);

    // Granular consent left out the Gmail scope.
    let narrow = slot("gmail-narrow");
    fx.exchange(
        "ya29.narrow",
        Some("1//r"),
        &format!("openid {USERINFO_EMAIL}"),
        3599,
    );
    fx.user("ya29.narrow", ADA, "ada@example.com");
    refused(
        fx.login(&narrow, AccountSelection::Discover).await,
        ConnectorOAuthRefusal::MissingScopes,
    );
    assert!(!fx.stored(&narrow).await);
}

/// A hosted domain the strategy requires and one the protocol pins are both
/// enforced: a matching account logs in, another domain is refused, and a
/// strategy configured for another domain cannot widen the protocol's pin.
#[tokio::test]
async fn google_hosted_domains_are_all_required() {
    let refused =
        |result: Result<_, ConnectorLoginError>, expected: ConnectorOAuthRefusal| match result {
            Err(ConnectorLoginError::Verification(refusal)) => assert_eq!(refusal, expected),
            other => panic!("expected {expected:?}, got {other:?}"),
        };

    // Protocol and strategy agree: the matching account logs in, another
    // domain is refused.
    let fx = fixture_with_domains(Some("a.example"), Some("a.example")).await;
    let start = fx
        .authority
        .login_start(
            &fx.target(&slot("hd-start"), AccountSelection::Discover),
            REDIRECT,
        )
        .await
        .unwrap();
    assert!(start.authorize_url.contains("hd=a.example"));
    fx.authority
        .login_cancel(&slot("hd-start"), &start.state)
        .unwrap();
    let matching = slot("hd-match");
    fx.exchange("ya29.a", Some("1//a"), &fx.granted(), 3599);
    fx.domain_user("ya29.a", ADA, "a.example");
    fx.login(&matching, AccountSelection::Discover)
        .await
        .unwrap();
    let outsider = slot("hd-outsider");
    fx.exchange("ya29.b", Some("1//b"), &fx.granted(), 3599);
    fx.domain_user("ya29.b", EVE, "b.example");
    refused(
        fx.login(&outsider, AccountSelection::Discover).await,
        ConnectorOAuthRefusal::AccountMismatch,
    );
    assert!(!fx.stored(&outsider).await);

    // Protocol pins A, strategy is configured for B, UserInfo says B: the
    // conflict is refused before the probe and nothing is committed.
    let fx = fixture_with_domains(Some("a.example"), Some("b.example")).await;
    let conflict = slot("hd-conflict");
    fx.exchange("ya29.c", Some("1//c"), &fx.granted(), 3599);
    fx.domain_user("ya29.c", ADA, "b.example");
    let probes = fx.state.userinfo_calls();
    refused(
        fx.login(&conflict, AccountSelection::Discover).await,
        ConnectorOAuthRefusal::InvalidDescriptor,
    );
    assert_eq!(fx.state.userinfo_calls(), probes);
    assert!(!fx.stored(&conflict).await);

    // Protocol pin only: still enforced by the shared strategy.
    let fx = fixture_with_domains(Some("a.example"), None).await;
    let pinned = slot("hd-pinned");
    fx.exchange("ya29.d", Some("1//d"), &fx.granted(), 3599);
    fx.domain_user("ya29.d", ADA, "b.example");
    refused(
        fx.login(&pinned, AccountSelection::Discover).await,
        ConnectorOAuthRefusal::AccountMismatch,
    );
    assert!(!fx.stored(&pinned).await);
}
