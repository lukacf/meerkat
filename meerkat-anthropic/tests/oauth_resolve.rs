//! Phase 4b — Anthropic Claude.ai OAuth resolution through the provider
//! runtime.
//!
//! Covers the full choke-point: TokenStore has persisted tokens for a
//! `(realm, binding)` → ProviderRuntimeRegistry.resolve returns a
//! `ResolvedConnection` whose Claude.ai OAuth authorizer emits the
//! persisted access token (or a freshly-refreshed one) as Bearer auth.
//!
//! Also covers the `oauth_to_api_key` path: persisted api_key entry
//! returns the raw API key material.

#![cfg(all(not(target_arch = "wasm32"), feature = "oauth",))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};

use meerkat_anthropic::runtime::oauth;
use meerkat_auth_core::auth_store::{
    EphemeralTokenStore, PersistedAuthMode, PersistedTokens, RefreshCoordinator, RefreshError,
    RefreshFn, TokenKey, TokenStore,
};
use meerkat_core::handles::{
    AuthLeaseHandle, AuthLeaseTransition, GeneratedAuthLeaseHandle, LeaseKey,
};
use meerkat_core::{
    AuthBindingRef, AuthConstraints, AuthProfileConfig, BackendProfileConfig, BindingId,
    CredentialSourceSpec, ProviderBindingConfig, RealmConfigSection, RealmConnectionSet, RealmId,
};
use meerkat_llm_core::provider_runtime::binding::ResolvedConnection;
use meerkat_llm_core::provider_runtime::{ProviderRuntimeRegistry, ResolverEnvironment};

fn claude_ai_declaration() -> meerkat_auth_core::oauth_flow::OAuthProviderDeclaration {
    meerkat_auth_core::oauth_flow::oauth_provider_declaration(
        meerkat_auth_core::oauth_flow::OAuthProviderIdentity::AnthropicClaudeAi,
    )
}

fn console_declaration() -> meerkat_auth_core::oauth_flow::OAuthProviderDeclaration {
    meerkat_auth_core::oauth_flow::oauth_provider_declaration(
        meerkat_auth_core::oauth_flow::OAuthProviderIdentity::AnthropicConsoleApiKey,
    )
}

fn realm_with_oauth_binding(auth_method: &str) -> RealmConnectionSet {
    realm_with_oauth_binding_source(auth_method, CredentialSourceSpec::PlatformDefault)
}

fn realm_with_oauth_binding_no_refresh(auth_method: &str) -> RealmConnectionSet {
    let mut backend = BTreeMap::new();
    backend.insert(
        "anthropic_api".into(),
        BackendProfileConfig {
            provider: "anthropic".into(),
            backend_kind: "anthropic_api".into(),
            base_url: None,
            options: serde_json::json!({"realm_id": "dev"}),
        },
    );
    let mut auth = BTreeMap::new();
    auth.insert(
        "claude_oauth".into(),
        AuthProfileConfig {
            provider: "anthropic".into(),
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
        "default_claude".into(),
        ProviderBindingConfig {
            backend_profile: "anthropic_api".into(),
            auth_profile: "claude_oauth".into(),
            default_model: None,
            policy: Default::default(),
            provider_default: false,
        },
    );
    let section = RealmConfigSection {
        backend,
        auth,
        binding,
        default_binding: Some("default_claude".into()),
    };
    RealmConnectionSet::from_config("dev", &section).unwrap()
}

fn realm_with_oauth_binding_source(
    auth_method: &str,
    source: CredentialSourceSpec,
) -> RealmConnectionSet {
    let mut backend = BTreeMap::new();
    backend.insert(
        "anthropic_api".into(),
        BackendProfileConfig {
            provider: "anthropic".into(),
            backend_kind: "anthropic_api".into(),
            base_url: None,
            options: serde_json::json!({"realm_id": "dev"}),
        },
    );
    let mut auth = BTreeMap::new();
    auth.insert(
        "claude_oauth".into(),
        AuthProfileConfig {
            provider: "anthropic".into(),
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
        "default_claude".into(),
        ProviderBindingConfig {
            backend_profile: "anthropic_api".into(),
            auth_profile: "claude_oauth".into(),
            default_model: None,
            policy: Default::default(),
            provider_default: false,
        },
    );
    let section = RealmConfigSection {
        backend,
        auth,
        binding,
        default_binding: Some("default_claude".into()),
    };
    RealmConnectionSet::from_config("dev", &section).unwrap()
}

fn default_auth_binding() -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse("dev").expect("valid realm"),
        binding: BindingId::parse("default_claude").expect("valid binding"),
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

async fn assert_claude_ai_authorizer_headers(connection: &ResolvedConnection, token: &str) {
    let authorizer = connection
        .resolved_authorizer()
        .expect("Claude.ai OAuth should resolve as a bearer authorizer");
    let mut headers = Vec::new();
    let mut request = meerkat_core::HttpAuthorizationRequest {
        method: "POST",
        url: "https://api.anthropic.com/v1/messages",
        headers: &mut headers,
    };
    authorizer.authorize(&mut request).await.unwrap();
    assert!(headers.contains(&("Authorization".to_string(), format!("Bearer {token}"),)));
    assert!(headers.contains(&(
        oauth::OAUTH_BETA_HEADER_NAME.to_string(),
        oauth::OAUTH_BETA_HEADER_VALUE.to_string(),
    )));
    assert!(
        connection.resolved_secret().is_none(),
        "Claude.ai OAuth access tokens must not be projected as x-api-key material"
    );
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

// --- Fresh OAuth bundle → inline secret ---------------------

#[tokio::test]
async fn claude_ai_oauth_fresh_token_returns_access_token() {
    let store = Arc::new(EphemeralTokenStore::new());
    // Persist a fresh token (expires in 1h).
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ClaudeAiOauth,
        primary_secret: Some("fresh-access-xyz".into()),
        refresh_token: Some("refresh-xyz".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: claude_ai_declaration()
            .scopes
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: None,
        metadata: serde_json::Value::Null,
    };
    let (persisted, auth_lease) =
        mark_tokens_and_auth_lease_handle_for_test(&persisted, 1, Some(1_000));
    store
        .save(
            &TokenKey::parse("dev", "default_claude").expect("valid slugs"),
            &persisted,
        )
        .await
        .unwrap();

    let realm = realm_with_oauth_binding("claude_ai_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_auth_lease_handle(auth_lease);
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

    let connection = registry
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .expect("fresh OAuth tokens should resolve");
    assert_claude_ai_authorizer_headers(&connection, "fresh-access-xyz").await;
}

#[tokio::test]
async fn claude_ai_oauth_rejects_token_without_auth_lifecycle() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ClaudeAiOauth,
        primary_secret: Some("stale-access-xyz".into()),
        refresh_token: Some("refresh-xyz".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: claude_ai_declaration()
            .scopes
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: None,
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_claude").expect("valid slugs"),
            &persisted,
        )
        .await
        .unwrap();

    let realm = realm_with_oauth_binding("claude_ai_oauth");
    let env = ResolverEnvironment::testing().with_token_store(store.clone());
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

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
}

#[tokio::test]
async fn claude_ai_oauth_reauth_required_is_typed() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ClaudeAiOauth,
        primary_secret: Some("reauth-access-xyz".into()),
        refresh_token: Some("refresh-xyz".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: claude_ai_declaration()
            .scopes
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: None,
        metadata: serde_json::Value::Null,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_claude").expect("valid slugs"),
            &mark_tokens_lifecycle_published_for_test(&persisted, 1, None),
        )
        .await
        .unwrap();

    let realm = realm_with_oauth_binding("claude_ai_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(reauth_required_auth_lease_handle());
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

    let err = registry
        .resolve(&realm, &default_auth_binding(), &env)
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
async fn claude_ai_oauth_rejects_wrong_persisted_mode() {
    let store = Arc::new(EphemeralTokenStore::new());
    store
        .save(
            &TokenKey::parse("dev", "default_claude").expect("valid slugs"),
            &PersistedTokens {
                auth_mode: PersistedAuthMode::OauthToApiKey,
                primary_secret: Some("sk-ant-api03-stale".into()),
                refresh_token: None,
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

    let realm = realm_with_oauth_binding("claude_ai_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(valid_auth_lease_handle());
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

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
        err.to_string().contains("credential mode OauthToApiKey"),
        "got {err}"
    );
}

#[tokio::test]
async fn claude_ai_oauth_rejects_wrong_source_even_with_matching_mode() {
    let store = Arc::new(EphemeralTokenStore::new());
    store
        .save(
            &TokenKey::parse("dev", "default_claude").expect("valid slugs"),
            &PersistedTokens {
                auth_mode: PersistedAuthMode::ClaudeAiOauth,
                primary_secret: Some("fresh-access-xyz".into()),
                refresh_token: Some("refresh-xyz".into()),
                id_token: None,
                expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
                last_refresh: Some(Utc::now()),
                scopes: claude_ai_declaration()
                    .scopes
                    .iter()
                    .map(|s| (*s).into())
                    .collect(),
                account_id: None,
                metadata: serde_json::Value::Null,
            },
        )
        .await
        .unwrap();

    let realm = realm_with_oauth_binding_source(
        "claude_ai_oauth",
        CredentialSourceSpec::ExternalResolver {
            handle: "external-claude".into(),
        },
    );
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(valid_auth_lease_handle());
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

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

// --- Expired OAuth bundle → refresh path ------------------------------

#[tokio::test]
async fn claude_ai_oauth_expired_authmachine_lease_refreshes_through_provider_runtime() {
    let key = TokenKey::parse("dev", "default_claude").expect("valid slugs");
    let store = Arc::new(EphemeralTokenStore::new());
    let expired = PersistedTokens {
        auth_mode: PersistedAuthMode::ClaudeAiOauth,
        primary_secret: Some("stale-access".into()),
        refresh_token: Some("valid-refresh".into()),
        id_token: None,
        expires_at: Some(Utc::now() - ChronoDuration::minutes(1)),
        last_refresh: Some(Utc::now() - ChronoDuration::hours(2)),
        scopes: vec![],
        account_id: None,
        metadata: serde_json::Value::Null,
    };
    let (marked_expired, auth_lease) =
        mark_tokens_and_auth_lease_handle_for_test(&expired, 1, Some(1_000));
    store.save(&key, &marked_expired).await.unwrap();
    let refreshed = PersistedTokens {
        auth_mode: PersistedAuthMode::ClaudeAiOauth,
        primary_secret: Some("refreshed-access-NEW".into()),
        refresh_token: Some("rotated-refresh".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: claude_ai_declaration()
            .scopes
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: None,
        metadata: serde_json::Value::Null,
    };

    let realm = realm_with_oauth_binding("claude_ai_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_refresh_coordinator(Arc::new(StaticRefreshCoordinator { tokens: refreshed }))
        .with_auth_lease_handle(auth_lease);
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));
    let connection = registry
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .expect("expired Claude OAuth lease should refresh through AuthMachine gate");

    assert_claude_ai_authorizer_headers(&connection, "refreshed-access-NEW").await;
    let stored = store.load(&key).await.unwrap().unwrap();
    assert_eq!(
        stored.primary_secret.as_deref(),
        Some("refreshed-access-NEW")
    );
    assert_eq!(stored.refresh_token.as_deref(), Some("rotated-refresh"));
    assert!(meerkat_core::tokens_lifecycle_published(&stored));
}

#[tokio::test]
async fn claude_ai_oauth_refresh_failure_is_typed() {
    let key = TokenKey::parse("dev", "default_claude").expect("valid slugs");
    let store = Arc::new(EphemeralTokenStore::new());
    let expired = PersistedTokens {
        auth_mode: PersistedAuthMode::ClaudeAiOauth,
        primary_secret: Some("stale-access".into()),
        refresh_token: Some("valid-refresh".into()),
        id_token: None,
        expires_at: Some(Utc::now() - ChronoDuration::minutes(1)),
        last_refresh: Some(Utc::now() - ChronoDuration::hours(2)),
        scopes: vec![],
        account_id: None,
        metadata: serde_json::Value::Null,
    };
    let (marked_expired, auth_lease) =
        mark_tokens_and_auth_lease_handle_for_test(&expired, 1, Some(1_000));
    store.save(&key, &marked_expired).await.unwrap();

    let realm = realm_with_oauth_binding("claude_ai_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_refresh_coordinator(Arc::new(FailingRefreshCoordinator {
            error: RefreshError::Refresh("claude refresh transport failed".into()),
        }))
        .with_auth_lease_handle(auth_lease);
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));
    let err = registry
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .unwrap_err();

    match err {
        meerkat_llm_core::provider_runtime::ProviderAuthError::Auth(
            meerkat_core::AuthError::RefreshFailed(detail),
        ) => assert!(
            detail.contains("claude refresh transport failed"),
            "got {detail}"
        ),
        other => panic!("got {other:?}"),
    }
}

#[tokio::test]
async fn claude_ai_oauth_force_refresh_uses_authmachine_gate_for_fresh_tokens() {
    let key = TokenKey::parse("dev", "default_claude").expect("valid slugs");
    let store = Arc::new(EphemeralTokenStore::new());
    let fresh = PersistedTokens {
        auth_mode: PersistedAuthMode::ClaudeAiOauth,
        primary_secret: Some("fresh-claude-access".into()),
        refresh_token: Some("refresh-claude".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: vec![],
        account_id: None,
        metadata: serde_json::Value::Null,
    };
    let (marked_fresh, auth_lease) =
        mark_tokens_and_auth_lease_handle_for_test(&fresh, 1, Some(1_000));
    store.save(&key, &marked_fresh).await.unwrap();
    let refreshed = PersistedTokens {
        auth_mode: PersistedAuthMode::ClaudeAiOauth,
        primary_secret: Some("forced-claude-access".into()),
        refresh_token: Some("forced-claude-refresh".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: claude_ai_declaration()
            .scopes
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: None,
        metadata: serde_json::Value::Null,
    };

    let realm = realm_with_oauth_binding("claude_ai_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_refresh_coordinator(Arc::new(StaticRefreshCoordinator { tokens: refreshed }))
        .with_auth_lease_handle(auth_lease)
        .with_force_refresh(true);
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));
    let connection = registry
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .expect("forced Claude OAuth refresh should resolve through AuthMachine gate");

    assert_claude_ai_authorizer_headers(&connection, "forced-claude-access").await;
    let stored = store.load(&key).await.unwrap().unwrap();
    assert_eq!(
        stored.primary_secret.as_deref(),
        Some("forced-claude-access")
    );
}

#[tokio::test]
async fn claude_ai_oauth_force_refresh_with_refresh_disallowed_errors_refresh_required() {
    // FOLD 2: a fresh credential + force_refresh forces the refresh branch, but
    // the binding disallows silent refresh. The AuthMachine-owned OAuth-login
    // disposition emits RefreshDisallowed, which the provider mirrors as the
    // RefreshRequired error — exactly the prior shell `!refresh_allowed` gate.
    let key = TokenKey::parse("dev", "default_claude").expect("valid slugs");
    let store = Arc::new(EphemeralTokenStore::new());
    let fresh = PersistedTokens {
        auth_mode: PersistedAuthMode::ClaudeAiOauth,
        primary_secret: Some("fresh-claude-access".into()),
        refresh_token: Some("refresh-claude".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: vec![],
        account_id: None,
        metadata: serde_json::Value::Null,
    };
    let (marked_fresh, auth_lease) =
        mark_tokens_and_auth_lease_handle_for_test(&fresh, 1, Some(1_000));
    store.save(&key, &marked_fresh).await.unwrap();

    let realm = realm_with_oauth_binding_no_refresh("claude_ai_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(auth_lease)
        .with_force_refresh(true);
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));
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

// --- oauth_to_api_key path → persisted api_key ------------------------

#[tokio::test]
async fn oauth_to_api_key_returns_persisted_api_key() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::OauthToApiKey,
        primary_secret: Some("sk-ant-api03-xyz".into()),
        refresh_token: None,
        id_token: None,
        expires_at: None,
        last_refresh: Some(Utc::now()),
        scopes: vec![],
        account_id: None,
        metadata: serde_json::Value::Null,
    };
    let (persisted, auth_lease) = mark_tokens_and_auth_lease_handle_for_test(&persisted, 1, None);
    store
        .save(
            &TokenKey::parse("dev", "default_claude").expect("valid slugs"),
            &persisted,
        )
        .await
        .unwrap();

    let realm = realm_with_oauth_binding("oauth_to_api_key");
    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_auth_lease_handle(auth_lease);
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

    let connection = registry
        .resolve(&realm, &default_auth_binding(), &env)
        .await
        .expect("persisted api_key should resolve");
    assert_eq!(
        connection.resolved_secret(),
        Some("sk-ant-api03-xyz".to_string()),
    );
}

#[tokio::test]
async fn oauth_to_api_key_rejects_claude_ai_oauth_mode() {
    let store = Arc::new(EphemeralTokenStore::new());
    store
        .save(
            &TokenKey::parse("dev", "default_claude").expect("valid slugs"),
            &PersistedTokens {
                auth_mode: PersistedAuthMode::ClaudeAiOauth,
                primary_secret: Some("claude-ai-access".into()),
                refresh_token: Some("refresh-token".into()),
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

    let realm = realm_with_oauth_binding("oauth_to_api_key");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(valid_auth_lease_handle());
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

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
        err.to_string().contains("credential mode ClaudeAiOauth"),
        "got {err}"
    );
}

// --- Missing tokens → InteractiveLoginRequired ------------------------

#[tokio::test]
async fn missing_oauth_tokens_surface_interactive_login_required() {
    let store = Arc::new(EphemeralTokenStore::new());
    let realm = realm_with_oauth_binding("claude_ai_oauth");
    let env = ResolverEnvironment::testing().with_token_store(store);
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

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

// --- No token store wired → InteractiveLoginRequired ------------------

#[tokio::test]
async fn no_token_store_surface_interactive_login_required() {
    let realm = realm_with_oauth_binding("claude_ai_oauth");
    let env = ResolverEnvironment::testing();
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

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

// --- Constants sanity (guards against silent drift) -------------------

#[test]
fn claude_oauth_constants_match_claude_code_source() {
    // These must exactly match claude-code/src/constants/oauth.ts. The
    // client id / endpoints / scopes are owned by the canonical auth-core
    // declaration; only the Anthropic-only facts still live in this crate.
    let claude_ai = claude_ai_declaration();
    let console = console_declaration();
    assert_eq!(claude_ai.client_id, "9d1c250a-e61b-44d9-88ed-5944d1962f5e");
    assert_eq!(
        claude_ai.authorize_endpoint,
        "https://claude.com/cai/oauth/authorize"
    );
    assert_eq!(
        claude_ai.token_endpoint,
        "https://platform.claude.com/v1/oauth/token"
    );
    assert_eq!(
        oauth::API_KEY_CREATE_URL,
        "https://api.anthropic.com/api/oauth/claude_cli/create_api_key"
    );
    assert_eq!(oauth::OAUTH_BETA_HEADER_NAME, "anthropic-beta");
    assert_eq!(oauth::OAUTH_BETA_HEADER_VALUE, "oauth-2025-04-20");
    assert_eq!(
        claude_ai.scopes,
        &[
            "user:profile",
            "user:inference",
            "user:sessions:claude_code",
            "user:mcp_servers",
            "user:file_upload",
        ]
    );
    assert_eq!(console.scopes, &["org:create_api_key", "user:profile"]);
}
