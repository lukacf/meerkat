//! End-to-end tests for rkat-rpc --tcp.
//!
//! These spawn the real `rkat-rpc` binary with `--tcp`, connect over TCP,
//! and exercise the JSON-RPC protocol. No LLM API key needed — the tests
//! only exercise handshake, config, and session create (deferred).
//!
//! Lane: e2e-system (deterministic, local resources only).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use futures::{SinkExt, StreamExt};
use meerkat_contracts::{RealtimeChannelOpenFrame, RealtimeClientFrame, RealtimeServerFrame};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Spawn an RPC binary with --tcp on a random port, return (child, port).
async fn spawn_rpc_tcp_with_bin(bin: &str) -> (tokio::process::Child, u16) {
    // Bind a listener to discover a free port, then release it.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let child = tokio::process::Command::new(bin)
        .args(["--isolated", "--tcp", &format!("127.0.0.1:{port}")])
        .env("RKAT_TEST_CLIENT", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap_or_else(|_| panic!("failed to spawn {bin}"));

    // Wait for the TCP port to become reachable.
    let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .is_ok()
        {
            return (child, port);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{bin} --tcp did not become reachable on port {port}");
}

async fn spawn_rpc_tcp() -> (tokio::process::Child, u16) {
    spawn_rpc_tcp_with_bin(env!("CARGO_BIN_EXE_rkat-rpc")).await
}

/// Spawn an RPC binary with --tcp and --realtime-ws on random ports.
async fn spawn_rpc_tcp_with_realtime_ws() -> (tokio::process::Child, u16, u16) {
    spawn_rpc_tcp_with_realtime_ws_env(&[]).await
}

async fn spawn_rpc_tcp_with_realtime_ws_env(
    extra_env: &[(&str, &str)],
) -> (tokio::process::Child, u16, u16) {
    let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tcp_port = tcp_listener.local_addr().unwrap().port();
    drop(tcp_listener);

    let ws_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_port = ws_listener.local_addr().unwrap().port();
    drop(ws_listener);

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_rkat-rpc"));
    command
        .args([
            "--isolated",
            "--tcp",
            &format!("127.0.0.1:{tcp_port}"),
            "--realtime-ws",
            &format!("127.0.0.1:{ws_port}"),
        ])
        .env("RKAT_TEST_CLIENT", "1")
        // Co-existence hermeticity:
        // these tests drive the realtime WS host with the deterministic
        // `RKAT_TEST_CLIENT` client only. Inherited `OPENAI_*` credentials
        // from the developer's shell would otherwise flip the host into the
        // OpenAI-backed session-factory path, which requires a materialized
        // session and is covered by `realtime_ws_protocol.rs` instead.
        // Explicit `extra_env` overrides (e.g. `test-openai-key`) still win
        // because they're applied after this scrub.
        .env_remove("OPENAI_API_KEY")
        .env_remove("RKAT_OPENAI_API_KEY")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let child = command
        .spawn()
        .unwrap_or_else(|_| panic!("failed to spawn rkat-rpc with realtime ws"));

    let tcp_deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    while tokio::time::Instant::now() < tcp_deadline {
        if TcpStream::connect(format!("127.0.0.1:{tcp_port}"))
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let ws_deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    while tokio::time::Instant::now() < ws_deadline {
        if connect_async(format!("ws://127.0.0.1:{ws_port}/realtime/ws"))
            .await
            .is_ok()
        {
            return (child, tcp_port, ws_port);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!("rkat-rpc --realtime-ws did not become reachable on port {ws_port}");
}

async fn send_jsonl(stream: &mut TcpStream, value: &Value) {
    let mut line = serde_json::to_string(value).unwrap();
    line.push('\n');
    stream.write_all(line.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
}

async fn read_jsonl(reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> Value {
    let mut line = String::new();
    timeout(READ_TIMEOUT, reader.read_line(&mut line))
        .await
        .expect("read timed out")
        .expect("read error");
    serde_json::from_str(line.trim()).expect("invalid JSON")
}

async fn read_response_for_id(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    expected_id: u64,
) -> Value {
    loop {
        let value = read_jsonl(reader).await;
        if value["id"].as_u64() == Some(expected_id) {
            return value;
        }
    }
}

#[tokio::test]
async fn tcp_e2e_initialize_and_config_roundtrip() {
    let (mut child, port) = spawn_rpc_tcp().await;
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .unwrap();

    // Initialize handshake
    send_jsonl(
        &mut stream,
        &json!({"jsonrpc":"2.0","method":"initialize","params":{},"id":1}),
    )
    .await;

    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);

    let resp = read_jsonl(&mut reader).await;
    assert_eq!(resp["id"], 1);
    assert!(resp["result"]["server_info"]["name"].is_string());
    let methods = resp["result"]["methods"].as_array().unwrap();
    assert!(methods.iter().any(|m| m == "session/create"));
    assert!(methods.iter().any(|m| m == "config/get"));
    assert!(methods.iter().any(|m| m == "turn/start"));

    // Config round-trip
    let mut stream2 = write.reunite(reader.into_inner()).unwrap();
    send_jsonl(
        &mut stream2,
        &json!({"jsonrpc":"2.0","method":"config/get","params":{},"id":2}),
    )
    .await;

    let (read, _write) = stream2.into_split();
    let mut reader2 = BufReader::new(read);
    let config_resp = read_jsonl(&mut reader2).await;
    assert_eq!(config_resp["id"], 2);
    assert!(config_resp["result"]["config"].is_object());

    child.kill().await.ok();
}

#[tokio::test]
async fn tcp_e2e_session_create_deferred() {
    let (mut child, port) = spawn_rpc_tcp().await;
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .unwrap();

    // Initialize
    send_jsonl(
        &mut stream,
        &json!({"jsonrpc":"2.0","method":"initialize","params":{},"id":1}),
    )
    .await;
    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let _init = read_jsonl(&mut reader).await;

    // Create deferred session (no LLM call needed)
    let mut stream2 = write.reunite(reader.into_inner()).unwrap();
    send_jsonl(
        &mut stream2,
        &json!({
            "jsonrpc": "2.0",
            "method": "session/create",
            "params": {
                "model": "gpt-5.4",
                "prompt": "hello",
                "initial_turn": "deferred"
            },
            "id": 2
        }),
    )
    .await;
    let (read, _write) = stream2.into_split();
    let mut reader2 = BufReader::new(read);
    let create_resp = read_response_for_id(&mut reader2, 2).await;
    assert_eq!(create_resp["id"], 2);
    let session_id = create_resp["result"]["session_id"].as_str().unwrap();
    assert!(!session_id.is_empty());

    // Session list should show the created session
    let mut stream3 = _write.reunite(reader2.into_inner()).unwrap();
    send_jsonl(
        &mut stream3,
        &json!({"jsonrpc":"2.0","method":"session/list","params":{},"id":3}),
    )
    .await;
    let (read, _write) = stream3.into_split();
    let mut reader3 = BufReader::new(read);
    let list_resp = read_jsonl(&mut reader3).await;
    assert_eq!(list_resp["id"], 3);
    let sessions = list_resp["result"]["sessions"].as_array().unwrap();
    assert!(
        sessions.iter().any(|s| s["session_id"] == session_id),
        "created session should appear in list"
    );

    child.kill().await.ok();
}

#[tokio::test]
async fn tcp_e2e_capabilities_get() {
    let (mut child, port) = spawn_rpc_tcp().await;
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .unwrap();

    send_jsonl(
        &mut stream,
        &json!({"jsonrpc":"2.0","method":"initialize","params":{},"id":1}),
    )
    .await;
    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let _init = read_jsonl(&mut reader).await;

    let mut stream2 = write.reunite(reader.into_inner()).unwrap();
    send_jsonl(
        &mut stream2,
        &json!({"jsonrpc":"2.0","method":"capabilities/get","params":{},"id":2}),
    )
    .await;
    let (read, _write) = stream2.into_split();
    let mut reader2 = BufReader::new(read);
    let resp = read_jsonl(&mut reader2).await;
    assert_eq!(resp["id"], 2);
    assert!(resp["result"]["capabilities"].is_array());

    child.kill().await.ok();
}

#[tokio::test]
async fn tcp_e2e_realtime_ws_host_coexists_with_tcp_rpc() {
    let (mut child, tcp_port, ws_port) = spawn_rpc_tcp_with_realtime_ws().await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{tcp_port}"))
        .await
        .unwrap();
    send_jsonl(
        &mut stream,
        &json!({"jsonrpc":"2.0","method":"initialize","params":{},"id":1}),
    )
    .await;

    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let init = read_jsonl(&mut reader).await;
    assert_eq!(init["id"], 1);
    let mut stream = write.reunite(reader.into_inner()).unwrap();

    send_jsonl(
        &mut stream,
        &json!({
            "jsonrpc":"2.0",
            "method":"session/create",
            "params":{"prompt":"realtime open-info e2e","initial_turn":"deferred"},
            "id":2
        }),
    )
    .await;
    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let created = read_response_for_id(&mut reader, 2).await;
    let session_id = created["result"]["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    let mut stream = write.reunite(reader.into_inner()).unwrap();
    send_jsonl(
        &mut stream,
        &json!({
            "jsonrpc":"2.0",
            "method":"realtime/open_info",
            "params":{
                "target":{"type":"session_target","session_id":session_id},
                "role":"primary",
                "turning_mode":"provider_managed"
            },
            "id":3
        }),
    )
    .await;
    let (read, _write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let open_info = read_response_for_id(&mut reader, 3).await;
    assert!(
        open_info["result"]["open_token"].as_str().is_some(),
        "open_info should return a token: {open_info}"
    );
    let protocol_version = open_info["result"]["default_protocol_version"]
        .as_str()
        .expect("default protocol version")
        .to_string();
    let open_token = open_info["result"]["open_token"]
        .as_str()
        .expect("open token")
        .to_string();

    let (mut ws_stream, _response) = connect_async(format!("ws://127.0.0.1:{ws_port}/realtime/ws"))
        .await
        .expect("realtime websocket handshake should succeed");
    ws_stream
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&RealtimeClientFrame::ChannelOpen(
                RealtimeChannelOpenFrame {
                    protocol_version,
                    open_token,
                    role: meerkat_contracts::RealtimeChannelRole::Primary,
                    turning_mode: meerkat_contracts::RealtimeTurningMode::ProviderManaged,
                },
            ))
            .expect("channel.open should serialize")
            .into(),
        ))
        .await
        .expect("channel.open should send");
    let opened_frame = timeout(READ_TIMEOUT, ws_stream.next())
        .await
        .expect("websocket frame should arrive")
        .expect("expected websocket frame")
        .expect("websocket frame should be ok");
    let opened_payload = match opened_frame {
        WsMessage::Text(text) => serde_json::from_str::<RealtimeServerFrame>(&text)
            .expect("opened frame should deserialize"),
        other => panic!("expected websocket text frame, got {other:?}"),
    };
    assert!(
        matches!(opened_payload, RealtimeServerFrame::ChannelOpened(_)),
        "expected channel.opened, got {opened_payload:?}"
    );

    child.kill().await.ok();
}

#[tokio::test]
async fn tcp_e2e_realtime_session_targets_accept_env_default_openai_credentials() {
    let (mut child, tcp_port, _ws_port) =
        spawn_rpc_tcp_with_realtime_ws_env(&[("OPENAI_API_KEY", "test-openai-key")]).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{tcp_port}"))
        .await
        .unwrap();
    send_jsonl(
        &mut stream,
        &json!({"jsonrpc":"2.0","method":"initialize","params":{},"id":1}),
    )
    .await;

    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let init = read_jsonl(&mut reader).await;
    assert_eq!(init["id"], 1);
    let mut stream = write.reunite(reader.into_inner()).unwrap();

    send_jsonl(
        &mut stream,
        &json!({
            "jsonrpc":"2.0",
            "method":"session/create",
            "params":{"prompt":"realtime capabilities e2e","initial_turn":"deferred"},
            "id":2
        }),
    )
    .await;
    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let created = read_response_for_id(&mut reader, 2).await;
    let session_id = created["result"]["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let mut stream = write.reunite(reader.into_inner()).unwrap();
    send_jsonl(
        &mut stream,
        &json!({
            "jsonrpc":"2.0",
            "method":"realtime/capabilities",
            "params":{"target":{"type":"session_target","session_id":session_id}},
            "id":3
        }),
    )
    .await;
    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let capabilities = read_response_for_id(&mut reader, 3).await;
    assert!(
        capabilities["error"].is_null(),
        "realtime capabilities should resolve through env-default OpenAI auth: {capabilities}"
    );
    let turning_modes = capabilities["result"]["capabilities"]["turning_modes"]
        .as_array()
        .unwrap_or_else(|| panic!("turning_modes should be an array: {capabilities}"));
    assert!(
        turning_modes
            .iter()
            .any(|mode| mode.as_str() == Some("explicit_commit")),
        "env-default OpenAI credentials should expose product realtime capabilities: {capabilities}"
    );

    let mut stream = write.reunite(reader.into_inner()).unwrap();
    send_jsonl(
        &mut stream,
        &json!({
            "jsonrpc":"2.0",
            "method":"realtime/open_info",
            "params":{
                "target":{"type":"session_target","session_id":session_id},
                "role":"primary",
                "turning_mode":"explicit_commit"
            },
            "id":4
        }),
    )
    .await;
    let (read, _write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let open_info = read_response_for_id(&mut reader, 4).await;
    assert!(
        open_info["result"]["open_token"].as_str().is_some(),
        "env-default OpenAI credentials should open explicit_commit realtime sessions: {open_info}"
    );

    child.kill().await.ok();
}

#[tokio::test]
#[ignore = "lane:e2e-build"]
async fn tcp_e2e_session_create_and_turn_start_with_test_client() {
    let (mut child, port) = spawn_rpc_tcp().await;
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .unwrap();

    send_jsonl(
        &mut stream,
        &json!({"jsonrpc":"2.0","method":"initialize","params":{},"id":1}),
    )
    .await;
    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let _init = read_jsonl(&mut reader).await;

    let mut stream2 = write.reunite(reader.into_inner()).unwrap();
    send_jsonl(
        &mut stream2,
        &json!({
            "jsonrpc": "2.0",
            "method": "session/create",
            "params": {
                "model": "claude-sonnet-4-5",
                "prompt": "Remember RPC_BINARY_42 and reply with ok."
            },
            "id": 2
        }),
    )
    .await;
    let (read, write) = stream2.into_split();
    let mut reader2 = BufReader::new(read);
    let create_resp = read_response_for_id(&mut reader2, 2).await;
    assert!(
        create_resp["error"].is_null(),
        "session/create failed: {create_resp}"
    );
    let session_id = create_resp["result"]["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let mut stream3 = write.reunite(reader2.into_inner()).unwrap();
    send_jsonl(
        &mut stream3,
        &json!({
            "jsonrpc": "2.0",
            "method": "turn/start",
            "params": {
                "session_id": session_id,
                "prompt": "What token was I asked to remember? Reply with the token only."
            },
            "id": 3
        }),
    )
    .await;
    let (read, _write) = stream3.into_split();
    let mut reader3 = BufReader::new(read);
    let turn_resp = read_response_for_id(&mut reader3, 3).await;
    assert!(
        turn_resp["error"].is_null(),
        "turn/start failed: {turn_resp}"
    );

    child.kill().await.ok();
}

#[tokio::test]
#[ignore = "lane:e2e-build"]
async fn tcp_e2e_rkat_rpc_mini_initialize_and_roundtrip() {
    let (mut child, port) = spawn_rpc_tcp_with_bin(env!("CARGO_BIN_EXE_rkat-rpc-mini")).await;
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .unwrap();

    send_jsonl(
        &mut stream,
        &json!({"jsonrpc":"2.0","method":"initialize","params":{},"id":1}),
    )
    .await;
    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let init = read_jsonl(&mut reader).await;
    let methods = init["result"]["methods"].as_array().unwrap();
    let method_names: Vec<&str> = methods.iter().filter_map(|m| m.as_str()).collect();
    assert!(method_names.contains(&"session/create"));
    assert!(method_names.contains(&"turn/start"));
    assert!(method_names.contains(&"config/get"));
    assert!(!method_names.contains(&"session/status"));
    assert!(!method_names.contains(&"session/submission"));
    assert!(!method_names.contains(&"blob/get"));
    assert!(!method_names.contains(&"session/stream_open"));
    assert!(!method_names.contains(&"skills/list"));
    assert!(!method_names.contains(&"schedule/list"));
    assert!(!method_names.contains(&"mcp/add"));

    let mut stream2 = write.reunite(reader.into_inner()).unwrap();
    send_jsonl(
        &mut stream2,
        &json!({
            "jsonrpc": "2.0",
            "method": "session/create",
            "params": {
                "model": "claude-sonnet-4-5",
                "prompt": "Remember MINI_RPC_42 and reply with ok."
            },
            "id": 2
        }),
    )
    .await;
    let (read, write) = stream2.into_split();
    let mut reader2 = BufReader::new(read);
    let create_resp = read_response_for_id(&mut reader2, 2).await;
    assert!(
        create_resp["error"].is_null(),
        "session/create failed: {create_resp}"
    );
    let session_id = create_resp["result"]["session_id"]
        .as_str()
        .unwrap()
        .to_string();

    let mut stream3 = write.reunite(reader2.into_inner()).unwrap();
    send_jsonl(
        &mut stream3,
        &json!({
            "jsonrpc": "2.0",
            "method": "turn/start",
            "params": {
                "session_id": session_id,
                "prompt": "Repeat the remembered token."
            },
            "id": 3
        }),
    )
    .await;
    let (read, _write) = stream3.into_split();
    let mut reader3 = BufReader::new(read);
    let turn_resp = read_response_for_id(&mut reader3, 3).await;
    assert!(
        turn_resp["error"].is_null(),
        "turn/start failed: {turn_resp}"
    );
    assert!(
        !turn_resp["result"]["text"]
            .as_str()
            .unwrap_or("")
            .trim()
            .is_empty(),
        "turn/start should return assistant text: {turn_resp}"
    );

    child.kill().await.ok();
}
