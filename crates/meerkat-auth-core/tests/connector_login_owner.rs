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
    ProviderAuthPersistence, RefreshCoordinator, TokenKey, TokenStore, TokenStoreAccess,
    TokenStoreError, TokenStoreRefusal,
};
use meerkat_auth_core::connector_login::{
    ConnectorAccountStrategy, ConnectorAuthPhase, ConnectorCallEntry, ConnectorCallExpectation,
    ConnectorClientAuthProfile, ConnectorClientAuthSource, ConnectorClientSecret,
    ConnectorLoginError, ConnectorOAuthAuthority, ConnectorOAuthCallback, ConnectorOAuthTarget,
    ConnectorStrategies, clear_connector_client_secret, save_connector_client_secret,
};
use meerkat_auth_core::connector_oauth::{
    AccountSelection, ConnectorAccountObservation, ConnectorCredentialMetadata,
    ConnectorOAuthDescriptor, ConnectorOAuthRefusal, ScopeEvidence, ScopeEvidenceRef,
};
use meerkat_auth_core::{EphemeralTokenStore, InMemoryCoordinator};
use meerkat_core::AuthCredentialIdentity;
use meerkat_core::connection::{CredentialAccountId, CredentialAccountRef, RealmId};
use meerkat_core::handles::{AuthLeaseHandle, LeaseKey};
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
    refresh_requests: std::sync::atomic::AtomicUsize,
    expires_in: Mutex<u64>,
    /// Every token-endpoint form received, in order.
    forms: Mutex<Vec<HashMap<String, String>>>,
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
    state.forms.lock().push(form.clone());
    if form.get("grant_type").map(String::as_str) == Some("refresh_token") {
        state
            .refresh_requests
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
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
        std::time::Duration::from_mins(5),
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
            client_auth_profile: None,
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
                    host_attempt_handle: None,
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

async fn cold_owner_read_preserves_credential(read_status: bool) {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-cold-owner");
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    fx.login(&work, AccountSelection::Discover, "code-a")
        .await
        .unwrap();
    let retained = fx.stored(&work).await.unwrap();

    // Reuse the actual committed credential with a fresh process-local owner.
    // Neither status nor bearer has warmed this owner's lifecycle projection.
    let lifecycle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    let lease_key =
        LeaseKey::from_credential_identity(&AuthCredentialIdentity::Account(work.clone()));
    assert!(!lifecycle.snapshot(&lease_key).credential_present);
    let flows = Arc::new(RuntimeOAuthFlowHandle::new_with_auth_lease(
        std::time::Duration::from_mins(5),
        lifecycle,
    ));
    let cold = ConnectorOAuthAuthority::with_http(
        fx.persistence.clone(),
        flows,
        ConnectorStrategies::default().with(fx.strategy.clone()),
        reqwest::Client::new(),
    )
    .unwrap();

    if read_status {
        let status = tokio::time::timeout(std::time::Duration::from_secs(5), cold.status(&work))
            .await
            .expect("cold status must not reacquire its already-held lifecycle guard")
            .unwrap();
        assert_eq!(status.phase, ConnectorAuthPhase::Authorized);
        assert_eq!(status.slot, work);
        assert_eq!(status.verified_account.unwrap().subject, "subject-a");
    } else {
        let bearer =
            tokio::time::timeout(std::time::Duration::from_secs(5), cold.bearer_token(&work))
                .await
                .expect("cold bearer must not reacquire its already-held lifecycle guard")
                .unwrap();
        assert_eq!(bearer.as_deref(), Some("access-a"));
    }
    assert_eq!(fx.stored(&work).await, Some(retained));
    assert_eq!(
        fx.state
            .refresh_requests
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "rehydrating an unexpired credential must not refresh it"
    );
}

#[tokio::test]
async fn cold_owner_status_rehydrates_without_refresh_or_credential_change() {
    cold_owner_read_preserves_credential(true).await;
}

#[tokio::test]
async fn cold_owner_bearer_rehydrates_without_refresh_or_credential_change() {
    cold_owner_read_preserves_credential(false).await;
}

#[tokio::test]
async fn cold_owner_expiring_bearer_refreshes_once_and_preserves_account_and_scope() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-cold-refresh");
    login_expiring(&fx, &work).await;
    let before = fx.stored(&work).await.unwrap();
    let original = ConnectorCredentialMetadata::from_tokens(&before).unwrap();
    assert_eq!(
        fx.state
            .refresh_requests
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    *fx.state.refresh.lock() = Some(("access-a2".into(), None, None));
    fx.subjects
        .lock()
        .insert("access-a2".into(), "subject-a".into());

    // The committed expiring credential is the only lifecycle evidence the
    // fresh owner has; do not warm it with status before the first bearer call.
    let lifecycle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    let lease_key =
        LeaseKey::from_credential_identity(&AuthCredentialIdentity::Account(work.clone()));
    assert!(!lifecycle.snapshot(&lease_key).credential_present);
    let flows = Arc::new(RuntimeOAuthFlowHandle::new_with_auth_lease(
        std::time::Duration::from_mins(5),
        lifecycle.clone(),
    ));
    let cold = ConnectorOAuthAuthority::with_http(
        fx.persistence.clone(),
        flows,
        ConnectorStrategies::default().with(fx.strategy.clone()),
        reqwest::Client::new(),
    )
    .unwrap();

    let status = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        assert_eq!(
            cold.bearer_token(&work).await.unwrap().as_deref(),
            Some("access-a2")
        );
        assert_eq!(
            cold.bearer_token(&work).await.unwrap().as_deref(),
            Some("access-a2"),
            "the refreshed credential must be reusable without another refresh"
        );
        cold.status(&work).await.unwrap()
    })
    .await
    .expect("cold rehydration and refresh must not reacquire an already-held lifecycle guard");
    assert!(lifecycle.snapshot(&lease_key).credential_present);
    assert_eq!(
        fx.state
            .refresh_requests
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "cold rehydration, repeated bearer and status must refresh exactly once"
    );

    let stored = fx.stored(&work).await.unwrap();
    assert_eq!(stored.primary_secret.as_deref(), Some("access-a2"));
    assert_eq!(stored.auth_mode, PersistedAuthMode::ConnectorOauth);
    assert_eq!(stored.account_id, before.account_id);
    assert_eq!(stored.refresh_token, before.refresh_token);
    assert_eq!(stored.scopes, vec!["files.read"]);
    let retained = ScopeEvidence::RetainedOnRefresh {
        from: ScopeEvidenceRef {
            granted_at_epoch_secs: original.granted_at_epoch_secs,
        },
    };
    let metadata = ConnectorCredentialMetadata::from_tokens(&stored).unwrap();
    assert_eq!(metadata.stable_context, original.stable_context);
    assert_eq!(metadata.scope_evidence, retained);
    assert_eq!(status.phase, ConnectorAuthPhase::Authorized);
    assert_eq!(status.slot, work);
    let verified = status.verified_account.unwrap();
    assert_eq!(verified.subject, "subject-a");
    assert_eq!(verified.issuer, fx.issuer);
    assert_eq!(verified.strategy_id, STRATEGY);
    assert_eq!(status.scopes, vec!["files.read"]);
    assert_eq!(status.scope_evidence, Some(retained));
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

/// A store that refuses chosen reads and writes with a typed
/// credential-access refusal: the next save, every read once armed and the
/// token endpoint saw a refresh, or one numbered read.
#[derive(Default)]
struct RefusingStore {
    inner: EphemeralTokenStore,
    loads: std::sync::atomic::AtomicUsize,
    refuse_load: std::sync::atomic::AtomicUsize,
    saves: std::sync::atomic::AtomicUsize,
    refuse_next_save: std::sync::atomic::AtomicBool,
    refuse_reads_after_refresh: std::sync::atomic::AtomicBool,
    issuer: std::sync::OnceLock<Arc<Issuer>>,
}

impl RefusingStore {
    fn refused(&self, detail: &str) -> TokenStoreError {
        TokenStoreError::refused(
            self.backend_name(),
            TokenStoreAccess::Unattended,
            TokenStoreRefusal::InteractionRequired,
            detail,
        )
    }
}

#[async_trait]
impl TokenStore for RefusingStore {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        use std::sync::atomic::Ordering::SeqCst;
        let number = self.loads.fetch_add(1, SeqCst) + 1;
        if self.refuse_load.load(SeqCst) == number {
            return Err(self.refused("rehydration read needs credential interaction"));
        }
        if self.refuse_reads_after_refresh.load(SeqCst)
            && self
                .issuer
                .get()
                .is_some_and(|issuer| issuer.refresh_requests.load(SeqCst) > 0)
        {
            return Err(self.refused("post-exchange read needs credential interaction"));
        }
        self.inner.load(key).await
    }
    async fn save(&self, key: &TokenKey, tokens: &PersistedTokens) -> Result<(), TokenStoreError> {
        use std::sync::atomic::Ordering::SeqCst;
        self.saves.fetch_add(1, SeqCst);
        if self.refuse_next_save.swap(false, SeqCst) {
            return Err(self.refused("publication save needs credential interaction"));
        }
        self.inner.save(key, tokens).await
    }
    async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        self.inner.clear(key).await
    }
    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        self.inner.list().await
    }
    fn backend_name(&self) -> &'static str {
        "test-connector-store"
    }
    async fn observe_status(
        &self,
        key: &TokenKey,
    ) -> Result<meerkat_auth_core::auth_store::TokenStoreObservation, TokenStoreError> {
        self.inner.observe_status(key).await
    }
}

fn credential_access_cause(error: &ConnectorLoginError) -> Option<(TokenStoreRefusal, &str)> {
    match error {
        ConnectorLoginError::CredentialAccess { refusal, detail } => Some((*refusal, detail)),
        _ => None,
    }
}

/// A refused publication save after the refresh exchange rolls the
/// credential back, and the completed rollback keeps the typed store cause.
#[tokio::test]
async fn a_refused_refresh_publication_keeps_its_typed_cause_after_rollback() {
    use std::sync::atomic::Ordering::SeqCst;
    let store = Arc::new(RefusingStore::default());
    let fx = fixture_with(store.clone(), Arc::new(InMemoryCoordinator::new())).await;
    let work = slot("tenant-a", "drive-refresh-save");
    login_expiring(&fx, &work).await;
    let before = fx.stored(&work).await.unwrap();
    *fx.state.refresh.lock() = Some(("access-a2".into(), None, None));
    fx.subjects
        .lock()
        .insert("access-a2".into(), "subject-a".into());
    store.saves.store(0, SeqCst);
    store.refuse_next_save.store(true, SeqCst);
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        fx.authority.bearer_token(&work),
    )
    .await
    .expect("the refresh finishes")
    .expect_err("a refused publication yields no bearer");
    assert_eq!(
        fx.state.refresh_requests.load(SeqCst),
        1,
        "the refresh reached HTTP"
    );
    assert_eq!(
        store.saves.load(SeqCst),
        2,
        "one refused publication and one rollback save"
    );
    let restored = fx.stored(&work).await.unwrap();
    assert_eq!(restored.primary_secret, before.primary_secret);
    assert_eq!(restored.refresh_token, before.refresh_token);
    assert_eq!(
        credential_access_cause(&error),
        Some((
            TokenStoreRefusal::InteractionRequired,
            "publication save needs credential interaction"
        )),
        "the completed rollback keeps the typed store cause: {error:?}"
    );
}

/// A refused reread after the refresh exchange closes the refresh, keeps
/// the stored credential and keeps the typed store cause.
#[tokio::test]
async fn a_refused_post_exchange_read_closes_the_refresh_with_its_typed_cause() {
    use std::sync::atomic::Ordering::SeqCst;
    let store = Arc::new(RefusingStore::default());
    let fx = fixture_with(store.clone(), Arc::new(InMemoryCoordinator::new())).await;
    let _ = store.issuer.set(fx.state.clone());
    let work = slot("tenant-a", "drive-post-exchange");
    login_expiring(&fx, &work).await;
    let before = fx.stored(&work).await.unwrap();
    *fx.state.refresh.lock() = Some(("access-a2".into(), None, None));
    fx.subjects
        .lock()
        .insert("access-a2".into(), "subject-a".into());
    store.refuse_reads_after_refresh.store(true, SeqCst);
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        fx.authority.bearer_token(&work),
    )
    .await
    .expect("the refresh finishes")
    .expect_err("a refused reread yields no bearer");
    store.refuse_reads_after_refresh.store(false, SeqCst);
    assert_eq!(
        fx.state.refresh_requests.load(SeqCst),
        1,
        "the refresh reached HTTP"
    );
    assert_eq!(
        credential_access_cause(&error),
        Some((
            TokenStoreRefusal::InteractionRequired,
            "post-exchange read needs credential interaction"
        )),
        "the closed refresh keeps the typed store cause: {error:?}"
    );
    assert_eq!(
        fx.stored(&work).await,
        Some(before),
        "nothing was published"
    );
}

/// A refused read while a fresh owner rehydrates the stored credential keeps
/// the typed store cause through the real admission.
#[tokio::test]
async fn a_refused_rehydration_read_keeps_its_typed_cause() {
    use std::sync::atomic::Ordering::SeqCst;
    let store = Arc::new(RefusingStore::default());
    let fx = fixture_with(store.clone(), Arc::new(InMemoryCoordinator::new())).await;
    let work = slot("tenant-a", "drive-rehydrate");
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    fx.login(&work, AccountSelection::Discover, "code-a")
        .await
        .unwrap();
    // A fresh process-local owner has no lifecycle: its admission reads the
    // stored credential, then rehydrates it with a second read.
    let flows = Arc::new(RuntimeOAuthFlowHandle::new_with_auth_lease(
        std::time::Duration::from_mins(5),
        Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new()),
    ));
    let cold = ConnectorOAuthAuthority::with_http(
        fx.persistence.clone(),
        flows,
        ConnectorStrategies::default().with(fx.strategy.clone()),
        reqwest::Client::new(),
    )
    .unwrap();
    store.loads.store(0, SeqCst);
    store.refuse_load.store(2, SeqCst);
    let error = tokio::time::timeout(std::time::Duration::from_secs(10), cold.bearer_token(&work))
        .await
        .expect("the admission finishes")
        .expect_err("a refused rehydration yields no bearer");
    assert_eq!(
        store.loads.load(SeqCst),
        2,
        "the admitted read, then the refused rehydration read"
    );
    assert_eq!(
        credential_access_cause(&error),
        Some((
            TokenStoreRefusal::InteractionRequired,
            "rehydration read needs credential interaction"
        )),
        "the rehydration keeps the typed store cause: {error:?}"
    );
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
    /// An in-memory store: status may read it.
    async fn observe_status(
        &self,
        key: &TokenKey,
    ) -> Result<
        meerkat_auth_core::auth_store::TokenStoreObservation,
        meerkat_auth_core::auth_store::TokenStoreError,
    > {
        self.0.observe_status(key).await
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
    across_a_replacement(
        replacement_subject,
        replacement_client,
        |authority, work| async move { authority.bearer_token(&work).await },
    )
    .await
}

/// Run `admit` on an expiring credential for subject-a, replacing the slot
/// (logout + discover) before the refresh reload runs.
async fn across_a_replacement<T, Fut>(
    replacement_subject: &str,
    replacement_client: &str,
    admit: impl FnOnce(ConnectorOAuthAuthority, CredentialAccountRef) -> Fut,
) -> (
    Result<T, ConnectorLoginError>,
    Fixture,
    CredentialAccountRef,
)
where
    Fut: std::future::Future<Output = Result<T, ConnectorLoginError>>,
{
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
                    host_attempt_handle: None,
                },
            )
            .await
            .unwrap();
    }));
    let result = admit(fx.authority.clone(), work.clone()).await;
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

// --- Client-auth profile (desktop-app clients) ------------------------------

const DESKTOP_SECRET: &str = "desktop-client-secret-canary";

fn desktop_profile(fx: &Fixture, id: &str) -> ConnectorClientAuthProfile {
    ConnectorClientAuthProfile {
        id: id.into(),
        issuer: fx.issuer.clone(),
        client: "connector-client".into(),
        client_secret: Some(DESKTOP_SECRET.into()),
        authorize_params: vec![
            ("access_type".into(), "offline".into()),
            ("prompt".into(), "consent".into()),
        ],
        resource_indicator: false,
        protocol: None,
        registered_redirect_uris: Vec::new(),
    }
}

fn with_profiles(fx: &mut Fixture, profiles: Vec<ConnectorClientAuthProfile>) {
    fx.authority = fx
        .authority
        .clone()
        .with_client_auth_profiles(profiles)
        .expect("valid profiles");
}

fn desktop_target(fx: &Fixture, slot: &CredentialAccountRef, id: &str) -> ConnectorOAuthTarget {
    ConnectorOAuthTarget {
        client_auth_profile: Some(id.into()),
        ..fx.target(slot, AccountSelection::Discover)
    }
}

#[tokio::test]
async fn without_a_profile_the_request_forms_are_unchanged() {
    let fx = fixture().await;
    let work = slot("tenant-a", "default-forms");
    let target = fx.target(&work, AccountSelection::Discover);
    let (_, authorize_url) = fx.start(&target, 41001).await;
    assert!(authorize_url.contains("resource="));
    assert!(!authorize_url.contains("access_type"));
    login_expiring(&fx, &work).await;
    *fx.state.refresh.lock() = Some(("access-a2".into(), None, None));
    fx.subjects
        .lock()
        .insert("access-a2".into(), "subject-a".into());
    fx.authority.bearer_token(&work).await.unwrap();
    let forms = fx.state.forms.lock().clone();
    assert_eq!(forms.len(), 2);
    for form in &forms {
        assert!(form.contains_key("resource"), "{form:?}");
        assert!(!form.contains_key("client_secret"), "{form:?}");
    }
    let metadata =
        ConnectorCredentialMetadata::from_tokens(&fx.stored(&work).await.unwrap()).unwrap();
    assert_eq!(metadata.client_auth_profile, None);
}

#[tokio::test]
async fn a_profile_drives_start_completion_and_refresh_consistently() {
    let mut fx = fixture().await;
    let profile = desktop_profile(&fx, "desktop");
    with_profiles(&mut fx, vec![profile.clone()]);
    assert!(!format!("{profile:?}").contains(DESKTOP_SECRET));
    let work = slot("tenant-a", "desktop-forms");
    let target = desktop_target(&fx, &work, "desktop");

    *fx.state.expires_in.lock() = 30;
    fx.grant("code-d", "access-d", "subject-d", Some("files.read"));
    let (state, authorize_url) = fx.start(&target, 41001).await;
    assert!(
        authorize_url.contains("access_type=offline"),
        "{authorize_url}"
    );
    assert!(authorize_url.contains("prompt=consent"), "{authorize_url}");
    assert!(!authorize_url.contains("resource="), "{authorize_url}");
    assert!(!authorize_url.contains(DESKTOP_SECRET));
    fx.complete(&work, &state, "code-d", 41001).await.unwrap();
    *fx.state.expires_in.lock() = 3600;

    let stored = fx.stored(&work).await.unwrap();
    assert!(
        !serde_json::to_string(&stored.metadata)
            .unwrap()
            .contains(DESKTOP_SECRET)
    );
    assert_eq!(
        ConnectorCredentialMetadata::from_tokens(&stored)
            .unwrap()
            .client_auth_profile,
        Some(profile.binding()),
        "the credential binds the profile's canonical meaning"
    );

    *fx.state.refresh.lock() = Some(("access-d2".into(), None, None));
    fx.subjects
        .lock()
        .insert("access-d2".into(), "subject-d".into());
    assert_eq!(
        fx.authority.bearer_token(&work).await.unwrap().as_deref(),
        Some("access-d2")
    );
    let forms = fx.state.forms.lock().clone();
    assert_eq!(forms.len(), 2, "one code exchange and one refresh");
    for form in &forms {
        assert_eq!(
            form.get("client_secret").map(String::as_str),
            Some(DESKTOP_SECRET)
        );
        assert!(!form.contains_key("resource"), "{form:?}");
    }
}

/// A host source whose profiles the test changes between uses.
struct ScriptedProfiles(Mutex<Vec<ConnectorClientAuthProfile>>);

#[async_trait]
impl ConnectorClientAuthSource for ScriptedProfiles {
    async fn current_profiles(
        &self,
    ) -> Result<Vec<ConnectorClientAuthProfile>, ConnectorLoginError> {
        Ok(self.0.lock().clone())
    }
}

#[tokio::test]
async fn source_profiles_are_read_at_each_use() {
    let mut fx = fixture().await;
    let source = Arc::new(ScriptedProfiles(Mutex::new(Vec::new())));
    fx.authority = fx
        .authority
        .clone()
        .with_client_auth_source(Arc::clone(&source) as Arc<dyn ConnectorClientAuthSource>);
    let work = slot("tenant-a", "sourced");
    let target = desktop_target(&fx, &work, "desktop");
    let refused = fx.authority.login_start(&target, &redirect(41001)).await;
    assert!(
        matches!(
            refused,
            Err(ConnectorLoginError::Verification(
                ConnectorOAuthRefusal::InvalidDescriptor
            ))
        ),
        "not configured yet: {refused:?}"
    );

    // Configured after the authority was built: start, completion and
    // refresh all use it.
    source.0.lock().push(desktop_profile(&fx, "desktop"));
    *fx.state.expires_in.lock() = 30;
    fx.grant("code-s", "access-s", "subject-s", Some("files.read"));
    let (state, authorize_url) = fx.start(&target, 41001).await;
    assert!(
        authorize_url.contains("access_type=offline"),
        "{authorize_url}"
    );
    fx.complete(&work, &state, "code-s", 41001).await.unwrap();
    *fx.state.expires_in.lock() = 3600;
    *fx.state.refresh.lock() = Some(("access-s2".into(), None, None));
    fx.subjects
        .lock()
        .insert("access-s2".into(), "subject-s".into());
    assert_eq!(
        fx.authority.bearer_token(&work).await.unwrap().as_deref(),
        Some("access-s2")
    );
    let forms = fx.state.forms.lock().clone();
    assert_eq!(forms.len(), 2, "one code exchange and one refresh");
    for form in &forms {
        assert_eq!(
            form.get("client_secret").map(String::as_str),
            Some(DESKTOP_SECRET)
        );
    }
}

fn stored_secret_profile(fx: &Fixture, realm: &RealmId) -> ConnectorClientAuthProfile {
    ConnectorClientAuthProfile {
        client_secret: Some(ConnectorClientSecret::RealmStore {
            realm: realm.clone(),
        }),
        ..desktop_profile(fx, "desktop")
    }
}

fn sent_secrets(fx: &Fixture) -> Vec<Option<String>> {
    fx.state
        .forms
        .lock()
        .iter()
        .map(|form| form.get("client_secret").cloned())
        .collect()
}

#[tokio::test]
async fn a_stored_client_secret_is_read_at_each_use_and_rotates() {
    let mut fx = fixture().await;
    let realm = RealmId::parse("tenant-a").unwrap();
    let profile = stored_secret_profile(&fx, &realm);
    let binding = profile.binding();
    with_profiles(&mut fx, vec![profile]);
    let work = slot("tenant-a", "stored-secret");
    let target = desktop_target(&fx, &work, "desktop");

    // The profile exists but its secret does not (for example a crash
    // between writing the configuration and the secret): typed refusal,
    // nothing sent.
    let refused = fx.authority.login_start(&target, &redirect(41001)).await;
    assert!(
        matches!(
            &refused,
            Err(ConnectorLoginError::ClientSecretUnavailable { profile }) if profile == "desktop"
        ),
        "{refused:?}"
    );
    assert!(fx.state.forms.lock().is_empty());

    save_connector_client_secret(fx.store.as_ref(), &realm, &binding, "secret-one")
        .await
        .unwrap();
    *fx.state.expires_in.lock() = 30;
    fx.grant("code-r", "access-r1", "subject-r", Some("files.read"));
    let (state, _) = fx.start(&target, 41001).await;
    fx.complete(&work, &state, "code-r", 41001).await.unwrap();
    let stored = fx.stored(&work).await.unwrap();
    assert!(
        !serde_json::to_string(&stored)
            .unwrap()
            .contains("secret-one")
    );

    // Rotated in the store: the next refresh sends the new secret, and the
    // credential minted under the old one stays valid.
    save_connector_client_secret(fx.store.as_ref(), &realm, &binding, "secret-two")
        .await
        .unwrap();
    *fx.state.refresh.lock() = Some(("access-r2".into(), None, None));
    fx.subjects
        .lock()
        .insert("access-r2".into(), "subject-r".into());
    assert_eq!(
        fx.authority.bearer_token(&work).await.unwrap().as_deref(),
        Some("access-r2")
    );
    assert_eq!(
        sent_secrets(&fx),
        vec![Some("secret-one".to_owned()), Some("secret-two".to_owned())]
    );
}

#[tokio::test]
async fn a_refresh_without_the_stored_secret_sends_nothing_and_keeps_the_credential() {
    let mut fx = fixture().await;
    let realm = RealmId::parse("tenant-a").unwrap();
    let profile = stored_secret_profile(&fx, &realm);
    let binding = profile.binding();
    with_profiles(&mut fx, vec![profile]);
    let work = slot("tenant-a", "secret-cleared");
    let target = desktop_target(&fx, &work, "desktop");
    save_connector_client_secret(fx.store.as_ref(), &realm, &binding, "secret-one")
        .await
        .unwrap();
    *fx.state.expires_in.lock() = 30;
    fx.grant("code-c", "access-c1", "subject-c", Some("files.read"));
    let (state, _) = fx.start(&target, 41001).await;
    fx.complete(&work, &state, "code-c", 41001).await.unwrap();
    let before = fx.stored(&work).await.unwrap();

    clear_connector_client_secret(fx.store.as_ref(), &realm, &binding)
        .await
        .unwrap();
    // No refreshed token: either a refusal or the still-unexpired one.
    let result = fx.authority.bearer_token(&work).await;
    assert!(
        result
            .as_ref()
            .map_or(true, |token| token.as_deref() == Some("access-c1")),
        "{result:?}"
    );
    assert_eq!(
        sent_secrets(&fx).len(),
        1,
        "only the code exchange was sent"
    );
    assert_eq!(
        fx.stored(&work).await,
        Some(before),
        "the credential is kept"
    );
}

fn refused_for_missing_secret(
    result: &Result<meerkat_auth_core::connector_login::ConnectorLoginStart, ConnectorLoginError>,
) -> bool {
    matches!(
        result,
        Err(ConnectorLoginError::ClientSecretUnavailable { profile }) if profile == "desktop"
    )
}

#[tokio::test]
async fn a_retargeted_client_never_reads_the_previous_clients_secret() {
    let mut fx = fixture().await;
    let realm = RealmId::parse("tenant-a").unwrap();
    let original = stored_secret_profile(&fx, &realm);
    save_connector_client_secret(fx.store.as_ref(), &realm, &original.binding(), "secret-a")
        .await
        .unwrap();
    // Same id, another client: its meaning (and so its key) differs.
    let retargeted = ConnectorClientAuthProfile {
        client: "replacement-client".into(),
        ..original.clone()
    };
    assert_ne!(original.binding(), retargeted.binding());
    with_profiles(&mut fx, vec![retargeted]);
    let work = slot("tenant-a", "retargeted");
    let target = ConnectorOAuthTarget {
        client: "replacement-client".into(),
        ..desktop_target(&fx, &work, "desktop")
    };
    let refused = fx.authority.login_start(&target, &redirect(41001)).await;
    assert!(refused_for_missing_secret(&refused), "{refused:?}");
    assert!(fx.state.forms.lock().is_empty());
}

#[tokio::test]
async fn an_inherited_or_shadowed_client_reads_only_its_defining_realms_secret() {
    let mut fx = fixture().await;
    let parent = RealmId::parse("tenant-parent").unwrap();
    let child = RealmId::parse("tenant-child").unwrap();
    let parent_profile = stored_secret_profile(&fx, &parent);
    save_connector_client_secret(
        fx.store.as_ref(),
        &parent,
        &parent_profile.binding(),
        "parent-secret",
    )
    .await
    .unwrap();
    // A child-defined client with the same id and meaning has its own slot:
    // the parent's secret never stands in for it.
    let child_profile = stored_secret_profile(&fx, &child);
    with_profiles(&mut fx, vec![child_profile]);
    let work = slot("tenant-a", "shadowed");
    let refused = fx
        .authority
        .login_start(&desktop_target(&fx, &work, "desktop"), &redirect(41001))
        .await;
    assert!(refused_for_missing_secret(&refused), "{refused:?}");
}

#[tokio::test]
async fn a_row_of_another_mode_or_binding_under_the_secret_key_is_not_a_secret() {
    let mut fx = fixture().await;
    let realm = RealmId::parse("tenant-a").unwrap();
    let profile = stored_secret_profile(&fx, &realm);
    let key = TokenKey::connector_client_secret(realm.clone(), &profile.binding()).unwrap();
    with_profiles(&mut fx, vec![profile.clone()]);
    let work = slot("tenant-a", "wrong-mode");
    let target = desktop_target(&fx, &work, "desktop");
    // A provider credential row.
    fx.store
        .save(&key, &PersistedTokens::api_key("not-a-client-secret"))
        .await
        .unwrap();
    let refused = fx.authority.login_start(&target, &redirect(41001)).await;
    assert!(refused_for_missing_secret(&refused), "{refused:?}");
    // A client-secret row stored for another meaning.
    let mut foreign = PersistedTokens::api_key("foreign-secret");
    foreign.auth_mode = PersistedAuthMode::ConnectorClientSecret;
    foreign.metadata = json!({ "connector_client_binding": "desktop@other" });
    fx.store.save(&key, &foreign).await.unwrap();
    let refused = fx.authority.login_start(&target, &redirect(41001)).await;
    assert!(refused_for_missing_secret(&refused), "{refused:?}");
}

#[tokio::test]
async fn a_source_profile_reusing_an_installed_id_is_a_configuration_error() {
    let mut fx = fixture().await;
    let profile = desktop_profile(&fx, "desktop");
    with_profiles(&mut fx, vec![profile.clone()]);
    let source = Arc::new(ScriptedProfiles(Mutex::new(vec![profile])));
    fx.authority = fx.authority.clone().with_client_auth_source(source);
    let work = slot("tenant-a", "sourced-twice");
    let refused = fx
        .authority
        .login_start(&desktop_target(&fx, &work, "desktop"), &redirect(41001))
        .await;
    assert!(
        matches!(refused, Err(ConnectorLoginError::ClientConfig(_))),
        "{refused:?}"
    );
    assert!(fx.state.forms.lock().is_empty());
}

#[tokio::test]
async fn a_profile_for_another_issuer_or_client_or_an_unknown_id_refuses_the_start() {
    let mut fx = fixture().await;
    let mut other_client = desktop_profile(&fx, "other-client");
    other_client.client = "another-client".into();
    let mut other_issuer = desktop_profile(&fx, "other-issuer");
    other_issuer.issuer = "https://issuer.elsewhere.example".into();
    with_profiles(&mut fx, vec![other_client, other_issuer]);
    let work = slot("tenant-a", "desktop-refused");
    for id in ["other-client", "other-issuer", "not-installed"] {
        let refused = fx
            .authority
            .login_start(&desktop_target(&fx, &work, id), &redirect(41001))
            .await;
        assert!(
            matches!(
                refused,
                Err(ConnectorLoginError::Verification(
                    ConnectorOAuthRefusal::InvalidDescriptor
                ))
            ),
            "{id}: {refused:?}"
        );
    }
    assert!(fx.state.forms.lock().is_empty());
}

#[tokio::test]
async fn a_completion_naming_another_profile_is_refused() {
    let mut fx = fixture().await;
    let second = ConnectorClientAuthProfile {
        id: "desktop-b".into(),
        ..desktop_profile(&fx, "desktop-b")
    };
    let first = desktop_profile(&fx, "desktop");
    with_profiles(&mut fx, vec![first, second]);
    let work = slot("tenant-a", "desktop-crossed");
    fx.grant("code-x", "access-x", "subject-x", Some("files.read"));
    let (state, _) = fx
        .start(&desktop_target(&fx, &work, "desktop"), 41001)
        .await;
    let crossed = fx
        .authority
        .login_complete_for_target(
            &desktop_target(&fx, &work, "desktop-b"),
            ConnectorOAuthCallback {
                redirect_uri: redirect(41001),
                state,
                code: "code-x".into(),
                host_attempt_handle: None,
            },
        )
        .await;
    assert!(
        matches!(
            crossed,
            Err(ConnectorLoginError::Verification(
                ConnectorOAuthRefusal::DescriptorMismatch
            ))
        ),
        "{crossed:?}"
    );
    assert!(fx.state.forms.lock().is_empty(), "no code exchange");
}

#[tokio::test]
async fn a_refresh_whose_profile_is_gone_fails_locally_and_never_sends_without_it() {
    let mut fx = fixture().await;
    let profile = desktop_profile(&fx, "desktop");
    with_profiles(&mut fx, vec![profile]);
    let work = slot("tenant-a", "desktop-uninstalled");
    *fx.state.expires_in.lock() = 30;
    fx.grant("code-u", "access-u", "subject-u", Some("files.read"));
    let (state, _) = fx
        .start(&desktop_target(&fx, &work, "desktop"), 41001)
        .await;
    fx.complete(&work, &state, "code-u", 41001).await.unwrap();
    *fx.state.expires_in.lock() = 3600;
    let before = fx.stored(&work).await.unwrap();

    // The host no longer installs the profile (or installs another one).
    let mut swapped = desktop_profile(&fx, "desktop-other");
    swapped.id = "desktop-other".into();
    with_profiles(&mut fx, vec![swapped]);
    let refused = fx.authority.bearer_token(&work).await;
    assert!(
        matches!(refused, Err(ConnectorLoginError::AuthLifecycle(_))),
        "{refused:?}"
    );
    assert_eq!(
        fx.state.forms.lock().len(),
        1,
        "only the code exchange was sent"
    );
    assert_eq!(fx.stored(&work).await, Some(before));
}

#[test]
fn a_profile_setting_a_login_owned_parameter_is_refused_at_install() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let fx = runtime.block_on(fixture());
    for reserved in [
        "state",
        "redirect_uri",
        "Scope",
        "resource",
        "code_challenge",
    ] {
        let mut profile = desktop_profile(&fx, "bad");
        profile.authorize_params.push((reserved.into(), "x".into()));
        assert!(
            fx.authority
                .clone()
                .with_client_auth_profiles(vec![profile])
                .is_err(),
            "{reserved}"
        );
    }
    // Names are single tokens: an encoded or composite name is refused.
    for smuggled in [
        "x=1&client_id",
        "cl%69ent_id",
        "Client_Secret",
        "a b",
        "code_verifier",
    ] {
        let mut profile = desktop_profile(&fx, "bad");
        profile.authorize_params.push((smuggled.into(), "x".into()));
        assert!(
            fx.authority
                .clone()
                .with_client_auth_profiles(vec![profile])
                .is_err(),
            "{smuggled}"
        );
    }
    for bad_id in ["", "desk@top", "a&b"] {
        assert!(
            fx.authority
                .clone()
                .with_client_auth_profiles(vec![desktop_profile(&fx, bad_id)])
                .is_err(),
            "{bad_id:?}"
        );
    }
    let duplicate = vec![desktop_profile(&fx, "dup"), desktop_profile(&fx, "dup")];
    assert!(fx.authority.with_client_auth_profiles(duplicate).is_err());
}

#[test]
fn stored_metadata_without_a_profile_field_still_loads() {
    let metadata: ConnectorCredentialMetadata = serde_json::from_value(json!({
        "issuer": "https://issuer.example",
        "client": "connector-client",
        "resource": "https://api.service.example",
        "strategy_id": STRATEGY,
        "requested_scopes": ["files.read"],
        "token_endpoint": "https://issuer.example/token",
        "stable_context": "connector-context:00",
        "scope_evidence": { "kind": "token_endpoint_response" },
        "granted_at_epoch_secs": 1,
    }))
    .expect("metadata stored before client-auth profiles");
    assert_eq!(metadata.client_auth_profile, None);
}

impl Fixture {
    /// What a call expects of `work`'s credential, as logged in by the fixture.
    fn expectation(&self, work: &CredentialAccountRef, account: &str) -> ConnectorCallExpectation {
        ConnectorCallExpectation {
            slot: work.clone(),
            account: account.to_owned(),
            issuer: self.issuer.clone(),
            client: "connector-client".into(),
            resource: "https://api.service.example".into(),
            strategy_id: STRATEGY.into(),
            required_scopes: ["files.read".to_owned()].into(),
        }
    }
}

fn admitted_token(result: Result<ConnectorCallEntry, ConnectorLoginError>) -> String {
    result
        .expect("the call credential is admitted")
        .credential()
        .expose_access_token()
        .to_owned()
}

#[tokio::test]
async fn a_call_credential_binds_the_expected_account_context_and_scopes() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    let expect = fx.expectation(&work, "subject-a");
    assert!(matches!(
        fx.authority.admit_call_credential(&expect).await,
        Err(ConnectorLoginError::ReauthRequired)
    ));
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    fx.login(&work, AccountSelection::Discover, "code-a")
        .await
        .unwrap();

    let entry = fx.authority.admit_call_credential(&expect).await.unwrap();
    let credential = entry.credential();
    assert_eq!(credential.expose_access_token(), "access-a");
    assert_eq!(credential.account(), "subject-a");
    assert_eq!(credential.scopes(), ["files.read".to_owned()]);
    assert!(
        !format!("{entry:?}").contains("access-a"),
        "the entry's Debug shows the token"
    );
    entry.recheck().unwrap();
    drop(entry);

    let mut other = expect.clone();
    other.account = "subject-b".into();
    assert!(matches!(
        fx.authority.admit_call_credential(&other).await,
        Err(ConnectorLoginError::Verification(
            ConnectorOAuthRefusal::AccountMismatch
        ))
    ));
    let mut other = expect.clone();
    other.client = "another-client".into();
    assert_eq!(
        slot_refusal(fx.authority.admit_call_credential(&other).await),
        CredentialSlotRefusal::ContextMismatch
    );
    let mut other = expect;
    other.required_scopes.insert("files.write".into());
    assert!(matches!(
        fx.authority.admit_call_credential(&other).await,
        Err(ConnectorLoginError::Verification(
            ConnectorOAuthRefusal::MissingScopes
        ))
    ));
}

/// While a call holds its admitted credential, a rotation (a compatible
/// reconnect) or a logout of the slot waits: the final recheck observes the
/// admitted credential's marker, freshness and use admission coherently, and
/// cannot classify a replacement. Once the entry is dropped the rotation
/// lands and a later call admits the current credential. (Fails-old: the
/// recheck read the marker and then the slot's freshness and use separately,
/// with a replacement able to land in between.)
#[tokio::test]
async fn a_held_call_credential_holds_off_rotation_and_logout_until_dropped() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    let expect = fx.expectation(&work, "subject-a");
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    fx.login(&work, AccountSelection::Discover, "code-a")
        .await
        .unwrap();
    fx.grant("code-a2", "access-a2", "subject-a", Some("files.read"));
    let target = fx.target(&work, AccountSelection::Known("subject-a".into()));
    let (state, _) = fx.start(&target, 41003).await;
    let before = fx.authority.admit_call_credential(&expect).await.unwrap();
    let rotation = {
        let authority = fx.authority.clone();
        let work = work.clone();
        tokio::spawn(async move {
            authority
                .login_complete(
                    &work,
                    ConnectorOAuthCallback {
                        redirect_uri: redirect(41003),
                        state,
                        code: "code-a2".into(),
                        host_attempt_handle: None,
                    },
                )
                .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        !rotation.is_finished(),
        "the rotation waited for the held call"
    );
    before.recheck().unwrap();
    assert_eq!(before.credential().expose_access_token(), "access-a");
    drop(before);
    rotation.await.unwrap().unwrap();

    let rotated = fx.authority.admit_call_credential(&expect).await.unwrap();
    assert_eq!(rotated.credential().expose_access_token(), "access-a2");
    let logout = {
        let authority = fx.authority.clone();
        let work = work.clone();
        tokio::spawn(async move { authority.logout(&work).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!logout.is_finished(), "the logout waited for the held call");
    rotated.recheck().unwrap();
    drop(rotated);
    logout.await.unwrap().unwrap();
    assert!(matches!(
        fx.authority.admit_call_credential(&expect).await,
        Err(ConnectorLoginError::ReauthRequired)
    ));
}

/// An expiring credential is refreshed for the call, and the call gets the
/// exact refreshed credential, still current at its recheck.
#[tokio::test]
async fn a_call_credential_is_the_refreshed_one_when_it_expires() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    login_expiring(&fx, &work).await;
    *fx.state.refresh.lock() = Some(("access-a2".into(), None, None));
    fx.subjects
        .lock()
        .insert("access-a2".into(), "subject-a".into());
    let expect = fx.expectation(&work, "subject-a");

    let entry = fx.authority.admit_call_credential(&expect).await.unwrap();

    assert_eq!(entry.credential().expose_access_token(), "access-a2");
    entry.recheck().unwrap();
    drop(entry);
    assert_eq!(
        admitted_token(fx.authority.admit_call_credential(&expect).await),
        "access-a2"
    );
    assert_eq!(
        fx.state
            .refresh_requests
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the second call reused the refreshed credential"
    );
}

/// A grant lost during the call's refresh (a permanent `invalid_grant`)
/// refuses the call instead of handing out the expiring credential.
#[tokio::test]
async fn a_call_credential_is_refused_when_its_grant_is_lost_during_refresh() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    login_expiring(&fx, &work).await;
    fx.state
        .refresh_rejected
        .store(true, std::sync::atomic::Ordering::SeqCst);

    let result = fx
        .authority
        .admit_call_credential(&fx.expectation(&work, "subject-a"))
        .await;

    assert!(
        matches!(result, Err(ConnectorLoginError::ReauthRequired)),
        "{result:?}"
    );
}

/// A slot replaced while the call's refresh waits: the call gets the
/// replacement only when it still has the expected account and context.
#[tokio::test]
async fn a_call_credential_holds_to_its_expectation_across_a_slot_replacement() {
    let call = |authority: ConnectorOAuthAuthority, work: CredentialAccountRef| async move {
        let issuer = authority_issuer(&authority, &work).await;
        authority
            .admit_call_credential(&ConnectorCallExpectation {
                slot: work.clone(),
                account: "subject-a".into(),
                issuer,
                client: "connector-client".into(),
                resource: "https://api.service.example".into(),
                strategy_id: STRATEGY.into(),
                required_scopes: ["files.read".to_owned()].into(),
            })
            .await
    };
    let (result, _, _) = across_a_replacement("subject-a", "connector-client", call).await;
    assert_eq!(admitted_token(result), "access-r");

    let (result, _, _) = across_a_replacement("subject-a", "another-client", call).await;
    assert_eq!(slot_refusal(result), CredentialSlotRefusal::ContextMismatch);

    let (result, _, _) = across_a_replacement("subject-b", "connector-client", call).await;
    assert!(matches!(
        result,
        Err(ConnectorLoginError::Verification(
            ConnectorOAuthRefusal::AccountMismatch
        ))
    ));
}

/// The issuer `work`'s credential was granted under, read before the call.
async fn authority_issuer(
    authority: &ConnectorOAuthAuthority,
    work: &CredentialAccountRef,
) -> String {
    authority
        .status(work)
        .await
        .unwrap()
        .verified_account
        .expect("logged in")
        .issuer
}

/// Freshness is observed again at the recheck: a credential admitted just
/// outside its refresh window and rechecked after it opened is refused, and
/// the next call refreshes it.
#[tokio::test]
async fn a_call_credential_entering_its_refresh_window_is_refused_at_the_recheck() {
    let fx = fixture().await;
    let work = slot("tenant-a", "drive-work");
    *fx.state.expires_in.lock() = 63;
    fx.grant("code-a", "access-a", "subject-a", Some("files.read"));
    fx.login(&work, AccountSelection::Discover, "code-a")
        .await
        .unwrap();
    let expect = fx.expectation(&work, "subject-a");
    let entry = fx.authority.admit_call_credential(&expect).await.unwrap();
    assert_eq!(entry.credential().expose_access_token(), "access-a");

    tokio::time::sleep(std::time::Duration::from_secs(4)).await;

    assert!(matches!(
        entry.recheck(),
        Err(ConnectorLoginError::ReauthRequired)
    ));
    drop(entry);
    *fx.state.refresh.lock() = Some(("access-a2".into(), None, None));
    fx.subjects
        .lock()
        .insert("access-a2".into(), "subject-a".into());
    assert_eq!(
        admitted_token(fx.authority.admit_call_credential(&expect).await),
        "access-a2"
    );
}

/// A changed meaning under the same id (here the resource choice) neither
/// completes the admitted login nor refreshes the credential; rotating only
/// the secret value keeps working and sends the new secret.
#[tokio::test]
async fn a_changed_profile_meaning_under_the_same_id_is_refused_but_secret_rotation_is_not() {
    let mut fx = fixture().await;
    let profile = desktop_profile(&fx, "desktop");
    with_profiles(&mut fx, vec![profile.clone()]);
    let work = slot("tenant-a", "desktop-meaning");
    *fx.state.expires_in.lock() = 30;
    fx.grant("code-m", "access-m", "subject-m", Some("files.read"));
    let (state, _) = fx
        .start(&desktop_target(&fx, &work, "desktop"), 41001)
        .await;

    // Same id, different meaning: the admitted completion is refused before
    // any exchange.
    let mut changed = profile.clone();
    changed.resource_indicator = true;
    with_profiles(&mut fx, vec![changed]);
    let refused = fx.complete(&work, &state, "code-m", 41001).await;
    assert!(
        matches!(
            refused,
            Err(ConnectorLoginError::Verification(
                ConnectorOAuthRefusal::InvalidDescriptor
            ))
        ),
        "{refused:?}"
    );
    assert!(fx.state.forms.lock().is_empty());

    // The original meaning completes a fresh login.
    with_profiles(&mut fx, vec![profile.clone()]);
    let (state, _) = fx
        .start(&desktop_target(&fx, &work, "desktop"), 41001)
        .await;
    fx.complete(&work, &state, "code-m", 41001).await.unwrap();
    *fx.state.expires_in.lock() = 3600;

    // A changed meaning fails the refresh locally, twice (the lifecycle is
    // closed, not left refreshing), and nothing is sent.
    let mut extras = profile.clone();
    extras
        .authorize_params
        .push(("include_granted_scopes".into(), "true".into()));
    with_profiles(&mut fx, vec![extras]);
    for _ in 0..2 {
        let refused = fx.authority.bearer_token(&work).await;
        assert!(
            matches!(refused, Err(ConnectorLoginError::AuthLifecycle(_))),
            "{refused:?}"
        );
    }
    assert_eq!(fx.state.forms.lock().len(), 1, "only the code exchange");

    // Rotating only the secret value keeps the meaning: the refresh runs
    // and sends the new secret.
    let mut rotated = profile.clone();
    rotated.client_secret = Some("rotated-secret".into());
    assert_eq!(rotated.binding(), profile.binding());
    with_profiles(&mut fx, vec![rotated]);
    *fx.state.refresh.lock() = Some(("access-m2".into(), None, None));
    fx.subjects
        .lock()
        .insert("access-m2".into(), "subject-m".into());
    assert_eq!(
        fx.authority.bearer_token(&work).await.unwrap().as_deref(),
        Some("access-m2")
    );
    let forms = fx.state.forms.lock().clone();
    assert_eq!(
        forms
            .last()
            .and_then(|form| form.get("client_secret"))
            .map(String::as_str),
        Some("rotated-secret")
    );
}

// --- Remote (https) callback under host custody ---------------------------

/// A redirect registered for a web-application client, as host config.
/// Tested with a loopback-shaped https host: no listener is involved, the
/// host hands the callback to the owner.
const REMOTE_REDIRECT: &str = "https://127.0.0.1:8443/connector/oauth/callback";

fn remote_profile(fx: &Fixture, id: &str) -> ConnectorClientAuthProfile {
    ConnectorClientAuthProfile {
        registered_redirect_uris: vec![REMOTE_REDIRECT.into()],
        ..desktop_profile(fx, id)
    }
}

fn remote_callback(state: &str, code: &str, handle: Option<String>) -> ConnectorOAuthCallback {
    ConnectorOAuthCallback {
        redirect_uri: REMOTE_REDIRECT.into(),
        state: state.into(),
        code: code.into(),
        host_attempt_handle: handle,
    }
}

/// An exactly registered https redirect is admitted, and its completion
/// needs the host's retained attempt handle: none or a forged one is
/// refused without consuming the attempt, the real one completes, and a
/// replay is refused.
#[tokio::test]
async fn a_registered_https_redirect_completes_only_from_the_hosts_custody() {
    let mut fx = fixture().await;
    let profile = remote_profile(&fx, "web");
    with_profiles(&mut fx, vec![profile]);
    let work = slot("tenant-a", "remote-web");
    fx.grant("code-r", "access-r", "subject-r", Some("files.read"));
    let start = fx
        .authority
        .login_start(&desktop_target(&fx, &work, "web"), REMOTE_REDIRECT)
        .await
        .expect("a registered https redirect is admitted");
    let handle = start
        .host_attempt_handle
        .clone()
        .expect("a remote attempt has a host handle");
    assert!(
        !start.authorize_url.contains(&handle),
        "the handle never reaches the browser"
    );

    for missing in [None, Some("forged".to_owned())] {
        let refused = fx
            .authority
            .login_complete(&work, remote_callback(&start.state, "code-r", missing))
            .await;
        assert!(
            matches!(refused, Err(ConnectorLoginError::RemoteCustodyRequired)),
            "{refused:?}"
        );
    }
    // A handle for another slot does not complete this one.
    let other = slot("tenant-a", "remote-other");
    let foreign = fx
        .authority
        .login_complete(
            &other,
            remote_callback(&start.state, "code-r", Some(handle.clone())),
        )
        .await;
    assert!(foreign.is_err());

    let done = fx
        .authority
        .login_complete(
            &work,
            remote_callback(&start.state, "code-r", Some(handle.clone())),
        )
        .await
        .expect("the retained custody completes the attempt");
    assert_eq!(done.verified_account.subject, "subject-r");
    let replay = fx
        .authority
        .login_complete(&work, remote_callback(&start.state, "code-r", Some(handle)))
        .await;
    assert!(replay.is_err(), "a consumed attempt cannot be replayed");
}

/// An https redirect that is not exactly registered (another host, path,
/// port, scheme or query), or one without a profile, is refused before any
/// discovery; a loopback attempt is unchanged and has no handle.
#[tokio::test]
async fn unregistered_https_redirects_are_refused_and_loopback_is_unchanged() {
    let mut fx = fixture().await;
    let profile = remote_profile(&fx, "web");
    with_profiles(&mut fx, vec![profile]);
    let work = slot("tenant-a", "remote-refused");
    for redirect in [
        "https://127.0.0.2:8443/connector/oauth/callback",
        "https://127.0.0.1:8443/connector/oauth/other",
        "https://127.0.0.1:9443/connector/oauth/callback",
        "https://127.0.0.1:8443/connector/oauth/callback?x=1",
        "http://app.example/connector/oauth/callback",
    ] {
        let refused = fx
            .authority
            .login_start(&desktop_target(&fx, &work, "web"), redirect)
            .await;
        assert!(
            matches!(refused, Err(ConnectorLoginError::InvalidRedirect)),
            "{redirect}: {refused:?}"
        );
    }
    let no_profile = fx
        .authority
        .login_start(
            &fx.target(&work, AccountSelection::Discover),
            REMOTE_REDIRECT,
        )
        .await;
    assert!(matches!(
        no_profile,
        Err(ConnectorLoginError::InvalidRedirect)
    ));

    let start = fx
        .authority
        .login_start(
            &fx.target(&work, AccountSelection::Discover),
            &redirect(41001),
        )
        .await
        .expect("loopback as before");
    assert_eq!(start.host_attempt_handle, None);
}

/// A registered redirect that is not an absolute https URL is refused at
/// install, and registered redirects are part of the profile's meaning.
#[tokio::test]
async fn registered_redirects_are_validated_and_bound() {
    let fx = fixture().await;
    for bad in [
        "http://127.0.0.1:8443/connector/oauth/callback",
        "https://127.0.0.1:8443/connector/oauth/callback#frag",
        "https://user@127.0.0.1:8443/connector/oauth/callback",
        "not a url",
    ] {
        let profile = ConnectorClientAuthProfile {
            registered_redirect_uris: vec![bad.into()],
            ..desktop_profile(&fx, "web")
        };
        assert!(
            fx.authority
                .clone()
                .with_client_auth_profiles(vec![profile])
                .is_err(),
            "{bad}"
        );
    }
    assert_ne!(
        remote_profile(&fx, "web").binding(),
        desktop_profile(&fx, "web").binding()
    );
}

/// A profile whose registered redirect was replaced by an authorize
/// parameter carrying the same text has another meaning, so an attempt
/// admitted under the registration does not complete after that
/// replacement, even with its original handle.
#[tokio::test]
async fn a_registration_replaced_by_a_lookalike_parameter_refuses_the_completion() {
    let mut fx = fixture().await;
    let registered = remote_profile(&fx, "web");
    let mut lookalike = desktop_profile(&fx, "web");
    lookalike
        .authorize_params
        .push(("redirect".into(), REMOTE_REDIRECT.into()));
    assert_ne!(registered.binding(), lookalike.binding());

    with_profiles(&mut fx, vec![registered]);
    let work = slot("tenant-a", "remote-replaced");
    fx.grant("code-x", "access-x", "subject-x", Some("files.read"));
    let start = fx
        .authority
        .login_start(&desktop_target(&fx, &work, "web"), REMOTE_REDIRECT)
        .await
        .expect("a registered https redirect is admitted");
    let handle = start.host_attempt_handle.clone().expect("remote handle");

    with_profiles(&mut fx, vec![lookalike]);
    let forms_before = fx.state.forms.lock().len();
    let refused = fx
        .authority
        .login_complete(&work, remote_callback(&start.state, "code-x", Some(handle)))
        .await;
    assert!(
        matches!(refused, Err(ConnectorLoginError::Verification(_))),
        "{refused:?}"
    );
    assert_eq!(
        fx.state.forms.lock().len(),
        forms_before,
        "refused before the code exchange"
    );
    let status = fx.authority.status(&work).await.expect("status");
    assert_eq!(status.phase, ConnectorAuthPhase::AuthorizationRequired);
}

// --- A credential commit whose outcome is unknown ----------------------------

/// Fails every save and every clear: a first login's commit fails to save,
/// and its rollback then fails to clear.
struct FailSaveAndRollbackStore(EphemeralTokenStore);

#[async_trait]
impl TokenStore for FailSaveAndRollbackStore {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        self.0.load(key).await
    }
    async fn save(
        &self,
        _key: &TokenKey,
        _tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        Err(TokenStoreError::Io("injected save failure".into()))
    }
    async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
        Err(TokenStoreError::Io("injected clear failure".into()))
    }
    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        self.0.list().await
    }
    fn backend_name(&self) -> &'static str {
        "fail-save-and-rollback"
    }
}

/// Runs a credential mutation (or never starts it), then answers that the
/// mutation's owner was lost: what a caller sees when the coordinator's work
/// task ends before it reports.
struct OwnerLostCoordinator {
    inner: InMemoryCoordinator,
    run_mutation: bool,
}

#[async_trait]
impl RefreshCoordinator for OwnerLostCoordinator {
    async fn with_exclusive_mutation(
        &self,
        _key: TokenKey,
        mutation_fn: meerkat_auth_core::auth_store::CredentialMutationFn,
    ) -> Result<meerkat_auth_core::auth_store::CredentialMutationOutcome, CredentialMutationError>
    {
        if self.run_mutation {
            let _ = mutation_fn().await;
        }
        Err(CredentialMutationError::Cancelled)
    }

    async fn with_refresh(
        &self,
        key: TokenKey,
        refresh_fn: meerkat_auth_core::auth_store::RefreshFn,
    ) -> Result<PersistedTokens, meerkat_auth_core::auth_store::RefreshError> {
        self.inner.with_refresh(key, refresh_fn).await
    }
}

/// A connector commit that fails after the consume and cannot restore the
/// previous state leaves what is stored unknown: CommitUnsettled, never a
/// clean lifecycle failure.
#[tokio::test]
async fn connector_commit_whose_rollback_fails_is_unsettled() {
    let fx = fixture_with(
        Arc::new(FailSaveAndRollbackStore(EphemeralTokenStore::new())),
        Arc::new(InMemoryCoordinator::new()),
    )
    .await;
    let work = slot("tenant-a", "rollback-fails");
    fx.grant("code-r", "access-r", "subject-r", Some("files.read"));
    let error = fx
        .login(&work, AccountSelection::Discover, "code-r")
        .await
        .expect_err("a commit whose save and rollback fail never succeeds");
    assert!(
        matches!(error, ConnectorLoginError::CommitUnsettled),
        "a failed rollback leaves the outcome unknown: {error:?}"
    );
    assert!(!error.is_refusal());
    assert_eq!(fx.stored(&work).await, None);
}

/// The commit's owner ended after it consumed the attempt (here, after it
/// saved): whether the credential landed is unknown to the caller, so the
/// answer is CommitUnsettled. The consumed attempt never completes again.
#[tokio::test]
async fn connector_commit_whose_owner_is_lost_after_the_consume_is_unsettled() {
    let fx = fixture_with(
        Arc::new(EphemeralTokenStore::new()),
        Arc::new(OwnerLostCoordinator {
            inner: InMemoryCoordinator::new(),
            run_mutation: true,
        }),
    )
    .await;
    let work = slot("tenant-a", "owner-lost-after");
    fx.grant("code-l", "access-l", "subject-l", Some("files.read"));
    let target = fx.target(&work, AccountSelection::Discover);
    let (state, _) = fx.start(&target, 41001).await;
    let error = fx
        .complete(&work, &state, "code-l", 41001)
        .await
        .expect_err("a lost commit owner is never a success");
    assert!(
        matches!(error, ConnectorLoginError::CommitUnsettled),
        "a consumed attempt whose owner was lost is unsettled: {error:?}"
    );
    assert_eq!(
        fx.stored(&work)
            .await
            .and_then(|tokens| tokens.primary_secret),
        Some("access-l".to_owned()),
        "here the commit did land: only a later read can tell"
    );
    assert!(
        fx.complete(&work, &state, "code-l", 41001).await.is_err(),
        "the consumed attempt never completes again"
    );
}

/// The commit's owner ended before it consumed the attempt: the flow owner
/// still retires the attempt, so nothing is or can be published, and the
/// answer is the typed pending refusal.
#[tokio::test]
async fn connector_commit_whose_owner_is_lost_before_the_consume_publishes_nothing() {
    let fx = fixture_with(
        Arc::new(EphemeralTokenStore::new()),
        Arc::new(OwnerLostCoordinator {
            inner: InMemoryCoordinator::new(),
            run_mutation: false,
        }),
    )
    .await;
    let work = slot("tenant-a", "owner-lost-before");
    fx.grant("code-b", "access-b", "subject-b", Some("files.read"));
    let target = fx.target(&work, AccountSelection::Discover);
    let (state, _) = fx.start(&target, 41001).await;
    let error = fx
        .complete(&work, &state, "code-b", 41001)
        .await
        .expect_err("a lost commit owner is never a success");
    assert!(
        matches!(
            error,
            ConnectorLoginError::CredentialAccess {
                refusal: TokenStoreRefusal::Pending,
                ..
            }
        ),
        "an attempt retired before its consume is a pending refusal: {error:?}"
    );
    assert_eq!(fx.stored(&work).await, None, "nothing was published");
    assert!(
        fx.complete(&work, &state, "code-b", 41001).await.is_err(),
        "the retired attempt never completes"
    );
}
