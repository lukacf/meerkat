#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../test-fixtures/live_smoke/support.rs"]
mod live_smoke;

use meerkat_client::AnthropicClient;
use meerkat_client::LlmClient;
use meerkat_core::{ContextConfig, McpServerConfig, RealmConfig, RealmSelection, RuntimeBootstrap};
use meerkat_mcp::McpConnection;
use meerkat_mcp_server::{MeerkatMcpState, handle_tools_call, tools_list};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const MCP_REALM_ID: &str = "mcp-live-smoke";

fn live_client() -> Option<Arc<dyn LlmClient>> {
    let api_key = live_smoke::anthropic_api_key()?;
    Some(Arc::new(
        AnthropicClient::new(api_key).expect("Anthropic client should initialize"),
    ))
}

fn mcp_bootstrap(root: &Path, instance_id: &str) -> RuntimeBootstrap {
    let project_root = root.join("project");
    std::fs::create_dir_all(project_root.join(".rkat")).expect("project root should initialize");
    RuntimeBootstrap {
        realm: RealmConfig {
            selection: RealmSelection::Explicit {
                realm_id: MCP_REALM_ID.to_string(),
            },
            instance_id: Some(instance_id.to_string()),
            backend_hint: None,
            state_root: Some(root.join("realms")),
        },
        context: ContextConfig {
            context_root: Some(project_root),
            user_config_root: None,
        },
    }
}

fn stdio_server_config(root: &Path, instance_id: &str) -> McpServerConfig {
    let mut env = HashMap::new();
    env.insert(
        "ANTHROPIC_API_KEY".to_string(),
        live_smoke::anthropic_api_key().expect("anthropic key should be present"),
    );
    env.insert("SMOKE_MODEL".to_string(), live_smoke::smoke_model());
    McpServerConfig::stdio(
        "rkat-mcp-live",
        live_smoke::cargo_bin("rkat-mcp").display().to_string(),
        vec![
            "--realm".to_string(),
            MCP_REALM_ID.to_string(),
            "--instance".to_string(),
            instance_id.to_string(),
            "--state-root".to_string(),
            root.join("realms").display().to_string(),
            "--context-root".to_string(),
            root.join("project").display().to_string(),
            "--expose-paths".to_string(),
        ],
        env,
    )
}

fn parse_tool_payload(blocks: &[meerkat_core::types::ContentBlock]) -> Value {
    let text = meerkat_core::types::text_content(blocks);
    serde_json::from_str(&text).expect("tool payload should be valid JSON")
}

fn _parse_tool_payload_str(raw: &str) -> Value {
    serde_json::from_str(raw).expect("tool payload should be valid JSON")
}

fn normalize_http_tool_result(result: &Value) -> Value {
    result["content"][0]["text"]
        .as_str()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_else(|| result.clone())
}

fn run_text(payload: &Value) -> String {
    payload["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

struct StreamableHttpHost {
    addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join: JoinHandle<()>,
}

impl StreamableHttpHost {
    async fn spawn(root: &Path, instance_id: &str) -> Self {
        let state = Arc::new(
            MeerkatMcpState::new_with_bootstrap_and_options(mcp_bootstrap(root, instance_id), true)
                .await
                .expect("mcp state should initialize"),
        );
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener addr");
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let join = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { break; };
                        let state = Arc::clone(&state);
                        tokio::spawn(async move {
                            let _ = serve_http_connection(socket, state).await;
                        });
                    }
                }
            }
        });
        Self {
            addr,
            shutdown_tx: Some(shutdown_tx),
            join,
        }
    }

    async fn initialize_handshake(&self) -> Value {
        let init = self
            .json_rpc_request(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {}
            }))
            .await;
        self.json_rpc_notification(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }))
        .await;
        init
    }

    async fn json_rpc_request(&self, payload: Value) -> Value {
        let response = self.http_post(payload).await;
        assert_eq!(response.0, 200, "json-rpc request should return 200");
        serde_json::from_slice(&response.1).expect("json-rpc response should be json")
    }

    async fn json_rpc_notification(&self, payload: Value) {
        let response = self.http_post(payload).await;
        assert_eq!(response.0, 202, "json-rpc notification should return 202");
    }

    async fn http_post(&self, payload: Value) -> (u16, Vec<u8>) {
        let body = serde_json::to_vec(&payload).expect("payload should serialize");
        let mut socket = TcpStream::connect(self.addr)
            .await
            .expect("http client should connect");
        let request = format!(
            "POST /mcp HTTP/1.1\r\nhost: {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            self.addr,
            body.len()
        );
        socket
            .write_all(request.as_bytes())
            .await
            .expect("request head should write");
        socket
            .write_all(&body)
            .await
            .expect("request body should write");
        socket.flush().await.expect("request should flush");
        read_http_response(&mut socket)
            .await
            .expect("response should parse")
    }

    async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        let _ = self.join.await;
    }
}

async fn serve_http_connection(
    mut socket: TcpStream,
    state: Arc<MeerkatMcpState>,
) -> io::Result<()> {
    let Some((method, request)) = read_http_request(&mut socket).await? else {
        return Ok(());
    };

    match method.as_str() {
        "POST" => {
            if request.get("id").is_none() {
                write_http_response(&mut socket, "202 Accepted", &[], None).await?;
                return Ok(());
            }
            let response = handle_http_jsonrpc(&state, &request).await;
            let body = serde_json::to_vec(&response).expect("response should serialize");
            write_http_response(
                &mut socket,
                "200 OK",
                &[("content-type", "application/json")],
                Some(&body),
            )
            .await?;
        }
        "DELETE" => {
            write_http_response(&mut socket, "204 No Content", &[], None).await?;
        }
        "GET" => {
            write_http_response(&mut socket, "405 Method Not Allowed", &[], None).await?;
        }
        _ => {
            write_http_response(&mut socket, "404 Not Found", &[], None).await?;
        }
    }

    Ok(())
}

async fn handle_http_jsonrpc(state: &MeerkatMcpState, request: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");

    match method {
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "rkat-mcp-http-test",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }),
        "ping" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {}
        }),
        "tools/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "tools": tools_list() }
        }),
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match handle_tools_call(state, name, &arguments).await {
                Ok(result) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result
                }),
                Err(err) => {
                    let mut error = json!({
                        "code": err.code,
                        "message": err.message
                    });
                    if let Some(data) = err.data {
                        error["data"] = data;
                    }
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": error
                    })
                }
            }
        }
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": format!("Method not found: {method}")
            }
        }),
    }
}

async fn read_http_request(socket: &mut TcpStream) -> io::Result<Option<(String, Value)>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            if buffer.is_empty() {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed mid-request",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(pos) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos;
        }
    };

    let header_bytes = &buffer[..header_end];
    let header_text = String::from_utf8(header_bytes.to_vec())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "headers must be utf8"))?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing request line"))?;
    let method = request_line
        .split_whitespace()
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid request line"))?
        .to_string();
    let content_length = lines
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);

    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed while reading body",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }

    let body = if content_length == 0 {
        json!({})
    } else {
        serde_json::from_slice(&buffer[body_start..body_start + content_length]).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("request body must be json: {err}"),
            )
        })?
    };

    Ok(Some((method, body)))
}

async fn write_http_response(
    socket: &mut TcpStream,
    status: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> io::Result<()> {
    let body = body.unwrap_or(&[]);
    let mut response = format!(
        "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n",
        body.len()
    )
    .into_bytes();
    for (name, value) in headers {
        response.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    response.extend_from_slice(b"\r\n");
    response.extend_from_slice(body);
    socket.write_all(&response).await?;
    socket.shutdown().await?;
    Ok(())
}

async fn read_http_response(socket: &mut TcpStream) -> io::Result<(u16, Vec<u8>)> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before response headers",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(pos) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos;
        }
    };
    let header_text = String::from_utf8(buffer[..header_end].to_vec())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "headers must be utf8"))?;
    let mut lines = header_text.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing status line"))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid status line"))?
        .parse::<u16>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid status code"))?;
    let content_length = lines
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed while reading response body",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    Ok((
        status,
        buffer[body_start..body_start + content_length].to_vec(),
    ))
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_scenario_31_mcp_stdio_run_resume_lifecycle() {
    let Some(_client) = live_client() else {
        eprintln!("Skipping scenario 31: missing ANTHROPIC_API_KEY");
        return;
    };

    let root = live_smoke::LiveSmokeDir::new("mcp-s31");
    let config = stdio_server_config(root.path(), "mcp-s31");
    let connection = McpConnection::connect(&config)
        .await
        .expect("stdio mcp connection should initialize");

    let tools = connection
        .list_tools()
        .await
        .expect("tools/list should work");
    assert!(tools.iter().any(|tool| tool.name == "meerkat_run"));
    assert!(tools.iter().any(|tool| tool.name == "meerkat_resume"));

    let created = parse_tool_payload(
        &connection
            .call_tool(
                "meerkat_run",
                &json!({
                    "prompt": "My codename is McpStdio31 and my color is cyan. Reply briefly.",
                    "model": live_smoke::smoke_model()
                }),
            )
            .await
            .expect("meerkat_run should succeed"),
    );
    let session_id = created["session_id"]
        .as_str()
        .expect("session id")
        .to_string();
    assert!(!run_text(&created).is_empty());

    let resumed = parse_tool_payload(
        &connection
            .call_tool(
                "meerkat_resume",
                &json!({
                    "session_id": session_id,
                    "prompt": "What are my codename and color? Reply in one sentence."
                }),
            )
            .await
            .expect("meerkat_resume should succeed"),
    );
    let resumed_text = run_text(&resumed).to_lowercase();
    assert!(
        (resumed_text.contains("mcpstdio31") || resumed_text.contains("mcp stdio 31"))
            && resumed_text.contains("cyan"),
        "stdio resume should preserve session context, got: {resumed_text}"
    );

    let listed = parse_tool_payload(
        &connection
            .call_tool("meerkat_sessions", &json!({}))
            .await
            .expect("meerkat_sessions should succeed"),
    );
    assert!(
        listed["sessions"]
            .as_array()
            .is_some_and(|sessions| sessions
                .iter()
                .any(|entry| entry["session_id"].as_str() == Some(session_id.as_str()))),
        "session list should include the created stdio session"
    );

    let archived = parse_tool_payload(
        &connection
            .call_tool("meerkat_archive", &json!({ "session_id": session_id }))
            .await
            .expect("archive should succeed"),
    );
    assert_eq!(archived["archived"], true);

    connection
        .close()
        .await
        .expect("stdio connection should close");
}

#[tokio::test]
#[ignore = "integration-real: diagnostics endpoints"]
async fn e2e_scenario_32_mcp_stdio_config_capabilities_and_skills() {
    let Some(_client) = live_client() else {
        eprintln!("Skipping scenario 32: missing ANTHROPIC_API_KEY");
        return;
    };

    let root = live_smoke::LiveSmokeDir::new("mcp-s32");
    let config = stdio_server_config(root.path(), "mcp-s32");
    let connection = McpConnection::connect(&config)
        .await
        .expect("stdio mcp connection should initialize");

    let config_before = parse_tool_payload(
        &connection
            .call_tool("meerkat_config", &json!({ "action": "get" }))
            .await
            .expect("config get should succeed"),
    );
    let generation = config_before["generation"]
        .as_u64()
        .expect("config generation should be present");
    assert!(config_before["resolved_paths"].is_object());

    let config_after = parse_tool_payload(
        &connection
            .call_tool(
                "meerkat_config",
                &json!({
                    "action": "patch",
                    "patch": { "max_tokens": 448 },
                    "expected_generation": generation
                }),
            )
            .await
            .expect("config patch should succeed"),
    );
    assert_eq!(config_after["config"]["max_tokens"], 448);

    let capabilities = parse_tool_payload(
        &connection
            .call_tool("meerkat_capabilities", &json!({}))
            .await
            .expect("capabilities should succeed"),
    );
    assert!(capabilities["capabilities"].is_array());

    let skills = parse_tool_payload(
        &connection
            .call_tool("meerkat_skills", &json!({ "action": "list" }))
            .await
            .expect("skills list should succeed"),
    );
    assert!(skills["skills"].is_array());

    connection
        .close()
        .await
        .expect("stdio connection should close");
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_scenario_33_mcp_stdio_event_stream_read_roundtrip() {
    let Some(_client) = live_client() else {
        eprintln!("Skipping scenario 33: missing ANTHROPIC_API_KEY");
        return;
    };

    let root = live_smoke::LiveSmokeDir::new("mcp-s33");
    let config = stdio_server_config(root.path(), "mcp-s33");
    let connection = McpConnection::connect(&config)
        .await
        .expect("stdio mcp connection should initialize");

    let created = parse_tool_payload(
        &connection
            .call_tool(
                "meerkat_run",
                &json!({
                    "prompt": "Say hello as EventMcp33.",
                    "model": live_smoke::smoke_model()
                }),
            )
            .await
            .expect("meerkat_run should succeed"),
    );
    let session_id = created["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let stream = parse_tool_payload(
        &connection
            .call_tool(
                "meerkat_event_stream_open",
                &json!({ "session_id": session_id }),
            )
            .await
            .expect("event stream open should succeed"),
    );
    let stream_id = stream["stream_id"].as_str().expect("stream id").to_string();

    let resumed = parse_tool_payload(
        &connection
            .call_tool(
                "meerkat_resume",
                &json!({
                    "session_id": session_id,
                    "prompt": "Reply with EventMcp33 acknowledged."
                }),
            )
            .await
            .expect("resume should succeed"),
    );
    assert!(run_text(&resumed).to_lowercase().contains("eventmcp33"));

    let mut saw_event = false;
    for _ in 0..12 {
        let event = parse_tool_payload(
            &connection
                .call_tool(
                    "meerkat_event_stream_read",
                    &json!({
                        "stream_id": stream_id,
                        "timeout_ms": 2_000
                    }),
                )
                .await
                .expect("event stream read should succeed"),
        );
        if event["status"] == "event" {
            saw_event = true;
            break;
        }
    }
    assert!(
        saw_event,
        "stdio event stream should emit at least one event"
    );

    let closed = parse_tool_payload(
        &connection
            .call_tool(
                "meerkat_event_stream_close",
                &json!({ "stream_id": stream_id }),
            )
            .await
            .expect("event stream close should succeed"),
    );
    assert_eq!(closed["closed"], true);

    connection
        .close()
        .await
        .expect("stdio connection should close");
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_scenario_34_mcp_streamable_http_run_resume_lifecycle() {
    let Some(_client) = live_client() else {
        eprintln!("Skipping scenario 34: missing ANTHROPIC_API_KEY");
        return;
    };

    let root = live_smoke::LiveSmokeDir::new("mcp-s34");
    let host = StreamableHttpHost::spawn(root.path(), "mcp-s34").await;
    let initialized = host.initialize_handshake().await;
    assert_eq!(
        initialized["result"]["serverInfo"]["name"],
        "rkat-mcp-http-test"
    );

    let tools = host
        .json_rpc_request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .await;
    assert!(tools["result"]["tools"].is_array());

    let created_response = host
        .json_rpc_request(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "meerkat_run",
                "arguments": {
                    "prompt": "My codename is HttpMcp34 and my number is 34. Reply briefly.",
                    "model": live_smoke::smoke_model()
                }
            }
        }))
        .await;
    let created = normalize_http_tool_result(&created_response["result"]);
    let session_id = created["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let resumed_response = host
        .json_rpc_request(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "meerkat_resume",
                "arguments": {
                    "session_id": session_id,
                    "prompt": "What are my codename and number? Reply in one sentence."
                }
            }
        }))
        .await;
    let resumed = normalize_http_tool_result(&resumed_response["result"]);
    let resumed_text = run_text(&resumed).to_lowercase();
    assert!(
        (resumed_text.contains("httpmcp34") || resumed_text.contains("http mcp 34"))
            && resumed_text.contains("34"),
        "streamable-http resume should preserve context, got: {resumed_text}"
    );

    host.shutdown().await;
}

#[tokio::test]
#[ignore = "integration-real: diagnostics endpoints"]
async fn e2e_scenario_35_mcp_streamable_http_config_capabilities_and_skills() {
    let Some(_client) = live_client() else {
        eprintln!("Skipping scenario 35: missing ANTHROPIC_API_KEY");
        return;
    };

    let root = live_smoke::LiveSmokeDir::new("mcp-s35");
    let host = StreamableHttpHost::spawn(root.path(), "mcp-s35").await;
    let _ = host.initialize_handshake().await;

    let config_before_response = host
        .json_rpc_request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "meerkat_config",
                "arguments": { "action": "get" }
            }
        }))
        .await;
    let config_before = normalize_http_tool_result(&config_before_response["result"]);
    let generation = config_before["generation"]
        .as_u64()
        .expect("config generation should be present");

    let config_after_response = host
        .json_rpc_request(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "meerkat_config",
                "arguments": {
                    "action": "patch",
                    "patch": { "max_tokens": 512 },
                    "expected_generation": generation
                }
            }
        }))
        .await;
    let config_after = normalize_http_tool_result(&config_after_response["result"]);
    assert_eq!(config_after["config"]["max_tokens"], 512);
    assert!(config_after["resolved_paths"].is_object());

    let capabilities_response = host
        .json_rpc_request(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "meerkat_capabilities",
                "arguments": {}
            }
        }))
        .await;
    let capabilities = normalize_http_tool_result(&capabilities_response["result"]);
    assert!(capabilities["capabilities"].is_array());

    let skills_response = host
        .json_rpc_request(json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "meerkat_skills",
                "arguments": { "action": "list" }
            }
        }))
        .await;
    let skills = normalize_http_tool_result(&skills_response["result"]);
    assert!(skills["skills"].is_array());

    host.shutdown().await;
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_scenario_36_mcp_streamable_http_event_stream_and_archive_roundtrip() {
    let Some(_client) = live_client() else {
        eprintln!("Skipping scenario 36: missing ANTHROPIC_API_KEY");
        return;
    };

    let root = live_smoke::LiveSmokeDir::new("mcp-s36");
    let host = StreamableHttpHost::spawn(root.path(), "mcp-s36").await;
    let _ = host.initialize_handshake().await;

    let created_response = host
        .json_rpc_request(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "meerkat_run",
                "arguments": {
                    "prompt": "Say hello as HttpEvent36.",
                    "model": live_smoke::smoke_model()
                }
            }
        }))
        .await;
    let created = normalize_http_tool_result(&created_response["result"]);
    let session_id = created["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    let stream_response = host
        .json_rpc_request(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "meerkat_event_stream_open",
                "arguments": { "session_id": session_id }
            }
        }))
        .await;
    let stream = normalize_http_tool_result(&stream_response["result"]);
    let stream_id = stream["stream_id"].as_str().expect("stream id").to_string();

    let _ = host
        .json_rpc_request(json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "meerkat_resume",
                "arguments": {
                    "session_id": session_id,
                    "prompt": "Reply with HttpEvent36 acknowledged."
                }
            }
        }))
        .await;

    let mut saw_event = false;
    for _ in 0..12 {
        let event_response = host
            .json_rpc_request(json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": "meerkat_event_stream_read",
                    "arguments": {
                        "stream_id": stream_id,
                        "timeout_ms": 2_000
                    }
                }
            }))
            .await;
        let event = normalize_http_tool_result(&event_response["result"]);
        if event["status"] == "event" {
            saw_event = true;
            break;
        }
    }
    assert!(
        saw_event,
        "http event stream should emit at least one event"
    );

    let archived_response = host
        .json_rpc_request(json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tools/call",
            "params": {
                "name": "meerkat_archive",
                "arguments": { "session_id": session_id }
            }
        }))
        .await;
    let archived = normalize_http_tool_result(&archived_response["result"]);
    assert_eq!(archived["archived"], true);

    let closed_response = host
        .json_rpc_request(json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {
                "name": "meerkat_event_stream_close",
                "arguments": { "stream_id": stream_id }
            }
        }))
        .await;
    let closed = normalize_http_tool_result(&closed_response["result"]);
    assert_eq!(closed["closed"], true);

    host.shutdown().await;
}
