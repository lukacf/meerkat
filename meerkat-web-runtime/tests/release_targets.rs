#![cfg(target_arch = "wasm32")]

use meerkat_web_runtime::{create_session_simple, init_runtime_from_config};
use serde_json::json;
use wasm_bindgen_test::wasm_bindgen_test;

fn parse_js_error(raw: wasm_bindgen::JsValue) -> serde_json::Value {
    serde_json::from_str(&raw.as_string().expect("error string")).expect("error json")
}

#[wasm_bindgen_test(async)]
#[ignore]
async fn release_targets_red_ok_browser_runtime_bootstrap_remains_explicit() {
    let err = create_session_simple(
        &json!({
            "model": "claude-sonnet-4-5",
            "api_key": "sk-test"
        })
        .to_string(),
    )
    .expect_err("session creation should require bootstrap");
    assert_eq!(parse_js_error(err)["code"], "not_initialized");

    let initialized = init_runtime_from_config(
        &json!({
            "anthropic_api_key": "sk-test",
            "model": "claude-sonnet-4-5"
        })
        .to_string(),
    )
    .expect("runtime init");
    assert!(
        initialized.as_string().is_some(),
        "runtime bootstrap should return a JSON payload"
    );
}
