//! OpenAI ChatGPT OAuth runtime.
//!
//! 1:1 with Codex:
//! - Client ID / issuer / scopes / redirect: `codex-rs/login/src/server.rs:51, 468-504`
//! - Token endpoint + refresh: `codex-rs/login/src/auth/manager.rs:89-90, 744, 852`
//! - JWT claims lift (`chatgpt_*`): `codex-rs/login/src/token_data.rs:71-160`
//! - ChatGPT-Account-ID + X-OpenAI-Fedramp wire headers:
//!   `codex-rs/login/src/auth/bearer_auth_provider.rs:23-38`
//!
//! Codex stores an `AuthDotJson` with `{ OPENAI_API_KEY?, tokens, last_refresh }`.
//! For the `managed_chatgpt_oauth` path we use the token bundle;
//! `api_key` mode reads `OPENAI_API_KEY` straight.

use std::sync::Arc;

use chrono::Utc;
use thiserror::Error;

use meerkat_auth_core::auth_oauth::{
    OAuthEndpoints, OAuthError, OAuthTokenResult, PkcePair, exchange_authorization_code,
    exchange_refresh_token, oauth_refresh_error,
};
use meerkat_auth_core::auth_store::{
    PersistedAuthMode, PersistedTokens, ProviderAuthPersistence, RefreshCoordinator, RefreshError,
    RefreshFn, TokenKey, TokenStore,
};
use meerkat_auth_core::oauth_flow::{
    OAuthProviderDeclaration, OAuthProviderIdentity, oauth_provider_declaration,
    oauth_provider_endpoints,
};
use meerkat_auth_core::resolver::{
    LockedManagedStoreOAuthRefresh, ManagedStoreOAuthRefreshPreparationSlot,
};

/// The canonical ChatGPT OAuth provider declaration, owned by auth-core.
///
/// The single owner of the ChatGPT OAuth `client_id`, authorize/token
/// endpoints, scopes, and typed backend kind is the auth-core declaration for
/// [`OAuthProviderIdentity::OpenAiChatGpt`] (verified against
/// codex-rs/login/src/{auth/manager,server}.rs). This runtime reads those
/// facts from here instead of redeclaring the literals (dogma row #123). If a
/// revoke/logout flow ever lands, its endpoint belongs on the auth-core
/// declaration like its siblings, not as a runtime-local constant.
pub fn chatgpt_declaration() -> OAuthProviderDeclaration {
    oauth_provider_declaration(OAuthProviderIdentity::OpenAiChatGpt)
}

// Wire header constants are defined in `auth.rs` (unconditional module) so
// they remain available when the interactive OAuth flow is feature-gated off.
pub use meerkat_core::provider_matrix::openai_auth::{CHATGPT_ACCOUNT_HEADER, FEDRAMP_HEADER};

pub type TokenPrepareFn = meerkat_auth_core::resolver::ManagedStoreOAuthRefreshPrepareFn;

// ---------------------------------------------------------------------
// Endpoints
// ---------------------------------------------------------------------

pub fn chatgpt_endpoints(redirect_uri: impl Into<String>) -> OAuthEndpoints {
    // Built from the single auth-core declaration for the ChatGPT provider;
    // the test-fixture endpoint override is applied inside `endpoints()`.
    oauth_provider_endpoints(OAuthProviderIdentity::OpenAiChatGpt, redirect_uri)
}

// ---------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum OpenAiOAuthError {
    #[error(transparent)]
    OAuth(#[from] OAuthError),
    #[error(transparent)]
    Refresh(#[from] RefreshError),
    #[error("no persisted ChatGPT tokens — interactive login required")]
    InteractiveLoginRequired,
    #[error("persisted tokens missing refresh_token")]
    MissingRefreshToken,
    #[error("token store error: {0}")]
    Store(String),
}

// ---------------------------------------------------------------------
// Claims lifted from the ID token (per Codex token_data.rs:71-160)
// ---------------------------------------------------------------------

/// ChatGPT-specific JWT claims lifted out of the id_token payload.
/// Populated from `auth.openai.com` / `{account_id, plan_type, ...}` in
/// the id_token.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ChatGptIdClaims {
    pub plan_type: Option<String>,
    pub user_id: Option<String>,
    pub account_id: Option<String>,
    pub is_fedramp: Option<bool>,
    pub email: Option<String>,
}

impl ChatGptIdClaims {
    /// Lift claims from a decoded JWT payload value.
    pub fn lift_from_claims(raw: &serde_json::Value) -> Self {
        // Codex claim keys live under `https://api.openai.com/auth` OR at
        // top level depending on token version. Profile email can live under
        // `https://api.openai.com/profile`. We probe all Codex-supported
        // shapes.
        let nested = raw.get("https://api.openai.com/auth");
        let profile = raw.get("https://api.openai.com/profile");
        fn get_str(v: &serde_json::Value, key: &str) -> Option<String> {
            v.get(key).and_then(|x| x.as_str()).map(ToString::to_string)
        }
        fn get_bool(v: &serde_json::Value, key: &str) -> Option<bool> {
            v.get(key).and_then(serde_json::Value::as_bool)
        }
        let nested_str = |key: &str| -> Option<String> {
            nested
                .and_then(|n| get_str(n, key))
                .or_else(|| get_str(raw, key))
        };
        let nested_bool = |key: &str| -> Option<bool> {
            nested
                .and_then(|n| get_bool(n, key))
                .or_else(|| get_bool(raw, key))
        };
        Self {
            plan_type: nested_str("chatgpt_plan_type"),
            user_id: nested_str("chatgpt_user_id").or_else(|| nested_str("user_id")),
            account_id: nested_str("chatgpt_account_id"),
            is_fedramp: nested_bool("chatgpt_account_is_fedramp"),
            email: get_str(raw, "email").or_else(|| profile.and_then(|p| get_str(p, "email"))),
        }
    }
}

// ---------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------

pub struct OpenAiOAuthRuntime {
    http: reqwest::Client,
    persistence: ProviderAuthPersistence,
    endpoints: OAuthEndpoints,
    key: TokenKey,
}

impl OpenAiOAuthRuntime {
    pub fn new(
        persistence: ProviderAuthPersistence,
        endpoints: OAuthEndpoints,
        key: TokenKey,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            persistence,
            endpoints,
            key,
        }
    }

    pub fn endpoints(&self) -> &OAuthEndpoints {
        &self.endpoints
    }

    pub fn key(&self) -> &TokenKey {
        &self.key
    }

    fn token_store(&self) -> Arc<dyn TokenStore> {
        self.persistence.token_store()
    }

    fn refresh_coordinator(&self) -> Arc<dyn RefreshCoordinator> {
        self.persistence.refresh_coordinator()
    }

    async fn refresh_tokens_with_locked_preparation_inner(
        &self,
        prepare_fn: TokenPrepareFn,
        force_refresh_coordination: bool,
    ) -> Result<PersistedTokens, OpenAiOAuthError> {
        let preparation = ManagedStoreOAuthRefreshPreparationSlot::new(prepare_fn);
        let http = self.http.clone();
        let endpoints = self.endpoints.clone();
        let token_store = self.token_store();
        let key = self.key.clone();
        let preparation_for_refresh = preparation.clone();
        let refresh_fn: RefreshFn = Box::new(move || {
            let http = http.clone();
            let endpoints = endpoints.clone();
            let token_store = Arc::clone(&token_store);
            let key = key.clone();
            let preparation = preparation_for_refresh.clone();
            Box::pin(async move {
                let current = token_store
                    .load(&key)
                    .await
                    .map_err(|e| RefreshError::Refresh(e.to_string()))?
                    .ok_or_else(|| {
                        RefreshError::Refresh(
                            "persisted tokens disappeared before OAuth refresh".into(),
                        )
                    })?;
                match preparation.claim_refresh_owner(current.clone()).await? {
                    LockedManagedStoreOAuthRefresh::UseCached(cached) => Ok(cached),
                    LockedManagedStoreOAuthRefresh::Refresh(transaction) => {
                        let refresh_token = match current.refresh_token.clone() {
                            Some(refresh_token) => refresh_token,
                            None => {
                                return Err(transaction.fail(RefreshError::Observed {
                                    message: "missing refresh_token".into(),
                                    observation: meerkat_core::RefreshFailureObservation::local_credential_unusable(),
                                }).await);
                            }
                        };
                        let account_id = current.account_id.clone();
                        let result =
                            match exchange_refresh_token(&http, &endpoints, &refresh_token, None)
                                .await
                            {
                                Ok(result) => result,
                                Err(error) => {
                                    return Err(transaction.fail(oauth_refresh_error(error)).await);
                                }
                            };
                        let refreshed = match oauth_result_to_persisted(
                            result,
                            PersistedAuthMode::ChatgptOauth,
                            Some(refresh_token),
                            account_id,
                        ) {
                            Ok(refreshed) => refreshed,
                            Err(error) => {
                                return Err(transaction
                                    .fail(RefreshError::Refresh(error.to_string()))
                                    .await);
                            }
                        };
                        transaction.commit(refreshed).await
                    }
                }
            })
        });
        let refreshed = if force_refresh_coordination {
            self.refresh_coordinator()
                .with_forced_refresh(self.key.clone(), refresh_fn)
                .await
        } else {
            self.refresh_coordinator()
                .with_refresh(self.key.clone(), refresh_fn)
                .await
        }
        .map_err(OpenAiOAuthError::from)?;

        preparation
            .finish_coordinated_refresh(
                self.refresh_coordinator(),
                self.token_store(),
                self.key.clone(),
                refreshed,
            )
            .await
            .map_err(OpenAiOAuthError::Refresh)
    }

    pub(crate) async fn refresh_tokens_with_locked_preparation(
        &self,
        prepare_fn: TokenPrepareFn,
        force_refresh_coordination: bool,
    ) -> Result<PersistedTokens, OpenAiOAuthError> {
        self.refresh_tokens_with_locked_preparation_inner(prepare_fn, force_refresh_coordination)
            .await
    }

    pub async fn complete_login(
        &self,
        code: &str,
        pkce_verifier: &str,
    ) -> Result<PersistedTokens, OpenAiOAuthError> {
        let result =
            exchange_authorization_code(&self.http, &self.endpoints, code, pkce_verifier, None)
                .await?;
        // Lift JWT claims from id_token if present, to populate account_id.
        let account_id = if let Some(ref id_token) = result.id_token {
            meerkat_auth_core::auth_oauth::jwt::decode_payload(id_token)
                .ok()
                .and_then(|c| {
                    let claims = ChatGptIdClaims::lift_from_claims(&c.raw);
                    claims.account_id
                })
        } else {
            None
        };
        let tokens =
            oauth_result_to_persisted(result, PersistedAuthMode::ChatgptOauth, None, account_id)?;
        Ok(tokens)
    }
}

fn oauth_result_to_persisted(
    result: OAuthTokenResult,
    mode: PersistedAuthMode,
    fallback_refresh: Option<String>,
    account_id: Option<String>,
) -> Result<PersistedTokens, OAuthError> {
    let now = Utc::now();
    let expires_at = result.expires_at_from(now)?;
    let scopes = result
        .scope
        .as_deref()
        .map(|s| s.split_whitespace().map(String::from).collect())
        .unwrap_or_default();
    Ok(PersistedTokens {
        auth_mode: mode,
        primary_secret: Some(result.access_token),
        refresh_token: result.refresh_token.or(fallback_refresh),
        id_token: result.id_token,
        expires_at,
        last_refresh: Some(now),
        scopes,
        account_id,
        metadata: serde_json::Value::Null,
    })
}

// ---------------------------------------------------------------------
// Interactive login session
// ---------------------------------------------------------------------

pub struct OpenAiLoginSession {
    pub pkce: PkcePair,
    pub state: String,
}

impl OpenAiLoginSession {
    pub fn new() -> Self {
        use std::time::SystemTime;
        let now_ns = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        Self {
            pkce: PkcePair::generate_s256(),
            state: format!("st-{now_ns:x}"),
        }
    }
}

impl Default for OpenAiLoginSession {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn chatgpt_constants_match_codex_source() {
        // The provider facts are sourced from the single auth-core declaration.
        let declaration = chatgpt_declaration();
        assert_eq!(declaration.client_id, "app_EMoamEEZ73f0CkXaXp7hrann");
        assert_eq!(
            declaration.authorize_endpoint,
            "https://auth.openai.com/oauth/authorize"
        );
        assert_eq!(
            declaration.token_endpoint,
            "https://auth.openai.com/oauth/token"
        );
        assert!(
            declaration
                .extra_authorize_params
                .contains(&("originator", "codex_cli_rs"))
        );
        assert_eq!(
            declaration.scopes,
            &[
                "openid",
                "profile",
                "email",
                "offline_access",
                "api.connectors.read",
                "api.connectors.invoke"
            ],
        );
        assert_eq!(CHATGPT_ACCOUNT_HEADER, "ChatGPT-Account-ID");
        assert_eq!(FEDRAMP_HEADER, "X-OpenAI-Fedramp");
    }

    #[test]
    fn oauth_result_to_persisted_rejects_expiry_overflow() {
        let err = oauth_result_to_persisted(
            OAuthTokenResult {
                access_token: "access-token".into(),
                refresh_token: Some("refresh-token".into()),
                id_token: None,
                expires_in_secs: Some(u64::MAX),
                scope: None,
            },
            PersistedAuthMode::ChatgptOauth,
            None,
            None,
        )
        .expect_err("oversized expires_in must not be persisted");

        assert!(matches!(
            err,
            OAuthError::TokenExpiryOutOfRange {
                expires_in_secs: u64::MAX
            }
        ));
    }

    #[test]
    fn id_claims_lift_from_nested_or_top_level() {
        // Nested under `https://api.openai.com/auth`.
        let nested = serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "pro",
                "chatgpt_user_id": "user_abc",
                "chatgpt_account_id": "acct_xyz",
                "chatgpt_account_is_fedramp": true,
            },
            "email": "luka@example.com",
        });
        let c = ChatGptIdClaims::lift_from_claims(&nested);
        assert_eq!(c.plan_type.as_deref(), Some("pro"));
        assert_eq!(c.user_id.as_deref(), Some("user_abc"));
        assert_eq!(c.account_id.as_deref(), Some("acct_xyz"));
        assert_eq!(c.is_fedramp, Some(true));
        assert_eq!(c.email.as_deref(), Some("luka@example.com"));

        // Top-level fallback.
        let top = serde_json::json!({
            "chatgpt_account_id": "acct_top",
            "chatgpt_plan_type": "plus",
        });
        let c = ChatGptIdClaims::lift_from_claims(&top);
        assert_eq!(c.account_id.as_deref(), Some("acct_top"));
        assert_eq!(c.plan_type.as_deref(), Some("plus"));

        let codex_shape = serde_json::json!({
            "https://api.openai.com/auth": {
                "user_id": "user_fallback",
            },
            "https://api.openai.com/profile": {
                "email": "profile@example.com",
            },
        });
        let c = ChatGptIdClaims::lift_from_claims(&codex_shape);
        assert_eq!(c.user_id.as_deref(), Some("user_fallback"));
        assert_eq!(c.email.as_deref(), Some("profile@example.com"));
    }

    mod ce_refresh_custody {
        use super::*;
        use axum::{
            Json, Router,
            body::Bytes,
            extract::State,
            http::StatusCode,
            response::{IntoResponse, Response},
            routing::post,
        };
        use meerkat_auth_core::resolver::{
            load_managed_store_tokens_with_lifecycle,
            prepare_managed_store_oauth_refresh_under_lock,
        };
        use meerkat_auth_core::{EphemeralTokenStore, InMemoryCoordinator};
        use meerkat_core::CredentialSourceSpec;
        use meerkat_core::handles::{AuthLeasePhase, GeneratedAuthLeaseHandle, LeaseKey};
        use meerkat_core::{AuthBindingRef, AuthProfile, BackendProfile, BindingPolicy, Provider};
        use meerkat_llm_core::provider_runtime::registry::ResolverEnvironment;
        use meerkat_llm_core::provider_runtime::{ProviderRuntimeCatalog, ValidatedBinding};
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Duration;
        use tokio::sync::{Notify, Semaphore};

        const BOUND: Duration = Duration::from_secs(10);
        #[derive(Clone, Copy)]
        enum Reply {
            Success,
            Transient,
            InvalidGrant,
        }
        struct Endpoint {
            arrived: Notify,
            release: Semaphore,
            requests: Mutex<Vec<Vec<u8>>>,
            reply: Reply,
        }
        async fn token(State(endpoint): State<Arc<Endpoint>>, body: Bytes) -> Response {
            endpoint.requests.lock().unwrap().push(body.to_vec());
            endpoint.arrived.notify_one();
            match endpoint.release.acquire().await {
                Ok(permit) => permit.forget(),
                Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
            }
            match endpoint.reply {
                Reply::Success => Json(serde_json::json!({"access_token":"ce-access-b", "refresh_token":"ce-refresh-b", "expires_in":3600})).into_response(),
                Reply::Transient => (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error":"temporarily_unavailable"}))).into_response(),
                Reply::InvalidGrant => (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error":"invalid_grant"}))).into_response(),
            }
        }
        struct FaultStore {
            inner: EphemeralTokenStore,
            fail_next_save: Arc<AtomicBool>,
        }
        #[async_trait::async_trait]
        impl TokenStore for FaultStore {
            async fn load(
                &self,
                key: &TokenKey,
            ) -> Result<Option<PersistedTokens>, meerkat_core::auth::TokenStoreError> {
                self.inner.load(key).await
            }
            async fn save(
                &self,
                key: &TokenKey,
                tokens: &PersistedTokens,
            ) -> Result<(), meerkat_core::auth::TokenStoreError> {
                if self.fail_next_save.swap(false, Ordering::SeqCst) {
                    return Err(meerkat_core::auth::TokenStoreError::Io(
                        "injected first save failure".into(),
                    ));
                }
                self.inner.save(key, tokens).await
            }
            async fn clear(
                &self,
                key: &TokenKey,
            ) -> Result<(), meerkat_core::auth::TokenStoreError> {
                self.inner.clear(key).await
            }
            async fn list(&self) -> Result<Vec<TokenKey>, meerkat_core::auth::TokenStoreError> {
                self.inner.list().await
            }
            fn backend_name(&self) -> &'static str {
                "ce-fault-store"
            }
        }
        struct Fixture {
            endpoint: Arc<Endpoint>,
            server: tokio::task::JoinHandle<()>,
            refresh: Option<tokio::task::JoinHandle<Result<PersistedTokens, OpenAiOAuthError>>>,
            store: Arc<dyn TokenStore>,
            auth: GeneratedAuthLeaseHandle,
            binding: ValidatedBinding,
            key: TokenKey,
            lease: LeaseKey,
            original: PersistedTokens,
            fail_next_save: Arc<AtomicBool>,
            persistence: ProviderAuthPersistence,
            endpoint_url: String,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                self.endpoint.release.close();
                self.server.abort();
                if let Some(refresh) = self.refresh.take() {
                    refresh.abort();
                }
            }
        }
        fn binding() -> ValidatedBinding {
            let auth_binding = AuthBindingRef {
                realm: meerkat_core::RealmId::parse("ce-refresh").unwrap(),
                binding: meerkat_core::BindingId::parse(format!(
                    "oauth-{}",
                    meerkat_core::SessionId::new()
                ))
                .unwrap(),
                profile: None,
                origin: meerkat_core::BindingOrigin::Configured,
            };
            ProviderRuntimeCatalog::validate_binding(
                &auth_binding,
                &BackendProfile {
                    id: "ce-backend".into(),
                    provider: Provider::OpenAI,
                    backend_kind: "chatgpt_backend".into(),
                    base_url: None,
                    options: serde_json::Value::Null,
                    server: None,
                },
                &AuthProfile {
                    id: "ce-oauth".into(),
                    provider: Provider::OpenAI,
                    auth_method: "managed_chatgpt_oauth".into(),
                    source: CredentialSourceSpec::ManagedStore,
                    constraints: Default::default(),
                    metadata_defaults: Default::default(),
                },
                &BindingPolicy::default(),
            )
            .unwrap()
        }
        impl Fixture {
            async fn start(reply: Reply) -> Self {
                let endpoint = Arc::new(Endpoint {
                    arrived: Notify::new(),
                    release: Semaphore::new(0),
                    requests: Mutex::new(Vec::new()),
                    reply,
                });
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let address = listener.local_addr().unwrap();
                let app = Router::new()
                    .route("/token", post(token))
                    .with_state(endpoint.clone());
                let server = tokio::spawn(async move {
                    axum::serve(listener, app).await.unwrap();
                });
                let binding = binding();
                let key = TokenKey::from_credential_identity(binding.credential_identity());
                let lease = LeaseKey::from_credential_identity(binding.credential_identity());
                let fail_next_save = Arc::new(AtomicBool::new(false));
                let store: Arc<dyn TokenStore> = Arc::new(FaultStore {
                    inner: EphemeralTokenStore::new(),
                    fail_next_save: fail_next_save.clone(),
                });
                let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
                let auth = meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(handle).unwrap();
                let raw = PersistedTokens {
                    auth_mode: PersistedAuthMode::ChatgptOauth,
                    primary_secret: Some("ce-access-a".into()),
                    refresh_token: Some("ce-refresh-a".into()),
                    id_token: None,
                    expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
                    last_refresh: Some(Utc::now()),
                    scopes: Vec::new(),
                    account_id: Some("ce-account".into()),
                    metadata: serde_json::Value::Null,
                };
                let transition = auth
                    .acquire_lease(
                        &lease,
                        meerkat_core::persisted_token_expires_at_epoch_secs(&raw),
                    )
                    .unwrap();
                let original = meerkat_core::mark_tokens_lifecycle_published_for_transition(
                    &key,
                    &raw,
                    &transition,
                )
                .unwrap();
                store.save(&key, &original).await.unwrap();
                let persistence = ProviderAuthPersistence::new(
                    store.clone(),
                    Arc::new(InMemoryCoordinator::new()),
                );
                let env = ResolverEnvironment::testing()
                    .with_provider_auth_persistence(persistence.clone())
                    .with_auth_lease_handle(auth.clone())
                    .with_force_refresh(true);
                let mut previous = load_managed_store_tokens_with_lifecycle(&env, &binding)
                    .await
                    .unwrap();
                // Preserve the production pre-coordinator release and exact re-admission.
                previous.release_prelock_lifecycle_guard();
                let prepare_binding = binding.clone();
                let prepare: TokenPrepareFn = Box::new(move |locked, mode| {
                    Box::pin(async move {
                        prepare_managed_store_oauth_refresh_under_lock(
                            &env,
                            &prepare_binding,
                            previous,
                            locked,
                            mode,
                        )
                        .await
                        .map_err(meerkat_auth_core::resolver::refresh_error_from_provider)
                    })
                });
                let mut endpoints = chatgpt_endpoints("http://127.0.0.1:0/callback");
                endpoints.token_url = format!("http://{address}/token");
                let runtime = OpenAiOAuthRuntime::new(persistence.clone(), endpoints, key.clone());
                let refresh = tokio::spawn(async move {
                    runtime
                        .refresh_tokens_with_locked_preparation(prepare, true)
                        .await
                });
                let fixture = Self {
                    endpoint,
                    server,
                    refresh: Some(refresh),
                    store,
                    auth,
                    binding,
                    key,
                    lease,
                    original,
                    fail_next_save,
                    persistence,
                    endpoint_url: format!("http://{address}/token"),
                };
                let started =
                    tokio::time::timeout(BOUND, fixture.endpoint.arrived.notified()).await;
                assert!(
                    started.is_ok(),
                    "actual OAuth HTTP request must enter the recording endpoint"
                );
                assert_eq!(
                    fixture.auth.snapshot(&fixture.lease).phase,
                    Some(AuthLeasePhase::Refreshing)
                );
                fixture
            }
            async fn restart(&mut self) {
                self.spawn_again(None).await;
                assert!(
                    tokio::time::timeout(BOUND, self.endpoint.arrived.notified())
                        .await
                        .is_ok()
                );
            }
            async fn spawn_again(&mut self, http: Option<reqwest::Client>) {
                assert!(self.refresh.is_none());
                let env = ResolverEnvironment::testing()
                    .with_provider_auth_persistence(self.persistence.clone())
                    .with_auth_lease_handle(self.auth.clone())
                    .with_force_refresh(true);
                let mut previous = load_managed_store_tokens_with_lifecycle(&env, &self.binding)
                    .await
                    .unwrap();
                previous.release_prelock_lifecycle_guard();
                let binding = self.binding.clone();
                let prepare: TokenPrepareFn = Box::new(move |locked, mode| {
                    Box::pin(async move {
                        prepare_managed_store_oauth_refresh_under_lock(
                            &env, &binding, previous, locked, mode,
                        )
                        .await
                        .map_err(meerkat_auth_core::resolver::refresh_error_from_provider)
                    })
                });
                let mut endpoints = chatgpt_endpoints("http://127.0.0.1:0/callback");
                endpoints.token_url = self.endpoint_url.clone();
                let mut runtime =
                    OpenAiOAuthRuntime::new(self.persistence.clone(), endpoints, self.key.clone());
                if let Some(http) = http {
                    runtime.http = http;
                }
                self.refresh = Some(tokio::spawn(async move {
                    runtime
                        .refresh_tokens_with_locked_preparation(prepare, true)
                        .await
                }));
            }
            async fn finish(&mut self) -> Result<PersistedTokens, OpenAiOAuthError> {
                self.endpoint.release.add_permits(1);
                let mut refresh = self.refresh.take().unwrap();
                let completed = tokio::time::timeout(BOUND, &mut refresh).await;
                if completed.is_err() {
                    refresh.abort();
                    let _ = refresh.await;
                }
                completed
                    .expect("refresh completion is bounded")
                    .expect("refresh task did not panic")
            }
            fn assert_one_original_exchange(&self) {
                let requests = self.endpoint.requests.lock().unwrap();
                assert_eq!(requests.len(), 1, "no hidden second refresh exchange");
                assert!(
                    String::from_utf8_lossy(&requests[0]).contains("ce-refresh-a"),
                    "the request uses the actual selected predecessor"
                );
            }
        }

        #[tokio::test]
        async fn ce_provider_http_releases_only_lifecycle_guard() {
            let mut fixture = Fixture::start(Reply::Success).await;
            let acquired = meerkat_core::try_acquire_auth_login_lifecycle_guard(&fixture.lease);
            let available_during_http = acquired.is_some();
            drop(acquired);
            let stored_during_http = fixture.store.load(&fixture.key).await.unwrap();
            let result = fixture.finish().await.unwrap();
            fixture.assert_one_original_exchange();
            assert_eq!(stored_during_http, Some(fixture.original.clone()));
            assert_eq!(result.refresh_token.as_deref(), Some("ce-refresh-b"));
            assert!(
                available_during_http,
                "actual endpoint entry proves network is in flight; its lifecycle guard must be available"
            );
        }

        #[tokio::test]
        async fn ce_provider_status_during_http_preserves_rotated_commit() {
            let mut fixture = Fixture::start(Reply::Success).await;
            let store = fixture.store.clone();
            let auth = fixture.auth.clone();
            let binding = fixture.binding.auth_binding_ref().clone();
            let lease = fixture.lease.clone();
            let mut status = tokio::spawn(async move {
                let result = meerkat_core::rehydrate_marked_tokens_for_status(
                    store.as_ref(),
                    &auth,
                    &binding,
                    PersistedAuthMode::ChatgptOauth,
                    Utc::now(),
                )
                .await;
                (result, auth.snapshot(&lease))
            });
            // Completion is required while the positive HTTP gate is closed.
            // A timeout cannot be mistaken for progress after releasing HTTP.
            let early = tokio::time::timeout(Duration::from_millis(250), &mut status).await;
            let completed_during_http = early.is_ok();
            let refresh_result = fixture.finish().await;
            let status_result = match early {
                Ok(result) => result.unwrap(),
                Err(_) => tokio::time::timeout(BOUND, &mut status)
                    .await
                    .expect("status settles after cleanup")
                    .unwrap(),
            };
            fixture.assert_one_original_exchange();
            assert!(
                completed_during_http,
                "status must finish before the endpoint is released"
            );
            assert!(status_result.0.is_ok());
            assert_eq!(
                status_result.1.phase,
                Some(AuthLeasePhase::Refreshing),
                "status preserves the in-flight owner"
            );
            let refreshed =
                refresh_result.expect("status must not discard a valid rotated response");
            assert_eq!(refreshed.refresh_token.as_deref(), Some("ce-refresh-b"));
            assert_eq!(
                fixture.store.load(&fixture.key).await.unwrap(),
                Some(refreshed)
            );
        }

        #[tokio::test]
        async fn ce_provider_stale_success_and_failure_preserve_replacement() {
            let mut observations = Vec::new();
            for reply in [Reply::Success, Reply::InvalidGrant] {
                let mut fixture = Fixture::start(reply).await;
                // Deliberate trusted-owner fault injection, not ordinary login:
                // the latter remains serialized behind the coordinator.
                let mut replacement = fixture.original.clone();
                replacement.primary_secret = Some("ce-new-owner-access".into());
                replacement.refresh_token = Some("ce-new-owner-refresh".into());
                let transition = fixture
                    .auth
                    .acquire_lease(
                        &fixture.lease,
                        meerkat_core::persisted_token_expires_at_epoch_secs(&replacement),
                    )
                    .unwrap();
                let replacement = meerkat_core::mark_tokens_lifecycle_published_for_transition(
                    &fixture.key,
                    &replacement,
                    &transition,
                )
                .unwrap();
                fixture
                    .store
                    .save(&fixture.key, &replacement)
                    .await
                    .unwrap();
                fixture.auth.begin_refresh(&fixture.lease).unwrap();
                let expected = fixture.auth.snapshot(&fixture.lease);
                let result = fixture.finish().await;
                fixture.assert_one_original_exchange();
                observations.push((
                    matches!(
                        result,
                        Err(OpenAiOAuthError::Refresh(RefreshError::StalePreparation))
                    ),
                    expected,
                    fixture.auth.snapshot(&fixture.lease),
                    replacement,
                    fixture.store.load(&fixture.key).await.unwrap(),
                ));
            }
            for (refused, expected, actual, replacement, stored) in observations {
                assert!(refused, "the old attempt must not return a credential");
                assert_eq!(
                    actual, expected,
                    "neither stale success nor stale failure may close the new owner's Refreshing phase"
                );
                assert_eq!(
                    stored,
                    Some(replacement),
                    "no stale save/clear/rollback may replace the new record"
                );
            }
        }

        #[tokio::test]
        async fn ce_provider_transient_and_definitive_failures_keep_distinct_lifecycle() {
            for (reply, phase) in [
                (Reply::Transient, AuthLeasePhase::Expiring),
                (Reply::InvalidGrant, AuthLeasePhase::ReauthRequired),
            ] {
                let mut fixture = Fixture::start(reply).await;
                let result = fixture.finish().await;
                fixture.assert_one_original_exchange();
                assert!(result.is_err());
                assert_eq!(fixture.auth.snapshot(&fixture.lease).phase, Some(phase));
                assert_eq!(
                    fixture.store.load(&fixture.key).await.unwrap(),
                    Some(fixture.original.clone())
                );
            }
        }
        #[tokio::test]
        async fn ce_provider_compensated_marker_cannot_revive_released_attempt() {
            let mut fixture = Fixture::start(Reply::Success).await;
            fixture.fail_next_save.store(true, Ordering::SeqCst);
            let first = fixture.finish().await;
            assert!(
                first.is_err(),
                "actual save failure must trigger compensation"
            );
            let compensated = fixture.store.load(&fixture.key).await.unwrap().unwrap();
            assert_eq!(compensated.primary_secret, fixture.original.primary_secret);
            assert!(meerkat_core::tokens_lifecycle_published(&compensated));
            assert_ne!(
                meerkat_core::tokens_lifecycle_publication(&compensated)
                    .unwrap()
                    .phase,
                Some(AuthLeasePhase::Refreshing),
                "compensation publishes only the actual closed transition"
            );
            // Baseline compensation writes its actual captured Refreshing
            // marker. Do not fabricate it or require a future fix to keep it.
            fixture.restart().await;
            let guard = meerkat_core::try_acquire_auth_login_lifecycle_guard(&fixture.lease);
            let available = guard.is_some();
            let after_status = if let Some(guard) = guard {
                fixture
                    .auth
                    .release_lease_with_guard(&fixture.lease, &guard)
                    .unwrap();
                drop(guard);
                let status = meerkat_core::rehydrate_marked_tokens_for_status(
                    fixture.store.as_ref(),
                    &fixture.auth,
                    fixture.binding.auth_binding_ref(),
                    PersistedAuthMode::ChatgptOauth,
                    Utc::now(),
                )
                .await;
                assert!(
                    status.is_ok(),
                    "preserve existing public status restore contract"
                );
                Some(fixture.auth.snapshot(&fixture.lease))
            } else {
                None
            };
            let second = fixture.finish().await;
            let after = fixture.auth.snapshot(&fixture.lease);
            let stored = fixture.store.load(&fixture.key).await.unwrap();
            assert!(
                available,
                "baseline prerequisite: lifecycle guard is free during second actual HTTP"
            );
            assert!(
                matches!(
                    second,
                    Err(OpenAiOAuthError::Refresh(RefreshError::StalePreparation))
                ),
                "a pre-Release attempt cannot become current again through its own compensated durable marker"
            );
            assert_eq!(
                Some(after),
                after_status,
                "stale completion cannot mutate the restored owner"
            );
            assert_eq!(
                stored,
                Some(compensated),
                "stale completion cannot overwrite the actual compensated predecessor"
            );
            assert_eq!(fixture.endpoint.requests.lock().unwrap().len(), 2);
        }

        // Construct historical bytes through actual generated restore/mark APIs,
        // not JSON edits. This reproduces the old compensation publication.
        async fn install_legacy_refreshing_marker(fixture: &Fixture) -> PersistedTokens {
            let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&fixture.lease).await;
            let previous = fixture.store.load(&fixture.key).await.unwrap().unwrap();
            fixture.auth.begin_refresh(&fixture.lease).unwrap();
            let captured = fixture
                .auth
                .capture_auth_lifecycle_restore_snapshot(&fixture.lease);
            fixture
                .auth
                .release_credential_lifecycle(&fixture.lease)
                .unwrap();
            let restored = meerkat_core::restore_token_lifecycle_snapshot(&fixture.auth, &captured)
                .unwrap()
                .unwrap();
            let legacy = meerkat_core::mark_tokens_lifecycle_published_for_transition(
                &fixture.key,
                &previous,
                &restored,
            )
            .unwrap();
            assert_eq!(
                meerkat_core::tokens_lifecycle_publication(&legacy)
                    .unwrap()
                    .phase,
                Some(AuthLeasePhase::Refreshing)
            );
            fixture.store.save(&fixture.key, &legacy).await.unwrap();
            fixture
                .auth
                .refresh_failed(
                    &fixture.lease,
                    meerkat_core::RefreshFailureObservation::transient(),
                )
                .unwrap();
            drop(guard);
            legacy
        }

        #[tokio::test]
        async fn ce_provider_legacy_marker_is_closed_before_next_http() {
            let mut fixture = Fixture::start(Reply::Success).await;
            fixture.finish().await.unwrap();
            let legacy = install_legacy_refreshing_marker(&fixture).await;
            fixture.restart().await;
            let stored = fixture.store.load(&fixture.key).await.unwrap().unwrap();
            let phase = meerkat_core::tokens_lifecycle_publication(&stored)
                .unwrap()
                .phase;
            let current = fixture.auth.snapshot(&fixture.lease);
            let result = fixture.finish().await.unwrap();
            assert_ne!(
                phase,
                Some(AuthLeasePhase::Refreshing),
                "old publication closed before another exchange"
            );
            assert_eq!(stored.primary_secret, legacy.primary_secret);
            assert_eq!(
                current.phase,
                Some(AuthLeasePhase::Refreshing),
                "new exchange owns its own live phase"
            );
            assert_eq!(
                result,
                fixture.store.load(&fixture.key).await.unwrap().unwrap()
            );
            assert_eq!(fixture.endpoint.requests.lock().unwrap().len(), 2);
        }

        #[tokio::test]
        async fn ce_provider_failed_legacy_normalization_save_never_sends_http() {
            let mut fixture = Fixture::start(Reply::Success).await;
            fixture.finish().await.unwrap();
            let legacy = install_legacy_refreshing_marker(&fixture).await;
            fixture.fail_next_save.store(true, Ordering::SeqCst);
            fixture.spawn_again(None).await;
            let result = tokio::time::timeout(BOUND, fixture.refresh.take().unwrap())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                result,
                Err(OpenAiOAuthError::Refresh(RefreshError::Refresh(_)))
            ));
            assert_eq!(
                fixture.endpoint.requests.lock().unwrap().len(),
                1,
                "positive original exchange, no new HTTP after normalization save failure"
            );
            assert_eq!(
                fixture.store.load(&fixture.key).await.unwrap(),
                Some(legacy)
            );
            assert_ne!(
                fixture.auth.snapshot(&fixture.lease).phase,
                Some(AuthLeasePhase::Refreshing)
            );
            assert!(meerkat_core::try_acquire_auth_login_lifecycle_guard(&fixture.lease).is_some());
        }

        #[tokio::test]
        async fn ce_provider_timeout_and_connect_error_close_only_own_attempt() {
            let mut fixture = Fixture::start(Reply::Success).await;
            fixture.finish().await.unwrap();
            let prior = fixture.store.load(&fixture.key).await.unwrap();
            let http = reqwest::Client::builder()
                .timeout(Duration::from_millis(100))
                .build()
                .unwrap();
            fixture.spawn_again(Some(http)).await;
            tokio::time::timeout(BOUND, fixture.endpoint.arrived.notified())
                .await
                .unwrap();
            let timeout = tokio::time::timeout(BOUND, fixture.refresh.take().unwrap())
                .await
                .unwrap()
                .unwrap();
            fixture.endpoint.release.add_permits(1);
            assert!(matches!(
                timeout,
                Err(OpenAiOAuthError::Refresh(RefreshError::Observed { .. }))
            ));
            assert_eq!(
                fixture.auth.snapshot(&fixture.lease).phase,
                Some(AuthLeasePhase::Expiring)
            );
            assert_eq!(fixture.store.load(&fixture.key).await.unwrap(), prior);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            fixture.endpoint_url = format!("http://{}/closed", listener.local_addr().unwrap());
            drop(listener);
            fixture.spawn_again(None).await;
            let network = tokio::time::timeout(BOUND, fixture.refresh.take().unwrap())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                network,
                Err(OpenAiOAuthError::Refresh(RefreshError::Observed { .. }))
            ));
            assert_eq!(
                fixture.auth.snapshot(&fixture.lease).phase,
                Some(AuthLeasePhase::Expiring)
            );
            assert_eq!(fixture.store.load(&fixture.key).await.unwrap(), prior);
            assert_eq!(fixture.endpoint.requests.lock().unwrap().len(), 2);
        }

        #[tokio::test]
        async fn ce_provider_legacy_rebase_cannot_erase_current_reauth() {
            let mut fixture = Fixture::start(Reply::Success).await;
            fixture.finish().await.unwrap();
            let env = ResolverEnvironment::testing()
                .with_provider_auth_persistence(fixture.persistence.clone())
                .with_auth_lease_handle(fixture.auth.clone())
                .with_force_refresh(true);
            let mut previous = load_managed_store_tokens_with_lifecycle(&env, &fixture.binding)
                .await
                .unwrap();
            previous.release_prelock_lifecycle_guard();
            let legacy = install_legacy_refreshing_marker(&fixture).await;
            assert_ne!(
                previous.tokens, legacy,
                "must exercise the changed-baseline rebase branch"
            );
            fixture.auth.mark_reauth_required(&fixture.lease).unwrap();
            let expected = fixture.auth.snapshot(&fixture.lease);
            let binding = fixture.binding.clone();
            let locked = legacy.clone();
            let result = fixture.persistence.refresh_coordinator().with_forced_refresh(fixture.key.clone(),
                Box::new(move || Box::pin(async move {
                    match prepare_managed_store_oauth_refresh_under_lock(&env, &binding, previous, locked,
                        meerkat_auth_core::resolver::ManagedStoreOAuthRefreshPreparationMode::RefreshOwner).await {
                        Err(error) => Err(meerkat_auth_core::resolver::refresh_error_from_provider(error)),
                        Ok(_) => panic!("a current reauth verdict cannot be replaced by a legacy marker"),
                    }
                }))).await;
            assert!(matches!(result, Err(RefreshError::StalePreparation)));
            assert_eq!(fixture.auth.snapshot(&fixture.lease), expected);
            assert_eq!(
                fixture.store.load(&fixture.key).await.unwrap(),
                Some(legacy)
            );
            assert_eq!(fixture.endpoint.requests.lock().unwrap().len(), 1);
        }

        #[tokio::test]
        async fn ce_provider_owned_refresh_survives_waiter_abort_at_reacquire() {
            for reply in [Reply::Success, Reply::Transient] {
                let mut fixture = Fixture::start(reply).await;
                let guard = meerkat_core::try_acquire_auth_login_lifecycle_guard(&fixture.lease)
                    .expect("the actual endpoint is entered without holding the lifecycle guard");
                fixture.endpoint.release.add_permits(1);
                let waiter = fixture.refresh.take().unwrap();
                waiter.abort();
                assert!(waiter.await.unwrap_err().is_cancelled());
                drop(guard);
                let store = fixture.store.clone();
                let key = fixture.key.clone();
                let fence = tokio::time::timeout(
                    Duration::from_secs(10),
                    fixture
                        .persistence
                        .refresh_coordinator()
                        .with_exclusive_mutation(
                            fixture.key.clone(),
                            Box::new(move || {
                                Box::pin(async move {
                                    Ok(meerkat_core::auth::CredentialMutationOutcome::Persisted(
                                        store.load(&key).await.unwrap().unwrap(),
                                    ))
                                })
                            }),
                        ),
                )
                .await
                .expect("owned refresh must release its coordinator after cancellation")
                .unwrap();
                let meerkat_core::auth::CredentialMutationOutcome::Persisted(stored) = fence else {
                    panic!("stored result");
                };
                let snapshot = fixture.auth.snapshot(&fixture.lease);
                match reply {
                    Reply::Success => {
                        assert_eq!(stored.refresh_token.as_deref(), Some("ce-refresh-b"));
                        assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
                    }
                    Reply::Transient => {
                        assert_eq!(stored, fixture.original);
                        assert_eq!(snapshot.phase, Some(AuthLeasePhase::Expiring));
                    }
                    Reply::InvalidGrant => unreachable!(),
                }
                fixture.assert_one_original_exchange();
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "oauth/ce_native_progress.rs"]
mod ce_native_progress;
