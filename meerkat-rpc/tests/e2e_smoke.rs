//! E2E smoke tests for the RPC server (scenarios 15-17).
//!
//! Reuses the same duplex-stream pattern from `integration_server.rs`.
//! Scenarios 15 and 17 require a live Anthropic API key and are `#[ignore]`.
//! Scenario 16 uses a mock LLM client and runs in the normal test suite.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream;
use meerkat::AgentFactory;
use meerkat_client::{LlmClient, LlmError};
use meerkat_core::{Config, MemoryConfigStore, StopReason};
use meerkat_rpc::server::RpcServer;
use meerkat_rpc::session_runtime::SessionRuntime;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::timeout;

// ---------------------------------------------------------------------------
// Mock LLM client
// ---------------------------------------------------------------------------

struct MockLlmClient;

#[async_trait]
impl LlmClient for MockLlmClient {
    fn stream<'a>(
        &'a self,
        _request: &'a meerkat_client::LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>>
    {
        Box::pin(stream::iter(vec![
            Ok(meerkat_client::LlmEvent::TextDelta {
                delta: "Hello from mock".to_string(),
                meta: None,
            }),
            Ok(meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::Success {
                    stop_reason: StopReason::EndTurn,
                },
            }),
        ]))
    }

    fn provider(&self) -> &'static str {
        "mock"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the Anthropic API key if set, or `None`.
fn anthropic_api_key() -> Option<String> {
    for var in &["ANTHROPIC_API_KEY", "RKAT_ANTHROPIC_API_KEY"] {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                return Some(val);
            }
        }
    }
    None
}

/// Model to use for live smoke tests (defaults to a cheaper model).
fn smoke_model() -> String {
    std::env::var("SMOKE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string())
}

/// Set up a server with duplex streams using the MockLlmClient.
///
/// Returns:
/// - `client_writer`: write JSONL requests here
/// - `client_reader`: read JSONL responses/notifications here (wrapped in BufReader)
/// - `server_handle`: JoinHandle for the server task
fn spawn_test_server() -> (
    tokio::io::DuplexStream,
    BufReader<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<Result<(), meerkat_rpc::server::ServerError>>,
) {
    spawn_test_server_with_client(Arc::new(MockLlmClient))
}

/// Set up a server with duplex streams using the given LLM client.
///
/// This variant allows injecting a real LLM client for live API tests.
fn spawn_test_server_with_client(
    client: Arc<dyn LlmClient>,
) -> (
    tokio::io::DuplexStream,
    BufReader<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<Result<(), meerkat_rpc::server::ServerError>>,
) {
    let temp = tempfile::tempdir().unwrap();
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let config = Config::default();
    let mut runtime = SessionRuntime::new(factory, config, 10);
    runtime.default_llm_client = Some(client);
    let runtime = Arc::new(runtime);
    let config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(MemoryConfigStore::new(Config::default()));

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
// Scenario 15: Full RPC conversation flow (live API)
// ---------------------------------------------------------------------------

/// Scenario 15: Full RPC conversation flow with a live Anthropic API.
///
/// Initialize -> session/create (real Anthropic) -> turn/start follow-up ->
/// session/read -> session/list (verify present) -> session/archive ->
/// session/list (verify gone).
#[tokio::test]
#[ignore = "e2e: live API"]
async fn e2e_scenario_15_full_rpc_conversation_flow() {
    let api_key = match anthropic_api_key() {
        Some(key) => key,
        None => {
            eprintln!("Skipping scenario 15: no ANTHROPIC_API_KEY set");
            return;
        }
    };

    let client = Arc::new(meerkat_client::AnthropicClient::new(api_key).unwrap());
    let (mut writer, mut reader, server_handle) = spawn_test_server_with_client(client);

    let live_timeout = Duration::from_secs(120);

    // 1. Initialize
    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    });
    send_request(&mut writer, &init_req).await;
    let init_resp = timeout(live_timeout, read_response(&mut reader))
        .await
        .expect("initialize timed out");
    assert_eq!(init_resp["id"], 1);
    assert!(
        init_resp["error"].is_null(),
        "initialize failed: {}",
        init_resp
    );
    assert_eq!(init_resp["result"]["server_info"]["name"], "meerkat-rpc");

    // 2. session/create with real Anthropic
    let model = smoke_model();
    let create_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/create",
        "params": {
            "prompt": "My name is SmokeBot and my favorite color is teal. Reply briefly.",
            "model": model
        }
    });
    send_request(&mut writer, &create_req).await;
    let create_resp = timeout(live_timeout, read_response(&mut reader))
        .await
        .expect("session/create timed out");
    assert_eq!(create_resp["id"], 2);
    assert!(
        create_resp["error"].is_null(),
        "session/create failed: {}",
        create_resp
    );
    let session_id = create_resp["result"]["session_id"]
        .as_str()
        .expect("session_id missing")
        .to_string();
    assert!(!session_id.is_empty());
    // The LLM should have returned some text
    let create_text = create_resp["result"]["text"].as_str().unwrap_or("");
    assert!(
        !create_text.is_empty(),
        "Expected non-empty text from LLM, got empty"
    );

    // 3. turn/start follow-up
    let turn_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "turn/start",
        "params": {
            "session_id": session_id,
            "prompt": "What is my name and what is my favorite color? Reply in one sentence."
        }
    });
    send_request(&mut writer, &turn_req).await;
    let turn_resp = timeout(live_timeout, read_response(&mut reader))
        .await
        .expect("turn/start timed out");
    assert_eq!(turn_resp["id"], 3);
    assert!(
        turn_resp["error"].is_null(),
        "turn/start failed: {}",
        turn_resp
    );
    assert_eq!(
        turn_resp["result"]["session_id"].as_str().unwrap(),
        session_id
    );
    let turn_text = turn_resp["result"]["text"]
        .as_str()
        .unwrap()
        .to_lowercase();
    assert!(
        turn_text.contains("smokebot") || turn_text.contains("smoke"),
        "Follow-up should recall the name. Got: {}",
        turn_text
    );
    assert!(
        turn_text.contains("teal"),
        "Follow-up should recall the color. Got: {}",
        turn_text
    );

    // 4. session/read
    let read_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/read",
        "params": {"session_id": session_id}
    });
    send_request(&mut writer, &read_req).await;
    let read_resp = timeout(live_timeout, read_response(&mut reader))
        .await
        .expect("session/read timed out");
    assert_eq!(read_resp["id"], 4);
    assert!(
        read_resp["error"].is_null(),
        "session/read failed: {}",
        read_resp
    );
    assert_eq!(
        read_resp["result"]["session_id"].as_str().unwrap(),
        session_id
    );
    assert_eq!(read_resp["result"]["state"].as_str().unwrap(), "idle");

    // 5. session/list (verify present)
    let list_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "session/list"
    });
    send_request(&mut writer, &list_req).await;
    let list_resp = timeout(live_timeout, read_response(&mut reader))
        .await
        .expect("session/list timed out");
    assert_eq!(list_resp["id"], 5);
    assert!(
        list_resp["error"].is_null(),
        "session/list failed: {}",
        list_resp
    );
    let sessions = list_resp["result"]["sessions"].as_array().unwrap();
    let ids: Vec<&str> = sessions
        .iter()
        .filter_map(|s| s["session_id"].as_str())
        .collect();
    assert!(
        ids.contains(&session_id.as_str()),
        "Session should appear in list. Got: {:?}",
        ids
    );

    // 6. session/archive
    let archive_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "session/archive",
        "params": {"session_id": session_id}
    });
    send_request(&mut writer, &archive_req).await;
    let archive_resp = timeout(live_timeout, read_response(&mut reader))
        .await
        .expect("session/archive timed out");
    assert_eq!(archive_resp["id"], 6);
    assert!(
        archive_resp["error"].is_null(),
        "session/archive failed: {}",
        archive_resp
    );
    assert_eq!(archive_resp["result"]["archived"], true);

    // 7. session/list (verify gone)
    let list_req2 = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "session/list"
    });
    send_request(&mut writer, &list_req2).await;
    let list_resp2 = timeout(live_timeout, read_response(&mut reader))
        .await
        .expect("session/list (after archive) timed out");
    assert_eq!(list_resp2["id"], 7);
    assert!(
        list_resp2["error"].is_null(),
        "session/list (after archive) failed: {}",
        list_resp2
    );
    let sessions2 = list_resp2["result"]["sessions"].as_array().unwrap();
    let ids2: Vec<&str> = sessions2
        .iter()
        .filter_map(|s| s["session_id"].as_str())
        .collect();
    assert!(
        !ids2.contains(&session_id.as_str()),
        "Archived session should NOT appear in list. Got: {:?}",
        ids2
    );

    // Clean up
    drop(writer);
    server_handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// Scenario 16: RPC capabilities + config round-trip (mock LLM)
// ---------------------------------------------------------------------------

/// Scenario 16: Capabilities and config round-trip using mock LLM.
///
/// Initialize -> capabilities/get (verify structure) -> config/get ->
/// config/patch (change max_tokens) -> config/get (verify patched).
/// No API key needed.
#[tokio::test]
async fn e2e_scenario_16_capabilities_and_config_roundtrip() {
    let (mut writer, mut reader, server_handle) = spawn_test_server();

    // 1. Initialize
    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    });
    send_request(&mut writer, &init_req).await;
    let init_resp = read_response(&mut reader).await;
    assert_eq!(init_resp["id"], 1);
    assert!(
        init_resp["error"].is_null(),
        "initialize failed: {}",
        init_resp
    );
    assert_eq!(init_resp["result"]["server_info"]["name"], "meerkat-rpc");

    // 2. capabilities/get
    let caps_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "capabilities/get"
    });
    send_request(&mut writer, &caps_req).await;
    let caps_resp = read_response(&mut reader).await;
    assert_eq!(caps_resp["id"], 2);
    assert!(
        caps_resp["error"].is_null(),
        "capabilities/get failed: {}",
        caps_resp
    );

    // Verify capabilities structure
    let caps_result = &caps_resp["result"];
    assert!(
        caps_result["contract_version"].is_object(),
        "Expected contract_version object, got: {}",
        caps_result
    );
    assert!(
        caps_result["contract_version"]["major"].is_u64(),
        "contract_version should have numeric major field"
    );
    let capabilities = caps_result["capabilities"]
        .as_array()
        .expect("capabilities should be an array");
    assert!(
        !capabilities.is_empty(),
        "capabilities array should not be empty"
    );

    // Each capability should have id, description, status
    for cap in capabilities {
        assert!(
            cap["id"].is_string(),
            "capability should have string id, got: {}",
            cap
        );
        assert!(
            cap["description"].is_string(),
            "capability should have string description, got: {}",
            cap
        );
        // Status is either a plain string ("Available") or an object ({"DisabledByPolicy": ...})
        assert!(
            cap["status"].is_string() || cap["status"].is_object(),
            "capability status should be a string or object, got: {}",
            cap
        );
    }

    // Verify "sessions" capability is present
    let cap_ids: Vec<&str> = capabilities
        .iter()
        .filter_map(|c| c["id"].as_str())
        .collect();
    assert!(
        cap_ids.contains(&"sessions"),
        "Expected 'sessions' capability, got: {:?}",
        cap_ids
    );

    // 3. config/get (initial)
    let config_get_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "config/get"
    });
    send_request(&mut writer, &config_get_req).await;
    let config_resp = read_response(&mut reader).await;
    assert_eq!(config_resp["id"], 3);
    assert!(
        config_resp["error"].is_null(),
        "config/get failed: {}",
        config_resp
    );
    let initial_max_tokens = config_resp["result"]["max_tokens"]
        .as_u64()
        .expect("max_tokens should be a number");

    // 4. config/patch (change max_tokens)
    let new_max_tokens = initial_max_tokens + 512;
    let patch_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "config/patch",
        "params": {"max_tokens": new_max_tokens}
    });
    send_request(&mut writer, &patch_req).await;
    let patch_resp = read_response(&mut reader).await;
    assert_eq!(patch_resp["id"], 4);
    assert!(
        patch_resp["error"].is_null(),
        "config/patch failed: {}",
        patch_resp
    );
    assert_eq!(
        patch_resp["result"]["max_tokens"], new_max_tokens,
        "Patched config should reflect new max_tokens"
    );

    // 5. config/get (verify patched value persists)
    let config_get_req2 = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "config/get"
    });
    send_request(&mut writer, &config_get_req2).await;
    let config_resp2 = read_response(&mut reader).await;
    assert_eq!(config_resp2["id"], 5);
    assert!(
        config_resp2["error"].is_null(),
        "config/get (after patch) failed: {}",
        config_resp2
    );
    assert_eq!(
        config_resp2["result"]["max_tokens"], new_max_tokens,
        "max_tokens should match patched value on re-read"
    );

    // Clean up
    drop(writer);
    server_handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// Scenario 17: RPC multi-turn with event streaming (live API)
// ---------------------------------------------------------------------------

/// Scenario 17: Multi-turn conversation with event streaming over RPC.
///
/// Initialize -> session/create -> collect session/event notifications ->
/// turn/start follow-up -> collect more events. Verify notification structure
/// and context maintained across turns.
#[tokio::test]
#[ignore = "e2e: live API"]
async fn e2e_scenario_17_multi_turn_event_streaming() {
    let api_key = match anthropic_api_key() {
        Some(key) => key,
        None => {
            eprintln!("Skipping scenario 17: no ANTHROPIC_API_KEY set");
            return;
        }
    };

    let client = Arc::new(meerkat_client::AnthropicClient::new(api_key).unwrap());
    let (mut writer, mut reader, server_handle) = spawn_test_server_with_client(client);

    let live_timeout = Duration::from_secs(120);

    // 1. Initialize
    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    });
    send_request(&mut writer, &init_req).await;
    let init_resp = timeout(live_timeout, read_response(&mut reader))
        .await
        .expect("initialize timed out");
    assert_eq!(init_resp["id"], 1);
    assert!(
        init_resp["error"].is_null(),
        "initialize failed: {}",
        init_resp
    );

    // 2. session/create - collect notifications emitted during the turn
    let model = smoke_model();
    let create_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/create",
        "params": {
            "prompt": "The secret passphrase is 'meerkat-sunrise-42'. Remember it. Reply with a brief acknowledgment.",
            "model": model
        }
    });
    send_request(&mut writer, &create_req).await;
    let (create_resp, create_notifications) =
        timeout(live_timeout, read_response_with_notifications(&mut reader))
            .await
            .expect("session/create timed out");
    assert_eq!(create_resp["id"], 2);
    assert!(
        create_resp["error"].is_null(),
        "session/create failed: {}",
        create_resp
    );
    let session_id = create_resp["result"]["session_id"]
        .as_str()
        .expect("session_id missing")
        .to_string();

    // Verify notifications from the first turn
    assert!(
        !create_notifications.is_empty(),
        "Expected at least one notification during session/create, got none"
    );
    for notif in &create_notifications {
        // Notifications have "method" but no "id"
        assert!(
            notif.get("id").is_none(),
            "Notifications should not have an id field: {}",
            notif
        );
        assert_eq!(
            notif["method"].as_str().unwrap(),
            "session/event",
            "Notification method should be session/event"
        );
        let params = &notif["params"];
        assert!(
            params["session_id"].is_string(),
            "Notification should have session_id: {}",
            notif
        );
        assert_eq!(
            params["session_id"].as_str().unwrap(),
            session_id,
            "Notification session_id should match"
        );
        assert!(
            params["event"].is_object(),
            "Notification should have event payload: {}",
            notif
        );
    }

    // 3. turn/start follow-up - collect notifications during the second turn
    let turn_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "turn/start",
        "params": {
            "session_id": session_id,
            "prompt": "What is the secret passphrase I told you? Repeat it exactly."
        }
    });
    send_request(&mut writer, &turn_req).await;
    let (turn_resp, turn_notifications) =
        timeout(live_timeout, read_response_with_notifications(&mut reader))
            .await
            .expect("turn/start timed out");
    assert_eq!(turn_resp["id"], 3);
    assert!(
        turn_resp["error"].is_null(),
        "turn/start failed: {}",
        turn_resp
    );
    assert_eq!(
        turn_resp["result"]["session_id"].as_str().unwrap(),
        session_id
    );

    // Verify context was maintained: the LLM should recall the passphrase
    let turn_text = turn_resp["result"]["text"]
        .as_str()
        .unwrap()
        .to_lowercase();
    assert!(
        turn_text.contains("meerkat-sunrise-42") || turn_text.contains("meerkat sunrise 42"),
        "Follow-up should recall the passphrase. Got: {}",
        turn_text
    );

    // Verify notifications from the second turn
    assert!(
        !turn_notifications.is_empty(),
        "Expected at least one notification during turn/start, got none"
    );
    for notif in &turn_notifications {
        assert_eq!(
            notif["method"].as_str().unwrap(),
            "session/event",
            "Notification method should be session/event"
        );
        assert_eq!(
            notif["params"]["session_id"].as_str().unwrap(),
            session_id,
            "Turn notification session_id should match"
        );
        assert!(
            notif["params"]["event"].is_object(),
            "Turn notification should have event payload"
        );
    }

    // Both turns produced notifications
    eprintln!(
        "Scenario 17: {} notifications from create, {} from follow-up turn",
        create_notifications.len(),
        turn_notifications.len()
    );

    // Clean up
    drop(writer);
    server_handle.await.unwrap().unwrap();
}
