#![cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::{Duration as ChronoDuration, Utc};

use meerkat_auth_core::auth_store::{
    EphemeralTokenStore, PersistedAuthMode, PersistedTokens, RefreshCoordinator, RefreshError,
    RefreshFn, TokenKey, TokenStore,
};
use meerkat_core::handles::{
    AuthLeaseHandle, AuthLeasePhase, AuthLeaseSnapshot, AuthLeaseTransition, DslTransitionError,
    LeaseKey,
};
use meerkat_core::{
    AuthBindingRef, AuthConstraints, AuthProfileConfig, BackendProfileConfig, BindingId,
    CredentialSourceSpec, ProviderBindingConfig, RealmConfigSection, RealmConnectionSet, RealmId,
};
use meerkat_gemini::runtime::oauth;
use meerkat_llm_core::provider_runtime::{ProviderRuntimeRegistry, ResolverEnvironment};

fn code_assist_realm() -> RealmConnectionSet {
    code_assist_realm_with_source(CredentialSourceSpec::PlatformDefault)
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
        },
    );
    RealmConnectionSet::from_config(
        "dev",
        &RealmConfigSection {
            backend,
            auth,
            binding,
            default_binding: Some("default_code_assist".into()),
        },
    )
    .unwrap()
}

fn default_auth_binding() -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse("dev").expect("valid realm"),
        binding: BindingId::parse("default_code_assist").expect("valid binding"),
        profile: None,
    }
}

fn generated_auth_transition_for_test(expires_at: u64) -> AuthLeaseTransition {
    let handle = meerkat_runtime::RuntimeAuthLeaseHandle::new();
    let lease_key = LeaseKey::new(
        RealmId::parse("test").unwrap(),
        BindingId::parse("auth_transition").unwrap(),
        None,
    );
    handle.acquire_lease(&lease_key, expires_at).unwrap()
}

fn mark_tokens_lifecycle_published_for_test(
    tokens: &PersistedTokens,
    generation: u64,
    credential_published_at_millis: Option<u64>,
) -> PersistedTokens {
    let mut marked = tokens.clone();
    let mut marker = serde_json::json!({
        "published": true,
        "version": 2,
        "generation": generation,
        "expires_at": meerkat_core::persisted_token_expires_at_epoch_secs(tokens),
    });
    if let Some(credential_published_at_millis) = credential_published_at_millis
        && let Some(marker) = marker.as_object_mut()
    {
        marker.insert(
            "credential_published_at_millis".to_string(),
            serde_json::json!(credential_published_at_millis),
        );
    }
    match &mut marked.metadata {
        serde_json::Value::Object(map) => {
            map.insert("meerkat_auth_lifecycle".to_string(), marker);
        }
        serde_json::Value::Null => {
            let mut metadata = serde_json::Map::new();
            metadata.insert("meerkat_auth_lifecycle".to_string(), marker);
            marked.metadata = serde_json::Value::Object(metadata);
        }
        _ => {
            let previous = std::mem::replace(&mut marked.metadata, serde_json::Value::Null);
            let mut metadata = serde_json::Map::new();
            metadata.insert("meerkat_auth_lifecycle".to_string(), marker);
            metadata.insert("meerkat_previous_metadata".to_string(), previous);
            marked.metadata = serde_json::Value::Object(metadata);
        }
    }
    marked
}

struct StaticAuthLeaseHandle {
    expires_at: Option<u64>,
    credential_published_at_millis: Option<u64>,
}

impl StaticAuthLeaseHandle {
    fn valid() -> Arc<Self> {
        Arc::new(Self {
            expires_at: None,
            credential_published_at_millis: None,
        })
    }

    fn valid_for_tokens(tokens: &PersistedTokens) -> Arc<Self> {
        Arc::new(Self {
            expires_at: tokens.expires_at.map(|ts| ts.timestamp().max(0) as u64),
            credential_published_at_millis: Some(1_000),
        })
    }
}

impl AuthLeaseHandle for StaticAuthLeaseHandle {
    fn acquire_lease(
        &self,
        _lease_key: &LeaseKey,
        expires_at: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        Ok(generated_auth_transition_for_test(expires_at))
    }

    fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn begin_refresh(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn complete_refresh(
        &self,
        _lease_key: &LeaseKey,
        new_expires_at: u64,
        _now: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        Ok(generated_auth_transition_for_test(new_expires_at))
    }

    fn refresh_failed(
        &self,
        _lease_key: &LeaseKey,
        _permanent: bool,
    ) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
        AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::Valid),
            expires_at: self.expires_at,
            credential_present: true,
            generation: 1,
            credential_published_at_millis: self.credential_published_at_millis,
        }
    }
}

struct MutableAuthLeaseHandle {
    snapshot: Mutex<AuthLeaseSnapshot>,
}

impl MutableAuthLeaseHandle {
    fn valid_for_tokens(tokens: &PersistedTokens) -> Arc<Self> {
        Arc::new(Self {
            snapshot: Mutex::new(AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: tokens.expires_at.map(|ts| ts.timestamp().max(0) as u64),
                credential_present: true,
                generation: 1,
                credential_published_at_millis: Some(1_000),
            }),
        })
    }
}

impl AuthLeaseHandle for MutableAuthLeaseHandle {
    fn acquire_lease(
        &self,
        _lease_key: &LeaseKey,
        expires_at: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        self.complete_refresh(_lease_key, expires_at, 0)
    }

    fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn begin_refresh(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshot.phase = Some(AuthLeasePhase::Refreshing);
        Ok(())
    }

    fn complete_refresh(
        &self,
        _lease_key: &LeaseKey,
        new_expires_at: u64,
        _now: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshot.phase = Some(AuthLeasePhase::Valid);
        snapshot.expires_at = Some(new_expires_at);
        snapshot.credential_present = true;
        let transition = generated_auth_transition_for_test(new_expires_at);
        snapshot.generation = transition.generation();
        snapshot.credential_published_at_millis = transition.credential_published_at_millis();
        Ok(transition)
    }

    fn refresh_failed(
        &self,
        _lease_key: &LeaseKey,
        _permanent: bool,
    ) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

struct ReauthRequiredAuthLeaseHandle;

impl AuthLeaseHandle for ReauthRequiredAuthLeaseHandle {
    fn acquire_lease(
        &self,
        _lease_key: &LeaseKey,
        _expires_at: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        Err(DslTransitionError::guard_rejected(
            "acquire_lease",
            "reauth required",
        ))
    }

    fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn begin_refresh(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn complete_refresh(
        &self,
        _lease_key: &LeaseKey,
        _new_expires_at: u64,
        _now: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        Err(DslTransitionError::guard_rejected(
            "complete_refresh",
            "reauth required",
        ))
    }

    fn refresh_failed(
        &self,
        _lease_key: &LeaseKey,
        _permanent: bool,
    ) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
        AuthLeaseSnapshot {
            phase: Some(AuthLeasePhase::ReauthRequired),
            expires_at: None,
            credential_present: false,
            generation: 1,
            credential_published_at_millis: None,
        }
    }
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
        scopes: oauth::CODE_ASSIST_SCOPES
            .iter()
            .map(|scope| (*scope).into())
            .collect(),
        account_id: None,
        metadata: serde_json::Value::Null,
    }
}

#[tokio::test]
async fn google_oauth_fresh_token_resolves_with_auth_lifecycle() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = persisted_google_oauth("fresh-google-access");
    store
        .save(
            &TokenKey::parse("dev", "default_code_assist").expect("valid slugs"),
            &mark_tokens_lifecycle_published_for_test(&persisted, 1, Some(1_000)),
        )
        .await
        .unwrap();

    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(StaticAuthLeaseHandle::valid_for_tokens(&persisted));
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
    store
        .save(
            &TokenKey::parse("dev", "default_code_assist").expect("valid slugs"),
            &mark_tokens_lifecycle_published_for_test(&persisted, 1, None),
        )
        .await
        .unwrap();

    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(Arc::new(ReauthRequiredAuthLeaseHandle));
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
        .with_auth_lease_handle(StaticAuthLeaseHandle::valid());
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
        .with_auth_lease_handle(StaticAuthLeaseHandle::valid());
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
    store
        .save(
            &key,
            &mark_tokens_lifecycle_published_for_test(&expired, 1, Some(1_000)),
        )
        .await
        .unwrap();
    let mut refreshed = persisted_google_oauth("refreshed-google-access");
    refreshed.refresh_token = Some("rotated-google-refresh".into());

    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_refresh_coordinator(Arc::new(StaticRefreshCoordinator { tokens: refreshed }))
        .with_auth_lease_handle(MutableAuthLeaseHandle::valid_for_tokens(&expired));
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
    store
        .save(
            &key,
            &mark_tokens_lifecycle_published_for_test(&expired, 1, Some(1_000)),
        )
        .await
        .unwrap();

    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_refresh_coordinator(Arc::new(FailingRefreshCoordinator {
            error: RefreshError::Refresh("google refresh transport failed".into()),
        }))
        .with_auth_lease_handle(MutableAuthLeaseHandle::valid_for_tokens(&expired));
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
    store
        .save(
            &key,
            &mark_tokens_lifecycle_published_for_test(&fresh, 1, Some(1_000)),
        )
        .await
        .unwrap();
    let mut refreshed = persisted_google_oauth("forced-google-access");
    refreshed.refresh_token = Some("forced-google-refresh".into());

    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_refresh_coordinator(Arc::new(StaticRefreshCoordinator { tokens: refreshed }))
        .with_auth_lease_handle(StaticAuthLeaseHandle::valid_for_tokens(&fresh))
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
