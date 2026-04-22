//! T18 (Phase 5): WASM external auth resolver — top-down observable
//! proof that the browser host can register a JS callback that yields
//! a bearer token, and the meerkat WASM runtime reads the registration
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
//! Runs on `wasm32` only via wasm-bindgen-test's default Node harness.
//! Non-wasm coverage of the registration no-op shim lives in the
//! module's `#[test]` block.

#![cfg(target_arch = "wasm32")]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use js_sys::Function;
use meerkat_providers::ExternalAuthResolverHandle;
use meerkat_web_runtime::external_auth::{
    WasmExternalAuthResolver, has_external_auth_resolver, register_external_auth_resolver,
};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_test::wasm_bindgen_test;

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
            (function (binding_key) {
                return Promise.resolve("bearer-" + binding_key);
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

/// Invoking the shipped wasm external-auth resolver with a canonical
/// `ValidatedBinding` forwards `<realm_id>:<binding_id>` to the host
/// callback and returns its bearer token envelope.
#[wasm_bindgen_test(async)]
async fn resolver_invocation_uses_canonical_binding_key() {
    register_external_auth_resolver(JsValue::NULL).expect("clear");
    let cb = js_sys::eval(
        r#"
            (function (binding_key) {
                globalThis.__meerkat_binding_key = binding_key;
                return Promise.resolve("bearer-" + binding_key);
            })
        "#,
    )
    .expect("eval callback")
    .dyn_into::<Function>()
    .expect("cast to Function");
    register_external_auth_resolver(JsValue::from(cb)).expect("register");

    let binding = meerkat_providers::ValidatedBinding {
        connection_ref: meerkat_core::ConnectionRef {
            realm_id: "browser".into(),
            binding_id: "openai_host".into(),
        },
        provider: meerkat_core::Provider::OpenAI,
        backend: meerkat_providers::NormalizedBackendKind::OpenAi(
            meerkat_core::provider_matrix::openai::OpenAiBackendKind::OpenAiApi,
        ),
        auth: meerkat_providers::NormalizedAuthMethod::OpenAi(
            meerkat_core::provider_matrix::openai::OpenAiAuthMethod::ExternalAuthorizer,
        ),
        backend_profile: std::sync::Arc::new(meerkat_core::BackendProfile {
            id: "backend".into(),
            provider: meerkat_core::Provider::OpenAI,
            backend_kind: "openai_api".into(),
            base_url: None,
            options: serde_json::Value::Null,
        }),
        auth_profile: std::sync::Arc::new(meerkat_core::AuthProfile {
            id: "auth".into(),
            provider: meerkat_core::Provider::OpenAI,
            auth_method: "external_authorizer".into(),
            source: meerkat_core::CredentialSourceSpec::ExternalResolver {
                handle: "wasm_host".into(),
            },
            storage: Default::default(),
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        }),
        policy: Default::default(),
    };

    let envelope = WasmExternalAuthResolver
        .resolve(&binding)
        .await
        .expect("resolve should succeed");
    let recorded = js_sys::Reflect::get(
        &js_sys::global(),
        &JsValue::from_str("__meerkat_binding_key"),
    )
    .expect("binding key global")
    .as_string()
    .expect("binding key string");
    assert_eq!(recorded, "browser:openai_host");
    match envelope {
        meerkat_core::ResolvedAuthEnvelope::InlineSecret { secret, .. } => {
            assert_eq!(secret, "bearer-browser:openai_host");
        }
        other => panic!("unexpected resolver envelope: {other:?}"),
    }
}
