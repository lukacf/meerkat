//! Integration tests for the RPC server.
//!
//! These tests exercise the full roundtrip: write JSONL requests to a stream,
//! read JSONL responses/notifications back. Uses `tokio::io::duplex` for
//! in-process paired channels.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream;
use meerkat::AgentFactory;
use meerkat_client::{LlmClient, LlmError};
use meerkat_core::{Config, ConfigRuntime, MemoryConfigStore, StopReason};
use meerkat_rpc::server::RpcServer;
use meerkat_rpc::session_runtime::SessionRuntime;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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

/// Set up a server with duplex streams and spawn it in a background task.
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
    let temp = tempfile::tempdir().unwrap();
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let config = Config::default();
    let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let mut runtime = SessionRuntime::new(factory, config, 10, store);
    let config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(MemoryConfigStore::new(Config::default()));
    runtime.default_llm_client = Some(Arc::new(MockLlmClient));
    runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
        Arc::clone(&config_store),
        temp.path().join("config_state.json"),
    )));
    let runtime = Arc::new(runtime);

    let (server_reader, client_writer) = tokio::io::duplex(4096);
    let (client_reader, server_writer) = tokio::io::duplex(4096);

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 1. Initialize roundtrip: send `initialize`, get capabilities response.
#[tokio::test]
async fn initialize_roundtrip() {
    let (mut writer, mut reader, server_handle) = spawn_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    });
    send_request(&mut writer, &req).await;

    let response = read_response(&mut reader).await;
    assert_eq!(response["id"], 1);
    assert!(
        response["error"].is_null(),
        "Expected success, got error: {}",
        response
    );
    assert_eq!(response["result"]["server_info"]["name"], "meerkat-rpc");
    assert!(response["result"]["server_info"]["version"].is_string());

    let methods = response["result"]["methods"].as_array().unwrap();
    let method_names: Vec<&str> = methods.iter().map(|m| m.as_str().unwrap()).collect();
    assert!(method_names.contains(&"session/create"));
    assert!(method_names.contains(&"turn/start"));
    assert!(method_names.contains(&"config/get"));

    // Close to trigger EOF
    drop(writer);
    server_handle.await.unwrap().unwrap();
}

/// 2. Session create and turn start: create session, then start a turn.
#[tokio::test]
async fn session_create_and_turn_start() {
    let (mut writer, mut reader, server_handle) = spawn_test_server();

    // Create session
    let create_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "session/create",
        "params": {"prompt": "Hello"}
    });
    send_request(&mut writer, &create_req).await;

    let create_resp = read_response(&mut reader).await;
    assert_eq!(create_resp["id"], 1);
    assert!(
        create_resp["error"].is_null(),
        "session/create failed: {}",
        create_resp
    );
    let session_id = create_resp["result"]["session_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!session_id.is_empty());
    assert!(
        create_resp["result"]["text"]
            .as_str()
            .unwrap()
            .contains("Hello from mock")
    );

    // Start another turn
    let turn_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "turn/start",
        "params": {"session_id": session_id, "prompt": "Follow up"}
    });
    send_request(&mut writer, &turn_req).await;

    let turn_resp = read_response(&mut reader).await;
    assert_eq!(turn_resp["id"], 2);
    assert!(
        turn_resp["error"].is_null(),
        "turn/start failed: {}",
        turn_resp
    );
    assert_eq!(
        turn_resp["result"]["session_id"].as_str().unwrap(),
        session_id
    );
    assert!(
        turn_resp["result"]["text"]
            .as_str()
            .unwrap()
            .contains("Hello from mock")
    );

    drop(writer);
    server_handle.await.unwrap().unwrap();
}

/// 3. Server shuts down cleanly on EOF.
#[tokio::test]
async fn server_shuts_down_on_eof() {
    let (writer, _reader, server_handle) = spawn_test_server();

    // Immediately close the writer (EOF)
    drop(writer);

    // Server should exit cleanly
    let result = server_handle.await.unwrap();
    assert!(
        result.is_ok(),
        "Server should shut down cleanly on EOF, got: {:?}",
        result.err()
    );
}

/// 4. Malformed JSON returns parse error, then server continues processing.
#[tokio::test]
async fn malformed_json_returns_error_and_continues() {
    let (mut writer, mut reader, server_handle) = spawn_test_server();

    // Send garbage
    writer.write_all(b"this is not json\n").await.unwrap();
    writer.flush().await.unwrap();

    // Should get a parse error response
    let error_resp = read_line_json(&mut reader).await;
    assert!(
        error_resp["error"].is_object(),
        "Expected error response, got: {}",
        error_resp
    );
    assert_eq!(error_resp["error"]["code"], -32700); // PARSE_ERROR

    // Now send a valid request - server should still work
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": "initialize",
        "params": {}
    });
    send_request(&mut writer, &req).await;

    let response = read_response(&mut reader).await;
    assert_eq!(response["id"], 42);
    assert!(
        response["error"].is_null(),
        "Expected success after recovery, got: {}",
        response
    );
    assert_eq!(response["result"]["server_info"]["name"], "meerkat-rpc");

    drop(writer);
    server_handle.await.unwrap().unwrap();
}

/// 5. Config get/patch roundtrip.
#[tokio::test]
async fn config_get_patch_roundtrip() {
    let (mut writer, mut reader, server_handle) = spawn_test_server();

    // Get config
    let get_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "config/get"
    });
    send_request(&mut writer, &get_req).await;

    let get_resp = read_response(&mut reader).await;
    assert_eq!(get_resp["id"], 1);
    assert!(
        get_resp["error"].is_null(),
        "config/get failed: {}",
        get_resp
    );
    let initial_max_tokens = get_resp["result"]["config"]["max_tokens"].as_u64().unwrap();

    // Patch max_tokens
    let new_max_tokens = initial_max_tokens + 1000;
    let patch_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "config/patch",
        "params": {"max_tokens": new_max_tokens}
    });
    send_request(&mut writer, &patch_req).await;

    let patch_resp = read_response(&mut reader).await;
    assert_eq!(patch_resp["id"], 2);
    assert!(
        patch_resp["error"].is_null(),
        "config/patch failed: {}",
        patch_resp
    );
    assert_eq!(patch_resp["result"]["config"]["max_tokens"], new_max_tokens);

    // Get again and verify
    let get_req2 = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "config/get"
    });
    send_request(&mut writer, &get_req2).await;

    let get_resp2 = read_response(&mut reader).await;
    assert_eq!(get_resp2["id"], 3);
    assert_eq!(get_resp2["result"]["config"]["max_tokens"], new_max_tokens);

    drop(writer);
    server_handle.await.unwrap().unwrap();
}

/// 6. Session list after create: create sessions, list them, verify count.
#[tokio::test]
async fn session_list_after_create() {
    let (mut writer, mut reader, server_handle) = spawn_test_server();

    // Create two sessions
    let create1 = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "session/create",
        "params": {"prompt": "First"}
    });
    send_request(&mut writer, &create1).await;
    let resp1 = read_response(&mut reader).await;
    assert!(resp1["error"].is_null(), "First create failed: {}", resp1);
    let sid1 = resp1["result"]["session_id"].as_str().unwrap().to_string();

    let create2 = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/create",
        "params": {"prompt": "Second"}
    });
    send_request(&mut writer, &create2).await;
    let resp2 = read_response(&mut reader).await;
    assert!(resp2["error"].is_null(), "Second create failed: {}", resp2);
    let sid2 = resp2["result"]["session_id"].as_str().unwrap().to_string();

    // List sessions
    let list_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/list"
    });
    send_request(&mut writer, &list_req).await;
    let list_resp = read_response(&mut reader).await;
    assert_eq!(list_resp["id"], 3);
    assert!(
        list_resp["error"].is_null(),
        "session/list failed: {}",
        list_resp
    );

    let sessions = list_resp["result"]["sessions"].as_array().unwrap();
    assert!(
        sessions.len() >= 2,
        "Expected at least 2 sessions, got {}",
        sessions.len()
    );

    // Both session IDs should appear
    let ids: Vec<&str> = sessions
        .iter()
        .filter_map(|s| s["session_id"].as_str())
        .collect();
    assert!(ids.contains(&sid1.as_str()), "Session 1 not found in list");
    assert!(ids.contains(&sid2.as_str()), "Session 2 not found in list");

    drop(writer);
    server_handle.await.unwrap().unwrap();
}

/// 7. Unknown method returns METHOD_NOT_FOUND error.
#[tokio::test]
async fn unknown_method_returns_error() {
    let (mut writer, mut reader, server_handle) = spawn_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "nonexistent/method",
        "params": {}
    });
    send_request(&mut writer, &req).await;

    let response = read_response(&mut reader).await;
    assert_eq!(response["id"], 1);
    assert!(response["error"].is_object(), "Expected error response");
    assert_eq!(response["error"]["code"], -32601); // METHOD_NOT_FOUND

    drop(writer);
    server_handle.await.unwrap().unwrap();
}
