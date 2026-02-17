//! E2E smoke tests for the RPC server (scenarios 15-17).
//!
//! Reuses the same duplex-stream pattern from `integration_server.rs`.
//! All scenarios require a live Anthropic API key and are `#[ignore]`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

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

/// Return the Anthropic API key if set, or `None`.
fn anthropic_api_key() -> Option<String> {
    for var in &["ANTHROPIC_API_KEY", "RKAT_ANTHROPIC_API_KEY"] {
        if let Ok(val) = std::env::var(var)
            && !val.is_empty()
        {
            return Some(val);
        }
    }
    None
}

/// Model to use for live smoke tests (defaults to a cheaper model).
fn smoke_model() -> String {
    std::env::var("SMOKE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string())
}

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
    let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let mut runtime = SessionRuntime::new(factory, config, 10, store);
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
// Scenario 15: Full RPC conversation flow (live API)
// ---------------------------------------------------------------------------

/// Scenario 15: Full RPC conversation flow with a live Anthropic API.
///
/// Initialize -> session/create (real Anthropic) -> turn/start follow-up ->
/// session/read -> session/list (verify present) -> session/archive ->
/// session/list (verify gone).
#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_scenario_15_full_rpc_conversation_flow() {
    let api_key = match anthropic_api_key() {
        Some(key) => key,
        None => {
            eprintln!("Skipping scenario 15: no ANTHROPIC_API_KEY set");
            return;
        }
    };

    let client = Arc::new(meerkat_client::AnthropicClient::new(api_key).unwrap());
    let (mut writer, mut reader, server_handle) = spawn_test_server(client);

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
    let turn_text = turn_resp["result"]["text"].as_str().unwrap().to_lowercase();
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
// Scenario 16: Kitchen-sink RPC compound test (live API)
// ---------------------------------------------------------------------------

/// Scenario 16: Compound test exercising many features through JSON-RPC.
///
/// Single test that chains: initialize (verify contract_version) ->
/// session/create with shell + builtins enabled -> agent uses shell tool ->
/// follow-up turn recalling context -> session/read (verify idle) ->
/// session/list (verify present) -> create SECOND session with structured
/// output -> verify structured_output in response -> session/archive first ->
/// session/list (first gone, second present) -> capabilities/get (verify
/// sessions + shell available) -> config/get + config/patch roundtrip.
///
/// Exercises: initialization, tool calling, multi-turn, structured output,
/// session CRUD, capabilities, config — all through a single RPC connection.
#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_scenario_16_kitchen_sink() {
    let api_key = match anthropic_api_key() {
        Some(key) => key,
        None => {
            eprintln!("Skipping scenario 16: no ANTHROPIC_API_KEY set");
            return;
        }
    };

    let client = Arc::new(meerkat_client::AnthropicClient::new(api_key).unwrap());
    let (mut writer, mut reader, server_handle) = spawn_test_server(client);

    let t = Duration::from_secs(120);
    let model = smoke_model();
    let mut req_id = 0u64;
    let mut next_id = || {
        req_id += 1;
        req_id
    };

    // --- 1. Initialize: verify contract_version ---
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{}}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "initialize failed: {resp}");
    assert!(
        resp["result"]["contract_version"].is_string(),
        "initialize should return contract_version string"
    );
    eprintln!(
        "[scenario 16] initialized, contract_version={}",
        resp["result"]["contract_version"]
    );

    // --- 2. Create session A: shell + builtins enabled ---
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"session/create",
            "params":{
                "prompt": "Use the shell tool to run: echo KITCHEN_SINK_42. Then tell me the output.",
                "model": model,
                "enable_builtins": true,
                "enable_shell": true
            }
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "session/create A failed: {resp}");
    let session_a = resp["result"]["session_id"].as_str().unwrap().to_string();
    let tool_calls_a = resp["result"]["tool_calls"].as_u64().unwrap_or(0);
    let text_a = resp["result"]["text"].as_str().unwrap_or("").to_lowercase();
    eprintln!(
        "[scenario 16] session A: tool_calls={tool_calls_a}, text={}",
        &text_a[..text_a.len().min(80)]
    );
    assert!(
        tool_calls_a >= 1,
        "Session A should have tool calls, got {tool_calls_a}"
    );
    assert!(
        text_a.contains("kitchen_sink_42"),
        "Session A should contain shell output, got: {text_a}"
    );

    // --- 3. Follow-up turn on session A: context recall ---
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"turn/start",
            "params":{"session_id": session_a, "prompt": "What was the output of the shell command you just ran? Reply briefly."}
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "turn/start A failed: {resp}");
    let text_a2 = resp["result"]["text"].as_str().unwrap_or("").to_lowercase();
    eprintln!(
        "[scenario 16] turn A2: {}",
        &text_a2[..text_a2.len().min(80)]
    );
    assert!(
        text_a2.contains("kitchen_sink_42") || text_a2.contains("kitchen"),
        "Follow-up should recall shell output, got: {text_a2}"
    );

    // --- 4. session/read: verify idle ---
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"session/read","params":{"session_id": session_a}}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "session/read failed: {resp}");
    assert_eq!(resp["result"]["state"], "idle");

    // --- 5. Create session B: structured output ---
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"session/create",
            "params":{
                "prompt": "What is the capital of France and what is its approximate population in millions?",
                "model": model,
                "output_schema": {
                    "type": "object",
                    "properties": {
                        "city": {"type": "string"},
                        "population_millions": {"type": "number"}
                    },
                    "required": ["city", "population_millions"]
                }
            }
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "session/create B failed: {resp}");
    let session_b = resp["result"]["session_id"].as_str().unwrap().to_string();
    let structured = &resp["result"]["structured_output"];
    eprintln!("[scenario 16] session B structured_output: {structured}");
    assert!(
        !structured.is_null(),
        "Session B should have structured_output"
    );
    assert!(
        structured["city"].is_string(),
        "structured_output should have city"
    );
    let city = structured["city"].as_str().unwrap().to_lowercase();
    assert!(city.contains("paris"), "City should be Paris, got: {city}");

    // --- 6. session/list: both sessions present ---
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"session/list"}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "session/list failed: {resp}");
    let ids: Vec<&str> = resp["result"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["session_id"].as_str())
        .collect();
    assert!(
        ids.contains(&session_a.as_str()),
        "Session A should be in list"
    );
    assert!(
        ids.contains(&session_b.as_str()),
        "Session B should be in list"
    );
    eprintln!("[scenario 16] list: {ids:?}");

    // --- 7. Archive session A ---
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"session/archive","params":{"session_id": session_a}}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "session/archive failed: {resp}");
    assert_eq!(resp["result"]["archived"], true);

    // --- 8. session/list: A gone, B still present ---
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"session/list"}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    let ids: Vec<&str> = resp["result"]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["session_id"].as_str())
        .collect();
    assert!(!ids.contains(&session_a.as_str()), "A should be gone");
    assert!(ids.contains(&session_b.as_str()), "B should remain");

    // --- 9. capabilities/get: verify sessions + shell ---
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"capabilities/get"}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "capabilities/get failed: {resp}");
    let cap_ids: Vec<&str> = resp["result"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["id"].as_str())
        .collect();
    assert!(
        cap_ids.contains(&"sessions"),
        "Should have sessions capability"
    );
    assert!(
        cap_ids.contains(&"builtins"),
        "Should have builtins capability"
    );
    assert!(cap_ids.contains(&"shell"), "Should have shell capability");
    eprintln!("[scenario 16] capabilities: {cap_ids:?}");

    // --- 10. config/get + config/patch roundtrip ---
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
        .or_else(|| resp["result"]["max_tokens"].as_u64())
        .expect("config/get should include max_tokens");

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
        .or_else(|| resp["result"]["max_tokens"].as_u64())
        .expect("config/patch should include max_tokens");
    assert_eq!(patched_max, original_max + 100);

    eprintln!(
        "[scenario 16] kitchen-sink test complete: tool calling + structured output + session CRUD + capabilities + config"
    );

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
#[ignore = "integration-real: live API"]
async fn e2e_scenario_17_multi_turn_event_streaming() {
    let api_key = match anthropic_api_key() {
        Some(key) => key,
        None => {
            eprintln!("Skipping scenario 17: no ANTHROPIC_API_KEY set");
            return;
        }
    };

    let client = Arc::new(meerkat_client::AnthropicClient::new(api_key).unwrap());
    let (mut writer, mut reader, server_handle) = spawn_test_server(client);

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
    let turn_text = turn_resp["result"]["text"].as_str().unwrap().to_lowercase();
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
