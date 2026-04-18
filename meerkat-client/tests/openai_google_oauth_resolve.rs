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

use meerkat_client::auth_store::{
    EphemeralTokenStore, PersistedAuthMode, PersistedTokens, TokenKey, TokenStore,
};
use meerkat_client::providers::google::oauth as g_oauth;
use meerkat_client::providers::openai::oauth as o_oauth;
use meerkat_client::runtime::{ProviderRuntimeRegistry, ResolverEnvironment};
use meerkat_core::{
    AuthProfileConfig, BackendProfileConfig, CredentialSourceSpec, ProviderBindingConfig,
    RealmConfigSection, RealmConnectionSet,
};

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
            storage: None,
            constraints: Default::default(),
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

fn google_realm(backend_kind: &str, auth_method: &str) -> RealmConnectionSet {
    let mut backend = BTreeMap::new();
    backend.insert(
        backend_kind.into(),
        BackendProfileConfig {
            provider: "gemini".into(),
            backend_kind: backend_kind.into(),
            base_url: None,
            options: serde_json::json!({"realm_id": "dev"}),
        },
    );
    let mut auth = BTreeMap::new();
    auth.insert(
        "google_auth_profile".into(),
        AuthProfileConfig {
            provider: "gemini".into(),
            auth_method: auth_method.into(),
            source: match auth_method {
                "api_key_express" => CredentialSourceSpec::InlineSecret {
                    secret: "vertex-express-key".into(),
                },
                _ => CredentialSourceSpec::PlatformDefault,
            },
            storage: None,
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    let mut binding = BTreeMap::new();
    binding.insert(
        "default_google".into(),
        ProviderBindingConfig {
            backend_profile: backend_kind.into(),
            auth_profile: "google_auth_profile".into(),
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
            default_binding: Some("default_google".into()),
        },
    )
    .unwrap()
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
        .save(&TokenKey::new("dev", "chatgpt_auth"), &persisted)
        .await
        .unwrap();

    let realm = openai_realm("chatgpt_backend", "managed_chatgpt_oauth");
    let env = ResolverEnvironment::testing().with_token_store(store);
    let registry = ProviderRuntimeRegistry::default();
    let connection = registry
        .resolve(&realm, "default_chatgpt", &env)
        .await
        .expect("fresh ChatGPT tokens should resolve");
    assert_eq!(
        connection.resolved_secret(),
        Some("fresh-chatgpt-access".to_string()),
    );
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
        .save(&TokenKey::new("dev", "chatgpt_auth"), &persisted)
        .await
        .unwrap();

    let realm = openai_realm("chatgpt_backend", "external_chatgpt_tokens");
    let env = ResolverEnvironment::testing().with_token_store(store);
    let registry = ProviderRuntimeRegistry::default();
    let connection = registry
        .resolve(&realm, "default_chatgpt", &env)
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
    let registry = ProviderRuntimeRegistry::default();
    let err = registry
        .resolve(&realm, "default_chatgpt", &env)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            meerkat_client::ProviderAuthError::Auth(
                meerkat_core::AuthError::InteractiveLoginRequired
            )
        ),
        "got {err:?}"
    );
}

// --- Google google_oauth ---------------------------------------------

#[tokio::test]
async fn google_oauth_fresh_token_resolves() {
    let store = Arc::new(EphemeralTokenStore::new());
    let persisted = PersistedTokens {
        auth_mode: PersistedAuthMode::GoogleOauth,
        primary_secret: Some("fresh-google-access".into()),
        refresh_token: Some("google-rt".into()),
        id_token: None,
        expires_at: Some(Utc::now() + ChronoDuration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: g_oauth::CODE_ASSIST_SCOPES
            .iter()
            .map(|s| (*s).into())
            .collect(),
        account_id: None,
        metadata: serde_json::Value::Null,
    };
    store
        .save(&TokenKey::new("dev", "google_auth_profile"), &persisted)
        .await
        .unwrap();

    let realm = google_realm("google_code_assist", "google_oauth");
    let env = ResolverEnvironment::testing().with_token_store(store);
    let registry = ProviderRuntimeRegistry::default();
    let connection = registry
        .resolve(&realm, "default_google", &env)
        .await
        .expect("fresh Google OAuth tokens should resolve");
    assert_eq!(
        connection.resolved_secret(),
        Some("fresh-google-access".to_string()),
    );
}

#[tokio::test]
async fn google_adc_returns_authorizer_marker() {
    let realm = google_realm("vertex_ai", "adc");
    let env = ResolverEnvironment::testing();
    let registry = ProviderRuntimeRegistry::default();
    let connection = registry
        .resolve(&realm, "default_google", &env)
        .await
        .unwrap();
    // Adc path: empty lease (build_client constructs the authorizer).
    assert_eq!(connection.resolved_secret(), None);
    assert!(connection.resolved_authorizer().is_none());
}

#[tokio::test]
async fn google_api_key_express_resolves_inline_secret() {
    let realm = google_realm("vertex_ai", "api_key_express");
    let env = ResolverEnvironment::testing();
    let registry = ProviderRuntimeRegistry::default();
    let connection = registry
        .resolve(&realm, "default_google", &env)
        .await
        .unwrap();
    assert_eq!(
        connection.resolved_secret(),
        Some("vertex-express-key".to_string()),
    );
}
