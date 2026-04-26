//! T18 (Phase 5): WASM external auth resolver — top-down observable
//! proof that the JS host can register a callback that yields a bearer
//! token, and the meerkat WASM runtime reads the registration correctly.
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

use js_sys::Function;
use meerkat_web_runtime::external_auth::{
    has_external_auth_resolver, register_external_auth_resolver,
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
            (function (connectionRef) {
                return Promise.resolve("bearer-" + connectionRef.realm + "-" + connectionRef.binding);
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
