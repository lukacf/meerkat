//! Phase 4b — Anthropic Claude.ai OAuth resolution through the provider
//! runtime.
//!
//! Covers the full choke-point: TokenStore has persisted tokens for a
//! `(realm, binding)` → ProviderRuntimeRegistry.resolve returns a
//! `ResolvedConnection` whose resolved inline secret is the persisted
//! access token (or a freshly-refreshed one).
//!
//! Also covers the `oauth_to_api_key` path: persisted api_key entry
//! returns the raw API key material.

#![cfg(all(not(target_arch = "wasm32"), feature = "oauth",))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::Router;
use axum::extract::Form;
use axum::response::Json;
use axum::routing::post;
use chrono::{Duration as ChronoDuration, Utc};
use tokio::net::TcpListener;

use meerkat_anthropic::runtime::oauth;
use meerkat_auth_core::auth_oauth::{OAuthEndpoints, exchange_refresh_token};
use meerkat_auth_core::auth_store::{
    EphemeralTokenStore, PersistedAuthMode, PersistedTokens, TokenKey, TokenStore,
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

struct FixedAuthLeaseHandle {
    snapshot: AuthLeaseSnapshot,
}

impl FixedAuthLeaseHandle {
    fn valid(expires_at: chrono::DateTime<Utc>, generation: u64) -> Self {
        Self {
            snapshot: AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: Some(expires_at.timestamp().max(0) as u64),
                generation,
            },
        }
    }

    fn valid_non_expiring(generation: u64) -> Self {
        Self {
            snapshot: AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: None,
                generation,
            },
        }
    }

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

impl AuthLeaseHandle for FixedAuthLeaseHandle {
    fn acquire_lease(
        &self,
        _lease_key: &LeaseKey,
        _expires_at: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        panic!("managed OAuth must not reacquire over reauth-required lease truth")
    }

    fn acquire_lease_if_snapshot(
        &self,
        _lease_key: &LeaseKey,
        _expected: &AuthLeaseSnapshot,
        _expires_at: u64,
    ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
        panic!("managed OAuth must not conditionally reacquire over reauth-required lease truth")
    }

    fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        panic!("managed OAuth must not mark expiring over reauth-required lease truth")
    }

    fn begin_refresh(
        &self,
        _lease_key: &LeaseKey,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        panic!("managed OAuth must not refresh over reauth-required lease truth")
    }

    fn begin_refresh_if_snapshot(
        &self,
        _lease_key: &LeaseKey,
        _expected: &AuthLeaseSnapshot,
    ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
        panic!("managed OAuth must not conditionally refresh over reauth-required lease truth")
    }

    fn complete_refresh(
        &self,
        _lease_key: &LeaseKey,
        _new_expires_at: u64,
        _now: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        panic!("managed OAuth must not complete refresh over reauth-required lease truth")
    }

    fn complete_refresh_if_snapshot(
        &self,
        _lease_key: &LeaseKey,
        _expected: &AuthLeaseSnapshot,
        _new_expires_at: u64,
        _now: u64,
    ) -> Result<Option<AuthLeaseTransition>, DslTransitionError> {
        panic!(
            "managed OAuth must not conditionally complete refresh over reauth-required lease truth"
        )
    }

    fn refresh_failed(
        &self,
        _lease_key: &LeaseKey,
        _permanent: bool,
    ) -> Result<(), DslTransitionError> {
        panic!("managed OAuth must not report refresh failure over reauth-required lease truth")
    }

    fn refresh_failed_if_snapshot(
        &self,
        _lease_key: &LeaseKey,
        _expected: &AuthLeaseSnapshot,
        _permanent: bool,
    ) -> Result<bool, DslTransitionError> {
        panic!("managed OAuth must not report refresh failure over reauth-required lease truth")
    }

    fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn mark_reauth_required_if_snapshot(
        &self,
        _lease_key: &LeaseKey,
        expected: &AuthLeaseSnapshot,
    ) -> Result<bool, DslTransitionError> {
        Ok(matches!(
            expected.phase,
            Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring)
        ))
    }

    fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
        self.snapshot.clone()
    }
}

fn realm_with_oauth_binding(auth_method: &str) -> RealmConnectionSet {
    realm_with_oauth_binding_source(auth_method, CredentialSourceSpec::PlatformDefault)
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

fn default_connection_ref() -> ConnectionRef {
    ConnectionRef {
        realm: RealmId::parse("dev").expect("valid realm"),
        binding: BindingId::parse("default_claude").expect("valid binding"),
        profile: None,
    }
}

fn default_token_key() -> TokenKey {
    TokenKey::from_connection_ref(&default_connection_ref())
}

// --- Fresh OAuth bundle → inline secret ---------------------

#[tokio::test]
async fn claude_ai_oauth_fresh_token_returns_access_token() {
    let store = Arc::new(EphemeralTokenStore::new());
    // Persist a fresh token (expires in 1h).
    let expires_at = Utc::now() + ChronoDuration::hours(1);
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ClaudeAiOauth,
        primary_secret: Some("fresh-access-xyz".into()),
        refresh_token: Some("refresh-xyz".into()),
        id_token: None,
        expires_at: Some(expires_at),
        last_refresh: Some(Utc::now()),
        scopes: oauth::CLAUDE_AI_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: None,
        metadata: serde_json::Value::Null,
        auth_lease: None,
    }
    .with_auth_lease_binding(default_token_key(), 7);
    store.save(&default_token_key(), &persisted).await.unwrap();

    let realm = realm_with_oauth_binding("claude_ai_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_auth_lease_handle(Arc::new(FixedAuthLeaseHandle::valid(expires_at, 7)));
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("fresh OAuth tokens should resolve");
    assert_eq!(
        connection.resolved_secret(),
        Some("fresh-access-xyz".to_string()),
    );
}

#[tokio::test]
async fn claude_ai_oauth_reauth_required_does_not_return_cached_token() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ClaudeAiOauth,
        primary_secret: Some("fresh-access-xyz".into()),
        refresh_token: Some("refresh-xyz".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: oauth::CLAUDE_AI_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: None,
        metadata: serde_json::Value::Null,
        auth_lease: None,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_claude").expect("valid slugs"),
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

    let realm = realm_with_oauth_binding("claude_ai_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(Arc::new(FixedAuthLeaseHandle::reauth_required()));
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

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
async fn claude_ai_oauth_rejects_token_without_auth_lifecycle() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ClaudeAiOauth,
        primary_secret: Some("stale-access-xyz".into()),
        refresh_token: Some("refresh-xyz".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: oauth::CLAUDE_AI_SCOPES
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
        .with_auth_lease_handle(StaticAuthLeaseHandle::valid());
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

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
                scopes: oauth::CLAUDE_AI_SCOPES
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
        .with_auth_lease_handle(StaticAuthLeaseHandle::valid());
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

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

// --- Expired OAuth bundle → refresh path ------------------------------

#[tokio::test]
async fn claude_ai_oauth_runtime_refresh_is_uncommitted() {
    // Mock token endpoint returns a new access_token + refresh_token.
    let app = Router::new().route(
        "/v1/oauth/token",
        post(|Form(_form): Form<serde_json::Value>| async {
            Json(serde_json::json!({
                "access_token": "refreshed-access-NEW",
                "refresh_token": "rotated-refresh",
                "expires_in": 3600,
                "token_type": "Bearer",
                "scope": "user:profile user:inference",
            }))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Point the OAuth runtime at our mock.
    let endpoints = OAuthEndpoints {
        client_id: oauth::CLAUDE_CLIENT_ID.into(),
        authorize_url: oauth::CLAUDE_AI_AUTHORIZE_URL.into(),
        token_url: format!("http://{addr}/v1/oauth/token"),
        device_code_url: None,
        redirect_uri: oauth::MANUAL_REDIRECT_URL.into(),
        scopes: oauth::CLAUDE_AI_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        extra_headers: vec![(
            oauth::OAUTH_BETA_HEADER_NAME.into(),
            oauth::OAUTH_BETA_HEADER_VALUE.into(),
        )],
    };

    let result = exchange_refresh_token(&reqwest::Client::new(), &endpoints, "valid-refresh", None)
        .await
        .unwrap();
    assert_eq!(result.access_token, "refreshed-access-NEW");
    assert_eq!(result.refresh_token.as_deref(), Some("rotated-refresh"));
    assert_eq!(result.expires_in_secs, Some(3600));
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
        auth_lease: None,
    }
    .with_auth_lease_binding(default_token_key(), 7);
    store
        .save(
            &TokenKey::parse("dev", "default_claude").expect("valid slugs"),
            &meerkat_core::mark_tokens_lifecycle_published_for_generation(&persisted, 1),
        )
        .await
        .unwrap();

    let realm = realm_with_oauth_binding("oauth_to_api_key");
    let env = ResolverEnvironment::testing()
        .with_token_store(store.clone())
        .with_auth_lease_handle(Arc::new(FixedAuthLeaseHandle::valid_non_expiring(7)));
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

    let connection = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect("persisted api_key should resolve");
    assert_eq!(
        connection.resolved_secret(),
        Some("sk-ant-api03-xyz".to_string()),
    );
}

#[tokio::test]
async fn oauth_to_api_key_unbound_token_does_not_bypass_lease_truth() {
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
        auth_lease: None,
    };
    store
        .save(
            &TokenKey::parse("dev", "default_claude").expect("valid slugs"),
            &persisted,
        )
        .await
        .unwrap();

    let realm = realm_with_oauth_binding("oauth_to_api_key");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(Arc::new(FixedAuthLeaseHandle::valid_non_expiring(7)));
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

    let err = registry
        .resolve(&realm, &default_connection_ref(), &env)
        .await
        .expect_err("oauth_to_api_key must not return unbound TokenStore material");

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

// --- Missing tokens under lease truth → Expired -----------------------

#[tokio::test]
async fn missing_oauth_tokens_surface_expired_under_authmachine_truth() {
    let store = Arc::new(EphemeralTokenStore::new());
    let expires_at = Utc::now() + ChronoDuration::hours(1);
    let realm = realm_with_oauth_binding("claude_ai_oauth");
    let env = ResolverEnvironment::testing()
        .with_token_store(store)
        .with_auth_lease_handle(Arc::new(FixedAuthLeaseHandle::valid(expires_at, 7)));
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

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

// --- Constants sanity (guards against silent drift) -------------------

#[test]
fn claude_oauth_constants_match_claude_code_source() {
    // These must exactly match claude-code/src/constants/oauth.ts.
    assert_eq!(
        oauth::CLAUDE_CLIENT_ID,
        "9d1c250a-e61b-44d9-88ed-5944d1962f5e"
    );
    assert_eq!(
        oauth::CLAUDE_AI_AUTHORIZE_URL,
        "https://claude.com/cai/oauth/authorize"
    );
    assert_eq!(
        oauth::TOKEN_URL,
        "https://platform.claude.com/v1/oauth/token"
    );
    assert_eq!(
        oauth::API_KEY_CREATE_URL,
        "https://api.anthropic.com/api/oauth/claude_cli/create_api_key"
    );
    assert_eq!(oauth::OAUTH_BETA_HEADER_NAME, "anthropic-beta");
    assert_eq!(oauth::OAUTH_BETA_HEADER_VALUE, "oauth-2025-04-20");
    assert_eq!(
        oauth::CLAUDE_AI_SCOPES,
        &[
            "user:profile",
            "user:inference",
            "user:sessions:claude_code",
            "user:mcp_servers",
            "user:file_upload",
        ]
    );
    assert_eq!(
        oauth::CONSOLE_SCOPES,
        &["org:create_api_key", "user:profile"]
    );
}
