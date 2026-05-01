//! Phase 4b — OpenAI ChatGPT + Google Code Assist OAuth resolution
//! through the provider runtime.
//!
//! Covers the same choke-point as the Anthropic test: persisted tokens
//! → resolve returns an inline secret. Also verifies
//! the external_chatgpt_tokens path and the Google api_key_express path
//! (which routes through the simple-secret resolver).

#![cfg(all(not(target_arch = "wasm32"), feature = "oauth",))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use axum::Router;
use axum::extract::Form;
use axum::response::Json;
use axum::routing::post;
use chrono::{Duration as ChronoDuration, Utc};
use tokio::net::TcpListener;

use meerkat_auth_core::auth_oauth::OAuthEndpoints;
use meerkat_auth_core::auth_store::{
    EphemeralTokenStore, PersistedAuthMode, PersistedTokens, RefreshCoordinator, RefreshError,
    RefreshFn, TokenKey, TokenStore,
};
use meerkat_core::handles::{
    AuthLeaseHandle, AuthLeasePhase, AuthLeaseSnapshot, AuthLeaseTransition, DslTransitionError,
    LeaseKey,
};
use meerkat_core::{
    AuthConstraints, AuthProfileConfig, BackendProfileConfig, BindingId, ConnectionRef,
    CredentialSourceSpec, ProviderBindingConfig, RealmConfigSection, RealmConnectionSet, RealmId,
};
use meerkat_llm_core::provider_runtime::{ProviderRuntimeRegistry, ResolverEnvironment};
use meerkat_openai::runtime::oauth as o_oauth;

fn openai_realm(backend_kind: &str, auth_method: &str) -> RealmConnectionSet {
    openai_realm_with_source(
        backend_kind,
        auth_method,
        CredentialSourceSpec::PlatformDefault,
    )
}

fn openai_realm_with_source(
    backend_kind: &str,
    auth_method: &str,
    source: CredentialSourceSpec,
) -> RealmConnectionSet {
    let mut backend = BTreeMap::new();
    backend.insert(
        backend_kind.into(),
        BackendProfileConfig {
            provider: "openai".into(),
            backend_kind: backend_kind.into(),
            base_url: None,
            options: serde_json::json!({"realm_id": "dev"}),
        },
    );
    let mut auth = BTreeMap::new();
    auth.insert(
        "chatgpt_auth".into(),
        AuthProfileConfig {
            provider: "openai".into(),
            auth_method: auth_method.into(),
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
        "default_chatgpt".into(),
        ProviderBindingConfig {
            backend_profile: backend_kind.into(),
            auth_profile: "chatgpt_auth".into(),
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
            default_binding: Some("default_chatgpt".into()),
        },
    )
    .unwrap()
}

fn default_connection_ref() -> ConnectionRef {
    ConnectionRef {
        realm: RealmId::parse("dev").expect("valid realm"),
        binding: BindingId::parse("default_chatgpt").expect("valid binding"),
        profile: None,
    }
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
        _expires_at: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        Ok(AuthLeaseTransition {
            generation: 1,
            credential_published_at_millis: self.credential_published_at_millis,
        })
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
        Ok(AuthLeaseTransition {
            generation: 1,
            credential_published_at_millis: self.credential_published_at_millis,
        })
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

struct BootstrappingAuthLeaseHandle {
    snapshot: Mutex<AuthLeaseSnapshot>,
}

impl BootstrappingAuthLeaseHandle {
    fn empty() -> Arc<Self> {
        Arc::new(Self {
            snapshot: Mutex::new(AuthLeaseSnapshot {
                phase: None,
                expires_at: None,
                credential_present: false,
                generation: 0,
                credential_published_at_millis: None,
            }),
        })
    }
}

impl AuthLeaseHandle for BootstrappingAuthLeaseHandle {
    fn acquire_lease(
        &self,
        _lease_key: &LeaseKey,
        expires_at: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshot.phase = Some(AuthLeasePhase::Valid);
        snapshot.expires_at = (expires_at != u64::MAX).then_some(expires_at);
        snapshot.credential_present = true;
        snapshot.generation = snapshot.generation.saturating_add(1);
        Ok(AuthLeaseTransition {
            generation: snapshot.generation,
            credential_published_at_millis: None,
        })
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
        self.acquire_lease(_lease_key, new_expires_at)
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
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snapshot.phase = None;
        snapshot.expires_at = None;
        snapshot.credential_present = false;
        snapshot.generation = snapshot.generation.saturating_add(1);
        Ok(())
    }

    fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

struct RejectingAuthLeaseHandle {
    snapshot: AuthLeaseSnapshot,
}

impl RejectingAuthLeaseHandle {
    fn expired(expires_at: chrono::DateTime<Utc>) -> Arc<Self> {
        Arc::new(Self {
            snapshot: AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: Some(expires_at.timestamp().max(0) as u64),
                credential_present: true,
                generation: 1,
                credential_published_at_millis: None,
            },
        })
    }
}

impl AuthLeaseHandle for RejectingAuthLeaseHandle {
    fn acquire_lease(
        &self,
        _lease_key: &LeaseKey,
        _expires_at: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        Err(DslTransitionError::guard_rejected(
            "acquire_lease",
            "test rejected lifecycle publication",
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
            "test rejected lifecycle publication",
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
        self.snapshot.clone()
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

struct StoreUpdatingRefreshCoordinator {
    store: Arc<dyn TokenStore>,
    key: TokenKey,
    tokens: PersistedTokens,
}

#[async_trait::async_trait]
impl RefreshCoordinator for StoreUpdatingRefreshCoordinator {
    async fn with_refresh(
        &self,
        _key: TokenKey,
        refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError> {
        self.store
            .save(&self.key, &self.tokens)
            .await
            .map_err(|e| RefreshError::Refresh(e.to_string()))?;
        refresh_fn().await
    }
}

struct CommitObservingRefreshCoordinator {
    store: Arc<dyn TokenStore>,
}

#[async_trait::async_trait]
impl RefreshCoordinator for CommitObservingRefreshCoordinator {
    async fn with_refresh(
        &self,
        key: TokenKey,
        refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError> {
        let refreshed = refresh_fn().await?;
        let stored = self
            .store
            .load(&key)
            .await
            .map_err(|e| RefreshError::Refresh(e.to_string()))?
            .ok_or_else(|| RefreshError::Refresh("TokenStore missing after refresh".into()))?;
        if stored.primary_secret != refreshed.primary_secret
            || !meerkat_core::tokens_lifecycle_published(&stored)
        {
            return Err(RefreshError::Refresh(
                "refresh commit escaped coordinator lock".into(),
            ));
        }
        Ok(refreshed)
    }
}

// --- OpenAI managed_chatgpt_oauth ------------------------------------

#[tokio::test]
async fn openai_managed_chatgpt_oauth_fresh_token_resolves() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("fresh-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &meerkat_core::mark_tokens_lifecycle_published_for_transition(
                &persisted,
                AuthLeaseTransition {
                    generation: 1,
                    credential_published_at_millis: Some(1_000),
                },
            ),
        )
        .await
        .unwrap();

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(StaticAuthLeaseHandle::valid_for_tokens(&persisted));
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("fresh ChatGPT tokens should resolve");
    assert_eq!(
        connection.resolved_secret(),
        Some("fresh-chatgpt-access".to_string()),
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_rejects_token_without_auth_lifecycle() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("stale-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &meerkat_core::mark_tokens_lifecycle_published_for_generation(&persisted, 1),
        )
        .await
        .unwrap();

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing().with_token_store(store);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::InteractiveLoginRequired
            )
        ),
        "got {err:?}"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_rehydrates_empty_auth_lifecycle_with_persisted_token() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("fresh-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &meerkat_core::mark_tokens_lifecycle_published_for_generation(&persisted, 1),
        )
        .await
        .unwrap();

    let auth_lease = BootstrappingAuthLeaseHandle::empty();
    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("fresh persisted OAuth tokens should rehydrate AuthMachine after restart");
    assert_eq!(
        connection.resolved_secret(),
        Some("fresh-chatgpt-access".to_string())
    );
    let snapshot = auth_lease.snapshot(&LeaseKey::from_connection_ref(&default_connection_ref()));
    assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
    assert!(snapshot.credential_present);
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_rejects_unmarked_token_after_empty_lifecycle_restart() {
    let store = Arc::new(EphemeralTokenStore::new());
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &PersistedTokens {
                auth_mode: PersistedAuthMode::ChatgptOauth,
                primary_secret: Some("stale-chatgpt-access".into()),
                refresh_token: Some("rt".into()),
                id_token: None,
                expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
                last_refresh: Some(Utc::now()),
                scopes: o_oauth::CHATGPT_SCOPES
                    .iter()
                    .map(|s| (*s).into())
                    .collect(),
                account_id: Some("acct-1".into()),
                metadata: serde_json::Value::Null,
            },
        )
        .await
        .unwrap();

    let auth_lease = BootstrappingAuthLeaseHandle::empty();
    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::InteractiveLoginRequired
            )
        ),
        "got {err:?}"
    );
    let snapshot = auth_lease.snapshot(&LeaseKey::from_connection_ref(&default_connection_ref()));
    assert_eq!(snapshot.phase, None);
    assert!(!snapshot.credential_present);
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_rehydrates_expired_empty_auth_lifecycle_before_refresh() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("expired-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() - ChronoDuration::minutes(5)),
        last_refresh: Some(Utc::now() - ChronoDuration::hours(1)),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &meerkat_core::mark_tokens_lifecycle_published_for_generation(&persisted, 1),
        )
        .await
        .unwrap();

    let refreshed = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("refreshed-chatgpt-access".into()),
        refresh_token: Some("rotated-rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let auth_lease = BootstrappingAuthLeaseHandle::empty();
    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_refresh_coordinator(Arc::new(StaticRefreshCoordinator {
            tokens: refreshed.clone(),
        }))
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("expired persisted OAuth tokens should rehydrate then refresh after restart");
    assert_eq!(
        connection.resolved_secret(),
        Some("refreshed-chatgpt-access".to_string())
    );
    let snapshot = auth_lease.snapshot(&LeaseKey::from_connection_ref(&default_connection_ref()));
    assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
    assert!(snapshot.credential_present);
    let stored = store
        .load(&TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.primary_secret.as_deref(),
        Some("refreshed-chatgpt-access")
    );
    assert!(meerkat_core::tokens_lifecycle_published(&stored));
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_rejects_wrong_persisted_mode() {
    let store = Arc::new(EphemeralTokenStore::new());
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &PersistedTokens {
                auth_mode: PersistedAuthMode::ApiKey,
                primary_secret: Some("stale-api-key-on-chatgpt-binding".into()),
                refresh_token: Some("rt".into()),
                id_token: None,
                expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
                last_refresh: Some(Utc::now()),
                scopes: vec![],
                account_id: Some("acct-1".into()),
                metadata: serde_json::Value::Null,
            },
        )
        .await
        .unwrap();

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(StaticAuthLeaseHandle::valid());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
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
async fn openai_managed_chatgpt_oauth_rejects_wrong_source_even_with_matching_mode() {
    let store = Arc::new(EphemeralTokenStore::new());
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &PersistedTokens {
                auth_mode: PersistedAuthMode::ChatgptOauth,
                primary_secret: Some("fresh-chatgpt-access".into()),
                refresh_token: Some("rt".into()),
                id_token: None,
                expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
                last_refresh: Some(Utc::now()),
                scopes: o_oauth::CHATGPT_SCOPES
                    .iter()
                    .map(|s| (*s).into())
                    .collect(),
                account_id: Some("acct-1".into()),
                metadata: serde_json::Value::Null,
            },
        )
        .await
        .unwrap();

    let realm = openai_realm_with_source(
        "chatgpt_backend",
        "managed_chatgpt_oauth",
        CredentialSourceSpec::ExternalResolver {
            handle: "external-chatgpt".into(),
        },
    );
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(StaticAuthLeaseHandle::valid());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
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
async fn openai_chatgpt_oauth_runtime_refresh_is_uncommitted() {
    let app = Router::new().route(
        "/oauth/token",
        post(|Form(_form): Form<serde_json::Value>| async {
            Json(serde_json::json!({
                "access_token": "refreshed-chatgpt-access",
                "refresh_token": "rotated-chatgpt-refresh",
                "expires_in": 3600,
                "token_type": "Bearer",
                "scope": "openid profile email offline_access",
            }))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let key = TokenKey::parse("dev", "default_chatgpt").expect("valid slugs");
    let store = Arc::new(EphemeralTokenStore::new());
    let old_expiry = Utc::now() - ChronoDuration::minutes(5);
    store
        .save(
            &key,
            &PersistedTokens {
                auth_mode: PersistedAuthMode::ChatgptOauth,
                primary_secret: Some("expired-chatgpt-access".into()),
                refresh_token: Some("refresh-chatgpt".into()),
                id_token: None,
                expires_at: Some(old_expiry),
                last_refresh: Some(Utc::now() - ChronoDuration::hours(2)),
                scopes: vec![],
                account_id: Some("acct-1".into()),
                metadata: serde_json::Value::Null,
            },
        )
        .await
        .unwrap();

    let endpoints = OAuthEndpoints {
        client_id: o_oauth::CHATGPT_CLIENT_ID.into(),
        authorize_url: o_oauth::CHATGPT_AUTHORIZE_URL.into(),
        token_url: format!("http://{addr}/oauth/token"),
        device_code_url: None,
        redirect_uri: "http://127.0.0.1:0/callback".into(),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|scope| (*scope).into())
            .collect(),
        extra_headers: vec![],
    };
    let runtime =
        o_oauth::OpenAiOAuthRuntime::new_with_default_coordinator(store.clone(), endpoints, key);
    let before_refresh = Utc::now();
    let refreshed = runtime.get_or_refresh_tokens().await.unwrap();

    assert_eq!(
        refreshed.primary_secret.as_deref(),
        Some("refreshed-chatgpt-access")
    );
    assert_eq!(
        refreshed.refresh_token.as_deref(),
        Some("rotated-chatgpt-refresh")
    );
    assert!(
        refreshed.expires_at.expect("refreshed expiry")
            > before_refresh + ChronoDuration::minutes(50),
        "refreshed lease expiry must be the new provider expiry, not the old expired value"
    );
    let stored = store.load(runtime.key()).await.unwrap().unwrap();
    assert_eq!(
        stored.primary_secret.as_deref(),
        Some("expired-chatgpt-access")
    );
    assert_eq!(stored.expires_at, Some(old_expiry));
    assert!(!meerkat_core::tokens_lifecycle_published(&stored));
}

#[tokio::test]
async fn openai_chatgpt_oauth_runtime_reloads_store_after_refresh_coordination() {
    let calls = Arc::new(AtomicUsize::new(0));
    let endpoint_calls = Arc::clone(&calls);
    let app = Router::new().route(
        "/oauth/token",
        post(move |Form(_form): Form<serde_json::Value>| {
            let endpoint_calls = Arc::clone(&endpoint_calls);
            async move {
                endpoint_calls.fetch_add(1, Ordering::SeqCst);
                Json(serde_json::json!({
                    "access_token": "unexpected-provider-refresh",
                    "refresh_token": "unexpected-rt",
                    "expires_in": 3600,
                    "token_type": "Bearer",
                    "scope": "openid profile email offline_access",
                }))
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let key = TokenKey::parse("dev", "default_chatgpt").expect("valid slugs");
    let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
    store
        .save(
            &key,
            &PersistedTokens {
                auth_mode: PersistedAuthMode::ChatgptOauth,
                primary_secret: Some("expired-chatgpt-access".into()),
                refresh_token: Some("stale-rt".into()),
                id_token: None,
                expires_at: Some(Utc::now() - ChronoDuration::minutes(5)),
                last_refresh: Some(Utc::now() - ChronoDuration::hours(1)),
                scopes: vec![],
                account_id: Some("acct-1".into()),
                metadata: serde_json::Value::Null,
            },
        )
        .await
        .unwrap();
    let already_committed = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("already-committed-access".into()),
        refresh_token: Some("rotated-rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: vec![],
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let coord = Arc::new(StoreUpdatingRefreshCoordinator {
        store: Arc::clone(&store),
        key: key.clone(),
        tokens: already_committed.clone(),
    });
    let endpoints = OAuthEndpoints {
        client_id: o_oauth::CHATGPT_CLIENT_ID.into(),
        authorize_url: o_oauth::CHATGPT_AUTHORIZE_URL.into(),
        token_url: format!("http://{addr}/oauth/token"),
        device_code_url: None,
        redirect_uri: "http://127.0.0.1:0/callback".into(),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|scope| (*scope).into())
            .collect(),
        extra_headers: vec![],
    };
    let runtime = o_oauth::OpenAiOAuthRuntime::new(store, coord, endpoints, key);

    let resolved = runtime.get_or_refresh_tokens_uncommitted().await.unwrap();

    assert_eq!(resolved, already_committed);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "runtime should adopt already committed fresh tokens after refresh coordination"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_commits_before_refresh_coordinator_returns() {
    let app = Router::new().route(
        "/oauth/token",
        post(move |Form(_form): Form<serde_json::Value>| async move {
            Json(serde_json::json!({
                "access_token": "coordinated-refreshed-access",
                "refresh_token": "coordinated-rotated-rt",
                "expires_in": 3600,
                "token_type": "Bearer",
                "scope": "openid profile email offline_access",
            }))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let key = TokenKey::parse("dev", "default_chatgpt").expect("valid slugs");
    let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
    let expired = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("expired-chatgpt-access".into()),
        refresh_token: Some("refresh-chatgpt".into()),
        id_token: None,
        expires_at: Some(Utc::now() - ChronoDuration::minutes(5)),
        last_refresh: Some(Utc::now() - ChronoDuration::hours(1)),
        scopes: vec![],
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &key,
            &meerkat_core::mark_tokens_lifecycle_published_for_generation(&expired, 1),
        )
        .await
        .unwrap();

    let endpoints = OAuthEndpoints {
        client_id: o_oauth::CHATGPT_CLIENT_ID.into(),
        authorize_url: o_oauth::CHATGPT_AUTHORIZE_URL.into(),
        token_url: format!("http://{addr}/oauth/token"),
        device_code_url: None,
        redirect_uri: "http://127.0.0.1:0/callback".into(),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|scope| (*scope).into())
            .collect(),
        extra_headers: vec![],
    };
    let runtime = o_oauth::OpenAiOAuthRuntime::new(
        Arc::clone(&store),
        Arc::new(CommitObservingRefreshCoordinator {
            store: Arc::clone(&store),
        }),
        endpoints,
        key.clone(),
    );
    let commit_store = Arc::clone(&store);
    let commit_key = key.clone();
    let resolved = runtime
        .get_or_refresh_tokens_with_commit(Box::new(move |tokens| {
            Box::pin(async move {
                let committed = meerkat_core::mark_tokens_lifecycle_published(&tokens);
                commit_store
                    .save(&commit_key, &committed)
                    .await
                    .map_err(|e| RefreshError::Refresh(e.to_string()))?;
                Ok(committed)
            })
        }))
        .await
        .expect("refresh commit should complete before coordinator releases its lock");

    assert_eq!(
        resolved.primary_secret.as_deref(),
        Some("coordinated-refreshed-access")
    );
    let stored = store.load(&key).await.unwrap().unwrap();
    assert_eq!(
        stored.primary_secret.as_deref(),
        Some("coordinated-refreshed-access")
    );
    assert!(meerkat_core::tokens_lifecycle_published(&stored));
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_commit_mode_marks_fresh_cached_tokens() {
    let key = TokenKey::parse("dev", "default_chatgpt").expect("valid slugs");
    let store: Arc<dyn TokenStore> = Arc::new(EphemeralTokenStore::new());
    let fresh = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("fresh-chatgpt-access".into()),
        refresh_token: Some("fresh-chatgpt-refresh".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: vec![],
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store.save(&key, &fresh).await.unwrap();

    let endpoints = OAuthEndpoints {
        client_id: o_oauth::CHATGPT_CLIENT_ID.into(),
        authorize_url: o_oauth::CHATGPT_AUTHORIZE_URL.into(),
        token_url: "http://127.0.0.1:9/oauth/token".into(),
        device_code_url: None,
        redirect_uri: "http://127.0.0.1:0/callback".into(),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|scope| (*scope).into())
            .collect(),
        extra_headers: vec![],
    };
    let commits = Arc::new(AtomicUsize::new(0));
    let runtime = o_oauth::OpenAiOAuthRuntime::new(
        Arc::clone(&store),
        Arc::new(meerkat_auth_core::auth_store::InMemoryCoordinator::new()),
        endpoints,
        key.clone(),
    );
    let commit_store = Arc::clone(&store);
    let commit_key = key.clone();
    let commit_counter = Arc::clone(&commits);
    let resolved = runtime
        .get_or_refresh_tokens_with_commit(Box::new(move |tokens| {
            Box::pin(async move {
                commit_counter.fetch_add(1, Ordering::SeqCst);
                let committed = meerkat_core::mark_tokens_lifecycle_published(&tokens);
                commit_store
                    .save(&commit_key, &committed)
                    .await
                    .map_err(|e| RefreshError::Refresh(e.to_string()))?;
                Ok(committed)
            })
        }))
        .await
        .expect("fresh shared tokens must still pass through the commit callback");

    assert_eq!(commits.load(Ordering::SeqCst), 1);
    assert_eq!(
        resolved.primary_secret.as_deref(),
        Some("fresh-chatgpt-access")
    );
    let stored = store.load(&key).await.unwrap().unwrap();
    assert!(meerkat_core::tokens_lifecycle_published(&stored));
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_refresh_restores_tokens_when_lifecycle_publication_fails() {
    let store = Arc::new(EphemeralTokenStore::new());
    let old_expiry = Utc::now() - ChronoDuration::minutes(5);
    let old_tokens = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("expired-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(old_expiry),
        last_refresh: Some(Utc::now() - ChronoDuration::hours(1)),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let key = TokenKey::parse("dev", "default_chatgpt").expect("valid slugs");
    store
        .save(
            &key,
            &meerkat_core::mark_tokens_lifecycle_published_for_generation(&old_tokens, 1),
        )
        .await
        .unwrap();

    let refreshed = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("refreshed-chatgpt-access".into()),
        refresh_token: Some("rotated-rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::CHATGPT_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_refresh_coordinator(Arc::new(StaticRefreshCoordinator { tokens: refreshed }))
        .with_auth_lease_handle(RejectingAuthLeaseHandle::expired(old_expiry));
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("AuthMachine lifecycle acquire failed"),
        "got {err}"
    );
    let stored = store.load(&key).await.unwrap().unwrap();
    assert_eq!(stored.primary_secret, old_tokens.primary_secret);
    assert_eq!(stored.refresh_token, old_tokens.refresh_token);
    assert_eq!(stored.expires_at, old_tokens.expires_at);
}

#[tokio::test]
async fn openai_external_chatgpt_tokens_returns_persisted_access() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ExternalTokens,
        primary_secret: Some("externally-managed-access".into()),
        refresh_token: None,
        id_token: None,
        expires_at: None,
        last_refresh: Some(Utc::now()),
        scopes: vec![],
        account_id: Some("acct-ext".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &meerkat_core::mark_tokens_lifecycle_published_for_generation(&persisted, 1),
        )
        .await
        .unwrap();

    let realm = openai_realm("chatgpt_backend", "external_chatgpt_tokens");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(StaticAuthLeaseHandle::valid());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("external tokens should resolve");
    assert_eq!(
        connection.resolved_secret(),
        Some("externally-managed-access".to_string()),
    );
}

#[tokio::test]
async fn openai_external_chatgpt_tokens_rehydrates_empty_lifecycle_as_expired() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ExternalTokens,
        primary_secret: Some("expired-externally-managed-access".into()),
        refresh_token: None,
        id_token: None,
        expires_at: Some(Utc::now() - ChronoDuration::minutes(5)),
        last_refresh: Some(Utc::now() - ChronoDuration::hours(1)),
        scopes: vec![],
        account_id: Some("acct-ext".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &meerkat_core::mark_tokens_lifecycle_published_for_generation(&persisted, 1),
        )
        .await
        .unwrap();

    let auth_lease = BootstrappingAuthLeaseHandle::empty();
    let realm = openai_realm("chatgpt_backend", "external_chatgpt_tokens");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::Expired
            )
        ),
        "got {err:?}"
    );
    let snapshot = auth_lease.snapshot(&LeaseKey::from_connection_ref(&default_connection_ref()));
    assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
    assert!(snapshot.credential_present);
}

#[tokio::test]
async fn openai_external_chatgpt_tokens_rejects_chatgpt_oauth_mode() {
    let store = Arc::new(EphemeralTokenStore::new());
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &PersistedTokens {
                auth_mode: PersistedAuthMode::ChatgptOauth,
                primary_secret: Some("managed-chatgpt-access".into()),
                refresh_token: Some("rt".into()),
                id_token: None,
                expires_at: None,
                last_refresh: Some(Utc::now()),
                scopes: vec![],
                account_id: Some("acct-managed".into()),
                metadata: serde_json::Value::Null,
            },
        )
        .await
        .unwrap();

    let realm = openai_realm("chatgpt_backend", "external_chatgpt_tokens");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(StaticAuthLeaseHandle::valid());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
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
        err.to_string().contains("credential mode ChatgptOauth"),
        "got {err}"
    );
}

#[tokio::test]
async fn openai_chatgpt_oauth_missing_tokens_surfaces_interactive_login_required() {
    let store = Arc::new(EphemeralTokenStore::new());
    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing().with_token_store(store);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::InteractiveLoginRequired
            )
        ),
        "got {err:?}"
    );
}
