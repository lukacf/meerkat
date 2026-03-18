#![cfg(target_arch = "wasm32")]

use js_sys::Function;
use meerkat_web_runtime::{
    append_system_context, clear_tool_callbacks, create_session_simple, destroy_session,
    get_session_state, init_runtime_from_config, inspect_mobpack, poll_events, register_js_tool,
    register_tool_callback, start_turn,
};
use serde_json::{Value, json};
use wasm_bindgen::JsValue;
use wasm_bindgen_test::wasm_bindgen_test;

fn parse_js_error(value: JsValue) -> Value {
    let raw = value.as_string().expect("error string");
    serde_json::from_str(&raw).expect("error json")
}

fn parse_js_result(value: JsValue) -> Value {
    let raw = value.as_string().expect("result string");
    serde_json::from_str(&raw).expect("result json")
}

fn install_tool_use_fetch_stub() {
    js_sys::eval(
        r#"
globalThis.__meerkat_fetch_count = 0;
globalThis.fetch = async function(input, init) {
  globalThis.__meerkat_fetch_count += 1;
  const first = [
    'data: {"type":"message_start","message":{"usage":{"input_tokens":1,"output_tokens":0}}}\n\n',
    'data: {"type":"content_block_start","content_block":{"type":"tool_use","id":"tu_1","name":"echo_browser"}}\n\n',
    'data: {"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\\"value\\":\\"browser\\"}"}}\n\n',
    'data: {"type":"content_block_stop"}\n\n',
    'data: {"type":"message_delta","usage":{"output_tokens":1},"delta":{"stop_reason":"tool_use"}}\n\n',
    'data: {"type":"message_stop"}\n\n',
  ].join('');
  const second = [
    'data: {"type":"message_start","message":{"usage":{"input_tokens":1,"output_tokens":0}}}\n\n',
    'data: {"type":"content_block_start","content_block":{"type":"text","text":""}}\n\n',
    'data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"browser tool contract ok"}}\n\n',
    'data: {"type":"content_block_stop"}\n\n',
    'data: {"type":"message_delta","usage":{"output_tokens":4},"delta":{"stop_reason":"end_turn"}}\n\n',
    'data: {"type":"message_stop"}\n\n',
  ].join('');
  const body = globalThis.__meerkat_fetch_count === 1 ? first : second;
  const response = new Response(body, {
    status: 200,
    headers: { 'content-type': 'text/event-stream' },
  });
  Object.defineProperty(response, 'url', {
    value: 'https://example.test/anthropic/v1/messages',
  });
  return response;
};
"#,
    )
    .expect("install fetch stub");
}

fn build_mobpack(capabilities: &[&str]) -> Vec<u8> {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::Builder;

    let manifest = if capabilities.is_empty() {
        r#"[mobpack]
name = "browser-contract"
version = "0.1.0"
"#
        .to_string()
    } else {
        format!(
            "[mobpack]\nname = \"browser-contract\"\nversion = \"0.1.0\"\n\n[requires]\ncapabilities = [{}]\n",
            capabilities
                .iter()
                .map(|cap| format!("\"{cap}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    let mut archive = Builder::new(Vec::new());

    let mut manifest_header = tar::Header::new_gnu();
    manifest_header
        .set_path("manifest.toml")
        .expect("manifest path");
    manifest_header.set_size(manifest.len() as u64);
    manifest_header.set_mode(0o644);
    manifest_header.set_cksum();
    archive
        .append(&manifest_header, manifest.as_bytes())
        .expect("append manifest");

    let definition = r#"{ "id": "browser-contract-mob" }"#;
    let mut definition_header = tar::Header::new_gnu();
    definition_header
        .set_path("definition.json")
        .expect("definition path");
    definition_header.set_size(definition.len() as u64);
    definition_header.set_mode(0o644);
    definition_header.set_cksum();
    archive
        .append(&definition_header, definition.as_bytes())
        .expect("append definition");

    let tar_bytes = archive.into_inner().expect("tar bytes");
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut encoder, &tar_bytes).expect("write gzip bytes");
    encoder.finish().expect("finish gzip")
}

#[wasm_bindgen_test(async)]
#[ignore]
async fn browser_contract_requires_bootstrap_and_uses_runtime_backed_sessions_tools_and_events() {
    let not_initialized = create_session_simple(
        &json!({
            "model": "claude-sonnet-4-5",
            "api_key": "sk-test"
        })
        .to_string(),
    )
    .expect_err("session creation should require runtime bootstrap");
    assert_eq!(parse_js_error(not_initialized)["code"], "not_initialized");

    let callback = Function::new_with_args(
        "args",
        "return Promise.resolve(JSON.stringify({ content: args, is_error: false }));",
    );
    let tool_error = register_tool_callback(
        "echo_browser".to_string(),
        "Echo browser payloads".to_string(),
        json!({
            "type": "object",
            "properties": {
                "value": { "type": "string" }
            },
            "required": ["value"]
        })
        .to_string(),
        callback.clone().into(),
    )
    .expect_err("tool registration should require runtime bootstrap");
    assert_eq!(parse_js_error(tool_error)["code"], "not_initialized");

    let js_tool_error = register_js_tool(
        "notify_browser".to_string(),
        "Emit a browser-side notification".to_string(),
        json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" }
            },
            "required": ["message"]
        })
        .to_string(),
    )
    .expect_err("fire-and-forget tool registration should require runtime bootstrap");
    assert_eq!(parse_js_error(js_tool_error)["code"], "not_initialized");

    let init = parse_js_result(
        init_runtime_from_config(
            &json!({
                "anthropic_api_key": "sk-test",
                "model": "claude-sonnet-4-5"
            })
            .to_string(),
        )
        .expect("init runtime"),
    );
    assert_eq!(init["status"], "initialized");

    register_tool_callback(
        "echo_browser".to_string(),
        "Echo browser payloads".to_string(),
        json!({
            "type": "object",
            "properties": {
                "value": { "type": "string" }
            },
            "required": ["value"]
        })
        .to_string(),
        callback.into(),
    )
    .expect("register runtime-scoped tool");
    register_js_tool(
        "notify_browser".to_string(),
        "Emit a browser-side notification".to_string(),
        json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" }
            },
            "required": ["message"]
        })
        .to_string(),
    )
    .expect("register runtime-scoped fire-and-forget tool");

    install_tool_use_fetch_stub();

    let handle = create_session_simple(
        &json!({
            "model": "claude-sonnet-4-5",
            "api_key": "sk-test",
            "base_url": "https://example.test/anthropic",
            "anthropic_base_url": "https://example.test/anthropic"
        })
        .to_string(),
    )
    .expect("create direct session façade");

    let staged = parse_js_result(
        append_system_context(
            handle,
            &json!({
                "text": "Remember the browser contract.",
                "source": "browser_contract",
                "idempotency_key": "browser-contract-ctx"
            })
            .to_string(),
        )
        .expect("append system context"),
    );
    assert_eq!(staged["handle"], handle);
    assert_eq!(staged["status"], "staged");

    let before: Value =
        serde_json::from_str(&get_session_state(handle).expect("session state before turn"))
            .expect("state json");
    let session_id = before["session_id"]
        .as_str()
        .expect("runtime-backed session id")
        .to_string();
    assert_eq!(before["handle"], handle);
    assert_ne!(session_id, handle.to_string());
    assert_eq!(before["run_counter"], 0);

    let turn = parse_js_result(
        start_turn(handle, "Use the echo_browser tool, then answer.", "{}")
            .await
            .expect("start turn"),
    );
    assert_eq!(turn["status"], "completed");
    assert_eq!(turn["session_id"], session_id);
    assert!(turn["tool_calls"].as_u64().unwrap_or_default() >= 1);
    assert_eq!(turn["text"], "browser tool contract ok");

    let events: Value =
        serde_json::from_str(&poll_events(handle).expect("poll events")).expect("events json");
    let items = events.as_array().expect("event array");
    assert!(
        items
            .iter()
            .any(|item| item["type"] == "tool_call_requested")
    );
    assert!(
        items
            .iter()
            .any(|item| item["type"] == "tool_execution_completed")
    );
    assert!(items.iter().any(|item| item["type"] == "text_complete"));

    let after: Value =
        serde_json::from_str(&get_session_state(handle).expect("session state after turn"))
            .expect("state json");
    assert_eq!(after["session_id"], session_id);
    assert_eq!(after["run_counter"], 1);

    clear_tool_callbacks();
    destroy_session(handle).expect("destroy session");
    assert!(get_session_state(handle).is_err());
}

#[wasm_bindgen_test]
#[ignore]
fn browser_contract_rejects_forbidden_browser_capabilities_before_session_creation() {
    let blocked = inspect_mobpack(&build_mobpack(&["shell"]))
        .expect_err("forbidden browser capability should be rejected");
    let error = parse_js_error(blocked);
    assert_eq!(error["code"], "invalid_mobpack");
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("forbidden capability 'shell'")),
        "unexpected invalid_mobpack payload: {error}"
    );

    let allowed = inspect_mobpack(&build_mobpack(&[])).expect("safe mobpack should inspect");
    let inspected: Value =
        serde_json::from_str(&allowed).expect("successful mobpack inspection json");
    assert_eq!(inspected["definition"]["id"], "browser-contract-mob");
}
