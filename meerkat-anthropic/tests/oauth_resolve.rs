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
use meerkat_core::{
    AuthConstraints, AuthProfileConfig, BackendProfileConfig, CredentialSourceSpec,
    ProviderBindingConfig, RealmConfigSection, RealmConnectionSet,
};
use meerkat_llm_core::provider_runtime::{ProviderRuntimeRegistry, ResolverEnvironment};

fn realm_with_oauth_binding(auth_method: &str) -> RealmConnectionSet {
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
            storage: None,
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
        .save(&TokenKey::new("dev", "default_claude"), &persisted)
        .await
        .unwrap();

    let realm = realm_with_oauth_binding("claude_ai_oauth");
    let env = ResolverEnvironment::testing().with_token_store(store.clone());
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

    let connection = registry
        .resolve(&realm, "default_claude", &env)
        .await
        .expect("fresh OAuth tokens should resolve");
    assert_eq!(
        connection.resolved_secret(),
        Some("fresh-access-xyz".to_string()),
    );
}

// --- Expired OAuth bundle → refresh path ------------------------------

#[tokio::test]
async fn claude_ai_oauth_expired_token_refreshes_via_token_endpoint() {
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
        .save(&TokenKey::new("dev", "default_claude"), &persisted)
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
        TokenKey::new("dev", "default_claude"),
    );
    let access = runtime.get_or_refresh_access_token().await.unwrap();
    assert_eq!(access, "refreshed-access-NEW");

    // Verify the new bundle was persisted.
    let updated = store
        .load(&TokenKey::new("dev", "default_claude"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        updated.primary_secret.as_deref(),
        Some("refreshed-access-NEW")
    );
    assert_eq!(
        updated.refresh_token.as_deref(),
        Some("rotated-refresh"),
        "refresh_token must rotate when the server returns one"
    );
    assert!(updated.expires_at.is_some());
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
        .save(&TokenKey::new("dev", "default_claude"), &persisted)
        .await
        .unwrap();

    let realm = realm_with_oauth_binding("oauth_to_api_key");
    let env = ResolverEnvironment::testing().with_token_store(store.clone());
    let registry = ProviderRuntimeRegistry::empty().with_runtime(std::sync::Arc::new(
        meerkat_anthropic::AnthropicProviderRuntime,
    ));

    let connection = registry
        .resolve(&realm, "default_claude", &env)
        .await
        .expect("persisted api_key should resolve");
    assert_eq!(
        connection.resolved_secret(),
        Some("sk-ant-api03-xyz".to_string()),
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
        .resolve(&realm, "default_claude", &env)
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
        .resolve(&realm, "default_claude", &env)
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
