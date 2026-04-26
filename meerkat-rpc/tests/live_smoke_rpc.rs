#![allow(clippy::large_futures)]

//! Live integration and smoke tests for the RPC server.
//!
//! Reuses the same duplex-stream pattern from `integration_server.rs`.
//! All scenarios require a live Anthropic API key and are `#[ignore]`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../test-fixtures/live_smoke/support.rs"]
mod live_smoke;

use std::sync::Arc;

use meerkat::AgentFactory;
use meerkat_client::LlmClient;
use meerkat_contracts::WireRunResult;
use meerkat_core::{BlobStore, Config, ConfigRuntime, MemoryConfigStore};
use meerkat_rpc::server::RpcServer;
use meerkat_rpc::session_runtime::SessionRuntime;
use meerkat_store::FsBlobStore;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::{Duration, timeout};

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
    let store =
        Arc::new(meerkat::SqliteSessionStore::open(temp.path().join("sessions.sqlite3")).unwrap());
    let runtime_store = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(store.path().to_path_buf()).unwrap(),
    ) as Arc<dyn meerkat_runtime::RuntimeStore>;
    let blob_store: Arc<dyn BlobStore> = Arc::new(FsBlobStore::new(temp.path().join("blobs")));
    let mut runtime = SessionRuntime::new(
        factory,
        config,
        10,
        meerkat::PersistenceBundle::new(
            store as Arc<dyn meerkat::SessionStore>,
            Some(runtime_store),
            blob_store,
        ),
        meerkat_rpc::router::NotificationSink::noop(),
    );
    let config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(MemoryConfigStore::new(Config::default()));
    runtime.set_default_llm_client(Some(client));
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

async fn read_response_with_callback_tool_reply(
    writer: &mut tokio::io::DuplexStream,
    reader: &mut BufReader<tokio::io::DuplexStream>,
    request_id: u64,
    timeout_duration: Duration,
    expected_tool_name: &str,
    callback_result_text: &str,
) -> (serde_json::Value, Vec<String>) {
    let mut callback_names = Vec::new();
    let response = timeout(timeout_duration, async {
        loop {
            let value = read_line_json(reader).await;

            if value.get("method").and_then(|m| m.as_str()) == Some("tool/execute") {
                let callback_name = value["params"]["name"]
                    .as_str()
                    .unwrap_or("<unknown>")
                    .to_string();
                callback_names.push(callback_name.clone());
                assert_eq!(
                    callback_name, expected_tool_name,
                    "unexpected callback tool during deferred smoke flow"
                );

                let callback_id = value["id"].clone();
                let callback_resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": callback_id,
                    "result": {
                        "content": callback_result_text,
                        "is_error": false
                    }
                });
                send_request(writer, &callback_resp).await;
                continue;
            }

            if value.get("id") == Some(&serde_json::json!(request_id)) {
                return value;
            }
        }
    })
    .await
    .expect("RPC request timed out while waiting for deferred callback flow");

    (response, callback_names)
}

async fn session_history_response(
    writer: &mut tokio::io::DuplexStream,
    reader: &mut BufReader<tokio::io::DuplexStream>,
    request_id: u64,
    timeout_duration: Duration,
    session_id: &str,
    limit: usize,
) -> serde_json::Value {
    send_request(
        writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "session/history",
            "params": {
                "session_id": session_id,
                "limit": limit
            }
        }),
    )
    .await;

    timeout(timeout_duration, read_response(reader))
        .await
        .expect("session/history timed out")
}

fn assert_history_contains_tool_uses(history: &serde_json::Value, tool_names: &[&str]) {
    let history_text = history["result"].to_string();
    for tool_name in tool_names {
        assert!(
            history_text.contains(&format!("\"name\":\"{tool_name}\"")),
            "expected session history to contain tool use `{tool_name}`, history={history_text}"
        );
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
#[ignore = "lane:e2e-live"]
async fn e2e_scenario_15_full_rpc_conversation_flow() {
    let api_key = match live_smoke::anthropic_api_key() {
        Some(key) => key,
        None => {
            eprintln!("Skipping scenario 15: no ANTHROPIC_API_KEY set");
            return;
        }
    };

    let client = Arc::new(meerkat_client::AnthropicClient::new(api_key).unwrap());
    let (mut writer, mut reader, server_handle) = spawn_test_server(client);

    let live_timeout = live_smoke::live_timeout();

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
        "initialize failed: {init_resp}"
    );
    assert_eq!(init_resp["result"]["server_info"]["name"], "meerkat-rpc");

    // 2. session/create with real Anthropic
    let model = live_smoke::smoke_model();
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
        "session/create failed: {create_resp}"
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
        "turn/start failed: {turn_resp}"
    );
    assert_eq!(
        turn_resp["result"]["session_id"].as_str().unwrap(),
        session_id
    );
    let turn_text = turn_resp["result"]["text"].as_str().unwrap().to_lowercase();
    assert!(
        turn_text.contains("smokebot") || turn_text.contains("smoke"),
        "Follow-up should recall the name. Got: {turn_text}"
    );
    assert!(
        turn_text.contains("teal"),
        "Follow-up should recall the color. Got: {turn_text}"
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
        "session/read failed: {read_resp}"
    );
    assert_eq!(
        read_resp["result"]["session_id"].as_str().unwrap(),
        session_id
    );
    assert_eq!(read_resp["result"]["is_active"], false);

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
        "session/list failed: {list_resp}"
    );
    let sessions = list_resp["result"]["sessions"].as_array().unwrap();
    let ids: Vec<&str> = sessions
        .iter()
        .filter_map(|s| s["session_id"].as_str())
        .collect();
    assert!(
        ids.contains(&session_id.as_str()),
        "Session should appear in list. Got: {ids:?}"
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
        "session/archive failed: {archive_resp}"
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
        "session/list (after archive) failed: {list_resp2}"
    );
    let sessions2 = list_resp2["result"]["sessions"].as_array().unwrap();
    let ids2: Vec<&str> = sessions2
        .iter()
        .filter_map(|s| s["session_id"].as_str())
        .collect();
    assert!(
        !ids2.contains(&session_id.as_str()),
        "Archived session should NOT appear in list. Got: {ids2:?}"
    );

    // Clean up
    drop(writer);
    server_handle.await.unwrap().unwrap();
}

/// Supplemental: degraded/unhealthy diagnostics are projected consistently
/// across RPC/REST wire payloads plus MCP/CLI JSON envelopes.
#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn e2e_diagnostics_projection_contract_across_cli_rest_rpc_mcp() {
    let run = meerkat_core::RunResult {
        text: "ok".to_string(),
        session_id: meerkat_core::SessionId::new(),
        usage: Default::default(),
        turns: 1,
        tool_calls: 0,
        structured_output: None,
        schema_warnings: None,
        skill_diagnostics: Some(meerkat_core::skills::SkillRuntimeDiagnostics {
            source_health: meerkat_core::skills::SourceHealthSnapshot {
                state: meerkat_core::skills::SourceHealthState::Unhealthy,
                invalid_ratio: 0.8,
                invalid_count: 8,
                total_count: 10,
                failure_streak: 10,
                handshake_failed: true,
            },
            quarantined: vec![],
        }),
    };

    // RPC/REST share WireRunResult; diagnostics must survive conversion.
    let wire: WireRunResult = run.clone().into();
    assert_eq!(
        wire.skill_diagnostics
            .as_ref()
            .expect("skill_diagnostics")
            .source_health
            .state,
        meerkat_core::skills::SourceHealthState::Unhealthy
    );

    // MCP projection: wrapped JSON payload must retain diagnostics object.
    let mcp_payload = serde_json::json!({
        "content": [{"type": "text", "text": run.text}],
        "session_id": run.session_id.to_string(),
        "turns": run.turns,
        "tool_calls": run.tool_calls,
        "structured_output": run.structured_output,
        "schema_warnings": run.schema_warnings,
        "skill_diagnostics": run.skill_diagnostics,
    });
    assert_eq!(
        mcp_payload["skill_diagnostics"]["source_health"]["state"],
        "unhealthy"
    );

    // CLI JSON envelope must expose the same diagnostics state.
    let cli_payload = serde_json::json!({
        "text": wire.text,
        "session_id": wire.session_id,
        "turns": wire.turns,
        "tool_calls": wire.tool_calls,
        "usage": wire.usage,
        "structured_output": wire.structured_output,
        "schema_warnings": wire.schema_warnings,
        "skill_diagnostics": wire.skill_diagnostics,
    });
    assert_eq!(
        cli_payload["skill_diagnostics"]["source_health"]["state"],
        "unhealthy"
    );
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
#[ignore = "lane:e2e-smoke"]
async fn e2e_scenario_16_kitchen_sink() {
    let api_key = match live_smoke::anthropic_api_key() {
        Some(key) => key,
        None => {
            eprintln!("Skipping scenario 16: no ANTHROPIC_API_KEY set");
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

    // --- 2. Create a deferred session and drive it through runtime/* before any turn/start ---
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"session/create",
            "params":{
                "prompt": "Remember the marker RPC_RUNTIME_DEFERRED_16.",
                "model": model,
                "initial_turn": "deferred"
            }
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(
        resp["error"].is_null(),
        "deferred session/create failed: {resp}"
    );
    let deferred_session = resp["result"]["session_id"].as_str().unwrap().to_string();

    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"runtime/session_status",
            "params":{"session_id": deferred_session}
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(
        resp["error"].is_null(),
        "runtime/session_status failed for deferred session: {resp}"
    );
    // RPC surface eagerly attaches an executor for all sessions (including
    // deferred ones), so the runtime state is Attached, not Idle.
    assert_eq!(resp["result"]["state"].as_str(), Some("attached"));

    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"runtime/session_submit",
            "params":{
                "session_id": deferred_session,
                "input": {
                    "input_type": "prompt",
                    "header": {
                        "id": meerkat_core::InputId::new(),
                        "timestamp": "2026-03-12T00:00:00Z",
                        "source": { "type": "operator" },
                        "durability": "durable",
                        "visibility": {
                            "transcript_eligible": true,
                            "operator_eligible": true
                        }
                    },
                    "text": "Reply with RPC_RUNTIME_DEFERRED_16 and confirm the saved marker."
                }
            }
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(
        resp["error"].is_null(),
        "runtime/session_submit failed for deferred session: {resp}"
    );
    assert_eq!(resp["result"]["outcome_type"].as_str(), Some("accepted"));
    let deferred_input_id = resp["result"]["input_id"].as_str().unwrap().to_string();

    let mut consumed = false;
    for _ in 0..120 {
        let id = next_id();
        send_request(
            &mut writer,
            &serde_json::json!({
                "jsonrpc":"2.0","id":id,"method":"runtime/session_submission",
                "params":{"session_id": deferred_session, "input_id": deferred_input_id}
            }),
        )
        .await;
        let resp = timeout(t, read_response(&mut reader)).await.unwrap();
        assert!(
            resp["error"].is_null(),
            "runtime/session_submission failed for deferred session: {resp}"
        );
        if resp["result"]["current_state"].as_str() == Some("consumed") {
            consumed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert!(
        consumed,
        "runtime/session_submit should fully consume the deferred-session input before continuing"
    );

    let mut deferred_runtime_text = String::new();
    for _ in 0..120 {
        let id = next_id();
        send_request(
            &mut writer,
            &serde_json::json!({
                "jsonrpc":"2.0","id":id,"method":"session/read",
                "params":{"session_id": deferred_session}
            }),
        )
        .await;
        let resp = timeout(t, read_response(&mut reader)).await.unwrap();
        assert!(
            resp["error"].is_null(),
            "session/read failed for deferred session: {resp}"
        );
        deferred_runtime_text = resp["result"]["last_assistant_text"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase();
        if deferred_runtime_text.contains("rpc_runtime_deferred_16")
            || deferred_runtime_text.contains("rpc runtime deferred 16")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert!(
        deferred_runtime_text.contains("rpc_runtime_deferred_16")
            || deferred_runtime_text.contains("rpc runtime deferred 16"),
        "deferred runtime turn should materialize an assistant reply via raw runtime/* methods, got: {deferred_runtime_text}"
    );

    // --- 3. Create session A: shell + builtins enabled ---
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

    // --- 4. Follow-up turn on session A: context recall ---
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

    // --- 5. session/read: verify idle ---
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"session/read","params":{"session_id": session_a}}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "session/read failed: {resp}");
    assert_eq!(resp["result"]["is_active"], false);

    // --- 6. Create session B: structured output ---
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
#[ignore = "lane:e2e-live"]
async fn e2e_scenario_17_multi_turn_event_streaming() {
    let api_key = match live_smoke::anthropic_api_key() {
        Some(key) => key,
        None => {
            eprintln!("Skipping scenario 17: no ANTHROPIC_API_KEY set");
            return;
        }
    };

    let client = Arc::new(meerkat_client::AnthropicClient::new(api_key).unwrap());
    let (mut writer, mut reader, server_handle) = spawn_test_server(client);

    let live_timeout = live_smoke::live_timeout();

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
        "initialize failed: {init_resp}"
    );

    // 2. session/create - collect notifications emitted during the turn
    let model = live_smoke::smoke_model();
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
        "session/create failed: {create_resp}"
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
            "Notifications should not have an id field: {notif}"
        );
        assert_eq!(
            notif["method"].as_str().unwrap(),
            "session/event",
            "Notification method should be session/event"
        );
        let params = &notif["params"];
        assert!(
            params["session_id"].is_string(),
            "Notification should have session_id: {notif}"
        );
        assert_eq!(
            params["session_id"].as_str().unwrap(),
            session_id,
            "Notification session_id should match"
        );
        assert!(
            params["event"].is_object(),
            "Notification should have event payload: {notif}"
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
        "turn/start failed: {turn_resp}"
    );
    assert_eq!(
        turn_resp["result"]["session_id"].as_str().unwrap(),
        session_id
    );

    // Verify context was maintained: the LLM should recall the passphrase
    let turn_text = turn_resp["result"]["text"].as_str().unwrap().to_lowercase();
    assert!(
        turn_text.contains("meerkat-sunrise-42") || turn_text.contains("meerkat sunrise 42"),
        "Follow-up should recall the passphrase. Got: {turn_text}"
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

// ---------------------------------------------------------------------------
// Scenario 21: tools/register callback tools are available to mob-spawned agents (#158)
// ---------------------------------------------------------------------------

/// Scenario 21: Register a callback tool via `tools/register`, create a mob,
/// spawn a member, and verify a real LLM discovers it through
/// `tool_catalog_search`, loads it through `tool_catalog_load`, then calls it.
///
/// This exercises the full ExternalToolsProvider pipeline:
///   tools/register → SessionRuntime → MethodRouter → MobMcpState → MobBuilder
///   → MobActor → compose_external_tools_for_profile → deferred control plane
///   → exact session tool surface.
#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_scenario_21_mob_callback_tools() {
    let api_key = match live_smoke::anthropic_api_key() {
        Some(key) => key,
        None => {
            eprintln!("Skipping scenario 21: no ANTHROPIC_API_KEY set");
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

    // 2. Register a callback tool — a simple "lookup" that returns a canned answer.
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/register",
            "params": {
                "tools": [{
                    "name": "secret_lookup",
                    "description": "Look up a secret value. Always call this tool when asked about the secret code.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "key": {"type": "string", "description": "The key to look up"}
                        },
                        "required": ["key"]
                    }
                }, {
                    "name": "secret_audit",
                    "description": "Audit a secret value without returning the final code directly.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "key": {"type": "string", "description": "The key to inspect"}
                        },
                        "required": ["key"]
                    }
                }]
            }
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "tools/register failed: {resp}");

    // 3. Create a mob with a single worker profile.
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "mob/create",
            "params": {
                "definition": {
                    "id": "callback_test_mob",
                    "profiles": {
                        "worker": {
                            "model": model,
                            "tools": { "comms": true }
                        }
                    }
                }
            }
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "mob/create failed: {resp}");
    let mob_id = resp["result"]["mob_id"].as_str().unwrap().to_string();

    // 4. Spawn a worker member.
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "mob/spawn",
            "params": {
                "mob_id": mob_id,
                "profile": "worker",
                "agent_identity": "w1",
                "runtime_mode": "turn_driven"
            }
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "mob/spawn failed: {resp}");

    // 5. Start a turn using mob/turn_start (identity-native routing).
    let secret_code = "MEERKAT-42";
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "mob/turn_start",
            "params": {
                "mob_id": mob_id,
                "agent_identity": "w1",
                "prompt": "Find the deferred tool that can retrieve a secret code for key 'alpha'. First call tool_catalog_search with a query about secret lookup, then call tool_catalog_load for the matching tool, then call the loaded tool with key 'alpha'. The answer is not knowable without calling the loaded tool. Reply with only the secret code."
            }
        }),
    )
    .await;

    let (turn_resp, callback_names) = read_response_with_callback_tool_reply(
        &mut writer,
        &mut reader,
        id,
        t,
        "secret_lookup",
        &format!("The secret code for 'alpha' is {secret_code}."),
    )
    .await;

    assert!(
        turn_resp["error"].is_null(),
        "turn/start failed: {turn_resp}"
    );
    let text = turn_resp["result"]["text"].as_str().unwrap_or("");
    eprintln!("[scenario 21] turn response text: {text}");

    assert!(
        callback_names.iter().any(|name| name == "secret_lookup"),
        "Expected the LLM to call secret_lookup but no tool/execute callback was received"
    );
    assert!(
        text.contains(secret_code),
        "Expected response to contain '{secret_code}' from callback tool result, got: {text}"
    );

    // Extract session_id from the turn response for subsequent session-level queries.
    let session_id = turn_resp["result"]["session_id"]
        .as_str()
        .expect("mob/turn_start response should include session_id from delegated turn/start")
        .to_string();

    let history_id = next_id();
    let history_resp =
        session_history_response(&mut writer, &mut reader, history_id, t, &session_id, 100).await;
    assert!(
        history_resp["error"].is_null(),
        "session/history failed: {history_resp}"
    );
    assert_history_contains_tool_uses(
        &history_resp,
        &["tool_catalog_search", "tool_catalog_load", "secret_lookup"],
    );

    eprintln!("[scenario 21] PASSED: mob member used deferred search/load/call flow");

    // Clean up
    drop(writer);
    server_handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// Scenario 22: Transport backpressure under notification flood (#157)
// ---------------------------------------------------------------------------

/// Scenario 22: Create multiple sessions that generate events concurrently,
/// verify the server doesn't crash and all responses are received.
///
/// Uses a small duplex buffer to simulate a slow pipe consumer. The
/// `BlockingWriter` fix ensures writes block instead of returning WouldBlock.
/// With the old `tokio::io::stdout()` approach, this would crash.
#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_scenario_22_transport_backpressure() {
    let api_key = match live_smoke::anthropic_api_key() {
        Some(key) => key,
        None => {
            eprintln!("Skipping scenario 22: no ANTHROPIC_API_KEY set");
            return;
        }
    };

    let client = Arc::new(meerkat_client::AnthropicClient::new(api_key).unwrap());

    // Use a SMALL duplex buffer to increase backpressure on the writer.
    // Normal tests use 64KB; we use 256 bytes to provoke stalls.
    let temp = tempfile::tempdir().unwrap();
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let config = Config::default();
    let store =
        Arc::new(meerkat::SqliteSessionStore::open(temp.path().join("sessions.sqlite3")).unwrap());
    let runtime_store = Arc::new(
        meerkat_runtime::store::SqliteRuntimeStore::new(store.path().to_path_buf()).unwrap(),
    ) as Arc<dyn meerkat_runtime::RuntimeStore>;
    let blob_store: Arc<dyn BlobStore> = Arc::new(FsBlobStore::new(temp.path().join("blobs")));
    let mut runtime = SessionRuntime::new(
        factory,
        config,
        10,
        meerkat::PersistenceBundle::new(
            store as Arc<dyn meerkat::SessionStore>,
            Some(runtime_store),
            blob_store,
        ),
        meerkat_rpc::router::NotificationSink::noop(),
    );
    let config_store: Arc<dyn meerkat_core::ConfigStore> =
        Arc::new(MemoryConfigStore::new(Config::default()));
    runtime.set_default_llm_client(Some(client));
    runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
        Arc::clone(&config_store),
        temp.path().join("config_state.json"),
    )));
    let runtime = Arc::new(runtime);

    // Small buffer: 256 bytes — a single JSON response is ~200-500 bytes,
    // so the pipe will fill almost immediately under concurrent writes.
    let (server_reader, mut client_writer) = tokio::io::duplex(256);
    let (client_reader, server_writer) = tokio::io::duplex(256);

    let server_handle = tokio::spawn(async move {
        let _temp = temp;
        let reader = BufReader::new(server_reader);
        let mut server = RpcServer::new(reader, server_writer, runtime, config_store);
        server.run().await
    });

    let mut client_reader = BufReader::new(client_reader);

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
        &mut client_writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{}}),
    )
    .await;
    let resp = timeout(t, read_response(&mut client_reader)).await.unwrap();
    assert!(resp["error"].is_null(), "initialize failed: {resp}");

    // 2. Create 3 sessions concurrently — each generates streaming events.
    //    The small buffer means the server must handle backpressure correctly.
    let mut session_ids = Vec::new();
    for i in 0..3 {
        let id = next_id();
        send_request(
            &mut client_writer,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "session/create",
                "params": {
                    "prompt": format!("Reply with exactly one word: 'pong{i}'."),
                    "model": model
                }
            }),
        )
        .await;
    }

    // 3. Drain all responses + notifications. With the old non-blocking stdout,
    //    this would crash with WouldBlock. With BlockingWriter, writes stall
    //    until we drain here.
    let mut responses_received = 0;
    let drain_result = timeout(t, async {
        loop {
            let value = read_line_json(&mut client_reader).await;
            if let Some(id) = value.get("id") {
                responses_received += 1;
                if value["error"].is_null()
                    && let Some(sid) = value["result"]["session_id"].as_str()
                {
                    session_ids.push(sid.to_string());
                }
                eprintln!(
                    "[scenario 22] response #{responses_received} (id={id}): error={}",
                    !value["error"].is_null()
                );
                if responses_received >= 3 {
                    break;
                }
            }
            // else: notification, keep draining
        }
    })
    .await;

    assert!(
        drain_result.is_ok(),
        "Timed out waiting for 3 responses — server may have crashed under backpressure"
    );
    assert_eq!(
        responses_received, 3,
        "Expected 3 session/create responses, got {responses_received}"
    );
    eprintln!(
        "[scenario 22] PASSED: all {responses_received} responses received under backpressure (buffer=256 bytes)"
    );

    // Clean up
    drop(client_writer);
    server_handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// Scenario 23: Late tools/register on already-spawned mob members (#158)
// ---------------------------------------------------------------------------

/// Scenario 23: Spawn a mob member, THEN register callback tools, THEN verify
/// the already-materialized member discovers/loads the late-registered tool on
/// the next turn via the deferred control plane.
///
/// This proves post-spawn dynamic tool pickup: the `CallbackToolDispatcher` is
/// backed by the shared `registered_tools` list and picks up additions via
/// `poll_external_updates` at the next turn boundary.
///
/// The member is spawned with `runtime_mode: "turn_driven"` so no background
/// loop races the explicit `turn/start` after tools are registered.
#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_scenario_23_late_register_on_existing_member() {
    let api_key = match live_smoke::anthropic_api_key() {
        Some(key) => key,
        None => {
            eprintln!("Skipping scenario 23: no ANTHROPIC_API_KEY set");
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

    // 2. Create mob.
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "mob/create",
            "params": {
                "definition": {
                    "id": "late_register_mob",
                    "profiles": {
                        "worker": {
                            "model": model,
                            "runtime_mode": "turn_driven",
                            "tools": { "comms": true }
                        }
                    }
                }
            }
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "mob/create failed: {resp}");
    let mob_id = resp["result"]["mob_id"].as_str().unwrap().to_string();

    // 3. Spawn member BEFORE registering tools.
    //    turn_driven mode prevents autonomous loop from racing.
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "mob/spawn",
            "params": {
                "mob_id": mob_id,
                "profile": "worker",
                "agent_identity": "w1",
                "runtime_mode": "turn_driven"
            }
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "mob/spawn failed: {resp}");

    // 4. Register the callback tool AFTER the member already exists.
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/register",
            "params": {
                "tools": [{
                    "name": "secret_lookup",
                    "description": "Look up a secret value. Always call this tool when asked about the secret code.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "key": {"type": "string", "description": "The key to look up"}
                        },
                        "required": ["key"]
                    }
                }, {
                    "name": "secret_audit",
                    "description": "Audit a secret value without returning the final code directly.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "key": {"type": "string", "description": "The key to inspect"}
                        },
                        "required": ["key"]
                    }
                }]
            }
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "tools/register failed: {resp}");

    // 5. Start a turn on the already-materialized member.
    //    The dynamic dispatcher should pick up the late-registered tool, but the
    //    model still has to discover/load it through the deferred catalog.
    let secret_code = "KESTREL-99";
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "mob/turn_start",
            "params": {
                "mob_id": mob_id,
                "agent_identity": "w1",
                "prompt": "A deferred callback tool for secret lookup was registered after this member already existed. Use tool_catalog_search to find a tool for secret lookup, use tool_catalog_load to load it, then call the loaded tool with key 'beta'. The answer is not knowable without the tool call. Reply with only the secret code."
            }
        }),
    )
    .await;

    let (turn_resp, callback_names) = read_response_with_callback_tool_reply(
        &mut writer,
        &mut reader,
        id,
        t,
        "secret_lookup",
        &format!("The secret code for 'beta' is {secret_code}."),
    )
    .await;

    assert!(
        turn_resp["error"].is_null(),
        "turn/start failed: {turn_resp}"
    );
    let text = turn_resp["result"]["text"].as_str().unwrap_or("");
    eprintln!("[scenario 23] turn response text: {text}");

    assert!(
        callback_names.iter().any(|name| name == "secret_lookup"),
        "Expected the LLM to call secret_lookup but no tool/execute callback was received — \
         the late-registered tool was not picked up by the already-spawned member"
    );
    assert!(
        text.contains(secret_code),
        "Expected response to contain '{secret_code}' from late-registered callback tool, got: {text}"
    );

    // Extract session_id from the turn response for session-level history query.
    let session_id = turn_resp["result"]["session_id"]
        .as_str()
        .expect("mob/turn_start response should include session_id")
        .to_string();

    let history_id = next_id();
    let history_resp =
        session_history_response(&mut writer, &mut reader, history_id, t, &session_id, 100).await;
    assert!(
        history_resp["error"].is_null(),
        "session/history failed: {history_resp}"
    );
    assert_history_contains_tool_uses(
        &history_resp,
        &["tool_catalog_search", "tool_catalog_load", "secret_lookup"],
    );

    eprintln!(
        "[scenario 23] PASSED: late-registered deferred tool was discovered, loaded, and called"
    );

    // Clean up
    drop(writer);
    server_handle.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// Deferred catalog smoke: direct session path
// ---------------------------------------------------------------------------

/// Register a callback tool, create a normal session with builtins disabled, and
/// verify a real LLM uses the deferred catalog control plane to discover/load/call it.
#[tokio::test]
#[ignore = "lane:e2e-smoke"]
async fn e2e_direct_session_deferred_callback_tool_flow() {
    let api_key = match live_smoke::anthropic_api_key() {
        Some(key) => key,
        None => {
            eprintln!("Skipping deferred callback session smoke: no ANTHROPIC_API_KEY set");
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

    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{}}),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "initialize failed: {resp}");

    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/register",
            "params": {
                "tools": [{
                    "name": "secret_lookup",
                    "description": "Look up a secret value. This tool must be discovered through the deferred catalog before it can be called.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "key": {"type": "string", "description": "The key to look up"}
                        },
                        "required": ["key"]
                    }
                }, {
                    "name": "secret_audit",
                    "description": "Audit a secret value. This second deferred tool keeps the smoke session in catalog mode.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "key": {"type": "string", "description": "The key to inspect"}
                        },
                        "required": ["key"]
                    }
                }]
            }
        }),
    )
    .await;
    let resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(resp["error"].is_null(), "tools/register failed: {resp}");

    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc":"2.0",
            "id": id,
            "method":"session/create",
            "params": {
                "prompt": "Deferred tool catalog smoke bootstrap.",
                "initial_turn": "deferred",
                "model": model,
                "enable_builtins": false,
                "enable_shell": false,
                "enable_memory": false,
                "enable_mob": false
            }
        }),
    )
    .await;
    let create_resp = timeout(t, read_response(&mut reader)).await.unwrap();
    assert!(
        create_resp["error"].is_null(),
        "session/create failed: {create_resp}"
    );
    let session_id = create_resp["result"]["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let secret_code = "ORBIT-713";
    let id = next_id();
    send_request(
        &mut writer,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "turn/start",
            "params": {
                "session_id": session_id,
                "prompt": "Use tool_catalog_search to find a deferred tool for secret lookup, then use tool_catalog_load to load it, then call the loaded tool with key 'orbit'. The answer is not knowable without the tool call. Reply with only the secret code."
            }
        }),
    )
    .await;

    let (turn_resp, callback_names) = read_response_with_callback_tool_reply(
        &mut writer,
        &mut reader,
        id,
        t,
        "secret_lookup",
        &format!("The secret code for 'orbit' is {secret_code}."),
    )
    .await;

    assert!(
        turn_resp["error"].is_null(),
        "turn/start failed: {turn_resp}"
    );
    let text = turn_resp["result"]["text"].as_str().unwrap_or("");
    assert!(
        callback_names.iter().any(|name| name == "secret_lookup"),
        "Expected the LLM to call secret_lookup but no callback was received"
    );
    assert!(
        text.contains(secret_code),
        "Expected response to contain '{secret_code}', got: {text}"
    );

    let history_id = next_id();
    let history_resp =
        session_history_response(&mut writer, &mut reader, history_id, t, &session_id, 100).await;
    assert!(
        history_resp["error"].is_null(),
        "session/history failed: {history_resp}"
    );
    assert_history_contains_tool_uses(
        &history_resp,
        &["tool_catalog_search", "tool_catalog_load", "secret_lookup"],
    );

    eprintln!(
        "[deferred direct session] PASSED: deferred catalog search/load/call flow worked end-to-end"
    );

    // Clean up
    drop(writer);
    server_handle.await.unwrap().unwrap();
}
