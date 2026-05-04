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

fn default_auth_binding() -> AuthBindingRef {
    AuthBindingRef {
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
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .expect("fresh ChatGPT tokens should resolve");
    assert_eq!(
        connection.resolved_secret(),
        Some("fresh-chatgpt-access".to_string()),
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_force_refresh_bypasses_fresh_token() {
    let store = Arc::new(EphemeralTokenStore::new());
    let key = TokenKey::parse("dev", "default_chatgpt").expect("valid slugs");
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
            &key,
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
    let refreshed = PersistedTokens {
        primary_secret: Some("forced-refresh-chatgpt-access".into()),
        refresh_token: Some("rotated-rt".into()),
        last_refresh: Some(Utc::now()),
        ..persisted.clone()
    };

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_refresh_coordinator(Arc::new(StaticRefreshCoordinator { tokens: refreshed }))
        .with_auth_lease_handle(StaticAuthLeaseHandle::valid_for_tokens(&persisted))
        .with_force_refresh(true);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    let connection = registry
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .expect("forced refresh should resolve through OAuth refresh path");

    assert_eq!(
        connection.resolved_secret(),
        Some("forced-refresh-chatgpt-access".to_string())
    );
    let stored = store.load(&key).await.unwrap().unwrap();
    assert_eq!(
        stored.primary_secret.as_deref(),
        Some("forced-refresh-chatgpt-access")
    );
    assert!(meerkat_core::tokens_lifecycle_published(&stored));
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_refresh_failure_is_typed() {
    let store = Arc::new(EphemeralTokenStore::new());
    let key = TokenKey::parse("dev", "default_chatgpt").expect("valid slugs");
    let expired = PersistedTokens {
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
            &key,
            &meerkat_core::mark_tokens_lifecycle_published_for_transition(
                &expired,
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
        .with_refresh_coordinator(Arc::new(FailingRefreshCoordinator {
            error: RefreshError::Refresh("temporary refresh outage".into()),
        }))
        .with_auth_lease_handle(StaticAuthLeaseHandle::valid_for_tokens(&expired));
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .unwrap_err();
    match err {
        meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
            meerkat_core::AuthError::RefreshFailed(detail),
        ) => assert!(detail.contains("temporary refresh outage"), "got {detail}"),
        other => panic!("got {other:?}"),
    }
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_rejects_marked_token_without_auth_lease() {
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
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::LeaseAbsent
            )
        ),
        "got {err:?}"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_rejects_marker_with_empty_auth_lifecycle() {
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
    let err = registry
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::LeaseAbsent
            )
        ),
        "got {err:?}"
    );
    let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&default_auth_binding()));
    assert_eq!(snapshot.phase, None);
    assert!(!snapshot.credential_present);
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
        .resolve(&realm, &default_auth_binding(), &env)
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
    let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&default_auth_binding()));
    assert_eq!(snapshot.phase, None);
    assert!(!snapshot.credential_present);
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_rejects_expired_marker_with_empty_auth_lifecycle() {
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
    let err = registry
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::LeaseAbsent
            )
        ),
        "got {err:?}"
    );
    let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&default_auth_binding()));
    assert_eq!(snapshot.phase, None);
    assert!(!snapshot.credential_present);
    let stored = store
        .load(&TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.primary_secret.as_deref(),
        Some("expired-chatgpt-access")
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
        .resolve(&realm, &default_auth_binding(), &env)
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
        .resolve(&realm, &default_auth_binding(), &env)
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
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("AuthMachine lifecycle complete_refresh failed"),
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
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .expect("external tokens should resolve");
    assert_eq!(
        connection.resolved_secret(),
        Some("externally-managed-access".to_string()),
    );
}

#[tokio::test]
async fn openai_external_chatgpt_tokens_empty_lifecycle_requires_auth_lease() {
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
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::LeaseAbsent
            )
        ),
        "got {err:?}"
    );
    let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&default_auth_binding()));
    assert_eq!(snapshot.phase, None);
    assert!(!snapshot.credential_present);
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
        .resolve(&realm, &default_auth_binding(), &env)
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
        .resolve(&realm, &default_auth_binding(), &env)
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
