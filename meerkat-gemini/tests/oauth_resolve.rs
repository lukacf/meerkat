#![cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::Form;
use axum::response::Json;
use axum::routing::post;
use chrono::{Duration as ChronoDuration, Utc};
use tokio::net::TcpListener;

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
            options: serde_json::json!({"realm_id": "dev"}),
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

fn default_connection_ref() -> ConnectionRef {
    ConnectionRef {
        realm: RealmId::parse("dev").expect("valid realm"),
        binding: BindingId::parse("default_code_assist").expect("valid binding"),
        profile: None,
    }
}

struct StaticAuthLeaseHandle;

impl StaticAuthLeaseHandle {
    fn valid() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl AuthLeaseHandle for StaticAuthLeaseHandle {
    fn acquire_lease(
        &self,
        _lease_key: &LeaseKey,
        _expires_at: u64,
    ) -> Result<AuthLeaseTransition, DslTransitionError> {
        Ok(AuthLeaseTransition { generation: 1 })
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
        Ok(AuthLeaseTransition { generation: 1 })
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
            expires_at: None,
            credential_present: true,
            generation: 1,
        }
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
    let connection = registry
        .resolve(&code_assist_realm(), &default_connection_ref(), &env)
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
        .resolve(&code_assist_realm(), &default_connection_ref(), &env)
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
        .resolve(&code_assist_realm(), &default_connection_ref(), &env)
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
            &default_connection_ref(),
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
async fn google_oauth_refresh_returns_refreshed_expiry_for_lease_publication() {
    let app = Router::new().route(
        "/token",
        post(|Form(_form): Form<serde_json::Value>| async {
            Json(serde_json::json!({
                "access_token": "refreshed-google-access",
                "refresh_token": "rotated-google-refresh",
                "expires_in": 3600,
                "token_type": "Bearer",
                "scope": "https://www.googleapis.com/auth/cloud-platform",
            }))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let key = TokenKey::parse("dev", "default_code_assist").expect("valid slugs");
    let store = Arc::new(EphemeralTokenStore::new());
    let old_expiry = Utc::now() - ChronoDuration::minutes(5);
    store
        .save(
            &key,
            &PersistedTokens {
                auth_mode: PersistedAuthMode::GoogleOauth,
                primary_secret: Some("expired-google-access".into()),
                refresh_token: Some("refresh-google".into()),
                id_token: None,
                expires_at: Some(old_expiry),
                last_refresh: Some(Utc::now() - ChronoDuration::hours(2)),
                scopes: vec![],
                account_id: None,
                metadata: serde_json::Value::Null,
            },
        )
        .await
        .unwrap();

    let endpoints = OAuthEndpoints {
        client_id: oauth::CODE_ASSIST_CLIENT_ID.into(),
        authorize_url: oauth::GOOGLE_AUTHORIZE_URL.into(),
        token_url: format!("http://{addr}/token"),
        device_code_url: None,
        redirect_uri: "http://127.0.0.1:0/callback".into(),
        scopes: oauth::CODE_ASSIST_SCOPES
            .iter()
            .map(|scope| (*scope).into())
            .collect(),
        extra_headers: vec![],
    };
    let runtime = oauth::GoogleCodeAssistOAuthRuntime::new_with_default_coordinator(
        store.clone(),
        endpoints,
        key,
    );
    let before_refresh = Utc::now();
    let refreshed = runtime.get_or_refresh_tokens().await.unwrap();

    assert_eq!(
        refreshed.primary_secret.as_deref(),
        Some("refreshed-google-access")
    );
    assert_eq!(
        refreshed.refresh_token.as_deref(),
        Some("rotated-google-refresh")
    );
    assert!(
        refreshed.expires_at.expect("refreshed expiry")
            > before_refresh + ChronoDuration::minutes(50),
        "refreshed lease expiry must be the new provider expiry, not the old expired value"
    );
    let stored = store.load(runtime.key()).await.unwrap().unwrap();
    assert_eq!(stored.expires_at, refreshed.expires_at);
}
