#![cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};

use meerkat_auth_core::auth_store::{
    EphemeralTokenStore, PersistedAuthMode, PersistedTokens, RefreshCoordinator, RefreshError,
    RefreshFn, TokenKey, TokenStore,
};
use meerkat_core::handles::{AuthLeaseHandle, GeneratedAuthLeaseHandle, LeaseKey};
use meerkat_core::{
    AuthBindingRef, AuthConstraints, AuthProfileConfig, BackendProfileConfig, BindingId,
    CredentialSourceSpec, ProviderBindingConfig, RealmConfigSection, RealmConnectionSet, RealmId,
};
use meerkat_gemini::runtime::oauth;
use meerkat_llm_core::provider_runtime::{ProviderRuntimeRegistry, ResolverEnvironment};

fn code_assist_realm() -> RealmConnectionSet {
    code_assist_realm_with_source(CredentialSourceSpec::PlatformDefault)
}

fn code_assist_realm_no_refresh() -> RealmConnectionSet {
    let mut backend = BTreeMap::new();
    backend.insert(
        "code_assist".into(),
        BackendProfileConfig {
            provider: "gemini".into(),
            backend_kind: "google_code_assist".into(),
            base_url: None,
            options: serde_json::json!({"realm_id": "dev", "project_id": "test-project"}),
        },
    );
    let mut auth = BTreeMap::new();
    auth.insert(
        "google_oauth".into(),
        AuthProfileConfig {
            provider: "gemini".into(),
            auth_method: "google_oauth".into(),
            source: CredentialSourceSpec::PlatformDefault,
            constraints: AuthConstraints {
                allow_interactive_login: true,
                allow_refresh: false,
                ..Default::default()
            },
            metadata_defaults: Default::default(),
        },
    );
    let mut binding = BTreeMap::new();
    binding.insert(
        "default_code_assist".into(),
        ProviderBindingConfig {
            backend_profile: "code_assist".into(),
            auth_profile: "google_oauth".into(),
            default_model: None,
            policy: Default::default(),
            provider_default: false,
        },
    );
    RealmConnectionSet::from_config(
        "dev",
        &RealmConfigSection {
            backend,
            auth,
            binding,
            default_binding: Some("default_code_assist".into()),
            parent: None,
        },
    )
    .unwrap()
}

fn code_assist_realm_with_source(source: CredentialSourceSpec) -> RealmConnectionSet {
    let mut backend = BTreeMap::new();
    backend.insert(
        "code_assist".into(),
        BackendProfileConfig {
            provider: "gemini".into(),
            backend_kind: "google_code_assist".into(),
            base_url: None,
            options: serde_json::json!({"realm_id": "dev", "project_id": "test-project"}),
        },
    );
    let mut auth = BTreeMap::new();
    auth.insert(
        "google_oauth".into(),
        AuthProfileConfig {
            provider: "gemini".into(),
            auth_method: "google_oauth".into(),
            source,
            constraints: AuthConstraints {
                allow_interactive_login: true,
                ..Default::default()
            },
            metadata_defaults: Default::default(),
        },
    );
    let mut binding = BTreeMap::new();
    binding.insert(
        "default_code_assist".into(),
        ProviderBindingConfig {
            backend_profile: "code_assist".into(),
            auth_profile: "google_oauth".into(),
            default_model: None,
            policy: Default::default(),
            provider_default: false,
        },
    );
    RealmConnectionSet::from_config(
        "dev",
        &RealmConfigSection {
            backend,
            auth,
            binding,
            default_binding: Some("default_code_assist".into()),
            parent: None,
        },
    )
    .unwrap()
}

fn default_auth_binding() -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse("dev").expect("valid realm"),
        binding: BindingId::parse("default_code_assist").expect("valid binding"),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    }
}

fn mark_tokens_lifecycle_published_for_handle(
    tokens: &PersistedTokens,
    handle: Arc<meerkat_runtime::RuntimeAuthLeaseHandle>,
) -> (PersistedTokens, GeneratedAuthLeaseHandle) {
    let key = TokenKey::from_auth_binding(&default_auth_binding());
    let lease_key = LeaseKey::from_auth_binding(&default_auth_binding());
    let transition = handle
        .acquire_lease(
            &lease_key,
            meerkat_core::persisted_token_expires_at_epoch_secs(tokens),
        )
        .expect("fixture AuthMachine accepts acquired lease");
    let marked =
        meerkat_core::mark_tokens_lifecycle_published_for_transition(&key, tokens, &transition)
            .expect("runtime AuthMachine transition marks fixture tokens");
    (marked, generated_auth_lease_handle_for_test(handle))
}

fn generated_auth_lease_handle_for_test(
    handle: Arc<meerkat_runtime::RuntimeAuthLeaseHandle>,
) -> GeneratedAuthLeaseHandle {
    meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(handle)
        .expect("runtime AuthLeaseHandle is certified by generated AuthMachine authority")
}

fn valid_auth_lease_handle() -> GeneratedAuthLeaseHandle {
    valid_auth_lease_handle_for_expires_at(u64::MAX)
}

fn marked_tokens_with_valid_auth_lease_handle(
    tokens: &PersistedTokens,
) -> (PersistedTokens, GeneratedAuthLeaseHandle) {
    mark_tokens_lifecycle_published_for_handle(
        tokens,
        Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new()),
    )
}

fn valid_auth_lease_handle_for_expires_at(expires_at: u64) -> GeneratedAuthLeaseHandle {
    let lease_key = LeaseKey::from_auth_binding(&default_auth_binding());
    let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    handle
        .acquire_lease(&lease_key, expires_at)
        .expect("fixture AuthMachine accepts acquired lease");
    generated_auth_lease_handle_for_test(handle)
}

fn reauth_required_auth_lease_handle() -> GeneratedAuthLeaseHandle {
    let lease_key = LeaseKey::from_auth_binding(&default_auth_binding());
    let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    handle
        .acquire_lease(&lease_key, u64::MAX)
        .expect("fixture AuthMachine accepts acquired lease");
    handle
        .mark_reauth_required(&lease_key)
        .expect("fixture AuthMachine marks reauth required");
    generated_auth_lease_handle_for_test(handle)
}

struct StaticRefreshCoordinator {
    tokens: PersistedTokens,
}

#[async_trait::async_trait]
impl RefreshCoordinator for StaticRefreshCoordinator {
    async fn with_refresh(
        &self,
        _key: TokenKey,
        _refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError> {
        Ok(self.tokens.clone())
    }
}

struct FailingRefreshCoordinator {
    error: RefreshError,
}

#[async_trait::async_trait]
impl RefreshCoordinator for FailingRefreshCoordinator {
    async fn with_refresh(
        &self,
        _key: TokenKey,
        _refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError> {
        Err(self.error.clone())
    }
}

fn persisted_google_oauth(secret: &str) -> PersistedTokens {
    PersistedTokens {
        auth_mode: PersistedAuthMode::GoogleOauth,
        primary_secret: Some(secret.into()),
        refresh_token: Some("refresh-google".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        // Scopes are sourced from the single auth-core OAuth declaration via
        // the runtime's endpoint projection (dogma §123: no duplicate scope
        // constant in the gemini runtime).
        scopes: oauth::code_assist_endpoints("http://127.0.0.1:0/callback").scopes,
        account_id: None,
        metadata: serde_json::Value::Null,
    }
}

#[tokio::test]
async fn google_oauth_fresh_token_resolves_with_auth_lifecycle() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = persisted_google_oauth("fresh-google-access");
    let (marked, auth_lease_handle) = marked_tokens_with_valid_auth_lease_handle(&persisted);
    store
        .save(
            &TokenKey::parse("dev", "default_code_assist").expect("valid slugs"),
            &marked,
        )
        .await
        .unwrap();

    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease_handle);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(meerkat_gemini::GoogleProviderRuntime));
    let connection = registry
        .resolve(&code_assist_realm(), &default_auth_binding(), &env)
        .await
        .expect("fresh Google OAuth tokens should resolve");

    assert_eq!(
        connection.resolved_secret(),
        Some("fresh-google-access".to_string())
    );
}

#[tokio::test]
async fn google_oauth_rejects_token_without_auth_lifecycle() {
    let store = Arc::new(EphemeralTokenStore::new());
    store
        .save(
            &TokenKey::parse("dev", "default_code_assist").expect("valid slugs"),
            &persisted_google_oauth("stale-google-access"),
        )
        .await
        .unwrap();

    let env = ResolverEnvironment::testing().with_token_store(store);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(meerkat_gemini::GoogleProviderRuntime));
    let err = registry
        .resolve(&code_assist_realm(), &default_auth_binding(), &env)
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::StaleCredential
            )
        ),
        "got {err:?}"
    );
}

#[tokio::test]
async fn google_oauth_reauth_required_is_typed() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = persisted_google_oauth("reauth-google-access");
    let (marked, _) = marked_tokens_with_valid_auth_lease_handle(&persisted);
    store
        .save(
            &TokenKey::parse("dev", "default_code_assist").expect("valid slugs"),
            &marked,
        )
        .await
        .unwrap();

    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(reauth_required_auth_lease_handle());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(meerkat_gemini::GoogleProviderRuntime));
    let err = registry
        .resolve(&code_assist_realm(), &default_auth_binding(), &env)
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::UserReauthRequired
            )
        ),
        "got {err:?}"
    );
}

#[tokio::test]
async fn google_oauth_rejects_wrong_persisted_mode() {
    let store = Arc::new(EphemeralTokenStore::new());
    store
        .save(
            &TokenKey::parse("dev", "default_code_assist").expect("valid slugs"),
            &PersistedTokens {
                auth_mode: PersistedAuthMode::ApiKey,
                primary_secret: Some("stale-google-api-key".into()),
                refresh_token: Some("refresh-google".into()),
                id_token: None,
                expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
                last_refresh: Some(Utc::now()),
                scopes: vec![],
                account_id: None,
                metadata: serde_json::Value::Null,
            },
        )
        .await
        .unwrap();

    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(valid_auth_lease_handle());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(meerkat_gemini::GoogleProviderRuntime));
    let err = registry
        .resolve(&code_assist_realm(), &default_auth_binding(), &env)
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::SourceResolutionFailed(_)
        ),
        "got {err:?}"
    );
    assert!(
        err.to_string().contains("credential mode ApiKey"),
        "got {err}"
    );
}

#[tokio::test]
async fn google_oauth_rejects_wrong_source_even_with_matching_mode() {
    let store = Arc::new(EphemeralTokenStore::new());
    store
        .save(
            &TokenKey::parse("dev", "default_code_assist").expect("valid slugs"),
            &persisted_google_oauth("fresh-google-access"),
        )
        .await
        .unwrap();

    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(valid_auth_lease_handle());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(meerkat_gemini::GoogleProviderRuntime));
    let err = registry
        .resolve(
            &code_assist_realm_with_source(CredentialSourceSpec::ExternalResolver {
                handle: "external-google".into(),
            }),
            &default_auth_binding(),
            &env,
        )
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::SourceResolutionFailed(_)
        ),
        "got {err:?}"
    );
    assert!(
        err.to_string().contains("source 'external_resolver'"),
        "got {err}"
    );
}

#[tokio::test]
async fn google_oauth_expired_authmachine_lease_refreshes_through_provider_runtime() {
    let key = TokenKey::parse("dev", "default_code_assist").expect("valid slugs");
    let store = Arc::new(EphemeralTokenStore::new());
    let old_expiry = Utc::now() - ChronoDuration::minutes(5);
    let mut expired = persisted_google_oauth("expired-google-access");
    expired.expires_at = Some(old_expiry);
    expired.last_refresh = Some(Utc::now() - ChronoDuration::hours(2));
    let (marked, auth_lease_handle) = marked_tokens_with_valid_auth_lease_handle(&expired);
    store.save(&key, &marked).await.unwrap();
    let mut refreshed = persisted_google_oauth("refreshed-google-access");
    refreshed.refresh_token = Some("rotated-google-refresh".into());

    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_refresh_coordinator(Arc::new(StaticRefreshCoordinator { tokens: refreshed }))
        .with_auth_lease_handle(auth_lease_handle);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(meerkat_gemini::GoogleProviderRuntime));
    let connection = registry
        .resolve(&code_assist_realm(), &default_auth_binding(), &env)
        .await
        .expect("expired Google OAuth lease should refresh through AuthMachine gate");

    assert_eq!(
        connection.resolved_secret(),
        Some("refreshed-google-access".to_string())
    );
    let stored = store.load(&key).await.unwrap().unwrap();
    assert_eq!(
        stored.primary_secret.as_deref(),
        Some("refreshed-google-access")
    );
    assert_eq!(
        stored.refresh_token.as_deref(),
        Some("rotated-google-refresh")
    );
    assert!(meerkat_core::tokens_lifecycle_published(&stored));
}

#[tokio::test]
async fn google_oauth_refresh_failure_is_typed() {
    let key = TokenKey::parse("dev", "default_code_assist").expect("valid slugs");
    let store = Arc::new(EphemeralTokenStore::new());
    let old_expiry = Utc::now() - ChronoDuration::minutes(5);
    let mut expired = persisted_google_oauth("expired-google-access");
    expired.expires_at = Some(old_expiry);
    expired.last_refresh = Some(Utc::now() - ChronoDuration::hours(2));
    let (marked, auth_lease_handle) = marked_tokens_with_valid_auth_lease_handle(&expired);
    store.save(&key, &marked).await.unwrap();

    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_refresh_coordinator(Arc::new(FailingRefreshCoordinator {
            error: RefreshError::Refresh("google refresh transport failed".into()),
        }))
        .with_auth_lease_handle(auth_lease_handle);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(meerkat_gemini::GoogleProviderRuntime));
    let err = registry
        .resolve(&code_assist_realm(), &default_auth_binding(), &env)
        .await
        .unwrap_err();

    match err {
        meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
            meerkat_core::AuthError::RefreshFailed(detail),
        ) => assert!(
            detail.contains("google refresh transport failed"),
            "got {detail}"
        ),
        other => panic!("got {other:?}"),
    }
}

#[tokio::test]
async fn google_oauth_force_refresh_uses_authmachine_gate_for_fresh_tokens() {
    let key = TokenKey::parse("dev", "default_code_assist").expect("valid slugs");
    let store = Arc::new(EphemeralTokenStore::new());
    let fresh = persisted_google_oauth("fresh-google-access");
    let (marked, auth_lease_handle) = marked_tokens_with_valid_auth_lease_handle(&fresh);
    store.save(&key, &marked).await.unwrap();
    let mut refreshed = persisted_google_oauth("forced-google-access");
    refreshed.refresh_token = Some("forced-google-refresh".into());

    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_refresh_coordinator(Arc::new(StaticRefreshCoordinator { tokens: refreshed }))
        .with_auth_lease_handle(auth_lease_handle)
        .with_force_refresh(true);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(meerkat_gemini::GoogleProviderRuntime));
    let connection = registry
        .resolve(&code_assist_realm(), &default_auth_binding(), &env)
        .await
        .expect("forced Google OAuth refresh should resolve through AuthMachine gate");

    assert_eq!(
        connection.resolved_secret(),
        Some("forced-google-access".to_string())
    );
    let stored = store.load(&key).await.unwrap().unwrap();
    assert_eq!(
        stored.primary_secret.as_deref(),
        Some("forced-google-access")
    );
    assert_eq!(
        stored.refresh_token.as_deref(),
        Some("forced-google-refresh")
    );
}

#[tokio::test]
async fn google_oauth_force_refresh_with_refresh_disallowed_errors_refresh_required() {
    // FOLD 2: fresh credential + force_refresh forces the refresh branch, but
    // the binding disallows silent refresh -> AuthMachine emits RefreshDisallowed
    // and the provider mirrors the RefreshRequired error.
    let key = TokenKey::parse("dev", "default_code_assist").expect("valid slugs");
    let store = Arc::new(EphemeralTokenStore::new());
    let fresh = persisted_google_oauth("fresh-google-access");
    let (marked, auth_lease_handle) = marked_tokens_with_valid_auth_lease_handle(&fresh);
    store.save(&key, &marked).await.unwrap();

    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease_handle)
        .with_force_refresh(true);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(meerkat_gemini::GoogleProviderRuntime));
    let err = registry
        .resolve(
            &code_assist_realm_no_refresh(),
            &default_auth_binding(),
            &env,
        )
        .await
        .unwrap_err();
    match err {
        meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
            meerkat_core::AuthError::RefreshRequired,
        ) => {}
        other => panic!("expected RefreshRequired, got {other:?}"),
    }
}
