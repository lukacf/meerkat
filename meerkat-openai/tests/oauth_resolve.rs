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
    AuthConstraints, AuthProfileConfig, BackendProfileConfig, BindingId, ConnectionRef,
    CredentialSourceSpec, ProviderBindingConfig, RealmConfigSection, RealmConnectionSet, RealmId,
};
use meerkat_llm_core::provider_runtime::{ProviderRuntimeRegistry, ResolverEnvironment};
use meerkat_openai::runtime::oauth as o_oauth;

struct FixedAuthLeaseHandle {
    snapshot: AuthLeaseSnapshot,
}

struct RecordingAuthLeaseHandle {
    snapshot: Mutex<AuthLeaseSnapshot>,
    completed_refreshes: Mutex<Vec<u64>>,
}

struct StaticRefreshCoordinator {
    refreshed: PersistedTokens,
}

struct FailingRefreshCoordinator {
    error: RefreshError,
}

impl FixedAuthLeaseHandle {
    fn reauth_required() -> Self {
        Self {
            snapshot: AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::ReauthRequired),
                expires_at: Some((Utc::now() + ChronoDuration::hours(1)).timestamp() as u64),
                generation: 7,
            },
        }
    }
}

impl RecordingAuthLeaseHandle {
    fn untracked() -> Self {
        Self {
            snapshot: Mutex::new(AuthLeaseSnapshot {
                phase: None,
                expires_at: None,
                generation: 0,
            }),
            completed_refreshes: Mutex::new(Vec::new()),
        }
    }

    fn valid(expires_at: chrono::DateTime<Utc>) -> Self {
        Self {
            snapshot: Mutex::new(AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: Some(expires_at.timestamp() as u64),
                generation: 3,
            }),
            completed_refreshes: Mutex::new(Vec::new()),
        }
    }

    fn completed_refreshes(&self) -> Vec<u64> {
        self.completed_refreshes.lock().unwrap().clone()
    }

    fn phase(&self) -> Option<AuthLeasePhase> {
        self.snapshot.lock().unwrap().phase
    }

    fn replace_snapshot(&self, phase: AuthLeasePhase, expires_at: Option<u64>) {
        let mut snapshot = self.snapshot.lock().unwrap();
        snapshot.phase = Some(phase);
        snapshot.expires_at = expires_at;
        snapshot.generation += 1;
    }
}

impl AuthLeaseHandle for FixedAuthLeaseHandle {
    fn acquire_lease(
        &self,
        _lease_key: &LeaseKey,
        _expires_at: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        panic!("managed OAuth must not reacquire over reauth-required lease truth")
    }

    fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        panic!("managed OAuth must not mark expiring over reauth-required lease truth")
    }

    fn begin_refresh(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        panic!("managed OAuth must not refresh over reauth-required lease truth")
    }

    fn complete_refresh(
        &self,
        _lease_key: &LeaseKey,
        _new_expires_at: u64,
        _now: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        panic!("managed OAuth must not complete refresh over reauth-required lease truth")
    }

    fn refresh_failed(
        &self,
        _lease_key: &LeaseKey,
        _permanent: bool,
    ) -> Result<(), DslTransitionError> {
        panic!("managed OAuth must not report refresh failure over reauth-required lease truth")
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

impl AuthLeaseHandle for RecordingAuthLeaseHandle {
    fn acquire_lease(
        &self,
        _lease_key: &LeaseKey,
        expires_at: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        self.replace_snapshot(AuthLeasePhase::Valid, lease_expires_at_arg(expires_at));
        Ok(AuthLeaseTransition {
            generation: self.snapshot.lock().unwrap().generation,
        })
    }

    fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        let expires_at = self.snapshot.lock().unwrap().expires_at;
        self.replace_snapshot(AuthLeasePhase::Expiring, expires_at);
        Ok(())
    }

    fn begin_refresh(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        let expires_at = self.snapshot.lock().unwrap().expires_at;
        self.replace_snapshot(AuthLeasePhase::Refreshing, expires_at);
        Ok(())
    }

    fn complete_refresh(
        &self,
        _lease_key: &LeaseKey,
        new_expires_at: u64,
        _now: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        self.completed_refreshes
            .lock()
            .unwrap()
            .push(new_expires_at);
        self.replace_snapshot(AuthLeasePhase::Valid, lease_expires_at_arg(new_expires_at));
        Ok(AuthLeaseTransition {
            generation: self.snapshot.lock().unwrap().generation,
        })
    }

    fn refresh_failed(
        &self,
        _lease_key: &LeaseKey,
        permanent: bool,
    ) -> Result<(), DslTransitionError> {
        let phase = if permanent {
            AuthLeasePhase::ReauthRequired
        } else {
            AuthLeasePhase::Expiring
        };
        let expires_at = self.snapshot.lock().unwrap().expires_at;
        self.replace_snapshot(phase, expires_at);
        Ok(())
    }

    fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        let expires_at = self.snapshot.lock().unwrap().expires_at;
        self.replace_snapshot(AuthLeasePhase::ReauthRequired, expires_at);
        Ok(())
    }

    fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        let mut snapshot = self.snapshot.lock().unwrap();
        snapshot.phase = None;
        snapshot.expires_at = None;
        snapshot.generation += 1;
        Ok(())
    }

    fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
        self.snapshot.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl RefreshCoordinator for StaticRefreshCoordinator {
    async fn with_refresh(
        &self,
        _key: TokenKey,
        _refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError> {
        Ok(self.refreshed.clone())
    }
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

fn lease_expires_at_arg(expires_at: u64) -> Option<u64> {
    (expires_at != u64::MAX).then_some(expires_at)
}

fn openai_realm(backend_kind: &str, auth_method: &str) -> RealmConnectionSet {
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
            source: CredentialSourceSpec::PlatformDefault,
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
            &persisted,
        )
        .await
        .unwrap();

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing().with_token_store(store);
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
async fn openai_managed_chatgpt_oauth_reauth_required_does_not_return_cached_token() {
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
            &persisted,
        )
        .await
        .unwrap();

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(Arc::new(FixedAuthLeaseHandle::reauth_required()));
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect_err("AuthMachine reauth-required truth must block cached TokenStore access");

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
                meerkat_core::AuthError::Expired
            )
        ),
        "got {err:?}"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_stale_lease_forces_refresh_and_publishes_new_expiry() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("cached-chatgpt-access".into()),
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
            &persisted,
        )
        .await
        .unwrap();

    let refreshed_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(2)).timestamp(),
        0,
    )
    .unwrap();
    let refreshed = PersistedTokens {
        primary_secret: Some("refreshed-chatgpt-access".into()),
        refresh_token: Some("rt-rotated".into()),
        expires_at: Some(refreshed_expires_at),
        last_refresh: Some(Utc::now()),
        ..persisted.clone()
    };
    let auth_lease = Arc::new(RecordingAuthLeaseHandle::valid(
        Utc::now() - ChronoDuration::minutes(5),
    ));
    let refresh_coord = Arc::new(StaticRefreshCoordinator {
        refreshed: refreshed.clone(),
    });

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_refresh_coordinator(refresh_coord)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("stale AuthMachine lease should refresh managed OAuth token");

    assert_eq!(
        connection.resolved_secret(),
        Some("refreshed-chatgpt-access".to_string()),
    );
    assert_eq!(
        connection.auth_lease.expires_at(),
        Some(refreshed_expires_at),
        "resolved lease must carry the refreshed token expiry"
    );
    assert_eq!(
        auth_lease.completed_refreshes(),
        vec![refreshed_expires_at.timestamp() as u64],
        "AuthMachine complete_refresh must receive the refreshed expiry, not the stale persisted expiry"
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_missing_expiry_remains_authmachine_non_expiring() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("non-expiring-chatgpt-access".into()),
        refresh_token: None,
        id_token: None,
        expires_at: None,
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
            &persisted,
        )
        .await
        .unwrap();

    let auth_lease = Arc::new(RecordingAuthLeaseHandle::untracked());
    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("missing expiry should remain non-expiring AuthMachine truth");

    assert_eq!(
        connection.resolved_secret(),
        Some("non-expiring-chatgpt-access".to_string()),
    );
    assert_eq!(connection.auth_lease.expires_at(), None);
    let snapshot = auth_lease.snapshot(&LeaseKey::from_connection_ref(&default_connection_ref()));
    assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));
    assert_eq!(snapshot.expires_at, None);
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_missing_cached_access_refreshes_when_lease_is_fresh() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: None,
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
            &persisted,
        )
        .await
        .unwrap();

    let refreshed_expires_at = chrono::DateTime::<Utc>::from_timestamp(
        (Utc::now() + ChronoDuration::hours(2)).timestamp(),
        0,
    )
    .unwrap();
    let refreshed = PersistedTokens {
        primary_secret: Some("refreshed-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        expires_at: Some(refreshed_expires_at),
        last_refresh: Some(Utc::now()),
        ..persisted.clone()
    };
    let auth_lease = Arc::new(RecordingAuthLeaseHandle::valid(
        Utc::now() + ChronoDuration::hours(1),
    ));
    let refresh_coord = Arc::new(StaticRefreshCoordinator { refreshed });

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_refresh_coordinator(refresh_coord)
        .with_auth_lease_handle(auth_lease);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("missing cached access should refresh even when lease expiry is fresh");

    assert_eq!(
        connection.resolved_secret(),
        Some("refreshed-chatgpt-access".to_string()),
    );
}

#[tokio::test]
async fn openai_managed_chatgpt_oauth_invalid_grant_marks_reauth_required() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("cached-chatgpt-access".into()),
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
            &persisted,
        )
        .await
        .unwrap();

    let auth_lease = Arc::new(RecordingAuthLeaseHandle::valid(
        Utc::now() - ChronoDuration::minutes(5),
    ));
    let refresh_coord = Arc::new(FailingRefreshCoordinator {
        error: RefreshError::Refresh(
            "token endpoint error: status=400 body={\"error\":\"invalid_grant\"}".into(),
        ),
    });

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_refresh_coordinator(refresh_coord)
        .with_auth_lease_handle(auth_lease.clone());
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect_err("invalid_grant should fail resolution");

    assert!(
        matches!(
            err,
            meerkat_llm_core::provider_runtime::ProviderAuthError::SourceResolutionFailed(_)
        ),
        "got {err:?}"
    );
    assert_eq!(auth_lease.phase(), Some(AuthLeasePhase::ReauthRequired));
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
            &persisted,
        )
        .await
        .unwrap();

    let realm = openai_realm("chatgpt_backend", "external_chatgpt_tokens");
    let env = ResolverEnvironment::testing().with_token_store(store);
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
