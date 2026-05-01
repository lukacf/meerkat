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
use std::sync::Arc;

use axum::Router;
use axum::extract::Form;
use axum::response::Json;
use axum::routing::post;
use chrono::{Duration as ChronoDuration, Utc};
use tokio::net::TcpListener;

use meerkat_anthropic::runtime::oauth;
use meerkat_auth_core::auth_oauth::OAuthEndpoints;
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
        .with_token_store(store.clone())
        .with_auth_lease_handle(StaticAuthLeaseHandle::valid_for_tokens(&persisted));
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

    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::ClaudeAiOauth,
        primary_secret: Some("stale-access".into()),
        refresh_token: Some("valid-refresh".into()),
        id_token: None,
        // Expired 1 minute ago.
        expires_at: Some(Utc::now() - ChronoDuration::minutes(1)),
        last_refresh: Some(Utc::now() - ChronoDuration::hours(2)),
        scopes: vec![],
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

    // Directly exercise AnthropicOAuthRuntime against the mock endpoint
    // (since the provider runtime's resolve_binding hardcodes the real
    // endpoint URLs — url-override support via the registry env is a
    // Phase 4c wire-config concern, not 4b).
    let runtime = oauth::AnthropicOAuthRuntime::new_with_default_coordinator(
        store.clone(),
        endpoints,
        TokenKey::parse("dev", "default_claude").expect("valid slugs"),
    );
    let before_refresh = Utc::now();
    let refreshed = runtime.get_or_refresh_tokens().await.unwrap();
    assert_eq!(
        refreshed.primary_secret.as_deref(),
        Some("refreshed-access-NEW")
    );
    assert!(
        refreshed.expires_at.expect("refreshed expiry")
            > before_refresh + ChronoDuration::minutes(50),
        "refreshed lease expiry must be the new provider expiry, not the old expired value"
    );

    // The runtime may refresh, but AuthMachine-owned resolver code owns
    // publication and durable persistence.
    let updated = store
        .load(&TokenKey::parse("dev", "default_claude").expect("valid slugs"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.primary_secret.as_deref(), Some("stale-access"));
    assert_eq!(updated.refresh_token.as_deref(), Some("valid-refresh"));
    assert_eq!(updated.expires_at, persisted.expires_at);
    assert!(!meerkat_core::tokens_lifecycle_published(&updated));
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
        .with_auth_lease_handle(StaticAuthLeaseHandle::valid());
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
