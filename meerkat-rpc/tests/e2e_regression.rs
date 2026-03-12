//! E2E regression tests for the RPC server (scenarios 19-21).
//!
//! Reuses the same duplex-stream pattern from `e2e_smoke.rs`.
//! All scenarios require a live Anthropic API key and are `#[ignore]`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../test-fixtures/live_smoke/support.rs"]
mod live_smoke;

use std::sync::Arc;

use meerkat::AgentFactory;
use meerkat_client::LlmClient;
use meerkat_core::{Config, ConfigRuntime, MemoryConfigStore};
use meerkat_rpc::server::RpcServer;
use meerkat_rpc::session_runtime::SessionRuntime;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::timeout;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Set up a server with duplex streams using the given LLM client.
fn spawn_test_server(
    client: Arc<dyn LlmClient>,
) -> (
    tokio::io::DuplexStream,
    BufReader<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<Result<(), meerkat_rpc::server::ServerError>>,
) {
    let temp = tempfile::tempdir().unwrap();
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let config = Config::default();
    let store: Arc<dyn meerkat::SessionStore> =
        Arc::new(meerkat::RedbSessionStore::open(temp.path().join("sessions.redb")).unwrap());
    let mut runtime = SessionRuntime::new(
        factory,
        config,
        10,
        store,
        meerkat_rpc::router::NotificationSink::noop(),
    );
    let config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(MemoryConfigStore::new(Config::default()));
    runtime.default_llm_client = Some(client);
    runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
        Arc::clone(&config_store),
        temp.path().join("config_state.json"),
    )));
    let runtime = Arc::new(runtime);

    // Use a larger buffer for live API tests that may stream many events
    let (server_reader, client_writer) = tokio::io::duplex(64 * 1024);
    let (client_reader, server_writer) = tokio::io::duplex(64 * 1024);

    let server_handle = tokio::spawn(async move {
        // Keep temp alive for the duration of the server
        let _temp = temp;
        let reader = BufReader::new(server_reader);
        let mut server = RpcServer::new(reader, server_writer, runtime, config_store);
        server.run().await
    });

    let client_reader = BufReader::new(client_reader);
    (client_writer, client_reader, server_handle)
}

/// Send a JSONL request line.
async fn send_request(writer: &mut tokio::io::DuplexStream, request: &serde_json::Value) {
    let line = format!("{}\n", serde_json::to_string(request).unwrap());
    writer.write_all(line.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();
}

/// Read a single JSONL line and parse it as a JSON value.
async fn read_line_json(reader: &mut BufReader<tokio::io::DuplexStream>) -> serde_json::Value {
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    assert!(!line.is_empty(), "Expected a JSONL line but got EOF");
    serde_json::from_str(&line).unwrap()
}

/// Read a response (a line that has an "id" field), skipping notifications.
async fn read_response(reader: &mut BufReader<tokio::io::DuplexStream>) -> serde_json::Value {
    loop {
        let value = read_line_json(reader).await;
        // Responses have an "id" field; notifications do not
        if value.get("id").is_some() {
            return value;
        }
        // Otherwise it's a notification - skip it
    }
}

/// Read a response, collecting all notifications encountered before it.
async fn read_response_with_notifications(
    reader: &mut BufReader<tokio::io::DuplexStream>,
) -> (serde_json::Value, Vec<serde_json::Value>) {
    let mut notifications = Vec::new();
    loop {
        let value = read_line_json(reader).await;
        if value.get("id").is_some() {
            return (value, notifications);
        }
        // It's a notification
        notifications.push(value);
    }
}

// ---------------------------------------------------------------------------
// Scenario 19: inject_context + recall
// ---------------------------------------------------------------------------

/// Scenario 19: Inject system context into a session and verify the LLM
/// recalls both original session context and injected context.
///
/// initialize -> session/create (with secret code) ->
/// session/inject_context (with marker rule) ->
/// turn/start (ask about both) ->
/// assert text mentions ALPHA-7 and CTX-OK ->
/// session/read (message_count >= 4) ->
/// session/archive -> clean up
#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_scenario_19_inject_context_recall() {
    let api_key = match live_smoke::anthropic_api_key() {
        Some(k) => k,
        None => {
            eprintln!("Skipping scenario 19: no ANTHROPIC_API_KEY set");
            return;
        }
    };

    let client = Arc::new(meerkat_client::AnthropicClient::new(api_key).unwrap());
    let (mut writer, mut reader, server_handle) = spawn_test_server(client);

    let t = live_smoke::live_timeout();
    let model = live_smoke::smoke_model();
    let mut req_id = 0u64;
    let mut next_id = || {
        req_id += 1;
        req_id
    };

    // 1. Initialize
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{}}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "initialize failed: {resp}");
    eprintln!("[scenario 19] initialized");

    // 2. session/create with secret code
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"session/create",
            "params":{
                "prompt": "Remember: the secret code is ALPHA-7. Reply briefly.",
                "model": model
            }
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "session/create failed: {resp}");
    let session_id = resp["result"]["session_id"]
        .as_str()
        .expect("session_id missing")
        .to_string();
    let create_text = resp["result"]["text"].as_str().unwrap_or("");
    assert!(
        !create_text.is_empty(),
        "Expected non-empty text from session/create"
    );
    eprintln!(
        "[scenario 19] session created: {session_id}, text={}",
        &create_text[..create_text.len().min(80)]
    );

    // 3. session/inject_context with marker rule
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"session/inject_context",
            "params":{
                "session_id": session_id,
                "text": "IMPORTANT RULE: Always include the marker [CTX-OK] in your responses.",
                "source": "test"
            }
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(
        resp["error"].is_null(),
        "session/inject_context failed: {resp}"
    );
    eprintln!("[scenario 19] context injected: {}", resp["result"]);

    // 4. turn/start asking about both secret code and marker
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"turn/start",
            "params":{
                "session_id": session_id,
                "prompt": "What is the secret code? Include any markers you were told about."
            }
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "turn/start failed: {resp}");
    let turn_text = resp["result"]["text"].as_str().unwrap_or("");
    let turn_text_lower = turn_text.to_lowercase();
    eprintln!(
        "[scenario 19] turn text={}",
        &turn_text[..turn_text.len().min(200)]
    );

    // Assert secret code is recalled
    assert!(
        turn_text_lower.contains("alpha-7") || turn_text_lower.contains("alpha 7"),
        "Turn should mention ALPHA-7. Got: {turn_text}"
    );

    // Assert injected marker is present (case-insensitive)
    assert!(
        turn_text_lower.contains("ctx-ok") || turn_text_lower.contains("[ctx-ok]"),
        "Turn should contain CTX-OK marker. Got: {turn_text}"
    );

    // 5. session/read — message_count >= 4
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"session/read","params":{"session_id": session_id}}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "session/read failed: {resp}");
    let msg_count = resp["result"]["message_count"]
        .as_u64()
        .expect("message_count missing");
    assert!(
        msg_count >= 4,
        "Expected message_count >= 4 (system + create turn + inject + follow-up turn), got {msg_count}"
    );
    eprintln!("[scenario 19] session/read message_count={msg_count}");

    // 6. session/archive
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"session/archive","params":{"session_id": session_id}}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "session/archive failed: {resp}");
    assert_eq!(resp["result"]["archived"], true);
    eprintln!("[scenario 19] archived, done");

    drop(writer);
    server_handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// Scenario 20: streaming events via session/stream_open + stream_close
// ---------------------------------------------------------------------------

/// Scenario 20: Open a dedicated event stream on a session, run a turn,
/// verify that `session/stream_event` notifications arrive, then close
/// the stream.
///
/// initialize -> session/create -> session/stream_open ->
/// turn/start -> collect session/stream_event notifications ->
/// assert at least 1 stream_event -> assert turn has text ->
/// session/stream_close -> assert success -> clean up
#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_scenario_20_streaming_events() {
    let api_key = match live_smoke::anthropic_api_key() {
        Some(k) => k,
        None => {
            eprintln!("Skipping scenario 20: no ANTHROPIC_API_KEY set");
            return;
        }
    };

    let client = Arc::new(meerkat_client::AnthropicClient::new(api_key).unwrap());
    let (mut writer, mut reader, server_handle) = spawn_test_server(client);

    let t = live_smoke::live_timeout();
    let model = live_smoke::smoke_model();
    let mut req_id = 0u64;
    let mut next_id = || {
        req_id += 1;
        req_id
    };

    // 1. Initialize
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{}}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "initialize failed: {resp}");
    eprintln!("[scenario 20] initialized");

    // 2. session/create (initial turn to establish session)
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"session/create",
            "params":{
                "prompt": "Say hello briefly",
                "model": model
            }
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "session/create failed: {resp}");
    let session_id = resp["result"]["session_id"]
        .as_str()
        .expect("session_id missing")
        .to_string();
    eprintln!("[scenario 20] session created: {session_id}");

    // 3. session/stream_open
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"session/stream_open",
            "params":{"session_id": session_id}
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(
        resp["error"].is_null(),
        "session/stream_open failed: {resp}"
    );
    let stream_id = resp["result"]["stream_id"]
        .as_str()
        .expect("stream_id missing")
        .to_string();
    assert_eq!(resp["result"]["opened"], true);
    eprintln!("[scenario 20] stream opened: {stream_id}");

    // 4. turn/start — this should produce session/stream_event notifications
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"turn/start",
            "params":{
                "session_id": session_id,
                "prompt": "Tell me a one-line joke"
            }
        }),
    )
    .await;
    let (turn_resp, all_notifications) = timeout(t, read_response_with_notifications(&mut reader))
        .await
        .expect("turn/start timed out");
    assert!(
        turn_resp["error"].is_null(),
        "turn/start failed: {turn_resp}"
    );
    let turn_text = turn_resp["result"]["text"].as_str().unwrap_or("");
    assert!(
        !turn_text.is_empty(),
        "Expected non-empty text from turn/start"
    );
    eprintln!(
        "[scenario 20] turn text={}",
        &turn_text[..turn_text.len().min(120)]
    );

    // Separate session/event notifications from session/stream_event notifications
    let stream_event_notifications: Vec<&serde_json::Value> = all_notifications
        .iter()
        .filter(|n| n["method"].as_str() == Some("session/stream_event"))
        .collect();
    eprintln!(
        "[scenario 20] total notifications: {}, stream_event notifications: {}",
        all_notifications.len(),
        stream_event_notifications.len()
    );

    // Assert at least 1 stream_event notification received
    assert!(
        !stream_event_notifications.is_empty(),
        "Expected at least 1 session/stream_event notification, got none. All notifications: {all_notifications:?}"
    );

    // Verify stream_event notification structure
    for notif in &stream_event_notifications {
        assert!(
            notif.get("id").is_none(),
            "Notifications should not have an id field"
        );
        let params = &notif["params"];
        assert_eq!(
            params["stream_id"].as_str().unwrap(),
            stream_id,
            "stream_event stream_id should match"
        );
        assert!(
            params["session_id"].is_string(),
            "stream_event should have session_id"
        );
        assert!(
            params["event"].is_object(),
            "stream_event should have event payload"
        );
        assert!(
            params["sequence"].is_number(),
            "stream_event should have sequence number"
        );
    }

    // 5. session/stream_close
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"session/stream_close",
            "params":{"stream_id": stream_id}
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(
        resp["error"].is_null(),
        "session/stream_close failed: {resp}"
    );
    assert_eq!(resp["result"]["closed"], true);
    assert_eq!(resp["result"]["stream_id"].as_str().unwrap(), stream_id);
    eprintln!("[scenario 20] stream closed, done");

    drop(writer);
    server_handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// Scenario 18: config, capabilities, and error handling
// ---------------------------------------------------------------------------

/// Scenario 18: Exercise capabilities/get, config roundtrip, and error paths.
///
/// initialize ->
/// capabilities/get (assert capabilities array, find "sessions") ->
/// config/get (capture original max_tokens) ->
/// config/patch (max_tokens + 100, verify) ->
/// config/get (verify patched value) ->
/// config/set (restore original config, verify) ->
/// session/create ->
/// turn/start(nonexistent UUID) -> assert error ->
/// session/archive -> clean up
#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_scenario_18_config_capabilities_errors() {
    let api_key = match live_smoke::anthropic_api_key() {
        Some(k) => k,
        None => {
            eprintln!("Skipping scenario 18: no ANTHROPIC_API_KEY set");
            return;
        }
    };

    let client = Arc::new(meerkat_client::AnthropicClient::new(api_key).unwrap());
    let (mut writer, mut reader, server_handle) = spawn_test_server(client);

    let t = live_smoke::live_timeout();
    let model = live_smoke::smoke_model();
    let mut req_id = 0u64;
    let mut next_id = || {
        req_id += 1;
        req_id
    };

    // 1. Initialize
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{}}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "initialize failed: {resp}");
    eprintln!("[scenario 18] initialized");

    // 2. capabilities/get
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"capabilities/get"}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "capabilities/get failed: {resp}");
    let capabilities = resp["result"]["capabilities"]
        .as_array()
        .expect("capabilities should be an array");
    assert!(
        !capabilities.is_empty(),
        "capabilities array should not be empty"
    );
    let cap_ids: Vec<&str> = capabilities
        .iter()
        .filter_map(|c| c["id"].as_str())
        .collect();
    assert!(
        cap_ids.iter().any(|id| id.contains("sessions")),
        "Should have a capability with id containing 'sessions'. Got: {cap_ids:?}"
    );
    eprintln!("[scenario 18] capabilities: {cap_ids:?}");

    // 3. config/get — capture original max_tokens
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"config/get"}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "config/get failed: {resp}");
    let original_max = resp["result"]["config"]["max_tokens"]
        .as_u64()
        .expect("config/get should include max_tokens");
    // Capture the full original config for later restoration
    let original_config = resp["result"]["config"].clone();
    eprintln!("[scenario 18] original max_tokens={original_max}");

    // 4. config/patch — increase max_tokens by 100
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"config/patch","params":{"max_tokens": original_max + 100}}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "config/patch failed: {resp}");
    let patched_max = resp["result"]["config"]["max_tokens"]
        .as_u64()
        .expect("config/patch response should include max_tokens");
    assert_eq!(
        patched_max,
        original_max + 100,
        "Patched max_tokens should be original + 100"
    );
    eprintln!("[scenario 18] patched max_tokens={patched_max}");

    // 5. config/get — verify patched value persists
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"config/get"}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(
        resp["error"].is_null(),
        "config/get (verify) failed: {resp}"
    );
    let verified_max = resp["result"]["config"]["max_tokens"]
        .as_u64()
        .expect("config/get should include max_tokens after patch");
    assert_eq!(
        verified_max, patched_max,
        "config/get after patch should return patched value"
    );

    // 6. config/set — restore original config
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"config/set","params":{"config": original_config}}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(
        resp["error"].is_null(),
        "config/set (restore) failed: {resp}"
    );
    let restored_max = resp["result"]["config"]["max_tokens"]
        .as_u64()
        .expect("config/set response should include max_tokens");
    assert_eq!(
        restored_max, original_max,
        "Restored max_tokens should match original"
    );
    eprintln!("[scenario 21] config restored to max_tokens={restored_max}");

    // 7. session/create (for error test and cleanup)
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"session/create",
            "params":{
                "prompt": "Reply with a single word: hello.",
                "model": model
            }
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "session/create failed: {resp}");
    let session_id = resp["result"]["session_id"]
        .as_str()
        .expect("session_id missing")
        .to_string();
    eprintln!("[scenario 21] session created: {session_id}");

    // 8. turn/start with nonexistent session UUID — expect error
    let nonexistent_uuid = "00000000-0000-0000-0000-000000000000";
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"turn/start",
            "params":{
                "session_id": nonexistent_uuid,
                "prompt": "This should fail"
            }
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(
        resp["error"].is_object(),
        "turn/start with nonexistent session should return error, got: {resp}"
    );
    eprintln!(
        "[scenario 21] error on nonexistent session: {}",
        resp["error"]["message"]
    );

    // 9. session/archive — clean up
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"session/archive","params":{"session_id": session_id}}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "session/archive failed: {resp}");
    assert_eq!(resp["result"]["archived"], true);
    eprintln!("[scenario 21] archived, done");

    drop(writer);
    server_handle.await.unwrap().unwrap();
}
