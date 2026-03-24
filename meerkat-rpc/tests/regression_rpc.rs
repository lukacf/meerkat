//! Regression tests for RPC surface response shapes and method contracts.
//!
//! Deterministic (mock LLM, no API keys, no `#[ignore]`). Runs in `cargo rct`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream;
use meerkat::AgentFactory;
use meerkat_client::{LlmClient, LlmError};
use meerkat_core::{Config, ConfigRuntime, MemoryConfigStore, StopReason};
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

const TIMEOUT: Duration = Duration::from_secs(5);

fn spawn_test_server() -> (
    tokio::io::DuplexStream,
    BufReader<tokio::io::DuplexStream>,
    tokio::task::JoinHandle<Result<(), meerkat_rpc::server::ServerError>>,
) {
    let temp = tempfile::tempdir().unwrap();
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let config = Config::default();
    let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let mut runtime = SessionRuntime::new(
        factory,
        config,
        10,
        meerkat::PersistenceBundle::new(store, None),
        meerkat_rpc::router::NotificationSink::noop(),
    );
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
        let _temp = temp;
        let reader = BufReader::new(server_reader);
        let mut server = RpcServer::new(reader, server_writer, runtime, config_store);
        server.run().await
    });

    let client_reader = BufReader::new(client_reader);
    (client_writer, client_reader, server_handle)
}

async fn send_request(writer: &mut tokio::io::DuplexStream, request: &serde_json::Value) {
    let line = format!("{}\n", serde_json::to_string(request).unwrap());
    writer.write_all(line.as_bytes()).await.unwrap();
    writer.flush().await.unwrap();
}

async fn read_line_json(reader: &mut BufReader<tokio::io::DuplexStream>) -> serde_json::Value {
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    assert!(!line.is_empty(), "Expected a JSONL line but got EOF");
    serde_json::from_str(&line).unwrap()
}

async fn read_response(reader: &mut BufReader<tokio::io::DuplexStream>) -> serde_json::Value {
    loop {
        let value = timeout(TIMEOUT, read_line_json(reader))
            .await
            .expect("timed out waiting for response");
        if value.get("id").is_some() {
            return value;
        }
    }
}

/// Helper: create a session and return (session_id, create_response).
async fn create_session(
    writer: &mut tokio::io::DuplexStream,
    reader: &mut BufReader<tokio::io::DuplexStream>,
    id: u64,
    prompt: &str,
) -> (String, serde_json::Value) {
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/create",
        "params": {"prompt": prompt}
    });
    send_request(writer, &req).await;
    let resp = read_response(reader).await;
    assert!(resp["error"].is_null(), "session/create failed: {resp}");
    let session_id = resp["result"]["session_id"].as_str().unwrap().to_string();
    (session_id, resp)
}

// ---------------------------------------------------------------------------
// 1. session_create_response_has_all_wire_fields
// ---------------------------------------------------------------------------

#[tokio::test]
async fn session_create_response_has_all_wire_fields() {
    let (mut writer, mut reader, handle) = spawn_test_server();

    let (_sid, resp) = create_session(&mut writer, &mut reader, 1, "Hi").await;
    let r = &resp["result"];

    // WireRunResult fields
    assert!(r["session_id"].is_string(), "session_id should be string");
    assert!(r["text"].is_string(), "text should be string");
    assert!(r["turns"].is_number(), "turns should be number");
    assert!(r["tool_calls"].is_number(), "tool_calls should be number");

    // WireUsage sub-fields
    assert!(r["usage"].is_object(), "usage should be object");
    assert!(
        r["usage"]["input_tokens"].is_number(),
        "usage.input_tokens should be number"
    );
    assert!(
        r["usage"]["output_tokens"].is_number(),
        "usage.output_tokens should be number"
    );
    assert!(
        r["usage"]["total_tokens"].is_number(),
        "usage.total_tokens should be number"
    );

    drop(writer);
    handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 2. session_read_response_has_all_wire_fields
// ---------------------------------------------------------------------------

#[tokio::test]
async fn session_read_response_has_all_wire_fields() {
    let (mut writer, mut reader, handle) = spawn_test_server();

    let (sid, _) = create_session(&mut writer, &mut reader, 1, "Hi").await;

    let read_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/read",
        "params": {"session_id": sid}
    });
    send_request(&mut writer, &read_req).await;
    let resp = read_response(&mut reader).await;
    assert!(resp["error"].is_null(), "session/read failed: {resp}");

    let r = &resp["result"];
    // WireSessionInfo fields
    assert!(r["session_id"].is_string(), "session_id should be string");
    assert!(r["created_at"].is_number(), "created_at should be number");
    assert!(r["updated_at"].is_number(), "updated_at should be number");
    assert!(
        r["message_count"].is_number(),
        "message_count should be number"
    );
    assert!(r["is_active"].is_boolean(), "is_active should be boolean");
    // last_assistant_text may be null or string; verify presence
    assert!(
        r.get("last_assistant_text").is_some(),
        "last_assistant_text should be present"
    );

    drop(writer);
    handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 3. session_list_response_shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn session_list_response_shape() {
    let (mut writer, mut reader, handle) = spawn_test_server();

    let (_sid1, _) = create_session(&mut writer, &mut reader, 1, "First").await;
    let (_sid2, _) = create_session(&mut writer, &mut reader, 2, "Second").await;

    let list_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/list"
    });
    send_request(&mut writer, &list_req).await;
    let resp = read_response(&mut reader).await;
    assert!(resp["error"].is_null(), "session/list failed: {resp}");

    let sessions = resp["result"]["sessions"]
        .as_array()
        .expect("sessions should be array");
    assert!(sessions.len() >= 2, "Expected at least 2 sessions");

    for s in sessions {
        assert!(s["session_id"].is_string(), "session_id should be string");
        assert!(s["created_at"].is_number(), "created_at should be number");
        assert!(s["updated_at"].is_number(), "updated_at should be number");
        assert!(
            s["message_count"].is_number(),
            "message_count should be number"
        );
        assert!(s["is_active"].is_boolean(), "is_active should be boolean");
    }

    drop(writer);
    handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 4. full_lifecycle_create_turn_read_archive_verify
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_lifecycle_create_turn_read_archive_verify() {
    let (mut writer, mut reader, handle) = spawn_test_server();

    // Create
    let (sid, _) = create_session(&mut writer, &mut reader, 1, "Hello").await;

    // Turn
    let turn_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "turn/start",
        "params": {"session_id": &sid, "prompt": "Follow up"}
    });
    send_request(&mut writer, &turn_req).await;
    let turn_resp = read_response(&mut reader).await;
    assert!(
        turn_resp["error"].is_null(),
        "turn/start failed: {turn_resp}"
    );

    // Read — message_count should be > 0
    let read_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/read",
        "params": {"session_id": &sid}
    });
    send_request(&mut writer, &read_req).await;
    let read_resp = read_response(&mut reader).await;
    assert!(
        read_resp["error"].is_null(),
        "session/read failed: {read_resp}"
    );
    let msg_count = read_resp["result"]["message_count"].as_u64().unwrap();
    assert!(msg_count > 0, "message_count should be > 0 after turns");

    // Archive
    let archive_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/archive",
        "params": {"session_id": &sid}
    });
    send_request(&mut writer, &archive_req).await;
    let archive_resp = read_response(&mut reader).await;
    assert!(
        archive_resp["error"].is_null(),
        "session/archive failed: {archive_resp}"
    );
    assert_eq!(archive_resp["result"]["archived"], true);

    // List — should not contain the archived session
    let list_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "session/list"
    });
    send_request(&mut writer, &list_req).await;
    let list_resp = read_response(&mut reader).await;
    assert!(
        list_resp["error"].is_null(),
        "session/list failed: {list_resp}"
    );
    let sessions = list_resp["result"]["sessions"].as_array().unwrap();
    let ids: Vec<&str> = sessions
        .iter()
        .filter_map(|s| s["session_id"].as_str())
        .collect();
    assert!(
        !ids.contains(&sid.as_str()),
        "Archived session should not appear in list"
    );

    // Read after archive — may return error (session not found) or
    // return the session with is_active=false, depending on the runtime.
    let read_req2 = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "session/read",
        "params": {"session_id": &sid}
    });
    send_request(&mut writer, &read_req2).await;
    let read_resp2 = read_response(&mut reader).await;
    if read_resp2["error"].is_null() {
        // Ephemeral service: session still readable but inactive
        assert_eq!(
            read_resp2["result"]["is_active"], false,
            "Archived session should be marked inactive"
        );
    } else {
        // Persistent service: session removed entirely
        assert!(read_resp2["error"].is_object());
    }

    drop(writer);
    handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 5. session_labels_roundtrip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn session_labels_roundtrip() {
    let (mut writer, mut reader, handle) = spawn_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "session/create",
        "params": {
            "prompt": "Hello",
            "labels": {"env": "test", "team": "backend"}
        }
    });
    send_request(&mut writer, &req).await;
    let create_resp = read_response(&mut reader).await;
    assert!(
        create_resp["error"].is_null(),
        "session/create with labels failed: {create_resp}"
    );
    let sid = create_resp["result"]["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Read back and verify labels
    let read_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/read",
        "params": {"session_id": &sid}
    });
    send_request(&mut writer, &read_req).await;
    let read_resp = read_response(&mut reader).await;
    assert!(
        read_resp["error"].is_null(),
        "session/read failed: {read_resp}"
    );
    let labels = &read_resp["result"]["labels"];
    assert_eq!(labels["env"], "test");
    assert_eq!(labels["team"], "backend");

    drop(writer);
    handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 6. inject_context_via_rpc
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inject_context_via_rpc() {
    let (mut writer, mut reader, handle) = spawn_test_server();

    let (sid, _) = create_session(&mut writer, &mut reader, 1, "Hello").await;

    // Inject system context
    let inject_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/inject_context",
        "params": {
            "session_id": &sid,
            "text": "Important context for the agent",
            "source": "test"
        }
    });
    send_request(&mut writer, &inject_req).await;
    let inject_resp = read_response(&mut reader).await;
    assert!(
        inject_resp["error"].is_null(),
        "session/inject_context failed: {inject_resp}"
    );
    assert!(
        inject_resp["result"]["status"].is_string(),
        "inject_context result should have status field"
    );

    // Follow-up turn should succeed
    let turn_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "turn/start",
        "params": {"session_id": &sid, "prompt": "Proceed"}
    });
    send_request(&mut writer, &turn_req).await;
    let turn_resp = read_response(&mut reader).await;
    assert!(
        turn_resp["error"].is_null(),
        "turn/start after inject_context failed: {turn_resp}"
    );

    drop(writer);
    handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 7. stream_open_close_roundtrip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_open_close_roundtrip() {
    let (mut writer, mut reader, handle) = spawn_test_server();

    let (sid, _) = create_session(&mut writer, &mut reader, 1, "Hello").await;

    // stream_open
    let open_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/stream_open",
        "params": {"session_id": &sid}
    });
    send_request(&mut writer, &open_req).await;
    let open_resp = read_response(&mut reader).await;
    assert!(
        open_resp["error"].is_null(),
        "session/stream_open failed: {open_resp}"
    );
    let stream_id = open_resp["result"]["stream_id"]
        .as_str()
        .expect("stream_id should be a string");
    assert!(!stream_id.is_empty(), "stream_id should not be empty");
    assert_eq!(open_resp["result"]["opened"], true);

    // stream_close
    let close_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/stream_close",
        "params": {"stream_id": stream_id}
    });
    send_request(&mut writer, &close_req).await;
    let close_resp = read_response(&mut reader).await;
    assert!(
        close_resp["error"].is_null(),
        "session/stream_close failed: {close_resp}"
    );
    assert_eq!(close_resp["result"]["closed"], true);
    assert_eq!(close_resp["result"]["already_closed"], false);

    drop(writer);
    handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 8. turn_start_nonexistent_session_returns_error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn turn_start_nonexistent_session_returns_error() {
    let (mut writer, mut reader, handle) = spawn_test_server();

    let fake_id = "00000000-0000-0000-0000-000000000099";
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "turn/start",
        "params": {"session_id": fake_id, "prompt": "This should fail"}
    });
    send_request(&mut writer, &req).await;
    let resp = read_response(&mut reader).await;
    assert!(
        resp["error"].is_object(),
        "turn/start on nonexistent session should return error: {resp}"
    );

    drop(writer);
    handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 9. session_read_after_archive_returns_error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn session_read_after_archive_returns_error() {
    let (mut writer, mut reader, handle) = spawn_test_server();

    let (sid, _) = create_session(&mut writer, &mut reader, 1, "Hi").await;

    // Archive
    let archive_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/archive",
        "params": {"session_id": &sid}
    });
    send_request(&mut writer, &archive_req).await;
    let archive_resp = read_response(&mut reader).await;
    assert!(
        archive_resp["error"].is_null(),
        "session/archive failed: {archive_resp}"
    );

    // Read after archive — ephemeral runtime keeps session readable (is_active=false)
    // while persistent runtime returns SESSION_NOT_FOUND.
    let read_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/read",
        "params": {"session_id": &sid}
    });
    send_request(&mut writer, &read_req).await;
    let read_resp = read_response(&mut reader).await;
    if read_resp["error"].is_null() {
        assert_eq!(
            read_resp["result"]["is_active"], false,
            "Archived session should be marked inactive: {read_resp}"
        );
    } else {
        assert_eq!(
            read_resp["error"]["code"], -32001,
            "Expected SESSION_NOT_FOUND error code"
        );
    }

    drop(writer);
    handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 10. turn_interrupt_idle_is_noop
// ---------------------------------------------------------------------------

#[tokio::test]
async fn turn_interrupt_idle_is_noop() {
    let (mut writer, mut reader, handle) = spawn_test_server();

    let (sid, _) = create_session(&mut writer, &mut reader, 1, "Hi").await;

    // Interrupt on idle session — the runtime maps NotRunning to Ok(())
    let interrupt_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "turn/interrupt",
        "params": {"session_id": &sid}
    });
    send_request(&mut writer, &interrupt_req).await;
    let resp = read_response(&mut reader).await;
    // Interrupt on idle is a no-op success, not an error
    assert!(
        resp["error"].is_null(),
        "turn/interrupt on idle session should succeed (no-op): {resp}"
    );
    assert_eq!(resp["result"]["interrupted"], true);

    drop(writer);
    handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 11. config_get_set_patch_roundtrip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn config_get_set_patch_roundtrip() {
    let (mut writer, mut reader, handle) = spawn_test_server();

    // config/get — baseline
    let get_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "config/get"
    });
    send_request(&mut writer, &get_req).await;
    let get_resp = read_response(&mut reader).await;
    assert!(get_resp["error"].is_null(), "config/get failed: {get_resp}");
    let initial_max_tokens = get_resp["result"]["config"]["max_tokens"].as_u64().unwrap();

    // config/set — replace entire config with a different max_tokens
    let new_max_tokens = initial_max_tokens + 500;
    let mut set_config = get_resp["result"]["config"].clone();
    set_config["max_tokens"] = serde_json::json!(new_max_tokens);
    let set_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "config/set",
        "params": {"config": set_config}
    });
    send_request(&mut writer, &set_req).await;
    let set_resp = read_response(&mut reader).await;
    assert!(set_resp["error"].is_null(), "config/set failed: {set_resp}");
    assert_eq!(
        set_resp["result"]["config"]["max_tokens"], new_max_tokens,
        "config/set should reflect new max_tokens"
    );

    // config/get — verify set took effect
    let get_req2 = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "config/get"
    });
    send_request(&mut writer, &get_req2).await;
    let get_resp2 = read_response(&mut reader).await;
    assert_eq!(
        get_resp2["result"]["config"]["max_tokens"], new_max_tokens,
        "config/get after set should match"
    );

    // config/patch — increment max_tokens again
    let patched_max_tokens = new_max_tokens + 100;
    let patch_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "config/patch",
        "params": {"max_tokens": patched_max_tokens}
    });
    send_request(&mut writer, &patch_req).await;
    let patch_resp = read_response(&mut reader).await;
    assert!(
        patch_resp["error"].is_null(),
        "config/patch failed: {patch_resp}"
    );
    assert_eq!(
        patch_resp["result"]["config"]["max_tokens"], patched_max_tokens,
        "config/patch should reflect patched max_tokens"
    );

    // config/get — verify both set and patch are reflected
    let get_req3 = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "config/get"
    });
    send_request(&mut writer, &get_req3).await;
    let get_resp3 = read_response(&mut reader).await;
    assert_eq!(
        get_resp3["result"]["config"]["max_tokens"], patched_max_tokens,
        "config/get after patch should match"
    );

    drop(writer);
    handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 12. capabilities_get_has_sessions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn capabilities_get_has_sessions() {
    let (mut writer, mut reader, handle) = spawn_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "capabilities/get"
    });
    send_request(&mut writer, &req).await;
    let resp = read_response(&mut reader).await;
    assert!(resp["error"].is_null(), "capabilities/get failed: {resp}");

    let capabilities = resp["result"]["capabilities"]
        .as_array()
        .expect("capabilities should be array");
    let ids: Vec<&str> = capabilities
        .iter()
        .filter_map(|c| c["id"].as_str())
        .collect();
    assert!(
        ids.contains(&"sessions"),
        "capabilities should include 'sessions', got: {ids:?}"
    );

    drop(writer);
    handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 13. skills_list_returns_array
// ---------------------------------------------------------------------------

#[tokio::test]
async fn skills_list_returns_array() {
    let (mut writer, mut reader, handle) = spawn_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "skills/list"
    });
    send_request(&mut writer, &req).await;
    let resp = read_response(&mut reader).await;
    // skills/list may return an error if skills runtime is not configured,
    // but the result (if present) should have a skills array.
    if resp["error"].is_null() {
        assert!(
            resp["result"]["skills"].is_array(),
            "skills/list result.skills should be array: {resp}"
        );
    } else {
        // Skills not enabled is acceptable in this test environment.
        assert!(
            resp["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("skills"),
            "Error should mention skills: {resp}"
        );
    }

    drop(writer);
    handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// 14. initialize_methods_list_complete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn initialize_methods_list_complete() {
    let (mut writer, mut reader, handle) = spawn_test_server();

    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {}
    });
    send_request(&mut writer, &req).await;
    let resp = read_response(&mut reader).await;
    assert!(resp["error"].is_null(), "initialize failed: {resp}");

    let methods = resp["result"]["methods"]
        .as_array()
        .expect("methods should be array");
    let method_names: Vec<&str> = methods.iter().filter_map(|m| m.as_str()).collect();

    // Core methods that must always be present
    let expected_core = [
        "initialize",
        "initialized",
        "session/create",
        "session/list",
        "session/read",
        "session/history",
        "session/archive",
        "session/external_event",
        "session/inject_context",
        "session/stream_open",
        "session/stream_close",
        "turn/start",
        "turn/interrupt",
        "runtime/state",
        "runtime/accept",
        "runtime/retire",
        "runtime/reset",
        "input/state",
        "input/list",
        "config/get",
        "config/set",
        "config/patch",
        "capabilities/get",
        "models/catalog",
        "skills/list",
        "skills/inspect",
        "tools/register",
    ];
    for method in &expected_core {
        assert!(
            method_names.contains(method),
            "Missing method '{method}' in initialize response. Got: {method_names:?}"
        );
    }

    #[cfg(feature = "mob")]
    {
        let expected_mob = [
            "mob/prefabs",
            "mob/create",
            "mob/list",
            "mob/status",
            "mob/lifecycle",
            "mob/spawn",
            "mob/spawn_many",
            "mob/retire",
            "mob/respawn",
            "mob/wire",
            "mob/unwire",
            "mob/members",
            "mob/send",
            "mob/events",
            "mob/append_system_context",
            "mob/flows",
            "mob/flow_run",
            "mob/flow_status",
            "mob/flow_cancel",
            "mob/spawn_helper",
            "mob/fork_helper",
            "mob/force_cancel",
            "mob/member_status",
            "mob/tools",
            "mob/call",
            "mob/stream_open",
            "mob/stream_close",
        ];
        for method in &expected_mob {
            assert!(
                method_names.contains(method),
                "Missing mob method '{method}' in initialize response. Got: {method_names:?}"
            );
        }
    }

    drop(writer);
    handle.await.unwrap().unwrap();
}

#[tokio::test]
async fn notification_catalog_lists_live_stream_notifications() {
    let notifications = meerkat_contracts::rpc_notification_names(
        meerkat_contracts::RpcMethodCatalogOptions::documented_surface(),
    );

    for expected in [
        "initialized",
        "session/event",
        "session/stream_event",
        "session/stream_end",
        "mob/stream_event",
        "mob/stream_end",
    ] {
        assert!(
            notifications.iter().any(|name| name == expected),
            "notification catalog missing {expected}"
        );
    }
}

// ---------------------------------------------------------------------------
// 15. mob_create_status_list_lifecycle (feature-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "mob")]
#[tokio::test]
async fn mob_create_status_list_lifecycle() {
    let (mut writer, mut reader, handle) = spawn_test_server();

    // mob/create using a prefab (avoids manual definition wiring)
    let create_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "mob/create",
        "params": {
            "prefab": "coding_swarm"
        }
    });
    send_request(&mut writer, &create_req).await;
    let create_resp = read_response(&mut reader).await;
    assert!(
        create_resp["error"].is_null(),
        "mob/create failed: {create_resp}"
    );
    let mob_id = create_resp["result"]["mob_id"]
        .as_str()
        .expect("mob_id should be string")
        .to_string();
    assert!(!mob_id.is_empty(), "mob_id should not be empty");

    // mob/status
    let status_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "mob/status",
        "params": {"mob_id": &mob_id}
    });
    send_request(&mut writer, &status_req).await;
    let status_resp = read_response(&mut reader).await;
    assert!(
        status_resp["error"].is_null(),
        "mob/status failed: {status_resp}"
    );
    assert_eq!(status_resp["result"]["mob_id"], mob_id);
    assert!(
        status_resp["result"]["status"].is_string(),
        "status should be a string"
    );

    // mob/list
    let list_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "mob/list"
    });
    send_request(&mut writer, &list_req).await;
    let list_resp = read_response(&mut reader).await;
    assert!(list_resp["error"].is_null(), "mob/list failed: {list_resp}");
    let mobs = list_resp["result"]["mobs"]
        .as_array()
        .expect("mobs should be array");
    let mob_ids: Vec<&str> = mobs.iter().filter_map(|m| m["mob_id"].as_str()).collect();
    assert!(
        mob_ids.contains(&mob_id.as_str()),
        "Created mob should appear in list"
    );

    // mob/lifecycle (stop)
    let lifecycle_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "mob/lifecycle",
        "params": {"mob_id": &mob_id, "action": "stop"}
    });
    send_request(&mut writer, &lifecycle_req).await;
    let lifecycle_resp = read_response(&mut reader).await;
    assert!(
        lifecycle_resp["error"].is_null(),
        "mob/lifecycle(stop) failed: {lifecycle_resp}"
    );
    assert_eq!(lifecycle_resp["result"]["ok"], true);
    assert_eq!(lifecycle_resp["result"]["action"], "stop");
    assert_eq!(lifecycle_resp["result"]["mob_id"], mob_id);

    drop(writer);
    handle.await.unwrap().unwrap();
}
