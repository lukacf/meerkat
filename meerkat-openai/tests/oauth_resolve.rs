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
use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};

use meerkat_auth_core::auth_store::{
    CredentialMutationError, CredentialMutationFn, EphemeralTokenStore, PersistedAuthMode,
    PersistedTokens, RefreshCoordinator, RefreshError, RefreshFn, TokenKey, TokenStore,
};
use meerkat_core::handles::{
    AuthLeaseHandle, AuthLeaseTransition, GeneratedAuthLeaseHandle, LeaseKey,
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

fn openai_realm_no_refresh(backend_kind: &str, auth_method: &str) -> RealmConnectionSet {
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
                allow_refresh: false,
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
            provider_default: false,
        },
    );
    RealmConnectionSet::from_config(
        "dev",
        &RealmConfigSection {
            backend,
            auth,
            binding,
            default_binding: Some("default_chatgpt".into()),
            parent: None,
        },
    )
    .unwrap()
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
            provider_default: false,
        },
    );
    RealmConnectionSet::from_config(
        "dev",
        &RealmConfigSection {
            backend,
            auth,
            binding,
            default_binding: Some("default_chatgpt".into()),
            parent: None,
        },
    )
    .unwrap()
}

fn default_auth_binding() -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse("dev").expect("valid realm"),
        binding: BindingId::parse("default_chatgpt").expect("valid binding"),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    }
}

fn fixture_refresh_observation_time(expires_at: u64) -> u64 {
    if expires_at == u64::MAX {
        0
    } else {
        expires_at.saturating_sub(1)
    }
}

fn auth_lease_transition_for_test(
    tokens: &PersistedTokens,
    generation: u64,
) -> (AuthLeaseTransition, GeneratedAuthLeaseHandle) {
    let lease_key = LeaseKey::from_auth_binding(&default_auth_binding());
    let expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(tokens);
    let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    let mut transition = handle
        .acquire_lease(&lease_key, expires_at)
        .expect("fixture AuthMachine accepts acquired lease");
    let target_generation = generation.max(1);
    while transition.generation() < target_generation {
        handle
            .begin_refresh(&lease_key)
            .expect("fixture AuthMachine accepts refresh start");
        transition = handle
            .complete_refresh(
                &lease_key,
                expires_at,
                fixture_refresh_observation_time(expires_at),
            )
            .expect("fixture AuthMachine accepts refresh completion");
    }
    let generated = generated_auth_lease_handle_for_test(handle);
    (transition, generated)
}

fn mark_tokens_and_auth_lease_handle_for_test(
    tokens: &PersistedTokens,
    generation: u64,
    credential_published_after_millis: Option<u64>,
) -> (PersistedTokens, GeneratedAuthLeaseHandle) {
    let key = TokenKey::from_auth_binding(&default_auth_binding());
    for _ in 0..100 {
        let (transition, generated) = auth_lease_transition_for_test(tokens, generation);
        let publication_time_matches = match credential_published_after_millis {
            Some(after) => transition
                .credential_published_at_millis()
                .is_some_and(|published| published > after),
            None => true,
        };
        if publication_time_matches {
            let marked = meerkat_core::mark_tokens_lifecycle_published_for_transition(
                &key,
                tokens,
                &transition,
            )
            .expect("runtime AuthMachine transition marks fixture tokens");
            return (marked, generated);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("fixture AuthMachine publication clock did not advance");
}

fn mark_tokens_lifecycle_published_for_test(
    tokens: &PersistedTokens,
    generation: u64,
    credential_published_at_millis: Option<u64>,
) -> PersistedTokens {
    mark_tokens_and_auth_lease_handle_for_test(tokens, generation, credential_published_at_millis).0
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

fn valid_auth_lease_handle_for_expires_at(expires_at: u64) -> GeneratedAuthLeaseHandle {
    let lease_key = LeaseKey::from_auth_binding(&default_auth_binding());
    let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    handle
        .acquire_lease(&lease_key, expires_at)
        .expect("fixture AuthMachine accepts acquired lease");
    generated_auth_lease_handle_for_test(handle)
}

fn empty_auth_lease_handle_for_test() -> (
    Arc<meerkat_runtime::RuntimeAuthLeaseHandle>,
    GeneratedAuthLeaseHandle,
) {
    let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    let generated = generated_auth_lease_handle_for_test(Arc::clone(&handle));
    (handle, generated)
}

struct StaticRefreshCoordinator {
    tokens: PersistedTokens,
}

#[async_trait::async_trait]
impl RefreshCoordinator for StaticRefreshCoordinator {
    async fn with_exclusive_mutation(
        &self,
        _key: TokenKey,
        mutation_fn: CredentialMutationFn,
    ) -> Result<PersistedTokens, CredentialMutationError> {
        mutation_fn().await
    }

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
    async fn with_exclusive_mutation(
        &self,
        _key: TokenKey,
        mutation_fn: CredentialMutationFn,
    ) -> Result<PersistedTokens, CredentialMutationError> {
        mutation_fn().await
    }

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
        scopes: o_oauth::chatgpt_declaration()
            .scopes
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let (persisted, auth_lease) =
        mark_tokens_and_auth_lease_handle_for_test(&persisted, 1, Some(1_000));
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
        .with_auth_lease_handle(auth_lease);
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
        scopes: o_oauth::chatgpt_declaration()
            .scopes
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let (marked_persisted, auth_lease) =
        mark_tokens_and_auth_lease_handle_for_test(&persisted, 1, Some(1_000));
    store.save(&key, &marked_persisted).await.unwrap();
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
        .with_auth_lease_handle(auth_lease)
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
async fn openai_managed_chatgpt_oauth_force_refresh_with_refresh_disallowed_errors_refresh_required()
 {
    // FOLD 2: fresh credential + force_refresh forces the refresh branch, but
    // the binding disallows silent refresh -> AuthMachine emits RefreshDisallowed
    // and the provider mirrors the RefreshRequired error.
    let store = Arc::new(EphemeralTokenStore::new());
    let key = TokenKey::parse("dev", "default_chatgpt").expect("valid slugs");
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("fresh-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::chatgpt_declaration()
            .scopes
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let (marked_persisted, auth_lease) =
        mark_tokens_and_auth_lease_handle_for_test(&persisted, 1, Some(1_000));
    store.save(&key, &marked_persisted).await.unwrap();

    let realm = openai_realm_no_refresh("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease)
        .with_force_refresh(true);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    let err = registry
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .unwrap_err();
    match err {
        meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
            meerkat_core::AuthError::RefreshRequired,
        ) => {}
        other => panic!("expected RefreshRequired, got {other:?}"),
    }
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
        scopes: o_oauth::chatgpt_declaration()
            .scopes
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let (expired, auth_lease) =
        mark_tokens_and_auth_lease_handle_for_test(&expired, 1, Some(1_000));
    store.save(&key, &expired).await.unwrap();

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_refresh_coordinator(Arc::new(FailingRefreshCoordinator {
            error: RefreshError::Refresh("temporary refresh outage".into()),
        }))
        .with_auth_lease_handle(auth_lease);
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
        scopes: o_oauth::chatgpt_declaration()
            .scopes
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &mark_tokens_lifecycle_published_for_test(&persisted, 1, None),
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
async fn openai_managed_chatgpt_oauth_restores_marker_with_empty_auth_lifecycle() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("fresh-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::chatgpt_declaration()
            .scopes
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &mark_tokens_lifecycle_published_for_test(&persisted, 1, None),
        )
        .await
        .unwrap();

    let (auth_lease, generated_auth_lease) = empty_auth_lease_handle_for_test();
    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(generated_auth_lease);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    let connection = registry
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .expect("marked tokens should restore through generated AuthMachine authority");
    assert_eq!(
        connection.resolved_secret(),
        Some("fresh-chatgpt-access".to_string())
    );
    let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&default_auth_binding()));
    assert_eq!(
        snapshot.phase,
        Some(meerkat_core::handles::AuthLeasePhase::Valid)
    );
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
                scopes: o_oauth::chatgpt_declaration()
                    .scopes
                    .iter()
                    .map(|s| (*s).into())
                    .collect(),
                account_id: Some("acct-1".into()),
                metadata: serde_json::Value::Null,
            },
        )
        .await
        .unwrap();

    let (auth_lease, generated_auth_lease) = empty_auth_lease_handle_for_test();
    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(generated_auth_lease);
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
async fn openai_managed_chatgpt_oauth_refreshes_expired_marker_with_empty_auth_lifecycle() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("expired-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() - ChronoDuration::minutes(5)),
        last_refresh: Some(Utc::now() - ChronoDuration::hours(1)),
        scopes: o_oauth::chatgpt_declaration()
            .scopes
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &mark_tokens_lifecycle_published_for_test(&persisted, 1, None),
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
        scopes: o_oauth::chatgpt_declaration()
            .scopes
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let (auth_lease, generated_auth_lease) = empty_auth_lease_handle_for_test();
    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_refresh_coordinator(Arc::new(StaticRefreshCoordinator {
            tokens: refreshed.clone(),
        }))
        .with_auth_lease_handle(generated_auth_lease);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));
    let connection = registry
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .expect("expired marked tokens should restore then refresh through AuthMachine authority");
    assert_eq!(
        connection.resolved_secret(),
        Some("refreshed-chatgpt-access".to_string())
    );
    let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&default_auth_binding()));
    assert_eq!(
        snapshot.phase,
        Some(meerkat_core::handles::AuthLeasePhase::Valid)
    );
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
    assert_eq!(stored.refresh_token.as_deref(), Some("rotated-rt"));
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
        .with_auth_lease_handle(valid_auth_lease_handle());
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
                scopes: o_oauth::chatgpt_declaration()
                    .scopes
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
        .with_auth_lease_handle(valid_auth_lease_handle());
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
async fn openai_managed_chatgpt_oauth_refresh_publishes_through_generated_auth_lifecycle() {
    let store = Arc::new(EphemeralTokenStore::new());
    let old_expiry = Utc::now() - ChronoDuration::minutes(5);
    let old_tokens = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("expired-chatgpt-access".into()),
        refresh_token: Some("rt".into()),
        id_token: None,
        expires_at: Some(old_expiry),
        last_refresh: Some(Utc::now() - ChronoDuration::hours(1)),
        scopes: o_oauth::chatgpt_declaration()
            .scopes
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let key = TokenKey::parse("dev", "default_chatgpt").expect("valid slugs");
    let (marked_old_tokens, auth_lease) =
        mark_tokens_and_auth_lease_handle_for_test(&old_tokens, 1, None);
    store.save(&key, &marked_old_tokens).await.unwrap();

    let refreshed = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("refreshed-chatgpt-access".into()),
        refresh_token: Some("rotated-rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: o_oauth::chatgpt_declaration()
            .scopes
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: Some("acct-1".into()),
        metadata: serde_json::Value::Null,
    };
    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let refreshed_access = refreshed.primary_secret.clone();
    let refreshed_refresh = refreshed.refresh_token.clone();
    let refreshed_expires_at = refreshed.expires_at;
    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_refresh_coordinator(Arc::new(StaticRefreshCoordinator { tokens: refreshed }))
        .with_auth_lease_handle(auth_lease);
    let registry = ProviderRuntimeRegistry::empty()
        .with_runtime(std::sync::Arc::new(meerkat_openai::OpenAiProviderRuntime));

    let connection = registry
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .expect("refresh should publish through generated AuthMachine authority");
    assert_eq!(
        connection.resolved_secret().as_deref(),
        refreshed_access.as_deref()
    );
    let stored = store.load(&key).await.unwrap().unwrap();
    assert_eq!(stored.primary_secret, refreshed_access);
    assert_eq!(stored.refresh_token, refreshed_refresh);
    assert_eq!(stored.expires_at, refreshed_expires_at);
    assert!(meerkat_core::tokens_lifecycle_published(&stored));
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
    let (persisted, auth_lease) = mark_tokens_and_auth_lease_handle_for_test(&persisted, 1, None);
    store
        .save(
            &TokenKey::parse("dev", "default_chatgpt").expect("valid slugs"),
            &persisted,
        )
        .await
        .unwrap();

    let realm = openai_realm("chatgpt_backend", "external_chatgpt_tokens");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease);
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
async fn openai_external_chatgpt_tokens_empty_lifecycle_restores_marker_before_refresh_required() {
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
            &mark_tokens_lifecycle_published_for_test(&persisted, 1, None),
        )
        .await
        .unwrap();

    let (auth_lease, generated_auth_lease) = empty_auth_lease_handle_for_test();
    let realm = openai_realm("chatgpt_backend", "external_chatgpt_tokens");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(generated_auth_lease);
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
                meerkat_core::AuthError::RefreshRequired
            )
        ),
        "got {err:?}"
    );
    let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&default_auth_binding()));
    assert_eq!(
        snapshot.phase,
        Some(meerkat_core::handles::AuthLeasePhase::Expired)
    );
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
        .with_auth_lease_handle(valid_auth_lease_handle());
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
