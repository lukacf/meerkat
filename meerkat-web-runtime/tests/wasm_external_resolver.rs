//! T18 (Phase 5): WASM external auth resolver — top-down observable
//! proof that the JS host can register a callback that yields a structured
//! auth envelope, and the meerkat WASM runtime reads the registration
//! correctly.
//!
//! Plan choke point K9 reads: "browser host registers
//! `ExternalAuthResolverHandle` via `wasm-bindgen`; calls
//! `build_session_request_with_connection_ref` → stream yields
//! tokens". This file exercises the registration + invoke half; the
//! full fetch-stream path is covered by the existing
//! `tests/browser_contract.rs` once a connection_ref binding is
//! resolved through the external resolver (future extension).
//!
//! Runs on `wasm32` only via wasm-bindgen-test. Non-wasm coverage of
//! the registration no-op shim lives in the module's `#[test]` block.

#![cfg(target_arch = "wasm32")]

use std::sync::Arc;

use js_sys::Function;
use meerkat_core::{
    AuthError, AuthProfile, BackendProfile, BindingPolicy, ConnectionRef, CredentialSourceSpec,
    Provider, ResolvedAuthEnvelope,
    connection::{BindingId, ProfileId, RealmId},
    provider_matrix::openai::{OpenAiAuthMethod, OpenAiBackendKind},
};
use meerkat_providers::{
    ExternalAuthResolverHandle, NormalizedAuthMethod, NormalizedBackendKind, ValidatedBinding,
};
use meerkat_web_runtime::external_auth::{
    WASM_EXTERNAL_AUTH_RESOLVER_ID, WasmExternalAuthResolver, has_external_auth_resolver,
    register_external_auth_resolver,
};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_test::wasm_bindgen_test;

fn test_binding() -> ValidatedBinding {
    ValidatedBinding {
        connection_ref: ConnectionRef {
            realm: RealmId::parse("browser").expect("realm"),
            binding: BindingId::parse("chatgpt").expect("binding"),
            profile: Some(ProfileId::parse("primary").expect("profile")),
        },
        provider: Provider::OpenAI,
        backend: NormalizedBackendKind::OpenAi(OpenAiBackendKind::ChatGptBackend),
        auth: NormalizedAuthMethod::OpenAi(OpenAiAuthMethod::ExternalAuthorizer),
        backend_profile: Arc::new(BackendProfile {
            id: "chatgpt-backend".into(),
            provider: Provider::OpenAI,
            backend_kind: "chatgpt_backend".into(),
            base_url: None,
            options: serde_json::Value::Null,
        }),
        auth_profile: Arc::new(AuthProfile {
            id: "host-oauth".into(),
            provider: Provider::OpenAI,
            auth_method: "external_authorizer".into(),
            source: CredentialSourceSpec::ExternalResolver {
                handle: "wasm_host".into(),
            },
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        }),
        policy: BindingPolicy::default(),
    }
}

fn register_eval_callback(source: &str) {
    let cb = js_sys::eval(source)
        .expect("eval callback")
        .dyn_into::<Function>()
        .expect("cast to Function");
    register_external_auth_resolver(JsValue::from(cb)).expect("register callback");
}

async fn resolve_with_registered_callback() -> Result<ResolvedAuthEnvelope, AuthError> {
    WasmExternalAuthResolver.resolve(&test_binding()).await
}

/// Before registration: `has_external_auth_resolver()` returns false.
#[wasm_bindgen_test]
fn resolver_is_absent_before_registration() {
    // Start from a known state: clear any prior registration.
    register_external_auth_resolver(JsValue::NULL).expect("clear");
    assert!(!has_external_auth_resolver());
}

/// After registering a function callback: presence flips to true.
#[wasm_bindgen_test]
fn register_callback_sets_resolver_present() {
    let cb = js_sys::eval(
        r#"
            (function (connectionRef) {
                return Promise.resolve({
                    kind: "inline_secret",
                    secret: "bearer-" + connectionRef.realm + "-" + connectionRef.binding,
                    metadata: {},
                    expires_at: "2030-01-02T03:04:05Z"
                });
            })
        "#,
    )
    .expect("eval callback")
    .dyn_into::<Function>()
    .expect("cast to Function");
    let value = JsValue::from(cb);
    register_external_auth_resolver(value).expect("register");
    assert!(has_external_auth_resolver());
}

/// Passing `null` clears the registration — a host page that needs
/// to rotate providers can reset the handle explicitly.
#[wasm_bindgen_test]
fn register_null_clears_resolver() {
    // Install then clear.
    let cb = js_sys::eval("(function (_) { return Promise.resolve('x'); })")
        .unwrap()
        .dyn_into::<Function>()
        .unwrap();
    register_external_auth_resolver(JsValue::from(cb)).expect("install");
    assert!(has_external_auth_resolver());
    register_external_auth_resolver(JsValue::NULL).expect("clear");
    assert!(!has_external_auth_resolver());
}

/// Passing a non-function value returns an error — the handle is
/// type-checked at registration time, not at invocation time.
#[wasm_bindgen_test]
fn register_non_function_returns_error() {
    let err = register_external_auth_resolver(JsValue::from_str("not a function")).unwrap_err();
    let s = err.as_string().unwrap_or_default();
    assert!(
        s.contains("must be a function"),
        "error must mention callback must be a function: {s}"
    );
}

/// Host pages must return structured envelopes. A bare bearer string does not
/// carry enough lease truth to become an `InlineSecret` with default metadata.
#[wasm_bindgen_test(async)]
async fn resolver_rejects_bare_bearer_string_without_creating_inline_secret() {
    register_eval_callback(
        r#"
            (function (connectionRef) {
                return Promise.resolve("bearer-" + connectionRef.realm + "-" + connectionRef.binding);
            })
        "#,
    );

    let err = resolve_with_registered_callback()
        .await
        .expect_err("bare bearer strings must not synthesize lease truth");
    let message = err.to_string();
    assert!(
        message.contains("structured auth envelope"),
        "unexpected bare string rejection: {message}"
    );
}

/// Typed JS resolver objects must cross the WASM boundary without losing lease
/// expiration or non-secret provenance metadata.
#[wasm_bindgen_test(async)]
async fn resolver_preserves_typed_inline_secret_expiration_and_metadata() {
    register_eval_callback(
        r#"
            (function (connectionRef) {
                if (
                    connectionRef.realm !== "browser" ||
                    connectionRef.binding !== "chatgpt" ||
                    connectionRef.profile !== "primary"
                ) {
                    return Promise.reject({
                        kind: "other",
                        detail: "connectionRef projection changed: " + JSON.stringify(connectionRef)
                    });
                }
                return Promise.resolve({
                    kind: "inline_secret",
                    secret: "typed-access-token",
                    metadata: {
                        account_id: "acct_browser",
                        workspace_id: "workspace_browser",
                        user_id: "user_browser"
                    },
                    expires_at: "2030-01-02T03:04:05Z"
                });
            })
        "#,
    );

    let envelope = resolve_with_registered_callback()
        .await
        .expect("resolve typed envelope");

    match envelope {
        ResolvedAuthEnvelope::InlineSecret {
            secret,
            metadata,
            expires_at,
        } => {
            assert_eq!(secret, "typed-access-token");
            assert_eq!(metadata.account_id.as_deref(), Some("acct_browser"));
            assert_eq!(metadata.workspace_id.as_deref(), Some("workspace_browser"));
            assert_eq!(metadata.user_id.as_deref(), Some("user_browser"));
            assert_eq!(
                expires_at.expect("expiration").to_rfc3339(),
                "2030-01-02T03:04:05+00:00"
            );
        }
        other => panic!("unexpected envelope: {other:?}"),
    }
}

/// Structured JS rejections preserve stable auth-failure truth instead of
/// collapsing every rejection into `AuthError::Other`.
#[wasm_bindgen_test(async)]
async fn resolver_preserves_structured_refresh_and_denial_failures() {
    register_eval_callback(
        r#"
            (function (_) {
                return Promise.reject({
                    kind: "refresh_failed",
                    detail: "browser refresh token revoked"
                });
            })
        "#,
    );
    let err = resolve_with_registered_callback()
        .await
        .expect_err("refresh failure should remain typed");
    match err {
        AuthError::RefreshFailed(detail) => {
            assert_eq!(detail, "browser refresh token revoked");
        }
        other => panic!("unexpected refresh error: {other:?}"),
    }

    register_eval_callback(
        r#"
            (function (_) {
                return Promise.reject({ kind: "interactive_login_required" });
            })
        "#,
    );
    let err = resolve_with_registered_callback()
        .await
        .expect_err("denial should remain typed");
    assert!(matches!(err, AuthError::InteractiveLoginRequired));
}

/// Missing credentials fail closed whether the host forgot to register a
/// resolver, produced an empty token, or resolved to JS null.
#[wasm_bindgen_test(async)]
async fn resolver_fails_closed_for_missing_credentials() {
    register_external_auth_resolver(JsValue::NULL).expect("clear");
    let err = resolve_with_registered_callback()
        .await
        .expect_err("missing resolver should fail closed");
    assert!(matches!(err, AuthError::MissingSecret));

    register_eval_callback("(function (_) { return Promise.resolve('   '); })");
    let err = resolve_with_registered_callback()
        .await
        .expect_err("blank token should fail closed");
    assert!(matches!(err, AuthError::MissingSecret));

    register_eval_callback("(function (_) { return Promise.resolve(null); })");
    let err = resolve_with_registered_callback()
        .await
        .expect_err("null token should fail closed");
    assert!(matches!(err, AuthError::MissingSecret));
}

/// Raw wasm-bindgen registration intentionally requires a Promise. The
/// TypeScript SDK adapter provides the JS-facing convenience for callbacks
/// that return synchronously, but the resolved auth value must still be
/// structured.
#[wasm_bindgen_test(async)]
async fn resolver_rejects_non_promise_js_results() {
    register_eval_callback("(function (_) { return 'bearer-without-promise'; })");

    let err = resolve_with_registered_callback()
        .await
        .expect_err("raw WASM callback must return Promise");
    let message = err.to_string();
    assert!(
        message.contains("must return a Promise"),
        "unexpected error: {message}"
    );
}

#[wasm_bindgen_test(async)]
async fn self_hosted_connection_ref_uses_registered_wasm_external_resolver() {
    register_external_auth_resolver(JsValue::NULL).expect("clear");
    let cb = js_sys::eval(
        r#"
            (function (connectionRef) {
                globalThis.__meerkat_self_hosted_external_ref =
                    connectionRef.realm + ":" + connectionRef.binding;
                return Promise.resolve({
                    kind: "inline_secret",
                    secret: "wasm-self-hosted-token",
                    metadata: {},
                    expires_at: "2030-01-02T03:04:05Z"
                });
            })
        "#,
    )
    .expect("eval callback")
    .dyn_into::<Function>()
    .expect("cast to Function");
    register_external_auth_resolver(JsValue::from(cb)).expect("register");

    let mut config = meerkat_core::Config::default();
    config.agent.model = "gemma-4-e2b".to_string();
    config.self_hosted.servers.insert(
        "local".to_string(),
        meerkat_core::SelfHostedServerConfig {
            transport: meerkat_core::SelfHostedTransport::OpenAiCompatible,
            base_url: "https://self-hosted.example/v1".to_string(),
            api_style: meerkat_core::SelfHostedApiStyle::ChatCompletions,
            bearer_token: None,
            bearer_token_env: None,
        },
    );
    config.self_hosted.models.insert(
        "gemma-4-e2b".to_string(),
        meerkat_core::SelfHostedModelConfig {
            server: "local".to_string(),
            remote_model: "gemma4:e2b".to_string(),
            display_name: "Gemma 4 E2B".into(),
            family: "gemma-4".to_string(),
            tier: meerkat_models::ModelTier::Supported,
            context_window: Some(128_000),
            max_output_tokens: Some(8_192),
            vision: true,
            image_tool_results: true,
            inline_video: false,
            supports_temperature: true,
            supports_thinking: true,
            supports_reasoning: true,
            supports_web_search: false,
            call_timeout_secs: Some(600),
        },
    );

    let mut realm = meerkat_core::RealmConfigSection {
        default_binding: Some("local_binding".to_string()),
        ..Default::default()
    };
    realm.backend.insert(
        "local_backend".to_string(),
        meerkat_core::BackendProfileConfig {
            provider: "self_hosted".to_string(),
            backend_kind: "self_hosted".to_string(),
            base_url: Some("https://self-hosted.example/v1".to_string()),
            options: serde_json::Value::Null,
        },
    );
    realm.auth.insert(
        "local_auth".to_string(),
        meerkat_core::AuthProfileConfig {
            provider: "self_hosted".to_string(),
            auth_method: "static_bearer".to_string(),
            source: meerkat_core::CredentialSourceSpec::ExternalResolver {
                handle: WASM_EXTERNAL_AUTH_RESOLVER_ID.to_string(),
            },
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    realm.binding.insert(
        "local_binding".to_string(),
        meerkat_core::ProviderBindingConfig {
            backend_profile: "local_backend".to_string(),
            auth_profile: "local_auth".to_string(),
            default_model: Some("gemma4:e2b".to_string()),
            policy: Default::default(),
        },
    );
    config.realm.insert("default".to_string(), realm);

    let connection_ref = meerkat_core::ConnectionRef {
        realm: meerkat_core::connection::RealmId::parse("default").expect("realm"),
        binding: meerkat_core::connection::BindingId::parse("local_binding").expect("binding"),
        profile: None,
    };
    let factory = meerkat::AgentFactory::minimal().with_external_auth_resolver(
        WASM_EXTERNAL_AUTH_RESOLVER_ID,
        Arc::new(WasmExternalAuthResolver),
    );
    let mut build = meerkat::AgentBuildConfig::new("gemma-4-e2b");
    build.provider = Some(meerkat_core::Provider::SelfHosted);
    build.connection_ref = Some(connection_ref.clone());
    build.override_builtins = meerkat_core::ToolCategoryOverride::Disable;

    let agent = factory
        .build_agent(build, &config)
        .await
        .expect("self-hosted build should resolve through wasm external auth");

    assert_eq!(
        agent.session().session_metadata().unwrap().connection_ref,
        Some(connection_ref)
    );
    let observed = js_sys::Reflect::get(
        &js_sys::global(),
        &JsValue::from_str("__meerkat_self_hosted_external_ref"),
    )
    .expect("read callback marker")
    .as_string()
    .expect("callback marker string");
    assert_eq!(observed, "default:local_binding");
}
