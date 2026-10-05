#![cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Generic connector OAuth owner: credential slots, the verified account
//! bound to them, Discover/Known slot admission, owner-assigned scope
//! evidence, nonce custody and refresh custody. The issuer is a local HTTP
//! fixture; the strategy maps the fixture's access tokens to subjects.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use meerkat_auth_core::auth_oauth::OAuthTokenResult;
use meerkat_auth_core::auth_store::{
    CredentialMutationError, CredentialSlotRefusal, PersistedAuthMode, PersistedTokens,
    ProviderAuthPersistence, RefreshCoordinator, TokenKey, TokenStore,
};
use meerkat_auth_core::connector_login::{
    ConnectorAccountStrategy, ConnectorAuthPhase, ConnectorLoginError, ConnectorOAuthAuthority,
    ConnectorOAuthCallback, ConnectorOAuthTarget, ConnectorStrategies,
};
use meerkat_auth_core::connector_oauth::{
    AccountSelection, ConnectorAccountObservation, ConnectorCredentialMetadata,
    ConnectorOAuthDescriptor, ConnectorOAuthRefusal, ScopeEvidence, ScopeEvidenceRef,
};
use meerkat_auth_core::{EphemeralTokenStore, InMemoryCoordinator};
use meerkat_core::AuthCredentialIdentity;
use meerkat_core::connection::{CredentialAccountId, CredentialAccountRef, RealmId};
use meerkat_runtime::handles::RuntimeOAuthFlowHandle;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::net::TcpListener;

const STRATEGY: &str = "fixture-evidence-v1";
const NONCE_STRATEGY: &str = "fixture-id-token-v1";

/// What the fixture token endpoint answers.
#[derive(Default)]
struct Issuer {
    /// Authorization code -> (access token, refresh token, scope).
    codes: Mutex<HashMap<String, (String, Option<String>, Option<String>)>>,
    /// The next refresh response: (access token, refresh token, scope).
    refresh: Mutex<Option<(String, Option<String>, Option<String>)>>,
    /// Refuse the next refresh as a permanent `invalid_grant`.
    refresh_rejected: std::sync::atomic::AtomicBool,
    expires_in: Mutex<u64>,
}

async fn metadata(State((issuer, _)): State<(String, Arc<Issuer>)>) -> Json<Value> {
    Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "code_challenge_methods_supported": ["S256"],
    }))
}

async fn token(
    State((_, state)): State<(String, Arc<Issuer>)>,
    Form(form): Form<HashMap<String, String>>,
) -> (axum::http::StatusCode, Json<Value>) {
    if form.get("grant_type").map(String::as_str) == Some("refresh_token")
        && state
            .refresh_rejected
            .swap(false, std::sync::atomic::Ordering::SeqCst)
    {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_grant" })),
        );
    }
    let (access, refresh, scope) = match form.get("grant_type").map(String::as_str) {
        Some("authorization_code") => state
            .codes
            .lock()
            .get(&form["code"])
            .cloned()
            .expect("fixture code"),
        Some("refresh_token") => state.refresh.lock().take().expect("fixture refresh"),
        other => panic!("unexpected grant {other:?}"),
    };
    let mut body = json!({
        "access_token": access,
        "token_type": "Bearer",
        "expires_in": *state.expires_in.lock(),
    });
    if let Some(refresh) = refresh {
        body["refresh_token"] = json!(refresh);
    }
    if let Some(scope) = scope {
        body["scope"] = json!(scope);
    }
    (axum::http::StatusCode::OK, Json(body))
}

/// Fixture provider evidence: access token -> subject. A token that is not
/// in the table cannot be observed.
struct FixtureStrategy {
    id: &'static str,
    subjects: Arc<Mutex<HashMap<String, String>>>,
    nonces: Arc<Mutex<Vec<Option<String>>>>,
    /// Report these scopes instead of the token response's (a lying strategy).
    scopes_override: Mutex<Option<BTreeSet<String>>>,
}

#[async_trait]
impl ConnectorAccountStrategy for FixtureStrategy {
    fn strategy_id(&self) -> &str {
        self.id
    }

    fn requires_nonce(&self) -> bool {
        self.id == NONCE_STRATEGY
    }

    async fn observe_account(
        &self,
        descriptor: &ConnectorOAuthDescriptor,
        tokens: &OAuthTokenResult,
        nonce: Option<&str>,
    ) -> Result<ConnectorAccountObservation, ConnectorOAuthRefusal> {
        self.nonces.lock().push(nonce.map(str::to_owned));
        let account = self
            .subjects
            .lock()
            .get(&tokens.access_token)
            .cloned()
            .ok_or(ConnectorOAuthRefusal::VerificationUnavailable)?;
        let granted_scopes = self
            .scopes_override
            .lock()
            .clone()
            .unwrap_or_else(|| descriptor.granted_scopes_from_response(tokens));
        Ok(ConnectorAccountObservation {
            account,
            granted_scopes,
        })
    }
}

struct Fixture {
    issuer: String,
    state: Arc<Issuer>,
    authority: ConnectorOAuthAuthority,
    store: Arc<dyn TokenStore>,
    subjects: Arc<Mutex<HashMap<String, String>>>,
    nonces: Arc<Mutex<Vec<Option<String>>>>,
    strategy: Arc<FixtureStrategy>,
    persistence: ProviderAuthPersistence,
    flows: Arc<RuntimeOAuthFlowHandle>,
}

async fn fixture() -> Fixture {
    fixture_with(
        Arc::new(EphemeralTokenStore::new()),
        Arc::new(InMemoryCoordinator::new()),
    )
    .await
}

async fn fixture_with(
    store: Arc<dyn TokenStore>,
    coordinator: Arc<dyn RefreshCoordinator>,
) -> Fixture {
    let state = Arc::new(Issuer {
        expires_in: Mutex::new(3600),
        ..Issuer::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/.well-known/oauth-authorization-server", get(metadata))
        .route("/token", post(token))
        .with_state((issuer.clone(), state.clone()));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let persistence = ProviderAuthPersistence::new(store.clone(), coordinator);
    let lifecycle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    let flows = Arc::new(RuntimeOAuthFlowHandle::new_with_auth_lease(
        std::time::Duration::from_secs(300),
        lifecycle,
    ));
    let subjects = Arc::new(Mutex::new(HashMap::new()));
    let nonces = Arc::new(Mutex::new(Vec::new()));
    let strategy = Arc::new(FixtureStrategy {
        id: STRATEGY,
        subjects: subjects.clone(),
        nonces: nonces.clone(),
        scopes_override: Mutex::new(None),
    });
    let nonce_strategy = Arc::new(FixtureStrategy {
        id: NONCE_STRATEGY,
        subjects: subjects.clone(),
        nonces: nonces.clone(),
        scopes_override: Mutex::new(None),
    });
    let authority = ConnectorOAuthAuthority::with_http(
        persistence.clone(),
        flows.clone(),
        ConnectorStrategies::default()
            .with(strategy.clone())
            .with(nonce_strategy),
        reqwest::Client::new(),
    )
    .expect("the runtime flow owner is AuthMachine-owned");
    Fixture {
        issuer,
        state,
        authority,
        store,
        subjects,
        nonces,
        strategy,
        persistence,
        flows,
    }
}

fn slot(realm: &str, name: &str) -> CredentialAccountRef {
    CredentialAccountRef {
        realm: RealmId::parse(realm).unwrap(),
        account: CredentialAccountId::parse(name).unwrap(),
    }
}

fn redirect(port: u16) -> String {
    format!("http://127.0.0.1:{port}/connector/oauth/callback")
}

impl Fixture {
    fn target(
        &self,
        slot: &CredentialAccountRef,
        account: AccountSelection,
    ) -> ConnectorOAuthTarget {
        ConnectorOAuthTarget {
            slot: slot.clone(),
            issuer: self.issuer.clone(),
            client: "connector-client".into(),
            resource: "https://api.service.example".into(),
            scopes: ["files.read".to_owned()].into(),
            strategy_id: STRATEGY.into(),
            account,
        }
    }

    /// Provision what the issuer answers for `code`, and whose token it is.
    fn grant(&self, code: &str, access: &str, subject: &str, scope: Option<&str>) {
        self.state.codes.lock().insert(
            code.to_owned(),
            (
                access.to_owned(),
                Some(format!("refresh-{access}")),
                scope.map(str::to_owned),
            ),
        );
        self.subjects
            .lock()
            .insert(access.to_owned(), subject.to_owned());
    }

    async fn start(&self, target: &ConnectorOAuthTarget, port: u16) -> (String, String) {
        let start = self
            .authority
            .login_start(target, &redirect(port))
            .await
            .expect("admit");
        (start.state, start.authorize_url)
    }

    async fn complete(
        &self,
        slot: &CredentialAccountRef,
        state: &str,
        code: &str,
        port: u16,
    ) -> Result<meerkat_auth_core::connector_login::ConnectorLoginComplete, ConnectorLoginError>
    {
        self.authority
            .login_complete(
                slot,
                ConnectorOAuthCallback {
                    redirect_uri: redirect(port),
                    state: state.to_owned(),
                    code: code.to_owned(),
                },
            )
            .await
    }

    async fn login(
        &self,
        slot: &CredentialAccountRef,
        account: AccountSelection,
        code: &str,
    ) -> Result<meerkat_auth_core::connector_login::ConnectorLoginComplete, ConnectorLoginError>
    {
        let target = self.target(slot, account);
        let (state, _) = self.start(&target, 41001).await;
        self.complete(slot, &state, code, 41001).await
    }

    async fn stored(&self, slot: &CredentialAccountRef) -> Option<PersistedTokens> {
        self.store
            .load(&TokenKey::from_credential_identity(
                &AuthCredentialIdentity::Account(slot.clone()),
            ))
            .await
            .unwrap()
    }
}

fn slot_refusal(
    result: Result<impl std::fmt::Debug, ConnectorLoginError>,
) -> CredentialSlotRefusal {
    match result {
        Err(ConnectorLoginError::Slot(refusal)) => refusal,
        other => panic!("expected a slot refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn discover_binds_the_verified_account_and_status_keeps_slot_and_account_apart() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    let done = fx
        .login(&work, AccountSelection::Discover, "code-a")
        .await
        .expect("discover commits into an empty slot");
    assert_eq!(done.slot, work);
    assert_eq!(done.verified_account.subject, "subject-a");
    assert_eq!(done.verified_account.issuer, fx.issuer);
    assert_eq!(done.verified_account.strategy_id, STRATEGY);
    assert_eq!(done.scope_evidence, ScopeEvidence::TokenEndpointResponse);

    let stored = fx
        .stored(&work)
        .await
        .expect("stored under the admitted slot");
    assert_eq!(stored.auth_mode, PersistedAuthMode::ConnectorOauth);
    assert_eq!(stored.account_id.as_deref(), Some("subject-a"));

    let status = fx.authority.status(&work).await.unwrap();
    assert_eq!(status.phase, ConnectorAuthPhase::Authorized);
    assert_eq!(status.slot, work);
    assert_eq!(
        status.verified_account.map(|account| account.subject),
        Some("subject-a".to_owned())
    );
    assert_eq!(
        status.scope_evidence,
        Some(ScopeEvidence::TokenEndpointResponse)
    );
    assert_eq!(
        fx.authority.bearer_token(&work).await.unwrap().as_deref(),
        Some("access-a")
    );
}

#[tokio::test]
async fn two_slots_for_one_connector_keep_distinct_accounts() {
    let fx = fixture().await;
    let personal = slot("tenant-a", "drive-personal");
    let work = slot("tenant-a", "drive-work");
    fx.grant("code-p", "access-p", "subject-p", Some("files.read"));
    fx.grant("code-w", "access-w", "subject-w", Some("files.read"));
    fx.login(&personal, AccountSelection::Discover, "code-p")
        .await
        .unwrap();
    fx.login(&work, AccountSelection::Discover, "code-w")
        .await
        .unwrap();
    for (slot, subject, access) in [
        (&personal, "subject-p", "access-p"),
        (&work, "subject-w", "access-w"),
    ] {
        let status = fx.authority.status(slot).await.unwrap();
        assert_eq!(status.verified_account.unwrap().subject, subject);
        assert_eq!(
            fx.authority.bearer_token(slot).await.unwrap().as_deref(),
            Some(access)
        );
    }
}

#[tokio::test]
async fn racing_discovers_with_the_same_account_and_independent_grants_first_commit_wins() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    fx.grant("code-1", "access-1", "subject-a", Some("files.read"));
    fx.grant("code-2", "access-2", "subject-a", Some("files.read"));
    let target = fx.target(&work, AccountSelection::Discover);
    let (first, _) = fx.start(&target, 41001).await;
    let (second, _) = fx.start(&target, 41002).await;
    fx.complete(&work, &first, "code-1", 41001).await.unwrap();
    let before = fx.stored(&work).await.unwrap();
    assert_eq!(
        slot_refusal(fx.complete(&work, &second, "code-2", 41002).await),
        CredentialSlotRefusal::Occupied
    );
    assert_eq!(
        fx.stored(&work).await.unwrap(),
        before,
        "first grant untouched"
    );
    assert_eq!(
        fx.authority.bearer_token(&work).await.unwrap().as_deref(),
        Some("access-1")
    );
    // The losing attempt is consumed: it cannot be replayed.
    assert!(matches!(
        fx.complete(&work, &second, "code-2", 41002).await,
        Err(ConnectorLoginError::Flow(_))
    ));
}

#[tokio::test]
async fn racing_discovers_for_different_accounts_cannot_overwrite_the_winner() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    fx.grant("code-b", "access-b", "subject-b", Some("files.read"));
    let target = fx.target(&work, AccountSelection::Discover);
    let (a, _) = fx.start(&target, 41001).await;
    let (b, _) = fx.start(&target, 41002).await;
    fx.complete(&work, &a, "code-a", 41001).await.unwrap();
    let before = fx.stored(&work).await.unwrap();
    assert_eq!(
        slot_refusal(fx.complete(&work, &b, "code-b", 41002).await),
        CredentialSlotRefusal::Occupied
    );
    assert_eq!(fx.stored(&work).await.unwrap(), before);
}

#[tokio::test]
async fn a_wrong_account_reconnect_leaves_the_old_material_untouched() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    fx.grant("code-b", "access-b", "subject-b", Some("files.read"));
    fx.login(&work, AccountSelection::Discover, "code-a")
        .await
        .unwrap();
    let before = fx.stored(&work).await.unwrap();
    // Known(subject-b) proving subject-b: the slot is bound to subject-a.
    assert_eq!(
        slot_refusal(
            fx.login(&work, AccountSelection::Known("subject-b".into()), "code-b")
                .await
        ),
        CredentialSlotRefusal::AccountMismatch
    );
    // Known(subject-a) proving subject-b: refused before the commit.
    fx.grant("code-b2", "access-b", "subject-b", Some("files.read"));
    assert!(matches!(
        fx.login(
            &work,
            AccountSelection::Known("subject-a".into()),
            "code-b2"
        )
        .await,
        Err(ConnectorLoginError::Verification(
            ConnectorOAuthRefusal::AccountMismatch
        ))
    ));
    assert_eq!(fx.stored(&work).await.unwrap(), before);
}

#[tokio::test]
async fn a_compatible_known_reconnect_replaces_the_grant_despite_port_and_scope_changes() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    fx.login(&work, AccountSelection::Discover, "code-a")
        .await
        .unwrap();
    fx.grant(
        "code-a2",
        "access-a2",
        "subject-a",
        Some("files.read files.write"),
    );
    let mut target = fx.target(&work, AccountSelection::Known("subject-a".into()));
    target.scopes.insert("files.write".into());
    let (state, _) = fx.start(&target, 42999).await;
    fx.complete(&work, &state, "code-a2", 42999)
        .await
        .expect("same account and stable context, new loopback port and scopes");
    let stored = fx.stored(&work).await.unwrap();
    assert_eq!(stored.primary_secret.as_deref(), Some("access-a2"));
    assert_eq!(stored.scopes, vec!["files.read", "files.write"]);
}

#[tokio::test]
async fn a_changed_stable_context_is_refused_into_an_occupied_slot() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    fx.login(&work, AccountSelection::Discover, "code-a")
        .await
        .unwrap();
    fx.grant("code-a2", "access-a2", "subject-a", Some("files.read"));
    let mut target = fx.target(&work, AccountSelection::Known("subject-a".into()));
    target.client = "another-client".into();
    let (state, _) = fx.start(&target, 41001).await;
    assert_eq!(
        slot_refusal(fx.complete(&work, &state, "code-a2", 41001).await),
        CredentialSlotRefusal::ContextMismatch
    );
}

#[tokio::test]
async fn the_same_subject_across_realms_and_slots_never_aliases() {
    let fx = fixture().await;
    let a = slot("tenant-a", "drive");
    let b = slot("tenant-b", "drive");
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    fx.login(&a, AccountSelection::Discover, "code-a")
        .await
        .unwrap();
    let status = fx.authority.status(&b).await.unwrap();
    assert_eq!(status.phase, ConnectorAuthPhase::AuthorizationRequired);
    assert!(status.verified_account.is_none());
    assert_eq!(fx.authority.bearer_token(&b).await.unwrap(), None);
    // A second grant for the same subject in the other realm is its own row.
    fx.grant("code-a2", "access-a2", "subject-a", Some("files.read"));
    fx.login(&b, AccountSelection::Discover, "code-a2")
        .await
        .unwrap();
    assert_eq!(
        fx.authority.bearer_token(&a).await.unwrap().as_deref(),
        Some("access-a")
    );
    assert_eq!(
        fx.authority.bearer_token(&b).await.unwrap().as_deref(),
        Some("access-a2")
    );
}

#[tokio::test]
async fn stale_replayed_or_cross_slot_completions_cannot_rebind_a_slot() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    let other = slot("tenant-a", "drive-other");
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    let (state, _) = fx
        .start(&fx.target(&work, AccountSelection::Discover), 41001)
        .await;
    // An attempt admitted for one slot cannot complete into another.
    assert!(matches!(
        fx.complete(&other, &state, "code-a", 41001).await,
        Err(ConnectorLoginError::Flow(_))
    ));
    fx.complete(&work, &state, "code-a", 41001).await.unwrap();
    // A replayed callback finds no live attempt.
    assert!(matches!(
        fx.complete(&work, &state, "code-a", 41001).await,
        Err(ConnectorLoginError::Flow(_))
    ));
    assert_eq!(
        fx.authority.status(&other).await.unwrap().verified_account,
        None
    );
    assert_eq!(
        fx.stored(&work).await.unwrap().account_id.as_deref(),
        Some("subject-a")
    );
}

#[tokio::test]
async fn a_strategy_cannot_widen_the_owner_parsed_token_response_scopes() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    *fx.strategy.scopes_override.lock() =
        Some(["files.read".to_owned(), "admin".to_owned()].into());
    assert!(matches!(
        fx.login(&work, AccountSelection::Discover, "code-a").await,
        Err(ConnectorLoginError::Verification(
            ConnectorOAuthRefusal::CredentialMismatch
        ))
    ));
    assert!(fx.stored(&work).await.is_none());
}

#[tokio::test]
async fn an_id_token_strategy_receives_the_attempt_nonce_from_the_authorize_url() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    let mut target = fx.target(&work, AccountSelection::Discover);
    target.strategy_id = NONCE_STRATEGY.into();
    let (state, url) = fx.start(&target, 41001).await;
    let nonce = reqwest::Url::parse(&url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "nonce")
        .map(|(_, value)| value.into_owned())
        .expect("authorize URL carries the nonce");
    assert_ne!(nonce, state, "nonce and state are independent secrets");
    fx.complete(&work, &state, "code-a", 41001).await.unwrap();
    assert_eq!(fx.nonces.lock().as_slice(), [Some(nonce)]);

    // A strategy without ID tokens gets no nonce and none is put in the URL.
    let plain = slot("tenant-a", "drive-plain");
    fx.grant("code-p", "access-p", "subject-p", Some("files.read"));
    let (_, url) = fx
        .start(&fx.target(&plain, AccountSelection::Discover), 41001)
        .await;
    assert!(!url.contains("nonce="));
}

async fn login_expiring(fx: &Fixture, work: &CredentialAccountRef) {
    *fx.state.expires_in.lock() = 30;
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    fx.login(work, AccountSelection::Discover, "code-a")
        .await
        .unwrap();
    *fx.state.expires_in.lock() = 3600;
}

#[tokio::test]
async fn a_refresh_without_scope_retains_the_original_grant_by_reference() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    login_expiring(&fx, &work).await;
    let granted_at = ConnectorCredentialMetadata::from_tokens(&fx.stored(&work).await.unwrap())
        .unwrap()
        .granted_at_epoch_secs;
    *fx.state.refresh.lock() = Some(("access-a2".into(), None, None));
    fx.subjects
        .lock()
        .insert("access-a2".into(), "subject-a".into());
    assert_eq!(
        fx.authority.bearer_token(&work).await.unwrap().as_deref(),
        Some("access-a2")
    );
    let stored = fx.stored(&work).await.unwrap();
    assert_eq!(stored.scopes, vec!["files.read"]);
    // No replacement refresh token: the existing one is kept.
    assert_eq!(stored.refresh_token.as_deref(), Some("refresh-access-a"));
    assert_eq!(
        ConnectorCredentialMetadata::from_tokens(&stored)
            .unwrap()
            .scope_evidence,
        ScopeEvidence::RetainedOnRefresh {
            from: ScopeEvidenceRef {
                granted_at_epoch_secs: granted_at
            }
        }
    );
    // Refresh proves the subject without any ID-token nonce.
    assert_eq!(fx.nonces.lock().last(), Some(&None));
}

#[tokio::test]
async fn a_refresh_rotates_the_refresh_token_with_the_access_token() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    login_expiring(&fx, &work).await;
    *fx.state.refresh.lock() = Some((
        "access-a2".into(),
        Some("refresh-rotated".into()),
        Some("files.read".into()),
    ));
    fx.subjects
        .lock()
        .insert("access-a2".into(), "subject-a".into());
    fx.authority.bearer_token(&work).await.unwrap();
    let stored = fx.stored(&work).await.unwrap();
    assert_eq!(stored.primary_secret.as_deref(), Some("access-a2"));
    assert_eq!(stored.refresh_token.as_deref(), Some("refresh-rotated"));
    assert_eq!(
        ConnectorCredentialMetadata::from_tokens(&stored)
            .unwrap()
            .scope_evidence,
        ScopeEvidence::TokenEndpointResponse
    );
}

#[tokio::test]
async fn a_refresh_that_narrows_a_required_scope_is_refused_without_change() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    login_expiring(&fx, &work).await;
    let before = fx.stored(&work).await.unwrap();
    *fx.state.refresh.lock() = Some(("access-a2".into(), None, Some("profile".into())));
    fx.subjects
        .lock()
        .insert("access-a2".into(), "subject-a".into());
    assert!(matches!(
        fx.authority.bearer_token(&work).await,
        Err(ConnectorLoginError::Verification(
            ConnectorOAuthRefusal::MissingScopes
        ))
    ));
    assert_eq!(fx.stored(&work).await.unwrap(), before);
}

#[tokio::test]
async fn a_refresh_for_another_subject_is_refused_without_change() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    login_expiring(&fx, &work).await;
    let before = fx.stored(&work).await.unwrap();
    *fx.state.refresh.lock() = Some(("access-b".into(), None, None));
    fx.subjects
        .lock()
        .insert("access-b".into(), "subject-b".into());
    assert!(matches!(
        fx.authority.bearer_token(&work).await,
        Err(ConnectorLoginError::Verification(
            ConnectorOAuthRefusal::AccountMismatch
        ))
    ));
    assert_eq!(fx.stored(&work).await.unwrap(), before);
}

#[tokio::test]
async fn connector_slots_refuse_other_modes_and_the_loader_refuses_their_rows() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    let key = TokenKey::from_credential_identity(&AuthCredentialIdentity::Account(work.clone()));
    let mut foreign = PersistedTokens::api_key("not-a-connector-credential");
    foreign.auth_mode = PersistedAuthMode::McpOauth;
    fx.store.save(&key, &foreign).await.unwrap();
    assert_eq!(fx.authority.bearer_token(&work).await.unwrap(), None);
    assert_eq!(
        fx.authority.status(&work).await.unwrap().phase,
        ConnectorAuthPhase::AuthorizationRequired
    );
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    assert_eq!(
        slot_refusal(fx.login(&work, AccountSelection::Discover, "code-a").await),
        CredentialSlotRefusal::Occupied
    );
    fx.grant("code-a2", "access-a", "subject-a", Some("files.read"));
    assert_eq!(
        slot_refusal(
            fx.login(
                &work,
                AccountSelection::Known("subject-a".into()),
                "code-a2"
            )
            .await
        ),
        CredentialSlotRefusal::ModeMismatch
    );
    assert_eq!(fx.store.load(&key).await.unwrap(), Some(foreign));
}

#[tokio::test]
async fn logout_frees_the_slot_for_a_new_account_and_refuses_foreign_rows() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    fx.login(&work, AccountSelection::Discover, "code-a")
        .await
        .unwrap();
    fx.authority.logout(&work).await.unwrap();
    assert!(fx.stored(&work).await.is_none());
    assert_eq!(
        fx.authority.status(&work).await.unwrap().phase,
        ConnectorAuthPhase::AuthorizationRequired
    );
    // An empty slot is already disconnected.
    fx.authority.logout(&work).await.unwrap();
    fx.grant("code-b", "access-b", "subject-b", Some("files.read"));
    fx.login(&work, AccountSelection::Discover, "code-b")
        .await
        .expect("a disconnected slot accepts a new account");
    assert_eq!(
        fx.stored(&work).await.unwrap().account_id.as_deref(),
        Some("subject-b")
    );

    let foreign_slot = slot("tenant-a", "not-a-connector");
    let key =
        TokenKey::from_credential_identity(&AuthCredentialIdentity::Account(foreign_slot.clone()));
    let foreign = PersistedTokens::api_key("another-owners-credential");
    fx.store.save(&key, &foreign).await.unwrap();
    assert!(matches!(
        fx.authority.logout(&foreign_slot).await,
        Err(ConnectorLoginError::Slot(
            CredentialSlotRefusal::ModeMismatch
        ))
    ));
    assert_eq!(fx.store.load(&key).await.unwrap(), Some(foreign));
}

#[tokio::test]
async fn other_publication_paths_never_overwrite_a_connector_credential() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    fx.login(&work, AccountSelection::Discover, "code-a")
        .await
        .unwrap();
    let before = fx.stored(&work).await.unwrap();
    let lease = meerkat_auth_core::oauth_flow::OAuthFlowAuthority::generated_credential_lifecycle(
        fx.flows.as_ref(),
    )
    .unwrap();
    let lease_key = meerkat_core::handles::LeaseKey::from_credential_identity(
        &AuthCredentialIdentity::Account(work.clone()),
    );
    let lifecycle_before = lease.snapshot(&lease_key);
    let refused = meerkat_auth_core::save_tokens_and_publish_lifecycle(
        fx.persistence.clone(),
        lease.clone(),
        AuthCredentialIdentity::Account(work.clone()),
        PersistedTokens::api_key("direct-secret"),
    )
    .await;
    assert!(matches!(
        refused,
        Err(CredentialMutationError::SlotRefused(
            CredentialSlotRefusal::ModeMismatch
        ))
    ));
    assert_eq!(fx.stored(&work).await.unwrap(), before);

    // A same-mode connector row through the generic path is refused too,
    // whatever it changes (account, context, scopes, secret), and so is a
    // connector row into an empty slot: only a verified completion publishes.
    let mut forged = before.clone();
    forged.account_id = Some("subject-forged".into());
    forged.primary_secret = Some("forged-secret".into());
    forged.scopes = vec!["admin".into(), "files.read".into()];
    let mut metadata = ConnectorCredentialMetadata::from_tokens(&before).unwrap();
    metadata.client = "forged-client".into();
    metadata.requested_scopes.insert("admin".into());
    forged.metadata = metadata.to_value();
    let empty = slot("tenant-a", "empty");
    for (target, previous) in [(work.clone(), Some(before.clone())), (empty, None)] {
        let refused = meerkat_auth_core::save_tokens_and_publish_lifecycle(
            fx.persistence.clone(),
            lease.clone(),
            AuthCredentialIdentity::Account(target.clone()),
            forged.clone(),
        )
        .await;
        assert!(matches!(
            refused,
            Err(CredentialMutationError::SlotRefused(
                CredentialSlotRefusal::UnverifiedConnectorPublication
            ))
        ));
        assert_eq!(fx.stored(&target).await, previous);
    }
    assert_eq!(lease.snapshot(&lease_key), lifecycle_before);
}

/// Fails every `clear`, so a permanent refresh rejection cannot remove the
/// durable bytes.
struct FailClearStore(EphemeralTokenStore);

#[async_trait]
impl TokenStore for FailClearStore {
    async fn load(
        &self,
        key: &TokenKey,
    ) -> Result<Option<PersistedTokens>, meerkat_auth_core::auth_store::TokenStoreError> {
        self.0.load(key).await
    }
    async fn save(
        &self,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), meerkat_auth_core::auth_store::TokenStoreError> {
        self.0.save(key, tokens).await
    }
    async fn clear(
        &self,
        _key: &TokenKey,
    ) -> Result<(), meerkat_auth_core::auth_store::TokenStoreError> {
        Err(meerkat_auth_core::auth_store::TokenStoreError::Io(
            "injected clear failure".into(),
        ))
    }
    async fn list(&self) -> Result<Vec<TokenKey>, meerkat_auth_core::auth_store::TokenStoreError> {
        self.0.list().await
    }
    fn backend_name(&self) -> &'static str {
        "fail-clear"
    }
}

#[tokio::test]
async fn status_follows_lifecycle_admission_after_a_failed_clear() {
    let fx = fixture_with(
        Arc::new(FailClearStore(EphemeralTokenStore::new())),
        Arc::new(InMemoryCoordinator::new()),
    )
    .await;
    let work = slot("tenant-a", "drive-work");
    login_expiring(&fx, &work).await;
    fx.state
        .refresh_rejected
        .store(true, std::sync::atomic::Ordering::SeqCst);
    // The token endpoint permanently rejects the refresh and the clear fails:
    // the bytes stay, but the lifecycle requires reauthentication.
    assert!(matches!(
        fx.authority.bearer_token(&work).await,
        Err(ConnectorLoginError::TokenStore(_))
    ));
    assert!(
        fx.stored(&work).await.is_some(),
        "the failed clear kept the bytes"
    );
    let status = fx.authority.status(&work).await.unwrap();
    assert_eq!(status.phase, ConnectorAuthPhase::ReauthRequired);
    assert_eq!(status.verified_account.unwrap().subject, "subject-a");
    assert!(status.scope_evidence.is_none() && status.scopes.is_empty());
    assert!(fx.authority.bearer_token(&work).await.is_err());
}

#[tokio::test]
async fn status_refuses_an_unmarked_connector_row() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    fx.login(&work, AccountSelection::Discover, "code-a")
        .await
        .unwrap();
    // The same bytes without the AuthMachine lifecycle marker.
    let mut unmarked = fx.stored(&work).await.unwrap();
    let mut metadata = unmarked.metadata.as_object().unwrap().clone();
    metadata.retain(|key, _| key == "connector");
    unmarked.metadata = Value::Object(metadata);
    let other = slot("tenant-a", "drive-unmarked");
    fx.store
        .save(
            &TokenKey::from_credential_identity(&AuthCredentialIdentity::Account(other.clone())),
            &unmarked,
        )
        .await
        .unwrap();
    assert_eq!(
        fx.authority.status(&other).await.unwrap().phase,
        ConnectorAuthPhase::ReauthRequired
    );
    assert!(matches!(
        fx.authority.bearer_token(&other).await,
        Err(ConnectorLoginError::ReauthRequired)
    ));
}

type Hook = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

/// Runs one injected step after a bearer call admitted its credential and
/// before the coordinator's refresh reload: the interleaving point.
struct InterleavingCoordinator {
    inner: InMemoryCoordinator,
    before_refresh: Mutex<Option<Hook>>,
}

#[async_trait]
impl RefreshCoordinator for InterleavingCoordinator {
    async fn with_exclusive_mutation(
        &self,
        key: TokenKey,
        mutation_fn: meerkat_auth_core::auth_store::CredentialMutationFn,
    ) -> Result<meerkat_auth_core::auth_store::CredentialMutationOutcome, CredentialMutationError>
    {
        self.inner.with_exclusive_mutation(key, mutation_fn).await
    }

    async fn with_refresh(
        &self,
        key: TokenKey,
        refresh_fn: meerkat_auth_core::auth_store::RefreshFn,
    ) -> Result<PersistedTokens, meerkat_auth_core::auth_store::RefreshError> {
        let hook = self.before_refresh.lock().take();
        if let Some(hook) = hook {
            hook.await;
        }
        self.inner.with_refresh(key, refresh_fn).await
    }
}

/// Admit an expiring credential for subject-a, then replace the slot
/// (logout + discover) before the refresh reload runs.
async fn bearer_across_a_replacement(
    replacement_subject: &str,
    replacement_client: &str,
) -> (
    Result<Option<String>, ConnectorLoginError>,
    Fixture,
    CredentialAccountRef,
) {
    let coordinator = Arc::new(InterleavingCoordinator {
        inner: InMemoryCoordinator::new(),
        before_refresh: Mutex::new(None),
    });
    let fx = fixture_with(Arc::new(EphemeralTokenStore::new()), coordinator.clone()).await;
    let work = slot("tenant-a", "drive-work");
    login_expiring(&fx, &work).await;
    fx.grant(
        "code-r",
        "access-r",
        replacement_subject,
        Some("files.read"),
    );
    let mut target = fx.target(&work, AccountSelection::Discover);
    target.client = replacement_client.to_owned();
    let authority = fx.authority.clone();
    let hook_slot = work.clone();
    *coordinator.before_refresh.lock() = Some(Box::pin(async move {
        authority.logout(&hook_slot).await.unwrap();
        let start = authority
            .login_start(&target, &redirect(41009))
            .await
            .unwrap();
        authority
            .login_complete(
                &hook_slot,
                ConnectorOAuthCallback {
                    redirect_uri: redirect(41009),
                    state: start.state,
                    code: "code-r".into(),
                },
            )
            .await
            .unwrap();
    }));
    let result = fx.authority.bearer_token(&work).await;
    (result, fx, work)
}

#[tokio::test]
async fn bearer_holds_to_the_admitted_binding_across_a_slot_replacement() {
    // Same subject, same context: still the admitted binding.
    let (result, _, _) = bearer_across_a_replacement("subject-a", "connector-client").await;
    assert_eq!(result.unwrap().as_deref(), Some("access-r"));

    // Same subject, another client: refused; a later call admits it.
    let (result, fx, work) = bearer_across_a_replacement("subject-a", "another-client").await;
    assert!(matches!(
        result,
        Err(ConnectorLoginError::Slot(
            CredentialSlotRefusal::ContextMismatch
        ))
    ));
    assert_eq!(
        fx.authority.bearer_token(&work).await.unwrap().as_deref(),
        Some("access-r")
    );

    // Another subject: refused.
    let (result, _, _) = bearer_across_a_replacement("subject-b", "connector-client").await;
    assert!(matches!(
        result,
        Err(ConnectorLoginError::Verification(
            ConnectorOAuthRefusal::AccountMismatch
        ))
    ));
}
